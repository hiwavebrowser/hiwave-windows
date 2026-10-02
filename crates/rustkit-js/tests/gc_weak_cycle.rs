//! A garbage collection must terminate when a live WeakMap entry's value
//! reaches a reference cycle.
//!
//! boa_gc 0.20's weak-mark phase traced queued nodes without checking or
//! setting their mark bit, so a cycle reachable from an ephemeron value was
//! re-traced forever and the tracer queue grew without bound (tripadvisor,
//! squarespace, bmw and toyota hung inside `Collector::collect` with RSS in
//! the GB). The vendored boa_gc (third_party/boa_gc) backports the fix.
//!
//! The collection runs on a worker thread; if it has not finished within the
//! deadline the process exits non-zero instead of letting the runaway tracer
//! eat the machine's memory.

use rustkit_js::JsRuntime;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn collecting_a_weakmap_whose_value_is_cyclic_terminates() {
    let (done, finished) = mpsc::channel();
    std::thread::spawn(move || {
        let mut rt = JsRuntime::new().expect("runtime");
        rt.evaluate_script(
            r#"
            var key = {};
            var value = { name: "v" };
            value.self = value;                    // a cycle
            value.pair = { back: value };          // and a longer one
            var wm = new WeakMap();
            wm.set(key, value);                    // key stays live (global)
            var ws = new WeakSet(); ws.add(key);
            "#,
        )
        .expect("script");
        boa_gc::force_collect();
        boa_gc::force_collect();
        let alive = rt
            .evaluate_script("wm.get(key).self.pair.back === wm.get(key)")
            .expect("script");
        done.send(format!("{alive:?}")).ok();
    });
    match finished.recv_timeout(Duration::from_secs(5)) {
        Ok(alive) => assert!(alive.contains("true"), "the entry survived: {alive}"),
        Err(_) => {
            eprintln!("GC did not terminate within 5 s (weak-phase cycle)");
            std::process::exit(101);
        }
    }
}
