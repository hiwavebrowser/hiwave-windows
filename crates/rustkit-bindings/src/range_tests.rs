//! `Range` and `document.elementFromPoint`. A range parses HTML into a
//! fragment (`createContextualFragment`); scripts in it run when the
//! fragment is inserted. A point is null until a layout is published.

use super::*;
use std::collections::HashMap;

const PAGE: &str = "<html><body><div id=outer><p id=inner>abcdef</p></div><table id=t></table></body></html>";

fn bound() -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    (b, doc)
}

fn id(doc: &Document, name: &str) -> usize {
    doc.get_element_by_id(name).unwrap().id.raw()
}

fn geom(x: f32, y: f32, w: f32, h: f32) -> BoxGeometry {
    BoxGeometry {
        x,
        y,
        width: w,
        height: h,
        border_left: 0.0,
        border_top: 0.0,
        client_width: w,
        client_height: h,
        scroll_width: w,
        scroll_height: h,
        position: 0,
        ..Default::default()
    }
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

#[test]
fn a_new_range_is_collapsed_at_the_document_and_can_select_a_node() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var r = document.createRange(); [r instanceof Range, r.collapsed, r.startContainer === document, r.startOffset, r.endOffset, new Range().collapsed].join()"),
        "true,true,true,0,0,true"
    );
    assert_eq!(
        ev(&b, "var p = document.getElementById('inner'); var r = document.createRange(); r.selectNodeContents(p); [r.startContainer === p, r.startOffset, r.endOffset, r.collapsed, r.commonAncestorContainer === p].join()"),
        "true,0,1,false,true"
    );
    assert_eq!(
        ev(&b, "var p = document.getElementById('inner'); var r = document.createRange(); r.selectNode(p); [r.startContainer === p.parentNode, r.startOffset === Array.prototype.indexOf.call(p.parentNode.childNodes, p), r.endOffset === r.startOffset + 1].join()"),
        "true,true,true"
    );
    assert_eq!(
        ev(&b, "var r = document.createRange(); r.selectNodeContents(document.getElementById('inner')); r.collapse(); String(r.collapsed)"),
        "true"
    );
    assert_eq!(
        ev(&b, "var name = ''; try { document.createRange().setStart(document.body, 99); } catch (e) { name = e.name; } name"),
        "IndexSizeError"
    );
}

#[test]
fn clone_and_delete_contents_cover_one_container() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var t = document.getElementById('inner').firstChild; var r = document.createRange(); r.setStart(t, 2); r.setEnd(t, 5); var f = r.cloneContents(); [f.nodeType, f.textContent, t.data].join()"),
        "11,cde,abcdef"
    );
    assert_eq!(
        ev(&b, "var t = document.getElementById('inner').firstChild; var r = document.createRange(); r.setStart(t, 1); r.setEnd(t, 4); r.deleteContents(); [t.data, r.collapsed, r.startOffset].join()"),
        "aef,true,1"
    );
    assert_eq!(
        ev(&b, "var o = document.getElementById('outer'); var r = document.createRange(); r.selectNodeContents(o); var f = r.cloneContents(); [f.childNodes.length, f.firstChild.id, o.childNodes.length].join()"),
        "1,inner,1"
    );
}

// ---- Range.surroundContents / insertNode / extractContents (CH2.E47) ----
// The app's find-in-page wraps each match with `range.surroundContents(mark)`;
// before this it threw there, after adding its <style>, so nothing highlighted.

const MIXED: &str = "<html><body><p id=p>ab<b id=b>cd</b>ef</p><div id=d><i>x</i><u>y</u></div><span id=s>old</span></body></html>";

fn mixed() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(MIXED).unwrap())).unwrap();
    b
}

#[test]
fn surround_contents_wraps_a_text_range_and_selects_the_wrapper() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var p = document.getElementById('inner'); var t = p.firstChild; var r = document.createRange(); r.setStart(t, 2); r.setEnd(t, 5); var m = document.createElement('span'); m.className = 'hit'; r.surroundContents(m); [p.innerHTML, m.textContent, r.startContainer === p, r.startOffset, r.endContainer === p, r.endOffset].join('|')"),
        "ab<span class=\"hit\">cde</span>f|cde|true|1|true|2"
    );
}

#[test]
fn surround_contents_wraps_whole_elements_and_a_range_across_text_ends() {
    let b = mixed();
    assert_eq!(
        ev(&b, "var d = document.getElementById('d'); var r = document.createRange(); r.selectNodeContents(d); var w = document.createElement('em'); r.surroundContents(w); d.innerHTML"),
        "<em><i>x</i><u>y</u></em>"
    );
    // The ends sit in two different text nodes, with an element between them
    // that the range contains whole: "b" + <b>cd</b> + "e" move into the wrapper.
    assert_eq!(
        ev(&b, "var p = document.getElementById('p'); var r = document.createRange(); r.setStart(p.firstChild, 1); r.setEnd(p.lastChild, 1); var w = document.createElement('mark'); r.surroundContents(w); p.innerHTML"),
        "a<mark>b<b id=\"b\">cd</b>e</mark>f"
    );
}

#[test]
fn surround_contents_empties_the_new_parent_first() {
    let b = mixed();
    assert_eq!(
        ev(&b, "var p = document.getElementById('p'); var s = document.getElementById('s'); var r = document.createRange(); r.setStart(p.firstChild, 0); r.setEnd(p.firstChild, 2); r.surroundContents(s); [p.innerHTML, s.textContent].join('|')"),
        "<span id=\"s\">ab</span><b id=\"b\">cd</b>ef|ab"
    );
}

#[test]
fn surround_contents_refuses_a_range_that_cuts_an_element_and_changes_nothing() {
    let b = mixed();
    // Starts inside <b>, ends after it: <b> is partially contained.
    assert_eq!(
        ev(&b, "var p = document.getElementById('p'); var before = p.innerHTML; var r = document.createRange(); r.setStart(document.getElementById('b').firstChild, 1); r.setEnd(p.lastChild, 1); var name = 'none'; try { r.surroundContents(document.createElement('i')); } catch (e) { name = e.name; } [name, p.innerHTML === before].join()"),
        "InvalidStateError,true"
    );
}

#[test]
fn surround_contents_refuses_a_document_type_and_a_fragment_as_the_wrapper() {
    let b = mixed();
    for wrapper in ["document", "document.createDocumentFragment()"] {
        let script = format!(
            "var p = document.getElementById('p'); var r = document.createRange(); r.setStart(p.firstChild, 0); r.setEnd(p.firstChild, 1); var name = 'none'; try {{ r.surroundContents({wrapper}); }} catch (e) {{ name = e.name; }} [name, p.firstChild.data].join()"
        );
        assert_eq!(ev(&b, &script), "InvalidNodeTypeError,ab", "{wrapper}");
    }
    assert_eq!(
        ev(&b, "var r = document.createRange(); var name = 'none'; try { r.surroundContents({}); } catch (e) { name = e.name; } name"),
        "TypeError"
    );
}

#[test]
fn insert_node_splits_a_text_start_and_collapsed_ranges_follow_the_insertion() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var p = document.getElementById('inner'); var r = document.createRange(); r.setStart(p.firstChild, 2); r.collapse(true); var i = document.createElement('i'); r.insertNode(i); [p.childNodes.length, p.innerHTML, r.endContainer === p, r.endOffset].join('|')"),
        "3|ab<i></i>cdef|true|2"
    );
    // A fragment goes in whole; an element start inserts before the child at the offset.
    assert_eq!(
        ev(&b, "var o = document.getElementById('outer'); var r = document.createRange(); r.setStart(o, 0); r.collapse(true); var f = document.createDocumentFragment(); f.appendChild(document.createElement('a')); f.appendChild(document.createElement('b')); r.insertNode(f); [o.firstChild.localName, o.childNodes[1].localName, o.childNodes[2].id, r.endOffset].join()"),
        "a,b,inner,2"
    );
}

#[test]
fn insert_node_refuses_a_comment_start() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var c = document.createComment('x'); document.getElementById('outer').appendChild(c); var r = document.createRange(); r.setStart(c, 0); var name = 'none'; try { r.insertNode(document.createElement('i')); } catch (e) { name = e.name; } name"),
        "HierarchyRequestError"
    );
}

#[test]
fn extract_contents_hands_back_what_it_removes() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var t = document.getElementById('inner').firstChild; var r = document.createRange(); r.setStart(t, 1); r.setEnd(t, 4); var f = r.extractContents(); [f.nodeType, f.textContent, t.data, r.collapsed].join()"),
        "11,bcd,aef,true"
    );
}

// ---- find_in_page.js must not rewrite the page beyond the wrapped matches (#1024 R1 nit 1) ----

const FIND_JS: &str = include_str!("../../hiwave-app/src/ui/find_in_page.js");

fn find_page(setup: &str) -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html("<html><head></head><body><div id=keep></div><p id=hit></p></body></html>").unwrap());
    b.set_document(doc).unwrap();
    b.evaluate(setup).unwrap();
    b.evaluate(FIND_JS).unwrap();
    b
}

#[test]
fn find_leaves_the_pages_own_adjacent_text_nodes_alone() {
    // Two adjacent text nodes the page made on purpose (a framework holds both). The old
    // find ended with document.body.normalize(), which merged them.
    let b = find_page(
        "var k = document.getElementById('keep'); k.appendChild(document.createTextNode('xx ')); k.appendChild(document.createTextNode('yy ')); \
         document.getElementById('hit').appendChild(document.createTextNode('say hello now'));",
    );
    assert_eq!(
        ev(&b, "var k = document.getElementById('keep'); var a = k.firstChild, c = k.lastChild; var r = window.__hiwaveFind.run({query: 'hello', case_sensitive: false, direction: 'reset'}); [r.count, k.childNodes.length, k.firstChild === a, k.lastChild === c, a.data, c.data].join('|')"),
        "1|2|true|true|xx |yy "
    );
}

#[test]
fn clearing_a_find_puts_every_node_back_exactly_as_it_was() {
    let b = find_page(
        "var h = document.getElementById('hit'); h.appendChild(document.createTextNode('hello a hello b hello')); h.appendChild(document.createElement('i')).textContent = 'hello in i';",
    );
    assert_eq!(
        ev(&b, "var h = document.getElementById('hit'); var t = h.firstChild, i = h.lastChild; var before = h.innerHTML; \
                var r = window.__hiwaveFind.run({query: 'hello', case_sensitive: false, direction: 'reset'}); var marked = h.querySelectorAll('.zen-find-match').length; \
                window.__hiwaveFind.run({query: '', direction: 'reset'}); \
                [r.count, marked, h.childNodes.length, h.firstChild === t, h.lastChild === i, h.innerHTML === before, t.data, h.querySelectorAll('.zen-find-match').length].join('|')"),
        "4|4|2|true|true|true|hello a hello b hello|0"
    );
}

#[test]
fn create_contextual_fragment_parses_in_context_and_runs_scripts_on_insert() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var f = document.createRange().createContextualFragment('<b id=hi>Hi</b>'); [f.nodeType, f.querySelector('#hi').textContent, f.querySelector('#hi').parentNode === f, String(document.getElementById('hi'))].join()"),
        "11,Hi,true,null"
    );
    assert_eq!(
        ev(&b, "var f = document.createRange().createContextualFragment('<b id=hi2>Hi</b><script>window.__ran = 1</script>'); var before = String(window.__ran); document.body.appendChild(f); [before, window.__ran, document.getElementById('hi2').textContent].join()"),
        "undefined,1,Hi"
    );
    assert_eq!(
        ev(&b, "var table = document.getElementById('t'); var r = document.createRange(); r.selectNodeContents(table); var f = r.createContextualFragment('<tr><td>A</td></tr>'); function find(n, name) { if (n.localName === name) return n; for (var c = n.firstChild; c; c = c.nextSibling) { var h = find(c, name); if (h) return h; } return null; } var cell = find(f, 'td'); [cell && cell.textContent, cell && cell.parentNode.localName, f.firstChild.localName].join()"),
        "A,tr,tbody"
    );
}

#[test]
fn element_from_point_is_null_until_a_layout_and_then_the_deepest_box() {
    let (b, doc) = bound();
    assert_eq!(ev(&b, "String(document.elementFromPoint(20, 30))"), "null");
    assert_eq!(ev(&b, "document.elementsFromPoint(20, 30).length"), "0");
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(0.0, 0.0, 200.0, 100.0));
    m.insert(id(&doc, "inner"), geom(10.0, 20.0, 40.0, 20.0));
    b.set_geometry(m);
    assert_eq!(ev(&b, "document.elementFromPoint(20, 30).id"), "inner");
    assert_eq!(ev(&b, "document.elementsFromPoint(20, 30).map(function (e) { return e.id || e.localName; }).join()"), "inner,outer,body,html");
    assert_eq!(ev(&b, "document.elementFromPoint(80, 30).id"), "outer");
    assert_eq!(ev(&b, "String(document.elementFromPoint(-1, 10))"), "null");
    assert_eq!(ev(&b, "String(document.elementFromPoint(900, 10))"), "null");
    let name = ev(&b, "var name = ''; try { document.elementFromPoint(Infinity, 0); } catch (e) { name = e.name; } name");
    assert_eq!(name, "TypeError");
    b.set_scroll_state((0.0, 500.0), (0.0, 500.0));
    let mut m = HashMap::new();
    m.insert(id(&doc, "inner"), geom(10.0, 520.0, 40.0, 20.0));
    b.set_geometry(m);
    assert_eq!(ev(&b, "document.elementFromPoint(20, 30).id"), "inner");
}
