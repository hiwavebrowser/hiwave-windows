//! CSS 2.1 chapter 17 table layout, first slice (#626).
//!
//! What is here:
//! - anonymous table objects (§17.2.1), as a box-tree pass
//!   ([`fixup_table_boxes`]) that is also re-run, idempotently, on every
//!   table before it lays out;
//! - the row/column grid with `colspan` / `rowspan` (§17.5), header groups
//!   first and footer groups last (§17.2);
//! - automatic column widths (§17.5.2.2, with CSS Tables 3 §3.9 for the
//!   distribution steps CSS 2.1 leaves open);
//! - row heights, cell stretching and `vertical-align` in cells (§17.5.3);
//! - `border-spacing` in the separated borders model (§17.6.1);
//! - the table as a block-level box: shrink-to-fit width, `margin: auto`,
//!   floats, captions above the grid, and its intrinsic widths for flex
//!   items, floats and inline-blocks.
//!
//! What is not (later slices): `table-layout: fixed`, the collapsing border
//! model (approximated by zero spacing, each cell keeping its own borders),
//! percentage column widths (treated as `auto`), `caption-side: bottom`,
//! `empty-cells`, and baseline alignment (falls back to `top`).
//!
//! The table box is ONE `LayoutBox`: CSS 2.1's table wrapper box and table
//! box share it. Captions are its children like rows are, laid out above the
//! table's border box inside its top margin area, so the margin box (what
//! the parent's flow and float placement read) holds caption and grid while
//! the border box (what paints the table's border and background) holds the
//! grid alone.

use crate::{BoxType, Dimensions, FloatContext, LayoutBox, MarginCollapseContext, Rect};
use rustkit_css::{BorderCollapse, BoxSizing, ComputedStyle, Display, Length, VerticalAlign};

/// `colspan` × `rowspan` of a cell, or `span` of a column (group).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableSpan {
    pub colspan: u32,
    pub rowspan: u32,
}

impl Default for TableSpan {
    fn default() -> Self {
        Self {
            colspan: 1,
            rowspan: 1,
        }
    }
}

/// HTML's limits on spans (HTML §4.9.11): `colspan` is clamped to 1000 and
/// `rowspan` to 65534; zero `colspan` is 1.
const MAX_COLSPAN: u32 = 1000;
const MAX_ROWSPAN: u32 = 65534;

// ===================================================================
// Anonymous table objects (CSS 2.1 §17.2.1)
// ===================================================================

/// The table role a box plays. Only element boxes take part: text, images,
/// form controls and line breaks are inline content whatever display their
/// style carries (a text box may hold a copy of its parent's style).
fn role(b: &LayoutBox) -> Display {
    match b.box_type {
        BoxType::Block | BoxType::AnonymousBlock => b.style.display,
        _ => Display::Inline,
    }
}

/// Collapsible whitespace only (CSS white space, not Unicode's: a no-break
/// space is content).
fn is_whitespace_text(b: &LayoutBox) -> bool {
    match &b.box_type {
        BoxType::Text(t) => t
            .chars()
            .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')),
        _ => false,
    }
}

/// A proper table child (§17.2.1): row group, row, caption, column or
/// column group.
fn is_proper_table_child(d: Display) -> bool {
    d.is_table_row_group()
        || matches!(
            d,
            Display::TableRow
                | Display::TableCaption
                | Display::TableColumn
                | Display::TableColumnGroup
        )
}

fn anonymous(parent: &LayoutBox, display: Display, children: Vec<LayoutBox>) -> LayoutBox {
    let mut style = ComputedStyle::inherit_from(&parent.style);
    style.display = display;
    let mut b = LayoutBox::new(BoxType::Block, style);
    b.viewport = parent.viewport;
    b.children = children;
    b
}

/// Generates the anonymous table objects CSS 2.1 §17.2.1 requires anywhere
/// under `root`, so that any mix of `table-*` displays forms proper tables:
///
/// 1. Columns lose their children, column groups their non-column children.
/// 2. Whitespace-only text directly in a table, row group or row is dropped.
/// 3. Missing children: non-rows in a table or row group are wrapped in an
///    anonymous row, non-cells in a row in an anonymous cell.
/// 4. Missing parents: a run of misparented cells gets an anonymous row; a
///    run of misparented rows, row groups, captions or columns gets an
///    anonymous table (`inline-table` inside an inline box). Whitespace
///    between the boxes of such a run goes with it and is dropped.
///
/// Idempotent: a fixed tree is left as it is.
pub fn fixup_table_boxes(root: &mut LayoutBox) {
    for child in &mut root.children {
        fixup_table_boxes(child);
    }
    fix_children(root);
}

/// [`fixup_table_boxes`] for `parent`'s own children only, for a builder
/// that fixes each box as it completes it (its children are already fixed).
pub fn fixup_table_children(parent: &mut LayoutBox) {
    fix_children(parent);
}

fn fix_children(parent: &mut LayoutBox) {
    let d = role(parent);
    if d == Display::TableColumn {
        parent.children.clear();
        return;
    }
    if d == Display::TableColumnGroup {
        parent.children.retain(|c| role(c) == Display::TableColumn);
        return;
    }
    if d.is_table() {
        wrap_runs(
            parent,
            |c| !is_proper_table_child(role(c)),
            Display::TableRow,
        );
    } else if d.is_table_row_group() {
        wrap_runs(parent, |c| role(c) != Display::TableRow, Display::TableRow);
    } else if d == Display::TableRow {
        wrap_runs(
            parent,
            |c| role(c) != Display::TableCell,
            Display::TableCell,
        );
    } else if parent_takes_misparented(parent) {
        wrap_misparented(parent);
    }
}

/// Every box other than the table containers themselves can hold
/// misparented table boxes; only element boxes have children to check.
fn parent_takes_misparented(parent: &LayoutBox) -> bool {
    matches!(
        parent.box_type,
        BoxType::Block | BoxType::AnonymousBlock | BoxType::Inline
    ) && parent.children.iter().any(|c| role(c).is_table_internal())
}

/// Inside a table, row group or row: drops whitespace-only text, then wraps
/// each run of children that `foreign` matches in an anonymous `wrapper`,
/// fixed in turn.
fn wrap_runs(parent: &mut LayoutBox, foreign: impl Fn(&LayoutBox) -> bool, wrapper: Display) {
    let children = std::mem::take(&mut parent.children);
    let mut out = Vec::with_capacity(children.len());
    let mut run: Vec<LayoutBox> = Vec::new();
    for c in children {
        if is_whitespace_text(&c) {
            continue;
        }
        if foreign(&c) {
            run.push(c);
            continue;
        }
        if !run.is_empty() {
            out.push(wrapped(parent, wrapper, std::mem::take(&mut run)));
        }
        out.push(c);
    }
    if !run.is_empty() {
        out.push(wrapped(parent, wrapper, run));
    }
    parent.children = out;
}

fn wrapped(parent: &LayoutBox, display: Display, children: Vec<LayoutBox>) -> LayoutBox {
    let mut b = anonymous(parent, display, children);
    fix_children(&mut b);
    b
}

/// Outside a table: each run of table-internal boxes (whitespace between
/// them included) goes into one anonymous table, which then generates its
/// own missing rows.
fn wrap_misparented(parent: &mut LayoutBox) {
    let table_display = if matches!(parent.box_type, BoxType::Inline) {
        Display::InlineTable
    } else {
        Display::Table
    };
    let children = std::mem::take(&mut parent.children);
    let mut out = Vec::with_capacity(children.len());
    let mut run: Vec<LayoutBox> = Vec::new();
    // Whitespace seen after the run's last table box: joins the run only if
    // another table box follows.
    let mut pending_ws: Vec<LayoutBox> = Vec::new();
    for c in children {
        if role(&c).is_table_internal() {
            run.append(&mut pending_ws);
            run.push(c);
        } else if !run.is_empty() && is_whitespace_text(&c) {
            pending_ws.push(c);
        } else {
            if !run.is_empty() {
                out.push(wrapped(parent, table_display, std::mem::take(&mut run)));
            }
            out.append(&mut pending_ws);
            out.push(c);
        }
    }
    if !run.is_empty() {
        out.push(wrapped(parent, table_display, run));
    }
    out.append(&mut pending_ws);
    parent.children = out;
}

// ===================================================================
// The grid (CSS 2.1 §17.5)
// ===================================================================

/// A cell's place in the grid and the path to its box from the table.
#[derive(Debug, Clone)]
struct CellSlot {
    path: Vec<usize>,
    row: usize,
    col: usize,
    colspan: usize,
    rowspan: usize,
}

#[derive(Debug, Default)]
struct Grid {
    /// Path from the table to each row box, in visual order.
    rows: Vec<Vec<usize>>,
    /// Path from the table to each row group, in visual order.
    groups: Vec<usize>,
    cells: Vec<CellSlot>,
    cols: usize,
}

/// Builds the grid. Row order is the first header group, then every other
/// row and row group in tree order, then the first footer group (CSS 2.1
/// §17.2: `table-header-group` / `table-footer-group`). Rows directly in the
/// table form implicit groups between the explicit ones; a `rowspan` never
/// reaches past its group's last row, and `rowspan="0"` spans to it.
fn build_grid(t: &LayoutBox) -> Grid {
    let header = t
        .children
        .iter()
        .position(|c| role(c) == Display::TableHeaderGroup);
    let footer = t
        .children
        .iter()
        .position(|c| role(c) == Display::TableFooterGroup);
    let mut order: Vec<usize> = Vec::new();
    order.extend(header);
    for (i, c) in t.children.iter().enumerate() {
        let d = role(c);
        if Some(i) != header
            && Some(i) != footer
            && (d.is_table_row_group() || d == Display::TableRow)
        {
            order.push(i);
        }
    }
    order.extend(footer);

    // Groups of row paths: each explicit group, and each run of bare rows.
    let mut grid = Grid::default();
    let mut row_groups: Vec<Vec<Vec<usize>>> = Vec::new();
    let mut bare_run: Vec<Vec<usize>> = Vec::new();
    for &i in &order {
        let c = &t.children[i];
        if role(c) == Display::TableRow {
            bare_run.push(vec![i]);
            continue;
        }
        if !bare_run.is_empty() {
            row_groups.push(std::mem::take(&mut bare_run));
        }
        grid.groups.push(i);
        row_groups.push(
            c.children
                .iter()
                .enumerate()
                .filter(|(_, r)| role(r) == Display::TableRow)
                .map(|(j, _)| vec![i, j])
                .collect(),
        );
    }
    if !bare_run.is_empty() {
        row_groups.push(bare_run);
    }

    // Slot assignment (HTML's "forming a table", as browsers do it): each
    // cell takes the first column at or after the cursor not covered by a
    // rowspan from above.
    let mut occupied: Vec<Vec<bool>> = Vec::new();
    for group in row_groups {
        let group_start = grid.rows.len();
        let group_end = group_start + group.len();
        for row_path in group {
            let r = grid.rows.len();
            if occupied.len() <= r {
                occupied.resize(r + 1, Vec::new());
            }
            let row_box = at(t, &row_path);
            let mut col = 0usize;
            for (k, cell) in row_box.children.iter().enumerate() {
                if role(cell) != Display::TableCell {
                    continue;
                }
                while occupied[r].get(col).copied().unwrap_or(false) {
                    col += 1;
                }
                let colspan = cell.table_span.colspan.clamp(1, MAX_COLSPAN) as usize;
                let rowspan = match cell.table_span.rowspan.min(MAX_ROWSPAN) {
                    0 => group_end - r,
                    n => (n as usize).min(group_end - r),
                };
                for rr in r..r + rowspan {
                    if occupied.len() <= rr {
                        occupied.resize(rr + 1, Vec::new());
                    }
                    if occupied[rr].len() < col + colspan {
                        occupied[rr].resize(col + colspan, false);
                    }
                    for slot in &mut occupied[rr][col..col + colspan] {
                        *slot = true;
                    }
                }
                let mut path = row_path.clone();
                path.push(k);
                grid.cells.push(CellSlot {
                    path,
                    row: r,
                    col,
                    colspan,
                    rowspan,
                });
                grid.cols = grid.cols.max(col + colspan);
                col += colspan;
            }
            grid.rows.push(row_path);
        }
    }
    grid
}

fn at<'a>(b: &'a LayoutBox, path: &[usize]) -> &'a LayoutBox {
    path.iter().fold(b, |b, &i| &b.children[i])
}

fn at_mut<'a>(b: &'a mut LayoutBox, path: &[usize]) -> &'a mut LayoutBox {
    path.iter().fold(b, |b, &i| &mut b.children[i])
}

// ===================================================================
// Column widths: automatic layout (CSS 2.1 §17.5.2.2)
// ===================================================================

/// A length that resolves without a containing block, in px. Percentages
/// (and anything else needing a base) are `None`: percentage widths on
/// cells and columns are treated as `auto` in this slice.
fn absolute_px(l: &Length, style: &ComputedStyle) -> Option<f32> {
    let font = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };
    match l {
        Length::Px(v) => Some(*v),
        Length::Zero => Some(0.0),
        Length::Em(v) => Some(v * font),
        Length::Rem(v) => Some(v * 16.0),
        _ => None,
    }
}

fn horizontal_margins(style: &ComputedStyle) -> f32 {
    absolute_px(&style.margin_left, style).unwrap_or(0.0)
        + absolute_px(&style.margin_right, style).unwrap_or(0.0)
}

/// A specified `width`, as a border-box figure.
fn specified_border_box_width(style: &ComputedStyle) -> Option<f32> {
    let w = absolute_px(&style.width, style)?;
    Some(match style.box_sizing {
        BoxSizing::BorderBox => w,
        BoxSizing::ContentBox => w + crate::grid::horizontal_padding_border(style),
    })
}

/// The border spacing the table actually uses. `border-collapse: collapse`
/// is approximated in this slice by zero spacing (each cell keeps its own
/// borders; nothing is collapsed).
fn spacing(style: &ComputedStyle) -> (f32, f32) {
    match style.border_collapse {
        BorderCollapse::Collapse => (0.0, 0.0),
        BorderCollapse::Separate => (
            style.border_spacing.0.max(0.0),
            style.border_spacing.1.max(0.0),
        ),
    }
}

/// A cell's (min-content, max-content) border-box widths, and its specified
/// border-box width. A specified width is the cell's preferred width, and a
/// minimum unless its content cannot fit (CSS 2.1 §17.5.2.2 step 1).
fn cell_widths(cell: &LayoutBox) -> (f32, f32, Option<f32>) {
    match specified_border_box_width(&cell.style) {
        Some(w) => {
            // `own_min_content_width` answers the specified width itself;
            // the content's minimum is read from the children instead.
            let pb = crate::grid::horizontal_padding_border(&cell.style);
            let content_min = cell
                .children
                .iter()
                .map(|c| crate::grid::estimate_min_content_width(c) + horizontal_margins(&c.style))
                .fold(0.0f32, f32::max)
                + pb;
            let min = content_min.max(w);
            (min, min, Some(w))
        }
        None => {
            let min = crate::grid::own_min_content_width(cell);
            let max = crate::grid::own_max_content_width(cell).max(min);
            (min, max, None)
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Column {
    min: f32,
    max: f32,
    /// Has a specified width (from a cell or a `<col>`).
    fixed: bool,
}

/// `(span, width)` of each column the table's `<col>` / `<colgroup>`
/// children describe, in order. A column without a width of its own takes
/// its group's.
fn column_widths_from_cols(t: &LayoutBox) -> Vec<Option<f32>> {
    let mut out = Vec::new();
    let mut push = |b: &LayoutBox, inherited: Option<f32>| {
        let w = specified_border_box_width(&b.style).or(inherited);
        for _ in 0..b.table_span.colspan.clamp(1, MAX_COLSPAN) {
            out.push(w);
        }
    };
    for c in &t.children {
        match role(c) {
            Display::TableColumn => push(c, None),
            Display::TableColumnGroup => {
                let group_w = specified_border_box_width(&c.style);
                if c.children.is_empty() {
                    push(c, None);
                } else {
                    for col in &c.children {
                        push(col, group_w);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Spreads `extra` over `cols` in proportion to `weight`, or equally when
/// every weight is zero.
fn distribute(widths: &mut [f32], cols: &[usize], weight: impl Fn(usize) -> f32, extra: f32) {
    if cols.is_empty() || extra <= 0.0 {
        return;
    }
    let total: f32 = cols.iter().map(|&c| weight(c)).sum();
    for &c in cols {
        widths[c] += if total > 0.0 {
            extra * weight(c) / total
        } else {
            extra / cols.len() as f32
        };
    }
}

/// Column minimum and maximum widths (§17.5.2.2 steps 1-2): single-column
/// cells set them; a column with a specified width (cell or `<col>`) has
/// that as its maximum, floored at its minimum; then spanning cells, fewest
/// columns first, widen what they span in proportion to the columns'
/// maximum widths (CSS Tables 3 §3.9.2's distribution).
fn measure_columns(t: &LayoutBox, grid: &Grid, hs: f32) -> Vec<Column> {
    let mut cols = vec![Column::default(); grid.cols];
    let mut fixed_w = vec![None::<f32>; grid.cols];
    for (c, w) in column_widths_from_cols(t).into_iter().enumerate() {
        if c < grid.cols && w.is_some() {
            fixed_w[c] = w;
        }
    }
    let measured: Vec<(f32, f32, Option<f32>)> = grid
        .cells
        .iter()
        .map(|s| cell_widths(at(t, &s.path)))
        .collect();
    for (slot, &(min, max, w)) in grid.cells.iter().zip(&measured) {
        if slot.colspan != 1 {
            continue;
        }
        let col = &mut cols[slot.col];
        col.min = col.min.max(min);
        col.max = col.max.max(max);
        if let Some(w) = w {
            fixed_w[slot.col] = Some(fixed_w[slot.col].unwrap_or(0.0).max(w));
        }
    }
    for (col, w) in cols.iter_mut().zip(&fixed_w) {
        match w {
            Some(w) => {
                col.fixed = true;
                col.max = w.max(col.min);
            }
            None => col.max = col.max.max(col.min),
        }
    }
    let mut spanning: Vec<usize> = (0..grid.cells.len())
        .filter(|&i| grid.cells[i].colspan > 1)
        .collect();
    spanning.sort_by_key(|&i| grid.cells[i].colspan);
    for i in spanning {
        let slot = &grid.cells[i];
        let (min, max, _) = measured[i];
        let range: Vec<usize> = (slot.col..slot.col + slot.colspan).collect();
        let gaps = hs * (slot.colspan - 1) as f32;
        let maxes: Vec<f32> = cols.iter().map(|c| c.max).collect();
        let span_min: f32 = range.iter().map(|&c| cols[c].min).sum::<f32>() + gaps;
        let mut mins: Vec<f32> = cols.iter().map(|c| c.min).collect();
        distribute(&mut mins, &range, |c| maxes[c], min - span_min);
        let span_max: f32 = range.iter().map(|&c| cols[c].max).sum::<f32>() + gaps;
        let mut new_maxes = maxes.clone();
        distribute(&mut new_maxes, &range, |c| maxes[c], max - span_max);
        for &c in &range {
            cols[c].min = mins[c];
            cols[c].max = new_maxes[c].max(mins[c]);
        }
    }
    cols
}

/// Final column widths for `grid_width` px of columns (the table's content
/// width less its spacing). Between the min and max sums the surplus over
/// the minimums goes in proportion to (max - min); past the max sum the
/// extra goes to the auto columns in proportion to their max (to every
/// column when all are fixed), or equally when those are all zero.
fn resolve_columns(cols: &[Column], grid_width: f32) -> Vec<f32> {
    let min_sum: f32 = cols.iter().map(|c| c.min).sum();
    let max_sum: f32 = cols.iter().map(|c| c.max).sum();
    if grid_width <= min_sum {
        return cols.iter().map(|c| c.min).collect();
    }
    if grid_width <= max_sum {
        let room = max_sum - min_sum;
        let f = if room > 0.0 {
            (grid_width - min_sum) / room
        } else {
            0.0
        };
        return cols.iter().map(|c| c.min + (c.max - c.min) * f).collect();
    }
    let mut widths: Vec<f32> = cols.iter().map(|c| c.max).collect();
    let auto: Vec<usize> = (0..cols.len()).filter(|&c| !cols[c].fixed).collect();
    let targets: Vec<usize> = if auto.is_empty() {
        (0..cols.len()).collect()
    } else {
        auto
    };
    distribute(&mut widths, &targets, |c| cols[c].max, grid_width - max_sum);
    widths
}

/// Total spacing across `n` tracks: one gap before, between and after them.
fn track_spacing(n: usize, gap: f32) -> f32 {
    if n == 0 {
        0.0
    } else {
        gap * (n + 1) as f32
    }
}

/// The table's (min-content, max-content) border-box widths. A specified
/// width (absolute units only; a percentage is auto here) is used as both,
/// floored at the min-content width. Captions' min-content widths floor
/// the table's.
pub(crate) fn table_intrinsic_widths(t: &LayoutBox) -> (f32, f32) {
    let mut fixed;
    let t = if needs_fixup(t) {
        fixed = t.clone();
        fixup_table_boxes(&mut fixed);
        &fixed
    } else {
        t
    };
    let (hs, _) = spacing(&t.style);
    let grid = build_grid(t);
    let cols = measure_columns(t, &grid, hs);
    let pb = crate::grid::horizontal_padding_border(&t.style);
    let extra = pb + track_spacing(grid.cols, hs);
    let caption_min = captions(t)
        .map(|c| crate::grid::own_min_content_width(c) + horizontal_margins(&c.style))
        .fold(0.0f32, f32::max);
    let min = (cols.iter().map(|c| c.min).sum::<f32>() + extra).max(caption_min);
    let max = (cols.iter().map(|c| c.max).sum::<f32>() + extra).max(min);
    match specified_border_box_width(&t.style) {
        Some(w) => (w.max(min), w.max(min)),
        None => (min, max),
    }
}

fn needs_fixup(t: &LayoutBox) -> bool {
    t.children.iter().any(|c| {
        !is_proper_table_child(role(c))
            || (role(c) == Display::TableRow
                && c.children.iter().any(|k| role(k) != Display::TableCell))
            || (role(c).is_table_row_group()
                && c.children.iter().any(|k| role(k) != Display::TableRow))
    })
}

fn captions(t: &LayoutBox) -> impl Iterator<Item = &LayoutBox> {
    t.children
        .iter()
        .filter(|c| role(c) == Display::TableCaption)
}

// ===================================================================
// Layout
// ===================================================================

/// Lays out a table's contents: captions above its border box, then the
/// grid inside its content box. Entered from `layout_block_children` and
/// `layout_block_children_with_collapse`, so every path that lays out a
/// block's children (in flow, floats, flex items, inline-tables) reaches it
/// with the table's own box already sized and placed by the block code:
/// margins (collapsing with its siblings'; a table is a BFC root, so nothing
/// collapses through it), border, padding, and a width that is
/// shrink-to-fit when `auto` (see `calculate_block_width`).
///
/// Here the width is floored at the table's min-content width (§17.5.2.2:
/// a table is never narrower than its columns need), and the content height
/// is set to the grid's; `calculate_block_height` then treats a specified
/// height as a minimum.
pub(crate) fn layout_table_contents(t: &mut LayoutBox) {
    fixup_table_boxes(t);
    // A pass that re-lays out the contents without recomputing the box
    // (flex relayout) finds the caption shift of the previous pass still
    // applied: undo it first.
    if let Some((margin_top, shift)) = t.table_caption_shift.take() {
        if t.dimensions.margin.top == margin_top {
            t.dimensions.margin.top -= shift;
            t.dimensions.content.y -= shift;
        }
    }
    let style = t.style.clone();
    let d = &t.dimensions;
    let pb_h = d.border.left + d.border.right + d.padding.left + d.padding.right;
    let pb_v = d.border.top + d.border.bottom + d.padding.top + d.padding.bottom;

    // Column widths (§17.5.2.2, CSS Tables 3 §3.9.3). The used width came in
    // from the block code: max-content clamped to the room there is, or the
    // specified width; never below the min-content width.
    let (hs, vs) = spacing(&style);
    let grid = build_grid(t);
    let cols = measure_columns(t, &grid, hs);
    let spacing_h = track_spacing(grid.cols, hs);
    let caption_min = captions(t)
        .map(|c| crate::grid::own_min_content_width(c) + horizontal_margins(&c.style))
        .fold(0.0f32, f32::max);
    let table_min = (cols.iter().map(|c| c.min).sum::<f32>() + spacing_h + pb_h).max(caption_min);
    let used = (d.content.width + pb_h).max(table_min);
    let content_width = used - pb_h;
    let widths = resolve_columns(&cols, (content_width - spacing_h).max(0.0));
    t.dimensions.content.width = content_width;

    // Captions sit above the border box, as wide as it, inside the top
    // margin area (CSS 2.1 §17.4: the table wrapper box holds caption and
    // table box; here one box plays both).
    let border_box = t.dimensions.border_box();
    let mut caption_height = 0.0;
    for c in t
        .children
        .iter_mut()
        .filter(|c| role(c) == Display::TableCaption)
    {
        let cb = Dimensions {
            content: Rect::new(border_box.x, border_box.y + caption_height, used, 0.0),
            ..Default::default()
        };
        c.layout_with_collapse_in(
            &cb,
            &mut formatting_root_context(),
            &mut FloatContext::new(),
            None,
        );
        caption_height = c.dimensions.margin_box().bottom() - border_box.y;
    }
    if caption_height > 0.0 {
        t.dimensions.margin.top += caption_height;
        t.dimensions.content.y += caption_height;
        t.table_caption_shift = Some((t.dimensions.margin.top, caption_height));
    }

    let specified_height = absolute_px(&style.height, &style).map(|h| match style.box_sizing {
        BoxSizing::BorderBox => (h - pb_v).max(0.0),
        BoxSizing::ContentBox => h,
    });
    layout_grid(t, &grid, &widths, (hs, vs), specified_height);
}

/// A context for laying out a formatting root's contents in isolation.
fn formatting_root_context() -> MarginCollapseContext {
    let mut ctx = MarginCollapseContext::new();
    ctx.children_are_formatting_roots = true;
    ctx
}

/// Places the rows and cells inside the table's content box, sizing rows
/// from their cells (§17.5.3), and sets the table's content height.
fn layout_grid(
    t: &mut LayoutBox,
    grid: &Grid,
    widths: &[f32],
    (hs, vs): (f32, f32),
    specified_height: Option<f32>,
) {
    let origin_x = t.dimensions.content.x;
    let origin_y = t.dimensions.content.y;
    let mut col_x = Vec::with_capacity(widths.len());
    let mut x = origin_x + hs;
    for w in widths {
        col_x.push(x);
        x += w + hs;
    }
    let span_width = |s: &CellSlot| -> f32 {
        widths[s.col..s.col + s.colspan].iter().sum::<f32>() + hs * (s.colspan - 1) as f32
    };

    // Lay every cell out at its width, at the top of the table for now.
    let mut cell_heights = Vec::with_capacity(grid.cells.len());
    for s in &grid.cells {
        let w = span_width(s);
        let cell = at_mut(t, &s.path);
        cell_heights.push(layout_cell(cell, col_x[s.col], origin_y, w));
    }

    // Row heights: the tallest single-row cell and any specified height;
    // a rowspanning cell's shortfall goes to its last row.
    let rows = grid.rows.len();
    let mut heights = vec![0.0f32; rows];
    for (r, path) in grid.rows.iter().enumerate() {
        let row = at(t, path);
        if let Some(h) = absolute_px(&row.style.height, &row.style) {
            heights[r] = h;
        }
    }
    for (s, &h) in grid.cells.iter().zip(&cell_heights) {
        if s.rowspan == 1 {
            heights[s.row] = heights[s.row].max(h);
        }
    }
    let mut spanning: Vec<usize> = (0..grid.cells.len())
        .filter(|&i| grid.cells[i].rowspan > 1)
        .collect();
    spanning.sort_by_key(|&i| grid.cells[i].rowspan);
    for i in spanning {
        let s = &grid.cells[i];
        let have: f32 =
            heights[s.row..s.row + s.rowspan].iter().sum::<f32>() + vs * (s.rowspan - 1) as f32;
        if cell_heights[i] > have {
            heights[s.row + s.rowspan - 1] += cell_heights[i] - have;
        }
    }
    // A taller specified table height grows the rows in proportion to
    // their heights (equally when they are all empty).
    let mut grid_height = heights.iter().sum::<f32>() + track_spacing(rows, vs);
    if let Some(h) = specified_height {
        if rows > 0 && h > grid_height {
            let all: Vec<usize> = (0..rows).collect();
            let weights = heights.clone();
            distribute(&mut heights, &all, |r| weights[r], h - grid_height);
        }
        grid_height = grid_height.max(h);
    }

    let mut row_y = Vec::with_capacity(rows);
    let mut y = origin_y + vs;
    for h in &heights {
        row_y.push(y);
        y += h + vs;
    }
    let rows_x = origin_x + hs;
    let rows_width =
        (widths.iter().sum::<f32>() + hs * widths.len().saturating_sub(1) as f32).max(0.0);

    // Cells to their rows, stretched to the rows they span.
    for s in &grid.cells {
        let target_y = row_y[s.row];
        let target_h =
            heights[s.row..s.row + s.rowspan].iter().sum::<f32>() + vs * (s.rowspan - 1) as f32;
        let cell = at_mut(t, &s.path);
        let dy = target_y - cell.dimensions.border_box().y;
        crate::flex::translate_subtree(cell, 0.0, dy);
        stretch_cell(cell, target_h);
    }

    // Rows and row groups cover their cells; columns are not rendered.
    let place = |b: &mut LayoutBox, y: f32, h: f32| {
        b.dimensions = Dimensions {
            content: Rect::new(rows_x, y, rows_width, h),
            ..Default::default()
        };
    };
    for (r, path) in grid.rows.iter().enumerate() {
        place(at_mut(t, path), row_y[r], heights[r]);
    }
    for &g in &grid.groups {
        let span: Vec<usize> = (0..rows).filter(|&r| grid.rows[r][0] == g).collect();
        let (y, h) = match (span.first(), span.last()) {
            (Some(&a), Some(&b)) => (row_y[a], row_y[b] + heights[b] - row_y[a]),
            _ => (origin_y, 0.0),
        };
        place(&mut t.children[g], y, h);
    }
    for c in &mut t.children {
        if matches!(role(c), Display::TableColumn | Display::TableColumnGroup) {
            hide(c, origin_x, origin_y);
        }
    }

    t.dimensions.content.height = grid_height;
}

fn hide(b: &mut LayoutBox, x: f32, y: f32) {
    b.dimensions = Dimensions {
        content: Rect::new(x, y, 0.0, 0.0),
        ..Default::default()
    };
    for c in &mut b.children {
        hide(c, x, y);
    }
}

/// Lays a cell out as a block formatting root `width` px wide (border box)
/// with its border box at (`x`, `y`), and returns the border-box height the
/// row must give it: its content's height, or its specified height when
/// that is taller. The cell's `width`, `height` and margins are set aside
/// meanwhile: the column decides the width, the row the height, and
/// margins do not apply to cells.
fn layout_cell(cell: &mut LayoutBox, x: f32, y: f32, width: f32) -> f32 {
    let saved = (
        std::mem::replace(&mut cell.style.width, Length::Auto),
        std::mem::replace(&mut cell.style.height, Length::Auto),
        std::mem::replace(&mut cell.style.margin_top, Length::Zero),
        std::mem::replace(&mut cell.style.margin_right, Length::Zero),
        std::mem::replace(&mut cell.style.margin_bottom, Length::Zero),
        std::mem::replace(&mut cell.style.margin_left, Length::Zero),
    );
    let cb = Dimensions {
        content: Rect::new(x, y, width, 0.0),
        ..Default::default()
    };
    cell.layout_with_collapse_in(
        &cb,
        &mut formatting_root_context(),
        &mut FloatContext::new(),
        None,
    );
    let style = &mut cell.style;
    (
        style.width,
        style.height,
        style.margin_top,
        style.margin_right,
        style.margin_bottom,
        style.margin_left,
    ) = saved;
    let d = &cell.dimensions;
    let pb_v = d.padding.vertical() + d.border.vertical();
    let content = d.border_box().height;
    let specified =
        absolute_px(&cell.style.height, &cell.style).map(|h| match cell.style.box_sizing {
            BoxSizing::BorderBox => h,
            BoxSizing::ContentBox => h + pb_v,
        });
    content.max(specified.unwrap_or(0.0))
}

/// Stretches a laid-out cell to a `height` px border box and moves its
/// content for `vertical-align` (§17.5.3): `middle` centres it, `bottom`
/// puts it at the bottom, everything else (`top`, and `baseline`, which
/// this slice does not align across cells) leaves it at the top.
fn stretch_cell(cell: &mut LayoutBox, height: f32) {
    let d = &cell.dimensions;
    let inner = (height - d.padding.vertical() - d.border.vertical()).max(0.0);
    let free = inner - d.content.height;
    let shift = match cell.style.vertical_align {
        VerticalAlign::Middle => free / 2.0,
        VerticalAlign::Bottom => free,
        _ => 0.0,
    };
    if shift > 0.0 {
        for c in &mut cell.children {
            crate::flex::translate_subtree(c, 0.0, shift);
        }
    }
    cell.dimensions.content.height = inner.max(cell.dimensions.content.height);
}
