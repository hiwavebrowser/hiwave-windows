//! CSS escapes in selectors (CSS Syntax §4.3.7).
//!
//! Tailwind-style class names are full of escapes: `.sm\:flex`, `.w-1\/2`,
//! `.\!mt-0`, `.bg-\[url\(\'x\'\)\]`. The engine's selector matchers split a
//! compound on `.`, `#`, `:` and `[` without knowing about escapes, so
//! `.sm\:flex` read as class `sm\` plus an unknown pseudo-class `:flex`, and
//! the selector list was dropped as invalid. On x, yahoo and weather that was
//! about two thirds of all selectors.
//!
//! [`encode_selector_escapes`] runs once per rule at parse time: every escape
//! becomes the code point it stands for, except that an escaped ASCII
//! character with selector meaning becomes a private-use stand-in the
//! matchers pass over as ordinary name text. [`css_ident`] turns a name
//! extracted by a matcher back into the literal text an element's `class` /
//! `id` attribute holds.

use std::borrow::Cow;

/// Stand-ins for escaped ASCII live at U+F0000 + the ASCII value
/// (Supplementary Private Use Area-A, which no real class name uses).
const STAND_IN_BASE: u32 = 0xF0000;

fn is_name_char(c: char) -> bool {
    !c.is_ascii() || c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

/// Resolve the escapes in a selector. Quoted strings (attribute values) are
/// copied unchanged.
pub fn encode_selector_escapes(selector: &str) -> Cow<'_, str> {
    if !selector.contains('\\') {
        return Cow::Borrowed(selector);
    }
    let mut out = String::with_capacity(selector.len());
    let mut chars = selector.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        if let Some(q) = quote {
            out.push(c);
            if c == '\\' {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            } else if c == q {
                quote = None;
            }
            continue;
        }
        if c == '"' || c == '\'' {
            quote = Some(c);
            out.push(c);
            continue;
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let code_point = match chars.peek().copied() {
            // Not a valid escape; leave it for the matcher to reject.
            None | Some('\n') | Some('\r') | Some('\x0c') => {
                out.push(c);
                continue;
            }
            Some(h) if h.is_ascii_hexdigit() => {
                let mut value = 0u32;
                let mut digits = 0;
                while digits < 6 {
                    match chars.peek().and_then(|d| d.to_digit(16)) {
                        Some(d) => {
                            value = value * 16 + d;
                            chars.next();
                            digits += 1;
                        }
                        None => break,
                    }
                }
                // One whitespace after a hex escape belongs to the escape.
                if matches!(chars.peek(), Some(' ' | '\t' | '\n')) {
                    chars.next();
                }
                match char::from_u32(value) {
                    Some(ch) if value != 0 => ch,
                    _ => '\u{FFFD}',
                }
            }
            Some(n) => {
                chars.next();
                n
            }
        };
        if is_name_char(code_point) {
            out.push(code_point);
        } else {
            // ASCII (non-name chars are all ASCII here): safe to offset.
            out.push(char::from_u32(STAND_IN_BASE + code_point as u32).unwrap_or('\u{FFFD}'));
        }
    }
    Cow::Owned(out)
}

/// A class, id or tag name taken from an encoded selector, as the literal
/// text it matches.
pub fn css_ident(name: &str) -> Cow<'_, str> {
    let is_stand_in = |c: char| (STAND_IN_BASE..STAND_IN_BASE + 0x80).contains(&(c as u32));
    if !name.chars().any(is_stand_in) {
        return Cow::Borrowed(name);
    }
    Cow::Owned(
        name.chars()
            .map(|c| {
                if is_stand_in(c) {
                    char::from_u32(c as u32 - STAND_IN_BASE).unwrap_or(c)
                } else {
                    c
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(selector: &str) -> String {
        css_ident(&encode_selector_escapes(selector)).into_owned()
    }

    #[test]
    fn escaped_syntax_characters_become_name_text() {
        let enc = encode_selector_escapes(".sm\\:flex:hover");
        assert_eq!(enc.matches(':').count(), 1, "only the real pseudo-class colon stays");
        assert_eq!(round_trip(".sm\\:flex"), ".sm:flex");
        assert_eq!(round_trip(".w-1\\/2"), ".w-1/2");
        assert_eq!(round_trip(".\\!mt-0"), ".!mt-0");
        assert_eq!(round_trip(".w-0\\.5"), ".w-0.5");
        assert!(!encode_selector_escapes(".w-0\\.5").contains("0.5"));
    }

    #[test]
    fn hex_escapes_and_their_trailing_space() {
        assert_eq!(round_trip(".\\31 0"), ".10");
        assert_eq!(round_trip(".a\\3A b"), ".a:b");
        assert_eq!(round_trip(".\\0 x"), ".\u{FFFD}x");
    }

    #[test]
    fn plain_selectors_and_quoted_strings_are_untouched() {
        assert!(matches!(encode_selector_escapes(".a .b > c"), Cow::Borrowed(_)));
        assert_eq!(encode_selector_escapes("[title=\"a\\:b\"]"), "[title=\"a\\:b\"]");
    }
}
