//! HiWave Inspector, Console and Network half: the per-view record of what a
//! page load did, in order, for an inspector window, the site board and
//! debugging agents to read.
//!
//! Each view owns one [`Inspector`]. It records only while something listens:
//! the view's ring buffer is on ([`InspectorConfig::buffer`]) or a sink is
//! attached ([`Inspector::add_sink`]). Otherwise every recording call returns
//! after one atomic load, before any event is built or any string formatted.
//!
//! Privacy (the log is pasted into public issues and PRs): a URL in an event
//! keeps its scheme, host, port and path, never its user name, password,
//! query, fragment or `;` path parameters (`/a;jsessionid=S/b` is `/a/b`; a
//! flag keeps the query, fragment and parameters); a `data:` URL keeps only
//! its MIME type, when it is a common one. A `file:` URL's path is the local
//! path, user name and all. A response's MIME type is a name from a fixed
//! list (else `<type>/other`, else none). No header, request or response
//! body, form value or storage is recorded. No text the page chose is recorded by default
//! (pages log and throw cookies, form values and their own URL): a console
//! message is each argument's type and a string's length; an error is the
//! thrown value's type, a standard error name (else `Error`), its message's
//! length, and the positions of the page's frames (script, line, column;
//! never a function name). No error message is kept, whatever it says;
//! only the engine's own failures, marked where the engine raises them,
//! carry a fixed one. A relayout's cause is engine words, a mutation kind
//! and a standard HTML tag name, never an element's id or a custom element
//! name. [`InspectorConfig::keep_console_text`]
//! and [`InspectorConfig::keep_error_text`] keep the page's text: never for a
//! log that will be shared.
//!
//! The buffer is behind a mutex shared with the embedder ([`Engine::inspector`]
//! hands out an `Arc`), so another thread can take a snapshot while the page's
//! script is running. Reading runs no script.
//!
//! An open devtools panel is not polled for (T6b): every record moves the
//! view's [`Inspector::watermark`], and while a panel watches the view the
//! devtools thread is signalled after the lock is released. Each record
//! carries its document ([`InspectorRecord::doc`]), and records outlive a
//! navigation and the panel closing until the ring drops them.
//!
//! [`Engine::inspector`]: crate::Engine::inspector

use std::collections::VecDeque;
use std::io::Write;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use url::Url;

/// Events a view's ring buffer holds by default.
pub const DEFAULT_CAPACITY: usize = 4096;
/// Events each tab keeps from the moment it is made, so an Inspector panel
/// opened late has the history (T6b; Atlas 10-24, Q1).
/// 2048: one yahoo load writes about 562 records with T3, so 512 lost its
/// `navigation_start` (Atlas 10-26).
pub const TAB_RING_CAPACITY: usize = 2048;
/// Longest console message kept, in characters.
pub const MAX_MESSAGE_CHARS: usize = 1024;
/// Longest error stack kept, in characters.
pub const MAX_STACK_CHARS: usize = 4096;
/// Longest short text (an error name, a reason, an interface or member name).
pub const MAX_SHORT_CHARS: usize = 256;
/// Most distinct unsupported members recorded per page.
pub const MAX_UNSUPPORTED_MEMBERS: usize = 512;
/// Most script-inserted scripts whose inserter is remembered per page.
pub const MAX_INSERTERS: usize = 1024;

/// How a view records (part of `EngineConfig`).
#[derive(Debug, Clone)]
pub struct InspectorConfig {
    /// Keep the last `capacity` events in the view's ring buffer.
    pub buffer: bool,
    /// Size of the ring buffer; past it the oldest event is dropped and
    /// counted.
    pub capacity: usize,
    /// Keep the query string, fragment and `;` path parameters of URLs in
    /// events. Off by default: they carry session ids and tokens.
    pub keep_url_query: bool,
    /// Keep the text of console messages. Off by default: a console line
    /// is then each argument's type, and a string's length (`<string
    /// len=12>`), since pages log cookies, form values and their own URL.
    /// When on, URLs in the text are still scrubbed as event URLs are, but
    /// cookies, form values and anything else the page logs are kept: never
    /// for a log that will be shared.
    pub keep_console_text: bool,
    /// Keep the message text of errors the page throws or rejects with.
    /// Off by default: an error is then its type, a standard name, its
    /// message's length and its frames' positions. When on, the message is
    /// kept verbatim (capped) with whatever the page put in it: never for a
    /// log that will be shared.
    pub keep_error_text: bool,
}

impl Default for InspectorConfig {
    fn default() -> Self {
        Self {
            buffer: false,
            capacity: DEFAULT_CAPACITY,
            keep_url_query: false,
            keep_console_text: false,
            keep_error_text: false,
        }
    }
}

impl InspectorConfig {
    /// A tab's: a [`TAB_RING_CAPACITY`] ring once a panel attaches.
    /// `HIWAVE_DEVTOOLS_RING=1` (or true/on/yes) keeps it from the moment
    /// the view is made, so a panel opened late has the history. Off by
    /// default: it costs every view a record per console call, script and
    /// request, and no quiet A/B has cleared that cost yet (#803 review).
    pub fn tab() -> Self {
        let on = matches!(
            std::env::var("HIWAVE_DEVTOOLS_RING").ok().as_deref(),
            Some("1" | "true" | "on" | "yes")
        );
        Self {
            buffer: on,
            capacity: TAB_RING_CAPACITY,
            ..Self::default()
        }
    }
}

/// A console method's level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

/// A place in a script: its URL (or `inline#n` for the page's n-th
/// `<script>` when inline; the line is then counted from the start of the
/// script's text), and a 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLocation {
    pub url: String,
    pub line: u32,
    pub column: u32,
}

/// Why a navigation stopped before its load finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    /// `Engine::stop`, or another navigation started in the view.
    NavigatedAway,
    /// The document could not be fetched or was refused.
    Failed,
}

/// What a request fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    Document,
    Stylesheet,
    Script,
    Module,
    Image,
    Font,
    Fetch,
    Xhr,
    Other,
}

impl ResourceType {
    pub fn as_str(self) -> &'static str {
        match self {
            ResourceType::Document => "document",
            ResourceType::Stylesheet => "stylesheet",
            ResourceType::Script => "script",
            ResourceType::Module => "module",
            ResourceType::Image => "image",
            ResourceType::Font => "font",
            ResourceType::Fetch => "fetch",
            ResourceType::Xhr => "xhr",
            ResourceType::Other => "other",
        }
    }
}

/// Where a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureStage {
    /// The host name did not resolve.
    Dns,
    /// No connection (refused, reset, unreachable).
    Connect,
    Tls,
    /// The network did not answer in time.
    Timeout,
    /// The server's answer was not usable HTTP (malformed, too many
    /// redirects). An answer with an error status is a `response`.
    HttpError,
    /// Refused by a rule before or while fetching: the shield, the
    /// private-network rule, mixed content, CSP, a request cap.
    Policy,
    /// A cross-origin script request the server did not allow.
    Cors,
    /// The response's MIME type is not one this kind of resource may have.
    Mime,
    /// The body arrived but could not be read as what it should be (text,
    /// an image).
    Decode,
    /// Abandoned: the page navigated away, the script aborted it, or the
    /// engine stopped waiting for a reason of its own.
    Cancelled,
    /// The page's script budget or a subresource phase's budget ran out
    /// before it finished.
    OverBudget,
}

impl FailureStage {
    pub fn as_str(self) -> &'static str {
        match self {
            FailureStage::Dns => "dns",
            FailureStage::Connect => "connect",
            FailureStage::Tls => "tls",
            FailureStage::Timeout => "timeout",
            FailureStage::HttpError => "http_error",
            FailureStage::Policy => "policy",
            FailureStage::Cors => "cors",
            FailureStage::Mime => "mime",
            FailureStage::Decode => "decode",
            FailureStage::Cancelled => "cancelled",
            FailureStage::OverBudget => "over_budget",
        }
    }
}

/// One thing that happened. Serialized with a `type` tag and the fields in
/// declaration order; new fields are only ever appended, and a field that
/// is `null` today may hold a value later.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InspectorEvent {
    /// A navigation began (`load_url`, or `load_html` with no URL). The
    /// event times that follow count from here.
    NavigationStart { url: Option<String> },
    /// The document's response arrived and replaced the previous document.
    NavigationCommit { url: Option<String>, status: Option<u16> },
    /// The load finished: scripts, lifecycle events and the timers up to
    /// the horizon ran. `scripts_over_budget` counts the scripts not run
    /// (or stopped) because the page's script budget was spent.
    LoadFinished { url: Option<String>, scripts_over_budget: u32 },
    /// The load ended without finishing.
    LoadEndedEarly { url: Option<String>, reason: EndReason, detail: String },
    /// A `console.*` call.
    Console {
        level: ConsoleLevel,
        message: String,
        /// The message was longer than [`MAX_MESSAGE_CHARS`] and was cut.
        truncated: bool,
        location: Option<SourceLocation>,
        /// `console.error` only: the page's frames, innermost first, as
        /// [`ScriptError`](Self::ScriptError)'s `stack` writes them.
        #[serde(default)]
        stack: Option<String>,
        /// `console.error` with an error among its arguments: the first
        /// one's own frames (where it was made), written as `stack` is.
        #[serde(default)]
        error_stack: Option<String>,
    },
    /// An exception no script caught, or a promise rejected with no handler.
    ScriptError {
        /// A standard error name (`TypeError`, `NotFoundError`, ...; any
        /// other name the page gave is `Error`); empty when a non-error was
        /// thrown.
        name: String,
        /// The message, only when it is one of the engine's own fixed
        /// messages (`not a callable function`, `… is not defined`) or
        /// [`InspectorConfig::keep_error_text`] is on; else `null`.
        message: Option<String>,
        /// Where it was thrown: the innermost of the page's frames.
        location: Option<SourceLocation>,
        /// The page's frames, innermost first, one `    at url:line:col`
        /// line each; with [`InspectorConfig::keep_error_text`] a named
        /// function's frame is `    at name (url:line:col)`.
        stack: Option<String>,
        unhandled_rejection: bool,
        /// Which of the page's scripts it came from: a script URL,
        /// `inline#n`, or the engine step that ran it (`event:load`,
        /// `timers`, `network`, ...).
        script: String,
        /// What was thrown: `error`, `string`, `number`, `bigint`,
        /// `boolean`, `symbol`, `null`, `undefined`, `object`, `array`,
        /// `function`, or `engine` for the engine's own failure.
        value: String,
        /// The message's length in characters, when it has one (an error's
        /// message, a thrown string).
        message_len: Option<u64>,
        /// When the innermost frame's script was inserted by page script
        /// (`appendChild` of a `<script src>`): where that insertion was.
        #[serde(default)]
        inserted_by: Option<SourceLocation>,
    },
    /// Page script used a Web API member the engine does not implement.
    /// Recorded once per page per member; `Inspector::unsupported_apis`
    /// and the summary carry the final counts.
    UnsupportedApi {
        interface: String,
        member: String,
        location: Option<SourceLocation>,
    },
    /// The view's first layout of this document.
    FirstLayout,
    /// The view's first frame of this document.
    FirstPaint,
    /// A relayout and the cause it was laid out for: the `relayout cause=`
    /// log line's value without page text (a script write's kind and, for
    /// a listed HTML element, its tag; never an id).
    Relayout { cause: String, took_ms: f64 },
    /// The document's request was redirected (one per hop, in order,
    /// between its `request` and its `response`).
    NavigationRedirect { from: String, to: String, status: u16 },
    /// The engine made a request. Exactly one `response` or `failure` with
    /// the same `id` follows.
    Request {
        /// Per view, from 1.
        id: u64,
        url: String,
        method: String,
        resource_type: ResourceType,
        /// What asked for it: `parser` (the document's markup), `css` (a
        /// stylesheet's `url()`), `fetch`, `xhr`, `inserted_script` (a
        /// `<script src>` page script inserted), `script` (other page
        /// script: `import`, images it added), `navigation`.
        initiator: Option<String>,
    },
    /// The request's answer arrived (any status, 4xx and 5xx included).
    Response {
        id: u64,
        status: u16,
        /// The MIME type's essence (`text/css`), parameters dropped.
        mime: Option<String>,
        /// Body bytes as transferred, before content decoding; `null` when
        /// none were (cache, unknown).
        encoded_size: Option<u64>,
        /// Body bytes after decoding, when the engine read the body.
        decoded_size: Option<u64>,
        from_cache: bool,
        duration_ms: f64,
        /// Of `duration_ms`, the time it waited for an in-flight slot (the
        /// per-origin and per-page caps queue a script request; they do not
        /// refuse it). `null` for a request no cap queues.
        #[serde(default)]
        queued_ms: Option<f64>,
    },
    /// The request ended without a usable answer.
    Failure {
        id: u64,
        stage: FailureStage,
        /// A short reason in the engine's words (no header, body or query).
        reason: String,
        duration_ms: f64,
        /// The shield's list and rule, when the shield refused it and said
        /// which; else `null`.
        #[serde(default)]
        blocked_by: Option<BlockedBy>,
        /// As on `response`. A deadline that passes while the request is
        /// still queued has the reason `in-flight cap`.
        #[serde(default)]
        queued_ms: Option<f64>,
    },
    /// One page script's life, recorded when it is over: run, thrown,
    /// stopped, or given up on.
    Script {
        /// The script's URL (as `request` shows it), `inline#n`, or
        /// `unparseable#n` for a `src` that is not a URL.
        script: String,
        /// `parser` (a parser-blocking classic `<script>`), `defer`,
        /// `async`, `module`, or `inserted` (a `<script src>` page script
        /// added).
        kind: String,
        /// `ran`, `threw`, `skipped`, `fetch_failed`, `over_budget` (not
        /// started: the script budget was spent), `stopped` (started, and
        /// stopped by the script budget), `engine_failed`, `cancelled` (its
        /// document was left first).
        outcome: String,
        /// Why, in the engine's words, for every outcome but `ran` and
        /// `threw`.
        reason: Option<String>,
        /// Source bytes (0 when nothing arrived).
        bytes: u64,
        /// `inserted` only: from the page inserting it to the engine taking
        /// it from the insertion queue (its request starts then).
        queued_ms: Option<f64>,
        /// From its request starting to its body (or failure) arriving;
        /// `null` for an inline script.
        fetch_ms: Option<f64>,
        /// From its body arriving (or its fetch failing) to the engine
        /// running it (or giving up on it).
        waited_ms: Option<f64>,
        /// Running it: parse, compile and run, its microtasks included.
        ran_ms: f64,
        /// The event its element heard, or will hear when it is owed to a
        /// later turn past a spent budget: `load` or `error`.
        event: Option<String>,
        /// The first lifecycle event that had not fired when it ended:
        /// `DOMContentLoaded`, `load`, or `null` (after `load`, or
        /// cancelled). When it ended, not that the event waited for it.
        held: Option<String>,
    },
    /// The document's `DOMContentLoaded` or `window` `load` is firing.
    Lifecycle {
        event: String,
        /// `load` only: the scripts it waited for after `DOMContentLoaded`
        /// (first 16, in the order they ended).
        held_by: Vec<String>,
        /// How many there were.
        held_by_count: u32,
    },
}

/// Which filter list, and which of its rules, refused a request: the
/// list's own public text, capped, never the page's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockedBy {
    pub list: String,
    pub rule: Option<String>,
}

impl BlockedBy {
    fn from_reason(reason: &rustkit_net::BlockReason) -> Self {
        BlockedBy {
            list: cap(&reason.list, MAX_SHORT_CHARS).0,
            rule: reason.rule.as_deref().map(|r| cap(r, MAX_RULE_CHARS).0),
        }
    }
}

/// A failure's reason when its deadline passed while it was still queued
/// behind an in-flight cap.
pub const IN_FLIGHT_CAP: &str = "in-flight cap";

/// Longest filter rule kept in `blocked_by`, in characters.
const MAX_RULE_CHARS: usize = 256;

impl InspectorEvent {
    /// The `type` tag.
    pub fn kind(&self) -> &'static str {
        match self {
            InspectorEvent::NavigationStart { .. } => "navigation_start",
            InspectorEvent::NavigationCommit { .. } => "navigation_commit",
            InspectorEvent::LoadFinished { .. } => "load_finished",
            InspectorEvent::LoadEndedEarly { .. } => "load_ended_early",
            InspectorEvent::Console { .. } => "console",
            InspectorEvent::ScriptError { .. } => "script_error",
            InspectorEvent::UnsupportedApi { .. } => "unsupported_api",
            InspectorEvent::FirstLayout => "first_layout",
            InspectorEvent::FirstPaint => "first_paint",
            InspectorEvent::Relayout { .. } => "relayout",
            InspectorEvent::NavigationRedirect { .. } => "navigation_redirect",
            InspectorEvent::Request { .. } => "request",
            InspectorEvent::Response { .. } => "response",
            InspectorEvent::Failure { .. } => "failure",
            InspectorEvent::Script { .. } => "script",
            InspectorEvent::Lifecycle { .. } => "lifecycle",
        }
    }
}

/// An event with its place in the view's sequence and its time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InspectorRecord {
    /// 0-based, per view, never reused.
    pub seq: u64,
    /// The document it belongs to: 0 before the view's first navigation,
    /// then each navigation's start begins the next.
    #[serde(default)]
    pub doc: u32,
    /// Milliseconds since the current navigation started (monotonic clock).
    pub t_ms: f64,
    #[serde(flatten)]
    pub event: InspectorEvent,
}

/// What [`Inspector::snapshot_since`] returns.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InspectorSnapshot {
    /// The buffered records with `seq >= since`, oldest first.
    pub records: Vec<InspectorRecord>,
    /// The `seq` the next record will get: pass it as `since` next time.
    pub next_seq: u64,
    /// Records the ring buffer has dropped since the view was made.
    pub dropped: u64,
}

/// Somewhere else records go, besides the ring buffer (a file, a socket).
/// Called in order, under the view's inspector lock: keep it quick.
pub trait InspectorSink: Send {
    fn record(&mut self, record: &InspectorRecord);
    fn flush(&mut self) {}
}

/// One unsupported member, with how often page script reached it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsupportedApiCount {
    pub interface: String,
    pub member: String,
    pub count: u64,
    pub location: Option<SourceLocation>,
}

struct State {
    buffer: bool,
    /// Whether the view's config keeps the ring on with no panel.
    buffer_default: bool,
    capacity: usize,
    /// [`InspectorRecord::doc`] of the next record.
    doc: u32,
    records: VecDeque<InspectorRecord>,
    next_seq: u64,
    dropped: u64,
    navigation_started: Instant,
    sinks: Vec<Box<dyn InspectorSink>>,
    unsupported: Vec<UnsupportedApiCount>,
    laid_out: bool,
    painted: bool,
    /// The names page scripts are evaluated under that are not their URL
    /// (`script#3`), and the URL each stands for (as events show it).
    aliases: Vec<(String, String)>,
    /// Script-inserted scripts' evaluation names (`inserted#n`) and where
    /// the page's script that inserted each was.
    inserters: Vec<(String, SourceLocation)>,
    /// The last request id given out (per view, never reused).
    last_request_id: u64,
    /// Requests recorded and not yet answered.
    /// Open request ids and the document each started in: the answer is
    /// stamped with that one, not the document current when it arrives
    /// (T6 R1 condition 6).
    open_requests: std::collections::HashMap<u64, u32>,
    /// The lifecycle events this document has fired, and the scripts that
    /// ended between `DOMContentLoaded` and `load` (first
    /// [`MAX_HELD_BY`]) with their count.
    dom_content_loaded: bool,
    loaded: bool,
    held_load: Vec<String>,
    held_load_count: u32,
}

/// How many of the scripts `load` waited for its `lifecycle` record names.
const MAX_HELD_BY: usize = 16;

/// What [`Inspector::script_ended`] records about one page script.
pub struct ScriptEnd<'a> {
    /// Its log label: URL or `inline#n`.
    pub source: &'a str,
    /// `parser`, `defer`, `async`, `module` or `inserted`.
    pub kind: &'static str,
    pub outcome: &'a crate::ScriptOutcome,
    /// It began running (an over-budget outcome is then `stopped`).
    pub started: bool,
    pub bytes: usize,
    pub queued: Option<std::time::Duration>,
    pub fetch: Option<std::time::Duration>,
    pub waited: Option<std::time::Duration>,
    pub ran_ms: u64,
    pub event: Option<&'static str>,
}

/// A view's recorder and ring buffer. See the module docs.
pub struct Inspector {
    active: AtomicBool,
    keep_url_query: bool,
    keep_console_text: bool,
    keep_error_text: bool,
    /// The `seq` the next record gets, readable without the lock.
    watermark: AtomicU64,
    /// A panel watches this view: tell the devtools thread about records.
    watched: AtomicBool,
    /// Console calls the page's runtime holds for its next flush, kept by
    /// the runtime (`set_console_held_counter`) and read without the page.
    console_held: Arc<std::sync::atomic::AtomicUsize>,
    state: Mutex<State>,
}

/// The state, locked. Dropping it releases the lock first, then tells the
/// devtools thread when records were pushed under it and a panel watches.
struct Locked<'a> {
    inspector: &'a Inspector,
    guard: Option<MutexGuard<'a, State>>,
    seq_at_lock: u64,
}

impl Deref for Locked<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        self.guard.as_ref().expect("held until drop")
    }
}

impl DerefMut for Locked<'_> {
    fn deref_mut(&mut self) -> &mut State {
        self.guard.as_mut().expect("held until drop")
    }
}

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        let Some(guard) = self.guard.take() else { return };
        let next_seq = guard.next_seq;
        drop(guard);
        if next_seq != self.seq_at_lock {
            // SeqCst with `set_watched`: a push that reads `watched` false
            // has its watermark seen by the devtools thread's re-read
            // before it sleeps (T6 R1 condition 1).
            self.inspector.watermark.store(next_seq, Ordering::SeqCst);
            if self.inspector.watched.load(Ordering::SeqCst) {
                crate::devtools_service::records_arrived();
            }
        }
    }
}

impl Inspector {
    pub fn new(config: &InspectorConfig) -> Arc<Self> {
        Arc::new(Self {
            active: AtomicBool::new(config.buffer),
            keep_url_query: config.keep_url_query,
            keep_console_text: config.keep_console_text,
            keep_error_text: config.keep_error_text,
            watermark: AtomicU64::new(0),
            watched: AtomicBool::new(false),
            console_held: Arc::default(),
            state: Mutex::new(State {
                buffer: config.buffer,
                buffer_default: config.buffer,
                capacity: config.capacity.max(1),
                doc: 0,
                records: VecDeque::new(),
                next_seq: 0,
                dropped: 0,
                navigation_started: Instant::now(),
                sinks: Vec::new(),
                unsupported: Vec::new(),
                laid_out: false,
                painted: false,
                aliases: Vec::new(),
                inserters: Vec::new(),
                last_request_id: 0,
                open_requests: std::collections::HashMap::new(),
                dom_content_loaded: false,
                loaded: false,
                held_load: Vec::new(),
                held_load_count: 0,
            }),
        })
    }

    /// Whether anything listens. Every recording path checks this first.
    #[inline]
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    fn state(&self) -> Locked<'_> {
        // A sink that panicked leaves the lock poisoned; the buffer is
        // still consistent (records are pushed whole).
        let guard = self.state.lock().unwrap_or_else(|e| e.into_inner());
        Locked {
            inspector: self,
            seq_at_lock: guard.next_seq,
            guard: Some(guard),
        }
    }

    /// The `seq` the next record will get. One atomic load: the window
    /// thread reads it to know whether there is anything new, without the
    /// lock (sinks run under it).
    pub fn watermark(&self) -> u64 {
        self.watermark.load(Ordering::SeqCst)
    }

    /// Whether the devtools thread is told about this view's records.
    pub(crate) fn set_watched(&self, on: bool) {
        self.watched.store(on, Ordering::SeqCst);
    }

    /// The page thread panicked while it held this view's lock (a sink or
    /// a push unwound). What was recorded is still read (`state` recovers
    /// with `into_inner`); the panel says the page crashed (T6 R1
    /// condition 2).
    pub fn page_crashed(&self) -> bool {
        self.state.is_poisoned()
    }

    /// Console calls made but not yet recorded: past the live rate cap, a
    /// running script's calls wait for its end, and a script that then
    /// hangs never hands them over (T6d review). One atomic load, so a
    /// panel can say "K console lines held by a running script" while the
    /// page is blocked.
    pub fn console_held(&self) -> usize {
        self.console_held.load(Ordering::Relaxed)
    }

    /// The counter the page's runtime keeps [`console_held`](Self::console_held) in.
    pub(crate) fn console_held_counter(&self) -> Arc<std::sync::atomic::AtomicUsize> {
        self.console_held.clone()
    }

    /// Test hook: panic on this thread while holding the lock, as a page
    /// thread whose sink panicked would.
    #[cfg(test)]
    pub(crate) fn panic_holding_lock(&self) {
        let _held = self.state.lock();
        panic!("page thread panicked holding the Inspector lock (test)");
    }

    /// The panel closed: recording goes back to the view's default. With
    /// the tab ring on nothing recorded is dropped (T6b: the panel reopens
    /// with history); with it off the session's records are, as nothing
    /// will read them.
    pub fn detach_panel(&self) {
        let mut state = self.state();
        state.buffer = state.buffer_default;
        if !state.buffer {
            state.records.clear();
        }
        self.refresh_active(&state);
    }

    fn refresh_active(&self, state: &State) {
        self.active
            .store(state.buffer || !state.sinks.is_empty(), Ordering::Relaxed);
    }

    /// Turn the ring buffer on or off. Turning it off empties it.
    pub fn set_buffer_enabled(&self, on: bool) {
        let mut state = self.state();
        state.buffer = on;
        if !on {
            state.records.clear();
        }
        self.refresh_active(&state);
    }

    /// Send every record from now on to `sink` as well.
    pub fn add_sink(&self, sink: Box<dyn InspectorSink>) {
        let mut state = self.state();
        state.sinks.push(sink);
        self.refresh_active(&state);
    }

    /// Flush and drop every sink.
    pub fn clear_sinks(&self) {
        let mut state = self.state();
        for sink in &mut state.sinks {
            sink.flush();
        }
        state.sinks.clear();
        self.refresh_active(&state);
    }

    /// Flush every sink.
    pub fn flush(&self) {
        for sink in &mut self.state().sinks {
            sink.flush();
        }
    }

    /// The buffered records from `since` on. Takes the lock briefly; runs
    /// no page script, so it can be called from any thread at any time.
    pub fn snapshot_since(&self, since: u64) -> InspectorSnapshot {
        let state = self.state();
        InspectorSnapshot {
            records: state.records.iter().filter(|r| r.seq >= since).cloned().collect(),
            next_seq: state.next_seq,
            dropped: state.dropped,
        }
    }

    /// The buffered records from `since` on, and the document the next
    /// record belongs to (the current one), read under one lock (T7 C5).
    pub fn snapshot_and_doc(&self, since: u64) -> (InspectorSnapshot, u32) {
        let state = self.state();
        let snapshot = InspectorSnapshot {
            records: state.records.iter().filter(|r| r.seq >= since).cloned().collect(),
            next_seq: state.next_seq,
            dropped: state.dropped,
        };
        (snapshot, state.doc)
    }

    /// The current document's network records (request, response, failure)
    /// and that document, read under one lock. Clones only those, not the
    /// whole ring (up to 2048 records, mostly console) as
    /// [`snapshot_and_doc`](Self::snapshot_and_doc) does (T8).
    pub fn network_for_current_doc(&self) -> (InspectorSnapshot, u32) {
        let state = self.state();
        let doc = state.doc;
        let snapshot = InspectorSnapshot {
            records: state
                .records
                .iter()
                .filter(|r| {
                    r.doc == doc
                        && matches!(
                            r.event,
                            InspectorEvent::Request { .. } | InspectorEvent::Response { .. } | InspectorEvent::Failure { .. }
                        )
                })
                .cloned()
                .collect(),
            next_seq: state.next_seq,
            dropped: state.dropped,
        };
        (snapshot, doc)
    }

    /// The current page's unsupported members, in the order first seen.
    pub fn unsupported_apis(&self) -> Vec<UnsupportedApiCount> {
        self.state().unsupported.clone()
    }

    /// Record `event` if anything listens. Prefer [`emit_with`](Self::emit_with)
    /// where building the event costs anything.
    pub fn emit(&self, event: InspectorEvent) {
        if self.is_active() {
            self.push(event);
        }
    }

    /// Record the event `build` makes, calling it only if anything listens.
    #[inline]
    pub fn emit_with(&self, build: impl FnOnce(&Self) -> InspectorEvent) {
        if self.is_active() {
            let event = build(self);
            self.push(event);
        }
    }

    fn push(&self, event: InspectorEvent) {
        let mut state = self.state();
        Self::push_locked(&mut state, event);
    }

    fn push_locked(state: &mut State, event: InspectorEvent) {
        let doc = state.doc;
        Self::push_locked_in(state, doc, event);
    }

    /// Push `event` as a record of document `doc`.
    fn push_locked_in(state: &mut State, doc: u32, event: InspectorEvent) {
        let record = InspectorRecord {
            seq: state.next_seq,
            doc,
            t_ms: round_ms(state.navigation_started.elapsed().as_secs_f64() * 1000.0),
            event,
        };
        state.next_seq += 1;
        for sink in &mut state.sinks {
            sink.record(&record);
        }
        if state.buffer {
            if state.records.len() >= state.capacity {
                state.records.pop_front();
                state.dropped += 1;
            }
            state.records.push_back(record);
        }
    }

    /// A navigation began: restart the clock and the per-page tables, then
    /// record its start.
    pub fn navigation_started(&self, url: Option<&Url>) {
        if !self.is_active() {
            // The lifecycle flags are kept unlistened (see `lifecycle`), and
            // the scripts the last document's `load` waited for go with it.
            let mut state = self.state();
            state.dom_content_loaded = false;
            state.loaded = false;
            state.held_load.clear();
            state.held_load_count = 0;
            return;
        }
        let url = url.map(|u| self.url(u));
        let mut state = self.state();
        state.navigation_started = Instant::now();
        state.doc += 1;
        state.unsupported.clear();
        state.laid_out = false;
        state.painted = false;
        state.aliases.clear();
        state.inserters.clear();
        state.dom_content_loaded = false;
        state.loaded = false;
        state.held_load.clear();
        state.held_load_count = 0;
        Self::push_locked(&mut state, InspectorEvent::NavigationStart { url });
    }

    /// The document is firing `DOMContentLoaded` or `load` (`event`).
    /// From here on a script that ends is not that event's to wait for.
    pub fn lifecycle(&self, event: &'static str) {
        // Kept with nobody listening too (one lock, twice a document): a
        // panel attached after `DOMContentLoaded` must not read the scripts
        // that end later as ending before it.
        let mut state = self.state();
        if !self.is_active() {
            if event == "load" {
                state.loaded = true;
            } else {
                state.dom_content_loaded = true;
            }
            return;
        }
        let (held_by, held_by_count) = if event == "load" {
            state.loaded = true;
            (std::mem::take(&mut state.held_load), std::mem::take(&mut state.held_load_count))
        } else {
            state.dom_content_loaded = true;
            (Vec::new(), 0)
        };
        Self::push_locked(&mut state, InspectorEvent::Lifecycle { event: event.to_string(), held_by, held_by_count });
    }

    /// A page script's life is over (see [`InspectorEvent::Script`]).
    pub fn script_ended(&self, end: ScriptEnd<'_>) {
        if !self.is_active() {
            return;
        }
        use crate::{FetchFailure, ScriptOutcome};
        let (outcome, reason): (&str, Option<String>) = match end.outcome {
            ScriptOutcome::Ran => ("ran", None),
            ScriptOutcome::Threw(_) => ("threw", None),
            ScriptOutcome::Skipped(why) => ("skipped", Some((*why).to_string())),
            ScriptOutcome::Nomodule => ("skipped", Some("nomodule".to_string())),
            ScriptOutcome::FetchFailed(FetchFailure::Status(status)) => {
                ("fetch_failed", Some(format!("HTTP {status}")))
            }
            // The transport error's own text can name hosts and paths; its
            // request's `failure` record has the stage.
            ScriptOutcome::FetchFailed(FetchFailure::Transport(_)) => ("fetch_failed", Some("network".to_string())),
            ScriptOutcome::OverBudget if end.started => ("stopped", Some("script budget".to_string())),
            ScriptOutcome::OverBudget => {
                ("over_budget", Some("script budget spent before it started".to_string()))
            }
            ScriptOutcome::EngineFailed(crate::EngineFailure::Panic) => {
                ("engine_failed", Some("JS engine panic".to_string()))
            }
            ScriptOutcome::EngineFailed(crate::EngineFailure::ImportMap(_)) => {
                ("engine_failed", Some("import map rejected".to_string()))
            }
        };
        let ms = |d: Option<std::time::Duration>| d.map(|d| round_ms(d.as_secs_f64() * 1000.0));
        let script = self.url_text(end.source);
        let mut state = self.state();
        let held = if !state.dom_content_loaded {
            Some("DOMContentLoaded")
        } else if !state.loaded {
            state.held_load_count = state.held_load_count.saturating_add(1);
            if state.held_load.len() < MAX_HELD_BY {
                state.held_load.push(script.clone());
            }
            Some("load")
        } else {
            None
        };
        Self::push_locked(
            &mut state,
            InspectorEvent::Script {
                script,
                kind: end.kind.to_string(),
                outcome: outcome.to_string(),
                reason,
                bytes: end.bytes as u64,
                queued_ms: ms(end.queued),
                fetch_ms: ms(end.fetch),
                waited_ms: ms(end.waited),
                ran_ms: end.ran_ms as f64,
                event: end.event.map(str::to_string),
                held: held.map(str::to_string),
            },
        );
    }

    /// A page script given up on because its document was left (a
    /// navigation, a cancelled load, a closed view) before it ended: its
    /// one `script` record, `cancelled`. No lifecycle event of a document
    /// still showing waits for it, so `held` is `null`.
    pub fn script_cancelled(&self, source: &str, kind: &'static str, queued: Option<std::time::Duration>) {
        if !self.is_active() {
            return;
        }
        let script = self.url_text(source);
        let mut state = self.state();
        Self::push_locked(
            &mut state,
            InspectorEvent::Script {
                script,
                kind: kind.to_string(),
                outcome: "cancelled".to_string(),
                reason: Some("navigation or view closed".to_string()),
                bytes: 0,
                queued_ms: queued.map(|d| round_ms(d.as_secs_f64() * 1000.0)),
                fetch_ms: None,
                waited_ms: None,
                ran_ms: 0.0,
                event: None,
                held: None,
            },
        );
    }

    /// The view laid this document out: `FirstLayout` the first time.
    pub fn laid_out(&self) {
        if !self.is_active() {
            return;
        }
        let mut state = self.state();
        if !state.laid_out {
            state.laid_out = true;
            Self::push_locked(&mut state, InspectorEvent::FirstLayout);
        }
    }

    /// A relayout finished, laid out for `cause` (recorded as
    /// [`RelayoutCause::record_label`] shows it: no page text).
    ///
    /// [`RelayoutCause::record_label`]: crate::RelayoutCause::record_label
    pub fn relayout(&self, cause: Option<&crate::RelayoutCause>, took_ms: f64) {
        self.emit_with(|_| InspectorEvent::Relayout {
            cause: cause.map(crate::RelayoutCause::record_label).unwrap_or_default(),
            took_ms: round_ms(took_ms),
        });
    }

    /// The view presented a frame of this document: `FirstPaint` once.
    pub fn painted(&self) {
        if !self.is_active() {
            return;
        }
        let mut state = self.state();
        if state.laid_out && !state.painted {
            state.painted = true;
            Self::push_locked(&mut state, InspectorEvent::FirstPaint);
        }
    }

    /// Page script reached `interface.member`, which the engine does not
    /// implement, `count` more times. The first sighting on a page is an
    /// event; later ones only count.
    pub fn unsupported_api(&self, interface: &str, member: &str, count: u64, location: Option<SourceLocation>) {
        if !self.is_active() {
            return;
        }
        let mut state = self.state();
        if let Some(seen) = state
            .unsupported
            .iter_mut()
            .find(|u| u.interface == interface && u.member == member)
        {
            seen.count += count;
            return;
        }
        if state.unsupported.len() >= MAX_UNSUPPORTED_MEMBERS {
            return;
        }
        let interface = cap(interface, MAX_SHORT_CHARS).0;
        let member = cap(member, MAX_SHORT_CHARS).0;
        state.unsupported.push(UnsupportedApiCount {
            interface: interface.clone(),
            member: member.clone(),
            count,
            location: location.clone(),
        });
        Self::push_locked(&mut state, InspectorEvent::UnsupportedApi { interface, member, location });
    }

    /// Page script `url` is evaluated under the name `alias`: positions in
    /// it are shown with its URL.
    pub fn alias_script(&self, alias: &str, url: &str) {
        if !self.is_active() {
            return;
        }
        let url = self.url_text(url);
        self.state().aliases.push((alias.to_string(), url));
    }

    /// A position as events show it: the script's URL (an alias resolved,
    /// a URL scrubbed) or its `inline#n` label.
    pub fn location(&self, path: &str, line: u32, column: u32) -> SourceLocation {
        SourceLocation { url: self.url_text(path), line, column }
    }

    /// The script to be evaluated as `name` (`inserted#n`) was inserted by
    /// the page's script at `path:line:column` (as the JS runtime placed the
    /// insertion call). A position that cannot be shown is not kept.
    pub fn note_inserted(&self, name: &str, path: &str, line: u32, column: u32) {
        if !self.is_active() {
            return;
        }
        let Some(at) = self.frame_location(path, line, column) else { return };
        let mut state = self.state();
        if state.inserters.len() < MAX_INSERTERS {
            state.inserters.push((name.to_string(), at));
        }
    }

    /// Where the script evaluated as `name` was inserted by page script.
    pub fn inserted_by(&self, name: &str) -> Option<SourceLocation> {
        self.state().inserters.iter().find(|(n, _)| n == name).map(|(_, at)| at.clone())
    }

    /// What the engine named `name` with [`alias_script`](Self::alias_script).
    fn alias(&self, name: &str) -> Option<String> {
        self.state().aliases.iter().find(|(alias, _)| alias == name).map(|(_, url)| url.clone())
    }

    /// Engine-made text that can name scripts (an error message, a stack):
    /// each `(path:line:col)` position is shown as [`location`](Self::location)
    /// shows it, and every `http(s)://`, `ws(s)://` or `file://` URL anywhere
    /// in it (after a quote, a bracket, an `=`) is scrubbed as event URLs are.
    pub fn scrub_text(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                out.push('\n');
            }
            // A position's path holds no space, so the last ` (` opens it
            // even when the path's query holds a `(`.
            let rewritten = line.rfind(" (").and_then(|space| {
                let open = space + 1;
                let inner = line[open + 1..].strip_suffix(')')?;
                let (path, l, c) = parse_position(inner)?;
                let at = self.location(&path, l, c);
                Some(format!("{}({}:{}:{})", self.scrub_urls(&line[..open]), at.url, at.line, at.column))
            });
            match rewritten {
                Some(line) => out.push_str(&line),
                None => out.push_str(&self.scrub_urls(line)),
            }
        }
        out
    }

    /// Record a request the engine is about to make. The returned guard
    /// records its one `response` or `failure`; dropped unanswered (the
    /// fetch's future was dropped: a deadline, a navigation) it records a
    /// failure at the stage set with [`RequestGuard::dropped_as`],
    /// `cancelled` by default. `None`, and nothing built, when nobody
    /// listens.
    pub fn begin_request(
        self: &Arc<Self>,
        url: &Url,
        method: &str,
        resource_type: ResourceType,
        initiator: Option<&str>,
    ) -> Option<RequestGuard> {
        if !self.is_active() {
            return None;
        }
        let url = self.url(url);
        // The page chooses the method: a standard one, else `OTHER`.
        let method = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"]
            .into_iter()
            .find(|m| m.eq_ignore_ascii_case(method))
            .unwrap_or("OTHER")
            .to_string();
        let initiator = initiator.map(|i| cap(i, MAX_SHORT_CHARS).0);
        let mut state = self.state();
        state.last_request_id += 1;
        let id = state.last_request_id;
        let doc = state.doc;
        state.open_requests.insert(id, doc);
        Self::push_locked(&mut state, InspectorEvent::Request { id, url, method, resource_type, initiator });
        drop(state);
        Some(RequestGuard {
            inspector: self.clone(),
            id,
            started: Instant::now(),
            dropped_as: FailureStage::Cancelled,
            dropped_reason: "abandoned (navigation or view closed)",
            budget: None,
            blocked_by: None,
            slot: None,
            deadline: None,
            done: false,
        })
    }

    /// Requests recorded and not yet answered.
    pub fn open_requests(&self) -> usize {
        self.state().open_requests.len()
    }

    fn finish_request(&self, id: u64, event: InspectorEvent) {
        let mut state = self.state();
        let started_in = state.open_requests.remove(&id);
        debug_assert!(started_in.is_some(), "request {id} answered twice, or never recorded");
        let doc = started_in.unwrap_or(state.doc);
        Self::push_locked_in(&mut state, doc, event);
    }

    /// The document's redirects, as `navigation_redirect` events.
    pub fn redirects(&self, chain: &[(Url, u16)], final_url: &Url) {
        if !self.is_active() || chain.is_empty() {
            return;
        }
        for (i, (from, status)) in chain.iter().enumerate() {
            let to = chain.get(i + 1).map(|(next, _)| next).unwrap_or(final_url);
            self.push(InspectorEvent::NavigationRedirect {
                from: self.url(from),
                to: self.url(to),
                status: *status,
            });
        }
    }

    /// Every URL in `text` scrubbed, wherever it starts. A URL's path runs
    /// to whitespace, a quote or a bracket; once its query or fragment
    /// starts, everything up to whitespace is the query (brackets, parens
    /// and quotes are legal in it), less a trailing `:line:col)` position
    /// or closing punctuation. A `data:` URL becomes `data:<mime>,…`, its
    /// payload (which may hold spaces) dropped to the end of the line.
    fn scrub_urls(&self, text: &str) -> String {
        const SCHEMES: [&str; 6] = ["https://", "http://", "wss://", "ws://", "file://", "data:"];
        let lower = text.to_ascii_lowercase();
        let mut out = String::with_capacity(text.len());
        let mut rest = 0;
        while rest < text.len() {
            let next = SCHEMES
                .iter()
                .filter_map(|scheme| find_scheme(&lower, rest, scheme))
                .min();
            let Some(start) = next else { break };
            out.push_str(&text[rest..start]);
            if lower[start..].starts_with("data:") {
                let tail = query_tail(&text[start..]);
                out.push_str(&format!("data:{},…", data_mime(&text[start + 5..])));
                out.push_str(tail);
                rest = text.len();
                break;
            }
            let stop = text[start..]
                .find(|c: char| c.is_whitespace() || "'\"`<>()[]{}?#".contains(c))
                .map(|i| start + i)
                .unwrap_or(text.len());
            // `(url, kept tail, where the text resumes)`.
            let (url_end, tail, end) = if text[stop..].starts_with(['?', '#']) {
                let end = text[stop..]
                    .find(char::is_whitespace)
                    .map(|i| stop + i)
                    .unwrap_or(text.len());
                let tail = query_tail(&text[stop..end]);
                let url_end = if self.keep_url_query { end - tail.len() } else { stop };
                (url_end, tail, end)
            } else {
                // Trailing sentence punctuation is not part of the URL.
                let mut end = stop;
                while end > start && text[..end].ends_with(['.', ',', ';', ':', '!']) {
                    end -= 1;
                }
                (end, "", end)
            };
            match Url::parse(&text[start..url_end]) {
                Ok(url) => out.push_str(&self.url(&url)),
                // Not a URL after all: keep the scheme, drop what follows.
                Err(_) => out.push_str(&text[start..start + text[start..].find("//").map_or(0, |i| i + 2)]),
            }
            out.push_str(tail);
            rest = end;
        }
        out.push_str(&text[rest..]);
        out
    }

    /// A URL as events show it (see the module docs).
    pub fn url(&self, url: &Url) -> String {
        scrub_url(url, self.keep_url_query)
    }

    /// [`url`](Self::url) for text that may not be a URL (a script label
    /// such as `inline#2`, which is kept as it is).
    pub fn url_text(&self, text: &str) -> String {
        // A name the engine gave a script (an inline module runs as
        // `<document>#inline-module-<n>`, aliased to `inline#n`).
        if let Some(url) = self.alias(text) {
            return url;
        }
        if text.starts_with("event:") {
            return cap(text, MAX_SHORT_CHARS).0;
        }
        match Url::parse(text) {
            // Any scheme, as the URL parser reads it.
            Ok(url) => self.url(&url),
            // `inline#2`, `timers`: names, not URLs.
            _ => cap(text, MAX_SHORT_CHARS).0,
        }
    }

    /// A page frame's script as events show it: an engine label
    /// (`inline#n`, `script#n` resolved to its URL), or a URL as
    /// [`url`](Self::url) shows it (the URL parser's reading: tabs and
    /// newlines dropped, backslashes as slashes, any scheme). `None` for
    /// anything else: a frame that cannot be read is dropped, never copied.
    pub fn frame_location(&self, path: &str, line: u32, column: u32) -> Option<SourceLocation> {
        if let Some(url) = self.alias(path) {
            return Some(SourceLocation { url, line, column });
        }
        let label = |prefix: &str| {
            path.strip_prefix(prefix)
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        };
        let url = if label("inline#") || label("script#") {
            path.to_string()
        } else {
            self.url(&Url::parse(path).ok()?)
        };
        Some(SourceLocation { url, line, column })
    }

    /// Whether error messages keep their text (see
    /// [`InspectorConfig::keep_error_text`]).
    pub fn keeps_error_text(&self) -> bool {
        self.keep_error_text
    }

    /// Whether console messages keep their text (see
    /// [`InspectorConfig::keep_console_text`]).
    pub fn keeps_console_text(&self) -> bool {
        self.keep_console_text
    }

    /// A console call (`message` is capped here, and URLs in it scrubbed).
    pub fn console(&self, level: ConsoleLevel, message: &str, location: Option<SourceLocation>) {
        self.console_with_stacks(level, message, location, &rustkit_js::ConsoleStacks::default());
    }

    /// [`console`](Self::console) with a `console.error`'s stacks.
    pub fn console_with_stacks(
        &self,
        level: ConsoleLevel,
        message: &str,
        location: Option<SourceLocation>,
        stacks: &rustkit_js::ConsoleStacks,
    ) {
        self.emit_with(|inspector| {
            let (message, truncated) = cap(message, MAX_MESSAGE_CHARS);
            let message = inspector.scrub_text(&message);
            let text = |frames: &[rustkit_js::ScriptFrame]| {
                let named: Vec<_> = frames.iter().map(|f| (f.function.as_deref(), &f.at)).collect();
                inspector.stack_text(&named)
            };
            let (stack, error_stack) = (text(&stacks.caller), text(&stacks.error));
            InspectorEvent::Console { level, message, truncated, location, stack, error_stack }
        });
    }

    /// The page's frames as a `stack` field: one `    at url:line:col` line
    /// each, `    at name (url:line:col)` for a named function when error
    /// text is kept here (the runtime may hold names for the console mirror,
    /// which this Inspector's setting does not follow); frames whose script
    /// cannot be shown are dropped. `None` when none is left.
    pub fn stack_text(&self, frames: &[(Option<&str>, &rustkit_js::ScriptPosition)]) -> Option<String> {
        let lines: Vec<String> = frames
            .iter()
            .filter_map(|(function, at)| {
                let place = self.frame_location(&at.path, at.line, at.column)?;
                let place = format!("{}:{}:{}", place.url, place.line, place.column);
                Some(match function.filter(|_| self.keep_error_text) {
                    Some(name) => format!("    at {name} ({place})"),
                    None => format!("    at {place}"),
                })
            })
            .collect();
        (!lines.is_empty()).then(|| cap(&lines.join("\n"), MAX_STACK_CHARS).0)
    }
}

/// One request's outcome, still to be recorded (see
/// [`Inspector::begin_request`]). Answering consumes it, so a request can
/// only be answered once; dropping it unanswered records a failure, so it
/// cannot be answered never.
pub struct RequestGuard {
    inspector: Arc<Inspector>,
    id: u64,
    started: Instant,
    dropped_as: FailureStage,
    dropped_reason: &'static str,
    /// Dropped at or past this deadline, it ran out of a budget: the stage
    /// and the reason to record.
    budget: Option<(Instant, FailureStage, &'static str)>,
    /// The shield's list and rule, for the failure it ends in.
    blocked_by: Option<BlockedBy>,
    /// Its wait for an in-flight slot, when a cap can queue it.
    slot: Option<rustkit_net::policy::SlotWait>,
    /// The deadline that ends it as `timeout` / `over_budget`, if known.
    deadline: Option<Instant>,
    done: bool,
}

impl RequestGuard {
    /// The request's id in the events.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The shield refused it with `reason` (if it said which list and rule):
    /// the failure this ends in carries them as `blocked_by`.
    pub fn blocked_by(mut self, reason: Option<&rustkit_net::BlockReason>) -> Self {
        self.blocked_by = reason.map(BlockedBy::from_reason);
        self
    }

    /// The request can be queued behind an in-flight cap: `slot` is what the
    /// policy stamps (see [`rustkit_net::policy::SlotWait`]); the outcome
    /// records `queued_ms` from it.
    pub fn waits_on(mut self, slot: &rustkit_net::policy::SlotWait) -> Self {
        self.slot = Some(slot.clone());
        self
    }

    /// Its slot wait, if it has one: hand it to the policy.
    pub fn slot(&self) -> Option<&rustkit_net::policy::SlotWait> {
        self.slot.as_ref()
    }

    /// The deadline the caller races it against: a `timeout` or
    /// `over_budget` failure is an `in-flight cap` if it was still queued
    /// then.
    pub fn deadline(mut self, deadline: tokio::time::Instant) -> Self {
        self.deadline = Some(deadline.into_std());
        self
    }

    fn queued_ms(&self) -> Option<f64> {
        self.slot.as_ref()?.waited().map(|d| round_ms(d.as_secs_f64() * 1000.0))
    }

    /// `reason`, unless a deadline (`timeout`, `over_budget`) ended the
    /// request while it was still queued behind a cap: then `in-flight cap`.
    fn deadline_reason<'a>(&self, stage: FailureStage, reason: &'a str) -> &'a str {
        if !matches!(stage, FailureStage::Timeout | FailureStage::OverBudget) {
            return reason;
        }
        let at = self.deadline.or(self.budget.map(|(at, _, _)| at));
        let queued = match (&self.slot, at) {
            (Some(slot), Some(at)) => slot.waiting_at(at),
            (Some(slot), None) => slot.is_waiting(),
            (None, _) => false,
        };
        if queued {
            IN_FLIGHT_CAP
        } else {
            reason
        }
    }

    /// The stage and reason recorded if this is dropped unanswered.
    pub fn dropped_as(mut self, stage: FailureStage, reason: &'static str) -> Self {
        self.dropped_as = stage;
        self.dropped_reason = reason;
        self
    }

    /// Dropped unanswered at or after `deadline` (the budget that bounds the
    /// fetch ran out), it is recorded `over_budget` with `reason`; dropped
    /// before it (a navigation, a closed view), at the drop stage.
    pub fn over_budget_at(mut self, deadline: tokio::time::Instant, reason: &'static str) -> Self {
        self.budget = Some((deadline.into_std(), FailureStage::OverBudget, reason));
        self
    }

    /// Dropped unanswered at or after `deadline`, it is recorded `timeout`
    /// with `reason`. A network cap, not the script budget: the script is
    /// not interrupted. Dropped before it, at the drop stage.
    pub fn timeout_at(mut self, deadline: tokio::time::Instant, reason: &'static str) -> Self {
        self.budget = Some((deadline.into_std(), FailureStage::Timeout, reason));
        self
    }

    fn duration_ms(&self) -> f64 {
        round_ms(self.started.elapsed().as_secs_f64() * 1000.0)
    }

    /// The answer arrived. `mime` may carry parameters; only its essence is
    /// read, and recorded only as a fixed name (see [`mime_essence`]):
    /// anything else is the server's text and is `null`.
    pub fn response(
        mut self,
        status: u16,
        mime: Option<&str>,
        encoded_size: Option<u64>,
        decoded_size: Option<u64>,
        from_cache: bool,
    ) {
        self.done = true;
        let mime = mime.and_then(mime_essence);
        let event = InspectorEvent::Response {
            id: self.id,
            status,
            mime,
            encoded_size,
            decoded_size,
            from_cache,
            duration_ms: self.duration_ms(),
            queued_ms: self.queued_ms(),
        };
        self.inspector.finish_request(self.id, event);
    }

    /// The request failed at `stage`. `reason` is engine text; URLs in it
    /// are scrubbed and it is capped.
    pub fn fail(mut self, stage: FailureStage, reason: &str) {
        self.done = true;
        let reason = self.deadline_reason(stage, reason);
        let reason = self.inspector.scrub_text(&cap(reason, MAX_SHORT_CHARS).0);
        let event = InspectorEvent::Failure {
            id: self.id,
            stage,
            reason,
            duration_ms: self.duration_ms(),
            blocked_by: self.blocked_by.take(),
            queued_ms: self.queued_ms(),
        };
        self.inspector.finish_request(self.id, event);
    }
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let (stage, reason) = match self.budget {
            Some((deadline, stage, reason)) if Instant::now() >= deadline => (stage, reason),
            _ => (self.dropped_as, self.dropped_reason),
        };
        let event = InspectorEvent::Failure {
            id: self.id,
            stage,
            reason: self.deadline_reason(stage, reason).to_string(),
            duration_ms: self.duration_ms(),
            blocked_by: self.blocked_by.take(),
            queued_ms: self.queued_ms(),
        };
        self.inspector.finish_request(self.id, event);
    }
}

impl RequestGuard {
    /// The response's status line arrived; its body is still to be read.
    pub fn received(self, response: &rustkit_net::Response) -> ReceivedResponse {
        let mime = response
            .content_type
            .as_ref()
            .map(|m| m.essence_str().to_string())
            .or_else(|| response.headers.get("content-type").and_then(|v| v.to_str().ok()).map(str::to_string));
        ReceivedResponse {
            status: response.status.as_u16(),
            mime,
            encoded_size: response.encoded_body_size,
            from_cache: response.from_cache,
            guard: self,
        }
    }
}

/// A request whose response arrived and whose body is being read: record
/// it with the decoded size once read, or as a failure if the body is not
/// usable. Dropped, it records the guard's drop stage.
pub struct ReceivedResponse {
    guard: RequestGuard,
    status: u16,
    mime: Option<String>,
    encoded_size: Option<u64>,
    from_cache: bool,
}

impl ReceivedResponse {
    /// The body was read: `decoded_size` bytes.
    pub fn read(self, decoded_size: usize) {
        self.guard.response(
            self.status,
            self.mime.as_deref(),
            self.encoded_size,
            Some(decoded_size as u64),
            self.from_cache,
        );
    }

    /// The response is recorded without reading its body.
    pub fn unread(self) {
        self.guard
            .response(self.status, self.mime.as_deref(), self.encoded_size, None, self.from_cache);
    }

    /// The body arrived and was not usable (`HTTP <status>` is prefixed).
    pub fn fail(self, stage: FailureStage, reason: &str) {
        let reason = format!("HTTP {}: {reason}", self.status);
        self.guard.fail(stage, &reason);
    }
}

/// Record `response` for `guard` (if any), its body `decoded_size` bytes.
pub fn answer(guard: Option<RequestGuard>, response: &rustkit_net::Response, decoded_size: usize) {
    if let Some(guard) = guard {
        guard.received(response).read(decoded_size);
    }
}

/// Record a module fetch through the page's policy: its source arrived,
/// the policy refused it, or the network cap ran out (`Err(Elapsed)`).
pub fn answer_module(
    guard: Option<RequestGuard>,
    fetched: &Result<Result<rustkit_net::policy::ModuleSource, rustkit_net::policy::Denial>, tokio::time::error::Elapsed>,
) {
    let Some(guard) = guard else { return };
    match fetched {
        Ok(Ok(module)) => guard.response(
            module.status,
            module.content_type.as_ref().and_then(|v| v.to_str().ok()),
            None,
            Some(module.body_len as u64),
            false,
        ),
        // The server answered; the engine will not run an error page.
        Ok(Err(rustkit_net::policy::Denial::BadStatus(status))) => guard.response(*status, None, None, None, false),
        Ok(Err(denial)) => {
            let (stage, reason) = denial_stage(denial);
            guard.blocked_by(denial_block_reason(denial)).fail(stage, reason);
        }
        Err(_) => guard.fail(FailureStage::Timeout, "timed out"),
    }
}

/// Fail `guard` (if any) for a network error.
pub fn fail_net(guard: Option<RequestGuard>, error: &rustkit_net::NetError) {
    if let Some(guard) = guard {
        let (stage, reason) = net_error_stage(error);
        guard.blocked_by(net_block_reason(error)).fail(stage, reason);
    }
}

/// The shield's list and rule, when `error` is its block and it said which.
pub fn net_block_reason(error: &rustkit_net::NetError) -> Option<&rustkit_net::BlockReason> {
    match error {
        rustkit_net::NetError::Blocked(reason) => reason.as_ref(),
        _ => None,
    }
}

/// The shield's list and rule, when `denial` is its block and it said which.
pub fn denial_block_reason(denial: &rustkit_net::policy::Denial) -> Option<&rustkit_net::BlockReason> {
    match denial {
        rustkit_net::policy::Denial::Shield(reason) => reason.as_ref(),
        _ => None,
    }
}

/// The stage of a script request's refusal.
pub fn denial_stage(denial: &rustkit_net::policy::Denial) -> (FailureStage, &'static str) {
    use rustkit_net::policy::Denial;
    match denial {
        Denial::Scheme(_) => (FailureStage::Policy, "scheme not allowed"),
        Denial::BadRequest(_) => (FailureStage::Policy, "malformed request"),
        Denial::PrivateNetwork(_) => (FailureStage::Policy, "private network"),
        Denial::MixedContent => (FailureStage::Policy, "mixed content"),
        Denial::Csp => (FailureStage::Policy, "content security policy"),
        Denial::SameOriginOnly => (FailureStage::Cors, "same-origin request to another origin"),
        Denial::Cors(_) => (FailureStage::Cors, "CORS check failed"),
        Denial::Preflight(_) => (FailureStage::Cors, "CORS preflight failed"),
        Denial::Redirect(_) => (FailureStage::Policy, "redirect not allowed"),
        Denial::TooManyRedirects => (FailureStage::HttpError, "too many redirects"),
        Denial::RequestTooLarge => (FailureStage::Policy, "request body too large"),
        Denial::ResponseTooLarge => (FailureStage::OverBudget, "response too large"),
        Denial::Timeout => (FailureStage::Timeout, "timed out"),
        Denial::BudgetExhausted => (FailureStage::OverBudget, "page request budget spent"),
        Denial::Shield(_) => (FailureStage::Policy, "blocked by the shield"),
        Denial::BadMime(_) => (FailureStage::Mime, "not a JavaScript MIME type"),
        Denial::BadStatus(_) => (FailureStage::HttpError, "module answered with an error status"),
        Denial::Network(_) => (FailureStage::Connect, "network error"),
        Denial::Cancelled => (FailureStage::Cancelled, "cancelled"),
    }
}

/// The stage of a network error, and a reason with nothing from the
/// error's own text (which can name hosts and paths).
pub fn net_error_stage(error: &rustkit_net::NetError) -> (FailureStage, &'static str) {
    match net_error_kind(error) {
        "dns" => (FailureStage::Dns, "host not found"),
        "connect" => (FailureStage::Connect, "could not connect"),
        "tls" => (FailureStage::Tls, "TLS failed"),
        "timeout" => (FailureStage::Timeout, "timed out"),
        "policy" => match error {
            rustkit_net::NetError::Blocked(_) => (FailureStage::Policy, "blocked by the shield"),
            _ => (FailureStage::Policy, "address not allowed"),
        },
        "redirect" => (FailureStage::HttpError, "too many redirects"),
        "over_budget" => (FailureStage::OverBudget, "response too large"),
        "http" => (FailureStage::HttpError, "invalid response"),
        "invalid_url" => (FailureStage::Policy, "invalid or unsupported URL"),
        "cancelled" => (FailureStage::Cancelled, "cancelled"),
        _ => (FailureStage::Connect, "network error"),
    }
}

fn round_ms(ms: f64) -> f64 {
    (ms * 1000.0).round() / 1000.0
}

/// `text` cut to at most `max` characters, and whether it was cut.
pub fn cap(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((end, _)) => (text[..end].to_string(), true),
        None => (text.to_string(), false),
    }
}

/// `url` with its credentials always removed, and its query, fragment and
/// `;` path parameters (`;jsessionid=`) unless `keep_query`. A `data:` URL
/// is reduced to `data:<mime>,…` (a common MIME type only); a `javascript:`
/// URL is always reduced to its scheme, with or without an authority
/// (`javascript://h/alert(1)` is a base URL to the parser; R1 M1); any
/// other URL that cannot have a host (`about:`, `blob:`) keeps its scheme
/// and, for `blob:`, its origin.
pub fn scrub_url(url: &Url, keep_query: bool) -> String {
    match url.scheme() {
        "data" => format!("data:{},…", data_mime(&url.as_str()[5..])),
        "javascript" => "javascript:".to_string(),
        "blob" => match Url::parse(url.path()) {
            Ok(inner) => format!("blob:{}", inner.origin().ascii_serialization()),
            Err(_) => "blob:".to_string(),
        },
        _ if url.cannot_be_a_base() => format!("{}:", url.scheme()),
        _ => {
            let mut clean = url.clone();
            let _ = clean.set_username("");
            let _ = clean.set_password(None);
            if !keep_query {
                clean.set_query(None);
                clean.set_fragment(None);
                // The parser's path: `\` already reads as `/` for a special
                // scheme, and tabs and newlines are gone.
                let path = drop_path_params(clean.path());
                // A host-less path that now starts with `//` (`foo:/;x/a@b`)
                // would serialise as an authority (`foo://a@b`); url 2.5.7's
                // `set_path` adds no `/.`, so it is written here (T8).
                if clean.host().is_none() && path.starts_with("//") {
                    return format!("{}:/.{path}", clean.scheme());
                }
                clean.set_path(&path);
            }
            clean.to_string()
        }
    }
}

/// MIME types a `data:` URL keeps; any other is the page's text, dropped.
const DATA_MIME_TYPES: [&str; 18] = [
    "text/javascript",
    "application/javascript",
    "application/x-javascript",
    "text/ecmascript",
    "application/ecmascript",
    "text/css",
    "text/html",
    "text/plain",
    "text/xml",
    "application/json",
    "application/wasm",
    "image/svg+xml",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "font/woff2",
    "application/octet-stream",
];

/// A `data:` URL's MIME type (`rest` follows `data:`) when it is a known
/// one, else nothing.
fn data_mime(rest: &str) -> &'static str {
    let mime = rest.split([',', ';']).next().unwrap_or("").trim();
    DATA_MIME_TYPES.iter().find(|m| m.eq_ignore_ascii_case(mime)).copied().unwrap_or("")
}

/// A path without its `;` parameters (`/cart;jsessionid=S/x` is `/cart/x`):
/// a servlet puts the session cookie there when cookies are off. Each one
/// runs from `;` (or `%3B`, to record less) to the end of its segment. The
/// same rule as the diagnostics half's `report_url`.
pub(crate) fn drop_path_params(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while !rest.is_empty() {
        let at = rest.find(';').into_iter().chain(rest.find("%3B")).chain(rest.find("%3b")).min();
        let Some(at) = at else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        rest = &rest[rest.find('/').unwrap_or(rest.len())..];
    }
    out
}

/// Where `scheme` next starts in `lower` from `from`: anywhere for a
/// `scheme://`, and for `data:` only where no letter or digit comes right
/// before it (`metadata:` is a word).
fn find_scheme(lower: &str, from: usize, scheme: &str) -> Option<usize> {
    let mut at = from;
    loop {
        let found = at + lower[at..].find(scheme)?;
        let word_before = lower[..found].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric());
        if scheme.ends_with("//") || !word_before {
            return Some(found);
        }
        at = found + scheme.len();
    }
}

/// What ends a URL's query in text and is not part of it: a frame's
/// `:line:col)`, else closing punctuation (`'`, `)`, `.`, ...).
fn query_tail(query: &str) -> &str {
    if let Some(inner) = query.strip_suffix(')') {
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if let Some((rest, column)) = inner.rsplit_once(':') {
            if let Some((_, line)) = rest.rsplit_once(':') {
                if digits(line) && digits(column) {
                    return &query[query.len() - line.len() - column.len() - 3..];
                }
            }
        }
    }
    let kept = query.len() - query.trim_end_matches(|c: char| ".,;:!?'\"`)]}>".contains(c)).len();
    &query[query.len() - kept..]
}

/// The response MIME types a record names; any other is the server's text.
const RESPONSE_MIME_TYPES: [&str; 42] = [
    "text/html",
    "text/css",
    "text/javascript",
    "application/javascript",
    "application/x-javascript",
    "text/ecmascript",
    "application/ecmascript",
    "application/json",
    "application/ld+json",
    "application/manifest+json",
    "text/plain",
    "text/xml",
    "application/xml",
    "application/xhtml+xml",
    "text/event-stream",
    "image/svg+xml",
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/avif",
    "image/bmp",
    "image/x-icon",
    "image/vnd.microsoft.icon",
    "font/woff",
    "font/woff2",
    "font/ttf",
    "font/otf",
    "application/font-woff",
    "audio/mpeg",
    "audio/ogg",
    "audio/wav",
    "audio/webm",
    "video/mp4",
    "video/webm",
    "video/ogg",
    "application/octet-stream",
    "application/wasm",
    "application/pdf",
    "application/x-www-form-urlencoded",
    "multipart/form-data",
    "text/csv",
];

/// The registered top-level types: a type not on the list above keeps only
/// its top-level type, as `<type>/other`.
const MIME_TOP_LEVEL_TYPES: [&str; 9] =
    ["application", "audio", "font", "image", "message", "model", "multipart", "text", "video"];

/// A Content-Type's essence as a record names it: one of a fixed list of
/// types, else `<registered top-level type>/other`, else `None`. Only a
/// well-formed `type/subtype` (each side an RFC 9110 token) is read at all:
/// anything else (`application/json, token=...`) is header text. Nothing the
/// server chose beyond the fixed names is kept.
pub fn mime_essence(content_type: &str) -> Option<String> {
    let essence = content_type.split(';').next().unwrap_or("").trim();
    let token = |s: &str| {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
    };
    let (kind, subtype) = essence.split_once('/')?;
    if !(token(kind) && token(subtype)) {
        return None;
    }
    if let Some(known) = RESPONSE_MIME_TYPES.iter().find(|m| m.eq_ignore_ascii_case(essence)) {
        return Some(known.to_string());
    }
    MIME_TOP_LEVEL_TYPES
        .iter()
        .find(|t| t.eq_ignore_ascii_case(kind))
        .map(|t| format!("{t}/other"))
}

/// A script position as text gives it: path, line, column.
type PositionParts = (String, u32, u32);

/// `path:line:col`, the path possibly holding colons itself.
fn parse_position(text: &str) -> Option<PositionParts> {
    let (rest, column) = text.rsplit_once(':')?;
    let (path, line) = rest.rsplit_once(':')?;
    let path = path.strip_prefix("unknown at ").unwrap_or(path);
    Some((path.trim().to_string(), line.parse().ok()?, column.parse().ok()?))
}

/// Writes each record as one JSON line.
pub struct JsonLinesSink<W: Write + Send> {
    out: W,
}

impl<W: Write + Send> JsonLinesSink<W> {
    pub fn new(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write + Send> InspectorSink for JsonLinesSink<W> {
    fn record(&mut self, record: &InspectorRecord) {
        if let Ok(line) = serde_json::to_string(record) {
            let _ = writeln!(self.out, "{line}");
        }
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

/// JSON Lines into `path`, rotated when it would pass `max_bytes`: `path`
/// becomes `path.1`, `path.1` becomes `path.2`, ..., and `path.<keep>` is
/// deleted. Disk use stays under `max_bytes * (keep + 1)`. Write errors
/// are swallowed: diagnostics never fail a page.
pub struct RollingFileSink {
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    file: Option<std::io::BufWriter<std::fs::File>>,
    written: u64,
}

impl RollingFileSink {
    pub fn new(path: impl Into<PathBuf>, max_bytes: u64, keep: usize) -> std::io::Result<Self> {
        let path = path.into();
        let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            max_bytes: max_bytes.max(1),
            keep,
            file: Some(std::io::BufWriter::new(file)),
            written,
        })
    }

    fn rotated(path: &Path, n: usize) -> PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    fn rotate(&mut self) {
        if let Some(mut file) = self.file.take() {
            let _ = file.flush();
        }
        if self.keep == 0 {
            let _ = std::fs::remove_file(&self.path);
        } else {
            let _ = std::fs::remove_file(Self::rotated(&self.path, self.keep));
            for n in (1..self.keep).rev() {
                let _ = std::fs::rename(Self::rotated(&self.path, n), Self::rotated(&self.path, n + 1));
            }
            let _ = std::fs::rename(&self.path, Self::rotated(&self.path, 1));
        }
        self.file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
            .ok()
            .map(std::io::BufWriter::new);
        self.written = 0;
    }
}

impl InspectorSink for RollingFileSink {
    fn record(&mut self, record: &InspectorRecord) {
        let Ok(mut line) = serde_json::to_string(record) else { return };
        line.push('\n');
        if self.written > 0 && self.written + line.len() as u64 > self.max_bytes {
            self.rotate();
        }
        if let Some(file) = &mut self.file {
            if file.write_all(line.as_bytes()).is_ok() {
                self.written += line.len() as u64;
            }
        }
    }

    fn flush(&mut self) {
        if let Some(file) = &mut self.file {
            let _ = file.flush();
        }
    }
}

/// Collects records in memory (parity-capture's whole-load log, tests).
#[derive(Clone, Default)]
pub struct CollectingSink(pub Arc<Mutex<Vec<InspectorRecord>>>);

impl CollectingSink {
    pub fn records(&self) -> Vec<InspectorRecord> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl InspectorSink for CollectingSink {
    fn record(&mut self, record: &InspectorRecord) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).push(record.clone());
    }
}

/// Console calls by level.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleCounts {
    pub log: u64,
    pub info: u64,
    pub warn: u64,
    pub error: u64,
    pub debug: u64,
}

/// Counts over one load's records: what `parity-capture --event-summary`
/// prints and the site board publishes per site. Stable JSON: fields keep
/// their names and order, maps are sorted by key, and new fields are only
/// appended (`schema` goes up when one is). Documented in
/// `docs/INSPECTOR_EVENT_LOG.md`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InspectorSummary {
    /// Version of this layout.
    pub schema: u32,
    /// Records the summary was made from.
    pub events: u64,
    pub console: ConsoleCounts,
    /// Uncaught exceptions, unhandled rejections included.
    pub script_errors: u64,
    /// Of `script_errors`, the unhandled rejections.
    pub unhandled_rejections: u64,
    /// `Interface.member` to how often page script reached it.
    pub unsupported_apis: std::collections::BTreeMap<String, u64>,
    /// Requests by resource type, then by outcome: the response's status
    /// class (`2xx`, `3xx`, `4xx`, `5xx`, `other`) or `failed`.
    pub requests: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>,
    /// Failed requests by stage.
    pub failures_by_stage: std::collections::BTreeMap<String, u64>,
}

/// The summary layout version (2: `requests`, `failures_by_stage`).
pub const SUMMARY_SCHEMA: u32 = 2;

impl InspectorSummary {
    /// Count `records`; the unsupported members' counts come from
    /// `unsupported` (the events only mark first sightings).
    pub fn from_records(records: &[InspectorRecord], unsupported: &[UnsupportedApiCount]) -> Self {
        let mut summary = InspectorSummary {
            schema: SUMMARY_SCHEMA,
            events: records.len() as u64,
            ..Default::default()
        };
        let mut types = std::collections::HashMap::new();
        for record in records {
            match &record.event {
                InspectorEvent::Request { id, resource_type, .. } => {
                    types.insert(*id, resource_type.as_str());
                }
                InspectorEvent::Response { id, status, .. } => {
                    let outcome = match status {
                        200..=299 => "2xx",
                        300..=399 => "3xx",
                        400..=499 => "4xx",
                        500..=599 => "5xx",
                        _ => "other",
                    };
                    let kind = types.get(id).copied().unwrap_or("other");
                    *summary.requests.entry(kind.into()).or_default().entry(outcome.into()).or_insert(0) += 1;
                }
                InspectorEvent::Failure { id, stage, .. } => {
                    let kind = types.get(id).copied().unwrap_or("other");
                    *summary.requests.entry(kind.into()).or_default().entry("failed".into()).or_insert(0) += 1;
                    *summary.failures_by_stage.entry(stage.as_str().into()).or_insert(0) += 1;
                }
                InspectorEvent::Console { level, .. } => {
                    let counter = match level {
                        ConsoleLevel::Log => &mut summary.console.log,
                        ConsoleLevel::Info => &mut summary.console.info,
                        ConsoleLevel::Warn => &mut summary.console.warn,
                        ConsoleLevel::Error => &mut summary.console.error,
                        ConsoleLevel::Debug => &mut summary.console.debug,
                    };
                    *counter += 1;
                }
                InspectorEvent::ScriptError { unhandled_rejection, .. } => {
                    summary.script_errors += 1;
                    summary.unhandled_rejections += u64::from(*unhandled_rejection);
                }
                InspectorEvent::UnsupportedApi { interface, member, .. } => {
                    summary.unsupported_apis.entry(format!("{interface}.{member}")).or_insert(1);
                }
                _ => {}
            }
        }
        for api in unsupported {
            summary
                .unsupported_apis
                .insert(format!("{}.{}", api.interface, api.member), api.count);
        }
        summary
    }
}

/// What stage a network error failed at, as one word (no host, path or
/// other detail from the error's own text).
pub fn net_error_kind(error: &rustkit_net::NetError) -> &'static str {
    use rustkit_http::HttpError;
    use rustkit_net::NetError;
    match error {
        NetError::Blocked(_) => "policy",
        NetError::Timeout(_) => "timeout",
        NetError::Cancelled => "cancelled",
        NetError::InvalidUrl(_) => "invalid_url",
        NetError::RequestFailed(_) | NetError::IoError(_) => "network",
        NetError::HttpError(http) => match http {
            HttpError::LookupFailed(_) => "dns",
            HttpError::ConnectionFailed(_) => "connect",
            HttpError::IoError(_) => "connect",
            HttpError::TlsError(_) => "tls",
            HttpError::Timeout => "timeout",
            HttpError::AddressDenied(_) => "policy",
            HttpError::TooManyRedirects => "redirect",
            HttpError::BodyTooLarge(_) => "over_budget",
            HttpError::InvalidResponse(_) => "http",
            HttpError::InvalidUrl(_) | HttpError::UnsupportedScheme(_) => "invalid_url",
        },
    }
}
