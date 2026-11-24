//! An empty element block keeps its box, as in Chrome.
//!
//! The box tree used to drop an empty block with nothing visible about it
//! (`parent_keeps`). Chrome lays every element block out, and a dropped one
//! differs in what you can see as well as in what scripts read:
//! - `aaa<div></div>bbb` is two lines in Chrome; with the block dropped,
//!   RustKit put `aaabbb` on one;
//! - the block has no rect, so the geometry oracle cannot join Chrome's
//!   (the report page's `#error-container`: a 960x0 box in Chrome, a join
//!   failure on every report row of the Linux chrome board).
//!
//! Numbers: Chrome for Testing 148.0.7778.216 (linux64), `getBoundingClientRect`
//! on the same markup, 600x300 viewport.

use super::*;
use rustkit_layout::{Dimensions, Rect};

fn by_id<'a>(b: &'a LayoutBox, id: &str) -> Option<&'a LayoutBox> {
    if b.identity
        .as_ref()
        .is_some_and(|i| i.selector == format!("#{id}"))
    {
        return Some(b);
    }
    b.children.iter().find_map(|c| by_id(c, id))
}

fn laid_out(html: &str) -> LayoutBox {
    let e = Engine::new(EngineConfig::default()).expect("engine");
    let d = Document::parse_html(html).expect("parse");
    let mut root = e.build_layout_from_document(&d, &[]);
    root.layout_with_collapse(
        &Dimensions {
            content: Rect::new(0.0, 0.0, 600.0, 0.0),
            ..Default::default()
        },
        &mut rustkit_layout::MarginCollapseContext::new(),
        &mut rustkit_layout::FloatContext::new(),
    );
    root
}

fn rect(root: &LayoutBox, id: &str) -> (f32, f32, f32, f32) {
    let b = by_id(root, id).unwrap_or_else(|| panic!("#{id} has no box"));
    let r = b.dimensions.border_box();
    (r.x, r.y, r.width, r.height)
}

/// Chrome: before 0,0 400x18; empty 0,18 400x0; empty-styled 5,23 390x0;
/// after 0,23 400x18.
#[test]
fn an_empty_unstyled_block_keeps_its_box() {
    let root = laid_out(
        r#"<!doctype html><html><body style="margin:0;font:16px/18px sans-serif">
<div style="width:400px"><div id="before">x</div><div id="empty"></div>
<div id="empty-styled" style="margin:5px"></div><div id="after">y</div></div></body></html>"#,
    );
    assert_eq!(rect(&root, "empty"), (0.0, 18.0, 400.0, 0.0));
    // Keeping the box changes nothing around it: margins still collapse
    // through the empty blocks.
    assert_eq!(rect(&root, "empty-styled"), (5.0, 23.0, 390.0, 0.0));
    assert_eq!(rect(&root, "after"), (0.0, 23.0, 400.0, 18.0));
}

/// Chrome: wrap 0,0 400x40 (two lines); mid 0,20 400x0.
#[test]
fn an_empty_block_between_inline_content_breaks_the_line() {
    let root = laid_out(
        r#"<!doctype html><html><body style="margin:0;font:16px/20px sans-serif">
<div id="wrap" style="width:400px">aaa<div id="mid"></div>bbb</div></body></html>"#,
    );
    assert_eq!(rect(&root, "wrap"), (0.0, 0.0, 400.0, 40.0));
    assert_eq!(rect(&root, "mid"), (0.0, 20.0, 400.0, 0.0));
}

/// Keeping the box adds no space: an empty block's margins (and its
/// neighbours' and parent's) still collapse through it, and the box sits at
/// the top of what follows. Chrome:
/// - a 10px block with margin-bottom 20, an unstyled empty block, a block
///   with margin-top 15: empty at y=30, next at y=30;
/// - the same with the empty block's own margins 30/10: empty and next at
///   y=40 (the largest of 20, 30, 10, 15);
/// - an empty first child: the parent's top margin collapses through it with
///   the next child's 20px, so parent, empty block and child all start at
///   5 + 20 = 25.
#[test]
fn margins_still_collapse_through_a_kept_empty_block() {
    let root = laid_out(
        r#"<!doctype html><html><body style="margin:0"><div style="width:400px">
<div id="a1" style="height:10px;margin-bottom:20px"></div><div id="e1"></div><div id="b1" style="height:10px;margin-top:15px"></div>
</div></body></html>"#,
    );
    assert_eq!(rect(&root, "e1"), (0.0, 30.0, 400.0, 0.0));
    assert_eq!(rect(&root, "b1"), (0.0, 30.0, 400.0, 10.0));

    let root = laid_out(
        r#"<!doctype html><html><body style="margin:0"><div style="width:400px">
<div id="a2" style="height:10px;margin-bottom:20px"></div><div id="e2" style="margin-top:30px;margin-bottom:10px"></div><div id="b2" style="height:10px;margin-top:15px"></div>
</div></body></html>"#,
    );
    assert_eq!(rect(&root, "e2"), (0.0, 40.0, 400.0, 0.0));
    assert_eq!(rect(&root, "b2"), (0.0, 40.0, 400.0, 10.0));

    let root = laid_out(
        r#"<!doctype html><html><body style="margin:0"><div id="top" style="height:5px"></div><div id="p3" style="width:400px"><div id="e3"></div><div id="c3" style="margin-top:20px;height:10px"></div></div>
</body></html>"#,
    );
    assert_eq!(rect(&root, "p3"), (0.0, 25.0, 400.0, 10.0));
    assert_eq!(rect(&root, "e3"), (0.0, 25.0, 400.0, 0.0));
    assert_eq!(rect(&root, "c3"), (0.0, 25.0, 400.0, 10.0));
}
