//! # RustKit Text
//!
//! Cross-platform font access, metrics, and glyph indices for RustKit.
//!
//! ## Platform Support
//!
//! - **Windows**: DirectWrite (via `windows` crate)
//! - **macOS**: Core Text (via `core-text` crate)
//! - **Linux**: Fontconfig + FreeType
//!
//! ## Features
//!
//! - System font collection lookup by family name
//! - Match a font by weight/stretch/style
//! - Create font face
//! - Read font metrics (design units)
//! - Map Unicode codepoints -> glyph indices
//! - Read design glyph metrics (advance widths)

//! ## Unicode text algorithms (platform-independent)
//!
//! - [`bidi`]: Unicode Bidirectional Algorithm for mixed LTR/RTL text
//! - [`line_break`]: Unicode Line Breaking Algorithm for text wrapping
//! - [`segmentation`]: Grapheme cluster, word, and sentence boundaries

pub mod bidi;
pub mod line_break;
pub mod segmentation;

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

// Shared types and trait for platform backends

#[derive(Debug, Clone)]
pub struct FontDescriptor {
    pub family: String,
    pub weight: FontWeight,
    pub style: FontStyle,
    pub size: f32,
}

#[derive(Debug, Clone)]
pub struct FontFamily {
    pub name: String,
    pub styles: Vec<FontStyle>,
}

#[derive(Debug, Clone)]
pub struct GlyphInfo {
    pub glyph_id: u32,
    pub advance: f32,
    pub x_offset: f32,
    pub y_offset: f32,
}

#[derive(Debug, Clone)]
pub struct ShapedGlyph {
    pub glyph_id: u32,
    pub x_offset: f32,
    pub y_offset: f32,
    pub advance: f32,
    pub cluster: u32,
}

#[derive(Debug, Clone)]
pub struct ShapedText {
    pub glyphs: Vec<ShapedGlyph>,
    pub width: f32,
    pub metrics: TextMetrics,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TextMetrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
    pub em_size: f32,
    pub x_height: f32,
    pub cap_height: f32,
}

#[derive(thiserror::Error, Debug, Clone)]
pub enum TextError {
    #[error("Not implemented on this platform")]
    NotImplemented,
    #[error("Initialization failed: {0}")]
    InitializationFailed(String),
    #[error("Font not found: {0}")]
    FontNotFound(String),
    #[error("Shaping failed: {0}")]
    ShapingFailed(String),
}

pub trait TextBackend {
    fn shape_text(&mut self, text: &str, descriptor: &FontDescriptor) -> Result<ShapedText, TextError>;
    fn get_font_families(&self) -> Result<Vec<FontFamily>, TextError>;
    fn get_metrics(&mut self, descriptor: &FontDescriptor) -> Result<TextMetrics, TextError>;
    fn get_fallback_fonts(&self, text: &str) -> Vec<String>;
}

// Platform-specific implementations
#[cfg(windows)]
mod win;

#[cfg(windows)]
pub use win::{FontCollection, FontFace, FontFamily as WinFontFamily, Font as WinFont};

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Fallback for unsupported platforms (no-op stubs using shared types)
#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod nowin {
    use super::*;

    #[derive(Clone)]
    pub struct FontCollection;
    pub struct Font;
    #[derive(Clone)]
    pub struct FontFace;

    impl FontCollection {
        pub fn system() -> Result<Self, TextError> {
            Err(TextError::NotImplemented)
        }

        pub fn font_family_by_name(&self, _name: &str) -> Result<Option<FontFamily>, TextError> {
            Err(TextError::NotImplemented)
        }
    }

    impl Font {
        pub fn create_font_face(&self) -> Result<FontFace, TextError> {
            Err(TextError::NotImplemented)
        }
    }

    impl FontFace {
        pub fn metrics(&self) -> Result<TextMetrics, TextError> {
            Err(TextError::NotImplemented)
        }

        pub fn glyph_indices(&self, _codepoints: &[u32]) -> Result<Vec<u16>, TextError> {
            Err(TextError::NotImplemented)
        }

        pub fn design_glyph_metrics(
            &self,
            _glyph_indices: &[u16],
            _is_sideways: bool,
        ) -> Result<Vec<GlyphInfo>, TextError> {
            Err(TextError::NotImplemented)
        }
    }
}

