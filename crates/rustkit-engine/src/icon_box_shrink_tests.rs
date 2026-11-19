//! A `width: 24px` icon box in a flex row narrower than it shrinks to its
//! icon and no further (CB6c (d), the pin #1068's R1 asked for): #1068 lets
//! a pixel-width flex item shrink below its width, down to its content's
//! min-content, and an icon's content is the icon. An `<img>` or `<svg>` of
//! 16px and a `::before` glyph each floor the box.
//!
//! Expected values are getBoundingClientRect widths in Chrome for Testing
//! 148.0.7778.216 on this markup (16px Helvetica, the `×` glyph 9.34375).

use super::*;

fn eval(page: &str, script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, script).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

const IMG: &str = "<img src=\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16'/%3E\" width=16 height=16>";

fn page() -> String {
    format!(
        "<!doctype html><meta charset=utf-8><style>body{{margin:0;font:16px Helvetica}} .r{{display:flex}} .i{{width:24px;height:24px}} .b::before{{content:\"\\00d7\";font-size:16px}}</style>\
         <div class=r style=\"width:0\"><div class=i id=img0>{IMG}</div></div>\
         <div class=r style=\"width:12px\"><div class=i id=img12>{IMG}</div></div>\
         <div class=r style=\"width:0\"><div class=i id=svg0><svg width=16 height=16></svg></div></div>\
         <div class=r style=\"width:12px\"><div class=i id=svg12><svg width=16 height=16></svg></div></div>\
         <div class=r style=\"width:0\"><div class=\"i b\" id=before0></div></div>\
         <div class=r style=\"width:4px\"><div class=\"i b\" id=before4></div></div>\
         <div class=r style=\"width:100px\"><div class=i id=img100>{IMG}</div></div>"
    )
}

#[test]
fn an_icon_box_shrinks_to_its_icon_and_no_further() {
    let got = eval(
        &page(),
        "['img0','img12','svg0','svg12','before0','before4','img100'].map(function (i) { \
         return Math.round(document.getElementById(i).getBoundingClientRect().width * 100) / 100; }).join(' ')",
    );
    assert_eq!(got, "16 16 16 16 9.34 9.34 24", "Chromium 148 widths: the icon floors the box, a wide row keeps 24");
}
