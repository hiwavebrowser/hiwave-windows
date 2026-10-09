//! A live window resize is dozens of `Resized` events a second. Until
//! 2026-10-06 the app answered each with `Engine::resize_view`: a full layout
//! and a `resize` event at `window`, before the next size was read, so the
//! content lagged the drag by as many layouts as events had queued
//! (hand-test item H9: "resize works but is slow to adapt"). Now the app
//! gives the view its bounds as they come (`set_view_bounds`) and the live
//! turn (`pump_live`) lays out once, at the last size, and the page hears
//! one `resize`, as a browser fires at most one per frame.

use super::*;
use crate::script_net_tests::serve_routes;

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server, tokio::runtime::Runtime) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server, rt)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// A page that counts its `resize` events and has a box half the viewport wide.
const PAGE: &str = "<html><head><style>body{margin:0} #half{width:50%;height:20px}</style></head><body>\
    <div id=half></div><script>\
    window.resizes = 0; window.last = '';\
    window.addEventListener('resize', function () {\
      window.resizes += 1; window.last = innerWidth + 'x' + innerHeight;\
    });\
    </script></body></html>";

/// Ten sizes between two live turns: one layout, one `resize`, at the last.
#[test]
fn many_bounds_in_one_turn_are_one_layout_and_one_resize_event() {
    let (mut engine, view, _server, rt) = load(PAGE);
    assert_eq!(read(&mut engine, view, "resizes"), "0");
    assert_eq!(read(&mut engine, view, "document.getElementById('half').offsetWidth"), "200");

    for width in (410..=500).step_by(10) {
        engine
            .set_view_bounds(view, Bounds { x: 0, y: 0, width, height: 200 })
            .expect("set_view_bounds");
    }
    // The bounds the view reports already follow the last size (the
    // surface half of a resize is immediate); the page has not been laid
    // out or told yet.
    assert_eq!(engine.views[&view].headless_bounds.unwrap().width, 500);
    assert_eq!(read(&mut engine, view, "resizes"), "0", "no resize event before the live turn");

    let turn = rt.block_on(engine.pump_live(view, 0));
    assert!(turn.relaid_out, "the live turn laid the page out");

    assert_eq!(read(&mut engine, view, "resizes"), "1", "ten sizes in one turn are one resize event");
    assert_eq!(read(&mut engine, view, "last"), "500x200", "the event carries the last size");
    assert_eq!(read(&mut engine, view, "innerWidth"), "500");
    assert_eq!(
        read(&mut engine, view, "document.getElementById('half').offsetWidth"),
        "250",
        "the layout is at the last size"
    );

    // Nothing left to do on the next turn.
    let turn = rt.block_on(engine.pump_live(view, 0));
    assert!(!turn.relaid_out, "a turn with no new bounds does not lay out again");
    assert_eq!(read(&mut engine, view, "resizes"), "1");
}

/// `flush_pending_resize` on its own: false with nothing pending, true once
/// after bounds were set, false again after that.
#[test]
fn flush_reports_whether_there_was_anything_to_do() {
    let (mut engine, view, _server, _rt) = load(PAGE);
    assert!(!engine.flush_pending_resize(view).unwrap(), "nothing pending after the load");
    engine
        .set_view_bounds(view, Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("set_view_bounds");
    assert!(engine.flush_pending_resize(view).unwrap(), "the new bounds are laid out");
    assert!(!engine.flush_pending_resize(view).unwrap(), "and only once");
    assert_eq!(read(&mut engine, view, "resizes"), "1");
    assert_eq!(read(&mut engine, view, "document.getElementById('half').offsetWidth"), "150");
}

/// `resize_view` keeps its meaning: everything now, and it clears what
/// `set_view_bounds` left pending (the live turn must not lay out a size the
/// view has already left).
#[test]
fn resize_view_is_immediate_and_settles_pending_bounds() {
    let (mut engine, view, _server, rt) = load(PAGE);
    engine
        .set_view_bounds(view, Bounds { x: 0, y: 0, width: 300, height: 200 })
        .expect("set_view_bounds");
    engine
        .resize_view(view, Bounds { x: 0, y: 0, width: 600, height: 200 })
        .expect("resize_view");
    assert_eq!(read(&mut engine, view, "resizes"), "1", "resize_view fires now");
    assert_eq!(read(&mut engine, view, "last"), "600x200");
    let turn = rt.block_on(engine.pump_live(view, 0));
    assert!(!turn.relaid_out, "nothing pending after resize_view");
    assert_eq!(read(&mut engine, view, "resizes"), "1");
}
