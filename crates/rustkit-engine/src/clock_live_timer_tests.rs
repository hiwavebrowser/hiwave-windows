//! A timer a page schedules AFTER the load's timer pass must fire during the live turns that follow,
//! in the deterministic capture mode as in the ordinary one. Squarespace's homepage settles an A/B
//! test with `setTimeout(resolve, max(0, 2000 - performance.now()))` read after load: in the
//! deterministic clock that read is a few tens of ms, the timer is about 1.9 s away, and it never
//! fired in a capture that ran 17 s of live turns (#1104, batch 71).

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// Load `page` under `config`, run `live_ms` of live turns in `step` slices, and read `title`.
fn title_after(config: EngineConfig, page: &str, live_ms: u64, step: u64) -> String {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let r = rt();
    let mut t = 0;
    while t < live_ms {
        r.block_on(engine.pump_live(view, step));
        std::thread::sleep(std::time::Duration::from_millis(2));
        t += step;
    }
    let v = engine.execute_script(view, "String(document.title)").unwrap();
    v.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

/// The reveal gate, as the page writes it: a delay computed from the clock after the load event.
const PAGE: &str = "<html><head><title>hidden</title></head><body><script>\
    window.addEventListener('load', function () {\
      var wait = Math.max(0, 2000 - performance.now());\
      setTimeout(function () { document.title = 'revealed'; }, wait);\
    });\
    </script></body></html>";

fn modes() -> [(&'static str, EngineConfig); 2] {
    [
        ("ordinary clock", EngineConfig::default()),
        ("deterministic capture clock", EngineConfig { disable_animations: true, ..Default::default() }),
    ]
}

#[test]
fn a_timer_set_after_load_fires_in_the_live_turns_after_it() {
    for (mode, config) in modes() {
        assert_eq!(title_after(config, PAGE, 3000, 100), "revealed", "{mode}");
    }
}
