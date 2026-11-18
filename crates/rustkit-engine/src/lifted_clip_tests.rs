//! A clipper keeps its clip over an absolute box under a positioned wrapper, vs Chrome for Testing
//! 148.0.7778.216 (`docs/diagnostics/2026-11-14/lifted_clip_cases.mjs`, output `lifted_clip_cft148.json`).
//! The second review of #1151 (HIGH 5): a positioned box lifted to its stacking context's list must keep
//! the clip of an overflow / clip-path / border-radius clipper that holds its containing block, and must
//! not take one that does not. For each case: is the probe point red (the box `r` shows there), and which
//! element `elementFromPoint` names there. The third review (HIGH 7) added an opacity layer between the
//! holder and the absolute box, scrolled clippers (a case's optional `[id, scrollTop]`), and transform /
//! filter boxes as the containing block of absolute and fixed boxes.

use super::*;
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-14/lifted_clip_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-14/lifted_clip_cft148.json");

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;

/// Whether the probe point is red, and what `elementFromPoint` names there (id, else tag).
fn red_and_hit(body: &str, x: u64, y: u64, scroll: Option<(&str, u64)>, tag: &str) -> (bool, String) {
    let page = format!("<!doctype html><html><head><style>body{{margin:0;background:#fff}}</style></head><body>{body}</body></html>");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds::new(0, 0, WIDTH, HEIGHT)).expect("view");
    engine.load_html(view, &page).expect("load_html");
    if let Some((id, top)) = scroll {
        engine
            .execute_script(view, &format!("document.getElementById({id:?}).scrollTop = {top}; 0"))
            .expect("scroll");
    }
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!("rustkit-lifted-clip-{tag}-{}.ppm", std::process::id()));
    engine.capture_frame(view, path.to_str().unwrap()).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    let at = ppm.len() - (WIDTH * HEIGHT * 3) as usize + (y as usize * WIDTH as usize + x as usize) * 3;
    let red = ppm[at] > 200 && ppm[at + 1] < 50 && ppm[at + 2] < 50;
    let out = engine
        .execute_script(
            view,
            &format!("String((function () {{ var e = document.elementFromPoint({x}, {y}); return e ? (e.id || e.tagName) : 'none'; }})())"),
        )
        .expect("elementFromPoint");
    let hit = out
        .strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or(&out)
        .trim_matches('"')
        .to_string();
    (red, hit)
}

#[test]
fn a_clipper_keeps_its_clip_over_a_lifted_absolute_box_as_in_chrome() {
    let cases: BTreeMap<String, Vec<serde_json::Value>> = serde_json::from_str(CASES).unwrap();
    let chrome: BTreeMap<String, serde_json::Value> = serde_json::from_str(CHROME).unwrap();
    let mut wrong = Vec::new();
    for (name, case) in &cases {
        let (body, x, y) = (case[0].as_str().unwrap(), case[1].as_u64().unwrap(), case[2].as_u64().unwrap());
        let scroll = case.get(3).map(|s| (s[0].as_str().unwrap(), s[1].as_u64().unwrap()));
        let (red, hit) = red_and_hit(body, x, y, scroll, name);
        let want_red = chrome[name]["red"].as_bool().unwrap();
        let want_hit = chrome[name]["hit"].as_str().unwrap();
        if red != want_red || hit != want_hit {
            wrong.push(format!("{name}: want red {want_red} hit {want_hit}, here red {red} hit {hit}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}
