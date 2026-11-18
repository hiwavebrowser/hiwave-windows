//! Portable TX2 / #1053 pins for the fixed-font (13px) default.
//!
//! The page oracle in `monospace_size_tests` is headless-only, so default
//! Linux `--lib` never saw `is_generic_monospace`, the keyword tables, the
//! keyword-clear on a length write, or `apply_fixed_font_default`.

use crate::{ComputedStyle, Length};

fn style_with_family(family: &str) -> ComputedStyle {
    let mut style = ComputedStyle::new();
    style.font_family = family.to_string();
    style
}

#[test]
fn bare_monospace_is_the_generic_alone() {
    assert!(style_with_family("monospace").is_generic_monospace());
    assert!(style_with_family("  Monospace  ").is_generic_monospace());
    assert!(!style_with_family("monospace, monospace").is_generic_monospace());
    assert!(!style_with_family("Menlo, monospace").is_generic_monospace());
    assert!(!style_with_family("sans-serif").is_generic_monospace());
}

#[test]
fn keyword_tables_use_13_for_fixed_and_16_for_proportional() {
    // medium (4): 16 proportional, 13 fixed — Blink kStrictFontSizeTable.
    assert_eq!(ComputedStyle::font_size_for_keyword(4, false), 16.0);
    assert_eq!(ComputedStyle::font_size_for_keyword(4, true), 13.0);
    // small (3): 13 vs 12; large (5): 18 vs 16.
    assert_eq!(ComputedStyle::font_size_for_keyword(3, false), 13.0);
    assert_eq!(ComputedStyle::font_size_for_keyword(3, true), 12.0);
    assert_eq!(ComputedStyle::font_size_for_keyword(5, false), 18.0);
    assert_eq!(ComputedStyle::font_size_for_keyword(5, true), 16.0);
}

#[test]
fn a_length_write_clears_the_keyword_and_marks_absolute() {
    let mut style = ComputedStyle::new();
    assert_eq!(style.font_size_keyword, 4, "initial medium");
    style.set_font_size_length(Length::Px(20.0));
    assert_eq!(style.font_size_keyword, 0);
    assert!(style.font_size_absolute);
    assert_eq!(style.font_size, Length::Px(20.0));

    // em/% keep inherited absoluteness unset here (parent not absolute).
    let mut relative = ComputedStyle::new();
    relative.font_size_absolute = false;
    relative.set_font_size_length(Length::Em(2.0));
    assert_eq!(relative.font_size_keyword, 0);
    assert!(!relative.font_size_absolute);
    assert_eq!(relative.font_size, Length::Em(2.0));
}

#[test]
fn a_keyword_write_keeps_the_keyword_and_is_not_absolute() {
    let mut style = ComputedStyle::new();
    style.set_font_size_keyword(3); // small → 13px proportional row
    assert_eq!(style.font_size_keyword, 3);
    assert!(!style.font_size_absolute);
    assert_eq!(style.font_size, Length::Px(13.0));
}

#[test]
fn apply_fixed_font_default_reads_the_fixed_row_for_bare_monospace() {
    let mut child = style_with_family("monospace");
    child.set_font_size_keyword(4); // medium
    let parent = style_with_family("sans-serif");
    child.apply_fixed_font_default(Some(&parent));
    assert_eq!(
        child.font_size,
        Length::Px(13.0),
        "bare monospace medium is the fixed-font default"
    );

    // Leaving monospace for a proportional parent scales a non-keyword size.
    let mut leaving = style_with_family("sans-serif");
    leaving.font_size = Length::Px(13.0);
    leaving.font_size_keyword = 0;
    leaving.font_size_absolute = false;
    let mono_parent = style_with_family("monospace");
    leaving.apply_fixed_font_default(Some(&mono_parent));
    assert_eq!(leaving.font_size, Length::Px(16.0));

    // An absolute size never moves.
    let mut absolute = style_with_family("monospace");
    absolute.set_font_size_length(Length::Px(20.0));
    absolute.apply_fixed_font_default(Some(&parent));
    assert_eq!(absolute.font_size, Length::Px(20.0));
}
