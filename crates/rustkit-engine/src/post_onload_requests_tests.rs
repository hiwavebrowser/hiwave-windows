//! What a page asks of the network after its `load` event does not hold the
//! load.
//!
//! Recorded eBay replay, 2026-10: after `load` the listing page kept sending
//! tracking pings (image pixels, metrics `fetch`es), and the load waited for
//! them on the window's thread, about 3.3 s of a 17.7 s hold. Chrome never
//! waits for these; they may never stop. With
//! `EngineConfig::post_load_requests_live` they go to the live loop, which
//! starts them and delivers their answers on later turns.

use super::*;
use crate::script_net_tests::serve_routes;
use std::time::{Duration, Instant};

/// The load handler appends a pixel and sends a metrics `fetch`; the server
/// answers each after 2 s (`/beacon*`).
const PINGS_AFTER_LOAD: &str = "<html><body><p id=p>listing</p><script>
window.addEventListener('load', function () {
  window.loadFired = true;
  var img = document.createElement('img');
  img.src = '/beacon-pixel.gif';
  document.body.appendChild(img);
  fetch('/beacon-metrics').then(function (r) { return r.text(); }).then(function (t) {
    window.got = t;
    document.title = 'answered';
  });
});
</script></body></html>";

fn live() -> EngineConfig {
    EngineConfig {
        post_load_requests_live: true,
        ..EngineConfig::default()
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

struct Loaded {
    engine: Engine,
    view: EngineViewId,
    rt: tokio::runtime::Runtime,
    server: crate::script_net_tests::Server,
    /// From the `load` event firing to the load's end.
    after_load_event: Duration,
    page_loaded: bool,
}

/// Loads `page` the way the app does (`step_load`): one runtime for the
/// load and the live turns after it, the load future polled 8 ms at a time.
fn load(config: EngineConfig, page: &str) -> Loaded {
    let server = serve_routes(vec![
        ("/", "text/html", page.to_string()),
        ("/held-pre.png", "image/png", "x".into()),
    ]);
    let mut engine = Engine::new(config).expect("engine");
    let mut events = engine.take_event_receiver().expect("events");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 200,
            height: 100,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = runtime();
    let load_event = std::rc::Rc::new(std::cell::Cell::new(None));
    let seen = load_event.clone();
    nav_cancel::LOAD_PHASE_HOOK.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(move |phase: &str| {
            if phase == "event:load" {
                seen.set(Some(Instant::now()));
            }
        }))
    });
    {
        let mut future = Box::pin(engine.load_url(view, url.clone()));
        loop {
            let slice = rt.block_on(async {
                tokio::time::timeout(Duration::from_millis(8), future.as_mut()).await
            });
            if let Ok(result) = slice {
                result.expect("load_url");
                break;
            }
        }
    }
    let ended = Instant::now();
    nav_cancel::LOAD_PHASE_HOOK.with(|hook| *hook.borrow_mut() = None);
    let fired = load_event.get().expect("the load event fired");
    let page_loaded = std::iter::from_fn(|| events.try_recv().ok())
        .any(|e| matches!(e, EngineEvent::PageLoaded { url: u, .. } if u == url));
    Loaded {
        engine,
        view,
        rt,
        server,
        after_load_event: ended - fired,
        page_loaded,
    }
}

fn js(loaded: &mut Loaded, script: &str) -> String {
    loaded.engine.execute_script(loaded.view, script).unwrap()
}

#[test]
fn requests_made_after_load_do_not_hold_the_load() {
    let mut loaded = load(live(), PINGS_AFTER_LOAD);
    assert_eq!(
        js(&mut loaded, "window.loadFired === true"),
        "Boolean(true)"
    );
    assert!(loaded.page_loaded, "PageLoaded was announced");
    // The page is local; only the pings are slow (2 s).
    assert!(
        loaded.after_load_event < Duration::from_millis(500),
        "the load waited for requests its load handler made: {:?}",
        loaded.after_load_event
    );
    assert_eq!(
        js(&mut loaded, "window.got === undefined"),
        "Boolean(true)",
        "answered inside the load"
    );
}

#[test]
fn requests_made_after_load_are_answered_on_later_short_turns() {
    let mut loaded = load(live(), PINGS_AFTER_LOAD);
    // Still out when the load ends: the turns below are what answer them.
    assert!(
        loaded.after_load_event < Duration::from_millis(500),
        "the load answered them: {:?}",
        loaded.after_load_event
    );
    let began = Instant::now();
    let mut longest = Duration::ZERO;
    let mut clock = Instant::now();
    loop {
        let turn_started = Instant::now();
        let elapsed_ms = turn_started.duration_since(clock).as_millis() as u64;
        clock += Duration::from_millis(elapsed_ms);
        let turn = loaded
            .rt
            .block_on(loaded.engine.pump_live(loaded.view, elapsed_ms));
        longest = longest.max(turn_started.elapsed());
        let answered = js(&mut loaded, "window.got === '/beacon-metrics'") == "Boolean(true)";
        if answered && turn.in_flight == 0 {
            break;
        }
        assert!(
            began.elapsed() < Duration::from_secs(8),
            "the pings never completed: {turn:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        longest < Duration::from_millis(200),
        "a live turn held the thread for {longest:?}"
    );
    assert_eq!(js(&mut loaded, "document.title"), r#"String("answered")"#);
    // The page, the pixel and the metrics request.
    assert_eq!(
        loaded.server.hits.load(std::sync::atomic::Ordering::SeqCst),
        3
    );
}

/// What a script asks for before `load` still delays it: an `<img>` the
/// parse-time script appends, and its `fetch`.
#[test]
fn requests_made_before_load_still_hold_it() {
    let page = "<html><body><script>
var img = document.createElement('img'); img.src = '/held-pre.png'; document.body.appendChild(img);
fetch('/slow-early').then(function (r) { return r.text(); }).then(function (t) { window.early = t; });
</script></body></html>";
    let mut loaded = load(live(), page);
    assert_eq!(js(&mut loaded, "window.early"), r#"String("/slow-early")"#);
    assert_eq!(
        loaded.server.hits.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "page, image, fetch"
    );
}

/// A capture keeps waiting: it shows the page once what it asked for has
/// arrived.
#[test]
fn without_the_switch_the_load_waits_as_before() {
    let mut loaded = load(EngineConfig::default(), PINGS_AFTER_LOAD);
    assert_eq!(js(&mut loaded, "window.got"), r#"String("/beacon-metrics")"#);
    assert!(
        loaded.after_load_event >= Duration::from_secs(2),
        "{:?}",
        loaded.after_load_event
    );
}
