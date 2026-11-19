//! WHL2: a wheel is held on what the wheel before it moved only inside a
//! gesture.
//!
//! The engine keeps a run of wheels on the scroller the first one moved
//! (`WHEEL_GESTURE_HOLD`), so a box coming under the pointer does not catch
//! a trackpad flick. It did that by time alone, for every wheel. A wheel
//! from a mouse is not part of a gesture (macOS gives it no phase), and
//! Chrome sends each one to what is under the pointer, however soon after
//! the last: docs/diagnostics/2026-11-12/whl2_discrete_wheel_cft148.txt.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-12/whl2_discrete_wheel.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-12/whl2_discrete_wheel_cft148.txt");

fn after_three_wheels(gesture: bool) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    // The engine's own delta is positive for up. One wheel over the page,
    // then two over the inner scroller, with nothing between them.
    engine.wheel_at(view, 50.0, 300.0, 0.0, -100.0, gesture).expect("wheel");
    engine.wheel_at(view, 450.0, 50.0, 0.0, -100.0, gesture).expect("wheel");
    engine.wheel_at(view, 450.0, 50.0, 0.0, -100.0, gesture).expect("wheel");
    let got = engine
        .execute_script(view, "'scrollY=' + window.scrollY + ' scroller=' + document.getElementById('scroller').scrollTop")
        .expect("script");
    got.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&got).to_string()
}

#[test]
fn a_discrete_wheel_goes_to_what_is_under_the_pointer_however_soon_after_the_last() {
    let expected = CHROME.lines().find(|l| !l.starts_with('#') && !l.is_empty()).expect("the Chrome line");
    assert_eq!(after_three_wheels(false), expected);
}

/// The hold itself stays, for the steps of a gesture: the page keeps a
/// flick that began on it.
#[test]
fn the_steps_of_a_gesture_stay_on_what_the_first_one_moved() {
    assert_eq!(after_three_wheels(true), "scrollY=300 scroller=0");
}
