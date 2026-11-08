//! CH2.B6: `text-transform` changes the text that is measured, not only the
//! text that is painted.
//!
//! The chrome's section titles are `text-transform: uppercase` (with
//! letter-spacing) inside flex headers. Paint drew "WORKSPACES" but layout
//! measured "workspaces", so the box was ~15 px narrow at 12px. A following
//! sibling started inside the painted text, a flex row's next item moved in,
//! and shrink-to-fit parents were too small. Chromium 143.0.7499.4 at 12px
//! system-ui: `workspaces` uppercase 81.72 px wide and the next span at
//! x = 81.72; capitalize "rust book" 57.39 px. Each control span holds the
//! already-transformed text, so the assertions hold on any face.

use super::*;

fn rects(html: &str, ids: &[&str]) -> Vec<(f64, f64)> {
    let mut engine =
        Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let view = engine.create_page_view(Bounds::new(0, 0, 600, 400)).expect("page view");
    engine.load_html(view, html).expect("load");
    ids.iter()
        .map(|id| {
            let mut read = |prop: &str| -> f64 {
                let v = engine
                    .execute_script(view, &format!("document.getElementById('{id}').getBoundingClientRect().{prop}"))
                    .unwrap();
                v.trim_start_matches("Number(")
                    .trim_end_matches(')')
                    .parse()
                    .unwrap_or_else(|_| panic!("{id}.{prop}: {v}"))
            };
            (read("x"), read("width"))
        })
        .collect()
}

const FONT: &str = "margin:0;font:12px -apple-system, BlinkMacSystemFont, sans-serif";

#[test]
fn an_uppercase_run_is_measured_as_uppercase() {
    let r = rects(
        &format!(
            "<body style=\"{FONT}\">\
             <div><span id=t style=\"text-transform:uppercase\">workspaces</span><span id=n>X</span></div>\
             <div><span id=c>WORKSPACES</span></div></body>"
        ),
        &["t", "n", "c"],
    );
    let (t, n, c) = (r[0], r[1], r[2]);
    assert!((t.1 - c.1).abs() < 0.05, "uppercase box {t:?} vs control {c:?}");
    // 0.5 px: the board's geometry threshold. Inline advance and the
    // span's box differ by ~0.06 px on any run (0.18 on develop before
    // this fix), independent of text-transform.
    assert!((n.0 - (t.0 + t.1)).abs() < 0.5, "next sibling starts after the painted text: {n:?} {t:?}");
}

#[test]
fn a_flex_row_places_the_next_item_after_an_uppercase_title() {
    let r = rects(
        &format!(
            "<body style=\"{FONT}\">\
             <div style=\"display:flex;gap:6px\"><span id=t style=\"text-transform:uppercase;letter-spacing:0.04em;font-weight:600\">workspaces</span><span id=n>2</span></div>\
             <div style=\"display:flex;gap:6px\"><span id=c style=\"letter-spacing:0.04em;font-weight:600\">WORKSPACES</span><span id=m>2</span></div></body>"
        ),
        &["t", "n", "c", "m"],
    );
    assert!((r[0].1 - r[2].1).abs() < 0.05, "title {:?} vs control {:?}", r[0], r[2]);
    assert!((r[1].0 - r[3].0).abs() < 0.05, "badge {:?} vs control badge {:?}", r[1], r[3]);
}

#[test]
fn a_capitalized_run_is_measured_capitalized() {
    let r = rects(
        &format!(
            "<body style=\"{FONT}\">\
             <div style=\"display:inline-block\" id=b><span id=t style=\"text-transform:capitalize\">rust book</span></div>\
             <div style=\"display:inline-block\" id=d><span id=c>Rust Book</span></div></body>"
        ),
        &["t", "c", "b", "d"],
    );
    assert!((r[0].1 - r[1].1).abs() < 0.05, "capitalize {:?} vs control {:?}", r[0], r[1]);
    assert!((r[2].1 - r[3].1).abs() < 0.05, "shrink-to-fit parent {:?} vs {:?}", r[2], r[3]);
}
