//! Paint order of positioned boxes (CSS 2.1 Appendix E): a later
//! `position: relative` sibling with `z-index: 10` (or `auto`) paints above
//! an earlier `position: absolute` one.
//!
//! railway.com's hero holds the background image in an `absolute inset-0`
//! layer and its heading, buttons and dashboard card in a later
//! `relative z-10` block. Chrome 148 paints the block over the image (its
//! hit test at the heading returns the heading); RustKit paints the image
//! last, so the first screen is the picture alone (Pete's hand test
//! 10-27). Layout is identical in both engines on this page; only the
//! order of the fills differs.

use super::*;
use rustkit_layout::DisplayCommand;

const IMAGE: (u8, u8, u8) = (200, 120, 60);
const FRONT: (u8, u8, u8) = (0, 255, 0);

fn fill_order(body: &str) -> Vec<&'static str> {
    let page = format!(
        "<!doctype html><html><head><style>body{{margin:0}}\
         .wrap{{position:relative;width:300px;height:200px}}\
         .img{{position:absolute;inset:0;width:100%;height:100%;background:rgb(200,120,60)}}\
         .front{{position:relative;width:100px;height:50px;background:rgb(0,255,0)}}\
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
fn a_later_positioned_sibling_paints_over_an_earlier_absolute_one() {
    for (z, body) in [
        ("10", "<div class=wrap><div class=img></div><div class=front style='z-index:10'></div></div>"),
        ("auto", "<div class=wrap><div class=img></div><div class=front></div></div>"),
        (
            "10, image one layer down",
            "<div class=wrap><div style='position:absolute;inset:0'><div class=img></div></div>\
             <div class=front style='z-index:10'></div></div>",
        ),
    ] {
        assert_eq!(fill_order(body), ["image", "front"], "z-index {z}");
    }
}
