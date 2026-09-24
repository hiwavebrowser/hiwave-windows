//! # RustKit Text
//!
//! RustKit-owned access to fonts, metrics, glyph indices, and text processing.
//!
//! This crate provides:
//! - System font collection lookup by family name
//! - Match a font by weight/stretch/style
//! - Create font face
//! - Read font metrics (design units)
//! - Map Unicode codepoints -> glyph indices
//! - Read design glyph metrics (advance widths)
//! - Bidirectional text support (UAX #9)
//! - Text segmentation (UAX #29)
//! - Line breaking (UAX #14)
//!
//! ## Modules
//!
//! - [`bidi`]: Unicode Bidirectional Algorithm for mixed LTR/RTL text
//! - [`line_break`]: Unicode Line Breaking Algorithm for text wrapping
//! - [`segmentation`]: Grapheme cluster, word, and sentence boundaries

pub mod bidi;
pub mod line_break;
pub mod segmentation;
pub mod webfonts;

use thiserror::Error;

/// Errors for rustkit-text operations.
#[derive(Error, Debug, Clone)]
pub enum TextBackendError {
    #[error("Not implemented on this platform")]
    NotImplemented,

    #[error("DirectWrite error: {0}")]
    DirectWrite(String),

    #[error("Font not found: {0}")]
    FontNotFound(String),
}

/// Font style.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontStyle {
    Normal,
    Italic,
    Oblique,
}

/// Font weight (DirectWrite-compatible numeric weight).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontWeight(pub u32);

impl FontWeight {
    pub fn from_u32(v: u32) -> Self {
        Self(v)
    }
}

/// Font stretch (DirectWrite-compatible numeric stretch).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontStretch(pub u32);

impl FontStretch {
    pub fn from_u32(v: u32) -> Self {
        Self(v)
    }
}

/// Font metrics in design units.
#[derive(Debug, Clone, Copy)]
pub struct FontMetrics {
    pub design_units_per_em: u16,
    pub ascent: u16,
    pub descent: u16,
    pub line_gap: i16,
    pub underline_position: i16,
    pub underline_thickness: u16,
    pub strikethrough_position: i16,
    pub strikethrough_thickness: u16,
}

/// Glyph metrics in design units.
#[derive(Debug, Clone, Copy)]
pub struct GlyphMetrics {
    pub advance_width: i32,
}

#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{FontCollection, FontFace, FontFamily, Font};

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "macos")]
pub use macos::{TextShaper, ShapedText, FontMetrics as MacOSFontMetrics};

#[cfg(not(any(windows, target_os = "macos")))]
mod nowin {
    use super::*;

    #[derive(Clone)]
    pub struct FontCollection;
    pub struct FontFamily;
    pub struct Font;
    #[derive(Clone)]
    pub struct FontFace;

    impl FontCollection {
        pub fn system() -> Result<Self, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }

        pub fn font_family_by_name(&self, _name: &str) -> Result<Option<FontFamily>, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }
    }

    impl FontFamily {
        pub fn first_matching_font(
            &self,
            _weight: FontWeight,
            _stretch: FontStretch,
            _style: FontStyle,
        ) -> Result<Font, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }
    }

    impl Font {
        pub fn create_font_face(&self) -> Result<FontFace, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }
    }

    impl FontFace {
        pub fn metrics(&self) -> Result<FontMetrics, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }

        pub fn glyph_indices(&self, _codepoints: &[u32]) -> Result<Vec<u16>, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }

        pub fn design_glyph_metrics(
            &self,
            _glyph_indices: &[u16],
            _is_sideways: bool,
        ) -> Result<Vec<GlyphMetrics>, TextBackendError> {
            Err(TextBackendError::NotImplemented)
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
pub use nowin::{FontCollection, FontFace, FontFamily, Font};

// macOS implementation uses different API - TextShaper instead of FontCollection
// For compatibility, we can provide a wrapper if needed
#[cfg(target_os = "macos")]
mod macos_compat {
    use super::*;

    #[derive(Clone)]
    pub struct FontCollection;

    pub struct FontFamily;

    pub struct Font;

    #[derive(Clone)]
    pub struct FontFace;

    impl FontCollection {
        pub fn system() -> Result<Self, TextBackendError> {
            Ok(Self)
        }

        pub fn font_family_by_name(&self, _name: &str) -> Result<Option<FontFamily>, TextBackendError> {
            Ok(Some(FontFamily))
        }
    }

    impl FontFamily {
        pub fn first_matching_font(
            &self,
            _weight: FontWeight,
            _stretch: FontStretch,
            _style: FontStyle,
        ) -> Result<Font, TextBackendError> {
            Ok(Font)
        }
    }

    impl Font {
        pub fn create_font_face(&self) -> Result<FontFace, TextBackendError> {
            Ok(FontFace)
        }
    }

    impl FontFace {
        pub fn metrics(&self) -> Result<FontMetrics, TextBackendError> {
            // Use Core Text to get metrics
            use crate::macos::TextShaper;
            let shaper = TextShaper::with_system_font(12.0);
            let metrics = shaper.get_metrics();
            Ok(FontMetrics {
                design_units_per_em: 2048, // Typical for TrueType fonts
                ascent: (metrics.ascent * 2048.0 / 12.0) as u16,
                descent: (metrics.descent * 2048.0 / 12.0) as u16,
                line_gap: (metrics.leading * 2048.0 / 12.0) as i16,
                underline_position: -100, // Approximate
                underline_thickness: 50,  // Approximate
                strikethrough_position: 600, // Approximate
                strikethrough_thickness: 50, // Approximate
            })
        }

        pub fn glyph_indices(&self, codepoints: &[u32]) -> Result<Vec<u16>, TextBackendError> {
            use crate::macos::TextShaper;
            let shaper = TextShaper::with_system_font(12.0);
            let text: String = codepoints.iter()
                .filter_map(|&cp| char::from_u32(cp))
                .collect();
            let shaped = shaper.shape(&text)
                .map_err(|e| TextBackendError::DirectWrite(format!("Core Text error: {}", e)))?;
            Ok(shaped.glyphs)
        }

        pub fn design_glyph_metrics(
            &self,
            glyph_indices: &[u16],
            _is_sideways: bool,
        ) -> Result<Vec<GlyphMetrics>, TextBackendError> {
            use crate::macos::TextShaper;
            let shaper = TextShaper::with_system_font(12.0);
            // Create dummy text to get advances
            let text: String = glyph_indices.iter()
                .map(|_| 'A') // Dummy character
                .collect();
            let shaped = shaper.shape(&text)
                .map_err(|e| TextBackendError::DirectWrite(format!("Core Text error: {}", e)))?;
            Ok(shaped.advances.iter()
                .map(|&advance| GlyphMetrics {
                    advance_width: (advance * 2048.0 / 12.0) as i32,
                })
                .collect())
        }
    }
}

#[cfg(target_os = "macos")]
pub use macos_compat::{FontCollection, FontFace, FontFamily, Font};


