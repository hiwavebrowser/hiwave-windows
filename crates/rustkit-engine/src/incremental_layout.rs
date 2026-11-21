//! Incremental layout, the engine side (`docs/design/INCREMENTAL_LAYOUT_PLAN.md`):
//! the `RUSTKIT_INCREMENTAL_LAYOUT` flag, the state a view keeps between
//! layouts, and what each layout did with it. The layout itself is
//! `rustkit_layout::incremental`.

use rustkit_layout::LayoutBox;
use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

/// `RUSTKIT_INCREMENTAL_LAYOUT`: `off` (default), `verify` (lay out in full
/// as today, run the incremental path beside it and count the box rects
/// that differ), `on` (paint the incremental result when it applies).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Mode {
    Off,
    Verify,
    On,
}

pub(crate) fn mode_from(value: Option<&str>) -> Mode {
    match value {
        Some("on") | Some("1") => Mode::On,
        Some("verify") => Mode::Verify,
        _ => Mode::Off,
    }
}

thread_local! {
    static TEST_MODE: Cell<Option<Mode>> = const { Cell::new(None) };
    #[cfg(test)]
    static TEST_COUNT_ONLY: Cell<bool> = const { Cell::new(false) };
}

/// Under `cfg(test)` a verify mismatch panics, so any engine test run with
/// `RUSTKIT_INCREMENTAL_LAYOUT=verify` checks the incremental path; a test
/// that sums mismatches over a corpus turns that off for its thread.
#[cfg(test)]
pub(crate) fn panic_on_mismatch() -> bool {
    !TEST_COUNT_ONLY.with(Cell::get)
}

#[cfg(test)]
pub(crate) fn set_count_only_for_test(on: bool) {
    TEST_COUNT_ONLY.with(|c| c.set(on));
}

pub(crate) fn mode() -> Mode {
    if let Some(mode) = TEST_MODE.with(Cell::get) {
        return mode;
    }
    static MODE: std::sync::OnceLock<Mode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| mode_from(std::env::var("RUSTKIT_INCREMENTAL_LAYOUT").ok().as_deref()))
}

/// Run the current thread's layouts under `mode` (tests); `None` goes back
/// to the environment's.
#[cfg(test)]
pub(crate) fn set_mode_for_test(mode: Option<Mode>) {
    TEST_MODE.with(|m| m.set(mode));
}

/// What a layout is keyed on besides the box tree: a change drops the
/// retained state.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Key {
    pub(crate) bounds: (u32, u32),
    pub(crate) cb_width: f32,
    /// The font-load generation (`rustkit_text::webfonts::generation()`)
    /// of the face set text is measured with: a web font landing after a
    /// layout changes text metrics, not the box tree.
    pub(crate) fonts: u64,
}

/// The root post-passes' inputs (`finish_root`): the root's laid-out size
/// and, when the empty-body fill changed it, the body's height.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RawRoot {
    pub(crate) root: (f32, f32),
    pub(crate) body_height: Option<f32>,
}

/// Where a view keeps its last laid-out tree.
pub(crate) enum Laid {
    /// The tree the view published; taken back without a copy when nothing
    /// else holds it.
    Published(Rc<LayoutBox>),
    /// Laid out and returned, not yet published (`adopt`).
    Pending,
}

/// A view's last layout: the tree it started from and the tree it made.
pub(crate) struct Retained {
    pub(crate) key: Key,
    pub(crate) unlaid: LayoutBox,
    pub(crate) laid: Laid,
    pub(crate) raw: RawRoot,
}

impl Retained {
    /// The laid-out tree as it was before the root post-passes, owned: the
    /// published tree itself when nothing else holds it, else a copy. The
    /// stamps the engine puts on a published tree (scroll offsets, sticky
    /// rules) are cleared, as on a fresh tree; layout never reads them.
    pub(crate) fn take_laid(&mut self, stats: &mut Stats) -> Option<LayoutBox> {
        let tree = match std::mem::replace(&mut self.laid, Laid::Pending) {
            Laid::Pending => return None,
            Laid::Published(rc) => match Rc::try_unwrap(rc) {
                Ok(tree) => {
                    stats.reused_trees += 1;
                    tree
                }
                Err(rc) => {
                    stats.cloned_trees += 1;
                    (*rc).clone()
                }
            },
        };
        let mut tree = tree;
        tree.dimensions.content.width = self.raw.root.0;
        tree.dimensions.content.height = self.raw.root.1;
        if let (Some(h), Some(body)) = (self.raw.body_height, tree.children.first_mut()) {
            body.dimensions.content.height = h;
        }
        clear_stamps(&tree);
        Some(tree)
    }
}

fn clear_stamps(b: &LayoutBox) {
    b.element_scroll.set((0.0, 0.0));
    b.sticky_rule.set(None);
    b.sticky_shift.set((0.0, 0.0));
    b.sticky_paths.borrow_mut().clear();
    b.scrollbar_thumb_hovered.set(false);
    for child in &b.children {
        clear_stamps(child);
    }
}

/// The view published `tree`, the one its last layout returned: keep it as
/// the retained laid-out tree instead of a copy.
pub(crate) fn adopt(lay: &crate::layout_ctx::ViewLayout, tree: &Rc<LayoutBox>) {
    if let Some(retained) = lay.incremental_layout.borrow_mut().as_mut() {
        if matches!(retained.laid, Laid::Pending) {
            retained.laid = Laid::Published(tree.clone());
        }
    }
}

/// What the incremental path did, per view.
#[derive(Default, Debug, Clone)]
pub(crate) struct Stats {
    /// Layouts that tried the path (the flag on or verify).
    pub(crate) attempts: u32,
    /// One box laid out again and the rest moved.
    pub(crate) relaid: u32,
    /// Nothing changed: the last layout stood.
    pub(crate) unchanged: u32,
    /// Full layouts, by reason.
    pub(crate) fallbacks: BTreeMap<&'static str, u32>,
    /// Verify: boxes whose rects differed from the full layout's, in all.
    pub(crate) mismatches: u32,
    /// Verify: layouts with at least one mismatch.
    pub(crate) mismatched_layouts: u32,
    /// Boxes the incremental relayouts laid out, in all.
    pub(crate) visits: u64,
    /// The last incremental relayout's boxes laid out.
    pub(crate) last_visits: u64,
    /// The last incremental relayout's tree diff, in milliseconds.
    pub(crate) last_diff_ms: f64,
    /// The last incremental relayout's boxes moved by the delta.
    pub(crate) last_moved: u64,
    /// The last layout's outcome, for the relayout line.
    pub(crate) last: String,
    /// Relayout roots laid out, in all (several per layout when several
    /// boxes changed).
    pub(crate) roots: u32,
    /// Levels changed boxes were hoisted to their relayout roots, in all.
    pub(crate) hoists: u32,
    /// Relayout roots whose delta stopped at a boundary below the root.
    pub(crate) bounded: u32,
    /// Script-mutation relayouts whose dirty set changed no style or box:
    /// the build was skipped and the last layout stood.
    pub(crate) skipped_builds: u32,
    /// Verify: a relayout that would have skipped its build, whose fresh
    /// build differed from the retained tree (the skip would have been
    /// wrong).
    pub(crate) skip_violations: u32,
    /// The last wrong skip hint's outcome, in shapes.
    pub(crate) last_violation: String,
    /// The published tree taken back without a copy, or copied because
    /// something else held it.
    pub(crate) reused_trees: u32,
    pub(crate) cloned_trees: u32,
}

impl Stats {
    pub(crate) fn fallback(&mut self, why: &'static str) {
        *self.fallbacks.entry(why).or_insert(0) += 1;
        self.last = format!("fallback({why})");
    }
}
