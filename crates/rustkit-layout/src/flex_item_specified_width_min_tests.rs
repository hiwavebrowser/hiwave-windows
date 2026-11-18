//! css-flexbox-1 §4.5: a flex item's automatic minimum size is the SMALLER of
//! its specified size suggestion (a definite `width`) and its content size
//! suggestion (the min-content size of its CONTENT). The content suggestion
//! read the item's own `width: 24px` back as its min-content, so an item with
//! a pixel width never shrank below it: HiWave's collapsed right sidebar kept
//! its 24px close button 24 wide where Chromium shrinks it to its label
//! (CB6b, `#rightSidebarClose`).
//!
//! Expected numbers are Chrome for Testing 148.0.7778.216
//! (`getBoundingClientRect`) for `<div style="display: flex; width: W"><div
//! style="width: 24px; height: 24px"><div style="width: 3px; height: 3px">`:
//! W = 12px gives the item 12 (shrunk to the line, content 3 below it);
//! W = 0 gives 3 (shrunk to its content, which floors it).

use super::*;
use rustkit_css::Display;

fn item_width(row_width: f32) -> f32 {
    let mut row_style = ComputedStyle::new();
    row_style.display = Display::Flex;
    row_style.width = Length::Px(row_width);
    let mut item_style = ComputedStyle::new();
    item_style.width = Length::Px(24.0);
    item_style.height = Length::Px(24.0);
    let mut kid_style = ComputedStyle::new();
    kid_style.width = Length::Px(3.0);
    kid_style.height = Length::Px(3.0);

    let mut item = LayoutBox::new(BoxType::Block, item_style);
    item.children.push(LayoutBox::new(BoxType::Block, kid_style));
    let mut row = LayoutBox::new(BoxType::Block, row_style);
    row.children.push(item);
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(row);
    body.set_viewport(1280.0, 800.0);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, 400.0, 0.0),
        ..Default::default()
    };
    let mut mc = MarginCollapseContext::new();
    let mut fc = FloatContext::new();
    body.layout_with_collapse(&cb, &mut mc, &mut fc);
    body.children[0].children[0].dimensions.border_box().width
}

#[test]
fn a_pixel_width_item_shrinks_to_a_narrower_line() {
    let got = item_width(12.0);
    assert!((got - 12.0).abs() < 0.5, "width: 24px item in a 12px row: {got}, Chromium has 12");
}

#[test]
fn a_pixel_width_item_shrinks_to_its_content_in_a_zero_line() {
    let got = item_width(0.0);
    assert!((got - 3.0).abs() < 0.5, "width: 24px item in a 0px row: {got}, Chromium has 3");
}
