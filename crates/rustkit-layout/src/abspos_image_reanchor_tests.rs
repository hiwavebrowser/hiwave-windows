//! An absolutely positioned replaced box is re-sized by `reanchor_absolute`
//! against the containing block it resolved (HT27 R3). On the flow path the
//! image is laid out against the static-position stand-in, whose height is
//! the parent's cursor, so `height: 100%` had no base and the image took its
//! natural ratio: railway's hero (`absolute inset-0 w-full h-full` inside an
//! `absolute inset-0` layer) was 1248x1248 for Chrome's 1248x1700. The
//! block path only re-ran `calculate_block_height`, which a replaced box
//! never uses.

use super::*;

fn image(natural: (f32, f32), style: ComputedStyle) -> LayoutBox {
    LayoutBox::with_position(
        BoxType::Image {
            url: String::new(),
            natural_width: natural.0,
            natural_height: natural.1,
        },
        style,
        Position::Absolute,
    )
}

/// `<div style="position: relative; width: 600px; height: 400px">
///   <div style="position: absolute; inset: 0"> {image} </div></div>`,
/// laid out through the real entry point; returns the image's content rect
/// relative to the outer box's content origin.
fn image_in_inset_layer(image: LayoutBox) -> Rect {
    let mut box_style = ComputedStyle::new();
    box_style.width = Length::Px(600.0);
    box_style.height = Length::Px(400.0);
    let mut outer = LayoutBox::with_position(BoxType::Block, box_style, Position::Relative);

    let mut layer = LayoutBox::with_position(BoxType::Block, ComputedStyle::new(), Position::Absolute);
    layer.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    layer.children.push(image);
    outer.children.push(layer);

    let viewport = Dimensions {
        content: Rect::new(0.0, 0.0, 1280.0, 800.0),
        ..Default::default()
    };
    outer.layout(&viewport);
    let origin = outer.dimensions.content;
    let r = outer.children[0].children[0].dimensions.content;
    Rect::new(r.x - origin.x, r.y - origin.y, r.width, r.height)
}

/// The railway shape: `width: 100%; height: 100%` fills the layer, 600x400,
/// not the 1:1 ratio's 600x600.
#[test]
fn a_percent_height_image_fills_the_inset_layer_it_is_anchored_to() {
    let mut s = ComputedStyle::new();
    s.width = Length::Percent(100.0);
    s.height = Length::Percent(100.0);
    let mut img = image((1.0, 1.0), s);
    img.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let r = image_in_inset_layer(img);
    assert_eq!((r.x, r.y, r.width, r.height), (0.0, 0.0, 600.0, 400.0));
}

/// `top` / `left` place the image, and a percent width is the layer's.
#[test]
fn an_abspos_image_takes_its_offsets_and_a_percent_width() {
    let mut s = ComputedStyle::new();
    s.width = Length::Percent(50.0);
    let mut img = image((2.0, 1.0), s);
    img.set_offsets(Some(50.0), None, None, Some(70.0));
    let r = image_in_inset_layer(img);
    assert_eq!((r.x, r.y, r.width, r.height), (70.0, 50.0, 300.0, 150.0));
}

/// An auto-sized replaced box between four offsets keeps its intrinsic
/// size (CSS 2.1 §10.3.8, §10.6.5); only a block stretches to `inset: 0`.
#[test]
fn an_auto_sized_abspos_image_with_inset_keeps_its_natural_size() {
    let mut img = image((30.0, 10.0), ComputedStyle::new());
    img.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let r = image_in_inset_layer(img);
    assert_eq!((r.x, r.y, r.width, r.height), (0.0, 0.0, 30.0, 10.0));
}

/// `max-height: 100%` on the re-anchored image resolves against the layer
/// too, and the width follows the ratio (the existing max-height rule).
#[test]
fn a_percent_max_height_on_an_abspos_image_resolves_against_the_layer() {
    let mut s = ComputedStyle::new();
    s.max_height = Length::Percent(100.0);
    let mut img = image((1000.0, 1000.0), s);
    img.set_offsets(Some(0.0), None, None, Some(0.0));
    let r = image_in_inset_layer(img);
    assert_eq!((r.width, r.height), (400.0, 400.0));
}
