//! Portable pins for CB1b / #1027: `appearance` / `-webkit-appearance`
//! cascade into `ComputedStyle.appearance`.
//!
//! The page/paint oracle in `form_control_decoration_tests` is headless-only,
//! so default Linux `--lib` never saw the apply path. These call
//! `apply_style_property` / `apply_initial_value` directly.

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
fn appearance_none_and_webkit_none_set_none() {
    assert_eq!(
        applied(&[("appearance", "none")]).appearance,
        rustkit_css::Appearance::None
    );
    assert_eq!(
        applied(&[("-webkit-appearance", "none")]).appearance,
        rustkit_css::Appearance::None
    );
    assert_eq!(
        applied(&[("appearance", " None ")]).appearance,
        rustkit_css::Appearance::None,
        "leading/trailing space and case are ignored"
    );
}

#[test]
fn other_keywords_are_auto() {
    for value in ["auto", "button", "menulist", "textfield", ""] {
        assert_eq!(
            applied(&[("appearance", value)]).appearance,
            rustkit_css::Appearance::Auto,
            "{value:?}"
        );
        assert_eq!(
            applied(&[("-webkit-appearance", value)]).appearance,
            rustkit_css::Appearance::Auto,
            "-webkit {value:?}"
        );
    }
}

#[test]
fn initial_and_unset_restore_auto() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::default();
    engine
        .lctx
        .apply_style_property(&mut style, "appearance", "none");
    assert_eq!(style.appearance, rustkit_css::Appearance::None);

    engine.lctx.apply_initial_value(&mut style, "appearance");
    assert_eq!(style.appearance, rustkit_css::Appearance::Auto);

    engine
        .lctx
        .apply_style_property(&mut style, "-webkit-appearance", "none");
    engine
        .lctx
        .apply_initial_value(&mut style, "-webkit-appearance");
    assert_eq!(style.appearance, rustkit_css::Appearance::Auto);
}
