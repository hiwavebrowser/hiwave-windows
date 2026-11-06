//! #787: author CSS `stroke` reaches inline SVG children on every Linux
//! `--lib` run (no GPU compositor).
//!
//! The existing `test_author_css_paints_inline_svg_children` / cascade pins
//! only collect `Fill*` and gate on `test_compositor()`, so they silently
//! skip on Linux CI — and never asserted stroke even when they ran. Stroke-
//! only icons (Census path that motivated #787) could regress while fill
//! stayed green.

use super::*;

/// Author paint styles + inline SVG cache for `html`, then the solid stroke
/// colours each cached svg paints (collapsed runs).
fn author_svg_strokes(html: &str) -> Vec<(u8, u8, u8)> {
    let document = Rc::new(Document::parse_html(html).expect("parse"));
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    engine.cache_svg_paint_styles(&document, EngineViewId::new(), Some((800.0, 600.0)));
    engine.lctx.cache_inline_svgs(&document);
    let mut out = Vec::new();
    for svg in document.get_elements_by_tag_name_shadow_including("svg") {
        let key = Engine::inline_svg_key(&engine.lctx.inline_svg_markup_styled(&svg));
        let svg_cache = engine.lctx.svg_cache.borrow();
        let Some(doc) = svg_cache.get(&key) else {
            continue;
        };
        for c in doc.render(0.0, 0.0, 10.0, 10.0) {
            match c {
                rustkit_layout::DisplayCommand::StrokeRect { color, .. }
                | rustkit_layout::DisplayCommand::StrokePolygon { color, .. }
                | rustkit_layout::DisplayCommand::StrokeCircle { color, .. } => {
                    out.push((color.r, color.g, color.b));
                }
                _ => {}
            }
        }
        out.dedup();
    }
    out
}

#[test]
fn author_stroke_reaches_an_inline_svg_child() {
    // Census-shaped pin: `.i rect { stroke }` on an inline svg (the fill
    // sibling of this used `fill`; stroke was never asserted).
    let strokes = author_svg_strokes(
        r##"<!DOCTYPE html><style>.i { stroke: green; fill: none } .i rect { stroke: #e00; fill: none }</style>
            <svg class="i" width="10" height="10"><rect width="10" height="10"/></svg>"##,
    );
    assert_eq!(
        strokes,
        vec![(238, 0, 0)],
        "author stroke must paint the rect; got {strokes:?}"
    );
}

#[test]
fn author_stroke_cascade_matches_fill_rules() {
    // Mirror of the fill pin in `test_author_css_paints_inline_svg_children`:
    // own !important beats author !important; author normal beats a
    // presentation attribute; own style beats author normal.
    let strokes = author_svg_strokes(
        r##"<!DOCTYPE html><style>
            .imp { stroke: green !important; fill: none }
            .n { stroke: #e00; fill: none }
            </style>
            <svg width="30" height="10">
              <rect class="imp" width="10" height="10" style="stroke:blue !important;fill:none"/>
              <rect class="n" x="10" width="10" height="10" stroke="blue" fill="none"/>
              <rect class="n" x="20" width="10" height="10" style="stroke:blue;fill:none"/>
            </svg>"##,
    );
    assert_eq!(
        strokes,
        vec![(0, 0, 255), (238, 0, 0), (0, 0, 255)],
        "own !important > author !important; author > presentation; own style > author: {strokes:?}"
    );
}
