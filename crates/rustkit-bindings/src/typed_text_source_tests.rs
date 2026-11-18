//! R1 MED 5: what the user types is never script source.
//!
//! The engine's key events were dispatched by evaluating a line of script
//! with the key written into it (`document.__rkFireKey(node, "keydown",
//! { key: "q", ... })`), and `textInput` the same with the typed text.
//! Anything that keeps or quotes evaluated source (an error log, an
//! Inspector's script list) then holds what was typed. The text goes to
//! the dispatcher through a host argument instead.

use super::*;

#[test]
fn a_typed_character_is_never_written_into_script_source() {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html("<!DOCTYPE html><html><body><input id=f></body></html>").unwrap());
    bindings.set_document(doc.clone()).unwrap();
    bindings
        .evaluate(
            "window.heard = []; ['keydown', 'keypress', 'keyup', 'textInput'].forEach(function (t) { \
             document.addEventListener(t, function (e) { heard.push(t + ':' + (t === 'textInput' ? e.data : e.key) + ':' + e.isTrusted); }, true); });",
        )
        .unwrap();
    let field = doc.get_element_by_id("f").unwrap().id.raw();
    let sources: Rc<RefCell<Vec<String>>> = Rc::default();
    let tap = sources.clone();
    bindings.runtime.borrow_mut().tap_sources(Some(Rc::new(move |source: &str| tap.borrow_mut().push(source.to_string()))));
    // A character no script of the engine's own contains.
    let typed = "\u{2603}";
    let data = KeyboardEventBindingData { key: typed.to_string(), ..Default::default() };
    for event in ["keydown", "keypress", "textInput", "keyup"] {
        bindings.fire_key_event(Some(field), event, &data).unwrap();
    }
    bindings.runtime.borrow_mut().tap_sources(None);
    let leaked: Vec<String> = sources.borrow().iter().filter(|s| s.contains(typed)).cloned().collect();
    assert!(leaked.is_empty(), "the typed character is in {} evaluated source(s), e.g. {:?}", leaked.len(), leaked.first());
    assert!(!sources.borrow().is_empty(), "the tap saw the dispatch scripts");
    match bindings.evaluate("heard.join(' ')").unwrap() {
        JsValue::String(s) => assert_eq!(s, "keydown:\u{2603}:true keypress:\u{2603}:true textInput:\u{2603}:true keyup:\u{2603}:true"),
        other => panic!("{other:?}"),
    }
}
