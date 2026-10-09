//! The string a `content` declaration generates on `::before` / `::after`.
//!
//! The value was taken as the text between its first and last quote, as
//! written: `content: "\E721"`, how every icon font names its glyph, became
//! the five characters `\E721` in the fallback font (62.7px wide for a 24px
//! icon), `"x" "y"` became `x" "y`, and `"\2192" / ""` kept its alternative
//! text. Each expected string was checked on the oracle Chromium 143: a
//! `::before` with the value is as wide as a span holding the string, in a
//! monospace font.

use super::*;

fn before_content(value: &str) -> Option<String> {
    let css = format!(".k::before {{ content: {value} }}");
    let sheet = Stylesheet::parse(&css).expect("css");
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut attributes = HashMap::new();
    attributes.insert("class".to_string(), "k".to_string());
    engine
        .pseudo_element_style(
            "i",
            &attributes,
            std::slice::from_ref(&sheet),
            &[],
            &[],
            SiblingContext::SOLE,
            "::before",
            None,
        )
        .and_then(|style| style.content)
}

fn assert_content(value: &str, want: &str) {
    assert_eq!(
        before_content(value).as_deref(),
        Some(want),
        "content: {value}"
    );
}

#[test]
fn a_hex_escape_is_the_character_it_names() {
    assert_content(r#""\E721""#, "\u{E721}");
    assert_content(r#""\f007""#, "\u{F007}");
    assert_content(r#""\201C""#, "\u{201C}");
    assert_content(r#""\41""#, "A");
    assert_content(r#""\41\42""#, "AB");
}

#[test]
fn one_white_space_after_a_hex_escape_ends_it_and_six_digits_do() {
    assert_content(r#""\41 B""#, "AB");
    assert_content(r#""\41  B""#, "A B");
    assert_content(r#""\000041B""#, "AB");
}

#[test]
fn an_escaped_character_is_that_character() {
    assert_content(r#""a\"b""#, "a\"b");
    assert_content(r#""\\""#, "\\");
    assert_content(r"'it\'s'", "it's");
    assert_content(r#""\g""#, "g");
    assert_content("\"a\\\nb\"", "ab");
}

#[test]
fn strings_side_by_side_are_joined_and_alternative_text_is_not_shown() {
    assert_content(r#""x" "y""#, "xy");
    assert_content(r#""\E721" "\E721""#, "\u{E721}\u{E721}");
    assert_content(r#""\2192" / """#, "\u{2192}");
    assert_content(r#""\2192" / "arrow""#, "\u{2192}");
}

/// Pass before the fix.
#[test]
fn a_plain_string_and_the_keywords_are_unchanged() {
    assert_content(r#""plain""#, "plain");
    assert_content("''", "");
    assert_content(r#""""#, "");
    assert_eq!(before_content("none"), None);
    assert_eq!(before_content("normal"), None);
}
