//! White space beside a block-level image or control.
//!
//! A collapsed space at the start or end of a line does not render
//! (css-text §4.1.2), so the white space between two block-level boxes makes
//! no line. The strip that removes it asked each neighbour whether it was
//! inline-level, and until 2026-10-09 every image and every form control
//! answered yes whatever its `display`. Wikipedia's logo is
//! `<span>\n<img style="display:block">\n<img style="display:block">\n</span>`:
//! the space between the two images kept an 18px line, the logo was 56px
//! tall for Chromium's 38 and the header grew with it (hand test, H18).

use super::*;
use crate::script_net_tests::serve_routes;

const GIF: &str = "data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7";

fn img(style: &str) -> String {
    format!("<img alt=\"\" src=\"{GIF}\" style=\"width:100px;height:20px;{style}\">")
}

fn height_of_b(body: &str) -> String {
    let page = format!(
        "<!DOCTYPE html><html><head><style>*{{box-sizing:border-box}}body{{margin:0;font:16px Arial}}</style></head>\
         <body>{body}</body></html>"
    );
    let server = serve_routes(vec![("/", "text/html", page)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 200 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let value = engine
        .execute_script(view, "String(document.getElementById('b').offsetHeight)")
        .unwrap();
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

/// Heights from Chromium 143 (the oracle) on these pages.
#[test]
fn white_space_between_block_level_images_makes_no_line() {
    let block = img("display:block");
    for (name, body, height) in [
        (
            "div > block img, block img",
            format!("<div id=b style=\"width:300px\">\n\t{block}\n\t{block}\n</div>"),
            "40",
        ),
        (
            "the wikipedia logo: flex a > span > block imgs",
            format!(
                "<a style=\"display:flex;align-items:center\">\n<span id=b style=\"width:140px\">\n\t{block}\n\t{}\n</span>\n</a>",
                img("display:block;margin-top:5px")
            ),
            "45",
        ),
        (
            "div > flex img, grid img",
            format!("<div id=b style=\"width:300px\">\n{}\n{}\n</div>", img("display:flex"), img("display:grid")),
            "40",
        ),
    ] {
        assert_eq!(height_of_b(&body), height, "{name}");
    }
}

/// Heights from Chromium 143 (the oracle) on these pages.
#[test]
fn white_space_between_block_level_controls_makes_no_line() {
    for (name, body) in [
        (
            "inputs",
            "<div id=b style=\"width:300px\">\n<input style=\"display:block;height:20px\">\n<input style=\"display:block;height:20px\">\n</div>",
        ),
        (
            "buttons",
            "<div id=b style=\"width:300px\">\n<button style=\"display:block;height:20px\">a</button>\n<button style=\"display:block;height:20px\">b</button>\n</div>",
        ),
        (
            "select and textarea",
            "<div id=b style=\"width:300px\">\n<select style=\"display:block;height:20px\"><option>a</option></select>\n<textarea style=\"display:block;height:20px\"></textarea>\n</div>",
        ),
    ] {
        assert_eq!(height_of_b(body), "40", "{name}");
    }
}

/// What must stay as it is: text beside a block-level image still makes its
/// lines, and an inline image after a block-level one sits on a line of its
/// own height. Heights from Chromium 143.
#[test]
fn lines_beside_a_block_level_image_are_kept() {
    let block = img("display:block");
    for (name, body, height) in [
        (
            "text, block img, text",
            format!("<div id=b style=\"width:300px;line-height:18px\">abc {block} def</div>"),
            "56",
        ),
        (
            "block img, div, block img",
            format!("<div id=b style=\"width:300px\">{block}\n<div style=\"height:20px\"></div>\n{block}</div>"),
            "60",
        ),
        (
            "block img, inline img",
            format!("<div id=b style=\"width:300px\">\n{block}\n{}\n</div>", img("vertical-align:top")),
            "40",
        ),
    ] {
        assert_eq!(height_of_b(&body), height, "{name}");
    }
}
