//! CANVAS-SHIM: what one canvas 2D call costs in the bindings' own JavaScript (web_canvas.js), counted
//! in interpreter opcodes (`RUSTKIT_VM_STATS=1`), not timed. MotionMark's canvas subtests make 3,500 to
//! 12,000 canvas calls a frame at 1000 elements; on the SCRIPT-SPLIT build the shim was ~1.9 us of Boa
//! per call against ~0.25 us of the page's own JS (docs/diagnostics/2026-11-17/motionmark_stage_split.md).
//! The count is the opcodes a loop of calls on the real context runs less the same loop on a plain
//! object with empty methods, per call. The flag is read once per process, so this is the only test
//! in its binary and sets it first.

use rustkit_bindings::DomBindings;
use rustkit_dom::Document;
use rustkit_js::{vm_stats_opcode_count, vm_stats_report, JsRuntime, JsValue};
use std::rc::Rc;

/// The interpreter's opcode total so far (this thread).
fn opcodes() -> u64 {
    let report = vm_stats_report();
    let line = report.lines().find(|l| l.starts_with("vm stats: ")).unwrap_or_else(|| panic!("no total:\n{report}"));
    line["vm stats: ".len()..].split(' ').next().unwrap().parse().unwrap()
}

/// How many times opcode `name` ran so far on this thread; fails when the counter has no such
/// opcode (a misspelt name, or the counters off), never reading 0.
fn opcode(name: &str) -> u64 {
    vm_stats_opcode_count(name).unwrap_or_else(|| panic!("no opcode count for {name}"))
}

const N: u64 = 1000;

#[test]
fn a_canvas_call_runs_few_opcodes_of_shim_and_makes_no_arguments_object() {
    assert_eq!(std::env::var_os("RUSTKIT_VM_STATS"), None, "run this test without the flag in the environment");
    // SAFETY: the only test in this binary; nothing else reads the environment concurrently.
    unsafe { std::env::set_var("RUSTKIT_VM_STATS", "1") };
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<!DOCTYPE html><html><body><canvas id=c width=200 height=100></canvas></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate(
        "var ctx = document.getElementById('c').getContext('2d');
         var stub = { strokeStyle: '', fillStyle: '', lineWidth: 1, beginPath: function () {}, closePath: function () {},
           moveTo: function () {}, lineTo: function () {}, arc: function () {}, quadraticCurveTo: function () {},
           bezierCurveTo: function () {}, rect: function () {}, stroke: function () {}, fill: function () {},
           fillRect: function () {}, clearRect: function () {} };
         function frame(c, n) {
           for (var i = 0; i < n; i++) {
             c.strokeStyle = '#e01040'; c.lineWidth = 3; c.beginPath(); c.moveTo(1, 2); c.lineTo(3, 4);
             c.arc(50, 50, 20, 0, 3, false); c.quadraticCurveTo(1, 2, 3, 4); c.stroke();
             c.fillStyle = '#101010'; c.fill(); c.clearRect(0, 0, 1, 1);
           }
         }
         frame(ctx, 10); frame(stub, 10);",
    )
    .unwrap();
    // Calls per iteration: strokeStyle, lineWidth, beginPath, moveTo, lineTo, arc, quadraticCurveTo,
    // stroke, fillStyle, fill, clearRect.
    const CALLS: u64 = 11;
    let run = |target: &str| -> (u64, u64, u64) {
        let (ops, mapped, unmapped) = (opcodes(), opcode("CreateMappedArgumentsObject"), opcode("CreateUnmappedArgumentsObject"));
        match b.evaluate(&format!("frame({target}, {N}); 'ok'")).unwrap() {
            JsValue::String(s) if s == "ok" => {}
            other => panic!("{other:?}"),
        }
        (opcodes() - ops, opcode("CreateMappedArgumentsObject") - mapped, opcode("CreateUnmappedArgumentsObject") - unmapped)
    };
    assert_eq!(vm_stats_opcode_count("NotAnOpcode"), None, "a name that is no opcode must not read as a count");
    let (stub_ops, _, _) = run("stub");
    let (real_ops, mapped, unmapped) = run("ctx");
    let per_call = (real_ops - stub_ops) as f64 / (N * CALLS) as f64;
    println!("shim opcodes per call {per_call:.1}; mapped arguments objects {mapped}, unmapped {unmapped}");
    // A sloppy-mode `arguments` object (mapped to the parameters) is the dearest kind to make; the
    // shim needs the argument count only.
    assert_eq!(mapped, 0, "the shim made {mapped} mapped arguments objects in {} calls", N * CALLS);
    assert!(per_call <= SHIM_OPCODES_PER_CALL, "{per_call:.1} shim opcodes per canvas call (bound {SHIM_OPCODES_PER_CALL})");
}

/// The bound on shim opcodes per call: 124.5 before CANVAS-SHIM (6,062 mapped arguments objects in
/// the 11,000 calls), 42.8 after.
const SHIM_OPCODES_PER_CALL: f64 = 50.0;
