//! `getImageData`, `putImageData`, `createImageData` and `ImageData`, case
//! by case against Chrome (IMG-F2).
//!
//! Chrome for Testing 148.0.7778.216 on the probe page
//! (`docs/diagnostics/2026-11-16/canvas_pixels.html`, record
//! `canvas_pixels_cft148.json`).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-16/canvas_pixels.html");
const PIN: &str = include_str!("../../../docs/diagnostics/2026-11-16/canvas_pixels_cft148.json");

/// Cases where the engine is known to differ, with what it answers. Each
/// is asserted, so a fix turns the test red until its line goes.
const KNOWN: &[(&str, &str)] = &[
    // Only sRGB pixels are kept: a colour space asked for is checked (an
    // unknown one is a TypeError, as in Chrome) and the answer is `srgb`.
    ("colorSpace asked for", "srgb srgb"),
    // `ImageData.pixelFormat` (Chrome 148) is not there yet.
    ("ImageData members", "[object ImageData] colorSpace,constructor,data,height,width"),
];

#[test]
fn canvas_pixels_behave_as_in_chrome() {
    let pin: serde_json::Value = serde_json::from_str(PIN).unwrap();
    assert_eq!(pin["browser"], "148.0.7778.216");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine.create_headless_view(Bounds::new(0, 0, 400, 300)).expect("view");
    engine.load_html(view, PAGE).expect("load");
    let out = engine.execute_script(view, "window.__out || ''").unwrap();
    let text: String = serde_json::from_str(out.trim_start_matches("String(").trim_end_matches(')')).unwrap_or_else(|e| panic!("{e}: {out}"));
    let ours: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cases = pin["cases"].as_object().unwrap();
    assert!(cases.len() >= 109);
    let mut failures = Vec::new();
    for (name, chrome) in cases {
        let expected = KNOWN.iter().find(|(known, _)| known == name).map(|(_, answer)| serde_json::Value::from(*answer));
        let expected = expected.as_ref().unwrap_or(chrome);
        if &ours[name] != expected {
            failures.push(format!("{name}: ours {} expected {expected} (Chrome {chrome})", ours[name]));
        }
    }
    assert!(failures.is_empty(), "{} of {} cases:\n{}", failures.len(), cases.len(), failures.join("\n"));
}
