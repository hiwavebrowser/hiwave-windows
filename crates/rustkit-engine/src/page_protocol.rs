//! The page/window protocol (threads plan M4, protocol PR).
//!
//! `PageHandle` is what the window side holds. It sends [`PageCmd`] and
//! receives [`PageEvent`], including a [`FrameCommit`] when the page's
//! frame changes. [`PageEndpoint`] is the page side: it owns the [`Engine`]
//! and applies commands.
//!
//! This PR runs both ends on the calling thread. `drain` applies every
//! command already queued before it returns, so a reply is ready as soon
//! as `drain` returns and nothing about a load, an input or a frame
//! changes. The content thread is a later PR: it moves the endpoint onto
//! its own thread and leaves the handle where it is.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::collections::VecDeque;
use std::sync::Arc;

use rustkit_viewhost::Bounds;
use tokio::sync::oneshot;
use url::Url;

/// A [`PageCmd::DevtoolsEval`] the page reached past its deadline answers
/// this, without running.
pub const EVAL_CANCELLED: &str = "cancelled: the page was busy past the deadline";

use crate::devtools_service::DevtoolsCounters;

use crate::{
    ClickOutcome, DevtoolsConsole, DevtoolsElement, DevtoolsNode, Engine, EngineEvent,
    EngineViewId, FrameCommit, InputMark, LivePump,
};

/// Where a document is in its load, as the page reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadState {
    Started,
    Complete,
    Failed { error: String },
}

/// What the window side asks the page to do.
///
/// Commands that answer something carry a oneshot. On this PR the endpoint
/// sends the answer inside `drain`, on the same thread.
pub enum PageCmd {
    /// Load `url`. `nav_id` is the window side's number for this request
    /// (see [`PageHandle::last_sent_nav_id`]). The engine still assigns the
    /// id the cancel path reads; the handle's counter is the one a later
    /// thread will send.
    Navigate { url: Url, nav_id: u64 },
    /// `nav_id` 0 lets the engine allocate (the same-thread path). A
    /// non-zero id was allocated when the command was sent, and a newer
    /// one already on the counter supersedes this document.
    LoadHtml {
        html: String,
        nav_id: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// Cancel the load in progress. `true` when one was in progress.
    Stop {
        reply: oneshot::Sender<bool>,
    },
    /// Load the document's current URL again.
    Reload {
        nav_id: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Input(PageInput),
    Resize {
        bounds: Bounds,
        /// Reserved for the content thread. The engine sizes from `bounds`.
        scale: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Eval {
        script: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    /// The Inspector console's input line (T6c, R1 condition 5). Reached
    /// past `deadline` (the page was busy and the panel gave up), it is
    /// cancelled, never run late.
    DevtoolsEval {
        script: String,
        deadline: std::time::Instant,
        reply: oneshot::Sender<Result<String, String>>,
    },
    SetVisible {
        visible: bool,
    },
    /// Destroy the view.
    Close,
    /// One live-loop turn.
    PumpLive {
        elapsed_ms: u64,
        reply: oneshot::Sender<LivePump>,
    },
    /// Draw. `every` is `HIWAVE_RENDER_EVERY_WAKE`.
    Render {
        every: bool,
    },
    /// The user made an input (LAT1c): its native event time, sequence
    /// number and kind.
    NoteInput {
        at: std::time::Instant,
        seq: u64,
        kind: crate::InputClass,
    },
    GrabKeyboard,
    Relayout,
    /// CH3.A7: the app owns this view (its chrome, a panel). Sent before
    /// the view's first document; see [`crate::Engine::set_app_owned`].
    SetAppOwned {
        owned: bool,
    },
    /// Test hook: the next JavaScript runtime uses this host re-entry cap.
    /// The frame cap and the operand-stack cap are not changed.
    SetHostReentry {
        limit: usize,
    },
    /// Test hook: install `__rk_reenter` on the current runtime.
    InstallReentryProbe {
        reply: oneshot::Sender<Result<(), String>>,
    },
    /// A developer-tools question (the Inspector, #764). Answered on the
    /// page's own thread, between its script turns, from the document and
    /// layout it last committed. The console's input line is [`PageCmd::Eval`].
    Devtools {
        query: DevtoolsQuery,
        reply: oneshot::Sender<DevtoolsAnswer>,
    },
    /// Test hook: hold the page thread in a host-side wait for this many
    /// ms, as a long script would (T6b: devtools reads are answered
    /// meanwhile).
    #[cfg(test)]
    Block(u64),
    /// Test hook: panic the page thread while it holds the view's
    /// Inspector lock (T6 R1 condition 2).
    #[cfg(test)]
    PanicHoldingInspector,
}

/// What the Inspector asks the page. Node ids are the document's raw
/// `NodeId`s; they do not survive a navigation.
#[derive(Debug, Clone, PartialEq)]
pub enum DevtoolsQuery {
    /// Record the page's console, errors and load events for the panel
    /// (on), or go back to the view's default (off), keeping what was
    /// recorded for the panel's next opening (T6b).
    Attach(bool),
    /// Outline this element on the page, or nothing.
    Highlight(Option<usize>),
    /// The element under viewport point (`x`, `y`), in CSS px.
    NodeAt { x: f32, y: f32 },
    /// The DOM below `root` (the document when `None`), expanded along the
    /// ancestors of `reveal`.
    DomTree { root: Option<usize>, reveal: Option<usize> },
    /// What the panel shows for one element.
    Element(usize),
    /// Console lines recorded since `since`.
    Console { since: u64 },
    /// The current document's requests (T7). Answered by the devtools
    /// thread, like console reads.
    Network,
}

/// The page's answer to a [`DevtoolsQuery`]. `None` and `Unavailable` mean
/// there was nothing to show (no document, no such node, a closed view).
#[derive(Debug, Clone, PartialEq)]
pub enum DevtoolsAnswer {
    Done,
    Node(Option<usize>),
    Tree(Option<DevtoolsNode>),
    Element(Option<DevtoolsElement>),
    Console(DevtoolsConsole),
    Network(crate::DevtoolsNetwork),
    Unavailable,
}

/// Answer one developer-tools question on the thread that owns `engine`.
/// Whether the page's frame changed (an outline was drawn or removed) is
/// the second value.
fn devtools_answer(engine: &mut Engine, view: EngineViewId, query: DevtoolsQuery) -> (DevtoolsAnswer, bool) {
    let answer = match query {
        DevtoolsQuery::Attach(on) => engine.devtools_attach(view, on).map(|_| DevtoolsAnswer::Done),
        DevtoolsQuery::Highlight(node) => {
            return match engine.devtools_highlight(view, node) {
                Ok(changed) => (DevtoolsAnswer::Done, changed),
                Err(_) => (DevtoolsAnswer::Unavailable, false),
            };
        }
        DevtoolsQuery::NodeAt { x, y } => engine.devtools_node_at(view, x, y).map(DevtoolsAnswer::Node),
        DevtoolsQuery::DomTree { root, reveal } => {
            engine.devtools_dom_tree(view, root, reveal).map(DevtoolsAnswer::Tree)
        }
        DevtoolsQuery::Element(node) => engine.devtools_element(view, node).map(DevtoolsAnswer::Element),
        DevtoolsQuery::Console { since } => engine.devtools_console(view, since).map(DevtoolsAnswer::Console),
        DevtoolsQuery::Network => engine.devtools_network(view).map(DevtoolsAnswer::Network),
    };
    (answer.unwrap_or(DevtoolsAnswer::Unavailable), false)
}

/// Whether an answer is worth waking the window for, rather than waiting
/// for its next wake. A console answer with nothing new is not: the panel
/// asks a few times a second on its own clock, and a woken panel used to ask
/// again at once, so an idle page and an open panel woke each other without
/// pause (#785 R1). An acknowledgement nobody reads is not either.
pub(crate) fn devtools_wakes_window(answer: &DevtoolsAnswer, frame_changed: bool) -> bool {
    match answer {
        DevtoolsAnswer::Console(c) => !c.entries.is_empty() || c.page_changed || c.tree_stale,
        DevtoolsAnswer::Done => frame_changed,
        _ => true,
    }
}

/// Input and the synchronous questions the window still asks.
///
/// The ones that return a value keep a oneshot so the window's current
/// callers stay synchronous. A later PR turns the questions the page can
/// answer on its own into events.
pub enum PageInput {
    TextKey {
        key_code: u32,
        key: String,
        ctrl: bool,
        shift: bool,
        alt: bool,
        reply: oneshot::Sender<bool>,
    },
    KeyUp {
        key_code: u32,
        key: String,
        ctrl: bool,
        shift: bool,
        alt: bool,
    },
    /// A primary-button press: the `clicks`-th of a multi-click (AppKit's
    /// `clickCount`), with Shift held or not.
    MouseDown { x: f32, y: f32, clicks: u32, shift: bool },
    /// A secondary-button press (CH2.E4): `contextmenu`. When no listener
    /// cancels it the page reports [`PageEvent::ContextMenu`].
    SecondaryDown { x: f32, y: f32 },
    /// Its release, when the browser's menu did not take it.
    SecondaryUp { x: f32, y: f32 },
    MouseMove { x: f32, y: f32 },
    MouseLeave,
    Click {
        x: f32,
        y: f32,
        reply: oneshot::Sender<ClickOutcome>,
    },
    FocusAt {
        x: f32,
        y: f32,
        reply: oneshot::Sender<Option<String>>,
    },
    LinkAt {
        x: f32,
        y: f32,
        reply: oneshot::Sender<Option<String>>,
    },
    Scroll {
        dx: f32,
        dy: f32,
        reply: oneshot::Sender<bool>,
    },
    /// A wheel over a point of the view: the box under it may take it.
    ScrollAt {
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        /// A step of a gesture (a trackpad's), not a wheel on its own.
        gesture: bool,
        /// The platform says a gesture begins here: a new scroll sequence.
        began: bool,
    },
    FormSubmit {
        reply: oneshot::Sender<Option<String>>,
    },
    TakeNavigation {
        reply: oneshot::Sender<Option<(String, bool)>>,
    },
    HasFocus {
        reply: oneshot::Sender<bool>,
    },
    /// Whether the focused element is a text field (a text input, a
    /// textarea, editable content): the editing chords go to the page
    /// only then.
    HasTextFocus {
        reply: oneshot::Sender<bool>,
    },
    /// A macOS Cmd chord (CH2.E3): the page's keydown first. The page
    /// answers [`PageEvent::KeyEquivalentDone`] with `token`; one it did
    /// not claim is the menu's.
    CommandKey {
        key_code: u32,
        key: String,
        shift: bool,
        alt: bool,
        token: u64,
    },
    /// The input method's marked or committed text (CH2.E3).
    Ime(rustkit_viewhost::ImeInput),
    /// An Edit menu item in the focused text field (CH2.E3).
    Edit(rustkit_dom::forms::EditCommand),
}

/// What the page tells the window.
///
/// `nav_id` is the navigation the page was on when it emitted the event.
/// The window drops one whose id is older than the navigation it has since
/// asked for.
#[derive(Debug, Clone)]
pub enum PageEvent {
    Commit(FrameCommit),
    Title {
        text: String,
        nav_id: u64,
    },
    Url {
        url: Url,
        nav_id: u64,
    },
    LoadState {
        state: LoadState,
        nav_id: u64,
    },
    NavigateRequested {
        url: String,
        replace: bool,
        nav_id: u64,
        /// The user activated a link or submitted a form. A script's
        /// `location.href`, `link.click()` or `form.submit()` is false.
        from_user: bool,
    },
    Focused(bool),
    /// Whether the focused element is a text field, sent with `Focused`.
    TextFocused(bool),
    /// A message the page posted with `window.ipc.postMessage` (CH2.E9),
    /// pushed to the window as it arrives; nothing polls the page for it.
    Ipc {
        payload: String,
        nav_id: u64,
    },
    /// Whether the page claimed the Cmd chord `token`
    /// ([`PageInput::CommandKey`]); the window hands one it did not to the
    /// menu (CH2.E3).
    KeyEquivalentDone {
        token: u64,
        claimed: bool,
    },
    /// The focused text field's marked range, selection and caret, when it
    /// changed; `None` when no text field has focus (CH2.E3). The input
    /// method asks for them synchronously; the window keeps the last.
    TextInput(Option<crate::TextInputState>),
    /// The used `cursor` keyword under the pointer, when it changed
    /// (CH2.E4): `auto` resolved to `text` or `default`; `default` once the
    /// pointer left the view.
    Cursor(String),
    /// The tooltip under the pointer, when it changed: the nearest
    /// `title`, `None` for none (CH2.E4).
    Tooltip(Option<String>),
    /// A secondary press no listener cancelled: the window shows its
    /// context menu at view point `x`, `y` (CH2.E4).
    ContextMenu { x: f32, y: f32 },
    /// Not produced on this PR. The variant is the protocol's.
    #[allow(dead_code)]
    Unresponsive,
    Closed,
}

/// The window side of one page.
///
/// Send a command, then [`PageEndpoint::drain`] on this thread. Events are
/// read with [`PageHandle::drain_events`].
pub struct PageHandle {
    tx: Sender<PageCmd>,
    events: Receiver<PageEvent>,
    stamp: Arc<rustkit_js::ThreadStamp>,
    control: Arc<rustkit_js::ViewControl>,
    view: EngineViewId,
    last_sent_nav_id: AtomicU64,
}

/// The page side. Owns the engine for `view`.
pub struct PageEndpoint {
    engine: Engine,
    side: EndpointSide,
}

/// Everything the page side keeps besides its engine. Split from the
/// engine so a load that holds the engine can lend it back to this half
/// at its safe points (LAT15): the commands are read and applied here,
/// with the engine the load passes in.
pub(crate) struct EndpointSide {
    rx: Receiver<PageCmd>,
    events: Sender<PageEvent>,
    engine_events: Option<tokio::sync::mpsc::UnboundedReceiver<EngineEvent>>,
    view: EngineViewId,
    /// Navigation, generation and scroll of the last [`PageEvent::Commit`]
    /// sent. A new document can reuse a generation; the navigation is what
    /// makes that frame a different one. Scroll-only updates keep both.
    last_commit: Option<(u64, u64, (f32, f32))>,
    /// `SetVisible`, recorded for the content thread: only a visible view's timers take the exact
    /// wait (LINKS-SLOW part 2 R1).
    visible: bool,
    /// The content thread settles an unhandled key as a scroll, because the
    /// window does not wait to find out whether the page consumed it.
    settle_keys: bool,
    /// Post the window's live wake when an event is published. The
    /// same-thread drain does not: it is already on the window thread.
    wake_window: bool,
    /// Notified with every command the content thread's window sends, so
    /// the thread's idle wait ends at once (CH3.A9). `None` on the
    /// same-thread path, which never waits.
    cmd_wake: Option<Arc<tokio::sync::Notify>>,
    /// The last [`PageEvent::Focused`] sent. A script's `focus()`, an
    /// autofocus or a new document moves focus without any input from the
    /// window, and the window answers its keys from this report.
    /// The last focus report: (anything focused, a text field focused).
    last_focused: Option<(bool, bool)>,
    /// The last [`PageEvent::TextInput`] sent.
    last_text_input: Option<Option<crate::TextInputState>>,
    /// The newest navigation the window was told failed. A navigation is
    /// reported failed at most once.
    failed_nav: u64,
    /// What devtools costs this page (`devtools_service`).
    devtools_counters: Arc<DevtoolsCounters>,
    /// Commands read at a load's safe point that may not run there (a
    /// navigation, Stop, Close, a live turn) and every command after
    /// the first of them, in order. Read before the channel.
    deferred: VecDeque<PageCmd>,
    /// A command served at a safe point asked to leave the page (a
    /// followed link, a submit, a script navigation).
    asked_to_leave: std::cell::Cell<bool>,
}

/// Pair a handle with the endpoint that applies its commands.
pub(crate) fn adopt(
    mut engine: Engine,
    view: EngineViewId,
    rx: Receiver<PageCmd>,
    events: Sender<PageEvent>,
) -> PageEndpoint {
    let engine_events = engine.take_event_receiver();
    PageEndpoint {
        engine,
        side: EndpointSide {
            rx,
            events,
            engine_events,
            view,
            last_commit: None,
            visible: true,
            settle_keys: false,
            wake_window: false,
            cmd_wake: None,
            last_focused: None,
            last_text_input: None,
            failed_nav: 0,
            devtools_counters: Arc::default(),
            deferred: VecDeque::new(),
            asked_to_leave: std::cell::Cell::new(false),
        },
    }
}

pub fn connect(engine: Engine, view: EngineViewId) -> (PageHandle, PageEndpoint) {
    let control = engine
        .view_control(view)
        .unwrap_or_else(|| panic!("page protocol: view {view:?} is not in the engine"));
    let stamp = rustkit_js::Watchdog::shared().thread_stamp();
    let (tx, rx) = mpsc::channel();
    let (ev_tx, ev_rx) = mpsc::channel();
    let handle = PageHandle {
        tx,
        events: ev_rx,
        stamp,
        control,
        view,
        last_sent_nav_id: AtomicU64::new(0),
    };
    let endpoint = adopt(engine, view, rx, ev_tx);
    (handle, endpoint)
}

impl PageHandle {
    pub fn view(&self) -> EngineViewId {
        self.view
    }

    pub fn stamp(&self) -> Arc<rustkit_js::ThreadStamp> {
        Arc::clone(&self.stamp)
    }

    pub fn control(&self) -> Arc<rustkit_js::ViewControl> {
        Arc::clone(&self.control)
    }

    /// How many navigations this handle has sent. The first is 1.
    pub fn last_sent_nav_id(&self) -> u64 {
        self.last_sent_nav_id.load(Ordering::Relaxed)
    }

    /// Queue `cmd`. It does nothing until [`PageEndpoint::drain`].
    pub fn send(&self, cmd: PageCmd) {
        if matches!(cmd, PageCmd::Navigate { .. } | PageCmd::Reload { .. }) {
            self.last_sent_nav_id.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.tx.send(cmd);
    }

    /// Events published since the last call, in order.
    pub fn drain_events(&self) -> Vec<PageEvent> {
        let mut out = Vec::new();
        loop {
            match self.events.try_recv() {
                Ok(event) => out.push(event),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

impl EndpointSide {
    /// Apply every command queued so far, on this thread.
    ///
    /// `live` is the runtime the live loop already uses. A navigation builds
    /// its own runtime, as the view did before this protocol.
    pub fn drain(&mut self, engine: &mut Engine, live: Option<&tokio::runtime::Runtime>) {
        loop {
            match self.rx.try_recv() {
                Ok(cmd) => self.apply(engine, cmd, live),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
    }

    fn apply(&mut self, engine: &mut Engine, cmd: PageCmd, live: Option<&tokio::runtime::Runtime>) {
        match cmd {
            PageCmd::Navigate { url, nav_id } => {
                self.load_url(engine, url, nav_id);
            }
            PageCmd::LoadHtml { html, nav_id, reply } => {
                let result = self.load_html_cmd(engine, &html, nav_id);
                let _ = reply.send(result);
                self.after_page_change(engine);
            }
            PageCmd::Stop { reply } => {
                let stopped = engine.stop(self.view);
                // `stop_all` left ALL in the word so the script that was
                // running could unwind. That script has returned by the time
                // this command runs. Lift ALL so the next script on this
                // document can run; a turn id left in the word names a turn
                // that has already ended.
                if let Some(control) = engine.view_control(self.view) {
                    control.clear_all();
                }
                // A load cancelled before the word was lowered could not
                // settle its document; it reads `complete` now (LAT15 R1).
                engine.settle_ready_state(self.view);
                let _ = reply.send(stopped);
            }
            PageCmd::Reload { reply, .. } => {
                let result = match engine.get_url(self.view) {
                    Some(url) => {
                        self.load_url(engine, url, 0);
                        Ok(())
                    }
                    None => Err("No URL to reload".to_string()),
                };
                let _ = reply.send(result);
            }
            PageCmd::Input(input) => self.apply_input(engine, input),
            PageCmd::Resize { bounds, reply, .. } => {
                let result = engine
                    .set_view_bounds(self.view, bounds)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            PageCmd::DevtoolsEval { reply, .. } if reply.is_closed() => {
                // The panel discarded it (navigated, closed): nothing to run for.
            }
            PageCmd::DevtoolsEval { deadline, reply, .. } if std::time::Instant::now() >= deadline => {
                let _ = reply.send(Err(EVAL_CANCELLED.to_string()));
            }
            PageCmd::DevtoolsEval { script, reply, .. } | PageCmd::Eval { script, reply } => {
                let result = engine
                    .execute_script(self.view, &script)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
                self.publish_commit(engine);
                self.report_focus(engine);
                // `location.href =` and `link.click()` from this script.
                // The same-thread window asks with TakeNavigation instead.
                self.report_script_navigation(engine);
                // What the script posted on `window.ipc` goes now, not
                // after the next live turn (CH3.A9: the chrome's
                // `sendIpc('create_tab')` for Cmd+T).
                self.report_ipc(engine);
            }
            PageCmd::SetVisible { visible } => {
                self.visible = visible;
            }
            PageCmd::Close => {
                if let Err(e) = engine.destroy_view(self.view) {
                    tracing::warn!(error = %e, "Failed to destroy RustKit view");
                }
                self.emit(PageEvent::Closed);
            }
            PageCmd::PumpLive { elapsed_ms, reply } => {
                let turn = match live {
                    Some(runtime) => runtime.block_on(engine.pump_live_real(self.view, elapsed_ms)),
                    None => LivePump::default(),
                };
                let _ = reply.send(turn);
                self.after_page_change(engine);
            }
            PageCmd::Render { every } => {
                if let Err(e) = engine.flush_pending_resize(self.view) {
                    tracing::debug!(error = %e, "layout after a resize failed");
                }
                if every {
                    engine.render_all_views();
                } else {
                    engine.render_changed_views();
                }
                self.publish_commit(engine);
            }
            PageCmd::NoteInput { at, seq, kind } => {
                engine.note_input(self.view, InputMark { at, seq, kind });
            }
            PageCmd::GrabKeyboard => {
                engine.grab_keyboard(self.view);
            }
            PageCmd::Relayout => {
                if let Err(e) = engine.relayout_for(self.view, crate::RelayoutCause::Input) {
                    tracing::debug!(error = %e, "relayout after edit failed");
                }
                self.publish_commit(engine);
            }
            PageCmd::SetHostReentry { limit } => {
                engine.set_host_reentry_limit(limit);
            }
            PageCmd::SetAppOwned { owned } => {
                engine.set_app_owned(self.view, owned);
            }
            PageCmd::InstallReentryProbe { reply } => {
                let result = engine
                    .install_reentry_probe(self.view)
                    .map_err(|e| e.to_string());
                let _ = reply.send(result);
            }
            PageCmd::Devtools { query, reply } => {
                self.devtools_counters.page_command();
                // The window watches at once when it can reach the page's
                // Inspector; while the page boots or holds its endpoint it
                // cannot, so the page watches too (#803 review (2)).
                if let (DevtoolsQuery::Attach(on), Some(inspector)) = (&query, engine.inspector(self.view)) {
                    crate::devtools_service::watch(&inspector, &self.devtools_counters, *on);
                }
                let (answer, frame_changed) = devtools_answer(engine, self.view, query);
                let wake = devtools_wakes_window(&answer, frame_changed);
                let _ = reply.send(answer);
                if frame_changed {
                    // On the page's own thread there is no presenter: this
                    // publishes the outlined frame for the window to draw.
                    if let Err(e) = engine.render_view(self.view) {
                        tracing::debug!(error = %e, "devtools outline frame failed");
                    }
                    self.after_page_change(engine);
                }
                // No event carries the answer: wake the content thread's
                // window so it reads it now, when there is something to read.
                if self.wake_window && wake {
                    self.devtools_counters.window_wake();
                    crate::script_net::poke_live_turn();
                }
            }
            #[cfg(test)]
            PageCmd::Block(ms) => std::thread::sleep(std::time::Duration::from_millis(ms)),
            #[cfg(test)]
            PageCmd::PanicHoldingInspector => {
                if let Some(inspector) = engine.inspector(self.view) {
                    inspector.panic_holding_lock();
                }
            }
        }
    }

    fn apply_input(&mut self, engine: &mut Engine, input: PageInput) {
        let view = self.view;
        // What the pointer is over may change: the window is told its
        // cursor and tooltip after these (CH2.E4).
        let pointer = matches!(
            input,
            PageInput::MouseDown { .. }
                | PageInput::MouseMove { .. }
                | PageInput::MouseLeave
                | PageInput::Click { .. }
                | PageInput::SecondaryDown { .. }
                | PageInput::SecondaryUp { .. }
                | PageInput::Scroll { .. }
                | PageInput::ScrollAt { .. }
        );
        match input {
            PageInput::TextKey {
                key_code,
                key,
                ctrl,
                shift,
                alt,
                reply,
            } => {
                let submitted = if key_code == 0x0D {
                    engine.submit_focused_form(view)
                } else {
                    None
                };
                if let Some(url) = submitted {
                    let nav_id = engine.nav_id(view).unwrap_or(0);
                    self.emit(PageEvent::NavigateRequested {
                        url,
                        replace: false,
                        nav_id,
                        from_user: true,
                    });
                    let _ = reply.send(true);
                } else {
                    let focused_before = engine.focused_node(view);
                    let consumed = engine
                        .handle_text_key(view, key_code, &key, ctrl, shift, alt);
                    if self.settle_keys && !consumed && focused_before.is_none() {
                        if let Some(dy) = scroll_dy_for_unhandled_key(key_code, &key) {
                            let _ = engine.scroll_view(view, 0.0, dy);
                        }
                    }
                    let _ = reply.send(consumed || self.settle_keys);
                }
            }
            PageInput::CommandKey { key_code, key, shift, alt, token } => {
                // A chord whose wait ran out went to the menu, which is its
                // one actor: the page does not hear its keydown (M1g; a
                // timeout-path divergence, Chrome has no timeout). Otherwise
                // the chord's edit runs only while the window still holds
                // the chord for this page (M1e: the menu's Paste/Undo is the
                // edit of a chord that timed out while its keydown ran).
                let claimed = if rustkit_viewhost::begin_key_equivalent(token) {
                    engine.handle_command_key_if(view, key_code, &key, shift, alt, &mut || {
                        rustkit_viewhost::claim_key_equivalent(token)
                    })
                } else {
                    tracing::debug!(token, "Cmd chord's wait ran out: the menu has it, the page does not hear it");
                    false
                };
                self.emit(PageEvent::KeyEquivalentDone { token, claimed });
            }
            PageInput::Ime(input) => {
                use rustkit_viewhost::ImeInput;
                match input {
                    ImeInput::SetMarked(text) => {
                        engine.set_marked_text(view, &text);
                    }
                    ImeInput::Insert { text, replace } => {
                        engine.insert_text_replacing(view, &text, replace);
                    }
                    ImeInput::Unmark => {
                        engine.unmark_text(view);
                    }
                }
            }
            PageInput::Edit(command) => {
                engine.run_edit_command(view, &command);
            }
            PageInput::KeyUp { key_code, key, ctrl, shift, alt } => {
                engine.handle_key_up(view, key_code, &key, ctrl, shift, alt);
            }
            PageInput::MouseDown { x, y, clicks, shift } => {
                engine.mouse_down_with(view, x, y, clicks.max(1), shift);
            }
            PageInput::SecondaryDown { x, y } => {
                if engine.context_menu_at_point(view, x, y) {
                    self.emit(PageEvent::ContextMenu { x, y });
                }
            }
            PageInput::SecondaryUp { x, y } => engine.context_menu_release(view, x, y),
            PageInput::MouseMove { x, y } => engine.mouse_move_at_point(view, x, y),
            PageInput::MouseLeave => engine.mouse_leave(view),
            PageInput::Click { x, y, reply } => {
                let outcome = engine.click_at_point(view, x, y);
                if let Some(url) = outcome.navigate.clone() {
                    let nav_id = engine.nav_id(view).unwrap_or(0);
                    self.emit(PageEvent::NavigateRequested {
                        url,
                        replace: false,
                        nav_id,
                        from_user: true,
                    });
                }
                let _ = reply.send(outcome);
            }
            PageInput::FocusAt { x, y, reply } => {
                let tag = engine.focus_at_point(view, x, y);
                self.emit_focus(engine);
                let _ = reply.send(tag);
            }
            PageInput::LinkAt { x, y, reply } => {
                let url = engine.link_at_point(view, x, y);
                let _ = reply.send(url);
            }
            PageInput::Scroll { dx, dy, reply } => {
                let changed = match engine.scroll_view(view, dx, dy) {
                    Ok(changed) => changed,
                    Err(e) => {
                        tracing::debug!(error = %e, "scroll_by failed");
                        false
                    }
                };
                let _ = reply.send(changed);
                self.publish_commit(engine);
            }
            PageInput::ScrollAt { x, y, dx, dy, gesture, began } => {
                if let Err(e) = engine.wheel_phase_at(view, x, y, dx, dy, gesture, began) {
                    tracing::debug!(error = %e, "scroll_at failed");
                }
                self.publish_commit(engine);
            }
            PageInput::FormSubmit { reply } => {
                let url = engine.submit_focused_form(view);
                if let Some(url) = url.clone() {
                    let nav_id = engine.nav_id(view).unwrap_or(0);
                    self.emit(PageEvent::NavigateRequested {
                        url,
                        replace: false,
                        nav_id,
                        from_user: true,
                    });
                }
                let _ = reply.send(url);
            }
            PageInput::TakeNavigation { reply } => {
                let taken = engine.take_script_navigation_with_replace(view);
                if let Some((url, replace)) = taken.clone() {
                    let nav_id = engine.nav_id(view).unwrap_or(0);
                    self.emit(PageEvent::NavigateRequested {
                        url,
                        replace,
                        nav_id,
                        from_user: false,
                    });
                }
                let _ = reply.send(taken);
            }
            PageInput::HasFocus { reply } => {
                let (focused, _) = self.emit_focus(engine);
                let _ = reply.send(focused);
            }
            PageInput::HasTextFocus { reply } => {
                let (_, text) = self.emit_focus(engine);
                let _ = reply.send(text);
            }
        }
        self.publish_commit(engine);
        self.report_focus(engine);
        if pointer {
            self.report_hover(engine);
        }
        // A listener's `location.href =` or `link.click()` is not the
        // click's own default action. The content thread reports it; the
        // same-thread window still pulls with TakeNavigation.
        self.report_script_navigation(engine);
    }

    fn load_html_cmd(&mut self, engine: &mut Engine, html: &str, nav_id: u64) -> Result<(), String> {
        let loaded = if nav_id == 0 {
            engine.load_html(self.view, html)
        } else {
            engine.load_html_as(self.view, html, Some(nav_id))
        };
        loaded.map_err(|e| e.to_string())
    }

    fn load_url(&mut self, engine: &mut Engine, url: Url, nav_id: u64) {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                tracing::error!(error = %e, "Failed to create runtime");
                return;
            }
        };
        rt.block_on(self.load_yielding(engine, url, nav_id, false));
        self.after_page_change(engine);
    }

    /// Yield after DOMContentLoaded while inserted scripts are still
    /// arriving. A page with none finishes inside this call. `nav_id` 0
    /// lets the engine allocate; any other id is the window's.
    ///
    /// Events the load emits while a fetch is outstanding are forwarded
    /// every couple of milliseconds, including a frame the load published
    /// after layout. The load future borrows only the engine, so the event
    /// channel can be read while it is pending.
    ///
    /// With `serve` (the content thread's own loop, LAT15) the load lends
    /// the engine back at its safe points, and the commands that may run
    /// there are applied ([`EndpointSide::serve_mid_load`]). Without it
    /// (the same-thread window, a drain) nothing is read until the load
    /// returns, as before.
    async fn load_yielding(&mut self, engine: &mut Engine, url: Url, nav_id: u64, serve: bool) {
        let view = self.view;
        let wake = self.wake_window;
        let failed = {
            // The load borrows the engine; this half is shared between the
            // safe points (inside a poll) and the loop below (between
            // polls), never both at once.
            let side = std::cell::RefCell::new(&mut *self);
            let mut lend = |engine: &mut Engine| side.borrow_mut().serve_mid_load(engine);
            let lend: Option<&mut crate::ServeFn<'_>> = if serve { Some(&mut lend) } else { None };
            let mut load = Box::pin(async {
                let nav_id = (nav_id != 0).then_some(nav_id);
                engine.load_url_serving(view, url, nav_id, lend).await
            });
            let forward = |side: &std::cell::RefCell<&mut EndpointSide>| {
                let mut side = side.borrow_mut();
                let side = &mut **side;
                forward_events(&mut side.engine_events, &side.events, &mut side.last_commit, &mut side.failed_nav, nav_id, wake);
            };
            loop {
                // Polled on the runtime's waker via the sleep below: a fetch
                // that completes during the sleep is observed on the next poll.
                // `yield_now` inside the load returns Pending on the first poll
                // and Ready on the next, so a frame sent before that yield is
                // forwarded before the load continues into scripts.
                let polled = std::future::Future::poll(
                    load.as_mut(),
                    &mut std::task::Context::from_waker(futures::task::noop_waker_ref()),
                );
                match polled {
                    std::task::Poll::Ready(result) => {
                        let failed = result.as_ref().err().map(|e| e.to_string());
                        if let Some(e) = &failed {
                            tracing::error!(error = %e, "Failed to load URL");
                        }
                        forward(&side);
                        break failed;
                    }
                    std::task::Poll::Pending => {
                        forward(&side);
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    }
                }
            }
        };
        self.end_uncommitted_nav(engine, nav_id, failed);
    }

    /// A load's safe point (LAT15): apply what the window sent that may run
    /// while the load holds the engine. Input, resizes, renders, evals and
    /// devtools reads run, in order. The first command of any other kind
    /// (a navigation, Stop, Close, a reload, a live turn) and everything
    /// after it stay queued for after the load, in order; while one heads
    /// the queue, nothing is applied here. Reports whether anything ran, and
    /// whether a command that ran, or a page script before this safe point,
    /// asked to leave the page.
    fn serve_mid_load(&mut self, engine: &mut Engine) -> crate::Served {
        let mut served = crate::Served::default();
        for _ in 0..64 {
            match self.rx.try_recv() {
                Ok(cmd) => self.deferred.push_back(cmd),
                Err(_) => break,
            }
        }
        // A command that may not run here heads the queue (the window's own
        // navigation, Stop, Close...): nothing runs and nothing is reported
        // until the load returns. A script navigation reported now would
        // overtake it.
        if self.deferred.front().is_some_and(|cmd| !runs_mid_load(cmd)) {
            return served;
        }
        // `deferred` holds only what the window has sent: commands move
        // here from the channel and leave in order once the load returns,
        // so it is bounded by what the channel carried meanwhile.
        let queued: Vec<PageCmd> = self.deferred.drain(..).collect();
        self.deferred = coalesce(queued).into();
        self.asked_to_leave.set(false);
        while self.deferred.front().is_some_and(runs_mid_load) {
            let Some(cmd) = self.deferred.pop_front() else { break };
            served.ran = true;
            self.apply(engine, cmd, None);
        }
        if served.ran {
            // A resize is laid out here, as a live turn would at its start:
            // the page reads its new size before the load's next phase.
            if let Err(e) = engine.flush_pending_resize(self.view) {
                tracing::debug!(error = %e, "layout after a resize at a safe point failed");
            }
            self.publish_commit(engine);
            self.report_focus(engine);
            self.report_script_navigation(engine);
            self.report_ipc(engine);
        } else if engine.follows_script_navigation() {
            // The load's own script asked to leave (`location.href = ...`)
            // and nothing else ran: report it now, not when the load ends,
            // so the window's navigation starts at once and ends this load
            // (LAT17). A page thread that does not follow script
            // navigation mid-load hears of it after the load, as before.
            self.report_script_navigation(engine);
        }
        served.leave = self.asked_to_leave.replace(false);
        served
    }

    /// A navigation that ended without committing a document: a refused
    /// connection, a TLS or fetch error, an empty error body. The window's
    /// `navigate` raised ALL on the interrupt word so the old document's
    /// running script would stop, and only a commit lowers it. Unless a
    /// newer navigation or a Stop cancelled this one (they lower it
    /// themselves), lower it here, so the document still on screen keeps
    /// its timers, clicks and keys. Tell the window the load is over, so
    /// it stops showing the page as loading: Failed once for an error,
    /// Complete for a load that ended Ok with nothing to commit (a 204).
    fn end_uncommitted_nav(&mut self, engine: &mut Engine, nav_id: u64, failed: Option<String>) {
        if nav_id == 0 {
            return;
        }
        let Some(control) = engine.view_control(self.view) else {
            return;
        };
        if !control.end_uncommitted(nav_id) {
            return;
        }
        // The address bar went to the URL that failed. The document on
        // screen is still the committed one; show its URL again.
        if let Some(url) = engine.get_url(self.view) {
            let _ = self.events.send(PageEvent::Url { url, nav_id });
        }
        // A load that ended Ok without a commit (a 204) is not a failure:
        // the page stays as it is and the load is over. A failure the
        // engine already reported is not reported again.
        let state = match failed {
            None => Some(LoadState::Complete),
            Some(_) if self.failed_nav == nav_id => None,
            Some(error) => {
                self.failed_nav = nav_id;
                Some(LoadState::Failed { error })
            }
        };
        if let Some(state) = state {
            let _ = self.events.send(PageEvent::LoadState { state, nav_id });
        }
        if self.wake_window {
            crate::script_net::poke_live_turn();
        }
    }

    /// Load `url` as [`PageCmd::Navigate`] does, as a future the window
    /// thread polls a slice at a time (#743). It holds the endpoint until
    /// the load ends or yields after `DOMContentLoaded`.
    pub async fn navigate(&mut self, engine: &mut Engine, url: Url, nav_id: u64) {
        self.load_yielding(engine, url, nav_id, false).await;
        self.after_page_change(engine);
    }

    /// Cancels this view's navigations without the endpoint (M3b).
    pub fn nav_canceller(&self, engine: &Engine) -> Option<crate::NavCanceller> {
        engine.nav_canceller(self.view)
    }

    /// Whether a yielded load is parked on the view; see
    /// [`Engine::load_tail_open`].
    pub fn load_tail_open(&self, engine: &Engine) -> bool {
        engine.load_tail_open(self.view)
    }

    fn after_page_change(&mut self, engine: &mut Engine) {
        self.translate_engine_events(engine);
        self.publish_commit(engine);
        self.report_focus(engine);
        self.report_text_input(engine);
        self.report_script_navigation(engine);
        self.report_ipc(engine);
    }

    /// Send the window what the page posted on `window.ipc` (CH2.E9).
    /// Only the content thread reports it; the queue is Rust-side, so an
    /// empty one costs no script.
    fn report_ipc(&mut self, engine: &mut Engine) {
        if !self.wake_window {
            return;
        }
        let messages = engine.drain_view_ipc(self.view);
        if messages.is_empty() {
            return;
        }
        let nav_id = engine.nav_id(self.view).unwrap_or(0);
        for message in messages {
            self.emit(PageEvent::Ipc { payload: message.payload, nav_id });
        }
    }

    /// Tell the content thread's window when the page's focus changed on
    /// its own: a script's `focus()` or `blur()`, a timer, a new document
    /// with nothing focused. Only the content thread reports it; the
    /// same-thread window asks with [`PageInput::HasFocus`].
    fn report_focus(&mut self, engine: &mut Engine) {
        if !self.wake_window {
            return;
        }
        if self.last_focused == Some(self.focus_state(engine)) {
            return;
        }
        self.emit_focus(engine);
    }

    /// Tell the content thread's window the focused text field's
    /// text-input state when it changed (CH2.E3), after the frame it was
    /// painted in, so the caret is the one on screen.
    fn report_text_input(&mut self, engine: &mut Engine) {
        if !self.wake_window {
            return;
        }
        let state = engine.text_input_state(self.view);
        if self.last_text_input == Some(state) {
            return;
        }
        self.last_text_input = Some(state);
        self.emit(PageEvent::TextInput(state));
    }

    /// Tell the window the cursor and tooltip under the pointer when they
    /// changed (CH2.E4). A pointer that left the view gets `default` and
    /// no tooltip.
    fn report_hover(&mut self, engine: &mut Engine) {
        let Some(now) = engine.take_hover_change(self.view) else {
            return;
        };
        let (cursor, title) = match now {
            Some(f) => (f.cursor, f.title),
            None => ("default", None),
        };
        self.emit(PageEvent::Cursor(cursor.to_string()));
        self.emit(PageEvent::Tooltip(title));
    }

    /// (anything focused, a text field focused) in the page now.
    fn focus_state(&self, engine: &Engine) -> (bool, bool) {
        (
            engine.focused_node(self.view).is_some(),
            engine.focused_is_text_control(self.view),
        )
    }

    /// Report the page's focus to the window (`Focused`, `TextFocused`).
    fn emit_focus(&mut self, engine: &mut Engine) -> (bool, bool) {
        let state = self.focus_state(engine);
        self.last_focused = Some(state);
        self.emit(PageEvent::Focused(state.0));
        self.emit(PageEvent::TextFocused(state.1));
        state
    }

    /// A navigation the page's script asked for (`location.href`,
    /// `location.replace`, `link.click()`, `form.submit()`), tagged with
    /// the document that asked. Only the content thread reports it here.
    /// The same-thread window sends [`PageInput::TakeNavigation`] and
    /// would otherwise see an empty queue.
    fn report_script_navigation(&mut self, engine: &mut Engine) {
        if !self.wake_window {
            return;
        }
        let Some((url, replace)) = engine.take_script_navigation_with_replace(self.view) else {
            return;
        };
        let nav_id = engine.nav_id(self.view).unwrap_or(0);
        self.emit(PageEvent::NavigateRequested {
            url,
            replace,
            nav_id,
            from_user: false,
        });
    }

    fn translate_engine_events(&mut self, engine: &mut Engine) {
        let nav_id = engine.nav_id(self.view).unwrap_or(0);
        let wake = self.wake_window;
        forward_events(
            &mut self.engine_events,
            &self.events,
            &mut self.last_commit,
            &mut self.failed_nav,
            nav_id,
            wake,
        );
    }

    fn publish_commit(&mut self, engine: &mut Engine) {
        let Some(commit) = engine.latest_commit(self.view).cloned() else {
            return;
        };
        // A commit from before this document was laid out has no display
        // list. Publishing it lets the window present the background and
        // then drop the real frame when the generation matches.
        if commit.list.is_none() {
            return;
        }
        let key = (commit.nav_generation, commit.generation, commit.scroll_offset);
        if self.last_commit == Some(key) {
            return;
        }
        self.last_commit = Some(key);
        self.emit(PageEvent::Commit(commit));
    }

    fn emit(&self, event: PageEvent) {
        if matches!(event, PageEvent::NavigateRequested { .. }) {
            self.asked_to_leave.set(true);
        }
        let _ = self.events.send(event);
        if self.wake_window {
            crate::script_net::poke_live_turn();
        }
    }

    pub(crate) fn set_settle_keys(&mut self, settle: bool) {
        self.settle_keys = settle;
    }

    pub(crate) fn set_wake_window(&mut self, wake: bool) {
        self.wake_window = wake;
    }

    pub(crate) fn set_command_wake(&mut self, wake: Arc<tokio::sync::Notify>) {
        self.cmd_wake = Some(wake);
    }

    /// What devtools has cost this page so far.
    pub fn devtools_counters(&self) -> Arc<DevtoolsCounters> {
        Arc::clone(&self.devtools_counters)
    }

    /// The page's Inspector, for reads the page need not answer
    /// (`devtools_service`).
    pub fn inspector(&self, engine: &Engine) -> Option<Arc<crate::inspector::Inspector>> {
        engine.inspector(self.view)
    }

    /// Commands waiting: those a load's safe point kept, then the channel,
    /// with adjacent resizes, renders and live turns collapsed. Clicks and
    /// keys keep their order.
    pub(crate) fn drain_coalesced(&mut self) -> Vec<PageCmd> {
        let mut batch: Vec<PageCmd> = self.deferred.drain(..).collect();
        for _ in 0..64 {
            match self.rx.try_recv() {
                Ok(cmd) => batch.push(cmd),
                Err(_) => break,
            }
        }
        coalesce(batch)
    }

    /// LINKS-SLOW part 2 delta R1: take a command that arrived while the loop waited, if any, onto
    /// the deferred queue. The wake permit its sender left is cleared first (a command sent after
    /// that leaves its own): taken here, it would end the next runtime wait at once, an extra turn.
    fn take_waiting_command(&mut self) -> bool {
        clear_stale_wake(self.cmd_wake.as_deref());
        match self.rx.try_recv() {
            Ok(cmd) => {
                self.deferred.push_back(cmd);
                true
            }
            Err(_) => false,
        }
    }

    /// The content thread's loop. One current-thread runtime, already
    /// entered by the caller: timers and fetches progress on every idle wait.
    pub(crate) async fn run(&mut self, engine: &mut Engine) {
        let mut clock = LiveClock::new(std::time::Instant::now());
        let mut overshoot = OvershootEstimate::default();
        let mut tail_estimate = TailEstimate::default();
        // LINKS-SLOW part 2, diagnostics only (`HIWAVE_LINKS_TRACE`): the last wait's due
        // instant, where it was aimed, and whether it was exact.
        let tracing = crate::links_trace_on();
        let mut traced_due: Option<(std::time::Instant, std::time::Instant, bool)> = None;
        loop {
            let mut cmds = self.drain_coalesced();
            if cmds.is_empty() {
                let woke = std::time::Instant::now();
                let elapsed = clock.turn(woke);
                // The real time the clock kept from this turn: the turn's timers count from real
                // time (LINKS-SLOW).
                let fraction = clock.behind(woke).as_secs_f64() * 1000.0;
                let turn = engine.pump_live_real_at(self.view, elapsed, fraction).await;
                if tracing {
                    trace_turn(traced_due.take(), woke, fraction, &turn);
                }
                self.after_page_change(engine);
                // A resize (or any other command) that arrived during the
                // turn is applied before the next timer slice. The frame
                // yield returns between timer tasks; the bounds message is
                // waiting here, and the next pump lays the page out. The
                // sleep is only the wait for the next timer. A single
                // callback that does not return is not a yield: the bounds
                // wait until that callback does.
                cmds = self.drain_coalesced();
                if cmds.is_empty() {
                    // A timer this turn's own timers set is aimed at its exact instant (to the
                    // microsecond), otherwise the whole-millisecond wait (LINKS-SLOW).
                    let exact = exact_wait(turn.next_timer_ms, turn.next_timer_exact_us, self.visible);
                    let wait = exact.unwrap_or_else(|| content_turn_wait(turn.next_timer_ms));
                    let now = std::time::Instant::now();
                    let due = clock.deadline(now, wait);
                    if due <= now {
                        // Due already: the runtime's timers and sockets get
                        // their turn first, as a sleep would give them.
                        tokio::task::yield_now().await;
                        continue;
                    }
                    // An exact wait leaves the runtime `PRECISE_TAIL` before its due instant, then a
                    // thread sleep aimed early by this thread's sleep overshoot, then a spin to the
                    // due instant, so nothing runs early (LINKS-SLOW part 2).
                    let precise = exact.is_some();
                    let tail = if precise { tail_estimate.tail(&overshoot) } else { std::time::Duration::ZERO };
                    let leave = runtime_wait_until(now, due, tail);
                    // A command ends the wait (CH3.A9): the chrome's Cmd+T
                    // used to sit here until the chrome's next timer.
                    let slept = runtime_wait(now, leave, self.cmd_wake.as_deref()).await;
                    if slept && precise {
                        let woke = std::time::Instant::now();
                        if leave > now {
                            tail_estimate.observe(leave, woke);
                        }
                        // A command that came while the runtime waited (or in its yield) is taken
                        // before the tail, not after it (R1), and again between the tail's sleep
                        // and its spin (delta R1).
                        if self.take_waiting_command() {
                            continue;
                        }
                        if tracing {
                            traced_due = Some((due, woke, true));
                        }
                        if finish_exact_wait(due, &mut overshoot, &mut ThreadTail, || self.take_waiting_command()).is_none() {
                            traced_due = None;
                        }
                    } else if tracing {
                        traced_due = Some((due, leave, precise));
                    }
                    continue;
                }
            }
            // Queued on this half, not held in a local: a load among them
            // serves the commands behind it at its safe points, and only in
            // order (LAT15).
            self.deferred.extend(cmds);
            while let Some(cmd) = self.deferred.pop_front() {
                let close = matches!(cmd, PageCmd::Close);
                self.apply_async(engine, cmd).await;
                if close {
                    return;
                }
            }
        }
    }

    async fn apply_async(&mut self, engine: &mut Engine, cmd: PageCmd) {
        match cmd {
            PageCmd::Navigate { url, nav_id } => {
                self.load_yielding(engine, url, nav_id, true).await;
                self.after_page_change(engine);
            }
            PageCmd::Reload { reply, nav_id } => {
                let result = match engine.get_url(self.view) {
                    Some(url) => {
                        self.load_yielding(engine, url, nav_id, true).await;
                        Ok(())
                    }
                    None => Err("No URL to reload".to_string()),
                };
                let _ = reply.send(result);
                self.after_page_change(engine);
            }
            PageCmd::PumpLive { elapsed_ms, reply } => {
                let turn = engine.pump_live_real(self.view, elapsed_ms).await;
                let _ = reply.send(turn);
                self.after_page_change(engine);
            }
            PageCmd::LoadHtml { html, nav_id, reply } => {
                let result = self.load_html_cmd(engine, &html, nav_id);
                let _ = reply.send(result);
                self.after_page_change(engine);
            }
            other => self.apply(engine, other, None),
        }
    }
}

/// How long the content thread's loop waits after a turn that left no
/// command: until the page's next timer, at most 50 ms, and 2 ms when a timer is
/// due already (a page whose timer is always due).
/// Nothing in it depends on how long the turn took: a turn that ran one long
/// callback is followed by the next frame as soon as its timer is due, as
/// in Chrome. (The window thread's loop with the content thread off,
/// `hiwave-app` `process_events`, keeps a wait at least as long as the last
/// turn, so the page cannot hold the window's thread more than half the
/// time.) parity-capture's live loop paces with this function, so a
/// headless capture paces as the app does.

pub fn content_turn_wait(next_timer_ms: Option<u64>) -> std::time::Duration {
    // The 2 ms floor is for a page whose timer is always due (a wait of 0): a timer due in 1 ms is
    // waited for 1 ms (LINKS-SLOW).
    let ms = match next_timer_ms {
        Some(0) => 2,
        Some(ms) => ms.min(50),
        None => 50,
    };
    std::time::Duration::from_millis(ms)
}

/// The content thread loop's clock: how much time each live turn is given,
/// and when the loop wakes for the next (PACE2).
///
/// The page's clock moves in whole milliseconds, a turn at a time. Each turn
/// is given the whole milliseconds since the last turn's time and the
/// fraction stays for the next, so the page's clock keeps up with real time.
/// Until PACE2 the clock restarted at every turn and dropped the fraction
/// (about 0.46 ms a turn), and the loop slept its wait after the turn and its
/// frame had ended, so a timer or frame due 10 ms after a turn came 10 ms
/// after its end: with sleeps that end late as well, a requestAnimationFrame
/// loop beside a 10 ms timer chain got 51-54 frames a second where Chrome
/// gets 60. parity-capture's live loop paces its content-thread arm with this
/// clock, so a capture paces as the app does.
///
/// A page whose timer is always due (a `setTimeout(0)` chain, a turn that
/// ran out of budget) waits at least 2 ms from each turn's start
/// ([`content_turn_wait`]), so cheap turns come at most every 2 ms; turns
/// follow one another at once only while each takes 2 ms or more of the
/// page's own work, and the loop takes its commands before every turn.
pub struct LiveClock {
    /// The last turn's time: where the page's clock stands, in real time.
    /// Behind the instant that turn began by the fraction it was not given.
    turn_at: std::time::Instant,
}

impl LiveClock {
    pub fn new(now: std::time::Instant) -> Self {
        Self { turn_at: now }
    }

    /// The whole milliseconds since the last turn's time, for the turn that
    /// begins `now`; the fraction is kept for the next turn.
    pub fn turn(&mut self, now: std::time::Instant) -> u64 {
        let ms = now.saturating_duration_since(self.turn_at).as_millis() as u64;
        self.turn_at += std::time::Duration::from_millis(ms);
        ms
    }

    /// When the loop wakes for the next turn: `wait` (the page's next timer
    /// or frame, from [`content_turn_wait`]) after this turn's time, where
    /// the page's clock stood when the wait was measured. Not before `now`
    /// (the loop asks after the turn and its frame): a turn that took longer
    /// than the wait is followed by the next at once.
    pub fn deadline(&self, now: std::time::Instant, wait: std::time::Duration) -> std::time::Instant {
        (self.turn_at + wait).max(now)
    }

    /// The real time this clock has not given the page yet at `now`: the fraction of a millisecond
    /// the last turn kept. A real turn is told it, so its timers' clock reads real time
    /// (LINKS-SLOW).
    pub fn behind(&self, now: std::time::Instant) -> std::time::Duration {
        now.saturating_duration_since(self.turn_at)
    }
}

/// LINKS-SLOW: how late this content thread's thread sleeps end past the instant they were asked
/// for (the OS's slack: about 0.3 ms on Windows with 1 ms timers; the runtime's timer, which keeps
/// whole milliseconds, is no longer used for the last stretch: part 2). The loop aims the last
/// stretch of an exact wait that much before a timer's due instant, so it wakes near it;
/// counted from when they are set (CLOCK3), timers otherwise ran one overshoot late every link
/// (the Mac's 10 ms chain: 236-245 links in 3000 ms, Chrome 278-283).
///
/// The estimate starts at 0 (the first wakes behave as before), is capped at
/// [`Self::CAP_MS`], and tracks a low quantile of the overshoot rather than its mean: a sample
/// below the estimate pulls it halfway down at once, one above moves it a sixteenth of the way up,
/// so a machine whose overshoot drops is followed within a few wakes and a rare long sleep barely
/// moves it. Aiming early never makes a timer early: a wake before the due instant sleeps the rest
/// ([`after_wake`]), and a timer runs only once the page's clock has reached it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OvershootEstimate {
    ms: f64,
}

impl OvershootEstimate {
    /// The most a sleep is aimed early.
    pub const CAP_MS: f64 = 2.0;

    /// The current estimate; read by the tests only.
    #[cfg(test)]
    pub fn ms(&self) -> f64 {
        self.ms
    }

    /// The instant to sleep until for a wake at `due`.
    pub fn aim(&self, due: std::time::Instant) -> std::time::Instant {
        due.checked_sub(std::time::Duration::from_secs_f64(self.ms / 1000.0)).unwrap_or(due)
    }

    /// A sleep asked to end at `aimed` ended at `woke`.
    pub fn observe(&mut self, aimed: std::time::Instant, woke: std::time::Instant) {
        let sample = (woke.saturating_duration_since(aimed).as_secs_f64() * 1000.0).min(2.0 * Self::CAP_MS);
        let step = if sample < self.ms { 0.5 } else { 0.0625 };
        self.ms = (self.ms + (sample - self.ms) * step).clamp(0.0, Self::CAP_MS);
    }
}

/// LINKS-SLOW part 2, diagnostics only (`HIWAVE_LINKS_TRACE`): one line per live turn that ran a
/// timer, in microseconds, all real time: `late` the turn began past the due instant the loop
/// waited for (negative: early), `aim` how early that wait was aimed (an exact wait: how long
/// before the due instant the runtime's wait ended, the stretch the thread was blocked),
/// `entry` from the turn's start to the engine's live turn, `pre` from the live turn to its timer
/// slice, `slice` the timer slice (timers, then the frame round), `post` the rest of the turn
/// (network, layout, paint) and `frac` the fraction of a millisecond the loop's clock kept back.
fn trace_turn(
    due: Option<(std::time::Instant, std::time::Instant, bool)>,
    woke: std::time::Instant,
    fraction_ms: f64,
    turn: &crate::LivePump,
) {
    let ended = std::time::Instant::now();
    let [entered, slice_from, slice_to] = crate::links_trace_take();
    if turn.timers_ran == 0 {
        return;
    }
    let us = |a: std::time::Instant, b: std::time::Instant| -> i64 {
        if b >= a {
            b.duration_since(a).as_micros() as i64
        } else {
            -(a.duration_since(b).as_micros() as i64)
        }
    };
    let (late, aim, exact) = match due {
        Some((due, aimed, exact)) => (us(due, woke), us(aimed, due), exact),
        None => (0, 0, false),
    };
    let entry = entered.map_or(-1, |e| us(woke, e));
    let pre = match (entered, slice_from) {
        (Some(e), Some(f)) => us(e, f),
        _ => -1,
    };
    let slice = match (slice_from, slice_to) {
        (Some(f), Some(t)) => us(f, t),
        _ => -1,
    };
    let post = slice_to.map_or(-1, |t| us(t, ended));
    eprintln!(
        "LTRACE ran={} waited={} exact={} late={late} aim={aim} entry={entry} pre={pre} slice={slice} post={post} frac={}",
        turn.timers_ran,
        due.is_some(),
        exact,
        (fraction_ms * 1000.0) as i64
    );
}

/// LINKS-SLOW part 2: an exact wait leaves the runtime's timer this long before its due instant
/// and ends with a thread sleep. The runtime's timer keeps whole milliseconds: a sleep to a
/// fraction of one ended 1.4 ms late on average, spread over a millisecond
/// (`docs/diagnostics/2026-11-16/links_slow_part2_windows.md`). Longer than that overshoot's usual
/// spread, so the runtime wakes before the due instant; the content thread is blocked at most this
/// long per wait.
pub const PRECISE_TAIL: std::time::Duration = std::time::Duration::from_millis(2);

/// LINKS-SLOW: until when the content loop's wait for `due` sleeps on the runtime (where commands
/// and sockets keep their turn), from `now`. An exact wait leaves it `tail` early (at once when
/// less is left; [`TailEstimate`]) and ends with [`finish_exact_wait`]; a whole-millisecond wait
/// (`tail` zero) is the runtime's to the end.
pub fn runtime_wait_until(
    now: std::time::Instant,
    due: std::time::Instant,
    tail: std::time::Duration,
) -> std::time::Instant {
    due.checked_sub(tail).unwrap_or(now).max(now)
}

/// LINKS-SLOW part 2 R1: the wait the content loop aims at a timer's exact instant, or `None` for
/// the whole-millisecond wait: only a timer this turn's own timers set (`exact_us`, matching the
/// whole-millisecond wait), and only while the view is visible.
pub fn exact_wait(
    next_timer_ms: Option<u64>,
    exact_us: Option<u64>,
    visible: bool,
) -> Option<std::time::Duration> {
    // A background view's timers are not worth a blocking tail and a spin: they wait whole
    // milliseconds on the runtime, as before LINKS-SLOW part 2.
    exact_us
        .filter(|_| visible)
        .filter(|us| next_timer_ms == Some(us.div_ceil(1000)))
        .map(|us| std::time::Duration::from_micros(us.min(50_000)))
}

/// LINKS-SLOW part 2: the content loop's wait on the runtime from `now` until `leave`, ended early
/// by a command (`wake`). `false` when a command ended it. A wait with nothing left on the runtime
/// (an exact wait shorter than its tail) still yields once: the runtime's tasks, timers and
/// sockets get their turn before the thread blocks in the tail (R1: a 1 ms interval otherwise
/// never parked).
pub async fn runtime_wait(
    now: std::time::Instant,
    leave: std::time::Instant,
    wake: Option<&tokio::sync::Notify>,
) -> bool {
    if leave <= now {
        tokio::task::yield_now().await;
        return true;
    }
    match wake {
        Some(wake) => tokio::time::timeout_at(tokio::time::Instant::from_std(leave), wake.notified())
            .await
            .is_err(),
        None => {
            tokio::time::sleep_until(tokio::time::Instant::from_std(leave)).await;
            true
        }
    }
}

/// LINKS-SLOW part 2 delta R1: clear a wake permit a command's sender left (`Notify::notify_one`
/// with no waiter) once the loop has taken that command another way.
pub fn clear_stale_wake(wake: Option<&tokio::sync::Notify>) {
    use futures::FutureExt;
    if let Some(wake) = wake {
        // Ready only on a stored permit, which it takes; pending otherwise, and dropped unregistered.
        let _ = wake.notified().now_or_never();
    }
}

/// LINKS-SLOW part 2 R1: how long before a timer's due instant an exact wait leaves the runtime.
/// The tail has to cover how late the runtime's timer wakes (it keeps whole milliseconds), or the
/// timer runs that late; past the wake the thread is blocked until the due instant, so the tail is
/// no longer than this machine needs. It tracks a high quantile of the runtime wake's lateness (a
/// sample above pulls it halfway up at once, one below a thirty-second of the way down), plus the
/// thread sleep's overshoot and a margin, and is at most [`PRECISE_TAIL`]. It starts there.
/// Windows with 1 ms timers: the runtime woke 0.7-2.0 ms late (mean 1.4), so the tail stays 2 ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TailEstimate {
    ms: f64,
}

impl Default for TailEstimate {
    fn default() -> Self {
        Self { ms: PRECISE_TAIL.as_secs_f64() * 1000.0 }
    }
}

impl TailEstimate {
    /// Added to the two estimates: what they miss.
    pub const MARGIN_MS: f64 = 0.1;

    /// The tail for the next exact wait.
    pub fn tail(&self, overshoot: &OvershootEstimate) -> std::time::Duration {
        let ms = (self.ms + overshoot.ms + Self::MARGIN_MS).min(PRECISE_TAIL.as_secs_f64() * 1000.0);
        std::time::Duration::from_secs_f64(ms / 1000.0)
    }

    /// A runtime wait asked to end at `leave` ended at `woke`.
    pub fn observe(&mut self, leave: std::time::Instant, woke: std::time::Instant) {
        let cap = PRECISE_TAIL.as_secs_f64() * 1000.0;
        let sample = (woke.saturating_duration_since(leave).as_secs_f64() * 1000.0).min(2.0 * cap);
        let step = if sample > self.ms { 0.5 } else { 1.0 / 32.0 };
        self.ms = (self.ms + (sample - self.ms) * step).clamp(0.0, cap);
    }
}

/// LINKS-SLOW part 2 R1: the most of an exact wait's tail spent spinning.
pub const SPIN_CAP: std::time::Duration = std::time::Duration::from_micros(500);

/// The clock and sleeps [`finish_exact_wait`] runs on: the thread's, or a test's.
pub trait TailClock {
    fn now(&mut self) -> std::time::Instant;
    fn sleep(&mut self, d: std::time::Duration);
    fn spin(&mut self);
}

/// The content thread's own clock and sleeps.
pub struct ThreadTail;

impl TailClock for ThreadTail {
    fn now(&mut self) -> std::time::Instant {
        std::time::Instant::now()
    }
    fn sleep(&mut self, d: std::time::Duration) {
        std::thread::sleep(d);
    }
    fn spin(&mut self) {
        std::hint::spin_loop();
    }
}

/// LINKS-SLOW part 2: the end of an exact wait, after the runtime: thread sleeps aimed early by
/// `overshoot`, then a spin of at most [`SPIN_CAP`] to `due`. Returns how long it spun, only at or
/// after `due` (never early). With more than the cap left and the estimate past it (a few long
/// sleeps), it sleeps to the cap's distance first rather than spinning the rest (R1). `None`: a
/// command was waiting after a sleep (`command`, asked before the spin), and the wait ended there
/// for it (delta R1); the timer is the next turn's, which runs nothing before its due instant.
pub fn finish_exact_wait(
    due: std::time::Instant,
    overshoot: &mut OvershootEstimate,
    clock: &mut impl TailClock,
    mut command: impl FnMut() -> bool,
) -> Option<std::time::Duration> {
    let mut spin_from = None;
    loop {
        let now = clock.now();
        let Some(due) = after_wake(due, now) else {
            return Some(spin_from.map_or(std::time::Duration::ZERO, |f| now.saturating_duration_since(f)));
        };
        if due - now <= SPIN_CAP {
            spin_from.get_or_insert(now);
            clock.spin();
            continue;
        }
        let aimed = overshoot.aim(due);
        let aimed = if aimed > now { aimed } else { due - SPIN_CAP };
        // Only a real sleep says how late sleeps end (LINKS-SLOW R1): this one is aimed past now.
        clock.sleep(aimed - now);
        let woke = clock.now();
        overshoot.observe(aimed, woke);
        if command() {
            return None;
        }
    }
}

/// LINKS-SLOW: after an aimed sleep that ended at `woke`, the instant to sleep until again so the
/// turn comes no earlier than `due` (the timer's due instant), or `None` when it may run now.
pub fn after_wake(due: std::time::Instant, woke: std::time::Instant) -> Option<std::time::Instant> {
    (woke < due).then_some(due)
}

impl PageEndpoint {
    /// Apply every command queued so far, on this thread.
    ///
    /// `live` is the runtime the live loop already uses. A navigation builds
    /// its own runtime, as the view did before this protocol.
    pub fn drain(&mut self, live: Option<&tokio::runtime::Runtime>) {
        self.side.drain(&mut self.engine, live);
    }

    /// Load `url` as [`PageCmd::Navigate`] does, as a future the window
    /// thread polls a slice at a time (#743). It holds the endpoint until
    /// the load ends or yields after `DOMContentLoaded`.
    pub async fn navigate(&mut self, url: Url, nav_id: u64) {
        self.side.navigate(&mut self.engine, url, nav_id).await;
    }

    /// Cancels this view's navigations without the endpoint (M3b).
    pub fn nav_canceller(&self) -> Option<crate::NavCanceller> {
        self.side.nav_canceller(&self.engine)
    }

    /// Whether a yielded load is parked on the view; see
    /// [`Engine::load_tail_open`].
    pub fn load_tail_open(&self) -> bool {
        self.side.load_tail_open(&self.engine)
    }

    pub(crate) fn set_settle_keys(&mut self, settle: bool) {
        self.side.set_settle_keys(settle);
    }

    pub(crate) fn set_wake_window(&mut self, wake: bool) {
        self.side.set_wake_window(wake);
    }

    pub(crate) fn set_command_wake(&mut self, wake: Arc<tokio::sync::Notify>) {
        self.side.set_command_wake(wake);
    }

    /// What devtools has cost this page so far.
    pub fn devtools_counters(&self) -> Arc<DevtoolsCounters> {
        self.side.devtools_counters()
    }

    /// The page's Inspector, for reads the page need not answer
    /// (`devtools_service`).
    pub fn inspector(&self) -> Option<Arc<crate::inspector::Inspector>> {
        self.side.inspector(&self.engine)
    }

    /// The content thread's loop. One current-thread runtime, already
    /// entered by the caller: timers and fetches progress on every idle wait.
    pub(crate) async fn run(mut self) {
        self.side.run(&mut self.engine).await;
    }
}

/// What a load's safe point may apply (LAT15): input and what reads or
/// redraws the page. Not what starts or ends a document (a navigation, a
/// reload, an HTML load, Stop, Close), a live turn (timers must not run
/// inside a load), or the test hooks.
pub(crate) fn runs_mid_load(cmd: &PageCmd) -> bool {
    match cmd {
        PageCmd::Input(_)
        | PageCmd::Resize { .. }
        | PageCmd::Eval { .. }
        | PageCmd::DevtoolsEval { .. }
        | PageCmd::SetVisible { .. }
        | PageCmd::Render { .. }
        | PageCmd::NoteInput { .. }
        | PageCmd::GrabKeyboard
        | PageCmd::Relayout
        | PageCmd::Devtools { .. } => true,
        PageCmd::Navigate { .. }
        | PageCmd::LoadHtml { .. }
        | PageCmd::Stop { .. }
        | PageCmd::Reload { .. }
        | PageCmd::Close
        | PageCmd::PumpLive { .. }
        | PageCmd::SetAppOwned { .. }
        | PageCmd::SetHostReentry { .. }
        | PageCmd::InstallReentryProbe { .. } => false,
        #[cfg(test)]
        PageCmd::Block(_) | PageCmd::PanicHoldingInspector => false,
    }
}

fn forward_events(
    engine_events: &mut Option<tokio::sync::mpsc::UnboundedReceiver<EngineEvent>>,
    tx: &Sender<PageEvent>,
    last_commit: &mut Option<(u64, u64, (f32, f32))>,
    failed_nav: &mut u64,
    nav_id: u64,
    wake: bool,
) {
    let Some(rx) = engine_events.as_mut() else {
        return;
    };
    let mut pending = Vec::new();
    while let Ok(event) = rx.try_recv() {
        let mapped = match event {
            EngineEvent::NavigationStarted { url, .. } => vec![
                PageEvent::LoadState {
                    state: LoadState::Started,
                    nav_id,
                },
                PageEvent::Url { url, nav_id },
            ],
            EngineEvent::NavigationCommitted { url, .. } => {
                vec![PageEvent::Url { url, nav_id }]
            }
            EngineEvent::PageLoaded { url, title, .. } => {
                let mut out = Vec::new();
                if let Some(text) = title {
                    out.push(PageEvent::Title { text, nav_id });
                }
                out.push(PageEvent::Url { url, nav_id });
                out.push(PageEvent::LoadState {
                    state: LoadState::Complete,
                    nav_id,
                });
                out
            }
            EngineEvent::NavigationFailed { url, error, .. } => {
                if *failed_nav == nav_id && nav_id != 0 {
                    Vec::new()
                } else {
                    *failed_nav = nav_id;
                    vec![
                        PageEvent::Url { url, nav_id },
                        PageEvent::LoadState {
                            state: LoadState::Failed { error },
                            nav_id,
                        },
                    ]
                }
            }
            EngineEvent::TitleChanged { title, .. } => {
                vec![PageEvent::Title {
                    text: title,
                    nav_id,
                }]
            }
            EngineEvent::ViewFocused { .. } => vec![PageEvent::Focused(true)],
            EngineEvent::FrameCommitted(commit) => {
                if commit.list.is_none() {
                    Vec::new()
                } else {
                    let key = (commit.nav_generation, commit.generation, commit.scroll_offset);
                    if *last_commit == Some(key) {
                        Vec::new()
                    } else {
                        *last_commit = Some(key);
                        vec![PageEvent::Commit(commit)]
                    }
                }
            }
            EngineEvent::ConsoleMessage { .. }
            | EngineEvent::ViewResized { .. }
            | EngineEvent::DownloadStarted { .. }
            | EngineEvent::ImageLoaded { .. }
            | EngineEvent::ImageError { .. }
            | EngineEvent::FaviconDetected { .. } => Vec::new(),
        };
        pending.extend(mapped);
    }
    if pending.is_empty() {
        return;
    }
    for event in pending {
        let _ = tx.send(event);
    }
    if wake {
        crate::script_net::poke_live_turn();
    }
}

/// Collapse adjacent resizes, renders and live turns. Anything else,
/// including clicks and keys, stays where it was.
pub(crate) fn coalesce(batch: Vec<PageCmd>) -> Vec<PageCmd> {
    let mut out = Vec::new();
    for cmd in batch {
        match cmd {
            PageCmd::Resize {
                bounds,
                scale,
                reply,
            } => {
                if matches!(out.last(), Some(PageCmd::Resize { .. })) {
                    if let Some(PageCmd::Resize {
                        reply: previous, ..
                    }) = out.pop()
                    {
                        let _ = previous.send(Ok(()));
                    }
                }
                out.push(PageCmd::Resize {
                    bounds,
                    scale,
                    reply,
                });
            }
            PageCmd::Render { every } => {
                if let Some(PageCmd::Render { every: previous }) = out.last_mut() {
                    *previous = *previous || every;
                } else {
                    out.push(PageCmd::Render { every });
                }
            }
            // A pointer move replaces an earlier one that only input marks
            // and renders separate it from: the page hears the latest
            // position once, as Chromium dispatches coalesced moves once a
            // frame. Nothing is passed: a press, key, click, live turn or
            // navigation between two moves keeps both. Every mark stays, so
            // input-to-present still counts from the first move (Z row S2:
            // eleven queued 430 ms moves held a navigation for 5.5 s).
            PageCmd::Input(PageInput::MouseMove { x, y }) => {
                let earlier = out
                    .iter()
                    .rposition(|c| !matches!(c, PageCmd::NoteInput { .. } | PageCmd::Render { .. }))
                    .filter(|&i| matches!(out[i], PageCmd::Input(PageInput::MouseMove { .. })));
                if let Some(i) = earlier {
                    out.remove(i);
                }
                out.push(PageCmd::Input(PageInput::MouseMove { x, y }));
            }
            PageCmd::PumpLive { elapsed_ms, reply } => {
                if matches!(out.last(), Some(PageCmd::PumpLive { .. })) {
                    if let Some(PageCmd::PumpLive {
                        reply: previous, ..
                    }) = out.pop()
                    {
                        let _ = previous.send(crate::LivePump::default());
                    }
                }
                out.push(PageCmd::PumpLive { elapsed_ms, reply });
            }
            other => out.push(other),
        }
    }
    // Pointer input queued ahead of a navigation is for a document the
    // user has left (Z row S2, reopened: a load held the thread for 11 s
    // while the pointer moved; the thirty moves, leaves, presses and
    // clicks waiting when the tab was closed were each a ~100 ms hover
    // restyle on the leaving page, every listener refused by the
    // navigation's interrupt word, and the new page loaded three seconds
    // late). A queued click is answered with nothing: the window does not
    // wait on it. Keys and scrolls are left alone; input marks stay for
    // the latency figure; input behind the navigation is the next
    // document's.
    let leaving = out.iter().rposition(|cmd| {
        matches!(
            cmd,
            PageCmd::Navigate { .. } | PageCmd::LoadHtml { .. } | PageCmd::Reload { .. } | PageCmd::Close
        )
    });
    let Some(at) = leaving else {
        return out;
    };
    let mut kept = Vec::with_capacity(out.len());
    let mut dropped = 0usize;
    for (i, cmd) in out.into_iter().enumerate() {
        if i < at {
            match cmd {
                PageCmd::Input(PageInput::MouseMove { .. }
                    | PageInput::MouseLeave
                    | PageInput::MouseDown { .. }
                    | PageInput::SecondaryDown { .. }
                    | PageInput::SecondaryUp { .. },
                ) => {
                    dropped += 1;
                    continue;
                }
                PageCmd::Input(PageInput::Click { reply, .. }) => {
                    let _ = reply.send(crate::ClickOutcome::default());
                    dropped += 1;
                    continue;
                }
                _ => {}
            }
        }
        kept.push(cmd);
    }
    if dropped > 0 {
        tracing::info!(dropped, "Pointer input queued ahead of a navigation dropped: the page is being left");
    }
    kept
}

fn scroll_dy_for_unhandled_key(key_code: u32, key: &str) -> Option<f32> {
    match (key_code, key) {
        (0x28, _) => Some(-40.0),
        (0x26, _) => Some(40.0),
        (0x22, _) | (0, " ") => Some(-600.0),
        (0x21, _) => Some(600.0),
        (0x23, _) => Some(-f32::MAX),
        (0x24, _) => Some(f32::MAX),
        _ => None,
    }
}

impl std::fmt::Debug for PageHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageHandle")
            .field("view", &self.view)
            .field("last_sent_nav_id", &self.last_sent_nav_id())
            .finish()
    }
}

#[cfg(all(test, feature = "headless"))]
mod tests {
    use super::*;
    use crate::EngineConfig;
    use rustkit_viewhost::Bounds;
    use tokio::sync::oneshot;
    use url::Url;

    fn pair() -> (PageHandle, PageEndpoint) {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
            .expect("view");
        connect(engine, view)
    }

    fn eval(handle: &PageHandle, endpoint: &mut PageEndpoint, script: &str) -> String {
        let (tx, mut rx) = oneshot::channel();
        handle.send(PageCmd::Eval { script: script.to_string(), reply: tx });
        endpoint.drain(None);
        rx.try_recv().expect("eval reply is ready when drain returns").expect("eval")
    }

    /// Commands apply on the caller, in send order, and a changed frame
    /// comes back as a commit. Nothing is deferred to another thread.
    #[test]
    fn commands_apply_on_the_calling_thread_before_drain_returns() {
        let (handle, mut endpoint) = pair();
        let (tx, mut rx) = oneshot::channel();
        handle.send(PageCmd::LoadHtml {
            html: "<p id=\"t\">hi</p>".into(),
            nav_id: 0,
            reply: tx,
        });
        let (tx1, mut rx1) = oneshot::channel();
        handle.send(PageCmd::Eval {
            script: "window.step = 1".into(),
            reply: tx1,
        });
        let (tx2, mut rx2) = oneshot::channel();
        handle.send(PageCmd::Eval {
            script: "window.step = window.step + 1; document.getElementById('t').textContent".into(),
            reply: tx2,
        });
        // No reply exists until drain, and drain does not leave this thread.
        assert!(rx.try_recv().is_err(), "a command ran before drain");
        endpoint.drain(None);
        assert!(rx.try_recv().expect("load reply").is_ok());
        assert!(rx1.try_recv().expect("first eval reply").is_ok());
        let text = rx2.try_recv().expect("second eval reply").expect("eval ok");
        assert!(text.contains("hi"), "{text}");
        // Load, then step = 1, then step + 1, all in that one drain.
        assert_eq!(
            eval(&handle, &mut endpoint, "String(window.step)"),
            r#"String("2")"#,
            "the later command ran after the earlier one"
        );
        handle.send(PageCmd::Render { every: false });
        endpoint.drain(None);
        let events = handle.drain_events();
        assert!(
            events.iter().any(|event| matches!(event, PageEvent::Commit(commit) if commit.generation > 0)),
            "a frame change is a commit: {events:?}"
        );
        assert_eq!(handle.last_sent_nav_id(), 0, "LoadHtml is not a Navigate");
        handle.send(PageCmd::Navigate {
            url: Url::parse("data:text/html,ok").unwrap(),
            nav_id: 1,
        });
        // The counter moves when the command is sent. The engine still
        // assigns the id its cancel path reads.
        assert_eq!(handle.last_sent_nav_id(), 1);
        endpoint.drain(None);
        assert_eq!(handle.last_sent_nav_id(), 1);
        let _ = handle.drain_events();
    }

    /// CLOCK3: the window thread's live turn (`PumpLive`, the loop with the
    /// content thread off) is on the real clock: its `elapsed_ms` is real time
    /// that has passed, so a 1000 ms chain held back for 5000 ms runs its
    /// overdue link once and sets the next from then. A stepped turn would
    /// run five (1000 to 5000 ms). The period is long because the turn's
    /// reach grows with the real time it takes: only a whole-second stall
    /// inside the turn would let the next link into it.
    #[test]
    fn the_window_threads_live_turn_does_not_replay_a_timeout_chain() {
        let (handle, mut endpoint) = pair();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let (tx, _rx) = oneshot::channel();
        handle.send(PageCmd::LoadHtml { html: "<p>chain</p>".into(), nav_id: 0, reply: tx });
        endpoint.drain(Some(&rt));
        let pump = |handle: &PageHandle, endpoint: &mut PageEndpoint, elapsed_ms: u64| {
            let (tx, mut rx) = oneshot::channel();
            handle.send(PageCmd::PumpLive { elapsed_ms, reply: tx });
            endpoint.drain(Some(&rt));
            rx.try_recv().expect("the turn ran")
        };
        // The turn after the load (its timers already ran in the load's pass).
        pump(&handle, &mut endpoint, 0);
        // A turn told 5 s passed runs at most one link of the chain, never the five it held back.
        // The exact count (one link) is pinned on the supplied clock in the bindings
        // (`a_chain_set_between_real_turns_does_not_replay_a_gap`): here no real time passes, and a
        // timeout set in a real turn's page waits its delay on the page clock (LINKS-SLOW).
        eval(&handle, &mut endpoint, "window.n = 0; (function link() { n++; setTimeout(link, 1000); })(); 0");
        pump(&handle, &mut endpoint, 5000);
        let n = eval(&handle, &mut endpoint, "String(window.n)");
        assert!(
            n == r#"String("1")"# || n == r#"String("2")"#,
            "the chain replayed the links the 5000 ms held back: {n}"
        );
    }

    /// CH2.E4: what the pointer is over reaches the window as events: the
    /// cursor and the tooltip when they change, the context menu when the
    /// page let a right press through, nothing when it cancelled it.
    #[test]
    fn hover_and_a_right_press_reach_the_window_as_events() {
        let (handle, mut endpoint) = pair();
        let (tx, _rx) = oneshot::channel();
        handle.send(PageCmd::LoadHtml {
            html: "<body style='margin:0'><a id=l href='#x' title='go' style='display:block;height:40px'>link</a>\
                   <div style='height:40px'></div></body>"
                .into(),
            nav_id: 0,
            reply: tx,
        });
        endpoint.drain(None);
        let _ = handle.drain_events();
        let hover = |handle: &PageHandle, endpoint: &mut PageEndpoint, input: PageInput| {
            handle.send(PageCmd::Input(input));
            endpoint.drain(None);
            handle
                .drain_events()
                .into_iter()
                .filter_map(|e| match e {
                    PageEvent::Cursor(c) => Some(format!("cursor {c}")),
                    PageEvent::Tooltip(t) => Some(format!("tooltip {t:?}")),
                    PageEvent::ContextMenu { x, y } => Some(format!("menu {x},{y}")),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            hover(&handle, &mut endpoint, PageInput::MouseMove { x: 300.0, y: 10.0 }),
            vec!["cursor pointer".to_string(), "tooltip Some(\"go\")".to_string()]
        );
        assert!(hover(&handle, &mut endpoint, PageInput::MouseMove { x: 310.0, y: 12.0 }).is_empty(), "no change, no event");
        assert_eq!(
            hover(&handle, &mut endpoint, PageInput::MouseMove { x: 300.0, y: 60.0 }),
            vec!["cursor default".to_string(), "tooltip None".to_string()]
        );
        assert_eq!(hover(&handle, &mut endpoint, PageInput::SecondaryDown { x: 300.0, y: 60.0 }), vec!["menu 300,60".to_string()]);
        eval(&handle, &mut endpoint, "document.addEventListener('contextmenu', function (e) { e.preventDefault(); })");
        assert!(hover(&handle, &mut endpoint, PageInput::SecondaryDown { x: 300.0, y: 60.0 }).is_empty(), "cancelled: no menu");
        assert_eq!(
            hover(&handle, &mut endpoint, PageInput::MouseMove { x: 300.0, y: 10.0 }),
            vec!["cursor pointer".to_string(), "tooltip Some(\"go\")".to_string()]
        );
        assert_eq!(
            hover(&handle, &mut endpoint, PageInput::MouseLeave),
            vec!["cursor default".to_string(), "tooltip None".to_string()]
        );
    }
}
