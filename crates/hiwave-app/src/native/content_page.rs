//! A web page hosted on the engine's content thread (`HIWAVE_CONTENT_THREAD=1`).
//!
//! Without the flag the native shell loads pages with `block_on` on the window
//! thread, so a heavy page freezes the whole window. With it, the page lives on a
//! dedicated thread: the window sends commands and presents the frames the page
//! publishes, and never waits for a load, a script or a fetch.
//!
//! The pieces that decide behaviour (the history, which events are stale, what
//! the address bar shows) are plain functions and a small struct, tested without
//! a window. [`ContentPage`] is the thin part that owns the thread and the surface.

use std::cell::{Cell, RefCell};

use rustkit_engine::{
    ContentThread, EngineBuilder, FrameCommit, LoadState, PageCmd, PageEvent, PageInput, WindowSurface,
};
use rustkit_viewhost::{Bounds, ViewId};
use tokio::sync::oneshot;
use tracing::{debug, info, warn};
use url::Url;
use windows::Win32::Foundation::HWND;

/// The flag. Same variable the macOS app reads.
pub fn enabled() -> bool {
    rustkit_engine::content_thread_enabled()
}

/// Back/forward history for a page that lives on another thread. The page does not
/// report its session history, so the window keeps it, as the macOS app does.
#[derive(Debug, Default)]
pub struct NavHistory {
    entries: Vec<Url>,
    index: usize,
}

impl NavHistory {
    /// A new entry after the current one; anything forward of it is dropped.
    pub fn push(&mut self, url: Url) {
        if !self.entries.is_empty() {
            self.entries.truncate(self.index + 1);
        }
        self.entries.push(url);
        self.index = self.entries.len() - 1;
    }

    /// Change what the current entry points at (a redirect, a page's own `replace`).
    pub fn replace_current(&mut self, url: Url) {
        match self.entries.get_mut(self.index) {
            Some(entry) => *entry = url,
            None => self.push(url),
        }
    }

    pub fn current(&self) -> Option<&Url> {
        self.entries.get(self.index)
    }

    pub fn can_go_back(&self) -> bool {
        self.index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.index + 1 < self.entries.len()
    }

    /// Step back and return where to load.
    pub fn back(&mut self) -> Option<Url> {
        if !self.can_go_back() {
            return None;
        }
        self.index -= 1;
        self.entries.get(self.index).cloned()
    }

    /// Step forward and return where to load.
    pub fn forward(&mut self) -> Option<Url> {
        if !self.can_go_forward() {
            return None;
        }
        self.index += 1;
        self.entries.get(self.index).cloned()
    }
}

/// Whether an event belongs to a navigation older than the one the window asked for
/// last. `nav_id` 0 means the event is not tied to a navigation.
pub fn is_stale(accepted: u64, nav_id: u64) -> bool {
    nav_id != 0 && nav_id < accepted
}

/// Whether the address bar should show `url`. Pages the app loaded from a string
/// (`about:`, `data:`) keep the address the app gave their tab.
pub fn shown_in_address_bar(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "file")
}

/// What the chrome needs to know about the page.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PageState {
    pub url: Option<String>,
    pub title: Option<String>,
    pub loading: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

/// One page on the content thread, with the window surface that shows it.
pub struct ContentPage {
    thread: ContentThread,
    surface: RefCell<WindowSurface>,
    history: RefCell<NavHistory>,
    /// The newest navigation id the window asked for; older events are dropped.
    accepted_nav: Cell<u64>,
    /// The newest frame the page published, and whether it has been drawn.
    latest_commit: RefCell<Option<FrameCommit>>,
    needs_present: Cell<bool>,
    /// Whether the page's window is shown. A hidden tab keeps its newest frame but is
    /// not drawn (no swapchain present for a window nobody sees).
    visible: Cell<bool>,
    url: RefCell<Option<String>>,
    title: RefCell<Option<String>>,
    loading: Cell<bool>,
    /// The page came from a string (the start page), not the network.
    from_string: Cell<bool>,
}

impl ContentPage {
    /// Attach the window surface (here, on the window thread) and start the content
    /// thread (the engine is built on it).
    pub fn spawn(parent: HWND, bounds: Bounds, builder: EngineBuilder) -> Result<Self, String> {
        let surface = WindowSurface::attach(parent, bounds).map_err(|e| format!("window surface: {e}"))?;
        let thread = ContentThread::spawn(builder, bounds).map_err(|e| format!("content thread: {e:?}"))?;
        info!("Content page started on the content thread");
        Ok(Self {
            thread,
            surface: RefCell::new(surface),
            history: RefCell::new(NavHistory::default()),
            accepted_nav: Cell::new(0),
            latest_commit: RefCell::new(None),
            needs_present: Cell::new(false),
            visible: Cell::new(true),
            url: RefCell::new(None),
            title: RefCell::new(None),
            loading: Cell::new(false),
            from_string: Cell::new(false),
        })
    }

    /// The surface's window.
    pub fn hwnd(&self) -> HWND {
        HWND(self.surface.borrow().raw_view() as *mut _)
    }

    /// The viewhost id the surface's window reports its input under.
    pub fn viewhost_id(&self) -> ViewId {
        self.surface.borrow().viewhost_id()
    }

    /// Give the page the keyboard.
    pub fn grab_keyboard(&self) {
        self.surface.borrow().grab_keyboard();
    }

    /// Run a script in the page. The answer arrives on the channel, a turn or more
    /// later; the window never waits for it.
    pub fn eval(&self, script: &str) -> oneshot::Receiver<Result<String, String>> {
        self.thread.eval(script)
    }

    /// Show or hide the page's window, and tell the page (a hidden page need not
    /// draw). A background tab keeps loading either way.
    pub fn set_visible(&self, visible: bool) {
        use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE, SW_SHOWNA};
        // SAFETY: the surface's window is live while `self` is, and this runs on the
        // window thread that created it.
        unsafe {
            let _ = ShowWindow(self.hwnd(), if visible { SW_SHOWNA } else { SW_HIDE });
        }
        self.thread.enqueue(PageCmd::SetVisible { visible });
        self.visible.set(visible);
        if visible {
            // What it last published may not have been drawn while it was hidden.
            self.needs_present.set(true);
        }
    }

    /// Close the page: stop its thread and destroy its window. `WindowSurface` has
    /// no `Drop` that does the second, so without this a closed tab's window would
    /// stay on screen.
    pub fn close(self) {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        let hwnd = self.hwnd();
        drop(self);
        // SAFETY: `hwnd` was created on this (the window) thread and is destroyed once.
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    // ---- input ------------------------------------------------------------------
    //
    // Everything here is fire-and-forget, as on macOS: the page acts on its own
    // thread and publishes what changed. The replies the protocol offers are not
    // read (a link click comes back as `NavigateRequested`, which `pump` follows;
    // a scrolled or typed-into page publishes a new frame).

    pub fn mouse_move(&self, x: f32, y: f32) {
        self.thread.input(PageInput::MouseMove { x, y });
    }

    pub fn mouse_leave(&self) {
        self.thread.input(PageInput::MouseLeave);
    }

    pub fn mouse_down(&self, x: f32, y: f32, clicks: u32, shift: bool) {
        self.thread.input(PageInput::MouseDown { x, y, clicks, shift });
    }

    /// The release: the page fires `mouseup` and `click` and follows a link itself.
    pub fn click(&self, x: f32, y: f32) {
        let (reply, _unread) = oneshot::channel();
        self.thread.input(PageInput::Click { x, y, reply });
    }

    /// A wheel over a point: the box under it may take it. Pixels; positive `dy`
    /// scrolls up (the engine's convention).
    pub fn scroll_at(&self, x: f32, y: f32, dx: f32, dy: f32) {
        self.thread.input(PageInput::ScrollAt { x, y, dx, dy });
    }

    /// A key for the page's focused element (or the page itself, which scrolls an
    /// unhandled one).
    pub fn key(&self, key_code: u32, key: &str, ctrl: bool, shift: bool, alt: bool) {
        let (reply, _unread) = oneshot::channel();
        self.thread.input(PageInput::TextKey { key_code, key: key.to_string(), ctrl, shift, alt, reply });
        // The key edits the page's text field; the page does not lay out and publish a
        // frame for it by itself, so ask (the macOS window does the same after a key).
        self.thread.enqueue(PageCmd::Relayout);
    }

    pub fn key_up(&self, key_code: u32, key: &str, ctrl: bool, shift: bool, alt: bool) {
        self.thread.input(PageInput::KeyUp { key_code, key: key.to_string(), ctrl, shift, alt });
    }

    /// Load a page from a string (the start page).
    pub fn load_html(&self, html: &str) {
        self.from_string.set(true);
        *self.url.borrow_mut() = None;
        // The reply says when it is done; the page's own events carry the rest.
        drop(self.thread.load_html(html));
        self.accepted_nav.set(self.thread.last_nav_id());
    }

    /// Load `url`, adding a history entry.
    pub fn navigate(&self, url: Url) {
        self.history.borrow_mut().push(url.clone());
        self.load(url);
    }

    fn load(&self, url: Url) {
        self.from_string.set(false);
        *self.url.borrow_mut() = shown_in_address_bar(&url).then(|| url.to_string());
        let nav_id = self.thread.navigate(url);
        self.accepted_nav.set(nav_id);
    }

    pub fn go_back(&self) {
        let target = self.history.borrow_mut().back();
        if let Some(url) = target {
            self.load(url);
        }
    }

    pub fn go_forward(&self) {
        let target = self.history.borrow_mut().forward();
        if let Some(url) = target {
            self.load(url);
        }
    }

    pub fn reload(&self) {
        let current = self.history.borrow().current().cloned();
        if let Some(url) = current {
            self.load(url);
        }
    }

    pub fn stop(&self) {
        self.thread.stop();
    }

    /// The window changed size: the surface takes it now, the page lays out when it
    /// receives the same bounds.
    pub fn resize(&self, bounds: Bounds) {
        if let Err(e) = self.surface.borrow_mut().resize(bounds) {
            warn!(error = %e, "Content surface resize failed");
        }
        self.thread.resize(bounds, 1.0);
        // The new surface is blank until the published commit is drawn again.
        self.needs_present.set(true);
    }

    /// Take what the page published since the last turn and draw the newest frame.
    /// Call once per window turn; it never waits on the page.
    pub fn pump(&self) {
        let accepted = self.accepted_nav.get();
        for event in self.thread.poll() {
            match event {
                PageEvent::Commit(commit) => {
                    if !is_stale(accepted, commit.nav_generation) {
                        *self.latest_commit.borrow_mut() = Some(commit);
                        self.needs_present.set(true);
                    }
                }
                PageEvent::Title { text, nav_id } if !is_stale(accepted, nav_id) => {
                    *self.title.borrow_mut() = Some(text.trim().to_string());
                }
                PageEvent::Url { url, nav_id } if !is_stale(accepted, nav_id) => {
                    // The address after redirects; reload and back should use it.
                    if shown_in_address_bar(&url) {
                        *self.url.borrow_mut() = Some(url.to_string());
                        self.history.borrow_mut().replace_current(url);
                    }
                }
                PageEvent::LoadState { state, nav_id } if !is_stale(accepted, nav_id) => match state {
                    LoadState::Started => self.loading.set(true),
                    LoadState::Complete => self.loading.set(false),
                    LoadState::Failed { error } => {
                        debug!(%error, "Page load failed");
                        self.loading.set(false);
                    }
                },
                PageEvent::NavigateRequested { url, replace, nav_id, .. } if !is_stale(accepted, nav_id) => {
                    // The page navigated itself (script, a form). Follow it.
                    if let Ok(parsed) = Url::parse(&url) {
                        if replace {
                            self.history.borrow_mut().replace_current(parsed.clone());
                            self.load(parsed);
                        } else {
                            self.navigate(parsed);
                        }
                    }
                }
                // Focus, cursor, tooltips, context menu, IPC: a later step.
                _ => {}
            }
        }

        if self.needs_present.get() && self.visible.get() {
            let commit = self.latest_commit.borrow().clone();
            if let (Some(commit), Some(images)) = (commit, self.thread.images()) {
                match self.surface.borrow_mut().present_commit(&commit, &images) {
                    Ok(()) => self.needs_present.set(false),
                    Err(e) => warn!(error = %e, "Presenting the page failed"),
                }
            }
        }
    }

    /// What the chrome should show for this page.
    pub fn state(&self) -> PageState {
        let history = self.history.borrow();
        let url = self.url.borrow().clone();
        let title = self.title.borrow().clone().filter(|t| !t.is_empty());
        PageState {
            // The start page has no address of its own.
            url: if self.from_string.get() { Some("hiwave://about".to_string()) } else { url },
            title,
            loading: self.loading.get(),
            can_go_back: history.can_go_back(),
            can_go_forward: history.can_go_forward(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn an_empty_history_goes_nowhere() {
        let mut h = NavHistory::default();
        assert!(!h.can_go_back() && !h.can_go_forward());
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), None);
        assert_eq!(h.current(), None);
    }

    #[test]
    fn pushing_builds_a_trail_you_can_walk_back_and_forward() {
        let mut h = NavHistory::default();
        h.push(u("https://a.test/"));
        h.push(u("https://b.test/"));
        h.push(u("https://c.test/"));
        assert_eq!(h.current(), Some(&u("https://c.test/")));
        assert!(h.can_go_back() && !h.can_go_forward());
        assert_eq!(h.back(), Some(u("https://b.test/")));
        assert_eq!(h.back(), Some(u("https://a.test/")));
        assert_eq!(h.back(), None, "nothing before the first page");
        assert!(h.can_go_forward());
        assert_eq!(h.forward(), Some(u("https://b.test/")));
    }

    #[test]
    fn a_new_page_after_going_back_drops_the_forward_pages() {
        let mut h = NavHistory::default();
        h.push(u("https://a.test/"));
        h.push(u("https://b.test/"));
        h.back();
        h.push(u("https://c.test/"));
        assert!(!h.can_go_forward(), "b is gone, as in every browser");
        assert_eq!(h.back(), Some(u("https://a.test/")));
    }

    #[test]
    fn replacing_the_current_entry_keeps_the_trail() {
        let mut h = NavHistory::default();
        h.push(u("https://a.test/"));
        h.push(u("http://b.test/"));
        h.replace_current(u("https://b.test/after-redirect"));
        assert_eq!(h.current(), Some(&u("https://b.test/after-redirect")));
        assert_eq!(h.back(), Some(u("https://a.test/")));
    }

    #[test]
    fn replacing_in_an_empty_history_starts_it() {
        let mut h = NavHistory::default();
        h.replace_current(u("https://a.test/"));
        assert_eq!(h.current(), Some(&u("https://a.test/")));
    }

    #[test]
    fn only_events_older_than_the_newest_navigation_are_stale() {
        assert!(is_stale(5, 4));
        assert!(!is_stale(5, 5));
        assert!(!is_stale(5, 6));
        assert!(!is_stale(5, 0), "an event with no navigation is never stale");
    }

    #[test]
    fn the_address_bar_shows_network_and_file_pages_only() {
        assert!(shown_in_address_bar(&u("https://example.com/")));
        assert!(shown_in_address_bar(&u("http://example.com/")));
        assert!(shown_in_address_bar(&u("file:///C:/a.html")));
        assert!(!shown_in_address_bar(&u("about:blank")));
        assert!(!shown_in_address_bar(&u("data:text/html,hi")));
    }
}
