//! #984 third HOLD (CH2.E20): the SVG paint path reads the same sheet list
//! the cascade does: `<style>` and `<link>` sheets in document order, a
//! link's sheet only where its `media` matches. Until this it chained every
//! inline sheet before every external one and ignored link media, so a
//! later `<style>` could not override a link's `fill` and a `media=print`
//! link coloured SVG on screen.
//!
//! Chrome for Testing 148.0.7778.216 headless at 800x600
//! (`scratchpad/fix39/e20_svg_probe.mjs`): `getComputedStyle(rect).fill`.

use super::*;
use crate::script_net_tests::{serve_routes, Server};

const RED: (u8, u8, u8) = (255, 0, 0);
const GREEN: (u8, u8, u8) = (0, 128, 0);
const BLUE: (u8, u8, u8) = (0, 0, 255);
const BLACK: (u8, u8, u8) = (0, 0, 0);

fn link(extra: &str) -> String {
    format!("<link rel=stylesheet href=/red.css {extra}>")
}

/// The solid fills the page's one inline `<svg>` paints after a load with
/// the sheets in `head`.
fn svg_fills(head: &str) -> Vec<(u8, u8, u8)> {
    let page = format!(
        "<!doctype html><html><head><style>body{{margin:0}}</style>{head}</head><body>\
         <svg width=20 height=20><rect id=r width=20 height=20></rect></svg></body></html>"
    );
    let server: Server = serve_routes(vec![
        ("/", "text/html", page),
        ("/red.css", "text/css", "rect { fill: rgb(255,0,0) }".to_string()),
    ]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let document = engine.views[&view].document.clone().expect("document");
    engine.cache_svg_paint_styles(&document, view, Some((800.0, 600.0)));
    engine.lctx.cache_inline_svgs(&document);
    let mut out = Vec::new();
    for svg in document.get_elements_by_tag_name("svg") {
        let key = Engine::inline_svg_key(&engine.lctx.inline_svg_markup_styled(&svg));
        let svg_cache = engine.lctx.svg_cache.borrow();
        let Some(doc) = svg_cache.get(&key) else { continue };
        for c in doc.render(0.0, 0.0, 20.0, 20.0) {
            if let rustkit_layout::DisplayCommand::FillRect { color, .. } = c {
                out.push((color.r, color.g, color.b));
            }
        }
        out.dedup();
    }
    out
}

#[test]
fn svg_paint_reads_style_and_link_sheets_in_document_order() {
    let cases = [
        ("link red, style blue", format!("{}<style>rect{{fill:rgb(0,0,255)}}</style>", link("")), BLUE),
        ("style blue, link red", format!("<style>rect{{fill:rgb(0,0,255)}}</style>{}", link("")), RED),
        (
            "style blue, link red, style green",
            format!("<style>rect{{fill:rgb(0,0,255)}}</style>{}<style>rect{{fill:rgb(0,128,0)}}</style>", link("")),
            GREEN,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, head, want) in cases {
        let got = svg_fills(&head);
        if got != vec![want] {
            wrong.push(format!("{name}: got {got:?}, Chromium {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn svg_paint_honours_a_links_media() {
    let cases = [
        ("print link alone", link("media=print"), BLACK),
        ("style blue, print link red", format!("<style>rect{{fill:rgb(0,0,255)}}</style>{}", link("media=print")), BLUE),
        ("style blue, screen link red", format!("<style>rect{{fill:rgb(0,0,255)}}</style>{}", link("media=screen")), RED),
        (
            "style blue, (min-width: 900px) link red at 800",
            format!("<style>rect{{fill:rgb(0,0,255)}}</style>{}", link("media=\"(min-width: 900px)\"")),
            BLUE,
        ),
    ];
    let mut wrong = Vec::new();
    for (name, head, want) in cases {
        let got = svg_fills(&head);
        if got != vec![want] {
            wrong.push(format!("{name}: got {got:?}, Chromium {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
