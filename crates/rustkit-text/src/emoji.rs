//! Which characters take the colour-glyph (emoji) path. Shared by the
//! macOS (CoreText) and Windows (DirectWrite) rasterizers.

/// Whether a character should be rendered via the color-glyph (emoji) path
/// rather than the grayscale coverage-mask path.
///
/// Covers the common emoji/pictograph blocks. Deliberately conservative: text
/// symbols that a normal font renders as monochrome outlines (e.g. ™, ©, →)
/// are left to the grayscale path; only ranges that are color-bitmap in Apple
/// Color Emoji are routed here. Variation-selector-16 (U+FE0F, emoji
/// presentation) is handled by the caller on the base char.
pub fn is_emoji(ch: char) -> bool {
    let c = ch as u32;
    matches!(c,
        0x1F300..=0x1FAFF   // misc symbols & pictographs, emoticons, transport,
                            // supplemental & extended-A (covers 🏔 🌅 🌲 🌸 🌊 🗼 ☕→no)
        | 0x1F000..=0x1F0FF // mahjong/dominoes/playing cards
        | 0x2600..=0x27BF   // misc symbols (☕ ✨ ⚡) + dingbats (✅ ✂)
        | 0x2B00..=0x2BFF   // misc symbols & arrows (⭐ ⬆ used as emoji)
        | 0x1F1E6..=0x1F1FF // regional indicators (flags)
    )
}

#[cfg(test)]
mod tests {
    use super::is_emoji;

    #[test]
    fn emoji_ranges_are_platform_independent() {
        for (ch, want) in [('🏔', true), ('✨', true), ('🎯', true), ('☕', true), ('A', false), ('7', false), (' ', false), ('™', false)] {
            assert_eq!(is_emoji(ch), want, "{ch:?}");
        }
    }
}
