//! css-flexbox-1 §9.7, resolving flexible lengths: free space is measured
//! from each item's flex BASE size, inflexible items freeze first, and min/max
//! violations freeze and hand their share back to the rest. Every expected
//! number was measured on pinned Chrome for Testing 148 at a 400px viewport
//! (`getBoundingClientRect`); each case runs through both entry points,
//! `layout` and `layout_with_collapse`.

use super::*;
use rustkit_css::{AlignItems, Display, FlexBasis, JustifyContent};

fn row() -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.display = Display::Flex;
    s.height = Length::Px(10.0);
    s
}

fn item(grow: f32, shrink: f32, basis: FlexBasis) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.flex_grow = grow;
    s.flex_shrink = shrink;
    s.flex_basis = basis;
    s
}

fn zero_pct() -> ComputedStyle {
    item(1.0, 1.0, FlexBasis::Percent(0.0))
}

fn sized_block(w: f32, h: f32) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::Px(w);
    s.height = Length::Px(h);
    LayoutBox::new(BoxType::Block, s)
}

/// Lays the flex `container` out in a block `<body>` 400px wide through the
/// named entry point and returns it.
fn laid_out(container: LayoutBox, collapse: bool) -> LayoutBox {
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(container);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, 400.0, 0.0),
        ..Default::default()
    };
    if collapse {
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        body.layout_with_collapse(&cb, &mut mc, &mut fc);
    } else {
        body.layout(&cb);
    }
    body.children.remove(0)
}

/// Lays out a row of `items` and returns each item's (x, width).
fn row_of(items: Vec<LayoutBox>, collapse: bool) -> Vec<(f32, f32)> {
    let mut c = LayoutBox::new(BoxType::Block, row());
    c.children = items;
    laid_out(c, collapse)
        .children
        .iter()
        .map(|b| (b.dimensions.content.x, b.dimensions.content.width))
        .collect()
}

fn assert_row(got: &[(f32, f32)], want: &[(f32, f32)], what: &str) {
    let close = got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| (g.0 - w.0).abs() < 0.5 && (g.1 - w.1).abs() < 0.5);
    assert!(
        close,
        "{what}: Chrome 148 has (x, width) {want:?}, got {got:?}"
    );
}

/// `flex: 1 1 0%` twice, one item holding a 50px box: 200/200, not 175/225.
/// The automatic minimum (50) lifts that item's hypothetical size, and free
/// space measured from hypothetical sizes gave it 50 extra.
#[test]
fn basis_zero_items_split_the_row_evenly_whatever_their_content() {
    for collapse in [false, true] {
        let mut a1 = zero_pct();
        a1.height = Length::Px(100.0);
        let mut a2 = LayoutBox::new(BoxType::Block, zero_pct());
        a2.children.push(sized_block(50.0, 50.0));
        let got = row_of(vec![LayoutBox::new(BoxType::Block, a1), a2], collapse);
        assert_row(&got, &[(0.0, 200.0), (200.0, 200.0)], "basis-0 split");
    }
}

/// The x.com shape end to end: the centring item gets its 200 and centres
/// the box in it (x 275; 262.5 before).
#[test]
fn a_centring_basis_zero_item_centres_in_its_even_share() {
    for collapse in [false, true] {
        let mut left = zero_pct();
        left.height = Length::Px(100.0);
        let mut right_s = zero_pct();
        right_s.display = Display::Flex;
        right_s.align_items = AlignItems::Center;
        right_s.justify_content = JustifyContent::Center;
        let mut right = LayoutBox::new(BoxType::Block, right_s);
        right.children.push(sized_block(50.0, 50.0));
        let mut c = LayoutBox::new(BoxType::Block, row());
        c.style.height = Length::Px(400.0);
        c.children = vec![LayoutBox::new(BoxType::Block, left), right];
        let root = laid_out(c, collapse);
        let x = root.children[1].children[0].dimensions.content.x;
        assert!(
            (x - 275.0).abs() < 0.5,
            "Chrome 148 puts the box at x=275, got {x}"
        );
    }
}

/// A min-width violation freezes that item at its minimum and the other two
/// share what is left: 250 / 75 / 75.
#[test]
fn a_min_violation_freezes_and_the_rest_share_the_remainder() {
    for collapse in [false, true] {
        let mut b1 = zero_pct();
        b1.min_width = Length::Px(250.0);
        let got = row_of(
            vec![
                LayoutBox::new(BoxType::Block, b1),
                LayoutBox::new(BoxType::Block, zero_pct()),
                LayoutBox::new(BoxType::Block, zero_pct()),
            ],
            collapse,
        );
        assert_row(
            &got,
            &[(0.0, 250.0), (250.0, 75.0), (325.0, 75.0)],
            "min violation",
        );
    }
}

/// A max-width violation hands its surplus to the others: 50 / 175 / 175.
#[test]
fn a_max_violation_freezes_and_the_rest_take_its_surplus() {
    for collapse in [false, true] {
        let mut c1 = zero_pct();
        c1.max_width = Length::Px(50.0);
        let got = row_of(
            vec![
                LayoutBox::new(BoxType::Block, c1),
                LayoutBox::new(BoxType::Block, zero_pct()),
                LayoutBox::new(BoxType::Block, zero_pct()),
            ],
            collapse,
        );
        assert_row(
            &got,
            &[(0.0, 50.0), (50.0, 175.0), (225.0, 175.0)],
            "max violation",
        );
    }
}

/// Shrinking: two 300px bases in 400, one with `min-width: 250`. Both shrink
/// by 100 to 200, the first violates, freezes at 250, and the second absorbs
/// the rest: 250 / 150.
#[test]
fn a_min_violation_while_shrinking_moves_the_overflow_to_the_other_item() {
    for collapse in [false, true] {
        let mut e1 = item(0.0, 1.0, FlexBasis::Length(300.0));
        e1.min_width = Length::Px(250.0);
        let got = row_of(
            vec![
                LayoutBox::new(BoxType::Block, e1),
                LayoutBox::new(BoxType::Block, item(0.0, 1.0, FlexBasis::Length(300.0))),
            ],
            collapse,
        );
        assert_row(
            &got,
            &[(0.0, 250.0), (250.0, 150.0)],
            "shrink min violation",
        );
    }
}

/// Grow factors summing below 1 take only that fraction of the free space:
/// two `flex-grow: 0.25` items fill half of 400.
#[test]
fn fractional_grow_factors_take_only_their_fraction_of_free_space() {
    for collapse in [false, true] {
        let got = row_of(
            vec![
                LayoutBox::new(BoxType::Block, item(0.25, 1.0, FlexBasis::Percent(0.0))),
                LayoutBox::new(BoxType::Block, item(0.25, 1.0, FlexBasis::Percent(0.0))),
            ],
            collapse,
        );
        assert_row(&got, &[(0.0, 100.0), (100.0, 100.0)], "fractional grow");
    }
}

/// Guard: an auto-height column's `flex: 1` item keeps at least its content
/// height (the shelf's command palette). Chrome 148 gives the item its 97px
/// of content through the vertical automatic minimum (§4.5); before the
/// early return in `resolve_flexible_lengths`, step 11d's re-run collapsed
/// it to 0.
#[test]
fn a_flex_one_item_in_an_auto_height_column_keeps_its_content() {
    for collapse in [false, true] {
        let mut body_s = ComputedStyle::new();
        body_s.display = Display::Flex;
        body_s.flex_direction = rustkit_css::FlexDirection::Column;
        let mut body = LayoutBox::new(BoxType::Block, body_s);
        let mut hdr_s = ComputedStyle::new();
        hdr_s.display = Display::Flex;
        hdr_s.align_items = AlignItems::Center;
        hdr_s.padding_top = Length::Px(8.0);
        hdr_s.padding_bottom = Length::Px(8.0);
        let mut hdr = LayoutBox::new(BoxType::Block, hdr_s);
        hdr.children.push(sized_block(24.0, 24.0));
        let mut pal_s = item(1.0, 1.0, FlexBasis::Length(0.0));
        pal_s.display = Display::Flex;
        pal_s.flex_direction = rustkit_css::FlexDirection::Column;
        pal_s.padding_top = Length::Px(12.0);
        pal_s.padding_bottom = Length::Px(12.0);
        let mut pal = LayoutBox::new(BoxType::Block, pal_s);
        pal.children.push(sized_block(100.0, 41.0));
        pal.children.push(sized_block(100.0, 56.0));
        body.children.push(hdr);
        body.children.push(pal);
        let root = laid_out(body, collapse);
        let h = root.children[1].dimensions.content.height;
        assert!(h >= 97.0, "the palette holds 97px of content, got {h}");
    }
}

fn column(height: f32) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.display = Display::Flex;
    s.flex_direction = rustkit_css::FlexDirection::Column;
    s.height = Length::Px(height);
    s
}

/// A `flex: 1 1 0%` item in a 50px column holding a 100px box. Returns the
/// item's content height.
fn squeezed_item_height(tweak: impl Fn(&mut ComputedStyle), collapse: bool) -> f32 {
    let mut s = zero_pct();
    tweak(&mut s);
    let mut it = LayoutBox::new(BoxType::Block, s);
    it.children.push(sized_block(100.0, 100.0));
    let mut c = LayoutBox::new(BoxType::Block, column(50.0));
    c.children.push(it);
    laid_out(c, collapse).children[0].dimensions.content.height
}

/// §4.5 on the vertical axis: `min-height: auto` floors a column item at its
/// content, so it overflows the 50px column at 100 rather than shrinking.
/// `min-height: 0` and a scroll container both opt out and get the 50.
#[test]
fn a_column_item_is_not_shrunk_below_its_content() {
    for collapse in [false, true] {
        let h = squeezed_item_height(|_| {}, collapse);
        assert!(
            (h - 100.0).abs() < 0.5,
            "min-height:auto: Chrome 148 has 100, got {h}"
        );
        let h = squeezed_item_height(|s| s.min_height = Length::Px(0.0), collapse);
        assert!(
            (h - 50.0).abs() < 0.5,
            "min-height:0: Chrome 148 has 50, got {h}"
        );
        let h = squeezed_item_height(|s| s.overflow_y = rustkit_css::Overflow::Auto, collapse);
        assert!(
            (h - 50.0).abs() < 0.5,
            "overflow-y:auto: Chrome 148 has 50, got {h}"
        );
    }
}

/// An auto-height column holding a `flex: 1 1 0%; height: fit-content` item
/// (two 40px boxes inside) and then a 20px box. Returns the item's content
/// height and the 20px box's y. `grid` makes the item a grid container, as
/// linkedin's hero wrapper is.
fn fit_content_item_in_auto_column(grid: bool, collapse: bool) -> (f32, f32) {
    let mut s = zero_pct();
    s.height = Length::FitContent;
    if grid {
        s.display = Display::Grid;
    }
    let mut it = LayoutBox::new(BoxType::Block, s);
    if grid {
        let mut col_s = ComputedStyle::new();
        col_s.display = Display::Flex;
        col_s.flex_direction = rustkit_css::FlexDirection::Column;
        let mut col = LayoutBox::new(BoxType::Block, col_s);
        col.children.push(sized_block(100.0, 40.0));
        col.children.push(sized_block(100.0, 40.0));
        it.children.push(col);
    } else {
        it.children.push(sized_block(100.0, 40.0));
        it.children.push(sized_block(100.0, 40.0));
    }
    let mut c_s = ComputedStyle::new();
    c_s.display = Display::Flex;
    c_s.flex_direction = rustkit_css::FlexDirection::Column;
    let mut c = LayoutBox::new(BoxType::Block, c_s);
    c.children.push(it);
    c.children.push(sized_block(100.0, 20.0));
    let root = laid_out(c, collapse);
    (
        root.children[0].dimensions.content.height,
        root.children[1].dimensions.content.y - root.dimensions.content.y,
    )
}

/// `height: fit-content` is content-sized, so the automatic minimum (§4.5)
/// floors a basis-0 column item at its content exactly as `height: auto`
/// does. Chrome 148: the item is 80 tall and the next box sits at 80; the
/// item was 0 tall with the next box drawn over its content.
#[test]
fn a_fit_content_height_column_item_keeps_its_content() {
    for collapse in [false, true] {
        for grid in [false, true] {
            let (h, next_y) = fit_content_item_in_auto_column(grid, collapse);
            assert!(
                (h - 80.0).abs() < 0.5 && (next_y - 80.0).abs() < 0.5,
                "grid={grid} collapse={collapse}: Chrome 148 has the item 80 tall and the \
                 next box at 80; got {h} and {next_y}"
            );
        }
    }
}

/// A `height: fit-content` flex CONTAINER is sized by its lines, as an
/// auto-height one is. Six 60x32 pills fit one line of a 400px row, with or
/// without `flex-wrap`: Chrome 148 has every pill and the container 32 tall.
/// The block pre-pass had stacked the pills to 192, and counted as a definite
/// height that became the container's height and its line's stretch target.
#[test]
fn a_fit_content_height_flex_container_is_sized_by_its_lines() {
    for collapse in [false, true] {
        for wrap in [false, true] {
            let mut s = ComputedStyle::new();
            s.display = Display::Flex;
            s.height = Length::FitContent;
            if wrap {
                s.flex_wrap = rustkit_css::FlexWrap::Wrap;
            }
            let mut c = LayoutBox::new(BoxType::Block, s);
            for _ in 0..6 {
                let mut pill_s = ComputedStyle::new();
                pill_s.width = Length::Px(60.0);
                pill_s.min_height = Length::Px(32.0);
                c.children.push(LayoutBox::new(BoxType::Block, pill_s));
            }
            let root = laid_out(c, collapse);
            let pill = root.children[0].dimensions.content.height;
            let h = root.dimensions.content.height;
            assert!(
                (pill - 32.0).abs() < 0.5 && (h - 32.0).abs() < 0.5,
                "wrap={wrap} collapse={collapse}: Chrome 148 has the pills and the container \
                 32 tall; got pill {pill}, container {h}"
            );
        }
    }
}

/// The shelf (hiwave-app `ui/shelf.html`), with Chrome 148's rects from
/// `baselines/chrome-148/builtins/shelf/layout-rects.json`: a 120px column
/// body, a 41px header, and a `flex: 1` palette (padding 12) holding a 43px
/// input row (margin-bottom 12) and a `flex: 1; overflow-y: auto` results
/// box whose content is 56. The palette's automatic minimum is its content,
/// 135, so it overflows the body; the results box keeps its 56.
#[test]
fn the_shelf_palette_overflows_at_its_content_height() {
    for collapse in [false, true] {
        let mut pal_s = zero_pct();
        pal_s.display = Display::Flex;
        pal_s.flex_direction = rustkit_css::FlexDirection::Column;
        pal_s.padding_top = Length::Px(12.0);
        pal_s.padding_bottom = Length::Px(12.0);
        let mut pal = LayoutBox::new(BoxType::Block, pal_s);
        let mut input = sized_block(100.0, 43.0);
        input.style.margin_bottom = Length::Px(12.0);
        let mut res_s = zero_pct();
        res_s.overflow_y = rustkit_css::Overflow::Auto;
        let mut res = LayoutBox::new(BoxType::Block, res_s);
        res.children.push(sized_block(100.0, 56.0));
        pal.children.push(input);
        pal.children.push(res);
        let mut body = LayoutBox::new(BoxType::Block, column(120.0));
        body.children.push(sized_block(100.0, 41.0));
        body.children.push(pal);
        let root = laid_out(body, collapse);
        let pal = &root.children[1];
        let pal_h = pal.dimensions.border_box().height;
        let res_h = pal.children[1].dimensions.content.height;
        assert!(
            (pal_h - 135.0).abs() < 0.5 && (res_h - 56.0).abs() < 0.5,
            "Chrome 148: palette 135, results 56; got palette {pal_h}, results {res_h}"
        );
    }
}

/// x.com's shape: a `flex: 1` main row in a definite column, holding an
/// auto-height `min-height: 50px` wrapper whose child is `height: 100%`,
/// then a 20px footer. The percentage behaves as `auto` (CSS 2.1 §10.5), so
/// the row's automatic minimum is its 50px content: it takes the 180 left
/// by the footer, and the footer sits at 180 inside the 200 column.
#[test]
fn a_percentage_height_descendant_does_not_raise_the_automatic_minimum() {
    for collapse in [false, true] {
        let mut full_s = ComputedStyle::new();
        full_s.height = Length::Percent(100.0);
        let mut full = LayoutBox::new(BoxType::Block, full_s);
        full.children.push(sized_block(10.0, 30.0));
        let mut wrap_s = ComputedStyle::new();
        wrap_s.min_height = Length::Px(50.0);
        let mut wrap = LayoutBox::new(BoxType::Block, wrap_s);
        wrap.children.push(full);
        let mut main = LayoutBox::new(BoxType::Block, zero_pct());
        main.children.push(wrap);
        let mut c = LayoutBox::new(BoxType::Block, column(200.0));
        c.children.push(main);
        c.children.push(sized_block(100.0, 20.0));
        let root = laid_out(c, collapse);
        let main_h = root.children[0].dimensions.content.height;
        let footer_y = root.children[1].dimensions.content.y - root.dimensions.content.y;
        assert!(
            (main_h - 180.0).abs() < 0.5 && (footer_y - 180.0).abs() < 0.5,
            "main row 180 with the footer at 180; got main {main_h}, footer at {footer_y}"
        );
    }
}

/// A `box-sizing: border-box` item with `flex: 1 1 0%` that may go below its
/// content (`overflow: hidden` or `min-width: 0`), with `edge` px of padding
/// or border on each side of the main axis, holding a `w` x 10 box.
fn clipped_border_box_item(edge: f32, border: bool, min_width_zero: bool, w: f32) -> LayoutBox {
    let mut s = zero_pct();
    s.box_sizing = rustkit_css::BoxSizing::BorderBox;
    if min_width_zero {
        s.min_width = Length::Px(0.0);
    } else {
        s.overflow_x = rustkit_css::Overflow::Hidden;
        s.overflow_y = rustkit_css::Overflow::Hidden;
    }
    if border {
        s.border_left_width = Length::Px(edge);
        s.border_right_width = Length::Px(edge);
        s.border_top_width = Length::Px(edge);
        s.border_bottom_width = Length::Px(edge);
    } else {
        s.padding_left = Length::Px(edge);
        s.padding_right = Length::Px(edge);
    }
    let mut b = LayoutBox::new(BoxType::Block, s);
    b.children.push(sized_block(w, 10.0));
    b
}

fn assert_row_143(got: &[(f32, f32)], want: &[(f32, f32)], what: &str) {
    let close = got.len() == want.len()
        && got.iter().zip(want).all(|(g, w)| (g.0 - w.0).abs() < 0.5 && (g.1 - w.1).abs() < 0.5);
    assert!(close, "{what}: the oracle Chromium 143 has content (x, width) {want:?}, got {got:?}");
}

/// ebay's search field (hand test H15): `flex: 1; overflow: hidden` with a
/// border, under `* { box-sizing: border-box }`. The basis of 0 is a
/// border-box size, so the flex base size is the item's own border and
/// padding; the hypothetical size stayed at 0, which is below that base, and
/// "a base past the hypothetical size" froze the item before it could grow.
/// It kept its 4px of border in a 400px row, and what followed it was placed
/// as if it were 0 wide. Measured in a 400px row (`getBoundingClientRect`).
#[test]
fn a_border_box_item_with_a_zero_basis_grows_from_its_padding_and_border() {
    for collapse in [false, true] {
        // One item with a 2px border: the whole row.
        assert_row_143(
            &row_of(vec![clipped_border_box_item(2.0, true, false, 100.0)], collapse),
            &[(2.0, 396.0)],
            "one clipped item, border 2",
        );
        // `min-width: 0` and 10px of padding beside a fixed 100px box.
        assert_row_143(
            &row_of(vec![clipped_border_box_item(10.0, false, true, 50.0), sized_block(100.0, 10.0)], collapse),
            &[(10.0, 280.0), (300.0, 100.0)],
            "min-width 0, padding 10, then a fixed box",
        );
        // Two of them share the row.
        assert_row_143(
            &row_of(
                vec![clipped_border_box_item(10.0, false, false, 50.0), clipped_border_box_item(10.0, false, false, 50.0)],
                collapse,
            ),
            &[(10.0, 180.0), (210.0, 180.0)],
            "two clipped items, padding 10",
        );
        // `flex: 1 1 auto; width: 0`: the same base by the other route.
        let mut by_width = clipped_border_box_item(10.0, false, false, 50.0);
        by_width.style.flex_basis = FlexBasis::Auto;
        by_width.style.width = Length::Px(0.0);
        assert_row_143(
            &row_of(vec![by_width, sized_block(100.0, 10.0)], collapse),
            &[(10.0, 280.0), (300.0, 100.0)],
            "width 0 with an auto basis",
        );
        // No grow factor: the item is its padding, and the next box starts
        // after it (it started at 0, on top of the item).
        let mut inflexible = clipped_border_box_item(10.0, false, false, 50.0);
        inflexible.style.flex_grow = 0.0;
        assert_row_143(
            &row_of(vec![inflexible, sized_block(100.0, 10.0)], collapse),
            &[(10.0, 0.0), (20.0, 100.0)],
            "flex-grow 0",
        );
    }
}

/// The same on the vertical axis: in a 200px column the item takes what the
/// 20px footer leaves, 180 border-box, so 176 inside its 2px borders.
#[test]
fn a_border_box_column_item_with_a_zero_basis_grows_from_its_border() {
    for collapse in [false, true] {
        let mut c = LayoutBox::new(BoxType::Block, column(200.0));
        c.children.push(clipped_border_box_item(2.0, true, false, 50.0));
        c.children.push(sized_block(100.0, 20.0));
        let root = laid_out(c, collapse);
        let item_h = root.children[0].dimensions.content.height;
        let footer_y = root.children[1].dimensions.content.y - root.dimensions.content.y;
        assert!(
            (item_h - 176.0).abs() < 0.5 && (footer_y - 180.0).abs() < 0.5,
            "the oracle Chromium 143 has the item 176 tall inside its borders and the footer at 180; got {item_h}, footer at {footer_y}"
        );
    }
}
