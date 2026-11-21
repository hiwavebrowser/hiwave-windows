//! The W-gate's hold log sees every entry the window thread makes into
//! the engine (threads plan §4 invariant 9): a script the app runs on the
//! page (`execute_script`: the chrome's IPC path, `history.back()` from a
//! toolbar button) and a draw it asks for (`render_view`) included (#704
//! review).

use super::*;
use crate::script_net_tests::serve_routes;
use crate::wgate::HoldBucket;

const PAGE: &str = "<html><head><style>body{margin:0} #a{height:40px;background:#8bc}</style>\
    </head><body><div id=a>a</div></body></html>";

/// Spins on the page for about `ms` milliseconds without writing to it.
fn spin(ms: u32) -> String {
    format!("var t = Date.now(); while (Date.now() - t < {ms}) {{}} 'spun'")
}

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

/// A page listener that spins for 200 ms on `popstate` without writing
/// to the DOM used to leave no hold line: only a relayout nested in the
/// script was timed.
///
/// `spin` reads `Date.now()`, whole milliseconds rounded down, so a
/// `spin(90)` can leave after 89.0 ms of real time while the hold is timed
/// in fractions; the bound asked of the hold sits 10 ms inside the spin
/// (`spin(80)` against 80.0 failed 9 of 21 runs at 79.52 to 79.95 ms).
#[test]
fn a_script_the_app_runs_on_the_page_is_a_hold() {
    let (mut engine, view, _server, _rt) = load();
    wgate::clear_last();
    let result = engine.execute_script(view, &spin(90)).expect("script");
    assert!(result.contains("spun"), "{result}");
    let (entry, ms) = wgate::last_hold().expect("a 90 ms script from the window thread is reported");
    assert_eq!(entry, "script");
    assert!(ms >= 80.0, "{ms}");
}

/// T12: a script from the window thread is a script turn in its hold line.
#[test]
fn a_script_the_app_runs_is_a_script_turn_in_its_hold() {
    let (mut engine, view, _server, _rt) = load();
    wgate::clear_last();
    engine.execute_script(view, &spin(90)).expect("script");
    let stages = wgate::last_stages();
    assert!(stages.in_bucket(HoldBucket::Script) >= 80.0, "{stages}");
    assert_eq!(stages.turns().0, 1, "{stages}");
}

/// T12: `other_ms` was 88% of a 36.8 s microsoft load hold. A load names its
/// document fetch, parse, sheets and script turns. No bound on what is left:
/// on this tiny page in a debug build it is most of the load (about 700 ms);
/// a parity build's microsoft load leaves 0.6%.
#[test]
fn a_load_hold_names_where_its_time_went() {
    let page = format!(
        "<html><head><link rel=stylesheet href=/a.css></head><body><img src=/i.svg>\
         <div id=a>a</div><script src=/s.js></script><script>{}</script></body></html>",
        spin(150)
    );
    let server = serve_routes(vec![
        ("/", "text/html", page),
        ("/a.css", "text/css", "#a{height:40px}".to_string()),
        ("/i.svg", "image/svg+xml", "<svg xmlns='http://www.w3.org/2000/svg'/>".to_string()),
        ("/s.js", "text/javascript", spin(1)),
    ]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    wgate::clear_last();
    let started = std::time::Instant::now();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let total = started.elapsed().as_secs_f64() * 1000.0;
    assert_eq!(wgate::last_entry(), Some("load"));
    assert!(wgate::last_hold().is_some_and(|(_, ms)| ms <= total));
    let stages = wgate::last_stages();
    assert!(stages.in_bucket(HoldBucket::Script) >= 140.0, "{stages}");
    assert!(stages.turns().0 >= 2, "{stages}");
    for bucket in [HoldBucket::DocFetch, HoldBucket::Parse, HoldBucket::Sheets, HoldBucket::Style, HoldBucket::Images] {
        assert!(stages.in_bucket(bucket) > 0.0, "{bucket:?}: {stages}");
    }
    assert!(stages.other_ms(total) <= total - stages.in_bucket(HoldBucket::Script), "{stages}");
}

/// The relayout that flushes the script's DOM writes is nested in the
/// script's entry: one line, named for the script, with the layout in its
/// breakdown.
#[test]
fn a_script_that_writes_the_page_reports_once_as_the_script() {
    let (mut engine, view, _server, _rt) = load();
    let generation = engine.views[&view].frame_generation;
    wgate::clear_last();
    let script = format!(
        "document.getElementById('a').style.height = '300px'; {}",
        spin(80)
    );
    engine.execute_script(view, &script).expect("script");
    assert_ne!(engine.views[&view].frame_generation, generation, "the write was flushed");
    assert_eq!(wgate::last_entry(), Some("script"), "the nested relayout did not report itself");
    let (entry, _) = wgate::last_hold().expect("reported");
    assert_eq!(entry, "script");
}

/// A draw the app asks for by view is an entry too (the chrome's render
/// path and the capture tool call it); it is short here, so it is
/// recorded and not reported.
#[test]
fn a_draw_the_app_asks_for_is_an_entry() {
    let (mut engine, view, _server, _rt) = load();
    wgate::clear_last();
    engine.render_view(view).expect("render");
    assert_eq!(wgate::last_entry(), Some("render_view"));
}
