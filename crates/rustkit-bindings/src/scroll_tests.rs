//! Scroll position for script (web_scroll.js): the window offset the engine
//! publishes, `scrollTo`/`scrollBy`, `scrollIntoView` over published
//! geometry, element `scrollTop`.

use super::*;
use std::collections::HashMap;

const PAGE: &str = "<html><body><div id=far></div><div id=near></div><div id=box></div></body></html>";

fn bound() -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    b.evaluate("window.innerWidth = 1000; window.innerHeight = 800;").unwrap();
    // A 3000 px tall document in a 800 px viewport: 2200 px of scroll.
    b.set_scroll_state((0.0, 0.0), (0.0, 2200.0));
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

fn boxed(x: f32, y: f32, w: f32, h: f32, scroll_w: f32, scroll_h: f32) -> BoxGeometry {
    BoxGeometry {
        x,
        y,
        width: w,
        height: h,
        border_left: 0.0,
        border_top: 0.0,
        client_width: w,
        client_height: h,
        scroll_width: scroll_w,
        scroll_height: scroll_h,
        position: 0,
    }
}

fn place(b: &DomBindings, doc: &Document, name: &str, g: BoxGeometry) {
    let id = doc.get_element_by_id(name).unwrap().id.raw();
    let mut m = HashMap::new();
    m.insert(id, g);
    b.set_geometry(m);
}

#[test]
fn the_window_reads_the_offset_the_engine_published() {
    let (b, _) = bound();
    assert_eq!(ev(&b, "[scrollX, scrollY, pageXOffset, pageYOffset].join()"), "0,0,0,0");
    b.set_scroll_state((0.0, 450.0), (0.0, 2200.0));
    assert_eq!(ev(&b, "[scrollX, scrollY, pageXOffset, pageYOffset].join()"), "0,450,0,450");
}

#[test]
fn scrollto_clamps_to_the_document_and_is_handed_to_the_engine() {
    let (b, _) = bound();
    assert_eq!(b.take_scroll_request(), None);
    b.evaluate("scrollTo(0, 500)").unwrap();
    assert_eq!(ev(&b, "scrollY"), "500");
    assert_eq!(b.take_scroll_request(), Some((0.0, 500.0)));
    assert_eq!(b.take_scroll_request(), None, "taken once");
    // Past the end, and above the start.
    b.evaluate("scrollTo(0, 99999)").unwrap();
    assert_eq!(ev(&b, "scrollY"), "2200");
    b.evaluate("scrollTo(0, -50)").unwrap();
    assert_eq!(ev(&b, "scrollY"), "0");
    assert_eq!(b.take_scroll_request(), Some((0.0, 0.0)), "the last position wins");
}

#[test]
fn the_options_form_scrollby_and_a_kept_axis() {
    let (b, _) = bound();
    b.set_scroll_state((0.0, 0.0), (300.0, 2200.0));
    b.evaluate("scrollTo({ top: 100, left: 40, behavior: 'smooth' })").unwrap();
    assert_eq!(ev(&b, "[scrollX, scrollY].join()"), "40,100");
    b.evaluate("scrollBy(10, 25)").unwrap();
    assert_eq!(ev(&b, "[scrollX, scrollY].join()"), "50,125");
    b.evaluate("scrollBy({ top: 5 })").unwrap();
    assert_eq!(ev(&b, "[scrollX, scrollY].join()"), "50,130", "an axis left out is kept");
    b.evaluate("scroll(0, 7)").unwrap();
    assert_eq!(ev(&b, "[scrollX, scrollY].join()"), "0,7");
}

#[test]
fn a_scroll_fires_one_scroll_event_from_a_timer() {
    let (b, _) = bound();
    b.evaluate("var n = 0; window.addEventListener('scroll', function () { n++; }); document.addEventListener('scroll', function () { n += 10; });").unwrap();
    b.evaluate("scrollTo(0, 100); scrollTo(0, 200);").unwrap();
    assert_eq!(ev(&b, "n"), "0", "not synchronous");
    b.run_timers(100, 10).unwrap();
    assert_eq!(ev(&b, "n"), "11", "one event on the document and one on the window");
    // No movement, no event.
    b.evaluate("scrollTo(0, 200)").unwrap();
    b.run_timers(100, 10).unwrap();
    assert_eq!(ev(&b, "n"), "11");
}

#[test]
fn assigning_scrolly_replaces_it_as_the_platform_does() {
    let (b, _) = bound();
    assert_eq!(ev(&b, "window.scrollY = 900; scrollY"), "900");
    assert_eq!(b.take_scroll_request(), None, "an assignment does not scroll");
}

#[test]
fn scrollintoview_aligns_the_element() {
    let (b, doc) = bound();
    // 100 px tall at document y 1000, viewport 800 tall.
    place(&b, &doc, "far", boxed(0.0, 1000.0, 200.0, 100.0, 200.0, 100.0));
    let far = "document.getElementById('far')";
    b.evaluate(&format!("{far}.scrollIntoView()")).unwrap();
    assert_eq!(ev(&b, "scrollY"), "1000", "block start is the default");
    b.evaluate(&format!("{far}.scrollIntoView(false)")).unwrap();
    assert_eq!(ev(&b, "scrollY"), "300", "false aligns the bottom edge: 1000 + 100 - 800");
    b.evaluate(&format!("{far}.scrollIntoView({{ block: 'center' }})")).unwrap();
    assert_eq!(ev(&b, "scrollY"), "650", "1000 + 50 - 400");
    b.evaluate(&format!("{far}.scrollIntoView({{ block: 'end', behavior: 'smooth' }})")).unwrap();
    assert_eq!(ev(&b, "scrollY"), "300");
    // 'nearest' leaves a visible element alone, and moves a hidden one the least.
    b.evaluate("scrollTo(0, 950)").unwrap();
    b.evaluate(&format!("{far}.scrollIntoView({{ block: 'nearest' }})")).unwrap();
    assert_eq!(ev(&b, "scrollY"), "950");
    b.evaluate("scrollTo(0, 0)").unwrap();
    b.evaluate(&format!("{far}.scrollIntoView({{ block: 'nearest' }})")).unwrap();
    assert_eq!(ev(&b, "scrollY"), "300", "below the viewport: its bottom edge goes to the bottom");
    assert_eq!(b.take_scroll_request(), Some((0.0, 300.0)));
}

#[test]
fn an_element_with_no_box_scrolls_nowhere() {
    let (b, _) = bound();
    b.evaluate("scrollTo(0, 400)").unwrap();
    b.evaluate("document.getElementById('near').scrollIntoView()").unwrap();
    // No geometry: its rect reads 0,0 in the viewport, so block start keeps the offset.
    assert_eq!(ev(&b, "scrollY"), "400");
}

#[test]
fn the_root_element_scrolls_the_window_and_other_elements_keep_their_own_offset() {
    let (b, doc) = bound();
    b.evaluate("document.documentElement.scrollTop = 321").unwrap();
    assert_eq!(
        ev(&b, "[scrollY, document.documentElement.scrollTop, document.scrollingElement === document.documentElement].join()"),
        "321,321,true"
    );
    assert_eq!(b.take_scroll_request(), Some((0.0, 321.0)));
    // A 200x100 box whose content is 200x400: scrollTop clamps to 300.
    place(&b, &doc, "box", boxed(0.0, 0.0, 200.0, 100.0, 200.0, 400.0));
    b.evaluate("var e = document.getElementById('box'); e.scrollTop = 999;").unwrap();
    assert_eq!(ev(&b, "e.scrollTop"), "300");
    b.evaluate("e.scrollBy(0, -50); e.scrollLeft = 10;").unwrap();
    assert_eq!(ev(&b, "[e.scrollTop, e.scrollLeft].join()"), "250,0", "no horizontal overflow: scrollLeft stays 0");
    assert_eq!(ev(&b, "scrollY"), "321", "an element's own scroll does not move the window");
}
