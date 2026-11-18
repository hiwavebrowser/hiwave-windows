//! SET1: a dropdown `<select>`'s height. Chrome for Testing 148 builds it as
//! a content line of round(ascent) + round(descent) of the select's font
//! (no line gap) plus 2px the menulist keeps inside, then the vertical
//! padding (the author's, else none) and border (the author's, else the
//! UA's 1px each side). `line-height` does not apply. RustKit used the
//! font's `normal` line, dropped the UA border once the author set a
//! padding, and fell back to a scaled blob for a bare select at another
//! font size: settings' `#theme` (13.6px, padding .5rem .75rem) was 33 for
//! Chrome's 35, and every select in the matrix below differed.
//!
//! Expected heights: docs/diagnostics/2026-11-11/set1_select_heights_cft148.txt
//! for set1_select_heights.html beside it (getBoundingClientRect).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/set1_select_heights.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/set1_select_heights_cft148.txt");

#[test]
fn a_select_is_as_tall_as_chrome_builds_it() {
    let expected: Vec<(&str, f32)> = CHROME
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            (it.next().unwrap(), it.next().unwrap().parse().unwrap())
        })
        .collect();
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
                "[{}].map(function (i) {{ return document.getElementById(i).getBoundingClientRect().height; }}).join(' ')",
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
    let wrong: Vec<String> = expected
        .iter()
        .zip(&got)
        .filter(|((_, want), got)| (*got - *want).abs() > 0.5)
        .map(|((id, want), got)| format!("{id}: {got} (Chromium {want})"))
        .collect();
    assert!(wrong.is_empty(), "{} of {} selects differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}
