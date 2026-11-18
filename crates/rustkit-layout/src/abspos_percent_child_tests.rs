//! A percentage height inside an absolutely positioned box resolves against
//! that box's USED height once `reanchor_absolute` has fixed it.
//!
//! On the flow path an abspos box is laid out against the static-position
//! stand-in (its height is the parent's flow cursor), so its children run
//! before its own height is known. An `inset: 0` layer (or one with
//! `height: 100%`) then stretched to its containing block in
//! `reanchor_absolute`, but its in-flow children kept what they had resolved
//! without a base: `height: 100%` fell back to the 800px viewport, and a
//! `height: 100%` grandchild under that took its content. A video player's
//! `absolute inset-0` > `relative h-full overflow-hidden` > `h-full` black
//! backdrop drew as a 32px strip clipped by an 800px box, the black bar the
//! batch A/B saw once LAY1 gave the player its 16:9 height.
//!
//! Expected numbers: Chrome for Testing 148.0.7778.216, headless,
//! `getBoundingClientRect`, a 1280x800 viewport, the page
//! `<div style="position: relative; width: 600px; height: 400px">
//!   <div id=ab style="position: absolute; {layer}">
//!     <div id=pl style="height: 100%"> <div id=hf style="height: 100%">
//!       <div style="height: 32px"></div></div></div></div></div>`.

use super::*;

fn block(height: Length) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.height = height;
    LayoutBox::new(BoxType::Block, s)
}

/// Lays the page out with `layer` as `#ab` and returns the content rects of
/// `#ab`, `#pl` and `#hf`, relative to the outer box's content origin, as
/// `(y, height)` pairs.
fn shape(mut layer: LayoutBox, pl_height: Length) -> [(f32, f32); 3] {
    let mut outer_style = ComputedStyle::new();
    outer_style.width = Length::Px(600.0);
    outer_style.height = Length::Px(400.0);
    let mut outer = LayoutBox::with_position(BoxType::Block, outer_style, Position::Relative);

    let mut hf = block(Length::Percent(100.0));
    hf.children.push(block(Length::Px(32.0)));
    let mut pl = block(pl_height);
    pl.children.push(hf);
    layer.children.push(pl);
    outer.children.push(layer);

    let viewport = Dimensions {
        content: Rect::new(0.0, 0.0, 1280.0, 800.0),
        ..Default::default()
    };
    outer.layout(&viewport);
    let oy = outer.dimensions.content.y;
    let ab = &outer.children[0];
    let pl = &ab.children[0];
    let hf = &pl.children[0];
    let r = |b: &LayoutBox| (b.dimensions.content.y - oy, b.dimensions.content.height);
    [r(ab), r(pl), r(hf)]
}

fn layer(style: ComputedStyle, offsets: [Option<f32>; 4]) -> LayoutBox {
    let mut b = LayoutBox::with_position(BoxType::Block, style, Position::Absolute);
    b.set_offsets(offsets[0], offsets[1], offsets[2], offsets[3]);
    b
}

/// `inset: 0`: the layer is 400 tall and both percentages are 400 (Chrome:
/// ab, pl and hf 600x400). The engine had pl 800 and hf 32.
#[test]
fn a_percent_height_child_of_an_inset_layer_fills_the_layer() {
    let l = layer(ComputedStyle::new(), [Some(0.0); 4]);
    assert_eq!(
        shape(l, Length::Percent(100.0)),
        [(0.0, 400.0), (0.0, 400.0), (0.0, 400.0)]
    );
}

/// `top: 0; left: 0; width: 100%; height: 100%`: the same 400 (Chrome).
#[test]
fn a_percent_height_child_of_a_percent_height_layer_fills_the_layer() {
    let mut s = ComputedStyle::new();
    s.width = Length::Percent(100.0);
    s.height = Length::Percent(100.0);
    let l = layer(s, [Some(0.0), None, None, Some(0.0)]);
    assert_eq!(
        shape(l, Length::Percent(100.0)),
        [(0.0, 400.0), (0.0, 400.0), (0.0, 400.0)]
    );
}

/// `top: 10px; bottom: 30px; left: 0; right: 0; padding: 5px`: the layer's
/// content box is 350 tall at y 15, and so are pl and hf (Chrome: ab border
/// box 360 at y 10, pl and hf 590x350 at y 15).
#[test]
fn a_percent_height_child_resolves_against_the_padded_layers_content_box() {
    let mut s = ComputedStyle::new();
    s.padding_top = Length::Px(5.0);
    s.padding_right = Length::Px(5.0);
    s.padding_bottom = Length::Px(5.0);
    s.padding_left = Length::Px(5.0);
    let l = layer(s, [Some(10.0), Some(0.0), Some(30.0), Some(0.0)]);
    assert_eq!(
        shape(l, Length::Percent(100.0)),
        [(15.0, 350.0), (15.0, 350.0), (15.0, 350.0)]
    );
}

/// `height: 50%` under an `inset: 0` layer is 200 (Chrome), and the
/// grandchild's 100% follows it. The engine had 400 (half the viewport).
#[test]
fn a_half_height_child_of_an_inset_layer_is_half_the_layer() {
    let l = layer(ComputedStyle::new(), [Some(0.0); 4]);
    assert_eq!(
        shape(l, Length::Percent(50.0)),
        [(0.0, 400.0), (0.0, 200.0), (0.0, 200.0)]
    );
}

/// Control: `top: 0; left: 0; right: 0` leaves the layer's height `auto`
/// (content-sized), so the percentages compute to `auto` and everything is
/// the 32px content (Chrome: ab, pl and hf 600x32).
#[test]
fn a_percent_height_child_of_a_content_sized_layer_is_its_content() {
    let l = layer(
        ComputedStyle::new(),
        [Some(0.0), Some(0.0), None, Some(0.0)],
    );
    assert_eq!(
        shape(l, Length::Percent(100.0)),
        [(0.0, 32.0), (0.0, 32.0), (0.0, 32.0)]
    );
}

/// `left: 0; right: 0; top: 0; aspect-ratio: 16 / 9` in a 640px-wide box
/// (Chrome for Testing 148.0.7778.216: ab, pl and hf 640x360): the ratio
/// makes the layer's height definite once its width is stretched, so
/// `height: 100%` children fill it. The engine sized the layer 360 and
/// left pl and hf at their 32px content.
#[test]
fn a_percent_height_child_of_an_aspect_ratio_layer_fills_the_layer() {
    let mut outer_style = ComputedStyle::new();
    outer_style.width = Length::Px(640.0);
    outer_style.height = Length::Px(500.0);
    let mut outer = LayoutBox::with_position(BoxType::Block, outer_style, Position::Relative);
    let mut s = ComputedStyle::new();
    s.aspect_ratio = Some(16.0 / 9.0);
    let mut l = layer(s, [Some(0.0), Some(0.0), None, Some(0.0)]);
    let mut hf = block(Length::Percent(100.0));
    hf.children.push(block(Length::Px(32.0)));
    let mut pl = block(Length::Percent(100.0));
    pl.children.push(hf);
    l.children.push(pl);
    outer.children.push(l);
    let viewport = Dimensions {
        content: Rect::new(0.0, 0.0, 1280.0, 800.0),
        ..Default::default()
    };
    outer.layout(&viewport);
    let ab = &outer.children[0];
    let heights = [
        ab.dimensions.content.height,
        ab.children[0].dimensions.content.height,
        ab.children[0].children[0].dimensions.content.height,
    ];
    assert_eq!(heights, [360.0, 360.0, 360.0]);
}

/// `n` nested `position: absolute; inset: 0` > `position: relative;
/// height: 100%` layers in a 600x400 relative box, the innermost holding a
/// `height: 100%` box. Returns how many boxes the layout visited and the
/// innermost box's height.
fn nested_layers(n: usize) -> (u64, f32) {
    let mut inner = block(Length::Percent(100.0));
    for _ in 0..n {
        let mut rel_style = ComputedStyle::new();
        rel_style.height = Length::Percent(100.0);
        let mut rel = LayoutBox::with_position(BoxType::Block, rel_style, Position::Relative);
        rel.children.push(inner);
        let mut l = layer(ComputedStyle::new(), [Some(0.0); 4]);
        l.children.push(rel);
        inner = l;
    }
    let mut outer_style = ComputedStyle::new();
    outer_style.width = Length::Px(600.0);
    outer_style.height = Length::Px(400.0);
    let mut outer = LayoutBox::with_position(BoxType::Block, outer_style, Position::Relative);
    outer.children.push(inner);
    let viewport = Dimensions {
        content: Rect::new(0.0, 0.0, 1280.0, 800.0),
        ..Default::default()
    };
    let before = crate::layout_visits();
    outer.layout(&viewport);
    let visits = crate::layout_visits() - before;
    let mut b = &outer;
    while let Some(c) = b.children.first() {
        b = c;
    }
    (visits, b.dimensions.content.height)
}

/// #1043 R1 nit 1: the relayout `reanchor_absolute` adds for a stretched
/// layer must not compound with nesting. Each level is three boxes; five
/// levels must cost about five times one level, not 2.5^5 times. The
/// innermost box fills the 400px box at every depth (Chrome 148: 400).
#[test]
fn nested_inset_layers_lay_out_in_time_linear_in_their_depth() {
    let (one, h1) = nested_layers(1);
    let (five, h5) = nested_layers(5);
    assert_eq!((h1, h5), (400.0, 400.0));
    assert!(
        five <= 6 * one,
        "five nested layers visited {five} boxes, one layer {one}: not linear in depth"
    );
}

/// #1043 R1 MED (review 5860844422): the re-anchor can change the layer's
/// WIDTH while its height stays, and the children must follow. A `relative;
/// width: 400px; height: 100px; padding: 0 20px` box: the flow pass sees its
/// 400px content box, the `inset: 0` layer is stretched to the 440px padding
/// box (Chrome for Testing 148.0.7778.216: layer, `height: 100%` child and
/// its `width: 100%` child all 440x100). Returns (layer, child, grandchild)
/// as (width, height).
fn padded_layer() -> [(f32, f32); 3] {
    let mut outer_style = ComputedStyle::new();
    outer_style.width = Length::Px(400.0);
    outer_style.height = Length::Px(100.0);
    outer_style.padding_left = Length::Px(20.0);
    outer_style.padding_right = Length::Px(20.0);
    let mut outer = LayoutBox::with_position(BoxType::Block, outer_style, Position::Relative);
    let mut hf_style = ComputedStyle::new();
    hf_style.width = Length::Percent(100.0);
    hf_style.height = Length::Percent(100.0);
    let hf = LayoutBox::new(BoxType::Block, hf_style);
    let mut pl = block(Length::Percent(100.0));
    pl.children.push(hf);
    let mut l = layer(ComputedStyle::new(), [Some(0.0); 4]);
    l.children.push(pl);
    outer.children.push(l);
    let viewport = Dimensions {
        content: Rect::new(0.0, 0.0, 1280.0, 800.0),
        ..Default::default()
    };
    outer.layout(&viewport);
    let ab = &outer.children[0];
    let pl = &ab.children[0];
    let hf = &pl.children[0];
    let r = |b: &LayoutBox| (b.dimensions.content.width, b.dimensions.content.height);
    [r(ab), r(pl), r(hf)]
}

#[test]
fn a_layer_stretched_wider_at_the_re_anchor_lays_its_children_out_at_the_new_width() {
    assert_eq!(padded_layer(), [(440.0, 100.0), (440.0, 100.0), (440.0, 100.0)]);
}
