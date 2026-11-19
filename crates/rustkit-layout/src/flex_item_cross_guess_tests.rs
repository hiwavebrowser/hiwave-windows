//! A block flex item in an auto-height row is as tall as its children laid
//! out, not as the one-line guess (`line-height` of its own font) the
//! hypothetical cross size starts from. Step 11b only ever GREW an item to
//! its laid-out children, so a 16px-font item holding a 12px line kept the
//! 16px font's 18px line height: HiWave's shelf rows were 2px taller than
//! Chromium's (CB6b, `.shelf-item-header`, `align-items: flex-start`).
//!
//! Expected numbers are Chrome for Testing 148.0.7778.216
//! (`getBoundingClientRect`) for `<div style="display: flex; align-items:
//! flex-start; font: 16px Helvetica"><div><div style="height: H">` (a 16px
//! font line is 18px): the row and the item are both H tall, for H = 5
//! and 14.

use super::*;
use rustkit_css::{AlignItems, Display};

/// (row height, item height) for an item whose only child is `child_h` tall.
fn heights(child_h: f32) -> (f32, f32) {
    let mut row_style = ComputedStyle::new();
    row_style.display = Display::Flex;
    row_style.align_items = AlignItems::FlexStart;
    row_style.font_size = Length::Px(16.0);
    let mut item_style = ComputedStyle::new();
    item_style.font_size = Length::Px(16.0);
    let mut kid_style = ComputedStyle::new();
    kid_style.height = Length::Px(child_h);
    kid_style.font_size = Length::Px(16.0);

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
    let row = &body.children[0];
    (row.dimensions.border_box().height, row.children[0].dimensions.border_box().height)
}

#[test]
fn a_flex_start_item_shorter_than_its_font_line_sets_the_row_height() {
    for h in [5.0, 14.0] {
        let (row, item) = heights(h);
        assert!(
            (row - h).abs() < 0.5 && (item - h).abs() < 0.5,
            "child {h}px: row {row}, item {item}; Chromium has {h} and {h}"
        );
    }
}

/// (row height, [item heights]) for items whose only child is `kid_h[i]`
/// tall, with the row's `align-items`.
fn row_of(align: AlignItems, kid_h: &[f32]) -> (f32, Vec<f32>) {
    let mut row_style = ComputedStyle::new();
    row_style.display = Display::Flex;
    row_style.align_items = align;
    row_style.font_size = Length::Px(16.0);
    let mut row = LayoutBox::new(BoxType::Block, row_style);
    for &h in kid_h {
        let mut item_style = ComputedStyle::new();
        item_style.font_size = Length::Px(16.0);
        let mut kid_style = ComputedStyle::new();
        kid_style.height = Length::Px(h);
        kid_style.font_size = Length::Px(16.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        item.children.push(LayoutBox::new(BoxType::Block, kid_style));
        row.children.push(item);
    }
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
    let row = &body.children[0];
    (
        row.dimensions.border_box().height,
        row.children.iter().map(|i| i.dimensions.border_box().height).collect(),
    )
}

/// CB6c (c): the stretched variant. Under the default `align-items:
/// stretch` the line is the tallest laid-out item and every item stretches
/// to it, not to the one-line guess. Chrome for Testing 148, `font: 16px
/// Helvetica` rows: one item holding a 5px block is 5 (row 5); items
/// holding 5px and 9px blocks are both 9 (row 9).
#[test]
fn a_stretched_row_is_its_tallest_laid_out_item() {
    let (row, items) = row_of(AlignItems::Stretch, &[5.0]);
    assert!((row - 5.0).abs() < 0.5 && (items[0] - 5.0).abs() < 0.5, "one 5px item: row {row}, item {items:?}; Chromium 5, 5");
    let (row, items) = row_of(AlignItems::Stretch, &[5.0, 9.0]);
    assert!(
        (row - 9.0).abs() < 0.5 && items.iter().all(|h| (h - 9.0).abs() < 0.5),
        "5px and 9px items: row {row}, items {items:?}; Chromium 9, [9, 9]"
    );
}

/// CB6c (c): an item whose content flows to 0 (a `height: 0` child) is 0,
/// and so is its row, under both alignments (Chrome for Testing 148).
#[test]
fn an_item_whose_content_flows_to_zero_is_zero() {
    for align in [AlignItems::FlexStart, AlignItems::Stretch] {
        let (row, items) = row_of(align, &[0.0]);
        assert!(row.abs() < 0.5 && items[0].abs() < 0.5, "{align:?}: row {row}, item {items:?}; Chromium 0, 0");
    }
}


/// CB6c follow-up (#1080): a stretched item whose height comes from
/// `aspect-ratio` keeps that ratio height. Without the exclusion, step 11b
/// would shrink it to the short child's laid-out height (10) the way a
/// stretch-without-ratio item does. Portable stand-in for the headless
/// `flex_aspect_ratio_tests` cases that were 30 tall for Chrome's 294/40.
#[test]
fn a_stretched_aspect_ratio_item_keeps_its_ratio_height() {
    let mut row_style = ComputedStyle::new();
    row_style.display = Display::Flex;
    row_style.align_items = AlignItems::Stretch;
    row_style.font_size = Length::Px(16.0);
    let mut item_style = ComputedStyle::new();
    item_style.font_size = Length::Px(16.0);
    item_style.width = Length::Px(80.0);
    item_style.aspect_ratio = Some(2.0);
    let mut kid_style = ComputedStyle::new();
    kid_style.height = Length::Px(10.0);
    kid_style.font_size = Length::Px(16.0);
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
    let row = &body.children[0];
    let item = &row.children[0];
    let (iw, ih) = (
        item.dimensions.border_box().width,
        item.dimensions.border_box().height,
    );
    let rh = row.dimensions.border_box().height;
    assert!(
        (iw - 80.0).abs() < 0.5 && (ih - 40.0).abs() < 0.5 && (rh - 40.0).abs() < 0.5,
        "stretched aspect-ratio item {iw}x{ih} (row {rh}); want 80x40 (not shrunk to the 10px child)"
    );
}
