//! TREE-REUSE step (2): an inline style write that changes only paint properties (colours,
//! `opacity`) moves no box, so a geometry read after it (`offsetWidth`, `getBoundingClientRect`,
//! `clientWidth`, hit testing) answers from the published layout instead of building the whole
//! document (a forced layout). A read that sees the written value (`getComputedStyle`) still
//! builds, and the engine still lays the write out when script settles. Anything that could move a
//! box keeps the build: a layout property beside the paint one, a custom property, or a page rule
//! that matches on the `style` attribute.

use super::*;

fn engine_with(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    engine.load_html(view, page).expect("load_html");
    (engine, view)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn forced(engine: &Engine, view: EngineViewId) -> u64 {
    engine.relayout_stats(view).unwrap().forced_layouts
}

fn relayouts(engine: &Engine, view: EngineViewId) -> u32 {
    engine.relayout_stats(view).unwrap().relayouts
}

const PAGE: &str = "<html><head><style>body{margin:0}</style></head><body>\
    <div id=a style='width:100px;height:20px'>text</div>\
    <div id=b style='width:50px;height:10px;--w:50px'>b</div></body></html>";

#[test]
fn a_paint_only_style_write_then_a_geometry_read_builds_nothing() {
    let (mut engine, view) = engine_with(PAGE);
    let before = relayouts(&engine, view);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var w = [];\
             a.style.color = 'red'; w.push(a.offsetWidth);\
             a.style.backgroundColor = 'rgb(0, 0, 255)'; w.push(a.getBoundingClientRect().width);\
             a.style.borderColor = 'green'; a.style.outlineColor = 'blue'; w.push(a.offsetTop);\
             w.push(document.elementFromPoint(5, 5) === a);\
             a.style.opacity = '0.5'; w.push(a.clientHeight);\
             window.__w = w.join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,100,0,true,20");
    assert_eq!(forced(&engine, view), 0, "no box moved: every read answers from the published layout");
    assert!(relayouts(&engine, view) > before, "the engine still lays the writes out when script settles");
    assert_eq!(
        read(&mut engine, view, "getComputedStyle(document.getElementById('a')).color"),
        "rgb(255, 0, 0)"
    );
}

#[test]
fn a_computed_style_read_after_a_paint_write_still_builds() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a');\
             a.style.color = 'red'; window.__w = a.offsetWidth + ',' + getComputedStyle(a).color;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,rgb(255, 0, 0)");
    assert_eq!(forced(&engine, view), 1, "the computed style sees the written value");
}

#[test]
fn a_layout_write_beside_a_paint_write_still_builds() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var w = [];\
             a.style.color = 'red'; w.push(a.offsetWidth);\
             a.style.width = '60px'; w.push(a.offsetWidth);\
             window.__w = w.join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,60");
    assert_eq!(forced(&engine, view), 1, "the width write moves the box");
}

#[test]
fn a_custom_property_write_still_builds() {
    let page = "<html><head><style>body{margin:0} #b{width:var(--w)}</style></head><body>\
        <div id=b style='height:10px;--w:50px'>b</div></body></html>";
    let (mut engine, view) = engine_with(page);
    engine
        .execute_script(
            view,
            "var b = document.getElementById('b');\
             b.style.setProperty('--w', '70px'); window.__w = b.offsetWidth;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "70");
    assert_eq!(forced(&engine, view), 1, "a custom property can reach any property");
}

#[test]
fn a_rule_on_the_style_attribute_keeps_the_build() {
    let page = "<html><head><style>body{margin:0} [style*=red]{width:30px !important}</style></head><body>\
        <div id=a style='width:100px;height:20px'>text</div></body></html>";
    let (mut engine, view) = engine_with(page);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a');\
             a.style.color = 'red'; window.__w = a.offsetWidth;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "30");
    assert_eq!(forced(&engine, view), 1, "the page matches on the style attribute");
}

#[test]
fn removing_a_style_with_layout_properties_still_builds() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a');\
             a.removeAttribute('style'); window.__w = a.offsetWidth;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "400");
    assert_eq!(forced(&engine, view), 1);
}

#[test]
fn a_paint_write_then_a_layout_write_then_a_read_builds_once() {
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var w = [];\
             a.style.color = 'red'; w.push(a.offsetWidth);\
             a.textContent = 'a much longer text that wraps onto more lines than one'; w.push(a.scrollHeight > 20);\
             a.style.color = 'blue'; w.push(a.offsetWidth);\
             window.__w = w.join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,true,100");
    assert_eq!(forced(&engine, view), 1, "only the text write moved a box");
}

#[test]
fn a_hit_test_after_an_opacity_write_still_builds() {
    // Below 1, opacity makes a stacking context, which can change what paints on top.
    let (mut engine, view) = engine_with(PAGE);
    engine
        .execute_script(
            view,
            "var a = document.getElementById('a'); var w = [];\
             a.style.opacity = '0.5'; w.push(a.offsetWidth);\
             w.push(document.elementFromPoint(5, 5) === a);\
             window.__w = w.join();",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "100,true");
    assert_eq!(forced(&engine, view), 1, "the offset read is a lookup, the hit test builds");
}

#[test]
fn a_rule_under_has_keeps_the_build() {
    let page = "<html><head><style>body{margin:0} body:has([style*=red]) #a{width:30px}</style></head><body>\
        <div id=a style='height:20px'>text</div><p id=p>p</p></body></html>";
    let (mut engine, view) = engine_with(page);
    engine
        .execute_script(
            view,
            "document.getElementById('p').style.color = 'red';\
             window.__w = document.getElementById('a').offsetWidth;",
        )
        .unwrap();
    assert_eq!(read(&mut engine, view, "window.__w"), "30");
    assert_eq!(forced(&engine, view), 1, "a :has() rule can match on what the write changed");
}
