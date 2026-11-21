//! `getComputedStyle(...).fontFamily` as Chrome serializes it, per family in the
//! list: a generic keyword written bare stays bare (`monospace`); a quoted one
//! stays quoted (`"monospace"`: a family name, not the generic); any other name is
//! bare when it is a CSS identifier (`Arial`, `My-Font2`) and quoted otherwise
//! (`"Times New Roman"`, `"2fast"`); an unquoted run of identifiers is one name with
//! its whitespace collapsed. Receipt: Chrome for Testing 148.0.7778.216 on Linux,
//! `docs/diagnostics/2026-11-21/font_family_serialization_cft148.json`.

use super::*;

const RECEIPT: &str = include_str!("../../../docs/diagnostics/2026-11-21/font_family_serialization_cft148.json");

fn computed(declared: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 }).expect("view");
    engine.load_html(view, "<!doctype html><p id=p>x</p>").expect("load_html");
    let script = format!(
        "(function () {{ var p = document.getElementById('p'); p.style.fontFamily = {}; return String(getComputedStyle(p).fontFamily); }})()",
        serde_json::to_string(declared).unwrap()
    );
    let out = engine.execute_script(view, &script).expect("script");
    let inner = out
        .strip_prefix("String(")
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or_else(|| panic!("{declared}: {out}"));
    serde_json::from_str(inner).unwrap_or_else(|_| panic!("{declared}: {out}"))
}

#[test]
fn font_family_serializes_as_chrome_serializes_it() {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let wrong: Vec<String> = receipt["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|c| {
            let (declared, chrome) = (c["declared"].as_str().unwrap(), c["computed"].as_str().unwrap());
            let got = computed(declared);
            (got != chrome).then(|| format!("{declared}: {got}, Chrome {chrome}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), receipt["cases"].as_array().unwrap().len(), wrong.join("\n"));
}
