//! SET3: a single-line text `<input>`'s height. Chrome for Testing 148
//! builds it as a content line, the larger of the font's `normal` line and
//! the author's `line-height`, plus the vertical padding (the author's, else
//! the UA's 1px each side) and border (the author's, else the UA's 2px each
//! side). RustKit dropped the UA border once the author set a padding (a
//! `padding: 8px 12px` input at 13.6px was 31 for Chrome's 35), ignored
//! `line-height`, and used a scaled blob for a bare input at another font
//! size. text, email, password, number, search, url and tel are the same.
//!
//! Expected heights: docs/diagnostics/2026-11-11/set3_input_heights_cft148.txt
//! for set3_input_heights.html beside it (getBoundingClientRect).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/set3_input_heights.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/set3_input_heights_cft148.txt");

#[test]
fn a_text_input_is_as_tall_as_chrome_builds_it() {
    let expected: Vec<(&str, f32)> = CHROME
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            (it.next().unwrap(), it.next().unwrap().parse().unwrap())
        })
        .collect();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let ids: Vec<String> = expected.iter().map(|(id, _)| format!("'{id}'")).collect();
    let got = engine
        .execute_script(
            view,
            &format!(
                "[{}].map(function (i) {{ return document.getElementById(i).getBoundingClientRect().height; }}).join(' ')",
                ids.join(",")
            ),
        )
        .expect("script");
    let got: Vec<f32> = got
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect();
    let wrong: Vec<String> = expected
        .iter()
        .zip(&got)
        .filter(|((_, want), got)| (*got - *want).abs() > 0.5)
        .map(|((id, want), got)| format!("{id}: {got} (Chromium {want})"))
        .collect();
    assert!(wrong.is_empty(), "{} of {} inputs differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

/// R1 on #1090 (LOW 1): a side the author did not declare keeps the UA's
/// padding or border on its own. Chrome for Testing 148.0.7778.216 at the
/// UA font: a bare input is 21 and a bare select 19; `padding-top: 4px`,
/// `padding-bottom: 4px` or `border-top-width: 5px` alone make the input
/// 24, and the select 23 with `padding-top: 4px` or `border-top-width:
/// 5px`. One flag for both vertical sides dropped the other side's UA
/// value (23 and 22).
#[test]
fn one_declared_side_keeps_the_other_sides_ua_value() {
    let page = "<!doctype html><input id=a style=\"padding-top:4px\"><input id=b style=\"padding-bottom:4px\">\
                <input id=c style=\"border-top-width:5px\"><input id=d>\
                <select id=e style=\"padding-top:4px\"><option>x</select><select id=f><option>x</select>\
                <select id=g style=\"border-top-width:5px\"><option>x</select>";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine
        .execute_script(
            view,
            "['a','b','c','d','e','f','g'].map(function (i) { return i + '=' + document.getElementById(i).getBoundingClientRect().height; }).join(' ')",
        )
        .expect("script");
    assert_eq!(got, "String(\"a=24 b=24 c=24 d=21 e=23 f=19 g=23\")");
}
