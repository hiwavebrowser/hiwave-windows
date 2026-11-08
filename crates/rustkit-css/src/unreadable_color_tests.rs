//! Portable pins for `is_unreadable_color_function` (#870 R1).
//!
//! Relative colour and nested math Chromium accepts but RustKit cannot
//! evaluate must not be treated as a grammar error that drops a shorthand.
//! The cascade companions live in headless `color_space_syntax_tests`; this
//! pins the pure classifier that decides keep-vs-drop on default `--lib`.

use crate::is_unreadable_color_function;

#[test]
fn relative_colour_and_unread_math_are_unreadable_not_junk() {
    for value in [
        "rgb(from red r g b)",
        "rgb(from red r g b / 50%)",
        "hsl(from red h s l)",
        "rgb(mod(5, 3) 0 0)",
        "rgb(sin(1) 0 0)",
        "rgba(from blue r g b / alpha)",
    ] {
        assert!(
            is_unreadable_color_function(value),
            "{value} must keep a shorthand"
        );
    }
}

#[test]
fn a_plain_grammar_error_is_not_unreadable() {
    for value in [
        "rgb(1, 2 3)",
        "rgb(1 2 3, 0.5)",
        "hsl(1, 2% 3%)",
        "rgb()",
        "not-a-color",
        "rgb(1 2)",
    ] {
        assert!(
            !is_unreadable_color_function(value),
            "{value} must drop a shorthand"
        );
    }
}

#[test]
fn a_colour_this_parser_reads_is_not_unreadable() {
    for value in [
        "rgb(70 31 140 / 60%)",
        "rgb(1, 2, 3)",
        "hsl(210 40% 50%)",
        "rgb(calc(1 + 2) 0 0)",
    ] {
        assert!(
            !is_unreadable_color_function(value),
            "{value} parses and is not 'unreadable'"
        );
    }
}
