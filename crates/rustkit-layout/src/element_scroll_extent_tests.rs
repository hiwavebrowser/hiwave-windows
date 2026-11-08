//! #850: portable pins for scroll overflow extents.
//!
//! `reach_as_overflow` / `escaped_reach` / `element_scroll_max` decide
//! scrollWidth/scrollHeight and scrollIntoView clamps. Headless
//! `scroll_into_view_tests` covers the page oracles but needs
//! `--features headless` on rustkit-engine; these pure LayoutBox pins run on
//! default `cargo test -p rustkit-layout --lib`. Independent of open #847's
//! `element_scroll_helper_tests` (scroller/clip/wheel flags only).

use super::*;
use rustkit_css::Overflow;

fn sized(overflow_x: Overflow, overflow_y: Overflow, content: Rect, padding: EdgeSizes) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.overflow_x = overflow_x;
    s.overflow_y = overflow_y;
    let mut b = LayoutBox::new(BoxType::Block, s);
    b.node_id = Some(1);
    b.dimensions.content = content;
    b.dimensions.padding = padding;
    b
}

fn child_at(x: f32, y: f32, w: f32, h: f32) -> LayoutBox {
    let mut b = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    b.dimensions.content = Rect::new(x, y, w, h);
    b
}

#[test]
fn element_scroll_max_counts_padding_end() {
    // Mirrors headless `#hid`: content 100x50, padding 7, child 200x100 → max (100, 50).
    let pad = EdgeSizes {
        top: 7.0,
        right: 7.0,
        bottom: 7.0,
        left: 7.0,
    };
    let mut scroller = sized(
        Overflow::Hidden,
        Overflow::Hidden,
        Rect::new(7.0, 7.0, 100.0, 50.0),
        pad,
    );
    scroller.children.push(child_at(7.0, 7.0, 200.0, 100.0));
    assert_eq!(scroller.element_scroll_max(), (100.0, 50.0));
}

#[test]
fn clipped_child_does_not_extend_outer_reach() {
    // Outer scroller sees a clipper's border box only; the tall in-flow
    // grandchild inside the clipper is not outer overflow (#850).
    let mut outer = sized(
        Overflow::Auto,
        Overflow::Auto,
        Rect::new(0.0, 0.0, 100.0, 50.0),
        EdgeSizes::default(),
    );
    let mut clipper = sized(
        Overflow::Clip,
        Overflow::Clip,
        Rect::new(0.0, 0.0, 100.0, 20.0),
        EdgeSizes::default(),
    );
    clipper.children.push(child_at(0.0, 0.0, 100.0, 100.0));
    outer.children.push(clipper);

    assert_eq!(outer.element_scroll_max(), (0.0, 0.0));

    // Without clipping, the tall child would make the outer scrollable.
    let mut open = sized(
        Overflow::Auto,
        Overflow::Auto,
        Rect::new(0.0, 0.0, 100.0, 50.0),
        EdgeSizes::default(),
    );
    let mut visible = sized(
        Overflow::Visible,
        Overflow::Visible,
        Rect::new(0.0, 0.0, 100.0, 20.0),
        EdgeSizes::default(),
    );
    visible.children.push(child_at(0.0, 0.0, 100.0, 100.0));
    open.children.push(visible);
    assert_eq!(open.element_scroll_max(), (0.0, 50.0));
}

#[test]
fn absolute_escaped_reach_past_a_static_clipper() {
    // An absolute under a static clipper escapes its clip and extends the
    // outer scroller (Chromium / #850 `o5` companion).
    let mut outer = sized(
        Overflow::Auto,
        Overflow::Auto,
        Rect::new(0.0, 0.0, 100.0, 50.0),
        EdgeSizes::default(),
    );
    let mut clipper = sized(
        Overflow::Auto,
        Overflow::Auto,
        Rect::new(0.0, 0.0, 100.0, 20.0),
        EdgeSizes::default(),
    );
    // Static clipper: position stays Static so holds_absolute_boxes is false.
    let mut abs = child_at(0.0, 200.0, 10.0, 10.0);
    abs.position = Position::Absolute;
    clipper.children.push(abs);
    outer.children.push(clipper);

    assert_eq!(outer.element_scroll_max(), (0.0, 160.0));
}
