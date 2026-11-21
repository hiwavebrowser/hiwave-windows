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

/// The fallback face is the one the system's cascade gives for the primary
/// face, as in Chrome. Courier lacks `⌘`, `←` and `→`; its cascade face is
/// Menlo, so each is measured at Menlo's advance. A fixed list tried Apple
/// Symbols first: an `about` `<kbd>⌘</kbd>` was 22.3px wide against
/// Chrome's 21.23px.
#[test]
fn a_symbol_courier_lacks_is_measured_in_its_cascade_face() {
    let m = |t: &str, family: &str| {
        measure_text_advanced(t, family, 16.0, FontWeight::NORMAL, FontStyle::Normal).width
    };
    for symbol in ["\u{2318}", "\u{2190}", "\u{2192}"] {
        let menlo = m(symbol, "Menlo");
        assert_eq!(m(symbol, "Courier"), menlo, "{symbol} in Courier");
        // `monospace` is Courier, so this is what a `<kbd>` measures.
        assert_eq!(m(symbol, "monospace"), menlo, "{symbol} in monospace");
        assert_ne!(m(symbol, "Apple Symbols"), menlo, "{symbol}: the test cannot tell the faces apart");
    }
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

    // `⏰` and `⌨` are emoji outside `rustkit_text::is_emoji`'s ranges; their
    // line is still the emoji face's (an `about` feature row and heading).
    assert_eq!(
        run_line_height(&s16, 16.0, &measure("\u{23F0} alarm", 16.0)),
        26.0
    );
    assert_eq!(
        run_line_height(&s16, 16.0, &measure("\u{2328}\u{FE0F} keys", 16.0)),
        26.0
    );

    // Chrome for Testing 148 (the chrome sidebar's action icons, flex items
    // at 14px system-ui): an emoji WITHOUT Emoji_Presentation takes the
    // system's cascade face. `🛡` is `.AppleColorEmojiUI` (the system font's
    // extents: 17, not 23) and `⚙` is Menlo (17, 8.44px wide). `🚫` has
    // Emoji_Presentation (Apple Color Emoji: 23), and so does `🛡` with
    // U+FE0F after it.
    let s14 = style(14.0, LineHeight::Normal);
    let line14 = |t: &str| run_line_height(&s14, 14.0, &measure(t, 14.0));
    assert_eq!(line14("plain"), 17.0);
    assert_eq!(line14("\u{1F6E1}"), 17.0, "shield");
    assert_eq!(line14("\u{2699}"), 17.0, "gear");
    assert_eq!(line14("\u{1F6AB}"), 23.0, "no-entry");
    assert_eq!(line14("\u{1F6E1}\u{FE0F}"), 23.0, "shield + VS16");
    let gear = measure("\u{2699}", 14.0).width;
    assert!((gear - 8.4375).abs() < 0.05, "gear is Menlo's advance (Chrome 8.4375), got {gear}");

    // A variation selector decides for the primary face too (Chrome for
    // Testing 148, presentation-matrix probe 2026-11-08): `©` + U+FE0F is
    // the emoji face though the system font has `©` (its advance, a 23px
    // line), while `▾` U+25BE + U+FE0F stays the primary (no Emoji
    // property: 17, `▾`'s own advance). `⚙` + U+FE0E keeps Menlo.
    let emoji_advance = measure("\u{1F6AB}", 14.0).width;
    assert_eq!(line14("\u{A9}\u{FE0F}"), 23.0, "copyright + VS16");
    assert_eq!(measure("\u{A9}\u{FE0F}", 14.0).width, emoji_advance, "copyright + VS16 advance");
    assert_eq!(line14("\u{25BE}\u{FE0F}"), 17.0, "small down triangle + VS16");
    assert_eq!(
        measure("\u{25BE}\u{FE0F}", 14.0).width,
        measure("\u{25BE}", 14.0).width,
        "small down triangle + VS16 advance"
    );
    assert_eq!(line14("\u{2699}\u{FE0E}"), 17.0, "gear + VS15");
    assert_eq!(measure("\u{2699}\u{FE0E}", 14.0).width, gear, "gear + VS15 advance");

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

/// The face is ONE decision shared by layout and paint: for every symbol ×
/// {plain, U+FE0E, U+FE0F} under a primary that has or lacks its glyph, the
/// advance the shaper measured is the advance the rasterizer's glyph carries
/// (Chrome for Testing 148 presentation-matrix probe, 2026-11-08; the face
/// names are pinned in rustkit-text).
#[test]
fn layout_advance_is_the_painted_advance_for_every_presentation() {
    use rustkit_css::FontStretch;
    use rustkit_layout::{FontFamilyChain, TextShaper};
    use rustkit_text::macos::GlyphRasterizer;

    let shaper = TextShaper::new();
    let mut wrong = Vec::new();
    for family in ["-apple-system, BlinkMacSystemFont, sans-serif", "Helvetica", "Menlo"] {
        let paint = GlyphRasterizer::with_style(family, 14.0, 400, false);
        for ch in ['\u{A9}', '\u{2699}', '\u{1F6E1}', '\u{25BE}', '\u{2328}', '\u{263A}', '\u{2764}'] {
            for (next, variant) in [(None, "plain"), (Some('\u{FE0E}'), "VS15"), (Some('\u{FE0F}'), "VS16")] {
                let mut text = ch.to_string();
                if let Some(vs) = next {
                    text.push(vs);
                }
                let run = shaper
                    .shape(
                        &text,
                        &FontFamilyChain::from_css_value(family),
                        FontWeight::NORMAL,
                        FontStyle::Normal,
                        FontStretch::Normal,
                        14.0,
                    )
                    .expect("shapes");
                let layout_advance = run.glyphs[0].advance;
                // Paint's side: the face the shaper recorded on the glyph
                // (the display list carries it), nothing re-decided.
                let face = run.glyphs[0].face;
                let paint_advance = paint.rasterize_with_face(ch, face).expect("paints").advance();
                if (layout_advance - paint_advance).abs() > 0.01 {
                    wrong.push(format!(
                        "{family} {ch:?} {variant}: {face:?} layout {layout_advance} vs paint {paint_advance}"
                    ));
                }
                // A rasterizer without the record decides the same face
                // from the same font.
                let auto = paint.select_face(ch, rustkit_text::Variation::after(next));
                if auto != face {
                    wrong.push(format!("{family} {ch:?} {variant}: layout {face:?} vs paint's own {auto:?}"));
                }
                // The selector itself is zero-width wherever it lands.
                if let Some(vs) = run.glyphs.get(1) {
                    if vs.advance != 0.0 {
                        wrong.push(format!("{family} {ch:?} {variant}: the selector advances {}", vs.advance));
                    }
                }
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
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
