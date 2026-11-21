//! A setter script can see is called only by script.
//!
//! The engine changed a control's state for the user (a click's activation,
//! a label, a radio group, a reset) by assigning to `el.checked`: the
//! accessor on `HTMLInputElement.prototype`, which a page may have wrapped.
//! React wraps it (its value tracker): it then saw the engine's own write,
//! took the new state for one it already knew, and fired no `onChange`, so
//! a click on a React checkbox changed the box and nothing else
//! (Speedometer's TodoMVC-React CompletingAllItems: 1 of 101 completed).
//! A browser keeps that state in its own slots.
//!
//! The same holds for the methods the page can reach: a label's activation
//! is not the control's `click` property, a reset button's is not the
//! form's `reset` property (which a control named `reset` shadows), and
//! the events the engine fires are not built with `window.Event`.
//!
//! The page wraps the `checked` and `value` setters and counts the calls
//! across the user's steps and script's own. Expected lines:
//! docs/diagnostics/2026-11-12/form_setter_calls_cft148.txt, the same steps
//! in Chrome for Testing 148.0.7778.216.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/form_setter_calls.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/form_setter_calls_cft148.txt");

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

fn key(engine: &mut Engine, id: EngineViewId, code: u32, text: &str) {
    engine.handle_text_key(id, code, text, false, false, false);
    engine.handle_key_up(id, code, text, false, false, false);
}

fn step(engine: &mut Engine, id: EngineViewId, label: &str) {
    js(engine, id, &format!("window.step('{label}')"));
}

/// The steps of the Chrome run, and the page's report after them.
fn report() -> String {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, PAGE).expect("load_html");
    let e = &mut engine;
    click(e, id, "box");
    step(e, id, "user clicks box");
    click(e, id, "lab");
    step(e, id, "user clicks lab");
    click(e, id, "r2");
    step(e, id, "user clicks r2");
    click(e, id, "r1");
    step(e, id, "user clicks r1");
    click(e, id, "box");
    step(e, id, "user clicks box again");
    key(e, id, 0, " ");
    step(e, id, "Space on focused box");
    click(e, id, "field");
    key(e, id, 0, "a");
    key(e, id, 0, "b");
    key(e, id, 0x08, "");
    step(e, id, "user types ab, Backspace in field");
    click(e, id, "area");
    key(e, id, 0, "x");
    step(e, id, "user types x in area");
    click(e, id, "ft");
    key(e, id, 0, "!");
    step(e, id, "user types ! in ft");
    click(e, id, "fc");
    click(e, id, "fr");
    step(e, id, "user clicks fc, fr");
    click(e, id, "rst");
    step(e, id, "user clicks the reset button");
    js(e, id, "window.scripted()");
    // The page replaces what it can reach; the user's steps go on.
    js(e, id, "window.hostile()");
    click(e, id, "lab");
    step(e, id, "user clicks lab");
    click(e, id, "nfb");
    step(e, id, "user clicks the reset button of the form whose reset is an input");
    click(e, id, "rst");
    step(e, id, "user clicks the reset button");
    click(e, id, "box");
    step(e, id, "user clicks box");
    js(e, id, "window.report()")
}

/// `label: checked-sets=N value-sets=N textarea-sets=N` of a report line.
fn counts(line: &str) -> &str {
    line.split(" events=").next().unwrap_or(line)
}

fn chrome() -> Vec<&'static str> {
    CHROME.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).collect()
}

#[test]
fn only_script_calls_the_checked_and_value_setters() {
    let got = report();
    let got: Vec<&str> = got.lines().collect();
    let want = chrome();
    assert_eq!(got.len(), want.len(), "steps: {got:#?}");
    let wrong: Vec<String> = got
        .iter()
        .zip(&want)
        .filter(|(g, w)| counts(g) != counts(w))
        .map(|(g, w)| format!("got  {}\n     want {}", counts(g), counts(w)))
        .collect();
    assert!(wrong.is_empty(), "setter calls differ from Chrome 148:\n{}", wrong.join("\n"));
}

#[test]
fn the_steps_leave_the_state_and_events_chrome_has() {
    let got = report();
    let got: Vec<&str> = got.lines().collect();
    let want = chrome();
    let wrong: Vec<String> = got.iter().zip(&want).filter(|(g, w)| g != w).map(|(g, w)| format!("got  {g}\n     want {w}")).collect();
    assert!(wrong.is_empty(), "the lines that differ from Chrome 148:\n{}", wrong.join("\n"));
}
