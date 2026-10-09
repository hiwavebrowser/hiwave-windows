//! Script scrolls the page (web_scroll.js, `Engine::apply_script_scroll`):
//! `scrollTo`, `scrollIntoView` and `scrollTop` move the view's offset, which
//! is what is painted, and `scrollY` reads it back.

use super::*;
use crate::script_net_tests::serve_routes;

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
const TALL: &str = "<html><head><style>body{margin:0} #spacer{height:1000px} #t{height:400px}</style></head><body>\
    <div id=spacer></div><div id=t></div><script>SCRIPT</script></body></html>";

fn page(script: &str) -> String {
    TALL.replace("SCRIPT", script)
}

#[test]
fn scrollintoview_during_load_moves_the_view() {
    let (mut engine, view, _server) =
        load(&page("window.addEventListener('load', function () { document.getElementById('t').scrollIntoView(); });"));
    assert_eq!(engine.views[&view].scroll_offset.1, 1000.0, "the view is scrolled to the target");
    assert_eq!(read(&mut engine, view, "scrollY"), "1000");
}

#[test]
fn scrollto_is_clamped_to_the_document() {
    let (mut engine, view, _server) =
        load(&page("window.addEventListener('load', function () { window.scrollTo(0, 99999); });"));
    // 1400 px document, 200 px viewport.
    assert_eq!(engine.views[&view].scroll_offset.1, 1200.0);
    assert_eq!(read(&mut engine, view, "scrollY"), "1200");
}

#[test]
fn a_scroll_after_the_load_reaches_the_view_too() {
    let (mut engine, view, _server) = load(&page(""));
    assert_eq!(engine.views[&view].scroll_offset.1, 0.0);
    engine.execute_script(view, "window.scrollTo(0, 300)").unwrap();
    assert_eq!(engine.views[&view].scroll_offset.1, 300.0);
    engine.execute_script(view, "document.documentElement.scrollTop = 450").unwrap();
    assert_eq!(engine.views[&view].scroll_offset.1, 450.0);
}

#[test]
fn a_user_scroll_is_what_the_page_reads() {
    let (mut engine, view, _server) = load(&page(""));
    // The viewhost scrolls with a delta that is negative downwards.
    assert!(engine.scroll_view(view, 0.0, -120.0).unwrap());
    assert_eq!(read(&mut engine, view, "scrollY"), "120");
}

#[test]
fn an_intersection_observer_reports_the_initial_state_and_then_a_user_scroll() {
    let (mut engine, view, _server) = load(&page(
        "window.__log = []; new IntersectionObserver(function (es) { es.forEach(function (e) { window.__log.push(e.isIntersecting); }); }).observe(document.getElementById('t'));",
    ));
    // The target starts at 1000 px, below the 200 px viewport.
    assert_eq!(read(&mut engine, view, "window.__log.join()"), "false");
    // The user scrolls it into view: it is reported without script running.
    assert!(engine.scroll_view(view, 0.0, -900.0).unwrap());
    assert_eq!(read(&mut engine, view, "window.__log.join()"), "false,true");
}

#[test]
fn a_user_scroll_fires_a_scroll_event() {
    let (mut engine, view, _server) =
        load(&page("window.__n = 0; window.addEventListener('scroll', function () { window.__n++; });"));
    assert_eq!(read(&mut engine, view, "window.__n"), "0");
    engine.scroll_view(view, 0.0, -50.0).unwrap();
    assert_eq!(read(&mut engine, view, "window.__n"), "1");
}

#[test]
fn a_resize_observer_reports_the_laid_out_size() {
    let (mut engine, view, _server) = load(&page(
        "window.__sizes = []; new ResizeObserver(function (es) { es.forEach(function (e) { window.__sizes.push(e.contentRect.width + 'x' + e.contentRect.height); }); }).observe(document.getElementById('t'));",
    ));
    // #t is 400 px tall; its width is the 400 px viewport's.
    assert_eq!(read(&mut engine, view, "window.__sizes.join()"), "400x400");
}
