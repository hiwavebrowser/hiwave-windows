//! Portable #869 `aspect-ratio: auto <ratio>` cascade pins.
//!
//! The flex/grid geometry oracle in `aspect_ratio_tests` is macos+headless
//! only. Linux `--lib` never saw the parse that sets `aspect_ratio` and
//! `aspect_ratio_auto` for `auto 16/9` (either order) — the bug that made
//! `width: 160px; aspect-ratio: auto 16/9` drop the whole declaration.

use super::*;

#[test]
fn auto_before_or_after_a_ratio_keeps_the_ratio_and_sets_the_flag() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();

    engine
        .lctx
        .apply_style_property(&mut style, "aspect-ratio", "auto 16/9");
    assert!((style.aspect_ratio.unwrap() - 16.0 / 9.0).abs() < 1e-5);
    assert!(style.aspect_ratio_auto);

    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "aspect-ratio", "16/9 auto");
    assert!((style.aspect_ratio.unwrap() - 16.0 / 9.0).abs() < 1e-5);
    assert!(style.aspect_ratio_auto);

    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "aspect-ratio", "auto 2");
    assert!((style.aspect_ratio.unwrap() - 2.0).abs() < 1e-5);
    assert!(style.aspect_ratio_auto);
}

#[test]
fn bare_auto_clears_and_a_bare_ratio_does_not_set_the_flag() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "aspect-ratio", "2/1");
    assert!((style.aspect_ratio.unwrap() - 2.0).abs() < 1e-5);
    assert!(!style.aspect_ratio_auto);

    engine
        .lctx
        .apply_style_property(&mut style, "aspect-ratio", "auto");
    assert_eq!(style.aspect_ratio, None);
    assert!(!style.aspect_ratio_auto);
}

#[test]
fn invalid_auto_forms_and_negatives_leave_the_prior_ratio() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "aspect-ratio", "2/1");
    let kept = style.aspect_ratio;
    let kept_auto = style.aspect_ratio_auto;

    for bad in ["auto auto", "auto 16/9 auto", "-1", "1/-1", "foo", "16/9/9"] {
        engine
            .lctx
            .apply_style_property(&mut style, "aspect-ratio", bad);
        assert_eq!(style.aspect_ratio, kept, "{bad} changed the ratio");
        assert_eq!(style.aspect_ratio_auto, kept_auto, "{bad} changed the flag");
    }
}
