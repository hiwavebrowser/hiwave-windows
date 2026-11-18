//! SET2: the line pitch of a wrapped inline element with its own font size
//! or line-height inside a block with a different strut. Every line box is
//! the union of the block's strut and the inline's box on the baseline
//! (CSS2 §10.8), so a 12.8px `line-height: 1.5` span (19.2) in a 16px
//! system-ui block steps 20.19 per line in Chrome, not 19.2. RustKit gets
//! the first line right and spaces the inline's later lines at its own
//! line-height (the wrapped text box keeps one pitch, `line_height`, for
//! every line), so settings' Privacy Notice paragraph was 2px short and
//! everything below it 2.6px high.
//!
//! Expected heights: docs/diagnostics/2026-11-11/set2_inline_line_pitch_cft148.txt
//! for set2_inline_line_pitch.html beside it (60 two-line blocks; 20 differ).

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/set2_inline_line_pitch.html");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-11/set2_inline_line_pitch_cft148.txt");

#[test]
fn a_wrapped_inline_steps_by_the_strut_union() {
    let expected: Vec<(&str, f32, String)> = CHROME
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let mut it = l.split_whitespace();
            let id = it.next().unwrap();
            let h = it.next().unwrap().parse().unwrap();
            (id, h, it.collect::<Vec<_>>().join(" "))
        })
        .collect();
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let ids: Vec<String> = expected.iter().map(|(id, ..)| format!("'{id}'")).collect();
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
        .filter(|((_, want, _), got)| (*got - *want).abs() > 0.5)
        .map(|((id, want, what), got)| format!("{id} ({what}): {got} (Chromium {want})"))
        .collect();
    assert!(wrong.is_empty(), "{} of {} blocks differ:\n{}", wrong.len(), expected.len(), wrong.join("\n"));
}

/// The heights of the fixture's blocks, in document order.
fn block_heights(engine: &mut Engine, view: EngineViewId) -> Vec<f32> {
    let got = engine
        .execute_script(
            view,
            "Array.prototype.map.call(document.querySelectorAll('body > div'), function (d) { return d.getBoundingClientRect().height; }).join(' ')",
        )
        .expect("script");
    got.trim_start_matches("String(\"")
        .trim_end_matches("\")")
        .split(' ')
        .map(|v| v.parse().unwrap())
        .collect()
}

/// R1 on #1090 (MED): the pitch is written into the run's height, so a
/// second visit to a run that was not laid out again must not add it a
/// second time. Each block is laid out again around its untouched span (a
/// text node appended and removed), three times over, in a full layout and
/// through the incremental path: the heights stay what the first layout
/// gave.
#[test]
fn a_wrapped_run_keeps_its_height_when_its_block_is_laid_out_again() {
    use crate::incremental_layout::{set_mode_for_test, Mode};
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            set_mode_for_test(None);
        }
    }
    for mode in [Mode::Off, Mode::On, Mode::Verify] {
        set_mode_for_test(Some(mode));
        let _reset = Reset;
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        engine.load_html(view, PAGE).expect("load");
        let first = block_heights(&mut engine, view);
        assert_eq!(first.len(), 60, "the fixture has 60 blocks");
        for pass in 0..3 {
            // A new last line in every block: the block is laid out again
            // and its wrapped span is the same box as before.
            engine
                .execute_script(
                    view,
                    "Array.prototype.forEach.call(document.querySelectorAll('body > div'), function (d) { var b = document.createElement('div'); b.className = 'again'; b.style.cssText = 'height:3px;margin:0;width:auto'; d.appendChild(b); }); 0",
                )
                .expect("append");
            let grown = block_heights(&mut engine, view);
            engine
                .execute_script(
                    view,
                    "Array.prototype.forEach.call(document.querySelectorAll('.again'), function (b) { b.remove(); }); 0",
                )
                .expect("remove");
            let again = block_heights(&mut engine, view);
            let moved: Vec<String> = first
                .iter()
                .zip(&grown)
                .zip(&again)
                .enumerate()
                .filter(|(_, ((first, grown), again))| ((*grown - *first) - 3.0).abs() > 0.01 || (*again - *first).abs() > 0.01)
                .map(|(i, ((first, grown), again))| format!("d{i}: {first} first, {grown} with a 3px child, {again} after"))
                .collect();
            assert!(moved.is_empty(), "{mode:?}, pass {pass}: {} of 60 blocks changed height:\n{}", moved.len(), moved.join("\n"));
        }
    }
}
