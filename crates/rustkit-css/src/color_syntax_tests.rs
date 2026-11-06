//! `rgb()` / `hsl()` in CSS Color 4's space-separated form, which Tailwind v3
//! and most current sites write (`rgb(70 31 140 / 60%)`). The parser took
//! only the legacy comma form, so every one of these was dropped and the
//! property fell back (an outline ring to currentcolor). Every expected value
//! is Chromium 143.0.7499.4's `getComputedStyle(..).color` for the input;
//! Chromium gives the same answer for background-color, border-color,
//! box-shadow, outline-color and gradient stops.

use crate::{parse_color, Color};

fn rgb(r: u8, g: u8, b: u8) -> Option<Color> {
    Some(Color::from_rgb(r, g, b))
}

fn rgba(r: u8, g: u8, b: u8, a: f32) -> Option<Color> {
    Some(Color::new(r, g, b, a))
}

fn check(cases: &[(&str, Option<Color>)]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter(|(input, want)| parse_color(input) != *want)
        .map(|(input, want)| format!("{input}: got {:?}, want {want:?}", parse_color(input)))
        .collect();
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn space_separated_rgb_and_hsl_parse() {
    check(&[
        ("rgb(24 25 26)", rgb(24, 25, 26)),
        ("rgb(70 31 140 / 60%)", rgba(70, 31, 140, 0.6)),
        ("rgb(70 31 140 / 0.6)", rgba(70, 31, 140, 0.6)),
        ("rgb(0 0 0 / .1)", rgba(0, 0, 0, 0.1)),
        ("rgb(1 2 3/.5)", rgba(1, 2, 3, 0.5)),
        ("rgb(  1   2   3  )", rgb(1, 2, 3)),
        ("rgba(1 2 3 / 0.5)", rgba(1, 2, 3, 0.5)),
        ("rgb(255 0 0 / 0.5)", rgba(255, 0, 0, 0.5)),
        ("rgb(1e2 0 0)", rgb(100, 0, 0)),
        ("hsl(210 40% 50% / 0.5)", rgba(77, 128, 179, 0.5)),
        ("hsl(210deg 40% 50%)", rgb(77, 128, 179)),
        ("hsla(120 100% 25%)", rgb(0, 128, 0)),
        ("hsl(120 100% 50% / 25%)", rgba(0, 255, 0, 0.25)),
        ("hsl(-120 50% 50%)", rgb(64, 64, 191)),
    ]);
}

#[test]
fn channels_take_percentages_none_and_clamp() {
    check(&[
        ("rgb(100% 50% 0%)", rgb(255, 128, 0)),
        ("rgb(10% 20 30)", rgb(26, 20, 30)),
        ("rgb(none 128 255)", rgb(0, 128, 255)),
        ("rgb(1 2 3 / none)", rgba(1, 2, 3, 0.0)),
        ("hsl(none 0% 50%)", rgb(128, 128, 128)),
        ("rgb(300 -5 128.6)", rgb(255, 0, 129)),
        ("rgb(1 2 3 / 150%)", rgb(1, 2, 3)),
        ("rgb(1 2 3 / -1)", rgba(1, 2, 3, 0.0)),
        ("hsl(210 40 50)", rgb(77, 128, 179)),
        ("hsl(0.5turn 100% 50%)", rgb(0, 255, 255)),
        ("hsl(1rad 50% 50%)", rgb(191, 186, 64)),
        ("hsl(100grad 50% 50%)", rgb(128, 191, 64)),
        ("hsl(90 50% 50%)", rgb(128, 191, 64)),
        ("RGB(1 2 3)", rgb(1, 2, 3)),
    ]);
}

#[test]
fn calc_channels_resolve() {
    check(&[
        ("rgb(calc(10 + 20) 0 0)", rgb(30, 0, 0)),
        ("rgb(calc(10 + 20), 0, 0)", rgb(30, 0, 0)),
        ("rgb(calc(50%) 0 0)", rgb(128, 0, 0)),
        ("rgb(0 0 0 / calc(0.5))", rgba(0, 0, 0, 0.5)),
    ]);
}

#[test]
fn mixed_and_malformed_forms_are_invalid() {
    check(&[
        ("rgb(1, 2 3)", None),
        ("rgb(1 2 3, 0.5)", None),
        ("rgb(1 2 3 / 0.5, 1)", None),
        ("rgb(10%, 20, 30)", None),
        ("rgb(none, 2, 3)", None),
        ("rgb(1 2)", None),
        ("rgb(1 2 3 4)", None),
        ("rgb(1 2 3 /)", None),
        ("rgb(/ 1 2 3)", None),
        ("rgb(1,2,3,)", None),
    ]);
}

#[test]
fn the_legacy_comma_form_matches_chromium() {
    check(&[
        ("rgb(255,0,0)", rgb(255, 0, 0)),
        ("rgb(1, 2, 3)", rgb(1, 2, 3)),
        ("rgba(255, 0, 0, 0.5)", rgba(255, 0, 0, 0.5)),
        ("rgba(0,0,0,.1)", rgba(0, 0, 0, 0.1)),
        ("hsla(0, 100%, 50%, .25)", rgba(255, 0, 0, 0.25)),
        ("hsl(-120, 50%, 50%)", rgb(64, 64, 191)),
        // Chromium also takes these legacy forms, which the comma parser
        // dropped (or, for alpha above 1, kept unclamped): a change, not a
        // no-change guard.
        ("rgba(10, 20, 30, 50%)", rgba(10, 20, 30, 0.5)),
        ("rgb(10%, 20%, 30%)", rgb(26, 51, 77)),
        ("rgba(255,0,0,2)", rgb(255, 0, 0)),
        ("rgb(1.5, 2, 3)", rgb(2, 2, 3)),
        // A channel on an exact .5 rounds up, as Chromium's does; the f32
        // conversion rounded these down (52 of a 1827-point hsl() grid
        // probed in Chromium were one step off; now none are).
        ("hsl(90, 50%, 50%)", rgb(128, 191, 64)),
        ("hsl(0, 0%, 90%)", rgb(230, 230, 230)),
        ("hsl(30, 100%, 90%)", rgb(255, 230, 204)),
    ]);
}

/// R1 on #870: what Chromium 143 does at the edges of the grammar.
/// A comma decides the syntax at the first separator that has one, so
/// `rgb(1 2, 3)` is the legacy form (then held to its rules); a CSS number
/// needs a digit after its point; out-of-range numbers clamp.
#[test]
fn the_comma_quirk_and_number_edges_match_chromium() {
    check(&[
        ("rgb(1 2, 3)", rgb(1, 2, 3)),
        ("rgb(1 2 , 3)", rgb(1, 2, 3)),
        ("rgb(1 2, 3, 0.5)", rgba(1, 2, 3, 0.5)),
        ("rgb(1% 2%, 3%)", rgb(3, 5, 8)),
        ("hsl(1 2%, 3%)", rgb(8, 8, 7)),
        ("rgb(1 2, 3 4)", None),
        ("rgb(1 2%, 3)", None),
        ("rgb(none 2, 3)", None),
        ("rgb(1 2, none)", None),
        ("rgb(1 2, 3 / 0.5)", None),
        ("hsl(1 2%, 3)", None),
        ("rgb(5. 0 0)", None),
        ("rgb(1e2.5 0 0)", None),
        ("rgb(1e 0 0)", None),
        ("rgb(.5e1 0 0)", rgb(5, 0, 0)),
        ("rgb(+5 0 0)", rgb(5, 0, 0)),
        ("rgb(5.0 0 0)", rgb(5, 0, 0)),
        ("rgb(1e40 0 0)", rgb(255, 0, 0)),
        ("rgb(-1e40 0 0)", rgb(0, 0, 0)),
        ("rgb(1 2 3 / 1e40)", rgb(1, 2, 3)),
        ("rgb(1e40%, 0%, 0%)", rgb(255, 0, 0)),
        ("rgba(1,2,3,1e40)", rgb(1, 2, 3)),
        ("rgb(1 /* c */ 2 3)", rgb(1, 2, 3)),
    ]);
}

/// Math functions and constants in a channel resolve as Chromium's do.
#[test]
fn math_function_channels_resolve() {
    check(&[
        ("rgb(min(255, 300) 0 0)", rgb(255, 0, 0)),
        ("rgb(min(255,300) 0 0 / 50%)", rgba(255, 0, 0, 0.5)),
        ("rgb(max(10, 20) 0 0)", rgb(20, 0, 0)),
        ("rgb(clamp(0, 300, 255) 0 0)", rgb(255, 0, 0)),
        ("rgb(clamp(0%, 50%, 100%) 0 0)", rgb(128, 0, 0)),
        ("rgb(min(10%, 50%) 0 0)", rgb(26, 0, 0)),
        ("rgb(min(1,2), 0, 0)", rgb(1, 0, 0)),
        ("rgb(0 0 0 / min(0.5, 1))", rgba(0, 0, 0, 0.5)),
        ("hsl(max(90deg, 10deg) 50% 50%)", rgb(128, 191, 64)),
        ("rgb(round(10.6) 0 0)", rgb(11, 0, 0)),
        ("rgb(round(10.5) 0 0)", rgb(11, 0, 0)),
        ("rgb(round(12, 5) 0 0)", rgb(10, 0, 0)),
        ("rgb(abs(-5) 0 0)", rgb(5, 0, 0)),
        ("rgb(sign(-3) 0 0)", rgb(0, 0, 0)),
        ("rgb(calc(pi * 10) 0 0)", rgb(31, 0, 0)),
        ("rgb(calc(e) 0 0)", rgb(3, 0, 0)),
        ("rgb(calc(infinity) 0 0)", rgb(255, 0, 0)),
        ("rgb(calc(-infinity) 0 0)", rgb(0, 0, 0)),
        ("rgb(calc(NaN) 0 0)", rgb(0, 0, 0)),
        ("rgb(calc(1e40) 0 0)", rgb(255, 0, 0)),
        ("rgb(calc(10% + 5) 0 0)", None),
        ("rgb(min(10%, 5) 0 0)", None),
    ]);
}
