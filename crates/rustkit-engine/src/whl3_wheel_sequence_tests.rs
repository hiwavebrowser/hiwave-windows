//! WHL3: the wheel's sequence rules and legacy `mousewheel`, and the
//! shared dispatch path's cancellation (R1 on #1106).
//!
//! * Only the first wheel of a scroll sequence can be cancelled: a later
//!   step of a gesture whose first step scrolled goes after the scroll,
//!   not cancelable. A mouse's wheels are each a first (Chrome for
//!   Testing 148: every one is cancelable).
//! * With no listener or handler that can cancel, the engine does not walk
//!   the path before the scroll.
//! * A target with no `wheel` listener hears the user's wheel as
//!   `mousewheel`.
//! * `initEvent` / `initCustomEvent` clear the canceled flag; `cancelable`
//!   is read-only; a cancelled click, submit and keydown leave their
//!   default undone.
//!
//! Expected lines: docs/diagnostics/2026-11-13/whl3_wheel_sequence_cft148.txt.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-13/whl3_wheel_sequence.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-13/whl3_wheel_sequence_cft148.txt");

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine.load_html(view, PAGE).expect("load");
    (engine, view)
}

fn js(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let value = engine.execute_script(view, script).expect("script");
    let value = value.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&value).to_string();
    serde_json::from_str::<String>(&value).unwrap_or(value)
}

fn center(engine: &mut Engine, view: EngineViewId, id: &str) -> (f32, f32) {
    let r = js(engine, view, &format!("var r = document.getElementById('{id}').getBoundingClientRect(); (r.left + r.width / 2) + ' ' + (r.top + r.height / 2)"));
    let mut it = r.split(' ').map(|v| v.parse::<f32>().unwrap());
    (it.next().unwrap(), it.next().unwrap())
}

#[test]
fn the_wheels_sequence_rules_and_the_dispatch_paths_cancellation_match_chrome() {
    let mut got: Vec<String> = Vec::new();
    let (mut engine, view) = page();
    js(&mut engine, view, "constructed(); 0");
    got.extend(js(&mut engine, view, "report()").lines().map(str::to_string));
    for mode in ["wheel cancels the first", "mousewheel only", "mousewheel and wheel", "mousewheel on document, wheel on box"] {
        let (mut engine, view) = page();
        js(&mut engine, view, &format!("wheelSetup({mode:?}); 0"));
        for _ in 0..3 {
            // A mouse's wheel: no gesture phase. The engine's delta is
            // positive for up.
            engine.wheel_at(view, 50.0, 100.0, 0.0, -100.0, false).expect("wheel");
        }
        js(&mut engine, view, &format!("after({mode:?}); 0"));
        got.extend(js(&mut engine, view, "report()").lines().map(str::to_string));
    }
    let (mut engine, view) = page();
    let (x, y) = center(&mut engine, view, "cb");
    engine.click_at_point(view, x, y);
    let (x, y) = center(&mut engine, view, "t");
    engine.click_at_point(view, x, y);
    for key in ["a", "q", "b"] {
        engine.handle_text_key(view, 0, key, false, false, false);
    }
    let (x, y) = center(&mut engine, view, "go");
    engine.click_at_point(view, x, y);
    js(&mut engine, view, "formReport(); 0");
    got.extend(js(&mut engine, view, "report()").lines().map(str::to_string));
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 34, "the Chrome report has 34 lines");
    let mut wrong = Vec::new();
    for i in 0..expected.len().max(got.len()) {
        let (g, e) = (got.get(i).map_or("(missing)", String::as_str), expected.get(i).copied().unwrap_or("(none)"));
        if g != e {
            wrong.push(format!("  {g}\n    Chromium: {e}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} report lines differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

/// Later steps of a gesture are not cancelable once the first step
/// scrolled; a gesture whose first step was cancelled stays cancelable.
#[test]
fn only_the_first_wheel_of_a_gesture_can_be_cancelled() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine
        .load_html(
            view,
            "<!doctype html><body style=\"margin:0\"><div id=b style=\"height:200px;overflow:auto\"><div style=\"height:2000px\"></div></div>\
             <script>var seen = []; document.getElementById('b').addEventListener('wheel', function (e) { seen.push(e.cancelable); }, { passive: false });</script>",
        )
        .expect("load");
    for _ in 0..3 {
        engine.wheel_at(view, 50.0, 100.0, 0.0, -50.0, true).expect("wheel");
    }
    assert_eq!(js(&mut engine, view, "seen.join(' ') + ' ' + document.getElementById('b').scrollTop"), "true false false 150");
    js(&mut engine, view, "seen = []; document.getElementById('b').scrollTop = 0; document.getElementById('b').addEventListener('wheel', function (e) { e.preventDefault(); }, { passive: false }); 0");
    // Past the hold: a new gesture.
    engine.views.get_mut(&view).expect("view").wheel_gesture = None;
    for _ in 0..3 {
        engine.wheel_at(view, 50.0, 100.0, 0.0, -50.0, true).expect("wheel");
    }
    assert_eq!(js(&mut engine, view, "seen.join(' ') + ' ' + document.getElementById('b').scrollTop"), "true true true 0");
}

/// A page whose wheel listeners are all passive hears the wheel after the
/// scroll, not cancelable; a handler attribute can cancel it.
#[test]
fn passive_only_listeners_hear_it_after_and_a_handler_attribute_can_cancel() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine
        .load_html(
            view,
            "<!doctype html><body style=\"margin:0;height:3000px\"><script>var seen = []; document.addEventListener('wheel', function (e) { seen.push(e.cancelable + ':' + scrollY); });</script>",
        )
        .expect("load");
    engine.wheel_at(view, 50.0, 100.0, 0.0, -100.0, false).expect("wheel");
    assert_eq!(js(&mut engine, view, "seen.join(' ')"), "false:100");
    // A page with only an `onwheel` attribute (no listener) hears it and
    // can cancel it.
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine
        .load_html(view, "<!doctype html><body style=\"margin:0;height:3000px\" onwheel=\"window.heard = event.cancelable; event.preventDefault()\">")
        .expect("load");
    engine.wheel_at(view, 50.0, 100.0, 0.0, -100.0, false).expect("wheel");
    assert_eq!(js(&mut engine, view, "String(window.heard) + ' ' + scrollY"), "true 0");
}

/// A second swipe that the platform says begins (AppKit's `Began`) is a
/// first wheel even when it follows the last swipe's steps inside the
/// hold, so it can be cancelled, and the rest of it with it (R1 on #1121).
#[test]
fn a_swipe_that_begins_inside_the_hold_is_a_first_wheel() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 }).expect("view");
    engine
        .load_html(
            view,
            "<!doctype html><body style=\"margin:0\"><div id=b style=\"height:200px;overflow:auto\"><div style=\"height:2000px\"></div></div>\
             <script>var seen = []; document.getElementById('b').addEventListener('wheel', function (e) { seen.push(e.cancelable); }, { passive: false });</script>",
        )
        .expect("load");
    engine.wheel_phase_at(view, 50.0, 100.0, 0.0, -50.0, true, true).expect("wheel");
    for _ in 0..2 {
        engine.wheel_phase_at(view, 50.0, 100.0, 0.0, -50.0, true, false).expect("wheel");
    }
    assert_eq!(js(&mut engine, view, "seen.join(' ') + ' ' + document.getElementById('b').scrollTop"), "true false false 150");
    js(&mut engine, view, "seen = []; document.getElementById('b').addEventListener('wheel', function (e) { e.preventDefault(); }, { passive: false }); 0");
    // At once: well inside the hold of the first swipe's last step.
    engine.wheel_phase_at(view, 50.0, 100.0, 0.0, -50.0, true, true).expect("wheel");
    for _ in 0..2 {
        engine.wheel_phase_at(view, 50.0, 100.0, 0.0, -50.0, true, false).expect("wheel");
    }
    assert_eq!(js(&mut engine, view, "seen.join(' ') + ' ' + document.getElementById('b').scrollTop"), "true true true 150");
}
