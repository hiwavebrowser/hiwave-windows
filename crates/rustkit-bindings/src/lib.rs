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

pub use dom::SelectorMatchFn;
pub mod events;

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
/// `window.onerror`), so those are caught here and queued in
/// `__rustkit_errors` for the engine to collect. The one thing that does
/// propagate is Boa's loop-iteration limit, which a script cannot catch.
///
/// Timers run on a virtual clock the engine advances (`__rustkit_run_timers`),
/// so a page's `setTimeout(f, 2000)` runs without the load waiting 2s of wall
/// time. Promise reactions run when the enclosing evaluation returns, not
/// between two timer callbacks.
const PAGE_LIFECYCLE_JS: &str = r#"
(function () {
    var errors = [];
    window.__rustkit_errors = errors;
    function report(e) {
        var msg;
        try { msg = String(e); } catch (_) { msg = '<unprintable exception>'; }
        errors.push(msg);
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
    window.__rustkit_fire = function (targetName, type) {
        var target = targetName === 'document' ? document : window;
        target.dispatchEvent({
            type: type, bubbles: false, cancelable: false, defaultPrevented: false,
            target: target, currentTarget: null, timeStamp: now, isTrusted: true,
            preventDefault: function () { this.defaultPrevented = true; },
            stopPropagation: function () {}, stopImmediatePropagation: function () {}
        });
    };

    function schedule(cb, ms, args, repeat) {
        var delay = Number(ms) || 0;
        if (delay < 0) delay = 0;
        var id = nextId++;
        timers.push({ id: id, seq: id, due: now + delay, cb: cb, args: args,
                      every: repeat ? Math.max(delay, 1) : 0 });
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
        return schedule(function () { cb(now); }, 16, [], false);
    };
    window.cancelAnimationFrame = clearTimer;
    window.queueMicrotask = function (cb) {
        Promise.resolve().then(function () { try { cb(); } catch (e) { report(e); } });
    };

    // Run due timers in (due, scheduling) order until the virtual clock
    // would pass `horizon` ms or `max` callbacks have run.
    window.__rustkit_run_timers = function (horizon, max) {
        var ran = 0;
        while (ran < max) {
            var best = -1;
            for (var i = 0; i < timers.length; i++) {
                var t = timers[i];
                if (best < 0 || t.due < timers[best].due ||
                    (t.due === timers[best].due && t.seq < timers[best].seq)) best = i;
            }
            if (best < 0 || timers[best].due > horizon) break;
            var timer = timers[best];
            now = timer.due;
            if (timer.every) { timer.due += timer.every; timer.seq = nextId++; }
            else timers.splice(best, 1);
            ran++;
            try {
                if (typeof timer.cb === 'function') timer.cb.apply(window, timer.args);
                else (0, eval)(String(timer.cb));
            } catch (e) { report(e); }
        }
        return ran;
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

/// DOM bindings context.
pub struct DomBindings {
    runtime: RefCell<JsRuntime>,
    window: RefCell<WindowState>,
    event_listeners: RefCell<Vec<EventListener>>,
    /// The document `document`/`Node`/`Element` read from (see `dom`).
    dom_host: dom::SharedDomHost,
    /// Queue of IPC messages from JavaScript
    _ipc_queue: RefCell<Vec<IpcMessage>>,
    /// Pending invalidation from script DOM writes (see `DomDirty`).
    /// Shared with the tree-write host functions, which mark it.
    dirty: Rc<Cell<DomDirty>>,
}

impl DomBindings {
    /// Create new DOM bindings.
    pub fn new(mut runtime: JsRuntime) -> Result<Self, BindingError> {
        debug!("Initializing DOM bindings");

        // Inject global objects
        Self::inject_globals(&mut runtime)?;
        let dom_host = dom::SharedDomHost::default();
        let dirty = Rc::new(Cell::new(DomDirty::Clean));
        dom::install(&mut runtime, &dom_host, &dirty)?;

        Ok(Self {
            runtime: RefCell::new(runtime),
            window: RefCell::new(WindowState::default()),
            event_listeners: RefCell::new(Vec::new()),
            dom_host,
            _ipc_queue: RefCell::new(Vec::new()),
            dirty,
        })
    }

    /// Record that script invalidated `bucket`. Marks combine: the pending
    /// bucket only grows until `take_dirty`.
    pub fn mark_dirty(&self, bucket: DomDirty) {
        self.dirty.set(self.dirty.get().max(bucket));
    }

    /// The invalidation pending since the last call, which is reset to
    /// `Clean`. The engine calls this once when script settles.
    pub fn take_dirty(&self) -> DomDirty {
        self.dirty.replace(DomDirty::Clean)
    }

    /// The `<input>`/`<textarea>` values script set since the last call, as
    /// (raw NodeId, value) in write order. The engine copies them into its
    /// edit state, which layout paints from, when it flushes `take_dirty`.
    pub fn take_value_writes(&self) -> Vec<(usize, String)> {
        self.dom_host.borrow_mut().take_value_writes()
    }

    /// Tell script what the user typed into a control, so its `value`
    /// reads the edit state's text rather than the default.
    pub fn sync_control_value(&self, node: usize, value: String) {
        self.dom_host.borrow_mut().sync_value(node, value);
    }

    /// Inject global JavaScript objects.
    fn inject_globals(runtime: &mut JsRuntime) -> Result<(), BindingError> {
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
                matchMedia: function(query) {
                    return { matches: false, media: query, addEventListener: function() {} };
                },
                alert: function(msg) { console.log('[alert]', msg); },
                confirm: function(msg) { console.log('[confirm]', msg); return false; },
                prompt: function(msg, def) { console.log('[prompt]', msg); return def || null; }
            });

            // Alias
            var self = window;
        "#;

        runtime.evaluate_script(window_js)?;

        // IPC bridge for communication with Rust
        let ipc_js = r#"
            // IPC queue for postMessage calls
            window.__ipcQueue = [];

            // IPC object for browser-to-Rust communication
            window.ipc = {
                postMessage: function(message) {
                    // Store message in queue for Rust to poll
                    window.__ipcQueue.push(message);
                }
            };

            // Helper to drain the IPC queue (called from Rust)
            window.__drainIpcQueue = function() {
                var queue = window.__ipcQueue;
                window.__ipcQueue = [];
                return JSON.stringify(queue);
            };

            // HiWave Chrome API (for Chrome UI compatibility)
            window.hiwaveChrome = {
                postMessage: function(message) {
                    window.ipc.postMessage(message);
                }
            };
        "#;

        runtime.evaluate_script(ipc_js)?;

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
        runtime.evaluate_script(PAGE_LIFECYCLE_JS)?;

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

    /// Set the document.
    pub fn set_document(&self, document: Rc<Document>) -> Result<(), BindingError> {
        // Update state. Marks against the previous document are moot: the
        // new one gets a full layout of its own.
        self.window.borrow_mut().document = Some(document.clone());
        self.dirty.set(DomDirty::Clean);

        // Sync to JS
        let title = document.title().unwrap_or_default();
        let mut runtime = self.runtime.borrow_mut();
        runtime.evaluate_script(&format!("document.title = {:?};", title))?;
        runtime.evaluate_script("document.readyState = 'complete';")?;

        // Rebind the DOM wrappers. Wrappers handed out for a previous
        // document stop resolving (their generation no longer matches).
        let generation = self.dom_host.borrow_mut().bind(document);
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
            window.location.href = {:?};
            window.location.protocol = {:?};
            window.location.host = {:?};
            window.location.hostname = {:?};
            window.location.port = {:?};
            window.location.pathname = {:?};
            window.location.search = {:?};
            window.location.hash = {:?};
            window.location.origin = {:?};
            document.URL = {:?};
            "#,
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

    /// Bound every loop a page script runs (see
    /// [`JsRuntime::set_loop_iteration_limit`]).
    pub fn set_loop_iteration_limit(&self, max_iterations: u64) {
        self.runtime
            .borrow_mut()
            .set_loop_iteration_limit(max_iterations);
    }

    /// Set `document.readyState` (`loading` / `interactive` / `complete`).
    pub fn set_ready_state(&self, state: &str) -> Result<(), BindingError> {
        self.runtime
            .borrow_mut()
            .evaluate_script(&format!("document.readyState = {:?};", state))?;
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

    /// Exceptions thrown in listeners and timer callbacks since the last
    /// call, as `String(error)` (`TypeError: x is not a function`).
    pub fn take_reported_errors(&self) -> Vec<String> {
        let drained = self
            .runtime
            .borrow_mut()
            .evaluate_script("JSON.stringify(window.__rustkit_errors.splice(0))");
        match drained {
            Ok(JsValue::String(json)) => serde_json::from_str(&json).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Drain the IPC message queue.
    ///
    /// This method collects all IPC messages that were queued via
    /// `window.ipc.postMessage()` since the last drain call.
    ///
    /// Returns a Vec of IpcMessage structs.
    pub fn drain_ipc_queue(&self) -> Vec<IpcMessage> {
        // Call JS to drain the queue and get JSON
        let result = self.runtime
            .borrow_mut()
            .evaluate_script("window.__drainIpcQueue()");

        match result {
            Ok(JsValue::String(json)) => {
                // Parse the JSON array
                match serde_json::from_str::<Vec<String>>(&json) {
                    Ok(messages) => {
                        messages
                            .into_iter()
                            .map(|payload| IpcMessage { payload })
                            .collect()
                    }
                    Err(e) => {
                        trace!(error = %e, "Failed to parse IPC queue JSON");
                        Vec::new()
                    }
                }
            }
            Ok(_) => {
                trace!("IPC queue returned non-string value");
                Vec::new()
            }
            Err(e) => {
                trace!(error = %e, "Failed to drain IPC queue");
                Vec::new()
            }
        }
    }

    /// Check if there are pending IPC messages.
    pub fn has_pending_ipc(&self) -> bool {
        let result = self.runtime
            .borrow_mut()
            .evaluate_script("window.__ipcQueue.length > 0");

        matches!(result, Ok(JsValue::Boolean(true)))
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

    fn eval_string(bindings: &DomBindings, script: &str) -> String {
        match bindings.evaluate(script).unwrap() {
            JsValue::String(s) => s,
            other => panic!("{script} evaluated to {other:?}"),
        }
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
        b.set_selector_matcher(Rc::new(|node, selector| {
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
        b.set_selector_matcher(Rc::new(|node, selector| {
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
                 log.push(window.__rustkit_errors.length, /missingFunction/.test(window.__rustkit_errors[0])); \
                 try { i.dispatchEvent('click'); } catch (e) { log.push(e instanceof TypeError); } \
                 try { Event('x'); } catch (e) { log.push(e instanceof TypeError); } \
                 log.push(i instanceof EventTarget, document instanceof EventTarget, \
                          Object.prototype.toString.call(new Event('x'))); \
                 log.join(',')"
            ),
            "click,true,true,1,true,true,true,true,true,[object Event]"
        );
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
