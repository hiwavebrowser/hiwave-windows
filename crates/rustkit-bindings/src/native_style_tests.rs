//! NATIVE-SETPROPERTY cross-check: the native inline style (`inline_style.rs`) against the JavaScript
//! copy it replaces (`__rk.styleNative(false)`), on the same writes from the same attribute text. What
//! is compared is everything a page can see: the attribute text after each write, every read
//! (`cssText`, `length`, each item with its value and priority), what each call returned, and the
//! MutationObserver records (`attributes`, name, old value). The one rule that differs on purpose is
//! the bad-url check, which the native path makes as CSS's tokenizer does: those rows are below, apart.

use super::*;

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><body><div id=t></div></body></html>").unwrap()))
        .unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

fn set_native(b: &DomBindings, native: bool) {
    b.runtime
        .borrow_mut()
        .evaluate_host_bookkeeping(&format!("__rk.styleNative({native}); return 0"))
        .unwrap();
}

/// Starting attribute texts (`None`: no attribute).
const STARTS: &[Option<&str>] = &[
    None,
    Some(""),
    Some("left: 1px; top: 2px"),
    Some("color: red !important; left:1px"),
    Some(" left : 1px ; ; top:2px;"),
    Some("--x: a; --X: b; --y:{ c: d; }"),
    Some("background-image: url(data:image/svg+xml;base64,AA==); width: 1px"),
    Some("font-family: \"A;B\", serif; content: 'a\\'b'"),
    Some("transform: rotate(3deg]; color: red /* c */"),
    Some("LEFT: 1PX; Color: Red ! Important"),
    Some("left: 1px; left: 2px; top: 3px; left: 4px"),
    Some("width: url(it's); height: 2px"),
    Some("x"),
    Some("\u{feff}left: 1px;\u{a0}top:\u{2003}2px\u{3000}"),
    Some("color: red\u{a0}; left:\u{feff}1px\u{feff}; top: 2px\u{2003} ! important"),
    Some("background-image: url(a\u{a0}b); width: url(a\u{2003}b); height: 1px"),
];

/// Each a statement on `s` (the element's style) that leaves its result in `r`.
const OPS: &[&str] = &[
    "r = (s.left = '5px')",
    "r = s.setProperty('COLOR', ' blue ', 'IMPORTANT')",
    "r = s.setProperty('color', 'blue', 'important')",
    "r = s.setProperty('left', '')",
    "r = s.setProperty('left', null)",
    "r = s.setProperty('left', undefined)",
    "r = s.removeProperty('top')",
    "r = s.removeProperty(' TOP ')",
    "r = s.removeProperty('nope')",
    "r = s.setProperty('--y', '{ b: c; }')",
    "r = s.setProperty('--Custom', '3px')",
    "r = (s.cssFloat = 'left')",
    "r = (s.webkitTransform = 'rotate(1deg)')",
    "r = s.setProperty('a;b', '1px')",
    "r = s.setProperty('x:y', '2px')",
    "r = s.setProperty('transform', 'rotate(3deg]')",
    "r = s.setProperty('color', 'red /* c */')",
    "r = s.setProperty('font-family', '\"A;B\", serif')",
    "r = s.setProperty('content', '\"a\\\\\"b\"')",
    "r = s.setProperty('background-image', 'url(\"a;b.png\")')",
    "r = s.setProperty('background-image', 'url(a b)')",
    "r = s.setProperty('left', s.getPropertyValue('left'))",
    "r = (s.left = 5)",
    "r = (s.opacity = 0.5)",
    "r = s.setProperty('left', '6px', 'bogus')",
    "r = s.setProperty('left', '7px !important')",
    "r = s.setProperty('  ', '1px')",
    "r = (s.left = '')",
    "r = s.setProperty('color', 'red\\u00a0')",
    "r = s.setProperty('left', '\\ufeff5px\\ufeff')",
    "r = s.setProperty('left', '\\u20035px\\u2003')",
    "r = s.setProperty('background-image', 'url(a\\u00a0b)')",
    "r = s.setProperty('background-image', 'url(a\\u0085b)')",
    "r = s.setProperty('background-image', 'url(\"a\\u00a0b\")')",
];

/// Everything a page can read of the style and its writes, after `op` on `start`.
fn observe(b: &DomBindings, start: Option<&str>, op: &str) -> String {
    let set_start = match start {
        None => "e.removeAttribute('style');".to_string(),
        Some(t) => format!("e.setAttribute('style', {t:?});"),
    };
    ev(
        b,
        &format!(
            "(function () {{ var e = document.getElementById('t'); {set_start} var s = e.style, r; \
             var mo = new MutationObserver(function () {{}}); mo.observe(e, {{ attributes: true, attributeOldValue: true }}); \
             var thrown = ''; try {{ {op}; }} catch (x) {{ thrown = 'threw ' + x.name; }} \
             var recs = mo.takeRecords().map(function (m) {{ return m.type + ':' + m.attributeName + ':' + m.oldValue; }}); mo.disconnect(); \
             var items = []; for (var i = 0; i < s.length; i++) items.push(s[i] + '=' + s.getPropertyValue(s[i]) + '/' + s.getPropertyPriority(s[i])); \
             return [thrown, typeof r + ':' + r, 'attr=' + e.getAttribute('style'), 'css=' + s.cssText, 'len=' + s.length, \
               items.join(','), 'item99=' + s.item(99), 'recs=' + recs.join(';')].join(' | '); }})()"
        ),
    )
}

#[test]
fn native_inline_style_reads_and_writes_as_the_javascript_copy_does() {
    let b = bound();
    let mut differ = Vec::new();
    let mut rows = 0;
    for start in STARTS {
        for op in OPS {
            set_native(&b, false);
            let js = observe(&b, *start, op);
            set_native(&b, true);
            let native = observe(&b, *start, op);
            rows += 1;
            if js != native {
                differ.push(format!("start {start:?}, {op}\n    js:     {js}\n    native: {native}"));
            }
        }
    }
    assert_eq!(rows, STARTS.len() * OPS.len());
    assert!(differ.is_empty(), "{} of {rows} rows differ:\n{}", differ.len(), differ.join("\n"));
}

/// A custom element watching `style` hears each native write that changed the attribute, with the
/// old and new text, as it does from the JavaScript copy, and nothing for a write that changed nothing.
#[test]
fn a_custom_element_watching_style_hears_native_writes_as_before() {
    let script = "customElements.define('x-s', class extends HTMLElement { static get observedAttributes() { return ['style']; } \
                    attributeChangedCallback(n, o, v) { (window.__heard = window.__heard || []).push(n + ':' + o + '>' + v); } }); \
                  var x = document.createElement('x-s'); document.body.appendChild(x); window.__heard = []; \
                  x.style.left = '1px'; x.style.left = '1px'; x.style.setProperty('top', '2px', 'important'); \
                  x.style.removeProperty('left'); x.style.removeProperty('left'); \
                  window.__heard.join(',')";
    let mut out = Vec::new();
    for native in [false, true] {
        let b = bound();
        set_native(&b, native);
        out.push(ev(&b, script));
    }
    assert_eq!(out[0], out[1]);
    assert_eq!(
        out[1],
        "style:null>left: 1px;,style:left: 1px;>left: 1px; top: 2px !important;,style:left: 1px; top: 2px !important;>top: 2px !important;"
    );
}

/// #1266's values pinned against Chrome 148 (docs/diagnostics/2026-11-17/style_url_value_chrome148.json):
/// both copies keep or refuse each as Chrome does, through `setProperty` and through the attribute.
/// The native check (`inline_style::has_bad_url`, Rust) and #1266's (`hasBadUrl`, JS) are the same rule
/// written twice; this is where they must agree.
#[test]
fn both_copies_keep_or_refuse_the_chrome_pinned_url_values() {
    let doc: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/diagnostics/2026-11-17/style_url_value_chrome148.json")).unwrap();
    assert_eq!(doc["chrome"].as_str(), Some("148.0.7778.216"));
    let b = bound();
    let mut wrong = Vec::new();
    let cases = doc["cases"].as_array().unwrap();
    // As style_value_url_tests reads the file (#1266), with its lists: rows Chrome refuses for another rule
    // are skipped, rows the engine's url reader refuses stay refused here, and each value is built from its
    // UTF-16 units.
    use crate::style_value_url_tests::{ENGINE_REFUSES, NOT_THE_URL_RULE};
    for native in [true, false] {
        set_native(&b, native);
        for case in cases {
            let css = case["css"].as_str().unwrap();
            if NOT_THE_URL_RULE.contains(&css) {
                continue;
            }
            let units: Vec<String> =
                case["css_units"].as_array().unwrap().iter().map(|u| u.as_u64().unwrap().to_string()).collect();
            let lit = format!("String.fromCharCode({})", units.join(","));
            let got = ev(
                &b,
                &format!(
                    "var el = document.getElementById('t'); el.removeAttribute('style');
                     el.style.setProperty('--x', 'before'); el.style.setProperty('--x', {lit});
                     var viaSet = el.style.getPropertyValue('--x') !== 'before';
                     el.setAttribute('style', '--x: ' + {lit});
                     viaSet + '|' + (el.style.length > 0)"
                ),
            );
            // As style_value_url_tests reads the file: a value that is only a comment is kept by Chrome
            // with an empty value, and dropped here as an empty declaration (not the url rule).
            let want_attr =
                !css.trim_start().starts_with("/*") && case["kept_via_attribute"].as_bool().unwrap();
            let want = if ENGINE_REFUSES.contains(&css) {
                "false|false".to_string()
            } else {
                format!("{}|{want_attr}", case["kept_via_setProperty"].as_bool().unwrap())
            };
            if got != want {
                wrong.push(format!("{} {css:?}: {got}, Chrome {want}", if native { "native" } else { "js" }));
            }
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), 2 * cases.len(), wrong.join("\n"));
}

/// The native path refuses a value holding a bad url as CSS's tokenizer reads it, escapes in the
/// function name decoded: the declaration is not taken and the attribute is not written.
#[test]
fn native_writes_refuse_an_escaped_bad_url() {
    let b = bound();
    for value in ["u\\\\72l(a b)", "\\\\75rl(a b)", "\\\\55 RL(a b)", "U\\\\52L(a b)", "ur\\\\l(a b)", "image-set(url(a b) 1x)"] {
        let got = ev(
            &b,
            &format!(
                "var e = document.getElementById('t'); e.setAttribute('style', 'left: 1px'); var s = e.style; \
                 s.setProperty('background-image', '{value}'); s.setProperty('--x', '{value}'); \
                 e.getAttribute('style') + '|' + s.length"
            ),
        );
        assert_eq!(got, "left: 1px|1", "{value}");
    }
}
