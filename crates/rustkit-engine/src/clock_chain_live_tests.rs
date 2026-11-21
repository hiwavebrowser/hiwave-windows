//! CLOCK3 through the engine: the app's loops turn the page on the real clock
//! (`Engine::pump_live_real`), so a timer set during a turn counts from when
//! it is set and a 10 ms chain held back by a long task does not replay its
//! missed links. The window thread's `PumpLive` is pinned in
//! `page_protocol`'s tests; this is the content thread's loop
//! (`PageHalf::run`, the app's default).
//!
//! Pinned Chrome (CfT 148.0.7778.216, five runs,
//! `docs/diagnostics/2026-11-15/chain_after_long_task_cft148.json`): one link
//! in the 5 ms after a 500 ms task, 28 in the 300 ms after. Bounds are
//! relations that hold at any load: links set from the one before are at
//! least about 10 ms apart, so at most one falls in those 5 ms, and at most
//! 31 in the 300 ms. On the stepped clock the same page replays about 50 in
//! the 5 ms and 80 in the 300 ms.

use std::time::{Duration, Instant};

use crate::load_turns_tests::{html, serve, Load};

const CHAIN_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-15/chain_after_long_task.html");

/// `rec` once the page has set `rec.done`.
fn finished_rec(load: &mut Load) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (answer, _) = load.eval(
            "window.rec && rec.done ? JSON.stringify(rec) : ''",
            Duration::from_secs(10),
        );
        let answer = answer.unwrap_or_default();
        let text = answer
            .strip_prefix("String(\"")
            .and_then(|s| s.strip_suffix("\")"))
            .unwrap_or_default()
            .replace("\\\"", "\"");
        if !text.is_empty() {
            return serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        }
        assert!(Instant::now() < deadline, "the page did not finish");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn the_content_thread_does_not_replay_a_timeout_chain_after_a_long_task() {
    let fixture = serve(vec![("/", html(CHAIN_PAGE))]);
    // Started after the load: the load's pass runs timers up to 5 s ahead of real time, and the
    // whole page would run in it (#manual keeps it from starting at the load event).
    let mut load = Load::start(&fixture, "/#manual");
    load.wait_complete();
    let (started, _) = load.eval("go(); 'started'", Duration::from_secs(10));
    assert!(
        started.unwrap_or_default().contains("started"),
        "go() did not run"
    );
    let rec = finished_rec(&mut load);
    eprintln!("CLOCK3-ENGINE {rec}");
    let n = |name: &str| {
        rec[name]
            .as_u64()
            .unwrap_or_else(|| panic!("{name}: {rec}"))
    };
    assert!(n("longMs") >= 500, "{rec}");
    assert!(
        n("chainBurst") <= 1,
        "chain links replayed after the long task: {rec}"
    );
    assert!(
        (1..=31).contains(&n("chainAfter")),
        "chain links in the 300 ms after: {rec}"
    );
}

/// Measurement only (CLOCK3 R1 item 1): pages through the engine's live turn on each clock, paced
/// as the content thread paces (the next timer, 2 to 50 ms), until `rec.done` or 5 s. Prints the
/// page's `rec` with the turns and relayouts the loop counted, three runs per clock.
#[test]
#[ignore]
fn measure_live_pages_on_each_clock() {
    use crate::script_net_tests::serve_routes;
    use crate::{Bounds, Engine, EngineConfig};
    const PAGES: [(&str, &str); 2] = [
        (
            "chunked_dom_work",
            include_str!("../../../docs/diagnostics/2026-11-15/chunked_dom_work.html"),
        ),
        (
            "zero_delay_chain",
            include_str!("../../../docs/diagnostics/2026-11-15/zero_delay_chain.html"),
        ),
    ];
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for (name, page) in PAGES {
        for real in [false, true] {
            for _ in 0..3 {
                let server = serve_routes(vec![("/", "text/html", page.to_string())]);
                let mut engine = Engine::new(EngineConfig::default()).expect("engine");
                let view = engine
                    .create_headless_view(Bounds {
                        x: 0,
                        y: 0,
                        width: 800,
                        height: 600,
                    })
                    .expect("view");
                let url =
                    url::Url::parse(&format!("http://127.0.0.1:{}/#manual", server.port)).unwrap();
                rt.block_on(engine.load_url(view, url)).expect("load_url");
                // The turn after the load, then the page starts (see the content thread test).
                rt.block_on(engine.pump_live(view, 0));
                engine.execute_script(view, "go()").unwrap();
                let began = Instant::now();
                let mut clock = began;
                let (mut turns, mut relayouts, mut timers) = (0u32, 0u32, 0u64);
                let rec = loop {
                    let elapsed_ms = clock.elapsed().as_millis() as u64;
                    clock += Duration::from_millis(elapsed_ms);
                    let out = if real {
                        rt.block_on(engine.pump_live_real(view, elapsed_ms))
                    } else {
                        rt.block_on(engine.pump_live(view, elapsed_ms))
                    };
                    turns += 1;
                    relayouts += out.relaid_out as u32;
                    timers += out.timers_ran as u64;
                    let v = engine
                        .execute_script(view, "window.rec && rec.done ? JSON.stringify(rec) : ''")
                        .unwrap();
                    let text = v
                        .strip_prefix("String(\"")
                        .and_then(|s| s.strip_suffix("\")"))
                        .unwrap_or_default()
                        .replace("\\\"", "\"");
                    if !text.is_empty() || began.elapsed() > Duration::from_secs(5) {
                        break text;
                    }
                    std::thread::sleep(Duration::from_millis(
                        out.next_timer_ms.unwrap_or(50).clamp(2, 50),
                    ));
                };
                eprintln!(
                    "CLOCK3-MEASURE {name} real={real} turns={turns} relayouts={relayouts} timers={timers} wall_ms={} rec={rec}",
                    began.elapsed().as_millis()
                );
            }
        }
    }
}
