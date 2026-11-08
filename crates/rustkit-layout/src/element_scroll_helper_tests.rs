//! Pure pins for the #845/#846 overflow helpers that gate element scroll,
//! paint clip, and wheel routing. Headless element_scroll / wheel_routing
//! suites cover end-to-end but need `--features headless`; these run on
//! default `cargo test -p rustkit-layout --lib`.

use super::*;
use rustkit_css::Overflow;

fn block(overflow_x: Overflow, overflow_y: Overflow) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.overflow_x = overflow_x;
    s.overflow_y = overflow_y;
    let mut b = LayoutBox::new(BoxType::Block, s);
    b.node_id = Some(1);
    b
}

fn tagged(tag: &str, overflow_x: Overflow, overflow_y: Overflow) -> LayoutBox {
    let mut b = block(overflow_x, overflow_y);
    b.set_identity(ElementIdentity {
        element_id: 1,
        tag: tag.to_string(),
        selector: tag.to_string(),
    });
    b
}

#[test]
fn overflow_auto_scroll_hidden_make_an_element_scroller_and_clip_does_not() {
    use Overflow::*;
    for o in [Auto, Scroll, Hidden] {
        let b = block(o, Visible);
        assert!(b.is_element_scroller(), "{o:?}");
        assert!(b.clips_its_content(), "{o:?}");
    }
    let clip = block(Clip, Clip);
    assert!(!clip.is_element_scroller(), "clip is not a scroller");
    assert!(clip.clips_its_content(), "clip still cuts overflow");
    let visible = block(Visible, Visible);
    assert!(!visible.is_element_scroller());
    assert!(!visible.clips_its_content());
}

#[test]
fn body_and_html_are_not_element_scrollers() {
    for tag in ["body", "html"] {
        let b = tagged(tag, Overflow::Auto, Overflow::Auto);
        assert!(
            !b.is_element_scroller(),
            "{tag} overflow scrolls the viewport"
        );
    }
    let div = tagged("div", Overflow::Auto, Overflow::Auto);
    assert!(div.is_element_scroller());
}

#[test]
fn a_box_without_a_node_id_is_not_an_element_scroller() {
    let mut b = block(Overflow::Auto, Overflow::Auto);
    b.node_id = None;
    assert!(!b.is_element_scroller());
}

#[test]
fn wheel_axes_follow_hidden_and_the_visible_peer_rule() {
    use Overflow::*;
    assert_eq!(block(Hidden, Hidden).wheel_axes(), (false, false));
    assert_eq!(block(Auto, Auto).wheel_axes(), (true, true));
    assert_eq!(block(Scroll, Visible).wheel_axes(), (true, true), "visible next to scroll → auto");
    assert_eq!(block(Visible, Hidden).wheel_axes(), (true, false));
    assert_eq!(block(Clip, Auto).wheel_axes(), (false, true));
    assert_eq!(block(Visible, Visible).wheel_axes(), (false, false));
    assert_eq!(block(Visible, Clip).wheel_axes(), (false, false));
}
