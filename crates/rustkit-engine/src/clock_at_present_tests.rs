//! CLOCK-AT-PRESENT: with one animation round per turn (RAF-ONE-ROUND, `set_one_animation_round`),
//! a turn that runs late presents the frame of the turn's time: its callbacks are stamped with
//! that frame's time, and between turns the document timeline and the CSS animation clock hold at
//! it. Pinned on Chrome for Testing 148.0.7778.216, headless,
//! `docs/diagnostics/2026-11-17/clock_at_present_probe_cft148.json` (5 runs, frames whose callback
//! works 25 ms, top document and a frame document inserted after load): in every late frame
//! `timeline.currentTime - t` = 0 and the CSS animation's `startTime + currentTime - t` = 0 and
//! `currentTime - opacity * 10000` = 0; in a task after it, `timeline.currentTime - t` = 0,
//! `startTime + currentTime - timeline` = 0, `currentTime - opacity * 10000` = 0 (200 of 200 rows).
//!
//! Before: a late round's callback asked for the frame of the turn's grid point, due already, which
//! ran on the next turn stamped with that older grid point (a turn behind the frame presented
//! with it), and the timeline a task read after the turn was the turn's grid point, ahead of the
//! last stamp; a frame document's CSS clock read the timers' clock while that frame waited.
//! Deterministic page clock: stamps are grid times, no wall-clock reading is asserted.

use super::*;
use crate::script_net_tests::serve_routes;

struct Doc {
    engine: Engine,
    view: EngineViewId,
    rt: tokio::runtime::Runtime,
    _server: crate::script_net_tests::Server,
}

const FADE: &str = "<style>@keyframes fade{from{opacity:0}to{opacity:1}}#a{width:10px;height:10px;animation:fade 10s linear}</style>";

/// A CSS animation and a rAF loop recording its stamps, in the document whose window `w()` is.
fn looped(body: &str) -> String {
    format!("<!doctype html>{FADE}<div id=a></div><script>window.stamps = [];
function tick(t) {{ stamps.push(t); requestAnimationFrame(tick); }}
{body}</script>")
}

impl Doc {
    fn load(page: String, child: String) -> Self {
        Self::load_switched(page, child, true)
    }

    /// With one round a turn (`on`) or not, on this thread from the start.
    fn load_switched(page: String, child: String, on: bool) -> Self {
        let server = serve_routes(vec![("/", "text/html", page), ("/child.html", "text/html", child)]);
        let config = EngineConfig {
            nested_contexts: true,
            post_load_requests_live: true,
            deterministic_page_clock: true,
            ..EngineConfig::default()
        };
        crate::set_one_animation_round(on);
        let mut engine = Engine::new(config).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        let mut doc = Doc { engine, view, rt, _server: server };
        for _ in 0..100 {
            if doc.read("window.ready") == "true" {
                break;
            }
            doc.turn(20);
        }
        assert_eq!(doc.read("window.ready"), "true", "the page is ready");
        doc
    }

    fn turn(&mut self, ms: u64) {
        self.rt.block_on(self.engine.pump_live(self.view, ms));
    }

    fn read(&mut self, expr: &str) -> String {
        let v = self.engine.execute_script(self.view, &format!("String({expr})")).unwrap();
        v.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&v).to_string()
    }

    fn nums(&mut self, expr: &str) -> Vec<f64> {
        let v = self.read(expr);
        v.split(',').map(|x| x.parse::<f64>().unwrap_or_else(|_| panic!("{expr}: {v}"))).collect()
    }

    /// After each of `turns` turns of `ms`: [timeline - the last stamp, the CSS animation's
    /// startTime + currentTime - timeline, its currentTime - the painted opacity's time].
    fn rows(&mut self, turns: usize, ms: u64) -> Vec<Vec<f64>> {
        let expr = "(function () { var W = w(), c = W.document.getElementById('a').getAnimations()[0], tl = W.document.timeline.currentTime;\
            return [tl - W.stamps[W.stamps.length - 1], c.startTime + c.currentTime - tl,\
              c.currentTime - parseFloat(W.getComputedStyle(W.document.getElementById('a')).opacity) * 10000].join(','); })()";
        (0..turns)
            .map(|_| {
                self.turn(ms);
                self.nums(expr)
            })
            .collect()
    }
}

fn single() -> Doc {
    Doc::load(looped("window.ready = true; function w() { return window; } requestAnimationFrame(tick);"), String::new())
}

fn framed() -> Doc {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>window.ready = false;
document.getElementById('f').addEventListener('load', function () { ready = true; });
function w() { return document.getElementById('f').contentWindow; }</script></body>"
        .to_string();
    Doc::load(page, looped("requestAnimationFrame(tick);"))
}

fn assert_rows(rows: &[Vec<f64>], what: &str) {
    for (i, r) in rows.iter().enumerate() {
        assert!(r[0].abs() < 1.0, "{what} turn {i}: timeline - last rAF stamp {rows:?} (Chrome: 0)");
        assert!(r[1].abs() < 1.0, "{what} turn {i}: startTime + currentTime - timeline {rows:?} (Chrome: 0)");
        assert!(r[2].abs() < 1.0, "{what} turn {i}: currentTime - painted time {rows:?} (Chrome: 0)");
    }
}

/// Late turns (50 ms, three grid steps): the frame presented is stamped with the turn's frame
/// time, and a task after it reads that stamp. Was: the stamp was a turn behind.
#[test]
fn a_late_turns_frame_is_stamped_with_its_own_time() {
    let mut doc = single();
    doc.rows(3, 50);
    let rows = doc.rows(8, 50);
    assert_rows(&rows, "single");
    let stamps = doc.nums("stamps.slice(-8).join(',')");
    for pair in stamps.windows(2) {
        assert!((pair[1] - pair[0] - 50.0).abs() < 1.0, "a stamp a turn: {stamps:?}");
    }
}

/// The same in a frame document of a frame tree.
#[test]
fn a_late_turns_frame_is_stamped_with_its_own_time_in_a_frame_document() {
    let mut doc = framed();
    doc.rows(3, 50);
    let rows = doc.rows(8, 50);
    assert_rows(&rows, "frame");
}

/// Turns a little longer than a frame (20 ms: now and then two grid points pass): the painted
/// time is the time the animation reports, in the top document.
#[test]
fn a_css_animation_paints_the_presented_frames_time() {
    let mut doc = single();
    doc.rows(3, 20);
    let rows = doc.rows(12, 20);
    assert_rows(&rows, "single");
}

/// ... and in a frame document (the css_timeline row the switch broke).
#[test]
fn a_css_animation_paints_the_presented_frames_time_in_a_frame_document() {
    let mut doc = framed();
    doc.rows(3, 20);
    let rows = doc.rows(12, 20);
    assert_rows(&rows, "frame");
}

// R1 follow-ups (#1270).

/// The page's stamps and its frame document's after each of `turns` (ms each): [ticks the top
/// ran, ticks the frame ran, top's last stamp, frame's last stamp].
fn both_per_turn(doc: &mut Doc, turns: &[u64]) -> Vec<[f64; 4]> {
    turns
        .iter()
        .map(|&ms| {
            let before = doc.nums("[stamps.length, w().stamps.length].join(',')");
            doc.turn(ms);
            let after = doc.nums("[stamps.length, w().stamps.length, stamps[stamps.length - 1], w().stamps[w().stamps.length - 1]].join(',')");
            [after[0] - before[0], after[1] - before[1], after[2], after[3]]
        })
        .collect()
}

const MIXED: [u64; 14] = [5, 50, 16, 33, 10, 70, 17, 1, 40, 25, 8, 100, 3, 34];

fn tree_with_two_loops() -> Doc {
    let page = format!(
        "<!doctype html>{FADE}<div id=a></div><iframe id=f src=/child.html></iframe><script>window.ready = false; window.stamps = [];
function tick(t) {{ stamps.push(t); requestAnimationFrame(tick); }}
document.getElementById('f').addEventListener('load', function () {{ ready = true; requestAnimationFrame(tick); }});
function w() {{ return document.getElementById('f').contentWindow; }}</script>"
    );
    Doc::load(page, looped("requestAnimationFrame(tick);"))
}

/// (3) Across the documents of a frame tree: every turn's round runs both loops once, at one
/// frame time (the frame's stamp is the page's less its later time origin, the same each turn),
/// and neither document's stamps go back, over turns of mixed lengths.
#[test]
fn a_frame_trees_documents_share_each_frames_time_and_never_step_back() {
    let mut doc = tree_with_two_loops();
    both_per_turn(&mut doc, &[20, 20, 20]);
    let rows = both_per_turn(&mut doc, &MIXED);
    let offset = rows[0][2] - rows[0][3];
    let mut last = (f64::MIN, f64::MIN);
    for (i, r) in rows.iter().enumerate() {
        assert!(r[0] <= 1.0 && r[1] <= 1.0, "turn {i} ({} ms): more than one round {rows:?}", MIXED[i]);
        assert_eq!(r[0], r[1], "turn {i}: one document ran a frame the other did not {rows:?}");
        assert!(((r[2] - r[3]) - offset).abs() < 1.0, "turn {i}: the documents' stamps are not one frame's {rows:?}");
        assert!(r[2] >= last.0 && r[3] >= last.1, "turn {i}: a stamp stepped back {rows:?}");
        last = (r[2], r[3]);
    }
}

/// (4) Turns of mixed lengths (1-100 ms) in a single document: at most one round a turn, stamps
/// that only go forward, and after each turn the timeline at the last stamp and the painted
/// time the animation's.
#[test]
fn turns_of_mixed_lengths_present_their_own_frames() {
    let mut doc = single();
    doc.rows(3, 20);
    let mut prev = doc.nums("[stamps.length, stamps[stamps.length - 1]].join(',')");
    for (i, &ms) in MIXED.iter().enumerate() {
        let rows = doc.rows(1, ms);
        let now = doc.nums("[stamps.length, stamps[stamps.length - 1]].join(',')");
        assert!(now[0] - prev[0] <= 1.0, "turn {i} ({ms} ms): {} rounds", now[0] - prev[0]);
        assert!(now[1] >= prev[1], "turn {i} ({ms} ms): the stamp stepped back {prev:?} -> {now:?}");
        assert_rows(&rows, &format!("mixed {ms} ms"));
        prev = now;
    }
}

/// (2) `performance.now()` is not the frames': read after each turn of mixed lengths it is the
/// same with one round a turn as without, but for the deterministic clock's read step (0.1 ms a
/// clock read: a turn that stamps a frame reads the clock once more than one that does not).
#[test]
fn performance_now_is_the_same_with_one_round_a_turn() {
    let reads = |on: bool| -> Vec<f64> {
        let page = looped("window.ready = true; function w() { return window; } requestAnimationFrame(tick);");
        let mut doc = Doc::load_switched(page, String::new(), on);
        MIXED
            .iter()
            .map(|&ms| {
                doc.turn(ms);
                doc.nums("performance.now()")[0]
            })
            .collect()
    };
    let (on, off) = (reads(true), reads(false));
    for (i, (a, b)) in on.iter().zip(&off).enumerate() {
        assert!((a - b).abs() <= 0.1 + 1e-9, "turn {i}: performance.now() {a} with one round a turn, {b} without ({on:?} vs {off:?})");
    }
}

/// (1) A frame stays asked for while zero-delay timers flood every turn (2000 of them, each
/// re-arming itself). Chrome 148
/// (`clock_at_present_probe_cft148.json` "flood", 5 runs): 120-121 frames in 1000 ms of flood,
/// the timeline moved 1008-1017 ms, a task's timeline - the last stamp 0, painted = currentTime.
/// Here the turn's timer slice ends the turn's timers; the frame still runs every turn and the
/// timeline does not stall (a pin: green before and after).
#[test]
fn a_frame_runs_and_the_timeline_moves_while_timers_flood() {
    let page = looped(
        "window.ready = true; function w() { return window; } requestAnimationFrame(tick);
window.tasks = 0; function flood() { tasks++; setTimeout(flood, 0); } for (var i = 0; i < 2000; i++) setTimeout(flood, 0);",
    );
    let mut doc = Doc::load(page, String::new());
    doc.rows(2, 20);
    let before = doc.nums("[stamps.length, document.timeline.currentTime, tasks].join(',')");
    let rows = doc.rows(10, 20);
    let after = doc.nums("[stamps.length, document.timeline.currentTime, tasks].join(',')");
    assert!(after[2] > before[2], "the flood ran: {before:?} -> {after:?}");
    assert_eq!(after[0] - before[0], 10.0, "a frame a turn under the flood: {before:?} -> {after:?}");
    assert!(after[1] - before[1] >= 180.0, "the timeline moved: {before:?} -> {after:?}");
    assert_rows(&rows, "flood");
}

// #1272 R1 follow-ups.

/// (1) The animation clock a turn leaves never steps back from the time its animations were
/// already drawn at before the round: a stamp more than a millisecond below that time (a real
/// clock read low against the grid) keeps the drawn time. Was: the stamp, so the painted time
/// stepped back.
#[test]
fn the_turns_animation_clock_never_steps_back_from_the_presampled_frame() {
    let f = crate::turn_animation_clock_for_tests;
    assert_eq!(f(100.4, Some(100.0)), 100.0, "a stamp within a millisecond keeps the drawn time");
    assert_eq!(f(120.0, Some(100.0)), 120.0, "a stamp past it is the clock");
    assert_eq!(f(95.0, Some(100.0)), 100.0, "a stamp below it does not step the painted time back");
    assert_eq!(f(95.0, None), 95.0, "without a presample the stamp");
}

/// (2) A CSS animation's events reach script before the rAF callbacks of the frame they happen
/// in (HTML update the rendering: animations are updated and their events sent before the
/// animation frame callbacks). Chrome 148 (`anim_event_order_probe_cft148.json`, 5 runs): each of
/// 65 animationstart / animationiteration / animationend events came before its frame's rAF
/// callback, whose stamp was the event's timeline time. Here: no rAF callback of a turn is logged
/// before an animation event of the same turn. Was: the events fired in the settle after the round.
#[test]
fn a_css_animations_events_come_before_the_frames_callbacks() {
    let page = "<!doctype html><style>@keyframes fade{from{opacity:0}to{opacity:1}}\
        #a{width:10px;height:10px}#a.go{animation:fade 50ms linear 12}</style><div id=a></div><script>\
        window.ready = true; window.turn = 0; window.log = [];\
        ['animationstart','animationiteration','animationend'].forEach(function (type) {\
          document.getElementById('a').addEventListener(type, function () { log.push(type + ':' + turn); }); });\
        function tick() { log.push('raf:' + turn); requestAnimationFrame(tick); } requestAnimationFrame(tick);\
        </script>"
        .to_string();
    let mut doc = Doc::load(page, String::new());
    doc.turn(20);
    doc.engine.execute_script(doc.view, "document.getElementById('a').classList.add('go')").unwrap();
    for i in 1..60 {
        doc.engine.execute_script(doc.view, &format!("window.turn = {i}")).unwrap();
        doc.turn(20);
    }
    let log = doc.read("log.join(' ')");
    let entries: Vec<(&str, &str)> = log.split(' ').filter_map(|e| e.split_once(':')).collect();
    let events = entries.iter().filter(|(k, _)| *k != "raf").count();
    assert!(events >= 12, "the animation's events fired: {log}");
    for (i, (kind, turn)) in entries.iter().enumerate() {
        if *kind == "raf" {
            continue;
        }
        let raf_first = entries[..i].iter().any(|(k, t)| *k == "raf" && t == turn);
        assert!(!raf_first, "turn {turn}: {kind} came after the turn's rAF callback: {log}");
    }
}
