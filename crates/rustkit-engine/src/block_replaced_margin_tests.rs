//! Block-level replaced boxes (`<img>` / `<svg>` with `display: block`) collapse their
//! margins as blocks do: with the margins of the siblings around them, and with a
//! parent's at its first / last child, but never through themselves (a replaced box is
//! never empty). Before this, block flow counted every replaced box as inline-level: the
//! margin pending before it was materialised (no collapse), and the box was placed by
//! its own top margin alone.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/block_replaced_margin_log.mjs`, receipt
//! `docs/diagnostics/2026-11-19/block_replaced_margin_cft148.json`, which this file
//! reads: every element with an id as `id=y/height`.

use super::*;

const RECEIPT: &str =
    include_str!("../../../docs/diagnostics/2026-11-19/block_replaced_margin_cft148.json");

fn rects(s: &str) -> Vec<(String, f32, f32)> {
    s.split_whitespace()
        .filter_map(|r| {
            let (id, rest) = r.split_once('=')?;
            let (y, h) = rest.split_once('/')?;
            Some((id.to_string(), y.parse().ok()?, h.parse().ok()?))
        })
        .collect()
}

fn check(label: &str) {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let case = receipt["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("no case {label:?}"));
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let page = format!(
        "<!doctype html><style>body{{margin:0}}</style>{}",
        case["body"].as_str().unwrap()
    );
    engine.load_html(view, &page).expect("load");
    let got = engine
        .execute_script(
            view,
            "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) { \
             var r = e.getBoundingClientRect(); return e.id + '=' + r.y + '/' + r.height; }).join(' ')",
        )
        .expect("script");
    let got = got
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .to_string();
    let want = case["want"].as_str().unwrap();
    let (g, w) = (rects(&got), rects(want));
    let same = g.len() == w.len()
        && g.iter()
            .zip(&w)
            .all(|(a, b)| a.0 == b.0 && (a.1 - b.1).abs() <= 0.5 && (a.2 - b.2).abs() <= 0.5);
    assert!(same, "{label}: {got}, Chrome {want}");
}

#[test]
fn a_block_img_collapses_with_the_margins_around_it() {
    check("img block between margined divs");
}

/// No height, and still not empty: its margins do not collapse through it.
#[test]
fn a_zero_height_block_img_is_not_collapsed_through() {
    check("img block h0 between margined divs");
}

#[test]
fn the_larger_margin_wins() {
    check("img block larger top margin");
}

#[test]
fn a_negative_margin_combines() {
    check("img block negative top margin");
}

#[test]
fn a_first_child_block_img_collapses_with_its_parent() {
    check("img block first child of a margined parent");
}

#[test]
fn a_last_child_block_img_collapses_with_its_parent() {
    check("img block last child, parent bottom margin");
}

#[test]
fn two_block_imgs_collapse_with_each_other() {
    check("two block imgs");
}

#[test]
fn a_block_svg_collapses_too() {
    check("svg block between margined divs");
}

/// An inline image opens a line box: nothing changes for it.
#[test]
fn an_inline_img_is_unchanged() {
    check("inline img (unchanged)");
}
