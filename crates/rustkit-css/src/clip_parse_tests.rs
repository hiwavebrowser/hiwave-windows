//! Portable `parse_clip` pins for Z row S5 (the legacy `clip` property).
//!
//! Which values parse is Chromium 143.0.7499.4's `CSS.supports('clip', v)`
//! (`trench/z/tools/s5/clip_parse_probe.mjs` on the Atlas hub). The paint is
//! pinned by the engine's headless `visually_hidden_tests`.

use crate::{parse_clip, ClipRect, Length};

#[test]
fn clip_takes_what_chromium_takes() {
    let cases: &[(&str, bool)] = &[
        ("auto", true),
        ("rect(0,0,0,0)", true),
        ("rect(0 0 0 0)", true),
        ("rect(1px, 1px, 1px, 1px)", true),
        ("rect(1px 1px 1px 1px)", true),
        ("rect(auto, 20px, auto, auto)", true),
        ("rect(auto auto auto auto)", true),
        ("rect(10%, 0, 0, 0)", false),
        ("rect(0, 0, 0)", false),
        ("rect(0,0 0,0)", false),
        ("rect(1em, 2em, 3em, 4em)", true),
        ("rect(-5px, 10px, 10px, -5px)", true),
        ("RECT(0,0,0,0)", true),
        ("rect(0,0,0,0,0)", false),
        ("rect()", false),
        ("inset(0)", false),
        ("rect(0,,0,0)", false),
        ("rect( 0 , 0 , 0 , 0 )", true),
        ("rect(1, 2, 3, 4)", false),
        ("rect(calc(1px + 1px), 0, 0, 0)", true),
    ];
    let wrong: Vec<String> = cases
        .iter()
        .filter(|(v, want)| parse_clip(v).is_some() != *want)
        .map(|(v, want)| format!("{v}: parsed {}, Chromium {want}", !want))
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn clip_sides_keep_auto_and_lengths() {
    assert_eq!(parse_clip("auto"), Some(None));
    assert_eq!(
        parse_clip("rect(auto, 20px, auto, 1em)"),
        Some(Some(ClipRect {
            top: None,
            right: Some(Length::Px(20.0)),
            bottom: None,
            left: Some(Length::Em(1.0)),
        }))
    );
}
