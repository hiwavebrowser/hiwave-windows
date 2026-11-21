//! A non-replaced inline's rect is its content area on Blink's whole-pixel
//! metrics (round(ascent) + round(descent) tall, its top the baseline minus
//! round(ascent)), and its text keeps the line's baseline whether the
//! half-leading is positive or negative. report.html's `.stat-value`
//! (32px/1) holds a 12px `span.time-format`: a 15px rect at baseline - 12
//! in Chrome; RustKit reported 14.13 at baseline - 11.6 and, because a
//! negative half-leading was never applied to the rect while the line
//! read the slot as rect - half-leading, seated the whole line 1px high.
//!
//! Expected values are getBoundingClientRect in Chrome for Testing
//! 148.0.7778.216 on this markup (impl-board3 probe `tf.html`, 2026-11-08).

use super::*;

fn eval(page: &str, script: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, script).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

const PAGE: &str = "<!doctype html><body style=\"margin:0;font-family:-apple-system,BlinkMacSystemFont,sans-serif\">\
    <div style=\"height:100px\"></div>\
    <div id=v style=\"font-size:32px;font-weight:700;line-height:1\">0<span id=s style=\"font-size:12px\">m</span></div>\
    <div id=v2 style=\"font-size:16px;line-height:1.5\">text <span id=s2>span</span> <a id=a2 href=x>link</a></div>\
    <div id=v3 style=\"font-size:20px;line-height:0.9\">tight <span id=s3 style=\"font-size:14px\">small</span></div>";

#[test]
fn inline_rects_sit_on_rounded_metrics_around_the_line_baseline() {
    let got = eval(
        PAGE,
        "['v','s','v2','s2','a2','v3','s3'].map(function (i) { var r = document.getElementById(i).getBoundingClientRect(); \
         return Math.round(r.y * 100) / 100 + '/' + Math.round(r.height * 100) / 100; }).join(' ')",
    );
    assert_eq!(
        got,
        "100/32 116/15 132/24 135/18 135/18 156/18 158/17",
        "Chromium 148 y/height: block lines advance by line-height, inline rects are the rounded content area"
    );
}
