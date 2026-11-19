//! HTML character references in data, RCDATA, and attribute values.

mod named;

use std::collections::HashMap;

lazy_static::lazy_static! {
    static ref NAMES: HashMap<&'static [u8], &'static str> =
        named::NAMES.iter().copied().collect();
}

/// Decode references in a text or RCDATA string.
pub fn decode(mut input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    while let Some((literal, rest)) = input.split_once('&') {
        result.push_str(literal);
        let mut numeric = [0; 4];
        let (consumed, value) = reference(rest.chars(), false, &mut numeric);
        result.push_str(value);
        input = &rest[consumed..];
    }
    result.push_str(input);
    result
}

/// Input starts just after '&'. Only ASCII characters can be consumed, so
/// the count is valid for both a UTF-8 string and the tokenizer's char slice.
pub(super) fn reference(
    input: impl Iterator<Item = char>,
    in_attribute: bool,
    numeric: &mut [u8; 4],
) -> (usize, &str) {
    let mut input = input.peekable();
    if input.next_if_eq(&'#').is_some() {
        let hex = input.next_if(|c| matches!(c, 'x' | 'X')).is_some();
        let radix = if hex { 16 } else { 10 };
        let mut digits = 0;
        let mut code_point = 0u32;
        while let Some(digit) = input.peek().and_then(|c| c.to_digit(radix)) {
            input.next();
            digits += 1;
            code_point = code_point.saturating_mul(radix).saturating_add(digit);
        }
        if digits == 0 {
            return (0, "&");
        }
        let consumed =
            1 + usize::from(hex) + digits + usize::from(input.next_if_eq(&';').is_some());
        return (consumed, numeric_character(code_point).encode_utf8(numeric));
    }

    // Try the whole terminated name first. Only the legacy spellings can
    // match a shorter prefix, and none requires unbounded lookahead.
    let mut name = [0; named::MAX_NAME];
    let mut len = 0;
    while len < name.len() - 1 {
        let Some(ch) = input.next_if(char::is_ascii_alphanumeric) else {
            break;
        };
        name[len] = ch as u8;
        len += 1;
    }
    if len == 0 {
        return (0, "&");
    }
    if input.peek() == Some(&';') {
        name[len] = b';';
        if let Some(&value) = NAMES.get(&name[..=len]) {
            return (len + 1, value);
        }
    }
    for matched in (1..=len.min(named::MAX_LEGACY)).rev() {
        if let Some(&value) = NAMES.get(&name[..matched]) {
            if in_attribute && (matched < len || input.peek() == Some(&'=')) {
                return (0, "&");
            }
            return (matched, value);
        }
    }
    (0, "&")
}

fn numeric_character(code_point: u32) -> char {
    match code_point {
        0 => '\u{fffd}',
        0x80 => '€',
        0x82 => '‚',
        0x83 => 'ƒ',
        0x84 => '„',
        0x85 => '…',
        0x86 => '†',
        0x87 => '‡',
        0x88 => 'ˆ',
        0x89 => '‰',
        0x8A => 'Š',
        0x8B => '‹',
        0x8C => 'Œ',
        0x8E => 'Ž',
        0x91 => '\u{2018}', // '
        0x92 => '\u{2019}', // '
        0x93 => '\u{201C}', // "
        0x94 => '\u{201D}', // "
        0x95 => '•',
        0x96 => '–',
        0x97 => '—',
        0x98 => '˜',
        0x99 => '™',
        0x9A => 'š',
        0x9B => '›',
        0x9C => 'œ',
        0x9E => 'ž',
        0x9F => 'Ÿ',
        _ => char::from_u32(code_point).unwrap_or('\u{fffd}'),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_whatwg_spelling_decodes_and_consumes_exactly_the_match() {
        assert_eq!(named::NAMES.len(), 2231);
        for &(name, expected) in named::NAMES {
            let name = std::str::from_utf8(name).unwrap();
            let mut numeric = [0; 4];
            for in_attribute in [false, true] {
                assert_eq!(
                    reference(name.chars(), in_attribute, &mut numeric),
                    (name.len(), expected),
                    "{name}"
                );
            }
            if !name.ends_with(';') {
                let followed = format!("{name}0");
                assert_eq!(
                    reference(followed.chars(), false, &mut numeric),
                    (name.len(), expected),
                    "{followed}"
                );
                assert_eq!(
                    reference(followed.chars(), true, &mut numeric),
                    (0, "&"),
                    "{followed}"
                );
            }
        }
    }

    #[test]
    fn test_decode_basic_entities() {
        assert_eq!(decode("&lt;"), "<");
        assert_eq!(decode("&gt;"), ">");
        assert_eq!(decode("&amp;"), "&");
        assert_eq!(decode("&quot;"), "\"");
        assert_eq!(decode("&apos;"), "'");
    }

    #[test]
    fn test_decode_numeric_entities() {
        assert_eq!(decode("&#65;"), "A");
        assert_eq!(decode("&#x41;"), "A");
        assert_eq!(decode("&#x1F600;"), "😀");
    }

    #[test]
    fn test_decode_multiple_entities() {
        assert_eq!(decode("&lt;div&gt;"), "<div>");
        assert_eq!(decode("Tom &amp; Jerry"), "Tom & Jerry");
    }

    #[test]
    fn test_decode_unknown_entity() {
        assert_eq!(decode("&unknown;"), "&unknown;");
    }

    #[test]
    fn test_decode_no_entities() {
        assert_eq!(decode("hello world"), "hello world");
    }

    #[test]
    fn test_decode_incomplete_entity() {
        assert_eq!(decode("&lt"), "<");
        assert_eq!(decode("&l"), "&l");
        assert_eq!(decode("&"), "&");
    }

    #[test]
    fn test_decode_nbsp() {
        assert_eq!(decode("&nbsp;"), "\u{00A0}");
    }

    #[test]
    fn test_decode_copyright() {
        assert_eq!(decode("&copy;"), "©");
    }

    #[test]
    fn test_decode_greek() {
        assert_eq!(decode("&alpha;"), "α");
        assert_eq!(decode("&Beta;"), "Β");
        assert_eq!(decode("&pi;"), "π");
        assert_eq!(decode("&Omega;"), "Ω");
    }

    #[test]
    fn test_decode_arrows() {
        assert_eq!(decode("&larr;"), "←");
        assert_eq!(decode("&rarr;"), "→");
        assert_eq!(decode("&harr;"), "↔");
    }

    #[test]
    fn test_decode_math() {
        assert_eq!(decode("&times;"), "×");
        assert_eq!(decode("&divide;"), "÷");
        assert_eq!(decode("&infin;"), "∞");
        assert_eq!(decode("&ne;"), "≠");
    }

    #[test]
    fn test_decode_accented() {
        assert_eq!(decode("&eacute;"), "é");
        assert_eq!(decode("&Ntilde;"), "Ñ");
        assert_eq!(decode("&uuml;"), "ü");
        assert_eq!(decode("&ccedil;"), "ç");
    }

    #[test]
    fn test_decode_quotes() {
        assert_eq!(decode("&ldquo;"), "\u{201C}"); // "
        assert_eq!(decode("&rdquo;"), "\u{201D}"); // "
        assert_eq!(decode("&lsquo;"), "\u{2018}"); // '
        assert_eq!(decode("&rsquo;"), "\u{2019}"); // '
    }

    #[test]
    fn test_windows_1252_mapping() {
        // €
        assert_eq!(decode("&#128;"), "€");
        // Smart quotes
        assert_eq!(decode("&#147;"), "\u{201C}"); // "
        assert_eq!(decode("&#148;"), "\u{201D}"); // "
                                                  // Em dash
        assert_eq!(decode("&#151;"), "—");
    }

    #[test]
    fn test_null_replacement() {
        assert_eq!(decode("&#0;"), "\u{FFFD}");
    }

    #[test]
    fn test_surrogate_replacement() {
        // Surrogate pairs should be replaced
        assert_eq!(decode("&#xD800;"), "\u{FFFD}");
        assert_eq!(decode("&#xDFFF;"), "\u{FFFD}");
    }
}
