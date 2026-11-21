//! What a frame costs while ~200 elements transition at once (R1 on
//! #1288): the group C properties (`border-radius`, `line-height`,
//! `background-size`, a `width` between two units) take the full relayout
//! path every frame, where before they jumped and cost no frame at all.
//! `background-color` (the paint-only frame path) is the reference.
//!
//! Ignored: a measurement, not a check. Run one case at a time, base and
//! head on the same machine and profile:
//! `cargo test -p rustkit-engine --lib --features headless css_transitions_cost -- --ignored --nocapture`
//! It prints one line per case: frames, the median and max ms of one live
//! turn of 16 ms, and the full relayouts and kept-layout frames it took.

use super::*;
use crate::script_net_tests::serve_routes;

const ELEMENTS: usize = 200;
const FRAMES: usize = 30;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn measure(label: &str, from: &str, to: &str) {
    let cells: String = (0..ELEMENTS).map(|i| format!("<div class=c>cell {i}</div>")).collect();
    let page = format!(
        "<!doctype html><style>body{{margin:0;font:12px sans-serif}}\
.c{{display:inline-block;width:100px;height:20px;margin:2px;line-height:20px;\
background:linear-gradient(red,blue) no-repeat;background-size:10px 10px;background-color:rgb(0,0,255);\
transition:all 2000ms linear;{from}}}\
body.on .c{{{to}}}</style>{cells}"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = rt();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    rt.block_on(engine.pump_live(view, 16));
    engine
        .execute_script(view, "document.body.classList.add('on');getComputedStyle(document.body).color")
        .unwrap();
    let stats = |e: &Engine| {
        let s = e.relayout_stats(view).expect("view");
        (s.relayouts, s.layouts_skipped)
    };
    let (r0, k0) = stats(&engine);
    let mut ms = Vec::with_capacity(FRAMES);
    for _ in 0..FRAMES {
        let start = std::time::Instant::now();
        rt.block_on(engine.pump_live(view, 16));
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let (r1, k1) = stats(&engine);
    ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let running = engine
        .execute_script(view, "String(document.getAnimations().length)")
        .unwrap();
    println!(
        "COST {label}: elements={ELEMENTS} frames={FRAMES} median_ms={:.2} max_ms={:.2} \
         full_relayouts={} kept_layout_frames={} animations={running}",
        ms[FRAMES / 2],
        ms[FRAMES - 1],
        r1 - r0,
        k1 - k0,
    );
}

#[test]
#[ignore]
fn css_transitions_cost_background_color_reference() {
    measure("background-color", "", "background-color:rgb(255,0,0)");
}

#[test]
#[ignore]
fn css_transitions_cost_border_radius() {
    measure("border-radius", "border-radius:0px", "border-radius:10px");
}

#[test]
#[ignore]
fn css_transitions_cost_line_height() {
    measure("line-height", "", "line-height:40px");
}

#[test]
#[ignore]
fn css_transitions_cost_background_size() {
    measure("background-size", "", "background-size:30px 30px");
}

#[test]
#[ignore]
fn css_transitions_cost_width_two_units() {
    measure("width px->%", "", "width:8%");
}

/// Group G: each cell a two-column grid whose columns move (relayout path,
/// as group C's).
#[test]
#[ignore]
fn css_transitions_cost_grid_template_columns() {
    measure(
        "grid-template-columns",
        "display:inline-grid;grid-template-columns:30px 70px",
        "grid-template-columns:60px 40px",
    );
}
