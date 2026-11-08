//! Named access on the Window object (HTML §7.2.2.3): `<div id=target>`
//! makes bare `target` and `window.target` the element, looked up when read.

use super::*;

const PAGE: &str = "<!DOCTYPE html><html><body>\
    <div id=target>hello</div>\
    <form name=signup></form><img name=logo src=a.png>\
    <span name=notexposed></span>\
    <p id=dup>a</p><p id=dup>b</p>\
    <img id=both name=both><embed name=both>\
    </body></html>";

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

fn thrown(b: &DomBindings, script: &str) -> String {
    ev(b, &format!("(function () {{ try {{ {script} }} catch (e) {{ return e.name; }} return 'no error'; }})()"))
}

#[test]
fn an_id_is_a_bare_global_and_a_window_property() {
    let b = bound();
    assert_eq!(ev(&b, "target.textContent"), "hello");
    assert_eq!(ev(&b, "window.target === document.getElementById('target')"), "true");
    assert_eq!(ev(&b, "typeof target"), "object");
    assert_eq!(ev(&b, "'target' in window"), "true");
    assert_eq!(ev(&b, "target instanceof HTMLDivElement"), "true");
}

#[test]
fn a_name_on_form_and_img_is_a_global_but_not_on_other_elements() {
    let b = bound();
    assert_eq!(ev(&b, "[signup.localName, window.logo.localName].join()"), "form,img");
    assert_eq!(ev(&b, "typeof notexposed + ',' + ('notexposed' in window)"), "undefined,false");
    assert_eq!(thrown(&b, "notexposed;"), "ReferenceError");
}

#[test]
fn several_matches_are_an_html_collection() {
    let b = bound();
    assert_eq!(
        ev(&b, "[dup instanceof HTMLCollection, dup.length, dup[0].textContent, dup[1].textContent].join()"),
        "true,2,a,b"
    );
    // An id and a name on different elements both count, in tree order.
    assert_eq!(ev(&b, "[both.length, both[0].localName, both[1].localName].join()"), "2,img,embed");
}

#[test]
fn declared_globals_shadow_named_properties() {
    let b = bound();
    b.evaluate("var target = 1;").unwrap();
    assert_eq!(ev(&b, "typeof target + ',' + window.target"), "number,1");
    let b = bound();
    b.evaluate("function signup() { return 'fn'; }").unwrap();
    assert_eq!(ev(&b, "signup()"), "fn");
    let b = bound();
    b.evaluate("let logo = 'lex';").unwrap();
    assert_eq!(ev(&b, "logo"), "lex");
    // A plain assignment on window wins too.
    let b = bound();
    b.evaluate("window.target = 'set';").unwrap();
    assert_eq!(ev(&b, "target"), "set");
}

#[test]
fn names_are_live() {
    let b = bound();
    assert_eq!(thrown(&b, "late;"), "ReferenceError");
    b.evaluate("var d = document.createElement('div'); d.id = 'late'; document.body.appendChild(d);")
        .unwrap();
    assert_eq!(ev(&b, "typeof late + ',' + (late === d)"), "object,true");
    b.evaluate("d.remove();").unwrap();
    assert_eq!(thrown(&b, "late;"), "ReferenceError");
    assert_eq!(ev(&b, "typeof window.late"), "undefined");
    // An existing element losing its id stops resolving.
    assert_eq!(ev(&b, "typeof target"), "object");
    b.evaluate("document.getElementById('target').removeAttribute('id');").unwrap();
    assert_eq!(thrown(&b, "target;"), "ReferenceError");
    assert_eq!(ev(&b, "'target' in window"), "false");
}

#[test]
fn named_properties_are_not_own_or_enumerable_on_window() {
    let b = bound();
    assert_eq!(
        ev(&b, "[Object.keys(window).indexOf('target'), Object.getOwnPropertyNames(window).indexOf('signup'), window.hasOwnProperty('target')].join()"),
        "-1,-1,false"
    );
    let mut keys = Vec::new();
    if let JsValue::String(s) = b.evaluate("var k = []; for (var p in window) k.push(p); k.join(' ')").unwrap() {
        keys = s.split(' ').map(String::from).collect();
    }
    assert!(!keys.iter().any(|k| k == "target" || k == "dup"), "{keys:?}");
}

#[test]
fn builtins_and_missing_names_are_unchanged() {
    let b = bound();
    assert_eq!(ev(&b, "typeof nothingHere"), "undefined");
    assert_eq!(thrown(&b, "nothingHere;"), "ReferenceError");
    assert_eq!(ev(&b, "window.toString === Object.prototype.toString"), "true");
    assert_eq!(ev(&b, "typeof window.addEventListener"), "function");
    assert_eq!(ev(&b, "Object.getPrototypeOf(window) === Window.prototype"), "true");
}

/// A browsing-context name (an iframe's `name`) is not a named property
/// here; reading one stays unresolved and is reported as unsupported.
#[test]
fn an_iframe_name_is_reported_unsupported() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<!DOCTYPE html><html><body><iframe name=child></iframe><iframe id=byid></iframe></body></html>").unwrap(),
    ))
    .unwrap();
    assert!(b.take_unsupported_apis().is_empty());
    assert_eq!(ev(&b, "typeof nothingHere"), "undefined");
    assert!(b.take_unsupported_apis().is_empty(), "a plain miss is not reported");
    assert_eq!(ev(&b, "typeof child"), "undefined");
    let apis = b.take_unsupported_apis();
    assert_eq!((apis[0].0.as_str(), apis[0].1.as_str()), ("Window", "named access (iframe name)"));
    // An iframe's id is an id like any other (the element, not its WindowProxy).
    assert_eq!(ev(&b, "byid.localName"), "iframe");
}

/// Only document-tree elements are named properties: ids inside a shadow
/// tree or a template's contents are not exposed (as in Chromium).
#[test]
fn shadow_and_template_ids_are_not_named_properties() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<!DOCTYPE html><html><body><div id=host></div><template><p id=intemplate></p></template></body></html>").unwrap(),
    ))
    .unwrap();
    b.evaluate("host.attachShadow({ mode: 'open' }).innerHTML = '<span id=inshadow></span><form name=shadowform></form>';")
        .unwrap();
    assert_eq!(ev(&b, "host.shadowRoot.getElementById('inshadow') !== null"), "true");
    assert_eq!(
        ev(&b, "[typeof inshadow, typeof shadowform, typeof intemplate, 'inshadow' in window].join()"),
        "undefined,undefined,undefined,false"
    );
}
