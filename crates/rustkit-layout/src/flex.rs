//! Flexbox layout implementation for RustKit.
//!
//! Implements the CSS Flexible Box Layout Module Level 1:
//! https://www.w3.org/TR/css-flexbox-1/
//!
//! The flexbox algorithm is complex and multi-step:
//! 1. Determine main/cross axes based on flex-direction
//! 2. Collect and sort flex items
//! 3. Calculate flex base sizes
//! 4. Collect items into flex lines (if wrapping)
//! 5. Resolve flexible lengths (grow/shrink)
//! 6. Calculate cross sizes
//! 7. Main axis alignment (justify-content)
//! 8. Cross axis alignment (align-items, align-self)
//! 9. Multi-line alignment (align-content)
//! 10. Handle reverse directions

use crate::{Dimensions, EdgeSizes, LayoutBox, Rect};
use rustkit_css::{
    AlignContent, AlignItems, AlignSelf, FlexBasis, FlexWrap, JustifyContent, Length,
};
use tracing::trace;

/// Represents the main and cross axes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// Get the perpendicular axis.
    pub fn cross(self) -> Self {
        match self {
            Axis::Horizontal => Axis::Vertical,
            Axis::Vertical => Axis::Horizontal,
        }
    }
}

/// A flex item during layout computation.
#[derive(Debug)]
pub struct FlexItem<'a> {
    /// Reference to the layout box.
    pub layout_box: &'a mut LayoutBox,

    /// Order property for sorting.
    pub order: i32,

    /// Flex grow factor.
    pub flex_grow: f32,

    /// Flex shrink factor.
    pub flex_shrink: f32,

    /// Flex basis (resolved to absolute value).
    pub flex_basis: f32,

    /// Hypothetical main size (clamped by min/max).
    pub hypothetical_main_size: f32,

    /// Target main size (after flex resolution).
    pub target_main_size: f32,

    /// Frozen flag (for grow/shrink algorithm).
    pub frozen: bool,

    /// Cross size.
    pub cross_size: f32,

    /// Main position (relative to container).
    pub main_position: f32,

    /// Cross position (relative to line start).
    pub cross_position: f32,

    /// Minimum main size.
    pub min_main_size: f32,

    /// Maximum main size.
    pub max_main_size: f32,

    /// Minimum cross size.
    pub min_cross_size: f32,

    /// Maximum cross size.
    pub max_cross_size: f32,

    /// Align self value.
    pub align_self: AlignSelf,

    /// Outer margin on main axis start.
    pub main_margin_start: f32,

    /// Outer margin on main axis end.
    pub main_margin_end: f32,

    /// Outer margin on cross axis start.
    pub cross_margin_start: f32,

    /// Outer margin on cross axis end.
    pub cross_margin_end: f32,

    /// Whether the item has an explicit cross size (not auto).
    /// If true, stretch should not apply per CSS spec.
    pub has_explicit_cross_size: bool,

    /// Explicit cross size (border-box), when the style specifies one.
    pub explicit_cross_size: Option<f32>,

    /// Padding+border extent at the main-axis start edge (resolved px).
    /// All FlexItem sizes (basis, hypothetical, target, cross) are
    /// border-box; these extents convert back to the content rect at
    /// apply_positions time.
    pub main_pb_start: f32,

    /// Padding+border extent at the main-axis end edge.
    pub main_pb_end: f32,

    /// Padding+border extent at the cross-axis start edge.
    pub cross_pb_start: f32,

    /// Padding+border extent at the cross-axis end edge.
    pub cross_pb_end: f32,

    /// The main size came from the item's CONTENT (flex-basis auto with an
    /// auto main size, or flex-basis content) on a non-replaced box. On the
    /// vertical main axis that content size is a line-height guess until the
    /// children are laid out (step 11); step 11d re-derives it from the real
    /// laid-out height for exactly these items.
    pub main_size_from_content: bool,
}

impl<'a> FlexItem<'a> {
    /// Get outer main size (target + margins).
    pub fn outer_main_size(&self) -> f32 {
        self.target_main_size + self.main_margin_start + self.main_margin_end
    }

    /// Get outer hypothetical main size.
    pub fn outer_hypothetical_main_size(&self) -> f32 {
        self.hypothetical_main_size + self.main_margin_start + self.main_margin_end
    }

    /// Get outer cross size.
    pub fn outer_cross_size(&self) -> f32 {
        self.cross_size + self.cross_margin_start + self.cross_margin_end
    }

    /// Total padding+border on the main axis.
    pub fn main_pb(&self) -> f32 {
        self.main_pb_start + self.main_pb_end
    }

    /// Total padding+border on the cross axis.
    pub fn cross_pb(&self) -> f32 {
        self.cross_pb_start + self.cross_pb_end
    }
}

/// A flex line containing multiple items.
#[derive(Debug)]
pub struct FlexLine<'a> {
    /// Items in this line.
    pub items: Vec<FlexItem<'a>>,

    /// Cross size of the line.
    pub cross_size: f32,

    /// Cross position of the line.
    pub cross_position: f32,
}

impl<'a> FlexLine<'a> {
    /// Create a new flex line.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            cross_size: 0.0,
            cross_position: 0.0,
        }
    }

    /// Get the total hypothetical main size of items.
    pub fn hypothetical_main_size(&self) -> f32 {
        self.items
            .iter()
            .map(|item| item.outer_hypothetical_main_size())
            .sum()
    }

    /// Get the largest outer cross size among items.
    pub fn max_outer_cross_size(&self) -> f32 {
        self.items
            .iter()
            .map(|item| item.outer_cross_size())
            .fold(0.0, f32::max)
    }
}

impl<'a> Default for FlexLine<'a> {
    fn default() -> Self {
        Self::new()
    }
}

/// A specified length, reduced to the container's INNER size under
/// `box-sizing: border-box`. Shared by every main-axis resolution below so
/// the two places that need this arithmetic cannot drift apart.
fn inner_main_from_spec(container: &LayoutBox, raw: f32) -> f32 {
    if container.style.box_sizing == rustkit_css::BoxSizing::BorderBox {
        let pb = container.dimensions.padding.vertical() + container.dimensions.border.vertical();
        (raw - pb).max(0.0)
    } else {
        raw
    }
}

/// The container's used inner main size on the VERTICAL main axis, resolved
/// from STYLE, or `None` when the height is indefinite (`auto`, or a
/// percentage this function cannot resolve without the real containing
/// block — see `min_inner_main_size` for the floor that still applies).
///
/// Only `Length::Px` counts as definite here, which is deliberately the same
/// bar step 11d has always used: a percentage height needs the containing
/// block, and every caller passes the container's own box in its place, so
/// resolving one here would be resolving it against the wrong number.
fn definite_inner_main_size(container: &LayoutBox) -> Option<f32> {
    match container.style.height {
        Length::Px(v) => Some(inner_main_from_spec(container, v)),
        _ => None,
    }
}

/// The `min-height` floor on the vertical main axis, in inner terms.
/// `min-height: 100vh` with `justify-content: center` is the centring idiom
/// on `new_tab`'s body and on real landing pages (css-sizing-3 §5.1).
fn min_inner_main_size(container: &LayoutBox) -> f32 {
    match container.style.min_height {
        Length::Px(px) => inner_main_from_spec(container, px),
        Length::Vh(vh) => inner_main_from_spec(container, vh / 100.0 * container.viewport.1),
        _ => 0.0,
    }
}

/// Layout a flex container and its children.
pub fn layout_flex_container(container: &mut LayoutBox, container_box: &Dimensions) {
    layout_flex_container_in(container, container_box, None)
}

/// As [`layout_flex_container`], plus the container's REAL containing block
/// where the caller has it.
///
/// The two are not the same box, and the old single parameter conflated them.
/// `container_box` is the container's OWN content box: every production caller
/// passes `container.dimensions.clone()`, and it is what the container's main
/// and cross sizes, its items' percentage bases and its children's origin are
/// read from. `positioning_cb` is the block the container is positioned
/// against.
///
/// Exactly one rule needs the second one. CSS2 §10.6.4: an out-of-flow box
/// with `height: auto` and both `top` and `bottom` set has a definite used
/// height, and that height is a function of the CONTAINING BLOCK's height.
/// Nothing in `container_box` carries it, because
/// `apply_position_offsets_absolute` — the pass that writes it — does not run
/// until after this call returns.
///
/// Asking `container_box` for it instead is the defect this parameter exists
/// to close. On the block path `container_box` is the container's own box, so
/// the question becomes "how tall are you, minus your insets", and the answer
/// is the block pre-pass's flow cursor. image-gallery's
/// `.aspect-box > .content` is the idiom in full — `position: absolute;
/// inset: 0; display: flex; flex-direction: column; justify-content: center`
/// — and it justified 19.65px of content in 19.65px of cursor inside a 288px
/// card, leaving both items packed against the top edge.
pub fn layout_flex_container_in(
    container: &mut LayoutBox,
    container_box: &Dimensions,
    positioning_cb: Option<&Dimensions>,
) {
    // Resolved once, up front, and read by both the main-size choice below
    // and step 11d's redistribution: these take `&LayoutBox`, and from the
    // moment the item list borrows `container.children` mutably no whole-box
    // borrow is available again.
    let style_definite_inner_main = definite_inner_main_size(container);
    let style_min_inner_main = min_inner_main_size(container);

    let style = &container.style;

    // 1. Determine main/cross axes
    let direction = style.flex_direction;
    let main_axis = if direction.is_row() {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    let cross_axis = main_axis.cross();

    // CSS2 §10.6.4, resolved before anything reads a main size: an
    // out-of-flow box with both insets set is definite, and only the
    // containing block knows the number. Resolved ONCE here so step 8
    // (justify) and step 11d (the main-size re-derivation) cannot disagree —
    // two derivations of this subtraction drifting apart is the defect class
    // night 8 recorded, and re-deriving it from `container_box` would also
    // subtract the insets a second time.
    let inset_used_main =
        positioning_cb.and_then(|cb| inset_definite_used_main(container, cb));

    // Get container dimensions
    //
    // css-flexbox-1 §9.2/§9.7 resolve flex lines, grow/shrink and
    // justify-content against the container's used inner MAIN size. On the
    // vertical main axis that number cannot be read off the box the caller
    // passes: every production call site hands `layout_flex_container` the
    // container's OWN dimensions, and on the block path their content.height
    // is the pre-pass FLOW CURSOR — the stack of the children's own heights.
    // Growing against that makes free space identically zero, so `flex-grow`
    // never applied in a column container with a definite height (measured
    // against Chrome 148: a 400px column with a `flex-grow:1; height:30px`
    // item kept the item at 30). Step 11d already resolved this from style,
    // but only on the branch where some CONTENT-sized item had been
    // corrected, so a column whose items are all explicitly sized or
    // basis-0 never reached it. Resolving it here puts the one number in
    // front of every step that consumes it; 11d now shares the helper
    // rather than restating the rule.
    let container_main_size = match main_axis {
        Axis::Horizontal => container_box.content.width,
        Axis::Vertical => style_definite_inner_main.unwrap_or(container_box.content.height),
    };
    // Deliberately NOT given the same treatment: the cross-axis analogue (an
    // inset-stretched ROW container centring items in a stale cursor) is the
    // same spec line, but no case on this corpus exercises it and
    // `definite_inner_cross` subtracts the container's own edges a second
    // time from whatever lands here. Recorded as a finding, not half-landed.
    let container_cross_size = match cross_axis {
        Axis::Horizontal => container_box.content.width,
        Axis::Vertical => container_box.content.height,
    };

    // Check if the flex container has a definite cross size
    // For row direction, cross axis is vertical (height)
    // For column direction, cross axis is horizontal (width)
    let has_definite_cross_size = match cross_axis {
        Axis::Vertical => !matches!(container.style.height, Length::Auto),
        // A block-level flex container with `width: auto` still has a
        // DEFINITE used width — it resolves against its containing block.
        // Treating auto as indefinite sent the stretch path down the
        // "auto container" arm, where the target is the largest item in the
        // line instead of the container width, so column children stretched
        // to each other rather than to the viewport.
        //
        // The height case is deliberately NOT symmetric: an auto-height row
        // container really is indefinite (it is sized BY its content), and
        // test_auto_height_stretch depends on that staying true.
        Axis::Horizontal => {
            !matches!(container.style.width, Length::Auto) || container_cross_size > 0.0
        }
    };

    // The definite inner cross size, resolved from STYLE rather than from
    // container_box: the block pre-pass can leave a stale
    // children-stacked height in content.height (logo 38.4 + nav 25.6 = 64
    // inside a height:60px header), which would center every item 2px low.
    // Px lengths resolve here; anything else falls back to the passed size.
    let definite_inner_cross = if has_definite_cross_size {
        let (spec, pb) = match cross_axis {
            Axis::Vertical => (
                &container.style.height,
                container.dimensions.padding.vertical() + container.dimensions.border.vertical(),
            ),
            Axis::Horizontal => (
                &container.style.width,
                container.dimensions.padding.horizontal()
                    + container.dimensions.border.horizontal(),
            ),
        };
        match spec {
            Length::Px(v) => {
                if container.style.box_sizing == rustkit_css::BoxSizing::BorderBox {
                    Some((v - pb).max(0.0))
                } else {
                    Some(*v)
                }
            }
            // `container_cross_size` is the CONTAINING BLOCK's content size,
            // not this container's. For the `width: auto` case above, the
            // used inner cross size is that minus the container's OWN margin,
            // border and padding — handing back the raw containing-block
            // number stretches every child past the container by exactly its
            // own edges. That is #81's defect inverted: items grew by their
            // padding instead of shrinking by it.
            _ if container_cross_size > 0.0 => {
                let own_edges = match cross_axis {
                    Axis::Horizontal => {
                        container.dimensions.margin.horizontal()
                            + container.dimensions.border.horizontal()
                            + container.dimensions.padding.horizontal()
                    }
                    Axis::Vertical => {
                        container.dimensions.margin.vertical()
                            + container.dimensions.border.vertical()
                            + container.dimensions.padding.vertical()
                    }
                };
                Some((container_cross_size - own_edges).max(0.0))
            }
            _ => None,
        }
    } else {
        None
    };

    // Get gap values
    let main_gap = match main_axis {
        Axis::Horizontal => resolve_length(&style.column_gap, container_main_size),
        Axis::Vertical => resolve_length(&style.row_gap, container_main_size),
    };
    let cross_gap = match cross_axis {
        Axis::Horizontal => resolve_length(&style.column_gap, container_cross_size),
        Axis::Vertical => resolve_length(&style.row_gap, container_cross_size),
    };

    // Step 11d re-derives the container's main size once the items are laid
    // out. Hand it the number resolved at the top rather than deriving it a
    // second time.
    let inset_inner_main = if main_axis == Axis::Vertical {
        inset_used_main
    } else {
        None
    };

    // 2. Collect flex items (skip absolutely positioned)
    let mut items: Vec<FlexItem> = Vec::new();
    for child in &mut container.children {
        if child.style.position == rustkit_css::Position::Absolute
            || child.style.position == rustkit_css::Position::Fixed
        {
            continue;
        }

        // css-flexbox-1 §4: an anonymous item containing only white space
        // is not rendered — it never becomes a flex item. Zero its rect so
        // stale pre-pass dimensions neither paint nor consume a gap slot.
        if let crate::BoxType::Text(t) = &child.box_type {
            if t.trim().is_empty() {
                child.dimensions.content.width = 0.0;
                child.dimensions.content.height = 0.0;
                continue;
            }
        }

        let item = create_flex_item(
            child,
            main_axis,
            container_main_size,
            container_cross_size,
            definite_inner_cross,
        );
        items.push(item);
    }

    // Sort by order property
    items.sort_by_key(|item| item.order);

    // 3. Collect items into flex lines
    let wrap = style.flex_wrap;
    let mut lines = collect_flex_lines(items, container_main_size, main_gap, wrap);

    if lines.is_empty() {
        return;
    }

    // 4. Resolve flexible lengths for each line
    for line in &mut lines {
        resolve_flexible_lengths(line, container_main_size, main_gap);
    }

    // 5. Calculate cross sizes for each line
    // Pass has_definite_cross_size so stretch behavior is correct for auto-height containers
    //
    // Stretch targets the container's INNER cross size, not the containing
    // block's content size. Those differ by the container's own margin,
    // border and padding, and handing over the outer number makes every
    // stretched child overflow its parent by exactly those edges.
    let stretch_cross_size = definite_inner_cross.unwrap_or(container_cross_size);
    for line in &mut lines {
        calculate_cross_sizes(
            line,
            stretch_cross_size,
            style.align_items,
            has_definite_cross_size,
            cross_axis,
        );
    }

    // css-flexbox-1 §9.4.8 rule 1: in a single-line container with a
    // definite cross size, the line's cross size IS the container's inner
    // cross size. Items align within that, never within a taller
    // content-derived line (oversized content overflows instead).
    if wrap == FlexWrap::NoWrap {
        if let (Some(cross), Some(line)) = (definite_inner_cross, lines.first_mut()) {
            line.cross_size = cross;
        }
    }

    // 6. Calculate line cross sizes and positions
    let total_cross_size: f32 = lines.iter().map(|l| l.cross_size).sum::<f32>()
        + cross_gap * (lines.len().saturating_sub(1)) as f32;

    // 7. Apply align-content for multi-line containers
    // Only distribute lines if we have a definite cross size
    let effective_cross_size = match definite_inner_cross {
        Some(c) => c,
        None => total_cross_size,
    };
    distribute_lines(
        &mut lines,
        effective_cross_size,
        total_cross_size,
        cross_gap,
        style.align_content,
    );

    // 8. Main axis alignment (justify-content) and positioning
    for line in &mut lines {
        distribute_main_axis(
            line,
            container_main_size,
            main_gap,
            style.justify_content,
            direction.is_reverse(),
        );
    }

    // 9. Cross axis alignment (align-items, align-self)
    for line in &mut lines {
        align_cross_axis(line, style.align_items);
    }

    // 10. Apply final positions to layout boxes
    // Pass the container's content origin so positions are absolute, not relative
    let container_origin = (container_box.content.x, container_box.content.y);
    apply_positions(
        &mut lines,
        main_axis,
        direction.is_reverse(),
        wrap == FlexWrap::WrapReverse,
        container_origin,
    );

    // 11. Recursively layout children of flex items (important for nested flex containers)
    // After flex positioning, each item's dimensions are set, so we can use them as containing blocks
    for line in &mut lines {
        for item in &mut line.items {
            // If this flex item has children and is a container (flex or block), lay them out
            if !item.layout_box.children.is_empty() {
                if item.layout_box.style.display.is_flex() {
                    // Nested flex container: recursively apply flex layout
                    let child_containing = item.layout_box.dimensions.clone();
                    layout_flex_container(item.layout_box, &child_containing);
                    // Absolutely positioned children are skipped by the flex
                    // item collection; lay them out against the item's FINAL
                    // dimensions so `inset: 0` overlays position AND stretch
                    // (the pre-pass ran them against pre-flex dims).
                    let cb = item.layout_box.dimensions.clone();
                    for child in &mut item.layout_box.children {
                        if matches!(
                            child.style.position,
                            rustkit_css::Position::Absolute | rustkit_css::Position::Fixed
                        ) {
                            child.layout(&cb);
                        }
                    }
                    item.layout_box.reanchor_absolute_children();
                } else {
                    // Block container: lay out children in normal flow.
                    // Cloning the item's FINAL dimensions per child would make every
                    // child position itself at content.y + content.height (block layout
                    // treats container_box.content.height as the flow cursor), i.e.
                    // stacked at the item's bottom edge. layout_block_children advances
                    // a real cursor from the item's content top instead.
                    //
                    // WITH sibling margin collapse: a flex item establishes an
                    // independent formatting context, so its own margins never
                    // collapse with its children's (fresh context), but the
                    // children still collapse among themselves (CSS 2.1
                    // §8.3.1). The plain layout_block_children here re-summed
                    // every sibling seam (mb+mt instead of max), un-doing the
                    // collapsed pre-pass — measured as the +20/+10 staircase
                    // on sticky-scroll's main column.
                    //
                    // css-flexbox-1 §9.7: an item's used MAIN size is the one
                    // the flex algorithm resolved in steps 4–10 — its flex base
                    // size after grow/shrink — and children's flow never
                    // changes it. In a COLUMN container that size is the item's
                    // height, and `layout_block_children_with_collapse` ends by
                    // assigning the flow cursor to `content.height`, silently
                    // replacing it. Chrome 148, measured on the bundled
                    // Chromium rather than assumed: a `height: 30px` column
                    // item holding 120px of children stays 30, its content
                    // overflows, and its sibling starts at y=30; a
                    // `flex: 1 1 auto; height: 30px` item in a 400px column
                    // keeps the 400 it grew to. Restoring the resolved number
                    // rather than the style length is what covers both.
                    //
                    // Items whose main size came from CONTENT are deliberately
                    // left alone: step 11d re-derives those from exactly this
                    // flow, and freezing them would stop a column of
                    // content-sized rows growing to its children at all.
                    let resolved_main_height = (main_axis == Axis::Vertical
                        && !item.main_size_from_content)
                        .then_some(item.layout_box.dimensions.content.height);
                    let mut item_margin_context = crate::MarginCollapseContext::new();
                    let mut item_float_context = crate::FloatContext::new();
                    // css-flexbox-1 §9.4: an item whose cross size is DEFINITE
                    // keeps it — content overflows rather than growing the box.
                    // layout_block_children_with_collapse ends by assigning the
                    // flow cursor to the item's `content.height`, and on this
                    // path that height is the size the flex algorithm already
                    // decided. Step 11b states this rule and `continue`s for
                    // exactly these items, so nothing downstream repairs the
                    // clobber: form-elements' `.toggle-switch { height: 26px }`
                    // came out 32.08, its two in-flow children's line boxes.
                    let definite_cross_height = (cross_axis == Axis::Vertical
                        && item.has_explicit_cross_size)
                        .then_some(item.layout_box.dimensions.content.height);
                    item.layout_box.layout_block_children_with_collapse(
                        &mut item_margin_context,
                        &mut item_float_context,
                        // Same definite height, same rule: a percentage-height
                        // child resolves against the item's used cross size
                        // (CSS 2.1 §10.5), not against the item's flow cursor.
                        definite_cross_height,
                    );
                    // A flex item is a formatting-context root, so its last
                    // in-flow child's bottom margin never collapses through
                    // it (CSS 2.1 §8.3.1) — it stays INSIDE the item. The
                    // collapse pass only materializes that pending margin
                    // when the box's own padding/border/height block the
                    // collapse (its style cannot tell it is a flex item), and
                    // leaves it in the context otherwise; take it here.
                    let pending = item_margin_context.resolve();
                    if pending > 0.0 {
                        item.layout_box.dimensions.content.height += pending;
                    }
                    // A definite cross size wins over both the flow and the
                    // pending margin (§9.4), so the restore goes last.
                    if let Some(height) = definite_cross_height {
                        item.layout_box.dimensions.content.height = height;
                    }
                    // A column item's definite MAIN size survives its
                    // children's flow for the same reason (n54): the two
                    // restores are mutually exclusive by axis.
                    if let Some(height) = resolved_main_height {
                        item.layout_box.dimensions.content.height = height;
                    }
                    // The item's box is final here; its abspos children
                    // (`bottom:` badges) anchor to that, not to the
                    // pre-pass cursor.
                    item.layout_box.reanchor_absolute_children();
                }
            }
        }
    }

    // 11b. Recompute cross sizes now that children are laid out
    // This fixes the chicken-and-egg problem where we need children heights
    // before we can determine item cross sizes
    for line in &mut lines {
        for item in &mut line.items {
            // A DEFINITE cross size never grows to fit content — content
            // overflows instead (css-flexbox-1 §9.4). Without this guard the
            // settings toggles (inline-flex, height:26px) ballooned to their
            // stacked children's 40.4px whenever they sat in a flex row.
            if item.has_explicit_cross_size {
                continue;
            }
            // Only recompute if cross_size is still using fallback (line_height or similar)
            // and we have children with actual heights
            if !item.layout_box.children.is_empty() {
                // A nested flex container was fully laid out in step 11 and
                // its content height is already final (its own step 12).
                // Summing a ROW container's children stacks side-by-side
                // items vertically: a 5-link nav measured 9 line-heights
                // tall, and align-items:center then pushed the sibling logo
                // 96px below a 60px header.
                //
                // The measure is taken on the CROSS axis: a row container's
                // items grow to their stacked children's HEIGHT, a column
                // container's items to their widest child's WIDTH. The old
                // code summed heights on both axes and then stored the sum
                // as a width — new_tab's `.container` (a column item,
                // max-width 600) came out 745.4 wide: 681.4 of stacked
                // child heights plus its own 64 of padding.
                //
                // A block item's height is the extent step 11 FLOWED
                // (content.height, written by the collapse pass), not the
                // sum of its children's margin boxes: the sum charges every
                // sibling seam mb + mt where the flow collapsed it to
                // max(mb, mt), so a column of margined blocks read taller
                // than it was laid out — new_tab's `.container` (search box
                // margin-bottom 2rem, next section margin-top 3rem) measured
                // 746 for its flowed 714, and the 32 landed as empty space
                // under the last child.
                let children_height: f32 = match cross_axis {
                    Axis::Vertical => item.layout_box.dimensions.content.height,
                    Axis::Horizontal => {
                        if item.layout_box.style.display.is_flex() {
                            item.layout_box.dimensions.content.width
                        } else {
                            item.layout_box
                                .children
                                .iter()
                                .map(|c| c.dimensions.margin_box().width)
                                .fold(0.0f32, f32::max)
                        }
                    }
                };

                // children_height is a content measure; item.cross_size is
                // border-box, so compare and store with padding+border added.
                if children_height > 0.0 && children_height + item.cross_pb() > item.cross_size {
                    // Update cross size based on actual children heights
                    item.cross_size = (children_height + item.cross_pb())
                        .max(item.min_cross_size)
                        .min(item.max_cross_size);

                    // Also update the layout box content height
                    match cross_axis {
                        Axis::Vertical => {
                            if item.layout_box.dimensions.content.height < children_height {
                                item.layout_box.dimensions.content.height = children_height;
                            }
                        }
                        Axis::Horizontal => {
                            if item.layout_box.dimensions.content.width < children_height {
                                item.layout_box.dimensions.content.width = children_height;
                            }
                        }
                    }
                }
            }
        }

        // Recompute line cross size based on updated item cross sizes
        line.cross_size = line
            .items
            .iter()
            .map(|i| i.cross_size + i.cross_margin_start + i.cross_margin_end)
            .fold(0.0, f32::max);
        // §9.4.8 rule 1 again: a definite single-line cross size is never
        // re-derived from content (see step 5).
        if wrap == FlexWrap::NoWrap {
            if let Some(cross) = definite_inner_cross {
                line.cross_size = cross;
            }
        }
    }

    // 11c. Re-position lines with the true cross sizes. Steps 6-10 placed
    // lines using the hypothetical (line-height fallback) cross sizes;
    // step 11's child layout revealed the real ones. Without this pass,
    // wrapped rows stack at the estimated heights and overlap whenever an
    // item is taller than one text line.
    let total_cross_size: f32 = lines.iter().map(|l| l.cross_size).sum::<f32>()
        + cross_gap * (lines.len().saturating_sub(1)) as f32;
    let effective_cross_size = match definite_inner_cross {
        Some(c) => c,
        None => total_cross_size,
    };
    distribute_lines(
        &mut lines,
        effective_cross_size,
        total_cross_size,
        cross_gap,
        style.align_content,
    );
    for line in &mut lines {
        // Re-stretch (css-flexbox-1 §9.4.11) against the line's FINAL cross
        // size. Step 5 stretched each auto-height item of a row, but step 11
        // re-flowed its children and wrote the flow height over the
        // stretched one (only definite heights are restored there), and 11b
        // only ever grows an item to its content. So every `align-items:
        // stretch` item shorter than its line kept its content height:
        // card-grid's cards were 256.7 in a 279.4 row where Chrome stretches
        // all three. Row containers only — a column item's width was
        // stretched before its children flowed and survives step 11.
        if cross_axis == Axis::Vertical {
            for item in &mut line.items {
                if item.has_explicit_cross_size
                    || resolved_align(item.align_self, style.align_items) != AlignItems::Stretch
                {
                    continue;
                }
                let target = (line.cross_size - item.cross_margin_start - item.cross_margin_end)
                    .max(item.min_cross_size)
                    .min(item.max_cross_size);
                let content_height = (target - item.cross_pb()).max(0.0);
                if content_height > item.layout_box.dimensions.content.height {
                    item.cross_size = target;
                    item.layout_box.dimensions.content.height = content_height;
                    item.layout_box.reanchor_absolute_children();
                }
            }
        }
        align_cross_axis(line, style.align_items);
        for item in &mut line.items {
            // New absolute border-box cross position, converted to a content
            // rect delta; main-axis positions are unchanged, so shifting the
            // already-laid-out subtree is sufficient.
            let d = &item.layout_box.dimensions;
            let (origin_cross, old_content_cross, pb_start) = match cross_axis {
                Axis::Vertical => (
                    container_origin.1,
                    d.content.y,
                    d.padding.top + d.border.top,
                ),
                Axis::Horizontal => (
                    container_origin.0,
                    d.content.x,
                    d.padding.left + d.border.left,
                ),
            };
            let delta = origin_cross + line.cross_position + item.cross_position + pb_start
                - old_content_cross;
            if delta != 0.0 {
                match cross_axis {
                    Axis::Vertical => translate_subtree(item.layout_box, 0.0, delta),
                    Axis::Horizontal => translate_subtree(item.layout_box, delta, 0.0),
                }
            }
        }
    }

    // 11d. Re-derive MAIN sizes on the vertical axis now that children are
    // laid out. create_flex_item guessed a content-sized item's height at
    // one line (get_intrinsic_main_size has no height estimator) and steps
    // 4–10 placed every later item on that guess. 11b/11c correct the CROSS
    // axis this way; the main axis had no such pass, so a column container
    // stacked its rows at one-line pitch and they overlapped whenever a row
    // was taller than a text line (flex-positioning section 4: row 2 placed
    // at y=534 while row 1 ended at 537; Chrome 548). Only items whose main
    // size came from content are touched; explicit heights and flex-basis
    // lengths keep their numbers. With an indefinite (auto) container height
    // there is no free space to distribute, so the main size used for
    // justification is the content sum; a definite height re-runs the
    // grow/shrink resolution from the corrected hypotheticals.
    if main_axis == Axis::Vertical {
        let mut any_changed = false;
        for line in &mut lines {
            for item in &mut line.items {
                if !item.main_size_from_content || item.layout_box.children.is_empty() {
                    continue;
                }
                // The laid-out height is the extent step 11 FLOWED
                // (content.height from the collapse pass, plus the kept
                // last-child margin) for a block item too — not the sum of
                // its children's margin boxes, which charges every sibling
                // seam mb + mt where the flow collapsed it to max(mb, mt).
                // new_tab's `.container` (a column item: search box
                // margin-bottom 2rem, next section margin-top 3rem) measured
                // 746 for its flowed 714, and the 32 landed as empty space
                // under the last child.
                let laid_out: f32 = item.layout_box.dimensions.content.height;
                if laid_out <= 0.0 {
                    continue;
                }
                let new_hyp = (laid_out + item.main_pb())
                    .max(item.min_main_size)
                    .min(item.max_main_size);
                if (new_hyp - item.hypothetical_main_size).abs() > 0.01 {
                    item.hypothetical_main_size = new_hyp;
                    item.target_main_size = new_hyp;
                    any_changed = true;
                }
            }
        }
        if any_changed {
            // The main size to justify/grow against. A definite pixel height
            // resolves from STYLE (inner, like definite_inner_cross above) —
            // container_main_size is the containing block's number, and it
            // is NOT the container's used height: the engine hands a column
            // container its own pre-pass stacked height (new_tab's body got
            // 713.4, the container's stale guess-sized stack; the repro's
            // 160px column grew its rows against 94). An auto height is
            // sized by content, floored at min-height (css-sizing-3 §5.1 —
            // `min-height: 100vh; justify-content: center` is the centring
            // idiom on new_tab's body and on real landing pages), so rows
            // can never be placed closer than their real heights and the
            // free space centred in is the real one.
            // Step 11d reads the same two numbers the entry resolved (n55),
            // and an out-of-flow box with both insets set keeps its §10.6.4
            // used height where style has none.
            let definite_inner_main = style_definite_inner_main.or(inset_inner_main);
            let min_inner_main = style_min_inner_main;
            for line in &mut lines {
                let content_sum = line.hypothetical_main_size()
                    + main_gap * line.items.len().saturating_sub(1) as f32;
                let redistribute_main = match definite_inner_main {
                    Some(m) => m,
                    None => content_sum.max(min_inner_main),
                };
                resolve_flexible_lengths(line, redistribute_main, main_gap);
                distribute_main_axis(
                    line,
                    redistribute_main,
                    main_gap,
                    style.justify_content,
                    direction.is_reverse(),
                );
                for item in &mut line.items {
                    let d = &item.layout_box.dimensions;
                    let new_content_y =
                        container_origin.1 + item.main_position + d.padding.top + d.border.top;
                    let delta = new_content_y - d.content.y;
                    if delta != 0.0 {
                        translate_subtree(item.layout_box, 0.0, delta);
                    }
                    item.layout_box.dimensions.content.height =
                        (item.target_main_size - item.main_pb()).max(0.0);
                }
            }
        }
    }

    // 12. Update container dimensions based on flex items
    // Calculate the total main and cross sizes used by items
    if !lines.is_empty() {
        let (total_main, total_cross) = match main_axis {
            Axis::Horizontal => {
                // Main axis is horizontal (width), cross axis is vertical (height)
                let max_main: f32 = lines
                    .iter()
                    .flat_map(|l| l.items.iter())
                    .map(|item| item.main_position + item.target_main_size)
                    .fold(0.0f32, f32::max);
                let total_cross: f32 = lines.iter().map(|l| l.cross_size).sum::<f32>()
                    + cross_gap * (lines.len().saturating_sub(1)) as f32;
                (max_main, total_cross)
            }
            Axis::Vertical => {
                // Main axis is vertical (height), cross axis is horizontal (width)
                let max_main: f32 = lines
                    .iter()
                    .flat_map(|l| l.items.iter())
                    .map(|item| item.main_position + item.target_main_size)
                    .fold(0.0f32, f32::max);
                let total_cross: f32 = lines.iter().map(|l| l.cross_size).sum::<f32>()
                    + cross_gap * (lines.len().saturating_sub(1)) as f32;
                (max_main, total_cross)
            }
        };

        // Update container height. Auto heights take the content size; an
        // EXPLICIT height that reaches here unresolved (content.height still
        // 0.0 — e.g. a nested inline-flex laid out mid-pass, like the
        // settings toggles) gets RESOLVED from style, never clobbered with
        // the children's sum. The old `== 0.0 ||` arm did exactly that
        // clobbering: height:26px toggles grew to their content (40.4px)
        // whenever they sat inside another flex row.
        let content_size = match main_axis {
            Axis::Horizontal => total_cross,
            Axis::Vertical => total_main,
        };
        if let Some(used_main) = inset_used_main {
            // CSS2 §10.6.4, a third time and in the other direction: an
            // inset-stretched box's `height: auto` does not mean "size me by
            // my content". The constraint equation already fixed the used
            // height, and the `Auto` arm below would overwrite it with the
            // items' sum — shrinking a 288px card overlay back to the 19.65px
            // of text it contains, which is exactly what happened when the
            // re-anchor re-ran this pass to re-justify the line.
            container.dimensions.content.height = used_main;
        } else if matches!(container.style.height, rustkit_css::Length::Auto) {
            container.dimensions.content.height = content_size;
        } else if container.dimensions.content.height == 0.0 {
            let explicit = match container.style.height {
                rustkit_css::Length::Px(px) => Some(px),
                rustkit_css::Length::Percent(pct) if container_box.content.height > 0.0 => {
                    Some(pct / 100.0 * container_box.content.height)
                }
                // Same condition as the percentage arm: a `calc()` is a
                // length, and it needs the containing block only for its
                // percentage term. Line 234's `is_definite` already counts a
                // calc height as specified, so leaving it on `_` would make
                // the container definite and then give it no used height.
                rustkit_css::Length::Calc(ref sum)
                    if sum.percent == 0.0 || container_box.content.height > 0.0 =>
                {
                    Some(resolve_length(
                        &container.style.height,
                        container_box.content.height,
                    ))
                }
                _ => None,
            };
            match explicit {
                Some(h) => {
                    // Specified heights are border-box under border-box sizing.
                    let pb = container.dimensions.padding.vertical()
                        + container.dimensions.border.vertical();
                    let is_bb = container.style.box_sizing == rustkit_css::BoxSizing::BorderBox;
                    container.dimensions.content.height = if is_bb { (h - pb).max(0.0) } else { h };
                }
                None => container.dimensions.content.height = content_size,
            }
        }
    }

    // 13. css-flexbox-1 §4.1: the STATIC POSITION of an out-of-flow child of a
    // flex container is where it would sit "as if it were the sole flex item"
    // — `justify-content` on the main axis, `align-self`/`align-items` on the
    // cross axis, inside the container's content box. Step 2 drops these
    // children from item collection and nothing put them back, so they kept
    // the BLOCK flow cursor the pre-pass gave them: new_tab's
    // `.footer { position: fixed; bottom: 1rem }` sat at x=0 under a
    // `align-items: center` body where Chrome centres it at 571.20 — the
    // largest single geometry error on that case and the only one on it that
    // does not depend on the font stack.
    //
    // Only an axis whose BOTH insets are auto takes the static position: a
    // specified `left`/`right`/`top`/`bottom` is resolved against the real
    // containing block (§10.3.7) by `apply_position_offsets_absolute`, and
    // that resolution must win. Measured against Chrome 148 on a 14-shape
    // probe: `top: 10px` keeps y and still centres x, `left: 10px` keeps x and
    // still centres y.
    {
        let main_is_horizontal = main_axis == Axis::Horizontal;
        let reverse_main = direction.is_reverse();
        let reverse_cross = wrap == FlexWrap::WrapReverse;
        let container_align_items = container.style.align_items;
        let container_justify = container.style.justify_content;
        // The container's content box, read from the same place steps 6–10
        // read it (`container_origin`, `container_main_size`): the caller
        // passes the container's own dimensions, so this is its content box.
        let content = container_box.content;

        // The main size to align in. On the horizontal axis the container's
        // used content width is final here. On the vertical axis it is NOT —
        // `container_main_size` is the CONTAINING BLOCK's number and
        // content.height is the children's stack — so the size is re-derived
        // by the same rule step 11d uses for the number it justifies in: a
        // definite `height` from style, else the content extent floored at
        // `min-height` (`min-height: 100vh` is the centring idiom on
        // new_tab's body, and Chrome aligns in the floored 800, not the stack).
        let used_inner_main = if main_is_horizontal {
            content.width
        } else {
            let pb = container.dimensions.padding.vertical() + container.dimensions.border.vertical();
            let is_bb = container.style.box_sizing == rustkit_css::BoxSizing::BorderBox;
            let inner_from_spec = |raw: f32| if is_bb { (raw - pb).max(0.0) } else { raw };
            match container.style.height {
                Length::Px(v) => inner_from_spec(v),
                _ => {
                    let min = match container.style.min_height {
                        Length::Px(px) => inner_from_spec(px),
                        Length::Vh(vh) => inner_from_spec(vh / 100.0 * container.viewport.1),
                        _ => 0.0,
                    };
                    // Step 12 has already written the flowed extent here; the
                    // passed box still carries the pre-pass stack.
                    container.dimensions.content.height.max(min)
                }
            }
        };
        // The cross size is the one steps 6–10 aligned the in-flow items in,
        // so an out-of-flow child and its in-flow siblings cannot disagree
        // about where the cross axis ends.
        let used_inner_cross = definite_inner_cross.unwrap_or(if main_is_horizontal {
            content.height
        } else {
            content.width
        });

        for child in &mut container.children {
            if !matches!(
                child.style.position,
                rustkit_css::Position::Absolute | rustkit_css::Position::Fixed
            ) {
                continue;
            }

            let offsets = child.resolved_offsets(container_box);
            let main_auto = if main_is_horizontal {
                offsets.left.is_none() && offsets.right.is_none()
            } else {
                offsets.top.is_none() && offsets.bottom.is_none()
            };
            let cross_auto = if main_is_horizontal {
                offsets.top.is_none() && offsets.bottom.is_none()
            } else {
                offsets.left.is_none() && offsets.right.is_none()
            };
            if !main_auto && !cross_auto {
                continue;
            }

            let margin_box = child.dimensions.margin_box();
            let (outer_main, outer_cross) = if main_is_horizontal {
                (margin_box.width, margin_box.height)
            } else {
                (margin_box.height, margin_box.width)
            };

            // A sole item leaves all the free space to the alignment keyword;
            // the distribution keywords degenerate: space-between packs to
            // main-start, space-around and space-evenly to the centre.
            let free_main = (used_inner_main - outer_main).max(0.0);
            let main_start = match container_justify {
                JustifyContent::FlexStart | JustifyContent::SpaceBetween => 0.0,
                JustifyContent::FlexEnd => free_main,
                JustifyContent::Center
                | JustifyContent::SpaceAround
                | JustifyContent::SpaceEvenly => free_main / 2.0,
            };
            // `row-reverse`/`column-reverse` put main-start at the far edge.
            let main_start = if reverse_main {
                free_main - main_start
            } else {
                main_start
            };

            let free_cross = (used_inner_cross - outer_cross).max(0.0);
            let align = resolved_align(child.style.align_self, container_align_items);
            let cross_start = match align {
                // A static-position box is not stretched by `stretch`, and
                // `baseline` has no line to sit on: both align to cross-start.
                AlignItems::FlexStart | AlignItems::Stretch | AlignItems::Baseline => 0.0,
                AlignItems::FlexEnd => free_cross,
                AlignItems::Center => free_cross / 2.0,
            };
            let cross_start = if reverse_cross {
                free_cross - cross_start
            } else {
                cross_start
            };

            let (target_x, target_y) = if main_is_horizontal {
                (content.x + main_start, content.y + cross_start)
            } else {
                (content.x + cross_start, content.y + main_start)
            };

            // Positions are absolute, so move by the delta on the MARGIN box
            // and carry the already-laid-out subtree (translate_subtree) —
            // shifting the box origin alone strands its text and children.
            let current = child.dimensions.margin_box();
            let (horizontal_auto, vertical_auto) = if main_is_horizontal {
                (main_auto, cross_auto)
            } else {
                (cross_auto, main_auto)
            };
            let dx = if horizontal_auto {
                target_x - current.x
            } else {
                0.0
            };
            let dy = if vertical_auto {
                target_y - current.y
            } else {
                0.0
            };
            if dx != 0.0 || dy != 0.0 {
                child.dimensions.content.x += dx;
                child.dimensions.content.y += dy;
                for grandchild in &mut child.children {
                    translate_subtree(grandchild, dx, dy);
                }
            }
        }
    }
}

/// Create a FlexItem from a LayoutBox.
fn create_flex_item<'a>(
    layout_box: &'a mut LayoutBox,
    main_axis: Axis,
    container_main: f32,
    container_cross: f32,
    definite_inner_cross: Option<f32>,
) -> FlexItem<'a> {
    // Extract all values from style first to avoid borrow conflicts
    let order = layout_box.style.order;
    let flex_grow = layout_box.style.flex_grow;
    let flex_shrink = layout_box.style.flex_shrink;
    let flex_basis_value = layout_box.style.flex_basis;
    let align_self = layout_box.style.align_self;

    // Get margins
    let (main_margin_start, main_margin_end, cross_margin_start, cross_margin_end) = match main_axis
    {
        Axis::Horizontal => (
            resolve_length(&layout_box.style.margin_left, container_main),
            resolve_length(&layout_box.style.margin_right, container_main),
            resolve_length(&layout_box.style.margin_top, container_cross),
            resolve_length(&layout_box.style.margin_bottom, container_cross),
        ),
        Axis::Vertical => (
            resolve_length(&layout_box.style.margin_top, container_main),
            resolve_length(&layout_box.style.margin_bottom, container_main),
            resolve_length(&layout_box.style.margin_left, container_cross),
            resolve_length(&layout_box.style.margin_right, container_cross),
        ),
    };

    // Padding and border were resolved onto dimensions by the block
    // pre-pass that runs before flex (layout_block_with_definite_height),
    // so read them from there. All flex sizes below are border-box: a
    // specified size under box-sizing:content-box gains padding+border,
    // under border-box it is used as-is. Intrinsic estimates measure
    // content and always gain padding+border.
    let (main_pb_start, main_pb_end, cross_pb_start, cross_pb_end) = {
        let d = &layout_box.dimensions;
        match main_axis {
            Axis::Horizontal => (
                d.padding.left + d.border.left,
                d.padding.right + d.border.right,
                d.padding.top + d.border.top,
                d.padding.bottom + d.border.bottom,
            ),
            Axis::Vertical => (
                d.padding.top + d.border.top,
                d.padding.bottom + d.border.bottom,
                d.padding.left + d.border.left,
                d.padding.right + d.border.right,
            ),
        }
    };
    let main_pb = main_pb_start + main_pb_end;
    let cross_pb = cross_pb_start + cross_pb_end;
    let is_border_box = layout_box.style.box_sizing == rustkit_css::BoxSizing::BorderBox;
    let spec_main_to_border_box = |v: f32| if is_border_box { v } else { v + main_pb };
    let spec_cross_to_border_box = |v: f32| if is_border_box { v } else { v + cross_pb };

    // Calculate flex basis (border-box)
    let content_sized_box = matches!(
        layout_box.box_type,
        crate::BoxType::Block | crate::BoxType::Inline | crate::BoxType::AnonymousBlock
    );
    let mut main_size_from_content = false;
    let flex_basis = match flex_basis_value {
        FlexBasis::Auto => {
            // Use main size property, or intrinsic size for replaced elements
            let explicit_size = match main_axis {
                Axis::Horizontal => resolve_length(&layout_box.style.width, container_main),
                Axis::Vertical => resolve_length(&layout_box.style.height, container_main),
            };

            // If explicit size is 0 (auto), check for intrinsic sizing
            if explicit_size == 0.0 {
                // Get intrinsic size for replaced elements (form controls, images)
                main_size_from_content = content_sized_box;
                get_intrinsic_main_size(layout_box, main_axis) + main_pb
            } else {
                spec_main_to_border_box(explicit_size)
            }
        }
        FlexBasis::Content => {
            // Use content size - for replaced elements, use intrinsic size
            main_size_from_content = content_sized_box;
            get_intrinsic_main_size(layout_box, main_axis) + main_pb
        }
        FlexBasis::Length(len) => spec_main_to_border_box(len),
        FlexBasis::Percent(pct) => spec_main_to_border_box(pct / 100.0 * container_main),
    };

    // Get min/max constraints from CSS
    let (css_min_main, max_main, css_min_cross, max_cross) = match main_axis {
        Axis::Horizontal => (
            resolve_length(&layout_box.style.min_width, container_main),
            resolve_max_length(&layout_box.style.max_width, container_main),
            resolve_length(&layout_box.style.min_height, container_cross),
            resolve_max_length(&layout_box.style.max_height, container_cross),
        ),
        Axis::Vertical => (
            resolve_length(&layout_box.style.min_height, container_main),
            resolve_max_length(&layout_box.style.max_height, container_main),
            resolve_length(&layout_box.style.min_width, container_cross),
            resolve_max_length(&layout_box.style.max_width, container_cross),
        ),
    };

    // For replaced elements (form controls, images), use intrinsic size as minimum
    // This ensures flex items have proper sizing even without explicit min-width/height
    let intrinsic_cross = get_intrinsic_cross_size(layout_box, main_axis);
    // CSS Flexbox §4.5 — automatic minimum size.
    //
    // `min-width: auto` is the DEFAULT for a flex item, and the spec resolves
    // it to the item's content-based minimum (min-content), not to zero.
    // Flooring at zero let shrink_items() squeeze items arbitrarily narrow,
    // including text: layout believed a run was 9.36px wide while paint drew
    // it at its true 18.66px, because the shaper is downstream of this and
    // never saw the squeeze. That mismatch is what put overlapping keyboard
    // chips on the new-tab page.
    //
    // Note the shape of the bug: the CROSS axis a few lines below already
    // falls back to `intrinsic_cross + cross_pb`. Only the main axis fell
    // through to 0.0, so the two axes disagreed about whether an unset
    // minimum means "no floor" or "the content floor".
    //
    // Conditions, both required by the spec:
    //   - the specified minimum is `auto` — an AUTHOR writing `min-width: 0`
    //     is explicitly asking to shrink to nothing and must keep getting it,
    //     which is why this tests the Length variant rather than `> 0.0`
    //     (resolve_length maps both Auto and Px(0) to 0.0).
    //   - the item's own overflow on the main axis is `visible`; any other
    //     value means the item can clip its content, so the content stops
    //     floring the box.
    //
    // estimate_min_content_width already returns a border-box figure for
    // element boxes (it adds padding+border itself) and a bare text measure
    // for text runs, which have neither. So it is used RAW — passing it
    // through spec_main_to_border_box would count padding twice.
    let specified_min_is_auto = matches!(
        match main_axis {
            Axis::Horizontal => &layout_box.style.min_width,
            Axis::Vertical => &layout_box.style.min_height,
        },
        rustkit_css::Length::Auto
    );
    let main_overflow_is_visible = matches!(
        match main_axis {
            Axis::Horizontal => layout_box.style.overflow_x,
            Axis::Vertical => layout_box.style.overflow_y,
        },
        rustkit_css::Overflow::Visible
    );

    let min_main = if css_min_main > 0.0 {
        spec_main_to_border_box(css_min_main)
    } else if specified_min_is_auto && main_overflow_is_visible {
        match main_axis {
            Axis::Horizontal => crate::grid::estimate_min_content_width(layout_box),
            // No min-content HEIGHT estimator exists yet. Returning 0.0 keeps
            // the previous behaviour on the vertical main axis rather than
            // inventing a number — stated so the gap is visible instead of
            // looking like the rule is implemented on both axes.
            Axis::Vertical => 0.0,
        }
    } else {
        0.0
    };
    let max_main = if max_main.is_finite() {
        spec_main_to_border_box(max_main)
    } else {
        max_main
    };
    // Check if the cross size is explicitly set (not auto)
    // Per CSS spec, items with explicit cross size should NOT be stretched
    let explicit_cross_length = match main_axis {
        Axis::Horizontal => &layout_box.style.height,
        Axis::Vertical => &layout_box.style.width,
    };
    let explicit_cross_size = match explicit_cross_length {
        rustkit_css::Length::Auto => None,
        // css-sizing-3 §5.1: a percentage resolves against the containing
        // block's corresponding size when that size is DEFINITE, and behaves
        // as `auto` when it is not. The containing block here is the flex
        // container, so the basis is its own definite inner cross size —
        // never `container_cross`, which is the containing block's number one
        // level further out. `.sidebar-toggle { height: 100% }` inside
        // `.nav-bar { height: 44px }` resolved against the 100px viewport and
        // came out 100 tall against Chrome's 43.
        //
        // With no definite basis the percentage stays on the content-measure
        // path, which is what `auto` does, so an indefinite container keeps
        // exactly the behaviour it had.
        rustkit_css::Length::Percent(pct) => {
            definite_inner_cross.map(|basis| spec_cross_to_border_box(pct / 100.0 * basis))
        }
        l => Some(spec_cross_to_border_box(resolve_length(l, container_cross))),
    };
    let has_explicit_cross_size = !matches!(explicit_cross_length, rustkit_css::Length::Auto);

    let min_cross = if css_min_cross > 0.0 {
        spec_cross_to_border_box(css_min_cross)
    } else if explicit_cross_size.is_some() {
        // An author-specified cross size is used as specified. The intrinsic
        // floor below is for CONTENT-sized items; applying it to an explicit
        // size makes any control smaller than its intrinsic box impossible to
        // author. css-flexbox-1 4.5's automatic minimum is a MAIN-axis rule
        // and only applies when the size is `auto` — note min_main above
        // correctly floors at 0.0. This asymmetry is what rendered the shelf's
        // 24x24 close button 36 tall (font_size*1.5+12, the intrinsic button
        // height) and inflated the header to 53 against Chrome's 41.
        0.0
    } else {
        intrinsic_cross + cross_pb
    };
    let max_cross = if max_cross.is_finite() {
        spec_cross_to_border_box(max_cross)
    } else {
        max_cross
    };

    // Hypothetical main size (clamped)
    let hypothetical_main_size = flex_basis.max(min_main).min(max_main);

    FlexItem {
        layout_box,
        order,
        flex_grow,
        flex_shrink,
        flex_basis,
        hypothetical_main_size,
        target_main_size: hypothetical_main_size,
        frozen: false,
        cross_size: 0.0,
        main_position: 0.0,
        cross_position: 0.0,
        min_main_size: min_main,
        max_main_size: max_main,
        min_cross_size: min_cross,
        max_cross_size: max_cross,
        align_self,
        main_margin_start,
        main_margin_end,
        cross_margin_start,
        cross_margin_end,
        has_explicit_cross_size,
        explicit_cross_size,
        main_pb_start,
        main_pb_end,
        cross_pb_start,
        cross_pb_end,
        main_size_from_content,
    }
}

/// Collect items into flex lines based on wrap property.
fn collect_flex_lines<'a>(
    mut items: Vec<FlexItem<'a>>,
    container_main: f32,
    main_gap: f32,
    wrap: FlexWrap,
) -> Vec<FlexLine<'a>> {
    if items.is_empty() {
        return Vec::new();
    }

    if wrap == FlexWrap::NoWrap {
        // Single line
        let mut line = FlexLine::new();
        line.items = items;
        return vec![line];
    }

    // Multi-line
    let mut lines = Vec::new();
    let mut current_line = FlexLine::new();
    let mut line_main_size = 0.0f32;

    for item in items.drain(..) {
        let item_size = item.outer_hypothetical_main_size();
        let gap = if current_line.items.is_empty() {
            0.0
        } else {
            main_gap
        };

        if !current_line.items.is_empty() && line_main_size + gap + item_size > container_main {
            // Start new line
            lines.push(current_line);
            current_line = FlexLine::new();
            line_main_size = 0.0;
        }

        line_main_size += if current_line.items.is_empty() {
            0.0
        } else {
            main_gap
        };
        line_main_size += item_size;
        current_line.items.push(item);
    }

    if !current_line.items.is_empty() {
        lines.push(current_line);
    }

    lines
}

/// Resolve flexible lengths (grow/shrink) for a line.
fn resolve_flexible_lengths(line: &mut FlexLine, container_main: f32, main_gap: f32) {
    if line.items.is_empty() {
        return;
    }

    // Calculate used space
    let total_gaps = main_gap * (line.items.len().saturating_sub(1)) as f32;
    let used_space: f32 = line
        .items
        .iter()
        .map(|i| i.hypothetical_main_size + i.main_margin_start + i.main_margin_end)
        .sum();
    let free_space = container_main - used_space - total_gaps;

    if free_space.abs() < 0.01 {
        // No adjustment needed
        return;
    }

    // Reset frozen state
    for item in &mut line.items {
        item.frozen = false;
        item.target_main_size = item.hypothetical_main_size;
    }

    if free_space > 0.0 {
        // Grow items
        grow_items(line, free_space);
    } else {
        // Shrink items
        shrink_items(line, -free_space);
    }
}

/// Grow items to fill free space.
fn grow_items(line: &mut FlexLine, free_space: f32) {
    let total_grow: f32 = line
        .items
        .iter()
        .filter(|i| !i.frozen)
        .map(|i| i.flex_grow)
        .sum();

    if total_grow <= 0.0 {
        return;
    }

    let space_per_grow = free_space / total_grow;

    for item in &mut line.items {
        if item.frozen {
            continue;
        }

        let grow = item.flex_grow * space_per_grow;
        let new_size = item.target_main_size + grow;

        if new_size > item.max_main_size {
            item.target_main_size = item.max_main_size;
            item.frozen = true;
        } else {
            item.target_main_size = new_size;
        }
    }
}

/// Shrink items to remove overflow.
fn shrink_items(line: &mut FlexLine, overflow: f32) {
    let total_shrink_scaled: f32 = line
        .items
        .iter()
        .filter(|i| !i.frozen)
        .map(|i| i.flex_shrink * i.flex_basis)
        .sum();

    if total_shrink_scaled <= 0.0 {
        return;
    }

    for item in &mut line.items {
        if item.frozen {
            continue;
        }

        let shrink_scaled = item.flex_shrink * item.flex_basis;
        let shrink_ratio = shrink_scaled / total_shrink_scaled;
        let shrink = overflow * shrink_ratio;
        let new_size = (item.target_main_size - shrink).max(item.min_main_size);

        if new_size <= item.min_main_size {
            item.target_main_size = item.min_main_size;
            item.frozen = true;
        } else {
            item.target_main_size = new_size;
        }
    }
}

/// Calculate cross sizes for items in a line.
///
/// The `has_definite_cross_size` parameter indicates whether the flex container
/// has a definite (non-auto) cross size. This affects stretch behavior:
/// - With definite cross size: stretch items to fill the container
/// - With auto cross size: stretch items to match the tallest item in the line
fn calculate_cross_sizes(
    line: &mut FlexLine,
    container_cross: f32,
    align_items: AlignItems,
    has_definite_cross_size: bool,
    cross_axis: Axis,
) {
    // PASS 1: Calculate content-based cross sizes for ALL items (ignore stretch for now)
    // This determines the "natural" height of each item
    let mut content_cross_sizes: Vec<f32> = Vec::with_capacity(line.items.len());

    for item in &mut line.items {
        // Compute the hypothetical cross size (border-box): the explicit
        // cross size when specified, otherwise the content-based size plus
        // the item's own padding+border.
        let content_cross_size = match item.explicit_cross_size {
            Some(explicit) => explicit,
            // On the HORIZONTAL cross axis an item that will NOT be stretched
            // keeps its fit-content width, which the already-laid-out width is
            // not. See `fit_content_cross_width` for what was wrong with it and
            // `SCOPE` below for why a stretching item is left alone.
            None if cross_axis == Axis::Horizontal
                && resolved_align(item.align_self, align_items) != AlignItems::Stretch =>
            {
                let available = (container_cross
                    - item.cross_margin_start
                    - item.cross_margin_end)
                    .max(0.0);
                fit_content_cross_width(item.layout_box, available, item.cross_pb())
            }
            None => get_content_cross_size(item.layout_box, cross_axis) + item.cross_pb(),
        };

        // Apply min/max constraints to content size
        let constrained_size = content_cross_size
            .max(item.min_cross_size)
            .min(item.max_cross_size);
        content_cross_sizes.push(constrained_size);

        // Initially set cross_size to content size
        item.cross_size = constrained_size;
    }

    // Compute the line cross size based on content sizes (largest item outer cross size)
    let line_cross_size = line
        .items
        .iter()
        .enumerate()
        .map(|(i, item)| content_cross_sizes[i] + item.cross_margin_start + item.cross_margin_end)
        .fold(0.0, f32::max);

    // PASS 2: Apply stretch behavior based on container sizing
    for (i, item) in line.items.iter_mut().enumerate() {
        let align = resolved_align(item.align_self, align_items);

        // Per CSS spec: stretch only applies if cross size is "auto"
        // Items with explicit height/width should NOT be stretched
        if align == AlignItems::Stretch && !item.has_explicit_cross_size {
            // Determine the stretch target based on container cross size
            let stretch_target = if has_definite_cross_size {
                // Container has definite height - stretch to fill container
                container_cross - item.cross_margin_start - item.cross_margin_end
            } else {
                // Container has auto height - stretch to match tallest item in line
                line_cross_size - item.cross_margin_start - item.cross_margin_end
            };

            // Stretch, but never below content size
            item.cross_size = stretch_target.max(content_cross_sizes[i]);
        }

        // Clamp to min/max
        item.cross_size = item
            .cross_size
            .max(item.min_cross_size)
            .min(item.max_cross_size);
    }

    // Set line cross size (largest item outer cross size after stretch)
    line.cross_size = line
        .items
        .iter()
        .map(|i| i.cross_size + i.cross_margin_start + i.cross_margin_end)
        .fold(0.0, f32::max);
}

/// `align-self` resolved against the container's `align-items`.
///
/// Two passes of `calculate_cross_sizes` need the same answer — the one that
/// decides whether an item keeps its fit-content cross size and the one that
/// stretches it — and a copy of this mapping that drifts from the other puts
/// an item at a size one pass chose and a position the other did.
fn resolved_align(align_self: AlignSelf, align_items: AlignItems) -> AlignItems {
    match align_self {
        AlignSelf::Auto => align_items,
        AlignSelf::FlexStart => AlignItems::FlexStart,
        AlignSelf::FlexEnd => AlignItems::FlexEnd,
        AlignSelf::Center => AlignItems::Center,
        AlignSelf::Baseline => AlignItems::Baseline,
        AlignSelf::Stretch => AlignItems::Stretch,
    }
}

/// Whether the intrinsic estimators can actually measure this subtree.
///
/// `estimate_max_content_width` knows about text, explicit pixel widths, flex
/// containers and children. It has no case for `BoxType::Image` or
/// `BoxType::FormControl`, so a replaced or form box with no specified pixel
/// width contributes **zero** to its ancestors' max-content — and an estimate
/// that is silently too small becomes a box that is silently too narrow the
/// moment it is used as a size rather than as a track-sizing hint.
///
/// This is not a guess about which subtrees are risky. Every axis the first
/// version of this fix added to Gate A traced to one such box:
/// `new_tab`'s `.search-input` (`width: 100%`, so no pixel width) zeroed the
/// max-content of `.container`, `.search-container` and `.shortcuts-section`
/// above it, and `settings`' `.blocklist-add` holds the same shape. Where the
/// estimators cannot see the content, the previously measured width is kept.
///
/// Percentage widths are deliberately NOT disqualifying: css-sizing-3 §5.2.2
/// says a percentage resolves against the containing block that is being
/// sized, so treating it as no contribution is the correct intrinsic answer,
/// not a gap. The gap is the replaced/control box underneath it.
fn estimators_can_measure(layout_box: &LayoutBox) -> bool {
    let opaque = matches!(
        layout_box.box_type,
        crate::BoxType::Image { .. } | crate::BoxType::FormControl(_)
    ) && !matches!(layout_box.style.width, Length::Px(_));
    if opaque {
        return false;
    }
    layout_box.children.iter().all(estimators_can_measure)
}

/// The hypothetical cross size of a flex item on the HORIZONTAL cross axis,
/// i.e. in a `flex-direction: column` container, as a BORDER-box width.
///
/// css-flexbox-1 §9.4 step 7 sizes an item whose cross size is `auto` to its
/// fit-content size, which css-sizing-3 §5.1 defines as
/// `min(max(min-content, available), max-content)`.
///
/// The previous answer came from `get_content_cross_width`, whose first line
/// is *"an already-laid-out width is the best answer available"* — the width a
/// prior block pass left on the box. For a `width: auto` child that width is
/// the whole containing block, so a column flex container handed every item
/// its own full width. That is invisible while `align-items` is `stretch`,
/// because the item would be stretched to exactly that anyway; it is the
/// entire defect the moment it is not. On the settings page,
/// `.setting-row[style="flex-direction: column; align-items: flex-start"]`
/// gave `.setting-label` and `.checkbox-group` 660px where Chrome gives
/// 205.25 and 221.23 — and the text that should have wrapped inside them did
/// not, so the page came out 306.67px too short below.
///
/// SCOPE: items whose resolved alignment is `stretch` are deliberately left on
/// the old measurement. Their used cross size is the stretch target, so the
/// hypothetical size only reaches them through the auto-cross-size line
/// (`line_cross_size`) — and narrowing that line shrinks a stretching item to
/// its content instead of to its container. Measured, not assumed: applying
/// this to every item took `shelf`'s `#commandResults` from 1248 to 1216 and
/// `new_tab`'s `.shortcuts-section` from 536 to 360, adding 12 failing axes.
/// A column container with `width: auto` still has a used width its items
/// should stretch to; that the stretch path reads the content line instead is
/// a separate defect, and it is not this one.
///
/// The estimators already include the item's own padding and border, so this
/// returns a border-box figure and the caller must NOT add `cross_pb()`.
///
/// LIMIT, stated rather than left to be discovered: the estimators measure
/// text, explicit widths and children, and know nothing about replaced or
/// form-control content — a childless `<img>` or `<input>` with no specified
/// width estimates 0. Collapsing such an item to zero would be a new defect
/// in place of the old one, so where max-content reports nothing this keeps
/// the previously measured width. That fallback is the OLD behaviour and is
/// not a fix; it is the blast radius held to boxes the estimators cannot see.
fn fit_content_cross_width(
    layout_box: &LayoutBox,
    available_border_box: f32,
    cross_pb: f32,
) -> f32 {
    if !estimators_can_measure(layout_box) {
        return get_content_cross_width(layout_box) + cross_pb;
    }
    let preferred = crate::grid::estimate_max_content_width(layout_box);
    if preferred <= 0.0 {
        return get_content_cross_width(layout_box) + cross_pb;
    }
    let preferred_min = crate::grid::estimate_min_content_width(layout_box);
    preferred_min.max(available_border_box.min(preferred))
}

/// Get the content-based cross size for a layout box.
/// This computes the hypothetical cross size based on content, intrinsic sizing, or children.
/// Content-based size of an item along the CROSS axis.
///
/// This used to be height-only, with no idea which axis it was measuring. In
/// a `flex-direction: column` container the cross axis is HORIZONTAL, so a
/// height was being handed back as a width: `apply_positions` then wrote it
/// into the box's width and produced literally square boxes whose width
/// tracked their line count. A two-line item came out 32x32.
///
/// Splitting on the axis is the fix. The vertical path is the original
/// behaviour, untouched; the horizontal path is new and must never fall back
/// to line-height, which is the specific wrong answer that caused this.
fn get_content_cross_size(layout_box: &LayoutBox, cross_axis: Axis) -> f32 {
    match cross_axis {
        Axis::Vertical => get_content_cross_height(layout_box),
        Axis::Horizontal => get_content_cross_width(layout_box),
    }
}

/// Content-based WIDTH, for items in a column flex container.
///
/// Returns a CONTENT-box figure: the caller adds the item's own padding and
/// border via `cross_pb()`, so including them here would double-count.
fn get_content_cross_width(layout_box: &LayoutBox) -> f32 {
    // An already-laid-out width is the best answer available.
    if layout_box.dimensions.content.width > 0.0 {
        return layout_box.dimensions.content.width;
    }

    let font_size = match layout_box.style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };

    if let crate::BoxType::Text(text) = &layout_box.box_type {
        return crate::measure_text_advanced(
            text,
            &layout_box.style.font_family,
            font_size,
            layout_box.style.font_weight,
            layout_box.style.font_style,
        )
        .width;
    }

    if let crate::BoxType::Image { natural_width, .. } = &layout_box.box_type {
        if *natural_width > 0.0 {
            return *natural_width;
        }
    }

    match layout_box.style.width {
        Length::Px(px) if px > 0.0 => return px,
        Length::Em(em) if em > 0.0 => return em * font_size,
        _ => {}
    }

    // Block-level children STACK vertically, so the container's content width
    // is the widest child — not the sum, which is the row-axis answer.
    if !layout_box.children.is_empty() {
        let widest = layout_box
            .children
            .iter()
            .map(|c| c.dimensions.margin_box().width)
            .fold(0.0f32, f32::max);
        if widest > 0.0 {
            return widest;
        }
    }

    // Deliberately 0.0 rather than line-height. On the horizontal axis a line
    // height is not a width, and returning one is what produced square boxes.
    // Zero lets the stretch path supply the real number.
    0.0
}

/// Content-based HEIGHT — the original implementation, unchanged in behaviour.
fn get_content_cross_height(layout_box: &LayoutBox) -> f32 {
    // If the box already has a computed height from layout, use it
    if layout_box.dimensions.content.height > 0.0 {
        return layout_box.dimensions.content.height;
    }

    // Get font size for intrinsic calculations
    let font_size = match layout_box.style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };

    // Get line height (used for text and inline boxes)
    let line_height = crate::resolve_line_height(&layout_box.style, font_size);

    // For text boxes, use line height
    if let crate::BoxType::Text(_) = &layout_box.box_type {
        return line_height;
    }

    // For inline boxes, use line height as minimum cross size
    // This ensures proper vertical rhythm in flex containers
    if let crate::BoxType::Inline = &layout_box.box_type {
        return line_height;
    }

    // For form controls, use the intrinsic border-box height block flow uses
    if let crate::BoxType::FormControl(control) = &layout_box.box_type {
        return crate::form_control_intrinsic_size(&layout_box.style, control).1;
    }

    // For images, use natural height
    if let crate::BoxType::Image { natural_height, .. } = &layout_box.box_type {
        if *natural_height > 0.0 {
            return *natural_height;
        }
    }

    // For containers with children, sum children heights (for block) or use max (for inline)
    if !layout_box.children.is_empty() {
        let children_height: f32 = layout_box
            .children
            .iter()
            .map(|c| c.dimensions.margin_box().height)
            .sum();
        if children_height > 0.0 {
            return children_height;
        }
    }

    // Check for explicit CSS height
    match layout_box.style.height {
        Length::Px(px) if px > 0.0 => return px,
        Length::Em(em) if em > 0.0 => return em * font_size,
        _ => {}
    }

    // For inline/block boxes without content, use line height as minimum
    crate::resolve_line_height(&layout_box.style, font_size)
}

/// Distribute lines according to align-content.
fn distribute_lines(
    lines: &mut [FlexLine],
    container_cross: f32,
    _total_cross: f32,
    cross_gap: f32,
    align_content: AlignContent,
) {
    if lines.is_empty() {
        return;
    }

    let total_line_size: f32 = lines.iter().map(|l| l.cross_size).sum();
    let total_gaps = cross_gap * (lines.len().saturating_sub(1)) as f32;
    let free_space = (container_cross - total_line_size - total_gaps).max(0.0);

    let (initial_offset, spacing) = match align_content {
        AlignContent::FlexStart => (0.0, cross_gap),
        AlignContent::FlexEnd => (free_space, cross_gap),
        AlignContent::Center => (free_space / 2.0, cross_gap),
        AlignContent::SpaceBetween => {
            if lines.len() > 1 {
                (0.0, free_space / (lines.len() - 1) as f32 + cross_gap)
            } else {
                (0.0, cross_gap)
            }
        }
        AlignContent::SpaceAround => {
            let space = free_space / lines.len() as f32;
            (space / 2.0, space + cross_gap)
        }
        AlignContent::SpaceEvenly => {
            let space = free_space / (lines.len() + 1) as f32;
            (space, space + cross_gap)
        }
        AlignContent::Stretch => {
            // Distribute free space to lines
            if free_space > 0.0 {
                let extra_per_line = free_space / lines.len() as f32;
                for line in lines.iter_mut() {
                    line.cross_size += extra_per_line;
                }
            }
            (0.0, cross_gap)
        }
    };

    // Set line positions
    let mut cross_pos = initial_offset;
    for line in lines.iter_mut() {
        line.cross_position = cross_pos;
        cross_pos += line.cross_size + spacing;
    }
}

/// Distribute items along main axis (justify-content).
/// The container's own used inner main size when `height: auto` is made
/// definite by opposite insets (CSS2 §10.6.4).
///
/// The arithmetic is `LayoutBox::inset_definite_content_height`, so this and
/// `apply_position_offsets_absolute` cannot drift. What is decided here is
/// only *which containing block to ask*: `position: fixed` resolves against
/// the viewport, not against the block that laid it out, exactly as
/// `apply_position_offsets` does — asking the passed block instead hands a
/// fixed overlay its parent's height and centres its items in the wrong box.
/// The bare-unit-tree fallback (no viewport set) is that same code's
/// fallback, not a new rule.
fn inset_definite_used_main(container: &LayoutBox, containing_block: &Dimensions) -> Option<f32> {
    if container.position == crate::Position::Fixed
        && container.viewport.0 > 0.0
        && container.viewport.1 > 0.0
    {
        let viewport_cb = Dimensions {
            content: Rect::new(0.0, 0.0, container.viewport.0, container.viewport.1),
            ..Default::default()
        };
        return container.inset_definite_content_height(&viewport_cb);
    }
    container.inset_definite_content_height(containing_block)
}

fn distribute_main_axis(
    line: &mut FlexLine,
    container_main: f32,
    main_gap: f32,
    justify_content: JustifyContent,
    reverse: bool,
) {
    if line.items.is_empty() {
        return;
    }

    let total_item_size: f32 = line.items.iter().map(|i| i.outer_main_size()).sum();
    let total_gaps = main_gap * (line.items.len().saturating_sub(1)) as f32;
    let free_space = (container_main - total_item_size - total_gaps).max(0.0);

    let (initial_offset, spacing) = match justify_content {
        JustifyContent::FlexStart => (0.0, main_gap),
        JustifyContent::FlexEnd => (free_space, main_gap),
        JustifyContent::Center => (free_space / 2.0, main_gap),
        JustifyContent::SpaceBetween => {
            if line.items.len() > 1 {
                (0.0, free_space / (line.items.len() - 1) as f32 + main_gap)
            } else {
                (0.0, main_gap)
            }
        }
        JustifyContent::SpaceAround => {
            let space = free_space / line.items.len() as f32;
            (space / 2.0, space + main_gap)
        }
        JustifyContent::SpaceEvenly => {
            let space = free_space / (line.items.len() + 1) as f32;
            (space, space + main_gap)
        }
    };

    // Position items
    let mut main_pos = initial_offset;
    let items_to_position: Vec<_> = if reverse {
        (0..line.items.len()).rev().collect()
    } else {
        (0..line.items.len()).collect()
    };

    for (i, &idx) in items_to_position.iter().enumerate() {
        let item = &mut line.items[idx];
        item.main_position = main_pos + item.main_margin_start;
        main_pos += item.outer_main_size();
        if i < items_to_position.len() - 1 {
            main_pos += spacing;
        }
    }
}

/// Align items on cross axis within line.
fn align_cross_axis(line: &mut FlexLine, align_items: AlignItems) {
    for item in &mut line.items {
        let align = if item.align_self == AlignSelf::Auto {
            align_items
        } else {
            match item.align_self {
                AlignSelf::Auto => align_items,
                AlignSelf::FlexStart => AlignItems::FlexStart,
                AlignSelf::FlexEnd => AlignItems::FlexEnd,
                AlignSelf::Center => AlignItems::Center,
                AlignSelf::Baseline => AlignItems::Baseline,
                AlignSelf::Stretch => AlignItems::Stretch,
            }
        };

        let outer_cross = item.cross_size + item.cross_margin_start + item.cross_margin_end;
        let free_space = (line.cross_size - outer_cross).max(0.0);

        item.cross_position = match align {
            AlignItems::FlexStart => item.cross_margin_start,
            AlignItems::FlexEnd => free_space + item.cross_margin_start,
            AlignItems::Center => free_space / 2.0 + item.cross_margin_start,
            AlignItems::Baseline => item.cross_margin_start, // Simplified
            AlignItems::Stretch => item.cross_margin_start,
        };
    }
}

/// Apply computed positions to layout boxes.
///
/// The `container_origin` is the (x, y) of the container's content area,
/// which is added to the flex-computed positions to get absolute coordinates.
fn apply_positions(
    lines: &mut [FlexLine],
    main_axis: Axis,
    _reverse_main: bool,
    reverse_cross: bool,
    container_origin: (f32, f32),
) {
    let (origin_x, origin_y) = container_origin;

    trace!(
        ?origin_x,
        ?origin_y,
        num_lines = lines.len(),
        "apply_positions: starting"
    );

    let lines_iter: Box<dyn Iterator<Item = &mut FlexLine>> = if reverse_cross {
        Box::new(lines.iter_mut().rev())
    } else {
        Box::new(lines.iter_mut())
    };

    for line in lines_iter {
        for item in &mut line.items {
            let (rel_x, rel_y, width, height) = match main_axis {
                Axis::Horizontal => (
                    item.main_position,
                    line.cross_position + item.cross_position,
                    item.target_main_size,
                    item.cross_size,
                ),
                Axis::Vertical => (
                    line.cross_position + item.cross_position,
                    item.main_position,
                    item.cross_size,
                    item.target_main_size,
                ),
            };

            let abs_x = origin_x + rel_x;
            let abs_y = origin_y + rel_y;

            trace!(
                ?rel_x,
                ?rel_y,
                ?abs_x,
                ?abs_y,
                ?width,
                ?height,
                main_position = item.main_position,
                cross_position = item.cross_position,
                line_cross_position = line.cross_position,
                "apply_positions: positioning flex item"
            );

            // Flex math above is border-box; dimensions.content is the
            // content rect, so inset by the item's own padding+border
            // (resolved onto dimensions by the block pre-pass).
            let d = &item.layout_box.dimensions;
            let pb_left = d.padding.left + d.border.left;
            let pb_right = d.padding.right + d.border.right;
            let pb_top = d.padding.top + d.border.top;
            let pb_bottom = d.padding.bottom + d.border.bottom;

            // Update layout box dimensions with absolute positions
            item.layout_box.dimensions.content = Rect {
                x: abs_x + pb_left,
                y: abs_y + pb_top,
                width: (width - pb_left - pb_right).max(0.0),
                height: (height - pb_top - pb_bottom).max(0.0),
            };

            // Set margins
            item.layout_box.dimensions.margin = match main_axis {
                Axis::Horizontal => EdgeSizes {
                    left: item.main_margin_start,
                    right: item.main_margin_end,
                    top: item.cross_margin_start,
                    bottom: item.cross_margin_end,
                },
                Axis::Vertical => EdgeSizes {
                    top: item.main_margin_start,
                    bottom: item.main_margin_end,
                    left: item.cross_margin_start,
                    right: item.cross_margin_end,
                },
            };
        }
    }
}

/// Shift a laid-out box and its entire subtree by (dx, dy).
/// Content rects hold absolute coordinates once layout has run, so every
/// descendant moves by the same delta.
pub(crate) fn translate_subtree(b: &mut crate::LayoutBox, dx: f32, dy: f32) {
    b.dimensions.content.x += dx;
    b.dimensions.content.y += dy;
    for child in &mut b.children {
        translate_subtree(child, dx, dy);
    }
}

/// Get the intrinsic main size for replaced elements (form controls, images).
fn get_intrinsic_main_size(layout_box: &crate::LayoutBox, main_axis: Axis) -> f32 {
    let box_type = &layout_box.box_type;
    let style = &layout_box.style;
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };

    match box_type {
        // One sizing model for block flow and flex items (n58): the blobs
        // that lived here ignored author padding/border.
        crate::BoxType::FormControl(control) => {
            let (w, h) = crate::form_control_intrinsic_size(style, control);
            match main_axis {
                Axis::Horizontal => w,
                Axis::Vertical => h,
            }
        }
        crate::BoxType::Image {
            natural_width,
            natural_height,
            ..
        } => match main_axis {
            Axis::Horizontal => *natural_width,
            Axis::Vertical => *natural_height,
        },
        crate::BoxType::Inline | crate::BoxType::Block | crate::BoxType::AnonymousBlock => {
            // Horizontal main axis: flex-basis:auto resolves to the item's
            // content size suggestion = MAX-content width (css-flexbox-1
            // §9.2.3.C) — text measured on one line, inline runs summed.
            // flex-shrink then pulls oversized items back to the container.
            // Falls back to line height when content gives nothing to
            // measure. Vertical main axis keeps the line-height heuristic
            // as a FIRST guess only: step 11d of layout_flex_container
            // replaces it with the laid-out height once the item's
            // children exist.
            //
            // This function's contract is a CONTENT figure — every caller
            // adds the item's own padding+border. estimate_max_content_width
            // returns a BORDER-box figure for element boxes (it folds
            // horizontal_padding_border in itself), so that term is taken
            // back out here. Without this every auto-width padded flex
            // item was max-content + 2×padding: flex-positioning's
            // `.flex-item { padding: 10px 20px }` measured 123.2 against
            // Chrome's 83.1 (+40), `.justify-item` +30, `.nested-item` +24.
            match main_axis {
                Axis::Horizontal => {
                    let border_box = crate::grid::estimate_max_content_width(layout_box);
                    let content =
                        (border_box - crate::grid::horizontal_padding_border(style)).max(0.0);
                    if content > 0.0 {
                        content
                    } else {
                        crate::resolve_line_height(style, font_size)
                    }
                }
                Axis::Vertical => crate::resolve_line_height(style, font_size),
            }
        }
        crate::BoxType::Text(text) => {
            // Anonymous text flex item: full single-line measure on the main
            // axis (max-content), line height on the cross/vertical axis.
            match main_axis {
                Axis::Horizontal => {
                    let w = crate::measure_text_advanced(
                        text,
                        &style.font_family,
                        font_size,
                        style.font_weight,
                        style.font_style,
                    )
                    .width;
                    if w > 0.0 {
                        w
                    } else {
                        crate::resolve_line_height(style, font_size)
                    }
                }
                Axis::Vertical => crate::resolve_line_height(style, font_size),
            }
        }
        // A forced break occupies no main-axis space of its own.
        crate::BoxType::LineBreak => 0.0,
    }
}

/// Get the intrinsic cross size for replaced elements (form controls, images).
/// This returns the height for horizontal main axis, width for vertical main axis.
fn get_intrinsic_cross_size(layout_box: &crate::LayoutBox, main_axis: Axis) -> f32 {
    let box_type = &layout_box.box_type;
    let style = &layout_box.style;
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };

    // Cross axis is the opposite of main axis
    let cross_axis = main_axis.cross();

    match box_type {
        crate::BoxType::FormControl(control) => {
            let (w, h) = crate::form_control_intrinsic_size(style, control);
            match cross_axis {
                Axis::Horizontal => w,
                Axis::Vertical => h,
            }
        }
        crate::BoxType::Image {
            natural_width,
            natural_height,
            ..
        } => {
            // Prefer the cross extent the block pre-pass already resolved for
            // this replaced element. `layout_image` sizes an image that has a
            // specified main size and an `auto` cross from its natural ratio,
            // so a `width: 80px` image with a 1:1 natural ratio is already 80
            // tall in `dimensions` by the time flex runs. Using the raw
            // `natural_height` (100) as the flex item's cross MINIMUM floored
            // that computed 80 back up to 100 — the 80x102 (vs Chrome 80x80)
            // defect on images-intrinsic test12, where the width applied but
            // the height stayed natural. Fall back to the natural dimension
            // only when nothing has been laid out yet (e.g. a bare item with
            // no pre-pass), which keeps the auto/auto image unchanged.
            let laid_out = match cross_axis {
                Axis::Vertical => layout_box.dimensions.content.height,
                Axis::Horizontal => layout_box.dimensions.content.width,
            };
            if laid_out > 0.0 {
                laid_out
            } else {
                match cross_axis {
                    Axis::Horizontal => *natural_width,
                    Axis::Vertical => *natural_height,
                }
            }
        }
        crate::BoxType::Text(_) => {
            // Text boxes have intrinsic height based on line height
            let line_height = crate::resolve_line_height(style, font_size);
            match cross_axis {
                Axis::Vertical => line_height,
                Axis::Horizontal => 0.0, // Text width depends on content
            }
        }
        _ => {
            // For block/inline boxes, provide a minimum based on line height
            // This ensures flex items have non-zero cross size
            let line_height = crate::resolve_line_height(style, font_size);
            match cross_axis {
                Axis::Vertical => line_height,
                Axis::Horizontal => 0.0,
            }
        }
    }
}

/// Resolve a Length to pixels.
fn resolve_length(length: &Length, container_size: f32) -> f32 {
    // Use the Length's built-in resolution with default viewport size
    length.to_px_with_viewport(16.0, 16.0, container_size, 800.0, 600.0)
}

/// Resolve a max Length (returns f32::INFINITY for Auto).
fn resolve_max_length(length: &Length, container_size: f32) -> f32 {
    match length {
        Length::Auto => f32::INFINITY,
        _ => resolve_length(length, container_size),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxType;
    use rustkit_css::{AlignItems, ComputedStyle, FlexDirection, JustifyContent, Length};

    #[test]
    fn test_header_nav_row_like_chrome() {
        // Regression (sticky-scroll header, 2026-07-10): four coupled bugs
        // scattered a sticky header's flex row:
        //   1. estimate_max_content_width ignored flex gaps, so a 30px-gap
        //      nav's basis came out 120px narrow and flex-shrink smashed
        //      every link to ~2px on the re-layout pass;
        //   2. whitespace-only text runs became flex items (css-flexbox-1
        //      §4 forbids this), adding four phantom gap slots;
        //   3. step 11b summed a nested ROW container's children heights —
        //      a 5-link nav measured 9 line-heights tall;
        //   4. line cross size ignored §9.4.8 rule 1, so align-items:center
        //      centered the logo within that phantom line instead of the
        //      definite 60px header (logo painted at y=96; Chrome: y=10.8).
        fn text_box(text: &str, font_px: f32, weight: u16) -> LayoutBox {
            let mut s = ComputedStyle::new();
            s.font_size = Length::Px(font_px);
            s.font_weight = rustkit_css::FontWeight(weight);
            LayoutBox::new(BoxType::Text(text.to_string()), s)
        }

        let mut nav_style = ComputedStyle::new();
        nav_style.display = rustkit_css::Display::Flex;
        nav_style.flex_direction = FlexDirection::Row;
        nav_style.column_gap = Length::Px(30.0);
        let mut nav = LayoutBox::new(BoxType::Block, nav_style);
        for (i, label) in ["Home", "Features", "Pricing", "About", "Contact"]
            .iter()
            .enumerate()
        {
            if i > 0 {
                // Inter-element whitespace from the HTML source.
                nav.children.push(text_box(" ", 16.0, 400));
            }
            let mut a_style = ComputedStyle::new();
            a_style.font_size = Length::Px(16.0);
            let mut a = LayoutBox::new(BoxType::Inline, a_style);
            a.children.push(text_box(label, 16.0, 500));
            nav.children.push(a);
        }

        let mut logo_style = ComputedStyle::new();
        logo_style.font_size = Length::Px(24.0);
        let mut logo = LayoutBox::new(BoxType::Block, logo_style);
        logo.children.push(text_box("HiWave", 24.0, 700));

        let mut header_style = ComputedStyle::new();
        header_style.display = rustkit_css::Display::Flex;
        header_style.flex_direction = FlexDirection::Row;
        header_style.justify_content = JustifyContent::SpaceBetween;
        header_style.align_items = AlignItems::Center;
        header_style.height = Length::Px(60.0);
        let mut header = LayoutBox::new(BoxType::Block, header_style);
        header.children.push(logo);
        header.children.push(nav);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1160.0, 60.0),
            ..Default::default()
        };
        layout_flex_container(&mut header, &containing);

        let logo = &header.children[0];
        let nav = &header.children[1];

        // Bugs 1+2: nav takes its max-content width — 5 measured link
        // widths (~250px with this font stack) plus exactly 4×30px gaps.
        let nav_w = nav.dimensions.content.width;
        assert!(
            (330.0..440.0).contains(&nav_w),
            "nav should be ~5 links + 4 gaps wide, got {nav_w}"
        );
        // space-between: nav flush to the container's right edge.
        let nav_right = nav.dimensions.content.x + nav_w;
        assert!(
            (nav_right - 1160.0).abs() < 1.0,
            "nav right edge should hit the container edge, got {nav_right}"
        );
        // Links keep their measured text widths (were ~2px when smashed),
        // and consecutive links sit exactly one 30px gap apart (whitespace
        // runs must not consume extra gap slots).
        let links: Vec<&LayoutBox> = nav
            .children
            .iter()
            .filter(|c| matches!(c.box_type, BoxType::Inline))
            .collect();
        assert_eq!(links.len(), 5);
        for a in &links {
            let w = a.dimensions.content.width;
            assert!(w > 30.0, "nav link should keep its text width, got {w}");
        }
        for pair in links.windows(2) {
            let gap = pair[1].dimensions.content.x
                - (pair[0].dimensions.content.x + pair[0].dimensions.content.width);
            assert!(
                (gap - 30.0).abs() < 1.0,
                "links should sit one 30px gap apart, got {gap}"
            );
        }

        // Bug 3: nav is one text line tall, not nine.
        let nav_h = nav.dimensions.content.height;
        assert!(
            nav_h < 35.0,
            "nav should be a single line tall, got {nav_h}"
        );

        // Bug 4: both items center within the DEFINITE 60px header.
        let logo_h = logo.dimensions.margin_box().height;
        let logo_y = logo.dimensions.content.y;
        let expected_logo_y = (60.0 - logo_h) / 2.0;
        assert!(
            (logo_y - expected_logo_y).abs() < 1.0,
            "logo should center in the 60px header (expected y≈{expected_logo_y}), got {logo_y}"
        );
        let nav_y = nav.dimensions.content.y;
        let expected_nav_y = (60.0 - nav_h) / 2.0;
        assert!(
            (nav_y - expected_nav_y).abs() < 1.0,
            "nav should center in the 60px header (expected y≈{expected_nav_y}), got {nav_y}"
        );
    }

    #[test]
    fn test_axis_cross() {
        assert_eq!(Axis::Horizontal.cross(), Axis::Vertical);
        assert_eq!(Axis::Vertical.cross(), Axis::Horizontal);
    }

    #[test]
    fn test_flex_direction_properties() {
        assert!(FlexDirection::Row.is_row());
        assert!(FlexDirection::RowReverse.is_row());
        assert!(!FlexDirection::Column.is_row());
        assert!(FlexDirection::RowReverse.is_reverse());
        assert!(!FlexDirection::Row.is_reverse());
    }

    #[test]
    fn test_flex_line_creation() {
        let line = FlexLine::new();
        assert!(line.items.is_empty());
        assert_eq!(line.cross_size, 0.0);
    }

    #[test]
    fn test_auto_width_item_sized_by_block_child_content() {
        // Regression: a row-flex item with width:auto and flex-basis:auto
        // must take its content's width (max-content contribution), not the
        // line-height. Was: get_intrinsic_main_size returned line_height for
        // Block boxes, so a wrapper <div> around a 150px box measured ~24px
        // and every wrapper on the line overlapped its neighbors.
        // (2026-07-09, macOS trench session 8, gpu-gradient-regression)
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        let mut container = LayoutBox::new(BoxType::Block, style);

        // Two wrapper items, each holding an explicit 150px-wide child.
        for _ in 0..2 {
            let mut wrapper = LayoutBox::new(BoxType::Block, ComputedStyle::new());
            let mut inner_style = ComputedStyle::new();
            inner_style.width = Length::Px(150.0);
            inner_style.height = Length::Px(100.0);
            wrapper
                .children
                .push(LayoutBox::new(BoxType::Block, inner_style));
            container.children.push(wrapper);
        }

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 760.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w0 = container.children[0].dimensions.content.width;
        let x0 = container.children[0].dimensions.content.x;
        let x1 = container.children[1].dimensions.content.x;
        assert!(
            (w0 - 150.0).abs() < 0.5,
            "wrapper item should size to its 150px child, got {w0}"
        );
        assert!(
            x1 >= x0 + 150.0,
            "second item must not overlap the first: x0={x0} x1={x1}"
        );
    }

    /// Build a flex item box with the given width/padding/box-sizing, with
    /// padding pre-resolved onto dimensions the way the block pre-pass does.
    fn padded_item(width: f32, padding: f32, border_box: bool) -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.width = Length::Px(width);
        style.padding_left = Length::Px(padding);
        style.padding_right = Length::Px(padding);
        style.padding_top = Length::Px(padding);
        style.padding_bottom = Length::Px(padding);
        style.box_sizing = if border_box {
            rustkit_css::BoxSizing::BorderBox
        } else {
            rustkit_css::BoxSizing::ContentBox
        };
        let mut b = LayoutBox::new(BoxType::Block, style);
        b.dimensions.padding = EdgeSizes {
            left: padding,
            right: padding,
            top: padding,
            bottom: padding,
        };
        // Give the item some content height, as the block pre-pass would.
        let mut inner = ComputedStyle::new();
        inner.width = Length::Px(60.0);
        inner.height = Length::Px(60.0);
        b.children.push(LayoutBox::new(BoxType::Block, inner));
        b
    }

    #[test]
    fn wrapped_items_stretch_to_their_line_after_reflow() {
        // card-grid: a wrapping row of auto-height cards; Chrome stretches
        // every card to its line, step 11 used to leave them at content height.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_wrap = rustkit_css::FlexWrap::Wrap;
        style.column_gap = Length::Px(24.0);
        style.row_gap = Length::Px(24.0);
        let mut container = LayoutBox::new(BoxType::Block, style);
        for h in [100.0, 150.0, 100.0, 120.0] {
            let mut item_style = ComputedStyle::new();
            item_style.width = Length::Px(300.0);
            let mut item = LayoutBox::new(BoxType::Block, item_style);
            let mut inner = ComputedStyle::new();
            inner.height = Length::Px(h);
            item.children.push(LayoutBox::new(BoxType::Block, inner));
            container.children.push(item);
        }
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children.push(container);
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 1000.0, 0.0);
        root.layout_with_collapse(
            &cb,
            &mut crate::MarginCollapseContext::new(),
            &mut crate::FloatContext::new(),
        );
        let hs: Vec<(f32, f32)> = root.children[0]
            .children
            .iter()
            .map(|c| (c.dimensions.content.y, c.dimensions.content.height))
            .collect();
        assert_eq!(hs, vec![(0.0, 150.0), (0.0, 150.0), (0.0, 150.0), (174.0, 120.0)]);
    }

    #[test]
    fn test_border_box_items_wrap_and_center_like_chrome() {
        // Regression (card-grid, macOS trench session 9): flex math treated
        // item sizes as content-box and never added padding/border, so a
        // border-box 300px card with 24px padding painted 348px wide while
        // neighbors were placed 300px apart — 48px of overlap per item on
        // both axes. Border-box items must occupy exactly their specified
        // width, and gaps must separate them.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        style.flex_wrap = rustkit_css::FlexWrap::Wrap;
        style.justify_content = JustifyContent::Center;
        style.row_gap = Length::Px(24.0);
        style.column_gap = Length::Px(24.0);
        let mut container = LayoutBox::new(BoxType::Block, style);
        for _ in 0..4 {
            container.children.push(padded_item(300.0, 24.0, true));
        }

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1200.0, 800.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        // Row 1: 3 cards of 300 + 2 gaps of 24 = 948, centered in 1200
        // -> border boxes start at 126, 450, 774 (Chrome's numbers).
        // content rect = border box + 24 padding inset.
        let xs: Vec<f32> = container
            .children
            .iter()
            .map(|c| c.dimensions.content.x)
            .collect();
        let ws: Vec<f32> = container
            .children
            .iter()
            .map(|c| c.dimensions.content.width)
            .collect();
        assert!(
            (xs[0] - 150.0).abs() < 0.5,
            "card 1 content x: got {}",
            xs[0]
        );
        assert!(
            (xs[1] - 474.0).abs() < 0.5,
            "card 2 content x: got {}",
            xs[1]
        );
        assert!(
            (xs[2] - 798.0).abs() < 0.5,
            "card 3 content x: got {}",
            xs[2]
        );
        assert!(
            (ws[0] - 252.0).abs() < 0.5,
            "border-box 300 - 48 pb = 252 content, got {}",
            ws[0]
        );

        // Row 2: the 4th card wraps, centered alone: border box at 450.
        assert!(
            (xs[3] - 474.0).abs() < 0.5,
            "wrapped card content x: got {}",
            xs[3]
        );
        let y3 = container.children[3].dimensions.content.y;
        let y0 = container.children[0].dimensions.content.y;
        // Row 2 starts below row 1's border-box height (60 content + 48 pb) plus the 24px row gap.
        assert!(
            (y3 - (y0 + 108.0 + 24.0)).abs() < 1.0,
            "row 2 must sit one border-box row + gap below row 1: y0={y0} y3={y3}"
        );
    }

    #[test]
    fn test_content_box_padded_items_do_not_overlap() {
        // Without border-box, a 300px-wide item with 24px padding occupies
        // 348px; the next item must start at least 348 + gap further over.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        style.column_gap = Length::Px(24.0);
        let mut container = LayoutBox::new(BoxType::Block, style);
        for _ in 0..2 {
            container.children.push(padded_item(300.0, 24.0, false));
        }

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1200.0, 800.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let c0 = &container.children[0].dimensions;
        let c1 = &container.children[1].dimensions;
        assert!(
            (c0.content.width - 300.0).abs() < 0.5,
            "content-box keeps 300 content width"
        );
        let border_box_end = c0.content.x - 24.0 + 348.0;
        let next_start = c1.content.x - 24.0;
        assert!(
            (next_start - (border_box_end + 24.0)).abs() < 0.5,
            "second item must start one gap after the first border box: end={border_box_end} next={next_start}"
        );
    }

    #[test]
    fn test_basic_flex_layout() {
        // Create a flex container with two children
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;

        let mut container = LayoutBox::new(BoxType::Block, style);

        // Add two children
        let mut child1_style = ComputedStyle::new();
        child1_style.width = Length::Px(100.0);
        child1_style.height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child1_style));

        let mut child2_style = ComputedStyle::new();
        child2_style.width = Length::Px(100.0);
        child2_style.height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child2_style));

        // Create containing block
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 300.0),
            ..Default::default()
        };

        // Layout
        layout_flex_container(&mut container, &containing);

        // Verify children have positions
        assert_eq!(container.children.len(), 2);
    }

    #[test]
    fn test_flex_grow() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;

        let mut container = LayoutBox::new(BoxType::Block, style);

        // Two children with flex-grow: 1
        let mut child1_style = ComputedStyle::new();
        child1_style.flex_grow = 1.0;
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child1_style));

        let mut child2_style = ComputedStyle::new();
        child2_style.flex_grow = 1.0;
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child2_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 100.0),
            ..Default::default()
        };

        layout_flex_container(&mut container, &containing);

        // Both children should share space equally
        let child1_width = container.children[0].dimensions.content.width;
        let child2_width = container.children[1].dimensions.content.width;
        assert!((child1_width - child2_width).abs() < 1.0);
    }

    #[test]
    fn test_justify_content_center() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.justify_content = JustifyContent::Center;

        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(100.0);
        child_style.flex_basis = rustkit_css::FlexBasis::Length(100.0);
        child_style.min_width = Length::Px(100.0); // Prevent shrinking
        child_style.flex_shrink = 0.0; // Don't shrink
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 100.0),
            ..Default::default()
        };

        layout_flex_container(&mut container, &containing);

        // Child should be centered - (400 - 100) / 2 = 150
        let child_x = container.children[0].dimensions.content.x;
        let child_w = container.children[0].dimensions.content.width;
        let expected_x = (400.0 - child_w) / 2.0;
        assert!(
            (child_x - expected_x).abs() < 1.0,
            "Expected child_x around {}, got {} (child_w={})",
            expected_x,
            child_x,
            child_w
        );
    }

    #[test]
    fn test_align_items_center() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.align_items = AlignItems::Center;

        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(100.0);
        child_style.height = Length::Px(50.0);
        child_style.min_height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 200.0),
            ..Default::default()
        };

        layout_flex_container(&mut container, &containing);

        // Child should be vertically centered (cross axis)
        let child_y = container.children[0].dimensions.content.y;
        // Note: actual centering depends on line cross_size calculation
        assert!(child_y >= 0.0);
    }

    /// Column flex items must STRETCH to the container width by default.
    ///
    /// `align-items` defaults to `stretch`, and in a column container the
    /// cross axis is horizontal — so children fill the width and keep their
    /// content heights.
    ///
    /// T-RED: before the axis-aware split of get_content_cross_size, this
    /// failed with widths tracking the items' own heights, producing square
    /// boxes. The existing test_column_direction could not catch it because
    /// it only asserts vertical ORDER, never a width.
    ///
    /// Root-caused by Prometheus from the repro in defect-report 8e21b9e12ffc.
    #[test]
    fn test_column_stretch_fills_cross_axis_width() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        // width stays Auto on purpose: a block-level flex container with
        // auto width still has a definite USED width from its containing
        // block, and treating that as indefinite is half of the bug.
        let mut container = LayoutBox::new(BoxType::Block, style);

        for h in [16.0f32, 32.0f32] {
            let mut cs = ComputedStyle::new();
            cs.height = Length::Px(h);
            cs.flex_basis = rustkit_css::FlexBasis::Length(h);
            container.children.push(LayoutBox::new(BoxType::Block, cs));
        }

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1000.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        for (i, child) in container.children.iter().enumerate() {
            let w = child.dimensions.content.width;
            assert!(
                (w - 1000.0).abs() < 0.5,
                "column child {i} width {w}, expected 1000 (align-items:stretch on the \
                 cross axis). A width equal to the child's own height means the cross \
                 size is being measured on the main axis."
            );
        }

        // Heights must survive: stretching the cross axis must not disturb
        // main-axis sizing, or "fixed" would just mean "square".
        assert!((container.children[0].dimensions.content.height - 16.0).abs() < 0.5);
        assert!((container.children[1].dimensions.content.height - 32.0).abs() < 0.5);
    }

    /// With stretch OFF, a column item's cross size is its content WIDTH.
    ///
    /// This is the companion to test_column_stretch_fills_cross_axis_width,
    /// and it exists because that test turned out NOT to be a T-RED for the
    /// axis fix: when stretch applies it overwrites the content-cross value,
    /// so a height-measured cross size is invisible. I found that by reverting
    /// the axis split and watching the stretch test stay green.
    ///
    /// `align-items: flex-start` is what makes the content measurement
    /// load-bearing, so this is the test that actually fails if
    /// get_content_cross_size goes back to measuring height on both axes.
    #[test]
    fn test_column_non_stretch_item_uses_content_width_not_height() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::FlexStart; // no stretch to mask it
        let mut container = LayoutBox::new(BoxType::Block, style);

        // A tall, narrow child whose width is AUTO. The auto width is what
        // makes this bite: an explicit cross size short-circuits the content
        // measurement entirely, so a test that sets `width` cannot reach
        // get_content_cross_size at all. (I wrote it that way first and the
        // T-RED stayed green — the code under test was simply unreachable.)
        //
        // Pre-seeding the laid-out rect at 200x400 gives the measurement two
        // clearly different numbers to pick from, so a wrong-axis read is
        // unambiguous rather than a near miss.
        let mut cs = ComputedStyle::new();
        cs.height = Length::Px(400.0);
        cs.flex_basis = rustkit_css::FlexBasis::Length(400.0);
        let mut child = LayoutBox::new(BoxType::Block, cs);
        child.dimensions.content = Rect::new(0.0, 0.0, 200.0, 400.0);
        container.children.push(child);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1000.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 200.0).abs() < 0.5,
            "non-stretch column child width {w}, expected 200. A value near 400 means \
             the cross size was measured as a HEIGHT."
        );
    }

    /// A column flex container whose items do NOT stretch sizes each item to
    /// its FIT-CONTENT width, not to the container.
    ///
    /// `.setting-row[style="flex-direction: column; align-items: flex-start"]`
    /// on the settings page: Chrome gives `.setting-label` 205.25px and
    /// RustKit gave it the row's whole 660px, because the hypothetical cross
    /// size came from `get_content_cross_width`, whose first answer is the
    /// width a previous block pass left on the box — for a `width: auto`
    /// child, the containing block.
    ///
    /// The child here carries real content (a 240px block) so the intrinsic
    /// estimators have something to measure; the fixture in the test above
    /// deliberately has none, which is why it exercises the fallback rather
    /// than this path.
    #[test]
    fn a_non_stretch_column_item_takes_its_fit_content_width() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::FlexStart;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item_style = ComputedStyle::new();
        let mut item = LayoutBox::new(BoxType::Block, item_style.clone());
        let mut inner = ComputedStyle::new();
        inner.width = Length::Px(240.0);
        inner.height = Length::Px(20.0);
        item.children.push(LayoutBox::new(BoxType::Block, inner));
        // A stale fill-available width, exactly as the block pre-pass leaves
        // one. Reading this is the defect; it must not win over the estimate.
        item.dimensions.content = Rect::new(0.0, 0.0, 660.0, 20.0);
        container.children.push(item);

        // A second item, narrower, so a wrong answer cannot come from "every
        // item got the same number" for some unrelated reason.
        item_style.height = Length::Px(20.0);
        let mut item2 = LayoutBox::new(BoxType::Block, item_style);
        let mut inner2 = ComputedStyle::new();
        inner2.width = Length::Px(100.0);
        item2.children.push(LayoutBox::new(BoxType::Block, inner2));
        item2.dimensions.content = Rect::new(0.0, 0.0, 660.0, 20.0);
        container.children.push(item2);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 660.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w0 = container.children[0].dimensions.content.width;
        let w1 = container.children[1].dimensions.content.width;
        assert!(
            (w0 - 240.0).abs() < 0.5,
            "flex-start column item width {w0}, expected its fit-content 240. \
             660 means the stale fill-available width won."
        );
        assert!(
            (w1 - 100.0).abs() < 0.5,
            "second flex-start column item width {w1}, expected its fit-content 100."
        );
    }

    /// Fit-content is `min(max(min-content, available), max-content)`, so the
    /// available cross space is a real term and not decoration: two 200px
    /// inline-level children give max-content 400 and min-content 200, and in
    /// 300px of available space the answer is 300 — neither end.
    #[test]
    fn a_non_stretch_column_item_takes_the_available_cross_space_between_the_two() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::FlexStart;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        for _ in 0..2 {
            let mut inner = ComputedStyle::new();
            inner.display = rustkit_css::Display::InlineBlock;
            inner.width = Length::Px(200.0);
            inner.height = Length::Px(20.0);
            item.children.push(LayoutBox::new(BoxType::Block, inner));
        }
        item.dimensions.content = Rect::new(0.0, 0.0, 300.0, 20.0);
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 300.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 300.0).abs() < 0.5,
            "item width {w}: max-content 400 clamped by 300 of available cross \
             space is 300. 400 means available was dropped from the formula."
        );
    }

    /// STRETCHING items keep the old measurement, and that scope is not
    /// cosmetic: applying fit-content to them shrinks a stretching item to its
    /// content instead of to its container. Measured on the corpus — `shelf`'s
    /// `#commandResults` went 1248 -> 1216 and `flex-positioning`'s nested-flex
    /// child 710 -> 690, four failing axes added, when the scope was removed.
    ///
    /// The shape that bites is an INDEFINITE cross size: `width: auto` with a
    /// containing block that has no content width yet, which is what a nested
    /// container sees during the pre-pass. There `stretch_target` is the line's
    /// own cross size rather than the container's, so narrowing the
    /// hypothetical sizes narrows the very target the items stretch to — the
    /// item ends up at its content width having gone through the stretch path.
    /// A definite-width container cannot show this: its target is the
    /// container, which the hypothetical size does not feed.
    #[test]
    fn a_stretching_column_item_is_not_shrunk_to_its_content() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::Stretch;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut inner = ComputedStyle::new();
        inner.width = Length::Px(240.0);
        inner.height = Length::Px(20.0);
        item.children.push(LayoutBox::new(BoxType::Block, inner));
        // The width a previous pass measured, which is the number a stretching
        // item must keep.
        item.dimensions.content = Rect::new(0.0, 0.0, 1216.0, 20.0);
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 0.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 1216.0).abs() < 0.5,
            "stretching column item width {w}, expected the measured 1216. \
             240 means the fit-content path reached an item that stretches."
        );
    }

    /// The min-content floor is a term, not decoration: fit-content never
    /// crushes an item below its min-content width, however little cross space
    /// is available.
    #[test]
    fn a_non_stretch_column_item_is_not_crushed_below_its_min_content() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::FlexStart;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut inner = ComputedStyle::new();
        inner.width = Length::Px(900.0);
        inner.height = Length::Px(20.0);
        item.children.push(LayoutBox::new(BoxType::Block, inner));
        item.dimensions.content = Rect::new(0.0, 0.0, 300.0, 20.0);
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 300.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 900.0).abs() < 0.5,
            "item width {w}: min-content 900 in 300px of available space stays \
             900. 300 means the min-content floor was dropped and the item was \
             crushed to the space it had."
        );
    }

    /// `align-self` on the ITEM decides, not only `align-items` on the
    /// container: an item that opts out of a stretching container takes its
    /// fit-content width.
    #[test]
    fn align_self_flex_start_opts_an_item_out_of_a_stretching_container() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::Stretch;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item_style = ComputedStyle::new();
        item_style.align_self = rustkit_css::AlignSelf::FlexStart;
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        let mut inner = ComputedStyle::new();
        inner.width = Length::Px(240.0);
        inner.height = Length::Px(20.0);
        item.children.push(LayoutBox::new(BoxType::Block, inner));
        item.dimensions.content = Rect::new(0.0, 0.0, 660.0, 20.0);
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 660.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 240.0).abs() < 0.5,
            "align-self:flex-start item width {w}, expected its fit-content 240. \
             660 means align-self was read as the container's stretch."
        );
    }

    /// A control the estimators CAN see — one with a specified pixel width —
    /// must still be measured. The unmeasurable guard is about what the
    /// estimators cannot read, not about the box type.
    #[test]
    fn a_control_with_a_pixel_width_is_still_measured() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::FlexStart;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut control_style = ComputedStyle::new();
        control_style.width = Length::Px(200.0);
        item.children.push(LayoutBox::new(
            BoxType::FormControl(crate::FormControlType::TextInput {
                value: String::new(),
                placeholder: String::new(),
                input_type: "text".to_string(),
            }),
            control_style,
        ));
        item.dimensions.content = Rect::new(0.0, 0.0, 660.0, 20.0);
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 660.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 200.0).abs() < 0.5,
            "item width {w}: a control with width:200px is measurable, so the \
             item is 200. 660 means every control was treated as opaque."
        );
    }

    /// Where the intrinsic estimators cannot see the content — a form control
    /// or image with no specified pixel width — the previously measured width
    /// is kept rather than a zero estimate becoming a zero box.
    ///
    /// `new_tab`'s `.search-input` is `width: 100%`, so it contributes nothing
    /// to max-content; without this the whole `.container` above it collapsed
    /// from 536 to 360 and took seven failing axes with it.
    #[test]
    fn an_item_the_estimators_cannot_measure_keeps_its_measured_width() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::Center;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        // Measurable content, so the estimate is NOT zero — the zero-estimate
        // fallback must not be what saves this box, or the guard is untested.
        let mut sibling = ComputedStyle::new();
        sibling.width = Length::Px(240.0);
        sibling.height = Length::Px(20.0);
        item.children
            .push(LayoutBox::new(BoxType::Block, sibling));
        // …and one control the estimators cannot read, nested a level down so
        // the guard has to walk to find it.
        let mut wrapper = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut control_style = ComputedStyle::new();
        control_style.width = Length::Percent(100.0);
        wrapper.children.push(LayoutBox::new(
            BoxType::FormControl(crate::FormControlType::TextInput {
                value: String::new(),
                placeholder: String::new(),
                input_type: "text".to_string(),
            }),
            control_style,
        ));
        item.children.push(wrapper);
        item.dimensions.content = Rect::new(0.0, 0.0, 536.0, 40.0);
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 600.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w = container.children[0].dimensions.content.width;
        assert!(
            (w - 536.0).abs() < 0.5,
            "item width {w}: an unmeasurable subtree keeps its measured 536, it \
             does not collapse to an estimate that could not see the control."
        );
    }

    #[test]
    fn test_column_direction() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;

        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut child1_style = ComputedStyle::new();
        child1_style.height = Length::Px(50.0);
        child1_style.flex_basis = rustkit_css::FlexBasis::Length(50.0);
        child1_style.min_height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child1_style));

        let mut child2_style = ComputedStyle::new();
        child2_style.height = Length::Px(50.0);
        child2_style.flex_basis = rustkit_css::FlexBasis::Length(50.0);
        child2_style.min_height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child2_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 300.0),
            ..Default::default()
        };

        layout_flex_container(&mut container, &containing);

        // In column layout, items should stack vertically
        let child1_y = container.children[0].dimensions.content.y;
        let child2_y = container.children[1].dimensions.content.y;
        assert!(
            child2_y >= child1_y,
            "Expected child2_y ({}) >= child1_y ({})",
            child2_y,
            child1_y
        );
    }

    /// `is_definite_cross_size` counts any non-`auto` height as specified, so
    /// a `calc()` container is definite — and then it must get a used height
    /// to match. Left on the `_` arm the container is definite with no
    /// explicit height, and falls back to its content.
    #[test]
    fn a_calc_height_flex_container_uses_its_resolved_height() {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        style.height = rustkit_css::parse_length("calc(100% - 100px)").expect("calc parses");
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(100.0);
        child_style.height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 500.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        assert_eq!(
            container.dimensions.content.height, 400.0,
            "100% of the containing block's 500px minus 100px, not the 50px of content"
        );
    }

    #[test]
    fn test_auto_height_stretch() {
        // Test that flex items in an auto-height container stretch to the tallest item,
        // not the parent container's height
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        style.height = Length::Auto; // Auto height container

        let mut container = LayoutBox::new(BoxType::Block, style);

        // First child: explicit height of 50px
        let mut child1_style = ComputedStyle::new();
        child1_style.width = Length::Px(100.0);
        child1_style.height = Length::Px(50.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child1_style));

        // Second child: auto height (should stretch to match first child)
        let mut child2_style = ComputedStyle::new();
        child2_style.width = Length::Px(100.0);
        child2_style.height = Length::Auto;
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child2_style));

        // Large parent container - items should NOT stretch to this
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 500.0),
            ..Default::default()
        };

        layout_flex_container(&mut container, &containing);

        // Both children should be ~50px (the height of the tallest item)
        // NOT 500px (the parent container height)
        let child1_height = container.children[0].dimensions.content.height;
        let child2_height = container.children[1].dimensions.content.height;

        assert!(
            child1_height < 100.0,
            "Child1 height {} should be less than 100px",
            child1_height
        );
        assert!(
            child2_height < 100.0,
            "Child2 height {} should be less than 100px (stretched to match tallest, not parent)",
            child2_height
        );
    }

    #[test]
    fn test_explicitly_sized_button_in_a_flex_row_keeps_its_size() {
        // The shelf header, reduced. Chrome 148 puts #closeBtn at
        // 1240,8,24,24 inside a 1280x41 header; RustKit rendered it 24x36 and
        // the header came out 53 tall instead of 41.
        //
        // A <button> is a FormControl with an intrinsic cross size
        // (font_size*1.5+12 = 36 at 16px). That intrinsic was used as the
        // item's minimum on the cross axis even when the author specified a
        // height, so `height: 24px` could not go below it. Any author-sized
        // control in a flex row is affected; the shelf is only where it was
        // measured.
        let mut hdr_style = ComputedStyle::new();
        hdr_style.display = rustkit_css::Display::Flex;
        hdr_style.flex_direction = FlexDirection::Row;
        hdr_style.align_items = AlignItems::Center;
        hdr_style.justify_content = rustkit_css::JustifyContent::SpaceBetween;
        hdr_style.width = Length::Px(1280.0);
        hdr_style.box_sizing = rustkit_css::BoxSizing::BorderBox;

        let mut hdr = LayoutBox::new(BoxType::Block, hdr_style);
        hdr.dimensions.padding = EdgeSizes { top: 8.0, bottom: 8.0, left: 16.0, right: 16.0 };

        let mut title_style = ComputedStyle::new();
        title_style.width = Length::Px(105.0);
        title_style.height = Length::Px(15.0);
        hdr.children.push(LayoutBox::new(BoxType::Block, title_style));

        let mut close_style = ComputedStyle::new();
        close_style.width = Length::Px(24.0);
        close_style.height = Length::Px(24.0);
        close_style.display = rustkit_css::Display::Flex;
        close_style.font_size = Length::Px(16.0);
        hdr.children.push(LayoutBox::new(
            BoxType::FormControl(crate::FormControlType::Button {
                label: "\u{00d7}".to_string(),
                button_type: "button".to_string(),
            }),
            close_style,
        ));

        let containing = Dimensions { content: Rect::new(0.0, 0.0, 1280.0, 600.0), ..Default::default() };
        layout_flex_container(&mut hdr, &containing);

        let close = &hdr.children[1].dimensions.content;
        assert!(
            (close.width - 24.0).abs() < 0.5 && (close.height - 24.0).abs() < 0.5,
            "explicitly sized button should stay 24x24, got {}x{}",
            close.width, close.height
        );
        assert!(close.x >= 1200.0, "space-between should push it to the end, got x={}", close.x);

        // The header height follows: 8 + max(15, 24) + 8 = 40 (Chrome 41 with
        // its 1px border). While the button measured 36, this was 52.
        let hdr_h = hdr.dimensions.content.height + hdr.dimensions.padding.vertical();
        assert!(
            (39.0..=42.0).contains(&hdr_h),
            "header height should be ~40-41 once the button is 24, got {}", hdr_h
        );
    }

    #[test]
    fn test_image_flex_item_cross_size_follows_its_ratio_not_natural_height() {
        // images-intrinsic test12: a flex row of `width: 80px` images with a
        // 100x100 natural size and a 1px border (box-sizing: border-box).
        // Chrome builds each image 80x80 — the specified width applies and the
        // height follows the 1:1 ratio. RustKit built 80x102: the width
        // applied but the height stayed the natural 100 (+2 border).
        //
        // The block pre-pass (layout_block_children_with_collapse, which runs
        // before flex in the real dispatch) already resolves the image to
        // 78x78 content / 80x80 border-box via layout_image's ratio handling.
        // The defect was purely in the flex CROSS MINIMUM: it read the raw
        // natural_height (100) rather than that laid-out 78, and floored the
        // correct 80 back up to 102. This test reproduces the pre-pass by
        // seeding the child's dimensions, then asserts the flex pass does not
        // re-inflate the height.
        let mut row_style = ComputedStyle::new();
        row_style.display = rustkit_css::Display::Flex;
        row_style.flex_direction = FlexDirection::Row;
        // flex-start, not the default stretch, so this isolates the cross
        // MINIMUM floor: with stretch a single item's stretch target equals
        // its own content size and would mask which term is wrong.
        row_style.align_items = AlignItems::FlexStart;
        row_style.box_sizing = rustkit_css::BoxSizing::BorderBox;

        let mut row = LayoutBox::new(BoxType::Block, row_style);
        row.dimensions.content = Rect::new(0.0, 0.0, 400.0, 0.0);

        let mut img_style = ComputedStyle::new();
        img_style.width = Length::Px(80.0);
        img_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        let mut img = LayoutBox::new(
            BoxType::Image {
                url: String::new(),
                natural_width: 100.0,
                natural_height: 100.0,
            },
            img_style,
        );
        // What the block pre-pass leaves behind: border-box 80x80, i.e. 78x78
        // content inside a 1px border on every side.
        img.dimensions.border = EdgeSizes { top: 1.0, bottom: 1.0, left: 1.0, right: 1.0 };
        img.dimensions.content = Rect::new(0.0, 0.0, 78.0, 78.0);
        row.children.push(img);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut row, &containing);

        let bb = row.children[0].dimensions.border_box();
        assert!(
            (bb.height - 80.0).abs() < 0.5,
            "a width-constrained image should keep its 1:1 ratio height (border-box 80), \
             got {} — the natural_height floored the cross minimum",
            bb.height
        );
        assert!(
            (bb.width - 80.0).abs() < 0.5,
            "the specified width (border-box 80) must be unchanged, got {}",
            bb.width
        );
    }

    #[test]
    fn test_auto_width_column_stretches_to_inner_width_not_containing_block() {
        // A `width: auto` column flex container takes its used width from the
        // containing block, but its children stretch to its INNER width --
        // containing block minus the container's own margin, border, padding.
        //
        // Reaching for the containing block's content width directly makes
        // every child overflow by exactly the container's own edges. That is
        // the #81 defect inverted (items grew by their padding instead of
        // shrinking by it), and it is what regressed the `shelf` parity case
        // from 3.71% to 33.87%: a full-width bar whose child ran past it.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.width = Length::Auto;
        style.align_items = AlignItems::Stretch;

        let mut container = LayoutBox::new(BoxType::Block, style);
        container.dimensions.padding = EdgeSizes {
            left: 20.0,
            right: 20.0,
            ..Default::default()
        };
        container.dimensions.border = EdgeSizes {
            left: 5.0,
            right: 5.0,
            ..Default::default()
        };

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Auto;
        child_style.height = Length::Px(40.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1280.0, 600.0),
            ..Default::default()
        };

        layout_flex_container(&mut container, &containing);

        // 1280 - (20+20 padding) - (5+5 border) = 1230.
        let child_width = container.children[0].dimensions.content.width;
        assert!(
            (child_width - 1230.0).abs() < 0.5,
            "stretched child width {} should be the container's inner width 1230, \
             not the containing block's 1280 (overflowing by the container's own edges)",
            child_width
        );
    }

    /// A block box with resolved padding (the way the block pre-pass leaves
    /// it) and no explicit size, wrapping one text run.
    fn padded_text_item(text: &str, font_px: f32, pad_x: f32, pad_y: f32) -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.font_size = Length::Px(font_px);
        s.padding_left = Length::Px(pad_x);
        s.padding_right = Length::Px(pad_x);
        s.padding_top = Length::Px(pad_y);
        s.padding_bottom = Length::Px(pad_y);
        s.box_sizing = rustkit_css::BoxSizing::BorderBox;
        let mut b = LayoutBox::new(BoxType::Block, s);
        b.dimensions.padding = EdgeSizes {
            left: pad_x,
            right: pad_x,
            top: pad_y,
            bottom: pad_y,
        };
        let mut ts = ComputedStyle::new();
        ts.font_size = Length::Px(font_px);
        b.children
            .push(LayoutBox::new(BoxType::Text(text.to_string()), ts));
        b
    }

    #[test]
    fn test_padded_auto_width_row_item_counts_padding_once() {
        // Regression (flex-positioning, n42): an auto-width flex item's
        // basis was estimate_max_content_width (a BORDER-box figure) plus
        // the item's padding again. `.flex-item { padding: 10px 20px }`
        // measured 123.2 wide against Chrome's 83.1 — +2×20 — and every
        // padded pill, tab and button in a flex row was too wide by its
        // horizontal padding.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        style.column_gap = Length::Px(10.0);
        let mut container = LayoutBox::new(BoxType::Block, style);
        container
            .children
            .push(padded_text_item("Item 1", 16.0, 20.0, 10.0));
        container
            .children
            .push(padded_text_item("Item 2", 16.0, 20.0, 10.0));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let text_style = &container.children[0].children[0].style;
        let text_w = crate::measure_text_advanced(
            "Item 1",
            &text_style.font_family,
            16.0,
            text_style.font_weight,
            text_style.font_style,
        )
        .width;
        let c0 = &container.children[0].dimensions;
        assert!(
            (c0.content.width - text_w).abs() < 0.5,
            "content width must be the text's max-content measure {text_w}, got {}",
            c0.content.width
        );
        let border_box_w = c0.content.width + 40.0;
        let c1 = &container.children[1].dimensions;
        assert!(
            ((c1.content.x - 20.0) - (border_box_w + 10.0)).abs() < 0.5,
            "second item must start one gap after the first border box ({border_box_w}), got x={}",
            c1.content.x - 20.0
        );
    }

    #[test]
    fn test_column_container_stacks_rows_at_laid_out_heights() {
        // Regression (flex-positioning section 4, n42): a column container
        // sized each auto-height row at one text line (18 + 16 padding = 34)
        // and placed the next row on that guess; the row then laid out at
        // its real 47 and overlapped the one below (row 2 at y=534, row 1
        // ending at 537; Chrome 548). The cross axis had 11b/11c to fix its
        // guesses — the main axis needs the same pass.
        fn padded_flex_row(child_h: f32) -> LayoutBox {
            let mut s = ComputedStyle::new();
            s.display = rustkit_css::Display::Flex;
            s.flex_direction = FlexDirection::Row;
            s.padding_top = Length::Px(8.0);
            s.padding_bottom = Length::Px(8.0);
            s.box_sizing = rustkit_css::BoxSizing::BorderBox;
            let mut row = LayoutBox::new(BoxType::Block, s);
            row.dimensions.padding = EdgeSizes {
                top: 8.0,
                bottom: 8.0,
                ..Default::default()
            };
            let mut cs = ComputedStyle::new();
            cs.height = Length::Px(child_h);
            cs.width = Length::Px(100.0);
            row.children.push(LayoutBox::new(BoxType::Block, cs));
            row
        }

        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.row_gap = Length::Px(10.0);
        let mut container = LayoutBox::new(BoxType::Block, style);
        container.children.push(padded_flex_row(31.0));
        container.children.push(padded_flex_row(31.0));

        let containing = Dimensions {
            content: Rect::new(0.0, 100.0, 710.0, 0.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let r0 = &container.children[0].dimensions;
        let r1 = &container.children[1].dimensions;
        let r0_border_h = r0.content.height + 16.0;
        assert!(
            (r0_border_h - 47.0).abs() < 0.5,
            "row 1 border-box height must be 31 + 16 = 47, got {r0_border_h}"
        );
        let r0_top = r0.content.y - 8.0;
        let r1_top = r1.content.y - 8.0;
        assert!(
            (r1_top - (r0_top + 47.0 + 10.0)).abs() < 0.5,
            "row 2 must start one gap below row 1's real border box: r0_top={r0_top} r1_top={r1_top}"
        );
        assert!(
            (container.dimensions.content.height - (47.0 + 10.0 + 47.0)).abs() < 0.5,
            "column container height must be the rows plus the gap, got {}",
            container.dimensions.content.height
        );
    }

    #[test]
    fn test_definite_height_column_grows_rows_from_laid_out_heights() {
        // Repro section E (n42): a 160px border-box column with 6px padding
        // and gap, two auto-height rows of real height 47 with flex-grow: 1.
        // Inner main = 148; content 47+47+6 = 100; free 48 -> 71 each
        // (Chrome). The grow must run against the container's OWN definite
        // inner height, not the containing block's number.
        fn padded_flex_row(child_h: f32) -> LayoutBox {
            let mut s = ComputedStyle::new();
            s.display = rustkit_css::Display::Flex;
            s.flex_direction = FlexDirection::Row;
            s.padding_top = Length::Px(8.0);
            s.padding_bottom = Length::Px(8.0);
            s.box_sizing = rustkit_css::BoxSizing::BorderBox;
            s.flex_grow = 1.0;
            let mut row = LayoutBox::new(BoxType::Block, s);
            row.dimensions.padding = EdgeSizes {
                top: 8.0,
                bottom: 8.0,
                ..Default::default()
            };
            let mut cs = ComputedStyle::new();
            cs.height = Length::Px(child_h);
            cs.width = Length::Px(100.0);
            row.children.push(LayoutBox::new(BoxType::Block, cs));
            row
        }

        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.row_gap = Length::Px(6.0);
        style.height = Length::Px(160.0);
        style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        let mut container = LayoutBox::new(BoxType::Block, style);
        container.dimensions.padding = EdgeSizes {
            top: 6.0,
            bottom: 6.0,
            left: 6.0,
            right: 6.0,
        };
        container.children.push(padded_flex_row(31.0));
        container.children.push(padded_flex_row(31.0));

        let containing = Dimensions {
            content: Rect::new(6.0, 6.0, 748.0, 94.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let r0 = &container.children[0].dimensions;
        let r1 = &container.children[1].dimensions;
        let h0 = r0.content.height + 16.0;
        let h1 = r1.content.height + 16.0;
        assert!(
            (h0 - 71.0).abs() < 0.5 && (h1 - 71.0).abs() < 0.5,
            "rows must grow to 71 each (148 inner, 100 content, 48 free): got {h0} / {h1}"
        );
        let r1_top = r1.content.y - 8.0;
        let r0_top = r0.content.y - 8.0;
        assert!(
            (r1_top - (r0_top + 71.0 + 6.0)).abs() < 0.5,
            "row 2 sits one gap below the grown row 1: r0_top={r0_top} r1_top={r1_top}"
        );
    }

    #[test]
    fn test_min_height_column_centres_content_sized_item_in_the_viewport() {
        // new_tab's body: `min-height: 100vh; display: flex; flex-direction:
        // column; align-items: center; justify-content: center`, one padded
        // content-sized `.container`. The engine passes the body its own
        // pre-pass stacked height as the containing block (713.4 on the
        // board), which is neither the content height nor the viewport.
        // The item must centre in max(content, min-height) = the viewport:
        // children 300 + padding 64 = 364 in 800 -> top at 218. On develop
        // an 82px line guess centred in 713.4 put it at 315.7 (Chrome 33.5
        // on the real page); the first cut of this pass put it at 0.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.align_items = AlignItems::Center;
        style.justify_content = JustifyContent::Center;
        style.min_height = Length::Vh(100.0);
        let mut body = LayoutBox::new(BoxType::Block, style);
        body.set_viewport(1280.0, 800.0);

        let mut cs = ComputedStyle::new();
        cs.padding_top = Length::Px(32.0);
        cs.padding_bottom = Length::Px(32.0);
        cs.padding_left = Length::Px(32.0);
        cs.padding_right = Length::Px(32.0);
        cs.max_width = Length::Px(600.0);
        cs.box_sizing = rustkit_css::BoxSizing::BorderBox;
        let mut item = LayoutBox::new(BoxType::Block, cs);
        item.dimensions.padding = EdgeSizes {
            top: 32.0,
            bottom: 32.0,
            left: 32.0,
            right: 32.0,
        };
        let mut child_style = ComputedStyle::new();
        child_style.height = Length::Px(300.0);
        child_style.width = Length::Px(400.0);
        item.children
            .push(LayoutBox::new(BoxType::Block, child_style));
        body.children.push(item);

        // The stale containing block the engine really passes.
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1280.0, 713.4),
            ..Default::default()
        };
        layout_flex_container(&mut body, &containing);

        let d = &body.children[0].dimensions;
        let top = d.content.y - 32.0;
        let border_h = d.content.height + 64.0;
        assert!(
            (border_h - 364.0).abs() < 0.5,
            "item border-box height must be 300 + 64 = 364, got {border_h}"
        );
        assert!(
            (top - 218.0).abs() < 0.5,
            "item must centre in the 800 viewport: expected top 218, got {top}"
        );
        // 11b must not turn the stacked child HEIGHT into the item's width.
        let border_w = d.content.width + 64.0;
        assert!(
            (border_w - 464.0).abs() < 0.5,
            "column item width is its widest child + padding (400 + 64), got {border_w}"
        );
    }

    /// A block flex item's cross size is the extent step 11 FLOWED, with the
    /// sibling seams collapsed — not the sum of its children's margin boxes.
    ///
    /// new_tab (2026-09-17): `.container` is a column flex item holding a
    /// search box (`margin-bottom: 2rem`) followed by a section
    /// (`margin-top: 3rem`). The collapse pass placed the section 48px below
    /// the box, then 11b re-derived the item's height as 52 + 80 + ... and the
    /// 32 the seam had absorbed landed as empty space under the last child:
    /// 746 tall for a flowed 714 (Chrome 733 with a 52px input, ours 51).
    #[test]
    fn a_block_item_measures_its_collapsed_seams_not_the_margin_sum() {
        let mut a_style = ComputedStyle::new();
        a_style.height = Length::Px(20.0);
        a_style.margin_bottom = Length::Px(32.0);
        let a = LayoutBox::new(BoxType::Block, a_style);

        let mut b_style = ComputedStyle::new();
        b_style.height = Length::Px(20.0);
        b_style.margin_top = Length::Px(48.0);
        let b = LayoutBox::new(BoxType::Block, b_style);

        let mut item_style = ComputedStyle::new();
        item_style.padding_top = Length::Px(16.0);
        item_style.padding_bottom = Length::Px(16.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        item.children.push(a);
        item.children.push(b);

        let mut container_style = ComputedStyle::new();
        container_style.display = rustkit_css::Display::Flex;
        container_style.flex_direction = FlexDirection::Column;
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        container.children.push(item);

        // The engine's root path: layout_with_collapse, so the block
        // pre-pass collapses the seam before flex runs (as on the page).
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 600.0, 0.0),
            ..Default::default()
        };
        let mut margins = crate::MarginCollapseContext::new();
        let mut floats = crate::FloatContext::new();
        container.layout_with_collapse(&cb, &mut margins, &mut floats);

        let item = &container.children[0];
        let b_y = item.children[1].dimensions.border_box().y;
        assert!(
            (b_y - (16.0 + 20.0 + 48.0)).abs() < 0.01,
            "the seam collapses to max(32, 48): B at y = 16 + 20 + 48 = 84, got {b_y}"
        );
        let item_h = item.dimensions.border_box().height;
        assert!(
            (item_h - (16.0 + 20.0 + 48.0 + 20.0 + 16.0)).abs() < 0.01,
            "item = padding + A + collapsed seam + B + padding = 120, got {item_h} \
             (152 is the un-collapsed margin-box sum: the 32 lands under B)"
        );
        let container_h = container.dimensions.content.height;
        assert!(
            (container_h - 120.0).abs() < 0.01,
            "the column's cross-derived height follows the item: 120, got {container_h}"
        );
    }

    /// A flex item is a formatting-context root: its last in-flow child's
    /// bottom margin stays INSIDE the item instead of collapsing through
    /// (CSS 2.1 §8.3.1). Pinned so measuring the flowed extent (above) does
    /// not drop the margin the old margin-box sum happened to include.
    #[test]
    fn a_block_item_keeps_its_last_childs_bottom_margin_inside() {
        let mut p_style = ComputedStyle::new();
        p_style.height = Length::Px(20.0);
        p_style.margin_bottom = Length::Px(16.0);
        let p = LayoutBox::new(BoxType::Block, p_style);

        // No padding/border/height: nothing on the item's own box blocks
        // the collapse, so only its BFC-root nature keeps the margin in.
        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        item.children.push(p);

        let mut container_style = ComputedStyle::new();
        container_style.display = rustkit_css::Display::Flex;
        container_style.flex_direction = FlexDirection::Row;
        container_style.align_items = AlignItems::FlexStart;
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        container.children.push(item);

        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, 600.0, 0.0),
            ..Default::default()
        };
        let mut margins = crate::MarginCollapseContext::new();
        let mut floats = crate::FloatContext::new();
        container.layout_with_collapse(&cb, &mut margins, &mut floats);

        let item_h = container.children[0].dimensions.border_box().height;
        assert!(
            (item_h - 36.0).abs() < 0.01,
            "item = child 20 + its bottom margin 16 kept inside = 36, got {item_h}"
        );
        let container_h = container.dimensions.content.height;
        assert!(
            (container_h - 36.0).abs() < 0.01,
            "row cross size follows the item: 36, got {container_h}"
        );
    }

    /// A flex item with a DEFINITE cross size keeps it; its children's flow
    /// never grows it (css-flexbox-1 §9.4 — content overflows instead).
    ///
    /// T-RED: step 11 lays a block flex item's children out with
    /// `layout_block_children_with_collapse`, which ends by assigning the flow
    /// cursor to the item's `content.height`. On this path that height is the
    /// size the flex algorithm already decided, and step 11b states the §9.4
    /// rule but `continue`s for exactly these items — so nothing repaired the
    /// clobber. `form-elements`' `.toggle-label > .toggle-switch`
    /// (`height: 26px`, two in-flow children) measured 32.08.
    fn toggle_switch_row() -> LayoutBox {
        fn border_box(s: &mut ComputedStyle) {
            // The corpus's `* { box-sizing: border-box }` is load-bearing:
            // the two sizing modes take different arithmetic through flex.
            s.box_sizing = rustkit_css::BoxSizing::BorderBox;
        }
        let mut switch_style = ComputedStyle::new();
        border_box(&mut switch_style);
        switch_style.position = rustkit_css::Position::Relative;
        switch_style.width = Length::Px(50.0);
        switch_style.height = Length::Px(26.0);
        let mut switch = LayoutBox::new(BoxType::Block, switch_style);

        let mut checkbox_style = ComputedStyle::new();
        border_box(&mut checkbox_style);
        checkbox_style.width = Length::Px(0.0);
        checkbox_style.height = Length::Px(0.0);
        switch.children.push(LayoutBox::new(
            BoxType::FormControl(crate::FormControlType::Checkbox { checked: true }),
            checkbox_style,
        ));

        let mut slider_style = ComputedStyle::new();
        border_box(&mut slider_style);
        slider_style.position = rustkit_css::Position::Absolute;
        slider_style.top = Some(Length::Px(0.0));
        slider_style.left = Some(Length::Px(0.0));
        slider_style.right = Some(Length::Px(0.0));
        slider_style.bottom = Some(Length::Px(0.0));
        switch
            .children
            .push(LayoutBox::new(BoxType::Inline, slider_style));

        let mut label_style = ComputedStyle::new();
        border_box(&mut label_style);
        label_style.display = rustkit_css::Display::Flex;
        label_style.flex_direction = FlexDirection::Row;
        label_style.align_items = AlignItems::Center;
        label_style.column_gap = Length::Px(12.0);
        let mut label = LayoutBox::new(BoxType::Block, label_style);
        label.children.push(switch);
        let mut text_style = ComputedStyle::new();
        text_style.font_size = Length::Px(16.0);
        label.children.push(LayoutBox::new(
            BoxType::Text("Enable notifications".into()),
            text_style,
        ));
        label
    }

    #[test]
    fn a_definite_cross_size_survives_its_childrens_flow() {
        let mut label = toggle_switch_row();
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 700.0, 0.0),
            ..Default::default()
        };
        layout_flex_container(&mut label, &containing);

        let switch_h = label.children[0].dimensions.border_box().height;
        assert!(
            (switch_h - 26.0).abs() < 0.01,
            "an explicit `height: 26px` flex item must stay 26 tall, got {switch_h}"
        );

        // The same clobber is visible one level out: align-items:center centres
        // every item against the line, so a line sized by the wrong item height
        // puts the sibling text at the wrong y. 26/2 - 16/2 = 5.
        let text = &label.children[1].dimensions;
        let text_y = text.content.y;
        let expected = (26.0 - text.border_box().height) / 2.0;
        assert!(
            (text_y - expected).abs() < 0.01,
            "the centred sibling must centre against the 26px line, expected \
             {expected}, got {text_y}"
        );
    }

    /// A flex item with a DEFINITE cross size is the containing block its
    /// in-flow children resolve percentage heights against (CSS 2.1 §10.5).
    /// Step 11 lays those children out through
    /// `layout_block_children_with_collapse`, which hands them the item's flow
    /// cursor in `content.height` — so before the definite height was passed
    /// alongside it, a `height: 100%` child fell back to the VIEWPORT.
    /// T-RED without the `definite_cross_height` argument: the fill is the
    /// viewport height, not 26.
    #[test]
    fn a_percentage_height_child_of_a_definite_flex_item_fills_that_item() {
        let mut label = toggle_switch_row();
        // Replace the abspos slider with an in-flow percentage-height fill:
        // the abspos path has its own re-anchor, the in-flow path did not.
        let mut fill_style = ComputedStyle::new();
        fill_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        fill_style.height = Length::Percent(100.0);
        label.children[0].children[1] = LayoutBox::new(BoxType::Block, fill_style);
        label.children[0].children[1].viewport = (900.0, 1000.0);
        label.children[0].viewport = (900.0, 1000.0);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 700.0, 0.0),
            ..Default::default()
        };
        layout_flex_container(&mut label, &containing);

        let fill = label.children[0].children[1].dimensions.content.height;
        assert!(
            (fill - 26.0).abs() < 0.01,
            "height:100% of a 26px flex item is 26, got {fill}"
        );
    }

    #[test]
    fn an_auto_cross_size_still_takes_its_childrens_flow() {
        // The other side of §9.4, and the boundary the fix must not cross: an
        // item with NO definite cross size is content-sized, so its children's
        // flow — not the measure it arrived with — is what decides its height.
        //
        // The seeded 60 is what the block pre-pass leaves on the box, and it
        // is deliberately wrong: the pre-pass measures at the container's
        // width, the flex algorithm then hands the item a different main size,
        // and step 11 relays the children. Asserting only "> 26.5" would pass
        // on the stale 60 as happily as on the real 32.08, which is the shape
        // of guard this file keeps writing and this sweep keeps catching.
        let mut label = toggle_switch_row();
        label.children[0].style.height = Length::Auto;
        label.children[0].dimensions.content.height = 60.0;
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 700.0, 0.0),
            ..Default::default()
        };
        layout_flex_container(&mut label, &containing);

        let switch_h = label.children[0].dimensions.border_box().height;
        assert!(
            (26.5..40.0).contains(&switch_h),
            "an auto-height item takes its two in-flow children's flow \
             (measured 32.08), not the stale 60 it arrived with, got {switch_h}"
        );
    }

    #[test]
    fn an_explicit_width_does_not_freeze_a_column_items_height() {
        // The axis half of the rule. In a COLUMN container the cross axis is
        // horizontal, so `width` is what `has_explicit_cross_size` reports —
        // and the item's HEIGHT is its main size, not its cross size. Freezing
        // the height here would be the same clobber with the sign flipped:
        // a content-sized column item would stop growing to its children.
        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        item_style.width = Length::Px(200.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        for _ in 0..3 {
            let mut child_style = ComputedStyle::new();
            child_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
            child_style.height = Length::Px(40.0);
            item.children
                .push(LayoutBox::new(BoxType::Block, child_style));
        }
        // As above: the height the item arrives with is the pre-pass's, and it
        // must not be what survives.
        item.dimensions.content.height = 60.0;

        let mut column_style = ComputedStyle::new();
        column_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        column_style.display = rustkit_css::Display::Flex;
        column_style.flex_direction = FlexDirection::Column;
        let mut column = LayoutBox::new(BoxType::Block, column_style);
        column.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 700.0, 0.0),
            ..Default::default()
        };
        layout_flex_container(&mut column, &containing);

        let item_h = column.children[0].dimensions.border_box().height;
        assert!(
            (item_h - 120.0).abs() < 0.01,
            "a column item with an explicit WIDTH still takes its three 40px \
             children's height, expected 120, got {item_h}"
        );
    }

    /// The chrome strip's nav bar, reduced to the two boxes that matter.
    ///
    /// `.nav-bar { height: 44px; border-bottom: 1px }` holds
    /// `.sidebar-toggle { width: 200px; height: 100% }`. The containing block
    /// is the 100px-tall chrome viewport, which is the trap: a percentage that
    /// resolves against IT instead of against the flex container comes out 100
    /// against Chrome's 43.
    fn nav_bar_with_percent_height_item(
        container_height: Length,
        item_height: Length,
    ) -> LayoutBox {
        let mut toggle_style = ComputedStyle::new();
        toggle_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        toggle_style.width = Length::Px(200.0);
        toggle_style.height = item_height;
        let toggle = LayoutBox::new(BoxType::Block, toggle_style);

        let mut btn_style = ComputedStyle::new();
        btn_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        btn_style.width = Length::Px(32.0);
        btn_style.height = Length::Px(32.0);
        let btn = LayoutBox::new(BoxType::Block, btn_style);

        let mut bar_style = ComputedStyle::new();
        bar_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        bar_style.display = rustkit_css::Display::Flex;
        bar_style.flex_direction = FlexDirection::Row;
        bar_style.align_items = AlignItems::Center;
        bar_style.height = container_height;
        let mut bar = LayoutBox::new(BoxType::Block, bar_style);
        // The block pre-pass resolves padding and border onto dimensions
        // before flex runs; the inner cross size is 44 - 1 = 43.
        bar.dimensions.border.bottom = 1.0;
        bar.children.push(toggle);
        bar.children.push(btn);
        bar
    }

    fn laid_out_item_height(container_height: Length, item_height: Length) -> f32 {
        let mut bar = nav_bar_with_percent_height_item(container_height, item_height);
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1280.0, 100.0),
            ..Default::default()
        };
        layout_flex_container(&mut bar, &containing);
        bar.children[0].dimensions.content.height
    }

    #[test]
    fn a_percentage_cross_size_resolves_against_the_flex_containers_definite_inner_size() {
        let h = laid_out_item_height(Length::Px(44.0), Length::Percent(100.0));
        assert!(
            (h - 43.0).abs() < 0.5,
            "height:100% of a 44px bar with a 1px bottom border is 43, got {h}"
        );
        assert!(
            (h - 100.0).abs() > 0.5,
            "the percentage must not resolve against the 100px containing block, got {h}"
        );
    }

    #[test]
    fn a_percentage_cross_size_behaves_as_auto_when_the_container_is_indefinite() {
        // css-sizing-3 §5.1: with no definite basis the percentage behaves as
        // `auto`, so the two trees must agree exactly. This is the half that
        // stops the fix from reaching a container it cannot resolve against.
        let percent = laid_out_item_height(Length::Auto, Length::Percent(100.0));
        let auto = laid_out_item_height(Length::Auto, Length::Auto);
        assert!(
            (percent - auto).abs() < 0.001,
            "an indefinite container must treat height:100% as auto: \
             percent gave {percent}, auto gave {auto}"
        );
    }

    #[test]
    fn a_resolved_percentage_cross_size_is_not_stretched_or_floored_by_content() {
        // The item is SHORTER than the line (the 32px button plus the bar's
        // own 43px inner size), so a stretch or an intrinsic floor that still
        // applied would show up as a taller box.
        let h = laid_out_item_height(Length::Px(84.0), Length::Percent(50.0));
        assert!(
            (h - 41.5).abs() < 0.5,
            "50% of an 83px inner size is 41.5, got {h}"
        );
    }

    #[test]
    fn a_resolved_percentage_is_a_content_size_under_content_box_sizing() {
        // The corpus is `* { box-sizing: border-box }`, so every fixture above
        // makes `spec_cross_to_border_box` the identity and none of them can
        // see whether the conversion is applied at all. Under content-box the
        // resolved 41.5 is the CONTENT height and the item's own padding is
        // added on top; skipping the conversion subtracts that padding twice.
        let mut toggle_style = ComputedStyle::new();
        toggle_style.width = Length::Px(200.0);
        toggle_style.height = Length::Percent(50.0);
        let mut toggle = LayoutBox::new(BoxType::Block, toggle_style);
        toggle.dimensions.padding.top = 6.0;
        toggle.dimensions.padding.bottom = 6.0;

        let mut bar_style = ComputedStyle::new();
        bar_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        bar_style.display = rustkit_css::Display::Flex;
        bar_style.flex_direction = FlexDirection::Row;
        bar_style.align_items = AlignItems::Center;
        bar_style.height = Length::Px(84.0);
        let mut bar = LayoutBox::new(BoxType::Block, bar_style);
        bar.dimensions.border.bottom = 1.0;
        bar.children.push(toggle);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 1280.0, 100.0),
            ..Default::default()
        };
        layout_flex_container(&mut bar, &containing);

        let h = bar.children[0].dimensions.content.height;
        assert!(
            (h - 41.5).abs() < 0.5,
            "content-box: 50% of 83 is a 41.5 CONTENT height, got {h}"
        );
    }


    /// An out-of-flow column flex container, `height: auto`, whose main size
    /// is definite only because both insets are set (CSS2 §10.6.4).
    ///
    /// Items are content-sized with a fixed-height child so that step 11d's
    /// re-derivation actually fires — that repass is where the container's
    /// main size is read a second time, and a container built from explicit
    /// item heights passes every assertion below without the fix.
    fn inset_column(
        position: crate::Position,
        top: Option<f32>,
        bottom: Option<f32>,
        height: Length,
        child_heights: &[f32],
    ) -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.justify_content = JustifyContent::Center;
        style.height = height;
        let mut container = LayoutBox::with_position(BoxType::Block, style, position);
        container.set_offsets(top, None, bottom, None);
        for h in child_heights {
            let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
            let mut cs = ComputedStyle::new();
            cs.height = Length::Px(*h);
            item.children.push(LayoutBox::new(BoxType::Block, cs));
            container.children.push(item);
        }
        container
    }

    fn inset_cb(height: f32) -> Dimensions {
        Dimensions {
            content: Rect::new(0.0, 0.0, 288.0, height),
            ..Default::default()
        }
    }

    /// The PRODUCTION call shape — and the reason the first version of these
    /// guards could not fail.
    ///
    /// Flex is handed the container's OWN box, whose `content.width` is
    /// already resolved (`calculate_block_width` runs first) but whose
    /// `content.height` is still the block pre-pass's flow cursor: the content
    /// sum, and the wrong answer. The containing block arrives separately.
    /// That asymmetry is the whole defect.
    ///
    /// The original guards passed the containing block itself as the
    /// container's own box — a shape no caller in the engine uses — so they
    /// handed the fix the number it was supposed to derive and stayed green
    /// while image-gallery stayed broken.
    fn layout_inset_column(c: &mut LayoutBox, cb: &Dimensions, flow_cursor: f32) {
        let mut own = c.dimensions.clone();
        own.content.width = cb.content.width;
        own.content.height = flow_cursor;
        layout_flex_container_in(c, &own, Some(cb));
    }

    /// Leading space above the first item and trailing space below the last,
    /// in the container's own content box. `justify-content: center` owes
    /// them equal halves of the free space.
    fn lead_and_trail(container: &LayoutBox, inner_main: f32) -> (f32, f32) {
        let top = container.dimensions.content.y;
        let first = &container.children[0].dimensions;
        let last = &container.children[container.children.len() - 1].dimensions;
        (
            first.content.y - top,
            (top + inner_main) - (last.content.y + last.content.height),
        )
    }

    #[test]
    fn an_inset_stretched_column_centres_in_its_used_height_not_its_content() {
        // image-gallery `.aspect-box > .content`: position:absolute; inset:0;
        // flex-direction:column; justify-content:center, in a 32px box with
        // 19.65 of content. Chrome leaves 6.17 above and below. Step 8 got
        // this right against the containing block's 32 and step 11d threw it
        // away, re-justifying against the 19.65 content sum: free space 0,
        // and `center` packed both items flush against the top edge.
        let mut c = inset_column(
            crate::Position::Absolute,
            Some(0.0),
            Some(0.0),
            Length::Auto,
            &[8.653847, 11.0],
        );
        layout_inset_column(&mut c, &inset_cb(32.0), 19.653847);
        let (lead, trail) = lead_and_trail(&c, 32.0);
        assert!(
            (lead - 6.173).abs() < 0.5 && (trail - 6.173).abs() < 0.5,
            "items centre in the 32px used height: expected 6.17 / 6.17, got {lead} / {trail}"
        );
    }

    #[test]
    fn an_inset_definite_main_size_subtracts_the_insets_and_the_containers_own_edges() {
        // 200px containing block, inset 20 top / 30 bottom, 10px padding top
        // and bottom: the inner main size is 200-20-30-10-10 = 130, not 200
        // and not 150. Reading the containing block raw, or forgetting the
        // container's own edges, centres the stack off by what was skipped.
        let mut c = inset_column(
            crate::Position::Absolute,
            Some(20.0),
            Some(30.0),
            Length::Auto,
            &[30.0, 20.0],
        );
        c.dimensions.padding = EdgeSizes {
            top: 10.0,
            bottom: 10.0,
            left: 0.0,
            right: 0.0,
        };
        c.dimensions.border = EdgeSizes {
            top: 5.0,
            bottom: 5.0,
            left: 0.0,
            right: 0.0,
        };
        layout_inset_column(&mut c, &inset_cb(200.0), 50.0);
        let (lead, trail) = lead_and_trail(&c, 120.0);
        assert!(
            (lead - 35.0).abs() < 0.5 && (trail - 35.0).abs() < 0.5,
            "inner main is 120 (200-20-30-10-10-5-5), 50 of content -> 35 a side: \
             got {lead} / {trail}"
        );
    }

    #[test]
    fn one_inset_alone_leaves_the_main_size_indefinite() {
        // CSS2 §10.6.4 needs BOTH offsets. `top: 0` with `height: auto` is
        // sized by content, so there is no free space and nothing to centre:
        // the stack starts at the container's content edge.
        let mut c = inset_column(
            crate::Position::Absolute,
            Some(0.0),
            None,
            Length::Auto,
            &[30.0, 20.0],
        );
        layout_inset_column(&mut c, &inset_cb(200.0), 50.0);
        let (lead, _) = lead_and_trail(&c, 50.0);
        assert!(
            lead.abs() < 0.5,
            "a single inset is not a definite height: expected the stack at 0, got {lead}"
        );
    }

    #[test]
    fn an_in_flow_box_does_not_take_a_definite_height_from_stray_offsets() {
        // `position: static` ignores `top`/`bottom` entirely. A static box
        // that happens to carry them must keep sizing by content, or every
        // auto-height column in the corpus starts centring in its containing
        // block — new_tab's body among them.
        let mut c = inset_column(
            crate::Position::Static,
            Some(0.0),
            Some(0.0),
            Length::Auto,
            &[30.0, 20.0],
        );
        layout_inset_column(&mut c, &inset_cb(200.0), 50.0);
        let (lead, _) = lead_and_trail(&c, 50.0);
        assert!(
            lead.abs() < 0.5,
            "static ignores insets: expected the stack at 0, got {lead}"
        );
    }

    #[test]
    fn an_explicit_height_beats_the_insets() {
        // With `height` specified, CSS2 §10.6.4 is over-constrained and the
        // specified height wins (`bottom` is ignored). Centring 50 of content
        // in the specified 100 leaves 25 a side, not the 75 the 200px
        // containing block would give.
        let mut c = inset_column(
            crate::Position::Absolute,
            Some(0.0),
            Some(0.0),
            Length::Px(100.0),
            &[30.0, 20.0],
        );
        layout_inset_column(&mut c, &inset_cb(200.0), 50.0);
        let (lead, trail) = lead_and_trail(&c, 100.0);
        assert!(
            (lead - 25.0).abs() < 0.5 && (trail - 25.0).abs() < 0.5,
            "the specified 100px height wins over the insets: expected 25 / 25, got {lead} / {trail}"
        );
    }

    #[test]
    fn a_fixed_inset_column_resolves_against_the_viewport_not_the_passed_block() {
        // CSS2 §10.1: a fixed box's containing block is the VIEWPORT. The
        // block handed to flex is whatever laid it out — 200 here against a
        // 600px viewport — and asking it centres a full-screen modal inside
        // its parent instead. 50 of content in 600 -> 275 a side.
        let mut c = inset_column(
            crate::Position::Fixed,
            Some(0.0),
            Some(0.0),
            Length::Auto,
            &[30.0, 20.0],
        );
        c.set_viewport(288.0, 600.0);
        layout_inset_column(&mut c, &inset_cb(200.0), 50.0);
        let (lead, trail) = lead_and_trail(&c, 600.0);
        assert!(
            (lead - 275.0).abs() < 0.5 && (trail - 275.0).abs() < 0.5,
            "a fixed container centres in the 600px viewport: expected 275 / 275, got {lead} / {trail}"
        );
    }


    /// A COLUMN flex item with a definite MAIN size keeps it; its children's
    /// flow never grows it (css-flexbox-1 §9.7 — the used main size is the
    /// resolved one, content overflows).
    ///
    /// T-RED: step 11 lays a block flex item's children out with
    /// `layout_block_children_with_collapse`, which ends by assigning the flow
    /// cursor to `content.height`. In a column container that field is the
    /// item's MAIN size, already decided in steps 4–10, so the assignment
    /// replaces it — and step 11d only re-derives items whose main size came
    /// from content, so nothing repairs it. Measured before the fix: 120.
    ///
    /// Chrome 148 on the bundled Chromium, same tree:
    ///   #a { height: 30px } with 3x40px children -> y 0, height 30
    ///   #b (its sibling)                         -> y 30
    ///
    /// `prepass_height` is what the block pre-pass leaves on the CONTAINER's
    /// own box, and it is the shape production uses: every caller passes
    /// `container.dimensions.clone()`, so `container_main_size` is that
    /// number, not a containing block's. A fixture that hands over a zero
    /// height instead makes every item shrink to nothing and then tests the
    /// flow that rescues them — which is a different tree from the engine's.
    fn column_of_two(
        first_height: Length,
        container_height: Length,
        prepass_height: f32,
    ) -> LayoutBox {
        let mut column_style = ComputedStyle::new();
        column_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        column_style.display = rustkit_css::Display::Flex;
        column_style.flex_direction = FlexDirection::Column;
        column_style.height = container_height;
        let mut column = LayoutBox::new(BoxType::Block, column_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        item_style.height = first_height;
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        for _ in 0..3 {
            let mut child_style = ComputedStyle::new();
            child_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
            child_style.height = Length::Px(40.0);
            item.children
                .push(LayoutBox::new(BoxType::Block, child_style));
        }
        // The height the block pre-pass leaves on the box, and it is
        // deliberately the WRONG one: the pre-pass measures before the flex
        // algorithm hands the item its main size, so a guard that asserted a
        // number this field already agreed with would pass on the clobber.
        item.dimensions.content.height = 120.0;
        column.children.push(item);

        let mut sibling_style = ComputedStyle::new();
        sibling_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        sibling_style.height = Length::Px(30.0);
        column
            .children
            .push(LayoutBox::new(BoxType::Block, sibling_style));
        column.dimensions.content = Rect::new(0.0, 0.0, 700.0, prepass_height);
        column
    }

    #[test]
    fn a_definite_main_size_survives_its_childrens_flow() {
        // 60 is the pre-pass's stack of the two 30px items, which is what the
        // engine hands an auto-height column container.
        let mut column = column_of_two(Length::Px(30.0), Length::Auto, 60.0);
        let containing = column.dimensions.clone();
        layout_flex_container(&mut column, &containing);

        let item_h = column.children[0].dimensions.border_box().height;
        assert!(
            (item_h - 30.0).abs() < 0.01,
            "an explicit `height: 30px` column item must stay 30 tall against \
             120px of children, got {item_h}"
        );

        // No assertion on the SIBLING's position, deliberately: measured, it
        // is y=30 with the clobber and without it, because apply_positions
        // places every item from `target_main_size` and never re-reads the
        // box the flow overwrote. The clobber damages the item's own height
        // only, and an assertion here would be green either way.
    }

    #[test]
    fn a_grown_main_size_survives_its_childrens_flow() {
        // The restore must carry the RESOLVED main size, not the style length.
        // `flex: 1 1 auto; height: 30px` in a 400px column grows to 400 in
        // Chrome 148 (measured); freezing `style.height` would report 30.
        let mut column = column_of_two(Length::Px(30.0), Length::Px(400.0), 400.0);
        column.children.pop();
        column.children[0].style.flex_grow = 1.0;
        let containing = column.dimensions.clone();
        layout_flex_container(&mut column, &containing);

        let item_h = column.children[0].dimensions.border_box().height;
        assert!(
            (item_h - 400.0).abs() < 0.01,
            "a grown column item keeps the 400 it grew to, not its 30px style \
             height and not its 120px of children, got {item_h}"
        );
    }

    #[test]
    fn a_content_sized_main_size_still_takes_its_childrens_flow() {
        // The boundary the restore must not cross. With `height: auto` the
        // item's main size IS its content, so the flow is the answer and step
        // 11d re-derives from it. Asserting only "> 30" would pass on the
        // stale 120 the fixture seeds; the number is the three 40px children.
        // The pre-pass stacks the auto item at its three children (120) plus
        // the 30px sibling.
        let mut column = column_of_two(Length::Auto, Length::Auto, 150.0);
        let containing = column.dimensions.clone();
        layout_flex_container(&mut column, &containing);

        let item_h = column.children[0].dimensions.border_box().height;
        assert!(
            (item_h - 120.0).abs() < 0.01,
            "an auto-height column item takes its three 40px children, got {item_h}"
        );
    }

    #[test]
    fn a_row_items_height_is_not_frozen_by_this_rule() {
        // The axis half. In a ROW container the item's height is its CROSS
        // size, which this rule does not own — a content-sized row item must
        // still grow to its children. (§9.4's definite-cross half is a
        // separate rule with its own guard.)
        let mut row_style = ComputedStyle::new();
        row_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        row_style.display = rustkit_css::Display::Flex;
        row_style.flex_direction = FlexDirection::Row;
        let mut row = LayoutBox::new(BoxType::Block, row_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        item_style.width = Length::Px(200.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        for _ in 0..3 {
            let mut child_style = ComputedStyle::new();
            child_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
            child_style.height = Length::Px(40.0);
            item.children
                .push(LayoutBox::new(BoxType::Block, child_style));
        }
        item.dimensions.content.height = 60.0;
        row.children.push(item);
        row.dimensions.content = Rect::new(0.0, 0.0, 700.0, 60.0);

        let containing = row.dimensions.clone();
        layout_flex_container(&mut row, &containing);

        let item_h = row.children[0].dimensions.border_box().height;
        assert!(
            (item_h - 120.0).abs() < 0.01,
            "a row item with an explicit WIDTH still takes its three 40px \
             children's height, expected 120, got {item_h}"
        );
    }


    #[test]
    fn a_column_flex_item_grows_into_the_containers_definite_height() {
        // css-flexbox-1 §9.7: free space is the container's inner MAIN size
        // minus the items' outer hypothetical main sizes. Chrome 148,
        // measured on the bundled Chromium before this test was written:
        //
        //   .colg { height: 400px; display: flex; flex-direction: column }
        //     #g1 { flex-grow: 1; height: 30px }   ->  370
        //     #g2 {               height: 30px }   ->   30, at y = 370
        //
        // The fixture mirrors the ENGINE's call shape, and that is the whole
        // point of it: every production caller passes the container's OWN
        // dimensions, whose content.height on the block path is the pre-pass
        // flow cursor — the 60px stack of the two children — and NOT the
        // container's 400px used height. Seeding 400 here would hand the fix
        // the number it is supposed to derive.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        style.height = Length::Px(400.0);

        let mut container = LayoutBox::new(BoxType::Block, style);
        for grow in [1.0f32, 0.0] {
            let mut s = ComputedStyle::new();
            s.height = Length::Px(30.0);
            s.flex_grow = grow;
            container.children.push(LayoutBox::new(BoxType::Block, s));
        }
        container.dimensions.content = Rect::new(0.0, 0.0, 200.0, 60.0);
        let containing = container.dimensions.clone();
        layout_flex_container(&mut container, &containing);

        let a = container.children[0].dimensions.content.height;
        let b = container.children[1].dimensions.content.height;
        let b_y = container.children[1].dimensions.content.y;
        assert!(
            (a - 370.0).abs() < 0.5,
            "the grow item takes the 340px of free space in the 400px column, got {a}"
        );
        assert!(
            (b - 30.0).abs() < 0.5,
            "the non-grow item keeps 30, got {b}"
        );
        assert!(
            (b_y - 370.0).abs() < 0.5,
            "the sibling sits after the grown item, got {b_y}"
        );
    }

    #[test]
    fn a_definite_column_main_size_is_inner_under_both_box_sizing_modes() {
        // The free space a column distributes is the container's INNER main
        // size, so `box-sizing` decides whether the specified height already
        // contains the padding and border. Chrome 148, measured:
        //
        //   .bb { box-sizing: border-box; height: 400px;
        //         padding: 25px 0; border-top/bottom: 5px }
        //     #k1 { flex-grow: 1; height: 30px }  -> 310 at y = 30
        //     #k2 {               height: 30px }  ->  30 at y = 340
        //   .cb { box-sizing: content-box; height: 400px; padding: 25px 0 }
        //     #m1 { flex-grow: 1; height: 30px }  -> 400
        //
        // 400 - 10 border - 50 padding = 340 inner, minus 60 of items = 280
        // of free space; content-box keeps all 400. Without the subtraction
        // the border-box item grows 60px past its container.
        fn column(border_box: bool) -> LayoutBox {
            let mut style = ComputedStyle::new();
            style.display = rustkit_css::Display::Flex;
            style.flex_direction = FlexDirection::Column;
            style.height = Length::Px(400.0);
            style.box_sizing = if border_box {
                rustkit_css::BoxSizing::BorderBox
            } else {
                rustkit_css::BoxSizing::ContentBox
            };
            let mut c = LayoutBox::new(BoxType::Block, style);
            for grow in [1.0f32, 0.0] {
                let mut s = ComputedStyle::new();
                s.height = Length::Px(30.0);
                s.flex_grow = grow;
                c.children.push(LayoutBox::new(BoxType::Block, s));
            }
            // Resolved by the block pre-pass before flex ever runs.
            c.dimensions.padding.top = 25.0;
            c.dimensions.padding.bottom = 25.0;
            if border_box {
                c.dimensions.border.top = 5.0;
                c.dimensions.border.bottom = 5.0;
            }
            // The content ORIGIN the pre-pass hands over already sits inside
            // the container's own border and padding, so items are placed
            // from there and Chrome's absolute y values carry straight over.
            let origin_y = if border_box { 30.0 } else { 25.0 };
            c.dimensions.content = Rect::new(0.0, origin_y, 200.0, 60.0);
            c
        }

        let mut bb = column(true);
        let cb_dims = bb.dimensions.clone();
        layout_flex_container(&mut bb, &cb_dims);
        let k1 = bb.children[0].dimensions.content.height;
        let k1_y = bb.children[0].dimensions.content.y;
        let k2_y = bb.children[1].dimensions.content.y;
        assert!(
            (k1_y - 30.0).abs() < 0.5,
            "border-box: the first item starts inside the border and padding, got {k1_y}"
        );
        assert!(
            (k1 - 310.0).abs() < 0.5,
            "border-box: 400 less 10 border and 50 padding leaves 340 inner, so the grow item is 310, got {k1}"
        );
        assert!(
            (k2_y - 340.0).abs() < 0.5,
            "border-box: the sibling sits at 340, got {k2_y}"
        );

        let mut cb = column(false);
        let cb2 = cb.dimensions.clone();
        layout_flex_container(&mut cb, &cb2);
        let m1 = cb.children[0].dimensions.content.height;
        assert!(
            (m1 - 370.0).abs() < 0.5,
            "content-box: the specified 400 IS the inner size, so 400 less the sibling's 30 leaves 370, got {m1}"
        );
    }


    /// Container + one in-flow item + one out-of-flow child, as the probe
    /// page builds it. `oof` is styled by the caller.
    fn probe(
        direction: FlexDirection,
        justify: JustifyContent,
        align: AlignItems,
        mut oof: LayoutBox,
    ) -> LayoutBox {
        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        item_style.width = Length::Px(60.0);
        item_style.height = Length::Px(30.0);
        let item = LayoutBox::new(BoxType::Block, item_style);

        oof.style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        oof.dimensions.content.width = 50.0;
        oof.dimensions.content.height = 20.0;

        let mut c_style = ComputedStyle::new();
        c_style.box_sizing = rustkit_css::BoxSizing::BorderBox;
        c_style.display = rustkit_css::Display::Flex;
        c_style.flex_direction = direction;
        c_style.justify_content = justify;
        c_style.align_items = align;
        c_style.width = Length::Px(400.0);
        c_style.height = Length::Px(200.0);
        let mut container = LayoutBox::new(BoxType::Block, c_style);
        container.children.push(item);
        container.children.push(oof);
        container
    }

    fn oof_box() -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.position = rustkit_css::Position::Absolute;
        s.width = Length::Px(50.0);
        s.height = Length::Px(20.0);
        LayoutBox::new(BoxType::Block, s)
    }

    fn run(container: &mut LayoutBox, w: f32, h: f32) -> Rect {
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, w, h),
            ..Default::default()
        };
        container.dimensions.content = containing.content;
        layout_flex_container(container, &containing);
        container.children[1].dimensions.border_box()
    }

    #[test]
    fn an_out_of_flow_flex_child_sits_where_the_sole_flex_item_would() {
        // Chrome: x=175, y=90 — centred on BOTH axes, not stacked after the
        // in-flow item at (0, 30), which is where the block flow cursor left
        // it and where RustKit left it until this fix.
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::Center,
            AlignItems::Center,
            oof_box(),
        );
        let r = run(&mut c, 400.0, 200.0);
        assert!(
            (r.x - 175.0).abs() < 0.01 && (r.y - 90.0).abs() < 0.01,
            "expected Chrome's (175, 90), got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn a_specified_inset_beats_the_static_position_on_that_axis_alone() {
        // Chrome, `top: 10px`: x=175 (still centred), y=10 (the inset).
        // `apply_position_offsets_absolute` owns resolving the inset and has
        // already run by the time §4.1 is reached on the production path, so
        // the box enters this step at y=10; what step 13 owns is LEAVING it
        // there. The end-to-end pair is the probe page, where the same shape
        // captures at Chrome's (175, 10) exactly.
        let mut oof = oof_box();
        oof.dimensions.content.y = 10.0;
        oof.offsets.top = Some(10.0);
        oof.style.top = Some(Length::Px(10.0));
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::Center,
            AlignItems::Center,
            oof,
        );
        let r = run(&mut c, 400.0, 200.0);
        assert!(
            (r.x - 175.0).abs() < 0.01 && (r.y - 10.0).abs() < 0.01,
            "top:10px keeps y at the inset and still centres x; \
             expected (175, 10), got ({}, {})",
            r.x,
            r.y
        );

        // Chrome, `left: 10px`: x=10 (the inset), y=90 (still centred).
        let mut oof = oof_box();
        oof.dimensions.content.x = 10.0;
        oof.offsets.left = Some(10.0);
        oof.style.left = Some(Length::Px(10.0));
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::Center,
            AlignItems::Center,
            oof,
        );
        let r = run(&mut c, 400.0, 200.0);
        assert!(
            (r.x - 10.0).abs() < 0.01 && (r.y - 90.0).abs() < 0.01,
            "left:10px keeps x at the inset and still centres y; \
             expected (10, 90), got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn the_static_position_aligns_the_childs_margin_box() {
        // Chrome, `margin: 8px 12px`: x=175, y=90 — the MARGIN box is
        // centred, so the border box lands 12 right and 8 down of it.
        // Centring the border box instead would give x=175, y=90 only by
        // accident of symmetry, so the assertion below also pins the
        // margin box itself.
        let mut oof = oof_box();
        oof.dimensions.margin = EdgeSizes {
            top: 8.0,
            bottom: 8.0,
            left: 12.0,
            right: 12.0,
        };
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::FlexEnd,
            AlignItems::FlexEnd,
            oof,
        );
        let r = run(&mut c, 400.0, 200.0);
        // Margin box flush to the far corner: its right edge at 400 and
        // bottom at 200 puts the border box at 400-12-50=338, 200-8-20=172.
        assert!(
            (r.x - 338.0).abs() < 0.01 && (r.y - 172.0).abs() < 0.01,
            "flex-end aligns the MARGIN box to the far edge; \
             expected (338, 172), got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn row_reverse_puts_main_start_at_the_far_edge() {
        // Chrome, row-reverse + justify-content:flex-start: x=350, y=0.
        let mut c = probe(
            FlexDirection::RowReverse,
            JustifyContent::FlexStart,
            AlignItems::FlexStart,
            oof_box(),
        );
        let r = run(&mut c, 400.0, 200.0);
        assert!(
            (r.x - 350.0).abs() < 0.01 && (r.y - 0.0).abs() < 0.01,
            "row-reverse main-start is the RIGHT edge; expected (350, 0), \
             got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn align_self_on_the_out_of_flow_child_beats_the_containers_align_items() {
        // Chrome, container align-items:center + child align-self:flex-end:
        // x=175, y=180.
        let mut oof = oof_box();
        oof.style.align_self = AlignSelf::FlexEnd;
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::Center,
            AlignItems::Center,
            oof,
        );
        let r = run(&mut c, 400.0, 200.0);
        assert!(
            (r.x - 175.0).abs() < 0.01 && (r.y - 180.0).abs() < 0.01,
            "align-self:flex-end wins over align-items:center; \
             expected (175, 180), got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn space_between_packs_a_sole_out_of_flow_child_to_main_start() {
        // Chrome, justify-content:space-between + default align-items
        // (stretch): x=0, y=0. A stretch keyword does not stretch a
        // static-position box, and space-between with one item is
        // main-start.
        // The box STARTS at the block flow cursor (0, 30) — after the 30px
        // in-flow item, which is where the pre-pass leaves it and where the
        // defect left it. Without that, main-start (0, 0) is also the default
        // origin and this guard would stay green with the whole fix removed.
        let mut oof = oof_box();
        oof.dimensions.content = Rect::new(0.0, 30.0, 50.0, 20.0);
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::SpaceBetween,
            AlignItems::Stretch,
            oof,
        );
        let r = run(&mut c, 400.0, 200.0);
        assert!(
            (r.x - 0.0).abs() < 0.01
                && (r.y - 0.0).abs() < 0.01
                && (r.height - 20.0).abs() < 0.01,
            "expected Chrome's (0, 0) at 20px tall, got ({}, {}) at {}",
            r.x,
            r.y,
            r.height
        );
    }

    #[test]
    fn the_static_position_rectangle_is_the_containers_content_box() {
        // Chrome, padding 20px 40px on the container: x=175, y=90 — i.e.
        // centred in the 320x160 CONTENT box at origin (40, 20), not in the
        // 400x200 border box. Reading the border box would give (175, 90)
        // as well, so the container is made asymmetric: padding-left 40,
        // padding-right 0 puts the content box at 40..400.
        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::Center,
            AlignItems::Center,
            oof_box(),
        );
        c.dimensions.padding = EdgeSizes {
            top: 20.0,
            bottom: 0.0,
            left: 40.0,
            right: 0.0,
        };
        let containing = Dimensions {
            content: Rect::new(40.0, 20.0, 360.0, 180.0),
            padding: c.dimensions.padding,
            ..Default::default()
        };
        c.dimensions.content = containing.content;
        layout_flex_container(&mut c, &containing);
        let r = c.children[1].dimensions.border_box();
        assert!(
            (r.x - 195.0).abs() < 0.01 && (r.y - 100.0).abs() < 0.01,
            "centred in the content box at (40,20,360,180): \
             expected (195, 100), got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn the_main_size_aligned_in_is_floored_by_min_height() {
        // Chrome, column + height:auto + min-height:200px: y=90. The free
        // space justify-content centres in is the FLOORED 200, not the
        // 30px in-flow item's stack. This is `min-height: 100vh` — the
        // centring idiom on new_tab's body — in miniature.
        let mut c = probe(
            FlexDirection::Column,
            JustifyContent::Center,
            AlignItems::Center,
            oof_box(),
        );
        c.style.height = Length::Auto;
        c.style.min_height = Length::Px(200.0);
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 0.0),
            ..Default::default()
        };
        c.dimensions.content = containing.content;
        layout_flex_container(&mut c, &containing);
        let r = c.children[1].dimensions.border_box();
        assert!(
            (r.x - 175.0).abs() < 0.01 && (r.y - 90.0).abs() < 0.01,
            "an auto height floored by min-height is the size justify-content \
             centres in; expected (175, 90), got ({}, {})",
            r.x,
            r.y
        );
    }

    #[test]
    fn the_static_position_carries_the_childs_own_subtree() {
        // A box that moves without its descendants strands their text at
        // the old origin — the defect `translate_subtree` exists for. The
        // grandchild sits 5px inside the out-of-flow box and must still be
        // 5px inside it after §4.1 moves it.
        let mut oof = oof_box();
        let mut inner_style = ComputedStyle::new();
        inner_style.width = Length::Px(10.0);
        inner_style.height = Length::Px(10.0);
        let mut inner = LayoutBox::new(BoxType::Block, inner_style);
        inner.dimensions.content = Rect::new(5.0, 35.0, 10.0, 10.0);
        oof.children.push(inner);
        oof.dimensions.content = Rect::new(0.0, 30.0, 50.0, 20.0);

        let mut c = probe(
            FlexDirection::Row,
            JustifyContent::Center,
            AlignItems::Center,
            oof,
        );
        let r = run(&mut c, 400.0, 200.0);
        let inner = c.children[1].children[0].dimensions.content;
        assert!(
            (r.x - 175.0).abs() < 0.01 && (r.y - 90.0).abs() < 0.01,
            "expected (175, 90), got ({}, {})",
            r.x,
            r.y
        );
        assert!(
            (inner.x - (r.x + 5.0)).abs() < 0.01
                && (inner.y - (r.y + 5.0)).abs() < 0.01,
            "the grandchild must travel with its box: expected ({}, {}), got \
             ({}, {})",
            r.x + 5.0,
            r.y + 5.0,
            inner.x,
            inner.y
        );
    }

}

// ── ported from hiwave-windows flex.rs (L1-WINDOWS-A #72 and the auto-basis /
//    stretch / wrap pins). `layout_flex_container` has the same signature on
//    both trees, so these are verbatim. ──
#[cfg(test)]
mod windows_flex_pins {
    use super::*;
    use crate::BoxType;
    use rustkit_css::{ComputedStyle, FlexDirection, FlexWrap, Length};


    // NOT ported: test_auto_basis_uses_pre_pass_measurement pinned the old
    // Windows flex model that read an item's pre-pass content rect as its
    // auto basis; this tree measures max-content (#184, #202), which the
    // sibling test_auto_basis_uses_max_content_not_block_width pins.

    #[test]
    fn test_positions_land_in_absolute_frame() {
        // Container content origin at (50, 70): first item must be placed at
        // that origin, not at (0, 0) — flex output shares the tree's frame.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(100.0);
        child_style.height = Length::Px(40.0);
        container.children.push(LayoutBox::new(BoxType::Block, child_style));

        let containing = Dimensions {
            content: Rect::new(50.0, 70.0, 400.0, 300.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        assert_eq!(container.children[0].dimensions.content.x, 50.0);
        assert_eq!(container.children[0].dimensions.content.y, 70.0);
    }

    #[test]
    fn test_item_subtree_relaid_after_flex() {
        // A block flex item's own children must be laid out against the
        // item's FINAL rect (step 11) — not left with stale/zero geometry.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item_style = ComputedStyle::new();
        item_style.width = Length::Px(300.0);
        item_style.height = Length::Px(200.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);

        let mut grandchild_style = ComputedStyle::new();
        grandchild_style.width = Length::Auto; // cascade default (::new() is Zero)
        grandchild_style.height = Length::Px(50.0);
        item.children.push(LayoutBox::new(BoxType::Block, grandchild_style));
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(10.0, 20.0, 800.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let item_rect = container.children[0].dimensions.content;
        let gc = container.children[0].children[0].dimensions.content;
        // Grandchild starts at the item's content top (normal flow), not its
        // bottom edge, not (0,0), and spans the item's width.
        assert_eq!(gc.y, item_rect.y);
        assert_eq!(gc.x, item_rect.x);
        assert_eq!(gc.height, 50.0);
        assert!(gc.width > 0.0);
    }

    #[test]
    fn test_column_item_width_not_corrupted_by_tall_children() {
        // Column-direction container (cross axis = horizontal): a 200px-wide
        // item whose children stack to 500px tall must KEEP width 200 —
        // 11b must not write the children's height-sum into content.width
        // (Atlas cross-seat review of PR #5).
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Column;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut item_style = ComputedStyle::new();
        item_style.width = Length::Px(200.0);
        item_style.height = Length::Px(500.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        for _ in 0..2 {
            let mut gc_style = ComputedStyle::new();
            gc_style.width = Length::Auto;
            gc_style.height = Length::Px(250.0);
            item.children.push(LayoutBox::new(BoxType::Block, gc_style));
        }
        container.children.push(item);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        assert_eq!(container.children[0].dimensions.content.width, 200.0);
        assert_eq!(container.children[0].dimensions.content.height, 500.0);
    }

    #[test]
    fn test_container_auto_height_updated_from_flex_extent() {
        // Row container with auto height: content height must reflect the
        // tallest line after flex, not the stale pre-pass value.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(100.0);
        child_style.height = Length::Px(120.0);
        container.children.push(LayoutBox::new(BoxType::Block, child_style));

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 0.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        assert_eq!(container.dimensions.content.height, 120.0);
    }

    #[test]
    fn test_wrap_lines_pack_tightly_in_auto_container() {
        // In an auto-height wrap container, wrapped lines must pack directly
        // under each other (align-content has no free space to distribute) — not
        // spread across the stale pre-flex stacked height, which pushed the
        // second row far below the container (card grid lost its 2nd row).
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        style.flex_wrap = FlexWrap::Wrap;
        let mut container = LayoutBox::new(BoxType::Block, style);

        // Four 300×100 items in a 650-wide row → two lines of two.
        for _ in 0..4 {
            let mut item_style = ComputedStyle::new();
            item_style.width = Length::Px(300.0);
            item_style.height = Length::Px(100.0);
            item_style.flex_basis = rustkit_css::FlexBasis::Length(300.0);
            let mut item = LayoutBox::new(BoxType::Block, item_style);
            item.dimensions.content = Rect::new(0.0, 0.0, 300.0, 100.0);
            container.children.push(item);
        }
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 650.0, 800.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let y0 = container.children[0].dimensions.content.y;
        let y2 = container.children[2].dimensions.content.y; // first item of line 2
        assert!(
            y2 - y0 < 160.0,
            "second wrap line should pack under the first (~100px), not be spread: dy={}",
            y2 - y0
        );
        assert!(y2 > y0, "second line must be below the first: y0={y0} y2={y2}");
    }

    #[test]
    fn test_stretch_equalizes_auto_height_row() {
        // An auto-height row with align-items:stretch (the default) must give
        // its children a common height equal to the tallest — the equal-height
        // card-grid behaviour. The stale stacked container height must NOT be
        // used as the stretch target.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        // align_items defaults to Stretch; container height stays Auto.
        let mut container = LayoutBox::new(BoxType::Block, style);

        // Two auto-height children with different measured content heights.
        let mut a = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        a.dimensions.content = Rect::new(0.0, 0.0, 100.0, 40.0);
        container.children.push(a);
        let mut b = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        b.dimensions.content = Rect::new(0.0, 0.0, 100.0, 90.0);
        container.children.push(b);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 300.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let h0 = container.children[0].dimensions.content.height;
        let h1 = container.children[1].dimensions.content.height;
        assert!(
            (h0 - h1).abs() < 0.5,
            "stretch should equalize heights: {h0} vs {h1}"
        );
        assert!(h0 >= 89.5, "should stretch to the taller child (90): {h0}");
    }

    #[test]
    fn test_auto_basis_uses_max_content_not_block_width() {
        // Two content-sized items in a wide row must stay content-sized (their
        // max-content), leaving free space — NOT inflate to the block full-width
        // the pre-pass stretched them to and then shrink to equal halves. With
        // the old behaviour each item used measured_main (~container width) as
        // its basis and landed at ~half the row (~400px).
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        let mut container = LayoutBox::new(BoxType::Block, style);

        for label in ["Hi", "Yo"] {
            let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
            // Simulate the normal-flow pre-pass stretching the block to the row.
            item.dimensions.content = Rect::new(0.0, 0.0, 700.0, 20.0);
            item.children
                .push(LayoutBox::new(BoxType::Text(label.to_string()), ComputedStyle::new()));
            container.children.push(item);
        }
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 300.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let w0 = container.children[0].dimensions.content.width;
        assert!(
            w0 < 200.0,
            "auto-basis flex item should be content-sized, not a fraction of the \
             row (block-width basis regression): got {w0}"
        );
    }

    #[test]
    fn test_explicit_height_child_not_stretched() {
        // A child with a definite cross size wins over align-items:stretch
        // (§9.4.11) — it keeps its own height while a stretchy sibling grows.
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        style.flex_direction = FlexDirection::Row;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let mut fixed_style = ComputedStyle::new();
        fixed_style.height = Length::Px(30.0);
        let fixed = LayoutBox::new(BoxType::Block, fixed_style);
        container.children.push(fixed);

        let mut tall = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        tall.dimensions.content = Rect::new(0.0, 0.0, 100.0, 90.0);
        container.children.push(tall);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 400.0, 300.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        let fixed_h = container.children[0].dimensions.content.height;
        assert!(
            (fixed_h - 30.0).abs() < 0.5,
            "definite-height child must not stretch: {fixed_h}"
        );
    }
}
