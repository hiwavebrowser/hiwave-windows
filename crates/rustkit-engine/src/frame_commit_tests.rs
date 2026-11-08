//! Threads plan M3, second PR: a frame reaches the presenter only as a
//! `FrameCommit` (invariant 4), the presenter keeps the newest commit per
//! view and draws that, and the W-gate's input-to-present figure rides
//! the commit. These pin what M4 (a page thread) relies on: the window
//! thread can draw a view's last frame from the commit alone, after the
//! page side has moved on.

use std::sync::Arc;
use std::time::Instant;

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
    InputMark { at: Instant::now(), seq }
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
/// that moved no frame is dropped at the next changed-views pass, not
/// charged to a later frame.
#[test]
fn an_input_is_measured_by_the_frame_that_shows_it_and_a_fruitless_one_is_dropped() {
    let (mut engine, view, _server, _rt) = load();
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 0, "the load is not an input");

    // A wheel: the scroll's own present measures it.
    engine.note_input(view, mark(1));
    assert!(engine.scroll_view(view, 0.0, -50.0).unwrap());
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 1);
    assert_eq!(engine.latest_commit(view).unwrap().input, None, "measured once");
    assert_eq!(engine.views[&view].input_pending, None);

    // Drawn again for another reason: not measured a second time.
    engine.render_all_views();
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 1);

    // A move over nothing: no frame moves; the drain ends with a
    // changed-views pass that drops the mark.
    engine.note_input(view, mark(2));
    engine.mouse_move_at_point(view, 10.0, 10.0);
    assert_eq!(engine.render_changed_views(), 0);
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 1, "nothing to measure");
    assert_eq!(engine.views[&view].input_pending, None, "dropped, not kept for a later frame");

    // Two inputs in one drain, the frame moves on the second: the
    // earliest arrival is the one measured (the user waited from it).
    engine.note_input(view, mark(3));
    engine.note_input(view, mark(4));
    assert_eq!(engine.views[&view].input_pending.map(|m| m.seq), Some(3));
    engine
        .execute_script(view, "document.getElementById('tall').style.background = 'blue'")
        .unwrap();
    assert_eq!(engine.presenter.as_ref().expect("window presenter").latency_samples(), 2);
    assert_eq!(engine.views[&view].input_pending, None);
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
