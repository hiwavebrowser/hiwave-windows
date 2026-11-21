//! SET4: a text input's width is its content width plus its horizontal
//! padding (the author's, else the UA's 2px each side) and border (the
//! author's, else the UA's 2px each side). RustKit's bare 149 was Chrome's
//! `padding: 0` width: the UA padding was missing, so a bare input was 149
//! for Chrome's 153.
//!
//! This pins the COMPOSITION, not the content width (which depends on the
//! font's average and maximum character widths; a row of its own, SET5):
//! every case is compared to the `padding: 0; border: none` input of the
//! same font size, and the absolute widths are pinned only at the UA font
//! size (13.333px), where the content model is calibrated (145).
//!
//! Expected widths: docs/diagnostics/2026-11-11/set4_input_widths_cft148.txt
//! for set4_input_widths.html beside it (Chrome for Testing 148.0.7778.216).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/set4_input_widths.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/set4_input_widths_cft148.txt");

#[test]
fn a_text_input_adds_its_padding_and_border_to_its_content_width() {
    let expected: Vec<(&str, f32)> = CHROME
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            (it.next().unwrap(), it.next().unwrap().parse().unwrap())
        })
        .collect();
    assert_eq!(expected.len(), 42);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let ids: Vec<String> = expected.iter().map(|(id, _)| format!("'{id}'")).collect();
    let got = engine
        .execute_script(
            view,
            &format!(
                "[{}].map(function (i) {{ return document.getElementById(i).getBoundingClientRect().width; }}).join(' ')",
                ids.join(",")
            ),
        )
        .expect("script");
    let got: Vec<f32> = got
        .trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect();
    assert_eq!(got.len(), expected.len());
    let mut wrong = Vec::new();
    // Six cases per font size; the fifth is `padding: 0; border: none`.
    for (chrome, engine) in expected.chunks(6).zip(got.chunks(6)) {
        let (chrome_base, engine_base) = (chrome[4].1, engine[4]);
        for ((id, want), got) in chrome.iter().zip(engine) {
            let (want_extra, got_extra) = (want - chrome_base, got - engine_base);
            if (want_extra - got_extra).abs() > 0.5 {
                wrong.push(format!("{id}: +{got_extra} over its content width (Chromium +{want_extra})"));
            }
            // 13.6px is not pinned absolutely: the content width there is
            // 145 in Chrome and 147.9 here, the scaled calibration (SET5).
            let ua_size = id.starts_with("t13_333_");
            if ua_size && (got - want).abs() > 0.5 {
                wrong.push(format!("{id}: {got} wide (Chromium {want})"));
            }
        }
    }
    assert!(wrong.is_empty(), "{} differ:\n{}", wrong.len(), wrong.join("\n"));
}
