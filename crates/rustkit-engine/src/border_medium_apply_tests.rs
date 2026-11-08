//! Portable pins for CH2.B18 / #950: a border side with a style and no
//! width takes `medium` (3px); `thin`/`medium`/`thick` are widths.
//!
//! The page oracle in `border_medium_tests` is macos+headless only, so
//! default Linux `--lib` never saw the apply path. These call
//! `apply_style_property` directly (no form-control UA hold).

use super::*;

fn applied(decls: &[(&str, &str)]) -> ComputedStyle {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::default();
    for &(property, value) in decls {
        engine.lctx.apply_style_property(&mut style, property, value);
    }
    style
}

#[test]
fn a_side_style_without_a_width_takes_medium() {
    let style = applied(&[("border-top-style", "solid")]);
    assert_eq!(style.border_top_style, rustkit_css::BorderStyle::Solid);
    assert_eq!(style.border_top_width, rustkit_css::Length::Px(3.0));
    // Unstyled sides stay at the unset width sentinel.
    assert_eq!(style.border_right_width, rustkit_css::Length::Zero);
}

#[test]
fn the_border_shorthand_without_a_width_is_medium_on_every_side() {
    let style = applied(&[("border", "solid")]);
    for width in [
        &style.border_top_width,
        &style.border_right_width,
        &style.border_bottom_width,
        &style.border_left_width,
    ] {
        assert_eq!(*width, rustkit_css::Length::Px(3.0));
    }
    assert_eq!(style.border_top_style, rustkit_css::BorderStyle::Solid);
}

#[test]
fn thin_medium_thick_are_one_three_five_pixels() {
    let thin = applied(&[("border-top-width", "thin"), ("border-top-style", "solid")]);
    assert_eq!(thin.border_top_width, rustkit_css::Length::Px(1.0));
    let medium = applied(&[("border-right-width", "medium"), ("border-right-style", "dashed")]);
    assert_eq!(medium.border_right_width, rustkit_css::Length::Px(3.0));
    let thick = applied(&[("border-bottom-width", "thick"), ("border-bottom-style", "dotted")]);
    assert_eq!(thick.border_bottom_width, rustkit_css::Length::Px(5.0));
}

#[test]
fn an_authored_zero_width_is_not_promoted_to_medium() {
    // `Length::Zero` is the unset sentinel; an authored `0` is `Px(0.0)` and
    // must keep painting nothing when a style is later set (#950).
    let style = applied(&[("border-left-width", "0"), ("border-left-style", "solid")]);
    assert_eq!(style.border_left_width, rustkit_css::Length::Px(0.0));
    assert_eq!(style.border_left_style, rustkit_css::BorderStyle::Solid);
}

#[test]
fn border_none_forces_a_zero_width() {
    let style = applied(&[("border", "thick solid red"), ("border", "none")]);
    assert_eq!(style.border_top_width, rustkit_css::Length::Zero);
    assert_eq!(style.border_top_style, rustkit_css::BorderStyle::None);
}
