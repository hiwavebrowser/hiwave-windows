//! A flex container whose `height` is in a font-relative or viewport unit.
//! The flex pass read a definite height from `px` only. With `height: 3rem`
//! (Tailwind's `h-12`, on almost every nav row and button) the container
//! came out 48 tall, but its items were aligned and stretched in a height it
//! did not have: `align-items: center` and `flex-end` left them at the top,
//! `stretch` left an auto item 0 tall, and a column neither justified nor
//! grew its items. Every expected number was measured on the oracle Chromium
//! 143 at a 1280x800 viewport with `font: 14px Arial`
//! (`getBoundingClientRect` of the item); each case runs through both entry
//! points, `layout` and `layout_with_collapse`.

use super::*;
use rustkit_css::{AlignItems, AlignSelf, Display, FlexBasis, FlexDirection, JustifyContent};

fn container(height: Length, edit: impl Fn(&mut ComputedStyle)) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.display = Display::Flex;
    s.width = Length::Px(176.0);
    s.height = height;
    s.font_size = Length::Px(14.0);
    edit(&mut s);
    s
}

fn item(width: f32, height: Option<f32>) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::Px(width);
    if let Some(h) = height {
        s.height = Length::Px(h);
    }
    LayoutBox::new(BoxType::Block, s)
}

/// The first item's content box `(y, height)` inside a block `<body>`.
fn first_item(style: ComputedStyle, items: Vec<LayoutBox>, collapse: bool) -> (f32, f32) {
    let mut outer = LayoutBox::new(BoxType::Block, style);
    outer.children = items;
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(outer);
    body.set_viewport(1280.0, 800.0);
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
    let c = body.children[0].children[0].dimensions.content;
    (c.y, c.height)
}

fn assert_item(
    name: &str,
    style: impl Fn() -> ComputedStyle,
    items: impl Fn() -> Vec<LayoutBox>,
    want: (f32, f32),
) {
    for collapse in [false, true] {
        let got = first_item(style(), items(), collapse);
        assert!(
            (got.0 - want.0).abs() < 0.5 && (got.1 - want.1).abs() < 0.5,
            "{name} (collapse = {collapse}): item at y {} and {} tall, Chromium has y {} and {} tall",
            got.0,
            got.1,
            want.0,
            want.1
        );
    }
}

fn square() -> Vec<LayoutBox> {
    vec![item(24.0, Some(24.0))]
}

#[test]
fn a_row_with_a_rem_height_aligns_its_items_in_that_height() {
    assert_item(
        "height: 3rem; align-items: center",
        || container(Length::Rem(3.0), |s| s.align_items = AlignItems::Center),
        square,
        (12.0, 24.0),
    );
    assert_item(
        "height: 3rem; align-items: flex-end",
        || container(Length::Rem(3.0), |s| s.align_items = AlignItems::FlexEnd),
        square,
        (24.0, 24.0),
    );
    assert_item(
        "height: 3rem; the item has align-self: flex-end",
        || container(Length::Rem(3.0), |_| {}),
        || {
            let mut b = item(24.0, Some(24.0));
            b.style.align_self = AlignSelf::FlexEnd;
            vec![b]
        },
        (24.0, 24.0),
    );
}

#[test]
fn a_row_with_a_rem_height_stretches_an_auto_item_to_it() {
    assert_item(
        "height: 3rem; an item with no height",
        || container(Length::Rem(3.0), |_| {}),
        || vec![item(24.0, None)],
        (0.0, 48.0),
    );
}

#[test]
fn em_and_viewport_heights_are_definite_too() {
    assert_item(
        "height: 3em at a 20px font; align-items: center",
        || {
            container(Length::Em(3.0), |s| {
                s.font_size = Length::Px(20.0);
                s.align_items = AlignItems::Center;
            })
        },
        square,
        (18.0, 24.0),
    );
    assert_item(
        "height: 10vh; align-items: center",
        || container(Length::Vh(10.0), |s| s.align_items = AlignItems::Center),
        square,
        (28.0, 24.0),
    );
    assert_item(
        "height: min(3rem, 100px); align-items: center",
        || {
            container(
                Length::Min(Box::new((Length::Rem(3.0), Length::Px(100.0)))),
                |s| s.align_items = AlignItems::Center,
            )
        },
        square,
        (12.0, 24.0),
    );
}

#[test]
fn a_border_box_row_takes_its_padding_out_of_the_rem_height() {
    assert_item(
        "box-sizing: border-box; height: 3rem; padding: 4px; align-items: center",
        || {
            container(Length::Rem(3.0), |s| {
                s.box_sizing = BoxSizing::BorderBox;
                s.padding_top = Length::Px(4.0);
                s.padding_bottom = Length::Px(4.0);
                s.align_items = AlignItems::Center;
            })
        },
        square,
        (12.0, 24.0),
    );
}

#[test]
fn a_column_with_a_rem_height_justifies_and_grows_its_items_in_it() {
    let column = |edit: fn(&mut ComputedStyle)| {
        move || {
            container(Length::Rem(3.0), |s| {
                s.flex_direction = FlexDirection::Column;
                edit(s);
            })
        }
    };
    assert_item(
        "column; height: 3rem; justify-content: center",
        column(|s| s.justify_content = JustifyContent::Center),
        square,
        (12.0, 24.0),
    );
    assert_item(
        "column; height: 3rem; justify-content: flex-end",
        column(|s| s.justify_content = JustifyContent::FlexEnd),
        square,
        (24.0, 24.0),
    );
    assert_item(
        "column; height: 3rem; an item with flex: 1",
        column(|_| {}),
        || {
            let mut b = item(24.0, None);
            b.style.flex_grow = 1.0;
            b.style.flex_shrink = 1.0;
            b.style.flex_basis = FlexBasis::Percent(0.0);
            vec![b]
        },
        (0.0, 48.0),
    );
}

/// Passes before the fix: the same shapes with the height in `px`.
#[test]
fn a_pixel_height_is_unchanged() {
    assert_item(
        "height: 48px; align-items: center",
        || container(Length::Px(48.0), |s| s.align_items = AlignItems::Center),
        square,
        (12.0, 24.0),
    );
    assert_item(
        "height: 48px; an item with no height",
        || container(Length::Px(48.0), |_| {}),
        || vec![item(24.0, None)],
        (0.0, 48.0),
    );
    assert_item(
        "height: auto; align-items: center",
        || container(Length::Auto, |s| s.align_items = AlignItems::Center),
        square,
        (0.0, 24.0),
    );
}
