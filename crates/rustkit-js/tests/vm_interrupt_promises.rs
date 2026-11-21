//! The vendored Boa's interrupt never panics inside promise, module and
//! async machinery (third_party/boa_engine/PATCHES.md, "Patch: no panic when
//! the interrupt reaches a call Boa assumed infallible").
//!
//! The interrupt makes calls Boa treated as unable to throw, such as a
//! promise's default `resolve`/`reject` functions, throw. Each test stops
//! a turn at one of those places, first exactly (script calls `arm()`, which
//! sets the word, so the next call the engine makes is stopped), then 200
//! times with the word set from another thread at varied delays. Every
//! test asserts that nothing panicked, that a turn that fails was stopped
//! by the interrupt (not turned into an engine panic error), and that the
//! same context then runs a new script, promise jobs and a dynamic import
//! correctly in the next turn, with the old turn's id still in the word.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use std::collections::VecDeque;
use std::future::Future;
use std::task::{Poll, Waker};

use boa_engine::error::{EngineError, RuntimeLimitError};
use boa_engine::job::{GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob};
use boa_engine::module::MapModuleLoader;
use boa_engine::{js_string, Context, JsError, JsResult, JsValue, Module, NativeFunction, Source};

const ITERATIONS: u64 = 200;

thread_local! {
    /// The word and turn `arm()` stops, per test thread.
    static ARMED: RefCell<Option<(Arc<AtomicU64>, u64)>> = const { RefCell::new(None) };
}

/// `arm()`: stop the running turn at the engine's next call. `arm(t)` does
/// so only if the running turn is `t`.
fn arm(_: &JsValue, args: &[JsValue], _: &mut Context) -> JsResult<JsValue> {
    ARMED.with(|armed| {
        if let Some((word, turn)) = &*armed.borrow() {
            let named = args.first().and_then(JsValue::as_number);
            if named.is_none_or(|t| t == *turn as f64) {
                word.fetch_max(*turn, Ordering::Relaxed);
            }
        }
    });
    Ok(JsValue::undefined())
}

/// The job policy of rustkit-js' `HostJobExecutor`: every queued job runs
/// and the first error is returned. (Boa's default executor drops the whole
/// queue at the first error, so in a stopped turn the engine's own reactions,
/// such as the one resuming a module's top-level `await`, never ran and a
/// later import of that module could wait for good.)
#[derive(Default)]
struct HostPolicyExecutor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
}

impl JobExecutor for HostPolicyExecutor {
    fn enqueue_job(self: Rc<Self>, job: Job, _: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::GenericJob(job) => self.generic_jobs.borrow_mut().push_back(job),
            _ => {}
        }
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        let context = RefCell::new(context);
        let mut first_error = None;
        let mut note = |result: JsResult<JsValue>| {
            if let Err(err) = result {
                first_error.get_or_insert(err);
            }
        };
        loop {
            let mut progress = false;
            let async_jobs = std::mem::take(&mut *self.async_jobs.borrow_mut());
            for job in async_jobs {
                progress = true;
                // `MapModuleLoader` answers at once: one poll settles it.
                let mut future = std::pin::pin!(job.call(&context));
                match future
                    .as_mut()
                    .poll(&mut std::task::Context::from_waker(Waker::noop()))
                {
                    Poll::Ready(result) => note(result),
                    Poll::Pending => panic!("an async job did not finish at once"),
                }
            }
            let promise_jobs = std::mem::take(&mut *self.promise_jobs.borrow_mut());
            for job in promise_jobs {
                progress = true;
                note(job.call(&mut context.borrow_mut()));
            }
            let generic_jobs = std::mem::take(&mut *self.generic_jobs.borrow_mut());
            for job in generic_jobs {
                progress = true;
                note(job.call(&mut context.borrow_mut()));
            }
            if !progress {
                break;
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

struct Page {
    context: Context,
    word: Arc<AtomicU64>,
    loader: Rc<MapModuleLoader>,
    turn: u64,
}

impl Page {
    fn new() -> Self {
        let loader = Rc::new(MapModuleLoader::new());
        let mut context = Context::builder()
            .module_loader(loader.clone())
            .job_executor(Rc::new(HostPolicyExecutor::default()))
            .build()
            .unwrap();
        let word = Arc::new(AtomicU64::new(0));
        context.set_interrupt_word(Some(word.clone()));
        context
            .register_global_builtin_callable(
                js_string!("arm"),
                0,
                NativeFunction::from_fn_ptr(arm),
            )
            .unwrap();
        let mut page = Self {
            context,
            word,
            loader,
            turn: 0,
        };
        page.next_turn();
        page
    }

    /// Starts a new turn. The word keeps naming the old one, as after a
    /// late watchdog store.
    fn next_turn(&mut self) {
        self.turn += 1;
        self.context.set_interrupt_turn(self.turn);
        let armed = (self.word.clone(), self.turn);
        ARMED.with(|a| *a.borrow_mut() = Some(armed));
    }

    fn module(&mut self, name: &str, source: &str) {
        let module = Module::parse(Source::from_bytes(source), None, &mut self.context).unwrap();
        self.loader.insert(name, module);
    }

    fn eval(&mut self, source: &str) -> JsResult<JsValue> {
        self.context.eval(Source::from_bytes(source))
    }

    /// Evaluates and drains the job queue, as one turn.
    fn turn(&mut self, source: &str) -> JsResult<()> {
        self.eval(source)?;
        self.context.run_jobs()
    }

    fn get(&mut self, source: &str) -> String {
        self.eval(source)
            .unwrap_or_else(|e| panic!("`{source}` threw {e}"))
            .to_string(&mut self.context)
            .unwrap()
            .to_std_string_escaped()
    }

    /// Stops the current turn now: the next call the engine makes fails.
    fn stop_now(&self) {
        self.word.fetch_max(self.turn, Ordering::Relaxed);
    }

    /// The same context runs new script correctly in a fresh turn: plain
    /// code, promise jobs, an async function and a dynamic import.
    fn assert_recovers(&mut self, tag: &str) {
        self.next_turn();
        let name = format!("fresh-{}", self.turn);
        self.module(&name, "export const answer = 42;");
        let source = format!(
            r#"
            var out = [];
            out.push([1, 2, 3].map(function (x) {{ return x * 2; }}).join());
            Promise.resolve(5).then(function (v) {{ out.push("then " + v); }});
            (async function () {{ await null; out.push("async"); }})();
            import("{name}").then(function (ns) {{ out.push("import " + ns.answer); }},
                                  function (e) {{ out.push("import failed " + e); }});
            "#
        );
        self.turn(&source)
            .unwrap_or_else(|e| panic!("{tag}: the next turn failed: {e}"));
        assert_eq!(
            self.get("out.join('|')"),
            "2,4,6|then 5|async|import 42",
            "{tag}: the next turn ran wrongly"
        );
    }
}

fn is_interrupt(err: &JsError) -> bool {
    matches!(
        err.as_engine(),
        Some(EngineError::RuntimeLimit(RuntimeLimitError::Interrupted))
    )
}

/// A failed turn must have been stopped by the interrupt itself.
fn assert_interrupt_or_ok(result: JsResult<()>, tag: &str) {
    if let Err(err) = result {
        assert!(
            is_interrupt(&err),
            "{tag}: failed with {err}, not the interrupt"
        );
    }
}

fn assert_interrupted(result: JsResult<()>, tag: &str) {
    let err = result.expect_err(tag);
    assert!(
        is_interrupt(&err),
        "{tag}: failed with {err}, not the interrupt"
    );
}

/// A cheap deterministic spread of delays (no `rand` dependency).
fn delay(i: u64, max_us: u64) -> Duration {
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 31;
    Duration::from_micros(x % max_us)
}

/// Runs `turn` 200 times on one page; each time another thread sets the
/// word for that turn after a varied delay, and the page must recover.
fn race(
    name: &str,
    max_us: u64,
    mut setup: impl FnMut(&mut Page, u64),
    source: impl Fn(u64) -> String,
) {
    let mut page = Page::new();
    let mut stopped = 0;
    for i in 0..ITERATIONS {
        page.next_turn();
        setup(&mut page, i);
        let (word, turn) = (page.word.clone(), page.turn);
        let after = delay(i, max_us);
        let setter = std::thread::spawn(move || {
            std::thread::sleep(after);
            word.fetch_max(turn, Ordering::Relaxed);
        });
        let result = page.turn(&source(i));
        setter.join().unwrap();
        stopped += u64::from(result.is_err());
        assert_interrupt_or_ok(result, &format!("{name} #{i}"));
        // Whatever the stopped turn left queued may run now, in a turn the
        // word does not name: nothing in it may raise the old stop again.
        page.next_turn();
        page.context
            .run_jobs()
            .unwrap_or_else(|e| panic!("{name} #{i} drain: a later turn failed: {e}"));
    }
    assert!(
        stopped > 0,
        "{name}: no iteration was stopped; the race tested nothing"
    );
    page.assert_recovers(name);
}

#[test]
fn interrupt_during_promise_resolve() {
    let mut page = Page::new();
    // A reaction with no handler: the job's first call is `resolve`. The
    // engine's own resolve runs no script, so it is entered even in the
    // stopped turn and the chain settles.
    let r = page.turn("var p = Promise.resolve(1).then(function () { arm(); }).then();");
    assert_interrupt_or_ok(r, "the reaction's resolve");
    assert_eq!(page.settle("p"), "ok undefined");
    page.assert_recovers("resolve: reaction");

    // Script's resolve, called while the turn is stopped, is still stopped.
    let r = page.turn("var q = new Promise(function (resolve) { arm(); resolve(1); });");
    assert_interrupted(r, "the executor's resolve");
    page.assert_recovers("resolve: executor");

    // An import whose module is already known: the loader answers at once
    // and the load's own capability and `resolve` run in the stopped job.
    page.module("known", "export const k = 1;");
    page.eval("import('known')").unwrap();
    page.stop_now();
    assert_interrupt_or_ok(page.context.run_jobs(), "resolve: module load");
    page.assert_recovers("resolve: module load");

    // `Module::load` and `load_link_evaluate` called by the embedder in a
    // stopped turn.
    let module = Module::parse(
        Source::from_bytes("export const z = 1;"),
        None,
        &mut page.context,
    )
    .unwrap();
    page.stop_now();
    let _ = module.load(&mut page.context);
    let _ = module.load_link_evaluate(&mut page.context);
    assert_interrupt_or_ok(page.context.run_jobs(), "resolve: load_link_evaluate");
    page.assert_recovers("resolve: load_link_evaluate");

    race(
        "resolve",
        400,
        |_, _| {},
        |_| {
            "for (var i = 0; i < 300; i++) { Promise.resolve(i).then().then(function (v) { return v; }); new Promise(function (r) { r(i); }); }".into()
        },
    );
}

#[test]
fn interrupt_during_promise_reject() {
    let mut page = Page::new();
    let r =
        page.turn("Promise.resolve().then(function () { arm(); throw 1; }).catch(function () {});");
    assert_interrupted(r, "the reaction's reject");
    page.assert_recovers("reject: reaction");

    // The executor's `reject` call itself is stopped.
    let r = page.turn("var rejected = new Promise(function (_, reject) { arm(); reject(1); });");
    assert_interrupted(r, "the executor's reject");
    page.assert_recovers("reject: executor");

    race(
        "reject",
        400,
        |_, _| {},
        |_| {
            "for (var i = 0; i < 300; i++) { Promise.reject(i).then(null).catch(function () {}); new Promise(function (_, j) { j(i); }).catch(function () {}); }".into()
        },
    );
}

#[test]
fn interrupt_during_a_failed_dynamic_import() {
    // PR #706's report: `import()` of a module the loader cannot supply,
    // finished in a stopped turn, panicked at "default `reject` function
    // cannot throw".
    let mut page = Page::new();
    page.eval("var failed = import('missing').catch(function () {});")
        .unwrap();
    page.stop_now();
    assert_interrupted(page.context.run_jobs(), "the import's reject");
    page.assert_recovers("failed import");

    // A module that loads but cannot link.
    page.module(
        "bad-link",
        "import { nope } from 'known-2'; export default nope;",
    );
    page.module("known-2", "export const k = 1;");
    page.eval("import('bad-link').catch(function () {});")
        .unwrap();
    page.stop_now();
    assert_interrupt_or_ok(page.context.run_jobs(), "link failure");
    page.assert_recovers("link failure");

    race(
        "failed import",
        600,
        |_, _| {},
        |i| {
            format!("for (var i = 0; i < 40; i++) {{ import('missing-{i}-' + i).catch(function () {{}}); }}")
        },
    );
}

#[test]
fn interrupt_during_a_successful_dynamic_imports_evaluation() {
    let mut page = Page::new();
    // Stopped in the module's body.
    page.module(
        "body",
        "export const x = 1; arm(); [1].map(function (v) { return v; });",
    );
    assert_interrupt_or_ok(
        page.turn("import('body').then(function () {});"),
        "in the body",
    );
    page.assert_recovers("import: body");

    // Stopped right after the body: the evaluation's own promise work.
    page.module("tail", "export const y = 2; arm();");
    assert_interrupt_or_ok(
        page.turn("import('tail').then(function () {});"),
        "after the body",
    );
    page.assert_recovers("import: tail");

    race(
        "successful import",
        800,
        |page, i| {
            if i > 0 {
                page.reimport_all(&format!("m-{}-", i - 1), 20);
            }
            for k in 0..20 {
                page.module(&format!("m-{i}-{k}"), "export let n = 0; for (let j = 0; j < 50; j++) { n += [j].map(function (v) { return v; })[0]; }");
            }
        },
        |i| {
            format!("for (var k = 0; k < 20; k++) {{ import('m-{i}-' + k).then(function (ns) {{ return ns.n; }}); }}")
        },
    );
}

#[test]
fn interrupt_during_an_async_function_await() {
    let mut page = Page::new();
    // Resumed from an await, then spinning: the stop now reaches the
    // caller of `run_jobs` instead of being dropped by the Await reaction.
    let r = page.turn("var caught = false; (async function () { try { await null; arm(); for (;;) {} } catch (e) { caught = true; } })();");
    assert_interrupted(r, "the async spin");
    page.assert_recovers("await: spin");
    assert_eq!(page.get("caught"), "false", "no catch saw the stop");

    // Resumed, then finishing: settling the async function's promise is
    // stopped.
    let r = page.turn("(async function () { await null; arm(); })().then(function () {});");
    assert_interrupt_or_ok(r, "the async return");
    page.assert_recovers("await: return");

    // An async generator resumed by `next()`: Boa asserted its body never
    // throws.
    page.turn("async function* ag() { yield 1; arm(); for (;;) {} } var it = ag(); it.next();")
        .unwrap();
    assert_interrupted(page.turn("it.next();"), "the async generator");
    page.assert_recovers("await: async generator");
    // Using the stopped generator again is harmless (and its calls settle:
    // `an_async_generator_stopped_while_it_runs_settles_later_calls`).
    page.next_turn();
    page.turn("it.next(); it.return(1); it.throw(2).catch(function () {});")
        .expect("the stopped generator can be called again");
    // And one resumed from an await.
    let r = page.turn("(async function* () { await null; arm(); for (;;) {} })().next();");
    assert_interrupted(r, "the async generator's await");
    page.assert_recovers("await: async generator await");

    race(
        "await",
        600,
        |_, _| {},
        |_| {
            "async function f(n) { var s = 0; for (var j = 0; j < n; j++) { s += await j; } return s; } for (var i = 0; i < 30; i++) { f(20).then(function (v) { return v; }); }".into()
        },
    );
}

#[test]
fn interrupt_during_a_microtask_drain() {
    let mut page = Page::new();
    let r = page.turn(
        "var ran = 0; for (var i = 0; i < 100; i++) { Promise.resolve().then(function () { ran++; if (ran === 50) arm(); }); }",
    );
    assert_interrupted(r, "the drain");
    // The 51st job was stopped at its call: no job after the stop ran in
    // the stopped turn.
    page.next_turn();
    assert_eq!(page.get("ran"), "50");
    page.assert_recovers("drain");

    race(
        "drain",
        800,
        |_, _| {},
        |_| {
            "var c = 0; function chain(n) { return n === 0 ? null : Promise.resolve(n).then(function (v) { c += v; return chain(n - 1); }); } for (var i = 0; i < 20; i++) { chain(30); }".into()
        },
    );
}

#[test]
fn interrupt_during_finally_and_cleanup() {
    let mut page = Page::new();
    // `finally` blocks, an iterator's `return` and a generator's `finally`
    // never run for the stop, sync or after an await.
    let r = page.turn(
        r#"
        var cleaned = [];
        var iterable = { [Symbol.iterator]() { var n = 0; return {
            next() { return { value: n++, done: false }; },
            return() { cleaned.push("iterator"); return {}; } }; } };
        function* g() { try { yield 1; arm(); for (;;) {} } finally { cleaned.push("generator"); } }
        (async function () {
            try { await null; for (var v of iterable) { if (v === 3) { var it = g(); it.next(); it.next(); } } }
            finally { cleaned.push("async finally"); }
        })().finally(function () { cleaned.push("promise finally"); });
        "#,
    );
    assert_interrupted(r, "cleanup");
    page.next_turn();
    page.context.run_jobs().unwrap();
    assert_eq!(
        page.get("cleaned.join()"),
        "",
        "no cleanup ran for the stop"
    );
    page.assert_recovers("cleanup");

    // An iterator's `return` stopped while it closes over a catchable
    // error: the stop wins, so the `catch` never runs.
    let r = page.turn(
        r#"
        var caught = false;
        var closing = { [Symbol.iterator]() { return {
            next() { return { value: 1, done: false }; },
            return() { arm(); [0].map(function (v) { return v; }); return {}; } }; } };
        try { for (var v of closing) { throw new Error("body"); } } catch (e) { caught = true; }
        "#,
    );
    assert_interrupted(r, "iterator close");
    page.next_turn();
    assert_eq!(
        page.get("caught"),
        "false",
        "the catch did not see the body's error"
    );
    page.assert_recovers("iterator close");
    // The same where Rust closes the iterator (`Array.from`'s mapper threw).
    let r = page.turn(
        r#"
        var caught2 = false;
        try { Array.from(closing, function () { throw new Error("map"); }); } catch (e) { caught2 = true; }
        "#,
    );
    assert_interrupted(r, "Array.from close");
    page.next_turn();
    assert_eq!(
        page.get("caught2"),
        "false",
        "the catch did not see the mapper's error"
    );
    page.assert_recovers("Array.from close");
    race(
        "finally",
        600,
        |_, _| {},
        |_| {
            "var f = 0; for (var i = 0; i < 100; i++) { Promise.resolve(i).finally(function () { f++; }).then(function () { try { return [i].map(function (v) { return v; }); } finally { f++; } }); }".into()
        },
    );
}

#[test]
fn interrupt_during_a_modules_top_level_await() {
    let mut page = Page::new();
    // Stopped before the first await: Boa asserted an async module's body
    // "cannot directly throw".
    page.module(
        "tla-before",
        "arm(); [1].map(function (v) { return v; }); await null; export const a = 1;",
    );
    assert_interrupt_or_ok(
        page.turn("import('tla-before').catch(function () {});"),
        "before await",
    );
    page.assert_recovers("tla: before await");

    // Stopped after it, resumed from a job.
    page.module(
        "tla-after",
        "await null; arm(); for (;;) {} export const b = 1;",
    );
    assert_interrupted(
        page.turn("import('tla-after').catch(function () {});"),
        "after await",
    );
    page.assert_recovers("tla: after await");

    // Through the embedder's API.
    let module = Module::parse(
        Source::from_bytes("arm(); [1].map(function (v) { return v; }); await null;"),
        None,
        &mut page.context,
    )
    .unwrap();
    let _ = module.load_link_evaluate(&mut page.context);
    assert_interrupt_or_ok(page.context.run_jobs(), "load_link_evaluate");
    page.assert_recovers("tla: load_link_evaluate");

    race(
        "tla",
        800,
        |page, i| {
            if i > 0 {
                page.reimport_all(&format!("t-{}-", i - 1), 10);
            }
            for k in 0..10 {
                page.module(
                    &format!("t-{i}-{k}"),
                    "let s = 0; for (let j = 0; j < 10; j++) { s += await j; } export { s };",
                );
            }
        },
        |i| {
            format!(
                "for (var k = 0; k < 10; k++) {{ import('t-{i}-' + k).catch(function () {{}}); }}"
            )
        },
    );
}

#[test]
fn a_page_stopped_thousands_of_times_keeps_working() {
    // Each stop used to leave the stopped frame's values on the VM's operand
    // stack (and each linked module its link frame's values, stopped or
    // not); about 2,000 of them filled it, and every later script failed
    // with "reached the maximum stack size".
    let mut page = Page::new();
    let stops = [
        "var a = 1, b = 2, d = [a, b]; arm(); d.map(function (v) { return v; });",
        "(async function () { var a = 1, b = [a]; await null; arm(); b.map(function (v) { return v; }); })();",
        "Promise.resolve().then(function () { var a = [1]; arm(); a.map(function (v) { return v; }); });",
    ];
    for i in 0..2500u64 {
        for (k, source) in stops.iter().enumerate() {
            assert_interrupted(page.turn(source), &format!("stop {k} #{i}"));
            page.next_turn();
        }
        page.module(
            &format!("stopped-{i}"),
            "let x = 1, z = [x]; arm(); z.map(function (v) { return v; }); export { x };",
        );
        assert_interrupted(
            page.turn(&format!("import('stopped-{i}').catch(function () {{}});")),
            "module stop",
        );
        page.next_turn();
        page.module(
            &format!("loaded-{i}"),
            "let x = 1, z = [x]; z.map(function (v) { return v; }); export { x };",
        );
        page.turn(&format!("import('loaded-{i}');"))
            .unwrap_or_else(|e| panic!("import #{i}: {e}"));
        page.next_turn();
    }
    page.assert_recovers("thousands of stops");
    page.next_turn();
    assert_eq!(
        page.get("(function f(n) { return n === 0 ? 0 : 1 + f(n - 1); })(400)"),
        "400"
    );
}

impl Page {
    /// In a new turn, `source` (an expression) as a promise, settled in that
    /// same turn: "ok <value>" or "err <reason>". A promise still pending
    /// once the turn's jobs ran fails the test: a later import must never
    /// hang.
    fn settle(&mut self, source: &str) -> String {
        self.next_turn();
        let script = format!(
            r#"var settled = "pending";
            Promise.resolve({source}).then(function (v) {{ settled = "ok " + v; }},
                                           function (e) {{ settled = "err " + e; }});"#
        );
        self.turn(&script)
            .unwrap_or_else(|e| panic!("`{source}` in a later turn failed: {e}"));
        let settled = self.get("settled");
        assert_ne!(settled, "pending", "`{source}` never settled");
        settled
    }
}

impl Page {
    /// In a later turn, imports `{prefix}0` .. `{prefix}{count - 1}`, which
    /// a raced turn may have stopped anywhere: each import settles, with the
    /// namespace or the ordinary interrupted-module error.
    fn reimport_all(&mut self, prefix: &str, count: u32) {
        let settled = self.settle(&format!(
            r#"Promise.all(Array.from({{ length: {count} }}, function (_, k) {{
                return import('{prefix}' + k).then(function () {{ return 1; }}, function (e) {{
                    if (e.message !== "module evaluation was interrupted") throw e;
                    return 1;
                }});
            }})).then(function (r) {{ return r.length; }})"#
        ));
        assert_eq!(settled, format!("ok {count}"), "re-importing {prefix}*");
    }
}

const INTERRUPTED_MODULE: &str = "err Error: module evaluation was interrupted";

const INTERRUPTED_GENERATOR: &str = "err Error: async generator was interrupted";

#[test]
fn an_async_generator_stopped_while_it_runs_settles_later_calls() {
    // R1 HOLD @ d627b635: a stopped async generator stayed `executing`, so
    // its pending and later `next`/`return`/`throw` never settled.
    let mut page = Page::new();
    // Stopped in a body that `next()` resumes synchronously
    // (`AsyncGenerator::resume`): later calls settle.
    page.turn("async function* g1() { yield 1; arm(); for (;;) {} } var it1 = g1(); it1.next();")
        .unwrap();
    assert_interrupted(page.turn("it1.next();"), "stopped by next()");
    page.next_turn();
    // Requests queued while the body awaits: the first is answered by a
    // `yield`, the body goes on for the second and is stopped, and a third
    // is queued behind it.
    assert_interrupted(
        page.turn(
            "async function* g3() { await null; yield 1; arm(); for (;;) {} } var it3 = g3(); var first3 = it3.next(), stopped3 = it3.next(), queued3 = it3.next();",
        ),
        "stopped for a queued request",
    );
    page.next_turn();
    // Stopped in a body resumed from an await (the `Await` reactions).
    assert_interrupted(
        page.turn(
            "async function* g2() { await null; arm(); for (;;) {} } var it2 = g2(); var stopped2 = it2.next(); var queued2 = it2.next();",
        ),
        "stopped after an await",
    );
    assert_eq!(
        page.settle("first3.then(function (r) { return r.value; })"),
        "ok 1"
    );
    for it in ["2", "3"] {
        // The requests of the stopped turn reject with an ordinary error.
        assert_eq!(page.settle(&format!("stopped{it}")), INTERRUPTED_GENERATOR);
        assert_eq!(page.settle(&format!("queued{it}")), INTERRUPTED_GENERATOR);
    }
    for it in ["1", "2", "3"] {
        // Later calls settle as on a finished generator.
        assert_eq!(
            page.settle(&format!(
                "it{it}.next().then(function (r) {{ return r.done; }})"
            )),
            "ok true"
        );
        assert_eq!(
            page.settle(&format!(
                "it{it}.return(5).then(function (r) {{ return r.value; }})"
            )),
            "ok 5"
        );
        assert_eq!(page.settle(&format!("it{it}.throw(7)")), "err 7");
        assert_eq!(
            page.settle(&format!(
                "(async function () {{ for await (var v of it{it}) {{}} return 'done'; }})()"
            )),
            "ok done"
        );
    }
    page.assert_recovers("async generator");

    // Every request made in a raced turn settles in the next one.
    race(
        "async generator",
        600,
        |page, i| {
            // A raced turn can stop anywhere, even before its first
            // statement, so count what it made; every one must settle.
            let settled = page.settle(
                r#"(function () {
                    var made = typeof ps === "undefined" ? [] : ps;
                    ps = [];
                    return Promise.all(made.map(function (p) {
                        return p.then(function () { return 1; }, function (e) {
                            if (e.message !== "async generator was interrupted") throw e;
                            return 1;
                        });
                    })).then(function (r) { return r.length === made.length; });
                })()"#,
            );
            assert_eq!(settled, "ok true", "#{i}: the previous turn's requests");
        },
        |_| {
            "ps = []; async function* g() { for (var j = 0; j < 10; j++) { yield await [j].map(function (v) { return v; })[0]; } } for (var i = 0; i < 10; i++) { var it = g(); for (var k = 0; k < 12; k++) { ps.push(it.next()); } }".into()
        },
    );
}

#[test]
fn a_module_stopped_while_it_evaluates_does_not_poison_later_turns() {
    // R1 HOLD @ 4c92f5b6: the stopped module kept `Interrupted` as its
    // evaluation error and the stopped turn's capability, so every later
    // import of it hung and one later turn failed with "the script was
    // interrupted".
    let mut page = Page::new();
    page.module(
        "leaf",
        "export const v = 1; arm(); [1].map(function (x) { return x; });",
    );
    page.module("mid", "import { v } from 'leaf'; export const w = v + 1;");
    assert_interrupted(page.turn("import('mid');"), "the stopped import");

    // Two later turns import the stopped module (and the one under it):
    // each settles with an ordinary, catchable error.
    assert_eq!(page.settle("import('mid')"), INTERRUPTED_MODULE);
    assert_eq!(page.settle("import('leaf')"), INTERRUPTED_MODULE);
    assert_eq!(page.settle("import('mid')"), INTERRUPTED_MODULE);
    // Awaiting its namespace: a `catch` takes it like any error.
    assert_eq!(
        page.settle(
            "(async function () { try { var ns = await import('mid'); return ns.w; } catch (e) { return 'caught ' + e.message; } })()"
        ),
        "ok caught module evaluation was interrupted"
    );
    // The next unrelated turn runs normally.
    page.assert_recovers("stopped module");

    // Stopped after the body, at the evaluation's own `resolve`: the module
    // evaluated, so later imports get its namespace.
    page.module("tail-2", "export const y = 2; arm();");
    assert_interrupt_or_ok(page.turn("import('tail-2');"), "after the body");
    assert_eq!(
        page.settle("import('tail-2').then(function (ns) { return ns.y; })"),
        "ok 2"
    );
    assert_eq!(
        page.settle("import('tail-2').then(function (ns) { return ns.y; })"),
        "ok 2"
    );
    page.assert_recovers("stopped resolve");

    // Through the embedder's API: the same module evaluated again later.
    let module = Module::parse(
        Source::from_bytes("export const e = 1; arm(); [1].map(function (x) { return x; });"),
        None,
        &mut page.context,
    )
    .unwrap();
    let _ = module.load_link_evaluate(&mut page.context);
    assert_interrupted(page.context.run_jobs(), "load_link_evaluate");
    page.next_turn();
    let promise = module
        .evaluate(&mut page.context)
        .expect("a later evaluate returns a promise");
    page.context.run_jobs().expect("a later evaluate");
    match promise.state() {
        boa_engine::builtins::promise::PromiseState::Rejected(e) => assert_eq!(
            e.to_string(&mut page.context)
                .unwrap()
                .to_std_string_escaped(),
            "Error: module evaluation was interrupted"
        ),
        state => panic!("a later evaluate: {state:?}"),
    }
    page.assert_recovers("embedder evaluate");
}

#[test]
fn a_module_stopped_around_top_level_await_does_not_poison_later_turns() {
    let mut page = Page::new();
    let import_twice = |page: &mut Page, name: &str, expect: &str| {
        let ns = format!("import('{name}')");
        assert_eq!(page.settle(&ns), expect, "{name}, first later import");
        assert_eq!(page.settle(&ns), expect, "{name}, second later import");
        page.assert_recovers(name);
    };

    // Before the first await.
    page.module(
        "tla-before-2",
        "arm(); [1].map(function (v) { return v; }); await null; export const a = 1;",
    );
    assert_interrupt_or_ok(page.turn("import('tla-before-2');"), "before await");
    import_twice(&mut page, "tla-before-2", INTERRUPTED_MODULE);

    // Resumed from an await, then stopped.
    page.module(
        "tla-after-2",
        "await null; arm(); for (;;) {} export const b = 1;",
    );
    page.module(
        "over-tla-after",
        "import { b } from 'tla-after-2'; export const c = b;",
    );
    assert_interrupted(page.turn("import('over-tla-after');"), "after await");
    import_twice(&mut page, "tla-after-2", INTERRUPTED_MODULE);
    import_twice(&mut page, "over-tla-after", INTERRUPTED_MODULE);

    // A module run once its async dependency settled (AsyncModuleExecution
    // Fulfilled), stopped in its body: plain, and with its own await.
    page.module("dep", "await null; export const d = 1;");
    page.module(
        "after-dep",
        "import { d } from 'dep'; arm(); [1].map(function (v) { return v; }); export const p = d;",
    );
    page.module(
        "after-dep-tla",
        "import { d } from 'dep'; arm(); [1].map(function (v) { return v; }); await null; export const q = d;",
    );
    page.module("top", "import { p } from 'after-dep'; export const t = p;");
    assert_interrupted(page.turn("import('top');"), "after an async dependency");
    import_twice(&mut page, "top", INTERRUPTED_MODULE);
    import_twice(&mut page, "after-dep", INTERRUPTED_MODULE);
    assert_interrupted(
        page.turn("import('after-dep-tla');"),
        "after an async dependency, with await",
    );
    import_twice(&mut page, "after-dep-tla", INTERRUPTED_MODULE);
    // The dependency itself evaluated.
    assert_eq!(
        page.settle("import('dep').then(function (ns) { return ns.d; })"),
        "ok 1"
    );

    // Resumed from an await, then stopped as its body settles its own
    // promise: the async evaluation never completed, so it is an
    // interrupted module too.
    page.module("tla-tail", "await null; export const r = 3; arm();");
    assert_interrupted(page.turn("import('tla-tail');"), "tla tail");
    import_twice(&mut page, "tla-tail", INTERRUPTED_MODULE);
}

#[test]
fn a_module_stopped_at_any_job_of_its_top_level_await_settles_later() {
    // The stop lands on each job in turn: the module's own await reactions
    // (an engine reaction, let in to record the stop), its evaluation's
    // bookkeeping, or the page's job between them. Every later import
    // settles, with the namespace or the interrupted-module error.
    let mut page = Page::new();
    for n in 0..16 {
        page.next_turn();
        let name = format!("tick-{n}");
        page.module(
            &name,
            "let s = 0; for (let j = 0; j < 3; j++) { s += await j; } export { s };",
        );
        let chain = "p = p.then(function () {});".repeat(n);
        let r = page.turn(&format!(
            "import('{name}').catch(function () {{}}); var p = Promise.resolve(); {chain} p.then(function () {{ arm(); [1].map(function (x) {{ return x; }}); }});"
        ));
        assert_interrupt_or_ok(r, &name);
        let import = format!("import('{name}').then(function (ns) {{ return ns.s; }})");
        for _ in 0..2 {
            let settled = page.settle(&import);
            assert!(
                settled == "ok 3" || settled == INTERRUPTED_MODULE,
                "{name}: {settled}"
            );
        }
    }
    page.assert_recovers("tla ticks");
}

#[test]
fn an_engine_reaction_let_in_after_the_stop_runs_no_script() {
    // `new_promise_reaction_job` enters the engine's own reactions in a
    // stopped turn so a module can record the stop; the body such a reaction
    // resumes still runs nothing.
    let mut page = Page::new();
    let r = page.turn(
        "var after = 0; Promise.resolve().then(function () { arm(); [0].map(function (v) { return v; }); }); (async function () { await null; after = 1; })(); (async function* () { await null; after = 2; })().next();",
    );
    assert_interrupted(r, "the stop");
    page.next_turn();
    assert_eq!(page.get("after"), "0", "a resumed body ran after the stop");
    page.assert_recovers("engine reaction");
}

#[test]
fn jobs_refused_in_a_stopped_turn_leave_nothing_on_the_stack() {
    // The host runs every job left in a stopped turn, and each is refused at
    // its handler's entry. `JsObject::call` used to leave `this`, the
    // handler and its argument on the VM's stack each time; a few hundred
    // stopped turns filled it and every later script failed with "reached
    // the maximum stack size".
    let mut page = Page::new();
    for i in 0..1000 {
        assert_interrupted(
            page.turn(
                "Promise.resolve().then(function () { arm(); [0].map(function (v) { return v; }); }); for (var k = 0; k < 20; k++) { Promise.resolve(k).then(function (v) { return v; }); new Promise(function (r) { r(); }).then(function () {}, function () {}); }",
            ),
            &format!("stop #{i}"),
        );
        page.next_turn();
    }
    page.assert_recovers("refused jobs");
}

/// The ordinary errors a promise left behind by a stopped turn settles with.
const INTERRUPTED_MESSAGES: &str = r#"["async generator was interrupted", "async function was interrupted", "promise job was interrupted", "module evaluation was interrupted"]"#;

impl Page {
    /// In a later turn, the result of `source` as JSON, once it settles
    /// (`err <message>` if it rejects).
    fn settle_json(&mut self, source: &str) -> String {
        self.settle(&format!(
            "Promise.resolve({source}).then(JSON.stringify, function (e) {{ throw e.message; }})"
        ))
    }

    /// In a later turn, every promise in the global `ps` (made by a turn
    /// that may have been stopped anywhere) settles: with a value, an error
    /// the script threw, or one of the ordinary interrupted errors. Every
    /// sync generator in `syncs` answers `next()` and `return()` without
    /// throwing, and every async generator in `asyncs` settles both.
    fn assert_left_nothing_pending(&mut self, tag: &str) {
        let settled = self.settle(&format!(
            r#"(function () {{
                var interrupted = {INTERRUPTED_MESSAGES};
                function settles(p) {{
                    return Promise.resolve(p).then(function () {{ return 1; }}, function (e) {{
                        if (e !== "thrown" && interrupted.indexOf(e && e.message) < 0) throw e;
                        return 1;
                    }});
                }}
                var made = typeof ps === "undefined" ? [] : ps;
                var syncs_ = typeof syncs === "undefined" ? [] : syncs;
                var asyncs_ = typeof asyncs === "undefined" ? [] : asyncs;
                ps = []; syncs = []; asyncs = [];
                syncs_.forEach(function (g) {{ g.next(); g.return(1); }});
                var all = made.concat(asyncs_.map(function (g) {{ return g.next(); }}),
                                      asyncs_.map(function (g) {{ return g.return(1); }}));
                return Promise.all(all.map(settles)).then(function (r) {{ return r.length === all.length; }});
            }})()"#
        ));
        assert_eq!(
            settled, "ok true",
            "{tag}: something the stopped turn left never settled"
        );
    }
}

#[test]
fn the_holds_async_generator_stopped_mid_body_answers_later_calls() {
    // R1 HOLD @ d627b635, its exact input: stopped after an `await`, and
    // stopped before the first one.
    for (tag, body) in [
        ("after an await", "await null; arm();"),
        ("before the first await", "arm();"),
    ] {
        let mut page = Page::new();
        let r = page.turn(&format!(
            "globalThis.ag = (async function* () {{ {body} [1].map(function (x) {{ return x; }}); yield 1; }})(); ag.next();"
        ));
        assert_interrupted(r, tag);
        for _ in 0..2 {
            assert_eq!(
                page.settle_json("ag.next()"),
                r#"ok {"done":true}"#,
                "{tag}"
            );
            assert_eq!(
                page.settle_json("ag.return(7)"),
                r#"ok {"value":7,"done":true}"#,
                "{tag}"
            );
        }
        page.assert_recovers(tag);
    }
}

#[test]
fn an_async_generator_stopped_in_return_or_delegation_answers_later_calls() {
    let cases = [
        // `return(v)` awaits v; the reaction that would finish it is refused.
        (
            "await-return refused",
            "var g = (async function* () {})(); g.next(); Promise.resolve().then(function () { arm(); }); var p = g.return(Promise.resolve(1));",
        ),
        // The thenable `return(v)` awaits is stopped in its `then`.
        (
            "await-return stopped in then",
            "var g = (async function* () {})(); g.next(); var p = g.return({ then: function (r) { arm(); [1].map(function (x) { return x; }); r(1); } });",
        ),
        // A `constructor` getter on the value `return(v)` resolves. Stopped
        // inside `return()` itself, the script never gets `p`.
        (
            "await-return stopped in PromiseResolve",
            "var g = (async function* () {})(); g.next(); var v = Promise.resolve(1); Object.defineProperty(v, 'constructor', { get: function () { arm(); [1].map(function (x) { return x; }); return Promise; } }); var p = g.return(v);",
        ),
        // R1 HOLD @ 2144eedf: `PromiseResolve` throws an ordinary error
        // after the stop, so only its rejection is refused (`await_return`'s
        // step 8). Suspended-start, then completed.
        (
            "await-return PromiseResolve throws (suspended start)",
            "var g = (async function* () {})(); var v = Promise.resolve(1); Object.defineProperty(v, 'constructor', { get: function () { arm(); throw 1; } }); var p = g.return(v);",
        ),
        (
            "await-return PromiseResolve throws (completed)",
            "var g = (async function* () {})(); g.next(); var v = Promise.resolve(1); Object.defineProperty(v, 'constructor', { get: function () { arm(); throw 1; } }); var p = g.return(v);",
        ),
        // `return()` into a `finally` that awaits, stopped after the await.
        (
            "return into an awaiting finally",
            "var g = (async function* () { try { yield 1; } finally { await null; arm(); [1].map(function (x) { return x; }); } })(); g.next(); var p = g.return(2);",
        ),
        // `yield*` to an async generator stopped after an await.
        (
            "yield* async",
            "async function* inner() { yield 1; await null; arm(); [1].map(function (x) { return x; }); yield 2; } var g = (async function* () { yield* inner(); yield 3; })(); g.next(); var p = g.next();",
        ),
        // `yield*` to a sync generator stopped mid-body.
        (
            "yield* sync",
            "function* inner() { yield 1; arm(); [1].map(function (x) { return x; }); yield 2; } var g = (async function* () { yield* inner(); yield 3; })(); g.next(); var p = g.next();",
        ),
    ];
    for (tag, source) in cases {
        let mut page = Page::new();
        assert_interrupt_or_ok(page.turn(source), tag);
        // The stopped request settles with the ordinary error.
        page.next_turn();
        if page.get("typeof p") != "undefined" {
            assert_eq!(page.settle("p"), INTERRUPTED_GENERATOR, "{tag}");
        } else {
            assert!(
                tag.contains("PromiseResolve"),
                "{tag}: the script never got `p`"
            );
        }
        // Later calls settle as on a finished generator.
        assert_eq!(page.settle_json("g.next()"), r#"ok {"done":true}"#, "{tag}");
        assert_eq!(
            page.settle_json("g.return(9)"),
            r#"ok {"value":9,"done":true}"#,
            "{tag}"
        );
        assert_eq!(page.settle("g.throw(4)"), "err 4", "{tag}");
        page.assert_recovers(tag);
    }
}

#[test]
fn a_stopped_async_function_settles_its_promise() {
    const FUNCTION: &str = "err Error: async function was interrupted";
    let cases = [
        // Stopped in a body resumed from an await.
        (
            "after an await",
            "var p = (async function () { await null; arm(); [1].map(function (x) { return x; }); return 1; })();",
        ),
        ("a method", "var o = { async m() { await null; arm(); [1].map(function (x) { return x; }); } }; var p = o.m();"),
        ("an arrow", "var p = (async () => { await null; arm(); [1].map(function (x) { return x; }); })();"),
        // The outer function waits on the stopped inner one.
        (
            "nested",
            "async function inner() { await null; arm(); [1].map(function (x) { return x; }); return 1; } var p = (async function () { return await inner(); })();",
        ),
        // It waits on a chain whose script handler the stopped turn refused.
        (
            "awaiting a refused job",
            "var p = (async function () { Promise.resolve().then(function () { arm(); }); await Promise.resolve(1).then(function (v) { return v; }); return 2; })();",
        ),
        // It waits on `Promise.all` whose inputs settle in the stopped turn.
        (
            "awaiting Promise.all",
            "var p = (async function () { Promise.resolve().then(function () { arm(); }); return await Promise.all([Promise.resolve(1), 2]); })();",
        ),
        // `for await` over a sync iterable, an async generator, and a sync
        // generator (async-from-sync) stopped mid-loop.
        (
            "for await over an array",
            "var p = (async function () { for await (var v of [1, 2, 3]) { if (v === 2) { arm(); [1].map(function (x) { return x; }); } } return 'done'; })();",
        ),
        (
            "for await over an async generator",
            "async function* src() { yield 1; yield 2; yield 3; } var p = (async function () { for await (var v of src()) { if (v === 2) { arm(); [1].map(function (x) { return x; }); } } return 'done'; })();",
        ),
        (
            "for await over a sync generator stopped in it",
            "function* src() { yield 1; arm(); [1].map(function (x) { return x; }); yield 2; } var p = (async function () { for await (var v of src()) {} return 'done'; })();",
        ),
        (
            "for await, a refused async-from-sync job",
            "function* src() { yield 1; yield 2; } var p = (async function () { Promise.resolve().then(function () { arm(); }); for await (var v of src()) {} return 'done'; })();",
        ),
        // Closing an async iterator (`break`) whose `finally` is stopped.
        (
            "async iterator close",
            "async function* src() { try { yield 1; yield 2; } finally { arm(); [1].map(function (x) { return x; }); } } var p = (async function () { for await (var v of src()) { break; } return 'done'; })();",
        ),
    ];
    for (tag, source) in cases {
        let mut page = Page::new();
        assert_interrupted(page.turn(source), tag);
        assert_eq!(page.settle("p"), FUNCTION, "{tag}");
        page.assert_recovers(tag);
    }

    // A script handler that is an async function: its `then` promise.
    let mut page = Page::new();
    assert_interrupted(
        page.turn("var p = Promise.resolve().then(async function () { await null; arm(); [1].map(function (x) { return x; }); });"),
        "then(async)",
    );
    assert_eq!(page.settle("p"), "err Error: promise job was interrupted");
    page.assert_recovers("then(async)");
}

#[test]
fn a_sync_generator_or_iterator_stopped_mid_body_completes() {
    let cases = [
        (
            "mid body",
            "function* gen() { yield 1; arm(); [1].map(function (x) { return x; }); yield 2; } var g = gen(); g.next(); g.next();",
        ),
        (
            "in finally through return()",
            "function* gen() { try { yield 1; } finally { arm(); [1].map(function (x) { return x; }); } } var g = gen(); g.next(); g.return(5);",
        ),
        (
            "in a yield* inner generator",
            "function* inner() { yield 1; arm(); [1].map(function (x) { return x; }); yield 2; } function* gen() { yield* inner(); yield 3; } var g = gen(); g.next(); g.next();",
        ),
        (
            "closing it from for-of",
            "function* gen() { try { yield 1; yield 2; } finally { arm(); [1].map(function (x) { return x; }); } } var g = gen(); for (var v of g) { break; }",
        ),
    ];
    for (tag, source) in cases {
        let mut page = Page::new();
        assert_interrupted(page.turn(source), tag);
        page.next_turn();
        assert_eq!(
            page.get("JSON.stringify(g.next())"),
            r#"{"done":true}"#,
            "{tag}"
        );
        assert_eq!(
            page.get("JSON.stringify(g.return(9))"),
            r#"{"value":9,"done":true}"#,
            "{tag}"
        );
        page.assert_recovers(tag);
    }
}

#[test]
fn promise_combinators_settle_in_a_stopped_turn() {
    // Their element functions and a capability's resolve are the engine's
    // own bookkeeping, so they still run after the stop; script does not.
    for (source, later, expected) in [
        ("Promise.all([Promise.resolve(1), 2])", "p", "ok 1,2"),
        (
            "Promise.allSettled([Promise.resolve(1)])",
            "p.then(function (r) { return r[0].status; })",
            "ok fulfilled",
        ),
        (
            "Promise.any([Promise.reject(1)])",
            "p",
            "err AggregateError: no promise in Promise.any was fulfilled.",
        ),
        ("Promise.race([Promise.resolve(1)])", "p", "ok 1"),
    ] {
        let mut page = Page::new();
        let r = page.turn(&format!(
            "var ran = 0; Promise.resolve().then(function () {{ arm(); }}); var p = {source}; p.then(function () {{ ran++; }}, function () {{ ran++; }});"
        ));
        assert_interrupt_or_ok(r, source);
        // No script handler ran after the stop.
        page.next_turn();
        assert_eq!(page.get("ran"), "0", "{source}");
        assert_eq!(page.settle(later), expected, "{source}");
        page.assert_recovers(source);
    }
}

const SUSPENDED_STATES_TURN: &str = r#"
            ps = []; syncs = []; asyncs = []; var n = 0;
            function tick() { if (n++ === K) arm(T); }
            async function leaf(v) { tick(); await null; tick(); return [v].map(function (x) { tick(); return x; })[0]; }
            async function* ag() { try { for (var j = 0; j < 3; j++) { tick(); yield await leaf(j); } } finally { tick(); await null; tick(); } }
            function* sg() { try { for (var j = 0; j < 3; j++) { tick(); yield j; } } finally { tick(); } }
            for (var i = 0; i < 4; i++) {
                ps.push(leaf(i));
                ps.push((async function () { var s = 0; for await (var v of ag()) { tick(); s += v; } return s; })());
                ps.push((async function () { for await (var v of sg()) { tick(); if (v === 1) break; } return 1; })());
                ps.push((async function () { for await (var v of ag()) { tick(); break; } return 1; })());
                ps.push(Promise.resolve(i).then(async function (v) { tick(); return await leaf(v); }));
                ps.push(Promise.all([leaf(i), leaf(i + 1)]).then(function (r) { tick(); return r; }));
                ps.push(Promise.race([leaf(i), new Promise(function (r) { tick(); r(0); })]));
                var a = ag(); asyncs.push(a); ps.push(a.next()); ps.push(a.return(leaf(i)));
                var s = sg(); syncs.push(s); s.next(); tick();
                for (var v of sg()) { tick(); if (v === 1) break; }
            }
            "#;

#[test]
fn every_suspended_state_a_raced_stop_leaves_settles_later() {
    // Each turn makes async functions, async and sync generators, `for
    // await` loops, iterator closes and combinators. It is stopped by
    // another thread after a varied delay, and also, deterministically,
    // at the `tick()` numbered `K`, which moves with the iteration (in its
    // own turn only: what a turn stopped in its script leaves queued runs in
    // the next one). The next turn checks that all of it settles.
    // The turn runs about 80 ticks in its script and 170 more in its jobs
    // (some 17 ms and 15 ms in a debug build): both the delay and `K` span
    // all of it.
    race(
        "suspended states",
        40_000,
        |page, i| {
            page.assert_left_nothing_pending(&format!("suspended states #{i}"));
            page.next_turn();
            let turn = page.turn;
            page.eval(&format!("var K = {}, T = {turn};", (i * 7) % 260))
                .unwrap();
        },
        |_| SUSPENDED_STATES_TURN.into(),
    );
}
