//! #786: `display: contents` declaration apply + flag, portable.
//!
//! The headless `css_display_tests` suite covers layout/paint companions but
//! is gated behind `feature = "headless"`, so default Linux CI never sees
//! even the pure `apply_style_property` pins. A regression that only sets
//! `display_contents_declared` (the early-arm → block bug) would ship
//! silently on every non-headless run.

use super::*;

#[test]
fn contents_sets_display_and_the_container_flag_and_a_later_display_clears_both() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::default();

    engine
        .lctx
        .apply_style_property(&mut style, "display", "contents");
    assert_eq!(style.display, rustkit_css::Display::Contents);
    assert!(style.display_contents_declared);

    engine
        .lctx
        .apply_style_property(&mut style, "display", " CONTENTS ");
    assert_eq!(style.display, rustkit_css::Display::Contents);
    assert!(style.display_contents_declared);

    engine
        .lctx
        .apply_style_property(&mut style, "display", "flex");
    assert_eq!(style.display, rustkit_css::Display::Flex);
    assert!(!style.display_contents_declared);

    engine
        .lctx
        .apply_style_property(&mut style, "display", "block");
    assert_eq!(style.display, rustkit_css::Display::Block);
    assert!(!style.display_contents_declared);
}

/// #840 / C3: `z-index: 0` and `z-index: auto` both leave `z_index` at 0;
/// only `z_index_declared` tells them apart for getComputedStyle. The page
/// oracle lives in headless-gated `computed_display_tests`.
#[test]
fn z_index_zero_is_declared_and_auto_is_not() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::default();

    engine
        .lctx
        .apply_style_property(&mut style, "z-index", "0");
    assert_eq!(style.z_index, 0);
    assert!(style.z_index_declared);

    engine
        .lctx
        .apply_style_property(&mut style, "z-index", "auto");
    assert_eq!(style.z_index, 0);
    assert!(!style.z_index_declared);

    engine
        .lctx
        .apply_style_property(&mut style, "z-index", "-1");
    assert_eq!(style.z_index, -1);
    assert!(style.z_index_declared);
}
