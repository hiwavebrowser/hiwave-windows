//! Text segmentation for grapheme clusters, words, and sentences.
//!
//! This module provides Unicode-compliant text segmentation using the
//! `unicode-segmentation` crate. It supports:
//!
//! - **Grapheme clusters**: User-perceived characters (e.g., emoji sequences)
//! - **Words**: For text selection and word-wrap
//! - **Sentences**: For text selection
//!
//! # Grapheme Clusters
//!
//! A grapheme cluster is what a user perceives as a single character. This can be:
//! - A single Unicode scalar value (like 'A')
//! - Multiple scalars (like 'é' = 'e' + combining acute accent)
//! - Complex emoji sequences (like 👨‍👩‍👧 = family emoji)
//!
//! # Example
//!
//! ```
//! use rustkit_text::segmentation::{grapheme_indices, word_indices, GraphemeSegment};
//!
//! // Grapheme clusters
//! let text = "Hello 👨‍👩‍👧";
//! for segment in grapheme_indices(text) {
//!     println!("Grapheme at {}: '{}'", segment.start, segment.text);
//! }
//!
//! // Word boundaries
//! for segment in word_indices(text) {
//!     println!("Word at {}: '{}'", segment.start, segment.text);
//! }
//! ```
//!
//! # References
//!
//! - Unicode Text Segmentation (UAX #29): <https://www.unicode.org/reports/tr29/>

use unicode_segmentation::UnicodeSegmentation;

/// A segment of text with its position in the source string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSegment<'a> {
    /// The segment text.
    pub text: &'a str,
    /// Start byte offset in the source string.
    pub start: usize,
    /// End byte offset in the source string (exclusive).
    pub end: usize,
}

impl<'a> TextSegment<'a> {
    /// Create a new text segment.
    pub fn new(text: &'a str, start: usize) -> Self {
        Self {
            text,
            start,
            end: start + text.len(),
        }
    }

    /// Get the length of this segment in bytes.
    pub fn len(&self) -> usize {
        self.end - self.start
    }

    /// Check if this segment is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Alias for grapheme segment.
pub type GraphemeSegment<'a> = TextSegment<'a>;

/// Alias for word segment.
pub type WordSegment<'a> = TextSegment<'a>;

/// Iterate over grapheme cluster boundaries with indices.
///
/// This returns an iterator over `TextSegment` for each grapheme cluster
/// in the text, following UAX #29 Extended Grapheme Cluster rules.
///
/// # Example
///
/// ```
/// use rustkit_text::segmentation::grapheme_indices;
///
/// let text = "Héllo";
/// let segments: Vec<_> = grapheme_indices(text).collect();
/// assert_eq!(segments[0].text, "H");
/// assert_eq!(segments[1].text, "é");
/// ```
pub fn grapheme_indices(text: &str) -> impl Iterator<Item = TextSegment<'_>> {
    text.grapheme_indices(true)
        .map(|(idx, g)| TextSegment::new(g, idx))
}

/// Get grapheme cluster boundaries as byte offsets.
///
/// Returns a vector of byte offsets where each grapheme cluster begins,
/// plus the final offset at the end of the string.
///
/// # Example
///
/// ```
/// use rustkit_text::segmentation::grapheme_boundaries;
///
/// let text = "Hi👋";
/// let boundaries = grapheme_boundaries(text);
/// assert_eq!(boundaries, vec![0, 1, 2, 6]); // 'H', 'i', '👋' (4 bytes)
/// ```
pub fn grapheme_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries: Vec<usize> = text.grapheme_indices(true).map(|(idx, _)| idx).collect();
    boundaries.push(text.len());
    boundaries
}

/// Count the number of grapheme clusters in text.
///
/// This is the "user-visible" character count.
pub fn grapheme_count(text: &str) -> usize {
    text.graphemes(true).count()
}

/// Iterate over word boundaries with indices.
///
/// This returns an iterator over `TextSegment` for each word in the text,
/// following UAX #29 Word Boundary rules. Note that spaces and punctuation
/// are included as separate "words".
///
/// # Example
///
/// ```
/// use rustkit_text::segmentation::word_indices;
///
/// let text = "Hello, world!";
/// let words: Vec<_> = word_indices(text).collect();
/// // Includes: "Hello", ",", " ", "world", "!"
/// ```
pub fn word_indices(text: &str) -> impl Iterator<Item = TextSegment<'_>> {
    text.split_word_bound_indices()
        .map(|(idx, w)| TextSegment::new(w, idx))
}

/// Get word boundaries as byte offsets.
///
/// Returns a vector of byte offsets where each word begins,
/// plus the final offset at the end of the string.
pub fn word_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries: Vec<usize> = text.split_word_bound_indices().map(|(idx, _)| idx).collect();
    boundaries.push(text.len());
    boundaries
}

/// Iterate over only "true" words (alphabetic/numeric content).
///
/// This filters out whitespace and punctuation, returning only segments
/// that contain word characters.
pub fn words(text: &str) -> impl Iterator<Item = TextSegment<'_>> {
    text.split_word_bound_indices()
        .filter(|(_, w)| w.chars().any(|c| c.is_alphanumeric()))
        .map(|(idx, w)| TextSegment::new(w, idx))
}

/// Iterate over sentence boundaries with indices.
///
/// This returns an iterator over `TextSegment` for each sentence in the text,
/// following UAX #29 Sentence Boundary rules.
pub fn sentence_indices(text: &str) -> impl Iterator<Item = TextSegment<'_>> {
    text.split_sentence_bound_indices()
        .map(|(idx, s)| TextSegment::new(s, idx))
}

/// Get the grapheme cluster at a byte offset.
///
/// Returns the grapheme cluster that contains the given byte offset,
/// or None if the offset is out of bounds.
pub fn grapheme_at_offset(text: &str, offset: usize) -> Option<TextSegment<'_>> {
    if offset >= text.len() {
        return None;
    }

    grapheme_indices(text).find(|seg| seg.start <= offset && offset < seg.end)
}

/// Find the word at a byte offset.
///
/// Returns the word segment that contains the given byte offset.
pub fn word_at_offset(text: &str, offset: usize) -> Option<TextSegment<'_>> {
    if offset >= text.len() {
        return None;
    }

    word_indices(text).find(|seg| seg.start <= offset && offset < seg.end)
}

/// Find the start of the grapheme cluster at or before an offset.
///
/// This is useful for cursor positioning to ensure we don't split a grapheme.
pub fn floor_grapheme_boundary(text: &str, offset: usize) -> usize {
    if offset == 0 || text.is_empty() {
        return 0;
    }

    let offset = offset.min(text.len());

    grapheme_boundaries(text)
        .iter()
        .copied()
        .take_while(|&b| b <= offset)
        .last()
        .unwrap_or(0)
}

/// Find the end of the grapheme cluster at or after an offset.
///
/// This is useful for cursor positioning to ensure we don't split a grapheme.
pub fn ceil_grapheme_boundary(text: &str, offset: usize) -> usize {
    if text.is_empty() {
        return 0;
    }

    grapheme_boundaries(text)
        .iter()
        .copied()
        .find(|&b| b >= offset)
        .unwrap_or(text.len())
}

/// Check if an offset is a valid grapheme cluster boundary.
pub fn is_grapheme_boundary(text: &str, offset: usize) -> bool {
    if offset == 0 || offset == text.len() {
        return true;
    }

    grapheme_boundaries(text).contains(&offset)
}

/// Split text at grapheme boundaries, respecting a maximum byte length per chunk.
///
/// This is useful for breaking text into chunks without splitting grapheme clusters.
pub fn split_at_grapheme_boundaries(text: &str, max_bytes: usize) -> Vec<&str> {
    if text.is_empty() || max_bytes == 0 {
        return vec![];
    }

    let boundaries = grapheme_boundaries(text);
    let mut chunks = Vec::new();
    let mut chunk_start = 0;

    for &boundary in &boundaries[1..] {
        if boundary - chunk_start > max_bytes && chunk_start < boundary {
            // Find the largest boundary that fits
            let chunk_end = boundaries
                .iter()
                .copied()
                .take_while(|&b| b <= chunk_start + max_bytes)
                .last()
                .unwrap_or(chunk_start);

            if chunk_end > chunk_start {
                chunks.push(&text[chunk_start..chunk_end]);
                chunk_start = chunk_end;
            }
        }
    }

    // Add remaining text
    if chunk_start < text.len() {
        chunks.push(&text[chunk_start..]);
    }

    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grapheme_indices_ascii() {
        let text = "Hello";
        let segments: Vec<_> = grapheme_indices(text).collect();
        assert_eq!(segments.len(), 5);
        assert_eq!(segments[0].text, "H");
        assert_eq!(segments[0].start, 0);
        assert_eq!(segments[4].text, "o");
        assert_eq!(segments[4].start, 4);
    }

    #[test]
    fn test_grapheme_indices_emoji() {
        let text = "Hi👋";
        let segments: Vec<_> = grapheme_indices(text).collect();
        assert_eq!(segments.len(), 3);
        assert_eq!(segments[2].text, "👋");
    }

    #[test]
    fn test_grapheme_count() {
        assert_eq!(grapheme_count("Hello"), 5);
        assert_eq!(grapheme_count("Hi👋"), 3);
        assert_eq!(grapheme_count(""), 0);
    }

    #[test]
    fn test_grapheme_boundaries() {
        let text = "Hi👋";
        let boundaries = grapheme_boundaries(text);
        assert_eq!(boundaries, vec![0, 1, 2, 6]);
    }

    #[test]
    fn test_word_indices() {
        let text = "Hello world";
        let words: Vec<_> = word_indices(text).collect();
        // "Hello", " ", "world"
        assert!(words.len() >= 3);
        assert_eq!(words[0].text, "Hello");
    }

    #[test]
    fn test_words_filter() {
        let text = "Hello, world!";
        let actual_words: Vec<_> = words(text).collect();
        // Should only get "Hello" and "world"
        assert_eq!(actual_words.len(), 2);
        assert_eq!(actual_words[0].text, "Hello");
        assert_eq!(actual_words[1].text, "world");
    }

    #[test]
    fn test_grapheme_at_offset() {
        let text = "Hi👋!";

        // At 'H'
        let seg = grapheme_at_offset(text, 0).unwrap();
        assert_eq!(seg.text, "H");

        // Within the emoji
        let seg = grapheme_at_offset(text, 3).unwrap();
        assert_eq!(seg.text, "👋");

        // Out of bounds
        assert!(grapheme_at_offset(text, 100).is_none());
    }

    #[test]
    fn test_floor_grapheme_boundary() {
        let text = "Hi👋";
        assert_eq!(floor_grapheme_boundary(text, 0), 0);
        assert_eq!(floor_grapheme_boundary(text, 1), 1);
        assert_eq!(floor_grapheme_boundary(text, 3), 2); // In middle of emoji
        assert_eq!(floor_grapheme_boundary(text, 6), 6);
    }

    #[test]
    fn test_ceil_grapheme_boundary() {
        let text = "Hi👋";
        assert_eq!(ceil_grapheme_boundary(text, 0), 0);
        assert_eq!(ceil_grapheme_boundary(text, 3), 6); // In middle of emoji
        assert_eq!(ceil_grapheme_boundary(text, 6), 6);
    }

    #[test]
    fn test_is_grapheme_boundary() {
        let text = "Hi👋";
        assert!(is_grapheme_boundary(text, 0));
        assert!(is_grapheme_boundary(text, 1));
        assert!(is_grapheme_boundary(text, 2));
        assert!(!is_grapheme_boundary(text, 3)); // Middle of emoji
        assert!(is_grapheme_boundary(text, 6));
    }

    #[test]
    fn test_combining_characters() {
        // é as e + combining acute accent
        let text = "e\u{0301}";
        let segments: Vec<_> = grapheme_indices(text).collect();
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "e\u{0301}");
    }

    #[test]
    fn test_empty_text() {
        assert_eq!(grapheme_count(""), 0);
        assert_eq!(grapheme_boundaries(""), vec![0]);
        assert!(grapheme_at_offset("", 0).is_none());
    }

    #[test]
    fn test_sentence_indices() {
        let text = "Hello. World!";
        let sentences: Vec<_> = sentence_indices(text).collect();
        assert!(!sentences.is_empty());
    }
}
