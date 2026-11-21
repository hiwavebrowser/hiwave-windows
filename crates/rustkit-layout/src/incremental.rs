//! Incremental layout, stages 1 and 2 (`docs/design/INCREMENTAL_LAYOUT_PLAN.md`).
//!
//! Given the box tree the last layout started from (unlaid), the tree it
//! produced (laid out), and a freshly built unlaid tree, find the boxes
//! whose content changed. Each is hoisted to a relayout root R: the nearest
//! box at or above it that can be laid out alone (in flow, its margins
//! closed off or its edge children inline-level, nothing in it anchored
//! outside it) and whose ancestors up to a boundary only grow by R's
//! height delta. R is laid out alone where the full pass put it; what
//! follows it moves by the delta, and every ancestor up to the boundary
//! grows by it. A boundary is the root or a block whose height cannot
//! depend on its content; boxes after it do not move.
//!
//! Everything else returns a [`Fallback`] reason and the caller lays the
//! whole tree out, which is always correct. The engine's verify mode runs
//! both and compares every box rect ([`compare_rects`]).

use crate::{
    BoxType, Dimensions, EdgeSizes, FloatContext, LayoutBox, MarginCollapseContext, Position, Rect,
};
use rustkit_css::{Clear, ContainerType, Display, Float, Length, WritingMode};
use std::cell::Cell;

thread_local! {
    static VISITS: Cell<u64> = const { Cell::new(0) };
}

/// Count one box laid out (the layout entry points call this).
pub(crate) fn count_visit() {
    VISITS.with(|v| v.set(v.get().wrapping_add(1)));
}

/// How many boxes the layout entry points have laid out on this thread, in
/// all. Tests and the engine read the difference across a layout.
pub fn layout_visits() -> u64 {
    VISITS.with(Cell::get)
}

/// Why the incremental path did not apply. The full layout is the answer.
pub type Fallback = &'static str;

/// What an incremental relayout did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Relaid {
    /// Child-index paths from the root to each relayout root laid out
    /// again, in document order; empty when nothing changed and the old
    /// layout stands as it is.
    pub roots: Vec<Vec<usize>>,
    /// Boxes laid out (the entry-point count across the relayout).
    pub visits: u64,
    /// Boxes moved by a delta, after a relayout root.
    pub moved: u64,
    /// Boxes compared by the diff (no layout).
    pub compared: u64,
    /// Levels the changed boxes were hoisted, in all.
    pub hoists: u32,
    /// Relayout roots whose delta stopped at a boundary below the root.
    pub bounded: u32,
    /// The sum of the roots' border-box bottom deltas.
    pub delta: f32,
    /// Time in the tree diff, in milliseconds.
    pub diff_ms: f64,
}

/// One relayout root and where its delta stops.
#[derive(Debug, Clone)]
struct Plan {
    root: Vec<usize>,
    /// Depth of the boundary on the root's path, if one is below the
    /// tree's root. Ancestors deeper than it grow by the delta; it and they
    /// move the children after the path. `None`: every ancestor grows.
    stop: Option<usize>,
    hoists: u32,
    /// The root is a flex item whose container's algorithm reads nothing
    /// that changed (stage 6a): its border box must come out the same.
    exact: bool,
}

/// Lay `fresh` out by updating `laid` (the layout of `old_unlaid`) in place.
///
/// On `Ok`, `laid` is what a full layout of `fresh` gives. On `Err`, `laid`
/// may be partly updated and must be dropped; the caller lays `fresh` out in
/// full.
pub fn relayout(
    old_unlaid: &LayoutBox,
    fresh: &LayoutBox,
    laid: &mut LayoutBox,
) -> Result<Relaid, Fallback> {
    let mut out = Relaid::default();
    let started = std::time::Instant::now();
    let changed = nest_free(changed_roots(old_unlaid, fresh, &mut out.compared)?);
    out.diff_ms = started.elapsed().as_secs_f64() * 1000.0;
    if changed.is_empty() {
        refresh_labels(laid, fresh);
        return Ok(out);
    }
    let mut plans = Vec::new();
    for path in changed {
        plans.push(plan_for(laid, fresh, old_unlaid, path)?);
    }
    let roots = nest_free(plans.iter().map(|p| p.root.clone()).collect());
    plans.retain(|p| roots.contains(&p.root));
    plans.dedup_by(|a, b| a.root == b.root);
    plans.sort_by(|a, b| a.root.cmp(&b.root));
    // A root holding most of the page costs more laid out alone than the
    // full pass (a flex shell around the whole article: 14 ms against 8 on
    // a 6k-element page, measured in the parity profile): fall back.
    // On a small page both take microseconds: only large trees are checked.
    let in_roots: u64 = plans.iter().filter_map(|p| at(fresh, &p.root)).map(count).sum();
    let all = count(fresh);
    if all >= 2000 && in_roots * 2 > all {
        return Err("root_large");
    }
    // Last first: a root's delta then moves the later roots already laid
    // out, and each is laid out at its own old position.
    for plan in plans.iter().rev() {
        let (nb, delta) = lay_out_root(laid, fresh, plan, &mut out.visits)?;
        if plan.exact {
            let old = at(laid, &plan.root).ok_or("shape")?.dimensions.border_box();
            let new = nb.dimensions.border_box();
            if !close(old.width, new.width) || !close(old.height, new.height) || delta.abs() > 0.01 {
                return Err("flex_item_resized");
            }
        }
        out.moved += splice(laid, &plan.root, 0, plan.stop, nb, delta)?;
        out.delta += delta;
        out.hoists += plan.hoists;
        out.bounded += u32::from(plan.stop.is_some());
    }
    out.roots = plans.into_iter().map(|p| p.root).collect();
    refresh_labels(laid, fresh);
    Ok(out)
}

/// The fresh build's element numbers and identities, onto the laid-out
/// tree (same shape by now): the diff ignores them, the painted tree must
/// carry the current ones.
fn refresh_labels(laid: &mut LayoutBox, fresh: &LayoutBox) {
    if laid.element_id != fresh.element_id {
        laid.element_id = fresh.element_id;
    }
    if laid.identity != fresh.identity {
        laid.identity = fresh.identity.clone();
    }
    for (l, f) in laid.children.iter_mut().zip(&fresh.children) {
        refresh_labels(l, f);
    }
}

/// Lay the box at `plan.root` out alone, from `fresh`, where the full pass
/// put it in `laid`. Returns the laid-out box and its border-box delta.
fn lay_out_root(
    laid: &LayoutBox,
    fresh: &LayoutBox,
    plan: &Plan,
    visits: &mut u64,
) -> Result<(LayoutBox, f32), Fallback> {
    let path = &plan.root;
    let old_b = at(laid, path).ok_or("shape")?;
    let new_b = at(fresh, path).ok_or("shape")?;
    let parent = at(laid, &path[..path.len() - 1]).ok_or("shape")?;
    let mut nb = new_b.clone();
    nb.set_viewport(old_b.viewport.0, old_b.viewport.1);
    nb.percent_height_is_auto = old_b.percent_height_is_auto;
    // Its parent's child loop hands an in-flow block that shares its
    // formatting context the floats so far; none reach it (`scan`).
    let shares_floats = matches!(nb.box_type, BoxType::Block | BoxType::AnonymousBlock)
        && !nb.style.display.is_atomic_inline()
        && !crate::margin_collapse::establishes_bfc(&nb.style, nb.float);
    nb.context_floats = shares_floats.then(FloatContext::new);
    // Its top margin counts as already adjoined (the context resolves to
    // 0), so its content edge lands at the old one.
    let d = &old_b.dimensions;
    let mut cb = parent.dimensions.clone();
    cb.content.x = d.content.x - d.margin.left - d.border_with_gutter().left - d.padding.left;
    cb.content.y = d.content.y - d.border_with_gutter().top - d.padding.top;
    cb.content.height = 0.0;
    let mut margins = MarginCollapseContext::new();
    margins.first_child_top_adjoined = true;
    // A flex or grid item is a formatting root: its children's margins
    // stay inside it, as the container's own layout of it has them.
    margins.children_are_formatting_roots =
        parent.style.display.is_flex() || parent.style.display.is_grid();
    // A fixed box under here is placed again by the root pass
    // (`reanchor_fixed_boxes`), against a containing block that may be
    // above this subtree: the whole tree is laid out instead. Asked before
    // this subtree is laid out, so it is not laid out to be thrown away.
    if nb.has_fixed_box_below() {
        return Err("fixed_box");
    }
    let before = layout_visits();
    nb.layout_with_collapse_in(&cb, &mut margins, &mut FloatContext::new(), None);
    // The root pass's relative offsets, for what is under the box (its own
    // is in the old position it took).
    if !matches!(
        nb.box_type,
        BoxType::Inline | BoxType::Text(_) | BoxType::LineBreak
    ) {
        let inner = nb.dimensions.clone();
        for child in &mut nb.children {
            LayoutBox::shift_relative_subtree(child, &inner);
        }
    }
    *visits += layout_visits() - before;
    nb.context_floats = None;
    if margins.child_collapsed_through {
        return Err("collapse_through");
    }
    let closed = edge_closed_top(old_b) && edge_closed_bottom(old_b);
    let (old_h, new_h) = (d.border_box().height, nb.dimensions.border_box().height);
    if (old_h <= 0.0 || new_h <= 0.0) && !closed {
        return Err("collapse_through");
    }
    let nd = &nb.dimensions;
    if !close(nd.content.x, d.content.x)
        || !close(nd.content.y, d.content.y)
        || !close(nd.content.width, d.content.width)
        || !edges_close(&nd.margin, &d.margin)
    {
        return Err("geometry_moved");
    }
    // Stage 3, the part that needs no resumed child loop: margins that
    // collapse through an open edge with the first (last) child's, down the
    // chain of open edges, sit outside the box. The full pass adjoined the
    // top chain above the box and leaves the bottom chain pending after it;
    // when both chains are the margins they were, the box's place and what
    // follows it are as before but for the delta.
    let same = |top: bool| {
        matches!((edge_chain(old_b, top), edge_chain(&nb, top)), (Some(a), Some(b)) if a == b)
    };
    if !edge_closed_top(old_b) && !same(true) {
        return Err("margin_collapse_top");
    }
    if !edge_closed_bottom(old_b) && !same(false) {
        return Err("margin_collapse_bottom");
    }
    let delta = nb.dimensions.border_box().bottom() - d.border_box().bottom();
    Ok((nb, delta))
}

fn edge_child(b: &LayoutBox, top: bool) -> Option<&LayoutBox> {
    let mut in_flow = b.children.iter().filter(|c| {
        !matches!(c.position, Position::Absolute | Position::Fixed) && c.float == Float::None
    });
    if top {
        in_flow.next()
    } else {
        in_flow.last()
    }
}

/// Before layout: down the chain of open edges of the laid-out box `l`,
/// the first (`top`) or last in-flow child is the same box as built last
/// time (`u`) and now (`n`), and had height. Same inputs give the same
/// margins at the same width; `edge_chain` confirms after layout.
fn chain_stays(l: &LayoutBox, n: &LayoutBox, u: &LayoutBox, top: bool) -> bool {
    let (mut l, mut n, mut u) = (l, n, u);
    loop {
        let open = if top {
            !edge_closed_top(l)
        } else {
            !edge_closed_bottom(l)
        };
        if !open {
            return true;
        }
        match (edge_child(l, top), edge_child(n, top), edge_child(u, top)) {
            (None, None, None) => return true,
            (Some(lc), Some(nc), Some(uc)) => {
                if is_inline_level(lc) && is_inline_level(nc) {
                    return true;
                }
                if !own_inputs_equal(nc, uc) || lc.dimensions.border_box().height <= 0.0 {
                    return false;
                }
                (l, n, u) = (lc, nc, uc);
            }
            _ => return false,
        }
    }
}

/// The margins of a laid-out box's chain of first (`top`) or last in-flow
/// children while its edge is open, each with whether the child has height
/// (an empty child collapses through and joins its sibling's margin:
/// `None`, so the chains never compare equal). Rounded to 0.01 px.
fn edge_chain(b: &LayoutBox, top: bool) -> Option<Vec<i64>> {
    let mut out = Vec::new();
    let mut cur = b;
    loop {
        let open = if top {
            !edge_closed_top(cur)
        } else {
            !edge_closed_bottom(cur)
        };
        if !open {
            return Some(out);
        }
        let Some(child) = edge_child(cur, top) else {
            return Some(out);
        };
        if is_inline_level(child) {
            return Some(out);
        }
        if child.dimensions.border_box().height <= 0.0 {
            return None;
        }
        let m = if top {
            child.dimensions.margin.top
        } else {
            child.dimensions.margin.bottom
        };
        out.push((m * 100.0).round() as i64);
        cur = child;
    }
}

/// Drop every path that has another (or an equal one) as a prefix, sorted.
fn nest_free(mut paths: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
    paths.sort();
    let mut kept: Vec<Vec<usize>> = Vec::new();
    for p in paths {
        if !kept.iter().any(|k| p.starts_with(k)) {
            kept.push(p);
        }
    }
    kept
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() <= 0.01
}

fn edges_close(a: &EdgeSizes, b: &EdgeSizes) -> bool {
    close(a.top, b.top)
        && close(a.right, b.right)
        && close(a.bottom, b.bottom)
        && close(a.left, b.left)
}

fn at<'a>(root: &'a LayoutBox, path: &[usize]) -> Option<&'a LayoutBox> {
    let mut b = root;
    for &i in path {
        b = b.children.get(i)?;
    }
    Some(b)
}

/// Replace the box at `path` with `nb`. At every level from the boundary
/// (`stop`) down, move the children after the path by `delta`; below the
/// boundary, grow the box by it. Returns how many boxes moved.
fn splice(
    node: &mut LayoutBox,
    path: &[usize],
    depth: usize,
    stop: Option<usize>,
    nb: LayoutBox,
    delta: f32,
) -> Result<u64, Fallback> {
    let (&i, rest) = path.split_first().expect("non-empty path");
    let mut moved = if rest.is_empty() {
        node.children[i] = nb;
        0
    } else {
        splice(&mut node.children[i], rest, depth + 1, stop, nb, delta)?
    };
    if delta != 0.0 && stop.map_or(true, |s| depth >= s) {
        for later in &mut node.children[i + 1..] {
            moved += count(later);
            LayoutBox::shift_subtree(later, 0.0, delta);
        }
        if stop.map_or(true, |s| depth > s) {
            let (old, new) = (
                node.dimensions.content.height,
                node.dimensions.content.height + delta,
            );
            let (min, max) = content_height_clamps(node);
            if min.is_some_and(|m| old <= m + 0.01 || new < m - 0.01)
                || max.is_some_and(|m| old >= m - 0.01 || new > m + 0.01)
            {
                return Err("ancestor_min_max");
            }
            node.dimensions.content.height = new;
        }
    }
    Ok(moved)
}

/// A growing ancestor's `min-height` and `max-height` as content heights
/// (`None` when unset). Percentages were refused by `classify_ancestor`.
fn content_height_clamps(b: &LayoutBox) -> (Option<f32>, Option<f32>) {
    let s = &b.style;
    let frame = if s.box_sizing == rustkit_css::BoxSizing::BorderBox {
        b.dimensions.padding.vertical() + b.dimensions.border_with_gutter().vertical()
    } else {
        0.0
    };
    let resolve = |l: &Length| match l {
        Length::Auto | Length::Zero => None,
        l => Some(b.length_to_px(l, 0.0) - frame),
    };
    (resolve(&s.min_height), resolve(&s.max_height))
}

fn count(b: &LayoutBox) -> u64 {
    1 + b.children.iter().map(count).sum::<u64>()
}

/// The child-index paths to the boxes whose children changed, or a
/// fallback when the root itself changed. Empty when the trees are equal.
fn changed_roots(
    old: &LayoutBox,
    new: &LayoutBox,
    compared: &mut u64,
) -> Result<Vec<Vec<usize>>, Fallback> {
    *compared += 1;
    if !own_inputs_equal(old, new) {
        return Err("root_changed");
    }
    let mut out = Vec::new();
    collect_changed(old, new, &mut Vec::new(), &mut out, compared);
    Ok(out)
}

fn collect_changed(
    a: &LayoutBox,
    b: &LayoutBox,
    path: &mut Vec<usize>,
    out: &mut Vec<Vec<usize>>,
    compared: &mut u64,
) {
    if a.children.len() != b.children.len() {
        out.push(path.clone());
        return;
    }
    let mut changed_here = false;
    for (i, (ca, cb)) in a.children.iter().zip(&b.children).enumerate() {
        if subtree_equal(ca, cb, compared) {
            continue;
        }
        if matches!(cb.box_type, BoxType::Block | BoxType::AnonymousBlock)
            && own_inputs_equal(ca, cb)
        {
            path.push(i);
            collect_changed(ca, cb, path, out, compared);
            path.pop();
        } else {
            changed_here = true;
        }
    }
    if changed_here {
        out.push(path.clone());
    }
}

fn subtree_equal(a: &LayoutBox, b: &LayoutBox, compared: &mut u64) -> bool {
    *compared += 1;
    own_inputs_equal(a, b)
        && a.children.len() == b.children.len()
        && a.children
            .iter()
            .zip(&b.children)
            .all(|(x, y)| subtree_equal(x, y, compared))
}

// Structural equality for the inputs the diff reads on every box (it was a
// Debug-string comparison, two allocations per field per box: most of the
// diff's time on a 6k-element page, Prometheus #1063 LOW 5). Each
// destructures exhaustively, so a new field fails to compile until it is
// compared. `==` on floats: -0 equals 0 and NaN differs, which is
// conservative.
fn opt_eq<T>(a: &Option<T>, b: &Option<T>, eq: impl Fn(&T, &T) -> bool) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => eq(x, y),
        _ => false,
    }
}

fn offsets_eq(a: &crate::PositionOffsets, b: &crate::PositionOffsets) -> bool {
    let crate::PositionOffsets { top, right, bottom, left } = a;
    *top == b.top && *right == b.right && *bottom == b.bottom && *left == b.left
}

fn stacking_eq(a: &crate::StackingContext, b: &crate::StackingContext) -> bool {
    let crate::StackingContext { z_index, creates_context, positioned_children } = a;
    *z_index == b.z_index
        && *creates_context == b.creates_context
        && *positioned_children == b.positioned_children
}

fn sticky_eq(a: &crate::scroll::StickyState, b: &crate::scroll::StickyState) -> bool {
    let crate::scroll::StickyState { original_rect, stuck_rect, offsets, is_stuck } = a;
    let crate::scroll::StickyOffsets { top, right, bottom, left } = offsets;
    rect_eq(original_rect, &b.original_rect)
        && opt_eq(stuck_rect, &b.stuck_rect, rect_eq)
        && *top == b.offsets.top
        && *right == b.offsets.right
        && *bottom == b.offsets.bottom
        && *left == b.offsets.left
        && *is_stuck == b.is_stuck
}

fn lines_eq(a: &[crate::TextLine], b: &[crate::TextLine]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let crate::TextLine { text, width, x_offset, justify_space } = x;
            *text == y.text
                && *width == y.width
                && *x_offset == y.x_offset
                && *justify_space == y.justify_space
        })
}

fn floats_eq(a: &FloatContext, b: &FloatContext) -> bool {
    let FloatContext { left_floats, right_floats } = a;
    let side = |x: &[crate::FloatExclusion], y: &[crate::FloatExclusion]| {
        x.len() == y.len()
            && x.iter().zip(y).all(|(p, q)| {
                let crate::FloatExclusion { rect, float_type } = p;
                rect_eq(rect, &q.rect) && *float_type == q.float_type
            })
    };
    side(left_floats, &b.left_floats) && side(right_floats, &b.right_floats)
}

fn rect_eq(a: &Rect, b: &Rect) -> bool {
    a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height
}

fn edges_eq(a: &EdgeSizes, b: &EdgeSizes) -> bool {
    a.top == b.top && a.right == b.right && a.bottom == b.bottom && a.left == b.left
}

fn dims_eq(a: &Dimensions, b: &Dimensions) -> bool {
    rect_eq(&a.content, &b.content)
        && edges_eq(&a.padding, &b.padding)
        && edges_eq(&a.border, &b.border)
        && edges_eq(&a.margin, &b.margin)
        && edges_eq(&a.scrollbar, &b.scrollbar)
}

fn box_type_eq(a: &BoxType, b: &BoxType) -> bool {
    match (a, b) {
        (BoxType::Block, BoxType::Block)
        | (BoxType::Inline, BoxType::Inline)
        | (BoxType::AnonymousBlock, BoxType::AnonymousBlock)
        | (BoxType::LineBreak, BoxType::LineBreak) => true,
        (BoxType::Text(x), BoxType::Text(y)) => x == y,
        (
            BoxType::Image {
                url: u1,
                natural_width: w1,
                natural_height: h1,
            },
            BoxType::Image {
                url: u2,
                natural_width: w2,
                natural_height: h2,
            },
        ) => u1 == u2 && w1 == w2 && h1 == h2,
        (BoxType::FormControl(x), BoxType::FormControl(y)) => x == y,
        _ => false,
    }
}

/// Every field of a box that layout reads, children aside. The destructure
/// names every field, so a new one does not compile until it is classed
/// here as an input (compared) or an output (ignored).
fn own_inputs_equal(a: &LayoutBox, b: &LayoutBox) -> bool {
    let LayoutBox {
        box_type,
        dimensions,
        style,
        children: _,
        position,
        offsets,
        float,
        clear,
        z_index,
        stacking_context,
        containing_block_index,
        viewport,
        sticky_state,
        // Labels the build numbers in document order (an element inserted
        // before a box renumbers it): layout reads only the tag. The
        // incremental result takes the fresh build's (`refresh_labels`).
        element_id: _,
        identity,
        link_href,
        focused_caret,
        focused_selection,
        text_sources,
        node_id,
        text_lines,
        text_flow_first_offset,
        percent_height_is_auto,
        // LAY1: layout's own bookkeeping, reset or set by every layout of
        // the box (`children_laid_at`) or by its parent just before it
        // (`abspos_cb_hint`); never an input from the tree.
        children_laid_at: _,
        abspos_cb_hint: _,
        root_element_height,
        table_span,
        table_caption_shift,
        context_floats,
        // A record of the last child layouts, never read by layout.
        layout_memo: _,
        // Outputs the engine stamps after layout; layout never reads them.
        element_scroll: _,
        sticky_rule: _,
        sticky_shift: _,
        sticky_paths: _,
        scrollbar_thumb_hovered: _,
        hit_overflow: _,
        stack_order: _,
        stack_gen: _,
        scrollbar_overflows,
        pseudo_animation_key,
        formatting_root,
        text_line_pitch,
    } = a;
    box_type_eq(box_type, &b.box_type)
        && *position == b.position
        && *float == b.float
        && *clear == b.clear
        && *z_index == b.z_index
        && *containing_block_index == b.containing_block_index
        && *viewport == b.viewport
        && identity.as_ref().map(|i| &i.tag) == b.identity.as_ref().map(|i| &i.tag)
        && *link_href == b.link_href
        && *focused_caret == b.focused_caret
        && *focused_selection == b.focused_selection
        && *text_sources == b.text_sources
        && *node_id == b.node_id
        && *text_flow_first_offset == b.text_flow_first_offset
        && *percent_height_is_auto == b.percent_height_is_auto
        && *root_element_height == b.root_element_height
        && *table_span == b.table_span
        && *table_caption_shift == b.table_caption_shift
        && *pseudo_animation_key == b.pseudo_animation_key
        && *formatting_root == b.formatting_root
        && *text_line_pitch == b.text_line_pitch
        && scrollbar_overflows.get() == b.scrollbar_overflows.get()
        && dims_eq(dimensions, &b.dimensions)
        && offsets_eq(offsets, &b.offsets)
        && opt_eq(stacking_context, &b.stacking_context, stacking_eq)
        && opt_eq(sticky_state, &b.sticky_state, sticky_eq)
        && opt_eq(text_lines, &b.text_lines, |x, y| lines_eq(x, y))
        && opt_eq(context_floats, &b.context_floats, floats_eq)
        && **style == *b.style
}

fn is_inline_level(b: &LayoutBox) -> bool {
    b.style.display.is_atomic_inline()
        || matches!(
            b.box_type,
            BoxType::Inline
                | BoxType::Text(_)
                | BoxType::Image { .. }
                | BoxType::FormControl(_)
                | BoxType::LineBreak
        )
}

fn percent(l: &Length) -> bool {
    crate::flex::has_percentage(l)
}

/// Hoist the changed box at `path` to a relayout root and find its
/// boundary. Each failed condition moves the root one level up; at the
/// tree's root, or its first level, the full layout is cheaper.
fn plan_for(
    laid: &LayoutBox,
    fresh: &LayoutBox,
    old_unlaid: &LayoutBox,
    path: Vec<usize>,
) -> Result<Plan, Fallback> {
    let mut root = path;
    let mut hoists = 0;
    // Depth of a flex container whose item at depth + 1 is the root and
    // holds its size (`column_item_holds`).
    let mut flex_hold: Option<usize> = None;
    let mut why: Fallback = if root.len() == 1 {
        "body_box"
    } else {
        "root_box"
    };
    'hoist: loop {
        if root.len() <= 1 {
            return Err(if root.is_empty() { "root_box" } else { why });
        }
        if flex_hold.is_some_and(|d| root.len() != d + 1) {
            flex_hold = None;
        }
        let (Some(l), Some(n), Some(u)) =
            (at(laid, &root), at(fresh, &root), at(old_unlaid, &root))
        else {
            return Err("shape");
        };
        if let Err(w) = root_ok(l, n, u) {
            why = w;
            root.pop();
            hoists += 1;
            continue;
        }
        let mut stop = None;
        let mut exact = false;
        for depth in (0..root.len()).rev() {
            let a = at(laid, &root[..depth]).ok_or("shape")?;
            if flex_hold == Some(depth) {
                stop = Some(depth);
                exact = true;
                break;
            }
            let parent = depth.checked_sub(1).and_then(|p| at(laid, &root[..p]));
            match classify_ancestor(a, parent) {
                Ancestor::Grows => {}
                Ancestor::Boundary => {
                    stop = Some(depth);
                    break;
                }
                Ancestor::Hoist(w) => {
                    // Stage 6a: the root moves up to the container's item
                    // instead, when the container would size and place
                    // every item as before.
                    let item = &root[..depth + 1];
                    if w == "ancestor_flex"
                        && depth >= 1
                        && at(fresh, item).is_some_and(|n| column_item_holds(a, n))
                    {
                        hoists += (root.len() - item.len()) as u32;
                        root.truncate(depth + 1);
                        flex_hold = Some(depth);
                        continue 'hoist;
                    }
                    why = w;
                    hoists += (root.len() - depth) as u32;
                    root.truncate(depth);
                    continue 'hoist;
                }
            }
        }
        if let Err(w) = scan(laid, &root, stop) {
            why = w;
            root.pop();
            hoists += 1;
            continue;
        }
        return Ok(Plan {
            root,
            stop,
            hoists,
            exact,
        });
    }
}

/// Stage 6a: whether the column flex container `c` would give its item
/// (`n`, as built now) the size and place it had, whatever
/// changed inside the item. Its main size (`height`) is a length the
/// algorithm takes as is: `flex-basis: auto`, min/max lengths or unset,
/// and no item shrank last time (so a content-based minimum never came
/// into play); its cross size is a `width` length or the container's
/// (stretched); not baseline-aligned, no auto margins.
fn column_item_holds(c: &LayoutBox, n: &LayoutBox) -> bool {
    use rustkit_css::{AlignItems, AlignSelf, FlexBasis, FlexDirection, FlexWrap};
    let cs = &c.style;
    if cs.display != Display::Flex
        || !matches!(cs.flex_direction, FlexDirection::Column | FlexDirection::ColumnReverse)
        || cs.flex_wrap != FlexWrap::NoWrap
        || c.float != Float::None
        || matches!(c.position, Position::Absolute | Position::Fixed)
    {
        return false;
    }
    let s = &n.style;
    let fixed = |l: &Length| {
        matches!(
            l,
            Length::Px(_)
                | Length::Em(_)
                | Length::Rem(_)
                | Length::Vw(_)
                | Length::Vh(_)
                | Length::Vmin(_)
                | Length::Vmax(_)
                | Length::Zero
        )
    };
    let unset_or_fixed = |l: &Length| matches!(l, Length::Auto) || fixed(l);
    if !fixed(&s.height)
        || s.flex_basis != FlexBasis::Auto
        || !unset_or_fixed(&s.min_height)
        || !unset_or_fixed(&s.max_height)
        || !unset_or_fixed(&s.min_width)
        || !unset_or_fixed(&s.max_width)
    {
        return false;
    }
    let stretch = match s.align_self {
        AlignSelf::Auto => match cs.align_items {
            AlignItems::Stretch => Some(true),
            AlignItems::Baseline => None,
            _ => Some(false),
        },
        AlignSelf::Stretch => Some(true),
        AlignSelf::Baseline => None,
        _ => Some(false),
    };
    let Some(stretch) = stretch else {
        return false;
    };
    if !(fixed(&s.width) || (matches!(s.width, Length::Auto) && stretch)) {
        return false;
    }
    if [&s.margin_top, &s.margin_right, &s.margin_bottom, &s.margin_left]
        .into_iter()
        .any(|m| matches!(m, Length::Auto))
    {
        return false;
    }
    let items: Vec<&LayoutBox> = c
        .children
        .iter()
        .filter(|x| {
            !matches!(x.position, Position::Absolute | Position::Fixed)
                && !matches!(
                    x.style.position,
                    rustkit_css::Position::Absolute | rustkit_css::Position::Fixed
                )
        })
        .collect();
    let gap = c.length_to_px(&cs.row_gap, 0.0).max(0.0);
    let sum: f32 = items
        .iter()
        .map(|x| x.dimensions.margin_box().height)
        .sum::<f32>()
        + gap * items.len().saturating_sub(1) as f32;
    sum <= c.dimensions.content.height + 0.5
}

/// Whether the box at a root's path can be laid out alone: `l` laid out,
/// `n` as built now, `u` as built last time.
fn root_ok(l: &LayoutBox, n: &LayoutBox, u: &LayoutBox) -> Result<(), Fallback> {
    let s = &n.style;
    if !matches!(n.box_type, BoxType::Block | BoxType::AnonymousBlock)
        || !matches!(s.display, Display::Block | Display::Flex | Display::Grid)
    {
        return Err("changed_not_block");
    }
    if l.position != Position::Static
        || !matches!(
            s.position,
            rustkit_css::Position::Static | rustkit_css::Position::Relative
        )
    {
        return Err("changed_position");
    }
    if [&s.top, &s.bottom, &s.left, &s.right]
        .into_iter()
        .flatten()
        .any(percent)
    {
        return Err("changed_position");
    }
    if n.float != Float::None {
        return Err("changed_float");
    }
    if n.clear != Clear::None {
        return Err("changed_clear");
    }
    if percent(&s.height) || percent(&s.min_height) || percent(&s.max_height) {
        return Err("changed_percent_height");
    }
    if s.column_count.is_some()
        || s.columns_declared
        || s.webkit_box.is_some()
        || s.writing_mode != WritingMode::HorizontalTb
    {
        return Err("changed_not_block");
    }
    // Margins collapsing through an open edge must be what they were: the
    // same boxes at the ends of the chain now (`chain_stays`), and the same
    // margins once the box is laid out (`edge_chain`, in `lay_out_root`).
    let bfc = crate::margin_collapse::establishes_bfc(s, n.float);
    if !bfc {
        if !chain_stays(l, n, u, true) {
            return Err("margin_collapse_top");
        }
        if !chain_stays(l, n, u, false) {
            return Err("margin_collapse_bottom");
        }
    }
    subtree_ok(n, bfc)?;
    subtree_ok(l, bfc)
}

/// What is under a relayout root must lay out inside it: no fixed box, and
/// no float or `clear` unless the root contains them (a formatting root).
/// An absolute box anchors to its parent's padding box
/// (`reanchor_absolute_children`), which is inside the root.
fn subtree_ok(b: &LayoutBox, contains_floats: bool) -> Result<(), Fallback> {
    for child in &b.children {
        if child.position == Position::Fixed || child.style.position == rustkit_css::Position::Fixed
        {
            return Err("subtree_fixed");
        }
        if (child.float != Float::None || child.clear != Clear::None) && !contains_floats {
            return Err("subtree_float");
        }
        let inner =
            contains_floats || crate::margin_collapse::establishes_bfc(&child.style, child.float);
        subtree_ok(child, inner)?;
    }
    Ok(())
}

fn edge_closed_top(b: &LayoutBox) -> bool {
    b.dimensions.border_with_gutter().top > 0.0
        || b.dimensions.padding.top > 0.0
        || crate::margin_collapse::establishes_bfc(&b.style, b.float)
}

fn edge_closed_bottom(b: &LayoutBox) -> bool {
    crate::margin_collapse::establishes_bfc(&b.style, b.float)
        || !crate::margin_collapse::should_collapse_with_last_child(
            &b.style,
            b.float,
            b.dimensions.border_with_gutter().bottom,
            b.dimensions.padding.bottom,
        )
}

enum Ancestor {
    /// An auto-height block in flow: grows by the delta.
    Grows,
    /// A block in flow whose height cannot depend on its content: the
    /// delta moves its later children and stops there.
    Boundary,
    /// Neither: the relayout root moves up to it.
    Hoist(Fallback),
}

fn classify_ancestor(a: &LayoutBox, parent: Option<&LayoutBox>) -> Ancestor {
    let s = &a.style;
    if !matches!(a.box_type, BoxType::Block | BoxType::AnonymousBlock) {
        return Ancestor::Hoist("ancestor_not_block");
    }
    if s.display != Display::Block {
        return Ancestor::Hoist(match s.display {
            Display::Flex | Display::InlineFlex => "ancestor_flex",
            Display::Grid | Display::InlineGrid => "ancestor_grid",
            d if d.is_table() => "ancestor_table",
            _ => "ancestor_not_block",
        });
    }
    if s.column_count.is_some()
        || s.columns_declared
        || s.webkit_box.is_some()
        || s.line_clamp().is_some()
    {
        return Ancestor::Hoist("ancestor_columns");
    }
    if s.writing_mode != WritingMode::HorizontalTb {
        return Ancestor::Hoist("ancestor_not_block");
    }
    if s.container_type != ContainerType::Normal {
        return Ancestor::Hoist("ancestor_container");
    }
    let out_of_flow = a.float != Float::None
        || matches!(a.position, Position::Absolute | Position::Fixed)
        || matches!(
            s.position,
            rustkit_css::Position::Absolute | rustkit_css::Position::Fixed
        );
    if out_of_flow {
        return Ancestor::Hoist("ancestor_out_of_flow");
    }
    if [&s.top, &s.bottom].into_iter().flatten().any(percent) {
        return Ancestor::Hoist("ancestor_out_of_flow");
    }
    // In flow with its width from its containing block: a height that does
    // not read its content makes it a boundary.
    let fixed_height = match &s.height {
        Length::Auto | Length::FitContent => false,
        Length::Min(_) | Length::Max(_) | Length::Clamp(_) => false,
        l if percent(l) => {
            !a.percent_height_computes_to_auto()
                && matches!(s.height, Length::Percent(_))
                && parent
                    .is_some_and(|p| p.definite_content_height_for_children(0.0, false).is_some())
        }
        _ => true,
    };
    if fixed_height {
        return Ancestor::Boundary;
    }
    let auto_height = matches!(s.height, Length::Auto) || a.percent_height_computes_to_auto();
    if !auto_height {
        return Ancestor::Hoist("ancestor_sized");
    }
    // An auto height with a ratio comes from the width, and the content
    // raises it only past the ratio height: it does not just grow.
    if s.aspect_ratio.is_some() {
        return Ancestor::Hoist("ancestor_aspect_ratio");
    }
    // A min- or max-height clamps the content height; `splice` checks the
    // old and new heights are both on the content side of it.
    let keyword = |l: &Length| {
        matches!(
            l,
            Length::FitContent | Length::Min(_) | Length::Max(_) | Length::Clamp(_)
        )
    };
    if percent(&s.min_height)
        || percent(&s.max_height)
        || keyword(&s.min_height)
        || keyword(&s.max_height)
    {
        return Ancestor::Hoist("ancestor_min_max");
    }
    Ancestor::Grows
}

/// The laid-out tree outside the root `r`, against its boundary at depth
/// `stop`: no float before the root reaches below its top; no absolute or
/// fixed box is a child of a growing ancestor, or follows the path in a
/// box whose later children move; no fixed box is among what moves; no
/// box that moves or sits in a growing box has percentage offsets.
fn scan(laid: &LayoutBox, r: &[usize], stop: Option<usize>) -> Result<(), Fallback> {
    let grows = |depth: usize| stop.map_or(true, |s| depth > s);
    let r_top = at(laid, r).ok_or("shape")?.dimensions.border_box().y;
    floats_before(laid, r, r_top)?;
    for depth in stop.unwrap_or(0)..r.len() {
        let node = at(laid, &r[..depth]).ok_or("shape")?;
        let on_path = r[depth];
        for (i, c) in node.children.iter().enumerate() {
            if i == on_path {
                continue;
            }
            let oof = matches!(c.position, Position::Absolute | Position::Fixed)
                || matches!(
                    c.style.position,
                    rustkit_css::Position::Absolute | rustkit_css::Position::Fixed
                );
            if oof && (grows(depth) || i > on_path) {
                return Err("abspos_on_path");
            }
            if grows(depth)
                && [&c.style.top, &c.style.bottom]
                    .into_iter()
                    .flatten()
                    .any(percent)
            {
                return Err("relative_percent");
            }
            if i > on_path && contains_fixed(c) {
                return Err("fixed_after");
            }
        }
    }
    Ok(())
}

fn contains_fixed(b: &LayoutBox) -> bool {
    b.position == Position::Fixed
        || b.style.position == rustkit_css::Position::Fixed
        || b.children.iter().any(contains_fixed)
}

/// No float before the root in document order (outside its ancestors)
/// reaches below its top edge.
fn floats_before(b: &LayoutBox, rest: &[usize], r_top: f32) -> Result<(), Fallback> {
    let Some((&i, below)) = rest.split_first() else {
        return Ok(());
    };
    for c in &b.children[..i] {
        any_float_below(c, r_top)?;
    }
    floats_before(&b.children[i], below, r_top)
}

fn any_float_below(b: &LayoutBox, r_top: f32) -> Result<(), Fallback> {
    if b.float != Float::None && b.dimensions.margin_box().bottom() > r_top + 0.01 {
        return Err("float_beside");
    }
    b.children
        .iter()
        .try_for_each(|c| any_float_below(c, r_top))
}

/// Where two laid-out trees differ, in shapes only: each entry is the child
/// path of a box and what differs (`shape`, `content`, `padding`, `border`,
/// `margin`, `lines`). Rects compare to 0.01 px.
pub fn compare_rects(a: &LayoutBox, b: &LayoutBox, limit: usize) -> (usize, Vec<String>) {
    let mut count = 0;
    let mut shown = Vec::new();
    let mut path = Vec::new();
    compare_into(a, b, &mut path, &mut count, &mut shown, limit);
    (count, shown)
}

fn compare_into(
    a: &LayoutBox,
    b: &LayoutBox,
    path: &mut Vec<usize>,
    count: &mut usize,
    shown: &mut Vec<String>,
    limit: usize,
) {
    let mut what = Vec::new();
    let (da, db) = (&a.dimensions, &b.dimensions);
    let rect_close = |x: &Rect, y: &Rect| {
        close(x.x, y.x) && close(x.y, y.y) && close(x.width, y.width) && close(x.height, y.height)
    };
    if a.children.len() != b.children.len() || !box_type_eq(&a.box_type, &b.box_type) {
        what.push("shape");
    }
    if !rect_close(&da.content, &db.content) {
        what.push("content");
    }
    if !edges_close(&da.padding, &db.padding) {
        what.push("padding");
    }
    if !edges_close(&da.border, &db.border) || !edges_close(&da.scrollbar, &db.scrollbar) {
        what.push("border");
    }
    if !edges_close(&da.margin, &db.margin) {
        what.push("margin");
    }
    if a.element_id != b.element_id || a.identity != b.identity {
        what.push("labels");
    }
    // A scroller's extent (its children's reach) and its styled bars'
    // marks (Prometheus #1064 MED): a boundary keeps its size while both
    // follow its content.
    if a.scrollbar_overflows.get() != b.scrollbar_overflows.get()
        || (a.is_element_scroller() && {
            let (x, y) = (a.element_scroll_max(), b.element_scroll_max());
            !close(x.0, y.0) || !close(x.1, y.1)
        })
    {
        what.push("overflow");
    }
    // A line's break points, width, offset and justification space fix
    // where each of its glyph runs is drawn.
    let lines_close = match (&a.text_lines, &b.text_lines) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.len() == y.len()
                && x.iter().zip(y).all(|(p, q)| {
                    p.text == q.text
                        && close(p.width, q.width)
                        && close(p.x_offset, q.x_offset)
                        && close(p.justify_space, q.justify_space)
                })
        }
        _ => false,
    };
    if !lines_close {
        what.push("lines");
    }
    if !what.is_empty() {
        *count += 1;
        if shown.len() < limit {
            let kind = match &a.box_type {
                BoxType::Block => "block",
                BoxType::AnonymousBlock => "anon",
                BoxType::Inline => "inline",
                BoxType::Text(_) => "text",
                BoxType::Image { .. } => "image",
                BoxType::FormControl(_) => "control",
                BoxType::LineBreak => "br",
            };
            let (c, d) = (da.content, db.content);
            // Shapes only (Pete's rule): the path, the kind, what differs
            // and the rects; never a line's text, which is compared above.
            shown.push(format!(
                "{path:?} {kind} {} ({:.1},{:.1} {:.1}x{:.1} vs {:.1},{:.1} {:.1}x{:.1})",
                what.join("+"),
                c.x,
                c.y,
                c.width,
                c.height,
                d.x,
                d.y,
                d.width,
                d.height
            ));
        }
    }
    if a.children.len() == b.children.len() {
        for (i, (x, y)) in a.children.iter().zip(&b.children).enumerate() {
            path.push(i);
            compare_into(x, y, path, count, shown, limit);
            path.pop();
        }
    }
}
