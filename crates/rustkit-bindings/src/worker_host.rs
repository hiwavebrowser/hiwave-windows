//! Dedicated workers (STR-4). A worker is one OS thread with a JsRuntime of
//! its own: a Boa context cannot move between threads, and a worker must not
//! share the page's objects anyway. The page and the worker talk through
//! channels carrying strings; the page side (web_worker.js) drains the
//! worker's channel on the timer pump, so every page callback still runs on
//! the page thread.
//!
//! `terminate()` stops a worker that never returns through the view-control
//! interrupt word the page's own watchdog uses: Boa reads it at every call
//! and loop back-edge, so a `while (true) {}` ends. The same happens to every
//! worker of a page when its bindings are dropped (a navigation).

use rustkit_js::{JsRuntime, JsValue, KeptFunction, ViewControl, Watchdog};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const PRELUDE: &str = include_str!("worker_prelude.js");
const STRUCTURED_CLONE: &str = include_str!("structured_clone.js");

/// A worker's turn is bounded by the interrupt word, not by a budget.
const TURN_BUDGET: Duration = Duration::from_secs(60 * 60 * 24);

/// At most this many workers a page may have at once.
const MAX_WORKERS: usize = 64;

/// At most this many workers' messages and events reach the page in one turn, between all its workers
/// (Chrome delivers one a task): workers that flood the page cannot hold a turn. web_worker.js's pump
/// carries the turn's budget across workers.
const MAX_ITEMS_PER_POLL: usize = 256;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Names of workers whose threads have returned (tests read it).
#[cfg(test)]
pub(crate) static ENDED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

pub(crate) enum FromWorker {
    Message(String),
    /// An uncaught error, as JSON `{m, f, l, c}`.
    Error(String),
    /// `importScripts`: the worker waits for the page's answer.
    Fetch { rid: u64, urls: String },
}

struct Handle {
    to: Sender<String>,
    reply: Sender<String>,
    from: Receiver<FromWorker>,
    control: Arc<ViewControl>,
    thread: JoinHandle<()>,
    /// Tests put items on the worker-to-page channel themselves (items no worker can produce).
    #[cfg(test)]
    inject: Sender<FromWorker>,
}

/// One page's workers. The host functions share it; the page's bindings
/// hold a [`Guard`], whose drop stops every worker (the JS context may keep
/// the host functions, and so this, alive past the page).
#[derive(Default)]
struct Registry {
    workers: RefCell<HashMap<u64, Handle>>,
    /// Terminated workers whose threads have not ended yet.
    ending: RefCell<Vec<JoinHandle<()>>>,
}

impl Registry {
    fn running(&self) -> usize {
        self.ending.borrow_mut().retain(|t| !t.is_finished());
        self.ending.borrow().len() + self.workers.borrow().values().filter(|h| !h.thread.is_finished()).count()
    }
}

impl Registry {
    fn stop_all(&self) {
        for (_, h) in self.workers.borrow_mut().drain() {
            h.control.stop_all();
            // `to` drops here, waking a worker that waits for a message. The
            // thread is not joined: the page must not wait on script.
            self.ending.borrow_mut().push(h.thread);
        }
    }
}

/// Stops the page's workers when the page's bindings go away.
pub struct Guard(Rc<Registry>);

impl Guard {
    /// Tests: put `item` on every live worker's worker-to-page channel, as the next thing it sends.
    #[cfg(test)]
    pub(crate) fn inject_all(&self, item: impl Fn() -> FromWorker) {
        for h in self.0.workers.borrow().values() {
            let _ = h.inject.send(item());
        }
    }

    /// How many of the page's worker threads are still running, terminated ones that have not ended
    /// included.
    pub(crate) fn running(&self) -> usize {
        self.0.running()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.stop_all();
    }
}

fn arg_str(args: &[JsValue], i: usize) -> String {
    match args.get(i) {
        Some(JsValue::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn arg_id(args: &[JsValue]) -> u64 {
    match args.first() {
        Some(JsValue::Number(n)) if *n >= 0.0 => *n as u64,
        _ => 0,
    }
}

fn item(m: FromWorker) -> Value {
    match m {
        FromWorker::Message(d) => json!({"k": "m", "d": d}),
        FromWorker::Error(m) => json!({"k": "e", "m": m}),
        FromWorker::Fetch { rid, urls } => json!({"k": "f", "r": rid, "u": urls}),
    }
}

/// Install `__rustkit_worker_*` on the page runtime.
pub fn install(runtime: &mut JsRuntime) -> Result<Guard, rustkit_js::JsError> {
    let reg = Rc::new(Registry::default());

    // spawn(source, name, locationJson) -> id, 0 when refused
    let r = reg.clone();
    runtime.register_host_function(
        "__rustkit_worker_spawn",
        3,
        Box::new(move |args| {
            if r.workers.borrow().len() >= MAX_WORKERS {
                return JsValue::Number(0.0);
            }
            let (source, name, loc) = (arg_str(args, 0), arg_str(args, 1), arg_str(args, 2));
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let (to, rx) = channel();
            let (reply, reply_rx) = channel();
            let (tx, from) = channel();
            #[cfg(test)]
            let inject = tx.clone();
            let control = Watchdog::shared().view_control();
            let c = control.clone();
            let thread = std::thread::Builder::new().name(format!("worker-{id}")).spawn(move || {
                #[cfg(test)]
                let label = name.clone();
                worker_main(source, name, loc, rx, reply_rx, tx, c);
                #[cfg(test)]
                ENDED.lock().unwrap_or_else(|e| e.into_inner()).push(label);
            });
            let Ok(thread) = thread else {
                return JsValue::Number(0.0);
            };
            r.workers.borrow_mut().insert(
                id,
                Handle {
                    to,
                    reply,
                    from,
                    control,
                    thread,
                    #[cfg(test)]
                    inject,
                },
            );
            JsValue::Number(id as f64)
        }),
    )?;
    let r = reg.clone();
    runtime.register_host_function(
        "__rustkit_worker_post",
        2,
        Box::new(move |args| {
            let (id, json) = (arg_id(args), arg_str(args, 1));
            if let Some(h) = r.workers.borrow().get(&id) {
                let _ = h.to.send(json);
            }
            JsValue::Undefined
        }),
    )?;
    let r = reg.clone();
    runtime.register_host_function(
        "__rustkit_worker_reply",
        3,
        Box::new(move |args| {
            let (id, json) = (arg_id(args), arg_str(args, 2));
            if let Some(h) = r.workers.borrow().get(&id) {
                let _ = h.reply.send(json);
            }
            JsValue::Undefined
        }),
    )?;
    // poll(id, max) -> at most `max` (and MAX_ITEMS_PER_POLL) items: a JSON array of {k:"m",d}|{k:"e",m}|{k:"f",r,u}, ended by
    // {k:"x"} once the worker has returned and been drained; "null" for an
    // unknown id.
    let r = reg.clone();
    runtime.register_host_function(
        "__rustkit_worker_poll",
        2,
        Box::new(move |args| {
            let id = arg_id(args);
            // The page's pump passes what is left of its turn's budget; never more than the cap.
            let max = match args.get(1) {
                Some(JsValue::Number(n)) if *n >= 1.0 => (*n as usize).min(MAX_ITEMS_PER_POLL),
                _ => MAX_ITEMS_PER_POLL,
            };
            let mut items = Vec::new();
            let finished = {
                let w = r.workers.borrow();
                let Some(h) = w.get(&id) else {
                    return JsValue::String("null".into());
                };
                // Read "finished" before draining, so nothing sent just
                // before the end is missed.
                let done = h.thread.is_finished();
                // At most MAX_ITEMS_PER_POLL a turn: a worker that floods the
                // page leaves the rest, in order, for the next turns.
                while items.len() < max {
                    match h.from.try_recv() {
                        Ok(m) => items.push(item(m)),
                        Err(_) => break,
                    }
                }
                // A full batch may have left items behind: the end waits for them.
                done && items.len() < max
            };
            if finished {
                if let Some(h) = r.workers.borrow_mut().remove(&id) {
                    let _ = h.thread.join();
                }
                items.push(json!({"k": "x"}));
            }
            JsValue::String(Value::Array(items).to_string())
        }),
    )?;
    let r = reg.clone();
    runtime.register_host_function(
        "__rustkit_worker_terminate",
        1,
        Box::new(move |args| {
            let id = arg_id(args);
            let removed = r.workers.borrow_mut().remove(&id);
            if let Some(h) = removed {
                h.control.stop_all();
                drop(h.to); // a worker waiting for a message wakes and ends
                r.ending.borrow_mut().push(h.thread);
            }
            JsValue::Undefined
        }),
    )?;
    Ok(Guard(reg))
}

/// The least a worker's loop waits for a message between turns, in milliseconds.
const MIN_WAIT_MS: u64 = 1;

/// The prelude's entry points (worker_prelude.js), kept by the host and removed from the worker's global.
struct Entry {
    setup: KeptFunction,
    run: KeptFunction,
    next: KeptFunction,
    run_due: KeptFunction,
    dispatch: KeptFunction,
}

impl Entry {
    fn take(rt: &mut JsRuntime) -> Option<Self> {
        let entry = Entry {
            setup: rt.keep_function("__rk_worker_setup").ok()?,
            run: rt.keep_function("__rk_worker_run").ok()?,
            next: rt.keep_function("__rk_worker_next").ok()?,
            run_due: rt.keep_function("__rk_worker_run_due").ok()?,
            dispatch: rt.keep_function("__rk_worker_dispatch").ok()?,
        };
        rt.evaluate_script(
            "delete globalThis.__rk_worker_setup; delete globalThis.__rk_worker_run; delete globalThis.__rk_worker_next; delete globalThis.__rk_worker_run_due; delete globalThis.__rk_worker_dispatch;",
        )
        .ok()?;
        Some(entry)
    }
}

fn worker_main(
    source: String,
    name: String,
    location: String,
    rx: Receiver<String>,
    reply_rx: Receiver<String>,
    out: Sender<FromWorker>,
    control: Arc<ViewControl>,
) {
    let Ok(mut rt) = JsRuntime::new() else {
        let _ = out.send(FromWorker::Error("the worker's runtime failed to start".into()));
        return;
    };
    rt.set_view_control(Some(control.clone()));
    let stamp = Watchdog::shared().thread_stamp();
    let closed = Rc::new(Cell::new(false));
    {
        let tx = out.clone();
        let _ = rt.register_host_function(
            "__rk_worker_out",
            1,
            Box::new(move |args| {
                let _ = tx.send(FromWorker::Message(arg_str(args, 0)));
                JsValue::Undefined
            }),
        );
        let tx = out.clone();
        let _ = rt.register_host_function(
            "__rk_worker_error",
            1,
            Box::new(move |args| {
                let _ = tx.send(FromWorker::Error(arg_str(args, 0)));
                JsValue::Undefined
            }),
        );
        // importScripts: ask the page, then wait for its answer (or the end).
        let (tx, ctl) = (out.clone(), control.clone());
        let rid_counter = Cell::new(0u64);
        let _ = rt.register_host_function(
            "__rk_worker_fetch",
            1,
            Box::new(move |args| {
                let rid = rid_counter.get() + 1;
                rid_counter.set(rid);
                let _ = tx.send(FromWorker::Fetch { rid, urls: arg_str(args, 0) });
                loop {
                    if ctl.stops_all() {
                        return JsValue::String(String::new());
                    }
                    match reply_rx.recv_timeout(Duration::from_millis(10)) {
                        Ok(r) => return JsValue::String(r),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => return JsValue::String(String::new()),
                    }
                }
            }),
        );
        let closed = closed.clone();
        let _ = rt.register_host_function(
            "__rk_worker_close",
            0,
            Box::new(move |_| {
                closed.set(true);
                JsValue::Undefined
            }),
        );
    }
    let report = |e: String, control: &ViewControl| {
        if !control.stops_all() {
            let _ = out.send(FromWorker::Error(json!({"m": e, "f": "", "l": 0, "c": 0}).to_string()));
        }
    };

    rt.begin_turn(&stamp, TURN_BUDGET);
    // The prelude's entry points, held by the host from here on and removed from the worker's global, so
    // the worker's own script cannot replace what each turn calls.
    let entry = if rt.evaluate_script(STRUCTURED_CLONE).is_ok() && rt.evaluate_script(PRELUDE).is_ok() {
        Entry::take(&mut rt)
    } else {
        None
    };
    let started = entry.as_ref().is_some_and(|entry| {
        rt.call_kept(entry.setup, &[JsValue::String(name.clone()), JsValue::String(location.clone())])
            .is_ok()
    });
    if let (true, Some(entry)) = (started, entry.as_ref()) {
        if let Err(e) = rt.call_kept(entry.run, &[JsValue::String(source)]) {
            report(e.to_string(), &control);
        }
        let _ = rt.run_jobs();
    } else {
        report("the worker scope failed to start".into(), &control);
    }
    rt.end_turn(&stamp);
    let Some(entry) = entry.filter(|_| started) else {
        return;
    };

    while !closed.get() && !control.stops_all() {
        // Sleep until a message arrives or the next timer is due, at least MIN_WAIT_MS: a worker whose
        // timers are always due does not spin its thread.
        let wait = match rt.call_kept(entry.next, &[]) {
            Ok(JsValue::Number(n)) if n >= 0.0 => Some((n as u64).max(MIN_WAIT_MS)),
            _ => None,
        };
        let message = match wait {
            None => match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => break,
            },
            Some(ms) => match rx.recv_timeout(Duration::from_millis(ms)) {
                Ok(m) => Some(m),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
                Err(_) => break,
            },
        };
        rt.begin_turn(&stamp, TURN_BUDGET);
        if let Some(json) = message {
            if let Err(e) = rt.call_kept(entry.dispatch, &[JsValue::String(json)]) {
                report(e.to_string(), &control);
            }
            let _ = rt.run_jobs();
        }
        if !closed.get() {
            if let Err(e) = rt.call_kept(entry.run_due, &[]) {
                report(e.to_string(), &control);
            }
            let _ = rt.run_jobs();
        }
        rt.end_turn(&stamp);
    }
}
