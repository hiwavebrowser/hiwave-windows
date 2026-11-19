//! STR-7b frame cost, on demand (`--ignored`): MotionMark-style canvas
//! scenes (a 900x600 canvas cleared and redrawn every frame) timed through
//! the engine, headless. Per frame it prints the median of
//!   script: `execute_script("frame()")`: the JS draws (rasterized as they
//!     run), then the display list rebuild, which reads the bitmap;
//!   render: `render_view` then `capture_frame` (to a temp PPM): the
//!     texture upload, the GPU draw, and a readback that waits for it
//!     (the readback and PPM write are in the number; they are the same
//!     on both sides of a comparison).
//! Test builds are opt-level 0 for workspace crates except rustkit-bindings
//! and rustkit-js (2); dependencies such as tiny-skia are at 2.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const SCENES: [(&str, &str); 3] = [
    ("arcs-200", "for (var i = 0; i < 200; i++) { x.strokeStyle = 'hsl(' + (i * 7 + f) % 360 + ',80%,50%)'; x.lineWidth = 1 + i % 5; \
        x.beginPath(); x.arc(r() * 900, r() * 600, 10 + r() * 60, r() * 6.28, r() * 6.28); x.stroke(); }"),
    ("lines-500", "for (var i = 0; i < 500; i++) { x.strokeStyle = 'hsl(' + (i * 3 + f) % 360 + ',70%,45%)'; x.lineWidth = 1 + i % 3; \
        x.beginPath(); x.moveTo(r() * 900, r() * 600); x.lineTo(r() * 900, r() * 600); x.stroke(); }"),
    ("rects-1000", "for (var i = 0; i < 1000; i++) { x.fillStyle = 'rgba(' + (i % 255) + ',' + (f % 255) + ',150,0.5)'; \
        x.fillRect(r() * 880, r() * 580, 20, 20); }"),
];

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

#[test]
#[ignore]
fn canvas_frame_cost() {
    for (name, body) in SCENES {
        let html = format!(
            "<!doctype html><body style='margin:0'><canvas id=c width=900 height=600></canvas><script>\
             var x = document.getElementById('c').getContext('2d'), f = 0, s = 1; \
             function r() {{ s = (s * 16807) % 2147483647; return s / 2147483647; }} \
             function frame() {{ f++; x.clearRect(0, 0, 900, 600); {body} }}</script></body>"
        );
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine.create_headless_view(Bounds::new(0, 0, 1000, 700)).expect("view");
        engine.load_html(id, &html).expect("load");
        let (mut script, mut render) = (Vec::new(), Vec::new());
        let ppm = std::env::temp_dir().join(format!("str7b-cost-{}.ppm", std::process::id()));
        for k in 0..45 {
            let t0 = std::time::Instant::now();
            engine.execute_script(id, "frame()").expect("script");
            let t1 = std::time::Instant::now();
            engine.render_view(id).expect("render");
            engine.capture_frame(id, ppm.to_str().unwrap()).expect("capture");
            let t2 = std::time::Instant::now();
            if k >= 5 {
                script.push((t1 - t0).as_secs_f64() * 1e3);
                render.push((t2 - t1).as_secs_f64() * 1e3);
            }
        }
        let _ = std::fs::remove_file(&ppm);
        println!("STR7B-COST {name}: script+list median {:.2} ms, render median {:.2} ms (40 frames)", median(script), median(render));
    }
}
