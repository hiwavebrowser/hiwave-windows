//! `min-height` on a row flex item that is not stretched. The flex pass
//! floors the item's cross size by it, but the item's box kept the height
//! its children flowed to: only a stretched item was written back. GitHub's
//! "Sign up" button (`display: flex; min-height: 2rem` in an
//! `align-items: center` row) was 27 to 30px tall for 32. Every expected
//! number was measured on the oracle Chromium 143 (`getBoundingClientRect`
//! of `<div style="display: flex; width: 400px; align-items: flex-start">
//! <div style="display: D; min-height: M; width: 42px"><div style="width:
//! 16px; height: 16px">`), with `font: 14px Arial`.

use super::*;
use rustkit_css::{AlignItems, Display};

fn item_height(display: Display, min_height: Length, inner: f32, align: AlignItems) -> f32 {
    let mut row_style = ComputedStyle::new();
    row_style.display = Display::Flex;
    row_style.width = Length::Px(400.0);
    row_style.align_items = align;
    row_style.font_size = Length::Px(14.0);

    let mut item_style = ComputedStyle::new();
    item_style.display = display;
    item_style.min_height = min_height;
    item_style.width = Length::Px(42.0);
    item_style.font_size = Length::Px(14.0);

    let mut kid_style = ComputedStyle::new();
    kid_style.width = Length::Px(16.0);
    kid_style.height = Length::Px(inner);

    let mut item = LayoutBox::new(BoxType::Block, item_style);
    item.children.push(LayoutBox::new(BoxType::Block, kid_style));
    let mut row = LayoutBox::new(BoxType::Block, row_style);
    row.children.push(item);

    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.set_viewport(1280.0, 800.0);
    row.set_viewport(1280.0, 800.0);
    body.children.push(row);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, 400.0, 0.0),
        ..Default::default()
    };
    let mut mc = MarginCollapseContext::new();
    let mut fc = FloatContext::new();
    body.layout_with_collapse(&cb, &mut mc, &mut fc);
    body.children[0].children[0].dimensions.content.height
}

#[test]
fn an_unstretched_row_item_is_as_tall_as_its_pixel_min_height() {
    for display in [Display::Block, Display::Flex, Display::Grid] {
        let got = item_height(display, Length::Px(32.0), 16.0, AlignItems::FlexStart);
        assert!(
            (got - 32.0).abs() < 0.5,
            "{display:?} item, min-height: 32px, flex-start: height {got}, Chromium has 32"
        );
    }
}

#[test]
fn an_unstretched_row_item_is_as_tall_as_its_rem_min_height() {
    for display in [Display::Block, Display::Flex, Display::Grid] {
        let got = item_height(display, Length::Rem(2.0), 16.0, AlignItems::FlexStart);
        assert!(
            (got - 32.0).abs() < 0.5,
            "{display:?} item, min-height: 2rem, flex-start: height {got}, Chromium has 32"
        );
    }
}

#[test]
fn a_stretched_item_and_content_taller_than_the_minimum_are_unchanged() {
    let got = item_height(Display::Block, Length::Px(32.0), 16.0, AlignItems::Stretch);
    assert!((got - 32.0).abs() < 0.5, "stretched, min-height: 32px: height {got}");
    let got = item_height(Display::Block, Length::Px(32.0), 60.0, AlignItems::FlexStart);
    assert!((got - 60.0).abs() < 0.5, "min-height: 32px under 60px of content: height {got}");
    let got = item_height(Display::Block, Length::Auto, 16.0, AlignItems::FlexStart);
    assert!((got - 16.0).abs() < 0.5, "no min-height: height {got}");
}
