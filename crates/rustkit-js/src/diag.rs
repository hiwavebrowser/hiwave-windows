//! Page errors described for diagnostics (the engine's Inspector), and the
//! queue they wait in.
//!
//! Default deny: a report carries no text the page chose. What was thrown is
//! its type (`error`, `string`, `object`, ...), an error's name only when it
//! is one of the names the engine itself gives ([`STANDARD_ERROR_NAMES`],
//! else `Error`), a message only as its length, and the page's frames only as
//! `path:line:column` of scripts the engine evaluated. A message is kept only
//! when the embedder asked for error text ([`Diagnostics::keep_text`], unsafe
//! for a log that will be shared). Nothing here reads a message's text to
//! decide anything: an error that reached page script, whatever raised it,
//! is the page's (page code can catch an engine error and rethrow it, or
//! throw the engine's very words), so it is never kept as an engine message.
//! The engine's own failures are marked where it raises them, outside this
//! module (`rustkit_engine::EngineFailure`).
//!
//! Everything here is filled from native code: the reporter the bindings'
//! own scripts call is handed to them in a closure (see
//! [`crate::JsRuntime::evaluate_with_host`]), never through anything page
//! script can reach, and describing a value runs none of the page's code.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Instant;

use boa_engine::builtins::error::Error;
use boa_engine::{Context, JsObject, JsValue};

use crate::{ConsoleStacks, ErrorReport, LogLevel, ScriptFrame, ScriptPosition};

/// One console call: level, line, its shape line (when one was asked
/// for, see `console_shapes`), the caller's position, and the stacks of an
/// error-level call while reports are on.
pub(crate) type ConsoleCall = (LogLevel, String, Option<String>, Option<ScriptPosition>, ConsoleStacks);

/// An unimplemented member the embedder's scripts saw page script reach.
struct Unsupported {
    interface: String,
    member: String,
    count: u64,
    sent: u64,
    at: Option<ScriptPosition>,
}

/// Reports held at once; past it the oldest is dropped.
const MAX_REPORTS: usize = 256;
/// Listener and timer exception texts held for the script log at once.
const MAX_REPORTED_TEXTS: usize = 1024;
/// Script paths remembered (frames in any other path are dropped).
const MAX_KNOWN_PATHS: usize = 4096;
/// Frames kept per report.
const MAX_FRAMES: usize = 32;

/// The path every script the embedder evaluates (not the page's) runs as:
/// its frames are told apart by where they are, never by what a function
/// is called. No page script is ever given it: theirs are `inline#N` or
/// the URL they came from, and it parses as neither.
pub(crate) const EMBEDDER_PATH: &str = "rustkit#embedder";

/// Console calls held between flushes; past it the oldest is dropped.
const MAX_CONSOLE_PENDING: usize = 1024;

/// Live hand-overs in one second (T6d). A hand-over carries the calls
/// held before it too, so this caps hand-overs, not calls: up to
/// `MAX_CONSOLE_LIVE + MAX_CONSOLE_PENDING` calls can reach the handler in
/// a second. A page logging in a hot loop past it has its calls held, as
/// with no live reader.
pub const MAX_CONSOLE_LIVE: u32 = 200;

/// Where console calls go as they are made ([`crate::JsRuntime::set_console_live`]).
pub(crate) struct LiveConsole {
    pub(crate) on: std::sync::Arc<crate::ConsoleLive>,
    pub(crate) handler: std::sync::Arc<crate::ConsoleHandler>,
    pub(crate) shapes: Option<std::sync::Arc<crate::ConsoleHandler>>,
}

/// Hand `calls` to the handlers, after a line for the `dropped` ones
/// before them, if any: engine text, so the shape handler gets it too.
/// `capped`: the live rate cap held calls, so it is why they were dropped.
pub(crate) fn deliver_console(
    handler: &crate::ConsoleHandler,
    shapes: Option<&std::sync::Arc<crate::ConsoleHandler>>,
    dropped: u64,
    calls: Vec<ConsoleCall>,
    capped: bool,
) {
    if dropped > 0 {
        let cause = if capped { "; console rate cap" } else { "" };
        let line = format!("{dropped} console calls dropped (more than {MAX_CONSOLE_PENDING} held{cause})");
        let none = ConsoleStacks::default();
        handler(LogLevel::Warn, &line, None, &none);
        if let Some(shapes) = shapes {
            shapes(LogLevel::Warn, &line, None, &none);
        }
    }
    for (level, msg, shape, at, stack) in calls {
        handler(level, &msg, at.as_ref(), &stack);
        if let (Some(shapes), Some(shape)) = (shapes, shape) {
            shapes(level, &shape, at.as_ref(), &stack);
        }
    }
}
/// `console.count` and `console.time` labels held at once; a new label past
/// it is not counted or timed.
const MAX_CONSOLE_LABELS: usize = 1024;
/// Distinct unsupported members held.
const MAX_UNSUPPORTED: usize = 64;

/// The error names the engine itself gives: ECMAScript's and the
/// `DOMException` names the bindings throw. Any other `name` is page text.
pub const STANDARD_ERROR_NAMES: [&str; 28] = [
    "Error",
    "TypeError",
    "RangeError",
    "SyntaxError",
    "ReferenceError",
    "EvalError",
    "URIError",
    "AggregateError",
    "DOMException",
    "AbortError",
    "DataCloneError",
    "HierarchyRequestError",
    "IndexSizeError",
    "InvalidCharacterError",
    "InvalidStateError",
    "NamespaceError",
    "NoModificationAllowedError",
    "NotAllowedError",
    "NotFoundError",
    "NotSupportedError",
    "OperationError",
    "QuotaExceededError",
    "SecurityError",
    "TimeoutError",
    "TypeMismatchError",
    "NetworkError",
    "EncodingError",
    "InvalidAccessError",
];

/// A thrown value's `name` as a report keeps it.
pub fn standard_name(name: Option<&str>) -> &'static str {
    name.and_then(|n| STANDARD_ERROR_NAMES.iter().find(|s| **s == n).copied())
        .unwrap_or("Error")
}

#[derive(Default)]
pub(crate) struct Diagnostics {
    /// The embedder records page errors ([`crate::JsRuntime::set_error_reports`]).
    pub(crate) on: Cell<bool>,
    /// Reports keep the page's message text (unsafe to share).
    pub(crate) keep_text: Cell<bool>,
    reports: RefCell<VecDeque<ErrorReport>>,
    /// What listeners and timers threw, as the script log has always had it
    /// (`describe::text`), whether or not reports are on.
    reported_texts: RefCell<VecDeque<String>>,
    /// The paths of the scripts and modules the engine evaluated.
    known_paths: RefCell<HashSet<String>>,
    /// The realm's own `Error.prototype.stack` getter, taken before any page
    /// script ran.
    pub(crate) stack_getter: RefCell<Option<JsObject>>,
    /// Console calls are recorded (an embedder set a console handler).
    pub(crate) console_capturing: Cell<bool>,
    /// Console arguments are recorded as type and length only.
    pub(crate) console_redacted: Cell<bool>,
    /// Each call also records a type-and-length line beside its line (an
    /// embedder set a shape handler).
    pub(crate) console_shapes: Cell<bool>,
    console: RefCell<VecDeque<ConsoleCall>>,
    /// Calls dropped from `console` since it was last taken.
    console_dropped: Cell<u64>,
    pub(crate) console_live: RefCell<Option<std::rc::Rc<LiveConsole>>>,
    /// The current second of live hand-overs: its start and how many.
    console_live_window: Cell<(Option<Instant>, u32)>,
    /// A call was held because the live rate cap was spent.
    console_capped: Cell<bool>,
    /// Where the embedder reads how many calls are held (`console.len()`).
    pub(crate) console_held: RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>>,
    console_counts: RefCell<HashMap<String, u64>>,
    console_timers: RefCell<HashMap<String, Instant>>,
    unsupported: RefCell<Vec<Unsupported>>,
}

impl Diagnostics {
    /// A console call at `level` with `args` (the native `console.*`
    /// methods): its line is built here, from the values as they are now,
    /// running none of the page's code.
    pub(crate) fn console_call(&self, level: LogLevel, args: &[JsValue], context: &mut Context) {
        if !self.console_capturing.get() {
            return;
        }
        let line = match self.console_redacted.get() {
            true => crate::describe::join_line(args, |arg| crate::describe::redacted_text(arg, context)),
            false => crate::describe::join_line(args, |arg| crate::describe::text(arg, context)),
        };
        let shape = self
            .console_shapes
            .get()
            .then(|| crate::describe::join_line(args, |arg| crate::describe::redacted_text(arg, context)));
        let at = caller_position(context, false);
        // Stacks only for `console.error` (and a failed `assert`), and only
        // for an embedder that records page errors: one more walk of the
        // frames the position walk already started, and the first error
        // argument's own frames through the realm's getter.
        let stack = match (level, self.on.get()) {
            (LogLevel::Error, true) => ConsoleStacks {
                caller: page_stack(context, self.keep_text.get()),
                error: self.first_error_frames(args, context),
            },
            _ => ConsoleStacks::default(),
        };
        let call = (level, line, shape, at, stack);
        // Decided per call: the reader may have come or gone since the last.
        let live = self.console_live.borrow().clone().filter(|live| (*live.on)());
        let wanted_live = live.is_some();
        if let Some(live) = live.filter(|_| self.live_slot()) {
            // What was held goes first: each call once, in order.
            let (dropped, mut calls, capped) = self.take_console_counted();
            calls.push(call);
            deliver_console(&live.handler, live.shapes.as_ref(), dropped, calls, capped);
            return;
        }
        // Held for the cap, or because nobody reads live: the dropped line
        // names the cap only when that is why (the latest hold decides).
        self.console_capped.set(wanted_live);
        let mut pending = self.console.borrow_mut();
        if pending.len() >= MAX_CONSOLE_PENDING {
            pending.pop_front();
            self.console_dropped.set(self.console_dropped.get() + 1);
        }
        pending.push_back(call);
        self.note_held(pending.len());
    }

    fn note_held(&self, n: usize) {
        if let Some(held) = self.console_held.borrow().as_ref() {
            held.store(n, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Whether one more call may be handed over live this second.
    fn live_slot(&self) -> bool {
        let now = Instant::now();
        let (start, n) = match self.console_live_window.get() {
            (Some(start), n) if now.saturating_duration_since(start) < std::time::Duration::from_secs(1) => (start, n),
            _ => (now, 0),
        };
        let ok = n < MAX_CONSOLE_LIVE;
        self.console_live_window.set((Some(start), n + u32::from(ok)));
        ok
    }

    /// A Console namespace member other than the five logging ones, as
    /// the standard's printer gets it: `table` and `dir` print their data,
    /// `assert` prints only when its condition is false, `count` and `time`
    /// keep per-label state here, `group` prints its label, `groupEnd` and
    /// `clear` print nothing. None of the page's code runs.
    pub(crate) fn console_member(&self, member: &str, args: &[JsValue], context: &mut Context) {
        if !self.console_capturing.get() {
            return;
        }
        let label = match args.first().filter(|v| !v.is_undefined()) {
            Some(value) => crate::describe::text(value, context),
            None => "default".to_string(),
        };
        let text = |line: String| JsValue::from(boa_engine::JsString::from(line.as_str()));
        match member {
            "table" | "dir" => self.console_call(LogLevel::Log, &args[..args.len().min(1)], context),
            "dirxml" | "trace" => self.console_call(LogLevel::Log, args, context),
            "group" | "groupCollapsed" if !args.is_empty() => self.console_call(LogLevel::Log, args, context),
            "assert" => {
                if args.first().is_some_and(JsValue::to_boolean) {
                    return;
                }
                let data = args.get(1..).unwrap_or_default();
                let mut line = Vec::with_capacity(data.len() + 1);
                match data.first().and_then(JsValue::as_string) {
                    Some(first) => {
                        line.push(text(format!("Assertion failed: {}", first.to_std_string_escaped())));
                        line.extend_from_slice(&data[1..]);
                    }
                    None => {
                        line.push(text("Assertion failed".to_string()));
                        line.extend_from_slice(data);
                    }
                }
                self.console_call(LogLevel::Error, &line, context);
            }
            "count" => {
                let n = {
                    let mut counts = self.console_counts.borrow_mut();
                    if counts.len() >= MAX_CONSOLE_LABELS && !counts.contains_key(&label) {
                        return;
                    }
                    let n = counts.entry(label.clone()).or_insert(0);
                    *n += 1;
                    *n
                };
                self.console_call(LogLevel::Info, &[text(format!("{label}: {n}"))], context);
            }
            "countReset" => match self.console_counts.borrow_mut().get_mut(&label) {
                Some(n) => *n = 0,
                None => self.console_call(LogLevel::Warn, &[text(format!("Count for '{label}' does not exist"))], context),
            },
            "time" => {
                let mut timers = self.console_timers.borrow_mut();
                if timers.contains_key(&label) {
                    drop(timers);
                    self.console_call(LogLevel::Warn, &[text(format!("Timer '{label}' already exists"))], context);
                } else if timers.len() < MAX_CONSOLE_LABELS {
                    timers.insert(label, Instant::now());
                }
            }
            "timeLog" | "timeEnd" => {
                let started = match member {
                    "timeEnd" => self.console_timers.borrow_mut().remove(&label),
                    _ => self.console_timers.borrow().get(&label).copied(),
                };
                let Some(started) = started else {
                    self.console_call(LogLevel::Warn, &[text(format!("Timer '{label}' does not exist"))], context);
                    return;
                };
                let ms = started.elapsed().as_secs_f64() * 1000.0;
                let mut line = vec![text(format!("{label}: {ms:.3}ms"))];
                if member == "timeLog" {
                    line.extend_from_slice(args.get(1..).unwrap_or_default());
                }
                self.console_call(LogLevel::Info, &line, context);
            }
            _ => {}
        }
    }

    /// The first error object among `args`: its frames as a report's are
    /// read (the realm's own `stack` getter, scripts the engine evaluated
    /// only), names only with error text kept. Runs none of the page's code.
    fn first_error_frames(&self, args: &[JsValue], context: &mut Context) -> Vec<ScriptFrame> {
        let Some(error) = args.iter().find(|a| a.as_object().is_some_and(|o| o.is::<Error>())) else {
            return Vec::new();
        };
        let Some(stack) = self.stack_of(error, context) else { return Vec::new() };
        let (frames, functions, _) = self.frames(&stack);
        let keep = self.keep_text.get();
        frames
            .into_iter()
            .zip(functions)
            .map(|(at, function)| ScriptFrame { function: function.filter(|_| keep), at })
            .collect()
    }

    /// The held calls, how many were dropped before them, and whether the
    /// live rate cap held any of them.
    pub(crate) fn take_console_counted(&self) -> (u64, Vec<ConsoleCall>, bool) {
        let calls = self.console.borrow_mut().drain(..).collect();
        self.note_held(0);
        (self.console_dropped.replace(0), calls, self.console_capped.replace(false))
    }

    /// The calls held now, for a counter just set.
    pub(crate) fn console_held_now(&self) -> usize {
        self.console.borrow().len()
    }

    /// The embedder's scripts saw page script reach `interface.member`,
    /// which the engine does not implement. The first caller's position is
    /// noted while reports are on.
    pub(crate) fn unsupported(&self, interface: &str, member: &str, context: &mut Context) {
        let mut table = self.unsupported.borrow_mut();
        if let Some(entry) = table.iter_mut().find(|u| u.interface == interface && u.member == member) {
            entry.count += 1;
            return;
        }
        if table.len() >= MAX_UNSUPPORTED {
            return;
        }
        let at = match self.on.get() {
            true => caller_position(context, true),
            false => None,
        };
        table.push(Unsupported { interface: interface.to_string(), member: member.to_string(), count: 1, sent: 0, at });
    }

    /// The members reached since the last call: `(interface, member, times,
    /// first caller's position)`.
    pub(crate) fn take_unsupported(&self) -> Vec<(String, String, u64, Option<ScriptPosition>)> {
        let mut table = self.unsupported.borrow_mut();
        table
            .iter_mut()
            .filter(|u| u.count > u.sent)
            .map(|u| {
                let new = u.count - u.sent;
                u.sent = u.count;
                (u.interface.clone(), u.member.clone(), new, u.at.clone())
            })
            .collect()
    }

    /// `path` names a script the engine evaluates (noted only while
    /// reports are on: nothing is kept for an embedder that records none).
    pub(crate) fn know_path(&self, path: &str) {
        if !self.on.get() {
            return;
        }
        let mut known = self.known_paths.borrow_mut();
        if known.len() < MAX_KNOWN_PATHS {
            known.insert(path.to_string());
        }
    }

    pub(crate) fn push(&self, report: ErrorReport) {
        let mut reports = self.reports.borrow_mut();
        if reports.len() >= MAX_REPORTS {
            reports.pop_front();
        }
        reports.push_back(report);
    }

    pub(crate) fn take(&self) -> Vec<ErrorReport> {
        self.reports.borrow_mut().drain(..).collect()
    }

    pub(crate) fn take_reported_texts(&self) -> Vec<String> {
        self.reported_texts.borrow_mut().drain(..).collect()
    }

    /// A listener or timer threw `value` (the bindings' reporter).
    pub(crate) fn reported(&self, value: &JsValue, context: &mut Context) {
        {
            let text = crate::describe::text(value, context);
            let mut texts = self.reported_texts.borrow_mut();
            if texts.len() >= MAX_REPORTED_TEXTS {
                texts.pop_front();
            }
            texts.push_back(text);
        }
        if self.on.get() {
            let report = self.describe(value, None, false, context);
            self.push(report);
        }
    }

    /// `value` as a report. `frames` is Boa's stack text for a value with
    /// none of its own (a thrown primitive); an error object's frames are
    /// the engine's record of where it was made.
    pub(crate) fn describe(
        &self,
        value: &JsValue,
        frames: Option<&str>,
        rejection: bool,
        context: &mut Context,
    ) -> ErrorReport {
        let keep = self.keep_text.get();
        let mut report = ErrorReport {
            value: "undefined",
            name: None,
            message: None,
            message_len: None,
            frames: Vec::new(),
            functions: Vec::new(),
            located: false,
            rejection,
            module: None,
        };
        if let Some(s) = value.as_string() {
            let s = s.to_std_string_escaped();
            report.value = "string";
            report.message_len = Some(s.chars().count());
            report.message = keep.then_some(s);
        } else if let Some(symbol) = value.as_symbol() {
            report.value = "symbol";
            report.message = keep.then(|| {
                let description = symbol.description().map(|d| d.to_std_string_escaped()).unwrap_or_default();
                format!("Symbol({description})")
            });
        } else if let Some(object) = value.as_object() {
            if crate::describe::is_error_like(&object, context) {
                report.value = "error";
                let name = crate::describe::error_name(&object);
                report.name = Some(standard_name(name.as_deref()));
                if let Some(message) = crate::describe::error_message(&object) {
                    report.message_len = Some(message.chars().count());
                    report.message = keep.then_some(message);
                }
            } else {
                report.value = match () {
                    _ if object.is_array() => "array",
                    _ if object.is_callable() => "function",
                    _ => "object",
                };
            }
        } else {
            report.value = match () {
                _ if value.is_number() => "number",
                _ if value.is_bigint() => "bigint",
                _ if value.is_boolean() => "boolean",
                _ if value.is_null() => "null",
                _ => "undefined",
            };
            if keep {
                report.message = value.to_string(context).ok().map(|s| s.to_std_string_escaped());
            }
        }
        let own_stack = match value.as_object() {
            Some(object) if object.is::<Error>() => self.stack_of(value, context),
            _ => None,
        };
        let stack = own_stack.as_deref().or(frames).unwrap_or("");
        self.set_frames(&mut report, stack);
        report
    }

    /// `report`'s frames from Boa's stack text, with their function names
    /// when error text is kept.
    fn set_frames(&self, report: &mut ErrorReport, stack: &str) {
        let (frames, functions, located) = self.frames(stack);
        report.frames = frames;
        report.functions = if self.keep_text.get() { functions } else { Vec::new() };
        report.located = located;
    }

    /// An error object's stack as the realm's own getter gives it (frame
    /// lines only), never a `stack` the page defined.
    fn stack_of(&self, value: &JsValue, context: &mut Context) -> Option<String> {
        let getter = self.stack_getter.borrow().clone()?;
        getter
            .call(value, &[], context)
            .ok()
            .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
    }

    /// The frames of Boa's stack text (`    at name (position)` lines) that
    /// are in a script the engine evaluated, as positions, and whether the
    /// innermost of the page's frames is one of them. Which frame is what is
    /// read from its position, the text Boa writes for the frame's source
    /// (after the last ` (`, past any name): a native function (`native`)
    /// and the embedder's own scripts ([`EMBEDDER_PATH`]) are passed over;
    /// any other frame is the page's, and one in no script the engine
    /// evaluated (`eval` code, `eval at :l:c`; a function `eval` code
    /// declared or `new Function` built, an `on*` attribute's handler among
    /// them, `unknown at :l:c`) has no place of its own and is never given
    /// its caller's. A function name never decides which frame is what, and
    /// a line that is not a whole frame is dropped, never copied: a name the
    /// page made can hold a newline. Each kept frame comes with its function
    /// name as Boa wrote it (`None` for top-level code and unnamed
    /// functions).
    fn frames(&self, stack: &str) -> (Vec<ScriptPosition>, Vec<Option<String>>, bool) {
        let known = self.known_paths.borrow();
        let mut frames = Vec::new();
        let mut functions = Vec::new();
        // `None` until the innermost frame that is the page's is seen.
        let mut located = None;
        for line in stack.lines() {
            let Some(frame) = line.strip_prefix("    at ") else { continue };
            let Some(inner) = frame.strip_suffix(')') else { continue };
            let Some(open) = inner.rfind(" (") else { continue };
            let position = &inner[open + 2..];
            let function = function_name(&inner[..open]);
            if position == "native" || position.starts_with("native at ") {
                continue;
            }
            // `path:line:column`, or `path:?:?` where Boa has no position.
            let Some((rest, column)) = position.rsplit_once(':') else { continue };
            let Some((path, line_number)) = rest.rsplit_once(':') else { continue };
            if path == EMBEDDER_PATH {
                continue;
            }
            match (known.contains(path), line_number.parse(), column.parse()) {
                (true, Ok(line), Ok(column)) => {
                    located.get_or_insert(true);
                    if frames.len() < MAX_FRAMES {
                        frames.push(ScriptPosition { path: path.to_string(), line, column });
                        functions.push(function);
                    }
                }
                _ => {
                    located.get_or_insert(false);
                }
            }
        }
        (frames, functions, located.unwrap_or(false))
    }

    /// A `JsError` an evaluation ended in, as a report.
    pub(crate) fn describe_error(&self, error: &boa_engine::JsError, context: &mut Context) -> ErrorReport {
        let full = error.to_string();
        if let Some(value) = error.as_opaque() {
            // Boa's text is the value's display, then one line per frame.
            let shown = value.display().to_string();
            let frames = full.strip_prefix(shown.as_str());
            return self.describe(value, frames, false, context);
        }
        // An error the engine raised (a native error, a runtime limit): as
        // the error object a script would have caught.
        match error.clone().into_opaque(context) {
            Ok(value) => {
                let mut report = self.describe(&value, None, false, context);
                if report.frames.is_empty() {
                    // No backtrace on the object: Boa's text has the frame
                    // lines, or the position, after the error's own name and
                    // message (which can hold any text, newlines and
                    // positions too): read only what follows them.
                    let after = error
                        .as_native()
                        .and_then(|native| {
                            let message = native.message().trim();
                            let head = match message.is_empty() {
                                true => native.kind().to_string(),
                                false => format!("{}: {message}", native.kind()),
                            };
                            full.strip_prefix(head.as_str())
                        })
                        .unwrap_or("");
                    let (at, rest) = after.split_once('\n').unwrap_or((after, ""));
                    self.set_frames(&mut report, rest);
                    if report.frames.is_empty() {
                        if let Some(at) = at.strip_prefix(" (").and_then(|at| at.strip_suffix(')')) {
                            self.set_frames(&mut report, &format!("    at <main> ({at})"));
                        }
                    }
                }
                report
            }
            Err(_) => {
                let head = full.lines().next().unwrap_or("");
                let message = head.split_once(": ").map_or(head, |(_, m)| m);
                ErrorReport {
                    value: "error",
                    name: Some("Error"),
                    message: self.keep_text.get().then(|| message.to_string()),
                    message_len: Some(message.chars().count()),
                    frames: Vec::new(),
                    functions: Vec::new(),
                    located: false,
                    rejection: false,
                    module: None,
                }
            }
        }
    }
}

/// Longest function name kept for a frame, in characters.
const MAX_FUNCTION_CHARS: usize = 128;

/// A frame's function name as Boa writes it: `None` for its placeholders
/// (`<main>` for top-level code, `<anonymous>`, empty), which no page
/// identifier can spell, and for a computed name holding a control
/// character or a parenthesis, which could forge a frame line
/// (`    at f (url:line:col)`) in the stack text.
fn function_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || (name.starts_with('<') && name.ends_with('>')) {
        return None;
    }
    if name.chars().any(|c| c.is_control() || c == '(' || c == ')') {
        return None;
    }
    Some(name.chars().take(MAX_FUNCTION_CHARS).collect())
}

/// The page's frames on the VM's stack, innermost first, as
/// [`caller_position`] reads them: frames in no named script (`eval` code,
/// an `on*` handler) and the embedder's own are passed over. Function names
/// only with `names`. Runs no script.
pub(crate) fn page_stack(context: &mut Context, names: bool) -> Vec<ScriptFrame> {
    use boa_engine::vm::SourcePath;
    let mut frames = Vec::new();
    for frame in context.stack_trace() {
        let location = frame.position();
        let path = match &location.path {
            SourcePath::Path(path) if path.as_os_str() != EMBEDDER_PATH => path.to_string_lossy().into_owned(),
            _ => continue,
        };
        let Some(position) = location.position else { continue };
        let function = match names {
            true => function_name(&location.function_name.to_std_string_escaped()),
            false => None,
        };
        frames.push(ScriptFrame {
            function,
            at: ScriptPosition { path, line: position.line_number(), column: position.column_number() },
        });
        if frames.len() >= MAX_FRAMES {
            break;
        }
    }
    frames
}

/// The position of the innermost script frame on the VM's stack: the
/// caller of the native that asks. When that frame is in no named script
/// (`eval` code, an `on*` attribute's handler) or in the embedder's own
/// ([`EMBEDDER_PATH`]) the answer is `None`, never an outer caller's
/// position; with `skip_unnamed` (the embedder's own script is the caller)
/// such frames are passed over instead. Reads the VM's frames only; runs no
/// script.
pub(crate) fn caller_position(context: &mut Context, skip_unnamed: bool) -> Option<ScriptPosition> {
    use boa_engine::vm::SourcePath;
    for frame in context.stack_trace() {
        let location = frame.position();
        let path = match &location.path {
            SourcePath::Path(path) if path.as_os_str() != EMBEDDER_PATH => path.to_string_lossy().into_owned(),
            _ if skip_unnamed => continue,
            _ => return None,
        };
        let position = location.position?;
        return Some(ScriptPosition {
            path,
            line: position.line_number(),
            column: position.column_number(),
        });
    }
    None
}
