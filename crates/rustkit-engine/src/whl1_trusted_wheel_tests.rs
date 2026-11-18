//! WHL1: the user's wheel reaches script.
//!
//! The engine scrolled without telling the page, so a chart that zooms on
//! the wheel, or a widget that cancels it to scroll itself, never heard
//! one. A trusted `wheel` WheelEvent is now dispatched (UI Events §5.5):
//!
//! * before the scroll, and cancelable, when a listener on the way up from
//!   the element under the pointer is not passive (an `onwheel` handler
//!   never is); `preventDefault()` then leaves the scroll alone;
//! * after the scroll, not cancelable, at whatever is then under the
//!   pointer, when only passive listeners can hear it (a wheel listener on
//!   the window, the document, its root or its body is passive unless it
//!   says otherwise);
//! * not at all on a page that has registered no wheel listener.
//!
//! Expected lines: docs/diagnostics/2026-11-12/whl1_trusted_wheel_cft148.txt,
//! `window.report()` of the page beside it in Chrome for Testing
//! 148.0.7778.216 after the same wheels (Playwright `mouse.wheel`).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/whl1_trusted_wheel.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/whl1_trusted_wheel_cft148.txt");

/// Lines still off, each for a reason outside this row; filled in from the
/// first run with the fix. They must still DIFFER.
const KNOWN: &[&str] = &[];

#[test]
fn the_users_wheel_reaches_script_before_it_scrolls() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    // (name, x, y, DOM deltaX, DOM deltaY): the probe's wheels. The engine's
    // own delta is positive for up.
    let wheels: [(&str, f32, f32, f32, f32); 8] = [
        ("plain", 50.0, 30.0, 0.0, 100.0),
        ("cancels", 50.0, 100.0, 0.0, 100.0),
        ("passive", 50.0, 170.0, 0.0, 100.0),
        ("handler", 50.0, 240.0, 0.0, 100.0),
        ("quiet", 50.0, 310.0, 0.0, 100.0),
        ("scroller", 450.0, 50.0, 0.0, 100.0),
        ("plain up at the top", 50.0, 30.0, 0.0, -100.0),
        ("plain sideways", 50.0, 30.0, 40.0, 0.0),
    ];
    for (name, x, y, dx, dy) in wheels {
        // Each is a wheel of its own, as the probe's are (seconds apart):
        // not a step of the gesture before it, which keeps its scroller.
        engine.views.get_mut(&view).expect("view").wheel_gesture = None;
        engine.scroll_at(view, x, y, dx, -dy).expect("scroll_at");
        engine.execute_script(view, &format!("window.step({name:?}); 0")).expect("step");
    }
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 15, "the Chrome report has 15 lines");
    let mut wrong = Vec::new();
    for (i, want) in expected.iter().enumerate() {
        let listed = KNOWN.iter().any(|k| want.starts_with(k));
        match got.get(i) {
            Some(have) if (have == want) != listed => {}
            Some(have) if listed => wrong.push(format!("  {have}\n    matches Chromium now: take it off KNOWN")),
            Some(have) => wrong.push(format!("  {have}\n    Chromium: {want}")),
            None => wrong.push(format!("  (missing)\n    Chromium: {want}")),
        }
    }
    if got.len() > expected.len() {
        wrong.push(format!("  {} extra lines, first: {}", got.len() - expected.len(), got[expected.len()]));
    }
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

const DP_PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/whl1_default_prevented.html");
const DP_CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/whl1_default_prevented_cft148.txt");

/// R1 LOW 1: `defaultPrevented` is a read-only getter on the canceled
/// flag. Assigning it does nothing (and throws in strict code), and a
/// passive listener cannot cancel by `preventDefault()`, `returnValue`
/// or assignment. Lines from Chrome for Testing 148.
#[test]
fn default_prevented_is_read_only_and_a_passive_listener_cannot_cancel() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine.load_html(view, DP_PAGE).expect("load");
    let got = engine.execute_script(view, "JSON.stringify(window.report().split('\\n'))").expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = DP_CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 13);
    let wrong: Vec<String> = expected
        .iter()
        .enumerate()
        .filter(|(i, e)| got.get(*i).map(String::as_str) != Some(**e))
        .map(|(i, e)| format!("  {}\n    Chromium: {e}", got.get(i).map_or("(missing)", String::as_str)))
        .collect();
    assert!(wrong.is_empty(), "{} of 13 lines differ:\n{}", wrong.len(), wrong.join("\n"));
}

/// The scroll itself: a non-passive listener on the scroller makes the
/// wheel cancelable, and a passive listener on the document that writes
/// `defaultPrevented` must not stop the scroll.
#[test]
fn a_passive_listener_cannot_stop_the_users_scroll_by_writing_default_prevented() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine
        .load_html(
            view,
            "<!doctype html><body style=\"margin:0\"><div id=s style=\"height:120px;overflow:auto\"><div style=\"height:1000px\"></div></div>\
             <script>document.getElementById('s').addEventListener('wheel', function () {}, { passive: false });\
             document.addEventListener('wheel', function (e) { e.defaultPrevented = true; try { Object.defineProperty(e, 'defaultPrevented', { value: true }); } catch (_) {} e.returnValue = false; });</script>",
        )
        .expect("load");
    engine.scroll_at(view, 50.0, 50.0, 0.0, -100.0).expect("scroll_at");
    assert_eq!(engine.execute_script(view, "String(document.getElementById('s').scrollTop)").unwrap(), "String(\"100\")");
}

/// The wheel dispatch is the engine's: nothing of it is a property a page
/// can read, call or replace.
#[test]
fn the_wheel_dispatch_is_not_the_pages_to_call() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine.load_html(view, "<!doctype html><body><p>x</p>").expect("load");
    let seen = engine
        .execute_script(view, "[typeof document.__rkFireWheel, typeof document.__rkWheelBlocks, typeof window.__rustkit_dom_note_wheel].join(' ')")
        .expect("script");
    assert_eq!(seen, "String(\"undefined undefined undefined\")");
}

/// The cost of a wheel, for the row's rule that only a page with a wheel
/// listener pays anything. A timing, run by hand:
///
/// `cargo test -p rustkit-engine --lib --features headless --release wheel_turn_cost -- --ignored --nocapture`
///
/// A long page (4,000 paragraphs) is wheeled 200 times down and back; the
/// median turn is printed with no listener, with a passive listener on the
/// document, and with a listener that can cancel on the body.
#[test]
#[ignore = "a timing, run by hand in release"]
fn wheel_turn_cost() {
    let mut body = String::from("<!doctype html><html><body style=\"margin:0\">");
    for i in 0..4000 {
        body.push_str(&format!("<p style=\"margin:0;height:20px\">paragraph {i}</p>"));
    }
    let cancelable = "<script>var n = 0; document.body.addEventListener('wheel', function () { n++; }, { passive: false });</script>";
    for (label, script, gesture) in [
        ("no listener", "", true),
        ("passive listener on the document", "<script>var n = 0; document.addEventListener('wheel', function () { n++; });</script>", true),
        ("cancelable listener on the body, trackpad gesture steps", cancelable, true),
        ("cancelable listener on the body, mouse wheels", cancelable, false),
    ] {
        let page = format!("{body}{script}</body></html>");
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        engine.load_html(view, &page).expect("load");
        let mut turns = Vec::new();
        for i in 0..400 {
            let delta = if (i / 100) % 2 == 0 { -40.0 } else { 40.0 };
            let started = std::time::Instant::now();
            engine.wheel_at(view, 400.0, 300.0, 0.0, delta, gesture).expect("wheel_at");
            turns.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        turns.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "{label}: median {:.3} ms, p90 {:.3} ms, max {:.3} ms over {} wheels",
            turns[turns.len() / 2],
            turns[turns.len() * 9 / 10],
            turns[turns.len() - 1],
            turns.len()
        );
    }
}
