//! A noscript's text children serialize raw only where scripting is on for
//! the node (HTML 13.3; Chrome for Testing 148, the engine's
//! html1_noscript_serialize fixture). Here: a page whose own scripting flag
//! is off, where nothing in a noscript is raw.

use super::*;

#[test]
fn with_scripting_off_for_the_document_a_noscripts_text_is_escaped() {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html("<!DOCTYPE html><html><body><div id=d><noscript>x</noscript></div></body></html>").unwrap());
    assert!(!doc.scripting(), "parsed without the flag: scripting is off");
    bindings.set_document(doc).unwrap();
    let html = bindings
        .evaluate(
            "var n = document.querySelector('noscript'); n.textContent = '</noscript><img src=x onerror=1>'; \
             document.getElementById('d').innerHTML + ' | ' + n.outerHTML",
        )
        .unwrap();
    match html {
        JsValue::String(s) => assert_eq!(
            s,
            "<noscript>&lt;/noscript&gt;&lt;img src=x onerror=1&gt;</noscript> | \
             <noscript>&lt;/noscript&gt;&lt;img src=x onerror=1&gt;</noscript>"
        ),
        other => panic!("{other:?}"),
    }
}
