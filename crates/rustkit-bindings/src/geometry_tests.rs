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
        ..Default::default()
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
fn a_static_element_stops_at_a_table_cell_and_a_positioned_one_does_not() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(
        Document::parse_html(
            "<html><body><table id=tb><tr id=tr><td id=c><p id=s></p><p id=r></p></td></tr></table></body></html>",
        )
        .unwrap(),
    );
    b.set_document(doc.clone()).unwrap();
    let mut m = HashMap::new();
    m.insert(id(&doc, "tb"), geom(5.0, 5.0, 100.0, 100.0));
    m.insert(id(&doc, "tr"), geom(5.0, 5.0, 100.0, 100.0));
    let mut cell = geom(5.0, 5.0, 100.0, 100.0);
    cell.border_left = 2.0;
    cell.border_top = 2.0;
    m.insert(id(&doc, "c"), cell);
    m.insert(id(&doc, "s"), geom(12.0, 13.0, 10.0, 10.0));
    let mut relative = geom(12.0, 33.0, 10.0, 10.0);
    relative.position = 1;
    m.insert(id(&doc, "r"), relative);
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var e = document.getElementById('s'); [e.offsetParent.id, e.offsetLeft, e.offsetTop].join()"),
        "c,5,6"
    );
    assert_eq!(ev(&b, "document.getElementById('c').offsetParent.id"), "tb");
    assert_eq!(ev(&b, "document.getElementById('tr').offsetParent.id"), "tb");
    assert_eq!(
        ev(&b, "var e = document.getElementById('r'); [e.offsetParent === document.body, e.offsetLeft, e.offsetTop].join()"),
        "true,12,33"
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
fn a_geometry_source_clears_old_maps_builds_once_and_serves_later_reads() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let (b, doc) = bound();
    let outer = id(&doc, "outer");
    let mut stale = HashMap::new();
    stale.insert(outer, geom(0.0, 0.0, 10.0, 10.0));
    b.set_geometry(stale);
    let mut stale_styles = HashMap::new();
    stale_styles.insert(outer, styles(&[("display", "flex")]));
    b.set_computed_styles(stale_styles);
    assert_eq!(ev(&b, "document.getElementById('outer').offsetWidth"), "10");

    let builds = Arc::new(AtomicUsize::new(0));
    let counted = builds.clone();
    b.set_geometry_source(Box::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        let mut geometry = HashMap::new();
        geometry.insert(outer, geom(0.0, 0.0, 40.0, 40.0));
        let mut computed = HashMap::new();
        computed.insert(outer, styles(&[("display", "block")]));
        (geometry, computed)
    }));
    assert_eq!(b.geometry_builds(), 0);
    // Publishing a source drops the previous maps: the first read must not
    // see width 10 / display flex from the eager publish above.
    assert_eq!(ev(&b, "document.getElementById('outer').offsetWidth"), "40");
    assert_eq!(
        ev(&b, "getComputedStyle(document.getElementById('outer')).display"),
        "block"
    );
    assert_eq!(b.geometry_builds(), 1);
    assert_eq!(builds.load(Ordering::SeqCst), 1);
    assert_eq!(ev(&b, "document.getElementById('outer').offsetWidth"), "40");
    assert_eq!(
        b.geometry_builds(),
        1,
        "later reads share the one build"
    );
}

#[test]
fn set_geometry_drops_a_pending_source_without_building_it() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let (b, doc) = bound();
    let outer = id(&doc, "outer");
    let builds = Arc::new(AtomicUsize::new(0));
    let counted = builds.clone();
    b.set_geometry_source(Box::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        (HashMap::new(), HashMap::new())
    }));
    let mut eager = HashMap::new();
    eager.insert(outer, geom(0.0, 0.0, 22.0, 22.0));
    b.set_geometry(eager);
    assert_eq!(ev(&b, "document.getElementById('outer').offsetWidth"), "22");
    assert_eq!(b.geometry_builds(), 0);
    assert_eq!(builds.load(Ordering::SeqCst), 0);
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
    // The published rows, the unset radii and the initial values of every
    // longhand the engine serializes (`COMPUTED_INITIAL`); shorthands and
    // empty values are not listed, `-webkit-` names come last, as in Chromium.
    assert_eq!(ev(&b, "var c = getComputedStyle(document.getElementById('outer')); [c.length, c.item(0), typeof c.getPropertyPriority].join()"), "125,align-content,function");
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
        ev(&b, "var c = getComputedStyle(document.getElementById('gone')); [c.display, c.visibility, c.borderRadius].join()"),
        "none,visible,0px"
    );
}

#[test]
fn an_unset_border_radius_is_zero_px_and_a_non_finite_box_stays_finite() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 40.0));
    m.insert(id(&doc, "inner"), geom(10.0, 20.0, 40.0, 20.0));
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var c = getComputedStyle(document.getElementById('inner')); [c.borderRadius, c.borderTopLeftRadius, c.borderTopRightRadius, c.borderBottomRightRadius, c.borderBottomLeftRadius, Number.parseInt(c.borderRadius)].join()"),
        "0px,0px,0px,0px,0px,0"
    );
    assert_eq!(
        ev(&b, "var r = document.getElementById('outer').getBoundingClientRect(); [r.x, r.y, r.width, r.height, r.top, r.left, r.right, r.bottom].every(Number.isFinite)"),
        "true"
    );
    assert_eq!(
        ev(&b, "var r = document.getElementById('outer').getBoundingClientRect(); [r.x, r.y, r.width, r.height].join()"),
        "0,0,0,40"
    );
    // The radius is a finite offset, so the point query does not throw.
    assert_eq!(
        ev(&b, "var e = document.elementFromPoint(20, 30); var half = Number.parseInt(getComputedStyle(e).borderRadius) / 2; document.elementFromPoint(20, 30 + half).id"),
        "inner"
    );
    assert_eq!(
        ev(&b, "var name = ''; try { document.elementFromPoint(NaN, 0); } catch (e) { name = e.name; } name"),
        "TypeError"
    );
}

#[test]
fn getcomputedstyle_rejects_a_non_element() {
    let (b, _) = bound();
    assert_eq!(ev(&b, "try { getComputedStyle(null); 'no throw' } catch (e) { e.name }"), "TypeError");
    assert_eq!(ev(&b, "try { getComputedStyle(document); 'no throw' } catch (e) { e.name }"), "TypeError");
}

/// #854: a fixed (`held`) box's client rect ignores the window scroll —
/// the publisher puts the scroll back so the JS reader subtracting it lands
/// where layout put the box. Portable beside headless transform_rect_tests.
#[test]
fn a_held_box_keeps_its_client_rect_under_a_window_scroll() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    let mut fixed = geom(300.0, 10.0, 100.0, 40.0);
    fixed.held = true;
    fixed.position = 2;
    m.insert(id(&doc, "fixed"), fixed);
    m.insert(id(&doc, "outer"), geom(8.0, 1000.0, 300.0, 120.0));
    b.set_geometry(m);
    b.set_scroll_state((0.0, 490.0), (0.0, 2200.0));
    b.evaluate("window.scrollY = 490; window.pageYOffset = 490;").unwrap();
    assert_eq!(
        ev(&b, "var r = document.getElementById('fixed').getBoundingClientRect(); [r.x, r.y, r.width, r.height].join()"),
        "300,10,100,40"
    );
    // A normal box still moves with the page.
    assert_eq!(
        ev(&b, "document.getElementById('outer').getBoundingClientRect().top"),
        "510"
    );
}

/// #852: a box's own transform is applied when reading the client rect;
/// offset* stay at the layout origin.
#[test]
fn a_transformed_box_reads_where_it_paints() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    // translate(20, 30) about the origin of a 100x50 box at (10, 40).
    let mut box_ = geom(10.0, 40.0, 100.0, 50.0);
    box_.transform = Some([1.0, 0.0, 0.0, 1.0, 20.0, 30.0]);
    m.insert(id(&doc, "outer"), box_);
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var e = document.getElementById('outer'); var r = e.getBoundingClientRect(); \
                [r.x, r.y, r.width, r.height, e.offsetLeft, e.offsetTop].join()"),
        "30,70,100,50,10,40"
    );
    // scale(2) grows the painted rect; layout size (offset*) stays.
    let mut m = HashMap::new();
    let mut scaled = geom(0.0, 0.0, 100.0, 50.0);
    scaled.transform = Some([2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
    m.insert(id(&doc, "outer"), scaled);
    b.set_geometry(m);
    assert_eq!(
        ev(&b, "var e = document.getElementById('outer'); var r = e.getBoundingClientRect(); \
                [r.width, r.height, e.offsetWidth, e.offsetHeight].join()"),
        "200,100,100,50"
    );
}

/// #850 follow-up ec0b831d: scrollChain lists only boxes that scroll. A
/// transformed ancestor rides the layout chain for rects but is skipped here
/// so setOwn never sees it. Pure pin of `scrolling_chain_ids` (the JS
/// `__rustkit_element_scroll` table is deleted after web_scroll.js boots).
#[test]
fn scroll_chain_lists_only_boxes_that_scroll() {
    let mut m = HashMap::new();
    m.insert(
        1,
        BoxGeometry {
            scrolls: true,
            scroller: None,
            ..Default::default()
        },
    );
    m.insert(
        2,
        BoxGeometry {
            scrolls: false,
            transform: Some([1.0, 0.0, 0.0, 1.0, 0.0, 50.0]),
            scroller: Some(1),
            ..Default::default()
        },
    );
    m.insert(
        3,
        BoxGeometry {
            scroller: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(crate::dom::scrolling_chain_ids(&m, Some(2)), vec![1]);
    assert_eq!(crate::dom::scrolling_chain_ids(&m, Some(1)), vec![1]);
    assert!(crate::dom::scrolling_chain_ids(&m, None).is_empty());
    // Two real scrollers with a transform between them keep both scrollers.
    m.insert(
        4,
        BoxGeometry {
            scrolls: true,
            scroller: Some(2),
            ..Default::default()
        },
    );
    assert_eq!(crate::dom::scrolling_chain_ids(&m, Some(4)), vec![4, 1]);
}

/// #840 / 8f3534fe: custom property names keep their case on a computed
/// read (Primer `--fgColor-*`). Portable beside headless computed_display_tests.
#[test]
fn a_custom_property_name_keeps_its_case() {
    let (b, doc) = bound();
    let mut m = HashMap::new();
    m.insert(id(&doc, "outer"), geom(0.0, 0.0, 10.0, 10.0));
    b.set_geometry(m);
    let mut s = HashMap::new();
    // Engine shape: standard rows, then `\ncustom\t` marker, then each var.
    s.insert(
        id(&doc, "outer"),
        format!(
            "{}\ncustom\t\ncustom\t--fgColor-default\tred\ncustom\t--x\t7px",
            styles(&[("display", "block")])
        ),
    );
    b.set_computed_styles(s);
    assert_eq!(
        ev(
            &b,
            "var c = getComputedStyle(document.getElementById('outer')); \
             [c.getPropertyValue('--fgColor-default'), c.getPropertyValue('--fgcolor-default'), \
              c.getPropertyValue('--x'), c.getPropertyValue('--missing')].join()"
        ),
        "red,,7px,"
    );
}
