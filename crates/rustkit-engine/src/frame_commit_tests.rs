//! Threads plan M3, second PR: a frame reaches the presenter only as a
//! `FrameCommit` (invariant 4), the presenter keeps the newest commit per
//! view and draws that, and the W-gate's input-to-present figure rides
//! the commit. These pin what M4 (a page thread) relies on: the window
//! thread can draw a view's last frame from the commit alone, after the
//! page side has moved on.

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE: &str = "<html><head><style>body{margin:0} #tall{height:1000px;background:#eee}\
    </style></head><body><div id=tall></div></body></html>";

fn load() -> (Engine, EngineViewId, crate::script_net_tests::Server, tokio::runtime::Runtime) {
    let server = serve_routes(vec![("/", "text/html", PAGE.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server, rt)
}

fn mark(seq: u64) -> InputMark {
    mark_of(seq, InputClass::Wheel)
}

fn mark_of(seq: u64, kind: InputClass) -> InputMark {
    InputMark { at: Instant::now(), seq, kind }
}

fn latency(engine: &mut Engine) -> &mut crate::wgate::InputLatency {
    engine.presenter.as_mut().expect("window presenter").latency()
}

/// The load's frame is a commit: the page's own list and the commit's are
/// one allocation, and the commit names the generation and the viewport
/// the list was laid out for.
#[test]
fn a_load_commits_the_frame_it_presents() {
    let (engine, view, _server, _rt) = load();
    let state = &engine.views[&view];
    let commit = engine.latest_commit(view).expect("the load committed a frame");
    assert_eq!(commit.generation, state.frame_generation);
    assert_eq!(commit.nav_generation, state.nav_id);
    assert_eq!(commit.scroll_offset, (0.0, 0.0));
    assert_eq!(commit.max_scroll_offset, (0.0, 800.0), "1000 px page in a 200 px view");
    assert_eq!(commit.laid_out_for, (400, 200));
    let page_list = state.display_list.as_ref().expect("the page keeps its list");
    let committed = commit.list.as_ref().expect("the commit holds the list");
    assert!(Arc::ptr_eq(page_list, committed), "shared, not copied");
    assert_eq!(Arc::strong_count(page_list), 2, "the page and the commit");
    assert_eq!(
        engine.views[&view].presented.as_ref().map(|p| p.generation),
        Some(commit.generation),
        "what was presented is the commit"
    );
}

/// The page side drops its list (as a navigation or a relayout in
/// progress will, on a page thread): the commit keeps it, and the
/// presenter draws the same frame from the commit alone.
#[test]
fn a_commit_outlives_the_page_side_and_is_drawn_without_it() {
    let (mut engine, view, _server, _rt) = load();
    let commands = engine.views[&view].display_list.as_ref().unwrap().commands.len();
    assert!(commands > 0);
    let frames_before = engine.views[&view].frames_presented;
    let generation = engine.views[&view].frame_generation;

    engine.views.get_mut(&view).unwrap().display_list = None;
    let commit = engine.latest_commit(view).expect("still committed");
    let list = commit.list.as_ref().expect("the commit holds the list alive");
    assert_eq!(Arc::strong_count(list), 1, "the commit is the last owner");
    assert_eq!(list.commands.len(), commands);

    // A draw for a reason other than a page change (here: asked for)
    // presents the commit the presenter holds; it does not look at the
    // page, which has nothing to give.
    engine.render_all_views();
    let state = &engine.views[&view];
    assert_eq!(state.frames_presented, frames_before + 1, "drawn");
    assert_eq!(state.presented.as_ref().unwrap().generation, generation);
    assert_eq!(engine.latest_commit(view).unwrap().generation, generation, "no new commit");
    assert!(state.display_list.is_none(), "the page side was not consulted");
}

/// A user scroll commits the new offset with the same list (no copy), so
/// the frame moves without a layout (#692) and without a clone.
#[test]
fn a_scroll_commits_the_offset_and_shares_the_list() {
    let (mut engine, view, _server, _rt) = load();
    let generation = engine.views[&view].frame_generation;
    let page_list = Arc::clone(engine.views[&view].display_list.as_ref().unwrap());

    assert!(engine.scroll_view(view, 0.0, -50.0).unwrap());
    let commit = engine.latest_commit(view).unwrap();
    assert_eq!(commit.scroll_offset, (0.0, 50.0));
    assert_eq!(commit.generation, generation, "no new display list for a scroll");
    assert!(Arc::ptr_eq(commit.list.as_ref().unwrap(), &page_list));
    assert_eq!(Arc::strong_count(&page_list), 3, "page, commit, this test");

    // A relayout commits a new list; the old one is released.
    engine
        .execute_script(view, "document.getElementById('tall').style.background = 'red'")
        .unwrap();
    let commit = engine.latest_commit(view).unwrap();
    assert!(commit.generation > generation);
    assert!(!Arc::ptr_eq(commit.list.as_ref().unwrap(), &page_list));
    assert_eq!(Arc::strong_count(&page_list), 1, "only this test holds the old list");
}

/// The W-gate's input-to-present figure: an input the app notes rides
/// the commit of the frame that shows it and is measured once; an input
/// that moved no frame is not charged to a later frame (LAT1c: it is
/// counted `no_frame`).
#[test]
fn an_input_is_measured_by_the_frame_that_shows_it_and_a_fruitless_one_is_dropped() {
    let (mut engine, view, _server, _rt) = load();
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 0, "the load is not an input");

    // A wheel: the scroll's own present measures it.
    engine.note_input(view, mark(1));
    assert!(engine.scroll_view(view, 0.0, -50.0).unwrap());
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 1);
    assert!(engine.latest_commit(view).unwrap().inputs.is_empty(), "measured once");
    assert!(engine.views[&view].input_pending.is_empty());

    // Drawn again for another reason: not measured a second time.
    engine.render_all_views();
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 1);

    // A move over nothing: no frame moves; the drain ends with a
    // changed-views pass that retires the mark.
    engine.note_input(view, mark_of(2, InputClass::Move));
    engine.mouse_move_at_point(view, 10.0, 10.0);
    assert_eq!(engine.render_changed_views(), 0);
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 1, "nothing to measure");
    assert!(engine.views[&view].input_pending.is_empty(), "dropped, not kept for a later frame");
    assert_eq!(engine.views[&view].inputs_unpresented, 1, "and counted");

    // Two inputs in one drain, the frame moves on the second: both are
    // measured by that frame (LAT1c), the earlier one the longer.
    engine.note_input(view, mark(3));
    engine.note_input(view, mark(4));
    assert_eq!(engine.views[&view].input_pending.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![3, 4]);
    engine
        .execute_script(view, "document.getElementById('tall').style.background = 'blue'")
        .unwrap();
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 3);
    assert!(engine.views[&view].input_pending.is_empty());
    assert_eq!(engine.views[&view].inputs_unpresented, 0, "the no_frame count rode that commit");
}

/// LAT1c, the red test: inputs held for a known time before the frame
/// that shows them is presented are measured at that time, each one, by
/// kind; moves are sampled (one per frame); the visit's line names the
/// view, the host and the role, and a navigation ends it.
#[test]
fn input_to_present_measures_a_known_delay_per_input_and_logs_it_per_visit() {
    const DELAY: Duration = Duration::from_millis(120);
    // Generous: a debug build on a loaded Mac draws a 400 x 200 frame in
    // well under this, and an input not measured from its own time
    // (the old "earliest per frame") fails the per-input count first.
    const SLACK_MS: f64 = 1500.0;
    let (mut engine, view, server, rt) = load();
    let host = "127.0.0.1";

    // Three keys and two moves made, then the page is busy for DELAY,
    // then the frame that shows them is drawn.
    for (seq, kind) in [
        (10, InputClass::Key),
        (11, InputClass::Move),
        (12, InputClass::Key),
        (13, InputClass::Move),
        (14, InputClass::Down),
    ] {
        engine.note_input(view, mark_of(seq, kind));
    }
    std::thread::sleep(DELAY);
    engine
        .execute_script(view, "document.getElementById('tall').style.background = 'green'")
        .unwrap();
    let taken = latency(&mut engine).taken.clone();
    let kinds: Vec<InputClass> = taken.iter().map(|&(k, _)| k).collect();
    assert_eq!(
        kinds,
        vec![InputClass::Key, InputClass::Move, InputClass::Key, InputClass::Down],
        "every key and press, one move of two"
    );
    for &(kind, ms) in &taken {
        assert!(
            ms >= DELAY.as_secs_f64() * 1000.0 && ms < DELAY.as_secs_f64() * 1000.0 + SLACK_MS,
            "{kind:?} measured {ms} ms for a {DELAY:?} delay"
        );
    }

    // A native time older than the handler's (the window was held): the
    // input is measured from when it was made.
    let made = Instant::now() - Duration::from_millis(300);
    engine.note_input(view, InputMark { at: made, seq: 20, kind: InputClass::Wheel });
    assert!(engine.scroll_view(view, 0.0, -40.0).unwrap());
    let (_, ms) = *latency(&mut engine).taken.last().expect("the wheel was measured");
    assert!((300.0..300.0 + SLACK_MS).contains(&ms), "wheel measured {ms} ms from a 300 ms-old event");

    // A key into nothing: no frame. Then a navigation: the visit's line.
    engine.note_input(view, mark_of(21, InputClass::Key));
    assert_eq!(engine.render_changed_views(), 0);
    assert!(latency(&mut engine).lines.is_empty(), "no line before the visit ends");
    engine.load_html(view, "<p>next</p>").expect("load_html");
    engine.note_input(view, mark_of(22, InputClass::Up));
    engine.execute_script(view, "document.body.style.background = 'red'").unwrap();
    let lines = latency(&mut engine).lines.clone();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    let prefix = format!("Input to present view={} host={host} n=4 ", view.raw());
    assert!(line.starts_with(&prefix), "{line}");
    assert!(line.contains(" by_kind=key:2:"), "{line}");
    assert!(line.contains(",move:1:"), "{line}");
    assert!(line.contains(",wheel:1:"), "{line}");
    assert!(line.contains(" no_frame=1 role=page at=navigation"), "{line}");
    let p95: f64 = line.split(" p95_ms=").nth(1).unwrap().split(' ').next().unwrap().parse().unwrap();
    assert!(p95 >= 300.0, "the wheel's 300 ms is the p95 of four: {line}");

    // The presenter going logs the HTML visit's line.
    latency(&mut engine).flush("drop");
    let last = latency(&mut engine).lines.last().cloned().expect("drop line");
    assert!(last.starts_with(&format!("Input to present view={} host=- n=1 ", view.raw())), "{last}");
    assert!(last.ends_with(" no_frame=0 role=page at=drop"), "{last}");
    drop((server, rt));
}

/// LAT1c: a commit the window never drew hands its inputs to the one that
/// replaces it, so an input made while the window was slow is measured,
/// not lost with the skipped frame; inputs of a visit a navigation ended
/// are counted `no_frame`.
#[test]
fn an_undrawn_commit_hands_its_inputs_to_the_next() {
    let (engine, view, _server, _rt) = load();
    let base = engine.latest_commit(view).unwrap().clone();
    let mut older = base.clone();
    older.inputs = vec![mark_of(1, InputClass::Key)];
    older.inputs_unpresented = 2;
    let mut newer = base.clone();
    newer.inputs = vec![mark_of(2, InputClass::Key)];
    newer.absorb_undrawn(&mut older);
    assert_eq!(newer.inputs.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(newer.inputs_unpresented, 2);
    assert!(older.inputs.is_empty() && older.inputs_unpresented == 0);

    let mut older = base.clone();
    older.inputs = vec![mark_of(3, InputClass::Key)];
    let mut navigated = base.clone();
    navigated.visit.nav += 1;
    navigated.absorb_undrawn(&mut older);
    assert!(navigated.inputs.is_empty());
    assert_eq!(navigated.inputs_unpresented, 1, "another visit's input: no frame of its own");
}

/// A destroyed view's commit goes with it: nothing holds its list.
#[test]
fn a_destroyed_view_drops_its_commit() {
    let (mut engine, view, _server, _rt) = load();
    let page_list = Arc::clone(engine.views[&view].display_list.as_ref().unwrap());
    assert_eq!(Arc::strong_count(&page_list), 3);
    engine.destroy_view(view).unwrap();
    assert!(engine.latest_commit(view).is_none());
    assert_eq!(Arc::strong_count(&page_list), 1, "only this test");
}

/// #704: a re-present with the page side intact and no scroll / generation
/// change keeps the held commit (surface size / max-age refresh path).
#[test]
fn a_noop_represent_keeps_the_held_commit() {
    let (mut engine, view, _server, _rt) = load();
    let list = Arc::clone(
        engine
            .latest_commit(view)
            .expect("load commit")
            .list
            .as_ref()
            .expect("list"),
    );
    let generation = engine.latest_commit(view).unwrap().generation;
    let scroll_offset = engine.latest_commit(view).unwrap().scroll_offset;
    let frames_before = engine.views[&view].frames_presented;

    engine.render_view(view).expect("re-present");
    let after = engine.latest_commit(view).expect("still committed");
    assert_eq!(after.generation, generation, "no new frame");
    assert_eq!(after.scroll_offset, scroll_offset);
    assert!(
        Arc::ptr_eq(after.list.as_ref().expect("list"), &list),
        "same commit list, not a replacement"
    );
    assert_eq!(
        engine.views[&view].frames_presented,
        frames_before + 1,
        "drawn again from the held commit"
    );
}
