//! LAYOUT-PER-FRAME, the measurement (no fix): MotionMark's Multiply on the Mac rebuilt the whole
//! layout tree every frame, 72 ms for 11,176 elements, for 2-12 inline style writes a frame (13.5
//! frames per second). The page here is that shape: absolutely positioned tiles in a positioned
//! stage, and an animation-frame loop that writes inline styles on a few tiles each frame.
//!
//! Ignored: it measures, it does not assert. Run with
//! `cargo test --profile parity -p rustkit-engine --lib --features headless -- --ignored layout_per_frame --nocapture`.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// `tiles` absolutely positioned tiles, Multiply's way (a 98-column grid in a 1280x720 stage).
fn page(tiles: usize, script: &str) -> String {
    page_styled(tiles, script, "")
}

fn page_styled(tiles: usize, script: &str, tile_css: &str) -> String {
    let mut p = String::from(
        "<!doctype html><style>body{margin:0}#stage{position:relative;width:1280px;height:720px;overflow:hidden}\
         .t{position:absolute;width:12px;height:12px;background:#08f}",
    );
    p.push_str(&format!(".t{{{tile_css}}}</style><div id=stage>"));
    for i in 0..tiles {
        let (x, y) = ((i % 98) * 13, (i / 98) * 6);
        p.push_str(&format!("<div class=t style=\"left:{x}px;top:{y}px\"></div>"));
    }
    p.push_str("</div><script>var tiles = document.querySelectorAll('.t'), n = 0;");
    p.push_str(script);
    p.push_str("</script>");
    p
}

struct Arm {
    name: &'static str,
    per_frame_ms: f64,
    relayouts: u32,
    reused: u32,
    skipped: u32,
    painted: u32,
    build_ms: f64,
    layout_ms: f64,
    total_ms: f64,
    first_causes: String,
}

fn measure(name: &'static str, tiles: usize, write: &str, mode: Option<crate::incremental_layout::Mode>) -> Arm {
    measure_styled(name, tiles, write, mode, "")
}

fn measure_styled(name: &'static str, tiles: usize, write: &str, mode: Option<crate::incremental_layout::Mode>, tile_css: &str) -> Arm {
    crate::incremental_layout::set_mode_for_test(mode);
    // One loop: each frame writes `write` (with `i` the tile, `n` the frame) on 6 tiles.
    let script = format!(
        "requestAnimationFrame(function f() {{ n++; for (var k = 0; k < 6; k++) {{ var i = (n * 7 + k * 1871) % tiles.length, el = tiles[i]; {write} }} requestAnimationFrame(f); }});"
    );
    let server = serve_routes(vec![("/", "text/html", page_styled(tiles, &script, tile_css))]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1280, height: 720 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let runtime = rt();
    // Settle, then measure.
    for _ in 0..3 {
        runtime.block_on(engine.pump_live(view, 17));
    }
    let before = engine.relayout_stats(view).expect("view").clone();
    const N: u32 = 20;
    let (mut build, mut layout) = (0.0, 0.0);
    let started = std::time::Instant::now();
    for _ in 0..N {
        runtime.block_on(engine.pump_live(view, 17));
        let s = engine.relayout_stats(view).expect("view");
        build += s.last_build_ms;
        layout += s.last_layout_ms;
    }
    let per_frame_ms = started.elapsed().as_secs_f64() * 1000.0 / N as f64;
    let after = engine.relayout_stats(view).expect("view").clone();
    crate::incremental_layout::set_mode_for_test(None);
    let relayouts = after.relayouts - before.relayouts;
    Arm {
        name,
        per_frame_ms,
        relayouts,
        reused: after.trees_reused - before.trees_reused,
        skipped: after.layouts_skipped - before.layouts_skipped,
        painted: after.animation_frames_painted - before.animation_frames_painted,
        build_ms: build / N as f64,
        layout_ms: layout / N as f64,
        total_ms: (after.total_ms - before.total_ms) / N as f64,
        first_causes: format!("{:?}", after.by_first_cause),
    }
}

#[test]
#[ignore]
fn layout_per_frame_multiply_shape() {
    use crate::incremental_layout::Mode;
    let tiles = 11_176;
    let arms = [
        measure("transform+background", tiles, "el.style.transform = 'rotate(' + (n % 360) + 'deg)'; el.style.backgroundColor = 'hsl(' + (n % 360) + ',60%,50%)';", None),
        measure_styled("transform+background, tiles already transformed", tiles, "el.style.transform = 'rotate(' + (n % 360) + 'deg)'; el.style.backgroundColor = 'hsl(' + (n % 360) + ',60%,50%)';", None, "transform:rotate(0deg)"),
        measure("background only", tiles, "el.style.backgroundColor = 'hsl(' + (n % 360) + ',60%,50%)';", None),
        measure("left+top", tiles, "el.style.left = (n % 1200) + 'px'; el.style.top = (n % 700) + 'px';", None),
        measure("data attribute", tiles, "el.setAttribute('data-n', String(n));", None),
        measure("transform+background, incremental on", tiles, "el.style.transform = 'rotate(' + (n % 360) + 'deg)'; el.style.backgroundColor = 'hsl(' + (n % 360) + ',60%,50%)';", Some(Mode::On)),
        measure("left+top, incremental on", tiles, "el.style.left = (n % 1200) + 'px'; el.style.top = (n % 700) + 'px';", Some(Mode::On)),
        measure("transform+background, 1,000 tiles", 1_000, "el.style.transform = 'rotate(' + (n % 360) + 'deg)'; el.style.backgroundColor = 'hsl(' + (n % 360) + ',60%,50%)';", None),
    ];
    eprintln!("LAYOUT-PER-FRAME, 20 frames per arm, 6 tiles written a frame:");
    for a in &arms {
        eprintln!(
            "  {:<40} {:>7.2} ms/frame (pump)  relayouts {:>2}  reused {:>2}  layout-skipped {:>2}  paint-only {:>2}  build {:>6.2}  layout {:>6.2}  relayout total {:>6.2} ms/frame  first causes {}",
            a.name, a.per_frame_ms, a.relayouts, a.reused, a.skipped, a.painted, a.build_ms, a.layout_ms, a.total_ms, a.first_causes
        );
    }
}
