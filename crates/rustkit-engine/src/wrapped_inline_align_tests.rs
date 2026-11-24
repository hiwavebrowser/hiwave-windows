//! YT-CARD-SUBTITLE: an inline element whose text wraps aligns each of its lines on its own.
//!
//! In a `text-align: center` or `right` block, a `<span>` (or a custom element, or `<b><span>`)
//! whose text wraps onto several lines was moved as ONE box: every line kept the span's left edge
//! and the whole span shifted by a single offset, so line 0 started mid-line and ran past the
//! block's right edge. youtube.com's "Try searching to get started" card shows it on its subtitle.
//! css-text-3 §7.1 aligns each line box's content; bare text already did.
//!
//! Expected lines: docs/diagnostics/2026-11-21/wrapped_inline_align_cft148.txt, `window.report()`
//! of the page beside it in Chrome for Testing 148.0.7778.216 at 800x600 (Menlo 16px/20px), from
//! tools/parity_oracle/wrapped_inline_align_log.mjs. Each line is a case: each text node's line
//! rects (x, y, width). The `shared-*` cases (R1 on #1407) put content after the inline on its last
//! line (` B`, a link, another span): that line aligns as one, the inline's tail with what follows.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-21/wrapped_inline_align.html");
const CHROME: &str =
    include_str!("../../../docs/diagnostics/2026-11-21/wrapped_inline_align_cft148.txt");

/// Chrome lays lines out on a 1/64 px grid and rounds the widths it centers with; the layout
/// here keeps unrounded widths, so a right- or center-aligned line whose slack is not a whole
/// 1/64 px lands up to ~0.02 px away (bare text too: `bare-right`, `bare-three`). Each number
/// may differ from Chrome's by this much; the case, the line count and the text nodes must match.
const TOLERANCE_PX: f64 = 0.05;

/// The numbers of one report line (`id: x,y,w; x,y,w | x,y,w`), with its id and shape.
fn parse(line: &str) -> (String, String, Vec<f64>) {
    let (id, rest) = line.split_once(": ").unwrap_or((line, ""));
    let shape: String = rest.chars().filter(|c| matches!(c, ';' | '|' | ',')).collect();
    let nums = rest
        .split(|c| matches!(c, ';' | '|' | ','))
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .collect();
    (id.to_string(), shape, nums)
}

fn close(have: &str, want: &str) -> bool {
    let (hid, hshape, h) = parse(have);
    let (wid, wshape, w) = parse(want);
    hid == wid && hshape == wshape && h.len() == w.len() && h.iter().zip(&w).all(|(a, b)| (a - b).abs() <= TOLERANCE_PX)
}

#[test]
fn a_wrapped_inline_aligns_each_line_like_bare_text() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let got = engine
        .execute_script(view, "JSON.stringify(window.report().split('\\n'))")
        .expect("report");
    let got = got.strip_prefix("String(").and_then(|v| v.strip_suffix(')')).unwrap_or(&got).to_string();
    let got: String = serde_json::from_str(&got).expect("a JSON string");
    let got: Vec<String> = serde_json::from_str(&got).expect("a JSON array");
    let expected: Vec<&str> = CHROME.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
    assert_eq!(expected.len(), 15, "the Chrome report has 15 cases");
    let wrong: Vec<String> = expected
        .iter()
        .enumerate()
        .filter_map(|(i, want)| match got.get(i) {
            Some(have) if close(have, want) => None,
            Some(have) => Some(format!("  {have}\n    Chromium: {want}")),
            None => Some(format!("  (missing)\n    Chromium: {want}")),
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} cases differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
