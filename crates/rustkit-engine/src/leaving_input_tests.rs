//! Input to a document the window has asked to leave (Z row S2, reopened
//! from Pete's hand test of 2026-11-04, log `live-20261104T2116`).
//!
//! That log has 959 `Script stopped at the script budget ... elapsed_ms=0`
//! lines, all for pointer events. None of them is a budget: every one sits
//! between a `navigate`/`load_html`/reload signal and the next document's
//! commit, when the view's interrupt word is `ALL` and the leaving
//! document's listeners are refused at their first opcode. Each refusal was
//! logged and recorded as a budget stop, and its stop note was itself a
//! script turn. The stale pointer input queued ahead of the navigation
//! (behind a load that held the thread for 11 s) was dispatched to that
//! document first, a ~100 ms hover restyle each, so the new load started
//! seconds late.
//!
//! The rolling post-load budget the row proposed has no failing case: the
//! load's cumulative budget ends with the load, and an event listener arms
//! a fresh live-turn deadline of its own. The first test below keeps that
//! property under guard; it was green before this change.

use super::*;
use crate::script_net_tests::serve_routes;

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

fn load(config: EngineConfig, page: String) -> (Engine, EngineViewId) {
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(config).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    runtime().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view)
}

fn over_budget(engine: &Engine, view: EngineViewId) -> Vec<String> {
    engine
        .script_log(view)
        .unwrap()
        .iter()
        .filter(|record| record.outcome == ScriptOutcome::OverBudget)
        .map(|record| record.source.clone())
        .collect()
}

/// A page whose load spends its whole script budget (six timers of 100 ms
/// of host calls each, inside the load's timer horizon, against a 300 ms
/// budget) still runs a click listener dispatched after `load`, and the
/// listener's DOM change is visible. The load budget is the load's; the
/// listener runs on a live-turn deadline of its own. Green before S2's
/// reopening: there is no page-life cumulative budget to replace.
#[test]
fn premise_a_load_that_spends_its_script_budget_still_runs_a_click_listener_after_load() {
    let page = "<html><body style=\"margin:0\"><p id=p style=\"margin:0;height:100px\">alive</p><script>
function burn(ms) { var t = Date.now(); while (Date.now() - t < ms) { document.getElementById('p'); } }
for (var i = 0; i < 6; i++) { setTimeout(function () { burn(100); }, i * 10); }
document.getElementById('p').addEventListener('click', function () {
  document.title = 'clicked';
  document.getElementById('p').textContent = 'clicked';
});
</script></body></html>";
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        script_budget_ms: 300,
        live_turn_budget_ms: 500,
        ..EngineConfig::default()
    };
    let (mut engine, view) = load(config, page.to_string());
    assert!(
        over_budget(&engine, view).iter().any(|source| source == "timers"),
        "the load's timers did not spend its budget: {:#?}",
        engine.script_log(view)
    );
    // Two seconds later. The timers the load left due run here, on their
    // own live-turn budget.
    runtime().block_on(engine.pump_live(view, 2_000));
    engine.click_at_point(view, 20.0, 20.0);
    assert!(
        !over_budget(&engine, view).iter().any(|source| source == "event:click"),
        "the click listener was refused: {:#?}",
        engine.script_log(view)
    );
    assert_eq!(engine.execute_script(view, "document.title").unwrap(), r#"String("clicked")"#);
    assert_eq!(
        engine.execute_script(view, "document.getElementById('p').textContent").unwrap(),
        r#"String("clicked")"#
    );
}

/// The window's `navigate` (and `load_html`, and a reload's cancel) sets
/// the view's interrupt word to `ALL` before it queues the command: the
/// leaving document runs no more script until the next document commits.
/// Pointer input that reaches that document is not dispatched to its
/// listeners at all: not refused one opcode in, not logged as a budget
/// stop, not an over-budget record for the Inspector. When the navigation
/// ends without a commit the word is lifted and the document's listeners
/// hear input again.
#[test]
fn input_to_a_document_being_left_is_not_dispatched_and_is_not_a_budget_stop() {
    let page = "<html><body style=\"margin:0\"><p id=p style=\"margin:0;height:100px\">alive</p><script>
window.moves = 0;
var p = document.getElementById('p');
p.addEventListener('pointerover', function () { window.moves++; });
p.addEventListener('pointermove', function () { window.moves++; document.title = 'moved'; });
p.addEventListener('click', function () { document.title = 'clicked'; });
</script></body></html>";
    let config = EngineConfig {
        interrupt_scripts_at_budget: true,
        live_turn_budget_ms: 200,
        ..EngineConfig::default()
    };
    let (mut engine, view) = load(config, page.to_string());
    let control = engine.view_control(view).expect("control");
    // What `PageHandle::navigate` signals before the Navigate command is
    // queued behind the input already waiting.
    control.stop_all();
    engine.mouse_move_at_point(view, 20.0, 20.0);
    engine.click_at_point(view, 20.0, 20.0);
    assert_eq!(
        over_budget(&engine, view),
        Vec::<String>::new(),
        "a listener of a document being left is not a budget stop: {:#?}",
        engine.script_log(view)
    );
    // The navigation ended without a commit: the document gets its script
    // back, as `end_uncommitted_nav` does.
    assert!(control.clear_all());
    assert_eq!(
        engine.execute_script(view, "window.moves").unwrap(),
        "Number(0.0)",
        "a listener ran while the document was being left"
    );
    assert_eq!(engine.execute_script(view, "document.title").unwrap(), r#"String("")"#);
    engine.mouse_move_at_point(view, 30.0, 30.0);
    assert_eq!(engine.execute_script(view, "document.title").unwrap(), r#"String("moved")"#);
    engine.click_at_point(view, 30.0, 30.0);
    assert_eq!(engine.execute_script(view, "document.title").unwrap(), r#"String("clicked")"#);
    assert_eq!(over_budget(&engine, view), Vec::<String>::new(), "{:#?}", engine.script_log(view));
}
