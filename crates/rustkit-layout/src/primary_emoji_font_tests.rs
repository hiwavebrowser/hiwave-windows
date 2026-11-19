//! A run whose PRIMARY face is Apple Color Emoji (`font-family: "Apple Color
//! Emoji"`) advances as the same emoji reached by fallback does: Chrome's
//! advance, not Core Text's tracked one (#1072 R1 nit 4). Chrome for Testing
//! 148.0.7778.216, `getBoundingClientRect`: `🔍` at 14px is 18, two at 16px
//! 40, `🛡` at 21px 23.

use super::*;
use rustkit_css::{FontStyle, FontWeight};

#[cfg(target_os = "macos")]
#[test]
fn the_emoji_face_as_primary_advances_as_chrome_places_it() {
    let w = |text: &str, size: f32| {
        measure_text_advanced(text, "Apple Color Emoji", size, FontWeight::NORMAL, FontStyle::Normal).width
    };
    for (text, size, chrome) in [
        ("\u{1F50D}", 14.0, 18.0),
        ("\u{1F50D}\u{1F50D}", 16.0, 40.0),
        ("\u{1F6E1}", 21.0, 23.0),
    ] {
        let got = w(text, size);
        assert!((got - chrome).abs() < 0.05, "{text:?} at {size}px: {got}, Chromium {chrome}");
    }
}

/// In the emoji face as the primary, the keycap bases, `#`, `*`, `©` and the
/// space take the same uniform advance as an emoji (Prometheus R1 LOW on
/// #1072, checked against Chrome for Testing 148: `1`, ` `, `#` and `©` are
/// each 18 at 14px; `"1 #"` and `"🔍 🔍"` are 54).
#[cfg(target_os = "macos")]
#[test]
fn characters_the_emoji_face_maps_take_its_uniform_advance() {
    let w = |text: &str| {
        measure_text_advanced(text, "Apple Color Emoji", 14.0, FontWeight::NORMAL, FontStyle::Normal).width
    };
    for (text, chrome) in [("1 #", 54.0), ("\u{1F50D} \u{1F50D}", 54.0), ("1", 18.0), (" ", 18.0), ("\u{A9}", 18.0)] {
        let got = w(text);
        assert!((got - chrome).abs() < 0.05, "{text:?}: {got}, Chromium {chrome}");
    }
}
