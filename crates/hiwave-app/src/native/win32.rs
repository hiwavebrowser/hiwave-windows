//! Native Win32 Entry Point for HiWave
//!
//! This module provides a pure Win32 entry point for HiWave that uses RustKit
//! for all rendering, completely bypassing wry/tao.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  Win32 Main Window (ViewHost)           │
//! ├─────────────────────────────────────────┤
//! │  RustKit Chrome View (tabs, toolbar)    │
//! ├─────────────────────────────────────────┤
//! │  RustKit Content View (web pages)       │
//! ├─────────────────────────────────────────┤
//! │  RustKit Shelf View (command palette)   │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Feature Flag
//!
//! This module is only compiled when the `native-win32` feature is enabled:
//!
//! ```bash
//! cargo build --features native-win32
//! ```

use rustkit_core::input::{InputEvent, KeyCode, KeyEvent, KeyEventType, Modifiers, MouseButton, MouseEventType};
use super::shortcuts::{self, Shortcut};
use super::chrome_data;
use super::content_page::{self, ContentPage};
use super::layout::{self, Layout, LayoutInput};
use super::tabs::{TabModel, TabSwitch};
use rustkit_engine::{Engine, EngineBuilder, EngineViewId, IpcMessage};
use rustkit_viewhost::{Bounds, MainWindowConfig, ViewEvent, ViewHost};
use std::cell::RefCell;
use std::time::{Duration, Instant};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, trace, warn};
use windows::Win32::Foundation::HWND;

use crate::state::AppState;

// Chrome UI constants
const CHROME_HEIGHT_DEFAULT: u32 = 104;
const CHROME_HEIGHT_EXPANDED: u32 = 460;
const SHELF_HEIGHT_DEFAULT: u32 = 0;
const SHELF_HEIGHT_EXPANDED: u32 = 280;
const SIDEBAR_WIDTH: u32 = 220;

/// HTML content embedded at compile time
const CHROME_HTML: &str = include_str!("../ui/chrome.html");
const SHELF_HTML: &str = include_str!("../ui/shelf.html");
const ABOUT_HTML: &str = include_str!("../ui/about.html");
/// The find-in-page helper. It runs inside the page and walks the DOM; it needs no engine support.
const FIND_IN_PAGE_HELPER: &str = include_str!("../ui/find_in_page.js");
/// The `<title>` of the start page (ui/about.html).
const ABOUT_TITLE: &str = "About HiWave";

/// One row of the chrome's tab strip.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TabRow {
    id: u64,
    title: String,
    url: String,
    active: bool,
}

/// What the chrome page was last told. The run loop compares the live state
/// against this each turn and only calls into the page when something changed,
/// so typing in the address bar is never overwritten by a periodic refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChromeSnapshot {
    tabs: Vec<TabRow>,
    url: String,
    title: String,
    can_go_back: bool,
    can_go_forward: bool,
    shield_enabled: bool,
    shield_blocked: u64,
    loading: bool,
}

/// View types in the browser
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewType {
    Chrome,
    Content,
    Shelf,
}

/// State for the native Win32 browser
pub struct NativeBrowser {
    /// ViewHost manages all Win32 windows
    viewhost: ViewHost,
    /// RustKit engine for rendering
    engine: RefCell<Engine>,
    /// One runtime owns page I/O across loads and subsequent live turns.
    runtime: tokio::runtime::Runtime,
    live_clocks: RefCell<HashMap<EngineViewId, Instant>>,
    /// Map of view types to engine view IDs
    views: HashMap<ViewType, EngineViewId>,
    /// Reverse map: engine view ID to view type (for IPC routing)
    engine_view_types: HashMap<EngineViewId, ViewType>,
    /// Key presses captured by the viewhost callback, drained each frame.
    ///
    /// A queue rather than direct dispatch because the callback is
    /// `Fn + Send + Sync` and the browser is not: it owns `RefCell<Engine>`
    /// and must only be touched from the loop thread. Same shape as the
    /// existing IPC drain.
    event_queue: Arc<Mutex<VecDeque<ViewEvent>>>,
    /// Tab strip. `None` until the first content view exists — the model
    /// cannot represent a zero-tab window, so it is not constructed until
    /// there is a real view to put in it (see `native/tabs.rs`).
    tabs: Option<TabModel<EngineViewId>>,
    /// Application state
    #[allow(dead_code)]
    app_state: Arc<Mutex<AppState>>,
    /// Current window dimensions
    window_width: u32,
    window_height: u32,
    /// Layout state
    chrome_height: u32,
    shelf_height: u32,
    /// Width of the left sidebar; 0 while it is closed (chrome.html starts closed).
    sidebar_width: u32,
    right_panel_open: bool,
    /// The view that last had the keyboard focus. Win32 can leave focus on the
    /// bare main window (activation, a click on the frame); the main window
    /// ignores keys, so the shell hands focus back to this view.
    last_focus: std::cell::Cell<Option<EngineViewId>>,
    /// The chrome page has loaded and sent `chrome_ready`; until then there is
    /// no `window.hiwaveChrome` to call.
    chrome_ready: bool,
    /// Stable ids for the tab strip (a tab is an engine view; the page wants a number).
    next_tab_id: u64,
    tab_ids: HashMap<EngineViewId, u64>,
    chrome_synced: Option<ChromeSnapshot>,
    /// The request filter the engine runs; kept so the chrome can show its count
    /// and switch it off.
    shield: Arc<crate::shield_adapter::ShieldInterceptHandler>,
    shield_enabled: bool,
    /// The page on the content thread (`HIWAVE_CONTENT_THREAD=1`). `None`: pages
    /// load on the window thread, as before.
    pages: HashMap<u64, ContentPage>,
    /// Which page is which tab, in order, and the active one. `None` unless the
    /// content thread is on.
    page_tabs: Option<TabModel<u64>>,
    next_page_id: u64,
    /// Which snapshots of the active tab are a new visit for the history.
    visits: chrome_data::VisitTracker,
    /// A find on the content thread whose answer has not come back yet, with the
    /// query the chrome sent (the answer is echoed with that, never the page's).
    pending_find: RefCell<Option<(tokio::sync::oneshot::Receiver<Result<String, String>>, String)>>,
    /// The page (not a chrome view) has the keyboard.
    page_focused: std::cell::Cell<bool>,
}

impl NativeBrowser {
    /// Create a new native browser instance.
    pub fn new() -> Result<Self, String> {
        info!("Initializing native Win32 browser");

        // The request filter. The engine gets it through an interceptor; we keep a
        // handle for the blocked-request count and the on/off switch.
        let shield = Arc::new(crate::shield_adapter::ShieldInterceptHandler::new());

        // Create the engine. The chrome and shelf live here; with the content thread
        // on, the page gets its own engine from the same settings.
        let engine = engine_builder(&shield)
            .build()
            .map_err(|e| format!("Failed to create engine: {}", e))?;

        // Initialize app state
        let app_state = Arc::new(Mutex::new(
            AppState::with_defaults()
                .map_err(|e| format!("Failed to initialize app state: {}", e))?,
        ));

        Ok(Self {
            viewhost: ViewHost::new(),
            engine: RefCell::new(engine),
            runtime: tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("Failed to create page runtime: {e}"))?,
            live_clocks: RefCell::new(HashMap::new()),
            views: HashMap::new(),
            engine_view_types: HashMap::new(),
            event_queue: Arc::new(Mutex::new(VecDeque::new())),
            tabs: None,
            app_state,
            window_width: 1280,
            window_height: 800,
            chrome_height: CHROME_HEIGHT_DEFAULT,
            shelf_height: SHELF_HEIGHT_DEFAULT,
            sidebar_width: 0,
            right_panel_open: false,
            last_focus: std::cell::Cell::new(None),
            chrome_ready: false,
            next_tab_id: 1,
            tab_ids: HashMap::new(),
            chrome_synced: None,
            shield,
            shield_enabled: true,
            pages: HashMap::new(),
            page_tabs: None,
            next_page_id: 1,
            pending_find: RefCell::new(None),
            visits: chrome_data::VisitTracker::default(),
            page_focused: std::cell::Cell::new(false),
        })
    }

    /// Initialize the browser window and views.
    pub fn init(&mut self) -> Result<(), String> {
        // Create the main window
        let config = MainWindowConfig::new("HiWave").with_size(1280, 800);
        let hwnd = self
            .viewhost
            .create_main_window(config)
            .map_err(|e| format!("Failed to create window: {}", e))?;

        // Set up event callback
        let _callback = Arc::new(move |event: ViewEvent| {
            // Events are handled in the message loop
            debug!(?event, "View event received");
        });

        // Lay out against the window as it really is, not the size asked for:
        // the client area is smaller than the outer size by the frame.
        self.sync_window_size();

        // Create the three views
        self.create_views(hwnd)?;
        self.subscribe_to_events();
        if let Some(page) = self.page() {
            page.grab_keyboard();
            self.page_focused.set(true);
        }
        // Focus the content view. WITHOUT THIS, KEYBOARD SHORTCUTS DO NOT
        // WORK AT ALL: Win32 delivers WM_KEYDOWN to the FOCUSED window, the
        // main window proc handles no keys, and only the child-view proc
        // emits them. At startup nothing held focus, so every keypress went
        // to a window that dropped it. Found by pressing Ctrl+T at a running
        // browser and reading the log — the unit tests were all green.
        if let Some(&content) = self.views.get(&ViewType::Content) {
            match self.engine.borrow().focus_view(content) {
                Ok(()) => self.last_focus.set(Some(content)),
                Err(e) => warn!(error = %e, "Failed to focus the content view"),
            }
        }

        // Load initial content
        self.load_initial_content()?;

        info!("Native browser initialized");
        Ok(())
    }

    /// The active page, when the content thread is on.
    fn page(&self) -> Option<&ContentPage> {
        let tabs = self.page_tabs.as_ref()?;
        self.pages.get(&tabs.active_view())
    }

    /// The page whose window reports its input under `host_view`.
    fn page_for_viewhost(&self, host_view: rustkit_viewhost::ViewId) -> Option<&ContentPage> {
        self.pages.values().find(|page| page.viewhost_id() == host_view)
    }

    /// Start a page on its own thread at the content bounds.
    fn spawn_page(&mut self) -> Result<u64, String> {
        let parent = self.viewhost.get_main_hwnd().ok_or("no main window")?;
        let bounds = to_bounds(self.layout().content);
        let page = ContentPage::spawn(parent, bounds, engine_builder(&self.shield))?;
        let id = self.next_page_id;
        self.next_page_id += 1;
        self.pages.insert(id, page);
        // Child windows do not stack in creation order here: keep the chrome under them.
        if let Some(&chrome_id) = self.views.get(&ViewType::Chrome) {
            lower_view(&self.engine.borrow(), chrome_id);
        }
        Ok(id)
    }

    /// Show the page that became active and hide the one that stopped being.
    fn apply_page_switch(&mut self, switch: TabSwitch<u64>) {
        if let Some(previous) = self.pages.get(&switch.previous) {
            previous.set_visible(false);
        }
        if let Some(current) = self.pages.get(&switch.current) {
            current.set_visible(true);
            current.grab_keyboard();
            self.page_focused.set(true);
        }
    }

    /// Create Chrome, Content, and Shelf views.
    fn create_views(&mut self, parent: HWND) -> Result<(), String> {
        let mut engine = self.engine.borrow_mut();

        // The chrome view spans the window; content and shelf are created
        // after it so they sit on top (see native/layout.rs).
        let Layout { chrome, content, shelf } = self.layout();
        let chrome_bounds = to_bounds(chrome);
        let content_bounds = to_bounds(content);
        let shelf_bounds = to_bounds(shelf);

        // Create Chrome view (tabs, address bar, toolbar)
        let chrome_id = engine
            .create_view(parent, chrome_bounds)
            .map_err(|e| format!("Failed to create Chrome view: {}", e))?;
        self.views.insert(ViewType::Chrome, chrome_id);
        self.engine_view_types.insert(chrome_id, ViewType::Chrome);
        debug!(?chrome_id, "Chrome view created");

        // Create Content view (web page rendering). With the content thread on, the
        // page is not an engine view of this engine: it runs on its own thread and
        // this window only shows what it publishes.
        let content_id = if content_page::enabled() {
            let page = ContentPage::spawn(parent, content_bounds, engine_builder(&self.shield))?;
            let id = self.next_page_id;
            self.next_page_id += 1;
            self.pages.insert(id, page);
            self.page_tabs = Some(TabModel::new(id));
            None
        } else {
            let content_id = engine
                .create_view(parent, content_bounds)
                .map_err(|e| format!("Failed to create Content view: {}", e))?;
            self.views.insert(ViewType::Content, content_id);
            self.engine_view_types.insert(content_id, ViewType::Content);
            self.tabs = Some(TabModel::new(content_id));
            debug!(?content_id, "Content view created");
            Some(content_id)
        };

        // Create Shelf view (command palette, hidden by default)
        let shelf_id = engine
            .create_view(parent, shelf_bounds)
            .map_err(|e| format!("Failed to create Shelf view: {}", e))?;
        self.views.insert(ViewType::Shelf, shelf_id);
        self.engine_view_types.insert(shelf_id, ViewType::Shelf);
        debug!(?shelf_id, "Shelf view created");
        
        // The chrome view spans the window, so it must sit UNDER the content view.
        // Child windows do not stack in creation order here (the chrome view, made
        // first, ended up on top and hid the page), so say so explicitly.
        lower_view(&engine, chrome_id);

        info!(chrome = ?chrome_id, content = ?content_id, on_content_thread = self.page_tabs.is_some(), shelf = ?shelf_id, "All views created");

        Ok(())
    }

    /// Load initial HTML content into views.
    fn load_initial_content(&self) -> Result<(), String> {
        let mut engine = self.engine.borrow_mut();

        // Load Chrome UI
        if let Some(&chrome_id) = self.views.get(&ViewType::Chrome) {
            // The app's own documents are the only ones given window.ipc, and the
            // engine drops IPC from every other view (CH3.A7). It must be set
            // BEFORE the load: the bridge is built with the document. Without it
            // the toolbar, tabs, sidebar and shelf all talk to nobody.
            engine.set_app_owned(chrome_id, true);
            engine
                .load_html(chrome_id, CHROME_HTML)
                .map_err(|e| format!("Failed to load Chrome HTML: {}", e))?;
            debug!(bytes = CHROME_HTML.len(), "Chrome HTML loaded");
        }

        // Load Shelf UI
        if let Some(&shelf_id) = self.views.get(&ViewType::Shelf) {
            engine.set_app_owned(shelf_id, true);
            engine
                .load_html(shelf_id, SHELF_HTML)
                .map_err(|e| format!("Failed to load Shelf HTML: {}", e))?;
            debug!(bytes = SHELF_HTML.len(), "Shelf HTML loaded");
        }

        // Load About page in content view
        if let Some(page) = self.page() {
            page.load_html(ABOUT_HTML);
        } else if let Some(&content_id) = self.views.get(&ViewType::Content) {
            engine
                .load_html(content_id, ABOUT_HTML)
                .map_err(|e| format!("Failed to load About HTML: {}", e))?;
            debug!(bytes = ABOUT_HTML.len(), "About page loaded");
        }

        info!("Initial content loaded into all views");
        Ok(())
    }

    /// Where each view goes for the current window and panel state.
    fn layout(&self) -> Layout {
        layout::compute(LayoutInput {
            window_width: self.window_width,
            window_height: self.window_height,
            chrome_height: self.chrome_height,
            shelf_height: self.shelf_height,
            sidebar_width: self.sidebar_width,
            right_panel_open: self.right_panel_open,
        })
    }

    /// Update layout after window resize or layout change.
    fn update_layout(&self) {
        let mut engine = self.engine.borrow_mut();
        let Layout { chrome, content, shelf } = self.layout();

        let mut targets = vec![
            (ViewType::Chrome, chrome, self.views.get(&ViewType::Chrome).copied()),
            (ViewType::Shelf, shelf, self.views.get(&ViewType::Shelf).copied()),
        ];
        // Every tab is its own content view; only the active one is shown, but
        // a background tab must already have the right size when it is
        // switched to, or it comes back at whatever the window used to be.
        match self.tabs.as_ref() {
            Some(tabs) => targets.extend(tabs.views().iter().map(|&id| (ViewType::Content, content, Some(id)))),
            None => targets.push((ViewType::Content, content, self.views.get(&ViewType::Content).copied())),
        }

        for (view_type, rect, id) in targets {
            let Some(id) = id else { continue };
            if let Err(e) = engine.resize_view(id, to_bounds(rect)) {
                warn!(?view_type, error = %e, "Failed to resize view");
            }
        }
        for page in self.pages.values() {
            page.resize(to_bounds(content));
        }
        if let Some(&chrome_id) = self.views.get(&ViewType::Chrome) {
            lower_view(&engine, chrome_id);
        }
    }

    /// Follow the window's real client size. Nothing else tells this shell the
    /// window was resized or maximised, so the run loop calls this every turn;
    /// it does nothing unless the size changed. A minimised window reports
    /// 0x0, which is ignored so the views keep their size for the restore.
    fn sync_window_size(&mut self) {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

        let Some(hwnd) = self.viewhost.get_main_hwnd() else {
            return;
        };
        let mut rect = RECT::default();
        // SAFETY: `hwnd` is the live main window this browser created, and
        // `rect` is a valid out-pointer for the duration of the call.
        if unsafe { GetClientRect(hwnd, &mut rect) }.is_err() {
            return;
        }
        let (width, height) = (
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        );
        if width == 0 || height == 0 {
            return;
        }
        if (width, height) != (self.window_width, self.window_height) {
            self.on_resize(width, height);
        }
    }

    /// Handle window resize event.
    pub fn on_resize(&mut self, width: u32, height: u32) {
        self.window_width = width;
        self.window_height = height;
        // Before the views exist (first call, from init) there is nothing to move.
        if !self.views.is_empty() {
            self.update_layout();
        }
    }

    /// Expand the Chrome UI (for overlays).
    pub fn expand_chrome(&mut self) {
        self.chrome_height = CHROME_HEIGHT_EXPANDED;
        self.update_layout();
    }

    /// Collapse the Chrome UI to default height.
    pub fn collapse_chrome(&mut self) {
        self.chrome_height = CHROME_HEIGHT_DEFAULT;
        self.update_layout();
    }

    /// Expand the Shelf (command palette).
    pub fn expand_shelf(&mut self) {
        self.shelf_height = SHELF_HEIGHT_EXPANDED;
        self.update_layout();
    }

    /// Collapse the Shelf.
    pub fn collapse_shelf(&mut self) {
        self.shelf_height = SHELF_HEIGHT_DEFAULT;
        self.update_layout();
    }

    /// Toggle sidebar visibility.
    pub fn toggle_sidebar(&mut self) {
        let open = self.sidebar_width == 0;
        self.set_sidebar_open(open);
    }

    /// Open the sidebar at its default width, or close it.
    pub fn set_sidebar_open(&mut self, open: bool) {
        self.sidebar_width = if open { SIDEBAR_WIDTH } else { 0 };
        self.update_layout();
    }

    /// Set the sidebar's width (the chrome sends this while it is dragged).
    pub fn set_sidebar_width(&mut self, width: u32) {
        self.sidebar_width = width;
        self.update_layout();
    }

    /// Open or close the right-hand panel.
    pub fn set_right_panel_open(&mut self, open: bool) {
        self.right_panel_open = open;
        self.update_layout();
    }

    /// Navigate to a URL.
    /// Stop the content view's in-flight navigation.
    ///
    /// This is the first of the five rented capabilities to become OURS. The
    /// hybrid shell implements Stop as `evaluate_script("window.stop()")` —
    /// a JavaScript call asking Chromium to cancel its own load. This asks
    /// nobody: `Engine::stop` marks the navigation superseded so the engine
    /// will not apply its result.
    ///
    /// Honest limit, same as the engine's: the socket is not aborted, because
    /// `rustkit-net::fetch` has no cancellation surface yet. The bytes arrive
    /// and are discarded. Stop-as-observed, not stop-as-transport.
    pub fn stop_loading(&self) {
        if let Some(page) = self.page() {
            page.stop();
            return;
        }
        if let Some(&content_id) = self.views.get(&ViewType::Content) {
            let mut engine = self.engine.borrow_mut();
            if engine.stop(content_id) {
                info!("Navigation stopped");
            } else {
                warn!("Stop requested for a content view that does not exist");
            }
        }
    }

    /// History traversal — back, forward, reload — on OUR engine.
    ///
    /// The hybrid shell rents all three from Chromium via
    /// `evaluate_script("history.back()")` etc. These run against RustKit's
    /// SessionHistory (canonical per the 2026-08-05 pin) and our own loader.
    ///
    /// Same block_on shape as `navigate` — one current-thread runtime per
    /// call. That is the known engine-thread soft spot (Atlas's follow-up
    /// unit, inherited by this tree), not a new decision made here.
    /// Apply a tab switch to the Win32 side.
    ///
    /// THE INVARIANT THIS MAINTAINS: `views[ViewType::Content]` always names
    /// the active tab's view. Every existing code path — navigate, traverse,
    /// resize, render, IPC routing — reads Content and keeps working without
    /// knowing tabs exist. Tabs became a list of content views plus this one
    /// rule, rather than a rewrite of the shell.
    fn apply_switch(&mut self, switch: TabSwitch<EngineViewId>) {
        {
            let engine = self.engine.borrow();
            if let Err(e) = engine.set_view_visible(switch.previous, false) {
                warn!(error = %e, "Failed to hide the outgoing tab");
            }
            if let Err(e) = engine.set_view_visible(switch.current, true) {
                warn!(error = %e, "Failed to show the incoming tab");
            }
        }
        self.views.insert(ViewType::Content, switch.current);
        self.engine_view_types
            .insert(switch.current, ViewType::Content);
        // Focus follows the active tab, or the next shortcut goes to a window
        // that is no longer showing.
        match self.engine.borrow().focus_view(switch.current) {
            Ok(()) => self.last_focus.set(Some(switch.current)),
            Err(e) => warn!(error = %e, "Failed to focus the incoming tab"),
        }
    }

    /// Open a new tab and select it.
    fn new_tab(&mut self) {
        if self.page_tabs.is_some() {
            match self.spawn_page() {
                Ok(id) => {
                    if let Some(tabs) = self.page_tabs.as_mut() {
                        let switch = tabs.push(id);
                        let count = tabs.count();
                        self.apply_page_switch(switch);
                        info!(id, count, "New tab (content thread)");
                    }
                }
                Err(e) => error!(error = %e, "Failed to start a page for the new tab"),
            }
            return;
        }
        let Some(parent) = self.viewhost.get_main_hwnd() else {
            warn!("No main window — cannot open a tab");
            return;
        };
        let bounds = to_bounds(self.layout().content);
        let created = self.engine.borrow_mut().create_view(parent, bounds);
        if let Some(&chrome_id) = self.views.get(&ViewType::Chrome) {
            lower_view(&self.engine.borrow(), chrome_id);
        }
        let view = match created {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "Failed to create a view for the new tab");
                return;
            }
        };
        let Some(tabs) = self.tabs.as_mut() else {
            warn!("Tab model not initialised — cannot open a tab");
            return;
        };
        let switch = tabs.push(view);
        let count = tabs.count();
        self.apply_switch(switch);
        info!(?view, count, "New tab");
    }

    /// Close the active tab.
    ///
    /// Closing the LAST tab is deliberately a no-op here rather than a window
    /// close: quitting on Ctrl+W is a product decision, and the tab strip
    /// refusing to empty itself (`close_active` -> None) is what keeps
    /// `active_view()` total. Wire the quit at the window layer when the
    /// product calls for it.
    fn close_active_tab(&mut self) {
        if let Some(tabs) = self.page_tabs.as_mut() {
            let Some((closed, switch)) = tabs.close_active() else {
                debug!("Refusing to close the last tab");
                return;
            };
            let count = tabs.count();
            self.apply_page_switch(switch);
            // After the switch, so the replacement is already showing.
            if let Some(page) = self.pages.remove(&closed) {
                page.close();
            }
            info!(closed, count, "Tab closed (content thread)");
            return;
        }
        let Some(tabs) = self.tabs.as_mut() else {
            return;
        };
        let Some((closed, switch)) = tabs.close_active() else {
            debug!("Refusing to close the last tab");
            return;
        };
        let count = tabs.count();
        self.apply_switch(switch);
        // Destroy AFTER the switch: the replacement is already visible, so
        // there is no frame where the window has nothing to show.
        if let Err(e) = self.engine.borrow_mut().destroy_view(closed) {
            warn!(error = %e, ?closed, "Failed to destroy the closed tab's view");
        }
        self.engine_view_types.remove(&closed);
        info!(?closed, count, "Tab closed");
    }

    /// Select a tab by index (Ctrl+1..9).
    fn activate_tab_by_index(&mut self, index: usize) {
        if let Some(tabs) = self.page_tabs.as_mut() {
            if let Some(switch) = tabs.activate(index) {
                self.apply_page_switch(switch);
                debug!(index, "Tab activated (content thread)");
            }
            return;
        }
        let Some(tabs) = self.tabs.as_mut() else {
            return;
        };
        match tabs.activate(index) {
            Some(switch) => {
                self.apply_switch(switch);
                debug!(index, "Tab activated");
            }
            // Out of range or already active: nothing to show or hide.
            None => debug!(index, "Tab activation: nothing to do"),
        }
    }

    /// Subscribe to viewhost events.
    ///
    /// ORPHAN #9, WIRED. `Engine::handle_view_event` is the engine's COMPLETE
    /// input dispatcher -- it maps viewhost_id -> EngineViewId and fans out to
    /// mouse, key and focus handlers that all already existed -- and it had
    /// ZERO production callers. That is why this shell had no input at all:
    /// no clicks, no keys, no scroll. The capability was never missing; the
    /// wire was never run.
    ///
    /// The callback queues WHOLE ViewEvents rather than extracting keys: the
    /// shortcut layer needs first refusal on keys, while everything else --
    /// mouse, scroll, focus -- belongs to the engine untouched.
    ///
    /// A queue rather than direct dispatch because the callback is
    /// `Fn + Send + Sync` and the browser is not: it owns `RefCell<Engine>`
    /// and must only be touched from the loop thread.
    fn subscribe_to_events(&self) {
        let queue = Arc::clone(&self.event_queue);
        self.viewhost.set_event_callback(Arc::new(move |event| {
            if let Ok(mut q) = queue.lock() {
                q.push_back(event);
            }
        }));
    }

    /// Drain queued input: shell shortcuts first, then the engine.
    ///
    /// ORDER IS THE DESIGN. A shell shortcut must beat the page: Ctrl+T has to
    /// open a tab even on a page that binds Ctrl+T itself, or a page can hold
    /// the browser hostage. Everything the shell does NOT claim is handed to
    /// the engine untouched -- swallowing unrecognised input would break every
    /// text field on the web.
    fn process_input_events(&mut self) {
        let pending: Vec<ViewEvent> = match self.event_queue.lock() {
            Ok(mut q) => q.drain(..).collect(),
            Err(e) => {
                warn!(error = %e, "Event queue poisoned");
                return;
            }
        };

        for event in pending {
            // KeyDown only: acting on both edges fires every shortcut twice.
            // Repeats are kept -- held Alt+Left walking back through history
            // is what people expect.
            if let ViewEvent::Input {
                event: InputEvent::Key(key),
                ..
            } = &event
            {
                if key.event_type == KeyEventType::KeyDown {
                    if let Some(shortcut) = shortcuts::resolve(key.key_code, key.modifiers)
                        .filter(|s| !self.chrome_owns_escape(*s))
                    {
                        debug!(?shortcut, "Shortcut");
                        self.run_shortcut(shortcut);
                        // Claimed by the shell; the page never sees it.
                        continue;
                    }
                }
            }
            // Input on the content-thread page goes to the page, not the engine.
            if let ViewEvent::Input { view_id, event: inner } = &event {
                if let Some(page) = self.page_for_viewhost(*view_id) {
                    self.route_page_input(page, inner);
                    continue;
                }
            }
            // A click gives the view it landed on the keyboard focus. Win32 sends
            // key messages only to the focused window, and nothing moved focus off
            // the page at startup, so typing in the chrome (the address bar) went
            // to the page.
            if let ViewEvent::Input {
                event: InputEvent::Mouse(mouse),
                ..
            } = &event
            {
                if mouse.event_type == MouseEventType::MouseDown {
                    self.focus_clicked_view();
                }
            }
            // Text goes to the engine's editor. handle_view_event only fires DOM
            // key events; it never types into a field.
            if let ViewEvent::Input {
                event: InputEvent::Key(key),
                ..
            } = &event
            {
                if self.route_text_key(key) {
                    continue;
                }
            }
            let navigation = self.engine.borrow_mut().handle_view_event(event);
            if let Some((id, url)) = navigation {
                self.navigate_view(id, &url);
            }
        }
    }

    /// Escape means "stop loading" only for the page. With one of the app's own views
    /// focused (the chrome: the find box, the address bar; or the shelf, the command
    /// palette) it belongs to that view's page, which closes its own UI on it.
    fn chrome_owns_escape(&self, shortcut: Shortcut) -> bool {
        shortcut == Shortcut::Stop
            && !self.page_focused.get()
            && self.focused_view().is_some_and(|focused| {
                [ViewType::Chrome, ViewType::Shelf]
                    .iter()
                    .any(|kind| self.views.get(kind) == Some(&focused))
            })
    }

    /// Deliver mouse and key input from the content-thread page's window to the page.
    fn route_page_input(&self, page: &ContentPage, input: &InputEvent) {
        match input {
            InputEvent::Mouse(m) => {
                let (x, y) = (m.position.x as f32, m.position.y as f32);
                match m.event_type {
                    MouseEventType::MouseDown if m.button == MouseButton::Primary => {
                        // Win32 does not move focus to a child window on a click.
                        page.grab_keyboard();
                        self.page_focused.set(true);
                        page.mouse_down(x, y, m.click_count.max(1), m.modifiers.shift);
                    }
                    MouseEventType::MouseUp if m.button == MouseButton::Primary => page.click(x, y),
                    MouseEventType::MouseMove | MouseEventType::MouseEnter => page.mouse_move(x, y),
                    MouseEventType::MouseLeave => page.mouse_leave(),
                    // ViewHost reports notches (120 native units); the page takes pixels.
                    MouseEventType::Wheel => page.scroll_at(x, y, m.delta.x as f32 * 120.0, m.delta.y as f32 * 120.0),
                    _ => {}
                }
            }
            InputEvent::Key(key) => {
                let mods = key.modifiers;
                match key_action(key) {
                    KeyAction::Send(vk, text) => page.key(vk, &text, mods.ctrl, mods.shift, mods.alt),
                    KeyAction::Pass if key.event_type == KeyEventType::KeyUp => {
                        page.key_up(key.key_code as u32, "", mods.ctrl, mods.shift, mods.alt)
                    }
                    _ => {}
                }
            }
            InputEvent::Focus(_) => {}
        }
    }

    /// The engine view whose window is `hwnd`, if it is one of ours. Child
    /// windows are identified by handle: the engine owns its own ViewHost, so
    /// the viewhost view ids on input events do not resolve in this shell's.
    fn engine_view_for_hwnd(&self, hwnd: HWND) -> Option<EngineViewId> {
        if hwnd.0.is_null() {
            return None;
        }
        let engine = self.engine.borrow();
        self.engine_view_types
            .keys()
            .copied()
            .find(|&id| engine.get_view_hwnd(id).map(|h| h == hwnd).unwrap_or(false))
    }

    /// The view that has the keyboard focus (key messages go only to it).
    fn focused_view(&self) -> Option<EngineViewId> {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
        // SAFETY: GetFocus has no preconditions; this runs on the window's own thread.
        self.engine_view_for_hwnd(unsafe { GetFocus() })
    }

    /// Remember which view has the focus, and give it back if it landed on the
    /// bare main window. Only acts when the main window itself is the focus
    /// window, so it never takes focus from another application.
    fn restore_focus(&self) {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
        // SAFETY: GetFocus has no preconditions; this runs on the window's own thread.
        let focus = unsafe { GetFocus() };
        if let Some(id) = self.engine_view_for_hwnd(focus) {
            self.last_focus.set(Some(id));
            self.page_focused.set(false);
            return;
        }
        if self.pages.values().any(|page| page.hwnd() == focus) {
            self.page_focused.set(true);
            return;
        }
        let Some(main) = self.viewhost.get_main_hwnd() else {
            return;
        };
        if focus != main {
            return;
        }
        if let (Some(page), true) = (self.page(), self.page_focused.get()) {
            page.grab_keyboard();
            return;
        }
        if let Some(id) = self.last_focus.get() {
            if self.engine_view_types.contains_key(&id) {
                let _ = self.engine.borrow().focus_view(id);
            }
        }
    }

    /// Give the keyboard focus to the view under the mouse (the one just
    /// clicked). Win32 does not move focus to a child window on a click.
    fn focus_clicked_view(&self) {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, WindowFromPoint};

        let mut pt = POINT::default();
        // SAFETY: `pt` is a valid out-pointer.
        if unsafe { GetCursorPos(&mut pt) }.is_err() {
            return;
        }
        // SAFETY: plain query by screen point.
        let Some(id) = self.engine_view_for_hwnd(unsafe { WindowFromPoint(pt) }) else {
            return;
        };
        match self.engine.borrow().focus_view(id) {
            Ok(()) => {
                self.last_focus.set(Some(id));
                self.page_focused.set(false);
            }
            Err(e) => warn!(error = %e, "Failed to focus the clicked view"),
        }
    }

    /// Deliver a key to the engine's text editor. Returns true when the event
    /// was handled here and must not also go to `handle_view_event` (which
    /// would fire the same DOM key event a second time); false for key-up.
    ///
    /// Same split as the macOS app: editing and navigation keys go as a virtual
    /// key with no text, typed characters as text with no key, and Ctrl+A/C/V/X/Z
    /// as the letter with Ctrl set. Printable keys arrive twice on Win32 (WM_KEYDOWN
    /// and then WM_CHAR); only the WM_CHAR half types.
    fn route_text_key(&mut self, key: &KeyEvent) -> bool {
        let (vk, text) = match key_action(key) {
            KeyAction::Pass => return false,
            KeyAction::Ignore => return true,
            KeyAction::Send(vk, text) => (vk, text),
        };

        let Some(id) = self.focused_view().or_else(|| self.last_focus.get()) else {
            return false;
        };
        let m = key.modifiers;

        // Enter goes through the engine's form submission first, in every view:
        // that is what fires Enter's keydown, which is how the chrome's address
        // bar hears it (handle_text_key fires none for Enter). A page's form
        // that submits to a URL navigates, as on macOS; if nothing took the
        // key, Enter's default (a newline in a textarea, a pick in a select)
        // falls through to handle_text_key.
        if vk == 0x0D {
            let submitted = self.engine.borrow_mut().submit_focused_form(id);
            if let Some(url) = submitted {
                if self.engine_view_types.get(&id) == Some(&ViewType::Content) {
                    info!(%url, "Form submitted");
                    self.navigate_view(id, &url);
                }
                return true;
            }
        }

        let changed = self
            .engine
            .borrow_mut()
            .handle_text_key(id, vk, &text, m.ctrl, m.shift, m.alt);
        if changed {
            if let Err(e) = self.engine.borrow_mut().relayout(id) {
                warn!(error = %e, "Relayout after a key failed");
            }
        }
        true
    }

    fn run_shortcut(&mut self, shortcut: Shortcut) {
        match shortcut {
            Shortcut::NewTab => self.new_tab(),
            Shortcut::CloseTab => self.close_active_tab(),
            Shortcut::ActivateTab(i) => self.activate_tab_by_index(i),
            Shortcut::ActivateLastTab => {
                let count = self.page_tabs.as_ref().map(|t| t.count()).or_else(|| self.tabs.as_ref().map(|t| t.count()));
                if let Some(last) = count.map(|c| c - 1) {
                    self.activate_tab_by_index(last);
                }
            }
            Shortcut::Back => self.traverse("back"),
            Shortcut::Forward => self.traverse("forward"),
            Shortcut::Reload => self.traverse("reload"),
            Shortcut::Stop => self.stop_loading(),
            Shortcut::Find => self.send_chrome_chord(0x46, "f"),
            Shortcut::FocusAddressBar => self.send_chrome_chord(0x4C, "l"),
            Shortcut::ToggleSidebar => self.send_chrome_chord(0x42, "b"),
            Shortcut::CommandPalette => self.send_chrome_chord(0x4B, "k"),
            Shortcut::History => self.send_chrome_chord(0x48, "h"),
        }
    }

    /// Deliver Ctrl+<key> to the chrome page as a keyboard event, the way typing it
    /// there would, and move the keyboard focus to the chrome so what is typed next
    /// lands in the field the page just focused. The page owns these chords (find,
    /// address bar, sidebar, command palette) in its own keydown handler.
    fn send_chrome_chord(&mut self, vk: u32, key: &str) {
        let Some(&chrome_id) = self.views.get(&ViewType::Chrome) else {
            return;
        };
        if self.engine.borrow().focus_view(chrome_id).is_ok() {
            self.last_focus.set(Some(chrome_id));
            self.page_focused.set(false);
        }
        let changed = self.engine.borrow_mut().handle_text_key(chrome_id, vk, key, true, false, false);
        if changed {
            if let Err(e) = self.engine.borrow_mut().relayout(chrome_id) {
                warn!(error = %e, "Relayout after a chrome chord failed");
            }
        }
    }

    /// Find `query` in the active page and tell the chrome's find bar how many
    /// matches there are. The helper highlights matches in the page itself.
    fn find_in_page(&self, query: &str, case_sensitive: bool, direction: &str) {
        let payload = serde_json::json!({
            "query": query,
            "case_sensitive": case_sensitive,
            "direction": direction,
        });
        let script = format!(
            "{FIND_IN_PAGE_HELPER}\n(() => {{ const r = window.__hiwaveFind && window.__hiwaveFind.run({payload}); return r ? JSON.stringify(r) : 'null'; }})();"
        );

        // On the content thread the answer comes back later: remember the query and
        // collect it in `poll_find`. A newer find replaces an older one still waiting.
        //
        // `page.eval` runs a script WE wrote (the bundled find helper plus the query
        // as a JSON-escaped literal, never concatenated text) inside the page, the
        // same as the engine's `execute_script` does for the window-thread path. No
        // page-supplied text becomes code here; the page's ANSWER is the untrusted
        // part, and `show_find_result` reduces it to two numbers before the chrome
        // sees it.
        if let Some(page) = self.page() {
            *self.pending_find.borrow_mut() = Some((page.eval(&script), query.to_string()));
            return;
        }

        let Some(&content_id) = self.views.get(&ViewType::Content) else {
            return;
        };
        let out = self.engine.borrow_mut().execute_script(content_id, &script);
        self.show_find_result(out.ok(), query);
    }

    /// Tell the find bar what the helper found. The helper ran in the PAGE, so its
    /// answer is page-controlled: only numbers parsed out of it, and the query the
    /// chrome itself sent, reach the chrome.
    fn show_find_result(&self, answer: Option<String>, query: &str) {
        match answer.and_then(|o| script_string(&o)).and_then(|raw| find_state_payload(&raw, query)) {
            Some(state) => self.run_chrome_script(&format!(
                "if(window.hiwaveChrome) {{ hiwaveChrome.updateFindState({state}); }}"
            )),
            None => debug!("find_in_page produced no result"),
        }
    }

    /// Collect a content-thread find's answer if it has arrived. Never waits.
    fn poll_find(&self) {
        use tokio::sync::oneshot::error::TryRecvError;
        let outcome = {
            let mut slot = self.pending_find.borrow_mut();
            let Some((rx, query)) = slot.as_mut() else {
                return;
            };
            match rx.try_recv() {
                Ok(answer) => {
                    let query = query.clone();
                    *slot = None;
                    Some((answer, query))
                }
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Closed) => {
                    *slot = None;
                    None
                }
            }
        };
        if let Some((answer, query)) = outcome {
            self.show_find_result(answer.ok(), &query);
        }
    }

    /// A command on the shared app state (history, downloads). These are the calls the
    /// WebView2 shell's handlers make; its `ipc` module is not built here (it needs the
    /// WebView platform layer), so the two that change state are made directly.
    fn app_data_command(&self, cmd: &str) {
        let Ok(mut state) = self.app_state.lock() else {
            return;
        };
        let script = match cmd {
            "clear_visit_history" => {
                if let Err(e) = state.clear_visit_history() {
                    warn!(error = %e, "Failed to clear the history");
                }
                chrome_data::history_script(&state)
            }
            "clear_downloads" => {
                state.clear_download_history();
                if let Err(e) = state.save_workspace_state() {
                    warn!(error = %e, "Failed to persist the download history");
                }
                chrome_data::downloads_script(&state)
            }
            "get_visit_history" => chrome_data::history_script(&state),
            _ => chrome_data::downloads_script(&state),
        };
        drop(state);
        self.run_chrome_script(&script);
    }

    /// Write the active page to the history when `VisitTracker` says it is a new visit.
    fn record_visit_if_new(&mut self, tab: Option<u64>, url: &str, title: &str, loading: bool) {
        if !self.visits.should_record(tab, url, title, loading) {
            return;
        }
        if let Ok(mut state) = self.app_state.lock() {
            if let Err(e) = state.record_visit(url, Some(title.to_string()), None) {
                warn!(error = %e, "Failed to write the history entry");
            }
        }
    }

    /// Run a script in the chrome page.
    fn run_chrome_script(&self, script: &str) {
        let Some(&chrome_id) = self.views.get(&ViewType::Chrome) else {
            return;
        };
        if let Err(e) = self.engine.borrow_mut().execute_script(chrome_id, script) {
            debug!(error = %e, "Chrome script failed");
        }
    }

    /// The tab id the page uses for `view`, assigned on first sight.
    fn tab_id_for(&mut self, view: EngineViewId) -> u64 {
        let next = &mut self.next_tab_id;
        *self.tab_ids.entry(view).or_insert_with(|| {
            let id = *next;
            *next += 1;
            id
        })
    }

    /// The position in the tab strip of the tab the page calls `id`.
    fn tab_index_for_id(&self, id: u64) -> Option<usize> {
        if let Some(tabs) = self.page_tabs.as_ref() {
            return tabs.views().iter().position(|page| *page == id);
        }
        let tabs = self.tabs.as_ref()?;
        tabs.views()
            .iter()
            .position(|view| self.tab_ids.get(view) == Some(&id))
    }

    /// Close the tab the page calls `id`, active or not.
    fn close_tab_by_id(&mut self, id: u64) {
        let Some(index) = self.tab_index_for_id(id) else {
            return;
        };
        let active = self.tabs.as_ref().map(|t| t.active_index());
        if active != Some(index) {
            // Closing is defined on the active tab: make it the active one first.
            self.activate_tab_by_index(index);
        }
        self.close_active_tab();
    }

    /// Minimise, maximise/restore or close the main window.
    fn window_command(&self, cmd: &str) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            IsZoomed, PostMessageW, ShowWindow, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE, WM_CLOSE,
        };
        let Some(hwnd) = self.viewhost.get_main_hwnd() else {
            return;
        };
        // SAFETY: `hwnd` is the live main window this browser created.
        unsafe {
            match cmd {
                "window_close" => {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                }
                "window_minimize" => {
                    let _ = ShowWindow(hwnd, SW_MINIMIZE);
                }
                _ => {
                    let _ = ShowWindow(hwnd, if IsZoomed(hwnd).as_bool() { SW_RESTORE } else { SW_MAXIMIZE });
                }
            }
        }
    }

    /// Tell the chrome page what changed: the tab strip, the address bar, the
    /// page title and the back/forward buttons. Compares against what it was
    /// last told and does nothing when it is current.
    fn sync_chrome(&mut self) {
        if !self.chrome_ready {
            return;
        }
        if let Some(tabs) = self.page_tabs.as_ref() {
            let active = tabs.active_view();
            let rows: Vec<TabRow> = tabs
                .views()
                .iter()
                .filter_map(|id| self.pages.get(id).map(|page| (*id, page.state())))
                .map(|(id, state)| TabRow {
                    id,
                    title: state.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                    url: state.url.clone().unwrap_or_default(),
                    active: id == active,
                })
                .collect();
            let state = self.pages.get(&active).map(|page| page.state()).unwrap_or_default();
            let now = ChromeSnapshot {
                url: state.url.clone().unwrap_or_default(),
                title: state.title.clone().unwrap_or_else(|| "New Tab".to_string()),
                tabs: rows,
                can_go_back: state.can_go_back,
                can_go_forward: state.can_go_forward,
                shield_enabled: self.shield_enabled,
                shield_blocked: self.shield.blocked_count(),
                loading: state.loading,
            };
            self.push_snapshot(now);
            return;
        }
        let Some(tabs) = self.tabs.as_ref() else {
            return;
        };
        let views: Vec<EngineViewId> = tabs.views().to_vec();
        let active = tabs.active_view();

        let mut rows = Vec::with_capacity(views.len());
        let (mut url, mut title, mut can_go_back, mut can_go_forward) =
            (String::new(), String::new(), false, false);
        {
            let engine = self.engine.borrow();
            for &view in &views {
                let page_url = engine.get_url(view).map(|u| u.to_string()).unwrap_or_default();
                let page_title = engine
                    .get_title(view)
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .unwrap_or_else(|| "New Tab".to_string());
                if view == active {
                    title = page_title.clone();
                    can_go_back = engine.can_go_back(view);
                    can_go_forward = engine.can_go_forward(view);
                }
                // The start page is loaded from a string, so the engine calls it
                // about:blank; the page's own name for it is hiwave://about.
                let page_url = if page_url == "about:blank" && page_title == ABOUT_TITLE {
                    "hiwave://about".to_string()
                } else {
                    page_url
                };
                if view == active {
                    url = page_url.clone();
                }
                rows.push((view, page_title, page_url));
            }
        }
        let rows: Vec<TabRow> = rows
            .into_iter()
            .map(|(view, title, url)| TabRow {
                id: self.tab_id_for(view),
                title,
                url,
                active: view == active,
            })
            .collect();

        let now = ChromeSnapshot {
            tabs: rows,
            url,
            title,
            can_go_back,
            can_go_forward,
            shield_enabled: self.shield_enabled,
            shield_blocked: self.shield.blocked_count(),
            loading: false,
        };
        self.push_snapshot(now);
    }

    /// Tell the chrome what differs between `now` and what it was last told.
    fn push_snapshot(&mut self, now: ChromeSnapshot) {
        let active_tab = now.tabs.iter().find(|r| r.active).map(|r| r.id);
        self.record_visit_if_new(active_tab, &now.url, &now.title, now.loading);
        if self.chrome_synced.as_ref() == Some(&now) {
            return;
        }
        let before = self.chrome_synced.clone();
        let mut script = String::from("if(window.hiwaveChrome){");
        if before.as_ref().map(|b| &b.tabs) != Some(&now.tabs) {
            let tabs_json: Vec<serde_json::Value> = now
                .tabs
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "title": r.title,
                        "url": r.url,
                        "is_active": r.active,
                        "locked": false,
                        "last_visited": 0,
                        "decay_level": 0
                    })
                })
                .collect();
            script.push_str(&format!("hiwaveChrome.renderTabs({});", serde_json::Value::Array(tabs_json)));
        }
        if before.as_ref().map(|b| &b.url) != Some(&now.url) {
            script.push_str(&format!("hiwaveChrome.updateUrl({});", serde_json::Value::String(now.url.clone())));
        }
        if before.as_ref().map(|b| &b.title) != Some(&now.title) {
            script.push_str(&format!("hiwaveChrome.updateTitle({});", serde_json::Value::String(now.title.clone())));
        }
        if before.as_ref().map(|b| (b.can_go_back, b.can_go_forward)) != Some((now.can_go_back, now.can_go_forward)) {
            script.push_str(&format!("hiwaveChrome.updateNavState({}, {});", now.can_go_back, now.can_go_forward));
        }
        if before.as_ref().map(|b| (b.shield_enabled, b.shield_blocked)) != Some((now.shield_enabled, now.shield_blocked)) {
            // The chrome adds the two counts together, so the filter's one count goes
            // in as requests and trackers stays 0 (it does not tell them apart).
            script.push_str(&format!(
                "hiwaveChrome.updateShieldStats({{\"enabled\":{},\"requests_blocked\":{},\"trackers_blocked\":0}});",
                now.shield_enabled, now.shield_blocked
            ));
        }
        if before.as_ref().map(|b| b.loading) != Some(now.loading) {
            script.push_str(&format!("hiwaveChrome.setLoading({});", now.loading));
        }
        script.push('}');
        self.run_chrome_script(&script);
        self.chrome_synced = Some(now);
    }

    fn traverse(&self, direction: &str) {
        if let Some(page) = self.page() {
            match direction {
                "back" => page.go_back(),
                "forward" => page.go_forward(),
                "reload" => page.reload(),
                _ => {}
            }
            return;
        }
        let Some(&content_id) = self.views.get(&ViewType::Content) else {
            return;
        };
        let mut engine = self.engine.borrow_mut();
        self.runtime.block_on(async {
            let result = match direction {
                "back" => engine.go_back(content_id).await,
                "forward" => engine.go_forward(content_id).await,
                "reload" => engine.reload(content_id).await,
                _ => return,
            };
            match result {
                Ok(true) => info!(direction, "History traversal complete"),
                // Nowhere to go is a no-op, not an error — mashing Back on
                // the first page must stay silent.
                Ok(false) => debug!(direction, "History traversal: nothing to do"),
                Err(e) => error!(error = %e, direction, "History traversal failed"),
            }
        });
        self.live_clocks.borrow_mut().insert(content_id, Instant::now());
    }

    pub fn navigate(&self, url: &str) {
        // The address bar sends what was typed; turn it into a URL (a host gets
        // https://, other text is a search) the way the WebView2 shell does.
        let Some(url) = super::address::resolve(url) else {
            debug!(input = url, "Nothing to navigate to");
            return;
        };
        if let Some(page) = self.page() {
            match url::Url::parse(&url) {
                Ok(parsed) => page.navigate(parsed),
                Err(e) => warn!(error = %e, url, "Invalid URL"),
            }
            return;
        }
        if let Some(&content_id) = self.views.get(&ViewType::Content) {
            self.navigate_view(content_id, &url);
        }
    }

    fn navigate_view(&self, content_id: EngineViewId, url: &str) {
        match url::Url::parse(url) {
            Ok(parsed) => {
                let mut engine = self.engine.borrow_mut();
                self.runtime.block_on(async {
                    if let Err(e) = engine.load_url(content_id, parsed).await {
                        error!(error = %e, url, "Failed to navigate");
                    }
                });
                self.live_clocks.borrow_mut().insert(content_id, Instant::now());
            }
            Err(e) => {
                warn!(error = %e, url, "Invalid URL");
            }
        }
    }

    /// Advance all documents, including background tabs, before dispatching
    /// input so a timer created by a click starts at the click's time.
    fn pump_live_views(&self) {
        let mut navigations = Vec::new();
        {
            let mut engine = self.engine.borrow_mut();
            let mut clocks = self.live_clocks.borrow_mut();
            clocks.retain(|id, _| self.engine_view_types.contains_key(id));
            for &id in self.engine_view_types.keys() {
                let now = Instant::now();
                let clock = clocks.entry(id).or_insert(now);
                let elapsed_ms = now.duration_since(*clock).as_millis() as u64;
                *clock += Duration::from_millis(elapsed_ms);
                self.runtime.block_on(engine.pump_live(id, elapsed_ms));
                if let Some(url) = engine.take_script_navigation(id) {
                    navigations.push((id, url));
                }
            }
        }
        for (id, url) in navigations {
            self.navigate_view(id, &url);
        }
    }

    /// Execute JavaScript in a view.
    #[allow(dead_code)]
    pub fn execute_script(&self, view_type: ViewType, script: &str) -> Option<String> {
        if let Some(&view_id) = self.views.get(&view_type) {
            let mut engine = self.engine.borrow_mut();
            match engine.execute_script(view_id, script) {
                Ok(result) => Some(result),
                Err(e) => {
                    debug!(error = %e, "Script execution failed");
                    None
                }
            }
        } else {
            None
        }
    }

    /// Run the browser's main message loop with IPC processing.
    pub fn run(&mut self) {
        info!("Starting native browser message loop");

        loop {
            // Process Win32 messages (non-blocking)
            if !self.viewhost.pump_messages() {
                // WM_QUIT received, exit the loop
                break;
            }

            // Follow the window if it was resized or maximised
            self.sync_window_size();

            // Keep keyboard focus on a view
            self.restore_focus();

            self.pump_live_views();

            // Process any IPC messages from views
            self.process_ipc_messages();

            // Run any keyboard shortcuts pressed since the last frame
            self.process_input_events();

            // Take what the content-thread page published and draw it
            for page in self.pages.values() {
                page.pump();
            }
            self.poll_find();

            // Keep the tab strip, address bar and nav buttons current
            self.sync_chrome();

            self.pump_live_views();
            self.engine.borrow_mut().render_changed_views();

            // Small sleep to prevent busy-waiting (target ~60fps)
            std::thread::sleep(std::time::Duration::from_millis(16));
        }

        info!("Browser message loop ended");
    }

    /// Process IPC messages from all views.
    fn process_ipc_messages(&mut self) {
        let messages = self.engine.borrow().drain_ipc_messages();

        for (view_id, ipc_msg) in messages {
            let view_type = self.engine_view_types.get(&view_id).copied();
            self.handle_ipc_message(view_type, ipc_msg);
        }
    }

    /// Handle a single IPC message.
    fn handle_ipc_message(&mut self, view_type: Option<ViewType>, msg: IpcMessage) {
        // Parse the JSON payload
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&msg.payload);
        let json = match parsed {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, payload_len = msg.payload.len(), "Failed to parse IPC message JSON");
                return;
            }
        };

        // Extract the command
        let cmd = match json.get("cmd").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                trace!("IPC message missing 'cmd' field (likely data payload)");
                return;
            }
        };

        // Log at trace level for high-frequency commands, debug for others
        match cmd {
            // High-frequency, log at trace only
            "log" | "sync_tabs" | "sync_workspaces" | "sync_downloads" => {
                trace!(?view_type, cmd, "IPC");
            }
            // Low-frequency, log at debug
            _ => {
                debug!(?view_type, cmd, "IPC command");
            }
        }

        // Handle the command
        match cmd {
            "navigate" => {
                if let Some(url) = json.get("url").and_then(|v| v.as_str()) {
                    info!(url, "Navigation started");
                    self.navigate(url);
                }
            }
            "go_back" => {
                self.traverse("back");
            }
            "go_forward" => {
                self.traverse("forward");
            }
            "new_tab" => self.new_tab(),
            "close_active_tab" => self.close_active_tab(),
            "activate_tab_by_index" => {
                match json.get("index").and_then(|v| v.as_u64()) {
                    Some(i) => self.activate_tab_by_index(i as usize),
                    None => warn!("activate_tab_by_index without an index"),
                }
            }
            "reload" => {
                self.traverse("reload");
            }
            "stop" => {
                self.stop_loading();
            }
            "expand_chrome" => {
                trace!("Layout: chrome expanded");
                self.expand_chrome();
            }
            "collapse_chrome" => {
                trace!("Layout: chrome collapsed");
                self.collapse_chrome();
            }
            "expand_shelf" | "open_command_palette" => {
                debug!("Shelf opened");
                self.expand_shelf();
            }
            "collapse_shelf" | "close_command_palette" => {
                trace!("Shelf closed");
                self.collapse_shelf();
            }
            "toggle_sidebar" => {
                debug!(open = self.sidebar_width == 0, "Sidebar toggled");
                self.toggle_sidebar();
            }
            // chrome.html owns the sidebar's state and tells the shell, so the
            // content view can make room for it (same messages the WebView2
            // shell handles).
            "set_sidebar_open" => {
                if let Some(open) = json.get("open").and_then(|v| v.as_bool()) {
                    self.set_sidebar_open(open);
                }
            }
            "set_sidebar_width" => {
                if let Some(width) = json.get("width").and_then(|v| v.as_f64()) {
                    self.set_sidebar_width(width.max(0.0) as u32);
                }
            }
            "set_right_panel_open" => {
                if let Some(open) = json.get("open").and_then(|v| v.as_bool()) {
                    self.set_right_panel_open(open);
                }
            }
            "chrome_ready" => {
                info!("Chrome UI initialized and ready");
                self.chrome_ready = true;
                // Whatever the page had is gone with its load: tell it everything.
                self.chrome_synced = None;
            }
            // The tab strip asks for its tabs; forget what it was told so the next
            // sync sends them all.
            "get_tabs" => self.chrome_synced = None,
            "create_tab" => {
                self.new_tab();
                if let Some(url) = json.get("url").and_then(|v| v.as_str()) {
                    if !url.is_empty() {
                        self.navigate(url);
                    }
                }
            }
            "close_tab" => match tab_id_of(&json) {
                Some(id) => self.close_tab_by_id(id),
                None => warn!("close_tab without an id"),
            },
            "activate_tab" => match tab_id_of(&json) {
                Some(id) => {
                    if let Some(index) = self.tab_index_for_id(id) {
                        self.activate_tab_by_index(index);
                    }
                }
                None => warn!("activate_tab without an id"),
            },
            // The chrome draws its own title-bar buttons.
            "window_close" | "window_minimize" | "window_toggle_maximize" => {
                self.window_command(cmd);
            }
            // The page keeps its sidebar width; answer with the default so it
            // starts from a known width (nothing persists it in this shell yet).
            "get_sidebar_width" => {
                let script = format!(
                    "if(window.hiwaveChrome && window.hiwaveChrome.setSidebarWidth) {{ window.hiwaveChrome.setSidebarWidth({}); }}",
                    SIDEBAR_WIDTH
                );
                self.run_chrome_script(&script);
            }
            "save_sidebar_width" => {}
            // The panels' data lives in the shared app state: run the shared handler,
            // then send the panel its new data.
            "get_visit_history" | "clear_visit_history" | "get_downloads" | "clear_downloads" => {
                self.app_data_command(cmd);
            }
            "find_in_page" => {
                let query = json.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let case_sensitive = json.get("case_sensitive").and_then(|v| v.as_bool()).unwrap_or(false);
                let direction = json.get("direction").and_then(|v| v.as_str()).unwrap_or("next");
                self.find_in_page(query, case_sensitive, direction);
            }
            // The shield button: the chrome tracks its own state and says what it is now.
            "toggle_shield" => {
                if let Some(enabled) = json.get("enabled").and_then(|v| v.as_bool()) {
                    self.shield.set_enabled(enabled);
                    self.shield_enabled = enabled;
                }
            }
            "log" => {
                // JavaScript console log forwarding
                let level = json.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("");
                match level {
                    "error" => error!(source = "js", "{}", message),
                    "warn" => warn!(source = "js", "{}", message),
                    "debug" => trace!(source = "js", "{}", message), // Downgrade JS debug to trace
                    _ => trace!(source = "js", "{}", message), // JS info → trace (very frequent)
                }
            }
            _ => {
                trace!(cmd, "Unhandled IPC command (may be hybrid-only)");
            }
        }
    }
}

impl Drop for NativeBrowser {
    fn drop(&mut self) {
        info!("Shutting down native browser");

        // Destroy all views
        let mut engine = self.engine.borrow_mut();
        for (&view_type, &view_id) in &self.views {
            if let Err(e) = engine.destroy_view(view_id) {
                warn!(?view_type, error = %e, "Failed to destroy view");
            }
        }
    }
}

/// The text of a script result. The engine reports a string as `String("...")`.
fn script_string(result: &str) -> Option<String> {
    let inner = result.strip_prefix("String(")?.strip_suffix(')')?;
    serde_json::from_str::<String>(inner).ok().filter(|s| s != "null")
}

/// What a key event means for a text editor, before anyone knows who has the focus.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeyAction {
    /// Not ours to act on (key-up): leave it for the generic path.
    Pass,
    /// Nothing to do, and it must not reach the generic path either.
    Ignore,
    /// Send this virtual key and text to the editor.
    Send(u32, String),
}

/// Same split as the macOS app: editing and navigation keys go as a virtual key
/// with no text, typed characters as text with no key, and Ctrl+A/C/V/X/Z as the
/// letter with Ctrl set. Printable keys arrive twice on Win32 (WM_KEYDOWN and then
/// WM_CHAR); only the WM_CHAR half types.
fn key_action(key: &KeyEvent) -> KeyAction {
    match key.event_type {
        KeyEventType::KeyUp => KeyAction::Pass,
        KeyEventType::KeyDown => {
            let vk = key.key_code as u32;
            let m = key.modifiers;
            match vk {
                // Backspace, Tab, Enter, Escape, PgUp..Down, End, Home, arrows, Delete
                0x08 | 0x09 | 0x0D | 0x1B | 0x21..=0x28 | 0x2E => KeyAction::Send(vk, String::new()),
                // Ctrl+A, C, V, X, Z
                0x41 | 0x43 | 0x56 | 0x58 | 0x5A if m.ctrl && !m.alt => {
                    KeyAction::Send(0, ((vk as u8 + 32) as char).to_string())
                }
                // A printable key: its WM_CHAR types it.
                _ => KeyAction::Ignore,
            }
        }
        KeyEventType::Input => {
            // Tab and Enter also come as WM_CHAR; their key-down already acted.
            if key.key.is_empty() || key.key == "\r" || key.key == "\t" {
                KeyAction::Ignore
            } else {
                KeyAction::Send(0, key.key.clone())
            }
        }
    }
}

/// What the chrome's find bar is told, built from the find helper's answer.
///
/// The helper runs inside the page, so a page can make it return anything. This
/// parses the answer and keeps only the two numbers, then re-serialises them with
/// `query` (the text the chrome sent, not anything the page said). The result is
/// plain JSON data; no page-supplied text can become script in the chrome.
fn find_state_payload(raw: &str, query: &str) -> Option<serde_json::Value> {
    let answer: serde_json::Value = serde_json::from_str(raw).ok()?;
    let count = answer.get("count")?.as_i64()?.clamp(0, 1_000_000);
    let index = answer.get("index")?.as_i64()?.clamp(-1, 1_000_000);
    Some(serde_json::json!({ "count": count, "index": index, "query": query }))
}

/// The engine settings every page engine here uses (the chrome's, and the content
/// thread's when it is on).
fn engine_builder(shield: &Arc<crate::shield_adapter::ShieldInterceptHandler>) -> EngineBuilder {
    let mut interceptor = rustkit_net::RequestInterceptor::new();
    interceptor.add_handler(shield.clone());
    EngineBuilder::new()
        .user_agent(
            // Finding 4 from the macOS live demo, ported: the bare
            // "HiWave/1.0 RustKit/1.0" UA is scraper-shaped — Wikimedia
            // 429'd it and eBay challenged it. Safari-shaped with an
            // HONEST Windows platform string plus the HiWave token: sites
            // get a recognisable engine family, and we stay identifiable
            // rather than impersonating Chrome outright.
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/605.1.15                  (KHTML, like Gecko) Version/17.4 Safari/605.1.15 HiWave/1.0",
        )
        .javascript_enabled(true)
        .cookies_enabled(true)
        .script_budget_ms(script_budget_ms())
        .interrupt_scripts_at_budget(true)
        .request_interceptor(interceptor)
}

/// The tab id in a chrome message. The page sends it as a number or a string.
fn tab_id_of(json: &serde_json::Value) -> Option<u64> {
    let id = json.get("id")?;
    id.as_u64().or_else(|| id.as_str().and_then(|s| s.parse().ok()))
}

/// Send a view's window to the bottom of its siblings' z-order.
fn lower_view(engine: &Engine, id: EngineViewId) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let Ok(hwnd) = engine.get_view_hwnd(id) else {
        return;
    };
    // SAFETY: `hwnd` is a live child window owned by the engine; no size or
    // position is touched, only the z-order.
    let _ = unsafe {
        SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)
    };
}

/// How long a page's scripts may run, in milliseconds.
///
/// Pages load on the window thread in this shell, so every millisecond a script
/// runs is a millisecond the window is frozen. 5 s is the parity board's own
/// budget. (macOS lets pages run for 60 s because its content thread keeps the
/// window alive; this shell has no such thread yet.) `HIWAVE_SCRIPT_BUDGET_MS`
/// overrides it.
fn script_budget_ms() -> u64 {
    parse_budget(std::env::var("HIWAVE_SCRIPT_BUDGET_MS").ok().as_deref())
}

fn parse_budget(value: Option<&str>) -> u64 {
    const DEFAULT_MS: u64 = 5_000;
    value
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&ms| ms > 0)
        .unwrap_or(DEFAULT_MS)
}

/// A layout rectangle as the engine's view bounds.
fn to_bounds(r: layout::Rect) -> Bounds {
    Bounds::new(r.x, r.y, r.width, r.height)
}

/// Print startup banner with build info.
fn print_startup_banner() {
    info!("=== HiWave Browser ===");
    info!(
        version = env!("CARGO_PKG_VERSION"),
        engine = "RustKit",
        mode = "native-win32",
        "Startup"
    );
    info!(
        target_os = std::env::consts::OS,
        target_arch = std::env::consts::ARCH,
        "Platform"
    );
    
    // Get DPI scale if available
    #[cfg(windows)]
    {
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        let dpi = unsafe { GetDpiForSystem() };
        let scale = dpi as f64 / 96.0;
        info!(dpi, scale, "Display");
    }
}

/// Install panic hook for structured crash logging.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Log the panic with tracing
        let location = panic_info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let message = panic_info.payload().downcast_ref::<&str>().map(|s| *s)
            .or_else(|| panic_info.payload().downcast_ref::<String>().map(|s| s.as_str()));
        
        error!(
            location = location.as_deref(),
            message = message,
            "PANIC - Browser crashed"
        );
        
        // Call the default hook for normal panic behavior
        default_hook(panic_info);
    }));
}

/// Entry point for native Win32 mode.
///
/// This function is called from main.rs when the `native-win32` feature is enabled.
pub fn run_native() -> Result<(), String> {
    install_panic_hook();
    print_startup_banner();

    // Check for screenshot test mode
    if let Some(config) = super::screenshot_harness::parse_screenshot_args() {
        info!("Running in screenshot test mode");
        return run_screenshot_mode(config);
    }

    let mut browser = NativeBrowser::new()?;
    browser.init()?;
    browser.run();

    info!("HiWave shutdown complete");
    Ok(())
}

/// Run in screenshot test mode.
fn run_screenshot_mode(config: super::screenshot_harness::ScreenshotConfig) -> Result<(), String> {
    use super::screenshot_harness::{get_test_scene_html, CaptureMetadata, GpuCaptureInfo, OsCaptureInfo, RenderStatsInfo};
    use std::fs;
    
    info!(
        output_dir = %config.output_dir.display(),
        scene = ?config.scene,
        url = ?config.url,
        frames = config.wait_frames,
        "Screenshot test configuration"
    );
    
    // Create output directory
    fs::create_dir_all(&config.output_dir)
        .map_err(|e| format!("Failed to create output directory: {}", e))?;
    
    // Create browser instance
    let mut browser = NativeBrowser::new()?;
    browser.init()?;
    
    // Determine what content to load
    let scene_name = if let Some(ref scene) = config.scene {
        // Load test scene HTML
        if let Some(html) = get_test_scene_html(scene) {
            info!(scene, "Loading test scene");
            if let Some(&content_id) = browser.views.get(&ViewType::Content) {
                browser.engine.borrow_mut()
                    .load_html(content_id, html)
                    .map_err(|e| format!("Failed to load scene HTML: {}", e))?;
            }
            scene.clone()
        } else {
            return Err(format!("Unknown test scene: {}. Available: {:?}", 
                scene, super::screenshot_harness::list_test_scenes()));
        }
    } else if let Some(ref url) = config.url {
        info!(url, "Loading URL");
        browser.navigate(url);
        url.replace(['/', ':', '.'], "_")
    } else {
        // Default to about page
        "about".to_string()
    };
    
    // Wait for frames to render
    info!(frames = config.wait_frames, "Rendering frames");
    for frame in 0..config.wait_frames {
        // Process messages and render
        browser.viewhost.pump_messages();
        browser.pump_live_views();
        browser.process_ipc_messages();
        browser.process_input_events();
        browser.engine.borrow_mut().render_changed_views();
        
        debug!(frame, "Frame rendered");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    
    // One more render pass to ensure content is ready
    browser.viewhost.pump_messages();
    browser.engine.borrow_mut().render_all_views();
    
    // Capture screenshots
    let timestamp = chrono_lite_timestamp();
    let mut gpu_captures: Vec<GpuCaptureInfo> = Vec::new();
    let mut os_captures: Vec<OsCaptureInfo> = Vec::new();
    
    // Get render stats
    let stats = browser.engine.borrow().get_render_stats();
    let render_stats = Some(RenderStatsInfo {
        color_vertex_count: stats.color_vertex_count,
        texture_vertex_count: stats.texture_vertex_count,
        color_index_count: stats.color_index_count,
        texture_index_count: stats.texture_index_count,
    });
    
    // GPU readback capture (per-view)
    if config.gpu_capture {
        for (view_type, &view_id) in browser.views.iter() {
            let view_name = match view_type {
                ViewType::Chrome => "chrome",
                ViewType::Content => "content",
                ViewType::Shelf => "shelf",
            };
            let gpu_path = config
                .output_dir
                .join(format!("{}_gpu_{}.png", scene_name, view_name));
            info!(view = view_name, path = %gpu_path.display(), "Capturing GPU readback screenshot");

            match browser.engine.borrow_mut().capture_view_screenshot(view_id, &gpu_path) {
                Ok(metadata) => {
                    info!(
                        view = view_name,
                        width = metadata.width,
                        height = metadata.height,
                        color_vertices = metadata.color_vertex_count,
                        texture_vertices = metadata.texture_vertex_count,
                        "GPU screenshot captured"
                    );
                    gpu_captures.push(GpuCaptureInfo {
                        view: view_name.to_string(),
                        path: gpu_path.to_string_lossy().to_string(),
                        adapter: metadata.adapter,
                        format: metadata.format,
                    });
                }
                Err(e) => {
                    error!(view = view_name, error = %e, "GPU screenshot capture failed");
                }
            }
        }
    }
    
    // OS window capture (main window + per-view child HWND)
    if config.os_capture {
        // Main window
        let os_main_path = config.output_dir.join(format!("{}_os_main.png", scene_name));
        info!(path = %os_main_path.display(), "Capturing OS window screenshot (main)");
        if let Some(hwnd) = browser.viewhost.get_main_hwnd() {
            match super::screenshot_harness::capture_os_window(hwnd, &os_main_path) {
                Ok((width, height)) => {
                    info!(width, height, "OS screenshot captured (main)");
                    os_captures.push(OsCaptureInfo {
                        view: "main".to_string(),
                        path: os_main_path.to_string_lossy().to_string(),
                        capture_method: "BitBlt".to_string(),
                    });
                }
                Err(e) => {
                    error!(error = %e, "OS screenshot capture failed (main)");
                }
            }
        }

        // Per-view child windows
        for (view_type, &view_id) in browser.views.iter() {
            let view_name = match view_type {
                ViewType::Chrome => "chrome",
                ViewType::Content => "content",
                ViewType::Shelf => "shelf",
            };
            let os_path = config
                .output_dir
                .join(format!("{}_os_{}.png", scene_name, view_name));
            info!(view = view_name, path = %os_path.display(), "Capturing OS window screenshot (view)");

            let hwnd = match browser.engine.borrow().get_view_hwnd(view_id) {
                Ok(h) => h,
                Err(e) => {
                    error!(view = view_name, error = %e, "Failed to get view HWND");
                    continue;
                }
            };

            match super::screenshot_harness::capture_os_window(hwnd, &os_path) {
                Ok((width, height)) => {
                    info!(view = view_name, width, height, "OS screenshot captured (view)");
                    os_captures.push(OsCaptureInfo {
                        view: view_name.to_string(),
                        path: os_path.to_string_lossy().to_string(),
                        capture_method: "BitBlt".to_string(),
                    });
                }
                Err(e) => {
                    error!(view = view_name, error = %e, "OS screenshot capture failed (view)");
                }
            }
        }
    }
    
    // Save metadata
    let metadata = CaptureMetadata {
        timestamp,
        scene: scene_name.clone(),
        width: browser.window_width,
        height: browser.window_height,
        wait_frames: config.wait_frames,
        gpu_captures,
        os_captures,
        render_stats,
    };
    
    let metadata_path = config.output_dir.join(format!("{}_metadata.json", scene_name));
    let json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
    fs::write(&metadata_path, json)
        .map_err(|e| format!("Failed to write metadata: {}", e))?;
    
    info!(path = %metadata_path.display(), "Metadata written");
    info!("Screenshot capture complete");
    
    Ok(())
}

/// Simple timestamp without chrono dependency.
fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    super::screenshot_harness::utc_timestamp_from_secs(secs)
}

#[cfg(test)]
mod chrome_message_tests {
    use super::{find_state_payload, key_action, parse_budget, tab_id_of, KeyAction};
    use rustkit_core::input::{KeyCode, KeyEvent, KeyEventType, Modifiers};
    use serde_json::json;

    #[test]
    fn a_tab_id_may_be_a_number_or_a_string() {
        assert_eq!(tab_id_of(&json!({"cmd": "close_tab", "id": 7})), Some(7));
        assert_eq!(tab_id_of(&json!({"cmd": "close_tab", "id": "12"})), Some(12));
    }

    #[test]
    fn a_missing_or_malformed_tab_id_is_none() {
        assert_eq!(tab_id_of(&json!({"cmd": "close_tab"})), None);
        assert_eq!(tab_id_of(&json!({"id": "not a number"})), None);
        assert_eq!(tab_id_of(&json!({"id": -3})), None);
    }

    #[test]
    fn the_script_budget_defaults_to_five_seconds_and_can_be_overridden() {
        assert_eq!(parse_budget(None), 5_000);
        assert_eq!(parse_budget(Some("15000")), 15_000);
        assert_eq!(parse_budget(Some(" 2500 ")), 2_500);
    }

    #[test]
    fn a_bad_script_budget_falls_back_to_the_default() {
        assert_eq!(parse_budget(Some("")), 5_000);
        assert_eq!(parse_budget(Some("abc")), 5_000);
        assert_eq!(parse_budget(Some("0")), 5_000);
        assert_eq!(parse_budget(Some("-1")), 5_000);
    }

    #[test]
    fn a_find_answer_becomes_only_two_numbers_and_the_chromes_own_query() {
        let state = find_state_payload(r#"{"count":4,"index":0,"query":"needle"}"#, "needle").unwrap();
        assert_eq!(state, json!({"count": 4, "index": 0, "query": "needle"}));
    }

    #[test]
    fn a_hostile_find_answer_cannot_put_page_text_into_the_chrome() {
        // A page can make the helper say anything. Extra fields are dropped and the
        // page's own "query" is ignored in favour of the one the chrome sent.
        let hostile = r#"{"count":2,"index":1,"query":"1);window.ipc.postMessage('x')//","evil":"</script>"}"#;
        let state = find_state_payload(hostile, "safe").unwrap();
        assert_eq!(state, json!({"count": 2, "index": 1, "query": "safe"}));
        let rendered = state.to_string();
        assert!(!rendered.contains("ipc") && !rendered.contains("evil"), "{rendered}");
    }

    #[test]
    fn a_find_answer_that_is_not_two_numbers_is_rejected() {
        assert!(find_state_payload("1);alert(1)//", "q").is_none());
        assert!(find_state_payload(r#"{"count":"1);x","index":0}"#, "q").is_none());
        assert!(find_state_payload(r#"{"count":3}"#, "q").is_none());
    }

    #[test]
    fn find_counts_are_clamped() {
        let state = find_state_payload(r#"{"count":-5,"index":99999999999}"#, "q").unwrap();
        assert_eq!(state, json!({"count": 0, "index": 1_000_000, "query": "q"}));
    }

    fn down(code: KeyCode, ctrl: bool) -> KeyEvent {
        KeyEvent::new(KeyEventType::KeyDown, code, Modifiers { ctrl, ..Default::default() })
    }

    #[test]
    fn editing_keys_go_as_a_virtual_key_with_no_text() {
        assert_eq!(key_action(&down(KeyCode::Backspace, false)), KeyAction::Send(0x08, String::new()));
        assert_eq!(key_action(&down(KeyCode::ArrowLeft, false)), KeyAction::Send(0x25, String::new()));
        assert_eq!(key_action(&down(KeyCode::Enter, false)), KeyAction::Send(0x0D, String::new()));
    }

    #[test]
    fn ctrl_a_c_v_x_z_go_as_the_lowercase_letter() {
        assert_eq!(key_action(&down(KeyCode::KeyA, true)), KeyAction::Send(0, "a".to_string()));
        assert_eq!(key_action(&down(KeyCode::KeyV, true)), KeyAction::Send(0, "v".to_string()));
    }

    #[test]
    fn a_printable_key_down_is_left_to_its_char_message() {
        assert_eq!(key_action(&down(KeyCode::KeyA, false)), KeyAction::Ignore);
        assert_eq!(key_action(&down(KeyCode::KeyQ, true)), KeyAction::Ignore, "Ctrl+Q is not an editing chord");
    }

    #[test]
    fn a_typed_character_goes_as_text_but_tab_and_enter_chars_do_not() {
        assert_eq!(key_action(&KeyEvent::input('x')), KeyAction::Send(0, "x".to_string()));
        assert_eq!(key_action(&KeyEvent::input('\r')), KeyAction::Ignore);
        assert_eq!(key_action(&KeyEvent::input('\t')), KeyAction::Ignore);
    }

    #[test]
    fn a_key_up_is_passed_on() {
        assert_eq!(key_action(&KeyEvent::new(KeyEventType::KeyUp, KeyCode::KeyA, Modifiers::default())), KeyAction::Pass);
    }
}
