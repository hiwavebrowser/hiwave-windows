//! A flex item or a grid item establishes an independent formatting context
//! (css-flexbox-1 §3, css-grid-1 §6), so its auto height encloses its floats.
//! Before #1079 a float-only item was 0 tall. Engine `float_in_items_tests`
//! pins the full Chromium page under macos+headless; these layout units run
//! on default Linux `--lib`.
//!
//! Expected heights match Chrome for Testing 148.0.7778.216 on
//! `docs/diagnostics/2026-11-11/float_in_flex_grid_items.html` (float-only
//! cases r1i / g1i: 30px). The floated-grid side-by-side pin is GR1's
//! `layout_float` re-pass (`100px 300px` tracks).

use super::*;
use rustkit_css::{AlignItems, Display, GridTemplate, TrackDefinition, TrackSize};

fn float_box(w: f32, h: f32) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::Px(w);
    s.height = Length::Px(h);
    s.float = Float::Left;
    LayoutBox::with_float(BoxType::Block, s, Float::Left)
}

fn lay(root: &mut LayoutBox, width: f32) {
    root.set_viewport(1280.0, 800.0);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, width, 0.0),
        ..Default::default()
    };
    let mut mc = MarginCollapseContext::new();
    let mut fc = FloatContext::new();
    root.layout_with_collapse(&cb, &mut mc, &mut fc);
}

/// #1079: a float-only flex item is as tall as its float (30), not 0.
#[test]
fn a_flex_item_encloses_a_float_only_child() {
    let mut row_s = ComputedStyle::new();
    row_s.display = Display::Flex;
    row_s.align_items = AlignItems::FlexStart;
    let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    item.children.push(float_box(10.0, 30.0));
    let mut row = LayoutBox::new(BoxType::Block, row_s);
    row.children.push(item);
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(row);
    lay(&mut body, 400.0);
    let item = body.children[0].children[0].dimensions.border_box();
    assert!(
        (item.height - 30.0).abs() < 0.5 && (item.width - 10.0).abs() < 0.5,
        "flex float-only item {}x{}; Chromium 10x30",
        item.width,
        item.height
    );
}

/// #1079: a float-only grid item is as tall as its float (30), not 0.
#[test]
fn a_grid_item_encloses_a_float_only_child() {
    let mut grid_s = ComputedStyle::new();
    grid_s.display = Display::Grid;
    grid_s.grid_template_columns = GridTemplate {
        tracks: vec![TrackDefinition::simple(TrackSize::Px(400.0))],
        ..Default::default()
    };
    let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    item.children.push(float_box(10.0, 30.0));
    let mut grid = LayoutBox::new(BoxType::Block, grid_s);
    grid.children.push(item);
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(grid);
    lay(&mut body, 400.0);
    let item = body.children[0].children[0].dimensions.border_box();
    assert!(
        (item.height - 30.0).abs() < 0.5,
        "grid float-only item height {}; Chromium 30",
        item.height
    );
}

/// Negative control: a plain in-flow block does not enclose a float-only
/// child (height stays 0). The flex/grid pins above must not leak here.
#[test]
fn a_plain_block_does_not_enclose_a_float_only_child() {
    let mut block = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    block.children.push(float_box(10.0, 30.0));
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(block);
    lay(&mut body, 400.0);
    let h = body.children[0].dimensions.border_box().height;
    assert!(h.abs() < 0.5, "plain block with only a float should be 0 tall, got {h}");
}

/// GR1: a floated grid container lays its items in columns, not stacked
/// as blocks (`float: left; display: grid; grid-template-columns: 100px 300px`).
#[test]
fn a_floated_grid_lays_its_items_side_by_side() {
    let mut grid_s = ComputedStyle::new();
    grid_s.display = Display::Grid;
    grid_s.float = Float::Left;
    grid_s.grid_template_columns = GridTemplate {
        tracks: vec![
            TrackDefinition::simple(TrackSize::Px(100.0)),
            TrackDefinition::simple(TrackSize::Px(300.0)),
        ],
        ..Default::default()
    };
    let mut a_s = ComputedStyle::new();
    a_s.height = Length::Px(20.0);
    let mut b_s = ComputedStyle::new();
    b_s.height = Length::Px(20.0);
    let mut grid = LayoutBox::with_float(BoxType::Block, grid_s, Float::Left);
    grid.children.push(LayoutBox::new(BoxType::Block, a_s));
    grid.children.push(LayoutBox::new(BoxType::Block, b_s));
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(grid);
    lay(&mut body, 500.0);
    let grid = &body.children[0];
    let a = grid.children[0].dimensions.border_box();
    let b = grid.children[1].dimensions.border_box();
    assert!(
        (a.x - 0.0).abs() < 0.5
            && (a.width - 100.0).abs() < 0.5
            && (b.x - 100.0).abs() < 0.5
            && (b.width - 300.0).abs() < 0.5
            && (a.y - b.y).abs() < 0.5,
        "floated grid stacked or wrong sizes: a=({:.1},{:.1},{:.1}x{:.1}) b=({:.1},{:.1},{:.1}x{:.1})",
        a.x, a.y, a.width, a.height, b.x, b.y, b.width, b.height
    );
}
