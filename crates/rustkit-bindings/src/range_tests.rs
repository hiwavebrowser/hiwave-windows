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
