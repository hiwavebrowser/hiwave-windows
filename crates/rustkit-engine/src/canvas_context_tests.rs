//! Z plan CH2.B4: HiWave's report page (report.html, Chart.js 4.4.0) shows
//! "Failed to load analytics data: cannot convert 'null' or 'undefined' to
//! object" in RustKit and two charts in Chromium 143.
//!
//! The cause is not `getComputedStyle`: `HTMLCanvasElement.getContext` returns
//! null for every kind (`rustkit-bindings/src/dom.rs`), so the first
//! `new Chart(ctx)` cannot acquire a context (console error, a chart with no
//! canvas), and the second reads `o.canvas.id` on that chart's null canvas
//! while it checks "Canvas is already in use". Boa reports any property read
//! on null with that one message, which is why it looked like a
//! getPropertyValue bug.
//!
//! Chromium 143: `getContext('2d')` returns a CanvasRenderingContext2D whose
//! `canvas` is the element, the same object on every call; report.html's two
//! charts are created and its error container stays empty.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const CHART_JS: &str = include_str!("../../hiwave-app/src/ui/chart.umd.min.js");
const REPORT_HTML: &str = include_str!("../../hiwave-app/src/ui/report.html");

fn eval(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let value = engine.execute_script(id, script).expect("script");
    let inner = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value);
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

fn view(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("view");
    engine.load_html(id, html).expect("load");
    (engine, id)
}

#[test]
fn canvas_2d_context_is_the_canvas_own_and_stable() {
    let (mut engine, id) = view("<!doctype html><canvas id=c width=300 height=150></canvas>");
    let read = eval(
        &mut engine,
        id,
        "var c = document.getElementById('c'); var x = c.getContext('2d'); \
         JSON.stringify([x !== null, x && x.canvas === c, c.getContext('2d') === x, \
                         String(x && Object.prototype.toString.call(x))])",
    );
    assert_eq!(
        read, r#"[true,true,true,"[object CanvasRenderingContext2D]"]"#,
        "Chromium 143: a context, its canvas is the element, the same object again"
    );
}

/// The page as the app serves it: Chart.js where the app injects it, a host
/// that answers `get_today_stats` the way the real one does (a
/// `hiwave-response` event after the post).
#[test]
fn report_page_creates_both_charts_without_an_error() {
    let host = "<script>window.ipc = { postMessage: function (raw) { \
        var cmd = JSON.parse(raw).cmd; if (cmd !== 'get_today_stats') return; \
        Promise.resolve().then(function () { window.dispatchEvent(new CustomEvent('hiwave-response', \
        { detail: { type: 'success', data: { pages_visited: 42, trackers_blocked: 311, ads_blocked: 97, \
        popups_blocked: 3, bandwidth_saved: 0, time_saved: 0, focus_time: 0 } } })); }); } };\
        window.__errs = []; console.error = function (m) { window.__errs.push(String(m)); };</script>";
    let marker = "<!-- Chart.js is injected by Rust before page load -->";
    assert!(REPORT_HTML.contains(marker), "report.html's injection marker");
    let html = REPORT_HTML.replacen(marker, &format!("{host}<script>{CHART_JS}</script>"), 1);
    let (mut engine, id) = view(&html);
    let read = eval(
        &mut engine,
        id,
        "JSON.stringify([document.getElementById('error-container').textContent.trim(), \
                         window.__errs, document.getElementById('stat-trackers').textContent, \
                         !!(activityChart && activityChart.canvas), !!(protectionChart && protectionChart.canvas)])",
    );
    assert_eq!(
        read, r#"["",[],"311",true,true]"#,
        "Chromium 143: no error banner, no console error, both charts on their canvases"
    );
}

// ---- CH2.E10 phase 1: the canvas as a replaced box, and what it paints.

use rustkit_layout::DisplayCommand;

fn list(engine: &Engine, id: EngineViewId) -> Vec<DisplayCommand> {
    engine.views[&id].display_list.as_ref().expect("a display list").commands.clone()
}

fn pump(engine: &mut Engine, id: EngineViewId, total: u64) {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(id, 16));
        t += 16;
    }
}

/// Chromium 143: a canvas with no attributes is 300x150; `width`/`height`
/// attributes are its natural size and CSS width stretches it at that
/// ratio; CSS width and height together stretch it freely. Setting
/// `canvas.width`/`height` resizes the box.
#[test]
fn a_canvas_lays_out_at_its_bitmap_size_unless_css_says_otherwise() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><canvas id=a></canvas>\
         <canvas id=b width=100 height=50 style='width:200px'></canvas>\
         <canvas id=c width=100 height=50 style='display:block;width:60px;height:70px'></canvas></body>",
    );
    let read = eval(
        &mut engine,
        id,
        "JSON.stringify(['a','b','c'].map(function (i) { var r = document.getElementById(i).getBoundingClientRect(); \
         return [r.x, r.width, r.height]; }))",
    );
    assert_eq!(read, "[[0,300,150],[300,200,100],[0,60,70]]", "Chromium 143 sizes and x");
    let read = eval(
        &mut engine,
        id,
        "var a = document.getElementById('a'); a.width = 40; a.height = 30; \
         var r = a.getBoundingClientRect(); JSON.stringify([r.width, r.height])",
    );
    assert_eq!(read, "[40,30]", "Chromium 143: the box follows the bitmap");
}

/// A 100x50 bitmap shown 200x100 at (0, 10): bitmap rect (10, 5, 20, 10)
/// is page rect (20, 20, 40, 20). Drawn after load, so it reaches the
/// list through the repaint a drawing canvas asks for.
#[test]
fn fill_rect_paints_at_page_coordinates() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><div style='height:10px'></div>\
         <canvas id=c width=100 height=50 style='display:block;width:200px;height:100px'></canvas></body>",
    );
    assert!(
        !list(&engine, id).iter().any(|c| matches!(c, DisplayCommand::FillRect { .. })),
        "nothing drawn yet"
    );
    engine
        .execute_script(id, "var x = document.getElementById('c').getContext('2d'); x.fillStyle = '#ff0000'; x.fillRect(10, 5, 20, 10);")
        .expect("script");
    // The bitmap is scaled 2x bilinear (STR-7b), so its edges blend over
    // a pixel; inside and outside the page rect are exact.
    let frame = frame_rgb(&mut engine, id, 100, 60);
    let at = |x: usize, y: usize| [frame[(y * 100 + x) * 3], frame[(y * 100 + x) * 3 + 1], frame[(y * 100 + x) * 3 + 2]];
    for (x, y) in [(22, 22), (40, 30), (57, 37)] {
        assert_eq!(at(x, y), [255, 0, 0], "inside page rect (20, 20, 40, 20) at ({x}, {y})");
    }
    for (x, y) in [(17, 30), (63, 30), (40, 17), (40, 43)] {
        assert_eq!(at(x, y), [255, 255, 255], "outside it at ({x}, {y})");
    }
}

/// fillText is a text run at the transformed baseline, its font scaled with
/// the bitmap (drawn over the bitmap until slice 3 rasterizes text); a
/// stroked path is in the bitmap.
#[test]
fn fill_text_and_stroke_paint_text_and_lines() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><canvas id=c width=200 height=100 style='display:block'></canvas>\
         <script>var x = document.getElementById('c').getContext('2d'); x.font = '20px sans-serif'; x.fillStyle = '#0000ff'; \
         x.fillText('Hi canvas', 10, 50); x.beginPath(); x.moveTo(0, 90); x.lineTo(200, 90); x.lineWidth = 2; x.stroke();</script></body>",
    );
    let commands = list(&engine, id);
    let text = commands.iter().find_map(|c| match c {
        DisplayCommand::Text { text, x, y, font_size, color, .. } if text == "Hi canvas" => Some((*x, *y, *font_size, *color)),
        _ => None,
    });
    assert_eq!(text, Some((10.0, 50.0, 20.0, rustkit_css::Color::from_rgb(0, 0, 255))));
    let frame = frame_rgb(&mut engine, id, 200, 100);
    for x in [1, 100, 198] {
        for y in [89, 90] {
            assert_eq!(&frame[(y * 200 + x) * 3..][..3], &[0, 0, 0], "the 2px line at ({x}, {y})");
        }
        assert_eq!(&frame[(87 * 200 + x) * 3..][..3], &[255, 255, 255], "above the line at x {x}");
    }
}

/// Chromium 143: measureText('Pages Visited 311') at 20px sans-serif is
/// 159.39 and a span of it 159.41; bold 12px Arial 'Trackers' 49.38 and
/// 49.39. Here the two must agree within 1px: the same shaper.
#[test]
fn measure_text_matches_a_span_in_the_same_font() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><canvas id=c></canvas>\
         <span id=s style='font:20px sans-serif'>Pages Visited 311</span> <span id=t style='font:bold 12px Arial'>Trackers</span></body>",
    );
    let read = eval(
        &mut engine,
        id,
        "var x = document.getElementById('c').getContext('2d'); x.font = '20px sans-serif'; var a = x.measureText('Pages Visited 311').width; \
         x.font = 'bold 12px Arial'; var b = x.measureText('Trackers').width; \
         JSON.stringify([a, document.getElementById('s').getBoundingClientRect().width, b, document.getElementById('t').getBoundingClientRect().width])",
    );
    let v: Vec<f64> = read.trim_matches(['[', ']']).split(',').map(|n| n.parse().unwrap()).collect();
    assert!((v[0] - v[1]).abs() <= 1.0, "measureText {} vs span {}", v[0], v[1]);
    assert!((v[2] - v[3]).abs() <= 1.0, "measureText {} vs span {}", v[2], v[3]);
    assert!((v[0] - 159.39).abs() <= 3.0 && (v[2] - 49.38).abs() <= 2.0, "near Chromium's widths: {read}");
}

/// report.html's bar chart, once its animation has run: the canvas box's
/// part of the display list has the legend's labels and the bars.
#[test]
fn report_page_charts_paint_into_their_canvases() {
    let host = "<script>window.ipc = { postMessage: function (raw) { \
        var cmd = JSON.parse(raw).cmd; if (cmd !== 'get_today_stats') return; \
        Promise.resolve().then(function () { window.dispatchEvent(new CustomEvent('hiwave-response', \
        { detail: { type: 'success', data: { pages_visited: 42, trackers_blocked: 311, ads_blocked: 97, \
        popups_blocked: 3, bandwidth_saved: 0, time_saved: 0, focus_time: 0 } } })); }); } };</script>";
    let marker = "<!-- Chart.js is injected by Rust before page load -->";
    let html = REPORT_HTML.replacen(marker, &format!("{host}<script>{CHART_JS}</script>"), 1);
    let (mut engine, id) = view(&html);
    pump(&mut engine, id, 1500);
    let rect = eval(
        &mut engine,
        id,
        "var r = document.getElementById('protectionChart').getBoundingClientRect(); JSON.stringify([r.x, r.y + window.scrollY, r.width, r.height])",
    );
    let r: Vec<f32> = rect.trim_matches(['[', ']']).split(',').map(|n| n.parse().unwrap()).collect();
    assert!(r[2] > 0.0 && r[3] > 0.0, "the canvas has a box: {rect}");
    let inside = |x: f32, y: f32| x >= r[0] - 1.0 && x <= r[0] + r[2] + 1.0 && y >= r[1] - 1.0 && y <= r[1] + r[3] + 1.0;
    let commands = list(&engine, id);
    let labels: Vec<String> = commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::Text { text, x, y, .. } if inside(*x, *y) => Some(text.clone()),
            _ => None,
        })
        .collect();
    for label in ["Trackers", "Ads", "Popups"] {
        assert!(labels.iter().any(|l| l == label), "legend label {label} in {labels:?}");
    }
    // The bars are in the canvas bitmap (STR-7b): it is painted inside the
    // box, and holds coloured (not grey) pixels.
    assert!(
        commands.iter().any(|c| matches!(c, DisplayCommand::Image { url, dest_rect, .. }
            if canvas_paint::is_bitmap_key(url) && inside(dest_rect.x, dest_rect.y))),
        "the canvas bitmap is painted in its box"
    );
    let source = engine.views[&id].canvases.clone().expect("canvases");
    let node = source_node(&mut engine, id, "protectionChart");
    let (_, _, px) = source.pixels(node).expect("a context");
    let coloured = px
        .chunks(4)
        .filter(|p| p[3] > 128 && p.iter().take(3).max().unwrap() - p.iter().take(3).min().unwrap() > 60)
        .count();
    assert!(coloured >= 200, "the datasets' bars are filled: {coloured} coloured pixels");
}

// ---- PR #905 R1 HOLD: partial clears paint, the paint is cached, and an
// animation loop stays bounded.

/// Chromium 143: fillRect(0, 0, 100, 50) red then clearRect(0, 0, 50, 50)
/// leaves red only in the right half (2500 px of a 100x50 bitmap); a
/// second clearRect(75, 0, 25, 25) takes a further 625 px out of it, and a
/// blue fillRect drawn after the clears is not cut by them.
#[test]
fn a_partial_clear_rect_erases_what_it_covers() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><canvas id=c width=100 height=50 style='display:block'></canvas></body>",
    );
    engine
        .execute_script(
            id,
            "var x = document.getElementById('c').getContext('2d'); x.fillStyle = '#ff0000'; x.fillRect(0, 0, 100, 50); \
             x.clearRect(0, 0, 50, 50); x.clearRect(75, 0, 25, 25); x.fillStyle = '#0000ff'; x.fillRect(0, 40, 100, 10);",
        )
        .expect("script");
    // Counted in the frame: the bitmap is drawn 1:1 (STR-7b).
    let frame = frame_rgb(&mut engine, id, 100, 50);
    let count = |c: [u8; 3]| frame.chunks(3).filter(|p| *p == c).count();
    assert_eq!((count([255, 0, 0]), count([0, 0, 255])), (2500 - 625 - 500, 1000), "red under the blue band is covered");
}

/// Listing the page again with the canvas unchanged reuses the canvas's
/// paint; a draw makes it paint once more.
#[test]
fn an_unchanged_canvas_is_painted_once() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><canvas id=c width=100 height=50></canvas>\
         <script>document.getElementById('c').getContext('2d').fillRect(0, 0, 10, 10);</script></body>",
    );
    let root = engine.views[&id].layout.clone().expect("laid out");
    engine.display_list_for(id, &root);
    let before = canvas_paint::paints();
    for _ in 0..5 {
        engine.display_list_for(id, &root);
    }
    assert_eq!(canvas_paint::paints(), before, "five lists, the canvas unchanged");
    engine.execute_script(id, "document.getElementById('c').getContext('2d').fillRect(20, 0, 10, 10);").expect("script");
    let root = engine.views[&id].layout.clone().expect("laid out");
    let mid = canvas_paint::paints();
    engine.display_list_for(id, &root);
    engine.display_list_for(id, &root);
    assert!(canvas_paint::paints() <= mid + 1, "one paint for the change");
}

/// 10k frames of a Chart.js-style animation (whole-bitmap clear, a fresh
/// gradient, clipped bars, labels; and a second canvas that clears only
/// its plot area) keep each recording to one frame and the display list
/// the same length.
#[test]
fn ten_thousand_animation_frames_stay_bounded() {
    let (mut engine, id) = view(
        "<!doctype html><body style='margin:0'><canvas id=a width=300 height=150></canvas><canvas id=b width=300 height=150></canvas>\
         <script>var A = document.getElementById('a').getContext('2d'), B = document.getElementById('b').getContext('2d'); var f = 0; \
         B.beginPath(); B.moveTo(0, 140); B.lineTo(300, 140); B.stroke(); B.fillText('axis', 0, 150); \
         function frame() { f++; A.clearRect(0, 0, 300, 150); var g = A.createLinearGradient(0, 0, 0, 150); \
           g.addColorStop(0, 'rgba(54,162,235,0.5)'); g.addColorStop(1, '#fff'); \
           A.save(); A.beginPath(); A.rect(0, 0, 300, 130); A.clip(); \
           for (var i = 0; i < 3; i++) { A.fillStyle = i ? '#36a2eb' : g; A.fillRect(20 + i * 90, 130 - (20 + (f + i * 13) % 100), 60, 20 + (f + i * 13) % 100); } \
           A.restore(); A.fillStyle = '#666'; A.font = '12px Arial'; A.fillText('Trackers', 20, 145); \
           B.save(); B.beginPath(); B.rect(0, 0, 300, 130); B.clip(); B.clearRect(0, 0, 300, 130); \
           for (var j = 0; j < 3; j++) { B.fillStyle = 'rgba(255,99,132,0.5)'; B.fillRect(20 + j * 90, 30 + (f + j) % 50, 60, 100 - (f + j) % 50); } \
           B.restore(); } \
         function frames(n) { for (var k = 0; k < n; k++) frame(); }</script></body>",
    );
    let source = engine.views[&id].canvases.clone().expect("canvases");
    let mut lengths = Vec::new();
    for _ in 0..10 {
        engine.execute_script(id, "frames(1000)").expect("script");
        lengths.push(list(&engine, id).len());
        for node in [source_node(&mut engine, id, "a"), source_node(&mut engine, id, "b")] {
            let n = source.snapshot(node).expect("a context").commands.len();
            assert!(n <= 16, "recording of {n} commands after {} frames", lengths.len() * 1000);
        }
        // The recordings, plus the two 300x150 bitmaps counted with them.
        assert!(source.retained_bytes() <= 64 * 1024 + 2 * 300 * 150 * 4, "{} bytes", source.retained_bytes());
    }
    assert!(lengths.windows(2).all(|w| w[0] == w[1]), "display list lengths {lengths:?}");
}

fn source_node(engine: &mut Engine, id: EngineViewId, element: &str) -> usize {
    let view = &engine.views[&id];
    let doc = view.document.as_ref().expect("document");
    doc.get_element_by_id(element).expect("element").id.raw()
}

/// STR-7 slice 2: drawImage of a loaded `<img>` (a data: PNG) at 1:1, a
/// rect and a scaled-up sub-rect, read back with getImageData, against
/// Chrome for Testing 148 (tools/parity_oracle/canvas_img_cases.json,
/// canvas_img_log.mjs). The image cache is where the canvas reads the
/// decoded pixels.
#[test]
fn draw_image_of_a_loaded_img_matches_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/canvas_img_cases.json")).unwrap();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (mut engine, id) = view(case["html"].as_str().unwrap());
        let mut ours = String::new();
        for _ in 0..40 {
            pump(&mut engine, id, 50);
            ours = eval(&mut engine, id, "typeof window.__r === 'string' ? window.__r : ''");
            if !ours.is_empty() {
                break;
            }
        }
        assert!(!ours.is_empty(), "{name}: the image never loaded or drew");
        let chrome = case["chrome_rgba"].as_str().unwrap();
        let px = |h: &str| -> Vec<u8> { (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()).collect() };
        let (a, b) = (px(&ours), px(chrome));
        assert_eq!(a.len(), b.len(), "{name}: length");
        let max = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        // The 1:1 draws are exact; the scaled-up sub-rect is bilinear, and
        // tiny-skia's 8-bit bilinear rounds differently from Skia's (11 here,
        // 15 in canvas_raster2_cases.json "draw-image-scaled-up").
        assert!(max <= 16, "{name}: max channel difference {max}\n ours   {ours}\n chrome {chrome}");
    }
}

/// The frame the engine draws for `view`, captured through the renderer
/// (render_view, then capture_frame): `w` x `h` RGB from the top left.
fn frame_rgb(engine: &mut Engine, id: EngineViewId, w: usize, h: usize) -> Vec<u8> {
    engine.render_view(id).expect("render");
    let path = std::env::temp_dir().join(format!("rustkit-canvas-screen-{}-{id:?}.ppm", std::process::id()));
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
    assert_eq!(fields[0], "P6");
    let width: usize = fields[1].parse().unwrap();
    let mut out = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        out.extend_from_slice(&ppm[pos + y * width * 3..][..w * 3]);
    }
    out
}

/// STR-7b: the canvas on screen is its bitmap. Each page draws into a
/// canvas at the origin; the engine's captured frame is compared with
/// Chrome for Testing 148's screenshot of the same region
/// (tools/parity_oracle/canvas_screen_cases.json, canvas_screen_log.mjs).
/// putImageData and drawImage of a canvas reach the screen only through
/// the bitmap; "css-scaled-2x" is a 30x30 bitmap in a 60x60 CSS box, the
/// backing-store-to-CSS scale Chrome applies (bilinear).
#[test]
fn canvas_frames_match_chrome_screenshots() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/canvas_screen_cases.json")).unwrap();
    let w = data["size"][0].as_u64().unwrap() as usize;
    let h = data["size"][1].as_u64().unwrap() as usize;
    let mut failures = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let (mut engine, id) = view(case["html"].as_str().unwrap());
        pump(&mut engine, id, 50);
        let ours = frame_rgb(&mut engine, id, w, h);
        let hex = case["chrome_rgb"].as_str().unwrap();
        let chrome: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
        assert_eq!(ours.len(), chrome.len(), "{name}: length");
        let diffs: Vec<u8> = ours.chunks(3).zip(chrome.chunks(3))
            .map(|(a, b)| a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap())
            .collect();
        let max = *diffs.iter().max().unwrap();
        let mean = diffs.iter().map(|&d| d as f64).sum::<f64>() / diffs.len() as f64;
        let over = diffs.iter().filter(|&&d| d > 2).count();
        println!("{name}: max {max} mean {mean:.2} pixels>2 {over}/{}", diffs.len());
        if std::env::var("STR7_DUMP").is_ok_and(|d| d == name) {
            for y in 0..h {
                let row = |px: &[u8]| (0..w).map(|x| format!("{:02x}{:02x}{:02x}", px[(y * w + x) * 3], px[(y * w + x) * 3 + 1], px[(y * w + x) * 3 + 2])).collect::<Vec<_>>().join(" ");
                println!("y{y} ours   {}\ny{y} chrome {}", row(&ours), row(&chrome));
            }
        }
        // Anti-aliased edges differ (tiny-skia supersamples, Skia's coverage
        // is analytic: the STR-7c row); everything else is within rounding.
        if max > 80 || mean > 2.0 {
            failures.push(format!("{name}: max {max} mean {mean:.2}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// STR-7 slice 3: drawImage of an SVG `<img>` at 50x50, as MotionMark's
/// Images test draws its icons, read back with getImageData, against
/// Chrome for Testing 148 (docs/diagnostics/2026-11-12/str7: the six
/// icons, canvas_draw_svg.html, canvas_draw_svg.chrome148.json). Chrome
/// rasterizes the SVG at the size it is drawn. The icons are loaded here
/// as data: URLs of the same files.
#[test]
fn draw_image_of_an_svg_img_matches_chrome() {
    const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/diagnostics/2026-11-12/str7/");
    let chrome: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(format!("{DIR}canvas_draw_svg.chrome148.json")).unwrap()).unwrap();
    let mut failures = Vec::new();
    for name in ["fill_curves", "fill_hole", "stroke_lines", "stroke_curves", "fill_and_stroke", "aa_edges"] {
        let svg = std::fs::read_to_string(format!("{DIR}{name}.svg")).unwrap();
        let encoded: String = svg
            .bytes()
            .map(|b| if b.is_ascii_alphanumeric() { (b as char).to_string() } else { format!("%{b:02X}") })
            .collect();
        let html = format!(
            "<!doctype html><body style='margin:0'><canvas id=c width=50 height=50></canvas><img id=i src=\"data:image/svg+xml,{encoded}\">\
             <script>var i = document.getElementById('i'); function go() {{ var x = document.getElementById('c').getContext('2d'); \
             x.drawImage(i, 0, 0, 50, 50); var d = x.getImageData(0, 0, 50, 50).data, h = ''; \
             for (var k = 0; k < d.length; k++) h += (d[k] < 16 ? '0' : '') + d[k].toString(16); window.__r = h; }} \
             if (i.complete && i.naturalWidth) go(); else i.addEventListener('load', go);</script></body>"
        );
        let (mut engine, id) = view(&html);
        let mut ours = String::new();
        for _ in 0..40 {
            pump(&mut engine, id, 50);
            ours = eval(&mut engine, id, "typeof window.__r === 'string' ? window.__r : ''");
            if !ours.is_empty() {
                break;
            }
        }
        if ours.is_empty() {
            failures.push(format!("{name}: never drew"));
            continue;
        }
        let px = |h: &str| -> Vec<u8> { (0..h.len() / 2).map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16).unwrap()).collect() };
        let (a, b) = (px(&ours), px(chrome["pixels"][name].as_str().unwrap()));
        assert_eq!(a.len(), b.len(), "{name}: length");
        // Premultiplied, per pixel the largest channel difference.
        let pm = |p: &[u8]| -> [i32; 4] { let al = p[3] as i32; [p[0] as i32 * al / 255, p[1] as i32 * al / 255, p[2] as i32 * al / 255, al] };
        let diffs: Vec<i32> = a.chunks(4).zip(b.chunks(4))
            .map(|(x, y)| { let (x, y) = (pm(x), pm(y)); (0..4).map(|k| (x[k] - y[k]).abs()).max().unwrap() })
            .collect();
        let max = *diffs.iter().max().unwrap();
        let mean = diffs.iter().map(|&d| d as f64).sum::<f64>() / diffs.len() as f64;
        let painted = b.chunks(4).filter(|p| p[3] > 0).count();
        let at = diffs.iter().position(|&d| d == max).unwrap();
        println!("{name}: max {max} at ({}, {}) mean {mean:.2} (Chrome paints {painted} px; ours {:?}, Chrome {:?})",
            at % 50, at / 50, &a[at * 4..at * 4 + 4], &b[at * 4..at * 4 + 4]);
        // Anti-aliased edges differ (tiny-skia's 4x supersampling against
        // Skia's analytic coverage, the STR-7c row); the worst pixel is on
        // the thin curved stroke of stroke_curves (96). Interiors match.
        if max > 100 || mean > 4.0 {
            failures.push(format!("{name}: max {max} mean {mean:.2}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}

/// A tiny HTTP server for the redirect cases: `routes` maps a path to
/// (status, extra header lines, content type, body), made from its port.
pub(crate) fn serve_with_redirects(routes: impl FnOnce(u16) -> Vec<(String, u16, String, &'static str, Vec<u8>)>) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = routes(port);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut buf = [0u8; 4096];
            let n = stream.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
            let found = routes.iter().find(|r| r.0 == path);
            let (status, headers, ctype, body) = match found {
                Some((_, s, h, c, b)) => (*s, h.clone(), *c, b.clone()),
                None => (404, String::new(), "text/plain", b"not found".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n{headers}\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    port
}

/// Canvas origin checks across redirects, Chrome for Testing 148
/// (tools/parity_oracle/canvas_taint_cases.json "redirect-*"): the page's
/// own URL that redirects to another origin's image leaves the canvas not
/// origin-clean (Chrome decides on the response, not the URL asked for);
/// one that redirects to the page's own image does not. The image cache
/// keeps the response's URLs (each hop and the final one), which the
/// canvas decides on. Here a loopback page cannot be redirected to another
/// origin at all (the fetch refuses the hop, so that image fails to load
/// and the canvas has nothing to read); the cross-origin outcome is pinned
/// in rustkit-bindings `canvas_origin_checks_follow_the_response_urls`.
#[test]
fn canvas_origin_checks_follow_redirects_like_chrome() {
    const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207, 192, 80, 15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/canvas_taint_cases.json")).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut failures = Vec::new();
    for case in data["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if name != "redirect-same-to-same-origin" {
            continue;
        }
        let script = case["script"].as_str().unwrap();
        let page = format!(
            "<!doctype html><body><canvas id=c width=8 height=8></canvas><script>var x = document.getElementById('c').getContext('2d'); \
             function done(n) {{ window.__r = n; }} \
             function read() {{ try {{ x.getImageData(0, 0, 1, 1); done('ok'); }} catch (e) {{ done(e.name); }} }} \
             function img(src, co, f) {{ var i = new Image(); i.onload = function () {{ f(i); }}; i.onerror = function () {{ done('load-error'); }}; i.src = src; }} \
             {script}</script></body>"
        );
        let port = serve_with_redirects(|port| vec![
            ("/".into(), 200, String::new(), "text/html", page.into_bytes()),
            ("/img.png".into(), 200, String::new(), "image/png", PNG.to_vec()),
            ("/to-other".into(), 302, format!("Location: http://localhost:{port}/img.png\r\n"), "text/plain", Vec::new()),
            ("/to-same".into(), 302, "Location: /img.png\r\n".into(), "text/plain", Vec::new()),
        ]);
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine.create_headless_view(Bounds::new(0, 0, 200, 200)).expect("view");
        rt.block_on(engine.load_url(id, url::Url::parse(&format!("http://127.0.0.1:{port}/")).unwrap())).expect("load");
        let mut ours = String::new();
        for _ in 0..100 {
            rt.block_on(engine.pump_live(id, 16));
            ours = eval(&mut engine, id, "typeof window.__r === 'string' ? window.__r : ''");
            if !ours.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let chrome = case["chrome"]["name"].as_str().unwrap();
        println!("{name}: ours {ours:?}, Chrome {chrome:?}");
        if ours != chrome {
            failures.push(format!("{name}: ours {ours:?}, Chrome {chrome:?}"));
        }
        let at = |path: &str| url::Url::parse(&format!("http://127.0.0.1:{port}{path}")).unwrap();
        let hops = engine.lctx.image_manager.response_urls(&at("/to-same"));
        if hops != vec![at("/to-same"), at("/img.png")] {
            failures.push(format!("{name}: response URLs {hops:?}"));
        }
    }
    assert!(failures.is_empty(), "{failures:?}");
}
