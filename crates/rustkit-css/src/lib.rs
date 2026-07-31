//! # RustKit CSS
//!
//! CSS parsing and style computation for the RustKit browser engine.
//!
//! ## Design Goals
//!
//! 1. **Property parsing**: Parse CSS property values
//! 2. **Cascade**: Apply specificity and origin rules
//! 3. **Inheritance**: Propagate inherited properties to children
//! 4. **Computed values**: Resolve relative units and keywords

use thiserror::Error;
use tracing::debug;
use rustkit_cssparser::parse_stylesheet;

/// Errors that can occur in CSS operations.
#[derive(Error, Debug)]
pub enum CssError {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Invalid value: {0}")]
    InvalidValue(String),
}

/// A CSS color value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: f32,
}

impl Color {
    pub const TRANSPARENT: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 0.0,
    };
    pub const BLACK: Color = Color {
        r: 0,
        g: 0,
        b: 0,
        a: 1.0,
    };
    pub const WHITE: Color = Color {
        r: 255,
        g: 255,
        b: 255,
        a: 1.0,
    };

    pub fn new(r: u8, g: u8, b: u8, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Convert to [f64; 4] for rendering.
    pub fn to_f64_array(&self) -> [f64; 4] {
        [
            self.r as f64 / 255.0,
            self.g as f64 / 255.0,
            self.b as f64 / 255.0,
            self.a as f64,
        ]
    }
}

impl Default for Color {
    fn default() -> Self {
        Self::BLACK
    }
}


/// High-precision color for internal rendering calculations.
/// RGB components are stored as f32 in 0.0-1.0 range.
/// Use for gradient interpolation and internal processing.
/// Convert to Color only at final display/storage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorF32 {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl ColorF32 {
    pub const TRANSPARENT: ColorF32 = ColorF32 { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };
    pub const BLACK: ColorF32 = ColorF32 { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
    pub const WHITE: ColorF32 = ColorF32 { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };

    #[inline]
    pub fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    #[inline]
    pub fn from_rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Convert from 8-bit Color to high-precision ColorF32.
    #[inline]
    pub fn from_color(c: Color) -> Self {
        Self {
            r: c.r as f32 / 255.0,
            g: c.g as f32 / 255.0,
            b: c.b as f32 / 255.0,
            a: c.a,
        }
    }

    /// Convert to 8-bit Color for final display.
    /// Uses rounding for best accuracy.
    #[inline]
    pub fn to_color(&self) -> Color {
        Color {
            r: (self.r * 255.0).round().clamp(0.0, 255.0) as u8,
            g: (self.g * 255.0).round().clamp(0.0, 255.0) as u8,
            b: (self.b * 255.0).round().clamp(0.0, 255.0) as u8,
            a: self.a,
        }
    }

    /// Convert to 8-bit Color with ordered dithering to reduce banding.
    /// `pixel_x` and `pixel_y` are the screen coordinates for dither pattern.
    #[inline]
    pub fn to_color_dithered(&self, pixel_x: u32, pixel_y: u32) -> Color {
        // 4x4 Bayer ordered dithering matrix (normalized to 0.0-1.0 range)
        const BAYER_4X4: [[f32; 4]; 4] = [
            [0.0/16.0, 8.0/16.0, 2.0/16.0, 10.0/16.0],
            [12.0/16.0, 4.0/16.0, 14.0/16.0, 6.0/16.0],
            [3.0/16.0, 11.0/16.0, 1.0/16.0, 9.0/16.0],
            [15.0/16.0, 7.0/16.0, 13.0/16.0, 5.0/16.0],
        ];

        let dither = BAYER_4X4[(pixel_y & 3) as usize][(pixel_x & 3) as usize];
        let dither_offset = (dither - 0.5) / 255.0;

        Color {
            r: ((self.r + dither_offset) * 255.0).round().clamp(0.0, 255.0) as u8,
            g: ((self.g + dither_offset) * 255.0).round().clamp(0.0, 255.0) as u8,
            b: ((self.b + dither_offset) * 255.0).round().clamp(0.0, 255.0) as u8,
            a: self.a,
        }
    }

    /// Linear interpolation between two colors using premultiplied alpha.
    /// Chrome/Skia uses premultiplied alpha interpolation for gradients, which
    /// prevents color bleeding from transparent color stops.
    #[inline]
    pub fn lerp(&self, other: &ColorF32, t: f32) -> ColorF32 {
        // Convert to premultiplied alpha
        let pre1_r = self.r * self.a;
        let pre1_g = self.g * self.a;
        let pre1_b = self.b * self.a;

        let pre2_r = other.r * other.a;
        let pre2_g = other.g * other.a;
        let pre2_b = other.b * other.a;

        // Interpolate in premultiplied space
        let pre_r = pre1_r + (pre2_r - pre1_r) * t;
        let pre_g = pre1_g + (pre2_g - pre1_g) * t;
        let pre_b = pre1_b + (pre2_b - pre1_b) * t;
        let a = self.a + (other.a - self.a) * t;

        // Convert back from premultiplied (avoid division by zero)
        if a > 0.0001 {
            ColorF32 {
                r: pre_r / a,
                g: pre_g / a,
                b: pre_b / a,
                a,
            }
        } else {
            // Fully transparent - color doesn't matter
            ColorF32 { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }
        }
    }

    /// Linear interpolation using straight (unpremultiplied) alpha.
    /// Use this when premultiplied interpolation is not desired.
    #[inline]
    pub fn lerp_straight(&self, other: &ColorF32, t: f32) -> ColorF32 {
        ColorF32 {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Gamma-correct interpolation for CSS gradients.
    /// Converts sRGB to linear space, interpolates in premultiplied linear,
    /// then converts back to sRGB. This matches Chrome's gradient rendering.
    #[inline]
    pub fn lerp_gamma_correct(&self, other: &ColorF32, t: f32) -> ColorF32 {
        // Convert sRGB to linear
        let l1_r = Self::srgb_to_linear(self.r);
        let l1_g = Self::srgb_to_linear(self.g);
        let l1_b = Self::srgb_to_linear(self.b);

        let l2_r = Self::srgb_to_linear(other.r);
        let l2_g = Self::srgb_to_linear(other.g);
        let l2_b = Self::srgb_to_linear(other.b);

        // Premultiply by alpha in linear space
        let pre1_r = l1_r * self.a;
        let pre1_g = l1_g * self.a;
        let pre1_b = l1_b * self.a;

        let pre2_r = l2_r * other.a;
        let pre2_g = l2_g * other.a;
        let pre2_b = l2_b * other.a;

        // Interpolate in linear premultiplied space
        let pre_r = pre1_r + (pre2_r - pre1_r) * t;
        let pre_g = pre1_g + (pre2_g - pre1_g) * t;
        let pre_b = pre1_b + (pre2_b - pre1_b) * t;
        let a = self.a + (other.a - self.a) * t;

        // Convert back from premultiplied and to sRGB
        if a > 0.0001 {
            ColorF32 {
                r: Self::linear_to_srgb(pre_r / a),
                g: Self::linear_to_srgb(pre_g / a),
                b: Self::linear_to_srgb(pre_b / a),
                a,
            }
        } else {
            ColorF32 { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }
        }
    }

    /// Convert sRGB to linear space.
    #[inline]
    fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Convert linear to sRGB space.
    #[inline]
    fn linear_to_srgb(c: f32) -> f32 {
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    }

    /// Convert to array for GPU vertex buffers.
    #[inline]
    pub fn to_array(&self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }
}

impl Default for ColorF32 {
    fn default() -> Self {
        Self::BLACK
    }
}

impl From<Color> for ColorF32 {
    fn from(c: Color) -> Self {
        ColorF32::from_color(c)
    }
}

impl From<ColorF32> for Color {
    fn from(c: ColorF32) -> Self {
        c.to_color()
    }
}

/// A single color stop in a gradient (`position` is 0.0–1.0 along the axis).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GradientStop {
    pub color: Color,
    pub position: f32,
}

/// A CSS `linear-gradient(...)`. `angle_deg` follows CSS convention: 0deg
/// points to the top, 90deg to the right, 180deg to the bottom (the default).
#[derive(Debug, Clone, PartialEq)]
pub struct LinearGradient {
    pub angle_deg: f32,
    pub stops: Vec<GradientStop>,
}

/// Radial gradient shape (`circle` | `ellipse`). Ellipse is the CSS default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadialShape {
    #[default]
    Ellipse,
    Circle,
}

/// A CSS `radial-gradient(...)`. `cx`/`cy` are the center as a fraction of the
/// box (0.0–1.0; 0.5,0.5 = center, the default). Size is treated as
/// farthest-corner (the CSS default) — the gradient axis runs from the center
/// to the farthest box corner. `stops` are ordered center→edge.
#[derive(Debug, Clone, PartialEq)]
pub struct RadialGradient {
    pub shape: RadialShape,
    pub cx: f32,
    pub cy: f32,
    pub stops: Vec<GradientStop>,
}

/// `background-clip` — how far the background paints. `Text` clips it to the
/// glyphs (the gradient-text effect), so the box fill is suppressed and the
/// text is filled with the background instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundClip {
    #[default]
    BorderBox,
    PaddingBox,
    ContentBox,
    Text,
}

/// `box-sizing` — whether `width`/`height` include padding+border. Grid reads
/// this to resolve item content boxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoxSizing {
    #[default]
    ContentBox,
    BorderBox,
}

/// A CSS length value.
///
/// Deliberately NOT `Copy`: the `Min`/`Max`/`Clamp` variants own boxed
/// operands, matching the macOS tree. Every other variant is a bare `f32`,
/// so clones are cheap — but they are clones, not implicit copies.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Length {
    /// Pixels.
    Px(f32),
    /// Em (relative to font size).
    Em(f32),
    /// Rem (relative to root font size).
    Rem(f32),
    /// Percentage.
    Percent(f32),
    /// Viewport width (1vw = 1% of viewport width).
    Vw(f32),
    /// Viewport height (1vh = 1% of viewport height).
    Vh(f32),
    /// Viewport min (1vmin = 1% of smaller viewport dimension).
    Vmin(f32),
    /// Viewport max (1vmax = 1% of larger viewport dimension).
    Vmax(f32),
    /// Auto.
    Auto,
    /// Zero.
    #[default]
    Zero,
    /// `min(a, b)` — the smaller of two lengths.
    Min(Box<(Length, Length)>),
    /// `max(a, b)` — the larger of two lengths.
    Max(Box<(Length, Length)>),
    /// `clamp(min, preferred, max)` — preferred, bounded by min and max.
    Clamp(Box<(Length, Length, Length)>),
}

impl Length {
    /// Compute the absolute pixel value.
    ///
    /// Viewport units resolve against a zero viewport here and therefore
    /// compute to 0.0 — matching the macOS tree, where `to_px` delegates to
    /// `to_px_with_viewport(.., 0.0, 0.0)`. Callers that have viewport
    /// dimensions should use `to_px_with_viewport` directly.
    pub fn to_px(&self, font_size: f32, root_font_size: f32, container_size: f32) -> f32 {
        self.to_px_with_viewport(font_size, root_font_size, container_size, 0.0, 0.0)
    }

    /// Compute the absolute pixel value with viewport dimensions for
    /// vw/vh/vmin/vmax units.
    pub fn to_px_with_viewport(
        &self,
        font_size: f32,
        root_font_size: f32,
        container_size: f32,
        viewport_width: f32,
        viewport_height: f32,
    ) -> f32 {
        match self {
            Length::Px(px) => *px,
            Length::Em(em) => em * font_size,
            Length::Rem(rem) => rem * root_font_size,
            Length::Percent(pct) => pct / 100.0 * container_size,
            Length::Vw(vw) => vw / 100.0 * viewport_width,
            Length::Vh(vh) => vh / 100.0 * viewport_height,
            Length::Vmin(vmin) => vmin / 100.0 * viewport_width.min(viewport_height),
            Length::Vmax(vmax) => vmax / 100.0 * viewport_width.max(viewport_height),
            Length::Auto => 0.0, // Context-dependent
            Length::Zero => 0.0,
            Length::Min(pair) => {
                let a = pair.0.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                let b = pair.1.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                a.min(b)
            }
            Length::Max(pair) => {
                let a = pair.0.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                let b = pair.1.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                a.max(b)
            }
            Length::Clamp(triple) => {
                let min_val = triple.0.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                let pref = triple.1.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                let max_val = triple.2.to_px_with_viewport(
                    font_size, root_font_size, container_size, viewport_width, viewport_height);
                pref.clamp(min_val, max_val)
            }
        }
    }
}

/// Display property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Display {
    #[default]
    Block,
    Inline,
    InlineBlock,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    None,
}

impl Display {
    /// Check if this is a flex container.
    pub fn is_flex(self) -> bool {
        matches!(self, Display::Flex | Display::InlineFlex)
    }

    /// Check if this is a grid container.
    pub fn is_grid(self) -> bool {
        matches!(self, Display::Grid | Display::InlineGrid)
    }

    /// Check if this is an inline-block box.
    pub fn is_inline_block(self) -> bool {
        matches!(self, Display::InlineBlock)
    }

    /// Check if this generates an inline-level box.
    pub fn is_inline_level(self) -> bool {
        matches!(
            self,
            Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid
        )
    }
}

// ==================== Flexbox Types ====================

/// Flex direction property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

impl FlexDirection {
    /// Check if this direction is reversed.
    pub fn is_reverse(self) -> bool {
        matches!(self, FlexDirection::RowReverse | FlexDirection::ColumnReverse)
    }

    /// Check if this is a row direction.
    pub fn is_row(self) -> bool {
        matches!(self, FlexDirection::Row | FlexDirection::RowReverse)
    }

    /// Check if this is a column direction.
    pub fn is_column(self) -> bool {
        matches!(self, FlexDirection::Column | FlexDirection::ColumnReverse)
    }
}

/// Flex wrap property.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

/// Justify content property (main axis alignment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifyContent {
    #[default]
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Align items property (cross axis alignment for all items).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignItems {
    #[default]
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
}

/// Align content property (multi-line cross axis alignment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignContent {
    #[default]
    Stretch,
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

/// Align self property (cross axis alignment for individual item).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlignSelf {
    #[default]
    Auto,
    FlexStart,
    FlexEnd,
    Center,
    Baseline,
    Stretch,
}

/// Flex basis property.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum FlexBasis {
    /// Use the item's main size property (width or height).
    #[default]
    Auto,
    /// Size based on content.
    Content,
    /// Explicit length.
    Length(f32),
    /// Percentage of container.
    Percent(f32),
}

// ==================== Grid Types ====================

/// A grid track size.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackSize {
    /// Fixed length in pixels.
    Px(f32),
    /// Percentage of container.
    Percent(f32),
    /// Fractional unit (flexible).
    Fr(f32),
    /// Size based on content minimum.
    MinContent,
    /// Size based on content maximum.
    MaxContent,
    /// Auto sizing.
    Auto,
    /// Minimum/maximum constraint.
    MinMax(Box<TrackSize>, Box<TrackSize>),
    /// Fit content with maximum.
    FitContent(f32),
}

impl Default for TrackSize {
    fn default() -> Self {
        TrackSize::Auto
    }
}

impl TrackSize {
    /// Create a fixed pixel size.
    pub fn px(value: f32) -> Self {
        TrackSize::Px(value)
    }

    /// Create a fractional size.
    pub fn fr(value: f32) -> Self {
        TrackSize::Fr(value)
    }

    /// Create a minmax constraint.
    pub fn minmax(min: TrackSize, max: TrackSize) -> Self {
        TrackSize::MinMax(Box::new(min), Box::new(max))
    }

    /// Check if this is a flexible track (contains fr units).
    pub fn is_flexible(&self) -> bool {
        match self {
            TrackSize::Fr(_) => true,
            TrackSize::MinMax(_, max) => max.is_flexible(),
            _ => false,
        }
    }

    /// Get the minimum size contribution.
    pub fn min_size(&self) -> f32 {
        match self {
            TrackSize::Px(v) => *v,
            TrackSize::MinMax(min, _) => min.min_size(),
            TrackSize::FitContent(max) => 0.0_f32.min(*max),
            _ => 0.0,
        }
    }
}

/// A grid track definition (for grid-template-columns/rows).
#[derive(Debug, Clone, PartialEq)]
pub struct TrackDefinition {
    /// Track sizing.
    pub size: TrackSize,
    /// Optional line name(s) before this track.
    pub line_names: Vec<String>,
}

impl TrackDefinition {
    /// Create a simple track without line names.
    pub fn simple(size: TrackSize) -> Self {
        Self {
            size,
            line_names: Vec::new(),
        }
    }

    /// Create a track with line name.
    pub fn named(size: TrackSize, name: &str) -> Self {
        Self {
            size,
            line_names: vec![name.to_string()],
        }
    }
}

/// Repeat function for grid tracks.
#[derive(Debug, Clone, PartialEq)]
pub enum TrackRepeat {
    /// Repeat a fixed number of times.
    Count(u32, Vec<TrackDefinition>),
    /// Auto-fill: as many as fit.
    AutoFill(Vec<TrackDefinition>),
    /// Auto-fit: as many as fit, collapsing empty tracks.
    AutoFit(Vec<TrackDefinition>),
}

/// Grid template definition.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridTemplate {
    /// Explicit track definitions.
    pub tracks: Vec<TrackDefinition>,
    /// Repeat patterns.
    pub repeats: Vec<(usize, TrackRepeat)>, // (insert_position, repeat)
    /// Final line names.
    pub final_line_names: Vec<String>,
}

impl GridTemplate {
    /// Create an empty template (no explicit tracks).
    pub fn none() -> Self {
        Self::default()
    }

    /// Create from a list of track sizes.
    pub fn from_sizes(sizes: Vec<TrackSize>) -> Self {
        Self {
            tracks: sizes.into_iter().map(TrackDefinition::simple).collect(),
            repeats: Vec::new(),
            final_line_names: Vec::new(),
        }
    }

    /// Get the number of explicit tracks.
    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// Expand `repeat(N, ...)` patterns into a flat track list. Auto-fill/-fit
    /// need the container size, so they are returned separately for layout-time
    /// handling rather than expanded here.
    pub fn expand_tracks(&self) -> (Vec<TrackDefinition>, Option<&TrackRepeat>) {
        if self.repeats.is_empty() {
            return (self.tracks.clone(), None);
        }

        let mut result = Vec::new();
        let mut auto_repeat = None;
        let mut track_idx = 0;

        let mut sorted_repeats: Vec<_> = self.repeats.iter().collect();
        sorted_repeats.sort_by_key(|(pos, _)| *pos);

        for (insert_pos, repeat) in &sorted_repeats {
            while track_idx < *insert_pos && track_idx < self.tracks.len() {
                result.push(self.tracks[track_idx].clone());
                track_idx += 1;
            }

            match repeat {
                TrackRepeat::Count(count, tracks) => {
                    for _ in 0..*count {
                        for track in tracks {
                            result.push(track.clone());
                        }
                    }
                }
                TrackRepeat::AutoFill(_) | TrackRepeat::AutoFit(_) => {
                    auto_repeat = Some(repeat);
                }
            }
        }

        while track_idx < self.tracks.len() {
            result.push(self.tracks[track_idx].clone());
            track_idx += 1;
        }

        (result, auto_repeat)
    }

    /// Number of tracks after repeat expansion (auto-fill/-fit left unexpanded).
    pub fn expanded_track_count(&self) -> usize {
        self.expand_tracks().0.len()
    }
}

/// Named grid area.
#[derive(Debug, Clone, PartialEq)]
pub struct GridArea {
    pub name: String,
    pub row_start: i32,
    pub row_end: i32,
    pub column_start: i32,
    pub column_end: i32,
}

/// Grid template areas.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridTemplateAreas {
    /// Row strings (e.g., ["header header", "nav main", "footer footer"]).
    pub rows: Vec<Vec<Option<String>>>,
    /// Named areas derived from rows.
    pub areas: Vec<GridArea>,
}

impl GridTemplateAreas {
    /// Parse grid-template-areas value.
    pub fn parse(value: &str) -> Option<Self> {
        let mut rows = Vec::new();
        
        for line in value.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Remove quotes if present
            let line = line.trim_matches('"').trim_matches('\'');
            
            let cells: Vec<Option<String>> = line
                .split_whitespace()
                .map(|s| {
                    if s == "." {
                        None
                    } else {
                        Some(s.to_string())
                    }
                })
                .collect();
            
            rows.push(cells);
        }

        if rows.is_empty() {
            return None;
        }

        // Extract named areas
        let mut areas = Vec::new();
        let mut area_names: std::collections::HashSet<String> = std::collections::HashSet::new();
        
        for (row_idx, row) in rows.iter().enumerate() {
            for (col_idx, cell) in row.iter().enumerate() {
                if let Some(name) = cell {
                    if !area_names.contains(name) {
                        // Find extent of this area
                        let (row_end, col_end) = Self::find_area_extent(&rows, row_idx, col_idx, name);
                        areas.push(GridArea {
                            name: name.clone(),
                            row_start: row_idx as i32 + 1,
                            row_end: row_end as i32 + 1,
                            column_start: col_idx as i32 + 1,
                            column_end: col_end as i32 + 1,
                        });
                        area_names.insert(name.clone());
                    }
                }
            }
        }

        Some(Self { rows, areas })
    }

    fn find_area_extent(rows: &[Vec<Option<String>>], start_row: usize, start_col: usize, name: &str) -> (usize, usize) {
        let mut row_end = start_row;
        let mut col_end = start_col;

        // Find column extent
        for col in start_col..rows[start_row].len() {
            if rows[start_row].get(col) == Some(&Some(name.to_string())) {
                col_end = col + 1;
            } else {
                break;
            }
        }

        // Find row extent
        for row in start_row..rows.len() {
            if rows[row].get(start_col) == Some(&Some(name.to_string())) {
                row_end = row + 1;
            } else {
                break;
            }
        }

        (row_end, col_end)
    }

    /// Get area by name.
    pub fn get_area(&self, name: &str) -> Option<&GridArea> {
        self.areas.iter().find(|a| a.name == name)
    }
}

/// Grid auto flow direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GridAutoFlow {
    #[default]
    Row,
    Column,
    RowDense,
    ColumnDense,
}

impl GridAutoFlow {
    /// Check if this is a row-based flow.
    pub fn is_row(self) -> bool {
        matches!(self, GridAutoFlow::Row | GridAutoFlow::RowDense)
    }

    /// Check if this uses dense packing.
    pub fn is_dense(self) -> bool {
        matches!(self, GridAutoFlow::RowDense | GridAutoFlow::ColumnDense)
    }
}

/// Grid line reference (for grid-column-start, etc.).
#[derive(Debug, Clone, PartialEq)]
pub enum GridLine {
    /// Auto placement.
    Auto,
    /// Specific line number (1-based, can be negative).
    Number(i32),
    /// Named line.
    Name(String),
    /// Span a number of tracks.
    Span(u32),
    /// Span to a named line.
    SpanName(String),
}

impl Default for GridLine {
    fn default() -> Self {
        GridLine::Auto
    }
}

/// Grid placement for an item.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridPlacement {
    /// Column start line.
    pub column_start: GridLine,
    /// Column end line.
    pub column_end: GridLine,
    /// Row start line.
    pub row_start: GridLine,
    /// Row end line.
    pub row_end: GridLine,
}

impl GridPlacement {
    /// Create placement from a named area.
    pub fn from_area(name: &str) -> Self {
        Self {
            column_start: GridLine::Name(format!("{}-start", name)),
            column_end: GridLine::Name(format!("{}-end", name)),
            row_start: GridLine::Name(format!("{}-start", name)),
            row_end: GridLine::Name(format!("{}-end", name)),
        }
    }

    /// Create placement from explicit lines.
    pub fn from_lines(col_start: i32, col_end: i32, row_start: i32, row_end: i32) -> Self {
        Self {
            column_start: GridLine::Number(col_start),
            column_end: GridLine::Number(col_end),
            row_start: GridLine::Number(row_start),
            row_end: GridLine::Number(row_end),
        }
    }
}

/// Justify items (horizontal alignment in grid cells).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifyItems {
    #[default]
    Stretch,
    Start,
    End,
    Center,
}

/// Justify self (horizontal alignment for individual item).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JustifySelf {
    #[default]
    Auto,
    Stretch,
    Start,
    End,
    Center,
}

/// Position property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

/// Font weight values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const NORMAL: FontWeight = FontWeight(400);
    pub const BOLD: FontWeight = FontWeight(700);
}

impl Default for FontWeight {
    fn default() -> Self {
        Self::NORMAL
    }
}

/// Font style values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
    Oblique,
}

/// Text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Right,
    Center,
    Justify,
}

/// Overflow behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overflow {
    #[default]
    Visible,
    Hidden,
    Scroll,
    Auto,
    Clip,
}

impl Overflow {
    /// Check if this overflow creates a scroll container.
    pub fn is_scrollable(self) -> bool {
        matches!(self, Overflow::Scroll | Overflow::Auto)
    }

    /// Check if content is clipped.
    pub fn clips_content(self) -> bool {
        !matches!(self, Overflow::Visible)
    }
}

/// Scroll behavior for smooth scrolling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollBehavior {
    #[default]
    Auto,
    Smooth,
}

/// Overscroll behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverscrollBehavior {
    #[default]
    Auto,
    Contain,
    None,
}

/// Scrollbar width.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarWidth {
    #[default]
    Auto,
    Thin,
    None,
}

/// Scrollbar gutter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarGutter {
    #[default]
    Auto,
    Stable,
    BothEdges,
}

/// Text decoration line values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextDecorationLine {
    pub underline: bool,
    pub overline: bool,
    pub line_through: bool,
}

impl TextDecorationLine {
    pub const NONE: TextDecorationLine = TextDecorationLine {
        underline: false,
        overline: false,
        line_through: false,
    };

    pub const UNDERLINE: TextDecorationLine = TextDecorationLine {
        underline: true,
        overline: false,
        line_through: false,
    };

    pub const OVERLINE: TextDecorationLine = TextDecorationLine {
        underline: false,
        overline: true,
        line_through: false,
    };

    pub const LINE_THROUGH: TextDecorationLine = TextDecorationLine {
        underline: false,
        overline: false,
        line_through: true,
    };
}

/// Text decoration style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextDecorationStyle {
    #[default]
    Solid,
    Double,
    Dotted,
    Dashed,
    Wavy,
}

/// Font stretch values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FontStretch {
    UltraCondensed,
    ExtraCondensed,
    Condensed,
    SemiCondensed,
    #[default]
    Normal,
    SemiExpanded,
    Expanded,
    ExtraExpanded,
    UltraExpanded,
}

impl FontStretch {
    /// Convert to DirectWrite font stretch value (1-9).
    pub fn to_dwrite_value(&self) -> u32 {
        match self {
            FontStretch::UltraCondensed => 1,
            FontStretch::ExtraCondensed => 2,
            FontStretch::Condensed => 3,
            FontStretch::SemiCondensed => 4,
            FontStretch::Normal => 5,
            FontStretch::SemiExpanded => 6,
            FontStretch::Expanded => 7,
            FontStretch::ExtraExpanded => 8,
            FontStretch::UltraExpanded => 9,
        }
    }
}

/// White space handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WhiteSpace {
    #[default]
    Normal,
    Nowrap,
    Pre,
    PreWrap,
    PreLine,
    BreakSpaces,
}

/// Word break behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WordBreak {
    #[default]
    Normal,
    BreakAll,
    KeepAll,
    BreakWord,
}

/// Vertical alignment.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum VerticalAlign {
    #[default]
    Baseline,
    Sub,
    Super,
    Top,
    TextTop,
    Middle,
    Bottom,
    TextBottom,
    Length(f32),
}

/// Writing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WritingMode {
    #[default]
    HorizontalTb,
    VerticalRl,
    VerticalLr,
}

/// Text transform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextTransform {
    #[default]
    None,
    Capitalize,
    Uppercase,
    Lowercase,
}

/// Direction for bidi text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Ltr,
    Rtl,
}

// ============ Background Layer Types (partial: gradient-free) ============


/// Background size specification.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundSize {
    /// Stretch to cover the entire area.
    Cover,
    /// Scale to fit within the area.
    Contain,
    /// Explicit width and height (None = auto for that dimension).
    Explicit { width: Option<f32>, height: Option<f32> },
    /// Auto sizing (use intrinsic dimensions).
    Auto,
}

impl Default for BackgroundSize {
    fn default() -> Self {
        BackgroundSize::Auto
    }
}

/// Background repeat specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundRepeat {
    /// Repeat in both directions.
    Repeat,
    /// Repeat horizontally only.
    RepeatX,
    /// Repeat vertically only.
    RepeatY,
    /// No repeat.
    NoRepeat,
    /// Space evenly to fill.
    Space,
    /// Round to fill without clipping.
    Round,
}

impl Default for BackgroundRepeat {
    fn default() -> Self {
        BackgroundRepeat::Repeat
    }
}

/// Background position specification.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundPosition {
    /// Horizontal position (0.0 = left, 0.5 = center, 1.0 = right, or pixel offset).
    pub x: BackgroundPositionValue,
    /// Vertical position (0.0 = top, 0.5 = center, 1.0 = bottom, or pixel offset).
    pub y: BackgroundPositionValue,
}

impl Default for BackgroundPosition {
    fn default() -> Self {
        BackgroundPosition {
            x: BackgroundPositionValue::Percent(0.0),
            y: BackgroundPositionValue::Percent(0.0),
        }
    }
}

/// A single dimension of background position.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundPositionValue {
    /// Percentage (0.0 = start, 1.0 = end).
    Percent(f32),
    /// Pixel offset from the start.
    Px(f32),
}

impl Default for BackgroundPositionValue {
    fn default() -> Self {
        BackgroundPositionValue::Percent(0.0)
    }
}

impl BackgroundPositionValue {
    /// Convert to a pixel offset given the container size and image size.
    pub fn to_px(&self, container_size: f32, image_size: f32) -> f32 {
        match self {
            BackgroundPositionValue::Percent(pct) => {
                // CSS background-position: percentage positions the image such that
                // X% of the image aligns with X% of the container
                (container_size - image_size) * pct
            }
            BackgroundPositionValue::Px(px) => *px,
        }
    }
}

/// Background origin - where the background positioning area starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundOrigin {
    /// Position relative to the border box.
    #[default]
    PaddingBox,
    /// Position relative to the border box.
    BorderBox,
    /// Position relative to the content box.
    ContentBox,
}

// ==================== Animation/Transition Types ====================

/// Animation timing function.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum TimingFunction {
    #[default]
    Ease,
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    StepStart,
    StepEnd,
    Steps(u32, bool), // (count, jump_start)
    CubicBezier(f32, f32, f32, f32),
}

/// Animation fill mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationFillMode {
    #[default]
    None,
    Forwards,
    Backwards,
    Both,
}

/// Animation play state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationPlayState {
    #[default]
    Running,
    Paused,
}

/// Animation direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnimationDirection {
    #[default]
    Normal,
    Reverse,
    Alternate,
    AlternateReverse,
}

/// Animation iteration count.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum AnimationIterationCount {
    #[default]
    One,
    Infinite,
    Count(f32),
}

/// A CSS box-shadow value.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BoxShadow {
    /// Horizontal offset (positive = right).
    pub offset_x: f32,
    /// Vertical offset (positive = down).
    pub offset_y: f32,
    /// Blur radius (0 = sharp edge).
    pub blur_radius: f32,
    /// Spread radius (positive = larger shadow).
    pub spread_radius: f32,
    /// Shadow color.
    pub color: Color,
    /// Whether this is an inset shadow.
    pub inset: bool,
}

impl BoxShadow {
    /// Create a new box shadow with default values.
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Create a simple drop shadow.
    pub fn drop_shadow(offset_x: f32, offset_y: f32, blur: f32, color: Color) -> Self {
        Self {
            offset_x,
            offset_y,
            blur_radius: blur,
            spread_radius: 0.0,
            color,
            inset: false,
        }
    }
    
    /// Check if this shadow is visible (non-zero offset, blur, or spread with non-transparent color).
    pub fn is_visible(&self) -> bool {
        self.color.a > 0.0 && 
        (self.offset_x != 0.0 || self.offset_y != 0.0 || self.blur_radius > 0.0 || self.spread_radius != 0.0)
    }
}

/// A filter function that can be applied to the backdrop.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum BackdropFilter {
    /// No backdrop filter.
    #[default]
    None,
    /// Gaussian blur with the specified radius in pixels.
    Blur(f32),
    /// Grayscale filter (0.0 = no effect, 1.0 = fully grayscale).
    Grayscale(f32),
    /// Brightness adjustment (1.0 = no change).
    Brightness(f32),
    /// Contrast adjustment (1.0 = no change).
    Contrast(f32),
    /// Saturate adjustment (1.0 = no change, 0.0 = grayscale, >1 = oversaturated).
    Saturate(f32),
    /// Sepia filter (0.0 = no effect, 1.0 = fully sepia).
    Sepia(f32),
}

impl BackdropFilter {
    /// Check if this filter has any effect.
    pub fn is_none(&self) -> bool {
        matches!(self, BackdropFilter::None)
    }

    /// Check if this filter requires blur (most expensive operation).
    pub fn needs_blur(&self) -> bool {
        matches!(self, BackdropFilter::Blur(r) if *r > 0.0)
    }
}

// ==================== Transform Types ====================

/// A single 2D transform operation.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformOp {
    /// translate(x, y)
    Translate(Length, Length),
    /// translateX(x)
    TranslateX(Length),
    /// translateY(y)
    TranslateY(Length),
    /// scale(x, y) or scale(s)
    Scale(f32, f32),
    /// scaleX(s)
    ScaleX(f32),
    /// scaleY(s)
    ScaleY(f32),
    /// rotate(angle) - angle in degrees
    Rotate(f32),
    /// skewX(angle) - angle in degrees
    SkewX(f32),
    /// skewY(angle) - angle in degrees
    SkewY(f32),
    /// skew(x, y) - angles in degrees
    Skew(f32, f32),
    /// matrix(a, b, c, d, e, f) - 2D affine transform
    Matrix(f32, f32, f32, f32, f32, f32),
}

/// A list of transform operations (applied in order).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TransformList {
    pub ops: Vec<TransformOp>,
}

impl TransformList {
    /// Create an empty (identity) transform list.
    pub fn none() -> Self {
        Self { ops: Vec::new() }
    }

    /// Check if this is the identity transform.
    pub fn is_identity(&self) -> bool {
        self.ops.is_empty()
    }

    /// Compute the 3x3 affine transform matrix.
    /// Returns [a, b, c, d, e, f] where the matrix is:
    /// | a c e |
    /// | b d f |
    /// | 0 0 1 |
    pub fn to_matrix(&self, container_width: f32, container_height: f32) -> [f32; 6] {
        let mut result = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]; // Identity

        for op in &self.ops {
            let m = match op {
                TransformOp::Translate(x, y) => {
                    let tx = x.to_px(16.0, 16.0, container_width);
                    let ty = y.to_px(16.0, 16.0, container_height);
                    [1.0, 0.0, 0.0, 1.0, tx, ty]
                }
                TransformOp::TranslateX(x) => {
                    let tx = x.to_px(16.0, 16.0, container_width);
                    [1.0, 0.0, 0.0, 1.0, tx, 0.0]
                }
                TransformOp::TranslateY(y) => {
                    let ty = y.to_px(16.0, 16.0, container_height);
                    [1.0, 0.0, 0.0, 1.0, 0.0, ty]
                }
                TransformOp::Scale(sx, sy) => [*sx, 0.0, 0.0, *sy, 0.0, 0.0],
                TransformOp::ScaleX(s) => [*s, 0.0, 0.0, 1.0, 0.0, 0.0],
                TransformOp::ScaleY(s) => [1.0, 0.0, 0.0, *s, 0.0, 0.0],
                TransformOp::Rotate(deg) => {
                    let rad = deg.to_radians();
                    let cos = rad.cos();
                    let sin = rad.sin();
                    [cos, sin, -sin, cos, 0.0, 0.0]
                }
                TransformOp::SkewX(deg) => {
                    let tan = deg.to_radians().tan();
                    [1.0, 0.0, tan, 1.0, 0.0, 0.0]
                }
                TransformOp::SkewY(deg) => {
                    let tan = deg.to_radians().tan();
                    [1.0, tan, 0.0, 1.0, 0.0, 0.0]
                }
                TransformOp::Skew(dx, dy) => {
                    let tan_x = dx.to_radians().tan();
                    let tan_y = dy.to_radians().tan();
                    [1.0, tan_y, tan_x, 1.0, 0.0, 0.0]
                }
                TransformOp::Matrix(a, b, c, d, e, f) => [*a, *b, *c, *d, *e, *f],
            };

            // Multiply: result = result * m
            result = multiply_matrices(result, m);
        }

        result
    }
}

/// Multiply two 2D affine matrices.
fn multiply_matrices(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

/// Transform origin (default: 50% 50%).
#[derive(Debug, Clone, PartialEq)]
pub struct TransformOrigin {
    pub x: Length,
    pub y: Length,
}

impl Default for TransformOrigin {
    fn default() -> Self {
        Self {
            x: Length::Percent(50.0),
            y: Length::Percent(50.0),
        }
    }
}

/// Computed style for an element.
#[derive(Debug, Clone, Default)]
pub struct ComputedStyle {
    // Transform (wire PR: the TransformList/TransformOrigin types landed
    // INERT in #36; these fields are what make the properties COMPUTE).
    pub transform: TransformList,
    pub transform_origin: TransformOrigin,
    // Shadow/Filter wire: BoxShadow landed INERT in #37; this field is what
    // makes box-shadow compute. Vec because box-shadow takes a comma list.
    pub box_shadows: Vec<BoxShadow>,
    // Animation/transition wire (Cluster A3). The enums landed INERT in #40;
    // these fields are what make the properties compute. Durations are in
    // SECONDS, matching the macOS tree - parse_time converts ms for us.
    pub transition_property: String,
    pub transition_duration: f32,
    pub transition_timing_function: TimingFunction,
    pub transition_delay: f32,
    pub animation_name: String,
    pub animation_duration: f32,
    pub animation_timing_function: TimingFunction,
    pub animation_delay: f32,
    pub animation_iteration_count: AnimationIterationCount,
    pub animation_direction: AnimationDirection,
    pub animation_fill_mode: AnimationFillMode,
    pub animation_play_state: AnimationPlayState,
    // Box model
    pub display: Display,
    pub position: Position,

    /// Box offsets for a positioned element. `None` is CSS `auto` — which is
    /// NOT the same as `0`, and the distinction is load-bearing: `auto` means
    /// "keep the static-flow position on this axis", while `0` pins the edge to
    /// the containing block. Storing `Option<Length>` rather than a Length with
    /// a zero default is what keeps those distinguishable.
    pub top: Option<Length>,
    pub right: Option<Length>,
    pub bottom: Option<Length>,
    pub left: Option<Length>,

    /// `z-index`. 0 stands for `auto` here, matching the macOS reference and
    /// `LayoutBox::z_index`, which is already an `i32`.
    pub z_index: i32,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,

    // Margin
    pub margin_top: Length,
    pub margin_right: Length,
    pub margin_bottom: Length,
    pub margin_left: Length,

    // Padding
    pub padding_top: Length,
    pub padding_right: Length,
    pub padding_bottom: Length,
    pub padding_left: Length,

    // Border
    pub border_top_width: Length,
    pub border_right_width: Length,
    pub border_bottom_width: Length,
    pub border_left_width: Length,
    pub border_top_color: Color,
    pub border_right_color: Color,
    pub border_bottom_color: Color,
    pub border_left_color: Color,

    // Colors
    pub color: Color,
    pub background_color: Color,

    // Typography - Basic
    pub font_size: Length,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub font_family: String,
    pub line_height: f32,
    pub text_align: TextAlign,

    // Typography - Advanced
    pub font_stretch: FontStretch,
    pub letter_spacing: Length,
    pub word_spacing: Length,
    pub text_indent: Length,
    pub text_decoration_line: TextDecorationLine,
    pub text_decoration_color: Option<Color>,
    pub text_decoration_style: TextDecorationStyle,
    pub text_decoration_thickness: Length,
    pub text_transform: TextTransform,
    pub white_space: WhiteSpace,
    pub word_break: WordBreak,
    pub vertical_align: VerticalAlign,
    pub writing_mode: WritingMode,
    pub direction: Direction,

    // Visual
    pub opacity: f32,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,

    // Flexbox Container
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub align_content: AlignContent,
    pub row_gap: Length,
    pub column_gap: Length,

    // Flexbox Item
    pub order: i32,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: FlexBasis,
    pub align_self: AlignSelf,

    // Scrolling
    pub scroll_behavior: ScrollBehavior,
    pub overscroll_behavior_x: OverscrollBehavior,
    pub overscroll_behavior_y: OverscrollBehavior,
    pub scrollbar_width: ScrollbarWidth,
    pub scrollbar_gutter: ScrollbarGutter,
    pub scrollbar_color: Option<(Color, Color)>, // (thumb, track)

    // Grid Container
    pub grid_template_columns: GridTemplate,
    pub grid_template_rows: GridTemplate,
    pub grid_template_areas: Option<GridTemplateAreas>,
    pub grid_auto_columns: TrackSize,
    pub grid_auto_rows: TrackSize,
    pub grid_auto_flow: GridAutoFlow,

    // Grid Item
    pub grid_column_start: GridLine,
    pub grid_column_end: GridLine,
    pub grid_row_start: GridLine,
    pub grid_row_end: GridLine,

    // Grid Alignment (also used by Flexbox)
    pub justify_items: JustifyItems,
    pub justify_self: JustifySelf,

    /// CSS custom properties (`--name: value`) in scope for this element.
    /// Custom properties inherit, so this is shared via `Arc` — every
    /// descendant of `:root` points at the same map (cheap refcount clone in
    /// `inherit_from`); only an element that defines its own `--x` pays a
    /// copy-on-write. Referenced by `var(--name)` at declaration time.
    pub custom_properties: std::sync::Arc<std::collections::HashMap<String, String>>,

    /// `background: linear-gradient(...)`, if any. Painted over
    /// `background_color`. Not inherited (background is per-element).
    pub background_gradient: Option<LinearGradient>,

    /// `background: radial-gradient(...)`, if any. Painted over
    /// `background_color`. Not inherited (background is per-element).
    pub background_radial_gradient: Option<RadialGradient>,

    /// `background-clip`. When `Text`, the background is clipped to the glyphs
    /// and the box fill is suppressed. Not inherited.
    pub background_clip: BackgroundClip,

    /// `box-sizing`. `ContentBox` (default) = width/height are the content box;
    /// `BorderBox` = they include padding+border. Not inherited.
    pub box_sizing: BoxSizing,
}

impl ComputedStyle {
    /// Create default style.
    pub fn new() -> Self {
        Self {
            font_size: Length::Px(16.0),
            // 0.0 is the `line-height: normal` sentinel (CSS initial value):
            // the layout resolves it from font metrics, not a flat 1.2 ratio
            // (W56, port of macOS #56). Author number/px set a positive ratio.
            line_height: 0.0,
            opacity: 1.0,
            color: Color::BLACK,
            background_color: Color::TRANSPARENT,
            font_family: "sans-serif".to_string(),
            text_decoration_line: TextDecorationLine::NONE,
            text_decoration_color: None,
            text_decoration_thickness: Length::Auto,
            // Flexbox item defaults
            flex_shrink: 1.0, // Default is 1, not 0
            // CSS initial values: width/height are `auto`, max-width/max-height
            // are `none` (no constraint). The derive-default Length::Zero made
            // every unstyled element lay out at width 0 — the zero-width tree
            // in the 2026-07-07 Windows parity baseline.
            width: Length::Auto,
            height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            ..Default::default()
        }
    }

    /// Create style with inheritance from parent.
    pub fn inherit_from(parent: &ComputedStyle) -> Self {
        Self {
            // Inherited properties
            color: parent.color,
            font_size: parent.font_size.clone(),
            font_weight: parent.font_weight,
            font_style: parent.font_style,
            font_stretch: parent.font_stretch,
            font_family: parent.font_family.clone(),
            line_height: parent.line_height,
            text_align: parent.text_align,
            letter_spacing: parent.letter_spacing.clone(),
            word_spacing: parent.word_spacing.clone(),
            text_indent: parent.text_indent.clone(),
            text_transform: parent.text_transform,
            white_space: parent.white_space,
            word_break: parent.word_break,
            direction: parent.direction,
            writing_mode: parent.writing_mode,

            // Text decoration is NOT inherited (each element sets its own)
            text_decoration_line: TextDecorationLine::NONE,
            text_decoration_color: None,
            text_decoration_style: TextDecorationStyle::Solid,
            text_decoration_thickness: Length::Auto,

            // Non-inherited sizing gets CSS initial values (see new())
            width: Length::Auto,
            height: Length::Auto,
            max_width: Length::Auto,
            max_height: Length::Auto,
            flex_shrink: 1.0,

            // Non-inherited paint initials. background-color is NOT inherited;
            // its initial value is `transparent`, and opacity's is 1.0 — without
            // these, `..Default::default()` gives opaque black / 0.0, so every
            // inheriting element painted a black box and could vanish.
            background_color: Color::TRANSPARENT,
            opacity: 1.0,

            // Custom properties inherit (cheap Arc clone).
            custom_properties: parent.custom_properties.clone(),

            // Remaining non-inherited get defaults
            ..Default::default()
        }
    }
}

/// CSS property value (unparsed or parsed).
#[derive(Debug, Clone)]
pub enum PropertyValue {
    /// Inherit from parent.
    Inherit,
    /// Initial value.
    Initial,
    /// Specific value.
    Specified(String),
}

/// A CSS declaration (property: value).
#[derive(Debug, Clone)]
pub struct Declaration {
    pub property: String,
    pub value: PropertyValue,
    pub important: bool,
}

/// A CSS rule (selector + declarations).
#[derive(Debug, Clone)]
pub struct Rule {
    pub selector: String,
    pub declarations: Vec<Declaration>,
}

/// A complete stylesheet.
#[derive(Debug, Default)]
pub struct Stylesheet {
    pub rules: Vec<Rule>,
}

impl Stylesheet {
    /// Create an empty stylesheet.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Parse a CSS string into a stylesheet.
    pub fn parse(css: &str) -> Result<Self, CssError> {
        debug!(len = css.len(), "Parsing CSS");
        let ast = parse_stylesheet(css).map_err(|e| CssError::ParseError(e.to_string()))?;

        let rules = ast
            .rules
            .into_iter()
            .map(|r| Rule {
                selector: r.selector,
                declarations: r
                    .declarations
                    .into_iter()
                    .map(|d| Declaration {
                        property: d.property,
                        value: PropertyValue::Specified(d.value),
                        important: d.important,
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();

        debug!(rule_count = rules.len(), "CSS parsed");
        Ok(Stylesheet { rules })
    }

    /// Get the number of rules in this stylesheet.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }
}

/// Parse a color value.
pub fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();

    // Named colors
    match value.to_lowercase().as_str() {
        "transparent" => return Some(Color::TRANSPARENT),
        "black" => return Some(Color::BLACK),
        "white" => return Some(Color::WHITE),
        "red" => return Some(Color::from_rgb(255, 0, 0)),
        "green" => return Some(Color::from_rgb(0, 128, 0)),
        "blue" => return Some(Color::from_rgb(0, 0, 255)),
        "yellow" => return Some(Color::from_rgb(255, 255, 0)),
        "gray" | "grey" => return Some(Color::from_rgb(128, 128, 128)),
        _ => {}
    }

    // Hex colors
    if let Some(hex) = value.strip_prefix('#') {
        let (r, g, b, a) = match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                (r, g, b, 1.0)
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                (r, g, b, 1.0)
            }
            8 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                let a = u8::from_str_radix(&hex[6..8], 16).ok()? as f32 / 255.0;
                (r, g, b, a)
            }
            _ => return None,
        };
        return Some(Color::new(r, g, b, a));
    }

    // rgb() / rgba()
    if value.starts_with("rgb") {
        // Simplified parsing
        let inner = value
            .trim_start_matches("rgba(")
            .trim_start_matches("rgb(")
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() >= 3 {
            let r = parts[0].trim().parse::<u8>().ok()?;
            let g = parts[1].trim().parse::<u8>().ok()?;
            let b = parts[2].trim().parse::<u8>().ok()?;
            let a = if parts.len() >= 4 {
                parts[3].trim().parse::<f32>().ok()?
            } else {
                1.0
            };
            return Some(Color::new(r, g, b, a));
        }
    }

    None
}

/// Parse a length value.
pub fn parse_length(value: &str) -> Option<Length> {
    let value = value.trim();

    if value == "auto" {
        return Some(Length::Auto);
    }
    if value == "0" {
        return Some(Length::Zero);
    }

    if value.ends_with("px") {
        let num = value.trim_end_matches("px").parse::<f32>().ok()?;
        return Some(Length::Px(num));
    }
    // rem MUST be checked before em: "2rem".ends_with("em") is true, so the
    // em branch would claim it, trim "em" to leave "2r", fail to parse that
    // as f32, and the `?` would bail out of the whole function — silently
    // dropping every rem value. Ordering is the fix, matching the macOS tree.
    if value.ends_with("rem") {
        let num = value.trim_end_matches("rem").parse::<f32>().ok()?;
        return Some(Length::Rem(num));
    }
    if value.ends_with("em") {
        let num = value.trim_end_matches("em").parse::<f32>().ok()?;
        return Some(Length::Em(num));
    }
    if value.ends_with('%') {
        let num = value.trim_end_matches('%').parse::<f32>().ok()?;
        return Some(Length::Percent(num));
    }

    // Try plain number (treated as px)
    if let Ok(num) = value.parse::<f32>() {
        return Some(Length::Px(num));
    }

    None
}

/// Parse display value.
pub fn parse_display(value: &str) -> Option<Display> {
    match value.trim().to_lowercase().as_str() {
        "block" => Some(Display::Block),
        "inline" => Some(Display::Inline),
        "inline-block" => Some(Display::InlineBlock),
        "flex" => Some(Display::Flex),
        "inline-flex" => Some(Display::InlineFlex),
        "grid" => Some(Display::Grid),
        "inline-grid" => Some(Display::InlineGrid),
        "none" => Some(Display::None),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_color_hex() {
        assert_eq!(parse_color("#fff"), Some(Color::from_rgb(255, 255, 255)));
        assert_eq!(parse_color("#000000"), Some(Color::BLACK));
        assert_eq!(parse_color("#ff0000"), Some(Color::from_rgb(255, 0, 0)));
    }

    #[test]
    fn test_parse_color_named() {
        assert_eq!(parse_color("red"), Some(Color::from_rgb(255, 0, 0)));
        assert_eq!(parse_color("black"), Some(Color::BLACK));
        assert_eq!(parse_color("transparent"), Some(Color::TRANSPARENT));
    }

    #[test]
    fn test_parse_length() {
        assert_eq!(parse_length("10px"), Some(Length::Px(10.0)));
        assert_eq!(parse_length("1.5em"), Some(Length::Em(1.5)));
        assert_eq!(parse_length("50%"), Some(Length::Percent(50.0)));
        assert_eq!(parse_length("auto"), Some(Length::Auto));
    }

    #[test]
    fn test_parse_stylesheet() {
        let css = r#"
            body {
                color: black;
            }
            .container {
                width: 100%;
            }
        "#;

        let stylesheet = Stylesheet::parse(css).unwrap();
        assert!(stylesheet.rules.len() >= 2);
    }

    #[test]
    fn test_computed_style_inherit() {
        let parent = ComputedStyle {
            color: Color::from_rgb(255, 0, 0),
            font_size: Length::Px(20.0),
            ..Default::default()
        };

        let child = ComputedStyle::inherit_from(&parent);
        assert_eq!(child.color, parent.color);
        assert_eq!(child.font_size, parent.font_size);
        // Non-inherited properties should be default
        assert_eq!(child.display, Display::Block);
    }
}

#[cfg(test)]
mod length_viewport_tests {
    use super::*;

    #[test]
    fn viewport_units_resolve_against_the_viewport() {
        let vp_w = 1000.0;
        let vp_h = 600.0;
        assert_eq!(Length::Vw(50.0).to_px_with_viewport(16.0, 16.0, 0.0, vp_w, vp_h), 500.0);
        assert_eq!(Length::Vh(50.0).to_px_with_viewport(16.0, 16.0, 0.0, vp_w, vp_h), 300.0);
    }

    #[test]
    fn vmin_and_vmax_pick_the_smaller_and_larger_axis() {
        // Deliberately landscape, then portrait: a implementation that hard-codes
        // width for vmin passes the first and fails the second.
        let landscape = Length::Vmin(10.0).to_px_with_viewport(16.0, 16.0, 0.0, 1000.0, 600.0);
        let portrait = Length::Vmin(10.0).to_px_with_viewport(16.0, 16.0, 0.0, 600.0, 1000.0);
        assert_eq!(landscape, 60.0, "vmin must follow the SHORTER axis");
        assert_eq!(portrait, 60.0, "vmin must follow the shorter axis in portrait too");

        assert_eq!(
            Length::Vmax(10.0).to_px_with_viewport(16.0, 16.0, 0.0, 1000.0, 600.0),
            100.0,
            "vmax must follow the LONGER axis"
        );
    }

    #[test]
    fn viewport_units_are_zero_without_viewport_context() {
        // Matches the macOS tree, where to_px delegates with (0.0, 0.0).
        // Documented rather than invented: a Windows-only fallback here would
        // diverge the trees silently.
        assert_eq!(Length::Vw(50.0).to_px(16.0, 16.0, 800.0), 0.0);
        assert_eq!(Length::Vh(50.0).to_px(16.0, 16.0, 800.0), 0.0);
    }

    #[test]
    fn existing_units_are_unchanged_by_the_new_resolver() {
        // to_px now delegates to to_px_with_viewport; every pre-existing
        // variant must compute exactly what it did before.
        assert_eq!(Length::Px(12.0).to_px(16.0, 16.0, 800.0), 12.0);
        assert_eq!(Length::Em(2.0).to_px(16.0, 16.0, 800.0), 32.0);
        assert_eq!(Length::Rem(2.0).to_px(16.0, 20.0, 800.0), 40.0);
        assert_eq!(Length::Percent(25.0).to_px(16.0, 16.0, 800.0), 200.0);
        assert_eq!(Length::Auto.to_px(16.0, 16.0, 800.0), 0.0);
        assert_eq!(Length::Zero.to_px(16.0, 16.0, 800.0), 0.0);
    }

    #[test]
    fn viewport_units_are_not_yet_parseable() {
        // Pins the INERT boundary of this PR: the variants exist, but the
        // parser is deliberately untouched, so no stylesheet behaves
        // differently yet. If a later PR wires the parser, this test SHOULD
        // fail and be updated -- that is the signal that behaviour changed.
        assert_eq!(parse_length("50vw"), None);
        assert_eq!(parse_length("10vmin"), None);
    }
}

#[cfg(test)]
mod rem_parse_regression {
    use super::*;

    #[test]
    fn rem_lengths_parse() {
        // REGRESSION: `ends_with("em")` was checked before `ends_with("rem")`.
        // "2rem".ends_with("em") is true, so the em branch claimed it, trimmed
        // "em" to leave "2r", failed to parse that as f32, and the `?` bailed
        // out of the whole function -- so EVERY rem value silently vanished.
        assert_eq!(parse_length("2rem"), Some(Length::Rem(2.0)));
        assert_eq!(parse_length("0.5rem"), Some(Length::Rem(0.5)));
        assert_eq!(parse_length("-1rem"), Some(Length::Rem(-1.0)));
    }

    #[test]
    fn em_still_parses_as_em_not_rem() {
        // The obvious wrong fix is to reorder and let "rem" swallow "em".
        assert_eq!(parse_length("2em"), Some(Length::Em(2.0)));
    }
}

mod background_partial_tests {
    use super::*;

    #[test]
    fn defaults_are_the_css_initial_values() {
        // background-size: auto, background-repeat: repeat,
        // background-origin: padding-box, background-position: 0% 0%.
        assert_eq!(BackgroundSize::default(), BackgroundSize::Auto);
        assert_eq!(BackgroundRepeat::default(), BackgroundRepeat::Repeat);
        assert_eq!(BackgroundOrigin::default(), BackgroundOrigin::PaddingBox);
        let p = BackgroundPosition::default();
        assert_eq!(p.x, BackgroundPositionValue::Percent(0.0));
        assert_eq!(p.y, BackgroundPositionValue::Percent(0.0));
    }

    #[test]
    fn explicit_size_distinguishes_auto_per_axis() {
        // `background-size: 100px auto` is one axis explicit and one auto.
        // Modelling that as Option per dimension is the whole point, so a
        // port that collapsed it to a single Option would fail here.
        let one_axis = BackgroundSize::Explicit { width: Some(100.0), height: None };
        let both = BackgroundSize::Explicit { width: Some(100.0), height: Some(50.0) };
        assert_ne!(one_axis, both);
        if let BackgroundSize::Explicit { width, height } = one_axis {
            assert_eq!(width, Some(100.0));
            assert_eq!(height, None, "auto on one axis must stay None");
        } else {
            panic!("expected Explicit");
        }
    }

    #[test]
    fn cover_and_contain_are_distinct_from_auto_and_each_other() {
        assert_ne!(BackgroundSize::Cover, BackgroundSize::Contain);
        assert_ne!(BackgroundSize::Cover, BackgroundSize::Auto);
    }

    #[test]
    fn position_percent_and_px_are_not_interchangeable() {
        // 50% and 50px mean different things; a port that flattened both to
        // f32 would lose the distinction silently.
        assert_ne!(
            BackgroundPositionValue::Percent(50.0),
            BackgroundPositionValue::Px(50.0)
        );
    }

    #[test]
    fn all_six_repeat_modes_are_distinct() {
        use BackgroundRepeat::*;
        let all = [Repeat, RepeatX, RepeatY, NoRepeat, Space, Round];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a, b, "repeat modes must not alias: {:?} vs {:?}", a, b);
            }
        }
    }

    #[test]
    fn origin_has_all_three_boxes() {
        assert_ne!(BackgroundOrigin::BorderBox, BackgroundOrigin::PaddingBox);
        assert_ne!(BackgroundOrigin::PaddingBox, BackgroundOrigin::ContentBox);
    }
}

#[cfg(test)]
mod animation_family_tests {
    use super::*;

    #[test]
    fn css_initial_values_are_the_derived_defaults() {
        // These defaults are the CSS initial values, so a wrong #[default]
        // would silently change every element that never sets the property.
        assert_eq!(TimingFunction::default(), TimingFunction::Ease);
        assert_eq!(AnimationFillMode::default(), AnimationFillMode::None);
        assert_eq!(AnimationPlayState::default(), AnimationPlayState::Running);
        assert_eq!(AnimationDirection::default(), AnimationDirection::Normal);
        assert_eq!(AnimationIterationCount::default(), AnimationIterationCount::One);
    }

    #[test]
    fn steps_carries_its_count_and_jump_flag_independently() {
        // Steps(count, jump_start): the bool is not a formality -- steps(2,
        // jump-start) and steps(2, jump-end) render differently, so a port
        // that dropped or aliased the flag must fail here.
        let jump_start = TimingFunction::Steps(2, true);
        let jump_end = TimingFunction::Steps(2, false);
        assert_ne!(jump_start, jump_end);
        if let TimingFunction::Steps(n, jump) = jump_start {
            assert_eq!(n, 2);
            assert!(jump);
        } else {
            panic!("expected Steps");
        }
    }

    #[test]
    fn cubic_bezier_keeps_all_four_control_values_in_order() {
        let b = TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0);
        // Reversing the pairs is the classic transcription error and would
        // produce a visibly different easing curve.
        assert_ne!(b, TimingFunction::CubicBezier(0.25, 1.0, 0.25, 0.1));
        assert_eq!(b, TimingFunction::CubicBezier(0.25, 0.1, 0.25, 1.0));
    }

    #[test]
    fn iteration_count_distinguishes_infinite_from_a_finite_count() {
        assert_ne!(
            AnimationIterationCount::Infinite,
            AnimationIterationCount::Count(f32::INFINITY),
            "Infinite is its own variant, not a sentinel float"
        );
        assert_ne!(AnimationIterationCount::One, AnimationIterationCount::Count(1.0));
    }

    #[test]
    fn fractional_iteration_counts_are_representable() {
        // animation-iteration-count: 0.5 is legal CSS and stops the
        // animation halfway -- an integer-typed port would lose it.
        assert_eq!(
            AnimationIterationCount::Count(0.5),
            AnimationIterationCount::Count(0.5)
        );
    }
}

#[cfg(test)]
mod shadow_filter_tests {
    use super::*;

    #[test]
    fn drop_shadow_is_outset_with_no_spread() {
        let s = BoxShadow::drop_shadow(2.0, 4.0, 6.0, Color::BLACK);
        assert_eq!((s.offset_x, s.offset_y, s.blur_radius), (2.0, 4.0, 6.0));
        assert_eq!(s.spread_radius, 0.0);
        assert!(!s.inset, "drop_shadow must not produce an inset shadow");
    }

    #[test]
    fn a_fully_transparent_shadow_is_not_visible() {
        // Guards the alpha half of is_visible: geometry alone must not make
        // a shadow visible, or the renderer draws invisible work.
        let s = BoxShadow {
            offset_x: 10.0,
            offset_y: 10.0,
            blur_radius: 5.0,
            spread_radius: 2.0,
            color: Color::TRANSPARENT,
            inset: false,
        };
        assert!(!s.is_visible());
    }

    #[test]
    fn a_zero_geometry_shadow_is_not_visible_even_when_opaque() {
        // Guards the other half: an opaque colour with no offset, blur or
        // spread paints nothing.
        let s = BoxShadow { color: Color::BLACK, ..Default::default() };
        assert!(!s.is_visible());
    }

    #[test]
    fn spread_alone_makes_a_shadow_visible() {
        // spread_radius uses != 0.0, not > 0.0 -- a NEGATIVE spread still
        // changes rendering, so it must count as visible.
        let s = BoxShadow {
            spread_radius: -3.0,
            color: Color::BLACK,
            ..Default::default()
        };
        assert!(s.is_visible(), "negative spread is still a visible change");
    }

    #[test]
    fn backdrop_filter_none_needs_no_blur() {
        let f = BackdropFilter::None;
        assert!(f.is_none());
        assert!(!f.needs_blur());
    }

    #[test]
    fn zero_radius_blur_needs_no_blur_pass() {
        // Blur(0.0) is a filter that is set but has no effect. Scheduling
        // the GPU blur pass for it would be pure cost.
        let f = BackdropFilter::Blur(0.0);
        assert!(!f.is_none(), "it is still a Blur variant");
        assert!(!f.needs_blur(), "but it must not request a blur pass");
    }

    #[test]
    fn positive_radius_blur_needs_the_blur_pass() {
        assert!(BackdropFilter::Blur(4.0).needs_blur());
    }
}

#[cfg(test)]
mod transform_family_tests {
    use super::*;

    const IDENTITY: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

    #[test]
    fn empty_list_is_identity() {
        let t = TransformList::none();
        assert!(t.is_identity());
        assert_eq!(t.to_matrix(100.0, 100.0), IDENTITY);
    }

    #[test]
    fn translate_lands_in_the_e_f_slots() {
        let t = TransformList {
            ops: vec![TransformOp::Translate(Length::Px(10.0), Length::Px(20.0))],
        };
        assert!(!t.is_identity());
        let m = t.to_matrix(0.0, 0.0);
        assert_eq!((m[4], m[5]), (10.0, 20.0));
    }

    #[test]
    fn scale_lands_in_the_a_d_slots() {
        let t = TransformList {
            ops: vec![TransformOp::Scale(2.0, 3.0)],
        };
        let m = t.to_matrix(0.0, 0.0);
        assert_eq!((m[0], m[3]), (2.0, 3.0));
    }

    #[test]
    fn percentage_translate_resolves_x_against_width_and_y_against_height() {
        // Guards an axis swap, which is silent: a square container would
        // hide it entirely, so the container is deliberately non-square.
        let t = TransformList {
            ops: vec![TransformOp::Translate(
                Length::Percent(50.0),
                Length::Percent(50.0),
            )],
        };
        let m = t.to_matrix(200.0, 80.0);
        assert_eq!(m[4], 100.0, "x% must resolve against container WIDTH");
        assert_eq!(m[5], 40.0, "y% must resolve against container HEIGHT");
    }

    #[test]
    fn composition_order_matters() {
        // The defining property of matrix composition, and the thing a
        // multiply-order bug silently breaks. Asserted as a property rather
        // than against hand-computed numbers so it cannot pass by accident.
        let translate_then_scale = TransformList {
            ops: vec![
                TransformOp::Translate(Length::Px(10.0), Length::Px(0.0)),
                TransformOp::Scale(2.0, 2.0),
            ],
        };
        let scale_then_translate = TransformList {
            ops: vec![
                TransformOp::Scale(2.0, 2.0),
                TransformOp::Translate(Length::Px(10.0), Length::Px(0.0)),
            ],
        };
        assert_ne!(
            translate_then_scale.to_matrix(0.0, 0.0),
            scale_then_translate.to_matrix(0.0, 0.0),
            "composing in the opposite order must not yield the same matrix"
        );
    }

    #[test]
    fn rotate_90_degrees_is_a_quarter_turn() {
        let t = TransformList {
            ops: vec![TransformOp::Rotate(90.0)],
        };
        let m = t.to_matrix(0.0, 0.0);
        // cos(90) == 0, sin(90) == 1 within f32 tolerance.
        assert!(m[0].abs() < 1e-6, "a should be ~0, got {}", m[0]);
        assert!((m[1].abs() - 1.0).abs() < 1e-6, "b should be ~±1, got {}", m[1]);
        assert!(!t.is_identity());
    }

    #[test]
    fn matrix_variant_passes_its_components_through() {
        let t = TransformList {
            ops: vec![TransformOp::Matrix(1.0, 2.0, 3.0, 4.0, 5.0, 6.0)],
        };
        assert_eq!(t.to_matrix(0.0, 0.0), [1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn transform_origin_defaults_to_the_centre() {
        let o = TransformOrigin::default();
        assert_eq!(o.x, Length::Percent(50.0));
        assert_eq!(o.y, Length::Percent(50.0));
    }
}

#[cfg(test)]
mod colorf32_tests {
    use super::*;

    #[test]
    fn round_trips_through_color() {
        let c = Color::new(64, 128, 255, 1.0);
        assert_eq!(ColorF32::from_color(c).to_color(), c);
    }

    #[test]
    fn lerp_premultiplies_and_lerp_straight_does_not() {
        // The whole reason both exist. Interpolating a transparent red with an
        // opaque blue: straight lerp drags the transparent color's RGB into
        // the result even though it contributes no visible ink; premultiplied
        // lerp weights by alpha, so the midpoint stays much closer to blue.
        let transparent_red = ColorF32::new(1.0, 0.0, 0.0, 0.0);
        let opaque_blue = ColorF32::new(0.0, 0.0, 1.0, 1.0);

        let pre = transparent_red.lerp(&opaque_blue, 0.5);
        let straight = transparent_red.lerp_straight(&opaque_blue, 0.5);

        assert_eq!(straight.r, 0.5, "straight lerp carries the invisible red");
        assert!(pre.r < 0.01, "premultiplied lerp must not, got {}", pre.r);
        assert_eq!(pre.a, straight.a, "alpha interpolates the same either way");
    }

    #[test]
    fn lerp_of_fully_transparent_endpoints_is_transparent() {
        // Guards the a <= 0.0001 branch that avoids dividing by zero.
        let a = ColorF32::new(1.0, 0.0, 0.0, 0.0);
        let b = ColorF32::new(0.0, 1.0, 0.0, 0.0);
        let mid = a.lerp(&b, 0.5);
        assert_eq!(mid.a, 0.0);
        assert!(mid.r.is_finite() && mid.g.is_finite() && mid.b.is_finite());
    }

    #[test]
    fn gamma_correct_midpoint_is_brighter_than_naive_midpoint() {
        // Black to white at t=0.5. Interpolating in linear light and
        // converting back lands well above the naive 0.5, which is the
        // entire point of lerp_gamma_correct.
        let black = ColorF32::BLACK;
        let white = ColorF32::WHITE;

        let naive = black.lerp_straight(&white, 0.5);
        let gamma = black.lerp_gamma_correct(&white, 0.5);

        assert_eq!(naive.r, 0.5);
        assert!(
            gamma.r > 0.70 && gamma.r < 0.76,
            "expected the sRGB encoding of linear 0.5 (~0.735), got {}",
            gamma.r
        );
    }

    #[test]
    fn dithering_varies_with_pixel_position() {
        // 0.5 is exactly 127.5 in 8-bit, i.e. sitting ON a rounding
        // boundary. The dither offset spans +/-0.5/255, so roughly half the
        // matrix cells push it below 127.5 and half at or above -- the byte
        // must therefore differ across positions. (Picking a value that is
        // NOT near a boundary, e.g. 0.5 + 0.5/255 = exactly 128.0, makes
        // every cell round the same way and says nothing about the dither.)
        let c = ColorF32::new(0.5, 0.0, 0.0, 1.0);
        let seen: std::collections::HashSet<u8> = (0..4)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .map(|(x, y)| c.to_color_dithered(x, y).r)
            .collect();
        assert!(seen.len() > 1, "dither produced one value: {:?}", seen);
    }

    #[test]
    fn to_array_is_rgba_ordered() {
        let c = ColorF32::new(0.1, 0.2, 0.3, 0.4);
        assert_eq!(c.to_array(), [0.1, 0.2, 0.3, 0.4]);
    }

    #[test]
    fn from_impls_match_the_explicit_conversions() {
        let c = Color::new(10, 20, 30, 0.5);
        let via_trait: ColorF32 = c.into();
        assert_eq!(via_trait, ColorF32::from_color(c));
        let back: Color = via_trait.into();
        assert_eq!(back, via_trait.to_color());
    }
}

#[cfg(test)]
mod length_math_tests {
    use super::*;

    fn px(v: f32) -> Length { Length::Px(v) }

    #[test]
    fn min_returns_the_smaller_operand() {
        let l = Length::Min(Box::new((px(100.0), px(40.0))));
        assert_eq!(l.to_px(16.0, 16.0, 0.0), 40.0);
    }

    #[test]
    fn max_returns_the_larger_operand() {
        let l = Length::Max(Box::new((px(100.0), px(40.0))));
        assert_eq!(l.to_px(16.0, 16.0, 0.0), 100.0);
    }

    #[test]
    fn clamp_bounds_the_preferred_value_from_both_sides() {
        let below = Length::Clamp(Box::new((px(50.0), px(10.0), px(100.0))));
        let inside = Length::Clamp(Box::new((px(50.0), px(75.0), px(100.0))));
        let above = Length::Clamp(Box::new((px(50.0), px(999.0), px(100.0))));
        assert_eq!(below.to_px(16.0, 16.0, 0.0), 50.0, "below min clamps up");
        assert_eq!(inside.to_px(16.0, 16.0, 0.0), 75.0, "inside passes through");
        assert_eq!(above.to_px(16.0, 16.0, 0.0), 100.0, "above max clamps down");
    }

    #[test]
    fn operands_are_resolved_not_assumed_to_be_px() {
        // clamp(1rem, 50%, 20vw) with root 16px, container 200px, viewport 1000px
        // -> min 16, preferred 100, max 200 -> 100.
        let l = Length::Clamp(Box::new((
            Length::Rem(1.0),
            Length::Percent(50.0),
            Length::Vw(20.0),
        )));
        assert_eq!(l.to_px_with_viewport(16.0, 16.0, 200.0, 1000.0, 500.0), 100.0);
    }

    #[test]
    fn math_functions_nest() {
        // max(10px, min(80px, 40px)) -> max(10, 40) -> 40
        let inner = Length::Min(Box::new((px(80.0), px(40.0))));
        let outer = Length::Max(Box::new((px(10.0), inner)));
        assert_eq!(outer.to_px(16.0, 16.0, 0.0), 40.0);
    }

    #[test]
    fn math_functions_are_not_yet_parseable() {
        // Pins the boundary: the variants exist, the parser is untouched.
        // If a later PR wires clamp()/min()/max(), this SHOULD fail.
        assert_eq!(parse_length("clamp(1rem, 2vw, 3rem)"), None);
        assert_eq!(parse_length("min(10px, 2em)"), None);
    }
}

#[cfg(test)]
mod inherit_partition_guard {
    use super::*;

    /// EXHAUSTIVE DESTRUCTURE GUARD.
    ///
    /// `inherit_from` assigns 17 fields from the parent, re-initialises 11
    /// explicitly, and lets the remaining 71 fall through
    /// `..Default::default()`. That tail is the hazard: add a NEW field to
    /// `ComputedStyle` that CSS says should inherit, and it silently will not
    /// - `..Default::default()` swallows it, every existing test still passes,
    /// and the only symptom is a page that renders subtly wrong.
    ///
    /// A measurement whose failure mode is invisible needs a structural guard,
    /// not vigilance. This test destructures `ComputedStyle` EXHAUSTIVELY, so
    /// adding any field FAILS TO COMPILE here until someone writes it into one
    /// of the two lists below - i.e. until a human makes a conscious
    /// inherit / do-not-inherit decision.
    ///
    /// Audited 2026-07-31: none of the 71 fall-through fields is a property CSS
    /// defines as inherited. The partition is correct TODAY; this keeps it
    /// correct.
    #[test]
    fn every_field_has_a_conscious_inheritance_decision() {
        // THE FIXTURE IS THE TEST. A parent built from ComputedStyle::new()
        // makes this assertion half VACUOUS: new().color is already BLACK, so
        // "inherited BLACK" and "did not inherit and defaulted to BLACK" are
        // indistinguishable. Verified by mutation on 2026-07-31 - breaking
        // inherit_from so it stopped inheriting `color` left this test GREEN.
        //
        // Every inherited field below therefore carries a value that is NOT
        // its default, so a field that stops inheriting changes observably.
        let mut parent = ComputedStyle::new();
        parent.color = Color::WHITE;
        parent.font_size = Length::Px(37.0);
        parent.font_weight = FontWeight(825);
        parent.font_style = FontStyle::Italic;
        parent.font_stretch = FontStretch::UltraCondensed;
        parent.font_family = "guard-sentinel-family".to_string();
        parent.line_height = 3.75;
        parent.text_align = TextAlign::Center;
        parent.letter_spacing = Length::Px(7.0);
        parent.word_spacing = Length::Px(9.0);
        parent.text_indent = Length::Px(11.0);
        parent.text_transform = TextTransform::Uppercase;
        parent.white_space = WhiteSpace::Pre;
        parent.word_break = WordBreak::BreakAll;
        parent.direction = Direction::Rtl;
        parent.writing_mode = WritingMode::VerticalRl;
        {
            let mut props = std::collections::HashMap::new();
            props.insert("--guard-sentinel".to_string(), "1".to_string());
            parent.custom_properties = std::sync::Arc::new(props);
        }

        let child = ComputedStyle::inherit_from(&parent);

        // Exhaustive: no `..` rest pattern. A new field breaks this line.
        let ComputedStyle {
            transform,
            transform_origin,
            box_shadows,
            transition_property,
            transition_duration,
            transition_timing_function,
            transition_delay,
            animation_name,
            animation_duration,
            animation_timing_function,
            animation_delay,
            animation_iteration_count,
            animation_direction,
            animation_fill_mode,
            animation_play_state,
            display,
            position,
            top,
            right,
            bottom,
            left,
            z_index,
            width,
            height,
            min_width,
            min_height,
            max_width,
            max_height,
            margin_top,
            margin_right,
            margin_bottom,
            margin_left,
            padding_top,
            padding_right,
            padding_bottom,
            padding_left,
            border_top_width,
            border_right_width,
            border_bottom_width,
            border_left_width,
            border_top_color,
            border_right_color,
            border_bottom_color,
            border_left_color,
            color,
            background_color,
            font_size,
            font_weight,
            font_style,
            font_family,
            line_height,
            text_align,
            font_stretch,
            letter_spacing,
            word_spacing,
            text_indent,
            text_decoration_line,
            text_decoration_color,
            text_decoration_style,
            text_decoration_thickness,
            text_transform,
            white_space,
            word_break,
            vertical_align,
            writing_mode,
            direction,
            opacity,
            overflow_x,
            overflow_y,
            flex_direction,
            flex_wrap,
            justify_content,
            align_items,
            align_content,
            row_gap,
            column_gap,
            order,
            flex_grow,
            flex_shrink,
            flex_basis,
            align_self,
            scroll_behavior,
            overscroll_behavior_x,
            overscroll_behavior_y,
            scrollbar_width,
            scrollbar_gutter,
            scrollbar_color,
            grid_template_columns,
            grid_template_rows,
            grid_template_areas,
            grid_auto_columns,
            grid_auto_rows,
            grid_auto_flow,
            grid_column_start,
            grid_column_end,
            grid_row_start,
            grid_row_end,
            justify_items,
            justify_self,
            custom_properties,
            background_gradient,
            background_radial_gradient,
            background_clip,
            box_sizing,
        } = child;

        // Silence unused-binding warnings for the deliberately non-inherited
        // tail; the binding itself is what enforces exhaustiveness.
        let _ = &transform;
        let _ = &transform_origin;
        let _ = &box_shadows;
        let _ = &transition_property;
        let _ = &transition_duration;
        let _ = &transition_timing_function;
        let _ = &transition_delay;
        let _ = &animation_name;
        let _ = &animation_duration;
        let _ = &animation_timing_function;
        let _ = &animation_delay;
        let _ = &animation_iteration_count;
        let _ = &animation_direction;
        let _ = &animation_fill_mode;
        let _ = &animation_play_state;
        let _ = &display;
        let _ = &position;
        // NOT inherited: CSS position offsets and z-index apply to the element
        // that declares them. A child of an `top: 10px` element does not
        // inherit that offset.
        let _ = &top;
        let _ = &right;
        let _ = &bottom;
        let _ = &left;
        let _ = &z_index;
        let _ = &width;
        let _ = &height;
        let _ = &min_width;
        let _ = &min_height;
        let _ = &max_width;
        let _ = &max_height;
        let _ = &margin_top;
        let _ = &margin_right;
        let _ = &margin_bottom;
        let _ = &margin_left;
        let _ = &padding_top;
        let _ = &padding_right;
        let _ = &padding_bottom;
        let _ = &padding_left;
        let _ = &border_top_width;
        let _ = &border_right_width;
        let _ = &border_bottom_width;
        let _ = &border_left_width;
        let _ = &border_top_color;
        let _ = &border_right_color;
        let _ = &border_bottom_color;
        let _ = &border_left_color;
        let _ = &background_color;
        let _ = &text_decoration_line;
        let _ = &text_decoration_color;
        let _ = &text_decoration_style;
        let _ = &text_decoration_thickness;
        let _ = &vertical_align;
        let _ = &opacity;
        let _ = &overflow_x;
        let _ = &overflow_y;
        let _ = &flex_direction;
        let _ = &flex_wrap;
        let _ = &justify_content;
        let _ = &align_items;
        let _ = &align_content;
        let _ = &row_gap;
        let _ = &column_gap;
        let _ = &order;
        let _ = &flex_grow;
        let _ = &flex_shrink;
        let _ = &flex_basis;
        let _ = &align_self;
        let _ = &scroll_behavior;
        let _ = &overscroll_behavior_x;
        let _ = &overscroll_behavior_y;
        let _ = &scrollbar_width;
        let _ = &scrollbar_gutter;
        let _ = &scrollbar_color;
        let _ = &grid_template_columns;
        let _ = &grid_template_rows;
        let _ = &grid_template_areas;
        let _ = &grid_auto_columns;
        let _ = &grid_auto_rows;
        let _ = &grid_auto_flow;
        let _ = &grid_column_start;
        let _ = &grid_column_end;
        let _ = &grid_row_start;
        let _ = &grid_row_end;
        let _ = &justify_items;
        let _ = &justify_self;
        let _ = &background_gradient;
        let _ = &background_radial_gradient;
        let _ = &background_clip;
        let _ = &box_sizing;

        // The 17 inherited properties must equal the parent's.
        assert_eq!(color, parent.color, "color must inherit");
        assert_eq!(custom_properties, parent.custom_properties, "custom_properties must inherit");
        assert_eq!(direction, parent.direction, "direction must inherit");
        assert_eq!(font_family, parent.font_family, "font_family must inherit");
        assert_eq!(font_size, parent.font_size, "font_size must inherit");
        assert_eq!(font_stretch, parent.font_stretch, "font_stretch must inherit");
        assert_eq!(font_style, parent.font_style, "font_style must inherit");
        assert_eq!(font_weight, parent.font_weight, "font_weight must inherit");
        assert_eq!(letter_spacing, parent.letter_spacing, "letter_spacing must inherit");
        assert_eq!(line_height, parent.line_height, "line_height must inherit");
        assert_eq!(text_align, parent.text_align, "text_align must inherit");
        assert_eq!(text_indent, parent.text_indent, "text_indent must inherit");
        assert_eq!(text_transform, parent.text_transform, "text_transform must inherit");
        assert_eq!(white_space, parent.white_space, "white_space must inherit");
        assert_eq!(word_break, parent.word_break, "word_break must inherit");
        assert_eq!(word_spacing, parent.word_spacing, "word_spacing must inherit");
        assert_eq!(writing_mode, parent.writing_mode, "writing_mode must inherit");
    }
}

#[cfg(test)]
mod initial_value_guard {
    use super::*;

    /// EXHAUSTIVE DESTRUCTURE GUARD on the CSS INITIAL VALUES.
    ///
    /// Sibling of `every_field_has_a_conscious_inheritance_decision`. That one
    /// guards `inherit_from`; this one guards `new()`. Both end in
    /// `..Default::default()`, and the derived `Default` is actively dangerous
    /// for layout and paint:
    ///
    ///   `Length::default()` is `Zero`  - not `Auto`
    ///   `Color::default()`  is opaque BLACK - not `TRANSPARENT`
    ///   `f32::default()`    is `0.0` - not `1.0` for opacity
    ///
    /// This is not hypothetical. It has already shipped twice:
    ///
    ///   - Windows, 2026-07-07: `Length::Zero` as the width default laid every
    ///     unstyled element out at width 0 - the zero-width tree in that day's
    ///     parity baseline. The scar comment above `width:` in `new()` is that
    ///     incident.
    ///   - hiwave-linux, 2026-07-31: `inherit_from` fell through to `Default`
    ///     for the same fields. Every inheriting element would have been 0x0,
    ///     opaque black and invisible ON EVERY PAGE - and all eight of that
    ///     PR's tests passed. Caught by Argos and Talos probing what the
    ///     function actually returned instead of trusting its name.
    ///
    /// Adding a field to `ComputedStyle` fails to compile here until someone
    /// states its initial value deliberately. A wrong initial is invisible in
    /// unit tests and catastrophic on screen, which is exactly the shape that
    /// needs a structural guard rather than review attention.
    #[test]
    fn every_field_has_a_deliberate_initial_value() {
        let ComputedStyle {
            transform,
            transform_origin,
            box_shadows,
            transition_property,
            transition_duration,
            transition_timing_function,
            transition_delay,
            animation_name,
            animation_duration,
            animation_timing_function,
            animation_delay,
            animation_iteration_count,
            animation_direction,
            animation_fill_mode,
            animation_play_state,
            display,
            position,
            top,
            right,
            bottom,
            left,
            z_index,
            width,
            height,
            min_width,
            min_height,
            max_width,
            max_height,
            margin_top,
            margin_right,
            margin_bottom,
            margin_left,
            padding_top,
            padding_right,
            padding_bottom,
            padding_left,
            border_top_width,
            border_right_width,
            border_bottom_width,
            border_left_width,
            border_top_color,
            border_right_color,
            border_bottom_color,
            border_left_color,
            color,
            background_color,
            font_size,
            font_weight,
            font_style,
            font_family,
            line_height,
            text_align,
            font_stretch,
            letter_spacing,
            word_spacing,
            text_indent,
            text_decoration_line,
            text_decoration_color,
            text_decoration_style,
            text_decoration_thickness,
            text_transform,
            white_space,
            word_break,
            vertical_align,
            writing_mode,
            direction,
            opacity,
            overflow_x,
            overflow_y,
            flex_direction,
            flex_wrap,
            justify_content,
            align_items,
            align_content,
            row_gap,
            column_gap,
            order,
            flex_grow,
            flex_shrink,
            flex_basis,
            align_self,
            scroll_behavior,
            overscroll_behavior_x,
            overscroll_behavior_y,
            scrollbar_width,
            scrollbar_gutter,
            scrollbar_color,
            grid_template_columns,
            grid_template_rows,
            grid_template_areas,
            grid_auto_columns,
            grid_auto_rows,
            grid_auto_flow,
            grid_column_start,
            grid_column_end,
            grid_row_start,
            grid_row_end,
            justify_items,
            justify_self,
            custom_properties,
            background_gradient,
            background_radial_gradient,
            background_clip,
            box_sizing,
        } = ComputedStyle::new();

        // --- The layout-critical initials. Length::default() is Zero, so each
        // of these must be set EXPLICITLY in new(); falling through gives a
        // zero-sized box. This is the 2026-07-07 regression, pinned.
        assert_eq!(width, Length::Auto, "width initial is auto, not 0");
        assert_eq!(height, Length::Auto, "height initial is auto, not 0");
        assert_eq!(max_width, Length::Auto, "max-width initial is none, not 0");
        assert_eq!(max_height, Length::Auto, "max-height initial is none, not 0");

        // min-width/min-height DO fall through, and that is correct: the CSS
        // 2.1 initial for both IS 0, unlike width/height. Asserted so the
        // difference is a recorded decision rather than an oversight that
        // happens to be right.
        assert_eq!(min_width, Length::Zero, "min-width initial IS 0 - correct fall-through");
        // Offsets: the CSS initial is `auto`, represented as None. These DO
        // fall through ..Default::default() and that is correct, because
        // Option::default() is None - unlike Length::default(), which is Zero
        // and would have meant "pinned to the edge" instead of "auto".
        // Asserted so the difference is a recorded decision, as with min-width.
        assert_eq!(top, None, "top initial is auto (None), not 0");
        assert_eq!(right, None, "right initial is auto (None), not 0");
        assert_eq!(bottom, None, "bottom initial is auto (None), not 0");
        assert_eq!(left, None, "left initial is auto (None), not 0");
        assert_eq!(z_index, 0, "z-index initial is auto, stored as 0");
        assert_eq!(min_height, Length::Zero, "min-height initial IS 0 - correct fall-through");

        // --- The paint-critical initials. Color::default() is opaque BLACK and
        // f32::default() is 0.0, so falling through paints a black box at zero
        // opacity. That is the Linux 2026-07-31 blank-page defect.
        assert_eq!(background_color, Color::TRANSPARENT, "background initial is transparent, not black");
        assert_eq!(opacity, 1.0, "opacity initial is 1.0, not 0.0");
        assert_eq!(color, Color::BLACK, "color initial IS black");

        // --- Remaining explicit initials in new().
        assert_eq!(font_size, Length::Px(16.0), "font-size initial is 16px");
        assert_eq!(flex_shrink, 1.0, "flex-shrink initial is 1, not 0");
        assert_eq!(font_family, "sans-serif", "font-family initial");
        assert_eq!(text_decoration_line, TextDecorationLine::NONE, "text-decoration initial");

        // Deliberately-defaulted tail. Bound so the destructure stays
        // exhaustive; a new field lands here only after someone reads the
        // doc comment above and decides it belongs here.
        let _ = &transform;
        let _ = &transform_origin;
        let _ = &box_shadows;
        let _ = &transition_property;
        let _ = &transition_duration;
        let _ = &transition_timing_function;
        let _ = &transition_delay;
        let _ = &animation_name;
        let _ = &animation_duration;
        let _ = &animation_timing_function;
        let _ = &animation_delay;
        let _ = &animation_iteration_count;
        let _ = &animation_direction;
        let _ = &animation_fill_mode;
        let _ = &animation_play_state;
        let _ = &display;
        let _ = &position;
        let _ = &margin_top;
        let _ = &margin_right;
        let _ = &margin_bottom;
        let _ = &margin_left;
        let _ = &padding_top;
        let _ = &padding_right;
        let _ = &padding_bottom;
        let _ = &padding_left;
        let _ = &border_top_width;
        let _ = &border_right_width;
        let _ = &border_bottom_width;
        let _ = &border_left_width;
        let _ = &border_top_color;
        let _ = &border_right_color;
        let _ = &border_bottom_color;
        let _ = &border_left_color;
        let _ = &font_weight;
        let _ = &font_style;
        let _ = &line_height;
        let _ = &text_align;
        let _ = &font_stretch;
        let _ = &letter_spacing;
        let _ = &word_spacing;
        let _ = &text_indent;
        let _ = &text_decoration_color;
        let _ = &text_decoration_style;
        let _ = &text_decoration_thickness;
        let _ = &text_transform;
        let _ = &white_space;
        let _ = &word_break;
        let _ = &vertical_align;
        let _ = &writing_mode;
        let _ = &direction;
        let _ = &overflow_x;
        let _ = &overflow_y;
        let _ = &flex_direction;
        let _ = &flex_wrap;
        let _ = &justify_content;
        let _ = &align_items;
        let _ = &align_content;
        let _ = &row_gap;
        let _ = &column_gap;
        let _ = &order;
        let _ = &flex_grow;
        let _ = &flex_basis;
        let _ = &align_self;
        let _ = &scroll_behavior;
        let _ = &overscroll_behavior_x;
        let _ = &overscroll_behavior_y;
        let _ = &scrollbar_width;
        let _ = &scrollbar_gutter;
        let _ = &scrollbar_color;
        let _ = &grid_template_columns;
        let _ = &grid_template_rows;
        let _ = &grid_template_areas;
        let _ = &grid_auto_columns;
        let _ = &grid_auto_rows;
        let _ = &grid_auto_flow;
        let _ = &grid_column_start;
        let _ = &grid_column_end;
        let _ = &grid_row_start;
        let _ = &grid_row_end;
        let _ = &justify_items;
        let _ = &justify_self;
        let _ = &custom_properties;
        let _ = &background_gradient;
        let _ = &background_radial_gradient;
        let _ = &background_clip;
        let _ = &box_sizing;
    }
}
