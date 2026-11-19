//! All `<script>`s of a page share one global declarative environment: a function from one
//! script reads a top-level `class`/`const`/`let` that a LATER script declares (Speedometer's
//! Perf-Dashboard: `RemoteAPI is not defined`). The rustkit-js test `global_lexical_scope` pins the
//! semantics on the runtime; this one pins that page scripts reach it.

use super::*;

#[test]
fn a_function_in_one_script_element_reads_a_class_and_a_const_from_the_next() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine.create_headless_view(Bounds::new(0, 0, 200, 100)).expect("view");
    engine
        .load_html(
            id,
            "<html><body><script>function use() { return new RemoteAPI().v + LIMIT; }</script>\
             <script>class RemoteAPI { v = 40; } const LIMIT = 2;</script>\
             <script>document.body.setAttribute('data-r', String(use()));</script></body></html>",
        )
        .expect("load_html");
    let got = engine.execute_script(id, "document.body.getAttribute('data-r')").expect("script");
    assert!(got.contains("42"), "{got}");
}
