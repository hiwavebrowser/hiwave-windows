//! Z plan CH2.B2: a `width: 0; overflow: hidden` box paints none of its
//! content. HiWave's chrome sidebar collapses to `width: 0 !important;
//! min-width: 0 !important` (`.sidebar.collapsed`, with `overflow: hidden`
//! and `position: relative` on `.sidebar`), and RustKit painted its sections,
//! buttons and text over the main area: `overflow_clip` treated an empty
//! padding box as "no clip" and pushed nothing, so the children painted
//! unclipped. Chromium 143.0.7499.4 paints no pixel of the content (the
//! reduced page: 0 red, 0 blue pixels at 300x200).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;
use rustkit_layout::{DisplayCommand, Rect};

const RED: (u8, u8, u8) = (255, 0, 0);
const BLUE: (u8, u8, u8) = (0, 0, 255);

fn page(side_extra: &str) -> String {
    format!(
        "<!doctype html><html><head><style>\
         html,body{{margin:0;height:100%;background:#fff}}\
         .row{{display:flex;height:100%}}\
         .side{{width:0;min-width:0;overflow:hidden;display:flex;flex-direction:column;\
         flex-shrink:0;background:#eee;{side_extra}}}\
         .main{{flex:1;background:#fff}}\
         </style></head><body><div class=row>\
         <aside class=side><section><div style='padding:8px;color:#000'>Workspaces</div>\
         <button style='background:rgb(255,0,0);width:120px;height:30px;border:0'>Open</button>\
         <div style='background:rgb(0,0,255);width:50px;height:20px'></div></section></aside>\
         <main class=main>main</main></div></body></html>"
    )
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

/// Each fill of `color`, with the clip it paints under (the intersection of
/// the clips open at that point; None when nothing clips it).
fn visible_fills(html: &str, color: (u8, u8, u8)) -> Vec<(Rect, Option<Rect>)> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 300, 200))
        .expect("view");
    engine.load_html(view, html).expect("load_html");
    let list = engine.views[&view].display_list.as_ref().expect("display list").commands.clone();
    let mut clips: Vec<Rect> = Vec::new();
    let mut out = Vec::new();
    for c in &list {
        match c {
            DisplayCommand::PushClip(r) => {
                let r = match clips.last() {
                    Some(top) => intersect(*top, *r),
                    None => *r,
                };
                clips.push(r);
            }
            DisplayCommand::PushClipRounded { rect, .. } => {
                let r = match clips.last() {
                    Some(top) => intersect(*top, *rect),
                    None => *rect,
                };
                clips.push(r);
            }
            DisplayCommand::PopClip => {
                clips.pop();
            }
            DisplayCommand::SolidColor(c, r) if (c.r, c.g, c.b) == color => {
                out.push((*r, clips.last().copied()));
            }
            _ => {}
        }
    }
    out
}

/// The area the fill can reach: infinite when nothing clips it.
fn clip_area(clip: Option<Rect>) -> f32 {
    clip.map_or(f32::INFINITY, |k| k.width * k.height)
}

#[test]
fn a_zero_width_overflow_hidden_box_paints_none_of_its_children() {
    for (case, extra) in [
        ("position: relative (the chrome sidebar)", "position:relative"),
        ("static", ""),
        ("height too", "height:100%"),
    ] {
        let html = page(extra);
        for (name, color) in [("red button", RED), ("blue box", BLUE)] {
            let fills = visible_fills(&html, color);
            assert!(
                !fills.is_empty(),
                "{case}: the {name} is laid out and painted somewhere (it is then clipped)"
            );
            for (rect, clip) in &fills {
                assert_eq!(
                    clip_area(*clip), 0.0,
                    "{case}: the {name} at {rect:?} paints under a clip with no area, \
                     not {clip:?} (Chromium 143: no pixel of a width:0 overflow:hidden \
                     box's content)"
                );
            }
        }
    }
}

/// The other axis, on a block: RustKit sizes a `display: flex` column with
/// `height: 0` by its content (84px here; Chromium 143: 0), a layout gap
/// outside this row, so the box under test is a block.
#[test]
fn a_zero_height_overflow_hidden_box_paints_none_of_its_children() {
    let html = page("").replace(
        "width:0;min-width:0;overflow:hidden;display:flex;flex-direction:column;",
        "height:0;min-height:0;overflow:hidden;display:block;",
    );
    assert!(html.contains("display:block;flex-shrink"), "the aside is a block now");
    let fills = visible_fills(&html, RED);
    assert!(!fills.is_empty(), "the red button is painted somewhere");
    for (rect, clip) in &fills {
        assert_eq!(
            clip_area(*clip), 0.0,
            "the red button at {rect:?} is under a clip with no area, not {clip:?}"
        );
    }
}

#[test]
fn a_wide_overflow_hidden_box_still_clips_to_its_padding_box() {
    let html = page("").replace("width:0;min-width:0;", "width:60px;");
    let fills = visible_fills(&html, RED);
    assert_eq!(fills.len(), 1, "one red fill");
    let (rect, clip) = fills[0];
    assert_eq!(rect.width, 120.0, "the button keeps its own width");
    let clip = clip.expect("the button paints under the column's overflow clip");
    assert_eq!(clip.width, 60.0, "the clip is the 60px-wide padding box: {clip:?}");
    assert!(clip.height > 30.0, "the clip is as tall as the column: {clip:?}");
}
