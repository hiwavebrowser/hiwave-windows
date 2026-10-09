//! IntersectionObserver and ResizeObserver that report (web_observers_live.js),
//! over hand-written geometry, computed styles and scroll state.

use super::*;
use std::collections::HashMap;

const PAGE: &str = "<html><body><div id=a></div><div id=b></div><div id=c></div></body></html>";

fn bound() -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    b.evaluate("window.innerWidth = 1000; window.innerHeight = 800; var log = [];").unwrap();
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

fn boxed(x: f32, y: f32, w: f32, h: f32) -> BoxGeometry {
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

fn place(b: &DomBindings, doc: &Document, boxes: &[(&str, BoxGeometry)]) {
    let mut m = HashMap::new();
    for (name, g) in boxes {
        m.insert(doc.get_element_by_id(name).unwrap().id.raw(), *g);
    }
    b.set_geometry(m);
}

fn sized(b: &DomBindings, doc: &Document, name: &str, w: &str, h: &str) {
    let mut m = HashMap::new();
    m.insert(
        doc.get_element_by_id(name).unwrap().id.raw(),
        format!("width\t{w}\nheight\t{h}\npadding-left\t3px\npadding-top\t2px"),
    );
    b.set_computed_styles(m);
}

#[test]
fn an_observed_element_gets_an_initial_entry_whether_or_not_it_shows() {
    let (b, doc) = bound();
    place(&b, &doc, &[("a", boxed(0.0, 100.0, 200.0, 100.0)), ("b", boxed(0.0, 1500.0, 200.0, 100.0))]);
    b.evaluate(
        "var io = new IntersectionObserver(function (es, o) { es.forEach(function (e) { log.push(e.target.id + ':' + e.isIntersecting + ':' + e.intersectionRatio); }); log.push(o === io); });\
         io.observe(document.getElementById('a')); io.observe(document.getElementById('b'));",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.length"), "0", "nothing is delivered synchronously");
    b.run_timers(100, 10).unwrap();
    assert_eq!(ev(&b, "log.join()"), "a:true:1,b:false:0,true");
    // Nothing changed: nothing more.
    assert_eq!(b.tick_observers(), 1);
    assert_eq!(ev(&b, "log.length"), "3");
}

#[test]
fn scrolling_crosses_thresholds() {
    let (b, doc) = bound();
    // 100 px tall at document y 1000; the viewport is 800 tall.
    place(&b, &doc, &[("a", boxed(0.0, 1000.0, 200.0, 100.0))]);
    b.evaluate(
        "var io = new IntersectionObserver(function (es) { es.forEach(function (e) { log.push(e.isIntersecting + ':' + e.intersectionRatio); }); }, { threshold: [0, 0.5, 1] });\
         io.observe(document.getElementById('a'));",
    )
    .unwrap();
    b.tick_observers();
    assert_eq!(ev(&b, "log.join()"), "false:0", "below the viewport");
    // Scrolled so 50 of its 100 px show (top at 750).
    b.set_scroll_state((0.0, 250.0), (0.0, 2200.0));
    b.tick_observers();
    assert_eq!(ev(&b, "log.join()"), "false:0,true:0.5");
    // Fully in view.
    b.set_scroll_state((0.0, 400.0), (0.0, 2200.0));
    b.tick_observers();
    assert_eq!(ev(&b, "log.join()"), "false:0,true:0.5,true:1");
    // The same state again reports nothing; leaving the viewport reports.
    b.tick_observers();
    assert_eq!(ev(&b, "log.length"), "3");
    b.set_scroll_state((0.0, 2200.0), (0.0, 2200.0));
    b.tick_observers();
    assert_eq!(ev(&b, "log.slice(3).join()"), "false:0");
}

#[test]
fn root_margin_widens_the_root() {
    let (b, doc) = bound();
    place(&b, &doc, &[("a", boxed(0.0, 1000.0, 200.0, 100.0))]);
    b.evaluate(
        "var io = new IntersectionObserver(function (es) { es.forEach(function (e) { log.push(e.isIntersecting); }); }, { rootMargin: '300px 0px' });\
         io.observe(document.getElementById('a'));",
    )
    .unwrap();
    b.tick_observers();
    assert_eq!(ev(&b, "log.join()"), "true", "1000 is inside 800 + 300");
    assert_eq!(ev(&b, "io.rootMargin"), "300px 0px");
}

#[test]
fn an_entry_carries_the_rects_and_the_target() {
    let (b, doc) = bound();
    place(&b, &doc, &[("a", boxed(10.0, 20.0, 200.0, 100.0))]);
    b.evaluate(
        "var got; var io = new IntersectionObserver(function (es) { got = es[0]; });\
         io.observe(document.getElementById('a'));",
    )
    .unwrap();
    b.tick_observers();
    assert_eq!(
        ev(&b, "[got.boundingClientRect.x, got.boundingClientRect.width, got.intersectionRect.height, got.rootBounds.width, got.rootBounds.height, got.target.id, typeof got.time].join()"),
        "10,200,100,1000,800,a,number"
    );
}

#[test]
fn unobserve_disconnect_and_takerecords() {
    let (b, doc) = bound();
    place(&b, &doc, &[("a", boxed(0.0, 0.0, 10.0, 10.0)), ("b", boxed(0.0, 0.0, 10.0, 10.0))]);
    b.evaluate(
        "var io = new IntersectionObserver(function (es) { es.forEach(function (e) { log.push(e.target.id); }); });\
         io.observe(document.getElementById('a')); io.observe(document.getElementById('b'));\
         io.unobserve(document.getElementById('a'));",
    )
    .unwrap();
    b.tick_observers();
    assert_eq!(ev(&b, "log.join()"), "b");
    b.evaluate("io.disconnect();").unwrap();
    assert_eq!(b.tick_observers(), 0, "a disconnected observer is gone");
    assert_eq!(ev(&b, "io.takeRecords().length"), "0");
}

#[test]
fn the_constructor_validates() {
    let (b, _) = bound();
    assert_eq!(ev(&b, "try { new IntersectionObserver(1); } catch (e) { e.name }"), "TypeError");
    assert_eq!(ev(&b, "try { new IntersectionObserver(function () {}, { threshold: 2 }); } catch (e) { e.name }"), "RangeError");
    assert_eq!(ev(&b, "try { new IntersectionObserver(function () {}, { rootMargin: '5em' }); } catch (e) { e.name }"), "SyntaxError");
    assert_eq!(ev(&b, "try { new IntersectionObserver(function () {}).observe({}); } catch (e) { e.name }"), "TypeError");
    assert_eq!(ev(&b, "var io = new IntersectionObserver(function () {}, { threshold: [1, 0.25] }); io.thresholds.join()"), "0.25,1");
    assert_eq!(ev(&b, "[io.root, io.rootMargin].join()"), ",0px 0px 0px 0px");
}

#[test]
fn a_resize_observer_reports_a_sized_element_and_then_a_change() {
    let (b, doc) = bound();
    place(&b, &doc, &[("a", boxed(0.0, 0.0, 106.0, 54.0)), ("b", boxed(0.0, 0.0, 0.0, 0.0))]);
    sized(&b, &doc, "a", "100px", "50px");
    b.evaluate(
        "var ro = new ResizeObserver(function (es, o) { es.forEach(function (e) { log.push(e.target.id + ':' + e.contentRect.width + 'x' + e.contentRect.height + ':' + e.borderBoxSize[0].inlineSize + ':' + e.contentRect.left); }); });\
         ro.observe(document.getElementById('a')); ro.observe(document.getElementById('b'));",
    )
    .unwrap();
    b.run_timers(100, 10).unwrap();
    assert_eq!(ev(&b, "log.join()"), "a:100x50:106:3", "the zero-size element reports nothing");
    sized(&b, &doc, "a", "120px", "50px");
    b.tick_observers();
    assert_eq!(ev(&b, "log.length"), "2");
    b.tick_observers();
    assert_eq!(ev(&b, "log.length"), "2", "unchanged: nothing");
}

#[test]
fn a_callback_that_throws_is_reported_and_does_not_stop_the_others() {
    let (b, doc) = bound();
    place(&b, &doc, &[("a", boxed(0.0, 0.0, 10.0, 10.0))]);
    b.evaluate(
        "var one = new IntersectionObserver(function () { throw new Error('boom'); });\
         var two = new IntersectionObserver(function () { log.push('two'); });\
         one.observe(document.getElementById('a')); two.observe(document.getElementById('a'));",
    )
    .unwrap();
    b.tick_observers();
    assert_eq!(ev(&b, "log.join()"), "two");
}

#[test]
fn a_user_scroll_fires_the_scroll_event_at_once() {
    let (b, _) = bound();
    b.evaluate("document.addEventListener('scroll', function () { log.push('scroll'); });").unwrap();
    b.notify_scrolled();
    assert_eq!(ev(&b, "log.join()"), "scroll");
}
