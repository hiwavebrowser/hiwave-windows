//! CH2.E2: `window.getSelection()` is a real Selection over one Range, and
//! `Range` stringifies. The engine moves the selection with
//! `set_selection` and hears script's changes with `take_selection_change`.

use super::*;

const PAGE: &str = "<html><body><p id=p>Hello <b id=b>brave</b> world</p><div id=d>two</div></body></html>";

fn bound() -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    (b, doc)
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

fn text_of(doc: &Document, id: &str) -> usize {
    doc.get_element_by_id(id).unwrap().children()[0].id.raw()
}

#[test]
fn range_to_string_spans_text_nodes_and_elements() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var p = document.getElementById('p'); var r = document.createRange(); r.setStart(p.firstChild, 3); r.setEnd(p.lastChild, 3); r.toString()"),
        "lo brave wo"
    );
    assert_eq!(
        ev(&b, "var r = document.createRange(); r.selectNodeContents(document.getElementById('p')); String(r)"),
        "Hello brave world"
    );
    assert_eq!(
        ev(&b, "var r = document.createRange(); r.selectNodeContents(document.body); r.toString()"),
        "Hello brave worldtwo"
    );
    assert_eq!(
        ev(&b, "var r = document.createRange(); r.selectNode(document.getElementById('b')); typeof r.getBoundingClientRect().width"),
        "number"
    );
}

#[test]
fn a_new_selection_is_empty() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var s = getSelection(); [s.rangeCount, s.isCollapsed, s.type, s.anchorNode, s.focusOffset, s.toString() === '', s instanceof Selection, document.getSelection() === s].join()"),
        "0,true,None,,0,true,true,true"
    );
}

#[test]
fn add_range_selects_and_remove_all_ranges_clears() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var s = getSelection(); var r = document.createRange(); r.selectNodeContents(document.getElementById('b')); s.addRange(r); [s.rangeCount, s.isCollapsed, s.type, s.toString(), s.anchorNode.id, s.anchorOffset, s.focusOffset, s.getRangeAt(0) === r].join()"),
        "1,false,Range,brave,b,0,1,true"
    );
    assert_eq!(ev(&b, "var s = getSelection(); s.removeAllRanges(); [s.rangeCount, s.toString(), String(s.anchorNode)].join()"), "0,,null");
}

#[test]
fn collapse_extend_and_select_all_children() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var s = getSelection(); var t = document.getElementById('p').firstChild; s.collapse(t, 4); var c = [s.isCollapsed, s.type, s.anchorOffset].join(); s.extend(t, 1); [c, s.toString(), s.anchorOffset, s.focusOffset, s.direction, s.getRangeAt(0).startOffset].join()"),
        "true,Caret,4,ell,4,1,backward,1"
    );
    assert_eq!(
        ev(&b, "var s = getSelection(); s.selectAllChildren(document.getElementById('d')); [s.toString(), s.anchorNode.id, s.focusOffset].join()"),
        "two,d,1"
    );
    assert_eq!(
        ev(&b, "var s = getSelection(); s.selectAllChildren(document.getElementById('p')); [s.containsNode(document.getElementById('b')), s.containsNode(document.getElementById('d'))].join()"),
        "true,false"
    );
}

#[test]
fn selection_change_fires_on_the_document() {
    let (b, _) = bound();
    ev(&b, "window.__changes = 0; document.addEventListener('selectionchange', function () { window.__changes++; });");
    ev(&b, "var r = document.createRange(); r.selectNodeContents(document.getElementById('b')); getSelection().addRange(r);");
    assert_eq!(ev(&b, "String(window.__changes)"), "1");
    ev(&b, "getSelection().removeAllRanges();");
    assert_eq!(ev(&b, "String(window.__changes)"), "2");
}

#[test]
fn the_engine_hears_script_changes_and_script_sees_engine_changes() {
    let (b, doc) = bound();
    assert_eq!(b.take_selection_change(), None);
    let d = text_of(&doc, "d");
    ev(&b, "var r = document.createRange(); r.setStart(document.getElementById('d').firstChild, 1); r.setEnd(document.getElementById('d').firstChild, 3); getSelection().addRange(r);");
    assert_eq!(b.take_selection_change(), Some(Some((d, 1, d, 3))));
    assert_eq!(b.take_selection_change(), None, "told once");

    let p = text_of(&doc, "p");
    b.set_selection(Some((p, 6, p, 0))).unwrap();
    assert_eq!(b.take_selection_change(), None, "the engine's own move is not echoed");
    assert_eq!(ev(&b, "var s = getSelection(); [s.toString(), s.anchorOffset, s.focusOffset, s.direction].join()"), "Hello ,6,0,backward");
    b.set_selection(None).unwrap();
    assert_eq!(ev(&b, "String(getSelection().rangeCount)"), "0");
}

#[test]
fn edit_keys_type_delete_and_break_lines_in_a_contenteditable_host() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html("<html><body><div id=e contenteditable>ab</div></body></html>").unwrap());
    b.set_document(doc.clone()).unwrap();
    let host = doc.get_element_by_id("e").unwrap().id.raw();
    ev(&b, "window.__inputs = []; document.getElementById('e').addEventListener('input', function (e) { window.__inputs.push(e.inputType + ':' + e.data); });");
    // No selection in the host: the caret goes to its end.
    assert!(b.edit_key(host, "insert", "c").unwrap());
    assert_eq!(ev(&b, "document.getElementById('e').textContent"), "abc");
    assert!(b.edit_key(host, "left", "").unwrap());
    assert!(b.edit_key(host, "back", "").unwrap());
    assert_eq!(ev(&b, "document.getElementById('e').textContent"), "ac");
    assert!(b.edit_key(host, "enter", "").unwrap());
    assert_eq!(ev(&b, "document.getElementById('e').innerHTML"), "a<br>c");
    assert!(b.edit_key(host, "back", "").unwrap(), "joins the lines again");
    assert_eq!(ev(&b, "document.getElementById('e').innerHTML"), "ac");
    assert!(b.edit_key(host, "del", "").unwrap());
    assert_eq!(ev(&b, "document.getElementById('e').textContent"), "a");
    assert!(b.edit_key(host, "all", "").unwrap());
    assert!(b.edit_key(host, "insert", "z").unwrap(), "typing replaces the selection");
    assert_eq!(ev(&b, "document.getElementById('e').textContent"), "z");
    assert_eq!(
        ev(&b, "window.__inputs.join()"),
        "insertText:c,deleteContentBackward:null,insertParagraph:null,deleteContentBackward:null,deleteContentForward:null,insertText:z"
    );
    assert!(!b.edit_key(host, "right", "").unwrap(), "nothing after the caret");
}
