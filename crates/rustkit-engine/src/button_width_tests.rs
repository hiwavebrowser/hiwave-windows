//! CB3c: a `<button>`'s width is its label plus the horizontal padding plus the horizontal border,
//! each side the author's when declared and the UA's otherwise (6 px padding, 2 px border per side),
//! as the height is (CB3b). The expected numbers are Chrome for Testing 148.0.7778.216, measured
//! 2026-11-11 with `docs/diagnostics/2026-11-11/cb3c_button_widths_chrome` (puppeteer-core) on the
//! page `cb3c_button_widths.html` that this test loads; they agree with Reduce's Chrome 143 numbers
//! on the cells both measured (`#lh1` 24, `#p0` 12, `#b0` 20, `#all` 8 for the label x).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb3c_button_widths.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb3c_button_widths_cft148.json");

fn measured() -> std::collections::BTreeMap<String, (f64, f64)> {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    let out = engine
        .execute_script(view, "String(document.getElementById('out').textContent)")
        .unwrap();
    let text = out.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&out).to_string();
    text.split(";;")
        .filter_map(|part| {
            let (id, size) = part.split_once('=')?;
            let (w, h) = size.split_once('x')?;
            Some((id.to_string(), (w.parse().ok()?, h.parse().ok()?)))
        })
        .collect()
}

fn chrome() -> std::collections::BTreeMap<String, (f64, f64)> {
    let value: serde_json::Value = serde_json::from_str(CHROME).expect("json");
    value
        .as_object()
        .expect("object")
        .iter()
        .map(|(id, wh)| (id.clone(), (wh[0].as_f64().unwrap(), wh[1].as_f64().unwrap())))
        .collect()
}

#[test]
fn every_button_in_the_matrix_is_the_size_chrome_148_measured() {
    let (got, want) = (measured(), chrome());
    assert_eq!(got.len(), want.len(), "the page measured every cell: {} of {}", got.len(), want.len());
    let mut wrong = Vec::new();
    for (id, (cw, ch)) in &want {
        let (w, h) = got[id];
        let width_ok = (w - cw).abs() < 0.75;
        let height_ok = (h - ch).abs() < 0.75;
        if !(width_ok && height_ok) {
            wrong.push(format!("{id}: got {w:.2}x{h:.2}, Chrome 148 {cw:.2}x{ch:.2}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} cells differ from Chrome 148:\n{}", wrong.len(), want.len(), wrong.join("\n"));
}

/// Reduce's reduced settings page (the blocklist remove button), on Chrome for Testing 148: every
/// element's border box, including `#item`, a `width: max-content` flex row holding a label and
/// the remove button (113.47 x 24, where the engine gave the whole 784 px container).
#[test]
fn the_settings_remove_button_page_is_the_size_chrome_148_measured() {
    const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb3c_settings_remove_button.html");
    const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb3c_settings_remove_button_cft148.json");
    let want: serde_json::Value = serde_json::from_str(CHROME).expect("json");
    let want = want.as_object().expect("object");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    let ids: Vec<&String> = want.keys().collect();
    let expr = ids
        .iter()
        .map(|id| format!("(function(r){{return r.width+'x'+r.height;}})(document.getElementById('{id}').getBoundingClientRect())"))
        .collect::<Vec<_>>()
        .join(" + ';;' + ");
    let out = engine.execute_script(view, &format!("String({expr})")).unwrap();
    let text = out.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&out).to_string();
    let mut wrong = Vec::new();
    for (id, part) in ids.iter().zip(text.split(";;")) {
        let (w, h) = part.split_once('x').expect("wxh");
        let (w, h): (f64, f64) = (w.parse().unwrap_or(f64::NAN), h.parse().unwrap_or(f64::NAN));
        let (cw, ch) = (want[*id][0].as_f64().unwrap(), want[*id][1].as_f64().unwrap());
        if (w - cw).abs() > 0.75 || (h - ch).abs() > 0.75 {
            wrong.push(format!("#{id}: got {w:.2}x{h:.2}, Chrome 148 {cw:.2}x{ch:.2}"));
        }
    }
    assert!(wrong.is_empty(), "{} of {} elements differ from Chrome 148:\n{}", wrong.len(), ids.len(), wrong.join("\n"));
}

/// CB3d: a button with a fixed width wraps its label onto more lines when the label does not fit,
/// and the height follows (lines x line height + padding + border); `white-space: nowrap` keeps one
/// line; `min-content` is the longest word. Chrome for Testing 148.0.7778.216, page
/// `cb3d_button_label_wrap.html`.
#[test]
fn a_fixed_width_button_wraps_its_label_as_chrome_148_does() {
    const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb3d_button_label_wrap.html");
    const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb3d_button_label_wrap_cft148.json");
    let want: serde_json::Value = serde_json::from_str(CHROME).expect("json");
    let want = want.as_object().expect("object");
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load_html");
    let out = engine
        .execute_script(view, "String(document.getElementById('out').textContent)")
        .unwrap();
    let text = out.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&out).to_string();
    let mut wrong = Vec::new();
    let mut seen = 0;
    for part in text.split(";;") {
        let Some((id, size)) = part.split_once('=') else { continue };
        let Some((w, h)) = size.split_once('x') else { continue };
        let (w, h): (f64, f64) = (w.parse().unwrap_or(f64::NAN), h.parse().unwrap_or(f64::NAN));
        let (cw, ch) = (want[id][0].as_f64().unwrap(), want[id][1].as_f64().unwrap());
        seen += 1;
        if (w - cw).abs() > 0.75 || (h - ch).abs() > 0.75 {
            wrong.push(format!("{id}: got {w:.2}x{h:.2}, Chrome 148 {cw:.2}x{ch:.2}"));
        }
    }
    assert_eq!(seen, want.len(), "the page measured every cell");
    assert!(wrong.is_empty(), "{} of {} cells differ from Chrome 148:
{}", wrong.len(), want.len(), wrong.join("
"));
}
