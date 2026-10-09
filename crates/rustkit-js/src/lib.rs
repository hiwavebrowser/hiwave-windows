//! # RustKit JS
//!
//! JavaScript engine integration for the RustKit browser engine.
//!
//! ## Design Goals
//!
//! 1. **Engine abstraction**: Support multiple JS engines (Boa, V8)
//! 2. **Web API compatibility**: console, setTimeout, etc.
//! 3. **Safe interop**: Controlled boundary between Rust and JS
//! 4. **Async support**: Event loop integration

mod import_map;
#[cfg(feature = "boa")]
mod module;
#[cfg(feature = "boa")]
pub use module::{FetchedModule, ModuleHandle, ModuleState};
#[cfg(feature = "boa")]
mod executor;
#[cfg(all(test, feature = "boa"))]
mod module_tests;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info, trace};

/// Errors that can occur in JS operations.
#[derive(Error, Debug)]
pub enum JsError {
    #[error("Execution error: {0}")]
    ExecutionError(String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Type error: {0}")]
    TypeError(String),

    #[error("Engine not initialized")]
    NotInitialized,
}

/// Unique identifier for a timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimerId(u64);

impl TimerId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn raw(&self) -> u64 {
        self.0
    }
}

/// A JavaScript value.
#[derive(Debug, Clone)]
pub enum JsValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(String),
    Object,
    Array,
    Function,
}

impl JsValue {
    pub fn is_truthy(&self) -> bool {
        match self {
            JsValue::Undefined | JsValue::Null => false,
            JsValue::Boolean(b) => *b,
            JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
            JsValue::String(s) => !s.is_empty(),
            _ => true,
        }
    }
}

/// Console log levels.
#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

/// Console output handler.
pub type ConsoleHandler = Box<dyn Fn(LogLevel, &str) + Send + Sync>;

/// Timer callback.
pub type TimerCallback = Box<dyn FnOnce() + Send + 'static>;

/// A Rust function callable from script. It sees its arguments as plain
/// values (objects arrive as the payload-less `Object`/`Array`/`Function`),
/// and whatever it returns other than a primitive reaches script as
/// `undefined`. That keeps every Boa-managed value on the Boa side: a host
/// function can capture Rust state freely but can never hold a JS object.
pub type HostFunction = Box<dyn Fn(&[JsValue]) -> JsValue>;

/// Pending timer.
#[allow(dead_code)]
struct PendingTimer {
    callback: String, // JS code to execute
    delay: Duration,
    repeat: bool,
}

/// Default maximum number of loop iterations/pumps per `run_jobs` call.
pub const DEFAULT_MAX_JOB_ITERATIONS: u64 = 10_000;

/// JavaScript runtime configuration.
#[derive(Clone, Debug)]
pub struct JsRuntimeConfig {
    /// Enable strict mode.
    pub strict_mode: bool,
    /// Maximum execution time for script evaluation and job processing.
    pub timeout: Option<Duration>,
    /// Maximum number of job iterations per `run_jobs` call before halting runaway recursion.
    pub max_job_iterations: u64,
}

impl Default for JsRuntimeConfig {
    fn default() -> Self {
        Self {
            strict_mode: false,
            timeout: None,
            max_job_iterations: DEFAULT_MAX_JOB_ITERATIONS,
        }
    }
}

/// A wall-clock time after which the host stops answering script (see
/// [`JsRuntime::set_execution_deadline`]), and whether that has happened.
#[derive(Default)]
struct ExecutionDeadline {
    at: std::cell::Cell<Option<std::time::Instant>>,
    hit: std::cell::Cell<bool>,
    /// How long after `at` the first refused host call came.
    late: std::cell::Cell<Option<std::time::Duration>>,
}

/// JavaScript runtime that wraps the underlying engine.
pub struct JsRuntime {
    #[cfg(feature = "boa")]
    context: boa_engine::Context,
    /// The module host (see `module`): imports the graph asks for are
    /// recorded here for the embedder to fetch.
    #[cfg(feature = "boa")]
    modules: module::ModuleHost,
    #[cfg(feature = "boa")]
    executor: std::rc::Rc<executor::HostJobExecutor>,
    config: JsRuntimeConfig,
    console_handler: Option<Arc<ConsoleHandler>>,
    timers: Arc<Mutex<HashMap<TimerId, PendingTimer>>>,
    globals: HashMap<String, JsValue>,
    deadline: std::rc::Rc<ExecutionDeadline>,
}

impl JsRuntime {
    /// Create a new JavaScript runtime.
    pub fn new() -> Result<Self, JsError> {
        Self::with_config(JsRuntimeConfig::default())
    }

    /// Create a new JavaScript runtime with configuration.
    pub fn with_config(config: JsRuntimeConfig) -> Result<Self, JsError> {
        info!("Initializing JavaScript runtime");

        #[cfg(feature = "boa")]
        let modules = module::ModuleHost::default();
        #[cfg(feature = "boa")]
        let executor = std::rc::Rc::new(executor::HostJobExecutor::with_limits(
            config.max_job_iterations,
            config.timeout,
        ));
        #[cfg(feature = "boa")]
        let mut context = boa_engine::Context::builder()
            .module_loader(modules.loader())
            .job_executor(executor.clone())
            .build()
            .map_err(|e| JsError::ExecutionError(e.to_string()))?;

        #[cfg(feature = "boa")]
        context.strict(config.strict_mode);

        let mut runtime = Self {
            #[cfg(feature = "boa")]
            context,
            #[cfg(feature = "boa")]
            modules,
            #[cfg(feature = "boa")]
            executor,
            config,
            console_handler: None,
            timers: Arc::new(Mutex::new(HashMap::new())),
            globals: HashMap::new(),
            deadline: std::rc::Rc::default(),
        };

        // Set up built-in APIs
        runtime.setup_console()?;

        debug!("JavaScript runtime initialized");
        Ok(runtime)
    }

    /// Get the runtime configuration.
    pub fn config(&self) -> &JsRuntimeConfig {
        &self.config
    }

    /// Set the microtask job iteration limit per `run_jobs` turn.
    pub fn set_max_job_iterations(&mut self, limit: u64) {
        self.config.max_job_iterations = limit;
        #[cfg(feature = "boa")]
        self.executor.set_max_job_iterations(limit);
    }

    /// Set the timeout for job execution.
    pub fn set_job_timeout(&mut self, timeout: Option<Duration>) {
        self.config.timeout = timeout;
        #[cfg(feature = "boa")]
        self.executor.set_timeout(timeout);
    }

    /// Run all currently pending jobs (microtasks and async completions).
    pub fn run_jobs(&mut self) -> Result<(), JsError> {
        #[cfg(feature = "boa")]
        {
            self.context
                .run_jobs()
                .map_err(|e| JsError::ExecutionError(e.to_string()))
        }
        #[cfg(not(feature = "boa"))]
        {
            Ok(())
        }
    }

    /// Set the console output handler.
    pub fn set_console_handler(&mut self, handler: ConsoleHandler) {
        self.console_handler = Some(Arc::new(handler));
    }

    /// Set up console API.
    fn setup_console(&mut self) -> Result<(), JsError> {
        // Console is set up via evaluate_script with native function bindings
        // For now, we'll inject a simple console object
        let console_script = r#"
            var console = {
                _logs: [],
                log: function() {
                    this._logs.push({level: 'log', args: Array.from(arguments)});
                },
                info: function() {
                    this._logs.push({level: 'info', args: Array.from(arguments)});
                },
                warn: function() {
                    this._logs.push({level: 'warn', args: Array.from(arguments)});
                },
                error: function() {
                    this._logs.push({level: 'error', args: Array.from(arguments)});
                },
                debug: function() {
                    this._logs.push({level: 'debug', args: Array.from(arguments)});
                },
                _flush: function() {
                    var logs = this._logs;
                    this._logs = [];
                    return logs;
                }
            };
        "#;

        self.evaluate_script(console_script)?;
        Ok(())
    }

    /// Evaluate JavaScript code.
    pub fn evaluate_script(&mut self, source: &str) -> Result<JsValue, JsError> {
        trace!(len = source.len(), "Evaluating script");

        #[cfg(feature = "boa")]
        {
            use boa_engine::Source;

            let result = self.context.eval(Source::from_bytes(source));
            // Promise reactions (`.then`, `await`) are jobs Boa queues but
            // does not run on its own; a page's async code never resumes
            // without this.
            let job_result = self.context.run_jobs();

            match result {
                Ok(value) => {
                    self.flush_console_logs();
                    if let Err(job_err) = job_result {
                        let msg = job_err.to_string();
                        // Only promote job queue limit or timeout breaches (runaway RangeError policy)
                        // to script failure; ordinary microtask rejections are handled by Promise rejection events
                        // and do not fail the synchronous script value that already completed successfully.
                        if msg.contains("Job queue") || msg.contains("limit") || msg.contains("timeout") {
                            return Err(JsError::ExecutionError(msg));
                        }
                    }
                    let js_value = self.convert_boa_value(&value);
                    Ok(js_value)
                }
                Err(err) => {
                    self.flush_console_logs();
                    let msg = err.to_string();
                    Err(JsError::ExecutionError(msg))
                }
            }
        }

        #[cfg(not(feature = "boa"))]
        {
            Err(JsError::NotInitialized)
        }
    }

    /// Bound how long any one loop may run: past `max_iterations` it throws
    /// an error the script cannot catch. Page scripts are untrusted, and
    /// Boa has no wall-clock interrupt, so this is what stops a
    /// `while (true) {}` from hanging the engine.
    pub fn set_loop_iteration_limit(&mut self, max_iterations: u64) {
        #[cfg(feature = "boa")]
        self.context
            .runtime_limits_mut()
            .set_loop_iteration_limit(max_iterations);
        #[cfg(not(feature = "boa"))]
        let _ = max_iterations;
    }

    /// Stop script that is still running at `at`: from then on every host
    /// function fails, before it does anything, with an error the script
    /// cannot catch, so the whole call stack unwinds to whoever started
    /// it. `None` lifts it. Boa has no wall-clock interrupt of its own and
    /// the loop limit counts loop statements only; a script inside nested
    /// `forEach` callbacks is stopped by this and by nothing else. Script
    /// that never calls the host is not stopped.
    pub fn set_execution_deadline(&mut self, at: Option<std::time::Instant>) {
        self.deadline.at.set(at);
        self.deadline.hit.set(false);
        self.deadline.late.set(None);
    }

    /// Whether the deadline has stopped a host call since it was last set
    /// or asked about.
    pub fn take_deadline_hit(&mut self) -> bool {
        self.deadline.hit.replace(false)
    }

    /// How long after the deadline the host call that stopped script came,
    /// once per stop. Script is only stopped where it calls the host, so
    /// this is the time it ran on past its budget.
    pub fn take_deadline_overrun(&mut self) -> Option<std::time::Duration> {
        self.deadline.late.take()
    }

    #[cfg(feature = "boa")]
    fn drain_console(&mut self) -> Vec<(LogLevel, String)> {
        use boa_engine::Source;

        // Evaluate console._flush() directly on the context without calling
        // evaluate_script, preventing infinite recursion.
        let Ok(logs_val) = self.context.eval(Source::from_bytes("console._flush()")) else {
            return Vec::new();
        };

        let Some(logs_obj) = logs_val.as_object() else {
            return Vec::new();
        };

        let Ok(len_val) = logs_obj.get(boa_engine::js_string!("length"), &mut self.context) else {
            return Vec::new();
        };

        let Some(len) = len_val.as_number() else {
            return Vec::new();
        };

        if len < 0.0 || !len.is_finite() {
            return Vec::new();
        }

        const MAX_FLUSH_ENTRIES: u32 = 10_000;
        const MAX_FLUSH_ARGS: u32 = 256;

        let count = (len as u32).min(MAX_FLUSH_ENTRIES);
        let mut entries = Vec::with_capacity(count as usize);

        for i in 0..count {
            let Ok(entry_val) = logs_obj.get(i, &mut self.context) else {
                continue;
            };
            let Some(entry_obj) = entry_val.as_object() else {
                continue;
            };

            let level_str = entry_obj
                .get(boa_engine::js_string!("level"), &mut self.context)
                .ok()
                .and_then(|v| v.as_string().map(|s| s.to_std_string_escaped()))
                .unwrap_or_else(|| "log".to_string());

            let level = match level_str.as_str() {
                "info" => LogLevel::Info,
                "warn" => LogLevel::Warn,
                "error" => LogLevel::Error,
                "debug" => LogLevel::Debug,
                _ => LogLevel::Log,
            };

            let mut msg_parts = Vec::new();
            if let Ok(args_val) = entry_obj.get(boa_engine::js_string!("args"), &mut self.context) {
                if let Some(args_obj) = args_val.as_object() {
                    if let Ok(args_len_val) = args_obj.get(boa_engine::js_string!("length"), &mut self.context) {
                        let args_count = (args_len_val.as_number().unwrap_or(0.0).max(0.0) as u32).min(MAX_FLUSH_ARGS);
                        for arg_idx in 0..args_count {
                            if let Ok(arg) = args_obj.get(arg_idx, &mut self.context) {
                                let s = arg
                                    .to_string(&mut self.context)
                                    .map(|js_s| js_s.to_std_string_escaped())
                                    .unwrap_or_else(|_| "[object]".to_string());
                                msg_parts.push(s);
                            }
                        }
                    }
                }
            }

            entries.push((level, msg_parts.join(" ")));
        }

        entries
    }

    /// Flush console logs and call handler.
    fn flush_console_logs(&mut self) {
        let Some(handler) = self.console_handler.clone() else {
            return;
        };

        #[cfg(feature = "boa")]
        {
            let logs = self.drain_console();
            for (level, msg) in logs {
                handler(level, &msg);
            }
        }

        #[cfg(not(feature = "boa"))]
        {
            let _ = handler;
        }
    }

    /// Define a global function `name` that calls `function`.
    pub fn register_host_function(
        &mut self,
        name: &str,
        length: usize,
        function: HostFunction,
    ) -> Result<(), JsError> {
        #[cfg(feature = "boa")]
        {
            use boa_engine::{JsString, NativeFunction};

            // SAFETY: `HostFunction` only ever sees and returns the
            // crate's own `JsValue`, which holds no GC-managed data, so the
            // closure captures nothing the collector would need to trace.
            let deadline = self.deadline.clone();
            let native = unsafe {
                NativeFunction::from_closure(move |_this, args, _context| {
                    // No deadline (the default): the clock is not read.
                    let passed = deadline.at.get().and_then(|at| {
                        let now = std::time::Instant::now();
                        (now >= at).then(|| now - at)
                    });
                    if let Some(late) = passed {
                        // Logged here, at the first refusal, and not only
                        // by whoever started the script: if the stack then
                        // takes long to unwind, the log still says when.
                        if !deadline.hit.replace(true) {
                            deadline.late.set(Some(late));
                            tracing::warn!(
                                late_ms = late.as_millis() as u64,
                                "Script budget spent: host calls are refused until the script has unwound"
                            );
                        }
                        // Boa's runtime-limit errors are the ones script
                        // cannot catch, and it has none for time; `hit`
                        // says which limit this was.
                        return Err(boa_engine::error::RuntimeLimitError::LoopIteration.into());
                    }
                    let args: Vec<JsValue> = args.iter().map(from_boa_value).collect();
                    Ok(to_boa_value(function(&args)))
                })
            };
            self.context
                .register_global_callable(JsString::from(name), length, native)
                .map_err(|e| JsError::ExecutionError(e.to_string()))
        }

        #[cfg(not(feature = "boa"))]
        {
            let _ = (name, length, function);
            Err(JsError::NotInitialized)
        }
    }

    /// Convert Boa value to JsValue.
    #[cfg(feature = "boa")]
    fn convert_boa_value(&self, value: &boa_engine::JsValue) -> JsValue {
        from_boa_value(value)
    }

    /// Set a global variable.
    pub fn set_global(&mut self, name: &str, value: JsValue) -> Result<(), JsError> {
        self.globals.insert(name.to_string(), value.clone());

        // Set in the JS context
        let js_code = match value {
            JsValue::Undefined => format!("var {} = undefined;", name),
            JsValue::Null => format!("var {} = null;", name),
            JsValue::Boolean(b) => format!("var {} = {};", name, b),
            JsValue::Number(n) => format!("var {} = {};", name, n),
            JsValue::String(s) => format!("var {} = {:?};", name, s),
            _ => return Ok(()), // Complex types handled differently
        };

        self.evaluate_script(&js_code)?;
        Ok(())
    }

    /// Get a global variable.
    pub fn get_global(&mut self, name: &str) -> Result<JsValue, JsError> {
        self.evaluate_script(name)
    }

    /// Schedule a timeout (setTimeout equivalent).
    pub fn set_timeout(&mut self, code: &str, delay_ms: u32) -> TimerId {
        let id = TimerId::new();
        let timer = PendingTimer {
            callback: code.to_string(),
            delay: Duration::from_millis(delay_ms as u64),
            repeat: false,
        };

        self.timers.lock().unwrap().insert(id, timer);
        trace!(?id, delay_ms, "Timeout scheduled");
        id
    }

    /// Schedule an interval (setInterval equivalent).
    pub fn set_interval(&mut self, code: &str, interval_ms: u32) -> TimerId {
        let id = TimerId::new();
        let timer = PendingTimer {
            callback: code.to_string(),
            delay: Duration::from_millis(interval_ms as u64),
            repeat: true,
        };

        self.timers.lock().unwrap().insert(id, timer);
        trace!(?id, interval_ms, "Interval scheduled");
        id
    }

    /// Cancel a timeout or interval.
    pub fn clear_timer(&mut self, id: TimerId) {
        self.timers.lock().unwrap().remove(&id);
        trace!(?id, "Timer cleared");
    }

    /// Get pending timers that are due.
    pub fn get_due_timers(&self) -> Vec<(TimerId, String, bool)> {
        let timers = self.timers.lock().unwrap();
        timers
            .iter()
            .map(|(id, t)| (*id, t.callback.clone(), t.repeat))
            .collect()
    }

    /// Execute a timer callback.
    pub fn execute_timer(&mut self, id: TimerId) -> Result<(), JsError> {
        let timer = {
            let timers = self.timers.lock().unwrap();
            timers.get(&id).map(|t| (t.callback.clone(), t.repeat))
        };

        if let Some((callback, repeat)) = timer {
            self.evaluate_script(&callback)?;

            if !repeat {
                self.timers.lock().unwrap().remove(&id);
            }
        }

        Ok(())
    }

    /// Check if a global variable exists.
    pub fn has_global(&mut self, name: &str) -> bool {
        let check = format!("typeof {} !== 'undefined'", name);
        matches!(self.evaluate_script(&check), Ok(JsValue::Boolean(true)))
    }
}

#[cfg(feature = "boa")]
fn to_boa_value(value: JsValue) -> boa_engine::JsValue {
    use boa_engine::{JsString, JsValue as BoaValue};

    match value {
        JsValue::Null => BoaValue::null(),
        JsValue::Boolean(b) => BoaValue::from(b),
        JsValue::Number(n) => BoaValue::from(n),
        JsValue::String(s) => BoaValue::from(JsString::from(s.as_str())),
        JsValue::Undefined | JsValue::Object | JsValue::Array | JsValue::Function => {
            BoaValue::undefined()
        }
    }
}

#[cfg(feature = "boa")]
fn from_boa_value(value: &boa_engine::JsValue) -> JsValue {
    if value.is_undefined() {
        JsValue::Undefined
    } else if value.is_null() {
        JsValue::Null
    } else if let Some(b) = value.as_boolean() {
        JsValue::Boolean(b)
    } else if let Some(n) = value.as_number() {
        JsValue::Number(n)
    } else if let Some(s) = value.as_string() {
        JsValue::String(s.to_std_string_escaped())
    } else if let Some(obj) = value.as_object() {
        if obj.is_array() {
            JsValue::Array
        } else if obj.is_callable() {
            JsValue::Function
        } else {
            JsValue::Object
        }
    } else {
        JsValue::Undefined
    }
}

impl Default for JsRuntime {
    fn default() -> Self {
        Self::new().expect("Failed to create default JsRuntime")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_evaluation() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("1 + 2").unwrap();
        assert!(matches!(result, JsValue::Number(n) if (n - 3.0).abs() < f64::EPSILON));
    }

    #[test]
    fn test_string_evaluation() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("'hello' + ' world'").unwrap();
        assert!(matches!(result, JsValue::String(s) if s == "hello world"));
    }

    #[test]
    fn test_boolean_evaluation() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("true && false").unwrap();
        assert!(matches!(result, JsValue::Boolean(false)));
    }

    #[test]
    fn test_global_variable() {
        let mut runtime = JsRuntime::new().unwrap();

        runtime
            .set_global("testVar", JsValue::Number(42.0))
            .unwrap();
        let result = runtime.evaluate_script("testVar * 2").unwrap();
        assert!(matches!(result, JsValue::Number(n) if (n - 84.0).abs() < f64::EPSILON));
    }

    #[test]
    fn test_console_exists() {
        let mut runtime = JsRuntime::new().unwrap();
        assert!(runtime.has_global("console"));
    }

    #[test]
    fn test_console_log() {
        let mut runtime = JsRuntime::new().unwrap();

        // Should not error
        runtime.evaluate_script("console.log('test')").unwrap();
    }

    #[test]
    fn test_timer_scheduling() {
        let mut runtime = JsRuntime::new().unwrap();

        let id1 = runtime.set_timeout("console.log('timeout')", 100);
        let id2 = runtime.set_interval("console.log('interval')", 50);

        assert_ne!(id1, id2);

        let timers = runtime.get_due_timers();
        assert_eq!(timers.len(), 2);

        runtime.clear_timer(id1);
        let timers = runtime.get_due_timers();
        assert_eq!(timers.len(), 1);
    }

    #[test]
    fn past_the_execution_deadline_a_host_call_unwinds_script_that_cannot_catch_it() {
        let mut runtime = JsRuntime::new().unwrap();
        let calls = std::rc::Rc::new(std::cell::Cell::new(0u32));
        let seen = calls.clone();
        runtime
            .register_host_function(
                "host",
                0,
                Box::new(move |_| {
                    seen.set(seen.get() + 1);
                    JsValue::Undefined
                }),
            )
            .unwrap();
        runtime
            .evaluate_script("var reached = [], caught = false;")
            .unwrap();
        let spin = "[1, 2, 3].forEach(function (n) { try { host(); } catch (e) { caught = true; } reached.push(n); });";

        // No deadline, and one that has not come: the host answers.
        runtime.evaluate_script(spin).unwrap();
        runtime.set_execution_deadline(Some(std::time::Instant::now() + Duration::from_secs(60)));
        runtime.evaluate_script(spin).unwrap();
        assert_eq!(calls.get(), 6);
        assert!(!runtime.take_deadline_hit());

        // Past it: the first host call ends the script, through the
        // `forEach` and the `try`, before the host function runs.
        runtime.set_execution_deadline(Some(std::time::Instant::now()));
        assert!(runtime.evaluate_script(spin).is_err());
        assert_eq!(calls.get(), 6);
        assert!(runtime.take_deadline_hit());
        assert!(!runtime.take_deadline_hit());

        // The stop comes where script next calls the host, and says how
        // late that was: this script computes for a while first.
        runtime.set_execution_deadline(Some(std::time::Instant::now()));
        assert_eq!(runtime.take_deadline_overrun(), None);
        let busy = "var t = Date.now(); [1].forEach(function () { while (Date.now() - t < 30) {} host(); });";
        assert!(runtime.evaluate_script(busy).is_err());
        let late = runtime.take_deadline_overrun().expect("the stop reports how late it came");
        // `Date.now()` counts whole milliseconds, so the 30 ms loop is a
        // little under 30 ms of wall clock.
        assert!(late >= Duration::from_millis(20), "{late:?}");
        assert_eq!(runtime.take_deadline_overrun(), None);
        assert!(runtime.take_deadline_hit());

        // Lifted: the runtime runs script again, and nothing was caught.
        runtime.set_execution_deadline(None);
        let result = runtime.evaluate_script("host(); caught === false && reached.length === 6").unwrap();
        assert!(matches!(result, JsValue::Boolean(true)), "{result:?}");
        assert_eq!(calls.get(), 7);
    }

    #[test]
    fn test_function_execution() {
        let mut runtime = JsRuntime::new().unwrap();

        runtime
            .evaluate_script("function add(a, b) { return a + b; }")
            .unwrap();
        let result = runtime.evaluate_script("add(2, 3)").unwrap();
        assert!(matches!(result, JsValue::Number(n) if (n - 5.0).abs() < f64::EPSILON));
    }

    #[test]
    fn test_object_creation() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("({ name: 'test' })").unwrap();
        assert!(matches!(result, JsValue::Object));
    }

    #[test]
    fn test_array_creation() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("[1, 2, 3]").unwrap();
        assert!(matches!(result, JsValue::Array));
    }

    #[test]
    fn a_runaway_loop_throws_instead_of_hanging() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_loop_iteration_limit(10_000);
        let result = runtime.evaluate_script("try { while (true) {} } catch (e) {} 'caught'");
        assert!(result.is_err(), "the limit must not be catchable: {result:?}");
        // The runtime is still usable afterwards.
        let after = runtime.evaluate_script("1 + 1").unwrap();
        assert!(matches!(after, JsValue::Number(n) if n == 2.0));
    }

    #[test]
    fn promise_reactions_run() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .evaluate_script("var done = false; Promise.resolve().then(function() { done = true; });")
            .unwrap();
        let done = runtime.evaluate_script("done").unwrap();
        assert!(matches!(done, JsValue::Boolean(true)));
    }

    #[test]
    fn a_runaway_microtask_chain_throws_instead_of_hanging() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime.set_max_job_iterations(500);
        let result = runtime.evaluate_script("function again() { Promise.resolve().then(again); } again();");
        assert!(result.is_err(), "runaway microtask chain must error instead of hanging: {result:?}");
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Job queue") || err_msg.contains("limit"), "error must mention limit: {err_msg}");
        // The runtime is still usable afterwards.
        let after = runtime.evaluate_script("1 + 1").unwrap();
        assert!(matches!(after, JsValue::Number(n) if n == 2.0));
    }

    #[test]
    fn strict_mode_in_config_is_respected() {
        let mut strict_rt = JsRuntime::with_config(JsRuntimeConfig {
            strict_mode: true,
            ..Default::default()
        }).unwrap();
        let result = strict_rt.evaluate_script("undeclaredVar = 42;");
        assert!(result.is_err(), "assignment to undeclared variable must fail in strict mode");

        let mut non_strict_rt = JsRuntime::with_config(JsRuntimeConfig {
            strict_mode: false,
            ..Default::default()
        }).unwrap();
        let result2 = non_strict_rt.evaluate_script("undeclaredVar = 42;");
        assert!(result2.is_ok(), "assignment to undeclared variable succeeds in non-strict mode");
    }

    #[test]
    fn job_timeout_in_config_is_respected() {
        let mut runtime = JsRuntime::with_config(JsRuntimeConfig {
            timeout: Some(Duration::from_millis(20)),
            max_job_iterations: 1_000_000,
            ..Default::default()
        }).unwrap();
        // Infinite recursion with timeout
        let result = runtime.evaluate_script("function loop() { Promise.resolve().then(loop); } loop();");
        assert!(result.is_err(), "must timeout rather than hanging");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("timeout") || err.contains("limit"), "must report timeout or limit: {err}");
        // Runtime remains usable
        let after = runtime.evaluate_script("40 + 2").unwrap();
        assert!(matches!(after, JsValue::Number(n) if n == 42.0));
    }

    #[test]
    fn host_functions_take_and_return_primitives() {
        let mut runtime = JsRuntime::new().unwrap();
        runtime
            .register_host_function(
                "__host_echo",
                2,
                Box::new(|args| match args {
                    [JsValue::String(s), JsValue::Number(n)] => {
                        JsValue::String(format!("{s}:{n}"))
                    }
                    [JsValue::Object] => JsValue::Object,
                    _ => JsValue::Null,
                }),
            )
            .unwrap();
        let echoed = runtime.evaluate_script("__host_echo('a', 2)").unwrap();
        assert!(matches!(echoed, JsValue::String(s) if s == "a:2"));
        let none = runtime.evaluate_script("__host_echo()").unwrap();
        assert!(matches!(none, JsValue::Null));
        // A non-primitive return reaches script as undefined.
        let object = runtime.evaluate_script("typeof __host_echo({})").unwrap();
        assert!(matches!(object, JsValue::String(s) if s == "undefined"));
    }

    #[test]
    fn test_error_handling() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("nonexistent.property");
        assert!(result.is_err());
    }

    #[test]
    fn console_handler_receives_flushed_logs_without_recursion() {
        let mut runtime = JsRuntime::new().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recv_clone = received.clone();
        runtime.set_console_handler(Box::new(move |level, msg| {
            recv_clone.lock().unwrap().push((format!("{level:?}"), msg.to_string()));
        }));

        runtime.evaluate_script("console.log('hello', 'world'); console.warn('caution');").unwrap();

        let logs = received.lock().unwrap().clone();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0], ("Log".to_string(), "hello world".to_string()));
        assert_eq!(logs[1], ("Warn".to_string(), "caution".to_string()));

        // Subsequent script evaluation only delivers new logs (buffer was flushed)
        runtime.evaluate_script("console.error('oops');").unwrap();
        let logs2 = received.lock().unwrap().clone();
        assert_eq!(logs2.len(), 3);
        assert_eq!(logs2[2], ("Error".to_string(), "oops".to_string()));
    }

    #[test]
    fn console_flush_handles_throwing_tostring_gracefully() {
        let mut runtime = JsRuntime::new().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recv_clone = received.clone();
        runtime.set_console_handler(Box::new(move |_level, msg| {
            recv_clone.lock().unwrap().push(msg.to_string());
        }));

        runtime
            .evaluate_script("console.log('val:', { toString() { throw new Error('boom'); } });")
            .unwrap();

        let logs = received.lock().unwrap().clone();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0], "val: [object]");
    }

    #[test]
    fn console_flush_bounded_against_hostile_length() {
        let mut runtime = JsRuntime::new().unwrap();
        let received = Arc::new(Mutex::new(Vec::new()));
        let recv_clone = received.clone();
        runtime.set_console_handler(Box::new(move |_level, msg| {
            recv_clone.lock().unwrap().push(msg.to_string());
        }));

        // Hostile script overwriting console._flush with huge length
        runtime
            .evaluate_script("console._flush = () => ({ length: 4294967295 });")
            .unwrap();

        // Evaluation completes without hanging or OOM
        let result = runtime.evaluate_script("1 + 1");
        assert!(result.is_ok());
    }
}
