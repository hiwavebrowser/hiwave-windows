//! The app calls the engine at every wake of its event loop: a mouse move,
//! a message from the browser's own UI, a timer. Until 2026-10-08 each wake
//! executed the view's whole display list and presented it, changed or not
//! (the Wikipedia portal's list was 2.2 million commands, github's 390
//! thousand: the "silent spin" after a load). `render_changed_views` draws a
//! view only when the frame would differ from the one it last presented.

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE: &str = "<html><head><style>body{margin:0} #tall{height:1000px;background:#eee}\
    #late{width:20px;height:20px;background-image:url(/late.png)}</style></head><body>\
    <div id=late></div><div id=tall></div></body></html>";

/// A 1 x 1 PNG.
const PNG: &[u8] = &[137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 252, 207, 192, 80, 15, 0, 4, 133, 1, 128, 132, 169, 140, 33, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130];

fn load() -> (Engine, EngineViewId, crate::script_net_tests::Server, tokio::runtime::Runtime) {
    let server = serve_routes(vec![("/", "text/html", PAGE.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server, rt)
}

#[test]
fn a_view_is_drawn_again_only_when_its_frame_would_differ() {
    let (mut engine, view, server, rt) = load();
    // The load presented its frame.
    assert_eq!(engine.render_changed_views(), 0, "nothing changed since the load");
    assert_eq!(engine.render_changed_views(), 0);

    // A scroll moves the frame.
    assert!(engine.scroll_view(view, 0.0, -50.0).unwrap());
    assert_eq!(engine.render_changed_views(), 1, "after a scroll");
    assert_eq!(engine.render_changed_views(), 0);
    // A scroll that cannot move (already at the top after going back) does not.
    assert!(engine.scroll_view(view, 0.0, 50.0).unwrap());
    assert_eq!(engine.render_changed_views(), 1);
    assert!(!engine.scroll_view(view, 0.0, 50.0).unwrap());
    assert_eq!(engine.render_changed_views(), 0, "a scroll against the edge");

    // Script writes to the page: the new layout presents its own frame,
    // and that is the frame on record.
    let presented = |engine: &Engine| engine.views[&view].presented.as_ref().unwrap().generation;
    let before = presented(&engine);
    engine
        .execute_script(view, "document.getElementById('tall').style.background = 'red'")
        .unwrap();
    assert!(presented(&engine) > before, "a script's write was laid out and presented");
    assert_eq!(engine.render_changed_views(), 0);

    // A new size: drawn at every wake until the live turn has laid it out.
    let before = presented(&engine);
    engine
        .set_view_bounds(view, Bounds { x: 0, y: 0, width: 500, height: 200 })
        .unwrap();
    assert_eq!(engine.render_changed_views(), 1, "the surface has a new size");
    assert_eq!(engine.render_changed_views(), 1, "and no layout for it yet");
    rt.block_on(engine.pump_live(view, 0));
    assert!(presented(&engine) > before, "the live turn laid it out and presented it");
    assert_eq!(engine.render_changed_views(), 0);

    // An image the frame was drawn without arrives in the cache.
    let late = Url::parse(&format!("http://127.0.0.1:{}/late.png", server.port)).unwrap();
    engine.image_manager.insert_fetched(&late, Some("image/png"), PNG).expect("png");
    assert_eq!(engine.render_changed_views(), 1, "an image the last frame lacked is here");
    assert_eq!(engine.render_changed_views(), 0);

    // The unconditional call still draws.
    engine.render_all_views();
    assert_eq!(engine.render_changed_views(), 0);
}
