//! Layout geometry for script (CSSOM View): `getBoundingClientRect`,
//! `offsetWidth/Height/Top/Left`, `offsetParent`, `clientWidth/Height`,
//! `scrollWidth/Height`. The engine publishes boxes with `set_geometry`; here
//! the boxes are written by hand.

use super::*;
use std::collections::HashMap;

const PAGE: &str = "<html><body><div id=outer><p id=inner>hi</p></div><span id=gone></span><p id=fixed></p></body></html>";

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
    }
}

fn bound() -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    (b, doc)
}

fn id(doc: &Document, name: &str) -> usize {
    doc.get_element_by_id(name).unwrap().id.raw()
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
fn an_element_with_no_box_reads_as_zero_and_has_no_offset_parent() {
    let (b, _) = bound();
    assert_eq!(
        ev(&b, "var r = document.getElementById('outer').getBoundingClientRect(); [r.x, r.y, r.width, r.height, r.top, r.left, r.right, r.bottom].join()"),
        "0,0,0,0,0,0,0,0"
    );
    assert_eq!(
        ev(&b, "var e = document.getElementById('outer'); [e.offsetWidth, e.offsetHeight, e.offsetTop, e.offsetLeft, e.clientWidth, e.clientHeight, e.scrollWidth, e.scrollHeight, String(e.offsetParent), e.getClientRects().length].join()"),
        "0,0,0,0,0,0,0,0,null,0"
    );
}

#[test]
fn getboundingclientrect_reports_the_border_box() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(8.0, 16.0, 300.0, 120.0));
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var r = document.getElementById('outer').getBoundingClientRect(); [r.x, r.y, r.width, r.height, r.top, r.left, r.right, r.bottom].join()"),
        "8,16,300,120,16,8,308,136"
    );
    assert_eq!(ev(&b, "document.getElementById('outer').getClientRects().length"), "1");
    assert_eq!(
        ev(&b, "JSON.stringify(document.getElementById('outer').getBoundingClientRect())"),
        r#"{"x":8,"y":16,"width":300,"height":120,"top":16,"right":308,"bottom":136,"left":8}"#
    );
}

#[test]
fn a_rect_is_viewport_relative_and_follows_the_scroll_offset() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(8.0, 1000.0, 300.0, 120.0));
    b.set_geometry(m);
    b.evaluate("window.scrollY = 900; window.pageYOffset = 900;").unwrap();
    assert_eq!(ev(&b, "document.getElementById('outer').getBoundingClientRect().top"), "100");
}

#[test]
fn offset_and_client_metrics_come_from_the_boxes() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    let mut outer = geom(8.0, 16.0, 300.5, 120.4);
    outer.border_left = 2.0;
    outer.border_top = 3.0;
    outer.client_width = 296.5;
    outer.client_height = 114.4;
    outer.scroll_width = 296.5;
    outer.scroll_height = 400.0;
    m.insert(id(&doc, "outer"), outer);
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var e = document.getElementById('outer'); [e.offsetWidth, e.offsetHeight, e.clientWidth, e.clientHeight, e.clientLeft, e.clientTop, e.scrollWidth, e.scrollHeight].join()"),
        "301,120,297,114,2,3,297,400"
    );
}

#[test]
fn offset_top_and_left_are_from_the_offset_parent() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    // #outer is positioned (relative), so it is #inner's offsetParent.
    let mut outer = geom(50.0, 100.0, 300.0, 120.0);
    outer.position = 1;
    outer.border_left = 2.0;
    outer.border_top = 2.0;
    m.insert(id(&doc, "outer"), outer);
    m.insert(id(&doc, "inner"), geom(60.0, 130.0, 100.0, 20.0));
    // A static element's offsetParent is the body, and its offsets are document coordinates.
    m.insert(id(&doc, "gone"), geom(5.0, 7.0, 10.0, 10.0));
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var e = document.getElementById('inner'); [e.offsetParent.id, e.offsetLeft, e.offsetTop].join()"),
        "outer,8,28"
    );
    assert_eq!(
        ev(&b, "var s = document.getElementById('gone'); [s.offsetParent === document.body, s.offsetLeft, s.offsetTop].join()"),
        "true,5,7"
    );
}

#[test]
fn a_fixed_element_has_no_offset_parent() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    let mut fixed = geom(0.0, 0.0, 50.0, 50.0);
    fixed.position = 2;
    m.insert(id(&doc, "fixed"), fixed);
    b.set_geometry(m);
    assert_eq!(ev(&b, "String(document.getElementById('fixed').offsetParent)"), "null");
    assert_eq!(ev(&b, "document.getElementById('fixed').offsetWidth"), "50");
}

#[test]
fn geometry_is_replaced_not_merged_by_the_next_publish() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(0.0, 0.0, 10.0, 10.0));
    b.set_geometry(m);
    assert_eq!(ev(&b, "document.getElementById('outer').offsetWidth"), "10");
    b.set_geometry(HashMap::new());
    assert_eq!(ev(&b, "document.getElementById('outer').offsetWidth"), "0");
}

#[test]
fn the_root_element_measures_the_viewport_and_the_document() {
    let (b, doc) = bound();
    b.evaluate("window.innerWidth = 1280; window.innerHeight = 800;").unwrap();
    let mut m = HashMap::new();
    let mut body = geom(0.0, 0.0, 1280.0, 3000.0);
    body.client_width = 1280.0;
    body.client_height = 3000.0;
    body.scroll_width = 1280.0;
    body.scroll_height = 3000.0;
    m.insert(doc.body().unwrap().id.raw(), body);
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var r = document.documentElement; [r.clientWidth, r.clientHeight, r.scrollWidth, r.scrollHeight].join()"),
        "1280,800,1280,3000"
    );
}

fn styles(rows: &[(&str, &str)]) -> String {
    rows.iter().map(|(n, v)| format!("{n}\t{v}")).collect::<Vec<_>>().join("\n")
}

#[test]
fn getcomputedstyle_reads_the_published_style() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(0.0, 0.0, 10.0, 10.0));
    b.set_geometry(m);
    let mut s = HashMap::new();
    s.insert(
        id(&doc, "outer"),
        styles(&[("display", "flex"), ("color", "rgb(1, 2, 3)"), ("font-size", "20px"), ("margin-top", "4px"), ("float", "left")]),
    );
    b.set_computed_styles(s);
    assert_eq!(
        ev(&b, "var c = getComputedStyle(document.getElementById('outer')); [c.display, c.getPropertyValue('color'), c.fontSize, c['margin-top'], c.marginTop, c.cssFloat, c.getPropertyValue('nope') === ''].join()"),
        "flex,rgb(1, 2, 3),20px,4px,4px,left,true"
    );
    assert_eq!(ev(&b, "var c = getComputedStyle(document.getElementById('outer')); [c.length, c.item(0), typeof c.getPropertyPriority].join()"), "5,color,function");
    assert_eq!(
        ev(&b, "var c = getComputedStyle(document.getElementById('outer')); try { c.setProperty('color', 'red'); 'no throw' } catch (e) { e.name }"),
        "NoModificationAllowedError"
    );
    assert_eq!(ev(&b, "getComputedStyle(document.getElementById('outer')) instanceof CSSStyleDeclaration"), "true");
}

#[test]
fn an_inline_declaration_shows_through_at_once() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(0.0, 0.0, 10.0, 10.0));
    b.set_geometry(m);
    let mut s = HashMap::new();
    s.insert(id(&doc, "outer"), styles(&[("display", "block")]));
    b.set_computed_styles(s);
    assert_eq!(
        ev(&b, "var e = document.getElementById('outer'); e.style.display = 'none'; getComputedStyle(e).display"),
        "none"
    );
}

#[test]
fn an_element_with_no_box_after_a_layout_is_display_none() {
    let (b, doc) = bound();
    // Before any layout nothing is known.
    assert_eq!(ev(&b, "getComputedStyle(document.getElementById('gone')).display"), "");
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(0.0, 0.0, 10.0, 10.0));
        b.set_geometry(m);
    assert_eq!(
        ev(&b, "var c = getComputedStyle(document.getElementById('gone')); [c.display, c.visibility].join()"),
        "none,visible"
    );
}

#[test]
fn getcomputedstyle_rejects_a_non_element() {
    let (b, _) = bound();
    assert_eq!(ev(&b, "try { getComputedStyle(null); 'no throw' } catch (e) { e.name }"), "TypeError");
    assert_eq!(ev(&b, "try { getComputedStyle(document); 'no throw' } catch (e) { e.name }"), "TypeError");
}
