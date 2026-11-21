//! Row ANIM-READ-JUMP: listing a view's animations (`getAnimations()`) mid-transition
//! moved them on. Found on #1330: a `getAnimations()` read at 50 ms made the next
//! sample, at 75 ms, read the end value, on HTML transitions too. A listing is a read;
//! in Chrome it moves nothing.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/anim_read_jump_log.mjs`, receipt
//! `docs/diagnostics/2026-11-19/anim_read_jump_cft148.json`, which this file reads: per
//! case (a 100 ms linear transition of width, color, opacity, transform, and a width
//! `@keyframes` animation) the computed value and the `getAnimations()` listing at 25,
//! 50 and 75 ms (Chrome paused and seeked), and the end value. Here the page's virtual
//! clock moves in 5 ms steps (`pump_live`), as in `css_transitions_svg_paint_tests`.
//! Each case runs twice: reading only the value, and reading `getAnimations()` (and the
//! listed `currentTime`) before each value. Both must match Chrome. A listing asks for an
//! animation frame (to settle the wrappers' state), and once a frame of the turn's time
//! has run, a task reads the animations at that frame's time (TIMELINE2, as Chrome's
//! tasks read the timeline of the last frame): with listings, a sample may be up to one
//! 60 Hz frame (16.7 ms) behind its time. The jump this row fixes went to the end.

use super::*;
use crate::script_net_tests::serve_routes;

const RECEIPT: &str =
    include_str!("../../../docs/diagnostics/2026-11-19/anim_read_jump_cft148.json");

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let value = engine
        .execute_script(view, &format!("String({expr})"))
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn pump(engine: &mut Engine, view: EngineViewId, rt: &tokio::runtime::Runtime, total: u64) {
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, 5));
        t += 5;
    }
}

/// The numbers in a value, for `close`.
fn numbers(value: &str) -> Vec<f64> {
    value
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .filter_map(|n| n.parse().ok())
        .collect()
}

/// `got` is `want`, or behind it (a value that only rises here) by at most `behind`, or
/// ahead by at most `ahead`: one-sided, so a jump ahead never passes as lag.
fn close(got: &str, want: &str, behind: f64, ahead: f64) -> bool {
    let (g, w) = (numbers(got), numbers(want));
    got.split(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
        .collect::<String>()
        == want
            .split(|c: char| c.is_ascii_digit() || c == '.' || c == '-')
            .collect::<String>()
        && g.len() == w.len()
        && g.iter()
            .zip(&w)
            .all(|(a, b)| *a >= b - behind && *a <= b + ahead)
}

/// The load "took" 100 s of real time (`__rk.testRealMs`, a host hook only tests install):
/// the real clock is far ahead of the virtual one the stepped turns move, and stands still.
/// A timeline taken from the real clock (the bug) is then 100 s ahead, never a few ms that a
/// fast machine could hide.
fn slow_load(engine: &mut Engine, view: EngineViewId) {
    engine
        .execute_host_turn(
            view,
            "__rk.testRealMs = function () { return 100000; }; return 0;",
            &[],
        )
        .expect("test real clock");
}

/// One case, sampled with or without `getAnimations()` reads. Returns what differs
/// from Chrome.
fn run(case: &serde_json::Value, list_too: bool) -> Vec<String> {
    let css = case["css"].as_str().unwrap();
    let prop = case["prop"].as_str().unwrap();
    let page = format!("<!doctype html><style>body{{margin:0}}{css}</style><div id=d></div>");
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        })
        .expect("view");
    let rt = rt();
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    slow_load(&mut engine, view);
    let value =
        format!("getComputedStyle(document.getElementById('d')).getPropertyValue('{prop}')");
    // The style read before and after the class lands starts the transition at the
    // clock as it stands.
    read(
        &mut engine,
        view,
        &format!("({value}, document.getElementById('d').classList.add('on'), {value})"),
    );
    let mode = if list_too {
        "with getAnimations()"
    } else {
        "value only"
    };
    let mut failures = Vec::new();
    // A value moves by 1% of its range (the end value here, from 0) a millisecond. Value
    // only, a sample is on its time within one 5 ms clock step either way; with listings,
    // a task reads the last frame's time, so it may lag by one frame (17 ms), never lead.
    let range = numbers(case["end"].as_str().unwrap())
        .iter()
        .fold(1.0_f64, |m, n| m.max(n.abs()));
    let behind_ms = if list_too { 17.0 } else { 5.0 };
    let (behind, ahead) = (range * behind_ms / 100.0 + 0.01, range * 5.0 / 100.0 + 0.01);
    for t in ["25", "50", "75"] {
        pump(&mut engine, view, &rt, 25);
        if list_too {
            let listed = read(
                &mut engine,
                view,
                "document.getElementById('d').getAnimations().map(function(a){return \
                 (a.transitionProperty?'transition:'+a.transitionProperty:'animation:'+a.animationName)\
                 +'@'+Math.round(a.currentTime)}).join()",
            );
            let want = case["listed"][t].as_str().unwrap();
            if !close(&listed, want, behind_ms, 5.0) {
                failures.push(format!(
                    "{mode}: listed at {t} ms {listed:?}, Chrome {want:?}"
                ));
            }
        }
        let got = read(&mut engine, view, &value);
        let want = case["at"][t].as_str().unwrap();
        if !close(&got, want, behind, ahead) {
            failures.push(format!("{mode}: {prop} at {t} ms {got:?}, Chrome {want:?}"));
        }
    }
    pump(&mut engine, view, &rt, 100);
    let end = read(&mut engine, view, &value);
    let want = case["end"].as_str().unwrap();
    if !close(&end, want, 0.01, 0.01) {
        failures.push(format!("{mode}: end {end:?}, Chrome {want:?}"));
    }
    drop(server);
    failures
}

fn check(label: &str) {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let case = receipt["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("no case {label:?}"));
    let mut failures = run(case, false);
    failures.extend(run(case, true));
    assert!(failures.is_empty(), "{label}:\n  {}", failures.join("\n  "));
}

#[test]
fn a_width_transition_is_not_moved_on_by_a_listing() {
    check("width transition");
}

#[test]
fn a_color_transition_is_not_moved_on_by_a_listing() {
    check("color transition");
}

#[test]
fn an_opacity_transition_is_not_moved_on_by_a_listing() {
    check("opacity transition");
}

#[test]
fn a_transform_transition_is_not_moved_on_by_a_listing() {
    check("transform transition");
}

#[test]
fn a_keyframes_animation_is_not_moved_on_by_a_listing() {
    check("width animation");
}

/// #1359 R1, a known divergence, pinned: Chrome 148 keeps a frame's `t` equal to
/// `document.timeline.currentTime` (the receipt's `raf_t_is_timeline`). In a turn that is not
/// a real one the timeline is the grid time on the timers' clock (the fix), while the
/// callback's stamp stays on `performance.now()`'s clock: the clock-parity pins
/// (`clock_parity_tests`, `frame_clock_tests`) hold that relation, and stamping the callback
/// on the grid too broke six of them. So after a load whose real time is 100 s ahead, `t` is
/// about 100 s and the timeline the grid time. Both still move a grid step a frame. A
/// capture with grid frame stamps already had this split (TIMELINE1,
/// `timeline_tests::known_divergence_in_a_capture_...`).
#[test]
fn known_divergence_a_stepped_frames_t_is_on_performance_nows_clock() {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert!(receipt["raf_t_is_timeline"]
        .as_array()
        .unwrap()
        .iter()
        .all(|v| v.as_bool() == Some(true)));
    let page = "<!doctype html><div id=d></div><script>window.seen=[];</script>".to_string();
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 400,
            height: 200,
        })
        .expect("view");
    let rt = rt();
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    slow_load(&mut engine, view);
    read(
        &mut engine,
        view,
        "(function step(t){if(t!==undefined)seen.push(t+'/'+document.timeline.currentTime);\
         if(seen.length<5)requestAnimationFrame(step);})()",
    );
    pump(&mut engine, view, &rt, 120);
    let seen = read(&mut engine, view, "seen.join()");
    let frames: Vec<(f64, f64)> = seen
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|f| {
            let (t, tl) = f.split_once('/').unwrap();
            (t.parse().unwrap(), tl.parse().unwrap())
        })
        .collect();
    assert_eq!(frames.len(), 5, "five frames ran: {seen}");
    for w in frames.windows(2) {
        for (a, b) in [(w[0].0, w[1].0), (w[0].1, w[1].1)] {
            let step = b - a;
            assert!((15.0..=18.0).contains(&step), "a grid step a frame: {seen}");
        }
    }
    for (t, tl) in &frames {
        assert!(*t > 99_000.0, "t on performance.now()'s clock: {seen}");
        assert!(*tl < 1_000.0, "the timeline on the timers' grid: {seen}");
    }
    drop(server);
}
