//! CSS escapes inside an attribute selector's value (CSS Syntax §4.3.7).
//!
//! The Inspector marks its selected DOM row with
//! `querySelector('[data-path="' + CSS.escape(path) + '"]')`. `CSS.escape`
//! of a path like `0.1.1` is `\30 \.1\.1`, and the matcher compared that
//! raw text with the attribute, so no row ever got `selected` (the chrome
//! board's inspector rows: a join failure and ~1.5 % of the viewport).
//! A quoted value's escapes resolve like any string token's (hex escape and
//! its one trailing whitespace, an escaped quote, an escaped newline that
//! vanishes), an unquoted value's like an ident's, and a `]` inside quotes
//! does not end the selector.
//!
//! Expected answers are `querySelectorAll` in Chrome for Testing
//! 148.0.7778.216 on the same markup (impl-board3 probe, 2026-11-08).

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

const PAGE: &str = "<!doctype html><body><div id=a data-path=\"0.1.1\"></div><div id=b data-x='a\"b'></div>\
    <div id=c data-x=\"x]y\"></div><div id=d data-x=\"ab\"></div><div id=e data-x=\"\u{e9}\"></div>\
    <div id=f data-x=\"a b\"></div>";

#[test]
fn escapes_in_an_attribute_value_resolve_like_chromium() {
    let cases: &[(&str, &str)] = &[
        (r#"[data-path="\30 \.1\.1"]"#, "a"),
        (r#"[data-path=\30 \.1\.1]"#, "a"),
        (r#"[data-path="\30 .1.1"]"#, "a"),
        (r#"[data-path="0\.1\.1"]"#, "a"),
        (r#"[data-x="a\"b"]"#, "b"),
        (r#"[data-x="a\62"]"#, "d"),
        (r#"[data-x="\61 \62"]"#, "d"),
        (r#"[data-x="\e9"]"#, "e"),
        ("[data-x=\"a\\\nb\"]", "d"),
        (r#"[data-x="a\20 b"]"#, "f"),
        (r#"[data-x^="\61"]"#, "b,d,f"),
        (r#"[data-x=a\62]"#, "d"),
        (r#"[data-x="x\]y"]"#, "c"),
        (r#"[data-x="x]y"]"#, "c"),
        (r#"[data-x="\0"]"#, ""),
    ];
    let list: Vec<String> = cases.iter().map(|(s, _)| format!("{s:?}")).collect();
    let script = format!(
        "[{}].map(function (s) {{ return Array.prototype.map.call(document.querySelectorAll(s), \
         function (e) {{ return e.id; }}).join(','); }}).join(' | ')",
        list.join(", ")
    );
    let want: Vec<&str> = cases.iter().map(|(_, w)| *w).collect();
    assert_eq!(eval(PAGE, &script), want.join(" | "), "Chromium 148 matches, in order");
}

/// The Inspector's own call: the selected row gets `selected`.
#[test]
fn the_inspector_row_for_a_css_escaped_path_is_found() {
    let got = eval(
        "<!doctype html><body><div id=tree><div data-path=\"0.1.1\"><div class=line>x</div></div></div>",
        "var n = document.getElementById('tree').querySelector('[data-path=\"' + CSS.escape('0.1.1') + '\"]'); \
         n ? n.querySelector('.line').className : 'none'",
    );
    assert_eq!(got, "line");
}
