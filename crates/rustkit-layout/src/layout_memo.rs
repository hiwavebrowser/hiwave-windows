//! LAT21: a memo of a box's child layout, so a subtree is not laid out again for a constraint it
//! has already been laid out for in the same pass.
//!
//! walmart.com's layout re-ran block layout ~160 times per box. The cause is structural: a flex
//! container is laid out by the block pre-pass (at the block's width) and again by the flex pass
//! (at the flex-resolved width), and every nested flex container does the same, so the work doubles
//! per nesting level. The widths alternate (W1, W2, W1, W2, ...), so the SECOND call at a width has
//! seen the same inputs as the call two before it. This module remembers the last two distinct
//! results per box and, on a hit, writes the recorded OUTPUT fields back instead of laying the
//! subtree out again.
//!
//! `RUSTKIT_LAYOUT_MEMO=off|verify|on`, default off:
//! - `off`: nothing changes.
//! - `verify`: never restores. When a restore would have applied, the subtree is laid out anyway and
//!   compared with what the restore would have produced; any difference panics.
//! - `on`: restores.
//!
//! `RUSTKIT_LAYOUT_MEMO_MIN=n` sets the smallest subtree (in descendant boxes) that is memoised
//! (default 33; the corpus runs with 1 so every child layout is checked).
//!
//! What a hit assumes, and what refuses it: the key is the box's width, edges, definite height,
//! margin context and the layout INPUT state below it (child counts, percent-height marks, which
//! boxes share a float context). Its position is not in the key: outputs are stored relative to
//! the box's content origin and translated back. A subtree with a fixed or sticky descendant is
//! never memoised: those are placed against the viewport or a scroll container, not this box.

use super::*;
use std::cell::{Cell, RefCell};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MemoMode {
    Off,
    Verify,
    On,
}

thread_local! {
    static LIVE: Cell<i64> = const { Cell::new(0) };
    static MODE_OVERRIDE: Cell<Option<MemoMode>> = const { Cell::new(None) };
    static MIN_OVERRIDE: Cell<Option<usize>> = const { Cell::new(None) };
    static EPOCH: Cell<u64> = const { Cell::new(1) };
    static COUNTERS: RefCell<MemoCounters> = RefCell::new(MemoCounters::default());
    static VERIFY_HITS: Cell<u64> = const { Cell::new(0) };
}

/// What the memo did on this thread since the last [`take_memo_counters`].
#[derive(Default, Debug, Clone, PartialEq)]
pub struct MemoCounters {
    /// Child layouts restored from a recorded result (`on`).
    pub hits: u64,
    /// Child layouts that could have been served from the memo and were checked (`verify`).
    pub verified: u64,
    /// Memoisable child layouts with no recorded result.
    pub misses: u64,
    /// Results recorded.
    pub stored: u64,
    /// Results not recorded because a descendant depends on state outside the subtree.
    pub refused: u64,
    /// Descendant boxes written by restores.
    pub restored_boxes: u64,
}

pub fn take_memo_counters() -> MemoCounters {
    COUNTERS.with(|c| std::mem::take(&mut *c.borrow_mut()))
}

fn count(f: impl FnOnce(&mut MemoCounters)) {
    let mut d = MemoCounters::default();
    f(&mut d);
    COUNTERS.with(|c| {
        let mut c = c.borrow_mut();
        c.hits += d.hits;
        c.verified += d.verified;
        c.misses += d.misses;
        c.stored += d.stored;
        c.refused += d.refused;
        c.restored_boxes += d.restored_boxes;
    });
    // Process-wide totals count only runs whose mode came from the environment (a corpus run),
    // not the tests that set a mode on their thread.
    if MODE_OVERRIDE.with(|m| m.get()).is_none() {
        use std::sync::atomic::Ordering::Relaxed;
        TOTALS[0].fetch_add(d.hits, Relaxed);
        TOTALS[1].fetch_add(d.verified, Relaxed);
        TOTALS[2].fetch_add(d.misses, Relaxed);
        TOTALS[3].fetch_add(d.stored, Relaxed);
        TOTALS[4].fetch_add(d.refused, Relaxed);
        TOTALS[5].fetch_add(d.restored_boxes, Relaxed);
    }
}

static TOTALS: [std::sync::atomic::AtomicU64; 6] = [const { std::sync::atomic::AtomicU64::new(0) }; 6];

/// What the memo did in this process over every layout whose mode came from `RUSTKIT_LAYOUT_MEMO`
/// (not a per-thread override): the corpus run's evidence that it was exercised.
pub fn memo_totals() -> MemoCounters {
    use std::sync::atomic::Ordering::Relaxed;
    MemoCounters {
        hits: TOTALS[0].load(Relaxed),
        verified: TOTALS[1].load(Relaxed),
        misses: TOTALS[2].load(Relaxed),
        stored: TOTALS[3].load(Relaxed),
        refused: TOTALS[4].load(Relaxed),
        restored_boxes: TOTALS[5].load(Relaxed),
    }
}

fn env_mode() -> MemoMode {
    static MODE: std::sync::OnceLock<MemoMode> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("RUSTKIT_LAYOUT_MEMO").as_deref() {
        Ok("on") => MemoMode::On,
        Ok("verify") => MemoMode::Verify,
        _ => MemoMode::Off,
    })
}

fn env_min() -> usize {
    static MIN: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MIN.get_or_init(|| std::env::var("RUSTKIT_LAYOUT_MEMO_MIN").ok().and_then(|v| v.parse().ok()).unwrap_or(33))
}

pub fn memo_mode() -> MemoMode {
    MODE_OVERRIDE.with(|m| m.get()).unwrap_or_else(env_mode)
}

/// The smallest subtree, in descendant boxes, that is memoised.
pub fn memo_min_subtree() -> usize {
    MIN_OVERRIDE.with(|m| m.get()).unwrap_or_else(env_min)
}

/// Set this thread's memo mode, overriding `RUSTKIT_LAYOUT_MEMO` (`None` restores the variable).
pub fn set_memo_mode(mode: Option<MemoMode>) {
    MODE_OVERRIDE.with(|m| m.set(mode));
}

/// Set this thread's smallest memoised subtree, overriding `RUSTKIT_LAYOUT_MEMO_MIN`.
pub fn set_memo_min_subtree(min: Option<usize>) {
    MIN_OVERRIDE.with(|m| m.set(min));
}

/// Start a new root layout: results recorded before it are not trusted after it.
pub(crate) fn bump_epoch() {
    EPOCH.with(|e| e.set(e.get() + 1));
}

#[derive(Clone, PartialEq)]
pub(crate) struct MemoKey {
    width: u32,
    definite_height: Option<u32>,
    edges: u64,
    margin: MarginCollapseContext,
    inputs: u64,
}

/// The fields layout writes on a descendant, positions relative to the memoised box's content
/// origin. (`incremental.rs`'s `own_inputs_equal` destructure is the full field list; the others
/// are style-derived, set by the engine after layout, or set before it.) `formatting_root` is an input,
/// hashed into the key (`input_fingerprint`), not an output.
#[derive(Clone)]
struct BoxOutputs {
    dimensions: Dimensions,
    text_lines: Option<Vec<TextLine>>,
    text_flow_first_offset: Option<f32>,
    table_caption_shift: Option<(f32, f32)>,
    percent_height_is_auto: bool,
    context_floats: Option<FloatContext>,
    /// LAY1 bookkeeping (the (width, percentage base) the children were last laid out at, and a padding-box
    /// rect in absolute coordinates, so relative here):
    /// set or reset by every layout of the box, so a skipped layout must leave what it would have.
    children_laid_at: Option<(f32, f32)>,
    abspos_cb_hint: Option<Rect>,
    /// SET2: the pitch `set_wrapped_pitch` gave a wrapped text run, which the next visit reads back
    /// to take it out of the run's height again.
    text_line_pitch: Option<f32>,
}

struct MemoEntry {
    key: MemoKey,
    outputs: Vec<BoxOutputs>,
    end_height: f32,
    end_margin: MarginCollapseContext,
}

impl MemoEntry {
    fn new(key: MemoKey, outputs: Vec<BoxOutputs>, end_height: f32, end_margin: MarginCollapseContext) -> Self {
        LIVE.with(|n| n.set(n.get() + 1));
        MemoEntry { key, outputs, end_height, end_margin }
    }
}

impl Drop for MemoEntry {
    fn drop(&mut self) {
        let _ = LIVE.try_with(|n| n.set(n.get() - 1));
    }
}

/// How many recorded results (whole-subtree snapshots) are alive on this thread. Zero once a root
/// layout has finished: the memo is per root layout, so nothing may keep a snapshot after it.
pub fn live_memo_entries() -> i64 {
    LIVE.with(|n| n.get())
}

/// Drop every recorded result below `b` (a root layout has ended).
pub(crate) fn clear_memos(b: &mut LayoutBox) {
    b.layout_memo = None;
    for c in &mut b.children {
        clear_memos(c);
    }
}

/// A box's recorded child layouts for one root layout. A cloned box starts with none.
pub(crate) struct BoxMemo {
    epoch: u64,
    entries: Vec<MemoEntry>,
}

impl Clone for BoxMemo {
    fn clone(&self) -> Self {
        BoxMemo { epoch: 0, entries: Vec::new() }
    }
}

impl std::fmt::Debug for BoxMemo {
    // A record of layout, not part of what layout produced: verify mode compares Debug output.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("memo")
    }
}

/// True when `b` has at least `n` descendants. Stops counting at `n`.
fn descendants_at_least(b: &LayoutBox, n: usize) -> bool {
    fn walk(b: &LayoutBox, left: &mut usize) -> bool {
        for c in &b.children {
            if *left == 0 {
                return true;
            }
            *left -= 1;
            if walk(c, left) {
                return true;
            }
        }
        *left == 0
    }
    let mut left = n;
    n == 0 || walk(b, &mut left)
}

fn hash_edges(e: &EdgeSizes, h: &mut impl std::hash::Hasher) {
    use std::hash::Hash;
    for v in [e.top, e.right, e.bottom, e.left] {
        v.to_bits().hash(h);
    }
}

/// Hash of the layout INPUT state of `b` and below: how many children each box has, its percent-height
/// mark, whether it shares a float context, and whether it is a formatting root (flex and grid set
/// it on an item before laying it out, so the block pre-pass and the item pass differ in it and
/// enclose their floats differently). Outputs (positions, sizes, lines) are left out on
/// purpose: they differ between the calls the memo is meant to match.
fn input_fingerprint(b: &LayoutBox) -> u64 {
    use std::hash::{Hash, Hasher};
    fn walk(b: &LayoutBox, h: &mut impl Hasher) {
        b.children.len().hash(h);
        for c in &b.children {
            c.percent_height_is_auto.hash(h);
            c.context_floats.is_some().hash(h);
            c.formatting_root.hash(h);
            walk(c, h);
        }
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    b.formatting_root.hash(&mut h);
    walk(b, &mut h);
    h.finish()
}

fn key_for(b: &LayoutBox, definite_height: Option<f32>, margin: &MarginCollapseContext) -> MemoKey {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let d = &b.dimensions;
    hash_edges(&d.padding, &mut h);
    hash_edges(&d.border, &mut h);
    hash_edges(&d.margin, &mut h);
    hash_edges(&d.scrollbar, &mut h);
    d.content.width.to_bits().hash(&mut h);
    MemoKey {
        width: d.content.width.to_bits(),
        definite_height: definite_height.map(f32::to_bits),
        edges: h.finish(),
        margin: margin.clone(),
        inputs: input_fingerprint(b),
    }
}

/// Every field of a box, classed for the memo. The destructure names every field, so a new one does
/// not compile until it is classed here: a field layout WRITES on a descendant must be in
/// `BoxOutputs` (snapshot, restore and `close_to`), or a memo hit leaves it stale. (Verify mode found
/// `text_line_pitch` missing on a live page, after #1090 added it; this is the check that would
/// have caught it at compile time.)
#[allow(dead_code)]
fn classify_every_field(b: &LayoutBox) {
    let LayoutBox {
        // Inputs: style, tag, text and tree shape. The key covers what layout reads of them, and a
        // different shape fails the restore.
        box_type: _,
        style: _,
        children: _,
        position: _,
        float: _,
        clear: _,
        z_index: _,
        text_sources: _,
        node_id: _,
        element_id: _,
        identity: _,
        link_href: _,
        focused_caret: _,
        focused_selection: _,
        root_element_height: _,
        table_span: _,
        offsets: _,
        viewport: _,
        // Built before layout, from the tree, not by layout.
        stacking_context: _,
        containing_block_index: _,
        pseudo_animation_key: _,
        // Written by layout and held in `BoxOutputs`.
        dimensions: _,
        text_lines: _,
        text_flow_first_offset: _,
        table_caption_shift: _,
        percent_height_is_auto: _,
        context_floats: _,
        children_laid_at: _,
        abspos_cb_hint: _,
        text_line_pitch: _,
        // An input, set by flex and grid on an item before laying it out (the block pre-pass lays
        // the same item out without it), so `input_fingerprint` hashes it for the box and below.
        formatting_root: _,
        // The memo itself.
        layout_memo: _,
        // Stamped after layout (scroll, sticky, scrollbar passes), never by the layout call.
        sticky_state: _,
        element_scroll: _,
        sticky_rule: _,
        sticky_shift: _,
        sticky_paths: _,
        scrollbar_thumb_hovered: _,
        scrollbar_overflows: _,
        // A note the hit test keeps beside the layout: made from the root on the first hit test
        // (`mark_hit_overflow` walks the whole tree) and forgotten by the engine on each new layout.
        hit_overflow: _,
        // The paint order the hit test keeps, emptied by that same walk.
        stack_order: _,
        stack_gen: _,
    } = b;
}

/// The recorded outputs of every descendant of `b` (pre-order), or `None` when one of them
/// depends on state outside the subtree.
fn snapshot(b: &LayoutBox) -> Option<Vec<BoxOutputs>> {
    fn walk(b: &LayoutBox, ox: f32, oy: f32, out: &mut Vec<BoxOutputs>) -> bool {
        for c in &b.children {
            // Fixed boxes are placed against the viewport and sticky ones against a scroll
            // container, neither of which moves with this box's origin, so a translated restore
            // would be wrong for them. An absolute box is anchored to its PARENT's padding box
            // (`reanchor_absolute_children`), always inside this subtree, so it translates like
            // any other descendant. A stacking context is a z-order record built before layout.
            if c.sticky_state.is_some()
                || matches!(c.position, Position::Fixed | Position::Sticky)
                || matches!(c.style.position, rustkit_css::Position::Fixed | rustkit_css::Position::Sticky)
            {
                return false;
            }
            let mut dimensions = c.dimensions.clone();
            dimensions.content.x -= ox;
            dimensions.content.y -= oy;
            let context_floats = c.context_floats.as_ref().map(|f| f.translated(-ox, -oy));
            out.push(BoxOutputs {
                dimensions,
                text_lines: c.text_lines.clone(),
                text_flow_first_offset: c.text_flow_first_offset,
                table_caption_shift: c.table_caption_shift,
                percent_height_is_auto: c.percent_height_is_auto,
                context_floats,
                children_laid_at: c.children_laid_at,
                abspos_cb_hint: c.abspos_cb_hint.map(|r| Rect { x: r.x - ox, y: r.y - oy, ..r }),
                text_line_pitch: c.text_line_pitch,
            });
            if !walk(c, ox, oy, out) {
                return false;
            }
        }
        true
    }
    let (ox, oy) = (b.dimensions.content.x, b.dimensions.content.y);
    let mut out = Vec::new();
    walk(b, ox, oy, &mut out).then_some(out)
}

/// Write `outputs` onto the descendants of `b`, translated to `b`'s content origin. `false`, with
/// nothing written, when the tree below `b` has a different shape from the one recorded.
fn restore(b: &mut LayoutBox, outputs: &[BoxOutputs]) -> bool {
    fn count(b: &LayoutBox) -> usize {
        b.children.iter().map(|c| 1 + count(c)).sum()
    }
    if count(b) != outputs.len() {
        return false;
    }
    fn walk(b: &mut LayoutBox, ox: f32, oy: f32, it: &mut std::slice::Iter<'_, BoxOutputs>) {
        for c in &mut b.children {
            let Some(o) = it.next() else { return };
            c.dimensions = o.dimensions.clone();
            c.dimensions.content.x += ox;
            c.dimensions.content.y += oy;
            c.text_lines = o.text_lines.clone();
            c.text_flow_first_offset = o.text_flow_first_offset;
            c.table_caption_shift = o.table_caption_shift;
            c.percent_height_is_auto = o.percent_height_is_auto;
            c.context_floats = o.context_floats.as_ref().map(|f| f.translated(ox, oy));
            c.children_laid_at = o.children_laid_at;
            c.abspos_cb_hint = o.abspos_cb_hint.map(|r| Rect { x: r.x + ox, y: r.y + oy, ..r });
            c.text_line_pitch = o.text_line_pitch;
            walk(c, ox, oy, it);
        }
    }
    let (ox, oy) = (b.dimensions.content.x, b.dimensions.content.y);
    walk(b, ox, oy, &mut outputs.iter());
    true
}

/// Round every decimal number in `s` to two places, so two Debug dumps that differ only by float
/// rounding from a translation compare equal.
fn rounded(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        let starts_number = c.is_ascii_digit() || (c == b'-' && bytes.get(i + 1).is_some_and(|d| d.is_ascii_digit()));
        if starts_number && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_digit() || bytes[j] == b'.' || bytes[j] == b'e' || bytes[j] == b'-') {
                j += 1;
            }
            match s[i..j].parse::<f64>() {
                Ok(v) => out.push_str(&format!("{:.2}", if v == 0.0 { 0.0 } else { v })),
                Err(_) => out.push_str(&s[i..j]),
            }
            i = j;
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    out
}

const EPS: f32 = 0.01;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= EPS || (a.is_nan() && b.is_nan())
}

fn close_edges(a: &EdgeSizes, b: &EdgeSizes) -> bool {
    close(a.top, b.top) && close(a.right, b.right) && close(a.bottom, b.bottom) && close(a.left, b.left)
}

fn close_rect(a: &Rect, b: &Rect) -> bool {
    close(a.x, b.x) && close(a.y, b.y) && close(a.width, b.width) && close(a.height, b.height)
}

fn close_floats(a: &Option<FloatContext>, b: &Option<FloatContext>) -> bool {
    fn side(a: &[FloatExclusion], b: &[FloatExclusion]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| close_rect(&x.rect, &y.rect) && format!("{:?}", x.float_type) == format!("{:?}", y.float_type))
    }
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => side(&a.left_floats, &b.left_floats) && side(&a.right_floats, &b.right_floats),
        _ => false,
    }
}

impl BoxOutputs {
    /// The same layout result, up to float rounding from translating positions.
    fn close_to(&self, other: &BoxOutputs) -> bool {
        let (a, b) = (&self.dimensions, &other.dimensions);
        close_rect(&a.content, &b.content)
            && close_edges(&a.padding, &b.padding)
            && close_edges(&a.border, &b.border)
            && close_edges(&a.margin, &b.margin)
            && close_edges(&a.scrollbar, &b.scrollbar)
            && match (&self.text_lines, &other.text_lines) {
                (None, None) => true,
                (Some(x), Some(y)) => {
                    x.len() == y.len()
                        && x.iter().zip(y).all(|(p, q)| {
                            p.text == q.text && close(p.width, q.width) && close(p.x_offset, q.x_offset) && close(p.justify_space, q.justify_space)
                        })
                }
                _ => false,
            }
            && match (self.text_flow_first_offset, other.text_flow_first_offset) {
                (None, None) => true,
                (Some(x), Some(y)) => close(x, y),
                _ => false,
            }
            && match (self.table_caption_shift, other.table_caption_shift) {
                (None, None) => true,
                (Some((a1, a2)), Some((b1, b2))) => close(a1, b1) && close(a2, b2),
                _ => false,
            }
            && self.percent_height_is_auto == other.percent_height_is_auto
            && close_floats(&self.context_floats, &other.context_floats)
            && match (self.children_laid_at, other.children_laid_at) {
                (None, None) => true,
                (Some((w1, b1)), Some((w2, b2))) => close(w1, w2) && close(b1, b2),
                _ => false,
            }
            && match (&self.abspos_cb_hint, &other.abspos_cb_hint) {
                (None, None) => true,
                (Some(x), Some(y)) => close_rect(x, y),
                _ => false,
            }
            && match (self.text_line_pitch, other.text_line_pitch) {
                (None, None) => true,
                (Some(a), Some(b)) => close(a, b),
                _ => false,
            }
    }
}

/// Subtrees up to this many boxes also get the whole-box Debug comparison in verify mode, which
/// catches a layout-written field the snapshot does not hold. Larger ones are compared field by
/// field only: formatting a thousand-box subtree on every hit made verify unusable on real pages.
const DEBUG_COMPARE_MAX: usize = 64;

/// Subtrees up to this size get the restore round-trip check on every verified hit; larger ones on
/// every 32nd (cloning a thousand-box subtree per hit is what made verify unusable).
const ROUNDTRIP_MAX: usize = 300;

impl LayoutBox {
    /// Lay out this box's children, or write back the result recorded for an earlier call with the
    /// same inputs (LAT21; see the module docs).
    pub(crate) fn layout_block_children_with_collapse(
        &mut self,
        margin_context: &mut MarginCollapseContext,
        float_context: &mut FloatContext,
        definite_height: Option<f32>,
    ) {
        let mode = memo_mode();
        if mode == MemoMode::Off
            || self.style.display.is_table()
            || !float_context.is_empty()
            || self.context_floats.is_some()
            || !descendants_at_least(self, memo_min_subtree())
        {
            return self.layout_block_children_with_collapse_run(margin_context, float_context, definite_height);
        }
        let epoch = EPOCH.with(|e| e.get());
        let key = key_for(self, definite_height, margin_context);
        let mut memo = self
            .layout_memo
            .take()
            .filter(|m| m.epoch == epoch)
            .unwrap_or_else(|| Box::new(BoxMemo { epoch, entries: Vec::new() }));
        let mut hit = memo.entries.iter().position(|e| e.key == key);

        let mut predicted: Option<String> = None;
        let mut verifying = false;
        if let Some(i) = hit {
            let entry = memo.entries.remove(i);
            match mode {
                MemoMode::On => {
                    // A recorded result for a tree of another shape cannot be written back: it is
                    // dropped, and this call is a miss that records a fresh one.
                    if restore(self, &entry.outputs) {
                        count(|c| {
                            c.hits += 1;
                            c.restored_boxes += entry.outputs.len() as u64;
                        });
                        self.dimensions.content.height = entry.end_height;
                        *margin_context = entry.end_margin.clone();
                        memo.entries.insert(0, entry);
                        self.layout_memo = Some(memo);
                        return;
                    }
                }
                MemoMode::Verify => {
                    // What the restore would leave, on a copy: the real layout below then starts
                    // from the state it would have had without the memo.
                    let n = entry.outputs.len();
                    let sampled = VERIFY_HITS.with(|c| {
                        c.set(c.get() + 1);
                        c.get() % 32 == 0
                    });
                    if n <= ROUNDTRIP_MAX || sampled {
                        let mut probe = self.clone();
                        if restore(&mut probe, &entry.outputs) {
                            probe.dimensions.content.height = entry.end_height;
                            // The restore must write back every field the snapshot holds.
                            let back = snapshot(&probe);
                            assert!(
                                back.as_ref().is_some_and(|b| {
                                    b.len() == entry.outputs.len() && b.iter().zip(&entry.outputs).all(|(a, r)| a.close_to(r))
                                }),
                                "LAT21 layout memo verify: restoring the recorded result does not reproduce it ({n} descendants)"
                            );
                            if n <= DEBUG_COMPARE_MAX {
                                predicted = Some(format!("{:?}", probe));
                            }
                        }
                    }
                    verifying = true;
                }
                MemoMode::Off => {}
            }
            if mode == MemoMode::On {
                // Reaching here in `on` means the restore failed (a success returned above).
                count(|c| c.misses += 1);
                hit = None;
            } else {
                memo.entries.insert(0, entry);
            }
        } else {
            count(|c| c.misses += 1);
        }

        self.layout_block_children_with_collapse_run(margin_context, float_context, definite_height);

        if verifying {
            count(|c| c.verified += 1);
            let recorded = &memo.entries[0];
            let ok = match snapshot(self) {
                Some(actual) => {
                    actual.len() == recorded.outputs.len()
                        && actual.iter().zip(&recorded.outputs).all(|(a, r)| a.close_to(r))
                        && close(self.dimensions.content.height, recorded.end_height)
                        && format!("{:?}", margin_context) == format!("{:?}", recorded.end_margin)
                }
                None => false,
            };
            assert!(
                ok,
                "LAT21 layout memo verify: restoring the recorded result would differ from laying this box's children out ({} descendants)",
                recorded.outputs.len()
            );
            if let Some(predicted) = predicted {
                let actual = format!("{:?}", self);
                assert!(
                    rounded(&predicted) == rounded(&actual),
                    "LAT21 layout memo verify: a field the snapshot does not hold differs\nrecorded: {:.1500}\n actual:   {:.1500}",
                    rounded(&predicted),
                    rounded(&actual)
                );
            }
        }
        if hit.is_none() {
            match snapshot(self) {
                Some(outputs) => {
                    count(|c| c.stored += 1);
                    memo.entries.insert(
                        0,
                        MemoEntry::new(key, outputs, self.dimensions.content.height, margin_context.clone()),
                    );
                    memo.entries.truncate(2);
                }
                None => count(|c| c.refused += 1),
            }
        }
        self.layout_memo = Some(memo);
    }
}

#[cfg(test)]
mod restore_tests {
    use super::*;
    use crate::BoxType;

    /// A memo hit writes the recorded (width, base) back, both components: LAY1's re-anchor skip
    /// compares the width, so a restore that dropped it would leave a stale one.
    #[test]
    fn a_restore_writes_back_the_width_and_the_base_of_children_laid_at() {
        let style = rustkit_css::ComputedStyle::default();
        let mut parent = LayoutBox::new(BoxType::Block, style.clone());
        let mut child = LayoutBox::new(BoxType::Block, style.clone());
        child.children_laid_at = Some((321.0, 45.0));
        parent.children.push(child);
        let outputs = snapshot(&parent).expect("snapshot");
        parent.children[0].children_laid_at = Some((7.0, 99.0));
        assert!(restore(&mut parent, &outputs));
        assert_eq!(parent.children[0].children_laid_at, Some((321.0, 45.0)));
        parent.children[0].children_laid_at = None;
        assert!(restore(&mut parent, &outputs));
        assert_eq!(parent.children[0].children_laid_at, Some((321.0, 45.0)));
    }

    /// `set_wrapped_pitch` (#1090) writes `text_line_pitch` on a wrapped text run and reads it back
    /// on the next visit to take the pitch out of the run's height again, so a restore that left it
    /// stale would make the next visit compute the wrong line height. Verify mode found the gap on
    /// a live page ("a field the snapshot does not hold differs").
    #[test]
    fn a_restore_writes_back_the_text_line_pitch() {
        let style = rustkit_css::ComputedStyle::default();
        let mut parent = LayoutBox::new(BoxType::Block, style.clone());
        let mut run = LayoutBox::new(BoxType::Text("a b".to_string()), style);
        run.text_line_pitch = Some(40.0);
        parent.children.push(run);
        let outputs = snapshot(&parent).expect("snapshot");
        for stale in [None, Some(7.0)] {
            parent.children[0].text_line_pitch = stale;
            assert!(restore(&mut parent, &outputs));
            assert_eq!(parent.children[0].text_line_pitch, Some(40.0), "stale {stale:?}");
        }
        // The verify comparison sees a pitch that differs.
        let mut other = outputs.clone();
        other[0].text_line_pitch = None;
        assert!(!outputs[0].close_to(&other[0]));
    }

    /// `formatting_root` is an INPUT to a box's own layout (it encloses its floats), and the same
    /// item is laid out twice: by the block pre-pass (not a root, height 0 around a float-only
    /// child) and by its grid or flex parent (a root, height 30). A key that left it out restored
    /// the first answer for the second. Found by `float_in_items_unit_tests` under the memo.
    #[test]
    fn a_grid_item_that_becomes_a_formatting_root_is_not_restored_from_its_block_pass() {
        use rustkit_css::{Display, Float, GridTemplate, Length, TrackDefinition, TrackSize};
        let float = {
            let mut st = rustkit_css::ComputedStyle::new();
            st.width = Length::Px(10.0);
            st.height = Length::Px(30.0);
            st.float = Float::Left;
            LayoutBox::with_float(BoxType::Block, st, Float::Left)
        };
        let mut grid_style = rustkit_css::ComputedStyle::new();
        grid_style.display = Display::Grid;
        grid_style.grid_template_columns = GridTemplate {
            tracks: vec![TrackDefinition::simple(TrackSize::Px(400.0))],
            ..Default::default()
        };
        let mut item = LayoutBox::new(BoxType::Block, rustkit_css::ComputedStyle::new());
        item.children.push(float);
        let mut grid = LayoutBox::new(BoxType::Block, grid_style);
        grid.children.push(item);
        let mut body = LayoutBox::new(BoxType::Block, rustkit_css::ComputedStyle::new());
        body.children.push(grid);

        set_memo_mode(Some(MemoMode::On));
        set_memo_min_subtree(Some(1));
        body.set_viewport(1280.0, 800.0);
        let cb = Dimensions { content: Rect::new(0.0, 0.0, 400.0, 0.0), ..Default::default() };
        body.layout_with_collapse(&cb, &mut MarginCollapseContext::new(), &mut FloatContext::new());
        set_memo_mode(None);
        set_memo_min_subtree(None);
        let h = body.children[0].children[0].dimensions.border_box().height;
        assert!((h - 30.0).abs() < 0.5, "grid float-only item height {h} with the memo on; Chromium 30");
    }
}
