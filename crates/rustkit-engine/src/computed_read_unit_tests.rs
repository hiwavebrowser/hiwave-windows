//! Portable #866 rem CSSOM pins.
//!
//! `computed_read_tests` is macos+headless only. Linux `--lib` never saw
//! `set_root_font` resolve a root `font-size: 62.5%` to 10px, nor `length`
//! multiply rem by that root — the R1 fix that made `1.5rem` serialize as
//! `15px` instead of `24px` against a percentage root.

use super::computed_read::{length, root_font, set_root_font};
use rustkit_css::{ComputedStyle, Length};

#[test]
fn a_percent_root_font_size_is_taken_against_sixteen() {
    let mut style = ComputedStyle::new();
    style.font_size = Length::Percent(62.5);
    set_root_font(&style);
    assert_eq!(root_font(), 10.0);

    let mut style = ComputedStyle::new();
    style.font_size = Length::Px(20.0);
    set_root_font(&style);
    assert_eq!(root_font(), 20.0);

    let mut style = ComputedStyle::new();
    style.font_size = Length::Rem(1.25);
    set_root_font(&style);
    assert_eq!(root_font(), 20.0);
}

#[test]
fn rem_length_uses_the_root_font_not_the_element_font() {
    let mut root = ComputedStyle::new();
    root.font_size = Length::Percent(62.5);
    set_root_font(&root);

    // Element font is still 16px; rem must ignore it.
    assert_eq!(length(&Length::Rem(1.5), 16.0).as_deref(), Some("15px"));
    assert_eq!(length(&Length::Em(1.5), 16.0).as_deref(), Some("24px"));
    assert_eq!(length(&Length::Px(8.0), 16.0).as_deref(), Some("8px"));
}
