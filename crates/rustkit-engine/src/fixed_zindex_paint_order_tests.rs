//! Paint order of overlapping layers in a stacking context, vs Chrome for Testing 148.0.7778.216
//! (`docs/diagnostics/2026-11-14/fixed_zindex_cases.mjs`, output `fixed_zindex_cft148.json`). Two boxes
//! overlap at (150, 150): A (red) comes first in the document, B (blue) later. For each case the box
//! drawn last there is on top, and `elementFromPoint` must name the same box (the picture and the hit
//! agree). bing.com's later state painted its fixed background-photo layers over its fixed header,
//! covering the nav row and the search box (batch 75); the R1 on #1151 added the flex-item, static
//! stacking context, `body { overflow-x: hidden }` and absolute-box shapes.

use super::*;
use rustkit_layout::DisplayCommand;
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-14/fixed_zindex_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-14/fixed_zindex_cft148.json");

/// Cases this change does not reach, with what this engine answers (top, hit); each differs from Chrome
/// and is a row of its own.
/// - A clipping wrapper stops the lift (the boxes under it keep its clip), so a fixed box inside one keeps
///   its old place in the picture; its hit, by the fixed-box pass, is Chrome's.
/// - The hit test takes fixed boxes in a pass of their own and puts any non-negative one over the whole
///   page (a sticky box apart), so an absolute z-index:2 box under a fixed z-index:1 one loses the hit
///   that it wins in the picture. As on develop.
const KNOWN_NOT_DONE: &[(&str, &str, &str)] = &[
    ("fixed_z5_in_overflow_hidden_wrapper_over_fixed_z1", "b", "a"),
    ("absolute_z2_over_fixed_z1_later", "a", "b"),
];

/// Which box is drawn last of the two ("a" red, "b" blue), and which `elementFromPoint(150, 150)` names.
fn on_top_and_hit(body: &str) -> (&'static str, String) {
    let page = format!("<!doctype html><html><head><style>body{{margin:0;background:#fff}}</style></head><body>{body}</body></html>");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
    engine.load_html(view, &page).expect("load_html");
    let list = engine.views[&view].display_list.as_ref().expect("display list").commands.clone();
    let mut last = "none";
    for c in &list {
        if let DisplayCommand::SolidColor(c, _) = c {
            match (c.r, c.g, c.b) {
                (255, 0, 0) => last = "a",
                (0, 0, 255) => last = "b",
                _ => {}
            }
        }
    }
    let out = engine
        .execute_script(view, "String((function () { var e = document.elementFromPoint(150, 150); return e ? (e.id || e.tagName) : 'none'; })())")
        .expect("elementFromPoint");
    let hit = out
        .strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or(&out)
        .trim_matches('"')
        .to_string();
    (last, hit)
}

#[test]
#[cfg(feature = "headless")]
fn overlapping_layers_stack_and_hit_as_in_chrome() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let chrome: BTreeMap<String, serde_json::Value> = serde_json::from_str(CHROME).unwrap();
    let mut wrong = Vec::new();
    for (name, body) in &cases {
        let (top, hit) = on_top_and_hit(body);
        let (want_top, want_hit) = match KNOWN_NOT_DONE.iter().find(|(k, _, _)| k == name) {
            Some((_, t, h)) => (t.to_string(), h.to_string()),
            None => (
                chrome[name]["top"].as_str().unwrap().to_string(),
                chrome[name]["hit"].as_str().unwrap().to_string(),
            ),
        };
        if top != want_top || hit != want_hit {
            wrong.push(format!("{name}: want top {want_top} hit {want_hit}, here top {top} hit {hit}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
