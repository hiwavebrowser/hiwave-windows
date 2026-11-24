//! ROOT-REM, R1 nit 2 on #1408: a view's `rem` holds on frames and reads that do not lay it out.
//!
//! `rem` is one value per thread (`rustkit_css::root_font_px`), which a view's layout sets from
//! its own `<html>`. Two paths use it without laying the view out first:
//!
//! - a paint-only frame (an animation frame, an element scroll) builds the display list over the
//!   boxes already laid out, and the list resolves `rem` in outlines, transforms,
//!   `transform-origin`, `backdrop-filter` and form-control padding;
//! - the lazy computed-style reader formats an element's rows on the first script read.
//!
//! Both took the `rem` of whichever view on the thread was laid out last, as the paint-only frame
//! once took its web fonts (E52). Here view A's root is 10px, view B's is 30px, and B is laid out
//! after A.

use super::*;
use rustkit_layout::DisplayCommand;

/// A green `1rem` outline (10px here) and a `1rem` radius, beside a spinner that keeps the view
/// painting frames without a layout.
const PAGE_A: &str = r#"<!doctype html><html style="font-size: 10px"><head><style>
body { margin: 0; }
#o { position: absolute; left: 50px; top: 50px; width: 100px; height: 40px;
  outline: 1rem solid rgb(0, 128, 0); }
#r { position: absolute; left: 50px; top: 150px; width: 40px; height: 40px; border-radius: 1rem;
  background: #ccc; }
#spin { position: absolute; left: 300px; top: 20px; width: 20px; height: 20px; background: #08f;
  animation: spin 1s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style></head><body><div id=o></div><div id=r></div><div id=spin></div></body></html>"#;

/// Another document with a 30px root and an animation of its own.
const PAGE_B: &str = r#"<!doctype html><html style="font-size: 30px"><head><style>
body { margin: 0; }
#bar { width: 2rem; height: 4px; background: red; animation: fade .7s linear infinite alternate; }
@keyframes fade { from { opacity: .2 } to { opacity: 1 } }
</style></head><body><div id=bar></div></body></html>"#;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn two_views() -> (Engine, EngineViewId, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let a = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 240 }).expect("view a");
    let b = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 60 }).expect("view b");
    engine.load_html(a, PAGE_A).expect("load a");
    rt().block_on(engine.pump_live(a, 16));
    engine.load_html(b, PAGE_B).expect("load b");
    rt().block_on(engine.pump_live(b, 16));
    (engine, a, b)
}

/// The width of view A's green outline ring: its top strip's height.
fn outline_width(engine: &Engine, view: EngineViewId) -> f32 {
    let green = |c: &rustkit_css::Color| c.r == 0 && c.g == 128 && c.b == 0;
    engine.views[&view]
        .display_list
        .as_ref()
        .expect("a display list")
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::SolidColor(color, rect) if green(color) && rect.width > rect.height => Some(rect.height),
            _ => None,
        })
        .expect("the outline's top strip")
}

#[test]
fn a_paint_only_frame_keeps_its_views_rem_after_another_view_laid_out() {
    let (mut engine, a, b) = two_views();
    let laid_out = engine.relayout_stats(a).expect("view").relayouts;
    let painted = engine.relayout_stats(a).expect("view").animation_frames_painted;
    // B lays out again, last on the thread, then A paints frames over its own boxes.
    engine.relayout(b).expect("relayout b");
    rt().block_on(engine.pump_live(a, 16));
    let stats = engine.relayout_stats(a).expect("view");
    assert_eq!(stats.relayouts, laid_out, "A was not laid out again");
    assert!(stats.animation_frames_painted > painted, "A painted a frame without a layout");
    assert_eq!(outline_width(&engine, a), 10.0, "A's 1rem outline is A's 10px, not B's 30px");
}

#[test]
fn a_computed_read_keeps_its_views_rem_after_another_view_laid_out() {
    let (mut engine, a, b) = two_views();
    engine.relayout(b).expect("relayout b");
    let radius = engine
        .execute_script(a, "getComputedStyle(document.getElementById('r')).borderTopLeftRadius")
        .expect("read");
    assert_eq!(radius, "String(\"10px\")", "A's 1rem radius is A's 10px, not B's 30px");
}
