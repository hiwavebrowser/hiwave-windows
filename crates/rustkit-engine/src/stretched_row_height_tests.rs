//! CB6c (c) on the measured page: a row flex container whose height is auto
//! is as tall as its tallest laid-out item, and its stretched items stretch
//! to that, not to the one-line guess of their own font (18 at 16px).
//!
//! Expected heights are getBoundingClientRect in Chrome for Testing
//! 148.0.7778.216 on docs/diagnostics/2026-11-11/cb6c_stretched_rows.html
//! (`font: 16px Helvetica`): one item holding a 5px block; items holding
//! 5px and 9px blocks; an item whose content is 0 tall, stretched and at
//! flex-start; an item holding a 12px line; and a 30px-tall row, whose
//! stretched item keeps the row's height.

use super::*;

const PAGE: &str = include_str!("../../../docs/diagnostics/2026-11-11/cb6c_stretched_rows.html");

#[test]
fn an_auto_height_row_is_its_tallest_laid_out_item() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    let got = engine
        .execute_script(
            view,
            "'r1 i1 r2 i2 j2 r3 i3 r4 i4 r5 i5 r6 i6'.split(' ').map(function (i) { \
             return i + ':' + Math.round(document.getElementById(i).getBoundingClientRect().height * 10) / 10; }).join(' ')",
        )
        .expect("script");
    let got = got.trim_start_matches("String(\"").trim_end_matches("\")");
    assert_eq!(
        got, "r1:5 i1:5 r2:9 i2:9 j2:9 r3:0 i3:0 r4:0 i4:0 r5:14 i5:14 r6:30 i6:30",
        "Chromium 148 heights"
    );
}
