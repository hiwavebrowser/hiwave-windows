//! The text a `content` value generates (CSS Content 3 §2, CSS Syntax §4.3.5
//! and §4.3.7).
//!
//! Icon fonts name their glyph by escape: `content: "\f007"`. The value used
//! to be taken as the text between its first and last quote, so the escape
//! was shown as written, five characters of the fallback font where one icon
//! goes, and `"x" "y"` came out as `x" "y`.

/// The string for a `content` value: its quoted strings, escapes resolved,
/// joined in order. `None` when the value holds no string.
///
/// Components that are not strings (`attr()`, `counter()`, `url()`,
/// `open-quote`) generate nothing here; that is a limit, not the standard.
/// Whatever follows a top-level `/` is alternative text for assistive
/// technology and is not rendered.
pub fn content_string(value: &str) -> Option<String> {
    let mut out: Option<String> = None;
    let mut chars = value.chars().peekable();
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' => {
                let text = string_token(&mut chars, c);
                if depth == 0 {
                    out.get_or_insert_with(String::new).push_str(&text);
                }
            }
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '/' if depth == 0 => break,
            _ => {}
        }
    }
    out
}

/// Consume a string token whose opening quote has been read, up to its
/// closing quote, an unescaped newline or the end of the input.
fn string_token(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, quote: char) -> String {
    let mut text = String::new();
    while let Some(c) = chars.next() {
        if c == quote || c == '\n' {
            break;
        }
        if c != '\\' {
            text.push(c);
            continue;
        }
        match chars.peek().copied() {
            // A backslash at the end of the input stands for nothing.
            None => {}
            // An escaped newline continues the string on the next line.
            Some('\n') => {
                chars.next();
            }
            Some(h) if h.is_ascii_hexdigit() => {
                let mut code = 0u32;
                let mut digits = 0;
                while let Some(d) = chars.peek().and_then(|d| d.to_digit(16)) {
                    if digits == 6 {
                        break;
                    }
                    code = code * 16 + d;
                    digits += 1;
                    chars.next();
                }
                // One white space after a hex escape belongs to the escape.
                if matches!(chars.peek(), Some(' ' | '\t' | '\n')) {
                    chars.next();
                }
                text.push(match char::from_u32(code) {
                    Some(ch) if code != 0 => ch,
                    _ => char::REPLACEMENT_CHARACTER,
                });
            }
            Some(other) => {
                chars.next();
                text.push(other);
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::content_string;

    #[test]
    fn hex_escapes() {
        assert_eq!(content_string(r#""\E721""#).as_deref(), Some("\u{E721}"));
        assert_eq!(content_string(r#""\41 B""#).as_deref(), Some("AB"));
        assert_eq!(content_string(r#""\41  B""#).as_deref(), Some("A B"));
        assert_eq!(content_string(r#""\000041B""#).as_deref(), Some("AB"));
        assert_eq!(content_string(r#""\41\42""#).as_deref(), Some("AB"));
        assert_eq!(content_string(r#""\1F600""#).as_deref(), Some("\u{1F600}"));
    }

    #[test]
    fn a_code_point_that_is_not_a_character_is_the_replacement_character() {
        assert_eq!(content_string(r#""\0""#).as_deref(), Some("\u{FFFD}"));
        assert_eq!(content_string(r#""\110000""#).as_deref(), Some("\u{FFFD}"));
        assert_eq!(content_string(r#""\D800""#).as_deref(), Some("\u{FFFD}"));
    }

    #[test]
    fn escaped_characters_and_line_continuation() {
        assert_eq!(content_string(r#""a\"b""#).as_deref(), Some("a\"b"));
        assert_eq!(content_string(r#""\\""#).as_deref(), Some("\\"));
        assert_eq!(content_string(r"'it\'s'").as_deref(), Some("it's"));
        assert_eq!(content_string(r#""\g""#).as_deref(), Some("g"));
        assert_eq!(content_string("\"a\\\nb\"").as_deref(), Some("ab"));
        assert_eq!(content_string("\"a\\").as_deref(), Some("a"));
    }

    #[test]
    fn several_components() {
        assert_eq!(content_string(r#""x" "y""#).as_deref(), Some("xy"));
        assert_eq!(
            content_string(r#""\2192" / "arrow""#).as_deref(),
            Some("\u{2192}")
        );
        assert_eq!(content_string(r#""a/b""#).as_deref(), Some("a/b"));
        assert_eq!(
            content_string(r#""a" attr(data-x) "b""#).as_deref(),
            Some("ab")
        );
        assert_eq!(
            content_string(r#"url("a/b.png") "x""#).as_deref(),
            Some("x")
        );
        assert_eq!(content_string(r#""""#).as_deref(), Some(""));
        assert_eq!(content_string("''").as_deref(), Some(""));
    }

    #[test]
    fn no_string_is_no_content() {
        assert_eq!(content_string("attr(data-x)"), None);
        assert_eq!(content_string("counter(item)"), None);
        assert_eq!(content_string(r#"url("a.png")"#), None);
        assert_eq!(content_string("open-quote"), None);
    }
}
