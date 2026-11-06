//! Paint order of flex and grid items with a `z-index` (CSS Flexbox §4.3,
//! CSS Grid §9.5): an item whose `z-index` is not `auto` paints as if it
//! were `position: relative`, even when it is `position: static`.
//!
//! netflix.com's hero holds the poster collage in an `absolute` layer and
//! the heading, subtitle and email form in a later `position: static;
//! z-index: 3` flex item of the same flex container. Chrome 148 paints the
//! item over the collage; RustKit painted it in normal flow, before the
//! absolute layer, so the collage covered the text and the form (hand test
//! 10-27, HT27F N1b). A static box that is not a flex or grid item keeps
//! ignoring its `z-index`, as in Chrome.

use super::*;
use rustkit_layout::DisplayCommand;

const IMAGE: (u8, u8, u8) = (200, 120, 60);
const FRONT: (u8, u8, u8) = (0, 255, 0);

fn fill_order(body: &str) -> Vec<&'static str> {
    let page = format!(
        "<!doctype html><html><head><style>body{{margin:0}}\
         .wrap{{position:relative;width:300px;height:200px}}\
         .img{{position:absolute;inset:0;width:100%;height:100%;background:rgb(200,120,60)}}\
         .front{{width:100px;height:50px;background:rgb(0,255,0)}}\
         </style></head><body>{body}</body></html>"
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 400, 300))
        .expect("view");
    engine.load_html(view, &page).expect("load_html");
    let list = engine.views[&view].display_list.as_ref().expect("display list").commands.clone();
    list.iter()
        .filter_map(|c| match c {
            DisplayCommand::SolidColor(c, _) if (c.r, c.g, c.b) == IMAGE => Some("image"),
            DisplayCommand::SolidColor(c, _) if (c.r, c.g, c.b) == FRONT => Some("front"),
            _ => None,
        })
        .collect()
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_static_flex_or_grid_item_with_a_z_index_paints_over_an_earlier_absolute_sibling() {
    for (case, body) in [
        (
            "flex item z-index 3 (netflix hero)",
            "<div class=wrap style='display:flex'><div class=img></div>\
             <div class=front style='z-index:3'></div></div>",
        ),
        (
            "grid item z-index 1",
            "<div class=wrap style='display:grid'><div class=img></div>\
             <div class=front style='z-index:1'></div></div>",
        ),
        (
            "flex item z-index 3, image one layer down",
            "<div class=wrap style='display:flex'><div style='position:absolute;inset:0'>\
             <div class=img></div></div><div class=front style='z-index:3'></div></div>",
        ),
    ] {
        assert_eq!(fill_order(body), ["image", "front"], "{case}");
    }
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_static_box_that_is_not_an_item_or_has_no_z_index_stays_in_normal_flow() {
    for (case, body) in [
        (
            "block child z-index 3",
            "<div class=wrap><div class=img></div><div class=front style='z-index:3'></div></div>",
        ),
        (
            "flex item z-index auto",
            "<div class=wrap style='display:flex'><div class=img></div><div class=front></div></div>",
        ),
    ] {
        assert_eq!(fill_order(body), ["front", "image"], "{case}");
    }
}
