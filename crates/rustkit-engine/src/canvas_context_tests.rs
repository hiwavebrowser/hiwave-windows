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
    let red = rustkit_css::Color::from_rgb(255, 0, 0);
    let fills: Vec<_> = list(&engine, id)
        .into_iter()
        .filter_map(|c| match c {
            DisplayCommand::FillRect { rect, color } if color == red => Some((rect.x, rect.y, rect.width, rect.height)),
            _ => None,
        })
        .collect();
    assert_eq!(fills, vec![(20.0, 20.0, 40.0, 20.0)]);
}

/// fillText is a text run at the transformed baseline, its font scaled with
/// the bitmap; a stroked path is a polyline.
#[test]
fn fill_text_and_stroke_paint_text_and_lines() {
    let (engine, id) = view(
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
    assert!(commands.iter().any(|c| matches!(c, DisplayCommand::Polyline { points, width, .. }
        if *width == 2.0 && points.first() == Some(&(0.0, 90.0)) && points.last() == Some(&(200.0, 90.0)))));
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
    let bars = commands
        .iter()
        .filter(|c| match c {
            DisplayCommand::FillRect { rect, .. } => inside(rect.x, rect.y) && inside(rect.x + rect.width, rect.y + rect.height),
            DisplayCommand::FillPolygon { points, .. } => points.iter().all(|&(x, y)| inside(x, y)),
            _ => false,
        })
        .count();
    assert!(bars >= 3, "the three datasets' bars (and legend boxes) are filled: {bars}");
}

// ---- PR #905 R1 HOLD: partial clears paint, the paint is cached, and an
// animation loop stays bounded.

/// The area of `color` the list paints, each fill rect cut by the clips in
/// force (the renderer intersects nested clips).
fn visible_area(commands: &[DisplayCommand], color: rustkit_css::Color) -> f32 {
    let mut clips: Vec<Rect> = Vec::new();
    let mut area = 0.0;
    for c in commands {
        match c {
            DisplayCommand::PushClip(r) => {
                let r = match clips.last() {
                    Some(top) => {
                        let x0 = r.x.max(top.x);
                        let y0 = r.y.max(top.y);
                        let x1 = (r.x + r.width).min(top.x + top.width);
                        let y1 = (r.y + r.height).min(top.y + top.height);
                        Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
                    }
                    None => *r,
                };
                clips.push(r);
            }
            DisplayCommand::PopClip => {
                clips.pop();
            }
            DisplayCommand::FillRect { rect, color: c } if *c == color => {
                let top = clips.last().copied().unwrap_or(*rect);
                let w = ((rect.x + rect.width).min(top.x + top.width) - rect.x.max(top.x)).max(0.0);
                let h = ((rect.y + rect.height).min(top.y + top.height) - rect.y.max(top.y)).max(0.0);
                area += w * h;
            }
            _ => {}
        }
    }
    area
}

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
    let commands = list(&engine, id);
    let red = visible_area(&commands, rustkit_css::Color::from_rgb(255, 0, 0));
    let blue = visible_area(&commands, rustkit_css::Color::from_rgb(0, 0, 255));
    assert_eq!((red, blue), (2500.0 - 625.0, 1000.0), "{commands:?}");
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
        assert!(source.retained_bytes() <= 64 * 1024, "{} bytes", source.retained_bytes());
    }
    assert!(lengths.windows(2).all(|w| w[0] == w[1]), "display list lengths {lengths:?}");
}

fn source_node(engine: &mut Engine, id: EngineViewId, element: &str) -> usize {
    let view = &engine.views[&id];
    let doc = view.document.as_ref().expect("document");
    doc.get_element_by_id(element).expect("element").id.raw()
}
