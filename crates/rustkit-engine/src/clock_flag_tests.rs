//! The page clock's capture mode (a clock with no real time in it) has its own configuration switch,
//! `deterministic_page_clock`. It rode on `disable_animations`, so a run that only wanted animations
//! off (parity-capture `--live-ms`) got a stepped clock too (R1 LOW on #1104).

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// Load a page under `config` and return `performance.now()` read twice with 40 ms of real time
/// between the reads.
fn two_reads_40ms_apart(config: EngineConfig) -> (f64, f64) {
    let server = serve_routes(vec![("/", "text/html", "<html><body></body></html>".to_string())]);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let read = |engine: &mut Engine| -> f64 {
        let v = engine.execute_script(view, "String(performance.now())").unwrap();
        v.trim_start_matches("String(\"").trim_end_matches("\")").parse().unwrap_or_else(|_| panic!("{v}"))
    };
    let a = read(&mut engine);
    std::thread::sleep(std::time::Duration::from_millis(40));
    let b = read(&mut engine);
    (a, b)
}

#[test]
fn animations_off_alone_leaves_the_page_clock_flowing() {
    let (a, b) = two_reads_40ms_apart(EngineConfig { disable_animations: true, ..Default::default() });
    assert!(b - a >= 35.0, "performance.now moved {} ms across 40 ms of real time", b - a);
}

#[test]
fn the_capture_flag_steps_the_page_clock_by_reads_not_by_real_time() {
    let (a, b) = two_reads_40ms_apart(EngineConfig { deterministic_page_clock: true, ..Default::default() });
    assert!(b - a < 5.0, "performance.now moved {} ms across 40 ms of real time", b - a);
}
