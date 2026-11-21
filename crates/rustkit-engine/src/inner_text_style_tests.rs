//! Z plan CH2.E13: `innerText` returned the text of subtrees author CSS
//! hides. The getter read each element's display from the UA defaults for
//! its tag, never from computed style, so `display: none` from a class or a
//! `style` attribute, `visibility: hidden`, and a `display` that makes a
//! span a block or a div inline were all invisible to it. Chromium's getter
//! follows the rendered-text collection steps (HTML §3.2.7) on computed
//! style. Found by #910's check, whose text reader read hidden panels.
//!
//! `tools/parity_oracle/inner_text_style_cases.json` holds the pages and
//! Chromium 143.0.7499.4's `encodeURIComponent(t.innerText)` for each
//! (`grid_flexible_row_log.mjs --file inner_text_style_cases.json --write`).

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

#[test]
fn inner_text_follows_computed_display_and_visibility_as_in_chrome() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../../../tools/parity_oracle/inner_text_style_cases.json"))
            .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let expr = data["expr"].as_str().expect("expr");
    let mut wrong = Vec::new();
    for case in data["cases"].as_array().expect("cases") {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine.load_html(id, case["html"].as_str().unwrap()).expect("load_html");
        let got = engine.execute_script(id, expr).expect("script");
        let got = got
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&got)
            .to_string();
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

/// Z plan CH2.E13b (from #923's review): `white-space` and the table
/// cell/row breaks came from the tag name, not computed style, and
/// `display: contents` was unpinned. `inner_text_style2_cases.json`: a
/// `white-space: pre` div (and pre-wrap, pre-line, a `<pre>` set to normal),
/// `display: table-cell`/`table-row` on divs and spans, a `<td>`/`<tr>` set
/// to block, and `display: contents` (on a child, on a `<p>`, on the target).
#[test]
fn inner_text_takes_white_space_and_table_breaks_from_computed_style_as_in_chrome() {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tools/parity_oracle/inner_text_style2_cases.json"
    ))
    .expect("case file");
    let (w, h) = (data["viewport"][0].as_u64().unwrap(), data["viewport"][1].as_u64().unwrap());
    let expr = data["expr"].as_str().expect("expr");
    let cases = data["cases"].as_array().expect("cases");
    let mut wrong = Vec::new();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let chrome = case["chrome_boxes"].as_str().expect("run grid_flexible_row_log.mjs --write");
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine
            .create_headless_view(Bounds::new(0, 0, w as u32, h as u32))
            .expect("headless view");
        engine.load_html(id, case["html"].as_str().unwrap()).expect("load_html");
        let got = engine.execute_script(id, expr).expect("script");
        let got = got
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&got)
            .to_string();
        if got != chrome {
            wrong.push(format!("{name}:\n  rustkit {got}\n  chrome  {chrome}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
