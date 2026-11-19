//! A stretched item in an `auto`-width column flex container whose used
//! width is 0. The container's inner width is still definite (it is a
//! stretched item of a definite-width column, css-flexbox-1 §9.8), so its
//! own stretched items take that 0, and their wider content overflows.
//! `has_definite_cross_size` read `width: auto` with a 0 used width as
//! indefinite and stretched the items to the widest item's content instead.
//!
//! HiWave's collapsed right sidebar is this shape (`aside { width: 0;
//! display: flex; flex-direction: column }` > padded column content >
//! `.right-sidebar-panel { display: flex; flex-direction: column }`): its
//! downloads panel came out 139.5 wide where Chromium has 0, which was most of
//! the chrome board's geometry diffs on the main chrome page (CB6).
//!
//! Every expected number was measured on Chrome for Testing 148.0.7778.216
//! (`getBoundingClientRect`) for `<div id=c style="width: 0; display: flex;
//! flex-direction: column; padding: 0 P"><div id=p style="display: flex;
//! flex-direction: column"><div id=t><div style="width: 136px; height:
//! 10px">` in a 400px wide body: P = 0 gives c 0, p 0, t 0; P = 12px gives
//! c 24 (x 0), p 0 (x 12), t 0 (x 12).

use super::*;
use rustkit_css::{Display, FlexDirection};

fn column(width: Length, padding_x: f32) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.display = Display::Flex;
    s.flex_direction = FlexDirection::Column;
    s.width = width;
    s.padding_left = Length::Px(padding_x);
    s.padding_right = Length::Px(padding_x);
    LayoutBox::new(BoxType::Block, s)
}

/// Border boxes (x, width) of c, p and t.
fn laid_out(padding_x: f32, collapse: bool) -> [(f32, f32); 3] {
    let mut kid_style = ComputedStyle::new();
    kid_style.width = Length::Px(136.0);
    kid_style.height = Length::Px(10.0);
    let mut t = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    t.children.push(LayoutBox::new(BoxType::Block, kid_style));
    let mut p = column(Length::Auto, 0.0);
    p.children.push(t);
    let mut c = column(Length::Px(0.0), padding_x);
    c.children.push(p);

    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(c);
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
    let c = &body.children[0];
    let p = &c.children[0];
    let t = &p.children[0];
    [c, p, t].map(|b| {
        let r = b.dimensions.border_box();
        (r.x, r.width)
    })
}

fn check(padding_x: f32, want: [(f32, f32); 3]) {
    for collapse in [false, true] {
        let got = laid_out(padding_x, collapse);
        for (name, (g, w)) in ["c", "p", "t"].iter().zip(got.iter().zip(want.iter())) {
            assert!(
                (g.0 - w.0).abs() < 0.5 && (g.1 - w.1).abs() < 0.5,
                "padding {padding_x} (collapse = {collapse}): {name} x {} width {}, Chromium has x {} width {}",
                g.0,
                g.1,
                w.0,
                w.1
            );
        }
    }
}

#[test]
fn a_zero_width_auto_column_stretches_its_items_to_zero() {
    check(0.0, [(0.0, 0.0), (0.0, 0.0), (0.0, 0.0)]);
}

#[test]
fn a_zero_width_auto_column_inside_padding_stretches_its_items_to_zero() {
    check(12.0, [(0.0, 24.0), (12.0, 0.0), (12.0, 0.0)]);
}
