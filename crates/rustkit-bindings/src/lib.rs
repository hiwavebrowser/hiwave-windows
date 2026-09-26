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

pub mod events;

pub use events::{
    AnimationEventData, DataTransfer, DragEventData, DroppedFile, Event, EventDispatcher,
    EventListenerEntry, EventListenerOptions, EventPhase, ExtendedEventData, FocusManager,
    FocusVisibility, FocusableElement, HoverTracker, MessageEventData, PointerEventData,
    PointerLockState, PointerType, RafCallbackId, RafScheduler, Touch, TouchEventData,
    TransitionEventData, WheelDeltaMode, WheelEventData,
};

use rustkit_dom::{Document, Node, NodeId};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::RefCell;
use std::collections::HashMap;
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

/// DOM bindings context.
pub struct DomBindings {
    runtime: RefCell<JsRuntime>,
    window: RefCell<WindowState>,
    event_listeners: RefCell<Vec<EventListener>>,
    node_map: RefCell<HashMap<u64, Rc<Node>>>,
    /// Queue of IPC messages from JavaScript
    _ipc_queue: RefCell<Vec<IpcMessage>>,
}

impl DomBindings {
    /// Create new DOM bindings.
    pub fn new(mut runtime: JsRuntime) -> Result<Self, BindingError> {
        debug!("Initializing DOM bindings");

        // Inject global objects
        Self::inject_globals(&mut runtime)?;

        Ok(Self {
            runtime: RefCell::new(runtime),
            window: RefCell::new(WindowState::default()),
            event_listeners: RefCell::new(Vec::new()),
            node_map: RefCell::new(HashMap::new()),
            _ipc_queue: RefCell::new(Vec::new()),
        })
    }

    /// Inject global JavaScript objects.
    fn inject_globals(runtime: &mut JsRuntime) -> Result<(), BindingError> {
        // Window object stub
        let window_js = r#"
            var window = {
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
            };

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
            var document = {
                _elements: {},
                documentElement: null,
                head: null,
                body: null,
                title: '',
                readyState: 'loading',
                cookie: '',
                domain: '',
                referrer: '',
                URL: 'about:blank',
                
                getElementById: function(id) {
                    return this._elements[id] || null;
                },
                
                getElementsByTagName: function(tagName) {
                    return [];
                },
                
                getElementsByClassName: function(className) {
                    return [];
                },
                
                querySelector: function(selector) {
                    return null;
                },
                
                querySelectorAll: function(selector) {
                    return [];
                },
                
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

        debug!("Global objects injected");
        Ok(())
    }

    /// Set the document.
    pub fn set_document(&self, document: Rc<Document>) -> Result<(), BindingError> {
        // Update state
        self.window.borrow_mut().document = Some(document.clone());

        // Sync to JS
        let title = document.title().unwrap_or_default();
        let mut runtime = self.runtime.borrow_mut();
        runtime.evaluate_script(&format!("document.title = {:?};", title))?;
        runtime.evaluate_script("document.readyState = 'complete';")?;

        // Index elements by ID
        document.traverse(|node| {
            if let Some(_id) = node.get_attribute("id") {
                let node_id = node.id.raw();
                self.node_map
                    .borrow_mut()
                    .insert(node_id as u64, node.clone());
            }
        });

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
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

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
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

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
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

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
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

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
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

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
        let runtime = JsRuntime::new().unwrap();
        let bindings = DomBindings::new(runtime).unwrap();

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
}
