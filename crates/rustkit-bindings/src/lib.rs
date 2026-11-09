//! # RustKit Bindings
//!
//! JavaScript-to-DOM bindings for the RustKit browser engine.
//!
//! ## Design Goals
//!
//! 1. **Web compatibility**: Match browser API behavior
//! 2. **Type safety**: Safe conversion between JS and Rust types
//! 3. **Performance**: Minimize overhead at the boundary
//! 4. **Extensibility**: Easy to add new APIs

mod dom;
mod inner_text;

thread_local! {
    static LOCATION_ARMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static LOCATION_HOOK: std::sync::atomic::AtomicPtr<()> =
    std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// `url`, `replace` (`location.replace` / reload).
pub type LocationDiag = fn(&str, bool);

pub fn set_location_armed(on: bool) {
    LOCATION_ARMED.with(|cell| cell.set(on));
}

pub fn set_location_hook(hook: Option<LocationDiag>) {
    let ptr = match hook {
        Some(hook) => hook as *mut (),
        None => std::ptr::null_mut(),
    };
    LOCATION_HOOK.store(ptr, std::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn note_location(url: &str, replace: bool) {
    if !LOCATION_ARMED.with(std::cell::Cell::get) {
        return;
    }
    let ptr = LOCATION_HOOK.load(std::sync::atomic::Ordering::Relaxed);
    if ptr.is_null() {
        return;
    }
    // SAFETY: stored from a `LocationDiag` function pointer. The engine
    // installs one static function for the process.
    let hook: LocationDiag = unsafe { std::mem::transmute(ptr) };
    hook(url, replace);
}
mod net_bridge;
pub use dom::editing::SharedEditStates;
pub use net_bridge::{NetDelivery, NetRequest};
pub use rustkit_js::{FetchedModule, ModuleHandle, ModuleState};
#[cfg(test)]
mod net_bridge_tests;
#[cfg(test)]
mod web_xhr_tests;
#[cfg(test)]
mod web_fetch_tests;
#[cfg(test)]
mod web_components_tests;
#[cfg(test)]
mod web_streams_tests;
#[cfg(test)]
mod web_interfaces_tests;
#[cfg(test)]
mod dnd_tests;
#[cfg(test)]
mod web_blob_tests;
#[cfg(test)]
mod web_utils_tests;
#[cfg(test)]
mod dom_utils_tests;
#[cfg(test)]
mod web_cssom_tests;
#[cfg(test)]
mod node_apis_tests;
#[cfg(test)]
mod form_controls_tests;
#[cfg(test)]
mod web_history_tests;
#[cfg(test)]
mod web_scripts_tests;
#[cfg(test)]
mod meta_refresh_noscript_tests;
#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod forced_layout_budget_tests;
#[cfg(test)]
mod dirty_mark_animation_tests;
#[cfg(test)]
mod range_tests;
#[cfg(test)]
mod selection_tests;
#[cfg(test)]
mod scroll_tests;
#[cfg(test)]
mod observers_tests;
#[cfg(test)]
mod shadow_tests;
#[cfg(test)]
mod raw_text_inner_html_tests;
#[cfg(test)]
mod svg_name_case_tests;
#[cfg(test)]
mod rejection_event_tests;
#[cfg(test)]
mod event_target_ctor_tests;
#[cfg(test)]
mod legacy_tests;
#[cfg(test)]
mod reflect_tests;
#[cfg(test)]
mod editing_tests;
#[cfg(test)]
mod traversal_tests;
#[cfg(test)]
mod document_members_tests;
#[cfg(test)]
mod web_intl_tests;
#[cfg(test)]
mod web_match_media_tests;
#[cfg(test)]
mod mutation_observer_tests;
#[cfg(test)]
mod web_messaging_tests;
#[cfg(test)]
mod web_named_access_tests;
mod web_canvas;
#[cfg(test)]
mod web_canvas_tests;
mod web_crypto;
mod web_match_media;
mod web_scroll;
mod web_url;

pub use dom::{
    AnimatedStyles, BoxGeometry, ControlWrites, CssAnimationControl, CssAnimationInfo, ElementScrolls,
    ComputedLines, ForcedGeometry, ForcedLayout, GeometrySource, HitTester, SelectorMatchFn, SharedAnimatedStyles,
    SharedElementScrolls,
};
pub mod events;
pub use rustkit_canvas as canvas;
pub use web_canvas::{CanvasSnapshot, CanvasSource, CanvasTextExtent, CanvasTextMeasurer};

pub use events::{
    AnimationEventData, DataTransfer, DragEventData, DroppedFile, Event, EventDispatcher,
    EventListenerEntry, EventListenerOptions, EventPhase, ExtendedEventData, FocusManager,
    FocusVisibility, FocusableElement, HoverTracker, MessageEventData, PointerEventData,
    PointerLockState, PointerType, RafCallbackId, RafScheduler, Touch, TouchEventData,
    TransitionEventData, WheelDeltaMode, WheelEventData,
};

use rustkit_dom::{Document, NodeId};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use tracing::{debug, trace};
use url::Url;

/// Errors that can occur in bindings.
#[derive(Error, Debug)]
pub enum BindingError {
    #[error("DOM error: {0}")]
    DomError(String),

    #[error("JS error: {0}")]
    JsError(#[from] JsError),

    #[error("Type error: expected {expected}, got {got}")]
    TypeError { expected: String, got: String },

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}

/// Unique identifier for an event listener.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ListenerId(u64);

/// An external classic script that script inserted
/// ([`DomBindings::take_inserted_scripts`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InsertedScript {
    /// The element's raw NodeId.
    pub node: usize,
    /// Its absolute `src`.
    pub url: String,
    /// Not async (HTML §4.12.1.1: `async = false` cleared the element's
    /// "force async"): it runs after the in-order scripts inserted before
    /// it, not as soon as it arrives.
    pub in_order: bool,
    /// Where the page's script that inserted it was (its innermost frame
    /// in a named script), when the runtime could place it.
    pub inserted_by: Option<rustkit_js::ScriptPosition>,
    /// When the page inserted it. The engine takes the queue later, at its
    /// next drain; the gap is the script's `queued_ms` in the Inspector.
    pub inserted_at: std::time::Instant,
}

impl ListenerId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// An event listener registration.
#[derive(Debug)]
pub struct EventListener {
    pub id: ListenerId,
    pub node_id: NodeId,
    pub event_type: String,
    pub callback: String, // JS code to execute
    pub capture: bool,
}

/// Mouse event data for JavaScript binding.
#[derive(Debug, Clone, Default)]
pub struct MouseEventBindingData {
    pub client_x: f64,
    pub client_y: f64,
    pub screen_x: f64,
    pub screen_y: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub button: i16,
    pub buttons: u16,
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
    pub meta_key: bool,
    /// How far the pointer moved since the last move event.
    pub movement_x: f64,
    pub movement_y: f64,
    /// The element the pointer came from or went to (raw NodeId), for the
    /// over/out/enter/leave events.
    pub related_target: Option<usize>,
    /// `UIEvent.detail` (the click count). `None`: 1 for a press, a
    /// release or a `click`, else 0.
    pub detail: Option<u32>,
}

/// Keyboard event data for JavaScript binding.
#[derive(Debug, Clone, Default)]
pub struct KeyboardEventBindingData {
    pub key: String,
    pub code: String,
    pub repeat: bool,
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub shift_key: bool,
    pub meta_key: bool,
    pub location: u32,
}

/// Focus event data for JavaScript binding.
#[derive(Debug, Clone, Default)]
pub struct FocusEventBindingData {
    pub related_target: Option<u64>,
}

/// Input event data for JavaScript binding.
#[derive(Debug, Clone, Default)]
pub struct InputEventBindingData {
    pub data: Option<String>,
    pub input_type: String,
    pub is_composing: bool,
}

/// Event data for JavaScript dispatch.
#[derive(Debug, Clone)]
pub enum EventData {
    Mouse(MouseEventBindingData),
    Keyboard(KeyboardEventBindingData),
    Focus(FocusEventBindingData),
    Input(InputEventBindingData),
}

/// Location object (window.location).
#[derive(Debug, Clone)]
pub struct Location {
    pub href: String,
    pub protocol: String,
    pub host: String,
    pub hostname: String,
    pub port: String,
    pub pathname: String,
    pub search: String,
    pub hash: String,
    pub origin: String,
}

impl Location {
    /// Create a Location from a URL.
    pub fn from_url(url: &Url) -> Self {
        Self {
            href: url.to_string(),
            protocol: format!("{}:", url.scheme()),
            host: url
                .host_str()
                .map(|h| {
                    if let Some(port) = url.port() {
                        format!("{}:{}", h, port)
                    } else {
                        h.to_string()
                    }
                })
                .unwrap_or_default(),
            hostname: url.host_str().unwrap_or("").to_string(),
            port: url.port().map(|p| p.to_string()).unwrap_or_default(),
            pathname: url.path().to_string(),
            search: url.query().map(|q| format!("?{}", q)).unwrap_or_default(),
            hash: url
                .fragment()
                .map(|f| format!("#{}", f))
                .unwrap_or_default(),
            origin: url.origin().unicode_serialization(),
        }
    }

    /// Create a Location from a string.
    pub fn from_string(href: &str) -> Result<Self, BindingError> {
        let url = Url::parse(href).map_err(|e| BindingError::InvalidArgument(e.to_string()))?;
        Ok(Self::from_url(&url))
    }
}

impl Default for Location {
    fn default() -> Self {
        Self {
            href: "about:blank".to_string(),
            protocol: "about:".to_string(),
            host: String::new(),
            hostname: String::new(),
            port: String::new(),
            pathname: "blank".to_string(),
            search: String::new(),
            hash: String::new(),
            origin: "null".to_string(),
        }
    }
}

/// History object (window.history).
#[derive(Debug, Clone, Default)]
pub struct JsHistory {
    /// Number of entries in the session history.
    pub length: usize,
    /// Scroll restoration mode.
    pub scroll_restoration: String,
    /// Current state (serialized).
    pub state: Option<String>,
}

impl JsHistory {
    /// Create a new History with default values.
    pub fn new() -> Self {
        Self {
            length: 1,
            scroll_restoration: "auto".to_string(),
            state: None,
        }
    }

    /// Update from history state.
    pub fn update(&mut self, length: usize, state: Option<String>) {
        self.length = length;
        self.state = state;
    }
}

/// Navigator object (window.navigator).
#[derive(Debug, Clone)]
pub struct JsNavigator {
    /// Browser name.
    pub app_name: String,
    /// Browser version.
    pub app_version: String,
    /// User agent string.
    pub user_agent: String,
    /// Platform.
    pub platform: String,
    /// Language.
    pub language: String,
    /// Languages in preference order.
    pub languages: Vec<String>,
    /// Online status.
    pub online: bool,
    /// Cookie enabled.
    pub cookie_enabled: bool,
    /// Hardware concurrency (CPU cores).
    pub hardware_concurrency: usize,
}

impl Default for JsNavigator {
    fn default() -> Self {
        Self {
            app_name: "RustKit".to_string(),
            app_version: "1.0".to_string(),
            user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) RustKit/1.0".to_string(),
            platform: "Win32".to_string(),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
            online: true,
            cookie_enabled: true,
            hardware_concurrency: num_cpus::get(),
        }
    }
}

/// Window object state.
pub struct WindowState {
    pub location: Location,
    pub history: JsHistory,
    pub navigator: JsNavigator,
    pub document: Option<Rc<Document>>,
    pub name: String,
    pub inner_width: f64,
    pub inner_height: f64,
    pub outer_width: f64,
    pub outer_height: f64,
    pub device_pixel_ratio: f64,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            location: Location::default(),
            history: JsHistory::new(),
            navigator: JsNavigator::default(),
            document: None,
            name: String::new(),
            inner_width: 800.0,
            inner_height: 600.0,
            outer_width: 800.0,
            outer_height: 600.0,
            device_pixel_ratio: 1.0,
        }
    }
}

/// IPC message from JavaScript.
#[derive(Debug, Clone)]
pub struct IpcMessage {
    /// The message payload (JSON string from postMessage)
    pub payload: String,
}

/// IPC callback type for handling messages from JavaScript.
pub type IpcCallback = Box<dyn Fn(IpcMessage) + Send + Sync>;

/// Event targets for `window` and `document`, timers on a virtual clock, and
/// the sink for exceptions nothing catches.
///
/// An exception thrown inside an event listener or a timer callback does
/// not propagate to whoever fired it (in a browser it goes to
/// `window.onerror`), so those are caught here and handed to the runtime's
/// host natives (`__rk.reportError`, which page script cannot reach) for the
/// engine to collect. The one thing that does
/// propagate is Boa's loop-iteration limit, which a script cannot catch.
///
/// Timers run on a virtual clock the engine advances (`__rustkit_run_timers`),
/// so a page's `setTimeout(f, 2000)` runs without the load waiting 2s of wall
/// time. Promise reactions run when the enclosing evaluation returns, not
/// between two timer callbacks.
/// The Web API members the bindings report as unimplemented when page
/// script reaches them (`__rk.unsupported` in the bindings' scripts, a
/// native page script cannot reach). Nothing else is ever reported.
pub const UNSUPPORTED_MEMBERS: [(&str, &str); 10] = [
    ("Element", "animate(pseudoElement)"),
    ("CSSAnimation", "currentTime"),
    ("CSSAnimation", "playbackRate"),
    ("CSSTransition", "pause"),
    ("XMLHttpRequest", "open(async=false)"),
    ("XMLHttpRequest", "responseXML"),
    ("CustomElementRegistry", "define(extends)"),
    ("ReadableStream", "getReader(byob)"),
    ("DOMParser", "parseFromString(XML)"),
    ("Window", "named access (iframe name)"),
];

const PAGE_LIFECYCLE_JS: &str = r#"
(function () {
    // Described natively (no page code runs) and queued in the runtime.
    var reportError = __rk.reportError;
    function report(e) {
        try { reportError(e); } catch (_) {}
    }

    function makeEventTarget(target) {
        var listeners = {};
        target.addEventListener = function (type, cb) {
            if (typeof cb !== 'function' && !(cb && typeof cb.handleEvent === 'function')) return;
            var list = listeners[type] || (listeners[type] = []);
            if (list.indexOf(cb) < 0) list.push(cb);
        };
        target.removeEventListener = function (type, cb) {
            var list = listeners[type];
            if (!list) return;
            var i = list.indexOf(cb);
            if (i >= 0) list.splice(i, 1);
        };
        target.dispatchEvent = function (event) {
            if (!event.target) event.target = target;
            event.currentTarget = target;
            var list = (listeners[event.type] || []).slice();
            var handler = target['on' + event.type];
            if (typeof handler === 'function') list.push(handler);
            for (var i = 0; i < list.length; i++) {
                try {
                    var cb = list[i];
                    if (typeof cb === 'function') cb.call(target, event); else cb.handleEvent(event);
                } catch (e) { report(e); }
            }
            return !event.defaultPrevented;
        };
    }
    makeEventTarget(window);
    makeEventTarget(document);

    var now = 0, nextId = 1, timers = [];
    // Real time, for the live turn's slice. Taken now, before page script
    // can replace `Date.now`.
    var realNow = Date.now;
    window.__rustkit_fire = function (targetName, type) {
        var target = targetName === 'document' ? document : window;
        target.dispatchEvent({
            type: type, bubbles: false, cancelable: false, defaultPrevented: false,
            target: target, currentTarget: null, timeStamp: now, isTrusted: true,
            preventDefault: function () { this.defaultPrevented = true; },
            stopPropagation: function () {}, stopImmediatePropagation: function () {}
        });
    };

    function schedule(cb, ms, args, repeat, frame) {
        var delay = Number(ms) || 0;
        if (delay < 0) delay = 0;
        var id = nextId++;
        timers.push({ id: id, seq: id, due: now + delay, cb: cb, args: args,
                      every: repeat ? Math.max(delay, 1) : 0, frame: !!frame });
        return id;
    }
    function clearTimer(id) {
        for (var i = 0; i < timers.length; i++) {
            if (timers[i].id === id) { timers.splice(i, 1); return; }
        }
    }
    window.setTimeout = function (cb, ms) {
        return schedule(cb, ms, Array.prototype.slice.call(arguments, 2), false);
    };
    window.setInterval = function (cb, ms) {
        return schedule(cb, ms, Array.prototype.slice.call(arguments, 2), true);
    };
    window.clearTimeout = clearTimer;
    window.clearInterval = clearTimer;
    window.requestAnimationFrame = function (cb) {
        return schedule(function () { cb(now); }, 16, [], false, true);
    };
    window.cancelAnimationFrame = clearTimer;
    window.queueMicrotask = function (cb) {
        if (typeof cb !== 'function') throw new TypeError("Failed to execute 'queueMicrotask' on 'Window': The callback provided as parameter 1 is not a function.");
        Promise.resolve().then(function () { try { cb(); } catch (e) { report(e); } });
    };

    function call(timer) {
        try {
            if (typeof timer.cb === 'function') timer.cb.apply(window, timer.args);
            else (0, eval)(String(timer.cb));
        } catch (e) { report(e); }
    }
    // Run due timers in (due, scheduling) order until the virtual clock
    // would pass `horizon` ms or `max` callbacks have run. The load's clock
    // has no frames, so an animation frame there is a 16 ms timer; the live
    // loop keeps them for its one round a turn (`timersOnly`).
    //
    // `until` (live loop only) is a time on the real clock: once a callback
    // ends past it, the rest wait for the next turn, as when `max` is hit.
    function runTimers(horizon, max, timersOnly, until) {
        var ran = 0;
        while (ran < max && !(until && realNow() >= until)) {
            var best = -1;
            for (var i = 0; i < timers.length; i++) {
                var t = timers[i];
                if (timersOnly && t.frame) continue;
                if (best < 0 || t.due < timers[best].due ||
                    (t.due === timers[best].due && t.seq < timers[best].seq)) best = i;
            }
            if (best < 0 || timers[best].due > horizon) break;
            var timer = timers[best];
            // A timer a cut-short live turn owed runs after the clock has
            // moved on; the clock does not go back for it.
            if (timer.due > now) now = timer.due;
            // An interval that fell behind (a live turn held it back) is not
            // caught up: its next run counts from now, as in a browser.
            if (timer.every) {
                timer.due += timer.every;
                if (timer.due <= now) timer.due = now + timer.every;
                timer.seq = nextId++;
            }
            else timers.splice(best, 1);
            ran++;
            call(timer);
        }
        return ran;
    }
    window.__rustkit_run_timers = function (horizon, max) {
        return runTimers(horizon, max, false);
    };
    // The load's clock, one callback at a time, so the host can publish a
    // frame between them. A negative return is `-callbacks` and means the
    // horizon still has timers due. Zero frame budget never stops early.
    window.__rustkit_run_timers_framed = function (horizon, max, frameMs) {
        var start = Date.now();
        var ran = 0;
        while (ran < max) {
            var n = runTimers(horizon, 1, false);
            if (n === 0) break;
            ran += n;
            if (frameMs > 0 && (Date.now() - start) >= frameMs) return -ran;
        }
        return ran;
    };
    // The live loop's clock: `delta` ms of real time have passed. Run what
    // came due and leave the clock there, so a timer set next (by a click,
    // say) counts from now and not from the last callback. With callbacks
    // still owed (the cap, or the slice below), the clock moves on all the
    // same: what is owed is overdue, and runs first on the next turn. (Left
    // behind, the clock stood still under a `setTimeout(0)` chain, each new
    // link due before every later timer and frame, which never ran.)
    //
    // A turn is one frame however long it took: after its timers, the
    // animation frame callbacks that are due run once, at the turn's time.
    // One asked for during the round is due a frame later. (As 16 ms timers
    // a turn of 3 s ran 187 of them back to back.)
    //
    // `slice` ms of real time (0: no limit) bounds the turn's timers, as
    // `max` does: a page that chains `setTimeout(0)` gets the window back
    // between its callbacks, rather than once 1000 of them have run
    // (seconds of them, until the watchdog stopped one part-way;
    // 2026-10-22). The turn's frame still comes.
    window.__rustkit_advance_timers = function (delta, max, slice) {
        var target = now + delta;
        var until = slice > 0 ? realNow() + slice : 0;
        var ran = runTimers(target, max, true, until);
        if (ran >= max) {
            if (now < target) now = target;
            return ran;
        }
        if (now < target) now = target;
        var round = [];
        for (var i = 0; i < timers.length; i++) {
            if (timers[i].frame && timers[i].due <= now) round.push(timers[i]);
        }
        for (var j = 0; j < round.length && ran < max; j++) {
            // Cancelled by an earlier callback of the round.
            var at = timers.indexOf(round[j]);
            if (at < 0) continue;
            timers.splice(at, 1);
            ran++;
            call(round[j]);
        }
        return ran;
    };
    // Like `__rustkit_advance_timers`, but stop between callbacks once
    // `frameMs` of wall time has passed so the host can publish a frame.
    // A negative return is `-callbacks` and means timers are still due.
    // Zero frame budget never stops early. Callbacks that finish inside
    // the budget run in this call, and the clock and animation-frame round
    // match `__rustkit_advance_timers`.
    //
    // The elapsed delta is applied even when a callback ran long enough to
    // stop the turn. Leaving the clock at that callback's due time drops
    // the time already spent, so the next interval waits a full extra
    // period after Stop. Timers still due are at or before `target` and
    // run on the next turn.
    //
    // A turn that yields still has its frame (#776): its animation-frame
    // round runs before it returns. Returning first, a `setTimeout(0)`
    // chain that outlasted every frame starved `requestAnimationFrame`.
    window.__rustkit_advance_timers_framed = function (delta, max, frameMs) {
        var target = now + delta;
        var start = Date.now();
        var ran = 0;
        var yielded = false;
        while (ran < max) {
            var n = runTimers(target, 1, true);
            if (n === 0) break;
            ran += n;
            if (frameMs > 0 && (Date.now() - start) >= frameMs) {
                yielded = true;
                break;
            }
        }
        if (now < target) now = target;
        if (ran >= max) return yielded ? -ran : ran;
        var round = [];
        for (var i = 0; i < timers.length; i++) {
            if (timers[i].frame && timers[i].due <= now) round.push(timers[i]);
        }
        for (var j = 0; j < round.length && ran < max; j++) {
            var at = timers.indexOf(round[j]);
            if (at < 0) continue;
            timers.splice(at, 1);
            ran++;
            call(round[j]);
        }
        return yielded ? -ran : ran;
    };
    // Ms until the earliest timer is due (0 when overdue), -1 with none set.
    // The virtual clock as it stands: the engine's animation clock.
    window.__rustkit_now = function () { return now; };
    window.__rustkit_next_timer = function () {
        var due = -1;
        for (var i = 0; i < timers.length; i++) {
            if (due < 0 || timers[i].due < due) due = timers[i].due;
        }
        return due < 0 ? -1 : Math.max(0, due - now);
    };
})();
"#;

/// Which object a page lifecycle event is fired at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleTarget {
    Window,
    Document,
}

/// What script's writes to the Rust DOM have invalidated since the last
/// flush (the DOM-bindings rung-0 pin §3 buckets). A later variant covers
/// the earlier ones' work, so marks combine by `max`.
///
/// - structure insert/remove/move, and style-affecting attributes
///   (`style`, `class`, `id`) → `Style`;
/// - a text content change → `Layout`;
/// - nothing written → `Clean`.
///
/// Script sets the bucket as it writes. The engine takes it once after the
/// script settles and relayouts if it isn't `Clean`, so a burst of writes
/// costs one relayout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum DomDirty {
    #[default]
    Clean,
    /// Boxes must be rebuilt, but no element's computed style changed.
    Layout,
    /// Styles must be recomputed, then layout.
    Style,
}

/// The pending `DomDirty` bucket, and whether script has written to the
/// DOM since the engine last published a layout. The bucket is the
/// engine's to take when script settles; the second is what a geometry
/// read inside the task asks, to lay the page out first (forced layout).
#[derive(Debug, Default)]
pub(crate) struct DirtyMark {
    bucket: Cell<DomDirty>,
    since_layout: Cell<bool>,
    /// The kind of the write that first dirtied the published layout
    /// (`attribute`, `class`, `style`, `text`, `html`, `insert`, `remove`,
    /// `animation`, `form`, `sheet`, `shadow`): the forced-layout log's
    /// cause. `None` while the layout is current.
    cause: Cell<Option<&'static str>>,
    /// The pending bucket holds a mark other than Web Animations' write of
    /// the animated-style layer (`mark_animation_layer`). Every other mark
    /// sets it, recorded in the `MutationLog` or not (`adoptedStyleSheets`,
    /// `attachShadow`, `customElements.define`, a CSS animation control,
    /// the embedder's `mark_dirty`), so the engine's paint-only animation
    /// frame is taken only when the layer write is all that changed.
    beyond_animation_layer: Cell<bool>,
    /// LAT7c: Web Animations wrote paint properties of the animated-style
    /// layer since the last published layout ([`LAYER_PAINT`],
    /// [`LAYER_TRANSFORM`]). The boxes did not move, so only the reads that
    /// see those values lay out first (`needs_layout_for`).
    layer_since_layout: Cell<u8>,
}

/// A layer write of paint properties (`opacity`, `color`,
/// `background-color`, the border colours): the computed style sees it.
pub(crate) const LAYER_PAINT: u8 = 1;
/// A layer write of `transform` (and `translate`/`rotate`/`scale`): the
/// computed style, the transformed rects, hit testing and the scroll
/// extents see it; offset and client boxes do not.
pub(crate) const LAYER_TRANSFORM: u8 = 2;

impl DirtyMark {
    /// A script write of kind `kind` that invalidated `bucket`.
    pub(crate) fn mark(&self, bucket: DomDirty, kind: &'static str) {
        if bucket != DomDirty::Clean {
            self.beyond_animation_layer.set(true);
            self.mark_animation_layer(bucket, kind);
        }
    }

    /// Web Animations wrote an element's animated-style layer, and nothing
    /// else: the one mark that leaves `beyond_animation_layer` alone. It is
    /// a script write all the same, and names its cause (`animation`) for
    /// the forced-layout log like every other mark.
    pub(crate) fn mark_animation_layer(&self, bucket: DomDirty, kind: &'static str) {
        if bucket != DomDirty::Clean {
            self.bucket.set(self.bucket.get().max(bucket));
            if !self.since_layout.replace(true) {
                self.cause.set(Some(kind));
            }
        }
    }

    /// LAT7c: Web Animations wrote only paint properties of an element's
    /// animated-style layer (`reads`: [`LAYER_PAINT`] and/or
    /// [`LAYER_TRANSFORM`]). The engine paints it when script settles (the
    /// bucket, and the paint-only frame: `beyond_animation_layer` is left
    /// alone), but no box moved: an offset or client read stays clean, and
    /// only the reads that see the values lay out (`needs_layout_for`).
    pub(crate) fn mark_animation_paint(&self, bucket: DomDirty, reads: u8) {
        if bucket != DomDirty::Clean {
            self.bucket.set(self.bucket.get().max(bucket));
            self.layer_since_layout.set(self.layer_since_layout.get() | reads);
        }
    }

    /// Whether the read `read` (the forced-layout log's read kinds) must
    /// lay the page out first: after any write a layout can see, or after a
    /// paint-layer write whose values it reads.
    pub(crate) fn needs_layout_for(&self, read: &str) -> bool {
        if self.since_layout.get() {
            return true;
        }
        let layer = self.layer_since_layout.get();
        match read {
            "computed_style" => layer != 0,
            "rect" | "hit_test" | "scroll_size" | "element_scroll" | "scroll_chain" | "window_scroll"
            | "scroll_to" => layer & LAYER_TRANSFORM != 0,
            _ => false,
        }
    }

    /// A script write the engine must lay out when script settles, but that
    /// no box of the published layout can see (it is inside a `display:
    /// none` subtree): a geometry read before then needs no forced layout.
    ///
    /// It is still a write the engine has to lay out, so it is beyond the
    /// animated-style layer: the paint-only animation frame must not skip
    /// it. A no-op write (`Clean`) is not a write and marks nothing.
    pub(crate) fn mark_unseen(&self, bucket: DomDirty) {
        if bucket != DomDirty::Clean {
            self.beyond_animation_layer.set(true);
            self.bucket.set(self.bucket.get().max(bucket));
        }
    }

    /// What first dirtied the published layout (see `cause`).
    pub(crate) fn cause(&self) -> Option<&'static str> {
        self.cause.get()
    }

    /// Whether the pending bucket holds a mark other than the animated-style
    /// layer's.
    pub(crate) fn beyond_animation_layer(&self) -> bool {
        self.beyond_animation_layer.get()
    }

    pub(crate) fn get(&self) -> DomDirty {
        self.bucket.get()
    }

    fn take(&self) -> DomDirty {
        self.beyond_animation_layer.set(false);
        self.bucket.replace(DomDirty::Clean)
    }

    /// Whether the published layout predates a script write.
    pub(crate) fn written_since_layout(&self) -> bool {
        self.since_layout.get()
    }

    /// A layout of the DOM as it is now was published.
    pub(crate) fn laid_out(&self) {
        self.since_layout.set(false);
        self.layer_since_layout.set(0);
        self.cause.set(None);
    }
}

/// A script task's forced layouts (`DomBindings::take_forced_report`): how
/// many, their time, and per cause how many, as `(write, read, count)` in
/// the order first seen. The write is the one that first dirtied the
/// layout the read found stale (`attribute`, `class`, `style`, `text`,
/// `html`, `insert`, `remove`, `animation`, `form`, `sheet`, `shadow`);
/// the read is what asked (`offset`, `rect`, `client`, `scroll_size`,
/// `computed_style`, `element_scroll`, `scroll_chain`, `inner_text`,
/// `laid_out`, `hit_test`, `window_scroll`, `scroll_to`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ForcedLayoutReport {
    pub count: u64,
    pub ms: f64,
    pub causes: Vec<(&'static str, &'static str, u64)>,
}

impl ForcedLayoutReport {
    pub(crate) fn note(&mut self, write: &'static str, read: &'static str, ms: f64) {
        self.ms += ms;
        match self.causes.iter_mut().find(|(w, r, _)| *w == write && *r == read) {
            Some(entry) => entry.2 += 1,
            None => self.causes.push((write, read, 1)),
        }
    }

    /// The most frequent cause as `write:read` (the first seen on a tie).
    pub fn top_cause(&self) -> Option<String> {
        let mut top: Option<&(&str, &str, u64)> = None;
        for entry in &self.causes {
            if top.is_none_or(|t| entry.2 > t.2) {
                top = Some(entry);
            }
        }
        top.map(|(w, r, _)| format!("{w}:{r}"))
    }

    /// Every cause, as `{write:read:n,...}`.
    pub fn causes_field(&self) -> String {
        let parts: Vec<String> = self.causes.iter().map(|(w, r, n)| format!("{w}:{r}:{n}")).collect();
        format!("{{{}}}", parts.join(","))
    }
}

/// Which kind of script DOM write dirtied the page. Diagnostics only: the
/// engine logs it as the cause of the relayout that follows, and nothing
/// decides on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MutationKind {
    /// An attribute other than `style` set or removed.
    Attribute,
    /// A node inserted, removed or moved, or an element's children replaced
    /// (`innerHTML`).
    ChildList,
    /// Text content (`textContent`, `innerText`, a text node's data).
    Text,
    /// The `style` attribute (`element.style.x = ...` writes through it).
    Style,
    /// A form control's value or checkedness.
    FormValue,
}

impl MutationKind {
    /// Every kind, in a fixed order (the order counts are kept and logged in).
    pub const ALL: [MutationKind; 5] = [
        MutationKind::Attribute,
        MutationKind::ChildList,
        MutationKind::Text,
        MutationKind::Style,
        MutationKind::FormValue,
    ];

    /// The snake_case name the relayout log uses.
    pub fn as_str(self) -> &'static str {
        match self {
            MutationKind::Attribute => "attribute",
            MutationKind::ChildList => "child_list",
            MutationKind::Text => "text",
            MutationKind::Style => "style",
            MutationKind::FormValue => "form_value",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// The script DOM writes that dirtied the page since the last take: the
/// first one (its kind and node) and a count per kind. Kept beside
/// `DomDirty`, marked by the same writes, taken by the engine when it takes
/// the dirty bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScriptMutations {
    /// The first dirtying write: its kind and its node as `tag#id` (`tag`
    /// alone without an id; empty for a non-element).
    pub first: Option<(MutationKind, String)>,
    counts: [u32; 5],
}

impl ScriptMutations {
    /// Record one dirtying write. `node` is only called for the first one.
    pub fn record(&mut self, kind: MutationKind, node: impl FnOnce() -> String) {
        if self.first.is_none() {
            self.first = Some((kind, node()));
        }
        let count = &mut self.counts[kind.index()];
        *count = count.saturating_add(1);
    }

    /// How many writes of `kind` were recorded.
    pub fn count(&self, kind: MutationKind) -> u32 {
        self.counts[kind.index()]
    }

    /// Every write recorded, of any kind.
    pub fn total(&self) -> u32 {
        self.counts.iter().sum()
    }

    pub fn is_empty(&self) -> bool {
        self.first.is_none()
    }
}

/// One script DOM write, per node (the incremental-restyle note's §3.1
/// `MutationLog`). Recorded beside `DomDirty` and `ScriptMutations` by the
/// same writes; the engine reads them to work out which nodes the writes can
/// restyle. NodeIds are raw: a write to a node's data keeps its NodeId.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationRecord {
    /// The node written to. For a [`MutationKind::ChildList`] record this is
    /// the parent whose children changed.
    pub node: usize,
    pub kind: MutationKind,
    /// The attribute set or removed (lowercased on an HTML element);
    /// `checked` for a checkedness change.
    pub attr: Option<String>,
    /// The value before the write, kept for `class` and `id` only (`None`
    /// also when the attribute was absent).
    pub old: Option<String>,
    /// [`MutationKind::ChildList`]: the node inserted, removed or moved
    /// under `node`. `None` means all of `node`'s children were replaced
    /// (`innerHTML`, `textContent`, `innerText` on an element).
    pub child: Option<usize>,
}

/// The most records a [`MutationLog`] keeps between takes.
pub const MUTATION_LOG_CAP: usize = 4096;

/// The per-node script DOM writes since the last take, in write order.
/// Past [`MUTATION_LOG_CAP`] records it gives up: `full` is set, the
/// records are dropped, and the engine must treat the whole page as dirty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MutationLog {
    pub records: Vec<MutationRecord>,
    pub full: bool,
}

impl MutationLog {
    /// Append one record, or overflow into `full`.
    pub fn push(&mut self, record: MutationRecord) {
        if self.full {
            return;
        }
        if self.records.len() >= MUTATION_LOG_CAP {
            self.full = true;
            self.records = Vec::new();
            return;
        }
        self.records.push(record);
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty() && !self.full
    }
}

/// DOM bindings context.
pub struct DomBindings {
    runtime: RefCell<JsRuntime>,
    window: RefCell<WindowState>,
    event_listeners: RefCell<Vec<EventListener>>,
    /// The document `document`/`Node`/`Element` read from (see `dom`).
    dom_host: dom::SharedDomHost,
    /// What the page posted with `window.ipc.postMessage`, held on the
    /// Rust side (`__rk_ipc_post`), so a drain runs no script.
    ipc_queue: Rc<RefCell<Vec<IpcMessage>>>,
    /// Pending invalidation from script DOM writes (see `DomDirty`).
    /// Shared with the tree-write host functions, which mark it.
    dirty: Rc<DirtyMark>,
    /// What the writes behind `dirty` were (see `ScriptMutations`).
    mutations: Rc<RefCell<ScriptMutations>>,
    /// The same writes per node (see `MutationLog`).
    mutation_log: Rc<RefCell<MutationLog>>,
    /// The scroll offset script reads and writes (see `web_scroll`).
    scroll: web_scroll::SharedScroll,
    /// The clipboard and edit states edit commands use (see `dom::editing`).
    edit: Rc<dom::editing::EditContext>,
    /// Canvas 2D contexts and what they drew (see `web_canvas`).
    canvases: web_canvas::SharedCanvases,
}

impl DomBindings {
    /// Install the test-only host re-entry probe on this page's runtime.
    pub fn install_reentry_probe(&self) {
        self.runtime.borrow_mut().install_reentry_probe();
    }

    /// Create DOM bindings for a web document: no app bridge. The page
    /// sees no `window.ipc`, `window.hiwaveChrome` or `__rk_ipc_post`, as
    /// in Chromium (CH3.A7).
    pub fn new(runtime: JsRuntime) -> Result<Self, BindingError> {
        Self::build(runtime, false)
    }

    /// Create DOM bindings for one of the app's own documents (the chrome,
    /// a panel): with the app bridge, `window.ipc.postMessage` and
    /// `window.hiwaveChrome.postMessage`, whose messages
    /// [`Self::drain_ipc_queue`] hands to the app (CH2.E9, CH3.A7).
    pub fn new_app_owned(runtime: JsRuntime) -> Result<Self, BindingError> {
        Self::build(runtime, true)
    }

    fn build(mut runtime: JsRuntime, app_bridge: bool) -> Result<Self, BindingError> {
        debug!(app_bridge, "Initializing DOM bindings");

        // `window.ipc.postMessage` lands here (CH2.E9), in an app-owned
        // document only (CH3.A7). Registered before the setup scripts: a
        // global added after them made every page load measurably slower
        // (two app tests 1.4 s -> 3.0 s).
        let ipc_queue = Rc::new(RefCell::new(Vec::new()));
        if app_bridge {
            let queue = Rc::clone(&ipc_queue);
            runtime.register_host_function(
                "__rk_ipc_post",
                1,
                Box::new(move |args| {
                    if let Some(JsValue::String(payload)) = args.first() {
                        queue.borrow_mut().push(IpcMessage { payload: payload.clone() });
                    }
                    JsValue::Undefined
                }),
            )?;
        }

        // ECMAScript members Boa lacks and pages call unguarded (substr, Set methods, ...).
        runtime.evaluate_script(include_str!("web_legacy.js"))?;
        // Inject global objects
        Self::inject_globals(&mut runtime, app_bridge)?;
        let dom_host = dom::SharedDomHost::default();
        let dirty = Rc::new(DirtyMark::default());
        let mutations = Rc::new(RefCell::new(ScriptMutations::default()));
        let mutation_log = Rc::new(RefCell::new(MutationLog::default()));
        dom::install(&mut runtime, &dom_host, &dirty, &mutations, &mutation_log)?;
        // Event subclasses, geometry types and interface objects pages test with
        // instanceof/typeof; needs the wrappers dom::install just made (web_interfaces.js).
        runtime.evaluate_script(include_str!("web_interfaces.js"))?;
        // A constructible DataTransfer and the drag data store the engine's drag and
        // drop drives (web_dnd.js); replaces web_interfaces.js's interface-only one.
        runtime.evaluate_script(include_str!("web_dnd.js"))?;
        // Named access on the Window object: `<div id=x>` makes bare `x` the
        // element, through a WindowProperties object on the prototype chain
        // that asks the document when read (web_named_access.js).
        runtime.evaluate_with_host(include_str!("web_named_access.js"))?;
        // window.matchMedia over the engine's @media evaluator, MediaQueryList with
        // change events after a resize (web_match_media.js); needs MediaQueryList and
        // MediaQueryListEvent from web_interfaces.js.
        web_match_media::install(&mut runtime)?;
        // customElements and a constructible HTMLElement (web_components.js); wraps the
        // tree and attribute mutators the DOM install just defined.
        runtime.evaluate_with_host(include_str!("web_components.js"))?;
        // attachShadow, ShadowRoot, slots, event retargeting (web_shadow.js).
        runtime.evaluate_script(include_str!("web_shadow.js"))?;
        // Reflected IDL attributes: link.href, script.type, img.alt, a.target, el.tabIndex, ... (web_reflect.js).
        runtime.evaluate_script(include_str!("web_reflect.js"))?;
        // NodeFilter, TreeWalker, NodeIterator, createTreeWalker/createNodeIterator (web_traversal.js).
        runtime.evaluate_script(include_str!("web_traversal.js"))?;
        // document.location/fonts/forms/visibilityState/..., FontFace (web_document.js) and
        // DOMMatrix (web_dommatrix.js): members pages read without feature-testing.
        runtime.evaluate_script(include_str!("web_document.js"))?;
        runtime.evaluate_script(include_str!("web_dommatrix.js"))?;
        // document.styleSheets, CSSStyleSheet, CSS.supports/escape (web_cssom.js);
        // insertRule writes into the <style>'s text, which the engine restyles from.
        runtime.evaluate_script(include_str!("web_cssom.js"))?;
        // Checkedness, selectedness, form/button/label state, Image and Option (web_forms.js).
        runtime.evaluate_script(include_str!("web_forms.js"))?;
        // execCommand, navigator.clipboard and the user's edit commands over
        // rustkit-dom's TextEditState (dom/editing.js); needs InputEvent.
        let edit = Rc::new(dom::editing::EditContext::default());
        dom::editing::install(&mut runtime, &dom_host, &dirty, &mutations, &mutation_log, &edit)?;
        // history (pushState/replaceState/popstate) and the anchor URL parts;
        // needs the interface objects and window's EventTarget (web_history.js).
        runtime.evaluate_script(include_str!("web_history.js"))?;
        // window.scrollTo/scrollBy/scrollX/scrollY, Element.scrollTop/scrollIntoView (web_scroll.js).
        let scroll = web_scroll::SharedScroll::default();
        dom_host.borrow_mut().scroll_state = scroll.clone();
        web_scroll::install(&mut runtime, &scroll, &dom_host, &dirty)?;
        // canvas.getContext('2d'), CanvasRenderingContext2D and canvas.width/height,
        // recorded by rustkit-canvas for the engine to paint (web_canvas.js).
        let canvases = web_canvas::SharedCanvases::default();
        web_canvas::install(&mut runtime, &canvases, &dom_host)?;
        // IntersectionObserver and ResizeObserver that report, over the geometry and
        // scroll state (web_observers_live.js); replace the inert stubs.
        runtime.evaluate_with_host(include_str!("web_observers_live.js"))?;
        // MutationObserver that records every DOM write and delivers in a microtask; replaces
        // the inert stub through the dom.rs write hook (web_mutation_observer.js).
        runtime.evaluate_with_host(include_str!("web_mutation_observer.js"))?;
        // postMessage, MessageChannel/MessagePort, MessageEvent, requestIdleCallback (web_messaging.js).
        runtime.evaluate_with_host(include_str!("web_messaging.js"))?;
        // Script elements script inserts are prepared and run (web_scripts.js).
        runtime.evaluate_with_host(include_str!("web_scripts.js"))?;
        // Element.animate(), Animation, KeyframeEffect, document.timeline, getAnimations()
        // over the animation-frame clock; the values go into the inline style (web_animations.js).
        runtime.evaluate_with_host(include_str!("web_animations.js"))?;
        // CH3.A7: the engine's own globals (`__rustkit_*`, `__rk*`) stay
        // out of `Object.keys(window)` and for-in, which list none of them
        // in Chromium. They stay configurable: setup scripts redefine some.
        runtime.evaluate_script(
            "Object.getOwnPropertyNames(globalThis).forEach(function (n) { \
               if (n.indexOf('__rustkit_') !== 0 && n.indexOf('__rk') !== 0) return; \
               var d = Object.getOwnPropertyDescriptor(globalThis, n); \
               if (d && d.enumerable && d.configurable) { d.enumerable = false; Object.defineProperty(globalThis, n, d); } \
             });",
        )?;

        Ok(Self {
            runtime: RefCell::new(runtime),
            window: RefCell::new(WindowState::default()),
            event_listeners: RefCell::new(Vec::new()),
            dom_host,
            ipc_queue,
            dirty,
            mutations,
            mutation_log,
            scroll,
            edit,
            canvases,
        })
    }

    /// What the canvas element `node` has drawn on its 2D context: `None`
    /// when script never asked it for one.
    pub fn canvas_snapshot(&self, node: usize) -> Option<CanvasSnapshot> {
        self.canvases.borrow().snapshot(node)
    }

    /// A handle on this page's canvases that outlives a borrow of the
    /// bindings (the engine's display list reads it).
    pub fn canvas_source(&self) -> CanvasSource {
        CanvasSource(self.canvases.clone())
    }

    /// Whether any canvas drew, cleared or was resized since the last call:
    /// the engine paints again when it did.
    pub fn take_canvas_painted(&self) -> bool {
        self.canvases.borrow_mut().take_painted()
    }

    /// The text shaper `measureText` and canvas text use (the engine's).
    pub fn set_canvas_text_measurer(&self, measurer: Option<CanvasTextMeasurer>) {
        self.canvases.borrow_mut().set_measurer(measurer);
    }

    /// Measure `text` in the canvas `font` the way the canvas does.
    pub fn measure_canvas_text(&self, font: &str, text: &str) -> CanvasTextExtent {
        self.canvases.borrow().measure_font(font, text)
    }

    /// Record that script invalidated `bucket`. Marks combine: the pending
    /// bucket only grows until `take_dirty`.
    pub fn mark_dirty(&self, bucket: DomDirty) {
        // The embedder's marks are for its own flush, not script writes:
        // they do not make a geometry read lay the page out.
        if bucket != DomDirty::Clean {
            self.dirty.beyond_animation_layer.set(true);
        }
        self.dirty.bucket.set(self.dirty.get().max(bucket));
    }

    /// Whether the invalidation pending (until `take_dirty`) holds anything
    /// besides Web Animations' writes of the animated-style layer: any other
    /// script write, with a `MutationLog` record or without one, or a
    /// `mark_dirty` from the embedder. Read before `take_dirty`. The engine
    /// paints an animation frame without a layout only when this is false.
    pub fn dirty_beyond_animation_layer(&self) -> bool {
        self.dirty.beyond_animation_layer()
    }

    /// The invalidation pending since the last call, which is reset to
    /// `Clean`. The engine calls this once when script settles.
    pub fn take_dirty(&self) -> DomDirty {
        self.dirty.take()
    }

    /// The writes that dirtied the page since the last call (what, not how
    /// much to redo: that is `take_dirty`), which are reset. Diagnostics
    /// only. A `mark_dirty` from the embedder records nothing here.
    pub fn take_script_mutations(&self) -> ScriptMutations {
        self.mutations.take()
    }

    /// The writes since the last call, per node and in order (see
    /// `MutationLog`), which are reset. Taken with `take_dirty`; a
    /// `mark_dirty` from the embedder records nothing here.
    pub fn take_mutation_log(&self) -> MutationLog {
        self.mutation_log.take()
    }

    /// The `<input>`/`<textarea>` values script set since the last call, as
    /// (raw NodeId, value) in write order. The engine copies them into its
    /// edit state, which layout paints from, when it flushes `take_dirty`.
    pub fn take_value_writes(&self) -> Vec<(usize, String)> {
        self.dom_host.borrow_mut().take_value_writes()
    }

    /// The checkbox and radio checkedness changes since the last call, as
    /// (raw NodeId, checkedness) in write order; `None` means the control
    /// follows its `checked` attribute again. The engine styles and paints
    /// a control from these, when it flushes `take_dirty`.
    pub fn take_checked_writes(&self) -> Vec<(usize, Option<bool>)> {
        self.dom_host.borrow_mut().take_checked_writes()
    }

    /// The forms whose `submit` event ran uncancelled since the last call,
    /// as (the form's raw NodeId, the submitting button's). The engine
    /// builds the submission; nothing navigates until it does.
    pub fn take_submit_requests(&self) -> Vec<(usize, Option<usize>)> {
        self.dom_host.borrow_mut().take_submit_requests()
    }

    /// Whether script has asked to navigate to a URL since the requests
    /// were last taken. Takes nothing. A form submitted by script does not
    /// count: the embedder may not follow it (only GET is wired).
    pub fn navigation_requested(&self) -> bool {
        self.dom_host.borrow().navigation_requested()
    }

    /// The absolute URLs script asked to navigate to since the last call
    /// (`location.href = url`, `location.assign/replace`, `link.click()`),
    /// oldest first. Nothing navigates until the embedder does.
    pub fn take_navigation_requests(&self) -> Vec<String> {
        self.take_navigation_requests_with_replace().into_iter().map(|(url, _)| url).collect()
    }

    /// [`Self::take_navigation_requests`], each with whether it replaces the
    /// current history entry (`location.replace`, a reload, the document's
    /// own URL again, a meta refresh) instead of adding one.
    pub fn take_navigation_requests_with_replace(&self) -> Vec<(String, bool)> {
        self.dom_host.borrow_mut().take_navigation_requests()
    }

    /// The external classic scripts script inserted since the last call,
    /// oldest first. The engine fetches each and runs it as that element's
    /// script ([`Self::set_current_script`], then [`Self::fire_script_event`]).
    pub fn take_inserted_scripts(&self) -> Vec<InsertedScript> {
        self.dom_host.borrow_mut().take_inserted_scripts()
    }

    /// Back `document.cookie` with the browser's jar: `get` answers the
    /// cookies script may read for this document, `set` stores one
    /// assignment. Without this, `document.cookie` is a string that keeps
    /// nothing (a document with no URL has no cookies).
    pub fn set_cookie_access(
        &self,
        get: Box<dyn Fn() -> String>,
        set: Box<dyn Fn(&str)>,
    ) -> Result<(), BindingError> {
        let mut runtime = self.runtime.borrow_mut();
        runtime.register_host_function("__rustkit_cookie_get", 0, Box::new(move |_| JsValue::String(get())))?;
        runtime.register_host_function(
            "__rustkit_cookie_set",
            1,
            Box::new(move |args| {
                if let Some(JsValue::String(value)) = args.first() {
                    set(value);
                }
                JsValue::Null
            }),
        )?;
        runtime.evaluate_script(
            r#"(function (g) {
                var get = g.__rustkit_cookie_get, set = g.__rustkit_cookie_set;
                delete g.__rustkit_cookie_get;
                delete g.__rustkit_cookie_set;
                Object.defineProperty(g.document, 'cookie', {
                    get: function () { return get(); },
                    set: function (v) { set(String(v)); },
                    configurable: true, enumerable: true
                });
            })(globalThis);"#,
        )?;
        Ok(())
    }

    /// Use `clipboard` for this page's edit commands and
    /// `navigator.clipboard` (the engine's platform clipboard).
    pub fn set_clipboard(&self, clipboard: rustkit_dom::clipboard::SharedClipboard) {
        *self.edit.clipboard.borrow_mut() = clipboard;
    }

    /// The clipboard this page's edit commands use.
    pub fn clipboard(&self) -> rustkit_dom::clipboard::SharedClipboard {
        self.edit.clipboard.borrow().clone()
    }

    /// Whether script may read the clipboard (`navigator.clipboard.readText`,
    /// `execCommand('paste')`). Off by default, for web content: the
    /// clipboard is shared across views, so reading it would leak what the
    /// user copied elsewhere. Trusted pages (HiWave's chrome) opt in. The
    /// user's own paste is always allowed.
    pub fn set_clipboard_read_allowed(&self, allowed: bool) {
        self.edit.script_read.set(allowed);
    }

    /// The user activated the page (a trusted key or mouse press): for
    /// the next few seconds script may write the clipboard
    /// (`navigator.clipboard.writeText`). [`Self::fire_key_event`] and
    /// [`Self::fire_mouse_event`] call it for the events that activate.
    pub fn notify_user_activation(&self) {
        self.edit.activate();
    }

    /// Whether the page has transient user activation.
    pub fn has_transient_activation(&self) -> bool {
        self.edit.has_activation()
    }

    /// Run edit commands on `states` (the engine's edit states for this
    /// view) instead of the bindings' own map.
    pub fn share_edit_states(&self, states: SharedEditStates) {
        *self.edit.states.borrow_mut() = states;
    }

    /// The user asked for `command` in the control `node` (a shortcut):
    /// `beforeinput` (cancelable) for a command that edits, the edit on
    /// the control's edit state, then `input`. What the command did, or
    /// the default (nothing) when the control is not a text control or a
    /// listener cancelled.
    pub fn user_edit(
        &self,
        node: usize,
        command: &rustkit_dom::forms::EditCommand,
    ) -> Result<rustkit_dom::forms::EditOutcome, BindingError> {
        use rustkit_dom::forms::EditCommand as C;
        let (name, value) = match command {
            C::Copy => ("copy", None),
            C::Cut => ("cut", None),
            C::Paste => ("paste", None),
            C::SelectAll => ("selectAll", None),
            C::Undo => ("undo", None),
            C::Redo => ("redo", None),
            C::InsertText(t) => ("insertText", Some(t.as_str())),
            C::DeleteWordBackward => ("deleteWordBackward", None),
            C::DeleteWordForward => ("deleteWordForward", None),
        };
        let value = value.map_or("null".to_string(), |v| {
            serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
        });
        let script = format!("document.__rkUserEdit({node}, {name:?}, {value})");
        let report = match self.runtime.borrow_mut().evaluate_script(&script)? {
            JsValue::String(json) => json,
            _ => return Ok(Default::default()),
        };
        let r: serde_json::Value = serde_json::from_str(&report).unwrap_or_default();
        let changed = r["changed"].as_bool().unwrap_or(false);
        Ok(rustkit_dom::forms::EditOutcome {
            handled: r["handled"].as_bool().unwrap_or(false),
            changed,
            selection_changed: !changed && r["handled"].as_bool().unwrap_or(false),
            input_type: command.input_type().filter(|_| changed),
            data: r["data"].as_str().map(str::to_string),
        })
    }

    /// The platform's marked text in the text control `node` (CH2.E3,
    /// macOS `NSTextInputClient`): `action` is "mark" (`setMarkedText:`),
    /// "commit" (`insertText:` while text is marked), "unmark"
    /// (`unmarkText`) or "state". Composition events, `beforeinput` and
    /// `input` fire as Chrome's do. The JSON report, or `None` when `node`
    /// is not a text control or, for "commit", nothing is marked.
    pub fn user_compose(
        &self,
        node: usize,
        action: &str,
        text: &str,
    ) -> Result<Option<serde_json::Value>, BindingError> {
        let text = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string());
        let script = format!("document.__rkCompose({node}, {action:?}, {text})");
        Ok(match self.runtime.borrow_mut().evaluate_script(&script)? {
            JsValue::String(json) => serde_json::from_str(&json).ok(),
            _ => None,
        })
    }

    /// How many times script has set a text control's selection
    /// (`select()`, `setSelectionRange`). The engine compares it across a
    /// press's focus change (CH2.E12).
    pub fn script_selections(&self) -> u64 {
        self.edit.script_selections.get()
    }

    /// Tell script what the user typed into a control, so its `value`
    /// reads the edit state's text rather than the default.
    pub fn sync_control_value(&self, node: usize, value: String) {
        self.dom_host.borrow_mut().sync_value(node, value);
    }

    /// Inject global JavaScript objects.
    fn inject_globals(runtime: &mut JsRuntime, app_bridge: bool) -> Result<(), BindingError> {
        // Window object stub. `window` IS the global object, as in every
        // browser: page bundles write `window.x = ...` and read bare `x`
        // (webpack's `self.webpackChunk*`), and that only works if the two
        // are the same object.
        let window_js = r#"
            var window = globalThis;
            Object.assign(window, {
                innerWidth: 800,
                innerHeight: 600,
                outerWidth: 800,
                outerHeight: 600,
                devicePixelRatio: 1,
                location: {
                    href: 'about:blank',
                    protocol: 'about:',
                    host: '',
                    hostname: '',
                    port: '',
                    pathname: 'blank',
                    search: '',
                    hash: '',
                    origin: 'null',
                    reload: function() {},
                    replace: function(url) { this.href = url; },
                    assign: function(url) { this.href = url; }
                },
                navigator: {
                    userAgent: 'RustKit/1.0',
                    language: 'en-US',
                    languages: ['en-US', 'en'],
                    platform: 'Win32',
                    onLine: true
                },
                history: {
                    length: 1,
                    back: function() {},
                    forward: function() {},
                    go: function(delta) {},
                    pushState: function(state, title, url) {},
                    replaceState: function(state, title, url) {}
                },
                localStorage: {
                    _data: {},
                    getItem: function(key) { return this._data[key] || null; },
                    setItem: function(key, value) { this._data[key] = String(value); },
                    removeItem: function(key) { delete this._data[key]; },
                    clear: function() { this._data = {}; },
                    get length() { return Object.keys(this._data).length; },
                    key: function(n) { return Object.keys(this._data)[n] || null; }
                },
                sessionStorage: {
                    _data: {},
                    getItem: function(key) { return this._data[key] || null; },
                    setItem: function(key, value) { this._data[key] = String(value); },
                    removeItem: function(key) { delete this._data[key]; },
                    clear: function() { this._data = {}; },
                    get length() { return Object.keys(this._data).length; },
                    key: function(n) { return Object.keys(this._data)[n] || null; }
                },
                addEventListener: function(type, callback, options) {},
                removeEventListener: function(type, callback, options) {},
                dispatchEvent: function(event) { return true; },
                requestAnimationFrame: function(callback) { return 0; },
                cancelAnimationFrame: function(id) {},
                getComputedStyle: function(element) { return {}; },
                // matchMedia is web_match_media.rs's, over the engine's @media evaluator.
                alert: function(msg) { console.log('[alert]', msg); },
                confirm: function(msg) { console.log('[confirm]', msg); return false; },
                prompt: function(msg, def) { console.log('[prompt]', msg); return def || null; }
            });

            // Alias
            var self = window;
        "#;

        runtime.evaluate_script(window_js)?;

        // The screen, performance and navigator facts, and the window
        // geometry, that pages read without feature-testing (web_platform.js).
        runtime.evaluate_script(include_str!("web_platform.js"))?;

        // Blob, File, FormData, AbortController/AbortSignal, structuredClone (web_blob.js).
        runtime.evaluate_script(include_str!("web_blob.js"))?;

        // The observer interfaces and requestIdleCallback (web_observers.js).
        runtime.evaluate_script(include_str!("web_observers.js"))?;

        // `URL` and `URLSearchParams` (parsing is the `url` crate's).
        web_url::install(runtime)?;

        // crypto.getRandomValues / randomUUID over the OS random source.
        web_crypto::install(runtime)?;

        // btoa/atob, escape/unescape, TextEncoder/TextDecoder (web_encoding.js).
        runtime.evaluate_script(include_str!("web_encoding.js"))?;

        // ReadableStream, WritableStream, TransformStream and strategies (web_streams.js).
        runtime.evaluate_with_host(include_str!("web_streams.js"))?;

        // Intl (en-US only) and the toLocale*String methods over it (web_intl.js).
        runtime.evaluate_script(include_str!("web_intl.js"))?;

        // IPC bridge for communication with Rust: the app's own documents
        // only (CH3.A7). A web page gets none of it, as in Chromium.
        let ipc_js = r#"
            // IPC object for browser-to-Rust communication. The message
            // goes to a queue on the Rust side (`__rk_ipc_post`, installed
            // after the globals); a non-string is sent as its JSON.
            window.ipc = {
                postMessage: function(message) {
                    __rk_ipc_post(typeof message === 'string' ? message : JSON.stringify(message));
                }
            };

            // HiWave Chrome API (for Chrome UI compatibility)
            window.hiwaveChrome = {
                postMessage: function(message) {
                    window.ipc.postMessage(message);
                }
            };
        "#;

        if app_bridge {
            runtime.evaluate_script(ipc_js)?;
        }

        // Document object stub
        let document_js = r#"
            // The read surface (getElementById, querySelector[All],
            // getElementsBy*, documentElement/head/body) is Rust-backed and
            // installed by `dom::install`.
            var document = {
                title: '',
                readyState: 'loading',
                cookie: '',
                domain: '',
                referrer: '',
                URL: 'about:blank',
                
                createElement: function(tagName) {
                    return {
                        tagName: tagName.toUpperCase(),
                        id: '',
                        className: '',
                        textContent: '',
                        innerHTML: '',
                        style: {},
                        attributes: {},
                        children: [],
                        parentNode: null,
                        
                        getAttribute: function(name) {
                            return this.attributes[name] || null;
                        },
                        setAttribute: function(name, value) {
                            this.attributes[name] = value;
                        },
                        removeAttribute: function(name) {
                            delete this.attributes[name];
                        },
                        appendChild: function(child) {
                            this.children.push(child);
                            child.parentNode = this;
                            return child;
                        },
                        removeChild: function(child) {
                            var idx = this.children.indexOf(child);
                            if (idx >= 0) {
                                this.children.splice(idx, 1);
                                child.parentNode = null;
                            }
                            return child;
                        },
                        addEventListener: function(type, callback, options) {},
                        removeEventListener: function(type, callback, options) {}
                    };
                },
                
                createTextNode: function(text) {
                    return { nodeType: 3, textContent: text };
                },
                
                createDocumentFragment: function() {
                    return { children: [], appendChild: function(c) { this.children.push(c); return c; } };
                },
                
                addEventListener: function(type, callback, options) {},
                removeEventListener: function(type, callback, options) {},
                dispatchEvent: function(event) { return true; },
                
                write: function(html) {},
                writeln: function(html) {}
            };
            
            window.document = document;
        "#;

        runtime.evaluate_script(document_js)?;

        // HTMLInputElement prototype
        let input_element_js = r#"
            // Save original createElement for internal use
            var _origCreateElement = document.createElement.bind(document);
            
            // Input element factory (does NOT call createElement)
            document._createInputElement = function(type) {
                var elem = {
                    tagName: 'INPUT',
                    id: '',
                    className: '',
                    textContent: '',
                    innerHTML: '',
                    style: {},
                    attributes: {},
                    children: [],
                    parentNode: null,
                    getAttribute: function(name) { return this.attributes[name] || null; },
                    setAttribute: function(name, value) { this.attributes[name] = value; },
                    removeAttribute: function(name) { delete this.attributes[name]; },
                    appendChild: function(child) { this.children.push(child); child.parentNode = this; return child; },
                    removeChild: function(child) { var idx = this.children.indexOf(child); if (idx >= 0) { this.children.splice(idx, 1); child.parentNode = null; } return child; }
                };
                elem.type = type || 'text';
                elem.value = '';
                elem.defaultValue = '';
                elem.name = '';
                elem.placeholder = '';
                elem.disabled = false;
                elem.readOnly = false;
                elem.required = false;
                elem.checked = false;
                elem.indeterminate = false;
                elem.maxLength = -1;
                elem.minLength = 0;
                elem.selectionStart = 0;
                elem.selectionEnd = 0;
                elem.selectionDirection = 'none';
                elem._form = null;
                
                // Selection methods
                elem.select = function() {
                    this.selectionStart = 0;
                    this.selectionEnd = this.value.length;
                };
                
                elem.setSelectionRange = function(start, end, direction) {
                    this.selectionStart = Math.max(0, Math.min(start, this.value.length));
                    this.selectionEnd = Math.max(this.selectionStart, Math.min(end, this.value.length));
                    this.selectionDirection = direction || 'none';
                };
                
                elem.setRangeText = function(replacement, start, end, selectMode) {
                    start = start !== undefined ? start : this.selectionStart;
                    end = end !== undefined ? end : this.selectionEnd;
                    var before = this.value.substring(0, start);
                    var after = this.value.substring(end);
                    this.value = before + replacement + after;
                    
                    switch(selectMode) {
                        case 'select':
                            this.selectionStart = start;
                            this.selectionEnd = start + replacement.length;
                            break;
                        case 'start':
                            this.selectionStart = this.selectionEnd = start;
                            break;
                        case 'end':
                            this.selectionStart = this.selectionEnd = start + replacement.length;
                            break;
                        default: // 'preserve'
                            break;
                    }
                };
                
                // Validation methods
                elem.checkValidity = function() {
                    if (this.required && this.value === '') return false;
                    if (this.minLength > 0 && this.value.length < this.minLength) return false;
                    if (this.maxLength >= 0 && this.value.length > this.maxLength) return false;
                    if (this.pattern) {
                        var regex = new RegExp('^' + this.pattern + '$');
                        if (!regex.test(this.value)) return false;
                    }
                    return true;
                };
                
                elem.reportValidity = function() {
                    return this.checkValidity();
                };
                
                elem.setCustomValidity = function(msg) {
                    this._customValidityMessage = msg;
                };
                
                // Form getter
                Object.defineProperty(elem, 'form', {
                    get: function() { return this._form; }
                });
                
                // Validity getter
                Object.defineProperty(elem, 'validity', {
                    get: function() {
                        var el = this;
                        return {
                            get valid() { return el.checkValidity(); },
                            get valueMissing() { return el.required && el.value === ''; },
                            get tooShort() { return el.minLength > 0 && el.value.length < el.minLength; },
                            get tooLong() { return el.maxLength >= 0 && el.value.length > el.maxLength; },
                            get patternMismatch() {
                                if (!el.pattern) return false;
                                var regex = new RegExp('^' + el.pattern + '$');
                                return !regex.test(el.value);
                            },
                            get typeMismatch() { return false; },
                            get stepMismatch() { return false; },
                            get rangeUnderflow() { return false; },
                            get rangeOverflow() { return false; },
                            get badInput() { return false; },
                            get customError() { return !!el._customValidityMessage; }
                        };
                    }
                });
                
                // Focus/blur methods
                elem.focus = function() {
                    this.dispatchEvent(new Event('focus', { bubbles: false }));
                };
                
                elem.blur = function() {
                    this.dispatchEvent(new Event('blur', { bubbles: false }));
                };
                
                // Event dispatch
                elem.dispatchEvent = function(event) {
                    // Simplified event dispatch
                    return true;
                };
                
                return elem;
            };
            
            // Textarea element factory
            document._createTextAreaElement = function() {
                var elem = document._createInputElement('text');
                elem.tagName = 'TEXTAREA';
                elem.rows = 2;
                elem.cols = 20;
                elem.wrap = 'soft';
                elem.textLength = 0;
                
                // Override to update textLength
                var origValue = '';
                Object.defineProperty(elem, 'value', {
                    get: function() { return origValue; },
                    set: function(val) {
                        origValue = val;
                        this.textLength = val.length;
                    }
                });
                
                return elem;
            };
            
            // Override createElement for input/textarea
            document.createElement = function(tagName) {
                var tag = tagName.toUpperCase();
                if (tag === 'INPUT') {
                    return document._createInputElement('text');
                } else if (tag === 'TEXTAREA') {
                    return document._createTextAreaElement();
                } else if (tag === 'FORM') {
                    return document._createFormElement();
                }
                // For other elements, create a basic element object
                return {
                    tagName: tag,
                    id: '',
                    className: '',
                    textContent: '',
                    innerHTML: '',
                    style: {},
                    attributes: {},
                    children: [],
                    parentNode: null,
                    getAttribute: function(name) { return this.attributes[name] || null; },
                    setAttribute: function(name, value) { this.attributes[name] = value; },
                    removeAttribute: function(name) { delete this.attributes[name]; },
                    appendChild: function(child) { this.children.push(child); child.parentNode = this; return child; },
                    removeChild: function(child) { var idx = this.children.indexOf(child); if (idx >= 0) { this.children.splice(idx, 1); child.parentNode = null; } return child; },
                    addEventListener: function(type, callback, options) {},
                    removeEventListener: function(type, callback, options) {}
                };
            };
            
            // HTMLFormElement prototype
            document._createFormElement = function() {
                var form = {
                    tagName: 'FORM',
                    id: '',
                    className: '',
                    style: {},
                    attributes: {},
                    children: [],
                    parentNode: null,
                    getAttribute: function(name) { return this.attributes[name] || null; },
                    setAttribute: function(name, value) { this.attributes[name] = value; },
                    removeAttribute: function(name) { delete this.attributes[name]; },
                    appendChild: function(child) { this.children.push(child); child.parentNode = this; return child; },
                    removeChild: function(child) { var idx = this.children.indexOf(child); if (idx >= 0) { this.children.splice(idx, 1); child.parentNode = null; } return child; }
                };
                form.action = '';
                form.method = 'get';
                form.enctype = 'application/x-www-form-urlencoded';
                form.target = '';
                form.noValidate = false;
                form.elements = [];
                
                form.submit = function() {
                    // Native submit - would be handled by engine
                    console.log('[form submit]', this.action, this.method);
                };
                
                form.reset = function() {
                    this.elements.forEach(function(el) {
                        if (el.defaultValue !== undefined) {
                            el.value = el.defaultValue;
                        }
                        if (el.defaultChecked !== undefined) {
                            el.checked = el.defaultChecked;
                        }
                    });
                };
                
                form.checkValidity = function() {
                    return this.elements.every(function(el) {
                        return !el.checkValidity || el.checkValidity();
                    });
                };
                
                form.reportValidity = function() {
                    return this.checkValidity();
                };
                
                return form;
            };
        "#;

        runtime.evaluate_script(input_element_js)?;
        runtime.evaluate_with_host(PAGE_LIFECYCLE_JS)?;

        debug!("Global objects injected");
        Ok(())
    }

    /// Install the selector matcher `querySelector`, `querySelectorAll`,
    /// `matches` and `closest` use. The engine owns the real one (the
    /// cascade's), and this crate can't depend on the engine; without one
    /// they fall back to rustkit-dom's single tag/`#id`/`.class` matcher.
    pub fn set_selector_matcher(&self, matcher: SelectorMatchFn) {
        self.dom_host.borrow_mut().matcher = Some(matcher);
    }

    /// Publish where the layout put each element (by raw NodeId), for
    /// `getBoundingClientRect`, `offsetWidth/Height/Top/Left`, `offsetParent`,
    /// `clientWidth/Height` and `scrollWidth/Height`. The engine calls this
    /// after a layout; script reads answer from the last one published.
    pub fn set_geometry(&self, geometry: std::collections::HashMap<usize, BoxGeometry>) {
        self.dirty.laid_out();
        let mut host = self.dom_host.borrow_mut();
        host.geometry_source = None;
        host.set_hit_tester(None);
        host.geometry = geometry;
    }

    /// The layout hit test `elementFromPoint` uses. `None` falls back to
    /// the published boxes. A point answers null until a layout exists.
    pub fn set_hit_tester(&self, tester: Option<HitTester>) {
        self.dom_host.borrow_mut().set_hit_tester(tester);
    }

    /// Publish a new layout without measuring it: `source` builds the maps
    /// `set_geometry` and `set_computed_styles` take, and runs on the first
    /// script read of geometry or computed style after this call, once. The
    /// maps of the previous layout are dropped here, so no read sees them.
    pub fn set_geometry_source(&self, source: GeometrySource) {
        // A layout is built from the live DOM, so it has every write so far.
        self.dirty.laid_out();
        let mut host = self.dom_host.borrow_mut();
        host.geometry.clear();
        host.computed.borrow_mut().clear();
        host.computed_lines = None;
        host.geometry_source = Some(source);
        host.renew_forced_budget();
    }

    /// Keep element scroll offsets in the view's table.
    /// Share the view's animated-style layer (`AnimatedStyles`): script's
    /// Web Animations write it, the engine's build reads it and publishes
    /// its CSS animations into it.
    pub fn share_animated_styles(&self, animated: SharedAnimatedStyles) {
        self.dom_host.borrow_mut().animated = animated;
    }

    pub fn share_element_scrolls(&self, scrolls: SharedElementScrolls) {
        self.dom_host.borrow_mut().element_scrolls = scrolls;
    }

    /// How a read of geometry after a script write in the same task lays
    /// the page out (forced layout). The engine installs one with each
    /// layout it publishes; a new document drops it. Without one, such a
    /// read answers from the last layout.
    pub fn set_forced_layout(&self, forced: Option<ForcedLayout>) {
        self.dom_host.borrow_mut().forced_layout = forced;
    }

    /// Forced layouts since the last call, which resets the count and the
    /// per-task budget (the engine calls this when a script task settles).
    pub fn take_forced_layouts(&self) -> u64 {
        self.dom_host.borrow_mut().take_forced_layouts()
    }

    /// `take_forced_layouts`, with each forced layout's cause and the time
    /// they took (the engine's per-task log line).
    pub fn take_forced_report(&self) -> ForcedLayoutReport {
        self.dom_host.borrow_mut().take_forced_report()
    }

    /// Whether the page's sheets have a `:has()` rule, asked when a write
    /// lands inside a `display: none` subtree: with one, the write may show
    /// it, and a geometry read after it lays the page out. Without a probe
    /// the answer is yes.
    pub fn set_has_rules_probe(&self, probe: Option<std::rc::Rc<dyn Fn() -> bool>>) {
        self.dom_host.borrow_mut().has_rules = probe;
    }

    /// How many times a source from `set_geometry_source` was built.
    pub fn geometry_builds(&self) -> u64 {
        self.dom_host.borrow().geometry_builds
    }

    /// Publish the view's scroll offset and the furthest it can scroll, for
    /// `window.scrollX/scrollY` and the clamp on `scrollTo`. The engine calls
    /// this after a layout and after the user scrolls.
    pub fn set_scroll_state(&self, offset: (f32, f32), max: (f32, f32)) {
        let mut s = self.scroll.borrow_mut();
        s.x = offset.0;
        s.y = offset.1;
        s.max_x = max.0;
        s.max_y = max.1;
        drop(s);
        self.dom_host.borrow_mut().note_scroll(offset);
    }

    /// Where script last scrolled the window to since the last call, if it
    /// did. The engine applies it to the view when script settles.
    pub fn take_scroll_request(&self) -> Option<(f32, f32)> {
        self.scroll.borrow_mut().request.take()
    }

    /// Compute and deliver the IntersectionObserver and ResizeObserver
    /// records from the geometry as it stands. The engine calls this after a
    /// layout and after a scroll. Returns how many observers have targets.
    pub fn tick_observers(&self) -> usize {
        match self.evaluate("typeof __rkObserversTick === 'function' ? __rkObserversTick() : 0") {
            Ok(JsValue::Number(n)) if n >= 0.0 => n as usize,
            _ => 0,
        }
    }

    /// Tell the page its window was scrolled by the user: fires `scroll`.
    pub fn notify_scrolled(&self) {
        let _ = self.evaluate("typeof __rkUserScrolled === 'function' && __rkUserScrolled()");
    }

    /// Tell the page the user scrolled an element: `scroll` fires at it.
    pub fn notify_element_scrolled(&self, node: usize) {
        let _ = self.evaluate(&format!(
            "typeof __rkUserScrolledElement === 'function' && __rkUserScrolledElement({node})"
        ));
    }

    /// Publish the computed style of each laid-out element (raw NodeId to
    /// `name\tvalue` lines joined by newlines), for `getComputedStyle`.
    pub fn set_computed_styles(&self, styles: std::collections::HashMap<usize, String>) {
        let mut host = self.dom_host.borrow_mut();
        host.geometry_source = None;
        host.computed_lines = None;
        *host.computed.borrow_mut() = styles;
    }

    /// Format each element's computed style of the layout just published
    /// (`set_geometry_source`) on its first read, instead of all of them
    /// with the geometry (LAT8). Call after `set_geometry_source`, which
    /// drops the previous layout's.
    pub fn set_computed_lines(&self, lines: Option<ComputedLines>) {
        self.dom_host.borrow_mut().computed_lines = lines;
    }

    /// Set the document.
    pub fn set_document(&self, document: Rc<Document>) -> Result<(), BindingError> {
        // Update state. Marks against the previous document are moot: the
        // new one gets a full layout of its own.
        self.window.borrow_mut().document = Some(document.clone());
        self.dirty.take();
        self.dirty.laid_out();
        self.mutations.take();
        self.mutation_log.take();

        // Sync to JS
        let title = document.title().unwrap_or_default();
        let mut runtime = self.runtime.borrow_mut();
        runtime.evaluate_script(&format!("document.title = {:?};", title))?;
        runtime.evaluate_script("document.readyState = 'complete';")?;

        // Rebind the DOM wrappers. Wrappers handed out for a previous
        // document stop resolving (their generation no longer matches).
        let generation = self.dom_host.borrow_mut().bind(document);
        self.canvases.borrow_mut().clear();
        runtime.evaluate_script(&format!("__rustkit_dom_reset({generation});"))?;

        debug!("Document bound to JS context");
        Ok(())
    }

    /// Set the current URL.
    pub fn set_location(&self, url: &Url) -> Result<(), BindingError> {
        let location = Location::from_url(url);

        // Update state
        self.window.borrow_mut().location = location.clone();

        // Sync to JS
        let mut runtime = self.runtime.borrow_mut();
        runtime.evaluate_script(&format!(
            r#"
            if (window.__rustkit_location_sync) window.__rustkit_location_sync({:?});
            else window.location.href = {:?};
            window.location.protocol = {:?};
            window.location.host = {:?};
            window.location.hostname = {:?};
            window.location.port = {:?};
            window.location.pathname = {:?};
            window.location.search = {:?};
            if (window.__rustkit_location_hash_sync) window.__rustkit_location_hash_sync({:?});
            window.location.origin = {:?};
            document.URL = {:?};
            if (window.__rustkit_history_reset) window.__rustkit_history_reset();
            "#,
            location.href,
            location.href,
            location.protocol,
            location.host,
            location.hostname,
            location.port,
            location.pathname,
            location.search,
            location.hash,
            location.origin,
            location.href
        ))?;

        Ok(())
    }

    /// Set window dimensions.
    pub fn set_dimensions(&self, width: f64, height: f64) -> Result<(), BindingError> {
        let mut window = self.window.borrow_mut();
        window.inner_width = width;
        window.inner_height = height;
        window.outer_width = width;
        window.outer_height = height;
        drop(window);

        let mut runtime = self.runtime.borrow_mut();
        runtime.evaluate_script(&format!(
            "window.innerWidth = {}; window.innerHeight = {}; \
             window.outerWidth = {}; window.outerHeight = {};",
            width, height, width, height
        ))?;

        Ok(())
    }

    /// Evaluate a script in the bound context.
    pub fn evaluate(&self, script: &str) -> Result<JsValue, BindingError> {
        self.runtime
            .borrow_mut()
            .evaluate_script(script)
            .map_err(Into::into)
    }

    /// Evaluate a page script as `path` (see
    /// [`JsRuntime::evaluate_script_named`]).
    pub fn evaluate_named(&self, script: &str, path: &str) -> Result<JsValue, BindingError> {
        self.runtime
            .borrow_mut()
            .evaluate_script_named(script, path)
            .map_err(Into::into)
    }

    /// Hand the page's console calls to `handler`, with their positions
    /// (see [`JsRuntime::set_console_handler`]).
    pub fn set_console_handler(&self, handler: rustkit_js::ConsoleHandler) {
        self.runtime.borrow_mut().set_console_handler(handler);
    }

    /// Also hand each console call to `handler` as types and lengths only
    /// (see [`JsRuntime::set_console_shape_handler`]).
    pub fn set_console_shape_handler(&self, handler: rustkit_js::ConsoleHandler) {
        self.runtime.borrow_mut().set_console_shape_handler(handler);
    }

    /// Hand console calls over as they are made while `live` says so (see
    /// [`JsRuntime::set_console_live`]).
    pub fn set_console_live(&self, live: Option<rustkit_js::ConsoleLive>) {
        self.runtime.borrow_mut().set_console_live(live);
    }

    /// Keep the number of console calls held for the next flush in
    /// `counter` (see [`JsRuntime::set_console_held_counter`]).
    pub fn set_console_held_counter(&self, counter: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>) {
        self.runtime.borrow_mut().set_console_held_counter(counter);
    }

    /// Console arguments reach the handler as type and length only (see
    /// [`JsRuntime::set_console_redaction`]).
    pub fn set_console_redaction(&self, on: bool) {
        self.runtime.borrow_mut().set_console_redaction(on);
    }

    /// Hand pending console calls to the console handler now.
    pub fn flush_console(&self) {
        self.runtime.borrow_mut().flush_console();
    }

    /// Diagnostics for an embedder that records them: describe each page
    /// error ([`take_error_reports`](Self::take_error_reports)), with its
    /// message text only when `keep_text` (see
    /// [`JsRuntime::set_error_reports`]).
    pub fn set_error_details(&self, on: bool, keep_text: bool) -> Result<(), BindingError> {
        self.runtime.borrow_mut().set_error_reports(on, keep_text);
        Ok(())
    }

    /// Keep each named script's source so an error log can quote it. Off
    /// drops what was kept. See [`JsRuntime::set_capture_script_sources`].
    pub fn capture_script_sources(&self, on: bool) {
        self.runtime.borrow_mut().set_capture_script_sources(on);
    }

    /// Source last evaluated as `path`, when capture is on.
    pub fn script_source(&self, path: &str) -> Option<String> {
        self.runtime.borrow().script_source(path)
    }

    /// Page errors since the last call (with
    /// [`set_error_details`](Self::set_error_details) on): exceptions,
    /// modules that threw after a top-level `await`, unhandled rejections.
    pub fn take_error_reports(&self) -> Vec<rustkit_js::ErrorReport> {
        self.runtime.borrow_mut().take_error_reports()
    }

    /// Unimplemented Web API members page script reached since the last
    /// call: `(interface, member, times, first caller's position)`. Only
    /// [`UNSUPPORTED_MEMBERS`] are ever returned.
    pub fn take_unsupported_apis(&self) -> Vec<(String, String, u64, Option<rustkit_js::ScriptPosition>)> {
        self.runtime
            .borrow_mut()
            .take_unsupported()
            .into_iter()
            .filter(|(i, m, _, _)| UNSUPPORTED_MEMBERS.contains(&(i.as_str(), m.as_str())))
            .collect()
    }

    /// Bound every loop a page script runs (see
    /// [`JsRuntime::set_loop_iteration_limit`]).
    pub fn set_loop_iteration_limit(&self, max_iterations: u64) {
        self.runtime
            .borrow_mut()
            .set_loop_iteration_limit(max_iterations);
    }

    /// Stop script still running at `at` at its next host call (see
    /// [`JsRuntime::set_execution_deadline`]); `None` lifts it.
    pub fn set_execution_deadline(&self, at: Option<std::time::Instant>) {
        self.runtime.borrow_mut().set_execution_deadline(at);
    }

    /// Whether the execution deadline has stopped script since it was
    /// last set or asked about.
    pub fn take_deadline_hit(&self) -> bool {
        self.runtime.borrow_mut().take_deadline_hit()
    }

    /// How long past the deadline the stopped script ran before the host
    /// call that ended it (see [`JsRuntime::take_deadline_overrun`]).
    pub fn take_deadline_overrun(&self) -> Option<std::time::Duration> {
        self.runtime.borrow_mut().take_deadline_overrun()
    }

    /// Let another thread stop this document's script through its view's
    /// interrupt word (see [`JsRuntime::set_view_control`]).
    pub fn set_view_control(&self, control: Option<std::sync::Arc<rustkit_js::ViewControl>>) {
        self.runtime.borrow_mut().set_view_control(control);
    }

    /// Open a turn of script on `stamp`, stopped by the watchdog past
    /// `budget` (see [`JsRuntime::begin_turn`]).
    pub fn begin_turn(&self, stamp: &std::sync::Arc<rustkit_js::ThreadStamp>, budget: std::time::Duration) -> u64 {
        self.runtime.borrow_mut().begin_turn(stamp, budget)
    }

    /// Close the turn [`DomBindings::begin_turn`] opened.
    pub fn end_turn(&self, stamp: &rustkit_js::ThreadStamp) {
        self.runtime.borrow_mut().end_turn(stamp);
    }

    /// Bound the number of microtask job iterations allowed per turn (see
    /// [`JsRuntime::set_max_job_iterations`]).
    pub fn set_max_job_iterations(&self, max_iterations: u64) {
        self.runtime
            .borrow_mut()
            .set_max_job_iterations(max_iterations);
    }

    /// Run any pending jobs (microtasks and async completions).
    pub fn run_jobs(&self) -> Result<(), BindingError> {
        self.runtime
            .borrow_mut()
            .run_jobs()
            .map_err(Into::into)
    }

    /// Name the `<script>` element being run, as `document.currentScript`
    /// sees it; `None` between scripts. `node` is the element's raw NodeId.
    pub fn set_current_script(&self, node: Option<usize>) -> Result<(), BindingError> {
        let script = match node {
            Some(id) => format!("document.__rkSetCurrentScript({id});"),
            None => "document.__rkSetCurrentScript(null);".to_string(),
        };
        self.runtime.borrow_mut().evaluate_script(&script)?;
        Ok(())
    }

    /// Fire `load` or `error` at a `<script>` element (by node id). Listener
    /// exceptions are queued, see [`Self::take_reported_errors`].
    pub fn fire_script_event(&self, node: usize, event_type: &str) -> Result<(), BindingError> {
        self.runtime.borrow_mut().evaluate_script(&format!(
            "document.__rkFireOn({node}, {event_type:?});"
        ))?;
        Ok(())
    }

    /// Fire the user's mouse input at an element (by node id) as a trusted,
    /// bubbling, cancelable `MouseEvent` (a `PointerEvent` for `pointer*`
    /// and `click`; the enter and leave events neither bubble nor can be
    /// cancelled). Returns false when a listener
    /// called `preventDefault()`. Listener exceptions are queued, see
    /// [`Self::take_reported_errors`].
    pub fn fire_mouse_event(
        &self,
        node: usize,
        event_type: &str,
        data: &MouseEventBindingData,
    ) -> Result<bool, BindingError> {
        // Activation-triggering input events (HTML §6.4.3).
        if matches!(event_type, "mousedown" | "pointerdown" | "mouseup" | "pointerup") {
            self.notify_user_activation();
        }
        let result = self.runtime.borrow_mut().evaluate_script(&format!(
            "document.__rkFireMouse({node}, {event_type:?}, {{ clientX: {}, clientY: {}, \
             screenX: {}, screenY: {}, offsetX: {}, offsetY: {}, button: {}, buttons: {}, ctrlKey: {}, \
             altKey: {}, shiftKey: {}, metaKey: {}, movementX: {}, movementY: {}, related: {}, detail: {} }})",
            data.client_x,
            data.client_y,
            data.screen_x,
            data.screen_y,
            data.offset_x,
            data.offset_y,
            data.button,
            data.buttons,
            data.ctrl_key,
            data.alt_key,
            data.shift_key,
            data.meta_key,
            data.movement_x,
            data.movement_y,
            data.related_target.map_or("null".to_string(), |n| n.to_string()),
            data.detail.map_or("null".to_string(), |n| n.to_string()),
        ))?;
        Ok(!matches!(result, JsValue::Boolean(false)))
    }

    /// Fire one event of the engine's drag-and-drop processing model at an
    /// element (by node id) as a trusted, bubbling `DragEvent` whose
    /// `dataTransfer` is the drag's (web_dnd.js). `dragstart` begins a new
    /// drag data store, `dragend` (or a cancelled `dragstart`) retires it.
    /// `operation` is the current drag operation (`none`, `copy`, `link`,
    /// `move`): the `dropEffect` of `dragleave`, `drop` and `dragend`.
    /// Returns whether no listener called `preventDefault()`, and the
    /// operation the `dataTransfer`'s `effectAllowed` and `dropEffect` give
    /// once the listeners ran.
    pub fn fire_drag_event(
        &self,
        node: usize,
        event_type: &str,
        data: &MouseEventBindingData,
        operation: &str,
    ) -> Result<(bool, String), BindingError> {
        let result = self.runtime.borrow_mut().evaluate_script(&format!(
            "document.__rkFireDrag({node}, {event_type:?}, {{ clientX: {}, clientY: {}, \
             screenX: {}, screenY: {}, offsetX: {}, offsetY: {}, button: {}, buttons: {}, ctrlKey: {}, \
             altKey: {}, shiftKey: {}, metaKey: {}, related: {} }}, {operation:?})",
            data.client_x,
            data.client_y,
            data.screen_x,
            data.screen_y,
            data.offset_x,
            data.offset_y,
            data.button,
            data.buttons,
            data.ctrl_key,
            data.alt_key,
            data.shift_key,
            data.meta_key,
            data.related_target.map_or("null".to_string(), |n| n.to_string()),
        ))?;
        Ok(match result {
            JsValue::String(s) => {
                let (flag, op) = s.split_once(':').unwrap_or(("1", "none"));
                (flag != "0", op.to_string())
            }
            _ => (true, "none".to_string()),
        })
    }

    /// Fire the user's key press or release at an element (by node id) as
    /// a trusted, bubbling, cancelable `KeyboardEvent`. `None` is the
    /// page's active element (the body unless script focused something).
    /// Returns false when a listener called `preventDefault()`. Listener
    /// exceptions are queued, see [`Self::take_reported_errors`].
    pub fn fire_key_event(
        &self,
        node: Option<usize>,
        event_type: &str,
        data: &KeyboardEventBindingData,
    ) -> Result<bool, BindingError> {
        // A keydown activates the page, except Escape (HTML §6.4.3).
        if event_type == "keydown" && data.key != "Escape" {
            self.notify_user_activation();
        }
        let node = node.map_or("null".to_string(), |n| n.to_string());
        let result = self.runtime.borrow_mut().evaluate_script(&format!(
            "document.__rkFireKey({node}, {event_type:?}, {{ key: {:?}, code: {:?}, repeat: {}, \
             ctrlKey: {}, altKey: {}, shiftKey: {}, metaKey: {} }})",
            data.key, data.code, data.repeat, data.ctrl_key, data.alt_key, data.shift_key, data.meta_key,
        ))?;
        Ok(!matches!(result, JsValue::Boolean(false)))
    }

    /// The user's click focused an element (by node id), or landed on
    /// nothing focusable (`None`): the page's focus follows, with `change`,
    /// `blur`/`focusout` and `focus`/`focusin`. The page may refuse (a
    /// disabled control); [`Self::take_focus_move`] has where it ended up.
    pub fn set_focus(&self, node: Option<usize>) -> Result<(), BindingError> {
        let node = node.map_or("null".to_string(), |n| n.to_string());
        self.runtime
            .borrow_mut()
            .evaluate_script(&format!("document.__rkSetFocus({node});"))?;
        Ok(())
    }

    /// Where the page's focus is, when it moved since the last call (the
    /// user's click, or script's `focus()`/`blur()`): `Some(Some(node))`,
    /// `Some(None)` for nothing focused, `None` when it has not moved.
    pub fn take_focus_move(&self) -> Option<Option<usize>> {
        match self.evaluate("document.__rkTakeFocus()") {
            Ok(JsValue::Number(n)) if n >= 0.0 => Some(Some(n as usize)),
            Ok(JsValue::Number(_)) => Some(None),
            _ => None,
        }
    }

    /// The user moved the document selection (a drag, a double-click):
    /// `Some((anchor, anchor_offset, focus, focus_offset))` by node id and
    /// UTF-16 offset, or `None` to clear it. Script sees it through
    /// `getSelection()` and hears `selectionchange`; the move is not echoed
    /// back by [`Self::take_selection_change`].
    pub fn set_selection(&self, ends: Option<(usize, usize, usize, usize)>) -> Result<(), BindingError> {
        let call = match ends {
            Some((a, ao, f, fo)) => format!("document.__rkSetSelection({a}, {ao}, {f}, {fo});"),
            None => "document.__rkSetSelection(null, 0, null, 0);".to_string(),
        };
        self.runtime.borrow_mut().evaluate_script(&call)?;
        Ok(())
    }

    /// An editing key in the contenteditable host `host` (by node id):
    /// `op` is `insert` (with `text`), `enter`, `back`, `del`, `left`,
    /// `right` or `all`. The page's DOM and selection change as a plain-text
    /// editor would change them, and the host hears `input`. Returns whether
    /// anything changed.
    pub fn edit_key(&self, host: usize, op: &str, text: &str) -> Result<bool, BindingError> {
        let result = self
            .runtime
            .borrow_mut()
            .evaluate_script(&format!("document.__rkEditKey({host}, {op:?}, {text:?})"))?;
        Ok(matches!(result, JsValue::Boolean(true)))
    }

    /// The document selection, when script changed it since the last call
    /// (or since [`Self::set_selection`]): `Some(Some(ends))` as in
    /// `set_selection`, `Some(None)` for no selection, `None` when it has
    /// not changed.
    #[allow(clippy::type_complexity)]
    pub fn take_selection_change(&self) -> Option<Option<(usize, usize, usize, usize)>> {
        match self.evaluate("document.__rkTakeSelection ? document.__rkTakeSelection() : null") {
            Ok(JsValue::String(s)) if s.is_empty() => Some(None),
            Ok(JsValue::String(s)) => {
                let n: Vec<usize> = s.split(' ').filter_map(|p| p.parse().ok()).collect();
                (n.len() == 4).then(|| Some((n[0], n[1], n[2], n[3])))
            }
            _ => None,
        }
    }

    /// Fire `input` at a control (by node id) whose value the user's typing
    /// changed. `data` is the inserted text, `None` for a deletion.
    pub fn fire_input_event(&self, node: usize, data: Option<&str>) -> Result<(), BindingError> {
        let data = data.map_or("null".to_string(), |d| format!("{d:?}"));
        self.runtime
            .borrow_mut()
            .evaluate_script(&format!("document.__rkFireInput({node}, {data});"))?;
        Ok(())
    }

    /// The user picked option `index` of the select `node` (by node id) in
    /// its drop-down or with a key: its selectedness moves there and the
    /// select hears `input` and `change`. Returns whether the selection
    /// changed (a disabled option, or the one already selected, does not
    /// change it and fires nothing).
    pub fn pick_option(&self, node: usize, index: usize) -> Result<bool, BindingError> {
        let picked = self
            .runtime
            .borrow_mut()
            .evaluate_script(&format!("document.__rkPickOption({node}, {index})"))?;
        Ok(matches!(picked, JsValue::Boolean(true)))
    }

    /// Enter in a field (by node id): implicit submission of its form. An
    /// uncancelled `submit` is then in [`Self::take_submit_requests`].
    pub fn implicit_submit(&self, node: usize) -> Result<(), BindingError> {
        self.runtime
            .borrow_mut()
            .evaluate_script(&format!("document.__rkImplicitSubmit({node});"))?;
        Ok(())
    }

    /// The document's URL: the base for a root module's imports.
    pub fn set_module_base(&self, url: &str) {
        self.runtime.borrow_mut().set_module_base(url);
    }

    /// Register a `<script type=importmap>` document (see
    /// `JsRuntime::add_import_map`): per-entry warnings, or an error when the
    /// document is unusable.
    pub fn add_import_map(&self, text: &str) -> Result<Vec<String>, String> {
        self.runtime.borrow_mut().add_import_map(text)
    }

    /// Parse and start a module (see `JsRuntime::begin_module`).
    pub fn begin_module(&self, url: &str, source: &str) -> Result<rustkit_js::ModuleHandle, String> {
        self.runtime.borrow_mut().begin_module(url, source)
    }

    /// The URLs a module graph has asked for since the last call.
    pub fn take_module_requests(&self) -> Vec<String> {
        self.runtime.borrow_mut().take_module_requests()
    }

    /// Dynamic `import()` calls that rejected since the last call.
    pub fn take_dynamic_import_failures(&self) -> Vec<rustkit_js::DynamicImportFailure> {
        self.runtime.borrow_mut().take_dynamic_import_failures()
    }

    /// Supply one requested module's source, or why there is none.
    pub fn supply_module(&self, requested: &str, outcome: Result<rustkit_js::FetchedModule, String>) {
        self.runtime.borrow_mut().supply_module(requested, outcome);
    }

    /// Advance a started module and say where it is.
    pub fn poll_module(&self, handle: &rustkit_js::ModuleHandle) -> rustkit_js::ModuleState {
        self.runtime.borrow_mut().poll_module(handle)
    }

    /// Set `document.readyState` (`loading` / `interactive` / `complete`)
    /// and fire `readystatechange` on the document. That is the spec's
    /// readiness update: the event comes before `DOMContentLoaded` and
    /// before `window` `load`.
    pub fn set_ready_state(&self, state: &str) -> Result<(), BindingError> {
        self.runtime.borrow_mut().evaluate_script(&format!(
            "document.readyState = {:?}; window.__rustkit_fire('document', 'readystatechange');",
            state
        ))?;
        Ok(())
    }

    /// Fire a lifecycle event (`DOMContentLoaded`, `load`) at `window` or
    /// `document`. Listener exceptions are queued, not returned; see
    /// [`Self::take_reported_errors`].
    pub fn fire_lifecycle_event(
        &self,
        target: LifecycleTarget,
        event_type: &str,
    ) -> Result<(), BindingError> {
        let target = match target {
            LifecycleTarget::Window => "window",
            LifecycleTarget::Document => "document",
        };
        self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_fire({:?}, {:?});",
            target, event_type
        ))?;
        Ok(())
    }

    /// Report media query changes: every `MediaQueryList` with a `change`
    /// listener whose answer flipped since it was last reported hears
    /// `change` (the "evaluate media queries and report changes" step of
    /// update the rendering, HTML §8.1.7.3). Run after
    /// [`Self::set_dimensions`] and the `resize` event, which is the order
    /// a browser keeps. Listener exceptions are queued, not returned; see
    /// [`Self::take_reported_errors`]. Returns how many lists in the
    /// document's list of media query lists flipped (a list stays there
    /// once it has listened, as in the spec, listeners or not).
    pub fn fire_media_query_changes(&self) -> Result<u32, BindingError> {
        let fired = self.runtime.borrow_mut().evaluate_script(
            "typeof window.__rustkit_media_changes === 'function' ? window.__rustkit_media_changes() : 0",
        )?;
        Ok(match fired {
            JsValue::Number(n) => n as u32,
            _ => 0,
        })
    }

    /// Run the page's timers on the virtual clock up to `horizon_ms`, at
    /// most `max_callbacks` of them. Returns how many ran.
    pub fn run_timers(&self, horizon_ms: u64, max_callbacks: u32) -> Result<u32, BindingError> {
        let ran = self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_run_timers({}, {})",
            horizon_ms, max_callbacks
        ))?;
        Ok(match ran {
            JsValue::Number(n) => n as u32,
            _ => 0,
        })
    }

    /// [`Self::run_timers`], stopping between callbacks once `frame_ms` of
    /// wall time has passed. `true` means the horizon still has timers due.
    pub fn run_timers_for_frame(
        &self,
        horizon_ms: u64,
        max_callbacks: u32,
        frame_ms: u64,
    ) -> Result<(u32, bool), BindingError> {
        let ran = self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_run_timers_framed({}, {}, {})",
            horizon_ms, max_callbacks, frame_ms
        ))?;
        Ok(match ran {
            JsValue::Number(n) if n < 0.0 => ((-n) as u32, true),
            JsValue::Number(n) => (n as u32, false),
            _ => (0, false),
        })
    }

    /// Move the virtual clock on by `delta_ms` (the live loop's real elapsed
    /// time) and run the timers that came due, at most `max_callbacks` of
    /// them. Returns how many ran.
    pub fn advance_timers(&self, delta_ms: u64, max_callbacks: u32) -> Result<u32, BindingError> {
        self.advance_timers_for(delta_ms, max_callbacks, None)
    }

    /// [`Self::advance_timers`], and once a callback ends `slice` after the
    /// call began, no more run: the rest wait for the next call, and
    /// [`Self::next_timer_delay`] says they are due.
    pub fn advance_timers_for(
        &self,
        delta_ms: u64,
        max_callbacks: u32,
        slice: Option<std::time::Duration>,
    ) -> Result<u32, BindingError> {
        let slice_ms = slice.map_or(0, |slice| slice.as_millis().max(1) as u64);
        let ran = self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_advance_timers({}, {}, {})",
            delta_ms, max_callbacks, slice_ms
        ))?;
        Ok(match ran {
            JsValue::Number(n) => n as u32,
            _ => 0,
        })
    }

    /// [`Self::advance_timers`], stopping between callbacks once `frame_ms`
    /// of wall time has passed. `true` means timers were still due.
    pub fn advance_timers_for_frame(
        &self,
        delta_ms: u64,
        max_callbacks: u32,
        frame_ms: u64,
    ) -> Result<(u32, bool), BindingError> {
        let ran = self.runtime.borrow_mut().evaluate_script(&format!(
            "window.__rustkit_advance_timers_framed({}, {}, {})",
            delta_ms, max_callbacks, frame_ms
        ))?;
        Ok(match ran {
            JsValue::Number(n) if n < 0.0 => ((-n) as u32, true),
            JsValue::Number(n) => (n as u32, false),
            _ => (0, false),
        })
    }

    /// The page's virtual clock, in ms: what its timers and animation
    /// frames have run up to. The engine's CSS transitions run on it.
    pub fn clock_now_ms(&self) -> f64 {
        match self
            .runtime
            .borrow_mut()
            .evaluate_bookkeeping("window.__rustkit_now()")
        {
            Ok(JsValue::Number(n)) if n.is_finite() => n,
            _ => 0.0,
        }
    }

    /// Fire a `TransitionEvent` (`transitionrun`, `transitionstart`,
    /// `transitionend`, `transitioncancel`) at an element (by node id), as
    /// the engine's transitions change phase. Bubbles; `transitionend` can
    /// be cancelled. Listener exceptions are queued, see
    /// [`Self::take_reported_errors`].
    pub fn fire_transition_event(
        &self,
        node: usize,
        pseudo: Option<&str>,
        event_type: &str,
        property: &str,
        elapsed_s: f32,
    ) -> Result<(), BindingError> {
        self.runtime.borrow_mut().evaluate_script(&format!(
            "document.__rkFireTransition({node}, {event_type:?}, {{ propertyName: {property:?}, \
             elapsedTime: {elapsed_s}, pseudoElement: {pseudo:?} }});",
            pseudo = pseudo.unwrap_or("")
        ))?;
        Ok(())
    }

    /// Fire an `AnimationEvent` (`animationstart`, `animationiteration`,
    /// `animationend`, `animationcancel`) at an element (by node id), as
    /// the engine's `@keyframes` animations change phase. Bubbles, not
    /// cancelable. Listener exceptions are queued, see
    /// [`Self::take_reported_errors`].
    pub fn fire_animation_event(
        &self,
        node: usize,
        pseudo: Option<&str>,
        event_type: &str,
        name: &str,
        elapsed_s: f32,
    ) -> Result<(), BindingError> {
        self.runtime.borrow_mut().evaluate_script(&format!(
            "document.__rkFireAnimation({node}, {event_type:?}, {{ animationName: {name:?}, \
             elapsedTime: {elapsed_s}, pseudoElement: {pseudo:?} }});",
            pseudo = pseudo.unwrap_or("")
        ))?;
        Ok(())
    }

    /// Milliseconds until the page's next timer is due on the virtual clock
    /// (0 when one is overdue). `None` when no timer is set.
    pub fn next_timer_delay(&self) -> Option<u64> {
        match self
            .runtime
            .borrow_mut()
            .evaluate_bookkeeping("window.__rustkit_next_timer()")
        {
            Ok(JsValue::Number(n)) if n >= 0.0 => Some(n as u64),
            _ => None,
        }
    }

    /// Exceptions thrown in listeners and timer callbacks since the last
    /// call, as text (`TypeError: x is not a function`, `[object Object]`).
    pub fn take_reported_errors(&self) -> Vec<String> {
        self.runtime.borrow_mut().take_reported_errors()
    }

    /// Drain the IPC message queue.
    ///
    /// This method collects all IPC messages that were queued via
    /// `window.ipc.postMessage()` since the last drain call.
    ///
    /// Returns a Vec of IpcMessage structs.
    pub fn drain_ipc_queue(&self) -> Vec<IpcMessage> {
        std::mem::take(&mut *self.ipc_queue.borrow_mut())
    }

    /// Check if there are pending IPC messages.
    pub fn has_pending_ipc(&self) -> bool {
        !self.ipc_queue.borrow().is_empty()
    }

    /// Add an event listener.
    pub fn add_event_listener(
        &self,
        node_id: NodeId,
        event_type: &str,
        callback: &str,
        capture: bool,
    ) -> ListenerId {
        let id = ListenerId::new();
        let listener = EventListener {
            id,
            node_id,
            event_type: event_type.to_string(),
            callback: callback.to_string(),
            capture,
        };

        self.event_listeners.borrow_mut().push(listener);
        trace!(?id, event_type, "Event listener added");
        id
    }

    /// Remove an event listener.
    pub fn remove_event_listener(&self, id: ListenerId) {
        self.event_listeners.borrow_mut().retain(|l| l.id != id);
        trace!(?id, "Event listener removed");
    }

    /// Dispatch an event.
    pub fn dispatch_event(&self, node_id: NodeId, event_type: &str) -> Result<bool, BindingError> {
        self.dispatch_event_with_data(node_id, event_type, None)
    }

    /// Dispatch an event with additional data.
    pub fn dispatch_event_with_data(
        &self,
        node_id: NodeId,
        event_type: &str,
        event_data: Option<&EventData>,
    ) -> Result<bool, BindingError> {
        let listeners: Vec<_> = self
            .event_listeners
            .borrow()
            .iter()
            .filter(|l| l.node_id == node_id && l.event_type == event_type)
            .map(|l| l.callback.clone())
            .collect();

        if listeners.is_empty() {
            return Ok(true);
        }

        // Create the Event object in JS
        let event_js = Self::create_event_object(event_type, event_data);

        let mut runtime = self.runtime.borrow_mut();
        runtime.evaluate_script(&event_js)?;

        // Execute each listener
        for callback in listeners {
            runtime.evaluate_script(&format!(
                "(function(e) {{ {} }})(__rustkit_event)",
                callback
            ))?;
        }

        // Check if default was prevented
        let prevented = runtime.evaluate_script("__rustkit_event.defaultPrevented")?;
        let was_prevented = matches!(prevented, JsValue::Boolean(true));

        // Clean up
        runtime.evaluate_script("delete __rustkit_event;")?;

        Ok(!was_prevented)
    }

    /// Create a JavaScript Event object.
    fn create_event_object(event_type: &str, data: Option<&EventData>) -> String {
        let mut props = vec![
            format!("type: {:?}", event_type),
            "bubbles: true".to_string(),
            "cancelable: true".to_string(),
            "defaultPrevented: false".to_string(),
            "target: null".to_string(),
            "currentTarget: null".to_string(),
            "eventPhase: 0".to_string(),
            "timeStamp: Date.now()".to_string(),
            "isTrusted: true".to_string(),
            "preventDefault: function() { this.defaultPrevented = true; }".to_string(),
            "stopPropagation: function() { this._stopped = true; }".to_string(),
            "stopImmediatePropagation: function() { this._stoppedImmediate = true; }".to_string(),
        ];

        // Add type-specific properties
        if let Some(event_data) = data {
            match event_data {
                EventData::Mouse(mouse) => {
                    props.push(format!("clientX: {}", mouse.client_x));
                    props.push(format!("clientY: {}", mouse.client_y));
                    props.push(format!("screenX: {}", mouse.screen_x));
                    props.push(format!("screenY: {}", mouse.screen_y));
                    props.push(format!("offsetX: {}", mouse.offset_x));
                    props.push(format!("offsetY: {}", mouse.offset_y));
                    props.push(format!("button: {}", mouse.button));
                    props.push(format!("buttons: {}", mouse.buttons));
                    props.push(format!("ctrlKey: {}", mouse.ctrl_key));
                    props.push(format!("altKey: {}", mouse.alt_key));
                    props.push(format!("shiftKey: {}", mouse.shift_key));
                    props.push(format!("metaKey: {}", mouse.meta_key));
                }
                EventData::Keyboard(keyboard) => {
                    props.push(format!("key: {:?}", keyboard.key));
                    props.push(format!("code: {:?}", keyboard.code));
                    props.push(format!("repeat: {}", keyboard.repeat));
                    props.push(format!("ctrlKey: {}", keyboard.ctrl_key));
                    props.push(format!("altKey: {}", keyboard.alt_key));
                    props.push(format!("shiftKey: {}", keyboard.shift_key));
                    props.push(format!("metaKey: {}", keyboard.meta_key));
                    props.push(format!("location: {}", keyboard.location));
                }
                EventData::Focus(focus) => {
                    if let Some(related) = focus.related_target {
                        props.push(format!("relatedTarget: {{ nodeId: {} }}", related));
                    } else {
                        props.push("relatedTarget: null".to_string());
                    }
                }
                EventData::Input(input) => {
                    if let Some(ref data) = input.data {
                        props.push(format!("data: {:?}", data));
                    } else {
                        props.push("data: null".to_string());
                    }
                    props.push(format!("inputType: {:?}", input.input_type));
                    props.push(format!("isComposing: {}", input.is_composing));
                }
            }
        }

        format!("var __rustkit_event = {{ {} }};", props.join(", "))
    }

    /// Dispatch a DOM event through the DOM event system.
    pub fn dispatch_dom_event(
        &self,
        dom_event: &mut rustkit_dom::DomEvent,
        target: &std::rc::Rc<rustkit_dom::Node>,
        ancestors: &[std::rc::Rc<rustkit_dom::Node>],
    ) -> bool {
        rustkit_dom::EventDispatcher::dispatch(dom_event, target, ancestors)
    }

    /// Get the current location.
    pub fn location(&self) -> Location {
        self.window.borrow().location.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_location_from_url() {
        let url = Url::parse("https://example.com:8080/path?query=1#hash").unwrap();
        let loc = Location::from_url(&url);

        assert_eq!(loc.href, "https://example.com:8080/path?query=1#hash");
        assert_eq!(loc.protocol, "https:");
        assert_eq!(loc.host, "example.com:8080");
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.port, "8080");
        assert_eq!(loc.pathname, "/path");
        assert_eq!(loc.search, "?query=1");
        assert_eq!(loc.hash, "#hash");
    }

    #[test]
    fn test_bindings_creation() {
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

        // Window should exist
        let result = bindings.evaluate("typeof window").unwrap();
        assert!(matches!(result, JsValue::String(s) if s == "object"));
    }

    #[test]
    fn test_document_exists() {
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

        let result = bindings.evaluate("typeof document").unwrap();
        assert!(matches!(result, JsValue::String(s) if s == "object"));
    }

    #[test]
    fn test_navigator() {
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

        let result = bindings.evaluate("window.navigator.userAgent").unwrap();
        assert!(matches!(result, JsValue::String(s) if s.contains("RustKit")));
    }

    #[test]
    fn test_local_storage() {
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

        bindings
            .evaluate("window.localStorage.setItem('key', 'value')")
            .unwrap();
        let result = bindings
            .evaluate("window.localStorage.getItem('key')")
            .unwrap();
        assert!(matches!(result, JsValue::String(s) if s == "value"));
    }

    #[test]
    fn test_set_dimensions() {
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

        bindings.set_dimensions(1024.0, 768.0).unwrap();

        let width = bindings.evaluate("window.innerWidth").unwrap();
        assert!(matches!(width, JsValue::Number(n) if (n - 1024.0).abs() < f64::EPSILON));
    }

    #[test]
    fn test_input_element_creation() {
        // Form controls are Rust-backed, so they need a document.
        let bindings = bound("<html><body></body></html>");

        bindings
            .evaluate("var input = document.createElement('input')")
            .unwrap();

        let tag = bindings.evaluate("input.tagName").unwrap();
        assert!(matches!(tag, JsValue::String(s) if s == "INPUT"));

        let input_type = bindings.evaluate("input.type").unwrap();
        assert!(matches!(input_type, JsValue::String(s) if s == "text"));
    }

    #[test]
    fn test_input_element_value() {
        // Form controls are Rust-backed, so they need a document.
        let bindings = bound("<html><body></body></html>");

        bindings
            .evaluate(
                r#"
            var input = document.createElement('input');
            input.value = 'Hello World';
        "#,
            )
            .unwrap();

        let value = bindings.evaluate("input.value").unwrap();
        assert!(matches!(value, JsValue::String(s) if s == "Hello World"));
    }

    #[test]
    fn test_input_element_selection() {
        // Form controls are Rust-backed, so they need a document.
        let bindings = bound("<html><body></body></html>");

        bindings
            .evaluate(
                r#"
            var input = document.createElement('input');
            input.value = 'Hello World';
            input.setSelectionRange(0, 5);
        "#,
            )
            .unwrap();

        let start = bindings.evaluate("input.selectionStart").unwrap();
        let end = bindings.evaluate("input.selectionEnd").unwrap();

        assert!(matches!(start, JsValue::Number(n) if n == 0.0));
        assert!(matches!(end, JsValue::Number(n) if n == 5.0));
    }

    #[test]
    fn test_input_element_select_all() {
        // Form controls are Rust-backed, so they need a document.
        let bindings = bound("<html><body></body></html>");

        bindings
            .evaluate(
                r#"
            var input = document.createElement('input');
            input.value = 'Hello World';
            input.select();
        "#,
            )
            .unwrap();

        let start = bindings.evaluate("input.selectionStart").unwrap();
        let end = bindings.evaluate("input.selectionEnd").unwrap();

        assert!(matches!(start, JsValue::Number(n) if n == 0.0));
        assert!(matches!(end, JsValue::Number(n) if n == 11.0)); // "Hello World" = 11 chars
    }

    #[test]
    fn test_input_element_validation() {
        // Form controls are Rust-backed, so they need a document.
        let bindings = bound("<html><body></body></html>");

        // Empty required field should be invalid
        bindings
            .evaluate(
                r#"
            var input = document.createElement('input');
            input.required = true;
        "#,
            )
            .unwrap();

        let valid = bindings.evaluate("input.checkValidity()").unwrap();
        assert!(matches!(valid, JsValue::Boolean(false)));

        // Non-empty required field should be valid
        bindings.evaluate("input.value = 'test'").unwrap();
        let valid = bindings.evaluate("input.checkValidity()").unwrap();
        assert!(matches!(valid, JsValue::Boolean(true)));
    }

    #[test]
    fn test_textarea_element() {
        // Form controls are Rust-backed, so they need a document.
        let bindings = bound("<html><body></body></html>");

        bindings
            .evaluate(
                r#"
            var textarea = document.createElement('textarea');
            textarea.value = 'Line1\nLine2';
        "#,
            )
            .unwrap();

        let tag = bindings.evaluate("textarea.tagName").unwrap();
        assert!(matches!(tag, JsValue::String(s) if s == "TEXTAREA"));

        let rows = bindings.evaluate("textarea.rows").unwrap();
        assert!(matches!(rows, JsValue::Number(n) if n == 2.0));

        let length = bindings.evaluate("textarea.textLength").unwrap();
        assert!(matches!(length, JsValue::Number(n) if n == 11.0)); // "Line1\nLine2" = 11 chars
    }

    #[test]
    fn test_form_element() {
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

        bindings
            .evaluate(
                r#"
            var form = document._createFormElement();
            form.action = '/submit';
            form.method = 'post';
        "#,
            )
            .unwrap();

        let action = bindings.evaluate("form.action").unwrap();
        assert!(matches!(action, JsValue::String(s) if s == "/submit"));

        let method = bindings.evaluate("form.method").unwrap();
        assert!(matches!(method, JsValue::String(s) if s == "post"));
    }

    /// Like `eval_string`, but renders a boolean or number result too, so
    /// a test can compare `a === b` directly.
    fn eval_any(bindings: &DomBindings, script: &str) -> String {
        match bindings.evaluate(script).unwrap() {
            JsValue::String(s) => s,
            JsValue::Boolean(b) => b.to_string(),
            JsValue::Number(n) => if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() },
            other => panic!("{script} evaluated to {other:?}"),
        }
    }

    fn eval_string(bindings: &DomBindings, script: &str) -> String {
        match bindings.evaluate(script).unwrap() {
            JsValue::String(s) => s,
            other => panic!("{script} evaluated to {other:?}"),
        }
    }

    /// Pages read these without feature-testing; each used to throw a
    /// ReferenceError or TypeError and end the rest of the script.
    #[test]
    fn the_screen_performance_window_and_navigator_baseline_exists() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let probe = |script: &str| eval_string(&bindings, script);
        // screen is the viewport.
        assert_eq!(probe("String(screen.width + 'x' + screen.height)"), "800x600");
        assert_eq!(probe("String(screen.colorDepth)"), "24");
        // performance: a monotonic clock and the Performance Timeline.
        assert_eq!(
            probe("var a = performance.now(); var b = performance.now(); String(typeof a + (b >= a))"),
            "numbertrue"
        );
        assert_eq!(
            probe("performance.mark('s'); performance.mark('e'); var m = performance.measure('m', 's', 'e');                    String(m.entryType + performance.getEntriesByType('mark').length + performance.getEntriesByName('m').length)"),
            "measure21"
        );
        assert_eq!(probe("String(typeof performance.timing.navigationStart)"), "number");
        // window geometry and frame tree.
        assert_eq!(probe("String([scrollX, scrollY, pageXOffset, pageYOffset, screenX].join())"), "0,0,0,0,0");
        assert_eq!(probe("String(window.top === window && window.parent === window && window.opener === null)"), "true");
        assert_eq!(probe("String(typeof scrollTo + typeof focus + typeof getSelection().toString())"), "functionfunctionstring");
        // navigator facts.
        assert_eq!(probe("String(navigator.hardwareConcurrency > 0)"), "true");
        assert_eq!(probe("String([navigator.maxTouchPoints, navigator.cookieEnabled, navigator.webdriver].join())"), "0,true,false");
        // Something defined first wins: the shim never overwrites.
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        assert_eq!(eval_string(&bindings, "String(navigator.userAgent)"), "RustKit/1.0");
    }

    /// `URL` / `URLSearchParams` (lyft, weather and others died on
    /// `ReferenceError: URL is not defined`). Parsing and setters are the
    /// `url` crate's; these pin the object layer and the cases pages hit.
    #[test]
    fn url_parses_resolves_and_exposes_its_components() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_string(&bindings, s);
        assert_eq!(
            ev("var u = new URL('https://user:pw@Example.COM:8080/a/b/../c?x=1&y=2#frag'); \
                [u.href, u.origin, u.protocol, u.username, u.password, u.host, u.hostname, u.port, u.pathname, u.search, u.hash].join('|')"),
            "https://user:pw@example.com:8080/a/c?x=1&y=2#frag|https://example.com:8080|https:|user|pw|example.com:8080|example.com|8080|/a/c|?x=1&y=2|#frag"
        );
        // Default port is empty; empty query and fragment read as ''.
        assert_eq!(ev("var d = new URL('https://example.com:443/?#'); [d.port, d.search, d.hash, d.pathname].join('|')"), "|||/");
        // Relative resolution against a base, including a base with a path.
        assert_eq!(ev("String(new URL('../x?q', 'https://a.test/dir/sub/page.html'))"), "https://a.test/dir/x?q");
        assert_eq!(ev("String(new URL('//cdn.test/lib.js', 'https://a.test/'))"), "https://cdn.test/lib.js");
        assert_eq!(ev("String(new URL('/abs', new URL('https://a.test/dir/')))"), "https://a.test/abs");
        // Non-special schemes and an opaque origin.
        assert_eq!(ev("var m = new URL('mailto:a@b.test'); [m.protocol, m.pathname, m.origin].join('|')"), "mailto:|a@b.test|null");
        // toString / toJSON / JSON.stringify.
        assert_eq!(ev("JSON.stringify({ u: new URL('https://a.test/p') })"), r#"{"u":"https://a.test/p"}"#);
        // Invalid input throws TypeError, and canParse says so without throwing.
        assert_eq!(ev("var r; try { new URL('not a url'); r = 'no throw'; } catch (e) { r = e.name; } r"), "TypeError");
        assert_eq!(ev("var r2; try { new URL('/rel'); r2 = 'no throw'; } catch (e) { r2 = e.name; } r2"), "TypeError");
        assert_eq!(ev("[URL.canParse('https://a.test'), URL.canParse('nope'), URL.canParse('/x', 'https://a.test')].join()"), "true,false,true");
        assert_eq!(ev("String(URL.parse('nope'))"), "null");
    }

    #[test]
    fn url_setters_rewrite_the_url() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_string(&bindings, s);
        assert_eq!(
            ev("var u = new URL('https://a.test/p?x=1#h'); \
                u.pathname = '/new path'; u.hash = 'top'; u.port = '9000'; u.username = 'bob'; \
                u.href"),
            "https://bob@a.test:9000/new%20path?x=1#top"
        );
        assert_eq!(ev("var v = new URL('http://a.test:81/'); v.protocol = 'https'; v.port = ''; v.hostname = 'b.test'; v.search = '?k=v'; v.href"), "https://b.test/?k=v");
        // host takes host:port together.
        assert_eq!(ev("var w = new URL('https://a.test/'); w.host = 'c.test:444'; [w.hostname, w.port].join('|')"), "c.test|444");
        // A value that does not parse is ignored, as the standard says.
        assert_eq!(ev("var x = new URL('https://a.test:5/'); x.port = 'abc'; x.port"), "5");
    }

    #[test]
    fn url_search_params_encode_decode_and_stay_in_step_with_the_url() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_string(&bindings, s);
        // Parsing: '+' is space, percent-escapes decode, a lone key has ''.
        assert_eq!(
            ev("var p = new URLSearchParams('?a=1&b=x+y&c=%C3%A9&flag&a=2'); \
                [p.get('a'), p.getAll('a').join('/'), p.get('b'), p.get('c'), String(p.get('flag') === ''), String(p.get('none') === null), p.has('flag'), p.size].join('|')"),
            "1|1/2|x y|é|true|true|true|5"
        );
        // Serialising: application/x-www-form-urlencoded.
        assert_eq!(
            ev("var q = new URLSearchParams(); q.append('k', 'a b&c=d'); q.append('é', \"it's (ok)!~\"); q.toString()"),
            "k=a+b%26c%3Dd&%C3%A9=it%27s+%28ok%29%21%7E"
        );
        // Every constructor form.
        assert_eq!(ev("new URLSearchParams({ a: 1, b: 'two' }).toString()"), "a=1&b=two");
        assert_eq!(ev("new URLSearchParams([['a', '1'], ['b', '2']]).toString()"), "a=1&b=2");
        assert_eq!(ev("new URLSearchParams(new URLSearchParams('z=9')).toString()"), "z=9");
        // set replaces the first and drops the rest; delete; sort is stable.
        assert_eq!(ev("var s = new URLSearchParams('b=2&a=1&b=3&a=0'); s.set('b', 'X'); s.sort(); s.toString()"), "a=1&a=0&b=X");
        assert_eq!(ev("var t = new URLSearchParams('a=1&b=2&a=3'); t.delete('a'); t.toString()"), "b=2");
        // Iteration protocols.
        assert_eq!(ev("var out = []; for (var kv of new URLSearchParams('a=1&b=2')) out.push(kv.join(':')); out.join()"), "a:1,b:2");
        assert_eq!(ev("var o = []; new URLSearchParams('a=1&b=2').forEach(function (v, k) { o.push(k + v); }); o.join()"), "a1,b2");
        assert_eq!(ev("Array.from(new URLSearchParams('a=1&b=2').keys()).join()"), "a,b");
        // URL.searchParams is live in both directions.
        assert_eq!(
            ev("var u = new URL('https://a.test/p?x=1'); u.searchParams.append('y', 'a b'); u.searchParams.set('x', '9'); u.href"),
            "https://a.test/p?x=9&y=a+b"
        );
        assert_eq!(ev("var v = new URL('https://a.test/'); v.search = '?k=1&k=2'; v.searchParams.getAll('k').join()"), "1,2");
        assert_eq!(ev("var w = new URL('https://a.test/?only=1'); w.searchParams.delete('only'); w.href"), "https://a.test/");
    }

    /// The observer interfaces and requestIdleCallback: pages construct them
    /// during start-up (walmart died on MutationObserver and
    /// IntersectionObserver, microsoft on MutationObserver). They exist and
    /// validate like the real ones.
    #[test]
    fn observers_exist_validate_their_arguments_and_report_nothing() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_string(&bindings, s);
        assert_eq!(
            ev("String([typeof MutationObserver, typeof IntersectionObserver, typeof ResizeObserver, typeof PerformanceObserver].join())"),
            "function,function,function,function"
        );
        // A callback is required, and `new` is.
        assert_eq!(ev("var a; try { new MutationObserver(); a = 'no throw'; } catch (e) { a = e.name; } a"), "TypeError");
        assert_eq!(ev("var b; try { new IntersectionObserver(1); b = 'no throw'; } catch (e) { b = e.name; } b"), "TypeError");
        assert_eq!(ev("var c; try { ResizeObserver(function () {}); c = 'no throw'; } catch (e) { c = e.name; } c"), "TypeError");
        // MutationObserver.observe needs a target and at least one record type.
        assert_eq!(ev("var el = {}; var m = new MutationObserver(function () {}); var d; try { m.observe(el, {}); d = 'no throw'; } catch (e) { d = e.name; } d"), "TypeError");
        assert_eq!(ev("var e2; try { m.observe(null, { childList: true }); e2 = 'no throw'; } catch (e) { e2 = e.name; } e2"), "TypeError");
        // A target that is not a Node throws (DOM §4.3.1); records are in mutation_observer_tests.
        assert_eq!(ev("var e3; try { m.observe(el, { childList: true }); e3 = 'no throw'; } catch (e) { e3 = e.name; } e3"), "TypeError");
        assert_eq!(ev("String(m.takeRecords().length)"), "0");
        assert_eq!(ev("m.disconnect(); String(typeof WebKitMutationObserver)"), "function");
        // IntersectionObserver reports its configuration.
        assert_eq!(
            ev("var io = new IntersectionObserver(function () {}); String([io.root, io.rootMargin, io.thresholds.join()].join('|'))"),
            "|0px 0px 0px 0px|0"
        );
        assert_eq!(
            ev("var io2 = new IntersectionObserver(function () {}, { rootMargin: '10px', threshold: [1, 0.5] }); String([io2.rootMargin, io2.thresholds.join()].join('|'))"),
            "10px|0.5,1"
        );
        // Targets must be elements, as in the platform (web_observers_live.js).
        assert_eq!(ev("var f; try { io.observe(el); f = 'no throw'; } catch (e) { f = e.name; } f"), "TypeError");
        assert_eq!(ev("io.unobserve(el); io.disconnect(); String(io.takeRecords().length)"), "0");
        assert_eq!(ev("var ro = new ResizeObserver(function () {}); var g2; try { ro.observe(el); g2 = 'no throw'; } catch (e) { g2 = e.name; } ro.unobserve(el); ro.disconnect(); g2"), "TypeError");
        assert_eq!(ev("var po = new PerformanceObserver(function () {}); po.observe({ entryTypes: ['mark'] }); po.disconnect(); String(PerformanceObserver.supportedEntryTypes.length)"), "0");
    }

    #[test]
    fn request_idle_callback_runs_once_on_the_timer_clock_and_can_be_cancelled() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate(
                r#"
                var log = [];
                var id1 = requestIdleCallback(function (d) { log.push('ran:' + d.didTimeout + ':' + (d.timeRemaining() >= 0)); });
                var id2 = requestIdleCallback(function () { log.push('cancelled-ran'); });
                cancelIdleCallback(id2);
                var thrown = 'none';
                try { requestIdleCallback('nope'); } catch (e) { thrown = e.name; }
                "#,
            )
            .unwrap();
        assert_eq!(eval_string(&bindings, "String(typeof id1 + ':' + (id1 !== id2) + ':' + thrown)"), "number:true:TypeError");
        assert_eq!(eval_string(&bindings, "log.join()"), "", "not run before the timers advance");
        bindings.run_timers(1_000, 100).unwrap();
        assert_eq!(eval_string(&bindings, "log.join()"), "ran:false:true");
    }

    /// btoa/atob, escape/unescape, TextEncoder/TextDecoder: netflix died on
    /// `TextEncoder is not defined`, squarespace on `escape`, and base64 and
    /// UTF-8 conversion sit in most bundles' first lines.
    #[test]
    fn btoa_and_atob_round_trip_latin1_and_reject_bad_input() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_any(&bindings, s);
        assert_eq!(ev("[btoa(''), btoa('a'), btoa('ab'), btoa('abc'), btoa('hello')].join()"), ",YQ==,YWI=,YWJj,aGVsbG8=");
        assert_eq!(ev("btoa(String.fromCharCode(0, 255, 128))"), "AP+A");
        // atob: padding optional, ASCII whitespace ignored.
        assert_eq!(ev("[atob(''), atob('YQ=='), atob('YQ'), atob('YW Jj\\n'), atob('aGVsbG8=')].join()"), ",a,a,abc,hello");
        assert_eq!(ev("atob('AP+A').split('').map(function (c) { return c.charCodeAt(0); }).join()"), "0,255,128");
        // Errors are InvalidCharacterError (DOMException) for both directions.
        assert_eq!(ev("var n1; try { btoa('\\u20ac'); n1 = 'no throw'; } catch (e) { n1 = e.name; } n1"), "InvalidCharacterError");
        assert_eq!(ev("var n2; try { atob('!!!!'); n2 = 'no throw'; } catch (e) { n2 = e.name; } n2"), "InvalidCharacterError");
        assert_eq!(ev("var n3; try { atob('a'); n3 = 'no throw'; } catch (e) { n3 = e.name; } n3"), "InvalidCharacterError");
    }

    #[test]
    fn escape_and_unescape_follow_annex_b() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_any(&bindings, s);
        assert_eq!(ev("escape('\\u00e4 b+c\\u20ac@*_-./')"), "%E4%20b+c%u20AC@*_-./");
        assert_eq!(ev("unescape('%E4%20b+c%u20AC')"), "\u{e4} b+c\u{20ac}");
        // A malformed escape passes through.
        assert_eq!(ev("unescape('%u0041%41%zz%u12')"), "AA%zz%u12");
    }

    #[test]
    fn text_encoder_produces_utf8_and_encode_into_respects_the_buffer() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_any(&bindings, s);
        assert_eq!(ev("String(new TextEncoder().encoding)"), "utf-8");
        // h é l l o space € 😀 = 1+2+1+1+1+1+3+4 bytes.
        assert_eq!(ev("var b = new TextEncoder().encode('h\\u00e9llo \\u20ac\\ud83d\\ude00'); b.length + ':' + Array.from(b).slice(0, 4).join()"), "14:104,195,169,108");
        assert_eq!(ev("Array.from(new TextEncoder().encode('\\ud83d\\ude00')).join()"), "240,159,152,128");
        // A lone surrogate is U+FFFD (EF BF BD), not an exception.
        assert_eq!(ev("Array.from(new TextEncoder().encode('a\\ud800b')).join()"), "97,239,191,189,98");
        assert_eq!(ev("String(new TextEncoder().encode().length) + ',' + new TextEncoder().encode('').length"), "0,0");
        // encodeInto stops before a character that does not fit.
        assert_eq!(ev("var d = new Uint8Array(4); var r = new TextEncoder().encodeInto('a\\u20acb', d); r.read + ',' + r.written + ',' + Array.from(d).join()"), "2,4,97,226,130,172");
        assert_eq!(ev("var d2 = new Uint8Array(3); var r2 = new TextEncoder().encodeInto('a\\u20ac', d2); r2.read + ',' + r2.written"), "1,1");
    }

    #[test]
    fn text_decoder_decodes_utf8_with_replacement_bom_fatal_and_streaming() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let ev = |s: &str| eval_any(&bindings, s);
        // Round trip, from a Uint8Array, an ArrayBuffer and a sub-view.
        assert_eq!(ev("var s = 'h\\u00e9llo \\u20ac\\ud83d\\ude00'; String(new TextDecoder().decode(new TextEncoder().encode(s)) === s)"), "true");
        assert_eq!(ev("new TextDecoder().decode(new Uint8Array([104, 105]).buffer)"), "hi");
        assert_eq!(ev("var big = new Uint8Array([0, 104, 105, 0]); new TextDecoder().decode(big.subarray(1, 3))"), "hi");
        // Invalid bytes become U+FFFD; a truncated sequence at the end is one.
        assert_eq!(ev("new TextDecoder().decode(new Uint8Array([0x61, 0xFF, 0x62])) === 'a\\ufffdb'"), "true");
        assert_eq!(ev("new TextDecoder().decode(new Uint8Array([0x61, 0xE2, 0x82])) === 'a\\ufffd'"), "true");
        // fatal throws TypeError.
        assert_eq!(ev("var f; try { new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xFF])); f = 'no throw'; } catch (e) { f = e.name; } f"), "TypeError");
        // A leading BOM is dropped unless ignoreBOM.
        assert_eq!(ev("String(new TextDecoder().decode(new Uint8Array([0xEF, 0xBB, 0xBF, 0x61])).length)"), "1");
        assert_eq!(ev("String(new TextDecoder('utf-8', { ignoreBOM: true }).decode(new Uint8Array([0xEF, 0xBB, 0xBF, 0x61])).length)"), "2");
        // stream: a character split across chunks decodes once whole.
        assert_eq!(
            ev("var sd = new TextDecoder(); var p1 = sd.decode(new Uint8Array([0xE2, 0x82]), { stream: true }); \
                var p2 = sd.decode(new Uint8Array([0xAC, 0x21])); (p1 + '|' + p2) === '|\\u20ac!'"),
            "true"
        );
        // Labels and the other supported encodings.
        assert_eq!(ev("[new TextDecoder().encoding, new TextDecoder('UTF8').encoding, new TextDecoder('latin1').encoding, new TextDecoder('utf-16le').encoding].join()"), "utf-8,utf-8,windows-1252,utf-16le");
        assert_eq!(ev("new TextDecoder('latin1').decode(new Uint8Array([0xE9]))"), "\u{e9}");
        assert_eq!(ev("new TextDecoder('utf-16le').decode(new Uint8Array([0x61, 0x00, 0xAC, 0x20]))"), "a\u{20ac}");
        assert_eq!(ev("var l; try { new TextDecoder('no-such-label'); l = 'no throw'; } catch (e) { l = e.name; } l"), "RangeError");
    }

    #[test]
    fn window_is_the_global_object() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate("window.fromWindow = 'w'; var fromVar = 'v';")
            .unwrap();
        assert_eq!(eval_string(&bindings, "fromWindow + window.fromVar"), "wv");
        assert!(matches!(
            bindings.evaluate("window === globalThis && self === window").unwrap(),
            JsValue::Boolean(true)
        ));
    }

    #[test]
    fn lifecycle_listeners_fire_and_their_errors_are_reported() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate(
                r#"
                var seen = [];
                document.addEventListener('DOMContentLoaded', function () { seen.push('dcl'); });
                window.addEventListener('load', function () { undefinedFn(); });
                window.onload = function () { seen.push('onload'); };
                "#,
            )
            .unwrap();
        bindings
            .fire_lifecycle_event(LifecycleTarget::Document, "DOMContentLoaded")
            .unwrap();
        bindings
            .fire_lifecycle_event(LifecycleTarget::Window, "load")
            .unwrap();

        // The throwing listener does not stop the next one.
        assert_eq!(eval_string(&bindings, "seen.join(',')"), "dcl,onload");
        let errors = bindings.take_reported_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("undefinedFn"), "{errors:?}");
        assert!(bindings.take_reported_errors().is_empty(), "drained");
    }

    /// Every member the bindings' scripts report is in
    /// `UNSUPPORTED_MEMBERS`, and every listed member is reported somewhere.
    #[test]
    fn the_unsupported_list_matches_the_reporting_scripts() {
        let sources = [
            include_str!("web_xhr.js"),
            include_str!("web_components.js"),
            include_str!("web_streams.js"),
            include_str!("web_named_access.js"),
            include_str!("web_animations.js"),
            dom::WRAPPERS_JS_FOR_TESTS,
        ]
        .concat();
        let mut reported: Vec<(String, String)> = sources
            .split("__rk.unsupported('")
            .skip(1)
            .map(|rest| {
                let (interface, rest) = rest.split_once("', '").unwrap();
                (interface.to_string(), rest.split_once("')").unwrap().0.to_string())
            })
            .collect();
        reported.sort();
        reported.dedup();
        let mut listed: Vec<(String, String)> =
            UNSUPPORTED_MEMBERS.iter().map(|(i, m)| (i.to_string(), m.to_string())).collect();
        listed.sort();
        assert_eq!(reported, listed);
    }

    /// Page script reaches neither the reporter nor the table.
    #[test]
    fn page_script_cannot_report_or_read_unsupported_members() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let seen = bindings
            .evaluate("[typeof __rustkit_unsupported, typeof __rustkit_take_unsupported, typeof __rustkit_errors_with_stacks].join()")
            .unwrap();
        assert!(matches!(seen, JsValue::String(ref s) if s == "undefined,undefined,undefined"), "{seen:?}");
        bindings
            .evaluate("try { new DOMParser().parseFromString('<a/>', 'text/xml'); } catch (e) {}")
            .unwrap();
        let apis = bindings.take_unsupported_apis();
        assert_eq!(apis.len(), 1);
        assert_eq!((apis[0].0.as_str(), apis[0].1.as_str(), apis[0].2), ("DOMParser", "parseFromString(XML)", 1));
    }

    #[test]
    fn a_listener_error_is_reported_as_text_and_as_a_report_without_page_text() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings.set_error_details(true, false).unwrap();
        bindings
            .evaluate_named(
                "window.addEventListener('load', function () { throw new Error('a\\u0000b'); });",
                "inline#1",
            )
            .unwrap();
        bindings.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
        assert_eq!(bindings.take_reported_errors(), vec!["Error: a\u{0}b".to_string()]);
        let reports = bindings.take_error_reports();
        assert_eq!(reports.len(), 1, "{reports:?}");
        let report = &reports[0];
        assert_eq!((report.value, report.name, report.message.as_deref(), report.message_len), ("error", Some("Error"), None, Some(3)));
        assert!(report.located);

        bindings.set_error_details(false, false).unwrap();
        bindings.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
        assert_eq!(bindings.take_reported_errors().len(), 1, "the script log keeps its text");
        assert!(bindings.take_error_reports().is_empty(), "no reports while off");
        assert_eq!(
            report.frames,
            vec![rustkit_js::ScriptPosition { path: "inline#1".into(), line: 1, column: 53 }]
        );
    }

    /// Page script reaches none of the queues errors are reported through.
    #[test]
    fn page_script_cannot_reach_the_error_reporter() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings.set_error_details(true, false).unwrap();
        let seen = bindings
            .evaluate("[typeof __rk, typeof window.__rustkit_errors, typeof window.__rustkit_error_text].join()")
            .unwrap();
        assert!(matches!(seen, JsValue::String(ref s) if s == "undefined,undefined,undefined"), "{seen:?}");
    }

    #[test]
    fn the_live_clock_advances_by_elapsed_time_and_reports_the_next_timer() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        assert_eq!(bindings.next_timer_delay(), None);
        bindings
            .evaluate("var log = []; setTimeout(function () { log.push('a'); }, 300);")
            .unwrap();
        assert_eq!(bindings.next_timer_delay(), Some(300));

        // Not due: nothing runs, but the time has passed.
        assert_eq!(bindings.advance_timers(100, 10).unwrap(), 0);
        assert_eq!(bindings.next_timer_delay(), Some(200));

        // A timer set now counts from now (100), not from 0.
        bindings.evaluate("setTimeout(function () { log.push('b'); }, 50);").unwrap();
        assert_eq!(bindings.advance_timers(60, 10).unwrap(), 1);
        assert_eq!(bindings.advance_timers(140, 10).unwrap(), 1);
        assert_eq!(bindings.next_timer_delay(), None);
        assert!(matches!(
            bindings.evaluate("log.join(',')").unwrap(),
            JsValue::String(s) if s == "b,a"
        ));

        // The cap holds callbacks back; the clock moves on, so they are due.
        bindings
            .evaluate("var n = 0; var t = setInterval(function () { n++; }, 10);")
            .unwrap();
        // What the cap held back is due now: the loop must not sleep on it.
        assert_eq!(bindings.advance_timers(100, 4).unwrap(), 4);
        assert_eq!(bindings.next_timer_delay(), Some(0));
        // The clock moved on to 100: the interval the cap held back runs once
        // on the next turn, however little time it is given, and is not
        // caught up (50 to 100 would be 6 runs): its next is 10 from now.
        assert_eq!(bindings.advance_timers(0, 100).unwrap(), 1);
        assert_eq!(bindings.next_timer_delay(), Some(10));
        assert_eq!(bindings.advance_timers(60, 100).unwrap(), 6);
    }

    /// A callback that runs past the frame budget must not swallow the
    /// elapsed time the turn was given. The next timer is due from the
    /// clock the turn reached, so Stop is not followed by a full extra period.
    #[test]
    fn a_frame_yield_still_advances_the_virtual_clock() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate(
                "setTimeout(function () { var end = Date.now() + 30; while (Date.now() < end) {} }, 0);
                 setTimeout(function () {}, 200);",
            )
            .unwrap();
        let (ran, yielded) = bindings.advance_timers_for_frame(80, 10, 16).unwrap();
        assert!(yielded, "the long callback did not yield the frame");
        assert_eq!(ran, 1);
        assert_eq!(bindings.next_timer_delay(), Some(120));
}

    /// A turn cut short by its slice still moves the clock and has its
    /// frame, so a `setTimeout(0)` chain that outlasts every slice does not
    /// hold back a later timer or an animation frame.
    #[test]
    fn a_zero_delay_chain_cut_by_the_slice_lets_later_timers_and_frames_run() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate(
                "var log = [], links = 0;
                 function chain() { var t = Date.now(); while (Date.now() - t < 3) {} links++; setTimeout(chain, 0); }
                 setTimeout(chain, 0);
                 setTimeout(function () { log.push('later'); }, 30);
                 requestAnimationFrame(function () { log.push('frame'); });",
            )
            .unwrap();
        let slice = Some(std::time::Duration::from_millis(10));
        for _ in 0..4 {
            assert!(bindings.advance_timers_for(20, 1000, slice).unwrap() < 10);
            // The chain is always owed.
            assert_eq!(bindings.next_timer_delay(), Some(0));
        }
        assert!(matches!(
            bindings.evaluate("log.join(',')").unwrap(),
            JsValue::String(s) if s == "frame,later"
        ));
    }

    /// The same chain through the live loop's frame-sliced turn
    /// (`advance_timers_for_frame`, the path the app runs): a turn that
    /// yields its frame still has its animation frame and moves the clock,
    /// so the later timer and the frame run under a chain that yields every
    /// turn.
    #[test]
    fn a_zero_delay_chain_that_yields_every_frame_lets_later_timers_and_frames_run() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate(
                "var log = [], links = 0;
                 function chain() { var t = Date.now(); while (Date.now() - t < 3) {} links++; setTimeout(chain, 0); }
                 setTimeout(chain, 0);
                 setTimeout(function () { log.push('later'); }, 30);
                 requestAnimationFrame(function () { log.push('frame'); });",
            )
            .unwrap();
        for _ in 0..4 {
            let (ran, yielded) = bindings.advance_timers_for_frame(20, 1000, 10).unwrap();
            assert!(yielded, "the chain outlasts the frame");
            assert!(ran < 10);
            assert_eq!(bindings.next_timer_delay(), Some(0));
        }
        assert!(matches!(
            bindings.evaluate("log.join(',')").unwrap(),
            JsValue::String(s) if s == "frame,later"
        ));
    }

    /// Chromium 143, a `requestAnimationFrame` loop across a task that holds
    /// the thread 600 ms: no callback during it, then 9 and 8 in the next two
    /// 100 ms (7 to 10 at rest). It owes one frame for the time, not 37.
    #[test]
    fn a_live_turn_runs_one_round_of_animation_frames_however_long_it_took() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let read = |js: &str| match bindings.evaluate(js).unwrap() {
            JsValue::String(s) => s,
            other => panic!("{other:?}"),
        };
        bindings
            .evaluate(
                "var stamps = [], log = [];
                 function loop(ts) { stamps.push(ts); requestAnimationFrame(loop); }
                 requestAnimationFrame(loop);",
            )
            .unwrap();
        assert_eq!(bindings.advance_timers(600, 1000).unwrap(), 1);
        // The callback is told the time of the frame it runs in.
        assert_eq!(read("stamps.join(',')"), "600");
        // The loop asked for the frame after this one.
        assert_eq!(bindings.next_timer_delay(), Some(16));
        assert_eq!(bindings.advance_timers(10, 1000).unwrap(), 0);
        assert_eq!(bindings.advance_timers(6, 1000).unwrap(), 1);
        assert_eq!(read("stamps.join(',')"), "600,616");

        // The turn's timers all run, and before its frame; a frame one of
        // them asks for is in the turn's round.
        bindings
            .evaluate(
                "var every = setInterval(function () { log.push('t'); }, 100);
                 setTimeout(function () { requestAnimationFrame(function () { log.push('f'); }); }, 50);",
            )
            .unwrap();
        assert_eq!(bindings.advance_timers(300, 1000).unwrap(), 6);
        assert_eq!(read("log.join(',')"), "t,t,t,f");
        assert_eq!(read("String(stamps.length)"), "3");

        // A callback cancelled by an earlier one of the same round does not
        // run, and one asked for inside the round waits for the next.
        bindings
            .evaluate(
                "clearInterval(every);
                 log = [];
                 var second;
                 requestAnimationFrame(function () {
                     log.push('a');
                     cancelAnimationFrame(second);
                     requestAnimationFrame(function () { log.push('c'); });
                 });
                 second = requestAnimationFrame(function () { log.push('b'); });",
            )
            .unwrap();
        bindings.advance_timers(1000, 1000).unwrap();
        assert_eq!(read("log.join(',')"), "a");
        bindings.advance_timers(16, 1000).unwrap();
        assert_eq!(read("log.join(',')"), "a,c");
    }

    /// The load's clock is virtual and has no frames of its own: a
    /// `requestAnimationFrame` loop there is a 16 ms timer, as before.
    #[test]
    fn the_load_clock_still_runs_animation_frames_as_16ms_timers() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate("var n = 0; function loop() { n++; requestAnimationFrame(loop); } requestAnimationFrame(loop);")
            .unwrap();
        assert_eq!(bindings.run_timers(160, 1000).unwrap(), 10);
    }

    #[test]
    fn timers_run_in_due_order_on_a_virtual_clock() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .evaluate(
                r#"
                var order = [];
                setTimeout(function () { order.push('b200'); }, 200);
                setTimeout(function (x) { order.push(x); }, 0, 'a0');
                var ticks = 0;
                var iv = setInterval(function () {
                    order.push('i' + (++ticks));
                    if (ticks === 2) clearInterval(iv);
                }, 150);
                setTimeout(function () { order.push('late'); }, 60000);
                "#,
            )
            .unwrap();

        let started = std::time::Instant::now();
        let ran = bindings.run_timers(5_000, 1_000).unwrap();
        assert!(started.elapsed().as_millis() < 1_000, "virtual, not wall-clock");
        assert_eq!(ran, 4);
        // Past the horizon stays queued.
        assert_eq!(eval_string(&bindings, "order.join(',')"), "a0,i1,b200,i2");
        assert_eq!(bindings.run_timers(120_000, 1_000).unwrap(), 1);
    }

    // Pin §3.4: marks coalesce into one pending bucket, taken once.
    #[test]
    fn dirty_marks_coalesce_until_taken() {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        assert_eq!(bindings.take_dirty(), DomDirty::Clean);

        bindings.mark_dirty(DomDirty::Layout);
        bindings.mark_dirty(DomDirty::Style);
        bindings.mark_dirty(DomDirty::Layout);
        assert_eq!(bindings.take_dirty(), DomDirty::Style, "the widest bucket wins");
        assert_eq!(bindings.take_dirty(), DomDirty::Clean, "taking resets it");

        // A new document drops marks made against the old one.
        bindings.mark_dirty(DomDirty::Layout);
        bindings
            .set_document(Rc::new(Document::parse_html("<p>x</p>").unwrap()))
            .unwrap();
        assert_eq!(bindings.take_dirty(), DomDirty::Clean);
    }

    // LAT7c: a Web Animations write of paint properties only marks the
    // layer for the reads that see it, not the published layout.
    #[test]
    fn lat7c_a_paint_layer_write_leaves_offset_reads_clean() {
        let d = DirtyMark::default();
        d.mark_animation_paint(DomDirty::Layout, LAYER_PAINT);
        assert!(!d.written_since_layout(), "the boxes did not move");
        assert!(!d.needs_layout_for("offset"));
        assert!(!d.needs_layout_for("client"));
        assert!(!d.needs_layout_for("rect"), "opacity moves no rect");
        assert!(d.needs_layout_for("computed_style"), "the computed value is the animated one");
        assert_eq!(d.get(), DomDirty::Layout, "the engine still paints the frame");
        assert!(!d.beyond_animation_layer());
        d.mark_animation_paint(DomDirty::Layout, LAYER_TRANSFORM);
        assert!(d.needs_layout_for("rect") && d.needs_layout_for("hit_test") && d.needs_layout_for("scroll_size"));
        assert!(!d.needs_layout_for("offset"), "a transform moves no offset box");
        d.laid_out();
        assert!(!d.needs_layout_for("computed_style"), "a layout publishes the layer");
        // A seen write afterwards forces every read again.
        d.mark(DomDirty::Layout, "text");
        assert!(d.needs_layout_for("offset"));
        assert_eq!(d.cause(), Some("text"));
    }

    // #947 x #941: the LAT7 marks (write cause, unseen writes, no-op writes)
    // against LAT2's paint-only marker (`beyond_animation_layer`).
    #[test]
    fn lat7_marks_and_the_animation_layer_marker() {
        // The WAAPI layer write is a script write: it names its cause.
        let d = DirtyMark::default();
        d.mark_animation_layer(DomDirty::Layout, "animation");
        assert!(d.written_since_layout());
        assert_eq!(d.cause(), Some("animation"), "the layer write logs cause=animation");
        assert!(!d.beyond_animation_layer(), "the layer write alone is paint-only");

        // A write the published layout cannot see (display:none subtree)
        // still changes the DOM the engine lays out next: it is beyond the
        // layer, so the paint-only frame is not taken.
        d.mark_unseen(DomDirty::Style);
        assert!(d.beyond_animation_layer(), "an unseen write is beyond the layer");
        assert_eq!(d.cause(), Some("animation"), "it does not rename the first cause");
        assert_eq!(d.take(), DomDirty::Style);
        assert!(!d.beyond_animation_layer(), "take resets it");

        // Unseen first, on a current layout: no forced layout, but beyond.
        let d = DirtyMark::default();
        d.mark_unseen(DomDirty::Layout);
        assert!(!d.written_since_layout(), "a read needs no forced layout");
        assert_eq!(d.cause(), None);
        assert!(d.beyond_animation_layer());

        // A no-op write (the same value again: `write` answers `Clean`) is
        // not a write, seen or unseen.
        let d = DirtyMark::default();
        d.mark_animation_layer(DomDirty::Layout, "animation");
        d.mark(DomDirty::Clean, "style");
        d.mark_unseen(DomDirty::Clean);
        assert!(!d.beyond_animation_layer(), "a no-op write leaves the paint-only frame alone");
        assert_eq!(d.cause(), Some("animation"));
        assert_eq!(d.get(), DomDirty::Layout);

        // A real seen write after the layer: beyond, cause kept as first.
        d.mark(DomDirty::Layout, "text");
        assert!(d.beyond_animation_layer());
        assert_eq!(d.cause(), Some("animation"));
    }

    fn bound(html: &str) -> DomBindings {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .set_document(Rc::new(Document::parse_html(html).unwrap()))
            .unwrap();
        bindings
    }

    fn eval_bool(bindings: &DomBindings, script: &str) -> bool {
        match bindings.evaluate(script).unwrap() {
            JsValue::Boolean(b) => b,
            other => panic!("{script} evaluated to {other:?}"),
        }
    }

    const MIXED: &str = "<html><body><ul id='u'> <li id='a'>1</li> <!--c--> <li id='b' \
         data-item-id='7' title='t' hidden>2</li> </ul></body></html>";

    #[test]
    fn element_traversal_skips_text_and_comments() {
        let b = bound(MIXED);
        assert_eq!(
            eval_string(
                &b,
                "var u = document.getElementById('u'), a = document.getElementById('a'), \
                     bb = document.getElementById('b'), r = []; \
                 r.push(u.firstElementChild === a, u.lastElementChild === bb, u.childElementCount, \
                        a.nextElementSibling === bb, bb.previousElementSibling === a, \
                        bb.nextElementSibling, a.previousElementSibling, \
                        u.firstChild.nextElementSibling === a, \
                        document.firstElementChild === document.documentElement, \
                        document.children.length, document.childElementCount); \
                 var p = document.createElement('p'); \
                 r.push(a.isConnected, p.isConnected, document.isConnected); \
                 u.appendChild(p); r.push(p.isConnected, u.lastElementChild === p); \
                 p.remove(); r.push(p.isConnected, p.firstElementChild, p.childElementCount); \
                 r.join(',')"
            ),
            "true,true,2,true,true,,,true,true,1,1,true,false,true,true,true,false,,0"
        );
    }

    #[test]
    fn reflected_attributes_and_dataset() {
        let b = bound(MIXED);
        assert_eq!(
            eval_string(
                &b,
                "var a = document.getElementById('a'), bb = document.getElementById('b'), r = []; \
                 r.push(bb.dataset.itemId, bb.title, bb.hidden, a.hidden, a.title === '', \
                        'itemId' in bb.dataset, 'nope' in bb.dataset, a.dataset.x); \
                 a.dataset.fooBar = 3; a.hidden = true; a.lang = 'en'; bb.hidden = false; \
                 delete bb.dataset.itemId; \
                 r.push(a.getAttribute('data-foo-bar'), a.hasAttribute('hidden'), a.getAttribute('lang'), \
                        bb.hasAttribute('hidden'), bb.getAttribute('data-item-id'), \
                        a.dataset === a.dataset); \
                 try { a.dataset['a-b'] = 1; } catch (e) { r.push(e.name); } \
                 r.join(',')"
            ),
            "7,t,true,false,true,true,false,,3,true,en,false,,true,SyntaxError"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
    }

    // The list-building pattern: fill a detached fragment, insert it once.
    #[test]
    fn document_fragment_children_move_in_on_insert() {
        let b = bound(MIXED);
        b.set_selector_matcher(Rc::new(|node, selector, _| {
            (selector != "!").then(|| node.tag_name() == Some(selector))
        }));
        assert_eq!(
            eval_string(
                &b,
                "var f = document.createDocumentFragment(), r = []; \
                 r.push(f instanceof DocumentFragment, f instanceof Node, f.nodeType, f.nodeName, \
                        Node.DOCUMENT_FRAGMENT_NODE, f.parentNode, f.isConnected); \
                 ['x', 'y'].forEach(function (t) { \
                     var li = document.createElement('li'); li.textContent = t; li.id = t; \
                     r.push(f.appendChild(li) === li); \
                 }); \
                 f.append('!'); \
                 r.push(f.childNodes.length, f.children.length, f.firstElementChild.id, \
                        f.childElementCount, f.textContent, f.querySelectorAll('li').length, \
                        f.getElementById('y') === f.lastElementChild, f.getElementById('b'), \
                        f.firstChild.parentNode === f, f.firstChild.isConnected); \
                 var u = document.getElementById('u'), first = f.firstChild; \
                 r.push(u.insertBefore(f, document.getElementById('a')) === f, \
                        f.childNodes.length, first.parentNode === u, first.isConnected, \
                        u.firstElementChild.id, document.getElementById('x') === first, \
                        u.childElementCount); \
                 r.join(',')"
            ),
            "true,true,11,#document-fragment,11,,false,true,true,3,2,x,2,xy!,2,true,,true,false,\
             true,0,true,true,x,true,4"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
        // The fragment's children landed, in order, before #a.
        assert_eq!(
            eval_string(&b, "document.getElementById('u').textContent.replace(/\\s+/g, '')"),
            "xy!12"
        );
        assert_eq!(
            eval_string(
                &b,
                "var f = document.createDocumentFragment(), r = []; \
                 f.textContent = 'abc'; r.push(f.childNodes.length, f.firstChild.data); \
                 f.replaceChildren(document.createElement('i')); r.push(f.firstChild.nodeName); \
                 try { f.appendChild(f); } catch (e) { r.push(e.name); } \
                 var g = document.createDocumentFragment(); g.appendChild(f); \
                 r.push(g.childNodes.length, f.childNodes.length, g.firstChild.nodeName); \
                 var p = document.createElement('p'); p.append(g, 'z'); \
                 r.push(p.innerHTML, g.childNodes.length, \
                        Object.prototype.toString.call(g)); \
                 r.join(',')"
            ),
            "1,abc,I,HierarchyRequestError,1,0,I,<i></i>z,0,[object DocumentFragment]"
        );
    }

    const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title></head>
<body><div id="main" class="box"><p class="x">Hello, <b>world</b>!</p><!--c--><p class="x">Two</p></div>
<p id="outside" class="x">Out</p></body></html>"#;

    // The injected matcher decides querySelector/All, matches and closest,
    // and its `None` (an invalid selector) throws SyntaxError. The stand-in
    // here matches on the tag name alone; "!" is its invalid selector.
    #[test]
    fn an_injected_selector_matcher_answers_queries_matches_and_closest() {
        let b = bound(PAGE);
        b.set_selector_matcher(Rc::new(|node, selector, _| {
            (selector != "!").then(|| node.tag_name() == Some(selector))
        }));
        assert!(eval_bool(&b, "document.querySelectorAll('p').length === 3"));
        assert!(eval_bool(
            &b,
            "document.getElementById('main').querySelectorAll('p').length === 2 && \
             document.getElementById('main').querySelector('b').textContent === 'world'"
        ));
        assert!(eval_bool(
            &b,
            "var w = document.querySelector('b'); \
             w.matches('b') && !w.matches('p') && w.webkitMatchesSelector('b') && \
             w.closest('p') === w.parentNode && w.closest('div').id === 'main' && \
             w.closest('b') === w && w.closest('table') === null"
        ));
        for call in [
            "document.querySelector('!')",
            "document.querySelectorAll('!')",
            "document.body.querySelector('!')",
            "document.body.matches('!')",
            "document.body.closest('!')",
        ] {
            assert_eq!(
                eval_string(&b, &format!("try {{ {call}; 'no throw' }} catch (e) {{ e.name }}")),
                "SyntaxError",
                "{call}"
            );
        }
    }

    // Pin (a): one wrapper per node, whatever the entry point.
    #[test]
    fn get_element_by_id_is_the_same_object_as_query_selector() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "document.getElementById('main') === document.querySelector('#main')"
        ));
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'); \
             m.firstChild.parentNode === m && m.childNodes[0] === m.firstChild && \
             document.body.parentNode === document.documentElement && \
             document.documentElement.parentNode === document"
        ));
    }

    // Pin (b): textContent is the Rust DOM's text.
    #[test]
    fn text_content_reads_the_rust_dom() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(&b, "document.getElementById('main').textContent"),
            "Hello, world!Two"
        );
        assert_eq!(
            eval_string(&b, "document.body.textContent.replace(/\\s+/g, ' ').trim()"),
            "Hello, world!Two Out"
        );
        assert!(eval_bool(&b, "document.textContent === null"));
    }

    // Pin (c): a missing id is null, not a stub object.
    #[test]
    fn missing_id_is_null() {
        let b = bound(PAGE);
        assert!(eval_bool(&b, "document.getElementById('nope') === null"));
        assert!(eval_bool(&b, "document.querySelector('#nope') === null"));
        assert!(eval_bool(&b, "document.querySelectorAll('.nope').length === 0"));
    }

    // Pin (d): wrappers from the previous document never read the new one's
    // nodes, although NodeIds restart per document.
    #[test]
    fn old_wrappers_do_not_read_the_next_document() {
        let b = bound(PAGE);
        b.evaluate("var old = document.getElementById('main');").unwrap();
        b.set_document(Rc::new(
            Document::parse_html("<html><body><div id='main'>New</div></body></html>")
                .unwrap(),
        ))
        .unwrap();
        assert!(eval_bool(
            &b,
            "old.textContent === null && old.firstChild === null && \
             old.getAttribute('id') === null && old.querySelector('p') === null"
        ));
        assert!(eval_bool(&b, "document.getElementById('main') !== old"));
        assert_eq!(
            eval_string(&b, "document.getElementById('main').textContent"),
            "New"
        );
    }

    #[test]
    fn element_reads_and_scoped_queries() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'); \
                 [m.tagName, m.localName, m.id, m.className, m.getAttribute('CLASS'), \
                  m.children.length, m.childNodes.length, m.childNodes[1].nodeType].join('|')"
            ),
            "DIV|div|main|box|box|2|3|8"
        );
        // Scoped to the element's descendants; document-wide sees all three.
        assert!(eval_bool(
            &b,
            "document.getElementById('main').querySelectorAll('.x').length === 2 && \
             document.querySelectorAll('.x').length === 3 && \
             document.getElementsByTagName('p').item(2) === document.getElementById('outside')"
        ));
        assert!(eval_bool(
            &b,
            "document.body instanceof HTMLElement && document.body instanceof Node && \
             document instanceof Document && document.head.firstChild.tagName === 'TITLE'"
        ));
        assert!(eval_bool(
            &b,
            "var n = 0; document.querySelectorAll('p').forEach(function () { n++; }); n === 3"
        ));
        assert!(eval_bool(
            &b,
            "try { new Node(); false } catch (e) { e instanceof TypeError }"
        ));
    }

    #[test]
    fn unbound_document_reads_are_null() {
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        assert!(eval_bool(
            &b,
            "document.body === null && document.getElementById('x') === null && \
             document.querySelectorAll('p').length === 0"
        ));
    }

    // Tree moves write the Rust tree, keep wrapper identity, and mark the
    // §3 bucket the engine flushes.
    #[test]
    fn append_child_moves_the_rust_node_and_marks_style() {
        let b = bound(PAGE);
        assert_eq!(b.take_dirty(), DomDirty::Clean);
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'), o = document.getElementById('outside'); \
             m.appendChild(o) === o && o.parentNode === m && m.lastChild === o && \
             m.querySelectorAll('.x').length === 3 && m.textContent === 'Hello, world!TwoOut'"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Style);
        // The move is in the Rust tree, not a JS-side overlay.
        let doc = b.window.borrow().document.clone().unwrap();
        let main = doc.get_element_by_id("main").unwrap();
        assert_eq!(
            main.last_child().unwrap().get_attribute("id"),
            Some("outside")
        );
        assert_eq!(main.text_content(), "Hello, world!TwoOut");
    }

    #[test]
    fn insert_before_orders_children() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'), o = document.getElementById('outside'); \
                 m.insertBefore(o, m.firstChild); var a = m.textContent; \
                 m.insertBefore(o, null); var z = m.textContent; \
                 m.insertBefore(o, o); [a, z, m.lastChild === o].join('|')"
            ),
            "OutHello, world!Two|Hello, world!TwoOut|true"
        );
    }

    #[test]
    fn removed_nodes_are_detached_but_their_wrappers_still_read() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var o = document.getElementById('outside'); \
             document.body.removeChild(o) === o && o.parentNode === null && \
             o.textContent === 'Out' && document.getElementById('outside') === null && \
             document.querySelectorAll('#outside').length === 0 && \
             document.body.appendChild(o) === o && document.getElementById('outside') === o"
        ));
        assert!(eval_bool(
            &b,
            "var p = document.querySelector('.x'); p.remove(); p.remove(); \
             p.parentNode === null && document.querySelectorAll('.x').length === 2"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Style);
    }

    // DOM §4.2.3 for a Document parent: one element child, at most one
    // doctype before it, no text. A page that removes `<html>` and appends a
    // new one gets a document whose head and body are the new tree's (yahoo
    // threw at `document.head.appendChild` after a root swap).
    #[test]
    fn a_document_takes_a_new_root_once_the_old_one_is_removed() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var h = document.createElement('html'), hd = document.createElement('head'), \
                     bd = document.createElement('body'); \
                 h.appendChild(hd); h.appendChild(bd); \
                 document.removeChild(document.documentElement); \
                 var none = [document.documentElement, document.head, document.body].join(); \
                 document.appendChild(h); \
                 var s = document.createElement('script'); document.head.appendChild(s); \
                 [none, document.documentElement === h, document.head === hd, document.body === bd, \
                  s.parentNode === hd, document.lastChild === h, document.firstChild.nodeType].join()"
            ),
            ",,,true,true,true,true,true,10"
        );
    }

    #[test]
    fn replace_child_swaps_the_document_root() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var old = document.documentElement, h = document.createElement('html'); \
                 h.appendChild(document.createElement('body')); \
                 var r = document.replaceChild(h, old); \
                 [r === old, old.parentNode, document.documentElement === h, \
                  document.body === h.firstChild, document.childNodes.length].join()"
            ),
            "true,,true,true,2"
        );
    }

    #[test]
    fn a_document_refuses_a_second_root_text_and_a_root_before_its_doctype() {
        let b = bound(PAGE);
        let threw = |script: &str| {
            eval_string(
                &b,
                &format!(
                    "(function () {{ try {{ {script}; return 'no throw'; }} \
                          catch (e) {{ return e.name; }} }})()"
                ),
            )
        };
        // With a root: no second element, also not by replacing the doctype.
        assert_eq!(threw("document.appendChild(document.createElement('html'))"), "HierarchyRequestError");
        assert_eq!(
            threw("document.replaceChild(document.createElement('html'), document.firstChild)"),
            "HierarchyRequestError"
        );
        assert_eq!(threw("document.appendChild(document.createTextNode('x'))"), "HierarchyRequestError");
        // A comment is fine anywhere in a document.
        assert_eq!(threw("document.appendChild(document.createComment('c'))"), "no throw");
        // replaceWith of a node that is one of its own arguments: it leaves
        // first, so nothing is replaced and the root still counts (#804 R1).
        assert_eq!(
            threw(
                "var c = document.createComment('r'); document.insertBefore(c, document.documentElement); \
                 c.replaceWith(c, document.createElement('html'))"
            ),
            "HierarchyRequestError"
        );
        assert_eq!(
            eval_string(
                &b,
                "String(Array.prototype.filter.call(document.childNodes, function (n) { return n.nodeType === 1; }).length)"
            ),
            "1"
        );
        // Without a root: no element before the doctype, no two elements
        // from one fragment.
        assert_eq!(
            threw(
                "document.removeChild(document.documentElement); \
                 document.insertBefore(document.createElement('html'), document.firstChild)"
            ),
            "HierarchyRequestError"
        );
        assert_eq!(
            threw(
                "var f = document.createDocumentFragment(); \
                 f.appendChild(document.createElement('html')); f.appendChild(document.createElement('html')); \
                 document.appendChild(f)"
            ),
            "HierarchyRequestError"
        );
        assert_eq!(
            eval_string(&b, "[document.documentElement, document.childNodes.length].join()"),
            ",3"
        );
    }

    // DOM §4.2.3 DocumentType half: at most one doctype, and only before the
    // element. The real `document.doctype` is the only path that reaches
    // `document_child_check`'s DocumentType arm — `createDocumentType` is a
    // fake JS object and never exercises it (#804).
    #[test]
    fn a_document_refuses_a_doctype_after_its_root_and_takes_it_back_before() {
        let b = bound(PAGE);
        let threw = |script: &str| {
            eval_string(
                &b,
                &format!(
                    "(function () {{ try {{ {script}; return 'no throw'; }} \
                          catch (e) {{ return e.name; }} }})()"
                ),
            )
        };
        // With a root: append puts the doctype after the element. DocumentType
        // has no ChildNode.remove (only Element/CharacterData), so use
        // removeChild. replaceChild onto a trailing comment is the same
        // refusal; the comment is cleaned up so later asserts stay local.
        assert_eq!(threw("document.appendChild(document.doctype)"), "HierarchyRequestError");
        assert_eq!(
            threw(
                "var c = document.createComment('x'); document.appendChild(c); \
                 try { document.replaceChild(document.doctype, c); } \
                 finally { if (c.parentNode) document.removeChild(c); }"
            ),
            "HierarchyRequestError"
        );
        // Remove, then re-insert before the root: success, and the doctype is
        // once again the first child.
        assert_eq!(
            eval_string(
                &b,
                "var dt = document.doctype, root = document.documentElement; \
                 document.removeChild(dt); \
                 var gone = [document.doctype, document.firstChild === root].join(); \
                 document.insertBefore(dt, root); \
                 [gone, document.doctype === dt, document.firstChild === dt, \
                  document.lastChild === root, document.childNodes.length].join()"
            ),
            ",true,true,true,true,2"
        );
        // Without a root: append of the lone doctype succeeds once; a second
        // doctype (via replaceChild onto a comment) is refused.
        assert_eq!(
            threw(
                "var dt = document.doctype; document.removeChild(document.documentElement); \
                 document.removeChild(dt); document.appendChild(dt)"
            ),
            "no throw"
        );
        assert_eq!(
            threw(
                "var c = document.createComment('d'); document.appendChild(c); \
                 try { document.replaceChild(document.doctype, c); } \
                 finally { if (c.parentNode) document.removeChild(c); }"
            ),
            "HierarchyRequestError"
        );
        assert_eq!(
            eval_string(
                &b,
                "[document.doctype !== null, document.documentElement, \
                  document.firstChild === document.doctype, document.childNodes.length].join()"
            ),
            "true,,true,1"
        );
    }

    // DOM §4.2.3 validity: the DOMException named, and the tree untouched.
    #[test]
    fn invalid_tree_writes_throw_and_change_nothing() {
        let b = bound(PAGE);
        let threw = |script: &str| {
            eval_string(
                &b,
                &format!(
                    "(function () {{ try {{ {script}; return 'no throw'; }} \
                          catch (e) {{ return e.name + '/' + (e instanceof DOMException); }} }})()"
                ),
            )
        };
        let m = "document.getElementById('main')";
        assert_eq!(
            threw(&format!("{m}.firstChild.appendChild({m})")),
            "HierarchyRequestError/true"
        );
        assert_eq!(
            threw(&format!("{m}.appendChild({m})")),
            "HierarchyRequestError/true"
        );
        assert_eq!(
            threw(&format!("{m}.firstChild.firstChild.appendChild({m})")),
            "HierarchyRequestError/true"
        );
        assert_eq!(
            threw(&format!("{m}.appendChild(document)")),
            "HierarchyRequestError/true"
        );
        assert_eq!(
            threw(&format!("document.appendChild({m})")),
            "HierarchyRequestError/true"
        );
        assert_eq!(
            threw(&format!("{m}.removeChild(document.body)")),
            "NotFoundError/true"
        );
        assert_eq!(
            threw(&format!(
                "{m}.insertBefore(document.body.lastChild, document.body)"
            )),
            "NotFoundError/true"
        );
        assert_eq!(threw(&format!("{m}.appendChild({{}})")), "TypeError/false");
        assert_eq!(
            threw(&format!("{m}.insertBefore({m}.firstChild)")),
            "TypeError/false"
        );
        assert_eq!(
            b.take_dirty(),
            DomDirty::Clean,
            "a failed write marks nothing"
        );
        assert_eq!(
            eval_string(&b, &format!("{m}.textContent")),
            "Hello, world!Two"
        );
    }

    // Pin (d) for writes: a wrapper from the previous document never names
    // a node of the next one.
    #[test]
    fn old_wrappers_cannot_write_the_next_document() {
        let b = bound(PAGE);
        b.evaluate("var old = document.getElementById('outside');")
            .unwrap();
        b.set_document(Rc::new(
            Document::parse_html("<html><body><div id='main'>New</div></body></html>").unwrap(),
        ))
        .unwrap();
        assert_eq!(
            eval_string(
                &b,
                "var r = []; \
                 try { document.body.appendChild(old); } catch (e) { r.push(e.name); } \
                 try { old.appendChild(document.body); } catch (e) { r.push(e.name); } \
                 try { document.body.insertBefore(document.getElementById('main'), old); } \
                 catch (e) { r.push(e.name); } r.join(',')"
            ),
            "NotFoundError,NotFoundError,NotFoundError"
        );
        assert_eq!(b.take_dirty(), DomDirty::Clean);
    }

    // Writes to a node's own data go through replace-on-write in the Rust
    // DOM: same NodeId, so the wrapper and every identity path are kept.
    #[test]
    fn set_attribute_writes_the_rust_node_and_keeps_identity() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'); \
             m.setAttribute('data-k', 'v'); m.setAttribute('TITLE', 't'); \
             m.getAttribute('data-k') === 'v' && m.getAttribute('title') === 't' && \
             document.getElementById('main') === m && \
             document.querySelector('#main') === m && \
             m.firstChild.parentNode === m && m.textContent === 'Hello, world!Two'"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Style);
        let doc = b.window.borrow().document.clone().unwrap();
        let main = doc.get_element_by_id("main").unwrap();
        assert_eq!(main.get_attribute("data-k"), Some("v"));
        assert_eq!(main.get_attribute("title"), Some("t"));
    }

    #[test]
    fn remove_and_toggle_attribute() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'); var r = []; \
                 m.removeAttribute('class'); r.push(m.hasAttribute('class')); \
                 r.push(m.toggleAttribute('hidden'), m.getAttribute('hidden')); \
                 r.push(m.toggleAttribute('hidden'), m.hasAttribute('hidden')); \
                 r.push(m.toggleAttribute('hidden', false)); r.join(',')"
            ),
            "false,true,,false,false,false"
        );
    }

    #[test]
    fn an_unchanged_attribute_write_marks_nothing() {
        let b = bound(PAGE);
        b.evaluate("document.getElementById('main').setAttribute('class', 'box'); \
                    document.getElementById('main').removeAttribute('nope');")
            .unwrap();
        assert_eq!(b.take_dirty(), DomDirty::Clean);
    }

    #[test]
    fn id_and_class_name_setters_move_the_lookups() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'); m.id = 'renamed'; m.className = 'a b'; \
             document.getElementById('main') === null && \
             document.getElementById('renamed') === m && m.id === 'renamed' && \
             document.getElementsByClassName('b')[0] === m && \
             document.querySelector('.a') === m"
        ));
    }

    #[test]
    fn class_list_edits_the_class_attribute() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'), c = m.classList, r = []; \
                 c.add('a', 'b', 'a'); r.push(m.className, c.length, c.contains('b')); \
                 c.remove('box'); r.push(m.className); \
                 r.push(c.toggle('z'), c.toggle('z'), c.toggle('a', true)); \
                 r.push(c.replace('a', 'q'), c.value, c.item(0), String(c.item(9))); \
                 r.push(m.classList === c, document.querySelector('.q') === m); \
                 try { c.add(''); } catch (e) { r.push(e.name); } \
                 try { c.add('x y'); } catch (e) { r.push(e.name); } \
                 r.join('|')"
            ),
            "box a b|3|true|a b|true|false|true|true|q b|q|null|true|true|SyntaxError|InvalidCharacterError"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
    }

    #[test]
    fn text_content_setter_replaces_an_elements_children() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'), p = m.firstChild; \
             m.textContent = 'plain'; \
             m.childNodes.length === 1 && m.firstChild.nodeType === 3 && \
             m.textContent === 'plain' && p.parentNode === null && \
             document.querySelectorAll('.x').length === 1"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Style);
        let doc = b.window.borrow().document.clone().unwrap();
        assert_eq!(doc.get_element_by_id("main").unwrap().text_content(), "plain");
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'); m.textContent = ''; \
             var a = m.firstChild === null; m.textContent = null; \
             a && m.childNodes.length === 0 && m.textContent === ''"
        ));
    }

    #[test]
    fn character_data_setters_keep_the_text_node() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var o = document.getElementById('outside'), t = o.firstChild; \
             t.data = 'Changed'; var a = o.textContent === 'Changed' && o.firstChild === t; \
             t.nodeValue = 'Again'; t.textContent = 'Last'; \
             a && t.data === 'Last' && t.length === 4 && o.textContent === 'Last'"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Layout);
    }

    #[test]
    fn created_nodes_are_detached_until_inserted() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var li = document.createElement('LI'); \
             var ok = li.tagName === 'LI' && li.localName === 'li' && li.parentNode === null && \
                      li instanceof HTMLElement && li.ownerDocument === document; \
             li.className = 'item'; li.appendChild(document.createTextNode('new')); \
             li.appendChild(document.createComment('c')); \
             ok && li.textContent === 'new' && li.childNodes.length === 2 && \
             document.querySelector('.item') === null"
        ));
        assert_eq!(
            b.take_dirty(),
            DomDirty::Style,
            "writes to a detached node are marked but harmless"
        );
        assert!(eval_bool(
            &b,
            "document.body.appendChild(li) === li && \
             document.querySelector('.item') === li && \
             document.body.lastChild === li && document.body.textContent.slice(-3) === 'new'"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Style);
    }

    #[test]
    fn invalid_names_throw_invalid_character_error() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var r = []; \
                 try { document.createElement(''); } catch (e) { r.push(e.name); } \
                 try { document.createElement('a b'); } catch (e) { r.push(e.name); } \
                 try { document.body.setAttribute('a=b', 'x'); } catch (e) { r.push(e.name); } \
                 try { document.body.setAttribute('x'); } catch (e) { r.push(e.name); } \
                 r.join(',')"
            ),
            "InvalidCharacterError,InvalidCharacterError,InvalidCharacterError,TypeError"
        );
        assert_eq!(b.take_dirty(), DomDirty::Clean);
    }

    #[test]
    fn old_wrappers_cannot_write_data_in_the_next_document() {
        let b = bound(PAGE);
        b.evaluate("var old = document.getElementById('main');").unwrap();
        b.set_document(Rc::new(
            Document::parse_html("<html><body><div id='main'>New</div></body></html>").unwrap(),
        ))
        .unwrap();
        assert_eq!(
            eval_string(
                &b,
                "var r = []; \
                 try { old.setAttribute('class', 'x'); } catch (e) { r.push(e.name); } \
                 try { old.textContent = 'x'; } catch (e) { r.push(e.name); } \
                 r.push(document.getElementById('main').textContent); r.join(',')"
            ),
            "NotFoundError,NotFoundError,New"
        );
        assert_eq!(b.take_dirty(), DomDirty::Clean);
    }

    #[test]
    fn style_reads_and_writes_the_style_attribute() {
        let b = bound(
            "<html><body><div id='d' style='color: red; margin-top:4px !important'>x</div></body></html>",
        );
        assert_eq!(
            eval_string(
                &b,
                "var d = document.getElementById('d'), s = d.style, r = []; \
                 r.push(s.color, s.marginTop, s.getPropertyValue('margin-top'), \
                        s.getPropertyPriority('margin-top'), s.length, s[0], s.fontSize === ''); \
                 s.backgroundColor = 'blue'; s.color = ''; s.setProperty('float', 'left'); \
                 r.push(d.getAttribute('style'), s.cssFloat, d.style === s); \
                 s.cssText = 'width: 10px'; r.push(s.width, s.length); \
                 d.style = 'height: 5px'; r.push(d.getAttribute('style'), s.removeProperty('height'), s.length); \
                 r.join('|')"
            ),
            "red|4px|4px|important|2|color|true|\
             margin-top: 4px !important; background-color: blue; float: left;|left|true|\
             10px|1|height: 5px|5px|0"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
        let doc = b.window.borrow().document.clone().unwrap();
        assert_eq!(doc.get_element_by_id("d").unwrap().get_attribute("style"), Some(""));
    }

    /// CH2.E27: the attribute is a declaration list, so the `;` in a data
    /// URL's `;base64,` does not end the declaration, and a write elsewhere
    /// keeps the URL whole. Chromium 148.0.7778.216 on the same page:
    /// `2|background-image|red|url("data:image/svg+xml;base64,AA==")|3|
    /// background-image: url("data:image/svg+xml;base64,AA=="); color: red; width: 1px;`
    /// -- the same declarations; it re-serializes the URL quoted, this
    /// keeps the author's text.
    #[test]
    fn style_keeps_a_semicolon_inside_a_data_url() {
        let b = bound(
            "<html><body><div id='d' style='background-image:url(data:image/svg+xml;base64,AA==);color:red'>x</div></body></html>",
        );
        assert_eq!(
            eval_string(
                &b,
                "var d = document.getElementById('d'), s = d.style, r = [s.length, s[0], s.color, s.backgroundImage]; \
                 s.width = '1px'; r.push(s.length, d.getAttribute('style')); r.join('|')"
            ),
            "2|background-image|red|url(data:image/svg+xml;base64,AA==)|3|\
             background-image: url(data:image/svg+xml;base64,AA==); color: red; width: 1px;"
        );
    }

    /// The JS copy of the declaration-list rule (`splitDecls`) on the case
    /// list `rustkit_cssparser`'s own copy runs, read back through
    /// `CSSStyleDeclaration` (#972 R1 nit 3: the two copies must not drift).
    /// A declaration whose value is a bad url is split the same way but is
    /// not in the block (CH2.E29: Chromium's `el.style` leaves it out), so
    /// the read-back leaves those out of the split.
    #[test]
    fn style_splits_the_shared_declaration_list_cases() {
        let doc: serde_json::Value = serde_json::from_str(include_str!(
            "../../rustkit-cssparser/src/declaration_list_cases.json"
        ))
        .expect("cases");
        let b = bound("<html><body><div id='d'>x</div></body></html>");
        let mut wrong = Vec::new();
        for case in doc["cases"].as_array().expect("cases") {
            let input = case["input"].as_str().unwrap();
            let want: Vec<String> = case["decls"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| {
                    let v = d[1].as_str().unwrap();
                    let is_url = v.get(..4).is_some_and(|h| h.eq_ignore_ascii_case("url("));
                    // A url left open at the end of the text is still a url
                    // token (CSS Syntax 3 §4.3.6, EOF): close it to ask.
                    let closed = if v.ends_with(')') { v.to_string() } else { format!("{v})") };
                    !is_url || rustkit_css::mask::parse_css_url(&closed).is_some()
                })
                .map(|d| format!("{}={}", d[0].as_str().unwrap(), d[1].as_str().unwrap()))
                .collect();
            let script = format!(
                "var d = document.getElementById('d'); d.setAttribute('style', {}); \
                 var s = d.style, r = []; for (var i = 0; i < s.length; i++) r.push(s[i] + '=' + s.getPropertyValue(s[i])); \
                 JSON.stringify(r)",
                serde_json::to_string(input).unwrap()
            );
            let got: Vec<String> = serde_json::from_str(&eval_string(&b, &script)).expect("json");
            if got != want {
                wrong.push(format!("{input:?}: got {got:?}, want {want:?}"));
            }
        }
        assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
    }

    const TREE: &str = "<html><body><div id='o'><p id='i'>x</p></div></body></html>";

    #[test]
    fn element_events_run_capture_target_and_bubble_phases() {
        let b = bound(TREE);
        assert_eq!(
            eval_string(
                &b,
                "var o = document.getElementById('o'), i = document.getElementById('i'), log = []; \
                 function on(t, name, capture) { t.addEventListener('ping', function (e) { \
                     log.push(name + e.eventPhase + (e.currentTarget === t) + (e.target === i)); }, capture); } \
                 on(window, 'w', true); on(document, 'd', false); on(window, 'W', false); \
                 on(o, 'oc', true); on(o, 'ob', false); on(i, 'ib', false); on(i, 'ic', { capture: true }); \
                 i.onping = function () { log.push('handler'); }; \
                 var e = new Event('ping', { bubbles: true }); \
                 log.push(i.dispatchEvent(e), e.eventPhase, e.currentTarget === null); \
                 i.dispatchEvent(new CustomEvent('ping')); \
                 log.join(',')"
            ),
            "w1truetrue,oc1truetrue,ic2truetrue,ib2truetrue,handler,ob3truetrue,d3truetrue,W3truetrue,true,0,true,\
             w1truetrue,oc1truetrue,ic2truetrue,ib2truetrue,handler"
        );
        assert_eq!(b.take_dirty(), DomDirty::Clean);
    }

    #[test]
    fn listeners_stop_once_dedupe_and_prevent_default() {
        let b = bound(TREE);
        assert_eq!(
            eval_string(
                &b,
                "var o = document.getElementById('o'), i = document.getElementById('i'), log = []; \
                 function f() { log.push('f'); } \
                 i.addEventListener('a', f); i.addEventListener('a', f); \
                 i.addEventListener('a', function () { log.push('once'); }, { once: true }); \
                 i.dispatchEvent(new Event('a')); i.dispatchEvent(new Event('a')); \
                 i.removeEventListener('a', f); i.dispatchEvent(new Event('a')); log.push('|'); \
                 i.addEventListener('b', function (e) { e.stopPropagation(); log.push('i1'); }); \
                 i.addEventListener('b', function () { log.push('i2'); }); \
                 o.addEventListener('b', function () { log.push('o'); }); \
                 i.dispatchEvent(new Event('b', { bubbles: true })); log.push('|'); \
                 i.addEventListener('c', function (e) { e.stopImmediatePropagation(); log.push('c1'); }); \
                 i.addEventListener('c', function () { log.push('c2'); }); \
                 i.dispatchEvent(new Event('c')); log.push('|'); \
                 o.onclick = function () { return false; }; \
                 var c = new Event('click', { bubbles: true, cancelable: true }); \
                 log.push(i.dispatchEvent(c), c.defaultPrevented); \
                 var n = new Event('click', { bubbles: true }); \
                 log.push(i.dispatchEvent(n), n.defaultPrevented); \
                 log.join(',')"
            ),
            "f,once,f,|,i1,i2,|,c1,|,false,true,true,false"
        );
    }

    #[test]
    fn listener_errors_are_logged_and_click_dispatches() {
        let b = bound(TREE);
        assert_eq!(
            eval_string(
                &b,
                "var i = document.getElementById('i'), log = []; \
                 i.addEventListener('click', function () { missingFunction(); }); \
                 i.addEventListener('click', { handleEvent: function (e) { log.push(e.type, e.bubbles); } }); \
                 document.body.addEventListener('click', function (e) { log.push(e.target === i); }); \
                 i.click(); \
                 try { i.dispatchEvent('click'); } catch (e) { log.push(e instanceof TypeError); } \
                 try { Event('x'); } catch (e) { log.push(e instanceof TypeError); } \
                 log.push(i instanceof EventTarget, document instanceof EventTarget, \
                          Object.prototype.toString.call(new Event('x'))); \
                 log.join(',')"
            ),
            "click,true,true,true,true,true,true,[object Event]"
        );
        let errors = b.take_reported_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("missingFunction"), "{errors:?}");
    }

    // An `on<type>` content attribute is an event handler (HTML §8.1.8.1):
    // its text is the body of `function (event)`, called with the element as
    // `this`, with the element and its document in scope.
    const INLINE: &str = "<html><body><div id='o'><a id='i' href='https://example.com/' \
         onclick=\"window.seen = [this.id, event.type, id, typeof getElementById].join(':'); return false\">x</a>\
         <b id='bad' onclick='this is not script'>y</b></div></body></html>";

    #[test]
    fn an_on_attribute_is_the_elements_handler() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var i = document.getElementById('i'), log = []; \
                 var c = new Event('click', { bubbles: true, cancelable: true }); \
                 log.push(i.dispatchEvent(c), c.defaultPrevented, window.seen); \
                 i.setAttribute('onclick', 'window.seen = 2'); \
                 var d = new Event('click', { bubbles: true, cancelable: true }); \
                 log.push(i.dispatchEvent(d), window.seen); \
                 log.join(',')"
            ),
            "false,true,i:click:i:function,true,2"
        );
    }

    #[test]
    fn an_assigned_handler_replaces_the_attributes_and_null_turns_it_off() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var i = document.getElementById('i'), log = []; \
                 i.onclick = function () { window.seen = 'property'; }; i.click(); log.push(window.seen); \
                 i.onclick = null; window.seen = 'off'; i.click(); log.push(window.seen); \
                 i.removeAttribute('onclick'); i.click(); log.push(window.seen); \
                 log.join(',')"
            ),
            "property,off,off"
        );
    }

    #[test]
    fn an_on_attribute_that_does_not_compile_is_logged_once_and_not_thrown() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var bad = document.getElementById('bad'), log = []; \
                 document.body.addEventListener('click', function () { log.push('bubbled'); }); \
                 bad.click(); bad.click(); \
                 log.join(',')"
            ),
            "bubbled,bubbled"
        );
        let errors = b.take_reported_errors();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].starts_with("SyntaxError"), "{errors:?}");
    }

    // HTML §8.1.8.2 (GlobalEventHandlers): `on<type>` is an accessor on
    // HTMLElement.prototype, not a data property of each element. The getter
    // answers the assigned value, else the compiled content attribute, else
    // null; a non-object assigned is null; setting the content attribute
    // again makes the attribute the handler.
    #[test]
    fn on_handlers_are_accessors_on_the_element_prototype() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var i = document.getElementById('i'), log = []; \
                 var has = function (o, k) { return Object.prototype.hasOwnProperty.call(o, k); }; \
                 var o = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onerror'); \
                 log.push(typeof o, o && typeof o.get, o && typeof o.set, o && o.enumerable, o && o.configurable); \
                 log.push(has(HTMLElement.prototype, 'onload'), has(HTMLElement.prototype, 'onclick'), \
                          has(SVGElement.prototype, 'onclick'), has(Element.prototype, 'onclick')); \
                 var s = document.createElement('script'); \
                 log.push(String(s.onerror), String(s.onload)); \
                 function h() {} \
                 s.onerror = h; log.push(s.onerror === h, has(s, 'onerror')); \
                 s.onerror = 'nope'; log.push(String(s.onerror)); \
                 log.push(typeof i.onclick, i.onclick === i.onclick); \
                 i.onclick = null; log.push(String(i.onclick)); \
                 i.setAttribute('onclick', 'window.seen = 3'); i.click(); log.push(window.seen); \
                 log.join(',')"
            ),
            "object,function,function,true,true,true,true,true,false,null,null,true,false,null,function,true,null,3"
        );
    }

    // R1 L1: the native setAttr lowercases the name for HTML elements, so a
    // mixed-case `OnClick` is the `onclick` content attribute and must drop
    // the assigned handler just as the lowercase spelling does.
    #[test]
    fn a_mixed_case_on_attribute_drops_the_assigned_handler() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var i = document.getElementById('i'), log = []; \
                 window.__hit = 0; window.__f = 0; \
                 function f() { window.__f++; } \
                 i.onclick = f; \
                 i.setAttribute('OnClick', 'window.__hit=(window.__hit||0)+1'); \
                 i.click(); \
                 log.push(window.__f, window.__hit, i.onclick === f, typeof i.onclick, \
                          i.getAttribute('onclick') === 'window.__hit=(window.__hit||0)+1'); \
                 log.join(',')"
            ),
            "0,1,false,function,true"
        );
    }

    // R1 B1: removing an absent attribute runs no attribute-change steps
    // (DOM "remove an attribute by name" on a missing attribute is a no-op),
    // so `removeAttribute('onclick')` with no such attribute keeps the
    // IDL-assigned handler, as Chrome does. Removing one that IS present
    // still deactivates it.
    #[test]
    fn removing_an_absent_on_attribute_keeps_the_assigned_handler() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var o = document.getElementById('o'), log = []; \
                 var b = document.createElement('button'); o.appendChild(b); \
                 window.__f = 0; function f() { window.__f++; } \
                 b.onclick = f; \
                 b.removeAttribute('onclick'); log.push(b.onclick === f); \
                 b.removeAttribute('OnClick'); log.push(b.onclick === f); \
                 b.removeAttributeNS(null, 'onclick'); log.push(b.onclick === f); \
                 b.click(); log.push(window.__f); \
                 b.setAttribute('onclick', ''); b.removeAttribute('onclick'); \
                 log.push(String(b.onclick)); b.click(); log.push(window.__f); \
                 log.join(',')"
            ),
            "true,true,true,1,null,1"
        );
    }

    // A page's script-retry shim wraps the native accessor: it takes
    // `Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onerror')` and
    // calls `o.get.call(this)` / `o.set.call(this, fn)` from its own. The
    // handler it stores through the original setter is the one dispatched.
    #[test]
    fn a_wrapped_on_handler_accessor_still_dispatches() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var o = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onerror'), log = []; \
                 Object.defineProperty(HTMLElement.prototype, 'onerror', { configurable: true, enumerable: true, \
                     get: function () { var e = o.get.call(this); return (e && e.__origin) || e; }, \
                     set: function (t) { \
                         var i = this; \
                         if (typeof t !== 'function') return o.set.call(this, t); \
                         var s = function () { log.push('wrapped'); return t.apply(i, arguments); }; \
                         s.__origin = t; o.set.call(this, s); \
                     } }); \
                 var s = document.createElement('script'); \
                 s.src = 'https://example.invalid/x.js'; \
                 s.onload = function () {}; \
                 var n = function (e) { log.push('error:' + (this === s) + ':' + e.type); }; \
                 s.onerror = n; \
                 log.push(s.onerror === n); \
                 document.body.appendChild(s); \
                 s.dispatchEvent(new Event('error')); \
                 s.onerror = null; log.push(String(s.onerror)); \
                 s.dispatchEvent(new Event('error')); \
                 log.join(',')"
            ),
            "true,wrapped,error:true:error,null"
        );
        assert_eq!(b.take_reported_errors(), Vec::<String>::new());
    }

    // HTML §8.1.8.2.1 (event handlers on body and frameset elements): the
    // WindowEventHandlers set and the Window-reflecting set (onload,
    // onresize, onscroll, onerror, onfocus, onblur) on the body element are
    // the Window's handlers. `<body onload>` compiles into the Window's
    // `load` handler, with the document and the body in scope and the Window
    // as `this`; the Window's `load` dispatch runs it.
    const BODY_ONLOAD: &str = "<html><body id='b' onload=\"window.__l = [this === window, id, typeof getElementById].join(':')\">\
         <a id='i'>x</a></body></html>";

    #[test]
    fn a_body_onload_attribute_is_the_windows_load_handler() {
        let b = bound(BODY_ONLOAD);
        assert_eq!(
            eval_string(
                &b,
                "var log = []; \
                 log.push(typeof window.onload, document.body.onload === window.onload, String(window.__l)); \
                 log.join(',')"
            ),
            "function,true,undefined"
        );
        b.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
        assert_eq!(eval_string(&b, "String(window.__l)"), "true:b:function");
        assert_eq!(b.take_reported_errors(), Vec::<String>::new());
    }

    // `document.body.onload = f` is `window.onload = f`: both read the same
    // slot, and `load` at the Window runs it with the Window as `this`.
    #[test]
    fn a_body_onload_assignment_is_the_windows_load_handler() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var log = []; \
                 function f() { window.__ran = (window.__ran || 0) + 1; window.__this = this === window; } \
                 document.body.onload = f; \
                 log.push(window.onload === f, document.body.onload === f); \
                 var o = Object.getOwnPropertyDescriptor(HTMLBodyElement.prototype, 'onload'); \
                 log.push(typeof o, o && typeof o.get, o && typeof o.set); \
                 log.push(Object.prototype.hasOwnProperty.call(HTMLBodyElement.prototype, 'onhashchange'), \
                          Object.prototype.hasOwnProperty.call(HTMLBodyElement.prototype, 'onclick')); \
                 log.join(',')"
            ),
            "true,true,object,function,function,true,false"
        );
        b.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
        assert_eq!(eval_string(&b, "[window.__ran, window.__this].join(',')"), "1,true");
        // The other direction too: a Window assignment shows on the body,
        // and null turns the handler off on both.
        assert_eq!(
            eval_string(
                &b,
                "function h() {} window.onresize = h; var log = [document.body.onresize === h]; \
                 document.body.onresize = null; log.push(String(window.onresize), String(document.body.onresize)); \
                 window.onhashchange = h; log.push(document.body.onhashchange === h); \
                 log.join(',')"
            ),
            "true,null,null,true"
        );
    }

    // The body's on<type> content attribute for a Window type, set later,
    // is the Window's handler (and replaces an assigned one, as on elements);
    // removing it leaves no handler. A bubbling event dispatched at the body
    // runs the handler once, at the Window, not again at the body.
    #[test]
    fn a_body_window_attribute_set_later_and_dispatch_once_at_window() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var log = [], body = document.body; \
                 window.__n = 0; \
                 function f() { log.push('assigned'); } \
                 window.onscroll = f; \
                 body.setAttribute('onscroll', 'window.__n++; window.__at = (this === window) + \":\" + (event.currentTarget === window)'); \
                 log.push(window.onscroll === f, typeof window.onscroll, body.onscroll === window.onscroll); \
                 body.dispatchEvent(new Event('scroll', { bubbles: true })); \
                 log.push(window.__n, window.__at); \
                 body.dispatchEvent(new Event('scroll')); \
                 log.push(window.__n); \
                 body.removeAttribute('onscroll'); \
                 log.push(String(window.onscroll), String(body.onscroll)); \
                 body.dispatchEvent(new Event('scroll', { bubbles: true })); \
                 log.push(window.__n); \
                 log.join(',')"
            ),
            "false,function,true,1,true:true,1,null,null,1"
        );
        assert_eq!(b.take_reported_errors(), Vec::<String>::new());
    }

    // GlobalEventHandlers that are not in the Window-reflecting set stay the
    // body element's own: `body.onclick` is not `window.onclick`.
    #[test]
    fn a_body_onclick_stays_the_elements_own_handler() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var log = [], body = document.body; \
                 function f() { log.push('click:' + (this === body)); } \
                 body.onclick = f; \
                 log.push(body.onclick === f, String(window.onclick)); \
                 body.dispatchEvent(new Event('click', { bubbles: true })); \
                 body.setAttribute('onclick', 'window.__c = this === document.body'); \
                 body.dispatchEvent(new Event('click', { bubbles: true })); \
                 log.push(window.__c, String(window.onclick)); \
                 log.join(',')"
            ),
            "true,undefined,click:true,true,undefined"
        );
    }

    // HTML §8.1.8.1 "determine the target of an event handler": a body or
    // frameset element's handler for a Window type is the Window's whenever
    // the element's node document is the active document, attached or not
    // (a `createElement('body')` forwards). One whose node document is
    // another document (a DOMParser body: no Window) has no target: the
    // getter answers null and assignment does nothing. A `load` dispatched
    // at a body runs nothing at the body itself: the handler is the Window's.
    #[test]
    fn any_body_of_the_active_document_forwards_and_another_documents_body_has_no_target() {
        let b = bound(INLINE);
        assert_eq!(
            eval_string(
                &b,
                "var log = [], d = document.createElement('body'); \
                 function f() { window.__ran = (window.__ran || 0) + 1; } \
                 d.onload = f; \
                 log.push(d.onload === f, window.onload === f, document.body.onload === f); \
                 d.onhashchange = f; log.push(window.onhashchange === f); \
                 d.onload = null; log.push(String(window.onload), String(d.onload)); \
                 window.onload = f; log.push(d.onload === f); \
                 d.dispatchEvent(new Event('load')); log.push(String(window.__ran)); \
                 var p = new DOMParser().parseFromString('<body onload=\"window.__p = 1\"></body>', 'text/html'); \
                 log.push(p.body.ownerDocument === p, String(p.body.onload)); \
                 p.body.onload = function () { window.__q = 1; }; \
                 log.push(String(p.body.onload), window.onload === f); \
                 p.body.dispatchEvent(new Event('load')); \
                 log.join(',')"
            ),
            "true,true,true,true,null,null,true,undefined,true,null,null,true"
        );
        b.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
        assert_eq!(
            eval_string(&b, "[String(window.__ran), String(window.__p), String(window.__q)].join(',')"),
            "1,undefined,undefined"
        );
        assert_eq!(b.take_reported_errors(), Vec::<String>::new());
    }

    // HTML §3.1.3: a frameset document's body element is its frameset, so
    // `<frameset onload>` is the Window's `load` handler as `<body onload>`
    // is, and `frameset.onload` reads the Window's slot. `onpagereveal` and
    // `onpageswap` are WindowEventHandlers members too (§8.1.8.2.1).
    #[test]
    fn a_frameset_onload_attribute_is_the_windows_load_handler() {
        let b = bound(
            "<html><head></head><frameset cols='*' onload=\"window.__f = 1\"><frame src='a'></frameset></html>",
        );
        assert_eq!(
            eval_string(
                &b,
                "var fs = document.body, log = [fs && fs.localName]; \
                 log.push(typeof window.onload, fs.onload === window.onload, String(window.__f)); \
                 log.push(Object.prototype.hasOwnProperty.call(HTMLFrameSetElement.prototype, 'onpagereveal'), \
                          Object.prototype.hasOwnProperty.call(HTMLBodyElement.prototype, 'onpageswap'), \
                          Object.prototype.hasOwnProperty.call(window, 'onpagereveal')); \
                 log.join(',')"
            ),
            "frameset,function,true,undefined,true,true,true"
        );
        b.fire_lifecycle_event(LifecycleTarget::Window, "load").unwrap();
        assert_eq!(eval_string(&b, "String(window.__f)"), "1");
        assert_eq!(b.take_reported_errors(), Vec::<String>::new());
    }

    const ROW: &str =
        "<html><body><div id='r'><i id='a'>a</i><i id='b'>b</i><i id='c'>c</i></div><i id='x'>x</i></body></html>";

    /// Each step's children of #r: element ids, and Text data in quotes.
    const ORDER: &str = "var r = document.getElementById('r'), out = []; \
         function $(id) { return document.getElementById(id); } var a = $('a'); \
         function order() { return Array.prototype.map.call(r.childNodes, function (n) { \
             return n.nodeType === 1 ? n.id : \"'\" + n.data + \"'\"; }).join(''); }";

    #[test]
    fn parent_node_append_prepend_and_replace_children() {
        let b = bound(ROW);
        assert_eq!(
            eval_string(
                &b,
                &[ORDER, " \
                     r.append($('x'), 's'); out.push(order()); \
                     r.prepend('p', $('c')); out.push(order()); \
                     r.append(); out.push(order()); \
                     r.replaceChildren($('b'), 'n'); out.push(order(), a.parentNode === null); \
                     out.join('|')"]
                .concat()
            ),
            "abcx's'|'p'cabx's'|'p'cabx's'|b'n'|true"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
        let doc = b.window.borrow().document.clone().unwrap();
        assert_eq!(doc.get_element_by_id("r").unwrap().text_content(), "bn");
    }

    #[test]
    fn child_node_before_after_and_replace_with() {
        let b = bound(ROW);
        assert_eq!(
            eval_string(
                &b,
                &[ORDER, " \
                     $('b').before($('x'), 't'); out.push(order()); \
                     $('b').after($('a')); out.push(order()); \
                     $('b').before($('b')); out.push(order()); \
                     $('x').after($('x'), $('c')); out.push(order()); \
                     $('b').replaceWith('B', $('b')); out.push(order()); \
                     $('a').replaceWith('A'); out.push(order(), a.parentNode === null); \
                     var lone = document.createElement('p'); lone.before('q'); lone.replaceWith('q'); \
                     out.push(lone.parentNode === null); \
                     out.join('|')"]
                .concat()
            ),
            "ax't'bc|x't'bac|x't'bac|xc't'ba|xc't''B'ba|xc't''B'b'A'|true|true"
        );
    }

    #[test]
    fn replace_child_swaps_and_validates_first() {
        let b = bound(ROW);
        assert_eq!(
            eval_string(
                &b,
                &[ORDER, " \
                     var old = $('b'); out.push(r.replaceChild($('x'), old) === old, old.parentNode === null); \
                     out.push(order()); \
                     out.push(r.replaceChild($('a'), $('a')) === $('a'), order()); \
                     r.replaceChild($('c'), $('a')); out.push(order()); \
                     try { r.replaceChild(document.createElement('p'), old); } catch (e) { out.push(e.name); } \
                     try { $('x').replaceChild(r, $('x').firstChild); } catch (e) { out.push(e.name); } \
                     try { r.replaceChild('s', $('x')); } catch (e) { out.push(e.name); } \
                     out.push(order()); out.join('|')"]
                .concat()
            ),
            "true|true|axc|true|axc|cx|NotFoundError|HierarchyRequestError|TypeError|cx"
        );
    }

    #[test]
    fn inner_and_outer_html_serialize_the_rust_tree() {
        let b = bound(PAGE);
        assert_eq!(
            eval_string(&b, "document.getElementById('main').innerHTML"),
            "<p class=\"x\">Hello, <b>world</b>!</p><!--c--><p class=\"x\">Two</p>"
        );
        assert_eq!(
            eval_string(&b, "document.getElementById('outside').outerHTML"),
            "<p class=\"x\" id=\"outside\">Out</p>"
        );
        // Escaping: text escapes & < > and NBSP; attributes also escape ".
        // Void elements have no end tag; raw-text elements are not escaped.
        let b = bound(
            "<html><body><div id=d title='a\"&lt;b'>1 &amp; 2 &lt;3&gt;&nbsp;<br><img src=x.png></div>\
             <style id=s>a > b { }</style></body></html>",
        );
        assert_eq!(
            eval_string(&b, "document.getElementById('d').outerHTML"),
            "<div id=\"d\" title=\"a&quot;&lt;b\">1 &amp; 2 &lt;3&gt;&nbsp;<br><img src=\"x.png\"></div>"
        );
        assert_eq!(eval_string(&b, "document.getElementById('s').innerHTML"), "a > b { }");
        assert_eq!(b.take_dirty(), DomDirty::Clean, "reads mark nothing");
    }

    #[test]
    fn inner_html_setter_parses_and_replaces_the_children() {
        let b = bound(PAGE);
        // `</li>` is written out: rustkit-html has no implied end tags for
        // `li` yet (in document parses too).
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'), old = m.firstChild; \
             m.innerHTML = '<ul id=\"l\"><li class=\"i\">one</li><li class=\"i\">two <b>2</b></li></ul>tail'; \
             var l = document.getElementById('l'); \
             old.parentNode === null && m.childNodes.length === 2 && l.parentNode === m && \
             l.children.length === 2 && document.querySelectorAll('.i').length === 2 && \
             l.children[1].lastChild.tagName === 'B' && m.lastChild.data === 'tail' && \
             m.innerHTML === '<ul id=\"l\"><li class=\"i\">one</li><li class=\"i\">two <b>2</b></li></ul>tail'"
        ));
        assert_eq!(b.take_dirty(), DomDirty::Style);
        // The Rust DOM holds the parsed nodes (the cascade and layout read it).
        let doc = b.window.borrow().document.clone().unwrap();
        assert_eq!(doc.get_element_by_id("main").unwrap().text_content(), "onetwo 2tail");
        // Empty and null clear the element.
        assert!(eval_bool(
            &b,
            "var m = document.getElementById('main'); m.innerHTML = ''; \
             var a = m.firstChild === null; m.innerHTML = '<i>x</i>'; m.innerHTML = null; \
             a && m.childNodes.length === 0 && m.innerHTML === ''"
        ));
    }

    #[test]
    fn get_element_by_id_finds_an_id_reused_by_new_content() {
        let b = bound(PAGE);
        assert!(eval_bool(
            &b,
            "var main = document.getElementById('main'); \
             document.body.innerHTML = '<span id=\"main\">again</span>'; \
             var now = document.getElementById('main'); \
             main.parentNode === null && now !== main && now.tagName === 'SPAN' && \
             now.parentNode === document.body && \
             document.getElementById('outside') === null && \
             document.getElementById('') === null"
        ));
    }
}
