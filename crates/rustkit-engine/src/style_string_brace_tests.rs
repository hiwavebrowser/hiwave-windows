//! Z plan CH2.E31: a `{`, `}` or `;` inside a CSS string is text, not
//! structure. The rule parser ended a style rule at a `}` inside a quoted
//! declaration value (`a{content:"}"}`) and opened one at a `{` inside an
//! attribute selector's string, so the rule and everything after it went
//! wrong, in a plain `<style>` and in a `<style media>` alike.
//!
//! Expected colours are Chrome for Testing 148.0.7778.216's on the same
//! pages (headless, `getComputedStyle`): for every sheet below the `a`
//! is red and the following `b` is blue, with or without `media="all"`.

use super::*;

const RED: &str = "rgb(255, 0, 0)";
const BLUE: &str = "rgb(0, 0, 255)";

const SHEETS: &[(&str, &str)] = &[
    (r#"a{content:"}";color:rgb(255,0,0)} b{color:rgb(0,0,255)}"#, ""),
    (r#"a{content:"{;";color:rgb(255,0,0)} b{color:rgb(0,0,255)}"#, ""),
    ("a{content:'}';color:rgb(255,0,0)} b{color:rgb(0,0,255)}", ""),
    (r#"a{background-image:url("x}.png");color:rgb(255,0,0)} b{color:rgb(0,0,255)}"#, ""),
    (r#"a[title="}"]{color:rgb(255,0,0)} b{color:rgb(0,0,255)}"#, "}"),
    (r#"a[title="{"]{color:rgb(255,0,0)} b{color:rgb(0,0,255)}"#, "{"),
    (r#"a[title=";"]{color:rgb(255,0,0)} b{color:rgb(0,0,255)}"#, ";"),
    (r#"@media all{a{content:"}";color:rgb(255,0,0)} b{color:rgb(0,0,255)}}"#, ""),
    (r#"i{& a{content:"}";color:rgb(255,0,0)}} b{color:rgb(0,0,255)}"#, ""),
];

fn color_of(engine: &mut Engine, id: EngineViewId, tag: &str) -> String {
    let value = engine
        .execute_script(id, &format!("getComputedStyle(document.querySelector('{tag}')).color"))
        .expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

#[test]
fn braces_and_semicolons_in_css_strings_keep_the_rule_and_the_next_one() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for media in ["", " media=\"all\""] {
        for (css, title) in SHEETS {
            let html = format!(
                "<style{media}>{css}</style><i><a title=\"{title}\">x</a></i><b>y</b>"
            );
            let id = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
            engine.load_html(id, &html).expect("load");
            let (a, b) = (color_of(&mut engine, id, "a"), color_of(&mut engine, id, "b"));
            if a != RED || b != BLUE {
                wrong.push(format!("<style{media}>{css}: a {a}, b {b}"));
            }
            let _ = engine.destroy_view(id);
        }
    }
    assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
}

/// #994 R1 HOLD: a backslash before a newline continues the string, and
/// CRLF, CR and FF each count as one newline (CSS Syntax 3 §3.3, §4.3.5).
/// `\` CR LF ended the string at the LF and the closing quote reopened it,
/// swallowing the next rule. CfT 148 on every page below: `a` red, `b`
/// blue, and the `i` after an `@media all` block green
/// (`scratchpad/fix39/e31_crlf_probe.mjs`).
#[test]
fn an_escaped_newline_in_a_css_string_keeps_the_rule_and_the_next_one() {
    const GREEN: &str = "rgb(0, 128, 0)";
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    let bs = char::from(92);
    for (what, nl) in [
        ("CRLF", String::from_iter([char::from(13), char::from(10)])),
        ("CR", String::from(char::from(13))),
        ("LF", String::from(char::from(10))),
        ("FF", String::from(char::from(12))),
    ] {
        let esc = format!("{bs}{nl}");
        let sheets = [
            (format!("a{{content:\"x{esc}y\";color:rgb(255,0,0)}} b{{color:rgb(0,0,255)}}"), ""),
            // The string sits in the prelude; the `a` simple selector does
            // the matching (the matcher does not yet drop an escaped
            // newline from an attribute value, a separate gap).
            (format!("a[title=\"x{esc}y\"], a{{color:rgb(255,0,0)}} b{{color:rgb(0,0,255)}}"), ""),
            (format!("@media all{{a{{content:\"x{esc}y\";color:rgb(255,0,0)}} b{{color:rgb(0,0,255)}}}} i{{color:rgb(0,128,0)}}"), ""),
        ];
        for (css, title) in &sheets {
            let html = format!("<style>{css}</style><i><a title=\"{title}\">x</a></i><b>y</b>");
            let id = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
            engine.load_html(id, &html).expect("load");
            let (a, b) = (color_of(&mut engine, id, "a"), color_of(&mut engine, id, "b"));
            let i_ok = !css.starts_with("@media") || color_of(&mut engine, id, "i") == GREEN;
            if a != RED || b != BLUE || !i_ok {
                wrong.push(format!("{what} {css:?}: a {a}, b {b}, i after media ok {i_ok}"));
            }
            let _ = engine.destroy_view(id);
        }
    }
    assert!(wrong.is_empty(), "{} wrong\n{}", wrong.len(), wrong.join("\n"));
}
