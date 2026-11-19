//! PACE1: after one long turn the content thread's loop runs the next frame
//! as soon as it is due, as Chrome does.
//!
//! The page (`docs/diagnostics/2026-11-15/long_turn_resume.html`) runs a
//! requestAnimationFrame loop and a 10 ms timer chain, makes one frame run
//! for 2000 ms, and records how long the next frame and timer took to come
//! and how many came in the second after. Chrome for Testing 148.0.7778.216,
//! three runs (`long_turn_resume_cft148.json`): the first frame and the
//! first timer 0-1 ms after the long frame; 60-61 frames and 93-94 timers in
//! the second after.
//!
//! The window thread's loop (content thread off, `hiwave-app`
//! `process_events`) waits at least as long as the last turn took, so there
//! the next frame comes about 2000 ms later. That wait belongs to the
//! window's thread only; the content thread's loop is the app's default
//! (`HIWAVE_CONTENT_THREAD=1`, set by `hiwave-app` at startup) and has no
//! such wait. The bounds are wall-clock bounds with wide margins: the next
//! frame and timer within a quarter of the long frame (500 ms; measured
//! 1-3 ms, `long_turn_resume_engine.json`), and at least 10 frames and
//! timers in the second after (measured 35-39 and 295-297).

use std::time::{Duration, Instant};

use crate::load_turns_tests::{html, serve, Load};

pub(crate) const LONG_TURN_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-15/long_turn_resume.html");

fn field(rec: &serde_json::Value, name: &str) -> i64 {
    rec[name]
        .as_i64()
        .unwrap_or_else(|| panic!("{name} missing: {rec}"))
}

#[test]
fn after_a_long_frame_the_content_thread_runs_the_next_frame_when_it_is_due() {
    let fixture = serve(vec![("/", html(LONG_TURN_PAGE))]);
    let mut load = Load::start(&fixture, "/");
    load.wait_complete();
    let deadline = Instant::now() + Duration::from_secs(30);
    let rec = loop {
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
            break serde_json::from_str::<serde_json::Value>(&text)
                .unwrap_or_else(|e| panic!("{e}: {text}"));
        }
        assert!(Instant::now() < deadline, "the page did not finish");
        std::thread::sleep(Duration::from_millis(200));
    };
    // One line per run for `long_turn_resume_engine.py`.
    eprintln!(
        "PACE1-RESULT {}",
        serde_json::json!({ "arm": "app-content-thread", "rec": rec })
    );
    let long = field(&rec, "longMs");
    assert!(long >= 2000, "{rec}");
    // Chrome: 0-1 ms. The window thread's loop would wait about `long`.
    assert!(
        field(&rec, "firstFrameAfterMs") * 4 < long,
        "the next frame waited on the long one: {rec}"
    );
    assert!(
        field(&rec, "firstTimerAfterMs") * 4 < long,
        "the next timer waited on the long one: {rec}"
    );
    // Chrome: 60-61 frames and 93-94 timers in the second after.
    assert!(field(&rec, "afterFrames") >= 10, "{rec}");
    assert!(field(&rec, "afterTimers") >= 10, "{rec}");
}
