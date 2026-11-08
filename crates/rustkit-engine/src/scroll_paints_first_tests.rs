//! T2 of the threads plan (`docs/design/THREADS_PLAN_2026-10.md` §5): a wheel
//! event the window thread processes moves the page on that frame.
//!
//! Until 2026-10-14 `scroll_view` told the page first: its `scroll` listeners
//! ran, the relayout their writes caused ran (about 500 ms per relayout on
//! eBay search in the 2026-10-13 hand test), and only then was the frame
//! drawn at the new offset, at the event loop's next wake. Now the offset is
//! applied and the last display list is presented again at it BEFORE the page
//! hears the scroll; the relayout follows and presents its own frame.
//!
//! This cannot help a wheel event that arrives while the thread is already
//! blocked (M4b, presenter-owned scroll, is for that); it makes every wheel
//! event that is processed move the page before the page's own work.

use super::*;
use crate::script_net_tests::serve_routes;
use std::time::{Duration, Instant};

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine.execute_script(view, &format!("String({expr})")).unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// 1000 px of spacer, then a 400 px target: a 1400 px document in a 200 px viewport.
const TALL: &str = "<html><head><style>body{margin:0} #spacer{height:1000px} #t{height:400px;background:#eee}</style></head><body>\
    <div id=spacer></div><div id=t></div><script>SCRIPT</script></body></html>";

fn page(script: &str) -> String {
    TALL.replace("SCRIPT", script)
}

/// A `scroll` listener that counts the scrolls it hears, holds the page for
/// `ms`, then runs `then`.
fn listener(ms: u32, then: &str) -> String {
    format!(
        "window.__heard = 0; document.addEventListener('scroll', function () {{ \
         window.__heard += 1; var end = Date.now() + {ms}; while (Date.now() < end) {{}} {then} }});"
    )
}

fn frame(engine: &Engine, view: EngineViewId) -> &PresentedFrame {
    engine.views[&view].presented.as_ref().expect("the load presented a frame")
}

fn presented(engine: &Engine, view: EngineViewId) -> u64 {
    engine.views[&view].frames_presented
}

#[test]
fn the_frame_moves_before_the_page_hears_the_scroll() {
    // The listener holds the page for 300 ms and writes nothing.
    let (mut engine, view, _server) = load(&page(&listener(300, "")));
    let n0 = presented(&engine, view);
    let g0 = engine.views[&view].frame_generation;
    let t0 = Instant::now();
    assert!(engine.scroll_view(view, 0.0, -120.0).unwrap());
    let took = t0.elapsed();
    assert!(took >= Duration::from_millis(300), "the listener ran inside the call: {took:?}");
    assert_eq!(read(&mut engine, view, "__heard"), "1", "the page heard the scroll once");

    let f = frame(&engine, view);
    assert_eq!(f.scroll_offset, (0.0, 120.0), "the frame on record is at the new offset");
    assert_eq!(f.generation, g0, "drawn from the display list as it stood; nothing was laid out");
    assert!(f.at > t0, "presented by this scroll, not by the load");
    // The present ended at least 200 ms before the call returned: it came
    // before the listener's 300 ms, not after it.
    let age = f.at.saturating_duration_since(t0);
    assert!(
        age + Duration::from_millis(200) < took,
        "the frame was presented {age:?} into a {took:?} call: after the page heard the scroll"
    );
    assert_eq!(presented(&engine, view), n0 + 1, "one present: the listener wrote nothing");
    assert_eq!(engine.render_changed_views(), 0, "that frame is the frame of record; no second draw");
}

#[test]
fn a_relayout_the_listener_causes_follows_the_moved_frame() {
    // The listener writes a style, so a relayout follows the scroll.
    let (mut engine, view, _server) = load(&page(&listener(
        50,
        "document.getElementById('t').style.background = 'red';",
    )));
    let n0 = presented(&engine, view);
    let g0 = engine.views[&view].frame_generation;
    assert!(engine.scroll_view(view, 0.0, -120.0).unwrap());
    assert_eq!(read(&mut engine, view, "__heard"), "1");

    assert_eq!(
        presented(&engine, view),
        n0 + 2,
        "two presents: the moved frame before the page heard it, the relaid-out frame after"
    );
    let f = frame(&engine, view);
    assert_eq!(f.generation, g0 + 1, "the frame on record is the relayout's");
    assert_eq!(f.scroll_offset, (0.0, 120.0));
    assert_eq!(engine.render_changed_views(), 0);
}

#[test]
fn a_listener_that_scrolls_back_has_the_last_word() {
    // The listener scrolls the page back to the top: the user's offset was
    // shown first, the page's is what stays.
    let (mut engine, view, _server) = load(&page(&listener(50, "window.scrollTo(0, 0);")));
    let n0 = presented(&engine, view);
    assert!(engine.scroll_view(view, 0.0, -120.0).unwrap());
    assert_eq!(read(&mut engine, view, "__heard"), "1");
    assert_eq!(presented(&engine, view), n0 + 2, "the user's frame, then the page's");
    assert_eq!(frame(&engine, view).scroll_offset, (0.0, 0.0), "the page's scrollTo is on record");
    assert_eq!(engine.views[&view].scroll_offset, (0.0, 0.0));
    assert_eq!(read(&mut engine, view, "scrollY"), "0");
    assert_eq!(engine.render_changed_views(), 0);
}

#[test]
fn a_scroll_that_cannot_move_presents_nothing_and_tells_nobody() {
    let (mut engine, view, _server) = load(&page(&listener(0, "")));
    let n0 = presented(&engine, view);
    // At the top already; a scroll up moves nothing.
    assert!(!engine.scroll_view(view, 0.0, 50.0).unwrap());
    assert_eq!(presented(&engine, view), n0, "nothing to draw");
    assert_eq!(read(&mut engine, view, "__heard"), "0", "no scroll event for no scroll");
    assert_eq!(engine.render_changed_views(), 0);
}
