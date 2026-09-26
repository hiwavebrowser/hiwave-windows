//! A character the primary face lacks (an emoji in system-ui text) is shaped
//! by the fallback face paint draws it with, and under `line-height: normal`
//! the line box grows to that face's extents — Blink's used-fonts model.
//! Receipt: pinned Chrome 148, `parity-tests/repro/emoji-line-height.html`
//! (16px system-ui: plain 18px, "☕ coffee" 26px; 20px: 23 → 29; an explicit
//! `line-height: 24px` stays 24). n40 lane, 2026-09-02.
#![cfg(target_os = "macos")]

use rustkit_css::{ComputedStyle, FontStyle, FontWeight, Length, LineHeight};
use rustkit_layout::{
    measure_text_advanced, run_line_height, BoxType, Dimensions, LayoutBox, Rect,
};

const FAMILY: &str = "-apple-system, BlinkMacSystemFont, sans-serif";

fn measure(text: &str, size: f32) -> rustkit_layout::TextMetrics {
    measure_text_advanced(text, FAMILY, size, FontWeight::NORMAL, FontStyle::Normal)
}

fn style(size: f32, line_height: LineHeight) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.font_family = FAMILY.to_string();
    s.font_size = Length::Px(size);
    s.line_height = line_height;
    s
}

#[test]
fn emoji_takes_the_fallback_face_advance_and_extents() {
    let plain = measure(" coffee", 16.0);
    let emoji = measure("☕ coffee", 16.0);

    // Apple Color Emoji's advance at 16px is ~1.25em; the old glyph-0 path
    // gave a half-em placeholder (8px) and drew the emoji over the "c".
    let emoji_advance = emoji.width - plain.width;
    assert!(
        emoji_advance > 12.0,
        "emoji advance should come from the fallback face, got {emoji_advance}"
    );

    // The run's extents are the union of the primary and the emoji face
    // (Apple Color Emoji 16px: ascent 20, descent 6.25).
    assert!(
        plain.ascent < 16.0,
        "primary ascent sanity: {}",
        plain.ascent
    );
    assert!(
        emoji.ascent >= 19.5,
        "united ascent should be the emoji face's, got {}",
        emoji.ascent
    );
    assert!(
        emoji.descent >= 6.0,
        "united descent should be the emoji face's, got {}",
        emoji.descent
    );
}

#[test]
fn normal_line_height_unites_the_used_faces_like_chrome() {
    let s16 = style(16.0, LineHeight::Normal);
    assert_eq!(
        run_line_height(&s16, 16.0, &measure("plain text", 16.0)),
        18.0
    );
    assert_eq!(
        run_line_height(&s16, 16.0, &measure("☕ coffee", 16.0)),
        26.0
    );

    let s20 = style(20.0, LineHeight::Normal);
    assert_eq!(
        run_line_height(&s20, 20.0, &measure("plain text", 20.0)),
        23.0
    );
    assert_eq!(
        run_line_height(&s20, 20.0, &measure("🎯 target", 20.0)),
        29.0
    );

    // An explicit line-height ignores the used faces (Chrome: 24 stays 24).
    let fixed = style(16.0, LineHeight::Px(24.0));
    assert_eq!(
        run_line_height(&fixed, 16.0, &measure("☕ coffee", 16.0)),
        24.0
    );

    // A primary face WITH a line gap (Arial: 14.48 + 3.39 + 0.52 → 18) gets
    // its half-leading per face, not stacked on the emoji face: Chrome 26.
    let mut arial = style(16.0, LineHeight::Normal);
    arial.font_family = "Arial".to_string();
    let m =
        |t: &str| measure_text_advanced(t, "Arial", 16.0, FontWeight::NORMAL, FontStyle::Normal);
    assert_eq!(run_line_height(&arial, 16.0, &m("plain arial")), 18.0);
    assert_eq!(run_line_height(&arial, 16.0, &m("☕ arial coffee")), 26.0);
}

#[test]
fn block_with_emoji_text_is_one_taller_line_box() {
    let mut cb = Dimensions::default();
    cb.content = Rect::new(0.0, 0.0, 600.0, 0.0);

    let lay = |text: &str| {
        let mut block = LayoutBox::new(BoxType::Block, style(16.0, LineHeight::Normal));
        block.children.push(LayoutBox::new(
            BoxType::Text(text.to_string()),
            style(16.0, LineHeight::Normal),
        ));
        block.layout(&cb);
        block.dimensions.content.height
    };

    assert_eq!(lay("plain text sixteen"), 18.0);
    assert_eq!(lay("☕ coffee sixteen"), 26.0);
}
