//! The script-network bridge through a real page load: `Engine::load_url`
//! runs a page whose script uses the bridge directly (the XHR/fetch surfaces
//! come later and are built on exactly this).

use super::*;
use crate::script_net_tests::serve_routes;

fn load(config: EngineConfig, routes: Vec<(&'static str, &'static str, String)>) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(routes);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

/// Asked while scripts run, from a load handler, and from a timer; the
/// page asks relative to its own origin through `location`.
const ORIGIN_PAGE: &str = r#"<html><head><script>
var log = [];
function ask(path, tag) {
    window.__rustkit_net.request({ url: location.origin + path }, function (r) { log.push(tag + ':' + (r.ok ? r.status : r.error)); });
}
ask('/script', 'script');
window.addEventListener('load', function () { ask('/load', 'load'); });
setTimeout(function () { ask('/timer', 'timer'); }, 100);
</script></head><body>hi</body></html>"#;

#[test]
fn requests_from_scripts_handlers_and_timers_are_all_answered_during_the_load() {
    let (mut engine, view, server) = load(
        EngineConfig::default(),
        vec![("/", "text/html", ORIGIN_PAGE.to_string())],
    );
    let log = engine.execute_script(view, "log.join(',')").unwrap();
    assert_eq!(log, r#"String("script:200,load:200,timer:200")"#);
    // The page itself, plus the three requests (distinct URLs: the loader
    // caches a repeated GET).
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 4);
}

/// Off: the bridge does not exist on the page at all.
#[test]
fn with_script_network_off_the_page_has_no_bridge() {
    let (mut engine, view, server) = load(
        EngineConfig { script_network_enabled: false, ..EngineConfig::default() },
        vec![("/", "text/html", "<html><body>hi</body></html>".to_string())],
    );
    assert_eq!(
        engine.execute_script(view, "typeof window.__rustkit_net").unwrap(),
        r#"String("undefined")"#
    );
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

/// `load_html` has no origin and no policy: no bridge either.
#[test]
fn a_page_loaded_from_a_string_has_no_bridge() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    engine.load_html(view, "<html><body>hi</body></html>").unwrap();
    assert_eq!(
        engine.execute_script(view, "typeof window.__rustkit_net").unwrap(),
        r#"String("undefined")"#
    );
}

/// XMLHttpRequest through a real page load: asked while scripts run, from a
/// load handler and from a timer; a POST body round-trips; a request to
/// another private address is refused by the policy and the page sees only
/// `error`.
#[test]
fn xmlhttprequest_works_end_to_end_under_the_policy() {
    let page = r#"<html><head><script>
var log = [];
function xhr(path, tag, body) {
    var x = new XMLHttpRequest();
    x.open(body ? 'POST' : 'GET', path);
    x.onload = function () { log.push(tag + ':' + x.status + ':' + x.responseText); };
    x.onerror = function () { log.push(tag + ':error'); };
    x.send(body || null);
}
xhr('/hello', 'get');
xhr('/echo', 'post', 'posted body');
xhr('http://127.0.0.2:' + location.port + '/secret', 'denied');
window.addEventListener('load', function () { xhr('/after-load', 'load'); });
setTimeout(function () { xhr('/timer', 'timer'); }, 100);
</script></head><body>hi</body></html>"#;
    let (mut engine, view, server) = load(
        EngineConfig::default(),
        vec![("/", "text/html", page.to_string())],
    );
    let log = engine.execute_script(view, "log.join('|')").unwrap();
    assert_eq!(
        log,
        r#"String("get:200:/hello|post:200:posted body|denied:error|load:200:/after-load|timer:200:/timer")"#
    );
    // The page, plus four fetched requests; the refused one never connected.
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 5);
}

/// fetch() through a real page load: a GET, a POST that echoes its JSON
/// body, a promise chain (response.json()) and a request to another private
/// address that the policy refuses (the page sees a TypeError, nothing more).
#[test]
fn fetch_works_end_to_end_under_the_policy() {
    let page = r#"<html><head><script>
var log = [];
fetch('/hello').then(function (r) { return r.text().then(function (t) { log.push('get:' + r.status + ':' + t); }); });
fetch('/echo', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ n: 7 }) })
    .then(function (r) { return r.json(); }).then(function (j) { log.push('post:' + j.n); });
fetch('http://127.0.0.2:' + location.port + '/secret').catch(function (e) { log.push('denied:' + e.constructor.name + ':' + e.message); });
window.addEventListener('load', function () {
    fetch('/after-load').then(function (r) { return r.text(); }).then(function (t) { log.push('load:' + t); });
});
setTimeout(function () {
    fetch('/timer').then(function (r) { return r.text(); }).then(function (t) { log.push('timer:' + t); });
}, 100);
</script></head><body>hi</body></html>"#;
    let (mut engine, view, server) = load(
        EngineConfig::default(),
        vec![("/", "text/html", page.to_string())],
    );
    let log = engine.execute_script(view, "log.slice().sort().join('|')").unwrap();
    assert_eq!(
        log,
        r#"String("denied:TypeError:Failed to fetch|get:200:/hello|load:/after-load|post:7|timer:/timer")"#
    );
    // The page, plus four fetched requests; the refused one never connected.
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 5);
}

/// The live loop (Z lane I0): a request made after the load, here by the
/// script a click or a late timer would run, is answered on a later turn of
/// the loop under the same policy, and what its callback writes is laid out.
#[test]
fn a_fetch_made_after_the_load_is_answered_on_a_later_live_turn() {
    let page = r#"<html><body style="margin:0"><div style="height:40px">early</div></body></html>"#;
    let (mut engine, view, server) = load(
        EngineConfig::default(),
        vec![("/", "text/html", page.to_string())],
    );
    engine
        .execute_script(
            view,
            "window.log = []; \
             fetch('/late').then(function (r) { return r.text(); }).then(function (t) { \
               log.push('late:' + t); \
               var a = document.createElement('a'); a.setAttribute('href', 'https://example.com' + t); \
               a.style.display = 'block'; a.style.height = '40px'; a.textContent = t; \
               document.body.appendChild(a); }); \
             fetch('http://127.0.0.2:' + location.port + '/secret').catch(function (e) { log.push('denied:' + e.message); });",
        )
        .unwrap();
    assert_eq!(engine.link_at_point(view, 5.0, 60.0), None);

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    // The turn that finds the requests starts them; the answers come on the
    // turns after. The refused one is answered without a connection.
    let started = std::time::Instant::now();
    let (mut answered, mut relaid_out) = (0, false);
    while answered < 2 {
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{answered} of 2 answered");
        let turn = rt.block_on(engine.pump_live(view, 16));
        answered += turn.requests;
        relaid_out |= turn.relaid_out;
    }
    assert!(relaid_out);
    assert_eq!(
        engine.execute_script(view, "log.slice().sort().join('|')").unwrap(),
        r#"String("denied:Failed to fetch|late:/late")"#
    );
    assert_eq!(
        engine.link_at_point(view, 5.0, 60.0).as_deref(),
        Some("https://example.com/late")
    );
    // The page and the one allowed request; the refused one never connected.
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}

/// The loop that calls `pump_live` is the app's UI thread, so a turn may not
/// wait for the network. A request the server holds (300 ms here, three
/// seconds in tools/real_window's `h1_slow`) used to hold the turn that
/// took it, up to two seconds, and then fail: the window took no input and
/// drew nothing meanwhile, and a slower request never arrived. The turn
/// returns with the request still out, the page's timers run on the turns
/// in between, and the turn after the answer comes delivers it.
#[test]
fn a_slow_request_does_not_hold_the_live_turn() {
    let page = r#"<html><body style="margin:0"><div style="height:40px">early</div></body></html>"#;
    let (mut engine, view, _server) = load(
        EngineConfig::default(),
        vec![("/", "text/html", page.to_string())],
    );
    engine
        .execute_script(
            view,
            "window.log = []; window.ticks = 0; window.ticksAtAnswer = -1; \
             setInterval(function () { ticks += 1; }, 10); \
             fetch('/slow').then(function (r) { return r.text(); }).then(function (t) { \
               ticksAtAnswer = ticks; log.push('slow:' + t); });",
        )
        .unwrap();

    // One runtime for every turn, as in the app: the connection lives on it.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let first = rt.block_on(engine.pump_live(view, 16));
    let held = started.elapsed();
    assert!(
        held < std::time::Duration::from_millis(150),
        "the turn that took a request the server holds for 300 ms lasted {held:?}"
    );
    assert_eq!((first.requests, first.in_flight), (0, 1));

    let mut turns = 0;
    while engine.execute_script(view, "log.join('|')").unwrap() == r#"String("")"# {
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "the slow request never arrived");
        std::thread::sleep(std::time::Duration::from_millis(5));
        rt.block_on(engine.pump_live(view, 5));
        turns += 1;
    }
    assert_eq!(engine.execute_script(view, "log.join('|')").unwrap(), r#"String("slow:/slow")"#);
    assert!(turns > 3, "the answer came on turn {turns}; it should take many 5 ms turns");
    // The page's clock ran while the request was out.
    let ticks = engine.execute_script(view, "ticksAtAnswer").unwrap();
    let ticks: f64 = ticks.trim_start_matches("Number(").trim_end_matches(')').parse().unwrap();
    assert!(ticks >= 3.0, "only {ticks} 10 ms ticks ran while a 300 ms request was out");
}

/// The same for an image a script adds after the load (tools/real_window's
/// `h4_slow`: an image held three seconds stopped the page for three
/// seconds). The turn that finds the image starts its fetch and returns; a
/// later turn keeps it and lays the page out again.
#[test]
fn a_slow_image_added_by_a_script_does_not_hold_the_live_turn() {
    let page = r#"<html><body style="margin:0"><div style="height:40px">early</div></body></html>"#;
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50"><rect width="100" height="50" fill="red"/></svg>"#;
    let (mut engine, view, server) = load(
        EngineConfig::default(),
        vec![("/", "text/html", page.to_string()), ("/held.svg", "image/svg+xml", svg.to_string())],
    );
    let image = format!("http://127.0.0.1:{}/held.svg", server.port);
    engine
        .execute_script(
            view,
            "setTimeout(function () { \
               var img = document.createElement('img'); img.setAttribute('src', '/held.svg'); \
               document.body.appendChild(img); }, 10);",
        )
        .unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = std::time::Instant::now();
    let first = rt.block_on(engine.pump_live(view, 16));
    let held = started.elapsed();
    assert!(first.relaid_out, "the turn lays out what its timer appended");
    assert!(
        held < std::time::Duration::from_millis(150),
        "the turn that found an image the server holds for 300 ms lasted {held:?}"
    );
    assert!(!engine.svg_cache.contains_key(&image));

    let mut turns = 0;
    loop {
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "the slow image never arrived");
        std::thread::sleep(std::time::Duration::from_millis(5));
        let turn = rt.block_on(engine.pump_live(view, 5));
        turns += 1;
        if engine.svg_cache.contains_key(&image) {
            assert!(turn.relaid_out, "the turn that keeps the image lays the page out again");
            break;
        }
        assert!(!turn.relaid_out, "nothing changed on turn {turns}");
    }
    assert!(turns > 3, "the image came on turn {turns}; it should take many 5 ms turns");
    // The page and the image, once.
    assert_eq!(server.hits.load(std::sync::atomic::Ordering::SeqCst), 2);
}
