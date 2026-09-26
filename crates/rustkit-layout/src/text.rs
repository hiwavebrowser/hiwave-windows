//! # Text Rendering Module
//!
//! Comprehensive text rendering support using DirectWrite on Windows and Core Text on macOS.
//! Provides font fallback, text shaping, text decoration, and line height calculation.
//!
//! ## Features
//!
//! - **Font Fallback Chain**: Automatic fallback for missing glyphs
//! - **Complex Script Support**: Full Unicode shaping via DirectWrite/Core Text
//! - **Text Decoration**: Underline, strikethrough, overline
//! - **Line Height**: Proper line-height calculation with various units
//! - **Font Variants**: Bold, italic, weights, stretches
//! - **Metrics**: Accurate glyph and line metrics
//! - **Bidirectional Text**: Support for mixed LTR/RTL text via UAX #9
//! - **Line Breaking**: Text wrapping with CSS word-break support via UAX #14

use rustkit_css::{
    Color, Direction as CssDirection, FontStretch, FontStyle, FontWeight, Length,
    OverflowWrap as CssOverflowWrap, TextDecorationLine, TextDecorationStyle, TextTransform,
    WhiteSpace, WordBreak as CssWordBreak,
};
use rustkit_text::bidi::{BidiInfo, Direction as BidiDirection};
use rustkit_text::line_break::{LineBreaker, OverflowWrap, WordBreak as LineBreakWordBreak};
use std::collections::HashMap;
use std::sync::RwLock;
use thiserror::Error;

#[cfg(windows)]
use rustkit_text::{
    FontCollection as RkFontCollection, FontStretch as RkFontStretch, FontStyle as RkFontStyle,
    FontWeight as RkFontWeight,
};
#[cfg(windows)]
use std::sync::Arc;

#[cfg(target_os = "macos")]
use core_foundation::base::TCFType;
#[cfg(target_os = "macos")]
use core_graphics::geometry::CGSize;
#[cfg(target_os = "macos")]
use core_text::font as ct_font;

/// Errors that can occur in text operations.
#[derive(Error, Debug)]
pub enum TextError {
    #[error("Font not found: {0}")]
    FontNotFound(String),

    #[error("Text shaping failed: {0}")]
    ShapingFailed(String),

    #[error("Font loading failed: {0}")]
    FontLoadFailed(String),

    #[error("DirectWrite error: {0}")]
    DirectWriteError(String),
}

/// A font family with fallback chain.
#[derive(Debug, Clone)]
pub struct FontFamilyChain {
    /// Primary font family name.
    pub primary: String,
    /// Fallback font families in order.
    pub fallbacks: Vec<String>,
}

impl FontFamilyChain {
    /// Create a new font family chain.
    pub fn new(primary: impl Into<String>) -> Self {
        Self {
            primary: primary.into(),
            fallbacks: Vec::new(),
        }
    }

    /// Add a fallback font.
    pub fn with_fallback(mut self, family: impl Into<String>) -> Self {
        self.fallbacks.push(family.into());
        self
    }

    /// Get all families in order (primary + fallbacks).
    pub fn all_families(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.primary.as_str()).chain(self.fallbacks.iter().map(|s| s.as_str()))
    }

    /// Create default font chain for sans-serif.
    #[cfg(target_os = "macos")]
    pub fn sans_serif() -> Self {
        Self::new("SF Pro")
            .with_fallback(".AppleSystemUIFont")
            .with_fallback("Helvetica Neue")
            .with_fallback("Helvetica")
            .with_fallback("Arial")
            .with_fallback("PingFang SC")
            .with_fallback("Hiragino Sans")
            .with_fallback("sans-serif")
    }

    /// Create default font chain for sans-serif.
    #[cfg(not(target_os = "macos"))]
    pub fn sans_serif() -> Self {
        Self::new("Segoe UI")
            .with_fallback("Arial")
            .with_fallback("Helvetica")
            .with_fallback("Noto Sans")
            .with_fallback("Noto Sans CJK SC")
            .with_fallback("Microsoft YaHei")
            .with_fallback("sans-serif")
    }

    /// Create default font chain for serif.
    #[cfg(target_os = "macos")]
    pub fn serif() -> Self {
        Self::new("New York")
            .with_fallback("Times New Roman")
            .with_fallback("Georgia")
            .with_fallback("Songti SC")
            .with_fallback("serif")
    }

    /// Create default font chain for serif.
    #[cfg(not(target_os = "macos"))]
    pub fn serif() -> Self {
        Self::new("Times New Roman")
            .with_fallback("Georgia")
            .with_fallback("Noto Serif")
            .with_fallback("Noto Serif CJK SC")
            .with_fallback("SimSun")
            .with_fallback("serif")
    }

    /// Create default font chain for monospace.
    /// Menlo first: it is Chrome's default `monospace` on macOS and ships
    /// with the OS. SF Mono is an Xcode/Terminal bundle font — leading with
    /// it measured a Core Text substitute on stock machines (see
    /// rustkit-text `named_font`) and would measure a different face from
    /// Chrome's on machines that have it.
    #[cfg(target_os = "macos")]
    pub fn monospace() -> Self {
        Self::new("Menlo")
            .with_fallback("SF Mono")
            .with_fallback("Monaco")
            .with_fallback("Courier New")
            .with_fallback("monospace")
    }

    /// Create default font chain for monospace.
    #[cfg(not(target_os = "macos"))]
    pub fn monospace() -> Self {
        Self::new("Cascadia Code")
            .with_fallback("Consolas")
            .with_fallback("Courier New")
            .with_fallback("Noto Sans Mono")
            .with_fallback("monospace")
    }

    /// Create system-ui font chain (platform-specific).
    #[cfg(target_os = "macos")]
    pub fn system_ui() -> Self {
        Self::new(".AppleSystemUIFont")
            .with_fallback("SF Pro")
            .with_fallback("Helvetica Neue")
            .with_fallback("Helvetica")
            .with_fallback("Arial")
    }

    /// Create system-ui font chain (platform-specific).
    #[cfg(not(target_os = "macos"))]
    pub fn system_ui() -> Self {
        Self::new("Segoe UI")
            .with_fallback("Roboto")
            .with_fallback("Arial")
            .with_fallback("Noto Sans")
    }

    /// Resolve a CSS font-family value to a chain.
    pub fn from_css_value(value: &str) -> Self {
        let families: Vec<&str> = value
            .split(',')
            .map(|s| s.trim().trim_matches('"').trim_matches('\''))
            .collect();

        if families.is_empty() {
            return Self::sans_serif();
        }

        let primary = families[0];

        // Handle generic families
        match primary.to_lowercase().as_str() {
            "sans-serif" => Self::sans_serif(),
            "serif" => Self::serif(),
            "monospace" => Self::monospace(),
            "system-ui" | "-apple-system" | "blinkmacsystemfont" => Self::system_ui(),
            "cursive" => Self::new("Comic Sans MS")
                .with_fallback("Brush Script MT")
                .with_fallback("cursive"),
            "fantasy" => Self::new("Impact")
                .with_fallback("Papyrus")
                .with_fallback("fantasy"),
            _ => {
                let mut chain = Self::new(primary);
                for fallback in families.iter().skip(1) {
                    // Recursively handle generic families in fallback chain
                    let lower = fallback.to_lowercase();
                    if lower == "system-ui"
                        || lower == "-apple-system"
                        || lower == "blinkmacsystemfont"
                    {
                        let sys_chain = Self::system_ui();
                        chain.fallbacks.push(sys_chain.primary);
                        chain.fallbacks.extend(sys_chain.fallbacks);
                    } else if lower == "sans-serif" {
                        let sans_chain = Self::sans_serif();
                        chain.fallbacks.push(sans_chain.primary);
                        chain.fallbacks.extend(sans_chain.fallbacks);
                    } else {
                        chain.fallbacks.push(fallback.to_string());
                    }
                }
                // Add platform-specific system fallbacks
                #[cfg(target_os = "macos")]
                {
                    chain.fallbacks.push(".AppleSystemUIFont".to_string());
                    chain.fallbacks.push("Helvetica".to_string());
                }
                #[cfg(not(target_os = "macos"))]
                {
                    chain.fallbacks.push("Segoe UI".to_string());
                    chain.fallbacks.push("Arial".to_string());
                }
                chain
            }
        }
    }
}

/// Text metrics from shaping.
#[derive(Debug, Clone, Default)]
pub struct TextMetrics {
    /// Total width of the text run.
    pub width: f32,
    /// Total height (ascent + descent + line gap).
    pub height: f32,
    /// Distance from baseline to top of highest glyph.
    pub ascent: f32,
    /// Distance from baseline to bottom of lowest glyph.
    pub descent: f32,
    /// Leading (line gap).
    pub leading: f32,
    /// Underline position relative to baseline.
    pub underline_offset: f32,
    /// Underline thickness.
    pub underline_thickness: f32,
    /// Strikethrough position relative to baseline.
    pub strikethrough_offset: f32,
    /// Strikethrough thickness.
    pub strikethrough_thickness: f32,
    /// Overline position relative to baseline (top of text).
    pub overline_offset: f32,
}

impl TextMetrics {
    /// Create metrics with baseline values.
    /// Ratios based on SF Pro font metrics (macOS system font).
    /// SF Pro: ~0.82 ascent, ~0.21 descent (measured from actual Core Text metrics).
    /// Previous values (0.88/0.24) were too large and caused baseline shifts.
    pub fn with_font_size(font_size: f32) -> Self {
        // Use SF Pro ratios as default - these match macOS system font better
        let ascent = font_size * 0.82;
        let descent = font_size * 0.21;
        let leading = 0.0;

        Self {
            width: 0.0,
            height: ascent + descent + leading,
            ascent,
            descent,
            leading,
            underline_offset: descent * 0.5,
            underline_thickness: font_size / 14.0,
            strikethrough_offset: -ascent * 0.35,
            strikethrough_thickness: font_size / 14.0,
            overline_offset: -ascent,
        }
    }

    /// Create metrics from a Core Text font (macOS).
    /// This provides accurate metrics directly from the font.
    #[cfg(target_os = "macos")]
    pub fn from_core_text_font(ct_font: &core_text::font::CTFont, width: f32) -> Self {
        let ascent = ct_font.ascent() as f32;
        let descent = ct_font.descent() as f32;
        let leading = ct_font.leading() as f32;
        let underline_position = ct_font.underline_position() as f32;
        let underline_thickness = ct_font.underline_thickness() as f32;
        let x_height = ct_font.x_height() as f32;
        let strikethrough_offset = x_height * 0.5;

        Self {
            width,
            height: ascent + descent + leading,
            ascent,
            descent,
            leading,
            underline_offset: underline_position,
            underline_thickness,
            strikethrough_offset,
            strikethrough_thickness: underline_thickness,
            overline_offset: -ascent,
        }
    }
}

/// A positioned glyph in a text run.
#[derive(Debug, Clone)]
pub struct PositionedGlyph {
    /// Glyph ID (font-specific).
    pub glyph_id: u16,
    /// X offset from the start of the run.
    pub x: f32,
    /// Y offset from the baseline.
    pub y: f32,
    /// Advance width.
    pub advance: f32,
    /// The character this glyph represents.
    pub character: char,
    /// Cluster index for multi-glyph characters.
    pub cluster: u32,
}

/// A shaped text run.
#[derive(Debug, Clone)]
pub struct ShapedRun {
    /// The original text.
    pub text: String,
    /// Positioned glyphs.
    pub glyphs: Vec<PositionedGlyph>,
    /// Font family used.
    pub font_family: String,
    /// Font weight.
    pub font_weight: FontWeight,
    /// Font style.
    pub font_style: FontStyle,
    /// Font stretch.
    pub font_stretch: FontStretch,
    /// Font size in pixels.
    pub font_size: f32,
    /// Text metrics.
    pub metrics: TextMetrics,
    /// Text direction (LTR or RTL).
    pub direction: TextDirection,
}

/// Text direction for a shaped run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextDirection {
    /// Left-to-right (Latin, Greek, Cyrillic, etc.)
    #[default]
    Ltr,
    /// Right-to-left (Arabic, Hebrew, etc.)
    Rtl,
}

impl TextDirection {
    /// Convert from CSS Direction.
    pub fn from_css(direction: CssDirection) -> Self {
        match direction {
            CssDirection::Ltr => TextDirection::Ltr,
            CssDirection::Rtl => TextDirection::Rtl,
        }
    }

    /// Convert from bidi Direction.
    pub fn from_bidi(direction: BidiDirection) -> Self {
        match direction {
            BidiDirection::Ltr => TextDirection::Ltr,
            BidiDirection::Rtl => TextDirection::Rtl,
        }
    }

    /// Convert to bidi Direction.
    pub fn to_bidi(self) -> BidiDirection {
        match self {
            TextDirection::Ltr => BidiDirection::Ltr,
            TextDirection::Rtl => BidiDirection::Rtl,
        }
    }

    /// Check if this is left-to-right.
    pub fn is_ltr(self) -> bool {
        self == TextDirection::Ltr
    }

    /// Check if this is right-to-left.
    pub fn is_rtl(self) -> bool {
        self == TextDirection::Rtl
    }
}

impl ShapedRun {
    /// Get the total width of the run.
    pub fn width(&self) -> f32 {
        self.metrics.width
    }

    /// Get the height of the run.
    pub fn height(&self) -> f32 {
        self.metrics.height
    }

    /// Apply letter-spacing to the shaped run.
    /// Letter-spacing adds extra space after each character.
    pub fn apply_letter_spacing(&mut self, letter_spacing: f32) {
        if letter_spacing == 0.0 || self.glyphs.is_empty() {
            return;
        }

        let mut accumulated_offset = 0.0;
        for glyph in &mut self.glyphs {
            // Shift glyph position by accumulated offset
            glyph.x += accumulated_offset;
            // Add letter-spacing to advance
            glyph.advance += letter_spacing;
            accumulated_offset += letter_spacing;
        }

        // Update total width
        self.metrics.width += accumulated_offset;
    }

    /// Apply word-spacing to the shaped run.
    /// Word-spacing adds extra space to whitespace characters.
    pub fn apply_word_spacing(&mut self, word_spacing: f32) {
        if word_spacing == 0.0 || self.glyphs.is_empty() {
            return;
        }

        let mut accumulated_offset = 0.0;
        for glyph in &mut self.glyphs {
            // Shift glyph position by accumulated offset
            glyph.x += accumulated_offset;

            // Add word-spacing to whitespace characters
            if glyph.character.is_whitespace() {
                glyph.advance += word_spacing;
                accumulated_offset += word_spacing;
            }
        }

        // Update total width
        self.metrics.width += accumulated_offset;
    }

    /// Apply both letter-spacing and word-spacing.
    pub fn apply_spacing(&mut self, letter_spacing: f32, word_spacing: f32) {
        // Apply word-spacing first, then letter-spacing
        // This matches CSS specification behavior
        self.apply_word_spacing(word_spacing);
        self.apply_letter_spacing(letter_spacing);
    }
}

/// Text decoration rendering information.
#[derive(Debug, Clone)]
pub struct TextDecoration {
    /// Decoration lines to draw.
    pub lines: TextDecorationLine,
    /// Decoration color (defaults to text color).
    pub color: Option<Color>,
    /// Decoration style.
    pub style: TextDecorationStyle,
    /// Decoration thickness (auto uses font metrics).
    pub thickness: Option<f32>,
}

impl TextDecoration {
    /// Create decoration from CSS properties.
    pub fn from_style(
        lines: TextDecorationLine,
        color: Option<Color>,
        style: TextDecorationStyle,
        thickness: Length,
        font_size: f32,
    ) -> Self {
        let thickness_px = match thickness {
            Length::Auto => None,
            Length::Px(px) => Some(px),
            Length::Em(em) => Some(em * font_size),
            Length::Rem(rem) => Some(rem * 16.0),
            _ => None,
        };

        Self {
            lines,
            color,
            style,
            thickness: thickness_px,
        }
    }

    /// Check if any decorations are active.
    pub fn has_decorations(&self) -> bool {
        self.lines.underline || self.lines.overline || self.lines.line_through
    }
}

/// Line height calculation modes.
#[derive(Debug, Clone, Copy)]
pub enum LineHeight {
    /// Normal line height (use font metrics).
    Normal,
    /// Multiplier (e.g., 1.5 = 150% of font size).
    Number(f32),
    /// Absolute length in pixels.
    Length(f32),
}

impl LineHeight {
    /// Parse from CSS line-height value.
    pub fn from_css(value: f32, is_number: bool) -> Self {
        if is_number {
            LineHeight::Number(value)
        } else {
            LineHeight::Length(value)
        }
    }

    /// Compute the actual line height in pixels.
    pub fn compute(&self, font_size: f32, metrics: &TextMetrics) -> f32 {
        match self {
            LineHeight::Normal => metrics.height,
            LineHeight::Number(n) => font_size * n,
            LineHeight::Length(px) => *px,
        }
    }

    /// Compute leading (extra space above/below text).
    pub fn compute_leading(&self, font_size: f32, metrics: &TextMetrics) -> f32 {
        let line_height = self.compute(font_size, metrics);
        let content_height = metrics.ascent + metrics.descent;
        (line_height - content_height).max(0.0)
    }
}

/// Apply text transform to a string.
pub fn apply_text_transform(text: &str, transform: TextTransform) -> String {
    match transform {
        TextTransform::None => text.to_string(),
        TextTransform::Uppercase => text.to_uppercase(),
        TextTransform::Lowercase => text.to_lowercase(),
        TextTransform::Capitalize => {
            let mut result = String::with_capacity(text.len());
            let mut capitalize_next = true;
            for c in text.chars() {
                if c.is_whitespace() {
                    capitalize_next = true;
                    result.push(c);
                } else if capitalize_next {
                    result.extend(c.to_uppercase());
                    capitalize_next = false;
                } else {
                    result.push(c);
                }
            }
            result
        }
    }
}

/// Collapse whitespace according to white-space property.
pub fn collapse_whitespace(text: &str, white_space: WhiteSpace) -> String {
    match white_space {
        WhiteSpace::Normal | WhiteSpace::Nowrap => {
            // Collapse sequences of whitespace to single space
            let mut result = String::with_capacity(text.len());
            let mut last_was_space = false;
            for c in text.chars() {
                if c.is_whitespace() {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                } else {
                    result.push(c);
                    last_was_space = false;
                }
            }
            result.trim().to_string()
        }
        WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::BreakSpaces => {
            // Preserve whitespace
            text.to_string()
        }
        WhiteSpace::PreLine => {
            // Collapse spaces but preserve newlines
            let mut result = String::with_capacity(text.len());
            let mut last_was_space = false;
            for c in text.chars() {
                if c == '\n' {
                    result.push('\n');
                    last_was_space = false;
                } else if c.is_whitespace() {
                    if !last_was_space {
                        result.push(' ');
                        last_was_space = true;
                    }
                } else {
                    result.push(c);
                    last_was_space = false;
                }
            }
            result
        }
    }
}

/// Font cache for reusing font objects.
#[derive(Default)]
pub struct FontCache {
    #[cfg(windows)]
    fonts: RwLock<HashMap<FontKey, Arc<FontCacheEntry>>>,
    #[cfg(not(windows))]
    _fonts: RwLock<HashMap<FontKey, ()>>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct FontKey {
    family: String,
    weight: u16,
    style: u8,
    stretch: u8,
}

#[cfg(windows)]
struct FontCacheEntry {
    #[allow(dead_code)]
    font_face: rustkit_text::FontFace,
    metrics: TextMetrics,
}

impl FontCache {
    /// Create a new font cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Get font metrics for a given font configuration.
    #[cfg(windows)]
    pub fn get_metrics(
        &self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<TextMetrics, TextError> {
        let key = FontKey {
            family: family.to_string(),
            weight: weight.0,
            style: match style {
                FontStyle::Normal => 0,
                FontStyle::Italic => 1,
                FontStyle::Oblique => 2,
            },
            stretch: stretch.to_dwrite_value() as u8,
        };

        // Try cache first
        {
            let cache = self.fonts.read().unwrap();
            if let Some(entry) = cache.get(&key) {
                let mut metrics = entry.metrics.clone();
                // Scale metrics to requested size
                let scale = size / 16.0;
                metrics.width *= scale;
                metrics.height *= scale;
                metrics.ascent *= scale;
                metrics.descent *= scale;
                metrics.leading *= scale;
                metrics.underline_offset *= scale;
                metrics.underline_thickness *= scale;
                metrics.strikethrough_offset *= scale;
                metrics.strikethrough_thickness *= scale;
                metrics.overline_offset *= scale;
                return Ok(metrics);
            }
        }

        // Load font and get metrics
        self.load_font_metrics(family, weight, style, stretch, size)
    }

    #[cfg(windows)]
    fn load_font_metrics(
        &self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<TextMetrics, TextError> {
        let collection =
            RkFontCollection::system().map_err(|e| TextError::DirectWriteError(e.to_string()))?;

        // Try to find the font family
        let dw_family = collection
            .font_family_by_name(family)
            .map_err(|e| TextError::DirectWriteError(e.to_string()))?
            .or_else(|| collection.font_family_by_name("Segoe UI").ok().flatten());

        if let Some(family) = dw_family {
            let dw_weight = RkFontWeight::from_u32(weight.0 as u32);
            let dw_style = match style {
                FontStyle::Normal => RkFontStyle::Normal,
                FontStyle::Italic => RkFontStyle::Italic,
                FontStyle::Oblique => RkFontStyle::Oblique,
            };
            let dw_stretch = RkFontStretch::from_u32(stretch.to_dwrite_value());

            if let Ok(font) = family.first_matching_font(dw_weight, dw_stretch, dw_style) {
                let face = font
                    .create_font_face()
                    .map_err(|e| TextError::DirectWriteError(e.to_string()))?;
                let design_metrics = face
                    .metrics()
                    .map_err(|e| TextError::DirectWriteError(e.to_string()))?;

                // Convert design units to pixels (DWRITE uses camelCase)
                let units_per_em = design_metrics.design_units_per_em as f32;
                let scale = size / units_per_em;

                let ascent = design_metrics.ascent as f32 * scale;
                let descent = design_metrics.descent as f32 * scale;
                let leading = design_metrics.line_gap as f32 * scale;

                return Ok(TextMetrics {
                    width: 0.0,
                    height: ascent + descent + leading,
                    ascent,
                    descent,
                    leading,
                    underline_offset: design_metrics.underline_position as f32 * scale,
                    underline_thickness: design_metrics.underline_thickness as f32 * scale,
                    strikethrough_offset: design_metrics.strikethrough_position as f32 * scale,
                    strikethrough_thickness: design_metrics.strikethrough_thickness as f32 * scale,
                    overline_offset: -ascent,
                });
            }
        }

        // Fallback to computed metrics
        Ok(TextMetrics::with_font_size(size))
    }

    #[cfg(target_os = "macos")]
    pub fn get_metrics(
        &self,
        family: &str,
        weight: FontWeight,
        style: FontStyle,
        _stretch: FontStretch,
        size: f32,
    ) -> Result<TextMetrics, TextError> {
        // Try to get real Core Text metrics for the requested font
        if let Ok(font) = TextShaper::create_ct_font_with_traits(
            family,
            size,
            weight.0,
            style == FontStyle::Italic,
        ) {
            return Ok(TextMetrics::from_core_text_font(&font, 0.0));
        }

        // Try system font as fallback
        if let Ok(font) = ct_font::new_from_name("Helvetica", size as f64) {
            return Ok(TextMetrics::from_core_text_font(&font, 0.0));
        }

        // Ultimate fallback to computed metrics
        Ok(TextMetrics::with_font_size(size))
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn get_metrics(
        &self,
        _family: &str,
        _weight: FontWeight,
        _style: FontStyle,
        _stretch: FontStretch,
        size: f32,
    ) -> Result<TextMetrics, TextError> {
        // Fallback metrics for other platforms (Linux, etc.)
        Ok(TextMetrics::with_font_size(size))
    }
}

/// Text shaper for complex text layout.
pub struct TextShaper {
    #[allow(dead_code)]
    cache: FontCache,
}

impl TextShaper {
    /// Create a new text shaper.
    pub fn new() -> Self {
        Self {
            cache: FontCache::new(),
        }
    }

    /// Shape text with the given style.
    #[cfg(windows)]
    pub fn shape(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<ShapedRun, TextError> {
        if text.is_empty() {
            return Ok(ShapedRun {
                text: String::new(),
                glyphs: Vec::new(),
                font_family: font_chain.primary.clone(),
                font_weight: weight,
                font_style: style,
                font_stretch: stretch,
                font_size: size,
                metrics: TextMetrics::with_font_size(size),
                direction: TextDirection::Ltr,
            });
        }

        let collection =
            RkFontCollection::system().map_err(|e| TextError::DirectWriteError(e.to_string()))?;

        // Find first available font in chain
        let mut font_family_name = font_chain.primary.clone();
        let mut found_font = None;

        for family_name in font_chain.all_families() {
            if let Ok(Some(family)) = collection.font_family_by_name(family_name) {
                let dw_weight = RkFontWeight::from_u32(weight.0 as u32);
                let dw_style = match style {
                    FontStyle::Normal => RkFontStyle::Normal,
                    FontStyle::Italic => RkFontStyle::Italic,
                    FontStyle::Oblique => RkFontStyle::Oblique,
                };
                let dw_stretch = RkFontStretch::from_u32(stretch.to_dwrite_value());

                if let Ok(font) = family.first_matching_font(dw_weight, dw_stretch, dw_style) {
                    font_family_name = family_name.to_string();
                    found_font = Some(font);
                    break;
                }
            }
        }

        // If we found a font, use DirectWrite for accurate shaping
        if let Some(font) = found_font {
            let face = font
                .create_font_face()
                .map_err(|e| TextError::DirectWriteError(e.to_string()))?;
            let design_metrics = face
                .metrics()
                .map_err(|e| TextError::DirectWriteError(e.to_string()))?;

            let units_per_em = design_metrics.design_units_per_em as f32;
            let scale = size / units_per_em;

            // Get glyph indices - handle Result
            let text_chars: Vec<char> = text.chars().collect();
            let codepoints: Vec<u32> = text_chars.iter().map(|c| *c as u32).collect();

            // Try to get glyph indices, fall back to simple shaping if it fails
            if let Ok(glyph_indices) = face.glyph_indices(&codepoints) {
                // Try to get glyph metrics
                if let Ok(glyph_metrics) = face.design_glyph_metrics(&glyph_indices, false) {
                    let mut glyphs = Vec::with_capacity(text_chars.len());
                    let mut x_offset: f32 = 0.0;

                    for (i, (&glyph_id, &c)) in
                        glyph_indices.iter().zip(text_chars.iter()).enumerate()
                    {
                        let advance = if i < glyph_metrics.len() {
                            glyph_metrics[i].advance_width as f32 * scale
                        } else {
                            size * 0.5
                        };

                        glyphs.push(PositionedGlyph {
                            glyph_id,
                            x: x_offset,
                            y: 0.0,
                            advance,
                            character: c,
                            cluster: i as u32,
                        });

                        x_offset += advance;
                    }

                    let ascent = design_metrics.ascent as f32 * scale;
                    let descent = design_metrics.descent as f32 * scale;
                    let leading = design_metrics.line_gap as f32 * scale;

                    let metrics = TextMetrics {
                        width: x_offset,
                        height: ascent + descent + leading,
                        ascent,
                        descent,
                        leading,
                        underline_offset: design_metrics.underline_position as f32 * scale,
                        underline_thickness: design_metrics.underline_thickness as f32 * scale,
                        strikethrough_offset: design_metrics.strikethrough_position as f32 * scale,
                        strikethrough_thickness: design_metrics.strikethrough_thickness as f32
                            * scale,
                        overline_offset: -ascent,
                    };

                    return Ok(ShapedRun {
                        text: text.to_string(),
                        glyphs,
                        font_family: font_family_name,
                        font_weight: weight,
                        font_style: style,
                        font_stretch: stretch,
                        font_size: size,
                        metrics,
                        direction: TextDirection::Ltr,
                    });
                }
            }
        }

        // Fallback to simple shaping
        self.shape_simple(text, font_chain, weight, style, stretch, size)
    }

    /// Simple shaping fallback when DirectWrite is unavailable.
    #[cfg(windows)]
    fn shape_simple(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<ShapedRun, TextError> {
        let avg_char_width = size * 0.5;
        let mut glyphs = Vec::with_capacity(text.len());
        let mut x_offset: f32 = 0.0;

        for (i, c) in text.chars().enumerate() {
            let advance = if c.is_ascii() {
                avg_char_width
            } else {
                size // CJK and other wide characters
            };

            glyphs.push(PositionedGlyph {
                glyph_id: c as u16,
                x: x_offset,
                y: 0.0,
                advance,
                character: c,
                cluster: i as u32,
            });

            x_offset += advance;
        }

        let metrics = TextMetrics {
            width: x_offset,
            ..TextMetrics::with_font_size(size)
        };

        Ok(ShapedRun {
            text: text.to_string(),
            glyphs,
            font_family: font_chain.primary.clone(),
            font_weight: weight,
            font_style: style,
            font_stretch: stretch,
            font_size: size,
            metrics,
            direction: TextDirection::Ltr,
        })
    }

    /// Shape text using Core Text on macOS.
    ///
    /// Results are memoised per thread. Layout shapes the same strings over
    /// and over: every flex/grid measuring pass re-wraps its text, and each
    /// wrap probes a prefix per break opportunity. On cnn, line wrapping was
    /// 29% of the main thread, spread over thousands of small repeated
    /// shapes. The answer depends only on the arguments and on the installed
    /// `@font-face` set, so entries are dropped whenever that set changes
    /// (the rule `create_ct_font_with_traits` uses).
    #[cfg(target_os = "macos")]
    pub fn shape(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<ShapedRun, TextError> {
        use std::cell::RefCell;

        type Key = (String, String, Vec<String>, u16, u8, u8, u32);
        struct Memo {
            generation: u64,
            runs: HashMap<Key, ShapedRun>,
        }
        // Bounds memory on a text-heavy page; refilling costs one shape per
        // entry, which is what every call cost before the memo.
        const MAX_ENTRIES: usize = 16384;
        thread_local! {
            static MEMO: RefCell<Memo> = RefCell::new(Memo {
                generation: 0,
                runs: HashMap::new(),
            });
        }

        let generation = rustkit_text::webfonts::generation();
        let key: Key = (
            text.to_string(),
            font_chain.primary.clone(),
            font_chain.fallbacks.clone(),
            weight.0,
            style as u8,
            stretch as u8,
            size.to_bits(),
        );
        let cached = MEMO.with(|m| {
            let mut m = m.borrow_mut();
            if m.generation != generation {
                m.runs.clear();
                m.generation = generation;
            }
            m.runs.get(&key).cloned()
        });
        if let Some(run) = cached {
            return Ok(run);
        }
        let run = self.shape_uncached(text, font_chain, weight, style, stretch, size)?;
        MEMO.with(|m| {
            let mut m = m.borrow_mut();
            if m.runs.len() >= MAX_ENTRIES {
                m.runs.clear();
            }
            m.runs.insert(key, run.clone());
        });
        Ok(run)
    }

    /// Shape text using Core Text on macOS (uncached; see `shape`).
    #[cfg(target_os = "macos")]
    fn shape_uncached(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<ShapedRun, TextError> {
        #[cfg(test)]
        font_resolve_tests::SHAPES.with(|n| n.set(n.get() + 1));

        if text.is_empty() {
            return Ok(ShapedRun {
                text: String::new(),
                glyphs: Vec::new(),
                font_family: font_chain.primary.clone(),
                font_weight: weight,
                font_style: style,
                font_stretch: stretch,
                font_size: size,
                metrics: TextMetrics::with_font_size(size),
                direction: TextDirection::Ltr,
            });
        }

        // Try to find a font from the chain
        let mut ct_font_opt: Option<core_text::font::CTFont> = None;
        let mut used_family = font_chain.primary.clone();

        for family in font_chain.all_families() {
            // Try to create font with traits
            if let Ok(font) =
                Self::create_ct_font_with_traits(family, size, weight.0, style == FontStyle::Italic)
            {
                ct_font_opt = Some(font);
                used_family = family.to_string();
                break;
            }
        }

        // Fallback to system font if nothing found
        let ct_font = ct_font_opt.unwrap_or_else(|| {
            ct_font::new_from_name("Helvetica", size as f64).unwrap_or_else(|_| {
                ct_font::new_from_name(".AppleSystemUIFont", size as f64).unwrap()
            })
        });

        // Convert text to UTF-16 for Core Text
        let utf16_chars: Vec<u16> = text.encode_utf16().collect();
        let char_count = utf16_chars.len();

        // Get glyph IDs
        let mut glyph_ids: Vec<u16> = vec![0; char_count];

        unsafe {
            extern "C" {
                fn CTFontGetGlyphsForCharacters(
                    font: core_text::font::CTFontRef,
                    characters: *const u16,
                    glyphs: *mut u16,
                    count: isize,
                ) -> bool;

                fn CTFontGetAdvancesForGlyphs(
                    font: core_text::font::CTFontRef,
                    orientation: u32,
                    glyphs: *const u16,
                    advances: *mut CGSize,
                    count: isize,
                ) -> f64;
            }

            let _success = CTFontGetGlyphsForCharacters(
                ct_font.as_concrete_TypeRef(),
                utf16_chars.as_ptr(),
                glyph_ids.as_mut_ptr(),
                char_count as isize,
            );

            // Get advances for each glyph
            let mut glyph_advances: Vec<CGSize> = vec![CGSize::new(0.0, 0.0); char_count];
            let _total_advance = CTFontGetAdvancesForGlyphs(
                ct_font.as_concrete_TypeRef(),
                0, // kCTFontOrientationHorizontal
                glyph_ids.as_ptr(),
                glyph_advances.as_mut_ptr(),
                char_count as isize,
            );

            // Pair kerning, which CTFontGetAdvancesForGlyphs leaves out.
            let kern = Self::kerning_deltas(&ct_font, text, &glyph_advances, size);

            // Build positioned glyphs
            let text_chars: Vec<char> = text.chars().collect();
            let mut glyphs = Vec::with_capacity(text_chars.len());
            let mut x_offset: f32 = 0.0;

            // Handle surrogate pairs - UTF-16 index to char index mapping
            let mut char_idx = 0;
            let mut utf16_idx = 0;

            // Fallback faces this run actually used, lazily created, and
            // the union of their extents (ascent, descent, leading).
            let mut fallback_fonts: Vec<(&'static str, Option<core_text::font::CTFont>)> =
                Vec::new();
            let mut used_fallback_extents: Option<(f32, f32, f32)> = None;

            while utf16_idx < char_count && char_idx < text_chars.len() {
                let c = text_chars[char_idx];
                let advance = glyph_advances[utf16_idx].width as f32;

                // A character the chosen face has no glyph for is shaped by
                // the SAME fallback face paint will draw it with (rustkit-text
                // `GLYPH_FALLBACK_FAMILIES`): its real advance, and its face's
                // extents folded into the run's — Blink unites every used
                // fallback face into the line box under `line-height: normal`
                // (NGInlineBoxState::AccumulateUsedFonts). Before: the
                // primary face's .notdef advance and extents, so "☕ coffee"
                // drew the emoji over the "c" and the line stayed 18px
                // (Chrome 26). Glyph 0 is .notdef and CARRIES an advance —
                // 15.69px on SF at 16px — so the miss is the id, never a
                // zero advance.
                let notdef_advance = if advance == 0.0 { size * 0.5 } else { advance };
                let final_advance = if glyph_ids[utf16_idx] == 0 {
                    if c.is_whitespace() || c.is_control() {
                        notdef_advance
                    } else {
                        match Self::fallback_glyph_advance(c, size, &mut fallback_fonts) {
                            Some((adv, asc, desc, lead)) => {
                                used_fallback_extents = Some(match used_fallback_extents {
                                    Some((a, d, l)) => (a.max(asc), d.max(desc), l.max(lead)),
                                    None => (asc, desc, lead),
                                });
                                adv
                            }
                            // Variation selectors / joiners are zero-width
                            // wherever they land (they modify a neighbour).
                            None if is_default_ignorable(c) => 0.0,
                            None => notdef_advance, // tofu
                        }
                    }
                } else {
                    advance + kern.get(utf16_idx).copied().unwrap_or(0.0)
                };

                glyphs.push(PositionedGlyph {
                    glyph_id: glyph_ids[utf16_idx],
                    x: x_offset,
                    y: 0.0,
                    advance: final_advance,
                    character: c,
                    cluster: char_idx as u32,
                });

                x_offset += final_advance;

                // Advance UTF-16 index (handle surrogate pairs)
                utf16_idx += c.len_utf16();
                char_idx += 1;
            }

            // Get font metrics from Core Text — united with the fallback
            // faces this run used (see `final_advance` above).
            let mut ascent = ct_font.ascent() as f32;
            let mut descent = ct_font.descent() as f32;
            let mut leading = ct_font.leading() as f32;
            if let Some((fb_ascent, fb_descent, fb_leading)) = used_fallback_extents {
                // Blink gives each face ITS OWN half-leading (half its rounded
                // line gap) before uniting, so a primary face's gap never
                // rides on top of a taller fallback face: Arial 16px
                // (14.48 + 3.39, gap 0.52) with an emoji (20 + 6.25, gap 0)
                // is max(14.5, 20) + max(3.5, 6.25) = 26 in Chrome, not 27.
                // The run then carries no gap of its own.
                let primary_half = leading.round() / 2.0;
                let fallback_half = fb_leading.round() / 2.0;
                ascent = (ascent + primary_half).max(fb_ascent + fallback_half);
                descent = (descent + primary_half).max(fb_descent + fallback_half);
                leading = 0.0;
            }
            let underline_position = ct_font.underline_position() as f32;
            let underline_thickness = ct_font.underline_thickness() as f32;

            // Calculate strikethrough position (approximately middle of x-height)
            let x_height = ct_font.x_height() as f32;
            let strikethrough_offset = x_height * 0.5;

            let metrics = TextMetrics {
                width: x_offset,
                height: ascent + descent + leading,
                ascent,
                descent,
                leading,
                underline_offset: underline_position,
                underline_thickness,
                strikethrough_offset,
                strikethrough_thickness: underline_thickness,
                overline_offset: -ascent,
            };

            Ok(ShapedRun {
                text: text.to_string(),
                glyphs,
                font_family: used_family,
                font_weight: weight,
                font_style: style,
                font_stretch: stretch,
                font_size: size,
                metrics,
                direction: TextDirection::Ltr,
            })
        }
    }

    /// Per-UTF-16-unit kerning adjustments for `text` in `font`.
    ///
    /// `CTFontGetAdvancesForGlyphs` returns each glyph's nominal advance, so
    /// runs were measured (and, through the advance contract, painted) with
    /// no pair kerning. Chrome kerns. "CSS Specificity Test" at 32px bold is
    /// 303.70px of nominal advances against 300.03px kerned, and every micro
    /// case's h1 drifted right by that much toward its last word.
    ///
    /// A CTLine of the same string in the same font, with ligatures off so
    /// glyphs stay one per unit, gives each unit's pen position. The delta
    /// for unit i is `pos(next unit) − pos(i) − nominal advance(i)`. A delta
    /// larger than a fifth of the size is not kerning (Core Text shaped that
    /// unit in another face), so it is dropped.
    #[cfg(target_os = "macos")]
    fn kerning_deltas(
        font: &core_text::font::CTFont,
        text: &str,
        nominal: &[CGSize],
        size: f32,
    ) -> Vec<f32> {
        use core_foundation::attributed_string::CFMutableAttributedString;
        use core_foundation::base::{CFRange, TCFType};
        use core_foundation::number::CFNumber;
        use core_foundation::string::CFString;
        use core_text::line::CTLine;
        use core_text::string_attributes::{kCTFontAttributeName, kCTLigatureAttributeName};

        let n = nominal.len();
        let mut deltas = vec![0.0f32; n];
        if n < 2 {
            return deltas;
        }
        let cf_text = CFString::new(text);
        let mut astr = CFMutableAttributedString::new();
        astr.replace_str(&cf_text, CFRange::init(0, 0));
        let len = astr.char_len();
        if len as usize != n {
            return deltas;
        }
        let range = CFRange::init(0, len);
        unsafe {
            astr.set_attribute(range, kCTFontAttributeName, font);
            astr.set_attribute(range, kCTLigatureAttributeName, &CFNumber::from(0i32));
        }
        let line = CTLine::new_with_attributed_string(astr.as_concrete_TypeRef());

        let mut pos: Vec<Option<f64>> = vec![None; n];
        for run in line.glyph_runs().iter() {
            let positions = run.positions();
            let indices = run.string_indices();
            for (p, &i) in positions.iter().zip(indices.iter()) {
                if let Some(slot) = pos.get_mut(i as usize) {
                    if slot.is_none() {
                        *slot = Some(p.x);
                    }
                }
            }
        }

        let limit = size * 0.2;
        let mut i = 0;
        while i < n {
            let Some(here) = pos[i] else {
                i += 1;
                continue;
            };
            let next = (i + 1..n).find(|&j| pos[j].is_some());
            if let Some(j) = next {
                let d = (pos[j].unwrap() - here - nominal[i].width) as f32;
                if d.abs() <= limit {
                    deltas[i] = d;
                }
                i = j;
            } else {
                break;
            }
        }
        deltas
    }

    /// Advance and face extents for a character the primary face lacks,
    /// from the first of rustkit-text's `GLYPH_FALLBACK_FAMILIES` that has
    /// a glyph for it. Faces are created once per run and kept in `fonts`.
    /// Returns `(advance, ascent, descent, leading)`.
    #[cfg(target_os = "macos")]
    fn fallback_glyph_advance(
        c: char,
        size: f32,
        fonts: &mut Vec<(&'static str, Option<core_text::font::CTFont>)>,
    ) -> Option<(f32, f32, f32, f32)> {
        extern "C" {
            fn CTFontGetGlyphsForCharacters(
                font: core_text::font::CTFontRef,
                characters: *const u16,
                glyphs: *mut u16,
                count: isize,
            ) -> bool;

            fn CTFontGetAdvancesForGlyphs(
                font: core_text::font::CTFontRef,
                orientation: u32,
                glyphs: *const u16,
                advances: *mut CGSize,
                count: isize,
            ) -> f64;
        }

        let mut units = [0u16; 2];
        let unit_count = c.encode_utf16(&mut units).len();

        for family in rustkit_text::macos::GLYPH_FALLBACK_FAMILIES {
            let slot = match fonts.iter().position(|(name, _)| name == family) {
                Some(i) => i,
                None => {
                    // Same lookup as the painter's `rasterize_fallback`, so
                    // measure and draw agree on the face.
                    fonts.push((family, ct_font::new_from_name(family, size as f64).ok()));
                    fonts.len() - 1
                }
            };
            let Some(font) = fonts[slot].1.as_ref() else {
                continue;
            };

            let mut glyph_ids = [0u16; 2];
            unsafe {
                // The bool is false when ANY unit lacks a glyph — a surrogate
                // pair's trailing unit always does — so read the first slot.
                let _ = CTFontGetGlyphsForCharacters(
                    font.as_concrete_TypeRef(),
                    units.as_ptr(),
                    glyph_ids.as_mut_ptr(),
                    unit_count as isize,
                );
                if glyph_ids[0] == 0 {
                    continue;
                }
                let mut advance = CGSize::new(0.0, 0.0);
                CTFontGetAdvancesForGlyphs(
                    font.as_concrete_TypeRef(),
                    0, // kCTFontOrientationHorizontal
                    glyph_ids.as_ptr(),
                    &mut advance,
                    1,
                );
                return Some((
                    advance.width as f32,
                    font.ascent() as f32,
                    font.descent() as f32,
                    font.leading() as f32,
                ));
            }
        }
        None
    }

    /// Create a Core Text font with specific traits, memoized.
    ///
    /// Every `shape` and `get_metrics` call resolves its font here, once per
    /// family in the chain until one hits. Resolution is Core Text
    /// descriptor matching plus up to eight name guesses for a family that
    /// doesn't exist, and nothing cached it: a text-heavy page (wikipedia,
    /// facebook) spent 6-10s per relayout re-resolving the same handful of
    /// fonts. The answer depends only on the arguments and on which
    /// `@font-face` set is installed, so results (misses too) are kept per
    /// thread and dropped whenever the installed set changes.
    #[cfg(target_os = "macos")]
    fn create_ct_font_with_traits(
        family: &str,
        size: f32,
        weight: u16,
        italic: bool,
    ) -> Result<core_text::font::CTFont, TextError> {
        use std::cell::RefCell;

        type Key = (String, u32, u16, bool);
        struct Resolved {
            generation: u64,
            fonts: HashMap<Key, Option<core_text::font::CTFont>>,
        }
        // Bounds memory on a page with unusually many sizes; refilling is
        // cheap next to resolving on every call.
        const MAX_ENTRIES: usize = 4096;
        thread_local! {
            static RESOLVED: RefCell<Resolved> = RefCell::new(Resolved {
                generation: 0,
                fonts: HashMap::new(),
            });
        }

        let generation = rustkit_text::webfonts::generation();
        let key: Key = (family.to_string(), size.to_bits(), weight, italic);
        let cached = RESOLVED.with(|r| {
            let mut r = r.borrow_mut();
            if r.generation != generation {
                r.fonts.clear();
                r.generation = generation;
            }
            r.fonts.get(&key).cloned()
        });
        let font = match cached {
            Some(font) => font,
            None => {
                let font = Self::resolve_ct_font_with_traits(family, size, weight, italic).ok();
                RESOLVED.with(|r| {
                    let mut r = r.borrow_mut();
                    if r.fonts.len() >= MAX_ENTRIES {
                        r.fonts.clear();
                    }
                    r.fonts.insert(key, font.clone());
                });
                font
            }
        };
        font.ok_or_else(|| TextError::FontNotFound(family.to_string()))
    }

    /// Resolve a Core Text font with specific traits (uncached; see
    /// `create_ct_font_with_traits`).
    #[cfg(target_os = "macos")]
    fn resolve_ct_font_with_traits(
        family: &str,
        size: f32,
        weight: u16,
        italic: bool,
    ) -> Result<core_text::font::CTFont, TextError> {
        #[cfg(test)]
        font_resolve_tests::RESOLUTIONS.with(|n| n.set(n.get() + 1));

        // A face the document registered via @font-face outranks every
        // platform lookup — the family may exist nowhere else. The engine
        // installs the current view's faces before each layout.
        if let Some(cg) = rustkit_text::webfonts::lookup(family, weight, italic) {
            return Ok(ct_font::new_from_CGFont(&cg, size as f64));
        }

        // The macOS system font has no by-name trait variants
        // (".AppleSystemUIFont-Bold" does not exist), so bold system-ui text
        // silently shaped with the REGULAR face — every bold heading
        // measured ~6% narrower than Chrome and re-centered off Chrome's x
        // (gradient-no-radius h1: ours 493.5px vs Chrome 529).
        //
        // The UI-font API alone exposes only TWO faces, so the original
        // `weight >= 600` gate merely moved the error: 100..500 all shaped as
        // .SFNS-Regular and 600..900 all as .SFNS-Bold. Chrome/Skia instead
        // apply kCTFontWeightTrait to the descriptor and get the real face.
        // `about`'s .tagline (font-weight:300, 20px) measured 670.0px as
        // Regular and wrapped inside its 672px block; Light is 659.1px and
        // fits on one line, as it does in Chrome.
        if family == ".AppleSystemUIFont" && !italic {
            return Ok(rustkit_text::macos::create_system_font_with_weight(
                size as f64,
                weight,
            ));
        }

        // The family's own face for this weight/style, chosen the way paint
        // (`GlyphRasterizer::with_style`) chooses it — ONE resolver for
        // measure and draw. Until n50 the list below led with the BARE
        // family name, so `named_font("Georgia")` answered first and every
        // `font-weight: 700` run on a named family was measured with the
        // regular face while paint drew Georgia-Bold at those advances:
        // article-typography's h1 read 443px of overlapping bold ink where
        // Chrome has 508 (every bold heading on every page using a named
        // family; italic went the same way through `-Italic`).
        if let Some(font) = rustkit_text::macos::family_face(family, size as f64, weight, italic) {
            return Ok(font);
        }

        // Name guesses for PostScript-name inputs the family lookup cannot
        // see ("HelveticaNeue-Light"): styled variants BEFORE the bare name.
        let mut variants_to_try = Vec::new();

        if weight >= 700 {
            variants_to_try.push(format!("{}-Bold", family));
            variants_to_try.push(format!("{}Bold", family));
            if italic {
                variants_to_try.push(format!("{}-BoldItalic", family));
                variants_to_try.push(format!("{}-BoldOblique", family));
            }
        }

        if italic {
            variants_to_try.push(format!("{}-Italic", family));
            variants_to_try.push(format!("{}-Oblique", family));
            variants_to_try.push(format!("{}Italic", family));
        }
        variants_to_try.push(family.to_string());

        // `CTFontCreateWithName` never fails: an uninstalled name comes back
        // as a substitute (Helvetica), so trusting `Ok` here stopped the
        // chain walk at the first MISSING family and MEASURED the substitute
        // while paint (which already walked, via `named_font`) drew the next
        // real family. Same accept/reject as paint: the face must be the
        // one asked for, or this family is a miss and the caller walks on.
        for variant in &variants_to_try {
            if let Some(font) = rustkit_text::macos::named_font(variant, size as f64) {
                return Ok(font);
            }
        }

        Err(TextError::FontNotFound(family.to_string()))
    }

    /// Simplified shaping fallback for non-Windows, non-macOS platforms.
    #[cfg(all(not(windows), not(target_os = "macos")))]
    pub fn shape(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<ShapedRun, TextError> {
        // Simplified shaping for other platforms
        let avg_char_width = size * 0.5;
        let mut glyphs = Vec::with_capacity(text.len());
        let mut x_offset: f32 = 0.0;

        for (i, c) in text.chars().enumerate() {
            let advance = if c.is_ascii() {
                avg_char_width
            } else {
                size // CJK characters are typically wider
            };

            glyphs.push(PositionedGlyph {
                glyph_id: c as u16,
                x: x_offset,
                y: 0.0,
                advance,
                character: c,
                cluster: i as u32,
            });

            x_offset += advance;
        }

        let metrics = TextMetrics {
            width: x_offset,
            ..TextMetrics::with_font_size(size)
        };

        Ok(ShapedRun {
            text: text.to_string(),
            glyphs,
            font_family: font_chain.primary.clone(),
            font_weight: weight,
            font_style: style,
            font_stretch: stretch,
            font_size: size,
            metrics,
            direction: TextDirection::Ltr,
        })
    }

    /// Measure text without full shaping (faster for layout).
    pub fn measure(
        &self,
        text: &str,
        font_family: &str,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
    ) -> Result<TextMetrics, TextError> {
        let chain = FontFamilyChain::from_css_value(font_family);
        let run = self.shape(text, &chain, weight, style, stretch, size)?;
        Ok(run.metrics)
    }

    /// Shape text with bidirectional text support.
    ///
    /// This function analyzes the text for bidirectional content (mixed LTR/RTL)
    /// using the Unicode Bidirectional Algorithm (UAX #9) and produces separate
    /// shaped runs for each directional segment in visual order.
    ///
    /// # Arguments
    /// * `text` - The text to shape
    /// * `font_chain` - Font family chain with fallbacks
    /// * `weight` - Font weight
    /// * `style` - Font style (normal, italic, oblique)
    /// * `stretch` - Font stretch
    /// * `size` - Font size in pixels
    /// * `base_direction` - Base paragraph direction (from CSS `direction` property)
    ///
    /// # Returns
    /// A vector of `ShapedRun`s in visual (display) order, each with its own direction.
    /// For pure LTR or RTL text, this returns a single run.
    pub fn shape_with_bidi(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        base_direction: Option<TextDirection>,
    ) -> Result<Vec<ShapedRun>, TextError> {
        if text.is_empty() {
            return Ok(vec![]);
        }

        // Convert to bidi direction for analysis
        let bidi_base = base_direction.map(|d| d.to_bidi());

        // Analyze bidirectional text
        let bidi_info = BidiInfo::with_base_direction(text, bidi_base);

        // Fast path: pure LTR or RTL text with single run
        let visual_runs = bidi_info.visual_runs();
        if visual_runs.len() == 1 && bidi_info.is_pure_ltr() {
            // Simple case: just shape the whole text as LTR
            let mut run = self.shape(text, font_chain, weight, style, stretch, size)?;
            run.direction = TextDirection::Ltr;
            return Ok(vec![run]);
        }

        // Handle mixed-direction text
        let mut shaped_runs = Vec::with_capacity(visual_runs.len());

        for bidi_run in visual_runs {
            let run_text = bidi_run.text(text);
            if run_text.is_empty() {
                continue;
            }

            // Shape this run
            let mut shaped = self.shape(run_text, font_chain, weight, style, stretch, size)?;
            shaped.direction = TextDirection::from_bidi(bidi_run.direction);

            // For RTL runs, we may need to reverse the glyph order
            // (depending on whether the underlying shaper already did this)
            // Note: Core Text and DirectWrite handle RTL internally,
            // so we typically don't need to reverse here.

            shaped_runs.push(shaped);
        }

        Ok(shaped_runs)
    }

    /// Shape text with bidirectional support using CSS direction property.
    ///
    /// Convenience wrapper around `shape_with_bidi` that takes a CSS direction value.
    pub fn shape_with_css_direction(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        css_direction: CssDirection,
    ) -> Result<Vec<ShapedRun>, TextError> {
        self.shape_with_bidi(
            text,
            font_chain,
            weight,
            style,
            stretch,
            size,
            Some(TextDirection::from_css(css_direction)),
        )
    }

    /// Wrap text into lines that fit within the specified width.
    ///
    /// This function shapes text and breaks it into multiple lines based on:
    /// - Available width
    /// - CSS word-break property
    /// - UAX #14 line breaking rules
    ///
    /// # Arguments
    /// * `text` - The text to wrap
    /// * `font_chain` - Font family chain with fallbacks
    /// * `weight` - Font weight
    /// * `style` - Font style
    /// * `stretch` - Font stretch
    /// * `size` - Font size in pixels
    /// * `max_width` - Maximum line width in pixels
    /// * `word_break` - CSS word-break property value
    /// * `overflow_wrap` - CSS overflow-wrap value (`line-break: anywhere`
    ///   also arrives here; see `rustkit_css::OverflowWrap`)
    ///
    /// # Returns
    /// A vector of `WrappedLine` structs, each containing shaped runs for one line.
    #[allow(clippy::too_many_arguments)]
    pub fn wrap_text(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        max_width: f32,
        word_break: CssWordBreak,
        overflow_wrap: CssOverflowWrap,
    ) -> Result<Vec<WrappedLine>, TextError> {
        self.wrap_text_with_first_line(
            text,
            font_chain,
            weight,
            style,
            stretch,
            size,
            max_width,
            max_width,
            word_break,
            overflow_wrap,
        )
    }

    /// Wrap text where the FIRST line has a different available width than
    /// the rest — the inline-formatting-context case: a run starting
    /// mid-line fills the remaining space of the current line box, then
    /// continues at the containing block's full width.
    ///
    /// If nothing fits on a narrower first line, the first line comes back
    /// EMPTY (the run starts on the next line box) instead of overflowing at
    /// the tail of a partially-filled line — css-text-3 §5.2 overflow only
    /// applies when a whole line box cannot take the word.
    #[allow(clippy::too_many_arguments)]
    pub fn wrap_text_with_first_line(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        first_line_max_width: f32,
        max_width: f32,
        word_break: CssWordBreak,
        overflow_wrap: CssOverflowWrap,
    ) -> Result<Vec<WrappedLine>, TextError> {
        // Legacy proxy: a narrower first line IS the mid-line signal here.
        self.wrap_text_lines(
            text,
            font_chain,
            weight,
            style,
            stretch,
            size,
            first_line_max_width,
            max_width,
            word_break,
            overflow_wrap,
            first_line_max_width < max_width,
            WhiteSpace::Normal,
        )
    }

    /// `wrap_text` for a run whose `white-space` is known. The wrapper's
    /// legacy entries assume collapsible white space: a space at a soft
    /// break is DROPPED so the next line starts on ink. Under
    /// `white-space: break-spaces` (css-text-3 §4.1.1) preserved spaces are
    /// content — they take up space, never hang, and a break before one
    /// leaves it at the START of the next line. Dropping it re-flowed every
    /// later line (WPT line-break-anywhere-005: `X XX` / ` XX ` / `X XX` /
    /// ` X` came out `X XX` / `XX X` / `XX X`).
    #[allow(clippy::too_many_arguments)]
    pub fn wrap_text_white_space(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        max_width: f32,
        word_break: CssWordBreak,
        overflow_wrap: CssOverflowWrap,
        white_space: WhiteSpace,
    ) -> Result<Vec<WrappedLine>, TextError> {
        self.wrap_text_lines(
            text,
            font_chain,
            weight,
            style,
            stretch,
            size,
            max_width,
            max_width,
            word_break,
            overflow_wrap,
            false,
            white_space,
        )
    }

    /// `wrap_text_mid_line` with the run's `white-space` (see
    /// `wrap_text_white_space`).
    #[allow(clippy::too_many_arguments)]
    pub fn wrap_text_mid_line_white_space(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        first_line_max_width: f32,
        max_width: f32,
        word_break: CssWordBreak,
        overflow_wrap: CssOverflowWrap,
        white_space: WhiteSpace,
    ) -> Result<Vec<WrappedLine>, TextError> {
        self.wrap_text_lines(
            text,
            font_chain,
            weight,
            style,
            stretch,
            size,
            first_line_max_width,
            max_width,
            word_break,
            overflow_wrap,
            true,
            white_space,
        )
    }

    /// `wrap_text_with_first_line` for a run that is KNOWN to start mid-line.
    ///
    /// The legacy entry infers "starts mid-line" from `first < max`, which is
    /// blind when the container is zero-wide: `first == max == 0`, so a run
    /// that cannot fit the remainder of a line it is not alone on used to
    /// glue its first grapheme onto that line (`xyz` + `d` on WPT
    /// break-boundary-2-chars-001) instead of starting on the next line box.
    /// Callers that know the cursor is past the line start say so explicitly.
    #[allow(clippy::too_many_arguments)]
    pub fn wrap_text_mid_line(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        first_line_max_width: f32,
        max_width: f32,
        word_break: CssWordBreak,
        overflow_wrap: CssOverflowWrap,
    ) -> Result<Vec<WrappedLine>, TextError> {
        self.wrap_text_lines(
            text,
            font_chain,
            weight,
            style,
            stretch,
            size,
            first_line_max_width,
            max_width,
            word_break,
            overflow_wrap,
            true,
            WhiteSpace::Normal,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn wrap_text_lines(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        first_line_max_width: f32,
        max_width: f32,
        word_break: CssWordBreak,
        overflow_wrap: CssOverflowWrap,
        starts_mid_line: bool,
        white_space: WhiteSpace,
    ) -> Result<Vec<WrappedLine>, TextError> {
        if text.is_empty() {
            return Ok(vec![]);
        }
        // Only break-spaces keeps a space that lands at a soft break: under
        // pre-wrap the trailing spaces HANG off the previous line (§4.1.3),
        // which dropping them from the next line already approximates.
        let preserve_spaces = matches!(white_space, WhiteSpace::BreakSpaces);

        // Convert CSS word-break to our line breaking enum
        let lb_word_break = match word_break {
            CssWordBreak::Normal => LineBreakWordBreak::Normal,
            CssWordBreak::BreakAll => LineBreakWordBreak::BreakAll,
            CssWordBreak::KeepAll => LineBreakWordBreak::KeepAll,
            CssWordBreak::BreakWord => LineBreakWordBreak::BreakWord,
        };
        // Was hardcoded to Normal: the breaker's break-word/anywhere arms
        // could never be reached from CSS.
        let lb_overflow_wrap = match overflow_wrap {
            CssOverflowWrap::Normal => OverflowWrap::Normal,
            CssOverflowWrap::BreakWord => OverflowWrap::BreakWord,
            CssOverflowWrap::Anywhere => OverflowWrap::Anywhere,
        };

        let breaker = LineBreaker::new(lb_word_break, lb_overflow_wrap);
        let mut lines = Vec::new();

        // First, handle mandatory line breaks
        for segment in rustkit_text::line_break::break_into_lines(text) {
            let segment_text = segment.text_without_break();
            if segment_text.is_empty() {
                // Empty line (just a line break)
                lines.push(WrappedLine {
                    runs: vec![],
                    width: 0.0,
                    start_offset: segment.start,
                    end_offset: segment.end,
                });
                continue;
            }

            // The narrower first-line width applies only to the very first
            // rendered line of the whole run.
            let seg_first_max = if lines.is_empty() {
                first_line_max_width
            } else {
                max_width
            };

            // Now wrap this segment within max_width
            let segment_lines = self.wrap_segment(
                segment_text,
                font_chain,
                weight,
                style,
                stretch,
                size,
                seg_first_max,
                max_width,
                &breaker,
                segment.start,
                starts_mid_line && lines.is_empty(),
                preserve_spaces,
            )?;

            lines.extend(segment_lines);
        }

        // Handle case where text has no mandatory breaks
        if lines.is_empty() && !text.is_empty() {
            lines = self.wrap_segment(
                text,
                font_chain,
                weight,
                style,
                stretch,
                size,
                first_line_max_width,
                max_width,
                &breaker,
                0,
                starts_mid_line,
                preserve_spaces,
            )?;
        }

        Ok(lines)
    }

    /// Internal helper to wrap a single segment (no mandatory breaks).
    /// `starts_mid_line`: the segment's first line begins at an inline
    /// cursor past the line start, so "nothing fits" means "start on the
    /// next line box", never "overflow the partially-filled line".
    #[allow(clippy::too_many_arguments)]
    fn wrap_segment(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        first_line_max_width: f32,
        max_width: f32,
        breaker: &LineBreaker,
        base_offset: usize,
        starts_mid_line: bool,
        preserve_spaces: bool,
    ) -> Result<Vec<WrappedLine>, TextError> {
        if text.is_empty() {
            return Ok(vec![]);
        }

        let mut lines = Vec::new();
        let mut line_start = 0;
        // Collapsible white space at a break point is consumed by the
        // break; a PRESERVED space (break-spaces) is content on the next
        // line and stays.
        let skip_break_spaces = |line_start: &mut usize| {
            if preserve_spaces {
                return;
            }
            while *line_start < text.len() && text[*line_start..].starts_with(is_collapsible_space)
            {
                *line_start += text[*line_start..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
            }
        };

        while line_start < text.len() {
            // The first rendered line may have a narrower budget (a run
            // starting mid-line fills the current line box's remainder).
            let cur_max = if lines.is_empty() {
                first_line_max_width
            } else {
                max_width
            };

            // Shape the remaining text to find where we need to break
            let remaining = &text[line_start..];
            let shaped = self.shape(remaining, font_chain, weight, style, stretch, size)?;

            if shaped.metrics.width <= cur_max {
                // Entire remaining text fits on one line
                let width = shaped.metrics.width;
                lines.push(WrappedLine {
                    runs: vec![shaped],
                    width,
                    start_offset: base_offset + line_start,
                    end_offset: base_offset + text.len(),
                });
                break;
            }

            // Need to find a break point
            // Binary search for the right break point
            let break_offset = self.find_line_break(
                remaining,
                font_chain,
                weight,
                style,
                stretch,
                size,
                cur_max,
                breaker,
                !preserve_spaces,
            )?;

            if break_offset == 0 {
                // Nothing fits on a first line that begins MID-LINE: start
                // the run on the next (full-width) line box instead of
                // overflowing a partially-filled line. `starts_mid_line`, not
                // `first < max`: at container width 0 both budgets are 0 and
                // the proxy went blind (WPT break-boundary-2-chars-001).
                if lines.is_empty() && starts_mid_line {
                    lines.push(WrappedLine {
                        runs: vec![],
                        width: 0.0,
                        start_offset: base_offset + line_start,
                        end_offset: base_offset + line_start,
                    });
                    continue;
                }
                // No break opportunity fits within max_width.
                let may_break_mid_word = breaker.allows_emergency_breaks()
                    || matches!(
                        breaker.word_break,
                        LineBreakWordBreak::BreakAll | LineBreakWordBreak::BreakWord
                    );
                let line_end = if may_break_mid_word {
                    // overflow-wrap: anywhere/break-word or word-break:
                    // break-all — emergency-break the word, taking as many
                    // graphemes as FIT the line (Chrome fills the line; it
                    // does not break after the first character). Minimum one
                    // grapheme so the loop always advances.
                    let boundaries =
                        rustkit_text::segmentation::grapheme_boundaries(remaining);
                    let mut fitted = 0usize;
                    for &offset in boundaries.iter().skip(1) {
                        let shaped_prefix = self.shape(
                            &remaining[..offset],
                            font_chain,
                            weight,
                            style,
                            stretch,
                            size,
                        )?;
                        if shaped_prefix.metrics.width <= cur_max {
                            fitted = offset;
                        } else {
                            break;
                        }
                    }
                    if fitted > 0 {
                        fitted
                    } else {
                        boundaries.get(1).copied().unwrap_or(remaining.len())
                    }
                } else {
                    // css-text-3 §5.2: when no break opportunity exists on the
                    // line, the unbreakable unit stays on it and OVERFLOWS —
                    // it is never broken mid-word. (Chrome behavior for
                    // word-break: normal/keep-all.) Take everything up to the
                    // next break opportunity as this line.
                    breaker
                        .find_break_after(remaining, 1)
                        .filter(|&o| o > 0)
                        .unwrap_or(remaining.len())
                        .min(remaining.len())
                };

                let line_text = &remaining[..line_end];
                let shaped_line =
                    self.shape(line_text, font_chain, weight, style, stretch, size)?;

                lines.push(WrappedLine {
                    width: line_ink_width(&shaped_line, !preserve_spaces),
                    runs: vec![shaped_line],
                    start_offset: base_offset + line_start,
                    end_offset: base_offset + line_start + line_end,
                });

                line_start += line_end;
                skip_break_spaces(&mut line_start);
            } else {
                let line_text = &remaining[..break_offset];
                let shaped_line =
                    self.shape(line_text, font_chain, weight, style, stretch, size)?;

                // A line closed by a soft break reports its INK width: the
                // collapsible space at the break point stays in the text
                // (paint skips it) but hangs off the line (§4.1.3), so it is
                // not part of the line's width — right/center alignment
                // and justification measure against the ink. The LAST line
                // (the fits-entirely arm above) keeps its trailing space:
                // it is live content between this run and the next sibling.
                lines.push(WrappedLine {
                    width: line_ink_width(&shaped_line, !preserve_spaces),
                    runs: vec![shaped_line],
                    start_offset: base_offset + line_start,
                    end_offset: base_offset + line_start + break_offset,
                });

                line_start += break_offset;
                skip_break_spaces(&mut line_start);
            }
        }

        Ok(lines)
    }

    /// Find the best line break point within max_width.
    fn find_line_break(
        &self,
        text: &str,
        font_chain: &FontFamilyChain,
        weight: FontWeight,
        style: FontStyle,
        stretch: FontStretch,
        size: f32,
        max_width: f32,
        breaker: &LineBreaker,
        hang_trailing_spaces: bool,
    ) -> Result<usize, TextError> {
        // Get all break opportunities
        let break_offsets = breaker.break_offsets(text);

        // Find the last break that fits
        let mut best_break = 0;

        for &offset in &break_offsets {
            if offset == 0 {
                continue;
            }

            // css-text-3 §4.1.3: collapsible spaces at the end of a line are
            // removed before the line is measured — they HANG past the edge
            // and never decide the break. Measuring the prefix with its
            // break-point space made every line whose ink fits but whose
            // ink + space does not break one word early (n49: a 300px Georgia
            // line "…and the official" wrapped "official" where Chrome fits
            // it), and the trailing-space width was silently added to the
            // slack of every justified line. `break-spaces` is the one value
            // whose spaces never hang (§4.1.3): they are measured.
            let prefix = if hang_trailing_spaces {
                text[..offset].trim_end_matches(is_collapsible_space)
            } else {
                &text[..offset]
            };
            // A space-only prefix is a line of hanging spaces: zero ink, it
            // always fits (pre-wrap " XXXXX" in 5ch breaks after the leading
            // space — WPT overflow-wrap-anywhere-004/005 — the space-only
            // first line is the break, not a skipped opportunity).
            let fits = if prefix.is_empty() {
                true
            } else {
                self.shape(prefix, font_chain, weight, style, stretch, size)?
                    .metrics
                    .width
                    <= max_width
            };

            if fits {
                best_break = offset;
            } else {
                break;
            }
        }

        Ok(best_break)
    }
}

/// The width of a closed line's ink: the run's width minus the advances of
/// the collapsible spaces that hang off its end (css-text-3 §4.1.3). With
/// `hang == false` (break-spaces) every space is measured.
fn line_ink_width(run: &ShapedRun, hang: bool) -> f32 {
    if !hang {
        return run.metrics.width;
    }
    let hanging: f32 = run
        .glyphs
        .iter()
        .rev()
        .take_while(|g| is_collapsible_space(g.character))
        .map(|g| g.advance)
        .sum();
    (run.metrics.width - hanging).max(0.0)
}

/// css-text-3 §4.1: the white space that COLLAPSES (and is removed at a
/// line's edges) is document white space — space, tab, and the line-ending
/// characters. `char::is_whitespace` also says yes to U+00A0 NO-BREAK SPACE,
/// which is a rendered, non-collapsible character: skipping it at a break
/// point deleted it from the next line (WPT line-break-anywhere-006:
/// "XXXX&nbsp;XXXX X X" lost its nbsp and re-flowed every later line).
fn is_collapsible_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c')
}

/// Default-ignorable code points that modify a neighbouring character and
/// take no space of their own when no face maps them: variation selectors
/// (U+FE00–FE0F, U+E0100–E01EF), ZWSP/ZWNJ/ZWJ, word joiner, BOM.
fn is_default_ignorable(c: char) -> bool {
    matches!(
        c,
        '\u{200B}'..='\u{200D}'
            | '\u{2060}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{E0100}'..='\u{E01EF}'
    )
}

/// A wrapped line of text.
#[derive(Debug, Clone)]
pub struct WrappedLine {
    /// Shaped runs for this line.
    pub runs: Vec<ShapedRun>,
    /// Total width of this line.
    pub width: f32,
    /// Start byte offset in the original text.
    pub start_offset: usize,
    /// End byte offset in the original text.
    pub end_offset: usize,
}

impl WrappedLine {
    /// Get the height of this line (max height of all runs).
    pub fn height(&self) -> f32 {
        self.runs
            .iter()
            .map(|r| r.metrics.height)
            .fold(0.0f32, f32::max)
    }

    /// Get the ascent of this line (max ascent of all runs).
    pub fn ascent(&self) -> f32 {
        self.runs
            .iter()
            .map(|r| r.metrics.ascent)
            .fold(0.0f32, f32::max)
    }

    /// Get the descent of this line (max descent of all runs).
    pub fn descent(&self) -> f32 {
        self.runs
            .iter()
            .map(|r| r.metrics.descent)
            .fold(0.0f32, f32::max)
    }

    /// Check if this line is empty.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty() || self.runs.iter().all(|r| r.glyphs.is_empty())
    }

    /// Get the text content of this line.
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text.as_str()).collect()
    }
}

impl Default for TextShaper {
    fn default() -> Self {
        Self::new()
    }
}

/// @font-face rule representation.
#[derive(Debug, Clone)]
pub struct FontFaceRule {
    /// Font family name to register.
    pub family: String,
    /// Font source URL.
    pub src: String,
    /// Font weight (defaults to normal).
    pub weight: FontWeight,
    /// Font style (defaults to normal).
    pub style: FontStyle,
    /// Font stretch (defaults to normal).
    pub stretch: FontStretch,
    /// Unicode range to support.
    pub unicode_range: Option<String>,
    /// Font display strategy.
    pub display: FontDisplay,
}

/// Font display strategies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontDisplay {
    /// Block period: 3s, swap period: infinite.
    #[default]
    Auto,
    /// Block period: short, swap period: infinite.
    Block,
    /// Block period: none, swap period: infinite.
    Swap,
    /// Block period: very short, swap period: short.
    Fallback,
    /// Block period: very short, swap period: none.
    Optional,
}

/// Font loader for @font-face rules.
/// The cache partition a font load belongs to.
///
/// PRIVACY PIN (Prometheus, 2026-08-08): the font cache is partitioned by
/// top-level site FROM DAY ONE. A shared font cache is a known cross-site
/// timing side channel -- site B can detect that site A loaded a font by
/// timing its own load -- and Chromium, Safari and Firefox all partition for
/// this reason. For a browser whose pitch is privacy-first, an unpartitioned
/// cache would be a privacy regression sold as a performance win.
///
/// DEVIATION FROM THE PIN, STATED: the pin says eTLD+1. Deriving eTLD+1
/// correctly needs the Public Suffix List, which is not a dependency of this
/// workspace, and adding one was not authorized by the pin. This type keys on
/// the HOST instead, which is STRICTLY MORE RESTRICTIVE: `a.example.com` and
/// `b.example.com` get separate partitions where eTLD+1 would share one. That
/// direction is the safe one to be wrong in -- loosening later is a
/// deliberate, reviewable change, whereas tightening later would mean shipping
/// a leak in the interim. Upgrade path: swap the body of `from_host` for a PSL
/// lookup; every call site already passes the context.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TopLevelSite(String);

impl TopLevelSite {
    /// Build a partition key from the top-level document's host.
    ///
    /// Hosts are compared case-insensitively (DNS is case-insensitive, and
    /// `EXAMPLE.com` must not get its own partition -- that would be a cache
    /// miss, not a security boundary).
    pub fn from_host(host: &str) -> Self {
        Self(host.trim().to_ascii_lowercase())
    }

    /// The partition used when no top-level document is available (about:
    /// pages, the built-in UI). Deliberately its own bucket rather than a
    /// shared default, so built-in pages can never be used as a cross-site
    /// oracle.
    pub fn opaque() -> Self {
        Self(String::from("\u{0}opaque"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Identity of a cached font face WITHIN a partition.
///
/// Face identity is the full (family, weight, style, stretch, src) tuple, not
/// just the family: one family routinely ships as many files, and keying on
/// family alone would serve the regular weight where bold was asked for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FontCacheKey {
    pub partition: TopLevelSite,
    pub family: String,
    pub weight: u16,
    /// Style/stretch stored as their debug discriminant strings: the CSS enums
    /// are not `Hash` in their own crate, and widening a shared type to satisfy
    /// one struct's derive is the wrong direction of change.
    pub style: String,
    pub stretch: String,
    pub src: String,
}

impl FontCacheKey {
    pub fn new(partition: TopLevelSite, rule: &FontFaceRule) -> Self {
        Self {
            partition,
            family: rule.family.clone(),
            weight: rule.weight.0,
            style: format!("{:?}", rule.style),
            stretch: format!("{:?}", rule.stretch),
            src: rule.src.clone(),
        }
    }
}

pub struct FontLoader {
    /// Loaded font faces, keyed by (partition, face identity).
    loaded: RwLock<HashMap<FontCacheKey, LoadedFont>>,
    /// Faces whose source could not be fetched or read. Remembered so a
    /// relayout does not retry (and re-log) the same dead URL every frame.
    failed: RwLock<std::collections::HashSet<FontCacheKey>>,
    /// Queued loads, each carrying the partition it was requested in.
    pending: RwLock<Vec<(FontCacheKey, FontFaceRule)>>,
}

/// The bytes of one fetched face plus the descriptors its rule declared.
/// The bytes are shared, not copied, into the platform registry on install.
struct LoadedFont {
    family: String,
    weight: u16,
    italic: bool,
    data: std::sync::Arc<Vec<u8>>,
}

impl FontLoader {
    /// Create a new font loader.
    pub fn new() -> Self {
        Self {
            loaded: RwLock::new(HashMap::new()),
            failed: RwLock::new(std::collections::HashSet::new()),
            pending: RwLock::new(Vec::new()),
        }
    }

    /// Queue a `@font-face` rule for loading in a given partition.
    ///
    /// The partition is taken HERE, at queue time, rather than being resolved
    /// later: the pin requires partition context on every queue/load/lookup
    /// from day one, precisely so that no call site can be written without it
    /// and then need retrofitting once callers exist.
    pub fn queue_font_face(&self, partition: TopLevelSite, rule: FontFaceRule) {
        let key = FontCacheKey::new(partition, &rule);
        let mut pending = self.pending.write().unwrap();
        pending.push((key, rule));
    }

    /// Number of queued loads. Test/observability surface.
    pub fn pending_count(&self) -> usize {
        self.pending.read().unwrap().len()
    }

    /// Hand the queued rules to whoever owns the network. The loader has no
    /// fetch path of its own — the engine resolves and fetches, then calls
    /// [`insert_loaded`](Self::insert_loaded) with the bytes.
    pub fn take_pending(&self) -> Vec<(FontCacheKey, FontFaceRule)> {
        let mut pending = self.pending.write().unwrap();
        std::mem::take(&mut *pending)
    }

    /// Record a fetched face. The family/weight/style come from the key, so
    /// the bytes can never be filed under a different identity than the rule
    /// that asked for them.
    pub fn insert_loaded(&self, key: FontCacheKey, data: Vec<u8>) {
        let entry = LoadedFont {
            family: key.family.clone(),
            weight: key.weight,
            italic: key.style != "Normal",
            data: std::sync::Arc::new(data),
        };
        self.failed.write().unwrap().remove(&key);
        self.loaded.write().unwrap().insert(key, entry);
    }

    /// Record that this face's source is dead, so relayouts stop retrying it.
    pub fn mark_failed(&self, key: FontCacheKey) {
        self.failed.write().unwrap().insert(key);
    }

    pub fn is_failed(&self, key: &FontCacheKey) -> bool {
        self.failed.read().unwrap().contains(key)
    }

    /// Is this exact face (partition + identity) loaded?
    pub fn is_loaded_key(&self, key: &FontCacheKey) -> bool {
        self.loaded.read().unwrap().contains_key(key)
    }

    /// Every loaded face of ONE partition, in a deterministic order, ready
    /// for the platform registry. Only the requested partition's faces are
    /// ever returned — this is the slice the engine installs before laying
    /// out a view of that site, and it is the only way faces leave here.
    pub fn faces_for(&self, partition: &TopLevelSite) -> Vec<rustkit_text::webfonts::WebFontFace> {
        let loaded = self.loaded.read().unwrap();
        let mut faces: Vec<(&FontCacheKey, &LoadedFont)> = loaded
            .iter()
            .filter(|(k, _)| k.partition == *partition)
            .collect();
        // HashMap order is arbitrary; a stable order keeps the installed set
        // (and its identity tag) the same across relayouts.
        faces.sort_by(|(a, _), (b, _)| {
            (&a.family, a.weight, &a.style, &a.stretch, &a.src)
                .cmp(&(&b.family, b.weight, &b.style, &b.stretch, &b.src))
        });
        faces
            .into_iter()
            .map(|(_, f)| rustkit_text::webfonts::WebFontFace {
                family: f.family.clone(),
                weight: f.weight,
                italic: f.italic,
                data: f.data.clone(),
            })
            .collect()
    }

    /// Is this face loaded IN THIS PARTITION?
    ///
    /// The partition parameter is not optional and there is no unpartitioned
    /// variant, deliberately: a lookup that omits the partition IS the
    /// cross-site oracle this design exists to prevent, so the type system
    /// refuses to express one.
    pub fn is_loaded(&self, partition: &TopLevelSite, family: &str) -> bool {
        let loaded = self.loaded.read().unwrap();
        loaded
            .keys()
            .any(|k| k.partition == *partition && k.family == family)
    }
}

impl Default for FontLoader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_font_family_chain() {
        let chain = FontFamilyChain::new("Arial")
            .with_fallback("Helvetica")
            .with_fallback("sans-serif");

        let families: Vec<_> = chain.all_families().collect();
        assert_eq!(families, vec!["Arial", "Helvetica", "sans-serif"]);
    }

    #[test]
    fn test_font_family_chain_from_css() {
        let chain = FontFamilyChain::from_css_value("\"Roboto\", Arial, sans-serif");
        assert_eq!(chain.primary, "Roboto");
        assert!(chain.fallbacks.contains(&"Arial".to_string()));
    }

    #[test]
    fn test_generic_font_families() {
        let sans = FontFamilyChain::from_css_value("sans-serif");
        #[cfg(target_os = "macos")]
        assert_eq!(sans.primary, "SF Pro");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(sans.primary, "Segoe UI");

        let mono = FontFamilyChain::from_css_value("monospace");
        #[cfg(target_os = "macos")]
        assert_eq!(mono.primary, "Menlo");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(mono.primary, "Cascadia Code");

        // Test system-ui and vendor-prefixed variants
        let system = FontFamilyChain::from_css_value("system-ui");
        #[cfg(target_os = "macos")]
        assert_eq!(system.primary, ".AppleSystemUIFont");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(system.primary, "Segoe UI");

        let apple = FontFamilyChain::from_css_value("-apple-system");
        #[cfg(target_os = "macos")]
        assert_eq!(apple.primary, ".AppleSystemUIFont");
        #[cfg(not(target_os = "macos"))]
        assert_eq!(apple.primary, "Segoe UI");
    }

    #[test]
    fn test_text_transform() {
        assert_eq!(
            apply_text_transform("hello world", TextTransform::Uppercase),
            "HELLO WORLD"
        );
        assert_eq!(
            apply_text_transform("HELLO WORLD", TextTransform::Lowercase),
            "hello world"
        );
        assert_eq!(
            apply_text_transform("hello world", TextTransform::Capitalize),
            "Hello World"
        );
        assert_eq!(
            apply_text_transform("hello world", TextTransform::None),
            "hello world"
        );
    }

    #[test]
    fn test_collapse_whitespace() {
        assert_eq!(
            collapse_whitespace("hello   world", WhiteSpace::Normal),
            "hello world"
        );
        assert_eq!(
            collapse_whitespace("hello   world", WhiteSpace::Pre),
            "hello   world"
        );
        assert_eq!(
            collapse_whitespace("hello\n\nworld", WhiteSpace::PreLine),
            "hello\n\nworld"
        );
    }

    #[test]
    fn test_line_height() {
        let metrics = TextMetrics::with_font_size(16.0);

        let normal = LineHeight::Normal;
        assert_eq!(normal.compute(16.0, &metrics), metrics.height);

        let number = LineHeight::Number(1.5);
        assert_eq!(number.compute(16.0, &metrics), 24.0);

        let length = LineHeight::Length(20.0);
        assert_eq!(length.compute(16.0, &metrics), 20.0);
    }

    #[test]
    fn test_text_metrics() {
        let metrics = TextMetrics::with_font_size(16.0);
        assert!(metrics.ascent > 0.0);
        assert!(metrics.descent > 0.0);
        assert!(metrics.height > 0.0);
        assert!(metrics.underline_thickness > 0.0);
    }

    #[test]
    fn test_text_decoration() {
        let decoration = TextDecoration::from_style(
            TextDecorationLine::UNDERLINE,
            Some(Color::from_rgb(255, 0, 0)),
            TextDecorationStyle::Solid,
            Length::Auto,
            16.0,
        );

        assert!(decoration.has_decorations());
        assert!(decoration.lines.underline);
        assert!(!decoration.lines.line_through);
    }

    /// Layout must MEASURE with the face paint draws. T-RED before n50:
    /// the resolver tried the bare family before "-Bold", so a 700 run on
    /// Georgia shaped with the regular face (same width as 400) while paint
    /// drew Georgia-Bold — the probe's h1 read 443px of overlapping ink vs
    /// Chrome's 508 for the bold row and 440 for the regular one.
    #[cfg(target_os = "macos")]
    #[test]
    fn bold_named_family_shapes_with_its_bold_face() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::from_css_value("Georgia, 'Times New Roman', serif");
        let width = |w: FontWeight, s: FontStyle| {
            shaper
                .shape(
                    "The Art of Typography",
                    &chain,
                    w,
                    s,
                    FontStretch::Normal,
                    44.0,
                )
                .unwrap()
                .metrics
                .width
        };
        let regular = width(FontWeight::NORMAL, FontStyle::Normal);
        let bold = width(FontWeight::BOLD, FontStyle::Normal);
        let italic = width(FontWeight::NORMAL, FontStyle::Italic);
        assert!(
            bold > regular * 1.10,
            "bold {bold} must be the wider face, regular {regular}"
        );
        assert!(
            (italic - regular).abs() > 0.5,
            "italic {italic} must be its own face, regular {regular}"
        );
        // The chain's face, not a substitute, on both sides.
        let font = TextShaper::create_ct_font_with_traits("Georgia", 44.0, 700, false).unwrap();
        assert_eq!(font.postscript_name(), "Georgia-Bold");
    }

    #[test]
    fn test_text_shaper_creation() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.shape(
            "Hello",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
        );
        assert!(result.is_ok());
        let run = result.unwrap();
        assert_eq!(run.text, "Hello");
        assert!(!run.glyphs.is_empty());
    }

    #[test]
    fn nbsp_survives_a_break_point() {
        // U+00A0 is White_Space to `char::is_whitespace` but NOT collapsible
        // document white space (css-text §4.1): the break-point skip must not
        // eat it. WPT line-break-anywhere-006 wraps "XXXX&nbsp;XXXX X X" in a
        // 4ch box as "XXXX" / "&nbsp;XXX" / ...; skipping the nbsp gave
        // "XXXX" / "XXXX" and re-flowed every later line.
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::new("monospace");
        let cell = |s: &str| {
            shaper
                .shape(s, &chain, FontWeight::NORMAL, FontStyle::Normal, FontStretch::Normal, 16.0)
                .unwrap()
                .metrics
                .width
        };
        let four = cell("XXXX");
        assert!(four > 0.0 && cell("XXXX\u{a0}") > four + 0.5, "monospace nbsp has an advance");
        let lines = shaper
            .wrap_text(
                "XXXX\u{a0}XXXX X X",
                &chain,
                FontWeight::NORMAL,
                FontStyle::Normal,
                FontStretch::Normal,
                16.0,
                four + 0.5,
                CssWordBreak::BreakAll,
                CssOverflowWrap::Normal,
            )
            .unwrap();
        let texts: Vec<String> = lines
            .iter()
            .map(|l| l.runs.iter().map(|r| r.text.as_str()).collect::<String>())
            .collect();
        assert_eq!(texts.first().map(String::as_str), Some("XXXX"), "{texts:?}");
        assert!(
            texts.get(1).map_or(false, |t| t.starts_with('\u{a0}')),
            "the nbsp must start line 2, not vanish at the break: {texts:?}"
        );
    }

    fn face(family: &str, src: &str) -> FontFaceRule {
        FontFaceRule {
            family: family.to_string(),
            src: src.to_string(),
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
            stretch: FontStretch::Normal,
            unicode_range: None,
            display: FontDisplay::Swap,
        }
    }

    #[test]
    fn test_font_loader() {
        let loader = FontLoader::new();
        let site = TopLevelSite::from_host("example.com");
        assert!(!loader.is_loaded(&site, "TestFont"));
        loader.queue_font_face(site, face("TestFont", "url(test.woff2)"));
        assert_eq!(loader.pending_count(), 1);
        assert_eq!(loader.take_pending().len(), 1);
        assert_eq!(loader.pending_count(), 0, "take_pending drains the queue");
    }

    #[test]
    fn a_loaded_face_is_visible_only_in_its_own_partition() {
        // THE PRIVACY PROPERTY on the read side: the slice handed to the
        // platform registry for site A must never carry a face site B loaded.
        let loader = FontLoader::new();
        let a = TopLevelSite::from_host("a.test");
        let b = TopLevelSite::from_host("b.test");
        let key = FontCacheKey::new(a.clone(), &face("Inter", "/i.woff2"));
        assert!(!loader.is_loaded_key(&key));
        loader.insert_loaded(key.clone(), vec![1, 2, 3]);
        assert!(loader.is_loaded_key(&key));
        assert!(loader.is_loaded(&a, "Inter"));
        assert!(!loader.is_loaded(&b, "Inter"));
        let faces_a = loader.faces_for(&a);
        assert_eq!(faces_a.len(), 1);
        assert_eq!(faces_a[0].family, "Inter");
        assert_eq!(faces_a[0].weight, 400);
        assert!(!faces_a[0].italic);
        assert_eq!(*faces_a[0].data, vec![1, 2, 3]);
        assert!(loader.faces_for(&b).is_empty());
    }

    #[test]
    fn a_failed_face_stays_failed_until_it_loads() {
        let loader = FontLoader::new();
        let site = TopLevelSite::from_host("example.com");
        let key = FontCacheKey::new(site, &face("Dead", "/dead.ttf"));
        assert!(!loader.is_failed(&key));
        loader.mark_failed(key.clone());
        assert!(loader.is_failed(&key), "a dead source is remembered, not retried every relayout");
        loader.insert_loaded(key.clone(), vec![0]);
        assert!(!loader.is_failed(&key), "a later successful load clears the failure");
    }

    #[test]
    fn faces_for_is_deterministically_ordered() {
        // The engine tags the installed set by partition + count; if the
        // order wandered between calls the registry would re-parse every
        // font file on every relayout.
        let loader = FontLoader::new();
        let site = TopLevelSite::from_host("example.com");
        for name in ["Zeta", "Alpha", "Mid"] {
            loader.insert_loaded(FontCacheKey::new(site.clone(), &face(name, "/x.ttf")), vec![]);
        }
        let names: Vec<String> = loader.faces_for(&site).into_iter().map(|f| f.family).collect();
        assert_eq!(names, vec!["Alpha", "Mid", "Zeta"]);
    }

    #[test]
    fn the_same_face_queued_from_two_sites_is_two_entries() {
        // THE PRIVACY PROPERTY. If these collapsed to one entry, site B could
        // time its own load to learn that site A had already fetched the font
        // -- the cross-site oracle this partitioning exists to prevent.
        let loader = FontLoader::new();
        loader.queue_font_face(TopLevelSite::from_host("a.test"), face("Inter", "/i.woff2"));
        loader.queue_font_face(TopLevelSite::from_host("b.test"), face("Inter", "/i.woff2"));
        assert_eq!(
            loader.pending_count(),
            2,
            "identical faces from different sites must not share a cache slot"
        );
    }

    #[test]
    fn a_partition_key_is_host_case_insensitive() {
        // DNS is case-insensitive, so EXAMPLE.com is the SAME site. Treating
        // it as a separate partition would be a cache miss wearing the costume
        // of a security boundary.
        assert_eq!(
            TopLevelSite::from_host("EXAMPLE.com"),
            TopLevelSite::from_host("example.com")
        );
    }

    #[test]
    fn subdomains_get_separate_partitions() {
        // Documents the DEVIATION from the eTLD+1 pin: host-keying is STRICTLY
        // TIGHTER. When this upgrades to a Public Suffix List lookup, THIS test
        // is the one that must change, and changing it is a deliberate
        // loosening rather than an accident.
        assert_ne!(
            TopLevelSite::from_host("a.example.com"),
            TopLevelSite::from_host("b.example.com")
        );
    }

    #[test]
    fn the_opaque_partition_is_nobody_elses() {
        // Built-in pages must never be usable as a cross-site oracle, so they
        // get their own bucket rather than a shared default.
        let opaque = TopLevelSite::opaque();
        assert_ne!(opaque, TopLevelSite::from_host(""));
        assert_ne!(opaque, TopLevelSite::from_host("opaque"));
    }

    #[test]
    fn face_identity_includes_more_than_the_family() {
        // One family ships as many files. Keying on family alone would serve
        // the regular weight where bold was asked for.
        let site = TopLevelSite::from_host("example.com");
        let regular = face("Inter", "/inter-regular.woff2");
        let mut bold = face("Inter", "/inter-bold.woff2");
        bold.weight = FontWeight::BOLD;
        assert_ne!(
            FontCacheKey::new(site.clone(), &regular),
            FontCacheKey::new(site, &bold)
        );
    }

    #[test]
    fn test_empty_text_shaping() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.shape(
            "",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
        );
        assert!(result.is_ok());
        let run = result.unwrap();
        assert!(run.glyphs.is_empty());
    }

    #[test]
    fn test_text_direction_conversions() {
        use rustkit_css::Direction as CssDirection;
        use rustkit_text::bidi::Direction as BidiDirection;

        // From CSS
        assert_eq!(
            TextDirection::from_css(CssDirection::Ltr),
            TextDirection::Ltr
        );
        assert_eq!(
            TextDirection::from_css(CssDirection::Rtl),
            TextDirection::Rtl
        );

        // From bidi
        assert_eq!(
            TextDirection::from_bidi(BidiDirection::Ltr),
            TextDirection::Ltr
        );
        assert_eq!(
            TextDirection::from_bidi(BidiDirection::Rtl),
            TextDirection::Rtl
        );

        // To bidi
        assert_eq!(TextDirection::Ltr.to_bidi(), BidiDirection::Ltr);
        assert_eq!(TextDirection::Rtl.to_bidi(), BidiDirection::Rtl);

        // Helper methods
        assert!(TextDirection::Ltr.is_ltr());
        assert!(!TextDirection::Ltr.is_rtl());
        assert!(TextDirection::Rtl.is_rtl());
        assert!(!TextDirection::Rtl.is_ltr());

        // Default
        assert_eq!(TextDirection::default(), TextDirection::Ltr);
    }

    #[test]
    fn test_shape_with_bidi_empty() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.shape_with_bidi(
            "",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            None,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_shape_with_bidi_ltr() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.shape_with_bidi(
            "Hello, world!",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            None,
        );
        assert!(result.is_ok());
        let runs = result.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].direction, TextDirection::Ltr);
        assert_eq!(runs[0].text, "Hello, world!");
    }

    #[test]
    fn test_shape_with_bidi_rtl() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        // Hebrew: "shalom" (שלום)
        let result = shaper.shape_with_bidi(
            "\u{05E9}\u{05DC}\u{05D5}\u{05DD}",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            None,
        );
        assert!(result.is_ok());
        let runs = result.unwrap();
        // Pure RTL text should produce a single RTL run
        assert!(!runs.is_empty());
    }

    #[test]
    fn test_shape_with_bidi_mixed() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        // Mixed: "Hello שלום world"
        let result = shaper.shape_with_bidi(
            "Hello \u{05E9}\u{05DC}\u{05D5}\u{05DD} world",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            None,
        );
        assert!(result.is_ok());
        let runs = result.unwrap();
        // Mixed text should produce multiple runs
        assert!(
            runs.len() >= 2,
            "Expected multiple runs for mixed text, got {}",
            runs.len()
        );
    }

    #[test]
    fn test_shape_with_css_direction() {
        use rustkit_css::Direction as CssDirection;

        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.shape_with_css_direction(
            "Hello",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            CssDirection::Ltr,
        );
        assert!(result.is_ok());
        let runs = result.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].direction, TextDirection::Ltr);
    }

    #[test]
    fn test_shaped_run_direction_field() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.shape(
            "Test",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
        );
        assert!(result.is_ok());
        let run = result.unwrap();
        // Default shape() should produce LTR direction
        assert_eq!(run.direction, TextDirection::Ltr);
    }

    #[test]
    fn test_wrap_text_empty() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.wrap_text(
            "",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            200.0,
            CssWordBreak::Normal,
            CssOverflowWrap::Normal,
        );
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    /// WPT line-break-anywhere-005: under `white-space: break-spaces` a
    /// preserved space at a soft break starts the next line (`X XX` /
    /// ` XX ` / `X XX` / ` X`); the collapsible-white-space rule that eats
    /// it re-flows every later line (`X XX` / `XX X` / `XX X`). Monospace so
    /// every 4-character line has the same advance; `break-all` stands in
    /// for `line-break: anywhere` (the layout crate maps it the same way).
    #[test]
    fn pre_wrap_leading_space_is_a_break_and_hangs_on_its_own_line() {
        // WPT overflow-wrap-anywhere-004: ` XXXXX ` in a 5ch pre-wrap box —
        // the leading space is a soft break opportunity and the word must
        // not be broken: line 1 is the space alone, line 2 is the word.
        // (n49's hanging-space fit test first skipped the space-only prefix
        // as "nothing to measure" and broke the word instead.)
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::monospace();
        let five = shaper
            .shape(
                "XXXXX",
                &chain,
                FontWeight::NORMAL,
                FontStyle::Normal,
                FontStretch::Normal,
                20.0,
            )
            .expect("shape")
            .metrics
            .width;
        let lines: Vec<String> = shaper
            .wrap_text_white_space(
                " XXXXX ",
                &chain,
                FontWeight::NORMAL,
                FontStyle::Normal,
                FontStretch::Normal,
                20.0,
                five * 1.01,
                CssWordBreak::Normal,
                CssOverflowWrap::Anywhere,
                rustkit_css::WhiteSpace::PreWrap,
            )
            .expect("wrap")
            .iter()
            .map(|l| l.text())
            .collect();
        assert_eq!(lines.len(), 2, "space line + word line, got {lines:?}");
        assert_eq!(
            lines[0].trim_end(),
            "",
            "line 1 is the hanging leading space: {lines:?}"
        );
        assert!(
            lines[1].starts_with("XXXXX"),
            "line 2 is the unbroken word: {lines:?}"
        );
    }

    #[test]
    fn test_wrap_break_spaces_keeps_the_space_at_a_soft_break() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::monospace();
        let four_chars = shaper
            .shape(
                "X XX",
                &chain,
                FontWeight::NORMAL,
                FontStyle::Normal,
                FontStretch::Normal,
                20.0,
            )
            .expect("shape")
            .metrics
            .width;
        let wrap = |white_space: rustkit_css::WhiteSpace| -> Vec<String> {
            shaper
                .wrap_text_white_space(
                    "X XX XX X XX X",
                    &chain,
                    FontWeight::NORMAL,
                    FontStyle::Normal,
                    FontStretch::Normal,
                    20.0,
                    four_chars * 1.01,
                    CssWordBreak::BreakAll,
                    CssOverflowWrap::Normal,
                    white_space,
                )
                .expect("wrap")
                .iter()
                .map(|l| l.text())
                .collect()
        };
        assert_eq!(
            wrap(rustkit_css::WhiteSpace::BreakSpaces),
            ["X XX", " XX ", "X XX", " X"]
        );
        // The collapsible default HANGS the space at the break (§4.1.3):
        // it stays in the line's text (paint skips it, consumers trim it)
        // but never decides the fit — the line's width is its ink. Before
        // n49 the fit test measured the space, so the break landed before
        // it and every line was one word short whenever ink fit and ink +
        // space did not.
        let normal = wrap(rustkit_css::WhiteSpace::Normal);
        assert_eq!(normal, ["X XX ", "XX X ", "XX X"]);
        let widths: Vec<f32> = shaper
            .wrap_text_white_space(
                "X XX XX X XX X",
                &chain,
                FontWeight::NORMAL,
                FontStyle::Normal,
                FontStretch::Normal,
                20.0,
                four_chars * 1.01,
                CssWordBreak::BreakAll,
                CssOverflowWrap::Normal,
                rustkit_css::WhiteSpace::Normal,
            )
            .expect("wrap")
            .iter()
            .map(|l| l.width)
            .collect();
        for (t, w) in normal.iter().zip(&widths) {
            assert!(
                *w <= four_chars * 1.01 + 0.01,
                "line {t:?} width {w} must be its ink (four chars = {four_chars})"
            );
        }
    }

    #[test]
    fn test_wrap_text_single_line() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.wrap_text(
            "Hello",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            1000.0, // Very wide, should fit on one line
            CssWordBreak::Normal,
            CssOverflowWrap::Normal,
        );
        assert!(result.is_ok());
        let lines = result.unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "Hello");
    }

    #[test]
    fn test_wrap_text_multiple_lines() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.wrap_text(
            "Hello world this is a test",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            80.0, // Narrow width to force wrapping
            CssWordBreak::Normal,
            CssOverflowWrap::Normal,
        );
        assert!(result.is_ok());
        let lines = result.unwrap();
        // Should have multiple lines due to narrow width
        assert!(
            lines.len() > 1,
            "Expected multiple lines, got {}",
            lines.len()
        );
    }

    #[test]
    fn test_wrap_text_with_newlines() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.wrap_text(
            "Line1\nLine2",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            1000.0,
            CssWordBreak::Normal,
            CssOverflowWrap::Normal,
        );
        assert!(result.is_ok());
        let lines = result.unwrap();
        // Should have at least 2 lines due to newline
        assert!(
            lines.len() >= 2,
            "Expected at least 2 lines for text with newline"
        );
    }

    #[test]
    fn test_wrap_text_break_all() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        // With break-all, should be able to break mid-word
        let result = shaper.wrap_text(
            "Supercalifragilisticexpialidocious",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            50.0, // Very narrow
            CssWordBreak::BreakAll,
            CssOverflowWrap::Normal,
        );
        assert!(result.is_ok());
        let lines = result.unwrap();
        // Should break the long word
        assert!(lines.len() > 1, "Expected word to be broken with break-all");
    }

    #[test]
    fn test_wrapped_line_properties() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let result = shaper.wrap_text(
            "Test",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            16.0,
            1000.0,
            CssWordBreak::Normal,
            CssOverflowWrap::Normal,
        );
        assert!(result.is_ok());
        let lines = result.unwrap();
        assert_eq!(lines.len(), 1);

        let line = &lines[0];
        assert!(line.width > 0.0);
        assert!(line.height() > 0.0);
        assert!(line.ascent() > 0.0);
        assert!(!line.is_empty());
        assert_eq!(line.start_offset, 0);
        assert_eq!(line.end_offset, 4);
    }
}

#[cfg(test)]
mod mid_line_zero_width_tests {
    use super::*;

    fn wrap(mid_line: bool) -> Vec<String> {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::sans_serif();
        let f = if mid_line {
            TextShaper::wrap_text_mid_line
        } else {
            TextShaper::wrap_text_with_first_line
        };
        f(
            &shaper,
            "def",
            &chain,
            FontWeight::NORMAL,
            FontStyle::Normal,
            FontStretch::Normal,
            32.0,
            0.0,
            0.0,
            CssWordBreak::BreakAll,
            CssOverflowWrap::Normal,
        )
        .expect("wraps")
        .iter()
        .map(|l| l.text())
        .collect()
    }

    #[test]
    fn a_mid_line_run_with_no_room_starts_on_the_next_line_box() {
        // WPT break-boundary-2-chars-001: `def` follows a `pre` span on a
        // zero-wide line. Nothing fits the remainder, so the run's first line
        // is EMPTY (closes the open line) and the graphemes go one per line.
        // The old `first < max` proxy saw 0 < 0 == false and glued `d` onto
        // the span's line.
        assert_eq!(wrap(true), ["", "d", "e", "f"]);
    }

    #[test]
    fn the_legacy_entry_keeps_its_first_lt_max_proxy() {
        // Callers that do not know the cursor position keep the old reading:
        // equal budgets mean "not mid-line", so no empty leading line.
        assert_eq!(wrap(false), ["d", "e", "f"]);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod measure_side_font_chain_tests {
    use super::*;

    fn width(chain_css: &str) -> f32 {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::from_css_value(chain_css);
        shaper
            .shape("0000", &chain, FontWeight::NORMAL, FontStyle::Normal, FontStretch::Normal, 16.0)
            .expect("shapes")
            .metrics
            .width
    }

    /// Core Text hands back a substitute for a name it does not have. Paint
    /// (`named_font`, #164) walks past it; MEASURE must too, or a page
    /// naming a missing family ahead of Menlo lays text out at Helvetica's
    /// advances and paints it in Menlo. T-RED: with `new_from_name` trusted
    /// as installed, the first width is Helvetica's "0000" (≈35.6px), not
    /// Menlo's (≈38.5px).
    #[test]
    fn measure_walks_past_an_uninstalled_family_like_paint_does() {
        let walked = width("No Such Face n34, Menlo");
        let menlo = width("Menlo");
        let helvetica = width("Helvetica");
        assert_ne!(menlo, helvetica, "probe fonts must differ for this test to discriminate");
        assert_eq!(walked, menlo, "missing family must be skipped at measure time");
    }

    /// Pair kerning reaches the advances. Chrome's h1 "CSS Specificity Test"
    /// (32px bold system-ui) is 300.03px kerned. The nominal advances sum to
    /// 303.70, which was every micro case's h1 drift before n64.
    #[test]
    fn runs_are_kerned_like_a_core_text_line() {
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::from_css_value("system-ui");
        let shape = |t: &str| {
            shaper
                .shape(t, &chain, FontWeight::BOLD, FontStyle::Normal, FontStretch::Normal, 32.0)
                .expect("shapes")
        };
        let run = shape("CSS Specificity Test");
        let nominal: f32 = "CSS Specificity Test"
            .chars()
            .map(|c| shape(&c.to_string()).metrics.width)
            .sum();
        assert!(
            (run.metrics.width - 300.03).abs() < 0.5,
            "kerned run {} (nominal {nominal})",
            run.metrics.width
        );
        assert!(nominal - run.metrics.width > 3.0);
        // Advances still sum to the run width, one per char (advance contract).
        let sum: f32 = run.glyphs.iter().map(|g| g.advance).sum();
        assert!((sum - run.metrics.width).abs() < 0.01);
        assert_eq!(run.glyphs.len(), "CSS Specificity Test".chars().count());
    }
}

#[cfg(all(test, target_os = "macos"))]
mod font_resolve_tests {
    use super::*;
    use std::cell::Cell;

    thread_local! {
        /// Uncached font resolutions on this thread.
        pub(super) static RESOLUTIONS: Cell<usize> = const { Cell::new(0) };
        /// Uncached shapes on this thread.
        pub(super) static SHAPES: Cell<usize> = const { Cell::new(0) };
    }

    #[test]
    fn rewrapping_the_same_text_shapes_nothing_new() {
        // Every flex measuring pass re-wraps its text. The second wrap of
        // the same paragraph at the same width must come from the memo and
        // give the same lines.
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::new("Helvetica");
        let text = (0..60).map(|i| format!("w{}rd", "o".repeat(i % 7))).collect::<Vec<_>>().join(" ");
        let wrap = || {
            shaper
                .wrap_text(
                    &text,
                    &chain,
                    FontWeight(400),
                    FontStyle::Normal,
                    FontStretch::Normal,
                    16.0,
                    300.0,
                    CssWordBreak::Normal,
                    CssOverflowWrap::Normal,
                )
                .unwrap()
        };
        let first = wrap();
        let before = SHAPES.with(Cell::get);
        let second = wrap();
        // 0; a few more if another test's web-font install bumps the
        // generation mid-wrap.
        let shaped = SHAPES.with(Cell::get) - before;
        assert!(shaped <= 8, "{shaped} new shapes re-wrapping the same text");
        assert!(first.len() > 3);
        let offsets = |lines: &[WrappedLine]| {
            lines
                .iter()
                .map(|l| (l.start_offset, l.end_offset, l.width.to_bits()))
                .collect::<Vec<_>>()
        };
        assert_eq!(offsets(&first), offsets(&second));
    }

    #[test]
    fn a_new_web_font_set_invalidates_shaped_runs() {
        let chain = FontFamilyChain::new("Helvetica");
        let shape = || {
            TextShaper::new()
                .shape("memo", &chain, FontWeight(400), FontStyle::Normal, FontStretch::Normal, 16.0)
                .unwrap()
        };
        // Other tests install web-font sets on their own threads, and the
        // generation is process-wide: only judge a hit when it held still.
        let mut judged = false;
        for _ in 0..20 {
            let generation = rustkit_text::webfonts::generation();
            shape();
            let before = SHAPES.with(Cell::get);
            shape();
            if rustkit_text::webfonts::generation() == generation {
                assert_eq!(SHAPES.with(Cell::get), before, "second shape is a hit");
                judged = true;
                break;
            }
        }
        assert!(judged, "the web-font generation never held still");
        let before = SHAPES.with(Cell::get);
        rustkit_text::webfonts::install(
            "shape-memo-test",
            &[rustkit_text::webfonts::WebFontFace {
                family: "ShapeMemoTestFace".into(),
                weight: 400,
                italic: false,
                data: std::sync::Arc::new(vec![0u8; 64]),
            }],
        );
        shape();
        assert!(SHAPES.with(Cell::get) > before, "re-shaped after the set changed");
    }

    #[test]
    fn a_font_is_resolved_once_not_once_per_shape() {
        // A missing first family (every site's web-font name the platform
        // lacks) and a real fallback: 200 shapes, 2 resolutions.
        let shaper = TextShaper::new();
        let chain = FontFamilyChain::new("NoSuchFamilyForTheResolveCache").with_fallback("Helvetica");
        let before = RESOLUTIONS.with(Cell::get);
        let mut widths = Vec::new();
        for _ in 0..200 {
            let run = shaper
                .shape("resolve me once", &chain, FontWeight(400), FontStyle::Normal, FontStretch::Normal, 16.0)
                .unwrap();
            widths.push(run.metrics.width);
        }
        // 2 per shape uncached (400). Cached: 2, plus 2 more if the other
        // test's install bumps the web-font generation mid-loop.
        let resolved = RESOLUTIONS.with(Cell::get) - before;
        assert!(resolved <= 4, "{resolved} resolutions for 200 identical shapes");
        assert!(widths.iter().all(|w| *w == widths[0] && *w > 0.0), "{:?}", &widths[..3]);
        assert_eq!(
            shaper
                .shape("x", &chain, FontWeight(400), FontStyle::Normal, FontStretch::Normal, 16.0)
                .unwrap()
                .font_family,
            "Helvetica"
        );
    }

    #[test]
    fn a_new_web_font_set_invalidates_the_cache() {
        let chain = FontFamilyChain::new("Helvetica");
        let shape = || {
            TextShaper::new()
                .shape("x", &chain, FontWeight(400), FontStyle::Normal, FontStretch::Normal, 16.0)
                .unwrap()
        };
        shape();
        let before = RESOLUTIONS.with(Cell::get);
        shape();
        assert_eq!(RESOLUTIONS.with(Cell::get), before, "second shape is a cache hit");
        rustkit_text::webfonts::install(
            "font-resolve-cache-test",
            &[rustkit_text::webfonts::WebFontFace {
                family: "FontResolveCacheTestFace".into(),
                weight: 400,
                italic: false,
                // Rejected by Core Graphics (as in rustkit-text's own
                // garbage-bytes test); the set still changed.
                data: std::sync::Arc::new(vec![0u8; 64]),
            }],
        );
        shape();
        assert_eq!(RESOLUTIONS.with(Cell::get), before + 1, "re-resolved after the set changed");
    }
}
