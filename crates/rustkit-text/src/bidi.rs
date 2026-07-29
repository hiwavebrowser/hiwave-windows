//! Unicode Bidirectional Algorithm (UAX #9) support.
//!
//! This module provides text direction analysis for mixed LTR/RTL text rendering.
//! The implementation wraps the `unicode-bidi` crate and provides a simplified
//! interface for the RustKit text pipeline.
//!
//! # Overview
//!
//! The Unicode Bidirectional Algorithm determines the visual ordering of text
//! that mixes left-to-right (LTR) scripts like Latin with right-to-left (RTL)
//! scripts like Arabic or Hebrew.
//!
//! # Example
//!
//! ```
//! use rustkit_text::bidi::{BidiInfo, Direction};
//!
//! // English text
//! let info = BidiInfo::new("Hello, world!");
//! assert_eq!(info.base_direction(), Direction::Ltr);
//!
//! // Mixed text (English + Hebrew)
//! let info = BidiInfo::new("Hello \u{05E9}\u{05DC}\u{05D5}\u{05DD} world!");
//! assert_eq!(info.base_direction(), Direction::Ltr);
//!
//! // Get visual runs
//! for run in info.visual_runs() {
//!     println!("Run: {:?} direction", run.direction);
//! }
//! ```
//!
//! # References
//!
//! - Unicode Bidirectional Algorithm (UAX #9): <https://www.unicode.org/reports/tr9/>

use unicode_bidi::{BidiInfo as UBidiInfo, Level};

/// Text direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    /// Left-to-right (Latin, Greek, Cyrillic, etc.)
    #[default]
    Ltr,
    /// Right-to-left (Arabic, Hebrew, etc.)
    Rtl,
}

impl Direction {
    /// Check if this direction is left-to-right.
    pub fn is_ltr(self) -> bool {
        self == Direction::Ltr
    }

    /// Check if this direction is right-to-left.
    pub fn is_rtl(self) -> bool {
        self == Direction::Rtl
    }

    /// Convert from CSS direction keyword.
    pub fn from_css(direction: &str) -> Self {
        match direction.to_lowercase().as_str() {
            "rtl" => Direction::Rtl,
            _ => Direction::Ltr,
        }
    }

    /// Convert a bidi level to direction (even = LTR, odd = RTL).
    pub fn from_level(level: u8) -> Self {
        if level % 2 == 0 {
            Direction::Ltr
        } else {
            Direction::Rtl
        }
    }
}

/// A visual run of text with a single direction.
#[derive(Debug, Clone)]
pub struct BidiRun {
    /// Start byte index in the source text.
    pub start: usize,
    /// End byte index in the source text (exclusive).
    pub end: usize,
    /// Direction of this run.
    pub direction: Direction,
    /// Bidi embedding level (even = LTR, odd = RTL).
    pub level: u8,
}

impl BidiRun {
    /// Get the text slice for this run from the source text.
    pub fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.end]
    }

    /// Get the length of this run in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Check if this run is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }
}

/// Bidirectional text analysis result.
///
/// This struct holds the result of applying the Unicode Bidirectional Algorithm
/// to a text string. It provides methods to query the base direction and
/// iterate over visual runs.
pub struct BidiInfo {
    /// The analyzed text.
    text: String,
    /// The base paragraph direction.
    base_direction: Direction,
    /// Bidi levels for each character.
    levels: Vec<Level>,
    /// Visual runs (computed lazily).
    runs: Vec<BidiRun>,
}

impl BidiInfo {
    /// Analyze text using the Unicode Bidirectional Algorithm.
    ///
    /// The base direction is automatically detected from the text content.
    pub fn new(text: &str) -> Self {
        Self::with_base_direction(text, None)
    }

    /// Analyze text with an explicit base direction.
    ///
    /// # Arguments
    /// * `text` - The text to analyze
    /// * `base_direction` - Override base direction, or None for auto-detection
    pub fn with_base_direction(text: &str, base_direction: Option<Direction>) -> Self {
        if text.is_empty() {
            return Self {
                text: String::new(),
                base_direction: base_direction.unwrap_or(Direction::Ltr),
                levels: Vec::new(),
                runs: Vec::new(),
            };
        }

        // Determine base level
        let base_level = match base_direction {
            Some(Direction::Ltr) => Some(Level::ltr()),
            Some(Direction::Rtl) => Some(Level::rtl()),
            None => None, // Auto-detect
        };

        // Run the bidi algorithm
        let bidi_info = UBidiInfo::new(text, base_level);

        // Get the paragraph info for the first (and typically only) paragraph
        let para = &bidi_info.paragraphs[0];
        let para_level = para.level;

        // Determine base direction from paragraph level
        let detected_direction = Direction::from_level(para_level.number());

        // Get levels for each character
        let levels: Vec<Level> = bidi_info.levels.clone();

        // Compute visual runs
        let (_reordered_levels, level_runs) = bidi_info.visual_runs(para, para.range.clone());
        let runs: Vec<BidiRun> = level_runs
            .into_iter()
            .map(|range| {
                // Get the level for this run from the first character in the range
                let level = levels
                    .get(range.start)
                    .copied()
                    .unwrap_or(Level::ltr())
                    .number();
                BidiRun {
                    start: range.start,
                    end: range.end,
                    direction: Direction::from_level(level),
                    level,
                }
            })
            .collect();

        Self {
            text: text.to_string(),
            base_direction: detected_direction,
            levels,
            runs,
        }
    }

    /// Get the base paragraph direction.
    pub fn base_direction(&self) -> Direction {
        self.base_direction
    }

    /// Get the analyzed text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Get the bidi level at a byte offset.
    ///
    /// Returns None if the offset is out of bounds.
    pub fn level_at(&self, byte_offset: usize) -> Option<u8> {
        // Find the character index for this byte offset
        let char_index = self.text[..byte_offset.min(self.text.len())]
            .chars()
            .count();
        self.levels.get(char_index).map(|l| l.number())
    }

    /// Get the direction at a byte offset.
    pub fn direction_at(&self, byte_offset: usize) -> Direction {
        self.level_at(byte_offset)
            .map(Direction::from_level)
            .unwrap_or(self.base_direction)
    }

    /// Get the visual runs in display order.
    ///
    /// Each run represents a contiguous sequence of characters with the same
    /// direction. The runs are ordered for visual display (left to right on screen).
    pub fn visual_runs(&self) -> &[BidiRun] {
        &self.runs
    }

    /// Check if the text contains any RTL characters.
    pub fn has_rtl(&self) -> bool {
        self.levels.iter().any(|l| l.is_rtl())
    }

    /// Check if the text is purely LTR (no RTL characters).
    pub fn is_pure_ltr(&self) -> bool {
        !self.has_rtl()
    }

    /// Get the number of visual runs.
    pub fn run_count(&self) -> usize {
        self.runs.len()
    }
}

/// Check if a character is a bidi format control character.
///
/// These are invisible characters used to control bidi algorithm behavior.
pub fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}'  // LEFT-TO-RIGHT MARK
        | '\u{200F}'  // RIGHT-TO-LEFT MARK
        | '\u{202A}'  // LEFT-TO-RIGHT EMBEDDING
        | '\u{202B}'  // RIGHT-TO-LEFT EMBEDDING
        | '\u{202C}'  // POP DIRECTIONAL FORMATTING
        | '\u{202D}'  // LEFT-TO-RIGHT OVERRIDE
        | '\u{202E}'  // RIGHT-TO-LEFT OVERRIDE
        | '\u{2066}'  // LEFT-TO-RIGHT ISOLATE
        | '\u{2067}'  // RIGHT-TO-LEFT ISOLATE
        | '\u{2068}'  // FIRST STRONG ISOLATE
        | '\u{2069}'  // POP DIRECTIONAL ISOLATE
    )
}

/// Strip bidi format control characters from text.
pub fn strip_bidi_controls(text: &str) -> String {
    text.chars().filter(|c| !is_bidi_control(*c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ltr_text() {
        let info = BidiInfo::new("Hello, world!");
        assert_eq!(info.base_direction(), Direction::Ltr);
        assert!(info.is_pure_ltr());
        assert_eq!(info.run_count(), 1);
    }

    #[test]
    fn test_rtl_text() {
        // Hebrew: "shalom"
        let info = BidiInfo::new("\u{05E9}\u{05DC}\u{05D5}\u{05DD}");
        assert_eq!(info.base_direction(), Direction::Rtl);
        assert!(info.has_rtl());
        assert_eq!(info.run_count(), 1);
    }

    #[test]
    fn test_mixed_text() {
        // "Hello שלום world!"
        let info = BidiInfo::new("Hello \u{05E9}\u{05DC}\u{05D5}\u{05DD} world!");
        assert!(info.has_rtl());
        // Should have multiple runs
        assert!(info.run_count() >= 2);
    }

    #[test]
    fn test_empty_text() {
        let info = BidiInfo::new("");
        assert_eq!(info.base_direction(), Direction::Ltr);
        assert_eq!(info.run_count(), 0);
    }

    #[test]
    fn test_explicit_direction() {
        let info = BidiInfo::with_base_direction("Hello", Some(Direction::Rtl));
        assert_eq!(info.base_direction(), Direction::Rtl);
    }

    #[test]
    fn test_direction_from_level() {
        assert_eq!(Direction::from_level(0), Direction::Ltr);
        assert_eq!(Direction::from_level(1), Direction::Rtl);
        assert_eq!(Direction::from_level(2), Direction::Ltr);
    }

    #[test]
    fn test_is_bidi_control() {
        assert!(is_bidi_control('\u{200E}')); // LRM
        assert!(is_bidi_control('\u{200F}')); // RLM
        assert!(!is_bidi_control('A'));
        assert!(!is_bidi_control(' '));
    }

    #[test]
    fn test_strip_bidi_controls() {
        let text = "Hello\u{200E}World\u{200F}!";
        assert_eq!(strip_bidi_controls(text), "HelloWorld!");
    }

    #[test]
    fn test_bidi_run_text() {
        let text = "Hello";
        let info = BidiInfo::new(text);
        let runs = info.visual_runs();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text(text), "Hello");
    }

    #[test]
    fn test_arabic_text() {
        // Arabic: "marhaba" (hello)
        let info = BidiInfo::new("\u{0645}\u{0631}\u{062D}\u{0628}\u{0627}");
        assert_eq!(info.base_direction(), Direction::Rtl);
        assert!(info.has_rtl());
    }
}
