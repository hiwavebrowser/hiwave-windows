//! #812 forced-layout budget and decline paths.
//!
//! A write/read loop in one script task (cnn's header) must not hold the
//! page thread forever. `FORCED_LAYOUTS_PER_TASK` (256) and the busy
//! (`None`) decline are the product brake. Engine companions live under
//! `feature = "headless"` (`script_fresh_layout_tests`); these pins exercise
//! the bindings half on every `cargo test -p rustkit-bindings --lib`.

use super::*;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

const PAGE: &str = "<html><body><div id=outer>hi</div></body></html>";

fn geom(w: f32, h: f32) -> BoxGeometry {
    BoxGeometry {
        x: 0.0,
        y: 0.0,
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

fn bound() -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    (b, doc)
}

fn outer_id(doc: &Document) -> usize {
    doc.get_element_by_id("outer").unwrap().id.raw()
}

fn read_width(b: &DomBindings) {
    let _ = b
        .evaluate("document.getElementById('outer').offsetWidth")
        .unwrap();
}

fn write_attr(b: &DomBindings, n: u64) {
    b.evaluate(&format!(
        "document.getElementById('outer').setAttribute('data-i', '{n}')"
    ))
    .unwrap();
}

fn counting_forced(calls: Rc<Cell<u64>>) -> ForcedLayout {
    Rc::new(move |_writes| {
        calls.set(calls.get() + 1);
        Some(ForcedGeometry {
            source: Box::new(|| (HashMap::new(), HashMap::new())),
            hit_tester: None,
            max_scroll: (0.0, 0.0),
        })
    })
}

#[test]
fn a_clean_read_does_not_force_layout() {
    let (b, doc) = bound();
    let calls = Rc::new(Cell::new(0u64));
    b.set_forced_layout(Some(counting_forced(calls.clone())));
    let mut m = HashMap::new();
    m.insert(outer_id(&doc), geom(10.0, 10.0));
    b.set_geometry(m);
    read_width(&b);
    assert_eq!(calls.get(), 0, "no write since layout → no force");
    assert_eq!(b.take_forced_layouts(), 0);
}

#[test]
fn a_write_then_read_forces_layout_once() {
    let (b, doc) = bound();
    let calls = Rc::new(Cell::new(0u64));
    b.set_forced_layout(Some(counting_forced(calls.clone())));
    let mut m = HashMap::new();
    m.insert(outer_id(&doc), geom(10.0, 10.0));
    b.set_geometry(m);
    write_attr(&b, 1);
    read_width(&b);
    assert_eq!(calls.get(), 1);
    assert_eq!(b.take_forced_layouts(), 1);
}

#[test]
fn the_task_budget_stops_further_forced_layouts() {
    let (b, doc) = bound();
    let calls = Rc::new(Cell::new(0u64));
    b.set_forced_layout(Some(counting_forced(calls.clone())));
    let mut m = HashMap::new();
    m.insert(outer_id(&doc), geom(10.0, 10.0));
    b.set_geometry(m);

    let budget = crate::dom::FORCED_LAYOUTS_PER_TASK;
    for n in 0..(budget + 4) {
        write_attr(&b, n);
        read_width(&b);
    }
    assert_eq!(
        calls.get(),
        budget,
        "after {budget} forces the brake must hold"
    );
    assert_eq!(b.take_forced_layouts(), budget);

    // take_forced_layouts renews the budget for the next task.
    write_attr(&b, 999);
    read_width(&b);
    assert_eq!(calls.get(), budget + 1);
    assert_eq!(b.take_forced_layouts(), 1);
}

#[test]
fn a_busy_forced_layout_declines_without_counting() {
    let (b, doc) = bound();
    b.set_forced_layout(Some(Rc::new(|_writes| None)));
    let mut m = HashMap::new();
    m.insert(outer_id(&doc), geom(10.0, 10.0));
    b.set_geometry(m);
    write_attr(&b, 1);
    read_width(&b);
    assert_eq!(
        b.take_forced_layouts(),
        0,
        "None from the callback is build-state-in-use, not a tally"
    );
}
