//! A `::before`/`::after` with `display: none` generates no box (CSS
//! Display 3 §2.5; CSS 2.1 §12.1). `pseudo_element_box` built one whenever
//! `content` was set, so reddit's interstitial `main:before { ...;
//! position: absolute; display: none }` painted its #d93900 disc over the
//! logo (RD2, found reducing RD1, 10-29). Pinned Chrome 148 paints nothing
//! for every pseudo below except the two controls.

use super::*;

const PAGE: &str = r#"<!doctype html>
<html><head><style>
body { margin: 0 }
main { position: relative; width: 300px; height: 300px }
.abs::before { content: ''; position: absolute; width: 100px; height: 100px; background: #d93900; display: none }
.flow::before { content: 'x'; display: none }
.after::after { content: 'y'; display: none; float: left }
.fixed::after { content: ''; position: fixed; display: none }
.late::before { content: 'z'; display: block }
.late.off::before { display: none }
.shown::before { content: 'kept'; position: absolute }
.inline::after { content: 'also' }
</style></head><body>
<main class="abs"></main><main class="flow"></main><main class="after"></main><main class="fixed"></main>
<main class="late off"></main><main class="shown"></main><main class="inline"></main>
</body></html>"#;

/// The boxes a pseudo-element generated: they keep `content` in their style
/// (`pseudo_element_box`); elements and text runs never carry it.
fn pseudo_boxes(b: &LayoutBox, out: &mut Vec<String>) {
    if let Some(content) = &b.style.content {
        out.push(format!("{content:?}"));
    }
    for c in &b.children {
        pseudo_boxes(c, out);
    }
}

#[test]
fn a_pseudo_element_with_display_none_generates_no_box() {
    let document = Rc::new(Document::parse_html(PAGE).expect("html"));
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let layout = engine.build_layout_from_document(&document, &[]);
    let mut out = Vec::new();
    pseudo_boxes(&layout, &mut out);
    assert_eq!(out.len(), 2, "only the two controls get a box: {out:?}");
}
