//! Portable #867 outline cascade pins via `apply_style_property`.
//!
//! Headless `outline_tests` needs GPU ink + computed script. These pin the
//! parse/apply contract Linux `--lib` actually compiles: shorthand leaves
//! `outline-offset` alone, negative width is a no-op, signed offset sticks,
//! and `hidden` is not an outline style.

use super::*;

#[test]
fn outline_shorthand_sets_style_and_width_but_leaves_offset() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    style.outline.offset = rustkit_css::Length::Px(7.0);

    engine
        .lctx
        .apply_style_property(&mut style, "outline", "2px solid red");
    assert_eq!(style.outline.style, rustkit_css::OutlineStyle::Solid);
    assert_eq!(style.outline.width, Some(rustkit_css::Length::Px(2.0)));
    assert_eq!(style.outline.color, Some(rustkit_css::Color::from_rgb(255, 0, 0)));
    assert_eq!(style.outline.offset, rustkit_css::Length::Px(7.0));
}

#[test]
fn outline_width_refuses_a_negative_and_offset_takes_one() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "outline-width", "4px");
    assert_eq!(style.outline.width, Some(rustkit_css::Length::Px(4.0)));

    engine
        .lctx
        .apply_style_property(&mut style, "outline-width", "-2px");
    assert_eq!(style.outline.width, Some(rustkit_css::Length::Px(4.0)));

    engine
        .lctx
        .apply_style_property(&mut style, "outline-offset", "-3px");
    assert_eq!(style.outline.offset, rustkit_css::Length::Px(-3.0));
}

#[test]
fn outline_style_refuses_hidden_and_takes_auto() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "outline-style", "solid");
    assert_eq!(style.outline.style, rustkit_css::OutlineStyle::Solid);

    engine
        .lctx
        .apply_style_property(&mut style, "outline-style", "hidden");
    assert_eq!(style.outline.style, rustkit_css::OutlineStyle::Solid);

    engine
        .lctx
        .apply_style_property(&mut style, "outline-style", "auto");
    assert_eq!(style.outline.style, rustkit_css::OutlineStyle::Auto);
}

#[test]
fn a_bad_outline_shorthand_part_refuses_the_whole_declaration() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut style = ComputedStyle::new();
    engine
        .lctx
        .apply_style_property(&mut style, "outline", "2px solid red");
    let kept = style.outline.clone();

    engine
        .lctx
        .apply_style_property(&mut style, "outline", "2px solid hidden");
    assert_eq!(style.outline, kept);

    engine
        .lctx
        .apply_style_property(&mut style, "outline", "2px solid -1px");
    assert_eq!(style.outline, kept);
}
