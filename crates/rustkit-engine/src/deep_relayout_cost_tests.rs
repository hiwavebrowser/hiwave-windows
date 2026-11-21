//! What one relayout of a deeply nested page costs, by nesting depth (Atlas's LAT row after #1151: on the
//! 300-deep page one relayout sat for seconds in `holds_float` / `collapses_through`). The page is a chain
//! of `depth` nested `position: relative; padding-left: 1px` blocks around one line of text, as in
//! `hit_test_cost_tests`. The timer wraps `Engine::relayout` (style, layout, display list). Each figure is
//! the median of five relayouts. Ignored: run by hand with
//! `cargo test --profile parity -p rustkit-engine --features headless --lib deep_relayout_cost -- --ignored --nocapture`.

use super::*;
use std::time::Instant;

fn page(depth: usize, style: &str) -> String {
    format!(
        "<!doctype html><html><head><style>body{{margin:0;font:12px/16px sans-serif}} .chain{{{style}}}</style></head><body>{}<div class=leaf>leaf</div>{}</body></html>",
        "<div class=chain>".repeat(depth),
        "</div>".repeat(depth)
    )
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
#[ignore]
fn one_relayout_of_a_deeply_nested_page() {
    for (label, style) in [
        ("relative_padding_left", "position:relative;padding-left:1px"),
        ("plain_block", ""),
    ] {
        for depth in [25, 50, 100, 200, 300] {
            let mut engine = Engine::new(EngineConfig::default()).expect("engine");
            let view = engine.create_headless_view(Bounds::new(0, 0, 1280, 800)).expect("view");
            let started = Instant::now();
            engine.load_html(view, &page(depth, style)).expect("load_html");
            let load_ms = started.elapsed().as_secs_f64() * 1e3;
            let mut v = Vec::new();
            for _ in 0..5 {
                let started = Instant::now();
                engine.relayout(view).expect("relayout");
                v.push(started.elapsed().as_secs_f64() * 1e3);
            }
            println!("DEEP_RELAYOUT {label} depth={depth} load_ms={load_ms:.1} relayout_ms={:.2}", median(v));
        }
    }
}

/// The margin-collapse walk's work in one relayout of the chain (`rustkit_layout::margin_walks`).
fn margin_walks_for(depth: usize) -> u64 {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds::new(0, 0, 800, 600)).expect("view");
    engine.load_html(view, &page(depth, "")).expect("load_html");
    let before = rustkit_layout::margin_walks();
    engine.relayout(view).expect("relayout");
    rustkit_layout::margin_walks() - before
}

/// Each block's top-margin chain walks the chain below it, asking at each step whether a box collapses
/// through. Doubling the depth must not more than about quadruple that work (it was x16: one relayout of
/// a 300-deep page took 2.4 s on develop 55f08f46, parity profile, Windows).
#[test]
fn the_margin_walk_grows_no_faster_than_the_square_of_the_depth() {
    // Its own thread with a large stack: a debug build of a 100-deep page overflows a test thread's
    // (DEPTH1: the box-tree builder has no nesting cap).
    let (at_50, at_100) = std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| (margin_walks_for(50), margin_walks_for(100)))
        .unwrap()
        .join()
        .unwrap();
    assert!(at_50 > 0, "the walk was not counted");
    let ratio = at_100 as f64 / at_50 as f64;
    assert!(ratio <= 4.5, "50 deep: {at_50} calls, 100 deep: {at_100} (x{ratio:.1}; the square of the depth is x4)");
}
