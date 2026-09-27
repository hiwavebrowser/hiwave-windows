//! css-flexbox-1 §9.4.11 / §9.8: a flex item that is itself a flex container
//! lays its own items out against the size the OUTER flex gave it (stretched
//! on the cross axis, or flexed on the main axis), not against its content.
//! Every expected number was measured on pinned Chrome for Testing 148 at a
//! 400px viewport (`getBoundingClientRect`); each case runs through both
//! entry points, `layout` and `layout_with_collapse`.

use super::*;
use rustkit_css::{AlignItems, Display, FlexBasis, FlexDirection, JustifyContent};

fn flex(direction: FlexDirection) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.display = Display::Flex;
    s.flex_direction = direction;
    s
}

fn grow(mut s: ComputedStyle) -> ComputedStyle {
    s.flex_grow = 1.0;
    s.flex_shrink = 1.0;
    s.flex_basis = FlexBasis::Percent(0.0);
    s
}

fn centring(mut s: ComputedStyle) -> ComputedStyle {
    s.align_items = AlignItems::Center;
    s.justify_content = JustifyContent::Center;
    s
}

fn red_box() -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::Px(50.0);
    s.height = Length::Px(50.0);
    LayoutBox::new(BoxType::Block, s)
}

/// Lays `outer` out inside a block `<body>` (the page's shape) through the
/// named entry point, and returns it.
fn laid_out(outer: LayoutBox, collapse: bool) -> LayoutBox {
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(outer);
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

fn assert_at(b: &LayoutBox, want: (f32, f32), what: &str) {
    let got = (b.dimensions.content.x, b.dimensions.content.y);
    assert!(
        (got.0 - want.0).abs() < 0.5 && (got.1 - want.1).abs() < 0.5,
        "{what}: Chrome 148 puts the box at {want:?}, got {got:?}"
    );
}

/// `<div flex;height:1000><div flex;flex:1;center/center><box>`
#[test]
fn a_stretched_flex_item_centres_its_items_in_the_stretched_height() {
    for collapse in [false, true] {
        let mut outer_s = flex(FlexDirection::Row);
        outer_s.height = Length::Px(1000.0);
        let mut outer = LayoutBox::new(BoxType::Block, outer_s);
        let mut inner = LayoutBox::new(BoxType::Block, centring(grow(flex(FlexDirection::Row))));
        inner.children.push(red_box());
        outer.children.push(inner);
        let root = laid_out(outer, collapse);
        assert_eq!(root.children[0].dimensions.content.height, 1000.0);
        assert_at(&root.children[0].children[0], (175.0, 475.0), "row-stretch");
    }
}

/// `<div flex;column;height:1000><div flex;flex:1;center/center><box>`
#[test]
fn a_column_grown_flex_item_centres_its_items_in_the_grown_height() {
    for collapse in [false, true] {
        let mut outer_s = flex(FlexDirection::Column);
        outer_s.height = Length::Px(1000.0);
        let mut outer = LayoutBox::new(BoxType::Block, outer_s);
        let mut inner = LayoutBox::new(BoxType::Block, centring(grow(flex(FlexDirection::Row))));
        inner.children.push(red_box());
        outer.children.push(inner);
        let root = laid_out(outer, collapse);
        assert_at(&root.children[0].children[0], (175.0, 475.0), "column-grow");
    }
}

/// x.com's logo column: a `min-height` column, a `flex: 1 1 0%` row in it,
/// and a centring flex item stretched in that row.
#[test]
fn a_centring_item_in_a_row_grown_into_a_min_height_column_centres_in_the_grown_row() {
    for collapse in [false, true] {
        let mut outer_s = flex(FlexDirection::Column);
        outer_s.min_height = Length::Px(1000.0);
        let mut outer = LayoutBox::new(BoxType::Block, outer_s);
        let mut row = LayoutBox::new(BoxType::Block, grow(flex(FlexDirection::Row)));
        let mut left_s = grow(ComputedStyle::new());
        left_s.height = Length::Px(100.0);
        row.children.push(LayoutBox::new(BoxType::Block, left_s));
        let mut right_s = centring(grow(flex(FlexDirection::Row)));
        right_s.min_height = Length::Px(450.0);
        let mut right = LayoutBox::new(BoxType::Block, right_s);
        right.children.push(red_box());
        row.children.push(right);
        outer.children.push(row);
        let root = laid_out(outer, collapse);
        let b = &root.children[0].children[1].children[0];
        assert_at(b, (275.0, 475.0), "x-shape");
    }
}

/// An AUTO-height row: the stretch target is the tall sibling (300), known
/// only after the items are laid out, and the nested container re-centres.
#[test]
fn a_flex_item_stretched_in_an_auto_height_row_centres_in_the_stretched_height() {
    for collapse in [false, true] {
        let mut outer = LayoutBox::new(BoxType::Block, flex(FlexDirection::Row));
        let mut tall_s = ComputedStyle::new();
        tall_s.width = Length::Px(100.0);
        tall_s.height = Length::Px(300.0);
        outer.children.push(LayoutBox::new(BoxType::Block, tall_s));
        let mut inner = LayoutBox::new(BoxType::Block, centring(flex(FlexDirection::Row)));
        inner.children.push(red_box());
        outer.children.push(inner);
        let root = laid_out(outer, collapse);
        assert_eq!(root.children[1].dimensions.content.height, 300.0);
        assert_at(&root.children[1].children[0], (100.0, 125.0), "auto-row-stretch");
    }
}
