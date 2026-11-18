//! EM1: an emoji SEQUENCE is one cluster with one advance, as Chrome shapes
//! it. RustKit measured it one code point at a time: a ZWJ family was three
//! emoji wide, a skin-toned thumbs-up and a flag two.
//!
//! Every number is Chrome for Testing 148.0.7778.216
//! (`getBoundingClientRect` width of a `white-space: pre` span holding the
//! sequence alone; the emoji face's advance is 18 at 14px and 20 at 16px).

use super::*;
use rustkit_css::{FontStyle, FontWeight};

fn width(text: &str, family: &str, size: f32) -> f32 {
    measure_text_advanced(text, family, size, FontWeight::NORMAL, FontStyle::Normal).width
}

const FAMILY: &str = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
const SKIN: &str = "\u{1F44D}\u{1F3FD}";
const FLAG: &str = "\u{1F1FA}\u{1F1F8}";
const RAINBOW_FLAG: &str = "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}";
const KEYCAP: &str = "1\u{FE0F}\u{20E3}";
const VS16: &str = "\u{00A9}\u{FE0F}";

#[cfg(target_os = "macos")]
#[test]
fn an_emoji_sequence_is_one_cluster_with_one_advance() {
    for family in ["Helvetica", "system-ui"] {
        for (size, one) in [(14.0, 18.0), (16.0, 20.0)] {
            for (name, text) in [
                ("ZWJ family", FAMILY),
                ("skin tone", SKIN),
                ("flag", FLAG),
                ("rainbow flag", RAINBOW_FLAG),
                ("keycap", KEYCAP),
                ("VS16", VS16),
            ] {
                let got = width(text, family, size);
                assert!(
                    (got - one).abs() < 0.05,
                    "{name} in {family} at {size}px: {got}, Chromium {one}"
                );
            }
            // Two sequences side by side are two advances.
            let both = format!("{SKIN}{FLAG}");
            let got = width(&both, family, size);
            assert!((got - 2.0 * one).abs() < 0.05, "skin tone + flag in {family} at {size}px: {got}, Chromium {}", 2.0 * one);
        }
    }
}

/// A ZWJ sequence takes one advance per glyph the face forms it into, as in
/// Chrome: `👨‍🦲‍🙈` is `👨‍🦲` and `🙈`, 36 at 14px.
#[cfg(target_os = "macos")]
#[test]
fn a_partly_formed_zwj_sequence_is_one_advance_per_glyph() {
    let text = "\u{1F468}\u{200D}\u{1F9B2}\u{200D}\u{1F648}";
    let got = width(text, "Helvetica", 14.0);
    assert!((got - 36.0).abs() < 0.05, "man-bald + see-no-evil at 14px: {got}, Chromium 36");
}

/// A run whose own face is Apple Color Emoji folds its sequences too:
/// `👍🏽` in `font: 14px "Apple Color Emoji"` is 18 in Chrome for Testing
/// 148, one advance, as by fallback.
#[cfg(target_os = "macos")]
#[test]
fn a_sequence_in_the_emoji_face_as_primary_is_one_advance() {
    let got = width(SKIN, "Apple Color Emoji", 14.0);
    assert!((got - 18.0).abs() < 0.05, "skin tone in the emoji face at 14px: {got}, Chromium 18");
}
