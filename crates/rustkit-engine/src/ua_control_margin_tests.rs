//! Chromium's html.css gives checkboxes `margin: 3px 3px 3px 4px` and radios
//! `margin: 3px 3px 0px 5px`. RustKit gave them none, so settings.html's
//! `.checkbox-label` rows (flex, gap .5rem) put every checkbox 4px left of
//! Chrome's and their rows 6px short. An author margin still wins.
//!
//! Expected values are getComputedStyle in Chrome for Testing 148.0.7778.216
//! on the same markup (impl-board2 probe, 2026-11-08).

use super::*;

fn eval(page: &str, script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, script).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

#[test]
fn checkbox_and_radio_carry_chromiums_ua_margins() {
    let got = eval(
        "<!doctype html><body><input type=radio id=r><input type=checkbox id=c>\
         <input type=checkbox id=c2 style=\"margin:0\"><input id=t>",
        "['r','c','c2','t'].map(function (i) { var c = getComputedStyle(document.getElementById(i)); \
         return [c.marginTop, c.marginRight, c.marginBottom, c.marginLeft].join(' '); }).join(';')",
    );
    assert_eq!(
        got,
        "3px 3px 0px 5px;3px 3px 3px 4px;0px 0px 0px 0px;0px 0px 0px 0px",
        "Chromium 148: radio 3 3 0 5, checkbox 3 3 3 4, author margin wins, text input none"
    );
}
