//! RAF-ONE-ROUND: one animation-frame round per rendering update.
//!
//! HTML's event loop runs the animation frame callbacks once per "update the
//! rendering" step, from the list as it stood when the step began; a callback
//! one of them asks for waits for the next update (HTML 8.1.7.3, "run the
//! animation frame callbacks"). Chrome for Testing 148 runs one rAF batch per
//! `BeginMainFrame`, one per presented frame (MotionMark's fixed-complexity
//! cells: 1.00 callbacks of the suite's loop per frame at every count). A live
//! turn of the engine is one presented frame.
//!
//! On a frame tree (an `<iframe>` with nested browsing contexts on) a late
//! turn ran two rounds: the page's turn runs its timers one sub-turn at a
//! time, and a sub-turn with no timer due ran the due frame callbacks; the
//! frame a late callback asks for is due already (the grid point of the
//! turn's time), so the next sub-turn, or the #776 round of a turn that
//! stopped for its time, ran the suite's callback a second time before
//! anything was drawn. MotionMark's heavy cells counted two of its frames per
//! presented frame (`docs/diagnostics/2026-11-17/motionmark_stage_split.md`).
//!
//! The counts here are callbacks per `pump_live` turn on the stepped clock;
//! no wall-clock reading is asserted. The busy-waits only make a turn
//! overrun its frame budget.

use super::*;
use crate::script_net_tests::serve_routes;
use std::time::{Duration, Instant};

struct Tree {
    engine: Engine,
    view: EngineViewId,
    rt: tokio::runtime::Runtime,
    _server: crate::script_net_tests::Server,
}

const LOOP_CHILD: &str = "<!doctype html><body><div id=t></div><script>
window.ticks = 0; window.stamps = [];
function tick(t) { ticks++; stamps.push(t); document.getElementById('t').style.width = (ticks % 50) + 'px'; requestAnimationFrame(tick); }
requestAnimationFrame(tick);
</script></body>";

/// Two loops, each asking for its next frame from its own callback.
const TWO_LOOPS_CHILD: &str = "<!doctype html><body><script>
window.a = 0; window.b = 0; window.stamps = [];
function fa(t) { a++; stamps.push(t); requestAnimationFrame(fa); }
function fb(t) { b++; stamps.push(t); requestAnimationFrame(fb); }
requestAnimationFrame(fa); requestAnimationFrame(fb);
</script></body>";

/// A loop whose callback spends 25 ms: every turn overruns its 16 ms frame.
const HEAVY_CHILD: &str = "<!doctype html><body><script>
window.ticks = 0;
function tick(t) { ticks++; var end = Date.now() + 25; while (Date.now() < end) {} requestAnimationFrame(tick); }
requestAnimationFrame(tick);
</script></body>";

const PAGE: &str = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false;
document.getElementById('f').addEventListener('load', function () { loaded = true; });
function w() { return document.getElementById('f').contentWindow; }
</script></body>";

impl Tree {
    fn load(page: &str, child: &str) -> Self {
        Self::load_switched(page, child, true)
    }

    /// With the RAF-ONE-ROUND switch `on` on this thread from the start.
    fn load_switched(page: &str, child: &str, on: bool) -> Self {
        let routes = vec![
            ("/", "text/html", page.to_string()),
            ("/child.html", "text/html", child.to_string()),
        ];
        let server = serve_routes(routes);
        let config = EngineConfig { nested_contexts: true, post_load_requests_live: true, ..EngineConfig::default() };
        crate::set_one_animation_round(on);
        let mut engine = Engine::new(config).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        let mut tree = Tree { engine, view, rt, _server: server };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) && tree.js("String(window.loaded)") != r#"String("true")"# {
            std::thread::sleep(Duration::from_millis(20));
            tree.rt.block_on(tree.engine.pump_live(tree.view, 20));
        }
        assert_eq!(tree.js("String(window.loaded)"), r#"String("true")"#, "the frame loaded");
        tree
    }

    fn js(&mut self, script: &str) -> String {
        self.engine.execute_script(self.view, script).unwrap()
    }

    fn num(&mut self, script: &str) -> u64 {
        let v = self.js(&format!("String({script})"));
        v.trim_start_matches("String(\"")
            .trim_end_matches("\")")
            .parse()
            .unwrap_or_else(|_| panic!("{script} read {v}"))
    }

    /// The callbacks each of `turns` turns ran, read as `count` before and after each.
    fn per_turn(&mut self, count: &str, turns: usize, elapsed_ms: u64) -> Vec<u64> {
        let mut out = Vec::new();
        for _ in 0..turns {
            let before = self.num(count);
            self.rt.block_on(self.engine.pump_live(self.view, elapsed_ms));
            out.push(self.num(count) - before);
        }
        out
    }
}

/// A turn 50 ms after the last (three grid steps: a late frame) runs the
/// frame's loop once. Before: twice (the callback's next frame was the
/// grid point of the turn's time, already due, and the next sub-turn ran it).
#[test]
fn a_late_turn_of_a_frame_tree_runs_one_animation_round() {
    let mut tree = Tree::load(PAGE, LOOP_CHILD);
    tree.per_turn("w().ticks", 3, 50);
    let ticks = tree.per_turn("w().ticks", 8, 50);
    assert_eq!(ticks, vec![1; 8], "the frame's rAF loop ran this many times per presented turn");
}

/// Two callbacks of one frame both run in its turn's round, once each, with
/// one stamp: the frame they ask for next is the next turn's. Before: each
/// ran twice a late turn, at two stamps.
#[test]
fn a_late_turns_round_runs_every_callback_of_its_frame_once() {
    let mut tree = Tree::load(PAGE, TWO_LOOPS_CHILD);
    tree.per_turn("w().a", 3, 50);
    for turn in 0..6 {
        let (a, b) = (tree.num("w().a"), tree.num("w().b"));
        let stamps = tree.num("w().stamps.length");
        tree.rt.block_on(tree.engine.pump_live(tree.view, 50));
        assert_eq!((tree.num("w().a") - a, tree.num("w().b") - b), (1, 1), "turn {turn}: callbacks of each loop");
        let distinct = tree.num(&format!("new Set(w().stamps.slice({stamps})).size"));
        assert_eq!(distinct, 1, "turn {turn}: the round's callbacks share their frame's stamp");
    }
}

/// A callback that overruns the frame (25 ms of a 16 ms budget) stops the
/// turn; the #776 round of a stopped turn does not run the loop again.
/// Before: two ticks a turn.
#[test]
fn an_overrunning_frame_callback_runs_once_a_turn() {
    let mut tree = Tree::load(PAGE, HEAVY_CHILD);
    tree.per_turn("w().ticks", 2, 30);
    let ticks = tree.per_turn("w().ticks", 6, 30);
    assert_eq!(ticks, vec![1; 6], "the overrunning loop ran this many times per presented turn");
}

/// #776 kept: a page `setTimeout(0)` chain that outlasts every frame still
/// lets the frame's loop run, once a turn.
#[test]
fn a_zero_delay_chain_that_outlasts_the_frame_still_gets_one_round() {
    let page = "<!doctype html><body><iframe id=f src=/child.html></iframe><script>
window.loaded = false; window.links = 0;
document.getElementById('f').addEventListener('load', function () { loaded = true; });
function w() { return document.getElementById('f').contentWindow; }
function link() { links++; var end = Date.now() + 4; while (Date.now() < end) {} setTimeout(link, 0); }
function arm() { setTimeout(link, 0); }
</script></body>";
    let mut tree = Tree::load(page, LOOP_CHILD);
    tree.js("arm()");
    tree.per_turn("w().ticks", 2, 20);
    let links_before = tree.num("links");
    let ticks = tree.per_turn("w().ticks", 6, 20);
    assert!(tree.num("links") > links_before + 6, "the chain kept running");
    assert_eq!(ticks, vec![1; 6], "the frame's loop per presented turn under the chain");
}

/// The single-document turn already ran one round: pinned as the reference.
#[test]
fn a_late_turn_without_frames_runs_one_animation_round() {
    let page = "<!doctype html><body><div id=t></div><script>
window.loaded = true; window.ticks = 0;
function tick(t) { ticks++; requestAnimationFrame(tick); }
requestAnimationFrame(tick);
</script></body>";
    let mut tree = Tree::load(page, LOOP_CHILD);
    tree.per_turn("ticks", 3, 50);
    let ticks = tree.per_turn("ticks", 8, 50);
    assert_eq!(ticks, vec![1; 8], "the page's rAF loop per presented turn");
}

/// Off (the default until the frame documents' CSS animation clock follows the deferred frame),
/// a late turn of a frame tree still runs the loop twice: what `HIWAVE_RAF_ONE_ROUND=1` changes.
#[test]
fn with_the_switch_off_a_late_turn_of_a_frame_tree_runs_two_rounds() {
    let mut tree = Tree::load_switched(PAGE, LOOP_CHILD, false);
    tree.per_turn("w().ticks", 3, 50);
    let ticks = tree.per_turn("w().ticks", 8, 50);
    assert_eq!(ticks, vec![2; 8], "the frame's rAF loop per presented turn with the switch off");
}
