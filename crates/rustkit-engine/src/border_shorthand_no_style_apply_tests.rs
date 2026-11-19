//! Portable pins for CH2.B18b / #987: a `border` / `border-<side>` shorthand
//! with no style keyword sets style to its initial value, `none`.
//!
//! The page oracle in `border_shorthand_no_style_tests` is macos+headless
//! only, so default Linux `--lib` never saw the apply path. These call
//! `apply_style_property` directly (same shape as `border_medium_apply_tests`).

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
fn a_width_only_border_shorthand_sets_style_none() {
    // Chromium 143: `div { border: 5px }` paints nothing. Before #987 the
    // apply path kept RustKit's default `Solid` and drew a 5px frame.
    let style = applied(&[("border", "5px")]);
    for side in [
        style.border_top_style,
        style.border_right_style,
        style.border_bottom_style,
        style.border_left_style,
    ] {
        assert_eq!(side, rustkit_css::BorderStyle::None);
    }
    assert_eq!(style.border_top_width, rustkit_css::Length::Px(5.0));
}

#[test]
fn a_later_width_only_shorthand_clears_a_prior_solid() {
    // `border-style: solid; border: 5px` is still none: the shorthand resets
    // every longhand it omits (CSS Backgrounds 3 §3.6).
    let style = applied(&[("border-style", "solid"), ("border", "5px")]);
    assert_eq!(style.border_top_style, rustkit_css::BorderStyle::None);
    assert_eq!(style.border_top_width, rustkit_css::Length::Px(5.0));
}

#[test]
fn a_width_and_style_shorthand_keeps_the_style() {
    let style = applied(&[("border", "5px solid")]);
    assert_eq!(style.border_top_style, rustkit_css::BorderStyle::Solid);
    assert_eq!(style.border_top_width, rustkit_css::Length::Px(5.0));
}

#[test]
fn a_width_only_side_shorthand_sets_that_side_to_none() {
    let style = applied(&[("border-left", "2px")]);
    assert_eq!(style.border_left_style, rustkit_css::BorderStyle::None);
    assert_eq!(style.border_left_width, rustkit_css::Length::Px(2.0));
    // Other sides stay at the unset defaults.
    assert_eq!(style.border_top_style, rustkit_css::BorderStyle::Solid);
}
