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

/// Pending timer.
#[allow(dead_code)]
struct PendingTimer {
    callback: String, // JS code to execute
    delay: Duration,
    repeat: bool,
}

/// JavaScript runtime configuration.
#[derive(Default)]
pub struct JsRuntimeConfig {
    /// Enable strict mode.
    pub strict_mode: bool,
    /// Maximum execution time.
    pub timeout: Option<Duration>,
}

/// JavaScript runtime that wraps the underlying engine.
pub struct JsRuntime {
    #[cfg(feature = "boa")]
    context: boa_engine::Context,
    console_handler: Option<Arc<ConsoleHandler>>,
    timers: Arc<Mutex<HashMap<TimerId, PendingTimer>>>,
    globals: HashMap<String, JsValue>,
}

impl JsRuntime {
    /// Create a new JavaScript runtime.
    pub fn new() -> Result<Self, JsError> {
        Self::with_config(JsRuntimeConfig::default())
    }

    /// Create a new JavaScript runtime with configuration.
    pub fn with_config(_config: JsRuntimeConfig) -> Result<Self, JsError> {
        info!("Initializing JavaScript runtime");

        #[cfg(feature = "boa")]
        let context = boa_engine::Context::default();

        let mut runtime = Self {
            #[cfg(feature = "boa")]
            context,
            console_handler: None,
            timers: Arc::new(Mutex::new(HashMap::new())),
            globals: HashMap::new(),
        };

        // Set up built-in APIs
        runtime.setup_console()?;

        debug!("JavaScript runtime initialized");
        Ok(runtime)
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
            self.context.run_jobs();

            match result {
                Ok(value) => {
                    let js_value = self.convert_boa_value(&value);
                    self.flush_console_logs();
                    Ok(js_value)
                }
                Err(err) => {
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

    /// Flush console logs and call handler.
    fn flush_console_logs(&mut self) {
        if self.console_handler.is_none() {
            return;
        }

        let _flush_result = self.evaluate_script("console._flush()");
        // Note: In a real implementation, we'd parse the returned array
        // and call the console handler for each log entry
    }

    /// Convert Boa value to JsValue.
    #[cfg(feature = "boa")]
    fn convert_boa_value(&self, value: &boa_engine::JsValue) -> JsValue {
        use boa_engine::JsValue as BoaValue;

        match value {
            BoaValue::Undefined => JsValue::Undefined,
            BoaValue::Null => JsValue::Null,
            BoaValue::Boolean(b) => JsValue::Boolean(*b),
            BoaValue::Integer(n) => JsValue::Number(*n as f64),
            BoaValue::Rational(n) => JsValue::Number(*n),
            BoaValue::String(s) => JsValue::String(s.to_std_string_escaped()),
            BoaValue::Object(obj) => {
                if obj.is_array() {
                    JsValue::Array
                } else if obj.is_callable() {
                    JsValue::Function
                } else {
                    JsValue::Object
                }
            }
            _ => JsValue::Undefined,
        }
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
    fn test_error_handling() {
        let mut runtime = JsRuntime::new().unwrap();

        let result = runtime.evaluate_script("nonexistent.property");
        assert!(result.is_err());
    }
}
