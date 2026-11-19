//! TIMELINE2: CSS animations and transitions on the document timeline, the one script animations
//! run on (TIMELINE1). Pinned on Chrome for Testing 148.0.7778.216, headless,
//! `docs/diagnostics/2026-11-16/css_timeline_probe_cft148.json` (5 runs):
//! - In every frame and every task a running CSS animation's `startTime + currentTime` is
//!   `document.timeline.currentTime` (0 in 25 of 25 frame rows and 15 of 15 task rows), and its
//!   `startTime` stays put.
//! - In a frame a long callback held up 100 ms, the same holds, and the painted value is the one
//!   for that time (`currentTime - opacity * 10000` = 0 in 25 of 25 rows).
//! - In a frame document inserted later than its parent (time origin 1850-1939 ms later), the
//!   start time is on the frame's own timeline (11-15 ms there, not the parent's), and a script
//!   animation given the CSS animation's start time runs at the same current time (0 in 15 of 15).

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn load_with(config: EngineConfig, routes: Vec<(&'static str, &'static str, String)>) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(routes);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
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

fn run(engine: &mut Engine, view: EngineViewId, script: &str) {
    engine.execute_script(view, script).unwrap();
}

/// Live turns of `step` ms until `total` ms have passed (the window loop).
fn pump(engine: &mut Engine, view: EngineViewId, total: u64, step: u64) {
    let rt = rt();
    let mut t = 0;
    while t < total {
        rt.block_on(engine.pump_live(view, step));
        t += step;
    }
}

/// Live turns of `step` ms until `done` reads true, at most `turns` of them.
fn pump_until(engine: &mut Engine, view: EngineViewId, done: &str, turns: usize, step: u64) -> bool {
    let rt = rt();
    for _ in 0..turns {
        if read(engine, view, done) == "true" {
            return true;
        }
        rt.block_on(engine.pump_live(view, step));
    }
    read(engine, view, done) == "true"
}

const FADE: &str = "@keyframes fade{from{opacity:0}to{opacity:1}}";

fn page() -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}#h{{width:20px;height:20px;background:#123;animation:fade 10s linear}}{FADE}</style>\
         <div id=h></div><script>var h=document.getElementById('h');\
         function tl(){{return document.timeline.currentTime}}</script>"
    )
}

/// Numbers from a comma-joined reading.
fn numbers(got: &str) -> Vec<f64> {
    got.split(',')
        .map(|v| v.parse::<f64>().unwrap_or_else(|_| panic!("{got}")))
        .collect()
}

/// In a timer task the timeline is the time of the task's frame, and a running CSS animation's
/// `startTime + currentTime` is that time, its `startTime` the same in every task (Chrome
/// "inTask": 0 in 15 of 15 rows).
#[test]
fn in_a_task_a_css_animation_is_on_the_document_timeline() {
    let (mut engine, view, _server) = load_with(EngineConfig::default(), vec![("/", "text/html", page())]);
    pump(&mut engine, view, 100, 20);
    run(
        &mut engine,
        view,
        "window.got = []; var c = h.getAnimations()[0]; (function f() { setTimeout(function () { \
           got.push(c.startTime + c.currentTime - tl(), c.startTime); if (got.length < 6) f(); }, 37); })();",
    );
    assert!(pump_until(&mut engine, view, "got.length >= 6", 40, 20), "{}", read(&mut engine, view, "got"));
    let got = numbers(&read(&mut engine, view, "got"));
    for row in got.chunks(2) {
        assert!(row[0].abs() < 0.01, "startTime + currentTime - timeline: {got:?} (Chrome: 0)");
        assert_eq!(row[1], got[1], "the start time moved: {got:?}");
    }
}

/// A frame the turn ran late, on the deterministic clock: the timeline reads the frame's time,
/// and the CSS animation's `startTime + currentTime` is that time (was: the turn's), its
/// painted value the one for that time (Chrome "late": 0 and 0 in 25 of 25 rows).
#[test]
fn in_a_late_frame_a_css_animation_is_on_the_document_timeline_and_paints_its_time() {
    late_frame_case(false);
}

/// The same with one animation round per turn (RAF-ONE-ROUND on), where the late frame is the
/// frame of the turn's time (CLOCK-AT-PRESENT): the animations are updated to it before its
/// callbacks run, so a computed read in the callback is that time's value (ANIM-UPDATE-ORDER;
/// Chrome `clock_at_present_probe_cft148.json` "frame": `currentTime - opacity * 10000` = 0 in
/// 100 of 100 late frames). Was: the last turn's layout, 34 ms behind.
#[test]
fn in_a_late_frame_a_css_animation_is_on_the_document_timeline_and_paints_its_time_with_one_round_a_turn() {
    late_frame_case(true);
}

fn late_frame_case(one_round: bool) {
    crate::set_one_animation_round(one_round);
    let config = EngineConfig { deterministic_page_clock: true, ..EngineConfig::default() };
    let (mut engine, view, _server) = load_with(config, vec![("/", "text/html", page())]);
    pump(&mut engine, view, 100, 10);
    run(
        &mut engine,
        view,
        "window.got = ''; var c = h.getAnimations()[0]; requestAnimationFrame(function (t) { \
           got = [t === tl(), c.startTime + c.currentTime - tl(), c.currentTime - parseFloat(getComputedStyle(h).opacity) * 10000].join(); });",
    );
    pump(&mut engine, view, 40, 40);
    let got = read(&mut engine, view, "got");
    let (same, rest) = got.split_once(',').unwrap_or_else(|| panic!("{got}"));
    assert_eq!(same, "true", "{got}");
    let rest = numbers(rest);
    assert!(rest[0].abs() < 0.01, "startTime + currentTime - timeline: {got} (Chrome: 0)");
    assert!(rest[1].abs() < 0.5, "currentTime - the painted opacity's time: {got} (Chrome: 0)");
}

const CHILD: &str = "<!doctype html><style>@keyframes fade{from{opacity:0}to{opacity:1}}\
    #a{width:10px;height:10px;animation:fade 10s linear}#s{width:10px;height:10px}</style>\
    <div id=a></div><div id=s></div><script>function tl(){return document.timeline.currentTime}\
    window.go = function () { var a = document.getElementById('a'), s = document.getElementById('s');\
      var c = a.getAnimations()[0]; window.rows = []; window.start0 = c.startTime;\
      (function f() { requestAnimationFrame(function () {\
        rows.push(c.startTime + c.currentTime - tl(), c.startTime - tl());\
        if (rows.length < 6) f(); else sync(); }); })();\
      function sync() { var sa = s.animate({opacity:[0,1]}, {duration:10000}); sa.startTime = c.startTime;\
        (function f() { requestAnimationFrame(function () {\
          rows.push(sa.currentTime - c.currentTime, c.startTime - tl());\
          if (rows.length < 12) f(); }); })(); } };</script>";

/// A frame document inserted after its parent's start counts from its own time origin:
/// its CSS animation's start time is on the frame's timeline, never ahead of it, and a script
/// animation given that start time runs at the CSS animation's current time (Chrome "child": 0
/// in 15 of 15 rows; start times 11-15 ms on the frame's timeline, origin 1850-1939 ms later).
/// Was: the frame's CSS start time was 0 on the tree's clock, before the frame was made, so it
/// read 0 on the frame's timeline and painted the frame's origin ahead of it.
/// On the deterministic page clock, so neither the origin nor the start depends on how fast the
/// machine is (R1: the wall-clock bounds failed on a fast or a slow machine).
#[test]
fn in_a_frame_document_a_css_animation_is_on_the_frames_own_timeline() {
    frame_document_case(false);
}

/// The same with one animation round per turn (RAF-ONE-ROUND on; CLOCK-AT-PRESENT): the frame
/// document's painted time is the presented frame's.
#[test]
fn in_a_frame_document_a_css_animation_is_on_the_frames_own_timeline_with_one_round_a_turn() {
    frame_document_case(true);
}

fn frame_document_case(one_round: bool) {
    crate::set_one_animation_round(one_round);
    let parent = "<!doctype html><body><script>window.ready = false;\
        setTimeout(function () { var f = document.createElement('iframe'); f.id = 'f';\
          f.onload = function () { window.ready = true; }; f.src = '/child.html'; document.body.appendChild(f); }, 300);</script></body>";
    let config = EngineConfig {
        nested_contexts: true,
        post_load_requests_live: true,
        deterministic_page_clock: true,
        ..EngineConfig::default()
    };
    let (mut engine, view, _server) = load_with(
        config,
        vec![("/", "text/html", parent.to_string()), ("/child.html", "text/html", CHILD.to_string())],
    );
    assert!(pump_until(&mut engine, view, "window.ready", 100, 20), "the frame did not load");
    // Its origin is later than the parent's: at one instant the two timelines differ by it, at
    // least the 300 ms the page waited before inserting the frame.
    let origin = read(&mut engine, view, "document.timeline.currentTime - f.contentDocument.timeline.currentTime");
    assert!(origin.parse::<f64>().unwrap() >= 300.0, "the frame's origin is later: {origin}");
    run(&mut engine, view, "f.contentWindow.go()");
    assert!(
        pump_until(&mut engine, view, "f.contentWindow.rows.length >= 12", 60, 20),
        "{}",
        read(&mut engine, view, "f.contentWindow.rows")
    );
    // The start time is on the frame's timeline, within its first frame, not the parent's time
    // (the start time cancels out of the rows below, so it is read on its own). Chrome 11-15 ms,
    // the frame's first frame: there a CSS animation is pending until then (TIMELINE2b); here it
    // starts at the frame's first layout, its origin.
    let start = read(&mut engine, view, "f.contentWindow.start0");
    let start: f64 = start.parse().unwrap_or_else(|_| panic!("{start}"));
    assert!((0.0..17.0).contains(&start), "the CSS start time on the frame's timeline: {start} (Chrome 11-15)");
    // ... and after each turn the frame shows the value for the time its animation reports (was:
    // its start was the tree's clock zero, before the frame was made, so it painted ~200 ms
    // ahead). Read after the turn, as a capture paints it: inside a frame callback a computed read
    // answers from the last turn's layout here (Chrome updates animations before the callbacks).
    let paint_expr = "f.contentWindow.document.getElementById('a').getAnimations()[0].currentTime - \
        parseFloat(f.contentWindow.getComputedStyle(f.contentDocument.getElementById('a')).opacity) * 10000";
    let mut paint = Vec::new();
    for _ in 0..3 {
        pump(&mut engine, view, 20, 20);
        paint.push(read(&mut engine, view, paint_expr).parse::<f64>().unwrap());
    }
    for p in &paint {
        assert!(p.abs() < 0.5, "currentTime - the painted opacity's time: {paint:?} (Chrome: 0)");
    }
    let rows = numbers(&read(&mut engine, view, "f.contentWindow.rows"));
    for (i, pair) in rows.chunks(2).enumerate() {
        assert!(pair[0].abs() < 0.01, "row {i}: {rows:?} (Chrome: 0)");
        assert!(pair[1] <= 0.0, "row {i}: the start time is ahead of the frame's timeline: {rows:?}");
    }
}

fn det_page(extra_css: &str, script: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let html = format!(
        "<!doctype html><style>body{{margin:0}}#h{{width:20px;height:20px;background:#123;animation:fade 10s linear}}{FADE}{extra_css}</style>\
         <div id=h></div><div id=c></div><script>var h=document.getElementById('h'), cEl=document.getElementById('c');\
         function tl(){{return document.timeline.currentTime}}{script}</script>"
    );
    let config = EngineConfig { deterministic_page_clock: true, ..EngineConfig::default() };
    load_with(config, vec![("/", "text/html", html)])
}

/// The time the hero's opacity was painted for, from the last turn's layout (10 s linear fade).
fn painted_ms(engine: &mut Engine, view: EngineViewId) -> f64 {
    let op = read(engine, view, "parseFloat(getComputedStyle(h).opacity)");
    op.parse::<f64>().unwrap_or_else(|_| panic!("{op}")) * 10000.0
}

/// R1 (1): the animation clock never steps back between turns. A turn with no frame of its grid
/// point runs the animations at the timers' clock (35, say); a timer at 40 then asks for a frame,
/// the frame of grid point 34 runs at once, stamped ~34, and the next turn's clock was that stamp:
/// computed style and paint went back up to a frame, and an end or iteration boundary could be
/// crossed twice. The clock keeps its high-water mark.
#[test]
fn the_animation_clock_never_steps_back_between_turns() {
    let (mut engine, view, _server) =
        det_page(
            "",
            "setTimeout(function () { setInterval(function () { requestAnimationFrame(function () {}); }, 50); }, 7);",
        );
    pump(&mut engine, view, 30, 10);
    let mut last = painted_ms(&mut engine, view);
    let mut seen = vec![last];
    // 3 ms turns: most pass a grid point with no frame asked for; every 50 ms, off the grid, a
    // timer asks for one, which runs at once as the frame of the grid point before it.
    for _ in 0..120 {
        pump(&mut engine, view, 3, 3);
        let now = painted_ms(&mut engine, view);
        seen.push(now);
        assert!(now >= last - 0.05, "the painted time went back: {seen:?}");
        last = now;
    }
}

/// R1 (3): in a stepped capture (`parity-capture --animations`: grid frame stamps) with animations on,
/// a running CSS animation paints the timeline's time: once a frame has run, that frame's exact
/// grid time (17, 34, 50, 67, ... ms), the time script animations read, not the step's. With no
/// animation frame asked for, the engine's own frame paints the step's time, less than one
/// frame past the timeline (which stays on the grid point at or before it).
#[test]
fn in_a_stepped_capture_a_css_animation_paints_the_timelines_grid_time() {
    let grid = |t: f64| (0..2000u32).any(|k| ((k * 50 + 2) / 3) as f64 == t);
    for frames in [true, false] {
        let script = if frames { "(function f() { requestAnimationFrame(f); })();" } else { "" };
        let html = format!(
            "<!doctype html><style>#h{{width:20px;height:20px;animation:fade 10s linear}}{FADE}</style>\
             <div id=h></div><script>var h=document.getElementById('h');{script}</script>"
        );
        let config = EngineConfig { grid_frame_stamps: true, ..EngineConfig::default() };
        let (mut engine, view, _server) = load_with(config, vec![("/", "text/html", html)]);
        for _ in 0..4 {
            pump(&mut engine, view, 50, 50);
            let got = numbers(&read(
                &mut engine,
                view,
                "[parseFloat(getComputedStyle(h).opacity) * 10000 + h.getAnimations()[0].startTime, document.timeline.currentTime].join()",
            ));
            let (painted, timeline) = (got[0], got[1]);
            assert!(grid(timeline), "the timeline is on the grid: {got:?}");
            if frames {
                assert!((painted - timeline).abs() < 0.05, "a frame ran: the painted time is the frame's: {got:?}");
            } else {
                assert!(painted >= timeline - 0.05 && painted < timeline + 16.7, "no frame: the step's time: {got:?}");
            }
        }
    }
}

/// KNOWN DIVERGENCE, row ANIM-UPDATE-ORDER (R1 (4)): inside a timer task Chrome's computed style
/// is the value for the time the animation's `currentTime` reports (`css_timeline_probe`
/// "inTask": `currentTime - opacity * 10000` = 0 in 25 of 25 rows): its style update samples the
/// animations at the timeline's time. Here a computed read answers from the last turn's layout,
/// at that turn's animation clock, while `currentTime` is the task's timeline time.
#[test]
fn known_divergence_in_a_task_a_computed_read_is_the_last_turns_value() {
    let (mut engine, view, _server) = det_page(
        "",
        "window.got = []; var c = h.getAnimations()[0]; (function f() { setTimeout(function () { \
           got.push(c.currentTime - parseFloat(getComputedStyle(h).opacity) * 10000); if (got.length < 5) f(); }, 37); })();",
    );
    assert!(pump_until(&mut engine, view, "got.length >= 5", 40, 20), "{}", read(&mut engine, view, "got"));
    let got = numbers(&read(&mut engine, view, "got"));
    assert!(got.iter().any(|g| g.abs() > 0.5), "{got:?}: the divergence is gone, flip this test (Chrome: all 0)");
}

/// KNOWN DIVERGENCE, row TIMELINE2b (R1 (5)): a transition a timer task starts is listed at once
/// in Chrome, pending, with a null start time, and stays so through the next frame's callbacks;
/// it starts at that frame's time (`css_timeline_probe` "trans" and "start": 50 of 50 rows,
/// `atOnce` [pending, startTime] = [true, null], `next` still pending). Here the task does not
/// see it at all, nor does the next frame's callback (`getAnimations()` is empty until the turn's
/// layout makes it); after the turn it is started, not pending, at the turn's time.
#[test]
fn known_divergence_a_transition_started_in_a_task_is_not_pending() {
    let (mut engine, view, _server) = det_page(
        "#c{width:10px;height:10px;opacity:0;transition:opacity 10s linear}#c.go{opacity:1}",
        "window.got = ''; function state() { var t = cEl.getAnimations()[0]; return t ? t.pending + ' ' + (t.startTime === null) : 'none'; }\
         setTimeout(function () { cEl.classList.add('go'); getComputedStyle(cEl).opacity; var atOnce = state();\
           requestAnimationFrame(function () { got = atOnce + ',' + state(); }); }, 30);",
    );
    assert!(pump_until(&mut engine, view, "got !== ''", 20, 20), "the task did not run");
    pump(&mut engine, view, 20, 20);
    let got = format!("{},{}", read(&mut engine, view, "got"), read(&mut engine, view, "state()"));
    // In the task, in the next frame's callbacks, and after the turn: [pending, startTime null].
    assert_eq!(got, "none,none,false false", "Chrome: 'true true,true true', then started at the next frame's time");
}
