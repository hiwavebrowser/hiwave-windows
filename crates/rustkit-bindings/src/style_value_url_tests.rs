//! Which inline style values hold a bad url (CSS Syntax 3 §4.3.6), so the declaration is not kept
//! (CH2.E29), against Chrome for Testing 148.0.7778.216
//! (docs/diagnostics/2026-11-17/style_url_value_chrome148.json, from
//! tools/parity_oracle/style_url_value_log.mjs). Written on a custom property, which keeps any value but
//! one with a bad url, so the answer is the url rule alone; through `setProperty` and through the style
//! attribute. Also: page script that replaces built-ins does not change the answer.

use super::*;

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><body><div id=a></div></body></html>").unwrap()))
        .unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => panic!("{script}: {other:?}"),
    }
}

/// Rows Chrome refuses for a reason other than the url rule, which this check does not make (separate rows):
/// an unmatched `)` (in `url\(a b)` the escaped `(` is a name character), and a string where
/// background-image takes an image.
pub(crate) const NOT_THE_URL_RULE: &[&str] = &["url\\(a b)", "\"url(a b)\""];

/// Rows Chrome keeps that stay refused here because the engine's url reader (`parse_css_url`, Rust whitespace
/// and control characters) refuses them: an unquoted url holding NBSP or NUL (a row against the engine).
pub(crate) const ENGINE_REFUSES: &[&str] = &["url(a\u{a0}b)", "url(a\0b)", "url(a\u{85}b)", "url(a\u{2003}b)"];

/// Regular-property values Chrome refuses by the property's grammar, which the bindings do not check (a
/// separate row, VALUE-GRAMMAR): a value holding NBSP, U+FEFF or U+2003 at either end is one token
/// that no color or length is. The bindings keep them, as they did before the CSS-whitespace trim
/// (which then stored `red` for `red` + NBSP).
const VALUE_GRAMMAR: &[(&str, &str)] = &[
    ("color", "red\u{a0}"),
    ("color", "\u{a0}red"),
    ("color", "red\u{feff}"),
    ("color", "\u{feff}red"),
    ("color", "red\u{2003}"),
    ("width", "10px\u{a0}"),
];

/// The exact value as a JS expression, from its UTF-16 code units (a lone surrogate does not survive JSON text).
fn from_units(case: &serde_json::Value) -> String {
    let units: Vec<String> = case["css_units"].as_array().unwrap().iter().map(|u| u.as_u64().unwrap().to_string()).collect();
    format!("String.fromCharCode({})", units.join(","))
}

#[test]
fn a_style_value_is_kept_or_refused_as_chrome_does() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/diagnostics/2026-11-17/style_url_value_chrome148.json")).unwrap();
    assert_eq!(doc["chrome"].as_str(), Some("148.0.7778.216"));
    let b = bound();
    let mut wrong = Vec::new();
    for case in doc["cases"].as_array().unwrap() {
        let css = case["css"].as_str().unwrap();
        if NOT_THE_URL_RULE.contains(&css) {
            continue;
        }
        let lit = from_units(case);
        let got = ev(
            &b,
            &format!(
                "var el = document.getElementById('a'); el.removeAttribute('style');
                 el.style.setProperty('--x', 'before'); el.style.setProperty('--x', {lit});
                 var viaSet = el.style.getPropertyValue('--x') !== 'before';
                 el.setAttribute('style', '--x: ' + {lit});
                 var viaAttr = el.style.length > 0;
                 viaSet + '|' + viaAttr"
            ),
        );
        let want_set = case["kept_via_setProperty"].as_bool().unwrap();
        // A value that is only a comment: Chrome keeps the declaration with an empty value; the attribute
        // path here drops a declaration whose value is empty (not the url rule; a separate row).
        let want_attr = if css.trim_start().starts_with("/*") { false } else { case["kept_via_attribute"].as_bool().unwrap() };
        let want = if ENGINE_REFUSES.contains(&css) { "false|false".to_string() } else { format!("{want_set}|{want_attr}") };
        if got != want {
            wrong.push(format!("{css:?}: kept (setProperty|attribute) {got}, Chrome {want}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), doc["cases"].as_array().unwrap().len(), wrong.join("\n"));
}

#[test]
fn a_value_on_a_regular_property_is_kept_or_refused_as_chrome_does() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/diagnostics/2026-11-17/style_url_value_chrome148.json")).unwrap();
    let b = bound();
    let mut wrong = Vec::new();
    for case in doc["by_property"].as_array().unwrap() {
        let (prop, css) = (case["property"].as_str().unwrap(), case["css"].as_str().unwrap());
        if NOT_THE_URL_RULE.contains(&css) || VALUE_GRAMMAR.contains(&(prop, css)) {
            continue;
        }
        // Through setProperty, and through the style attribute where the oracle wrote it that way too.
        let got = ev(
            &b,
            &format!(
                "var el = document.getElementById('a'), v = {}; el.removeAttribute('style');
                 el.style.setProperty('{prop}', v); var viaSet = el.style.getPropertyValue('{prop}') !== '';
                 el.setAttribute('style', '{prop}: ' + v); var viaAttr = el.style.getPropertyValue('{prop}') !== '';
                 viaSet + ({} ? '|' + viaAttr : '')",
                from_units(case),
                !case["kept_via_attribute"].is_null()
            ),
        );
        let mut want = case["kept_via_setProperty"].as_bool().unwrap().to_string();
        if let Some(attr) = case["kept_via_attribute"].as_bool() {
            want = format!("{want}|{attr}");
        }
        if got != want {
            wrong.push(format!("{prop}: {css:?}: kept {got}, Chrome {want}"));
        }
    }
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}

#[test]
fn replaced_built_ins_do_not_change_which_values_are_kept() {
    let b = bound();
    let got = ev(
        &b,
        "var el = document.getElementById('a'), out = [];
         var saved = [String.prototype.substr, String.prototype.toLowerCase, String.prototype.charAt,
                      RegExp.prototype.test, String.prototype.indexOf, String.prototype.charCodeAt];
         String.prototype.substr = function () { return 'x'; };
         String.prototype.toLowerCase = function () { return 'x'; };
         String.prototype.charAt = function () { return 'x'; };
         RegExp.prototype.test = function () { return false; };
         String.prototype.indexOf = function () { return -1; };
         String.prototype.charCodeAt = function () { return 120; };
         ['url(a b)', 'URL(a b)', 'url(a\"b)'].forEach(function (v) {
             el.style.setProperty('--x', 'before'); el.style.setProperty('--x', v);
             out.push(el.style.getPropertyValue('--x'));
         });
         String.prototype.substr = saved[0]; String.prototype.toLowerCase = saved[1]; String.prototype.charAt = saved[2];
         RegExp.prototype.test = saved[3]; String.prototype.indexOf = saved[4]; String.prototype.charCodeAt = saved[5];
         out.join('|')",
    );
    assert_eq!(got, "before|before|before", "Chrome 148 keeps 'before' (replaced_builtins in the oracle file)");
}
