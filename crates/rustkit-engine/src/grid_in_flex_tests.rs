//! A grid container that is a flex item, against the pinned Chromium.
//!
//! `tools/parity_oracle/grid_in_flex_cases.json` holds small pages and, for
//! each, what the pinned Chromium reports for every element with an id:
//! `id:x:y:width:height` of its border box (`grid_flexible_row_log.mjs --file
//! grid_in_flex_cases.json --write`). The engine must give the same string.
//!
//! The first page is reddit's feed (H26b): `display: grid` with two
//! `minmax(0, <px>)` columns inside a flex row. The grid was as wide as its
//! longest word (46px) and its two items were stacked in one column, so the
//! sidebar sat below the feed.
//!
//! The six tests that load every page into a view run on macOS, where the
//! shaper is the one the pinned boxes assume. The pages marked `no_text`
//! (words are inline-blocks of a fixed width, `font-size: 0`) depend on no
//! shaper and are also checked on every OS, without a view or a script.

use super::*;

/// The expression `grid_flexible_row_log.mjs` evaluates in Chromium
/// (`BOXES_XYWH`).
#[cfg(all(target_os = "macos", feature = "headless"))]
const BOXES: &str = "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {\
    var r = e.getBoundingClientRect();\
    return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);\
    }).join(' ')";

/// Shapes that do not match Chrome yet, each with its reason. A shape listed
/// here that starts to match fails the test, so the list cannot go stale.
///
/// The three below are bing's search box (a textarea that is `height: 100%`
/// and shares `grid-area: content`, a name no area has, with an `::after`).
/// Their heights and rows match since the missing name resolves to the first
/// implicit line; `percent-height-item-sharing-an-area-in-a-block` and
/// `...-in-a-row-with-a-width` pin that. What is left is width:
///
/// `percent-height-item-sharing-an-area-in-a-row`: the grid is 38 wide, its
/// word, where the item's `min-width: 200px` makes it 200 in Chromium (the
/// content-width estimate does not read `min-width`).
///
/// `bing-textarea-and-its-replica-in-a-row`: the grid and the textarea are
/// 214 wide; Chromium has 200, the `min-width`.
///
/// `bing-textarea-and-its-replica-in-a-block`: the textarea's `width: 100%`
/// counts as the whole 600 toward its column, so the empty first column is 0
/// wide; in Chromium the columns are 200 and 400.
///
/// google's logo block (two `height: 100%` items in one cell of a grid whose
/// height is `calc(100% - 560px)`, in a column) is no gap any more:
/// `two-items-in-one-cell-of-a-calc-height-grid-in-a-column` matches since a
/// `height: 100%` column hands its items the height the block pass resolved.
///
/// The `percent-height-column-*` pages have no grid: they are the column
/// google's logo block sits in, a flex column whose own `height` is a
/// percentage (`html, body, #x { height: 100% }`). The block pass resolves
/// that height and hands it to the flex pass, so a `height: 25%` item, a
/// `flex: 1` item, `justify-content: center` and a border-box column with
/// padding are as in Chromium. One neighbour is not:
///
/// `percent-height-column-items-shrink`: a `height: 200px` item in an 80px
/// column stays 200; Chromium shrinks it to 60 beside its 20px sibling.
///
/// (`percent-height-row-is-not-touched`, a `height: 50%` item of a 400px
/// row, left this list with CH2.B1: a row hands its resolved percentage
/// height to its line as the definite cross size, so the item is 200 as in
/// Chromium, not 110.)
///
/// In the six below the grid is as wide as in Chromium and the columns in
/// it are not. They are text-free, so the every-OS test reads the list too.
///
/// `auto-tracks-min-width-*`: two `auto` columns of 150px words in items
/// with a `min-width` length, in a 100 or 200px grid. Chromium shares the
/// width, 50 or 100 each; here each column is 150. The grid pass has one
/// contribution per item, and an `auto` column needs two (the minimum, here
/// the `min-width`, and the max-content it may grow to). The same items in
/// `1fr` columns match.
///
/// `zero-fr-and-fr-tracks-*`: a `0fr` column is 0 wide; in Chromium it is
/// its item's min-content, 60.
///
/// The `auto-tracks-*-in-a-narrow-row` and `-inline-block` four are the
/// same `auto` columns in two more hosts.
///
/// The rest are the two track shapes the shrink-to-fit estimate leaves to
/// the generic walk (`grid_columns_content_width` returns `None`): a
/// `fit-content()` column, and a flexible column with a factor below 1.
/// The grid is then as wide as before the estimate existed, which is wrong
/// in its own way but was nearer Chromium than the estimate on these: two
/// `fit-content(80px)` columns of 150px words in `overflow: hidden` items
/// are 150 in a 100px float (Chromium 100; the estimate gave 300). Each of
/// these pages has `engine_boxes` in the case file, the walk's answer, and
/// the tests hold the engine to it until the estimate covers the shape.
///
/// The last is the header over two of three flexible columns in a 100px
/// inline-block: with a spanning item over a flexible column the whole
/// grid keeps the content floors (third column 60; Chromium 32). (The wide
/// header across `minmax(0, 1fr)` and an empty `auto` column left this list
/// with hub S17: a spanning item over a flexible track grows only the
/// flexible tracks with an intrinsic minimum, so main is 600 as in
/// Chromium.)
const GAPS: &[&str] = &[
    "percent-height-item-sharing-an-area-in-a-row",
    "bing-textarea-and-its-replica-in-a-row",
    "bing-textarea-and-its-replica-in-a-block",
    "percent-height-column-items-shrink",
    "auto-tracks-min-width-0-items-flex-1-beside-a-sibling",
    "auto-tracks-min-width-0-items-in-a-narrow-float",
    "auto-tracks-min-width-20px-items-flex-1-beside-a-sibling",
    "auto-tracks-min-width-20px-items-in-a-narrow-float",
    "zero-fr-and-fr-tracks-in-a-float",
    "zero-fr-and-fr-tracks-in-a-row",
    "auto-tracks-min-width-0-items-in-a-narrow-row",
    "auto-tracks-min-width-0-items-in-a-narrow-inline-block",
    "auto-tracks-min-width-20px-items-in-a-narrow-row",
    "auto-tracks-min-width-20px-items-in-a-narrow-inline-block",
    "half-and-quarter-fr-tracks-in-a-row",
    "tenth-fr-tracks-in-a-float",
    "fit-content-tracks-overflow-hidden-items-in-a-narrow-float",
    "fit-content-tracks-overflow-hidden-items-in-a-wide-float",
    "fit-content-tracks-min-width-20px-items-in-a-narrow-float",
    "fit-content-tracks-min-width-20px-items-in-a-wide-float",
    "fit-content-tracks-overflow-hidden-items-with-a-gap-in-a-narrow-row",
    "fit-content-tracks-two-word-items-in-a-narrow-float",
    "fit-content-tracks-two-word-items-in-a-wide-float",
    "zero-fr-and-2fr-tracks-min-width-20px-item-in-a-narrow-float",
    "zero-fr-and-2fr-tracks-min-width-20px-item-in-a-wide-float",
    "zero-fr-and-2fr-tracks-overflow-hidden-item-in-a-wide-float",
    "quarter-fr-and-2fr-tracks-min-width-20px-items-in-a-wide-float",
    "quarter-fr-and-2fr-tracks-in-a-wide-float",
    "half-fr-and-quarter-fr-tracks-in-a-wide-float",
    "header-over-two-of-three-flexible-columns-min-width-0-items-in-a-narrow-inline-block",
];

/// The pages are checked in six tests, a sixth each. As one test they
/// held the GPU for over 120 s on a loaded machine (31 pages at more than a
/// second each, when there were 31), and the tests queued behind it failed in the GPU test guard.
#[cfg(all(target_os = "macos", feature = "headless"))]
const PARTS: usize = 6;

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_that_is_a_flex_item_is_as_in_chrome_1_of_6() {
    pages_are_as_in_chrome(0);
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_that_is_a_flex_item_is_as_in_chrome_2_of_6() {
    pages_are_as_in_chrome(1);
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_that_is_a_flex_item_is_as_in_chrome_3_of_6() {
    pages_are_as_in_chrome(2);
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_that_is_a_flex_item_is_as_in_chrome_4_of_6() {
    pages_are_as_in_chrome(3);
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_that_is_a_flex_item_is_as_in_chrome_5_of_6() {
    pages_are_as_in_chrome(4);
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_grid_that_is_a_flex_item_is_as_in_chrome_6_of_6() {
    pages_are_as_in_chrome(5);
}

#[cfg(all(target_os = "macos", feature = "headless"))]
fn pages_are_as_in_chrome(part: usize) {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/grid_in_flex_cases.json"))
            .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    let cases = data["cases"].as_array().expect("cases");
    for case in cases.iter().skip(part).step_by(PARTS) {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");

        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine.load_html(id, case["html"].as_str().unwrap()).expect("load_html");
        let value = engine.execute_script(id, BOXES).expect("script");
        let got = value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value);

        match (got == chrome, GAPS.contains(&name)) {
            (false, false) => wrong.push(format!("{name}\n   engine {got}\n   chrome {chrome}")),
            (true, true) => gaps_that_pass.push(name),
            (false, true) => {
                // A gap whose answer is pinned: it may not drift either.
                if let Some(pinned) = case["engine_boxes"].as_str().filter(|pinned| *pinned != got) {
                    wrong.push(format!("{name} (a gap, pinned)\n   engine {got}\n   pinned {pinned}"));
                }
            }
            _ => {}
        }
    }
    assert!(wrong.is_empty(), "{} wrong:\n{}", wrong.len(), wrong.join("\n"));
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}

/// The `no_text` pages, laid out as `relayout` lays out a view and read as
/// `getBoundingClientRect` reads the layout (the nested layout matrix's
/// way), so this needs no view, GPU or shaper and runs on every OS.
#[test]
fn text_free_pages_are_as_in_chrome_on_every_os() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/grid_in_flex_cases.json"))
            .expect("case file");
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    // The style memo is keyed by the document's address: keep every
    // document alive so no later page can reuse an earlier one's address.
    let mut documents = Vec::new();
    let mut wrong = Vec::new();
    let mut gaps_that_pass = Vec::new();
    let mut checked = 0;
    for case in data["cases"].as_array().expect("cases") {
        if case["no_text"].as_bool() != Some(true) {
            continue;
        }
        checked += 1;
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        let ids: Vec<&str> = chrome.split(' ').map(|b| b.split(':').next().unwrap()).collect();
        let document = Document::parse_html(case["html"].as_str().unwrap()).expect("parse");
        let rects = super::nested_layout_matrix_tests::engine_rects(&engine, &document, &ids);
        documents.push(document);
        let got = ids
            .iter()
            .zip(&rects)
            .map(|(id, r)| match r {
                Some(r) => format!("{id}:{}:{}:{}:{}", r[0].round(), r[1].round(), r[2].round(), r[3].round()),
                None => format!("{id}:none"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        match (got == chrome, GAPS.contains(&name)) {
            (false, false) => wrong.push(format!("{name}\n   engine {got}\n   chrome {chrome}")),
            (true, true) => gaps_that_pass.push(name),
            (false, true) => {
                // A gap whose answer is pinned: it may not drift either.
                if let Some(pinned) = case["engine_boxes"].as_str().filter(|pinned| *pinned != got) {
                    wrong.push(format!("{name} (a gap, pinned)\n   engine {got}\n   pinned {pinned}"));
                }
            }
            _ => {}
        }
    }
    assert!(checked > 0, "no page is marked no_text");
    assert!(wrong.is_empty(), "{} of {checked} wrong:\n{}", wrong.len(), wrong.join("\n"));
    assert!(
        gaps_that_pass.is_empty(),
        "listed as a gap but matches Chrome now, take it off the list: {gaps_that_pass:?}"
    );
}
