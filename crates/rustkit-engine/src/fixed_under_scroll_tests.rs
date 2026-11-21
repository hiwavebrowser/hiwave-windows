//! A `position: fixed` box stays where it is in the window while the page
//! scrolls under it. The page's scroll is one translate around the whole
//! list, so until this file a fixed header left the screen with the first
//! screen of the page. Seen in the real window on 2026-10-12
//! (`fixed_scroll`: the bar's area after the scroll was 0).
//!
//! The window draws the scroll as a renderer offset (threads plan P3); every
//! scrolled frame here is drawn that way and must match, byte for byte, the
//! copied list the window drew before (`scrolled_commands`).

use super::*;
use crate::script_net_tests::serve_routes;
use rustkit_layout::DisplayCommand;

/// The engine's list for `body`, in a 200x100 view.
fn page_list(body: &str) -> (Engine, EngineViewId, Vec<DisplayCommand>) {
    let page = format!(
        "<html><head><style>body{{margin:0}} div{{background:#000}} #tall{{height:1000px;background:none}}</style></head>\
         <body>{body}<div id=tall></div></body></html>"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let list = engine.views[&view].display_list.as_ref().expect("display list").commands.clone();
    (engine, view, list)
}

fn capture(engine: &mut Engine, view: EngineViewId, commands: &[DisplayCommand], tag: &str) -> Vec<u8> {
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-fixed-{tag}-{}.ppm", std::process::id()));
    engine
        .presenter
        .as_mut()
        .expect("window presenter")
        .capture_commands(viewhost_id, (200, 100), path.to_str().unwrap(), commands)
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

/// The view's list drawn by the window's path at `offset`, checked against
/// the list the window drew before P3 (`scrolled_commands`).
fn capture_at(engine: &mut Engine, view: EngineViewId, offset: (f32, f32), tag: &str) -> Vec<u8> {
    let list = engine.views[&view].display_list.as_ref().expect("display list").commands.clone();
    let before = capture(engine, view, &scrolled_commands(&list, offset), &format!("{tag}-copied"));
    let viewhost_id = engine.views[&view].viewhost_id;
    let path = std::env::temp_dir().join(format!("rustkit-fixed-{tag}-offset-{}.ppm", std::process::id()));
    let display_list = engine.views[&view].display_list.as_ref().expect("display list");
    engine
        .presenter
        .as_mut()
        .expect("window presenter")
        .draw_scrolled(viewhost_id, display_list, offset, Some(path.to_str().unwrap()))
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    assert!(ppm == before, "{tag}: the offset draws what the copied list drew");
    ppm
}

/// Whether the pixel is dark, in a `P6` frame 200 wide and 100 tall.
fn dark(ppm: &[u8], x: usize, y: usize) -> bool {
    let pixels = 200 * 100 * 3;
    assert!(ppm.len() > pixels, "a 200x100 frame");
    ppm[ppm.len() - pixels + (y * 200 + x) * 3] < 128
}

/// A fixed bar at the top left and an absolutely positioned block 60 down,
/// in a page scrolled by 50: the block moves up, the bar does not.
#[test]
fn a_fixed_box_stays_put_while_the_page_scrolls() {
    let (mut engine, view, list) = page_list(
        "<div style='position:fixed;left:0;top:0;width:40px;height:20px'></div>\
         <div style='position:absolute;left:100px;top:60px;width:40px;height:40px'></div>",
    );
    let at_top = capture(&mut engine, view, &list, "top");
    assert!(dark(&at_top, 20, 10) && dark(&at_top, 120, 80), "the bar at the top, the block 60 down");

    let scrolled = capture_at(&mut engine, view, (0.0, 50.0), "scrolled");
    assert!(dark(&scrolled, 120, 30), "the block is drawn 50 up");
    assert!(!dark(&scrolled, 120, 80), "and not where it was");
    assert!(dark(&scrolled, 20, 10), "the fixed bar is still at the top of the window");
}

/// What is inside a fixed box stays with it, a fixed box inside it too,
/// and so does a fixed box that is deep in the scrolling content.
#[test]
fn a_fixed_box_keeps_its_contents_and_can_sit_deep_in_the_page() {
    let (mut engine, view, _) = page_list(
        "<div style='position:fixed;left:0;top:0;width:100px;height:10px'>\
           <div style='position:absolute;left:0;top:20px;width:20px;height:20px'></div>\
           <div style='position:fixed;left:40px;top:20px;width:20px;height:20px'></div>\
         </div>\
         <div style='height:300px;background:none'></div>\
         <section><div style='position:fixed;left:160px;top:60px;width:20px;height:20px'></div></section>",
    );
    let scrolled = capture_at(&mut engine, view, (0.0, 50.0), "nested");
    assert!(dark(&scrolled, 50, 5), "the bar");
    assert!(dark(&scrolled, 10, 30), "its absolutely positioned child");
    assert!(dark(&scrolled, 50, 30), "the fixed box inside it is not moved twice");
    assert!(dark(&scrolled, 170, 70), "a fixed box 300 down the document");
}

/// Across as well as down.
#[test]
fn a_fixed_box_stays_put_when_the_page_scrolls_sideways() {
    let (mut engine, view, _) =
        page_list("<div style='position:fixed;left:100px;top:40px;width:40px;height:20px'></div>");
    let scrolled = capture_at(&mut engine, view, (30.0, 20.0), "sideways");
    assert!(dark(&scrolled, 120, 50), "where it was");
    assert!(!dark(&scrolled, 85, 25), "not 30 left and 20 up");
}

/// With no scroll the window draws the page's list as it is.
#[test]
fn an_unscrolled_page_draws_its_fixed_box_where_layout_put_it() {
    let (mut engine, view, list) =
        page_list("<div style='position:fixed;left:100px;top:40px;width:40px;height:20px'></div>");
    let frame = capture(&mut engine, view, &list, "plain");
    assert!(dark(&frame, 120, 50) && !dark(&frame, 120, 20) && !dark(&frame, 50, 50));
}

fn marks(list: &[DisplayCommand]) -> (usize, usize) {
    (
        list.iter().filter(|c| matches!(c, DisplayCommand::PushViewportFixed)).count(),
        list.iter().filter(|c| matches!(c, DisplayCommand::PopViewportFixed)).count(),
    )
}

/// One run for each outermost fixed box, none for a box that scrolls.
#[test]
fn a_run_is_marked_for_each_outermost_fixed_box_only() {
    let (_engine, _view, list) = page_list(
        "<div style='position:fixed;left:0;top:0;width:100px;height:10px'>\
           <div style='position:fixed;left:40px;top:20px;width:20px;height:20px'></div></div>\
         <div style='position:absolute;left:100px;top:60px;width:40px;height:40px'></div>\
         <section><div style='position:fixed;left:160px;top:60px;width:20px;height:20px'></div></section>",
    );
    assert_eq!(marks(&list), (2, 2), "{list:?}");
    let (_engine, _view, list) =
        page_list("<div style='position:absolute;left:100px;top:60px;width:40px;height:40px'></div>");
    assert_eq!(marks(&list), (0, 0), "{list:?}");
}

/// A transformed box is the containing block of the fixed boxes inside it
/// (css-transforms-1): they move with it, so with the page.
#[test]
fn a_fixed_box_under_a_transformed_ancestor_moves_with_the_page() {
    let (mut engine, view, list) = page_list(
        "<section style='transform:translate(10px,0)'>\
           <div style='position:fixed;left:20px;top:60px;width:20px;height:20px'></div></section>",
    );
    assert_eq!(marks(&list), (0, 0), "{list:?}");
    let at_top = capture(&mut engine, view, &list, "transformed-top");
    assert!(dark(&at_top, 40, 70), "10 right of its left, 60 down");
    let scrolled = capture_at(&mut engine, view, (0.0, 50.0), "transformed");
    assert!(dark(&scrolled, 40, 20) && !dark(&scrolled, 40, 70), "drawn 50 up");
}

/// A fixed box escapes the clip of a box that scrolls. The clip moves with
/// the page; the fixed box inside it does not, and what follows the fixed
/// box in the clipping box is still clipped.
#[test]
fn a_fixed_box_inside_a_clipping_box_stays_put_and_the_clip_scrolls() {
    let (mut engine, view, _) = page_list(
        "<div style='height:60px;background:none'></div>\
         <section style='overflow:hidden;height:20px'>\
           <div style='position:fixed;left:0;top:0;width:40px;height:20px'></div>\
           <div style='margin-left:100px;width:40px;height:60px'></div></section>",
    );
    let scrolled = capture_at(&mut engine, view, (0.0, 50.0), "clip");
    assert!(dark(&scrolled, 20, 10), "the fixed box, outside the clip and at the top of the window");
    assert!(dark(&scrolled, 120, 20), "the clipped block, 50 up: 10 to 30");
    assert!(!dark(&scrolled, 120, 40), "and still cut at the clipping box's edge");
}

/// The reviewer's page on #681: a fixed link across the top of the window
/// and, 40 down the page, a link that scrolls under it.
const BAR_OVER_A_LINK: &str = "<a href='/bar' style='display:block;position:fixed;left:0;top:0;width:200px;height:20px'>bar</a>\
     <div style='height:40px;background:none'></div>\
     <a href='/under' style='display:block;height:100px'>under</a>";

fn path(link: Option<String>) -> Option<String> {
    link.map(|link| Url::parse(&link).expect("url").path().to_string())
}

/// The bar is drawn at the top of the window after a scroll, so a click
/// there is a click on the bar and not on the link drawn under it.
#[test]
fn a_click_on_a_fixed_bar_after_a_scroll_reaches_the_bar() {
    let (mut engine, view, _) = page_list(BAR_OVER_A_LINK);
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/bar"));
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/bar"), "on the bar");
    assert_eq!(path(engine.link_at_point(view, 20.0, 30.0)).as_deref(), Some("/under"), "below the bar");
    let outcome = engine.click_at_point(view, 20.0, 10.0);
    assert_eq!(path(outcome.navigate).as_deref(), Some("/bar"), "the click follows the bar");
}

/// A bar fixed 60 down the window would have scrolled to 10 down. A click
/// there reaches the page, and a click where the bar is drawn reaches it.
#[test]
fn a_fixed_bar_is_not_clicked_where_it_would_have_scrolled_to() {
    let (mut engine, view, _) = page_list(
        "<a href='/bar' style='display:block;position:fixed;left:0;top:60px;width:200px;height:20px'>bar</a>\
         <a href='/under' style='display:block;height:300px'>under</a>",
    );
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 20.0)).as_deref(), Some("/under"), "its place in the page");
    assert_eq!(path(engine.link_at_point(view, 20.0, 70.0)).as_deref(), Some("/bar"), "where it is drawn");
}

/// A fixed box is not inside its parent's box: a header fixed to the top
/// from deep in the page, or from a parent with no height, is clicked
/// where it is drawn, scrolled or not.
#[test]
fn a_fixed_bar_is_clicked_from_outside_its_parents_box() {
    let (mut engine, view, _) = page_list(
        "<div style='height:300px;background:none'></div>\
         <div style='height:0'><a href='/bar' style='display:block;position:fixed;left:0;top:0;width:200px;height:20px'>bar</a></div>",
    );
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/bar"), "not scrolled");
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/bar"), "scrolled");
}

/// The press, the release, the click and the pointer moves all go to the
/// fixed button under the pointer, with offsets from the button's own
/// corner, and a fixed field takes the focus.
#[test]
fn pointer_events_and_focus_reach_a_fixed_box_after_a_scroll() {
    let (mut engine, view, _) = page_list(
        "<div style='position:fixed;left:0;top:0;width:100px;height:20px;background:none'>\
         <button id=b style='display:block;width:100px;height:20px;border:0;padding:0;margin:0'></button></div>\
         <div style='position:fixed;left:100px;top:0;width:80px;height:20px;background:none'>\
         <input id=i style='display:block;width:80px;height:20px;border:0;padding:0;margin:0'></div>\
         <p id=p style='margin:0;height:300px'></p>\
         <script>var log = [];\
         ['pointerdown','mousedown','pointerup','mouseup','click','mousemove'].forEach(function (type) {\
           document.addEventListener(type, function (e) { log.push(type + ':' + e.target.id + ':' + e.offsetY); });\
         });</script>",
    );
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    engine.mouse_move_at_point(view, 20.0, 10.0);
    engine.mouse_down_at_point(view, 20.0, 10.0);
    engine.click_at_point(view, 20.0, 10.0);
    assert_eq!(
        engine.execute_script(view, "log.join(' ')").expect("script"),
        "String(\"mousemove:b:10 pointerdown:b:10 mousedown:b:10 pointerup:b:10 mouseup:b:10 click:b:10\")"
    );
    assert_eq!(engine.focus_at_point(view, 120.0, 8.0).as_deref(), Some("input"), "the fixed field");
}

/// A fixed box inside a transformed box moves with the page (it is drawn
/// that way above), so it is clicked where it scrolled to.
#[test]
fn a_fixed_box_under_a_transformed_ancestor_is_clicked_where_it_scrolled_to() {
    let (mut engine, view, _) = page_list(
        "<section style='transform:translate(0,0);height:300px'>\
         <a href='/bar' style='display:block;position:fixed;left:0;top:60px;width:200px;height:20px'>bar</a>\
         </section>",
    );
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 20.0)).as_deref(), Some("/bar"), "60 down the page");
    assert_eq!(path(engine.link_at_point(view, 20.0, 70.0)), None, "nothing where it was");
}

/// Of two fixed boxes at the point the higher `z-index` is clicked,
/// whichever comes first in the page, and a fixed box with a negative
/// `z-index` is under the page's own boxes.
#[test]
fn fixed_boxes_are_clicked_in_z_order() {
    let fixed = |href: &str, z: i32| {
        format!("<a href='{href}' style='display:block;position:fixed;left:0;top:0;width:200px;height:20px;z-index:{z}'>x</a>")
    };
    let (mut engine, view, _) = page_list(&format!(
        "{}{}<a href='/under' style='display:block;height:300px'>under</a>",
        fixed("/dialog", 2),
        fixed("/bar", 1)
    ));
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/dialog"));

    let (mut engine, view, _) = page_list(&format!(
        "{}<a href='/under' style='display:block;height:300px'>under</a>",
        fixed("/behind", -1)
    ));
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/under"));
}

/// A fixed box that is not shown (a closed menu, a dialog's backdrop)
/// takes no click from the page under it.
#[test]
fn a_hidden_fixed_box_takes_no_click() {
    let (mut engine, view, _) = page_list(
        "<div style='position:fixed;left:0;top:0;width:200px;height:100px;visibility:hidden;background:none'></div>\
         <a href='/under' style='display:block;height:300px'>under</a>",
    );
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/under"), "not scrolled");
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/under"), "scrolled");
}

// `pointer-events` and boxes that are not shown (the second hold on #681).
// Chromium's answers: `tools/parity_oracle/fixed_scroll_click_log.mjs`.

const UNDER: &str = "<a href='/under' style='display:block;height:600px'>under</a>";

/// A fixed box over the whole window with `pointer-events: none` (a
/// decoration, a notification layer) takes no click at any scroll
/// position: the page under it does.
#[test]
fn a_see_through_fixed_overlay_takes_no_click() {
    let (mut engine, view, _) = page_list(&format!(
        "<div style='position:fixed;left:0;top:0;width:200px;height:100px;pointer-events:none;background:none'></div>{UNDER}"
    ));
    assert_eq!(path(engine.link_at_point(view, 20.0, 60.0)).as_deref(), Some("/under"), "not scrolled");
    engine.set_scroll_offset(view, 0.0, 200.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 60.0)).as_deref(), Some("/under"), "scrolled");
}

/// A toast container (react-hot-toast's shape) at the bottom of the
/// window: only a toast that says `pointer-events: auto` is clicked.
#[test]
fn a_toast_container_is_clicked_only_on_a_toast_that_asks() {
    let (mut engine, view, _) = page_list(&format!(
        "<div style='position:fixed;left:0;right:0;bottom:0;height:60px;pointer-events:none;background:none'>\
         <a href='/quiet' style='display:block;height:10px'>q</a>\
         <a href='/toast' style='display:block;height:10px;pointer-events:auto'>t</a></div>{UNDER}"
    ));
    engine.set_scroll_offset(view, 0.0, 200.0).unwrap();
    assert_eq!(path(engine.link_at_point(view, 20.0, 45.0)).as_deref(), Some("/under"), "inherited none");
    assert_eq!(path(engine.link_at_point(view, 20.0, 55.0)).as_deref(), Some("/toast"), "auto again");
    assert_eq!(path(engine.link_at_point(view, 20.0, 70.0)).as_deref(), Some("/under"), "the container");
}

/// The same rule for a box that scrolls with the page, and for a link
/// that is itself `pointer-events: none` (its text is no target either).
#[test]
fn pointer_events_none_is_honoured_in_the_page_too() {
    let (engine, view, _) = page_list(&format!(
        "<div style='position:absolute;left:0;top:0;width:200px;height:60px;pointer-events:none;background:none'>\
         <a href='/off' style='display:block;height:20px'>off</a>\
         <a href='/on' style='display:block;height:20px;pointer-events:auto'>on</a></div>{UNDER}"
    ));
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/under"));
    assert_eq!(path(engine.link_at_point(view, 20.0, 30.0)).as_deref(), Some("/on"));
    assert_eq!(path(engine.link_at_point(view, 20.0, 50.0)).as_deref(), Some("/under"));

    let (engine, view, _) = page_list(
        "<p style='margin:0;height:20px'><a href='/off' style='pointer-events:none'>off off off off</a></p>",
    );
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)), None);
}

/// A box that is not shown is no target, in the page or held; a child of
/// it that is `visible` again is.
#[test]
fn a_hidden_box_takes_no_click_and_its_shown_child_does() {
    let (engine, view, _) = page_list(&format!(
        "<div style='position:absolute;left:0;top:0;width:200px;height:40px;visibility:hidden;background:none'>\
         <a href='/shown' style='display:block;height:20px;visibility:visible'>shown</a></div>\
         <div style='position:fixed;left:0;top:50px;width:200px;height:40px;visibility:hidden;background:none'>\
         <a href='/held' style='display:block;height:20px;visibility:visible'>held</a></div>{UNDER}"
    ));
    assert_eq!(path(engine.link_at_point(view, 20.0, 10.0)).as_deref(), Some("/shown"));
    assert_eq!(path(engine.link_at_point(view, 20.0, 30.0)).as_deref(), Some("/under"));
    assert_eq!(path(engine.link_at_point(view, 20.0, 60.0)).as_deref(), Some("/held"));
    assert_eq!(path(engine.link_at_point(view, 20.0, 80.0)).as_deref(), Some("/under"));
}

/// A page with no fixed box is hit with the one pruned walk it always
/// had, and a page with some is hit at those boxes and with that walk:
/// the layout says where they are once, and no pointer move looks for them.
#[test]
fn the_held_fixed_boxes_are_found_once_per_layout() {
    let held = |body: &str| {
        let (engine, view, _) = page_list(body);
        let view = &engine.views[&view];
        let paths = view.held_fixed.clone().expect("known once a layout is stored");
        assert_eq!(paths, view.layout.as_ref().unwrap().held_fixed_paths());
        paths.len()
    };
    assert_eq!(held(UNDER), 0, "no fixed box");
    assert_eq!(held(&format!("<div><p style='position:fixed;top:0'>bar</p></div>{UNDER}")), 1, "deep in the page");
    assert_eq!(
        held(&format!(
            "<section style='transform:translate(0,0)'><p style='position:fixed;top:0'>bar</p></section>{UNDER}"
        )),
        0,
        "a fixed box that scrolls with the page"
    );
    assert_eq!(
        held(&format!(
            "<p style='position:fixed;top:0'>one<span style='position:fixed;top:50px'>inside it</span></p>\
             <div><div><p style='position:fixed;top:20px'>two</p></div></div>{UNDER}"
        )),
        3,
        "a fixed box inside a fixed box is held too: it can lie outside it"
    );
}

/// With the paths and without them the answer is the same box, for every
/// point of a scrolled page with fixed boxes over and under it.
#[test]
fn a_hit_test_with_the_paths_is_the_hit_test_that_looks_for_them() {
    let (mut engine, view, _) = page_list(&format!(
        "<div><a href='/bar' style='display:block;position:fixed;left:0;top:0;width:200px;height:20px'>bar</a></div>\
         <div style='position:fixed;left:0;top:0;width:200px;height:100px;pointer-events:none;background:none'>\
         <a href='/toast' style='display:block;position:absolute;right:0;bottom:0;width:60px;height:20px;pointer-events:auto'>t</a></div>\
         <a href='/behind' style='display:block;position:fixed;z-index:-1;left:0;top:40px;width:40px;height:20px'>b</a>{UNDER}"
    ));
    engine.set_scroll_offset(view, 0.0, 50.0).unwrap();
    let view = &engine.views[&view];
    let (layout, held) = (view.layout.as_ref().unwrap(), view.held_fixed.as_deref().unwrap());
    assert_eq!(held.len(), 3);
    let mut seen = std::collections::BTreeSet::new();
    for y in (0..100).step_by(5) {
        for x in (0..200).step_by(10) {
            let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
            let key = |hit: Option<rustkit_layout::HitTestResult>| {
                hit.map(|h| (h.node_id, h.link_href, h.viewport_fixed, h.depth, h.ancestors.len()))
            };
            let with = key(layout.hit_test_scrolled_with(x, y, view.scroll_offset, held));
            assert_eq!(with, key(layout.hit_test_scrolled(x, y, view.scroll_offset)), "at {x},{y}");
            seen.extend(with.and_then(|k| k.1));
        }
    }
    assert_eq!(seen.len(), 3, "the bar, the toast and the page (the z-index: -1 box is under the page's link): {seen:?}");
}

/// A press and a click at the point, as the page's document listeners
/// logged them (`type:id-or-tag`).
fn logged_press(root_css: &str, x: f32, y: f32) -> String {
    let (mut engine, view, _) = page_list(&format!(
        "<style>{root_css}</style>\
         <div id=menu style='pointer-events:auto;height:30px;width:100px;background:none'>menu</div>\
         <div id=content style='height:100px;background:none'>page</div>\
         <script>var log=[];['mousedown','click'].forEach(function(t){{\
         document.addEventListener(t,function(e){{log.push(t+':'+(e.target.id||e.target.tagName))}})}})</script>"
    ));
    engine.mouse_down_at_point(view, x, y);
    engine.click_at_point(view, x, y);
    engine.execute_script(view, "log.join(' ')").expect("script")
}

/// The modal-menu pattern: `body { pointer-events: none }` while a menu
/// is open, closed by a document listener that hears a press outside it.
/// No box under the point is a target then, and the event goes to the
/// root element (Chromium 141: `mousedown:HTML click:HTML`), also when
/// the root is `none` itself. It must not go to the box under the point,
/// and it must not be dropped.
#[test]
fn a_point_where_no_box_is_a_target_reaches_the_root_element() {
    for root_css in ["body{pointer-events:none}", "html,body{pointer-events:none}"] {
        assert_eq!(
            logged_press(root_css, 150.0, 60.0),
            "String(\"mousedown:HTML click:HTML\")",
            "outside the menu, {root_css}"
        );
        assert_eq!(
            logged_press(root_css, 20.0, 10.0),
            "String(\"mousedown:menu click:menu\")",
            "on the menu, {root_css}"
        );
    }
}

/// The same answer from the element lookup the pointer paths share, at
/// the top of the page and scrolled, and with a see-through fixed overlay
/// as the only box there.
#[test]
fn the_element_at_a_point_with_no_target_is_the_root_element() {
    let (mut engine, view, _) = page_list(
        "<style>body{pointer-events:none}</style>\
         <div style='position:fixed;left:0;top:0;width:200px;height:100px;background:none'></div>\
         <p id=text>text</p>",
    );
    let root = engine.views[&view].document.as_ref().unwrap().document_element().unwrap().id.raw();
    assert_eq!(engine.element_at_point(view, 150.0, 60.0), Some(root));
    engine.set_scroll_offset(view, 0.0, 300.0).unwrap();
    assert_eq!(engine.element_at_point(view, 150.0, 60.0), Some(root));
}

/// Microseconds per `element_at_point` on the review's page: 3000 rows in
/// a 400x300 view scrolled by 4000, 2000 calls at moving points. Printed,
/// not asserted (`--release -- --ignored --nocapture`).
#[test]
#[ignore = "a timing, run by hand in a release build"]
fn timing_of_a_hit_test_with_and_without_a_fixed_box() {
    for (name, extra) in [
        ("no fixed box", ""),
        ("one fixed bar", "<div style='position:fixed;top:0;left:0;right:0;height:20px'>bar</div>"),
        (
            "full-window pointer-events:none overlay",
            "<div style='position:fixed;top:0;left:0;right:0;bottom:0;pointer-events:none'></div>",
        ),
    ] {
        let rows: String = (0..3000).map(|n| format!("<div style='height:4px'><span>{n}</span></div>")).collect();
        let page = format!("<html><body style='margin:0'>{extra}{rows}</body></html>");
        let server = serve_routes(vec![("/", "text/html", page)]);
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        engine.set_scroll_offset(view, 0.0, 4000.0).unwrap();
        let mut runs = Vec::new();
        for _ in 0..3 {
            let start = std::time::Instant::now();
            let mut found = 0usize;
            for n in 0..2000u32 {
                let (x, y) = ((n * 7 % 400) as f32, (n * 13 % 300) as f32);
                found += usize::from(engine.element_at_point(view, x, y).is_some());
            }
            runs.push(format!("{:.1}", start.elapsed().as_secs_f64() * 1e6 / 2000.0));
            assert_eq!(found, 2000);
        }
        eprintln!("TIMING {name}: {} us per call", runs.join(", "));
    }
}
