//! LAT21: a box must be laid out about once per layout pass. walmart.com ran ~160 visits per box
//! (256,036 visits over 1,598 boxes in one forced layout; 76% of the 665 ms was block layout
//! re-run through `layout_block_children_with_collapse`).
//!
//! Root cause, measured on these pages with `rustkit_layout::take_layout_visits`: a flex container
//! is laid out TWICE per pass, once by the block pre-pass (its items as block children) and once by
//! the flex pass (`flex.rs`, `item.layout_box.layout_block_children_with_collapse`), and each of
//! those lays out every nested flex container the same way. So visits grow as 2^k in the number k
//! of flex containers nested through blocks. Direct flex-in-flex is only quadratic, floats and
//! inline-blocks add a constant factor, plain blocks and abspos are flat.
//!
//! Measured (visits / boxes): flex+block alternation d=4: 3.5, d=8: 9.3, d=12: 27.2; flex+float
//! d=12: 109.9. Each extra nested flex roughly doubles the work.

#![cfg(feature = "headless")]


use super::*;

/// Layout visits for one `load_html` of `html`.
fn visits(html: &str) -> u64 {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 1200, height: 800 })
        .expect("view");
    rustkit_layout::take_layout_visits();
    engine.load_html(id, html).expect("load_html");
    rustkit_layout::take_layout_visits()
}

/// `depth` levels cycling through `parts` (open, close), around the text "text".
fn nest(parts: &[(&str, &str)], depth: usize) -> String {
    let mut open = String::new();
    let mut close = Vec::new();
    for i in 0..depth {
        let (o, c) = parts[i % parts.len()];
        open.push_str(o);
        close.push(c);
    }
    close.reverse();
    format!("<html><body>{open}text{}</body></html>", close.concat())
}

const FLEX: (&str, &str) = ("<div style=\"display:flex\"><span>w</span>", "</div>");
const BLOCK: (&str, &str) = ("<div>", "</div>");
const INLINE_BLOCK: (&str, &str) = ("<div style=\"display:inline-block\"><span>a</span>", "</div>");
const FLOAT: (&str, &str) = ("<div style=\"float:left\">", "</div>");
const ABSPOS: (&str, &str) = ("<div style=\"position:absolute;inset:0\">", "</div>");

/// Elements in the page (a stand-in for boxes: every `div` makes one, plus html and body).
fn boxes(html: &str) -> u64 {
    (html.matches("<div").count() + 2) as u64
}

#[test]
fn plain_blocks_and_abspos_are_laid_out_once_each() {
    for parts in [&[BLOCK][..], &[ABSPOS][..]] {
        let html = nest(parts, 24);
        let (v, b) = (visits(&html), boxes(&html));
        assert!(v <= b + b / 4, "{v} visits for {b} boxes: {html:.80}");
    }
}

#[test]
fn inline_blocks_and_nested_floats_stay_within_a_small_multiple_of_the_boxes() {
    // Inline-blocks holding inline content cost a constant ~3 per box; nested floats grow
    // quadratically but stay small at the depths real pages use.
    let ib = nest(&[INLINE_BLOCK], 12);
    assert!(visits(&ib) <= 4 * boxes(&ib), "inline-block: {} visits for {} boxes", visits(&ib), boxes(&ib));
    let fl = nest(&[FLOAT], 8);
    assert!(visits(&fl) <= 6 * boxes(&fl), "float: {} visits for {} boxes", visits(&fl), boxes(&fl));
}

use rustkit_layout::layout_memo::{set_memo_min_subtree, set_memo_mode, take_memo_counters, MemoMode};

/// Every element's rounded box, in document order, plus the layout visits of the load.
fn layout_of(html: &str, mode: MemoMode, min: usize) -> (String, u64, rustkit_layout::layout_memo::MemoCounters) {
    set_memo_mode(Some(mode));
    set_memo_min_subtree(Some(min));
    take_memo_counters();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine.create_headless_view(Bounds { x: 0, y: 0, width: 1200, height: 800 }).expect("view");
    rustkit_layout::take_layout_visits();
    engine.load_html(id, html).expect("load_html");
    let v = rustkit_layout::take_layout_visits();
    let counters = take_memo_counters();
    let rects = engine
        .execute_script(
            id,
            "Array.prototype.map.call(document.querySelectorAll('*'), function (e) { var r = e.getBoundingClientRect(); \
             return e.tagName + ':' + Math.round(r.left * 10) / 10 + ':' + Math.round(r.top * 10) / 10 + ':' + Math.round(r.width * 10) / 10 + ':' + Math.round(r.height * 10) / 10; }).join(' ')",
        )
        .expect("script");
    set_memo_mode(None);
    set_memo_min_subtree(None);
    (rects, v, counters)
}

fn with_mode(html: &str, mode: MemoMode) -> (String, u64, rustkit_layout::layout_memo::MemoCounters) {
    layout_of(html, mode, 1)
}

/// Off, verify and on (every subtree, and the default 33+) must produce the same rectangles;
/// returns the counters of the `on` run.
fn assert_same_in_every_mode(name: &str, html: &str) -> rustkit_layout::layout_memo::MemoCounters {
    let (off, _, _) = with_mode(html, MemoMode::Off);
    let (verify, _, v) = with_mode(html, MemoMode::Verify);
    let (on, _, c) = with_mode(html, MemoMode::On);
    let (on33, _, _) = layout_of(html, MemoMode::On, 33);
    assert_eq!(off, verify, "{name}: verify mode changed the layout");
    assert_eq!(off, on, "{name}: the memo changed the layout");
    assert_eq!(off, on33, "{name}: the memo (33+) changed the layout");
    assert!(v.verified > 0 || c.hits == 0, "{name}: nothing was verified");
    c
}

#[test]
fn with_the_memo_off_nesting_doubles_the_work_per_container() {
    // What the memo removes: two more flex containers, ~4x the visits (93 -> 381).
    let (shallow, deep) = (nest(&[FLEX, BLOCK], 8), nest(&[FLEX, BLOCK], 12));
    let (_, vs, _) = with_mode(&shallow, MemoMode::Off);
    let (_, vd, _) = with_mode(&deep, MemoMode::Off);
    assert!(vd >= 3 * vs, "expected the exponential growth this row documents: {vs} -> {vd}");
}

/// The pin for the fix. With the memo on, nesting depth costs a LINEAR number of visits (off: it
/// doubles per flex container). A box is still laid out about once per distinct constraint it is
/// given (the memo's misses: 3.9 per box at depth 8 growing to 13.0 at depth 32 for flex/block), so
/// visits per box do not reach 1 and grow ~0.5 per level; memo hits are not visits. flex/float
/// stays higher because the float path (`layout_block_children`) is not memoised. The counts are
/// deterministic (no timing), so the bounds sit ~10% above what was measured.
#[test]
fn with_the_memo_on_nested_flex_containers_cost_visits_linear_in_depth() {
    // (name, parts, visits per box allowed at depth 12; measured 7.1 / 10.9 / 11.4)
    for (name, parts, per_box_12) in [
        ("flex/block", &[FLEX, BLOCK][..], 8.0),
        ("flex/inline-block", &[FLEX, INLINE_BLOCK][..], 12.0),
        ("flex/float", &[FLEX, FLOAT][..], 12.5),
    ] {
        let at = |d: usize| {
            let html = nest(parts, d);
            let (_, v, _) = with_mode(&html, MemoMode::On);
            (v, boxes(&html))
        };
        let (v12, b12) = at(12);
        assert!(
            (v12 as f64) <= per_box_12 * b12 as f64,
            "{name}: {v12} visits for {b12} boxes at depth 12 ({:.1} per box, allowed {per_box_12})",
            v12 as f64 / b12 as f64
        );
        let ((v16, _), (v32, _)) = (at(16), at(32));
        // Linear growth: doubling the depth ~doubles the boxes and each box's constraints
        // (measured 3.5-3.9x). Exponential growth would multiply the visits by 2^8 here.
        assert!(v32 * 10 <= v16 * 42, "{name}: depth 16 -> 32 grew {v16} -> {v32} visits");
    }
}

/// Off, verify and on agree on a nest of `parts`. Depth 10 keeps the memo-off pass (2^k visits)
/// to a few seconds in the debug profile on a loaded machine; the deep (24+) nesting is the
/// release-profile timing tool's job (`lat21_time`), not a unit test's. One test per series, so no
/// piece holds the GPU test guard (taken by `Engine::new`) for long.
fn nested_flex_is_identical_in_every_mode(name: &str, parts: &[(&str, &str)]) {
    let html = nest(parts, 10);
    let c = assert_same_in_every_mode(name, &html);
    assert!(c.hits > 0, "{name}: the memo never hit");
}

#[test]
fn every_mode_lays_flex_block_nests_out_identically() {
    nested_flex_is_identical_in_every_mode("flex/block", &[FLEX, BLOCK]);
}

#[test]
fn every_mode_lays_flex_inline_block_nests_out_identically() {
    nested_flex_is_identical_in_every_mode("flex/inline-block", &[FLEX, INLINE_BLOCK]);
}

#[test]
fn every_mode_lays_flex_float_nests_out_identically() {
    nested_flex_is_identical_in_every_mode("flex/float", &[FLEX, FLOAT]);
}

#[test]
fn every_mode_lays_flex_only_nests_out_identically() {
    nested_flex_is_identical_in_every_mode("flex only", &[FLEX]);
}

const LOREM: &str = "Lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod tempor incididunt ut labore et dolore magna aliqua";

/// Risk 1: positions are stored relative to the box's origin and translated back, so a restore must
/// agree with a real layout for text that wraps and for floats that the text wraps around, at
/// origins that differ between the passes (an aside of changing width beside the main column).
fn rich(depth: usize) -> String {
    let inner = if depth == 0 {
        format!("<p>{LOREM}</p>")
    } else {
        rich_body(depth - 1)
    };
    format!(
        "<div style=\"display:flex;padding:3px;border:1px solid\"><div style=\"flex:1;min-width:0\">\
         <div style=\"float:left;width:40px;height:30px;margin:2px\"></div>{LOREM} {inner}</div>\
         <div style=\"width:{}px;margin-left:5px\">{LOREM}</div></div>",
        90 + 7 * depth
    )
}
fn rich_body(depth: usize) -> String {
    rich(depth)
}

#[test]
fn wrapping_text_and_floats_at_shifting_origins_restore_exactly() {
    let html = format!("<html><body style=\"width:900px\">{}</body></html>", rich(5));
    let c = assert_same_in_every_mode("rich", &html);
    assert!(c.hits > 0, "the memo never hit on the rich page: {c:?}");
}

/// Risk 2: a descendant placed against state outside its subtree (a fixed box: the viewport; a
/// sticky one: a scroll container) is never memoised, and the page lays out as without the memo.
#[test]
fn subtrees_with_fixed_or_sticky_descendants_are_never_memoised() {
    for (name, style) in [("fixed", "position:fixed;top:0;left:0"), ("sticky", "position:sticky;top:0")] {
        let mut inner = format!("<div style=\"{style}\">x</div>");
        for _ in 0..6 {
            inner = format!("<div style=\"display:flex;position:relative\"><span>w</span><div>{inner}</div></div>");
        }
        let html = format!("<html><body>{inner}</body></html>");
        let c = assert_same_in_every_mode(name, &html);
        assert!(c.refused > 0, "{name}: a subtree with a {name} descendant was memoised: {c:?}");
    }
}

/// An absolute box is anchored to its parent's padding box, inside the subtree, so it IS memoised
/// and restores to the same rectangles, including a `bottom`/`right` anchored one and one whose
/// parent is positioned or static, at origins that differ between the passes.
fn absolute_restores_exactly(style: &str) {
    let mut inner = format!("<div style=\"{style}\">x</div>{LOREM}");
    for i in 0..6 {
        let rel = if i % 2 == 0 { "position:relative;" } else { "" };
        inner = format!("<div style=\"display:flex;{rel}padding:2px\"><span>w</span><div>{inner}</div></div>");
    }
    let html = format!("<html><body style=\"width:800px\">{inner}</body></html>");
    let c = assert_same_in_every_mode(style, &html);
    assert!(c.hits > 0 && c.stored > 0, "{style}: the memo did not engage: {c:?}");
}

#[test]
fn absolute_top_left_descendants_restore_exactly() {
    absolute_restores_exactly("position:absolute;top:2px;left:3px");
}

#[test]
fn absolute_bottom_right_descendants_restore_exactly() {
    absolute_restores_exactly("position:absolute;bottom:4px;right:5px;width:20px;height:10px");
}

#[test]
fn absolute_inset_descendants_restore_exactly() {
    absolute_restores_exactly("position:absolute;inset:0");
}

#[test]
fn absolute_static_position_descendants_restore_exactly() {
    absolute_restores_exactly("position:absolute");
}

/// Risk 2 (scroll): an overflow scroller inside nested flex lays out the same with the memo.
#[test]
fn an_overflow_scroller_inside_nested_flex_lays_out_identically() {
    let mut inner = format!("<div style=\"overflow:auto;height:80px\"><div style=\"height:300px\">{LOREM}</div></div>");
    for _ in 0..6 {
        inner = format!("<div style=\"display:flex\"><span>w</span><div>{inner}</div></div>");
    }
    assert_same_in_every_mode("scroller", &format!("<html><body>{inner}</body></html>"));
}

/// Risk 3: percent-height marks. A percentage height under an auto-height parent is `auto`; the
/// flex pass resolves it against a definite height. The marks are part of the key, so a restore
/// must still agree with a real layout.
#[test]
fn percent_height_marks_in_nested_flex_columns_lay_out_identically() {
    let mut inner = "<div style=\"height:100%;background:red\">x</div><div style=\"height:50%\">y</div>".to_string();
    for i in 0..6 {
        let height = if i % 3 == 0 { "height:300px;" } else { "" };
        inner = format!("<div style=\"display:flex;flex-direction:column;{height}\"><div style=\"height:100%\">{inner}</div><span>w</span></div>");
    }
    assert_same_in_every_mode("percent", &format!("<html><body style=\"height:600px\">{inner}</body></html>"));
}

/// Not a test of behaviour: with `RUSTKIT_LAYOUT_MEMO_REPORT=<file>` set, writes what the memo did
/// over the whole run to that file. The name sorts last, so on a serial run every other test has
/// already laid out its pages. Used to report how many memo hits a corpus run exercised.
///
/// VALID ONLY WITH `--test-threads=1` (the corpus command): the totals are process-wide and the sort
/// order is what puts this test last; under parallel `cargo test` or nextest they are wrong or zero.
#[test]
fn zzz_report_memo_totals_for_the_corpus_run() {
    if let Ok(path) = std::env::var("RUSTKIT_LAYOUT_MEMO_REPORT") {
        let totals = rustkit_layout::layout_memo::memo_totals();
        std::fs::write(path, format!("{totals:?}\n")).expect("write report");
    }
}

/// R1 on #1073 (MED): the memo's whole-subtree snapshots must not stay on the tree after the root
/// layout, or a page's resident memory would carry two copies of every big subtree's output. The
/// engine (so the laid-out tree) stays alive while the entries are counted.
#[test]
fn no_snapshot_outlives_the_root_layout() {
    let html = nest(&[FLEX, BLOCK], 10);
    for mode in [MemoMode::On, MemoMode::Verify] {
        set_memo_mode(Some(mode));
        set_memo_min_subtree(Some(1));
        take_memo_counters();
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine.create_headless_view(Bounds { x: 0, y: 0, width: 1200, height: 800 }).expect("view");
        engine.load_html(id, &html).expect("load_html");
        let stored = take_memo_counters().stored;
        let alive = rustkit_layout::layout_memo::live_memo_entries();
        set_memo_mode(None);
        set_memo_min_subtree(None);
        assert!(stored > 0, "{mode:?}: nothing was recorded, the test proves nothing");
        assert_eq!(alive, 0, "{mode:?}: {alive} of {stored} recorded results outlived the root layout");
        drop(engine);
    }
}
