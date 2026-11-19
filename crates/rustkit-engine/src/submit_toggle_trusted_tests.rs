//! The engine's `submit` and `toggle` events are trusted and reach the
//! page's listeners however the page has replaced `dispatchEvent`
//! (Chrome for Testing 148: trusted for the user's click, Enter, script's
//! `click()` and `requestSubmit()`, and a script `open` change). They went
//! through the page-visible `dispatchEvent` and were untrusted.
//!
//! Expected lines: docs/diagnostics/2026-11-13/submit_toggle_trusted_cft148.txt.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-13/submit_toggle_trusted.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-13/submit_toggle_trusted_cft148.txt");

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, &format!("String(eval({script:?}))")).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\n", "\n").replace("\\\"", "\"").replace("\\\\", "\\"))
        .unwrap_or(out)
}

fn click(engine: &mut Engine, id: EngineViewId, el: &str) {
    let at = js(engine, id, &format!("window.center('{el}')"));
    let (x, y) = at.split_once(',').expect("x,y");
    let (x, y): (f32, f32) = (x.parse().expect("x"), y.parse().expect("y"));
    engine.mouse_move_at_point(id, x, y);
    engine.mouse_down_at_point(id, x, y);
    engine.click_at_point(id, x, y);
}

/// What `window.later(label)` does in the Chrome run: let the page's queued
/// work run (the toggle comes in a microtask), then record the step.
fn later(engine: &mut Engine, id: EngineViewId, label: &str) {
    js(engine, id, "0");
    js(engine, id, &format!("window.step('{label}')"));
}

#[test]
fn the_engines_submit_and_toggle_are_trusted_and_reach_the_page() {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, PAGE).expect("load_html");
    let e = &mut engine;
    click(e, id, "b");
    later(e, id, "user clicks the button");
    click(e, id, "t");
    let _ = e.submit_focused_form(id);
    e.handle_text_key(id, 0x0D, "", false, false, false);
    e.handle_key_up(id, 0x0D, "", false, false, false);
    later(e, id, "Enter in the field");
    click(e, id, "s");
    later(e, id, "user clicks the summary");
    click(e, id, "s");
    later(e, id, "user clicks the summary again");
    js(e, id, "window.scripted()");
    later(e, id, "script sets open");
    js(e, id, "window.hostile()");
    click(e, id, "b");
    later(e, id, "hostile page: user clicks the button");
    click(e, id, "s");
    later(e, id, "hostile page: user clicks the summary");
    let got = js(e, id, "window.report()");
    let got: Vec<&str> = got.lines().collect();
    let want: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect();
    assert_eq!(got.len(), want.len(), "lines: {got:#?}");
    let wrong: Vec<String> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, w)| format!("got  {g}\n     want {w}")).collect();
    assert!(wrong.is_empty(), "the lines that differ from Chrome 148:\n{}", wrong.join("\n"));
}
