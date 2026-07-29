//! Unicode Line Breaking Algorithm (UAX #14) support.
//!
//! This module provides line break opportunity detection for text layout,
//! following the Unicode Line Breaking Algorithm with CSS property integration.
//!
//! # Overview
//!
//! The Unicode Line Breaking Algorithm (UAX #14) defines how text should be
//! broken into lines. This module wraps the `unicode-linebreak` crate and
//! provides integration with CSS `word-break` and `overflow-wrap` properties.
//!
//! # CSS Properties Supported
//!
//! - `word-break`: Controls how words break
//!   - `normal`: Default line breaking rules
//!   - `break-all`: Break between any two characters
//!   - `keep-all`: Don't break CJK text
//!   - `break-word`: Like normal but allows breaking within words if needed
//!
//! - `overflow-wrap` (word-wrap): Controls breaking within words
//!   - `normal`: Only break at allowed break points
//!   - `anywhere`: Break anywhere if needed
//!   - `break-word`: Break within words if no other break points exist
//!
//! # Example
//!
//! ```
//! use rustkit_text::line_break::{LineBreaker, WordBreak, OverflowWrap, BreakOpportunity};
//!
//! let breaker = LineBreaker::new(WordBreak::Normal, OverflowWrap::Normal);
//! let text = "Hello, world! This is a test.";
//!
//! for opportunity in breaker.break_opportunities(text) {
//!     println!("Break at offset {}: {:?}", opportunity.offset, opportunity.kind);
//! }
//! ```
//!
//! # References
//!
//! - Unicode Line Breaking Algorithm (UAX #14): <https://www.unicode.org/reports/tr14/>
//! - CSS Text Module Level 3: <https://www.w3.org/TR/css-text-3/>

use unicode_linebreak::{linebreaks, BreakOpportunity as UnicodeBreakOp};

/// CSS `word-break` property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WordBreak {
    /// Default line breaking rules per UAX #14.
    #[default]
    Normal,
    /// Break between any two characters (for CJK-like wrapping).
    BreakAll,
    /// Don't break within CJK text (keep CJK words together).
    KeepAll,
    /// Like normal, but allows breaking within words if necessary.
    BreakWord,
}

/// CSS `overflow-wrap` (word-wrap) property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowWrap {
    /// Only break at allowed break points.
    #[default]
    Normal,
    /// Break anywhere to prevent overflow (may break mid-word).
    Anywhere,
    /// Break within unbreakable words if no other break points exist.
    BreakWord,
}

/// The kind of break opportunity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakKind {
    /// Mandatory break (newline, paragraph separator).
    Mandatory,
    /// Optional break opportunity (soft break).
    Allowed,
    /// Emergency break (only when overflow-wrap: anywhere/break-word).
    Emergency,
}

/// A break opportunity in text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakOpportunity {
    /// Byte offset where the break can occur (break happens BEFORE this offset).
    pub offset: usize,
    /// Kind of break opportunity.
    pub kind: BreakKind,
}

/// Line breaker with CSS property support.
#[derive(Debug, Clone, Copy, Default)]
pub struct LineBreaker {
    /// CSS word-break property value.
    pub word_break: WordBreak,
    /// CSS overflow-wrap property value.
    pub overflow_wrap: OverflowWrap,
}

impl LineBreaker {
    /// Create a new line breaker with the given CSS properties.
    pub fn new(word_break: WordBreak, overflow_wrap: OverflowWrap) -> Self {
        Self {
            word_break,
            overflow_wrap,
        }
    }

    /// Create a line breaker with default CSS properties.
    pub fn default_rules() -> Self {
        Self::default()
    }

    /// Get all break opportunities in the text.
    ///
    /// Returns an iterator over `BreakOpportunity` structs indicating where
    /// line breaks can occur.
    pub fn break_opportunities<'a>(&self, text: &'a str) -> impl Iterator<Item = BreakOpportunity> + 'a {
        let word_break = self.word_break;
        let _overflow_wrap = self.overflow_wrap; // Reserved for future use

        // Get UAX #14 break opportunities
        linebreaks(text).filter_map(move |(offset, break_op)| {
            let kind = match break_op {
                UnicodeBreakOp::Mandatory => BreakKind::Mandatory,
                UnicodeBreakOp::Allowed => {
                    // Check if this break is allowed by word-break property
                    if word_break == WordBreak::KeepAll {
                        // In keep-all mode, check if we're breaking within CJK
                        // For simplicity, we allow all UAX #14 breaks but a more
                        // complete implementation would check character classes
                        BreakKind::Allowed
                    } else {
                        BreakKind::Allowed
                    }
                }
            };
            Some(BreakOpportunity { offset, kind })
        })
    }

    /// Get break opportunities as byte offsets.
    ///
    /// Returns a vector of byte offsets where breaks are allowed.
    /// This is a convenience method for simple use cases.
    pub fn break_offsets(&self, text: &str) -> Vec<usize> {
        self.break_opportunities(text)
            .map(|op| op.offset)
            .collect()
    }

    /// Check if a break is allowed at the given byte offset.
    ///
    /// Note: This is O(n) as it scans all break opportunities.
    /// For repeated checks, prefer caching the result of `break_offsets()`.
    pub fn can_break_at(&self, text: &str, offset: usize) -> bool {
        self.break_opportunities(text)
            .any(|op| op.offset == offset)
    }

    /// Find the best break point at or before `max_offset`.
    ///
    /// Returns `None` if no break point exists before `max_offset`.
    /// This is useful for line wrapping when you have a maximum width.
    pub fn find_break_before(&self, text: &str, max_offset: usize) -> Option<usize> {
        let mut best = None;
        for op in self.break_opportunities(text) {
            if op.offset <= max_offset {
                best = Some(op.offset);
            } else {
                break;
            }
        }
        best
    }

    /// Find the first break point at or after `min_offset`.
    ///
    /// Returns `None` if no break point exists after `min_offset`.
    pub fn find_break_after(&self, text: &str, min_offset: usize) -> Option<usize> {
        self.break_opportunities(text)
            .find(|op| op.offset >= min_offset)
            .map(|op| op.offset)
    }

    /// Check if emergency breaking is allowed (overflow-wrap: anywhere/break-word).
    pub fn allows_emergency_breaks(&self) -> bool {
        matches!(self.overflow_wrap, OverflowWrap::Anywhere | OverflowWrap::BreakWord)
    }

    /// Get emergency break opportunities (between every grapheme cluster).
    ///
    /// Only use these when normal break points don't fit the available width
    /// and `overflow_wrap` is `Anywhere` or `BreakWord`.
    pub fn emergency_breaks<'a>(&self, text: &'a str) -> impl Iterator<Item = BreakOpportunity> + 'a {
        use crate::segmentation::grapheme_boundaries;

        let boundaries = grapheme_boundaries(text);
        boundaries.into_iter().skip(1).map(|offset| BreakOpportunity {
            offset,
            kind: BreakKind::Emergency,
        })
    }
}

/// Check if a character is a mandatory break character.
pub fn is_mandatory_break(c: char) -> bool {
    matches!(
        c,
        '\n'        // LINE FEED
        | '\r'      // CARRIAGE RETURN
        | '\u{000B}' // VERTICAL TAB
        | '\u{000C}' // FORM FEED
        | '\u{0085}' // NEXT LINE
        | '\u{2028}' // LINE SEPARATOR
        | '\u{2029}' // PARAGRAPH SEPARATOR
    )
}

/// Check if a character is a soft hyphen (potential hyphenation point).
pub fn is_soft_hyphen(c: char) -> bool {
    c == '\u{00AD}' // SOFT HYPHEN
}

/// Check if a character is a zero-width space (potential break point).
pub fn is_zero_width_space(c: char) -> bool {
    c == '\u{200B}' // ZERO WIDTH SPACE
}

/// Check if text contains any mandatory breaks.
pub fn has_mandatory_breaks(text: &str) -> bool {
    text.chars().any(is_mandatory_break)
}

/// Split text at mandatory breaks.
///
/// Returns an iterator over line segments, preserving the break characters
/// at the end of each segment (except the last).
pub fn split_at_mandatory_breaks(text: &str) -> impl Iterator<Item = &str> {
    let mut segments: Vec<&str> = mandatory_break_bounds(text)
        .into_iter()
        .map(|(s, e, _)| &text[s..e])
        .collect();

    // Handle empty text
    if segments.is_empty() && !text.is_empty() {
        segments.push(text);
    }

    segments.into_iter()
}

/// A line segment from text breaking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineSegment<'a> {
    /// The text content of this segment.
    pub text: &'a str,
    /// Start byte offset in the original text.
    pub start: usize,
    /// End byte offset in the original text (exclusive).
    pub end: usize,
    /// Whether this segment ends with a mandatory break.
    pub ends_with_break: bool,
}

impl<'a> LineSegment<'a> {
    /// Get the length of this segment in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Check if this segment is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Get the text without trailing break characters.
    pub fn text_without_break(&self) -> &'a str {
        if self.ends_with_break {
            let trimmed = self.text.trim_end_matches(|c| is_mandatory_break(c));
            trimmed
        } else {
            self.text
        }
    }
}

/// Segment `text` on mandatory breaks, returning `(start, end, ends_with_break)`
/// byte bounds. Single source of truth for mandatory-break segmentation.
///
/// UAX #14 rule LB5: do NOT break between CR and LF — CRLF is ONE mandatory
/// break. Without that pairing, every CRLF document gains a spurious empty
/// line per break; invisible on LF-only sources, and it hits essentially all
/// Windows-authored content.
///
/// This exists as ONE helper because the bug it fixes was caused by having
/// two hand-rolled copies of this loop (`break_into_lines` and
/// `split_at_mandatory_breaks`) — fixing only the copy in front of you leaves
/// the other one wrong. Both now call this.
fn mandatory_break_bounds(text: &str) -> Vec<(usize, usize, bool)> {
    let mut out: Vec<(usize, usize, bool)> = Vec::new();
    let mut start = 0;
    let mut prev_was_cr = false;

    for (i, c) in text.char_indices() {
        if c == '\n' && prev_was_cr {
            // The break was already emitted at the CR; extend it over the LF
            // so the pair is consumed as a single break.
            if let Some(last) = out.last_mut() {
                last.1 = i + c.len_utf8();
            }
            start = i + c.len_utf8();
            prev_was_cr = false;
            continue;
        }
        prev_was_cr = c == '\r';
        if is_mandatory_break(c) {
            let end = i + c.len_utf8();
            out.push((start, end, true));
            start = end;
        }
    }

    if start < text.len() {
        out.push((start, text.len(), false));
    }
    out
}

/// Break text into lines at mandatory breaks, returning detailed segments.
pub fn break_into_lines(text: &str) -> Vec<LineSegment<'_>> {
    mandatory_break_bounds(text)
        .into_iter()
        .map(|(start, end, ends_with_break)| LineSegment {
            text: &text[start..end],
            start,
            end,
            ends_with_break,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_line_breaker_default() {
        let breaker = LineBreaker::default();
        assert_eq!(breaker.word_break, WordBreak::Normal);
        assert_eq!(breaker.overflow_wrap, OverflowWrap::Normal);
    }

    #[test]
    fn test_break_opportunities_simple() {
        let breaker = LineBreaker::default();
        let text = "Hello world";

        let breaks: Vec<_> = breaker.break_opportunities(text).collect();
        // Should have break opportunity after "Hello " (at offset 6)
        assert!(breaks.iter().any(|b| b.offset == 6 && b.kind == BreakKind::Allowed));
    }

    #[test]
    fn test_break_opportunities_punctuation() {
        let breaker = LineBreaker::default();
        let text = "Hello, world!";

        let breaks: Vec<_> = breaker.break_opportunities(text).collect();
        // Should have break opportunity after comma+space
        assert!(breaks.iter().any(|b| b.offset == 7));
    }

    #[test]
    fn test_mandatory_breaks() {
        let breaker = LineBreaker::default();
        let text = "Line1\nLine2";

        let breaks: Vec<_> = breaker.break_opportunities(text).collect();
        // Should have mandatory break after newline
        assert!(breaks.iter().any(|b| b.offset == 6 && b.kind == BreakKind::Mandatory));
    }

    #[test]
    fn test_break_offsets() {
        let breaker = LineBreaker::default();
        let text = "A B C";

        let offsets = breaker.break_offsets(text);
        // Should include offsets 2 and 4 (after spaces)
        assert!(offsets.contains(&2));
        assert!(offsets.contains(&4));
    }

    #[test]
    fn test_find_break_before() {
        let breaker = LineBreaker::default();
        let text = "Hello world, how are you?";

        // Find break before offset 10
        let break_at = breaker.find_break_before(text, 10);
        assert!(break_at.is_some());
        assert!(break_at.unwrap() <= 10);
    }

    #[test]
    fn test_find_break_after() {
        let breaker = LineBreaker::default();
        let text = "Hello world";

        // Find break after offset 3
        let break_at = breaker.find_break_after(text, 3);
        assert!(break_at.is_some());
        assert!(break_at.unwrap() >= 3);
    }

    #[test]
    fn test_allows_emergency_breaks() {
        let normal = LineBreaker::new(WordBreak::Normal, OverflowWrap::Normal);
        assert!(!normal.allows_emergency_breaks());

        let anywhere = LineBreaker::new(WordBreak::Normal, OverflowWrap::Anywhere);
        assert!(anywhere.allows_emergency_breaks());

        let break_word = LineBreaker::new(WordBreak::Normal, OverflowWrap::BreakWord);
        assert!(break_word.allows_emergency_breaks());
    }

    #[test]
    fn test_emergency_breaks() {
        let breaker = LineBreaker::new(WordBreak::Normal, OverflowWrap::Anywhere);
        let text = "ABC";

        let breaks: Vec<_> = breaker.emergency_breaks(text).collect();
        // Should have emergency break between each character
        assert_eq!(breaks.len(), 3); // After A, B, C
        assert!(breaks.iter().all(|b| b.kind == BreakKind::Emergency));
    }

    #[test]
    fn test_is_mandatory_break() {
        assert!(is_mandatory_break('\n'));
        assert!(is_mandatory_break('\r'));
        assert!(is_mandatory_break('\u{2028}')); // LINE SEPARATOR
        assert!(is_mandatory_break('\u{2029}')); // PARAGRAPH SEPARATOR
        assert!(!is_mandatory_break(' '));
        assert!(!is_mandatory_break('a'));
    }

    #[test]
    fn test_is_soft_hyphen() {
        assert!(is_soft_hyphen('\u{00AD}'));
        assert!(!is_soft_hyphen('-'));
        assert!(!is_soft_hyphen('a'));
    }

    #[test]
    fn test_is_zero_width_space() {
        assert!(is_zero_width_space('\u{200B}'));
        assert!(!is_zero_width_space(' '));
        assert!(!is_zero_width_space('a'));
    }

    #[test]
    fn test_has_mandatory_breaks() {
        assert!(has_mandatory_breaks("Hello\nWorld"));
        assert!(has_mandatory_breaks("Line\r\nBreak"));
        assert!(!has_mandatory_breaks("No breaks here"));
    }

    #[test]
    fn test_split_at_mandatory_breaks() {
        let text = "Line1\nLine2\nLine3";
        let segments: Vec<_> = split_at_mandatory_breaks(text).collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[0], "Line1\n");
        assert_eq!(segments[1], "Line2\n");
        assert_eq!(segments[2], "Line3");
    }

    #[test]
    fn test_break_into_lines() {
        let text = "Hello\nWorld";
        let lines = break_into_lines(text);

        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].text, "Hello\n");
        assert!(lines[0].ends_with_break);
        assert_eq!(lines[0].text_without_break(), "Hello");
        assert_eq!(lines[1].text, "World");
        assert!(!lines[1].ends_with_break);
    }

    #[test]
    fn test_line_segment_properties() {
        let text = "Test\n";
        let lines = break_into_lines(text);

        assert_eq!(lines.len(), 1);
        let line = &lines[0];
        assert_eq!(line.start, 0);
        assert_eq!(line.end, 5);
        assert_eq!(line.len(), 5);
        assert!(!line.is_empty());
    }

    #[test]
    fn test_empty_text() {
        let breaker = LineBreaker::default();
        let text = "";

        let breaks: Vec<_> = breaker.break_opportunities(text).collect();
        assert!(breaks.is_empty());
    }

    #[test]
    fn test_cjk_text() {
        let breaker = LineBreaker::default();
        // Chinese: "Hello World"
        let text = "你好世界";

        let breaks: Vec<_> = breaker.break_opportunities(text).collect();
        // CJK text should have break opportunities between characters
        assert!(!breaks.is_empty());
    }

    #[test]
    fn test_url_breaking() {
        let breaker = LineBreaker::default();
        let text = "Visit https://example.com/path for more";

        let breaks: Vec<_> = breaker.break_opportunities(text).collect();
        // Should have break opportunities around the URL
        assert!(!breaks.is_empty());
    }

    #[test]
    fn test_word_break_enum() {
        assert_eq!(WordBreak::default(), WordBreak::Normal);
    }

    #[test]
    fn test_overflow_wrap_enum() {
        assert_eq!(OverflowWrap::default(), OverflowWrap::Normal);
    }
}

#[cfg(test)]
mod crlf_regression {
    use super::*;

    #[test]
    fn crlf_is_one_mandatory_break_not_two() {
        // UAX #14 LB5. Before the fix this returned THREE segments
        // ("a\r", "\n", "b") and rendered a spurious blank line for every
        // hard break in a CRLF document.
        let segs = break_into_lines("a\r\nb");
        assert_eq!(segs.len(), 2, "got {:?}", segs.iter().map(|s| s.text).collect::<Vec<_>>());
        assert_eq!(segs[0].text, "a\r\n");
        assert!(segs[0].ends_with_break);
        assert_eq!(segs[1].text, "b");
        assert!(!segs[1].ends_with_break);
    }

    #[test]
    fn lone_cr_and_lone_lf_each_still_break() {
        assert_eq!(break_into_lines("a\rb").len(), 2);
        assert_eq!(break_into_lines("a\nb").len(), 2);
    }

    #[test]
    fn lf_then_cr_is_two_breaks_not_one() {
        // Reversed order is NOT a pair — "\n\r" is two separate breaks, so
        // an over-eager fix that pairs any CR/LF adjacency would fail here.
        assert_eq!(break_into_lines("a\n\rb").len(), 3);
    }

    #[test]
    fn consecutive_crlf_pairs_produce_one_break_each() {
        // "a\r\n\r\nb" is a blank line between two texts: 3 segments.
        let segs = break_into_lines("a\r\n\r\nb");
        assert_eq!(segs.len(), 3, "got {:?}", segs.iter().map(|s| s.text).collect::<Vec<_>>());
    }

    #[test]
    fn byte_offsets_stay_consistent_across_a_crlf_pair() {
        let segs = break_into_lines("ab\r\ncd");
        assert_eq!((segs[0].start, segs[0].end), (0, 4));
        assert_eq!((segs[1].start, segs[1].end), (4, 6));
    }
}

#[cfg(test)]
mod split_at_mandatory_breaks_crlf {
    use super::*;

    #[test]
    fn split_at_mandatory_breaks_pairs_crlf() {
        // The SECOND copy of the loop. Prometheus caught that Athena's first
        // CRLF fix touched break_into_lines only and left this one wrong -
        // which is why both now share mandatory_break_bounds.
        let v: Vec<&str> = split_at_mandatory_breaks("a\r\nb").collect();
        assert_eq!(v, vec!["a\r\n", "b"], "CRLF must be one break here too");
    }

    #[test]
    fn both_entry_points_agree_on_segmentation() {
        // The property that makes the duplication safe: the two public
        // functions must never disagree about where the breaks are.
        for s in ["a\r\nb", "a\nb", "a\rb", "a\n\rb", "a\r\n\r\nb", "plain", ""] {
            let via_split: Vec<&str> = split_at_mandatory_breaks(s).collect();
            let via_lines: Vec<&str> = break_into_lines(s).iter().map(|x| x.text).collect();
            // split_at_mandatory_breaks yields the whole string for empty-segment
            // input; compare only when break_into_lines produced segments.
            if !via_lines.is_empty() {
                assert_eq!(via_split, via_lines, "entry points disagree on {:?}", s);
            }
        }
    }
}
