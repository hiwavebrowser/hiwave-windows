//! Plan row C2 (b): a wheel moves the innermost box under the pointer that can
//! move that way, the window only when none can. Each case's expected state is
//! Chromium 143.0.7499.4 on the same page, driven by real wheel input
//! (`trench/z/tools/c2/c2b_probe.mjs` on the hub). The sign is the app's: a
//! negative `dy` scrolls down, a positive `dx` scrolls right.

use super::*;

const PAGE: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}",
    "#out{overflow:auto;width:200px;height:200px;position:absolute;left:0;top:0}",
    "#out-in{height:600px}",
    "#in{overflow:auto;width:150px;height:100px}",
    "#in-in{height:300px;background:#ddd}",
    "#hid{overflow:hidden;width:100px;height:100px;position:absolute;left:300px;top:0}",
    "#hid-in{height:400px}",
    "#x{overflow-x:auto;overflow-y:hidden;width:100px;height:100px;position:absolute;left:600px;top:0}",
    "#x-in{width:400px;height:100px}",
    "#short{overflow:auto;width:100px;height:100px;position:absolute;left:150px;top:300px}",
    "</style><body><div id=out><div id=out-in><div id=in><div id=in-in></div></div></div></div>",
    "<div id=hid><div id=hid-in></div></div>",
    "<div id=x><div id=x-in></div></div>",
    "<div id=short>s</div>",
    "</body>",
);

/// Longer than a wheel gesture holds its target.
const NEW_GESTURE: std::time::Duration = std::time::Duration::from_millis(350);

fn loaded() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    (engine, view)
}

fn state(engine: &mut Engine, view: EngineViewId) -> String {
    let got = engine
        .execute_script(
            view,
            "['in', 'out', 'hid', 'x'].map(function (i) { var e = document.getElementById(i); \
             return i + '=' + e.scrollLeft + ',' + e.scrollTop; }).join(' ') + ' win=' + scrollX + ',' + scrollY",
        )
        .expect("script");
    got.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&got).to_string()
}

#[track_caller]
fn wheel(setup: &str, x: f32, y: f32, dx: f32, dy: f32, want: &str) {
    let (mut engine, view) = loaded();
    if !setup.is_empty() {
        engine.execute_script(view, setup).expect("setup");
    }
    engine.scroll_at(view, x, y, dx, dy).expect("wheel");
    assert_eq!(state(&mut engine, view), want);
}

const AT_END: &str = "document.getElementById('in').scrollTop = 9999";
const BOTH_AT_END: &str = "document.getElementById('in').scrollTop = 9999; document.getElementById('out').scrollTop = 9999";

#[test]
fn the_innermost_box_takes_the_wheel() {
    wheel("", 20.0, 20.0, 0.0, -50.0, "in=0,50 out=0,0 hid=0,0 x=0,0 win=0,0");
}

/// What it could not use goes nowhere in the same wheel.
#[test]
fn a_box_stops_at_its_end_and_keeps_the_rest() {
    wheel("", 20.0, 20.0, 0.0, -500.0, "in=0,200 out=0,0 hid=0,0 x=0,0 win=0,0");
}

#[test]
fn a_box_at_its_end_hands_on_to_its_scroller() {
    wheel(AT_END, 20.0, 20.0, 0.0, -50.0, "in=0,200 out=0,50 hid=0,0 x=0,0 win=0,0");
}

#[test]
fn every_box_at_its_end_hands_on_to_the_window() {
    wheel(BOTH_AT_END, 20.0, 20.0, 0.0, -50.0, "in=0,200 out=0,400 hid=0,0 x=0,0 win=0,50");
}

#[test]
fn at_the_top_going_up_hands_on_too() {
    wheel("document.getElementById('out').scrollTop = 30", 20.0, 20.0, 0.0, 50.0, "in=0,0 out=0,0 hid=0,0 x=0,0 win=0,0");
}

#[test]
fn outside_the_inner_box_the_outer_one_takes_it() {
    wheel("", 170.0, 150.0, 0.0, -50.0, "in=0,0 out=0,50 hid=0,0 x=0,0 win=0,0");
}

#[test]
fn overflow_hidden_does_not_take_the_wheel() {
    wheel("", 320.0, 20.0, 0.0, -50.0, "in=0,0 out=0,0 hid=0,0 x=0,0 win=0,50");
}

#[test]
fn a_box_that_scrolls_across_only_leaves_a_down_wheel_to_the_window() {
    wheel("", 620.0, 20.0, 0.0, -50.0, "in=0,0 out=0,0 hid=0,0 x=0,0 win=0,50");
}

#[test]
fn a_box_that_scrolls_across_takes_a_wheel_across() {
    wheel("", 620.0, 20.0, 50.0, 0.0, "in=0,0 out=0,0 hid=0,0 x=50,0 win=0,0");
}

/// It can move across, so it takes the wheel, and the down half goes nowhere.
#[test]
fn a_diagonal_wheel_stays_on_the_box_that_moves() {
    wheel("", 620.0, 20.0, 50.0, -50.0, "in=0,0 out=0,0 hid=0,0 x=50,0 win=0,0");
}

#[test]
fn a_box_with_nothing_to_scroll_leaves_it_to_the_window() {
    wheel("", 170.0, 320.0, 0.0, -50.0, "in=0,0 out=0,0 hid=0,0 x=0,0 win=0,50");
}

#[test]
fn the_plain_page_scrolls_the_window() {
    wheel("", 700.0, 500.0, 0.0, -50.0, "in=0,0 out=0,0 hid=0,0 x=0,0 win=0,50");
}

/// The wheel paints, and the box hears one `scroll`, which does not bubble.
#[test]
fn the_wheel_paints_and_fires_scroll_at_the_box() {
    let (mut engine, view) = loaded();
    engine
        .execute_script(
            view,
            "window.ev = []; document.getElementById('in').addEventListener('scroll', function (e) { ev.push('in:' + e.bubbles); }); \
             document.getElementById('out').addEventListener('scroll', function () { ev.push('out'); }); \
             document.addEventListener('scroll', function () { ev.push('doc'); });",
        )
        .expect("script");
    let list_before = engine.views.get(&view).unwrap().frame_generation;
    assert!(engine.scroll_at(view, 20.0, 20.0, 0.0, -50.0).expect("wheel"));
    assert!(engine.views.get(&view).unwrap().frame_generation > list_before, "a new list");
    let got = engine.execute_script(view, "ev.join(' ')").expect("script");
    assert_eq!(got, "String(\"in:false\")");
    let hit = engine.execute_script(view, "document.elementFromPoint(20, 20).id").expect("script");
    assert_eq!(hit, "String(\"in-in\")");
}

/// A wheel that moves nothing reports it, and fires nothing.
#[test]
fn nothing_to_move_reports_no_change() {
    let (mut engine, view) = loaded();
    assert!(!engine.scroll_at(view, 700.0, 500.0, 0.0, 50.0).expect("wheel"), "the window is at the top");
}

// The gesture hold. A run of wheels stays on what the first one moved, but
// only while that can still take the wheel: Chromium 143 hands a wheel on the
// moment its box is at its end, with no pause (`c2b_latch_probe.mjs` on the
// hub: `box-then-again` box=50 win=500), and a box coming under the pointer
// does not catch a page's run (`window-then-box-under` box=0 win=200).

/// A run that reaches the box's end hands the next step on at once.
#[test]
fn a_gesture_hands_on_when_its_box_is_at_its_end() {
    let (mut engine, view) = loaded();
    engine.scroll_at(view, 20.0, 20.0, 0.0, -500.0).expect("wheel");
    assert!(engine.scroll_at(view, 20.0, 20.0, 0.0, -50.0).expect("wheel"), "handed on to the outer box");
    assert_eq!(state(&mut engine, view), "in=0,200 out=0,50 hid=0,0 x=0,0 win=0,0");
    std::thread::sleep(NEW_GESTURE);
    engine.scroll_at(view, 20.0, 20.0, 0.0, -50.0).expect("wheel");
    assert_eq!(state(&mut engine, view), "in=0,200 out=0,100 hid=0,0 x=0,0 win=0,0", "a new gesture routes the same");
}

/// A held box that can still move on one axis keeps the whole step; only
/// when no axis can move does the run hand on.
#[test]
fn a_held_box_keeps_a_diagonal_step_while_one_axis_can_move() {
    let (mut engine, view) = loaded();
    engine.scroll_at(view, 620.0, 20.0, 280.0, -50.0).expect("wheel");
    assert_eq!(state(&mut engine, view), "in=0,0 out=0,0 hid=0,0 x=280,0 win=0,0");
    assert!(engine.scroll_at(view, 620.0, 20.0, 50.0, -50.0).expect("wheel"), "the box still moves across");
    assert_eq!(state(&mut engine, view), "in=0,0 out=0,0 hid=0,0 x=300,0 win=0,0", "the down half goes nowhere");
    assert!(engine.scroll_at(view, 620.0, 20.0, 50.0, -50.0).expect("wheel"), "handed on to the window");
    assert_eq!(state(&mut engine, view), "in=0,0 out=0,0 hid=0,0 x=300,0 win=0,50", "at its end on both axes it hands on");
}

/// A flick scrolling the page is not caught by a box that comes under the pointer.
#[test]
fn a_gesture_on_the_window_is_not_caught_by_a_box_coming_under_the_pointer() {
    let (mut engine, view) = loaded();
    engine.scroll_at(view, 700.0, 500.0, 0.0, -50.0).expect("wheel");
    engine.scroll_at(view, 20.0, 20.0, 0.0, -50.0).expect("wheel");
    assert_eq!(state(&mut engine, view), "in=0,0 out=0,0 hid=0,0 x=0,0 win=0,100");
}

/// Trackpad steps smaller than a pixel add up.
#[test]
fn small_steps_add_up() {
    let (mut engine, view) = loaded();
    for _ in 0..10 {
        engine.scroll_at(view, 20.0, 20.0, 0.0, -0.4).expect("wheel");
    }
    assert_eq!(state(&mut engine, view), "in=0,4 out=0,0 hid=0,0 x=0,0 win=0,0");
}

/// An absolute box is not in the scrolling box of a static scroller around it:
/// the wheel over it goes to its containing block's scroller
/// (`c2b_escape_probe.mjs`).
#[test]
fn an_absolute_box_chains_past_a_static_scroller_around_it() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine
        .load_html(
            view,
            concat!(
                "<!doctype html><style>body{margin:0;height:3000px}",
                "#st{overflow:auto;width:100px;height:100px;position:absolute;left:0;top:300px}",
                "#st2{overflow:auto;width:80px;height:50px}#st2-in{height:200px}",
                "#esc{position:absolute;left:10px;top:10px;width:30px;height:30px;background:#00f}",
                "#rest{height:300px}",
                "</style><body><div id=st><div id=st2><div id=st2-in><div id=esc></div></div></div>",
                "<div id=rest></div></div></body>",
            ),
        )
        .expect("load");
    engine.scroll_at(view, 20.0, 320.0, 0.0, -50.0).expect("wheel");
    let got = engine
        .execute_script(
            view,
            "'st=' + document.getElementById('st').scrollTop + ' st2=' + document.getElementById('st2').scrollTop + ' win=' + scrollY",
        )
        .expect("script");
    assert_eq!(got, "String(\"st=50 st2=0 win=0\")");
}
