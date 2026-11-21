//! CSS 2.1 chapter 17 table layout, first slice (#626): the grid, automatic
//! column widths, spans, `border-spacing`, row heights, `vertical-align` in
//! cells, anonymous table objects, and the table as a block-level box.
//!
//! Every cell here holds fixed-size blocks (or inline-blocks), never text, so
//! the expected rectangles are exact and independent of the host's fonts.
//! Each layout runs through both entry points, `layout` and
//! `layout_with_collapse`, and both must agree.

use super::*;
use crate::table::{fixup_table_boxes, TableSpan};
use rustkit_css::{Display, VerticalAlign};

fn st(display: Display) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.display = display;
    s
}

fn bx(display: Display, children: Vec<LayoutBox>) -> LayoutBox {
    let mut b = LayoutBox::new(BoxType::Block, st(display));
    b.children = children;
    b
}

/// A fixed-size block: its min-content and max-content widths are both `w`.
fn block(w: f32, h: f32) -> LayoutBox {
    let mut s = st(Display::Block);
    s.width = Length::Px(w);
    s.height = Length::Px(h);
    LayoutBox::new(BoxType::Block, s)
}

fn inline_block(w: f32, h: f32) -> LayoutBox {
    let mut s = st(Display::InlineBlock);
    s.width = Length::Px(w);
    s.height = Length::Px(h);
    LayoutBox::new(BoxType::Block, s)
}

/// A cell holding one `w`×`h` block.
fn cell(w: f32, h: f32) -> LayoutBox {
    bx(Display::TableCell, vec![block(w, h)])
}

fn empty_cell() -> LayoutBox {
    bx(Display::TableCell, vec![])
}

/// A cell whose min-content width is `piece` and max-content width is
/// `n * piece`: `n` inline-blocks that may wrap between each other.
fn flexible_cell(piece: f32, n: usize) -> LayoutBox {
    bx(
        Display::TableCell,
        (0..n).map(|_| inline_block(piece, 10.0)).collect(),
    )
}

fn span(mut c: LayoutBox, colspan: u32, rowspan: u32) -> LayoutBox {
    c.table_span = TableSpan { colspan, rowspan };
    c
}

fn row(cells: Vec<LayoutBox>) -> LayoutBox {
    bx(Display::TableRow, cells)
}

/// A table with `border-spacing: 0` and a border-box `width` (the UA's
/// `box-sizing` for tables).
fn table(children: Vec<LayoutBox>) -> LayoutBox {
    let mut t = bx(Display::Table, children);
    t.style.border_spacing = (0.0, 0.0);
    t.style.box_sizing = BoxSizing::BorderBox;
    t
}

fn spaced(mut t: LayoutBox, h: f32, v: f32) -> LayoutBox {
    t.style.border_spacing = (h, v);
    t
}

fn text(s: &str) -> LayoutBox {
    LayoutBox::new(BoxType::Text(s.to_string()), st(Display::Inline))
}

/// Lays `root` out as the only child of a `width`-wide body, once through
/// each entry point; returns the laid-out root from each.
fn layout(root: LayoutBox, width: f32) -> Vec<LayoutBox> {
    let mut out = Vec::new();
    for collapse in [false, true] {
        let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        body.children.push(root.clone());
        fixup_table_boxes(&mut body);
        body.set_viewport(1280.0, 800.0);
        let cb = Dimensions {
            content: Rect::new(0.0, 0.0, width, 0.0),
            ..Default::default()
        };
        if collapse {
            body.layout_with_collapse(
                &cb,
                &mut MarginCollapseContext::new(),
                &mut FloatContext::new(),
            );
        } else {
            body.layout(&cb);
        }
        out.push(body.children.remove(0));
    }
    out
}

/// The table cells under `b` in tree order (not descending into cells).
fn cells(b: &LayoutBox) -> Vec<&LayoutBox> {
    let mut out = Vec::new();
    fn walk<'a>(b: &'a LayoutBox, out: &mut Vec<&'a LayoutBox>) {
        for c in &b.children {
            if c.style.display == Display::TableCell {
                out.push(c);
            } else {
                walk(c, out);
            }
        }
    }
    walk(b, &mut out);
    out
}

fn rect(b: &LayoutBox) -> Rect {
    b.dimensions.border_box()
}

#[track_caller]
fn close(got: f32, want: f32, what: &str) {
    assert!((got - want).abs() < 0.01, "{what}: got {got}, want {want}");
}

/// Asserts a cell's border box (x, y, width, height).
#[track_caller]
fn cell_at(b: &LayoutBox, i: usize, want: (f32, f32, f32, f32)) {
    let cs = cells(b);
    assert!(i < cs.len(), "cell {i} missing: only {} cells", cs.len());
    let r = rect(cs[i]);
    let got = (r.x, r.y, r.width, r.height);
    assert!(
        (got.0 - want.0).abs() < 0.01
            && (got.1 - want.1).abs() < 0.01
            && (got.2 - want.2).abs() < 0.01
            && (got.3 - want.3).abs() < 0.01,
        "cell {i}: got {got:?}, want {want:?}"
    );
}

// ---------------------------------------------------------------- the grid

#[test]
fn one_row_two_cells_sit_side_by_side() {
    for t in layout(
        table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 10.0)])]),
        400.0,
    ) {
        cell_at(&t, 0, (0.0, 0.0, 50.0, 20.0));
        cell_at(&t, 1, (50.0, 0.0, 30.0, 20.0));
        close(rect(&t).width, 80.0, "table width");
        close(rect(&t).height, 20.0, "table height");
    }
}

#[test]
fn two_by_two_columns_line_up_across_rows() {
    let t = table(vec![
        row(vec![cell(50.0, 20.0), cell(30.0, 20.0)]),
        row(vec![cell(20.0, 20.0), cell(60.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (0.0, 0.0, 50.0, 20.0));
        cell_at(&t, 1, (50.0, 0.0, 60.0, 20.0));
        cell_at(&t, 2, (0.0, 20.0, 50.0, 20.0));
        cell_at(&t, 3, (50.0, 20.0, 60.0, 20.0));
    }
}

#[test]
fn a_column_is_as_wide_as_its_widest_cell() {
    let t = table(vec![
        row(vec![cell(20.0, 10.0)]),
        row(vec![cell(70.0, 10.0)]),
        row(vec![cell(40.0, 10.0)]),
    ]);
    for t in layout(t, 400.0) {
        for i in 0..3 {
            close(rect(cells(&t)[i]).width, 70.0, "column width");
        }
    }
}

// ----------------------------------------------------------- column widths

fn min_max_table() -> LayoutBox {
    // Column 0: min 50, max 100. Column 1: min 40, max 200.
    table(vec![row(vec![
        flexible_cell(50.0, 2),
        flexible_cell(40.0, 5),
    ])])
}

#[test]
fn between_min_and_max_the_surplus_goes_in_proportion_to_max_minus_min() {
    // 195 available: 105 over the min sum, shared 50 : 160.
    for t in layout(min_max_table(), 195.0) {
        close(rect(&t).width, 195.0, "table width");
        close(rect(cells(&t)[0]).width, 75.0, "column 0");
        close(rect(cells(&t)[1]).width, 120.0, "column 1");
        close(rect(cells(&t)[1]).x, 75.0, "column 1 x");
    }
}

#[test]
fn below_min_content_the_table_overflows_at_its_min_widths() {
    for t in layout(min_max_table(), 60.0) {
        close(rect(&t).width, 90.0, "table width");
        close(rect(cells(&t)[0]).width, 50.0, "column 0");
        close(rect(cells(&t)[1]).width, 40.0, "column 1");
    }
}

#[test]
fn with_room_an_auto_table_is_its_max_content_width() {
    for t in layout(min_max_table(), 1000.0) {
        close(rect(&t).width, 300.0, "table width");
        close(rect(cells(&t)[0]).width, 100.0, "column 0");
        close(rect(cells(&t)[1]).width, 200.0, "column 1");
    }
}

#[test]
fn a_specified_table_width_wider_than_content_is_shared_by_max_width() {
    let mut t = table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]);
    t.style.width = Length::Px(300.0);
    for t in layout(t, 400.0) {
        close(rect(&t).width, 300.0, "table width");
        close(rect(cells(&t)[0]).width, 187.5, "column 0");
        close(rect(cells(&t)[1]).width, 112.5, "column 1");
    }
}

#[test]
fn a_specified_table_width_never_goes_below_min_content() {
    let mut t = min_max_table();
    t.style.width = Length::Px(20.0);
    for t in layout(t, 400.0) {
        close(rect(&t).width, 90.0, "table width");
    }
}

#[test]
fn a_specified_cell_width_sets_its_columns_width() {
    let mut wide = cell(50.0, 20.0);
    wide.style.width = Length::Px(120.0);
    let t = table(vec![
        row(vec![wide, cell(30.0, 20.0)]),
        row(vec![cell(10.0, 20.0), cell(30.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        close(rect(cells(&t)[0]).width, 120.0, "column 0, row 0");
        close(rect(cells(&t)[2]).width, 120.0, "column 0, row 1");
        close(rect(cells(&t)[1]).x, 120.0, "column 1 x");
    }
}

#[test]
fn a_col_width_sets_its_columns_width() {
    let mut col = bx(Display::TableColumn, vec![]);
    col.style.width = Length::Px(100.0);
    let t = table(vec![col, row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]);
    for t in layout(t, 400.0) {
        close(rect(cells(&t)[0]).width, 100.0, "column 0");
        close(rect(cells(&t)[1]).x, 100.0, "column 1 x");
    }
}

// ------------------------------------------------------------------- spans

#[test]
fn a_colspan_2_cell_covers_both_columns() {
    let t = table(vec![
        row(vec![cell(40.0, 20.0), cell(60.0, 20.0)]),
        row(vec![span(cell(50.0, 20.0), 2, 1)]),
    ]);
    for t in layout(t, 400.0) {
        cell_at(&t, 2, (0.0, 20.0, 100.0, 20.0));
    }
}

#[test]
fn a_wide_spanning_cell_widens_its_columns_in_proportion_to_max() {
    let t = spaced(
        table(vec![
            row(vec![cell(40.0, 20.0), cell(60.0, 20.0)]),
            row(vec![span(cell(202.0, 20.0), 2, 1)]),
        ]),
        2.0,
        0.0,
    );
    // The span owns the 2px between its columns: 200 to share, 100 extra.
    for t in layout(t, 400.0) {
        close(rect(cells(&t)[0]).width, 80.0, "column 0");
        close(rect(cells(&t)[1]).width, 120.0, "column 1");
        close(rect(cells(&t)[2]).width, 202.0, "the spanning cell");
    }
}

#[test]
fn a_rowspan_2_cell_covers_both_rows_and_pushes_the_next_cell_over() {
    let t = table(vec![
        row(vec![span(cell(30.0, 100.0), 1, 2), cell(40.0, 20.0)]),
        row(vec![cell(50.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (0.0, 0.0, 30.0, 100.0));
        cell_at(&t, 1, (30.0, 0.0, 50.0, 20.0));
        // The spanning cell's extra 60px goes to its last row.
        cell_at(&t, 2, (30.0, 20.0, 50.0, 80.0));
    }
}

#[test]
fn a_rowspan_past_the_last_row_is_clamped_to_the_grid() {
    let t = table(vec![row(vec![
        span(cell(30.0, 20.0), 1, 5),
        cell(40.0, 20.0),
    ])]);
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (0.0, 0.0, 30.0, 20.0));
        close(rect(&t).height, 20.0, "table height");
    }
}

#[test]
fn a_colspan_of_zero_counts_as_one() {
    let t = table(vec![row(vec![
        span(cell(30.0, 20.0), 0, 1),
        cell(40.0, 20.0),
    ])]);
    for t in layout(t, 400.0) {
        cell_at(&t, 1, (30.0, 0.0, 40.0, 20.0));
    }
}

// --------------------------------------------------------- border-spacing

#[test]
fn border_spacing_zero_puts_cells_edge_to_edge() {
    for t in layout(
        table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]),
        400.0,
    ) {
        close(
            rect(cells(&t)[1]).x,
            rect(cells(&t)[0]).right(),
            "edge to edge",
        );
    }
}

#[test]
fn default_border_spacing_2px_surrounds_every_cell() {
    let t = spaced(
        table(vec![
            row(vec![cell(50.0, 20.0), cell(30.0, 20.0)]),
            row(vec![cell(50.0, 20.0), cell(30.0, 20.0)]),
        ]),
        2.0,
        2.0,
    );
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (2.0, 2.0, 50.0, 20.0));
        cell_at(&t, 1, (54.0, 2.0, 30.0, 20.0));
        cell_at(&t, 2, (2.0, 24.0, 50.0, 20.0));
        close(rect(&t).width, 86.0, "table width");
        close(rect(&t).height, 46.0, "table height");
    }
}

#[test]
fn two_value_border_spacing_is_horizontal_then_vertical() {
    let t = spaced(
        table(vec![
            row(vec![cell(50.0, 20.0), cell(30.0, 20.0)]),
            row(vec![cell(50.0, 20.0), cell(30.0, 20.0)]),
        ]),
        10.0,
        4.0,
    );
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (10.0, 4.0, 50.0, 20.0));
        cell_at(&t, 1, (70.0, 4.0, 30.0, 20.0));
        cell_at(&t, 3, (70.0, 28.0, 30.0, 20.0));
        close(rect(&t).width, 110.0, "table width");
        close(rect(&t).height, 52.0, "table height");
    }
}

#[test]
fn border_spacing_sits_inside_the_tables_border_and_padding() {
    let mut t = spaced(table(vec![row(vec![cell(50.0, 20.0)])]), 2.0, 2.0);
    t.style.border_left_width = Length::Px(3.0);
    t.style.border_top_width = Length::Px(3.0);
    t.style.padding_left = Length::Px(4.0);
    t.style.padding_top = Length::Px(4.0);
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (9.0, 9.0, 50.0, 20.0));
        close(rect(&t).width, 3.0 + 4.0 + 2.0 + 50.0 + 2.0, "table width");
    }
}

// ------------------------------------------------------------ cell boxes

#[test]
fn cell_padding_wraps_the_cells_content() {
    let mut c = cell(50.0, 20.0);
    for p in [
        &mut c.style.padding_top,
        &mut c.style.padding_right,
        &mut c.style.padding_bottom,
        &mut c.style.padding_left,
    ] {
        *p = Length::Px(5.0);
    }
    for t in layout(table(vec![row(vec![c, cell(30.0, 20.0)])]), 400.0) {
        cell_at(&t, 0, (0.0, 0.0, 60.0, 30.0));
        cell_at(&t, 1, (60.0, 0.0, 30.0, 30.0));
        let content = rect(&cells(&t)[0].children[0]);
        close(content.x, 5.0, "content x");
        close(content.y, 5.0, "content y");
    }
}

#[test]
fn a_row_is_as_tall_as_its_tallest_cell() {
    for t in layout(
        table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 50.0)])]),
        400.0,
    ) {
        close(rect(cells(&t)[0]).height, 50.0, "short cell stretched");
        close(rect(cells(&t)[1]).height, 50.0, "tall cell");
    }
}

#[test]
fn a_specified_row_height_is_a_minimum() {
    let mut r = row(vec![cell(50.0, 20.0)]);
    r.style.height = Length::Px(40.0);
    for t in layout(table(vec![r, row(vec![cell(50.0, 20.0)])]), 400.0) {
        cell_at(&t, 0, (0.0, 0.0, 50.0, 40.0));
        cell_at(&t, 1, (0.0, 40.0, 50.0, 20.0));
    }
}

#[test]
fn a_specified_cell_height_is_a_minimum_for_its_row() {
    let mut c = cell(50.0, 20.0);
    c.style.height = Length::Px(45.0);
    for t in layout(table(vec![row(vec![c, cell(30.0, 20.0)])]), 400.0) {
        close(rect(cells(&t)[1]).height, 45.0, "row height");
    }
}

#[test]
fn vertical_align_places_content_in_a_taller_row() {
    for (va, want) in [
        (VerticalAlign::Top, 0.0),
        (VerticalAlign::Middle, 20.0),
        (VerticalAlign::Bottom, 40.0),
    ] {
        let mut c = cell(30.0, 20.0);
        c.style.vertical_align = va;
        for t in layout(table(vec![row(vec![cell(50.0, 60.0), c])]), 400.0) {
            let cs = cells(&t);
            close(rect(cs[1]).height, 60.0, "stretched cell");
            close(
                rect(&cs[1].children[0]).y,
                want,
                &format!("{va:?} content y"),
            );
        }
    }
}

#[test]
fn an_empty_cell_keeps_its_column_and_row() {
    let t = table(vec![
        row(vec![cell(50.0, 20.0), empty_cell(), cell(30.0, 20.0)]),
        row(vec![cell(10.0, 20.0), cell(40.0, 20.0), cell(10.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        cell_at(&t, 1, (50.0, 0.0, 40.0, 20.0));
        cell_at(&t, 2, (90.0, 0.0, 30.0, 20.0));
    }
}

// ------------------------------------------------ anonymous table objects

#[test]
fn bare_cells_in_a_table_get_an_anonymous_row() {
    for t in layout(table(vec![cell(50.0, 20.0), cell(30.0, 20.0)]), 400.0) {
        assert_eq!(t.children.len(), 1, "one anonymous row");
        assert_eq!(t.children[0].style.display, Display::TableRow);
        cell_at(&t, 0, (0.0, 0.0, 50.0, 20.0));
        cell_at(&t, 1, (50.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn a_bare_row_gets_an_anonymous_table() {
    let mut div = bx(
        Display::Block,
        vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])],
    );
    fixup_table_boxes(&mut div);
    assert_eq!(div.children.len(), 1);
    assert_eq!(div.children[0].style.display, Display::Table);
    assert_eq!(div.children[0].children[0].style.display, Display::TableRow);
    for d in layout(div, 400.0) {
        cell_at(&d, 0, (0.0, 0.0, 50.0, 20.0));
        cell_at(&d, 1, (50.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn bare_cells_in_a_block_get_an_anonymous_row_and_table() {
    let mut div = bx(
        Display::Block,
        vec![text("a"), cell(50.0, 20.0), text(" "), cell(30.0, 20.0)],
    );
    fixup_table_boxes(&mut div);
    assert_eq!(div.children.len(), 2, "text, then one anonymous table");
    let t = &div.children[1];
    assert_eq!(t.style.display, Display::Table);
    assert_eq!(t.children.len(), 1, "one row");
    assert_eq!(
        t.children[0].children.len(),
        2,
        "the whitespace between the cells is gone"
    );
}

#[test]
fn non_cell_content_of_a_row_gets_an_anonymous_cell() {
    let mut t = table(vec![row(vec![block(50.0, 20.0), cell(30.0, 20.0)])]);
    fixup_table_boxes(&mut t);
    let r = &t.children[0];
    assert_eq!(r.children.len(), 2);
    assert_eq!(r.children[0].style.display, Display::TableCell);
    for t in layout(t, 400.0) {
        cell_at(&t, 1, (50.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn whitespace_between_rows_generates_no_box() {
    let t = table(vec![
        text("\n  "),
        row(vec![cell(50.0, 20.0)]),
        text(" "),
        row(vec![cell(50.0, 20.0)]),
        text("\n"),
    ]);
    for t in layout(t, 400.0) {
        assert!(
            t.children
                .iter()
                .all(|c| !matches!(c.box_type, BoxType::Text(_))),
            "no text box survives in a table"
        );
        cell_at(&t, 0, (0.0, 0.0, 50.0, 20.0));
        cell_at(&t, 1, (0.0, 20.0, 50.0, 20.0));
        close(rect(&t).height, 40.0, "table height");
    }
}

#[test]
fn fixup_is_idempotent() {
    let mut once = bx(Display::Block, vec![cell(50.0, 20.0), cell(30.0, 20.0)]);
    fixup_table_boxes(&mut once);
    let mut twice = once.clone();
    fixup_table_boxes(&mut twice);
    assert_eq!(
        format!("{:?}", once.children.len()),
        format!("{:?}", twice.children.len())
    );
    assert_eq!(
        once.children[0].children.len(),
        twice.children[0].children.len()
    );
    assert_eq!(twice.children[0].children[0].children.len(), 2);
}

// ------------------------------------------------------------- row groups

#[test]
fn thead_comes_first_and_tfoot_last_whatever_the_source_order() {
    let t = table(vec![
        bx(Display::TableFooterGroup, vec![row(vec![cell(10.0, 10.0)])]),
        bx(Display::TableRowGroup, vec![row(vec![cell(10.0, 20.0)])]),
        bx(Display::TableHeaderGroup, vec![row(vec![cell(10.0, 30.0)])]),
    ]);
    for t in layout(t, 400.0) {
        let cs = cells(&t);
        // Tree order is foot, body, head.
        close(rect(cs[2]).y, 0.0, "thead row");
        close(rect(cs[1]).y, 30.0, "tbody row");
        close(rect(cs[0]).y, 50.0, "tfoot row");
        close(rect(&t).height, 60.0, "table height");
    }
}

#[test]
fn row_and_row_group_boxes_cover_their_cells() {
    let t = spaced(
        table(vec![bx(
            Display::TableRowGroup,
            vec![
                row(vec![cell(50.0, 20.0), cell(30.0, 20.0)]),
                row(vec![cell(50.0, 20.0)]),
            ],
        )]),
        2.0,
        2.0,
    );
    for t in layout(t, 400.0) {
        let g = &t.children[0];
        let r0 = rect(&g.children[0]);
        close(r0.x, 2.0, "row x");
        close(r0.y, 2.0, "row y");
        close(r0.width, 82.0, "row width");
        close(r0.height, 20.0, "row height");
        let gr = rect(g);
        close(gr.y, 2.0, "group y");
        close(gr.height, 42.0, "group height");
    }
}

// ----------------------------------------------------------------- caption

#[test]
fn a_caption_sits_above_the_grid() {
    let t = table(vec![
        bx(Display::TableCaption, vec![block(40.0, 30.0)]),
        row(vec![cell(50.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        let cap = rect(&t.children[0]);
        close(cap.y, 0.0, "caption y");
        close(cap.height, 30.0, "caption height");
        close(cap.width, 50.0, "caption takes the table's width");
        cell_at(&t, 0, (0.0, 30.0, 50.0, 20.0));
        close(
            t.dimensions.margin_box().height,
            50.0,
            "caption + grid in the flow",
        );
    }
}

#[test]
fn a_wide_caption_widens_the_table() {
    let t = table(vec![
        bx(Display::TableCaption, vec![block(100.0, 30.0)]),
        row(vec![cell(50.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        close(rect(&t).width, 100.0, "table width");
        close(
            rect(cells(&t)[0]).width,
            100.0,
            "the column takes the caption's width",
        );
    }
}

// --------------------------------------------------- the table as a block

#[test]
fn a_nested_table_lays_out_inside_its_cell() {
    let inner = table(vec![row(vec![cell(30.0, 20.0), cell(40.0, 20.0)])]);
    let outer = table(vec![row(vec![
        bx(Display::TableCell, vec![inner]),
        cell(10.0, 20.0),
    ])]);
    for t in layout(outer, 400.0) {
        let outer_cells = cells(&t);
        close(rect(outer_cells[0]).width, 70.0, "outer column 0");
        close(rect(outer_cells[1]).x, 70.0, "outer column 1 x");
        let inner = &outer_cells[0].children[0];
        let ic = cells(inner);
        close(rect(ic[0]).x, 0.0, "inner cell 0 x");
        close(rect(ic[1]).x, 30.0, "inner cell 1 beside cell 0");
        close(rect(ic[1]).y, rect(ic[0]).y, "inner cells on one row");
    }
}

#[test]
fn margin_auto_centres_the_table() {
    let mut t = table(vec![row(vec![cell(50.0, 20.0), cell(50.0, 20.0)])]);
    t.style.margin_left = Length::Auto;
    t.style.margin_right = Length::Auto;
    for t in layout(t, 400.0) {
        close(rect(&t).x, 150.0, "table x");
        cell_at(&t, 0, (150.0, 0.0, 50.0, 20.0));
    }
}

#[test]
fn a_table_follows_the_block_before_it() {
    let mut body = bx(
        Display::Block,
        vec![block(100.0, 25.0), table(vec![row(vec![cell(50.0, 20.0)])])],
    );
    body.style.width = Length::Px(400.0);
    for b in layout(body, 400.0) {
        let t = &b.children[1];
        cell_at(t, 0, (0.0, 25.0, 50.0, 20.0));
        close(rect(&b).height, 45.0, "the table is in the flow");
    }
}

#[test]
fn a_floated_table_shrinks_to_fit_on_the_right() {
    let mut t = table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]);
    t.float = Float::Right;
    t.style.float = Float::Right;
    let mut wrap = bx(Display::Block, vec![t]);
    wrap.style.width = Length::Px(400.0);
    for w in layout(wrap, 400.0) {
        let t = &w.children[0];
        close(rect(t).width, 80.0, "shrink-to-fit width");
        close(rect(t).x, 320.0, "floated right");
        cell_at(t, 1, (370.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn a_table_in_a_flex_row_is_as_wide_as_its_columns() {
    let mut flex = bx(
        Display::Flex,
        vec![
            table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]),
            block(100.0, 20.0),
        ],
    );
    flex.style.width = Length::Px(500.0);
    for f in layout(flex, 500.0) {
        close(rect(&f.children[0]).width, 80.0, "table item width");
        close(rect(&f.children[1]).x, 80.0, "next item x");
        cell_at(&f.children[0], 1, (50.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn a_tables_intrinsic_widths_are_its_columns_plus_spacing() {
    let mut t = spaced(min_max_table(), 2.0, 2.0);
    fixup_table_boxes(&mut t);
    close(
        crate::grid::own_min_content_width(&t),
        90.0 + 6.0,
        "min-content",
    );
    close(
        crate::grid::own_max_content_width(&t),
        300.0 + 6.0,
        "max-content",
    );
    let mut b = t.clone();
    b.style.border_left_width = Length::Px(1.0);
    b.style.border_right_width = Length::Px(1.0);
    close(
        crate::grid::own_max_content_width(&b),
        308.0,
        "max-content with border",
    );
}

#[test]
fn column_boxes_are_not_rendered() {
    let mut col = bx(Display::TableColumn, vec![]);
    col.style.width = Length::Px(100.0);
    let group = bx(Display::TableColumnGroup, vec![col]);
    for t in layout(table(vec![group, row(vec![cell(50.0, 20.0)])]), 400.0) {
        let g = rect(&t.children[0]);
        close(g.width, 0.0, "column group width");
        close(g.height, 0.0, "column group height");
        close(
            rect(cells(&t)[0]).width,
            100.0,
            "the col still sets the width",
        );
    }
}

// ------------------------------------------------------- the infobox shape

/// Wikipedia's infobox: `width: 22em` at 14px, `border-spacing: 3px`, a 1px
/// border, a header cell spanning both columns, then key/value rows.
#[test]
fn a_wikipedia_shaped_infobox() {
    let mut t = spaced(
        table(vec![bx(
            Display::TableRowGroup,
            vec![
                row(vec![span(cell(100.0, 20.0), 2, 1)]),
                row(vec![cell(60.0, 15.0), cell(120.0, 15.0)]),
                row(vec![cell(60.0, 15.0), cell(120.0, 15.0)]),
                row(vec![cell(60.0, 15.0), cell(120.0, 15.0)]),
            ],
        )]),
        3.0,
        3.0,
    );
    t.style.font_size = Length::Px(14.0);
    t.style.width = Length::Em(22.0);
    for side in [
        &mut t.style.border_top_width,
        &mut t.style.border_right_width,
        &mut t.style.border_bottom_width,
        &mut t.style.border_left_width,
    ] {
        *side = Length::Px(1.0);
    }
    t.float = Float::Right;
    t.style.float = Float::Right;
    let mut wrap = bx(Display::Block, vec![t]);
    wrap.style.width = Length::Px(800.0);
    for w in layout(wrap, 800.0) {
        let t = &w.children[0];
        close(rect(t).width, 308.0, "22em at 14px");
        close(rect(t).x, 492.0, "floated right");
        let cs = cells(t);
        // 308 - 2 border - 9 spacing = 297 for the columns; 60 : 120 by max.
        cell_at(t, 0, (496.0, 4.0, 300.0, 20.0));
        for r in 0..3 {
            let key = rect(cs[1 + 2 * r]);
            let value = rect(cs[2 + 2 * r]);
            close(key.x, 496.0, "key column x");
            close(key.width, 99.0, "key column width");
            close(value.x, 598.0, "value column x");
            close(value.width, 198.0, "value column width");
            close(key.y, 27.0 + 18.0 * r as f32, "row y");
        }
        close(
            rect(t).height,
            // Border, five spacings around four rows, the rows, border.
            1.0 + 5.0 * 3.0 + 20.0 + 3.0 * 15.0 + 1.0,
            "table height",
        );
    }
}

// ------------------------------------------- intrinsic sizing, more cases

#[test]
fn a_table_inside_an_inline_block_sizes_it() {
    let ib = bx(
        Display::InlineBlock,
        vec![table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])])],
    );
    for b in layout(bx(Display::Block, vec![ib]), 400.0) {
        let ib = &b.children[0];
        close(rect(ib).width, 80.0, "inline-block width");
        cell_at(&ib.children[0], 1, (50.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn an_inline_table_sits_on_the_line_beside_an_inline_block() {
    let mut it = table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]);
    it.style.display = Display::InlineTable;
    for b in layout(
        bx(Display::Block, vec![inline_block(40.0, 20.0), it]),
        400.0,
    ) {
        let t = &b.children[1];
        close(rect(t).x, 40.0, "inline-table x");
        close(rect(t).width, 80.0, "inline-table width");
        close(rect(cells(t)[1]).x, 90.0, "second cell x");
    }
}

/// Known gap: flex places an item from its STYLE margins after laying out
/// its children, so the caption height table.rs adds to the top margin is
/// lost and the caption lands above the flex line. Captioned tables as flex
/// items are left to a later slice.
#[test]
#[ignore = "captioned table as a flex item: flex re-places the item by its style margins"]
fn a_captioned_table_in_a_flex_row_is_shifted_once() {
    let t = table(vec![
        bx(Display::TableCaption, vec![block(40.0, 30.0)]),
        row(vec![cell(50.0, 20.0)]),
    ]);
    let mut flex = bx(Display::Flex, vec![t]);
    flex.style.width = Length::Px(400.0);
    for f in layout(flex, 400.0) {
        let t = &f.children[0];
        close(rect(&t.children[0]).y, 0.0, "caption y");
        cell_at(t, 0, (0.0, 30.0, 50.0, 20.0));
        close(rect(&f).height, 50.0, "flex container height");
    }
}

#[test]
fn a_floated_tables_width_is_its_columns_not_the_line() {
    let mut t = min_max_table();
    t.float = Float::Left;
    t.style.float = Float::Left;
    for b in layout(bx(Display::Block, vec![t]), 1000.0) {
        close(rect(&b.children[0]).width, 300.0, "max-content");
    }
    let mut t = min_max_table();
    t.float = Float::Left;
    t.style.float = Float::Left;
    for b in layout(bx(Display::Block, vec![t]), 195.0) {
        close(rect(&b.children[0]).width, 195.0, "clamped to the line");
    }
}

// ------------------------------------------------- heights and the rest

#[test]
fn a_taller_table_height_grows_the_rows() {
    let mut t = table(vec![
        row(vec![cell(50.0, 20.0)]),
        row(vec![cell(50.0, 20.0)]),
    ]);
    t.style.height = Length::Px(100.0);
    for t in layout(t, 400.0) {
        close(rect(&t).height, 100.0, "table height");
        cell_at(&t, 0, (0.0, 0.0, 50.0, 50.0));
        cell_at(&t, 1, (0.0, 50.0, 50.0, 50.0));
    }
}

#[test]
fn a_shorter_table_height_does_not_cut_the_rows() {
    let mut t = table(vec![
        row(vec![cell(50.0, 20.0)]),
        row(vec![cell(50.0, 20.0)]),
    ]);
    t.style.height = Length::Px(10.0);
    for t in layout(t, 400.0) {
        close(rect(&t).height, 40.0, "table height");
    }
}

#[test]
fn border_collapse_is_approximated_by_zero_spacing() {
    let mut t = spaced(
        table(vec![row(vec![cell(50.0, 20.0), cell(30.0, 20.0)])]),
        5.0,
        5.0,
    );
    t.style.border_collapse = rustkit_css::BorderCollapse::Collapse;
    for t in layout(t, 400.0) {
        cell_at(&t, 0, (0.0, 0.0, 50.0, 20.0));
        cell_at(&t, 1, (50.0, 0.0, 30.0, 20.0));
    }
}

#[test]
fn vertical_align_baseline_falls_back_to_top() {
    let mut c = cell(30.0, 20.0);
    c.style.vertical_align = VerticalAlign::Baseline;
    for t in layout(table(vec![row(vec![cell(50.0, 60.0), c])]), 400.0) {
        close(rect(&cells(&t)[1].children[0]).y, 0.0, "baseline content y");
    }
}

#[test]
fn rowspan_zero_and_overlong_rowspans_stop_at_their_row_group() {
    let t = table(vec![
        bx(
            Display::TableRowGroup,
            vec![
                row(vec![span(cell(30.0, 20.0), 1, 0), cell(40.0, 20.0)]),
                row(vec![cell(40.0, 20.0)]),
            ],
        ),
        bx(
            Display::TableRowGroup,
            vec![row(vec![cell(30.0, 20.0), cell(40.0, 20.0)])],
        ),
    ]);
    for t in layout(t, 400.0) {
        // rowspan=0 covers both rows of the first group, no more.
        cell_at(&t, 0, (0.0, 0.0, 30.0, 40.0));
        cell_at(&t, 2, (30.0, 20.0, 40.0, 20.0));
        // The second group starts again at column 0.
        cell_at(&t, 3, (0.0, 40.0, 30.0, 20.0));
    }
}

#[test]
fn a_cell_in_a_short_row_leaves_the_missing_slots_empty() {
    let t = table(vec![
        row(vec![cell(50.0, 20.0), cell(30.0, 20.0), cell(20.0, 20.0)]),
        row(vec![cell(10.0, 20.0)]),
    ]);
    for t in layout(t, 400.0) {
        close(rect(&t).width, 100.0, "three columns");
        cell_at(&t, 3, (0.0, 20.0, 50.0, 20.0));
    }
}
