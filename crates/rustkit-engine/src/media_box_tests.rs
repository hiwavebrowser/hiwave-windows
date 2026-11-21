//! MEDIA-AVF Milestone 1 step 1 (docs/design/MEDIA_AVF.md §3.8; pins P-size, P-ua):
//! `<video>` and `<audio>` boxes with no media loaded. Before this a `<video>` had no
//! replaced box (no 300x150 default, its fallback children laid out as content) and
//! `<audio>` without `controls` was displayed.
//!
//! Oracle: Chrome for Testing 148.0.7778.216 through
//! `tools/parity_oracle/media_box_log.mjs`, receipt
//! `docs/diagnostics/2026-11-18/media_box_cft148.json`, which this file reads: each
//! case's border box (getBoundingClientRect) and its computed `display` and
//! `object-fit`. A video with no metadata has natural sizes 300x150 and no natural
//! ratio: `width=100` leaves the height at 150.

use super::*;
use crate::script_net_tests::serve_routes;

const RECEIPT: &str = include_str!("../../../docs/diagnostics/2026-11-18/media_box_cft148.json");

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

/// The case's rect and styles as RustKit reads them, in the receipt's shape.
fn measure(html: &str) -> serde_json::Value {
    let server = serve_routes(vec![("/", "text/html", html.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    let value = engine
        .execute_script(
            view,
            "String(JSON.stringify((function(){var m=document.getElementById('m');var r=m.getBoundingClientRect();\
             var cs=getComputedStyle(m);return {x:r.x,y:r.y,width:r.width,height:r.height,display:cs.display,\
             object_fit:cs.objectFit};})()))",
        )
        .unwrap();
    let json = value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .replace("\\\"", "\"");
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}: {json}"))
}

fn check(label: &str) {
    let receipt: serde_json::Value = serde_json::from_str(RECEIPT).unwrap();
    assert_eq!(receipt["chrome"], "148.0.7778.216");
    let case = receipt["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == label)
        .unwrap_or_else(|| panic!("no case {label:?}"));
    let got = measure(case["html"].as_str().unwrap());
    let mut failures = Vec::new();
    for key in ["x", "y", "width", "height"] {
        let (g, w) = (
            got[key].as_f64().unwrap_or(f64::NAN),
            case[key].as_f64().unwrap(),
        );
        if (g - w).abs() > 0.5 {
            failures.push(format!("{key} {g}, Chrome {w}"));
        }
    }
    for key in ["display", "object_fit"] {
        if got[key] != case[key] {
            failures.push(format!("{key} {}, Chrome {}", got[key], case[key]));
        }
    }
    assert!(failures.is_empty(), "{label}: {}", failures.join("; "));
}

#[test]
fn a_video_is_an_inline_300_by_150_replaced_box() {
    check("video");
}

#[test]
fn a_videos_fallback_content_is_not_laid_out() {
    check("video with fallback text");
}

#[test]
fn video_width_and_height_attributes_size_the_box() {
    check("video width=100 height=50");
}

/// No natural ratio before metadata: the other axis keeps its 150.
#[test]
fn a_video_width_attribute_alone_keeps_the_height() {
    check("video width=100");
}

#[test]
fn a_video_height_attribute_alone_keeps_the_width() {
    check("video height=60");
}

#[test]
fn a_css_width_alone_keeps_the_height() {
    check("video css width:120px");
}

#[test]
fn a_video_with_controls_is_300_by_150() {
    check("video controls");
}

#[test]
fn a_block_video() {
    check("video display:block");
}

#[test]
fn a_videos_border_and_padding_are_outside_its_300_by_150() {
    check("video border 5px padding 3px");
}

#[test]
fn an_audio_without_controls_is_not_displayed() {
    check("audio");
}

#[test]
fn an_audio_with_controls_is_an_inline_300_by_54_box() {
    check("audio controls");
}

/// #1332 R1: Chrome's UA rule hides an `<audio>` without `controls` over any author
/// `display` (`audio:not([controls]) { display: none !important }`).
#[test]
fn an_audio_without_controls_stays_hidden_under_an_author_display() {
    check("audio style display:block");
    check("audio sheet display:inline-block");
    check("audio style display:block !important");
}

/// #1332 R1: the UA's `object-fit: contain` is an ordinary declaration a page overrides.
#[test]
fn a_page_overrides_the_videos_object_fit() {
    check("video style object-fit:cover");
    check("video sheet object-fit:fill");
}
