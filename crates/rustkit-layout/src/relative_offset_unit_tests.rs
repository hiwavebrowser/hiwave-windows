//! Portable pins for `position: relative` offsets (Z C13 / #843).
//!
//! The engine's page oracles in `relative_offset_tests` are all
//! `macos`+`headless`, so Linux `--lib` never exercised the shift rules or
//! the "apply once after the whole tree" contract that stopped facebook's
//! login inputs drifting 24px. These unit tests cover the pure layout path
//! that every CI lane runs.

use super::*;
use rustkit_css::Length;

fn viewport(w: f32, h: f32) -> Dimensions {
    Dimensions {
        content: Rect::new(0.0, 0.0, w, h),
        ..Default::default()
    }
}

fn static_block(x: f32, y: f32, w: f32, h: f32) -> LayoutBox {
    let mut b = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    b.dimensions.content = Rect::new(x, y, w, h);
    b
}

/// Engine transfer shape: layout `Position::Static`, computed style `Relative`.
fn relative_block(x: f32, y: f32, w: f32, h: f32) -> LayoutBox {
    let mut b = static_block(x, y, w, h);
    b.style.position = rustkit_css::Position::Relative;
    b
}

fn with_offsets(mut b: LayoutBox, top: Option<Length>, right: Option<Length>, bottom: Option<Length>, left: Option<Length>) -> LayoutBox {
    b.style.top = top;
    b.style.right = right;
    b.style.bottom = bottom;
    b.style.left = left;
    b
}

#[test]
fn relative_shift_takes_left_and_top_over_right_and_bottom() {
    let cb = viewport(200.0, 100.0);
    let left_top = with_offsets(
        relative_block(0.0, 0.0, 50.0, 50.0),
        Some(Length::Px(20.0)),
        Some(Length::Px(40.0)),
        Some(Length::Px(40.0)),
        Some(Length::Px(30.0)),
    );
    assert_eq!(left_top.relative_shift(&cb, true), (30.0, 20.0));

    let right_bottom = with_offsets(
        relative_block(0.0, 0.0, 50.0, 50.0),
        None,
        Some(Length::Px(30.0)),
        Some(Length::Px(20.0)),
        None,
    );
    assert_eq!(right_bottom.relative_shift(&cb, true), (-30.0, -20.0));
}

#[test]
fn a_percent_top_is_auto_when_the_containing_block_height_is_indefinite() {
    let cb = viewport(200.0, 100.0);
    let b = with_offsets(
        relative_block(0.0, 0.0, 50.0, 50.0),
        Some(Length::Percent(10.0)),
        None,
        None,
        Some(Length::Percent(10.0)),
    );
    // Width percent still resolves; height percent becomes auto.
    assert_eq!(b.relative_shift(&cb, false), (20.0, 0.0));
    assert_eq!(b.relative_shift(&cb, true), (20.0, 10.0));
}

#[test]
fn apply_relative_offsets_moves_the_subtree_and_leaves_siblings() {
    let cb = viewport(200.0, 200.0);
    let mut root = static_block(0.0, 0.0, 200.0, 200.0);

    let mut rel = with_offsets(
        relative_block(0.0, 0.0, 50.0, 50.0),
        Some(Length::Px(20.0)),
        None,
        None,
        Some(Length::Px(30.0)),
    );
    // Absolute descendant rides the shifted containing block.
    let mut abs = static_block(0.0, 0.0, 10.0, 10.0);
    abs.position = Position::Absolute;
    rel.children.push(abs);

    let sib = static_block(0.0, 50.0, 50.0, 50.0);
    root.children.push(rel);
    root.children.push(sib);

    root.apply_relative_offsets(&cb);

    let moved = &root.children[0];
    assert_eq!(
        (moved.dimensions.content.x, moved.dimensions.content.y),
        (30.0, 20.0),
        "the relative box itself"
    );
    assert_eq!(
        (
            moved.children[0].dimensions.content.x,
            moved.children[0].dimensions.content.y
        ),
        (30.0, 20.0),
        "its absolute child moves with it"
    );
    assert_eq!(
        (
            root.children[1].dimensions.content.x,
            root.children[1].dimensions.content.y
        ),
        (0.0, 50.0),
        "the static sibling stays where flow put it"
    );
}

/// The root entry applies offsets; the child entry used inside a pass does
/// not — so a flex/grid measure never sees a shifted item (facebook login
/// inputs, Top-10 title wrap).
#[test]
fn layout_applies_relative_offsets_once_and_the_child_entry_does_not() {
    let mut style = ComputedStyle::new();
    style.width = Length::Px(200.0);
    style.height = Length::Px(100.0);
    let mut root = LayoutBox::new(BoxType::Block, style);

    let mut child_style = ComputedStyle::new();
    child_style.position = rustkit_css::Position::Relative;
    child_style.top = Some(Length::Px(24.0));
    child_style.left = Some(Length::Px(24.0));
    child_style.width = Length::Px(50.0);
    child_style.height = Length::Px(50.0);
    root.children.push(LayoutBox::new(BoxType::Block, child_style));

    let cb = viewport(200.0, 100.0);

    // Child entry: static position only (no post-pass).
    let mut via_child = root.clone();
    via_child.layout_with_definite_height(&cb, cb.content.height);
    let static_pos = via_child.children[0].dimensions.content;
    assert_eq!(static_pos.x, 0.0, "left: 24px must not be applied yet");
    let mut shifted = via_child.clone();
    shifted.apply_relative_offsets(&cb);
    assert_eq!(
        (
            shifted.children[0].dimensions.content.x,
            shifted.children[0].dimensions.content.y
        ),
        (static_pos.x + 24.0, static_pos.y + 24.0),
        "the post-pass is what moves the box"
    );

    // Root entry: layout + post-pass in one call equals the manual shift.
    let mut via_root = root;
    via_root.layout(&cb);
    assert_eq!(
        (
            via_root.children[0].dimensions.content.x,
            via_root.children[0].dimensions.content.y
        ),
        (static_pos.x + 24.0, static_pos.y + 24.0),
        "layout() shifts after the tree is final"
    );
}

#[test]
fn a_nested_relative_box_shifts_against_its_parents_final_box() {
    let cb = viewport(300.0, 300.0);
    let mut root = static_block(0.0, 0.0, 300.0, 300.0);

    let mut outer = with_offsets(
        relative_block(10.0, 10.0, 100.0, 100.0),
        Some(Length::Px(5.0)),
        None,
        None,
        Some(Length::Px(5.0)),
    );
    let inner = with_offsets(
        relative_block(20.0, 20.0, 30.0, 30.0),
        Some(Length::Px(2.0)),
        None,
        None,
        Some(Length::Px(3.0)),
    );
    outer.children.push(inner);
    root.children.push(outer);

    root.apply_relative_offsets(&cb);

    let outer = &root.children[0];
    assert_eq!(
        (outer.dimensions.content.x, outer.dimensions.content.y),
        (15.0, 15.0)
    );
    // Inner starts at (20,20) inside outer; outer shift moves it to (25,25),
    // then its own (3,2) lands at (28,27).
    assert_eq!(
        (
            outer.children[0].dimensions.content.x,
            outer.children[0].dimensions.content.y
        ),
        (28.0, 27.0)
    );
}
