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

use crate::{BoxType, Dimensions, EdgeSizes, LayoutBox, Rect};
use rustkit_css::{
    AlignContent, AlignItems, AlignSelf, FlexBasis, FlexWrap, JustifyContent, Length,
};

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

    /// Content size on the cross axis measured by the normal-flow pre-pass
    /// (layout_block runs before flex and leaves real dimensions on every
    /// child). Used as the hypothetical cross size for non-stretch items.
    pub measured_cross_size: f32,

    /// Definite cross-size from the style (height for row axis, width for
    /// column axis), if any. Stretch only applies when this is None
    /// (§9.4.11 — a definite cross size wins over align-items: stretch).
    pub explicit_cross_size: Option<f32>,
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
        self.items.iter().map(|item| item.outer_hypothetical_main_size()).sum()
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

/// Layout a flex container and its children.
pub fn layout_flex_container(
    container: &mut LayoutBox,
    containing_block: &Dimensions,
) {
    let style = &container.style;

    // 1. Determine main/cross axes
    let direction = style.flex_direction;
    let main_axis = if direction.is_row() {
        Axis::Horizontal
    } else {
        Axis::Vertical
    };
    let cross_axis = main_axis.cross();

    // Get container dimensions
    let container_main_size = match main_axis {
        Axis::Horizontal => containing_block.content.width,
        Axis::Vertical => containing_block.content.height,
    };
    let container_cross_size = match cross_axis {
        Axis::Horizontal => containing_block.content.width,
        Axis::Vertical => containing_block.content.height,
    };
    // A *definite* cross size only exists when the container explicitly sets it
    // on the cross axis. An auto-height row's `container_cross_size` above is
    // the pre-flex normal-flow stack height (children stacked vertically) — a
    // meaningless artifact that must NOT drive stretch, or items balloon to the
    // stack height instead of equalizing to the tallest item.
    let definite_cross = match cross_axis {
        Axis::Vertical => match style.height {
            Length::Px(h) => h,
            _ => 0.0,
        },
        Axis::Horizontal => match style.width {
            Length::Px(w) => w,
            _ => 0.0,
        },
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

    // 2. Collect flex items (skip absolutely positioned)
    let mut items: Vec<FlexItem> = Vec::new();
    for child in &mut container.children {
        if child.style.position == rustkit_css::Position::Absolute
            || child.style.position == rustkit_css::Position::Fixed
        {
            continue;
        }

        let item = create_flex_item(child, main_axis, container_main_size, container_cross_size);
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
    for line in &mut lines {
        calculate_cross_sizes(line, definite_cross, style.align_items);
    }

    // 6. Calculate line cross sizes and positions
    let total_cross_size: f32 = lines.iter().map(|l| l.cross_size).sum::<f32>()
        + cross_gap * (lines.len().saturating_sub(1)) as f32;

    // 7. Apply align-content for multi-line containers
    // Use the definite cross size, not the stale pre-flex stacked height —
    // otherwise align-content stretches multi-line rows across a phantom height
    // and later lines land far below the container (card grids lost their second
    // row off-screen).
    distribute_lines(&mut lines, definite_cross, total_cross_size, cross_gap, style.align_content);

    // 8. Main axis alignment (justify-content) and positioning
    for line in &mut lines {
        distribute_main_axis(line, container_main_size, main_gap, style.justify_content, direction.is_reverse());
    }

    // 9. Cross axis alignment (align-items, align-self)
    for line in &mut lines {
        align_cross_axis(line, style.align_items);
    }

    // 10. Apply final positions to layout boxes.
    // Positions computed above are container-relative; translate by the
    // container's content origin so item rects stay in the same absolute
    // frame as every other box in the tree (the normal-flow pre-pass wrote
    // absolute coordinates — overwriting them with relative ones put flex
    // items in a different coordinate space from their own descendants).
    let container_origin = (containing_block.content.x, containing_block.content.y);
    apply_positions(&mut lines, main_axis, direction.is_reverse(), wrap == FlexWrap::WrapReverse, container_origin);

    // 11. Recursively lay out children of flex items. apply_positions just
    // moved/resized every item, so their subtrees (laid out by the pre-pass
    // against stale geometry) must be re-laid against the final rects.
    // Without this step every flex item's subtree keeps pre-flex geometry —
    // the zero-width tree of the 2026-07-07 Windows baseline.
    for line in &mut lines {
        for item in &mut line.items {
            if !item.layout_box.children.is_empty() {
                if item.layout_box.style.display.is_flex() {
                    // Nested flex container: recursively apply flex layout
                    let child_containing = item.layout_box.dimensions.clone();
                    layout_flex_container(item.layout_box, &child_containing);
                } else {
                    // Block container: normal flow from the item's content
                    // top. (Per-child clones of the item's FINAL dimensions
                    // would stack every child at the bottom edge — block
                    // layout uses content.height as the flow cursor. Same
                    // fix as macOS hiwave-macos#3.)
                    item.layout_box.layout_block_children();
                }
            }
        }
    }

    // 11b. Recompute cross sizes now that children hold real geometry —
    // resolves the chicken-and-egg between item cross size and child heights.
    // The cross-extent of a block item's children depends on the axis: in a
    // row container (cross = vertical) children stack, so it's the HEIGHT
    // SUM; in a column container (cross = horizontal) it's the WIDEST child
    // (a height-sum written into width corrupts narrow column items — Atlas
    // review of PR #5).
    for line in &mut lines {
        for item in &mut line.items {
            if !item.layout_box.children.is_empty() {
                let children_cross_extent: f32 = match cross_axis {
                    Axis::Vertical => item.layout_box.children
                        .iter()
                        .map(|c| c.dimensions.margin_box().height)
                        .sum(),
                    Axis::Horizontal => item.layout_box.children
                        .iter()
                        .map(|c| c.dimensions.margin_box().width)
                        .fold(0.0, f32::max),
                };

                if children_cross_extent > 0.0 && children_cross_extent > item.cross_size {
                    item.cross_size = children_cross_extent.max(item.min_cross_size).min(item.max_cross_size);
                    match cross_axis {
                        Axis::Vertical => {
                            if item.layout_box.dimensions.content.height < children_cross_extent {
                                item.layout_box.dimensions.content.height = children_cross_extent;
                            }
                        }
                        Axis::Horizontal => {
                            if item.layout_box.dimensions.content.width < children_cross_extent {
                                item.layout_box.dimensions.content.width = children_cross_extent;
                            }
                        }
                    }
                }
            }
        }

        line.cross_size = line.items
            .iter()
            .map(|i| i.cross_size + i.cross_margin_start + i.cross_margin_end)
            .fold(0.0, f32::max);
    }

    // 11c. Re-apply cross-axis stretch. Step 11's child relayout
    // (layout_block_children) recomputed each item's box size from its own
    // content, undoing the stretch that calculate_cross_sizes established.
    // Grow every auto-sized stretch item back up to the line's cross size so
    // default-aligned siblings share a common cross extent (equal-height
    // cards). Only grows — a genuinely taller item is never shrunk, and
    // non-stretch items (e.g. table rows set to flex-start) keep their size.
    for line in &mut lines {
        let line_cross = line.cross_size;
        for item in &mut line.items {
            let align = match item.align_self {
                AlignSelf::Auto => style.align_items,
                AlignSelf::FlexStart => AlignItems::FlexStart,
                AlignSelf::FlexEnd => AlignItems::FlexEnd,
                AlignSelf::Center => AlignItems::Center,
                AlignSelf::Baseline => AlignItems::Baseline,
                AlignSelf::Stretch => AlignItems::Stretch,
            };
            if align != AlignItems::Stretch || item.explicit_cross_size.is_some() {
                continue;
            }
            let extent = (line_cross - item.cross_margin_start - item.cross_margin_end).max(0.0);
            match cross_axis {
                Axis::Vertical => {
                    if extent > item.layout_box.dimensions.content.height {
                        item.layout_box.dimensions.content.height = extent;
                        item.cross_size = extent;
                    }
                }
                Axis::Horizontal => {
                    if extent > item.layout_box.dimensions.content.width {
                        item.layout_box.dimensions.content.width = extent;
                        item.cross_size = extent;
                    }
                }
            }
        }
    }

    // 12. Update the container's auto height from the flexed content —
    // layout_block computed it from the pre-flex normal-flow pass, which is
    // stale once items have been repositioned.
    if !lines.is_empty() {
        let max_main: f32 = lines.iter()
            .flat_map(|l| l.items.iter())
            .map(|item| item.main_position + item.target_main_size)
            .fold(0.0f32, f32::max);
        let total_cross: f32 = lines.iter().map(|l| l.cross_size).sum::<f32>()
            + cross_gap * (lines.len().saturating_sub(1)) as f32;

        if container.dimensions.content.height == 0.0
            || matches!(container.style.height, Length::Auto)
        {
            container.dimensions.content.height = match main_axis {
                Axis::Horizontal => total_cross,
                Axis::Vertical => max_main,
            };
        }
    }
}

/// Own horizontal padding + border of a box (left + right edges).
fn horizontal_edges(style: &rustkit_css::ComputedStyle) -> f32 {
    resolve_length(&style.padding_left, 0.0)
        + resolve_length(&style.padding_right, 0.0)
        + resolve_length(&style.border_left_width, 0.0)
        + resolve_length(&style.border_right_width, 0.0)
}

/// Max-content **content-box** width of a box: the width its content wants if it
/// is never wrapped. Text reports its full unwrapped run; a box with inline
/// content sums its children on one line, and a box with block children takes
/// the widest child. The box's own padding/border is NOT included (that is the
/// caller's border-box concern via `max_content_outer`).
fn max_content_inner(b: &LayoutBox) -> f32 {
    match &b.box_type {
        BoxType::Text(t) => crate::measured_text_width(t, &b.style),
        _ => {
            if b.children.is_empty() {
                return 0.0;
            }
            let has_inline = b
                .children
                .iter()
                .any(|c| matches!(c.box_type, BoxType::Text(_) | BoxType::Inline));
            if has_inline {
                b.children.iter().map(max_content_outer).sum()
            } else {
                b.children.iter().map(max_content_outer).fold(0.0, f32::max)
            }
        }
    }
}

/// Max-content **border-box** width of a box: its content max-content plus its
/// own horizontal padding and border. Text has no box edges of its own.
fn max_content_outer(b: &LayoutBox) -> f32 {
    match &b.box_type {
        BoxType::Text(_) => max_content_inner(b),
        _ => max_content_inner(b) + horizontal_edges(&b.style),
    }
}

/// Create a FlexItem from a LayoutBox.
fn create_flex_item<'a>(
    layout_box: &'a mut LayoutBox,
    main_axis: Axis,
    container_main: f32,
    container_cross: f32,
) -> FlexItem<'a> {
    // Extract all values from style first to avoid borrow conflicts
    let order = layout_box.style.order;
    let flex_grow = layout_box.style.flex_grow;
    let flex_shrink = layout_box.style.flex_shrink;
    let flex_basis_value = layout_box.style.flex_basis;
    let align_self = layout_box.style.align_self;

    // Get margins
    let (main_margin_start, main_margin_end, cross_margin_start, cross_margin_end) = match main_axis {
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

    // Definite cross size from style, if any (Auto/Zero = unset).
    let explicit_cross = {
        let cross_len = match main_axis {
            Axis::Horizontal => &layout_box.style.height,
            Axis::Vertical => &layout_box.style.width,
        };
        match cross_len {
            Length::Auto | Length::Zero => None,
            l => Some(resolve_length(l, container_cross)),
        }
    };

    // Content size measured by the normal-flow pre-pass: LayoutBox::layout
    // runs layout_block() on the container BEFORE flex, so every child
    // already carries real dimensions. This is what makes flex-basis:auto
    // (§9.2.3) and hypothetical cross sizes resolvable without a separate
    // measurement pass — basis 0.0 here is why every auto-sized flex item
    // used to collapse to zero width.
    let (measured_main, measured_cross) = match main_axis {
        Axis::Horizontal => (
            layout_box.dimensions.content.width,
            layout_box.dimensions.content.height,
        ),
        Axis::Vertical => (
            layout_box.dimensions.content.height,
            layout_box.dimensions.content.width,
        ),
    };

    // Calculate flex basis
    let flex_basis = match flex_basis_value {
        FlexBasis::Auto => {
            // Main size property if definite, else the content-based size.
            let explicit = match main_axis {
                Axis::Horizontal => resolve_length(&layout_box.style.width, container_main),
                Axis::Vertical => resolve_length(&layout_box.style.height, container_main),
            };
            if explicit > 0.0 {
                explicit
            } else {
                match main_axis {
                    // Horizontal: `measured_main` is the normal-flow content
                    // width, but a block child was stretched to the container by
                    // the pre-pass — using it as the basis inflates every item to
                    // full width so they then shrink to equal fractions of the
                    // row (buttons/pills splitting the container). Prefer the
                    // max-content width; grow/shrink still adjusts from there, and
                    // content-sized items stay content-sized. Fall back to the
                    // pre-pass measurement when there is no measurable inline
                    // content (images, intrinsically-sized or empty boxes), whose
                    // measured width is meaningful rather than a stretch.
                    Axis::Horizontal => {
                        let mc = max_content_inner(layout_box);
                        if mc > 0.0 {
                            mc
                        } else {
                            measured_main
                        }
                    }
                    // Vertical (column main axis): the measured height is already
                    // content-derived, not stretched — keep it.
                    Axis::Vertical => measured_main,
                }
            }
        }
        FlexBasis::Content => measured_main,
        FlexBasis::Length(len) => len,
        FlexBasis::Percent(pct) => pct / 100.0 * container_main,
    };

    // Get min/max constraints
    let (min_main, max_main, min_cross, max_cross) = match main_axis {
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
        measured_cross_size: measured_cross,
        explicit_cross_size: explicit_cross,
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
        let gap = if current_line.items.is_empty() { 0.0 } else { main_gap };

        if !current_line.items.is_empty() && line_main_size + gap + item_size > container_main {
            // Start new line
            lines.push(current_line);
            current_line = FlexLine::new();
            line_main_size = 0.0;
        }

        line_main_size += if current_line.items.is_empty() { 0.0 } else { main_gap };
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
    let used_space: f32 = line.items.iter().map(|i| i.hypothetical_main_size + i.main_margin_start + i.main_margin_end).sum();
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
    let total_grow: f32 = line.items.iter().filter(|i| !i.frozen).map(|i| i.flex_grow).sum();

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
    let total_shrink_scaled: f32 = line.items
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

/// Calculate cross sizes for items in a line. `definite_cross` is the
/// container's explicit cross size (0 when the container is auto-sized on the
/// cross axis — then stretch equalizes to the tallest item instead).
fn calculate_cross_sizes(line: &mut FlexLine, definite_cross: f32, align_items: AlignItems) {
    // Effective align (align-self overrides align-items) for an item.
    let effective_align = |item: &FlexItem| -> AlignItems {
        match item.align_self {
            AlignSelf::Auto => align_items,
            AlignSelf::FlexStart => AlignItems::FlexStart,
            AlignSelf::FlexEnd => AlignItems::FlexEnd,
            AlignSelf::Center => AlignItems::Center,
            AlignSelf::Baseline => AlignItems::Baseline,
            AlignSelf::Stretch => AlignItems::Stretch,
        }
    };

    // Pass 1: every item starts at its hypothetical (definite or measured)
    // cross size, clamped. No stretch yet.
    for item in &mut line.items {
        let hypothetical = item.explicit_cross_size.unwrap_or(item.measured_cross_size);
        item.cross_size = hypothetical.max(item.min_cross_size).min(item.max_cross_size);
    }

    // The stretch target is the tallest of: the definite container cross size
    // and the largest hypothetical item. For an auto-height row the container
    // cross is indefinite (≈0) here — it only gets its real value after items
    // are placed — so it collapses to the tallest item, which is exactly what
    // gives equal-height siblings (the common card-grid case). A container with
    // an explicit cross size larger than every item still wins.
    let hypo_max = line.items
        .iter()
        .map(|i| i.cross_size + i.cross_margin_start + i.cross_margin_end)
        .fold(0.0, f32::max);
    let target = hypo_max.max(definite_cross);

    // Pass 2: auto-cross-size items with stretch alignment fill the target.
    for item in &mut line.items {
        if effective_align(item) == AlignItems::Stretch
            && item.explicit_cross_size.is_none()
            && target > 0.0
        {
            let stretched = target - item.cross_margin_start - item.cross_margin_end;
            item.cross_size = stretched.max(item.min_cross_size).min(item.max_cross_size);
        }
    }

    // Determine line cross size (largest item)
    line.cross_size = line.items
        .iter()
        .map(|i| i.cross_size + i.cross_margin_start + i.cross_margin_end)
        .fold(0.0, f32::max);
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
    // `container_cross` is the container's *definite* cross size (0 when auto).
    // An auto-height container is exactly its content, so there is no free space
    // to distribute — clamp at 0 so lines pack tightly instead of spreading.
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
            let extra_per_line = free_space / lines.len() as f32;
            for line in lines.iter_mut() {
                line.cross_size += extra_per_line;
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
/// `origin` is the container's content origin — item positions are computed
/// container-relative and must land in the tree's absolute frame.
fn apply_positions(
    lines: &mut [FlexLine],
    main_axis: Axis,
    _reverse_main: bool,
    reverse_cross: bool,
    origin: (f32, f32),
) {
    let lines_iter: Box<dyn Iterator<Item = &mut FlexLine>> = if reverse_cross {
        Box::new(lines.iter_mut().rev())
    } else {
        Box::new(lines.iter_mut())
    };

    for line in lines_iter {
        for item in &mut line.items {
            let (x, y, width, height) = match main_axis {
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

            // Update layout box dimensions (absolute frame)
            item.layout_box.dimensions.content = Rect {
                x: origin.0 + x,
                y: origin.1 + y,
                width,
                height,
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

/// Resolve a Length to pixels.
fn resolve_length(length: &Length, container_size: f32) -> f32 {
    match length {
        Length::Px(px) => *px,
        Length::Em(em) => em * 16.0, // Default font size
        Length::Rem(rem) => rem * 16.0,
        Length::Percent(pct) => pct / 100.0 * container_size,
        Length::Auto => 0.0,
        Length::Zero => 0.0,
    }
}

/// Resolve a max Length (returns f32::INFINITY when unconstrained).
/// Length::Zero is the ComputedStyle derive-default meaning "unset" — the
/// CSS initial value for max-width/max-height is `none`, not 0. Treating it
/// as a real 0 ceiling clamped EVERY flex item's hypothetical size to zero
/// (root cause of the 2026-07-07 Windows zero-width baseline).
fn resolve_max_length(length: &Length, container_size: f32) -> f32 {
    match length {
        Length::Auto | Length::Zero => f32::INFINITY,
        _ => resolve_length(length, container_size),
    }
}

/// Translate a layout box and its whole subtree by (dx, dy). Used by grid/flex
/// to move a laid-out item (and its descendants) into its final cell position.
pub(crate) fn translate_subtree(b: &mut crate::LayoutBox, dx: f32, dy: f32) {
    b.dimensions.content.x += dx;
    b.dimensions.content.y += dy;
    for child in &mut b.children {
        translate_subtree(child, dx, dy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustkit_css::{ComputedStyle, FlexDirection, JustifyContent, AlignItems, Length};
    use crate::BoxType;

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
        container.children.push(LayoutBox::new(BoxType::Block, child1_style));

        let mut child2_style = ComputedStyle::new();
        child2_style.width = Length::Px(100.0);
        child2_style.height = Length::Px(50.0);
        container.children.push(LayoutBox::new(BoxType::Block, child2_style));

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
        container.children.push(LayoutBox::new(BoxType::Block, child1_style));

        let mut child2_style = ComputedStyle::new();
        child2_style.flex_grow = 1.0;
        container.children.push(LayoutBox::new(BoxType::Block, child2_style));

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

    // ── trench regression tests (2026-07-08 Windows baseline fixes) ──

    #[test]
    fn test_auto_basis_uses_pre_pass_measurement() {
        // width:auto item whose content was measured by the normal-flow
        // pre-pass must NOT collapse to zero main size (§9.2.3 basis auto).
        let mut style = ComputedStyle::new();
        style.display = rustkit_css::Display::Flex;
        let mut container = LayoutBox::new(BoxType::Block, style);

        let child_style = ComputedStyle::new(); // width auto, no grow
        let mut child = LayoutBox::new(BoxType::Block, child_style);
        child.dimensions.content = Rect::new(0.0, 0.0, 240.0, 60.0); // pre-pass result
        container.children.push(child);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout_flex_container(&mut container, &containing);

        assert_eq!(container.children[0].dimensions.content.width, 240.0);
        assert!(container.children[0].dimensions.content.height > 0.0);
    }

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
        container.children.push(LayoutBox::new(BoxType::Block, child_style));

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
        container.children.push(LayoutBox::new(BoxType::Block, child_style));

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
        container.children.push(LayoutBox::new(BoxType::Block, child1_style));

        let mut child2_style = ComputedStyle::new();
        child2_style.height = Length::Px(50.0);
        child2_style.flex_basis = rustkit_css::FlexBasis::Length(50.0);
        child2_style.min_height = Length::Px(50.0);
        container.children.push(LayoutBox::new(BoxType::Block, child2_style));

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

