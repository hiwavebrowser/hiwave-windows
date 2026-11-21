//! Portable `Outline::parse_*` / used-size pins for #867.
//!
//! The headless `outline_tests` suite covers paint + computed script but is
//! gated behind `feature = "headless"`, so default Linux `--lib` never sees
//! negative-width refuse, signed offset, or Chromium's used-width floor.
//! A cascade/parser regression would ship green on every non-headless run.

use crate::{Length, Outline, OutlineStyle};

#[test]
fn parse_width_takes_keywords_and_non_negative_lengths() {
    assert_eq!(Outline::parse_width("thin"), Some(Length::Px(1.0)));
    assert_eq!(Outline::parse_width("medium"), Some(Length::Px(3.0)));
    assert_eq!(Outline::parse_width("thick"), Some(Length::Px(5.0)));
    assert_eq!(Outline::parse_width("1.5px"), Some(Length::Px(1.5)));
    // A bare `0` is `0px` (CH2.B14).
    assert_eq!(Outline::parse_width("0"), Some(Length::Px(0.0)));
    assert_eq!(Outline::parse_width("2em"), Some(Length::Em(2.0)));
}

#[test]
fn parse_width_refuses_negatives_percent_and_bare_nonzero() {
    assert_eq!(Outline::parse_width("-1px"), None);
    assert_eq!(Outline::parse_width("-2em"), None);
    assert_eq!(Outline::parse_width("10%"), None);
    assert_eq!(Outline::parse_width("2"), None);
    assert_eq!(Outline::parse_width("hidden"), None);
}

#[test]
fn parse_offset_takes_a_signed_length_and_refuses_percent() {
    assert_eq!(Outline::parse_offset("3px"), Some(Length::Px(3.0)));
    assert_eq!(Outline::parse_offset("-3px"), Some(Length::Px(-3.0)));
    assert_eq!(Outline::parse_offset("1.5em"), Some(Length::Em(1.5)));
    // A bare `0` is `0px` (CH2.B14).
    assert_eq!(Outline::parse_offset("0"), Some(Length::Px(0.0)));
    assert_eq!(Outline::parse_offset("10%"), None);
    assert_eq!(Outline::parse_offset("2"), None);
    assert_eq!(Outline::parse_offset("auto"), None);
}

#[test]
fn used_width_is_zero_without_style_and_floors_a_drawn_width() {
    let none = Outline {
        style: OutlineStyle::None,
        width: Some(Length::Px(10.0)),
        ..Outline::default()
    };
    assert_eq!(none.used_width(16.0, 16.0), 0.0);

    let solid = Outline {
        style: OutlineStyle::Solid,
        width: Some(Length::Px(1.5)),
        ..Outline::default()
    };
    assert_eq!(solid.used_width(16.0, 16.0), 1.0);

    let half = Outline {
        style: OutlineStyle::Solid,
        width: Some(Length::Px(0.5)),
        ..Outline::default()
    };
    assert_eq!(half.used_width(16.0, 16.0), 1.0);

    let medium = Outline {
        style: OutlineStyle::Solid,
        width: None,
        ..Outline::default()
    };
    assert_eq!(medium.used_width(16.0, 16.0), 3.0);
}

#[test]
fn used_offset_floors_to_a_sixty_fourth() {
    let outline = Outline {
        offset: Length::Px(1.0 / 64.0 + 0.001),
        ..Outline::default()
    };
    assert_eq!(outline.used_offset(16.0, 16.0), 1.0 / 64.0);

    let negative = Outline {
        offset: Length::Px(-3.0),
        ..Outline::default()
    };
    assert_eq!(negative.used_offset(16.0, 16.0), -3.0);
}

/// #866: rem outline-offset resolves against the root font, not the
/// element's. A 62.5% root is 10px, so `1.5rem` is 15px.
#[test]
fn used_offset_rem_uses_the_root_font() {
    let outline = Outline {
        offset: Length::Rem(1.5),
        ..Outline::default()
    };
    assert_eq!(outline.used_offset(16.0, 10.0), 15.0);
    assert_eq!(outline.used_offset(16.0, 16.0), 24.0);
}

#[test]
fn outline_style_keeps_auto_and_refuses_hidden() {
    assert_eq!(OutlineStyle::from_keyword("auto"), Some(OutlineStyle::Auto));
    assert_eq!(OutlineStyle::from_keyword("solid"), Some(OutlineStyle::Solid));
    assert_eq!(OutlineStyle::from_keyword("hidden"), None);
    assert_eq!(OutlineStyle::from_keyword("none"), Some(OutlineStyle::None));
}
