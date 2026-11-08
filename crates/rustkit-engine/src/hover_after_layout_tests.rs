//! CH2.E15: `:hover` follows a layout or a scroll that moves boxes under a
//! pointer that does not move.
//!
//! The expected logs are Chromium 143.0.7499.4's, from
//! `tools/parity_oracle/hover_after_layout_log.mjs` on
//! `tools/parity_oracle/hover_after_layout_page.html` (the page below is
//! that page with the tab a `div`). Chromium does not update `:hover` in the
//! task that moved the boxes (a read right after a forced layout still sees
//! the old chain); on the next frame, before its animation callbacks, it
//! hit-tests the last pointer position again and fires the boundary events
//! a move would (`pointerout`, `pointerleave`, `pointerover`,
//! `pointerenter`, then the four mouse ones), with no `pointermove` or
//! `mousemove`. A layout that leaves the same element under the pointer
//! fires nothing. A script scroll and a wheel scroll do the same.
//!
//! The frame here is the window's: `render_view` / `render_changed_views`,
//! or a live turn (`pump_live`).

use super::*;
use crate::hover_restyle_tests::skipped;

const PAGE: &str = r##"<html><head><style>
  body { margin: 0; }
  #side { width: 100px; height: 40px; background: #ccc; }
  #row { display: flex; }
  #side:hover { background: #00f; }
  #row.collapsed #side { width: 0; }
  #tab { display: block; width: 80px; height: 40px; background: #eee; }
  #tab:hover { background: #f00; }
  #scroller { height: 100px; overflow: auto; margin-top: 20px; }
  #scroller div { height: 40px; }
  #scroller div:hover { background: #0f0; }
  #tall { height: 2000px; }
</style></head><body>
<div id="row"><div id="side"></div><div id="tab">tab</div></div>
<div id="scroller"><div id="s0">0</div><div id="s1">1</div><div id="s2">2</div><div id="s3">3</div></div>
<div id="tall"></div>
</body></html>"##;

/// Over `#side`, 50 px left of where `#tab` starts until the collapse.
const SIDE: (f32, f32) = (50.0, 20.0);
/// Over the scroller's first row while it is not scrolled.
const ROW0: (f32, f32) = (10.0, 70.0);

const LISTEN: &str = "window.log = []; \
    ['pointerover', 'pointerenter', 'pointerout', 'pointerleave', 'pointermove', \
     'mouseover', 'mouseenter', 'mouseout', 'mouseleave', 'mousemove'].forEach(function (t) { \
      document.addEventListener(t, function (e) { \
        if (e.target.nodeType !== 1) return; \
        var name = function (n) { return (n && (n.id || n.nodeName)) || 'null'; }; \
        log.push(t + ' ' + name(e.target) + ' rel=' + name(e.relatedTarget)); \
      }, true); }); ''";

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 400, 300))
        .expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    js(&mut engine, id, LISTEN);
    (engine, id)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, script).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&out)
        .to_string()
}

/// The log since the last call, one event a line.
fn take_log(engine: &mut Engine, id: EngineViewId) -> Vec<String> {
    let joined = js(engine, id, "String(window.log.splice(0).join('|'))");
    joined.split('|').filter(|l| !l.is_empty()).map(str::to_string).collect()
}

/// The elements painted in their `:hover` background (script cannot ask
/// `:hover` itself: `matches(':hover')` is not supported, disclosed).
fn hovered(engine: &mut Engine, id: EngineViewId) -> String {
    js(
        engine,
        id,
        "String(['side', 'tab', 's0', 's1', 's2', 's3'].filter(function (i) { \
           var c = getComputedStyle(document.getElementById(i)).backgroundColor; \
           return c === 'rgb(0, 0, 255)' || c === 'rgb(255, 0, 0)' || c === 'rgb(0, 255, 0)'; }).join())",
    )
}

/// The window's next frame.
fn frame(engine: &mut Engine, id: EngineViewId) {
    engine.render_view(id).expect("render");
}

fn live_turn(engine: &mut Engine, id: EngineViewId) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(engine.pump_live(id, 16));
}

fn relayouts(engine: &Engine, id: EngineViewId) -> u32 {
    engine.relayout_stats(id).expect("view").relayouts
}

fn lines(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|s| s.to_string()).collect()
}

/// The pointer over `#side`, the log empty.
fn pointer_over_side() -> (Engine, EngineViewId) {
    let (mut engine, id) = page();
    engine.mouse_move_at_point(id, SIDE.0, SIDE.1);
    frame(&mut engine, id);
    take_log(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "side");
    (engine, id)
}

const SIDE_TO_TAB: &[&str] = &[
    "pointerout side rel=tab",
    "pointerleave side rel=tab",
    "pointerover tab rel=side",
    "pointerenter tab rel=side",
    "mouseout side rel=tab",
    "mouseleave side rel=tab",
    "mouseover tab rel=side",
    "mouseenter tab rel=side",
];

const TAB_TO_SIDE: &[&str] = &[
    "pointerout tab rel=side",
    "pointerleave tab rel=side",
    "pointerover side rel=tab",
    "pointerenter side rel=tab",
    "mouseout tab rel=side",
    "mouseleave tab rel=side",
    "mouseover side rel=tab",
    "mouseenter side rel=tab",
];

#[test]
fn a_box_layout_moves_under_a_still_pointer_is_hovered_on_the_next_frame() {
    let (mut engine, id) = pointer_over_side();
    // The task that moved it still sees the old chain, after a forced
    // layout too (Chromium).
    let same_task = js(
        &mut engine,
        id,
        "document.getElementById('row').classList.add('collapsed'); document.body.offsetWidth; \
         String(getComputedStyle(document.getElementById('tab')).backgroundColor)",
    );
    assert_eq!(same_task, "rgb(238, 238, 238)");
    assert_eq!(take_log(&mut engine, id), Vec::<String>::new(), "nothing before the frame");

    frame(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "tab");
    assert_eq!(take_log(&mut engine, id), lines(SIDE_TO_TAB), "Chromium's boundary events, no move");
}

#[test]
fn a_box_layout_moves_away_from_a_still_pointer_loses_hover_on_the_next_frame() {
    let (mut engine, id) = pointer_over_side();
    js(&mut engine, id, "document.getElementById('row').classList.add('collapsed'); ''");
    frame(&mut engine, id);
    take_log(&mut engine, id);

    js(&mut engine, id, "document.getElementById('row').classList.remove('collapsed'); ''");
    frame(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "side");
    assert_eq!(take_log(&mut engine, id), lines(TAB_TO_SIDE));
}

#[test]
fn a_live_turn_is_a_frame_for_the_hover_update() {
    let (mut engine, id) = pointer_over_side();
    js(&mut engine, id, "document.getElementById('row').classList.add('collapsed'); ''");
    live_turn(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "tab");
    assert_eq!(take_log(&mut engine, id), lines(SIDE_TO_TAB));
}

#[test]
fn a_layout_that_keeps_the_box_under_the_pointer_fires_nothing_and_restyles_nothing() {
    let (mut engine, id) = pointer_over_side();
    js(&mut engine, id, "document.getElementById('side').style.width = '90px'; ''");
    let (laid, short) = (relayouts(&engine, id), skipped(&engine, id));
    frame(&mut engine, id);
    assert_eq!(take_log(&mut engine, id), Vec::<String>::new());
    assert_eq!(hovered(&mut engine, id), "side");
    assert_eq!((relayouts(&engine, id), skipped(&engine, id)), (laid, short), "no restyle");
}

#[test]
fn the_hover_update_is_a_hover_restyle_not_a_relayout() {
    let (mut engine, id) = pointer_over_side();
    js(&mut engine, id, "document.getElementById('row').classList.add('collapsed'); ''");
    let (laid, short) = (relayouts(&engine, id), skipped(&engine, id));
    frame(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "tab");
    // `#tab:hover` changes only its background: one pointer restyle, on
    // the short path (#895), and no layout.
    assert_eq!(relayouts(&engine, id), laid + 1, "one pointer restyle");
    assert_eq!(skipped(&engine, id), short + 1, "and it skipped layout");
    // A second frame with nothing new does nothing.
    frame(&mut engine, id);
    assert_eq!((relayouts(&engine, id), skipped(&engine, id)), (laid + 1, short + 1));
    assert_eq!(take_log(&mut engine, id).len(), 8);
}

#[test]
fn a_script_scroll_under_a_still_pointer_moves_hover_on_the_next_frame() {
    let (mut engine, id) = page();
    engine.mouse_move_at_point(id, ROW0.0, ROW0.1);
    frame(&mut engine, id);
    take_log(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "s0");

    js(&mut engine, id, "document.getElementById('scroller').scrollTop = 40; ''");
    assert_eq!(hovered(&mut engine, id), "s0", "not in the scrolling task");
    frame(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "s1");
    assert_eq!(
        take_log(&mut engine, id),
        lines(&[
            "pointerout s0 rel=s1",
            "pointerleave s0 rel=s1",
            "pointerover s1 rel=s0",
            "pointerenter s1 rel=s0",
            "mouseout s0 rel=s1",
            "mouseleave s0 rel=s1",
            "mouseover s1 rel=s0",
            "mouseenter s1 rel=s0",
        ])
    );
}

#[test]
fn a_view_scroll_under_a_still_pointer_moves_hover_on_the_next_frame() {
    let (mut engine, id) = pointer_over_side();
    // The page scrolls 40 px: the scroller's first row (y 60 in the page)
    // is now at y 20, under the pointer at (50, 20).
    engine.scroll_view(id, 0.0, -40.0).expect("scroll"); // wheel delta: negative scrolls down
    frame(&mut engine, id);
    assert_eq!(hovered(&mut engine, id), "s0");
    assert_eq!(
        take_log(&mut engine, id),
        lines(&[
            "pointerout side rel=s0",
            "pointerleave side rel=s0",
            "pointerleave row rel=s0",
            "pointerover s0 rel=side",
            "pointerenter scroller rel=side",
            "pointerenter s0 rel=side",
            "mouseout side rel=s0",
            "mouseleave side rel=s0",
            "mouseleave row rel=s0",
            "mouseover s0 rel=side",
            "mouseenter scroller rel=side",
            "mouseenter s0 rel=side",
        ])
    );
}

#[test]
fn a_pointer_that_left_the_view_is_not_hit_tested_again() {
    let (mut engine, id) = pointer_over_side();
    engine.mouse_leave(id);
    take_log(&mut engine, id);
    js(&mut engine, id, "document.getElementById('row').classList.add('collapsed'); ''");
    frame(&mut engine, id);
    assert_eq!(take_log(&mut engine, id), Vec::<String>::new());
    assert_eq!(hovered(&mut engine, id), "");
}
