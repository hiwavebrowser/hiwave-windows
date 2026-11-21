//! Portable #870 R1 cascade pins: keep a shorthand over an unreadable but
//! valid colour function; drop it over a mixed-form grammar error.
//!
//! The same cases live in headless `color_space_syntax_tests`, which default
//! Linux `--lib` never compiles. These use only `apply_style_property`.

use super::*;
use rustkit_css::Length;

#[test]
fn a_mixed_form_colour_drops_the_whole_shorthand() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for bad in ["rgb(1, 2 3)", "rgb(1 2 3, 0.5)", "hsl(1, 2% 3%)"] {
        let mut style = ComputedStyle::new();
        style.border_top_width = Length::Px(1.0);
        engine
            .lctx
            .apply_style_property(&mut style, "border", &format!("7px solid {bad}"));
        if style.border_top_width != Length::Px(1.0) {
            wrong.push(format!(
                "border: 7px solid {bad}: width became {:?}",
                style.border_top_width
            ));
        }
        let mut style = ComputedStyle::new();
        engine
            .lctx
            .apply_style_property(&mut style, "box-shadow", &format!("0 0 0 3px {bad}"));
        if !style.box_shadows.is_empty() {
            wrong.push(format!(
                "box-shadow: 0 0 0 3px {bad}: kept {:?}",
                style.box_shadows
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn an_unreadable_but_valid_colour_function_keeps_the_shorthand() {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut wrong = Vec::new();
    for value in [
        "rgb(from red r g b)",
        "rgb(from red r g b / 50%)",
        "hsl(from red h s l)",
        "rgb(mod(5, 3) 0 0)",
        "rgb(sin(1) 0 0)",
    ] {
        let mut style = ComputedStyle::new();
        engine
            .lctx
            .apply_style_property(&mut style, "border", "4px dashed red");
        engine
            .lctx
            .apply_style_property(&mut style, "border", &format!("1px solid {value}"));
        if style.border_top_width != Length::Px(1.0) {
            wrong.push(format!(
                "border: 1px solid {value}: width {:?}",
                style.border_top_width
            ));
        }
        let mut style = ComputedStyle::new();
        engine
            .lctx
            .apply_style_property(&mut style, "background", "url(a.png) red");
        engine
            .lctx
            .apply_style_property(&mut style, "background", &format!("url(x.png) {value}"));
        let image = style
            .background_layers
            .first()
            .map(|l| format!("{:?}", l.image));
        if !image.as_deref().is_some_and(|i| i.contains("x.png")) {
            wrong.push(format!("background: url(x.png) {value}: image {image:?}"));
        }
        let mut style = ComputedStyle::new();
        engine
            .lctx
            .apply_style_property(&mut style, "box-shadow", &format!("1px 2px 3px {value}"));
        if style.box_shadows.len() != 1 {
            wrong.push(format!(
                "box-shadow: 1px 2px 3px {value}: {} shadows",
                style.box_shadows.len()
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
