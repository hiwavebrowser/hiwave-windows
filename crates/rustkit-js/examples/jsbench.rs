//! JIT-plan measurement harness: runs a JetStream-style workload (script files that end by
//! defining `class Benchmark { runIteration() }`) in a fresh `JsRuntime` and prints one JSON line
//! with each iteration's wall time in ms. The timer wraps one `runIteration()` call only (no load,
//! no parse). `RUSTKIT_VM_STATS=1` adds the interpreter's opcode counters to stderr.
//!
//! cargo run --release -p rustkit-js --example jsbench -- --iterations 10 [--data input.js] a.js b.js ...

use rustkit_js::JsRuntime;
use std::time::Instant;

fn main() {
    let mut iterations = 10usize;
    let mut files = Vec::new();
    // `--data path`: the file's text is what the workload's `read(path)` returns (a JetStream
    // shell workload reads its inputs that way); it is loaded before the timer starts.
    let mut data = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--iterations" {
            iterations = args.next().and_then(|n| n.parse().ok()).expect("--iterations N");
        } else if a == "--data" {
            data.push(args.next().expect("--data path"));
        } else {
            files.push(a);
        }
    }
    let mut rt = JsRuntime::new().expect("runtime");
    let mut table = serde_json::Map::new();
    for d in &data {
        let text = std::fs::read_to_string(d).unwrap_or_else(|e| panic!("{d}: {e}"));
        table.insert(d.clone(), serde_json::Value::String(text));
    }
    let prelude = format!(
        "globalThis.isInBrowser = false; globalThis.__data = {}; \
         globalThis.read = function (p) {{ if (!(p in __data)) throw new Error('read: ' + p + ' not given with --data'); return __data[p]; }};",
        serde_json::Value::Object(table)
    );
    rt.evaluate_script(&prelude).expect("prelude");
    // Page globals a bare runtime lacks and these workloads use: Annex B's `String.prototype.substr`
    // and friends, and `escape`/`unescape`, from the bindings' own scripts (the code pages run); and `performance.now` at Date.now resolution
    // (splay reads it only for its latency figure, outside what the timer here measures).
    rt.evaluate_script(include_str!("../../rustkit-bindings/src/web_legacy.js")).expect("web_legacy.js");
    rt.evaluate_script(include_str!("../../rustkit-bindings/src/web_encoding.js")).expect("web_encoding.js");
    rt.evaluate_script("if (typeof performance === 'undefined') globalThis.performance = { now: function () { return Date.now(); } };")
        .expect("performance");
    let load = Instant::now();
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{f}: {e}"));
        if let Err(e) = rt.evaluate_script(&src) {
            panic!("{f}: {e:?}");
        }
    }
    let load_ms = load.elapsed().as_secs_f64() * 1e3;
    rt.evaluate_script("globalThis.__bench = new Benchmark();").expect("new Benchmark()");
    let mut times = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let t = Instant::now();
        if let Err(e) = rt.evaluate_script("__bench.runIteration(); undefined") {
            panic!("runIteration: {e:?}");
        }
        times.push(t.elapsed().as_secs_f64() * 1e3);
    }
    let list: Vec<String> = times.iter().map(|t| format!("{t:.2}")).collect();
    println!("{{\"engine\":\"rustkit\",\"load_ms\":{load_ms:.1},\"iteration_ms\":[{}]}}", list.join(","));
    if std::env::var_os("RUSTKIT_VM_STATS").is_some() {
        eprintln!("{}", rustkit_js::vm_stats_report());
    }
}
