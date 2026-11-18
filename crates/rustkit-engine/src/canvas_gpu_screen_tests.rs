//! GPU canvas A1: the canvas on screen drawn by the presenter's GPU replay
//! (switch on) against the same page with the switch off (the CPU bitmap
//! uploaded, today's path) and against Chrome for Testing 148's screenshot
//! (tools/parity_oracle/canvas_screen_cases.json, as
//! `canvas_frames_match_chrome_screenshots`).
//!
//! Gated: the display list names the canvas's GPU texture, the page
//! thread rasterized nothing for it, and the GPU frame is within the
//! interior rule of A3 (every pixel that is not an anti-aliased edge in the
//! CPU frame within 2 per channel). Edge numbers are printed, not gated:
//! A3 pins their bounds from Chrome.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;
use rustkit_layout::DisplayCommand;

fn view(html: &str, gpu: bool) -> (Engine, EngineViewId) {
    rustkit_bindings::canvas::set_gpu_canvas(Some(gpu));
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine.create_headless_view(Bounds::new(0, 0, 1280, 800)).expect("view");
    engine.load_html(id, html).expect("load");
    (engine, id)
}

fn pump(engine: &mut Engine, id: EngineViewId, total: u64) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(id, 16));
        t += 16;
    }
}

/// The frame as RGB, `w` x `h` from the top left (render, then capture).
fn frame_rgb(engine: &mut Engine, id: EngineViewId, w: usize, h: usize) -> Vec<u8> {
    engine.render_view(id).expect("render");
    let path = std::env::temp_dir().join(format!("rustkit-canvas-gpu-{}-{id:?}.ppm", std::process::id()));
    engine.capture_frame(id, path.to_str().unwrap()).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    let mut fields = Vec::new();
    let mut pos = 0;
    while fields.len() < 4 {
        let start = pos;
        while !ppm[pos].is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(std::str::from_utf8(&ppm[start..pos]).unwrap().to_string());
        pos += 1;
    }
    let width: usize = fields[1].parse().unwrap();
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        out.extend_from_slice(&ppm[pos + y * width * 3..][..w * 3]);
    }
    out
}

fn rgba(rgb: &[u8]) -> Vec<u8> {
    rgb.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect()
}

/// (max, mean, share > 2 in %) of the largest channel difference.
fn stats(a: &[u8], b: &[u8]) -> (u8, f64, f64) {
    let d: Vec<u8> = a.chunks(3).zip(b.chunks(3)).map(|(x, y)| x.iter().zip(y).map(|(p, q)| p.abs_diff(*q)).max().unwrap()).collect();
    let max = *d.iter().max().unwrap_or(&0);
    let mean = d.iter().map(|&v| v as f64).sum::<f64>() / d.len().max(1) as f64;
    let over = 100.0 * d.iter().filter(|&&v| v > 2).count() as f64 / d.len().max(1) as f64;
    (max, mean, over)
}

#[test]
fn gpu_canvas_frames_match_the_cpu_frame_inside_and_report_their_edges() {
    let data: serde_json::Value = serde_json::from_str(include_str!("../../../tools/parity_oracle/canvas_screen_cases.json")).unwrap();
    let w = data["size"][0].as_u64().unwrap() as usize;
    let h = data["size"][1].as_u64().unwrap() as usize;
    let mut failures = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let html = case["html"].as_str().unwrap();
        let (mut off_engine, off) = view(html, false);
        pump(&mut off_engine, off, 50);
        let cpu = frame_rgb(&mut off_engine, off, w, h);
        let (mut engine, id) = view(html, true);
        rustkit_bindings::canvas::set_gpu_canvas(None);
        pump(&mut engine, id, 50);
        rustkit_bindings::canvas::set_raster_timing(true);
        let before = rustkit_bindings::canvas::raster_totals();
        let ours = frame_rgb(&mut engine, id, w, h);
        let after = rustkit_bindings::canvas::raster_totals();
        rustkit_bindings::canvas::set_raster_timing(false);
        let list = engine.views[&id].display_list.as_ref().expect("list").commands.clone();
        let gpu_keys = list
            .iter()
            .filter(|c| matches!(c, DisplayCommand::Image { url, .. } if canvas_paint::is_gpu_key(url)))
            .count();
        let hex = case["chrome_rgb"].as_str().unwrap();
        let chrome: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        let d = rustkit_bindings::canvas::compare_premultiplied(&rgba(&ours), &rgba(&cpu), w as u32, h as u32, 2);
        let (cmax, cmean, cover) = stats(&ours, &chrome);
        let (pmax, pmean, pover) = stats(&cpu, &chrome);
        println!(
            "gpu-screen {name}: gpu keys {gpu_keys}, page raster draws {}; GPU vs CPU: differing {:.2}% (interior over 2: {}, edge {} px, max {}); \
             vs Chrome GPU: max {cmax} mean {cmean:.2} >2 {cover:.2}% / CPU: max {pmax} mean {pmean:.2} >2 {pover:.2}%",
            after.draws - before.draws,
            d.differing_pct(),
            d.interior_over,
            d.edge_pixels,
            d.max
        );
        // Every case's canvas draws only what A1 draws on the GPU except
        // drawImage ("draw-image-canvas"): that batch is a reset, the
        // CPU bitmap uploaded into the texture, still under the GPU key.
        if gpu_keys != 1 {
            failures.push(format!("{name}: {gpu_keys} GPU canvas keys in the list"));
        }
        if d.interior_over != 0 {
            failures.push(format!("{name}: {} interior pixels differ from the CPU frame", d.interior_over));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// A canvas drawing every frame without a read: with the switch on, the
/// page thread rasterizes nothing (the display list build hands draws
/// over), and the screen follows each frame.
#[test]
fn an_animated_gpu_canvas_never_rasterizes_on_the_page_thread() {
    let html = r#"<canvas id=c width=64 height=64 style="display:block"></canvas><script>
        const ctx = document.getElementById('c').getContext('2d');
        window.__f = 0;
        window.step = () => {
            ctx.clearRect(0, 0, 64, 64);
            ctx.fillStyle = 'rgb(0,0,0)';
            ctx.beginPath();
            ctx.arc(10 + 4 * window.__f, 32, 8, 0, 6.3);
            ctx.fill();
            window.__f++;
        };
        step();
    </script>"#;
    let (mut engine, id) = view(html, true);
    rustkit_bindings::canvas::set_gpu_canvas(None);
    pump(&mut engine, id, 32);
    rustkit_bindings::canvas::set_raster_timing(true);
    let before = rustkit_bindings::canvas::raster_totals();
    let mut centres = Vec::new();
    for _ in 0..5 {
        engine.execute_script(id, "step()").expect("step");
        pump(&mut engine, id, 16);
        let frame = frame_rgb(&mut engine, id, 64, 64);
        // The darkest column on row 32: the circle's centre.
        let col = (0..64).min_by_key(|&x| frame[(32 * 64 + x) * 3] as u32).unwrap();
        centres.push(col);
    }
    let after = rustkit_bindings::canvas::raster_totals();
    rustkit_bindings::canvas::set_raster_timing(false);
    assert_eq!(after.draws - before.draws, 0, "nothing rasterized on the page thread");
    for pair in centres.windows(2) {
        assert!(pair[1] > pair[0], "the screen follows the frames: {centres:?}");
    }
}
