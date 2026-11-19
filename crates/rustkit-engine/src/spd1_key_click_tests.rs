//! SPD1 (Speedometer run path): what a test runner's synthetic input sees.
//!
//! * `KeyboardEvent.which` was undefined, so TodoMVC-jQuery and
//!   TodoMVC-Backbone (`e.which === ENTER_KEY`) never added an item. It is
//!   the event's keyCode (Chrome ignores `which` in the init dictionary).
//! * The engine's own key events had keyCode 0 and no `keypress`: `keydown`
//!   / `keyup` carry the virtual-key code, a character key (and Enter) also
//!   fires `keypress` with the character, and a text field hears
//!   `textInput` before its `input`.
//! * `el.click()` dispatched a plain Event; it is a PointerEvent, and does
//!   nothing on a disabled control.
//! * `Event.cancelBubble`, `TextEvent`, and a `document.hasFocus()` that the
//!   view's focus decides.
//!
//! Expected lines: docs/diagnostics/2026-11-12/spd1_key_click_events_cft148.txt,
//! `window.report()` of the page beside it in Chrome for Testing
//! 148.0.7778.216 after the same key presses (Playwright `keyboard.press`).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/spd1_key_click_events.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/spd1_key_click_events_cft148.txt");

/// One key press and release, as the page protocol delivers it: Enter tries
/// the focused field's form first.
fn press(engine: &mut Engine, view: EngineViewId, key: &str) {
    let (code, text) = match key {
        "Enter" => (0x0D, ""),
        "Escape" => (0x1B, ""),
        "Backspace" => (0x08, ""),
        "ArrowLeft" => (0x25, ""),
        "ArrowDown" => (0x28, ""),
        "Delete" => (0x2E, ""),
        "Home" => (0x24, ""),
        character => (0, character),
    };
    if code == 0x0D {
        engine.submit_focused_form(view);
    }
    engine.handle_text_key(view, code, text, false, false, false);
    engine.handle_key_up(view, code, text, false, false, false);
}

#[test]
fn synthetic_keys_and_clicks_answer_as_chrome() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    for key in ["Enter", "Escape", "a", "A", "1", "!", " ", ";", "/", "Backspace", "ArrowLeft", "ArrowDown", "Delete", "Home"] {
        press(&mut engine, view, key);
    }
    // The probe clicks #field; here the click goes to the middle of
    // wherever the engine put the field, so this test is about the events.
    let rect = engine
        .execute_script(view, "(function () { var r = document.getElementById('field').getBoundingClientRect(); return [r.x, r.y, r.width, r.height].join(' '); })()")
        .expect("rect");
    let numbers: Vec<f32> = rect
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .split(' ')
        .map(|n| n.parse().expect("a number"))
        .collect();
    let (x, y) = (numbers[0] + numbers[2] / 2.0, numbers[1] + numbers[3] / 2.0);
    let under = engine
        .execute_script(view, &format!("(function () {{ var e = document.elementFromPoint({x}, {y}); return (e ? e.tagName + '#' + e.id : 'none') + ' scrollY=' + scrollY + ' hash=' + location.hash; }})()"))
        .expect("elementFromPoint");
    assert_eq!(engine.focus_at_point(view, x, y).as_deref(), Some("input"), "the click focuses #field at {rect}; under the point: {under}");
    for key in ["a", "B", "2", "Backspace", "Enter", "Escape"] {
        press(&mut engine, view, key);
    }
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got);
    let got: String = serde_json::from_str(got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 123, "the Chrome report has 123 lines");
    // Still off, for a reason outside this row: `from another window` needs
    // an iframe with a window of its own (`contentWindow` is null here). It
    // must still DIFFER; when it matches it comes off the list.
    const KNOWN: &[&str] = &["from another window: "];
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
