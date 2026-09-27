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

use rustkit_cssparser::parse_stylesheet;
use thiserror::Error;
use tracing::debug;

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
    pub const TRANSPARENT: ColorF32 = ColorF32 {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 0.0,
    };
    pub const BLACK: ColorF32 = ColorF32 {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    pub const WHITE: ColorF32 = ColorF32 {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    };

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
            [0.0 / 16.0, 8.0 / 16.0, 2.0 / 16.0, 10.0 / 16.0],
            [12.0 / 16.0, 4.0 / 16.0, 14.0 / 16.0, 6.0 / 16.0],
            [3.0 / 16.0, 11.0 / 16.0, 1.0 / 16.0, 9.0 / 16.0],
            [15.0 / 16.0, 7.0 / 16.0, 13.0 / 16.0, 5.0 / 16.0],
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
            ColorF32 {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }
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
            ColorF32 {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }
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

/// The normal form css-values-3 §8.1 reduces a `calc()` over lengths and
/// percentages to: one coefficient per unit, summed.
///
/// `calc()` over lengths is *linear* — `+`/`-` between terms, and `*`/`/` only
/// by plain numbers — so an expression tree buys nothing a sum of coefficients
/// does not already carry, and the sum resolves in one pass once the
/// percentage basis is known. `calc(100% - 84px)` is `{ percent: 100.0,
/// px: -84.0 }`.
///
/// Only produced where the expression genuinely MIXES units: a `calc()` whose
/// terms all reduce to one unit collapses back to that unit's `Length`
/// variant (see `CalcSum::into_length`), so `calc(2 * 50px)` stays
/// `Length::Px(100.0)` and every existing match site keeps working on it.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CalcSum {
    /// Absolute px coefficient.
    pub px: f32,
    /// Percentage coefficient, in percent (100.0 is `100%`).
    pub percent: f32,
    /// `em` coefficient.
    pub em: f32,
    /// `rem` coefficient.
    pub rem: f32,
    /// `vw` coefficient.
    pub vw: f32,
    /// `vh` coefficient.
    pub vh: f32,
    /// `vmin` coefficient.
    pub vmin: f32,
    /// `vmax` coefficient.
    pub vmax: f32,
}

impl CalcSum {
    fn scaled(self, k: f32) -> Self {
        CalcSum {
            px: self.px * k,
            percent: self.percent * k,
            em: self.em * k,
            rem: self.rem * k,
            vw: self.vw * k,
            vh: self.vh * k,
            vmin: self.vmin * k,
            vmax: self.vmax * k,
        }
    }

    fn add(self, other: Self, sign: f32) -> Self {
        CalcSum {
            px: self.px + sign * other.px,
            percent: self.percent + sign * other.percent,
            em: self.em + sign * other.em,
            rem: self.rem + sign * other.rem,
            vw: self.vw + sign * other.vw,
            vh: self.vh + sign * other.vh,
            vmin: self.vmin + sign * other.vmin,
            vmax: self.vmax + sign * other.vmax,
        }
    }

    fn terms(&self) -> [f32; 8] {
        [
            self.px,
            self.percent,
            self.em,
            self.rem,
            self.vw,
            self.vh,
            self.vmin,
            self.vmax,
        ]
    }

    /// Collapse to a plain `Length` where the sum uses at most one unit.
    ///
    /// This is what keeps the blast radius of `Length::Calc` to the values
    /// that are actually broken without it. Before this variant existed
    /// `parse_length` returned `None` for any `calc()` it could not read as a
    /// single value, so the declaration was DROPPED — a `height:
    /// calc(100% - 84px)` became `auto`. Single-unit expressions were already
    /// handled, and they stay on their old variant here, so no site that
    /// matches `Length::Px` or `Length::Percent` loses a value it used to see.
    fn into_length(self) -> Length {
        let terms = self.terms();
        let nonzero = terms.iter().filter(|c| **c != 0.0).count();
        if nonzero > 1 {
            return Length::Calc(Box::new(self));
        }
        match () {
            _ if self.percent != 0.0 => Length::Percent(self.percent),
            _ if self.em != 0.0 => Length::Em(self.em),
            _ if self.rem != 0.0 => Length::Rem(self.rem),
            _ if self.vw != 0.0 => Length::Vw(self.vw),
            _ if self.vh != 0.0 => Length::Vh(self.vh),
            _ if self.vmin != 0.0 => Length::Vmin(self.vmin),
            _ if self.vmax != 0.0 => Length::Vmax(self.vmax),
            // px last, and it also carries the all-zero case: `calc(0px)` and
            // `calc(10px - 10px)` are both a definite zero length, which is
            // `Px(0.0)` and NOT `Length::Zero`'s default-ness.
            _ => Length::Px(self.px),
        }
    }
}

/// A CSS length value.
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
    /// `fit-content` — css-sizing-3 §4.1.
    ///
    /// Content-sized like `auto`, but it is NOT `auto`, and that distinction is
    /// the whole reason the keyword exists: a grid or flex item only stretches
    /// to its area when its size is `auto`, so `height: fit-content` is how a
    /// page opts one item out of stretching. Parsing it away as `auto` (which
    /// is what happened before this variant existed — `parse_length` returned
    /// `None` and the declaration was dropped) makes the item stretch, which is
    /// the opposite of what it asks for.
    ///
    /// Everywhere that sizes content it behaves exactly as `auto`; only the
    /// stretch decision may tell the two apart.
    FitContent,
    /// Zero.
    #[default]
    Zero,
    /// min(a, b) - returns the smaller of two lengths.
    Min(Box<(Length, Length)>),
    /// max(a, b) - returns the larger of two lengths.
    Max(Box<(Length, Length)>),
    /// clamp(min, preferred, max) - clamps preferred between min and max.
    Clamp(Box<(Length, Length, Length)>),
    /// `calc()` over more than one unit, in css-values-3 §8.1 normal form.
    ///
    /// A `calc()` that reduces to a single unit is NOT this variant — see
    /// `CalcSum::into_length`.
    Calc(Box<CalcSum>),
}

impl Length {
    /// Compute the absolute pixel value.
    ///
    /// For viewport units, pass viewport dimensions via `viewport_width` and `viewport_height`.
    pub fn to_px(&self, font_size: f32, root_font_size: f32, container_size: f32) -> f32 {
        self.to_px_with_viewport(font_size, root_font_size, container_size, 0.0, 0.0)
    }

    /// Compute the absolute pixel value with viewport dimensions for vh/vw units.
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
            Length::FitContent => 0.0, // Context-dependent, exactly as Auto
            Length::Zero => 0.0,
            Length::Min(pair) => {
                let a = pair.0.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                let b = pair.1.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                a.min(b)
            }
            Length::Max(pair) => {
                let a = pair.0.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                let b = pair.1.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                a.max(b)
            }
            Length::Clamp(triple) => {
                let min_val = triple.0.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                let pref = triple.1.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                let max_val = triple.2.to_px_with_viewport(
                    font_size,
                    root_font_size,
                    container_size,
                    viewport_width,
                    viewport_height,
                );
                pref.clamp(min_val, max_val)
            }
            Length::Calc(sum) => {
                sum.px
                    + sum.percent / 100.0 * container_size
                    + sum.em * font_size
                    + sum.rem * root_font_size
                    + sum.vw / 100.0 * viewport_width
                    + sum.vh / 100.0 * viewport_height
                    + sum.vmin / 100.0 * viewport_width.min(viewport_height)
                    + sum.vmax / 100.0 * viewport_width.max(viewport_height)
            }
        }
    }
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
        self.color.a > 0.0
            && (self.offset_x != 0.0
                || self.offset_y != 0.0
                || self.blur_radius > 0.0
                || self.spread_radius != 0.0)
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

/// Position along a gradient stop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StopPosition {
    /// Percentage position (0.0 to 1.0).
    Percent(f32),
    /// Pixel position along the gradient line.
    Pixels(f32),
}

impl StopPosition {
    /// Convert to a normalized 0-1 position given the gradient line length in pixels.
    /// For percentage values, returns the percentage directly.
    /// For pixel values, divides by the gradient line length.
    pub fn to_normalized(&self, gradient_length: f32) -> f32 {
        match self {
            StopPosition::Percent(p) => *p,
            StopPosition::Pixels(px) => {
                if gradient_length > 0.0 {
                    *px / gradient_length
                } else {
                    0.0
                }
            }
        }
    }

    /// Get the raw value (for calculating repeat length in pixels).
    pub fn raw_value(&self) -> f32 {
        match self {
            StopPosition::Percent(p) => *p,
            StopPosition::Pixels(px) => *px,
        }
    }

    /// Check if this is a pixel-based position.
    pub fn is_pixels(&self) -> bool {
        matches!(self, StopPosition::Pixels(_))
    }
}

/// A color stop for gradients.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorStop {
    /// The color at this stop.
    pub color: Color,
    /// Position along the gradient (percentage 0.0-1.0, pixels, or None for auto).
    pub position: Option<StopPosition>,
}

impl ColorStop {
    pub fn new(color: Color, position: Option<f32>) -> Self {
        Self {
            color,
            position: position.map(StopPosition::Percent),
        }
    }

    /// Create a color stop with a pixel position.
    pub fn with_pixels(color: Color, pixels: f32) -> Self {
        Self {
            color,
            position: Some(StopPosition::Pixels(pixels)),
        }
    }

    /// Create a color stop with a percentage position.
    pub fn with_percent(color: Color, percent: f32) -> Self {
        Self {
            color,
            position: Some(StopPosition::Percent(percent)),
        }
    }
}

/// Direction for linear gradients.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum GradientDirection {
    /// Angle in degrees (0 = to top, 90 = to right, 180 = to bottom, 270 = to left).
    Angle(f32),
    /// To top (0deg).
    #[default]
    ToTop,
    /// To right (90deg).
    ToRight,
    /// To bottom (180deg).
    ToBottom,
    /// To left (270deg).
    ToLeft,
    /// To top-right (45deg).
    ToTopRight,
    /// To top-left (315deg).
    ToTopLeft,
    /// To bottom-right (135deg).
    ToBottomRight,
    /// To bottom-left (225deg).
    ToBottomLeft,
}

impl GradientDirection {
    /// Convert to angle in degrees.
    pub fn to_degrees(&self) -> f32 {
        match self {
            GradientDirection::Angle(deg) => *deg,
            GradientDirection::ToTop => 0.0,
            GradientDirection::ToRight => 90.0,
            GradientDirection::ToBottom => 180.0,
            GradientDirection::ToLeft => 270.0,
            GradientDirection::ToTopRight => 45.0,
            GradientDirection::ToTopLeft => 315.0,
            GradientDirection::ToBottomRight => 135.0,
            GradientDirection::ToBottomLeft => 225.0,
        }
    }
}

/// A CSS linear gradient.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearGradient {
    /// Direction of the gradient.
    pub direction: GradientDirection,
    /// Color stops.
    pub stops: Vec<ColorStop>,
    /// Whether this is a repeating gradient.
    pub repeating: bool,
}

impl LinearGradient {
    pub fn new(direction: GradientDirection, stops: Vec<ColorStop>) -> Self {
        Self {
            direction,
            stops,
            repeating: false,
        }
    }

    pub fn new_repeating(direction: GradientDirection, stops: Vec<ColorStop>) -> Self {
        Self {
            direction,
            stops,
            repeating: true,
        }
    }
}

/// A CSS radial gradient.
#[derive(Debug, Clone, PartialEq)]
pub struct RadialGradient {
    /// Shape: "circle" or "ellipse".
    pub shape: RadialShape,
    /// Size of the gradient.
    pub size: RadialSize,
    /// Center position (0.0 to 1.0, default 0.5).
    pub center: (f32, f32),
    /// Color stops.
    pub stops: Vec<ColorStop>,
    /// Whether this is a repeating gradient.
    pub repeating: bool,
}

impl RadialGradient {
    pub fn new(
        shape: RadialShape,
        size: RadialSize,
        center: (f32, f32),
        stops: Vec<ColorStop>,
    ) -> Self {
        Self {
            shape,
            size,
            center,
            stops,
            repeating: false,
        }
    }

    pub fn new_repeating(
        shape: RadialShape,
        size: RadialSize,
        center: (f32, f32),
        stops: Vec<ColorStop>,
    ) -> Self {
        Self {
            shape,
            size,
            center,
            stops,
            repeating: true,
        }
    }
}

/// A CSS conic gradient.
#[derive(Debug, Clone, PartialEq)]
pub struct ConicGradient {
    /// Starting angle in degrees (default 0, pointing up).
    pub from_angle: f32,
    /// Center position (0.0 to 1.0, default 0.5).
    pub center: (f32, f32),
    /// Color stops (positions are in degrees or percentages).
    pub stops: Vec<ColorStop>,
    /// Whether this is a repeating gradient.
    pub repeating: bool,
}

impl ConicGradient {
    pub fn new(from_angle: f32, center: (f32, f32), stops: Vec<ColorStop>) -> Self {
        Self {
            from_angle,
            center,
            stops,
            repeating: false,
        }
    }

    pub fn new_repeating(from_angle: f32, center: (f32, f32), stops: Vec<ColorStop>) -> Self {
        Self {
            from_angle,
            center,
            stops,
            repeating: true,
        }
    }
}

/// Shape of a radial gradient.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RadialShape {
    /// Circle (equal radius in all directions).
    Circle,
    /// Ellipse (can stretch in one direction).
    #[default]
    Ellipse,
}

/// Size of a radial gradient.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RadialSize {
    /// Closest side.
    ClosestSide,
    /// Farthest side.
    #[default]
    FarthestSide,
    /// Closest corner.
    ClosestCorner,
    /// Farthest corner.
    FarthestCorner,
    /// Explicit radius (for circles) or radii (for ellipses).
    Explicit(f32, f32),
}

/// A CSS gradient (linear, radial, or conic).
#[derive(Debug, Clone, PartialEq)]
pub enum Gradient {
    Linear(LinearGradient),
    Radial(RadialGradient),
    Conic(ConicGradient),
}

// ==================== Background Layer Types ====================

/// The image source for a background layer.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundImage {
    /// No image (transparent).
    None,
    /// A gradient.
    Gradient(Gradient),
    /// A URL reference to an image.
    Url(String),
}

impl Default for BackgroundImage {
    fn default() -> Self {
        BackgroundImage::None
    }
}

/// Background size specification.
#[derive(Debug, Clone, PartialEq)]
pub enum BackgroundSize {
    /// Stretch to cover the entire area.
    Cover,
    /// Scale to fit within the area.
    Contain,
    /// Explicit width and height (None = auto for that dimension).
    Explicit {
        width: Option<f32>,
        height: Option<f32>,
    },
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

/// A single background layer combining image, position, size, and repeat.
#[derive(Debug, Clone, PartialEq)]
pub struct BackgroundLayer {
    /// The background image (gradient or url).
    pub image: BackgroundImage,
    /// Positioning within the element.
    pub position: BackgroundPosition,
    /// How the background is sized.
    pub size: BackgroundSize,
    /// How the background repeats.
    pub repeat: BackgroundRepeat,
    /// Where the background positioning area starts.
    pub origin: BackgroundOrigin,
    /// Where the background is clipped.
    pub clip: BackgroundClip,
}

impl Default for BackgroundLayer {
    fn default() -> Self {
        BackgroundLayer {
            image: BackgroundImage::None,
            position: BackgroundPosition::default(),
            size: BackgroundSize::Auto,
            repeat: BackgroundRepeat::Repeat,
            origin: BackgroundOrigin::PaddingBox,
            clip: BackgroundClip::BorderBox,
        }
    }
}

impl BackgroundLayer {
    /// Create a new background layer with a gradient.
    pub fn from_gradient(gradient: Gradient) -> Self {
        BackgroundLayer {
            image: BackgroundImage::Gradient(gradient),
            ..Default::default()
        }
    }

    /// Create a new background layer with a URL.
    pub fn from_url(url: String) -> Self {
        BackgroundLayer {
            image: BackgroundImage::Url(url),
            ..Default::default()
        }
    }

    /// Check if this layer has a visible image.
    pub fn has_image(&self) -> bool {
        !matches!(self.image, BackgroundImage::None)
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

    /// Check if this is an inline-level display (inline, inline-block, inline-flex, inline-grid).
    pub fn is_inline_level(self) -> bool {
        matches!(
            self,
            Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid
        )
    }

    /// Check if this is inline-block.
    pub fn is_inline_block(self) -> bool {
        matches!(self, Display::InlineBlock)
    }

    /// Check if this is an atomic inline-level box (inline-block, inline-flex,
    /// inline-grid): participates in inline flow as a single opaque box while
    /// laying out its own contents with its inner display type (CSS Display 3
    /// §2.4).
    pub fn is_atomic_inline(self) -> bool {
        matches!(
            self,
            Display::InlineBlock | Display::InlineFlex | Display::InlineGrid
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
        matches!(
            self,
            FlexDirection::RowReverse | FlexDirection::ColumnReverse
        )
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

    /// Expand repeat() patterns into a flat list of track definitions.
    ///
    /// This handles `repeat(N, ...)` patterns by expanding them inline.
    /// For `auto-fill` and `auto-fit`, returns them unexpanded (handled at layout time
    /// when container size is known).
    ///
    /// # Returns
    /// A tuple of (expanded_tracks, has_auto_repeat) where:
    /// - expanded_tracks: All tracks with Count repeats expanded
    /// - has_auto_repeat: Whether an auto-fill/auto-fit needs layout-time expansion
    pub fn expand_tracks(&self) -> (Vec<TrackDefinition>, Option<&TrackRepeat>) {
        if self.repeats.is_empty() {
            return (self.tracks.clone(), None);
        }

        let mut result = Vec::new();
        let mut auto_repeat = None;
        let mut track_idx = 0;

        // Sort repeats by insert position
        let mut sorted_repeats: Vec<_> = self.repeats.iter().collect();
        sorted_repeats.sort_by_key(|(pos, _)| *pos);

        for (insert_pos, repeat) in &sorted_repeats {
            // Add any tracks before this repeat position
            while track_idx < *insert_pos && track_idx < self.tracks.len() {
                result.push(self.tracks[track_idx].clone());
                track_idx += 1;
            }

            match repeat {
                TrackRepeat::Count(count, tracks) => {
                    // Expand: repeat(N, track1 track2...) → N copies of the track list
                    for _ in 0..*count {
                        for track in tracks {
                            result.push(track.clone());
                        }
                    }
                }
                TrackRepeat::AutoFill(_) | TrackRepeat::AutoFit(_) => {
                    // Auto-fill/auto-fit need container size to expand
                    // Store for layout-time handling
                    auto_repeat = Some(repeat);
                }
            }
        }

        // Add remaining tracks after last repeat
        while track_idx < self.tracks.len() {
            result.push(self.tracks[track_idx].clone());
            track_idx += 1;
        }

        (result, auto_repeat)
    }

    /// Get the number of tracks after repeat expansion.
    /// For auto-fill/auto-fit, returns the count with repeat not expanded.
    pub fn expanded_track_count(&self) -> usize {
        let (expanded, _) = self.expand_tracks();
        expanded.len()
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
                .map(|s| if s == "." { None } else { Some(s.to_string()) })
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
                        let (row_end, col_end) =
                            Self::find_area_extent(&rows, row_idx, col_idx, name);
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

    fn find_area_extent(
        rows: &[Vec<Option<String>>],
        start_row: usize,
        start_col: usize,
        name: &str,
    ) -> (usize, usize) {
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

/// CSS float property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Float {
    #[default]
    None,
    Left,
    Right,
}

/// CSS clear property values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Clear {
    #[default]
    None,
    Left,
    Right,
    Both,
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

/// Line height values.
///
/// CSS line-height can be:
/// - `normal` - use font metrics (typically ~1.2)
/// - a number (unitless multiplier of font-size)
/// - a length (absolute value like `24px`)
/// - a percentage (of font-size)
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    /// Normal line height (use font metrics, typically ~1.2).
    Normal,
    /// Unitless number (multiplier of font-size).
    Number(f32),
    /// Absolute length in pixels.
    Px(f32),
}

impl Default for LineHeight {
    fn default() -> Self {
        LineHeight::Normal
    }
}

/// Fallback ratio for `line-height: normal` when no font metrics are available.
///
/// CSS says `normal` is derived from the font (ascent + descent + line-gap);
/// this constant is only a stand-in for callers that cannot shape text. Layout
/// must use [`LineHeight::to_px_with_normal`] instead — see its docs.
pub const NORMAL_LINE_HEIGHT_FALLBACK_RATIO: f32 = 1.2;

impl LineHeight {
    /// Compute the line height in pixels, with no font metrics available.
    ///
    /// `Normal` falls back to a flat 1.2 x font-size, which is NOT what the
    /// spec (or Chrome) does. Any caller that can shape text should call
    /// [`LineHeight::to_px_with_normal`] and pass the font's own metrics.
    pub fn to_px(&self, font_size: f32) -> f32 {
        self.to_px_with_normal(font_size, font_size * NORMAL_LINE_HEIGHT_FALLBACK_RATIO)
    }

    /// Compute the line height in pixels, given the font's own `normal` height.
    ///
    /// `normal_px` is the font's ascent + descent + line-gap at this font-size
    /// (rustkit-layout's `TextMetrics::height`). Only `Normal` consults it;
    /// `Number`/`Px` are font-independent by definition.
    ///
    /// The flat-1.2 model this replaces was wrong by up to ~1.2px per line on
    /// the common 16px system-ui case, and the error compounds down the page.
    pub fn to_px_with_normal(&self, font_size: f32, normal_px: f32) -> f32 {
        match self {
            LineHeight::Normal => normal_px,
            LineHeight::Number(n) => font_size * n,
            LineHeight::Px(px) => *px,
        }
    }

    /// Check if this represents a multiplier (Normal or Number).
    pub fn is_multiplier(&self) -> bool {
        matches!(self, LineHeight::Normal | LineHeight::Number(_))
    }

    /// Get the multiplier value, if this is a multiplier type.
    /// Returns None for absolute Px values.
    pub fn as_multiplier(&self) -> Option<f32> {
        match self {
            LineHeight::Normal => Some(1.2),
            LineHeight::Number(n) => Some(*n),
            LineHeight::Px(_) => None,
        }
    }
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

/// `visibility` (CSS 2.1 §11.2). Inherited. A hidden box still takes up
/// space; it just paints nothing of its own, and a descendant can set
/// `visible` again. `collapse` is treated as `hidden` (no table/flex
/// collapsing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Visibility {
    #[default]
    Visible,
    Hidden,
    Collapse,
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

/// `text-overflow` (css-overflow-3 §5.1): how inline content that overflows
/// its line box in the inline direction is rendered, on a block container
/// whose `overflow` is other than `visible`. Not inherited — the block
/// owns its line boxes, so the block owns the ellipsis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextOverflow {
    /// Overflowing content is simply clipped (initial value).
    #[default]
    Clip,
    /// Overflowing content is cut and `U+2026 …` is painted at the line
    /// box's end edge in its place.
    Ellipsis,
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

/// Overflow-wrap behavior (CSS Text 3 §5.5).
///
/// Distinct from [`WordBreak`]: `word-break` changes where soft wrap
/// opportunities exist in normal text, while `overflow-wrap` only adds
/// last-resort opportunities for words that would otherwise overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverflowWrap {
    #[default]
    Normal,
    BreakWord,
    Anywhere,
}

/// Line-break strictness (CSS Text 3 §5.3).
///
/// Only `anywhere` changes where opportunities exist in a way the line
/// breaker models: a soft wrap opportunity around EVERY typographic
/// character unit, disregarding every prohibition — including
/// `word-break: keep-all`. It is NOT `overflow-wrap: anywhere`: that only
/// breaks a word that would otherwise overflow, whereas `line-break:
/// anywhere` fills each line to the last character that fits (WPT
/// line-break-anywhere-004: "XX XXX" in a 4ch box is "XX X" / "XX", not
/// "XX" / "XXX"). loose/normal/strict are recorded, not distinguished.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineBreak {
    #[default]
    Auto,
    Loose,
    Normal,
    Strict,
    Anywhere,
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

/// Box sizing model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BoxSizing {
    #[default]
    ContentBox,
    BorderBox,
}

/// Background clip mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackgroundClip {
    #[default]
    BorderBox,
    PaddingBox,
    ContentBox,
    /// Clip to text (for gradient text effects).
    Text,
}

/// `border-<side>-style`, as far as paint distinguishes it. The styles paint
/// does not draw yet (double, groove, ridge, inset, outset) stay `Solid` —
/// which is also the default, so a border given only a width keeps painting.
/// `None` covers `none` and `hidden`: the cascade zeroes that side's width
/// once every declaration is in (CSS Backgrounds 3 §3.3), so the order in
/// which width and style were declared does not matter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
    None,
}

impl BorderStyle {
    /// The paint-relevant style named by a CSS keyword, or `None` when the
    /// token is not a border-style keyword.
    pub fn from_keyword(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "dashed" => Some(Self::Dashed),
            "dotted" => Some(Self::Dotted),
            "solid" | "double" | "groove" | "ridge" | "inset" | "outset" => Some(Self::Solid),
            "none" | "hidden" => Some(Self::None),
            _ => None,
        }
    }
}

/// Computed style for an element.
#[derive(Debug, Clone, Default)]
pub struct ComputedStyle {
    // Box model
    pub display: Display,
    pub position: Position,
    pub float: Float,
    pub clear: Clear,
    pub width: Length,
    pub height: Length,
    pub min_width: Length,
    pub min_height: Length,
    pub max_width: Length,
    pub max_height: Length,
    pub aspect_ratio: Option<f32>, // width / height ratio

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
    pub border_top_style: BorderStyle,
    pub border_right_style: BorderStyle,
    pub border_bottom_style: BorderStyle,
    pub border_left_style: BorderStyle,

    // Border radius (for rounded corners)
    pub border_top_left_radius: Length,
    pub border_top_right_radius: Length,
    pub border_bottom_right_radius: Length,
    pub border_bottom_left_radius: Length,

    // Colors
    pub color: Color,
    pub background_color: Color,
    /// Background layers (painted bottom-to-top, index 0 is bottom).
    /// For backwards compatibility, also check background_gradient.
    pub background_layers: Vec<BackgroundLayer>,
    /// Legacy single gradient field - prefer using background_layers.
    /// This is kept for backwards compatibility during migration.
    pub background_gradient: Option<Gradient>,

    // Typography - Basic
    pub font_size: Length,
    pub font_weight: FontWeight,
    pub font_style: FontStyle,
    pub font_family: String,
    pub line_height: LineHeight,
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
    pub overflow_wrap: OverflowWrap,
    pub line_break: LineBreak,
    pub vertical_align: VerticalAlign,
    pub writing_mode: WritingMode,
    pub direction: Direction,

    // Positioning offsets
    pub top: Option<Length>,
    pub right: Option<Length>,
    pub bottom: Option<Length>,
    pub left: Option<Length>,
    pub z_index: i32,

    // Transforms
    pub transform: TransformList,
    pub transform_origin: TransformOrigin,

    // Transitions (parsed but not executed during parity capture)
    pub transition_property: String,
    pub transition_duration: f32, // seconds
    pub transition_timing_function: TimingFunction,
    pub transition_delay: f32, // seconds

    // Animations (parsed but not executed during parity capture)
    pub animation_name: String,
    pub animation_duration: f32, // seconds
    pub animation_timing_function: TimingFunction,
    pub animation_delay: f32, // seconds
    pub animation_iteration_count: AnimationIterationCount,
    pub animation_direction: AnimationDirection,
    pub animation_fill_mode: AnimationFillMode,
    pub animation_play_state: AnimationPlayState,

    // Box sizing
    pub box_sizing: BoxSizing,

    // Visual
    pub opacity: f32,
    pub visibility: Visibility,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    /// css-overflow-3 §5.1; only meaningful when the overflow above clips.
    pub text_overflow: TextOverflow,

    // Box shadows (multiple shadows supported)
    pub box_shadows: Vec<BoxShadow>,

    // Backdrop filter (blur, grayscale, etc.)
    pub backdrop_filter: BackdropFilter,

    // Image/replaced element
    pub image_url: Option<String>,
    pub object_fit: String, // "fill", "contain", "cover", "none", "scale-down"
    pub object_position: (f32, f32),

    // Flexbox Container
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub justify_content: JustifyContent,
    pub align_items: AlignItems,
    pub align_content: AlignContent,
    pub row_gap: Length,
    pub column_gap: Length,

    // Multi-column (css-multicol-1)
    /// `column-count`; `None` is `auto` — not a multi-column container.
    pub column_count: Option<u32>,

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

    // Pseudo-element content
    /// The `content` property for ::before/::after pseudo-elements.
    /// None means no content (element not rendered).
    /// Some("") means empty content (element rendered but empty).
    /// Some("text") means text content.
    pub content: Option<String>,

    // Background clip for gradient text
    pub background_clip: BackgroundClip,
    pub webkit_text_fill_color: Option<Color>,
}

impl ComputedStyle {
    /// Create default style.
    pub fn new() -> Self {
        Self {
            font_size: Length::Px(16.0),
            line_height: LineHeight::Normal,
            opacity: 1.0,
            color: Color::BLACK,
            background_color: Color::TRANSPARENT,
            font_family: "sans-serif".to_string(),
            text_decoration_line: TextDecorationLine::NONE,
            text_decoration_color: None,
            text_decoration_thickness: Length::Auto,
            // Flexbox item defaults
            flex_shrink: 1.0, // Default is 1, not 0
            // Width/height defaults to auto (fill available space)
            width: Length::Auto,
            height: Length::Auto,
            // CSS 2.1 / Flexbox §4.5: the INITIAL value of min-width and
            // min-height is `auto`, not zero. The distinction is invisible in
            // most contexts — Length::Auto and Length::Zero both resolve to
            // 0.0 in to_px_with_viewport — but it is load-bearing for flex
            // items, where `auto` means "floor at the content-based minimum"
            // and an explicitly authored `0` means "you may shrink me to
            // nothing". Defaulting to Zero made those two indistinguishable,
            // so every flex item was shrinkable to zero and text got squeezed
            // below the width it actually paints at.
            min_width: Length::Auto,
            min_height: Length::Auto,
            max_width: Length::Auto, // No max constraint
            max_height: Length::Auto,
            // Image/replaced element defaults
            image_url: None,
            // CSS Images 3 §5.5: the initial value of object-fit is FILL.
            // We defaulted to `contain`, which letterboxes every image that
            // does not set the property — i.e. almost all of them — and is
            // why sized images rendered smaller than their box with visible
            // gaps (Wikipedia globe, live session 2026-08-07).
            object_fit: "fill".to_string(),
            object_position: (0.5, 0.5), // center center
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
            overflow_wrap: parent.overflow_wrap,
            line_break: parent.line_break,
            direction: parent.direction,
            writing_mode: parent.writing_mode,
            visibility: parent.visibility,

            // Text decoration is NOT inherited (each element sets its own)
            text_decoration_line: TextDecorationLine::NONE,
            text_decoration_color: None,
            text_decoration_style: TextDecorationStyle::Solid,
            text_decoration_thickness: Length::Auto,

            // Non-inherited get defaults
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
    /// Media query lists of the enclosing `@media` blocks, outermost first;
    /// the rule applies only where all of them match (`Rule::applies_at`).
    pub media: Vec<String>,
}

impl Rule {
    /// Whether the rule's `@media` conditions hold for a viewport of
    /// `width` x `height` CSS px.
    pub fn applies_at(&self, width: f32, height: f32) -> bool {
        self.media
            .iter()
            .all(|m| media::media_query_list_matches(m, width, height))
    }
}

/// A complete stylesheet.
#[derive(Debug, Default, Clone)]
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
                media: r.media,
                selector: match encode_selector_escapes(&r.selector) {
                    std::borrow::Cow::Borrowed(_) => r.selector,
                    std::borrow::Cow::Owned(encoded) => encoded,
                },
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

pub mod font_face;
pub use font_face::{parse_font_face, FontDisplayValue, FontFaceRule};

pub mod media;
pub use media::media_query_list_matches;

pub mod selector_escape;
pub use selector_escape::{css_ident, encode_selector_escapes};

/// Parse a color value.
pub fn parse_color(value: &str) -> Option<Color> {
    let value = value.trim();

    // Named colors (CSS Color Level 4)
    match value.to_lowercase().as_str() {
        "transparent" => return Some(Color::TRANSPARENT),
        "black" => return Some(Color::BLACK),
        "white" => return Some(Color::WHITE),
        "red" => return Some(Color::from_rgb(255, 0, 0)),
        "green" => return Some(Color::from_rgb(0, 128, 0)),
        "blue" => return Some(Color::from_rgb(0, 0, 255)),
        "yellow" => return Some(Color::from_rgb(255, 255, 0)),
        "gray" | "grey" => return Some(Color::from_rgb(128, 128, 128)),
        // Extended named colors
        "coral" => return Some(Color::from_rgb(255, 127, 80)),
        "orange" => return Some(Color::from_rgb(255, 165, 0)),
        "pink" => return Some(Color::from_rgb(255, 192, 203)),
        "purple" => return Some(Color::from_rgb(128, 0, 128)),
        "cyan" => return Some(Color::from_rgb(0, 255, 255)),
        "magenta" | "fuchsia" => return Some(Color::from_rgb(255, 0, 255)),
        "lime" => return Some(Color::from_rgb(0, 255, 0)),
        "navy" => return Some(Color::from_rgb(0, 0, 128)),
        "teal" => return Some(Color::from_rgb(0, 128, 128)),
        "olive" => return Some(Color::from_rgb(128, 128, 0)),
        "maroon" => return Some(Color::from_rgb(128, 0, 0)),
        "aqua" => return Some(Color::from_rgb(0, 255, 255)),
        "silver" => return Some(Color::from_rgb(192, 192, 192)),
        "lightgray" | "lightgrey" => return Some(Color::from_rgb(211, 211, 211)),
        "darkgray" | "darkgrey" => return Some(Color::from_rgb(169, 169, 169)),
        "dimgray" | "dimgrey" => return Some(Color::from_rgb(105, 105, 105)),
        "lightblue" => return Some(Color::from_rgb(173, 216, 230)),
        "lightgreen" => return Some(Color::from_rgb(144, 238, 144)),
        "lightyellow" => return Some(Color::from_rgb(255, 255, 224)),
        "lightpink" => return Some(Color::from_rgb(255, 182, 193)),
        "lightcoral" => return Some(Color::from_rgb(240, 128, 128)),
        "darkblue" => return Some(Color::from_rgb(0, 0, 139)),
        "darkgreen" => return Some(Color::from_rgb(0, 100, 0)),
        "darkred" => return Some(Color::from_rgb(139, 0, 0)),
        "gold" => return Some(Color::from_rgb(255, 215, 0)),
        "brown" => return Some(Color::from_rgb(165, 42, 42)),
        "beige" => return Some(Color::from_rgb(245, 245, 220)),
        "ivory" => return Some(Color::from_rgb(255, 255, 240)),
        "wheat" => return Some(Color::from_rgb(245, 222, 179)),
        "tan" => return Some(Color::from_rgb(210, 180, 140)),
        "khaki" => return Some(Color::from_rgb(240, 230, 140)),
        "salmon" => return Some(Color::from_rgb(250, 128, 114)),
        "tomato" => return Some(Color::from_rgb(255, 99, 71)),
        "crimson" => return Some(Color::from_rgb(220, 20, 60)),
        "indianred" => return Some(Color::from_rgb(205, 92, 92)),
        "firebrick" => return Some(Color::from_rgb(178, 34, 34)),
        "orangered" => return Some(Color::from_rgb(255, 69, 0)),
        "chocolate" => return Some(Color::from_rgb(210, 105, 30)),
        "sienna" => return Some(Color::from_rgb(160, 82, 45)),
        "peru" => return Some(Color::from_rgb(205, 133, 63)),
        "sandybrown" => return Some(Color::from_rgb(244, 164, 96)),
        "goldenrod" => return Some(Color::from_rgb(218, 165, 32)),
        "darkgoldenrod" => return Some(Color::from_rgb(184, 134, 11)),
        "lemonchiffon" => return Some(Color::from_rgb(255, 250, 205)),
        "palegoldenrod" => return Some(Color::from_rgb(238, 232, 170)),
        "greenyellow" => return Some(Color::from_rgb(173, 255, 47)),
        "chartreuse" => return Some(Color::from_rgb(127, 255, 0)),
        "lawngreen" => return Some(Color::from_rgb(124, 252, 0)),
        "springgreen" => return Some(Color::from_rgb(0, 255, 127)),
        "mediumspringgreen" => return Some(Color::from_rgb(0, 250, 154)),
        "seagreen" => return Some(Color::from_rgb(46, 139, 87)),
        "forestgreen" => return Some(Color::from_rgb(34, 139, 34)),
        "limegreen" => return Some(Color::from_rgb(50, 205, 50)),
        "palegreen" => return Some(Color::from_rgb(152, 251, 152)),
        "mediumseagreen" => return Some(Color::from_rgb(60, 179, 113)),
        "aquamarine" => return Some(Color::from_rgb(127, 255, 212)),
        "turquoise" => return Some(Color::from_rgb(64, 224, 208)),
        "mediumturquoise" => return Some(Color::from_rgb(72, 209, 204)),
        "darkturquoise" => return Some(Color::from_rgb(0, 206, 209)),
        "cadetblue" => return Some(Color::from_rgb(95, 158, 160)),
        "steelblue" => return Some(Color::from_rgb(70, 130, 180)),
        "lightsteelblue" => return Some(Color::from_rgb(176, 196, 222)),
        "powderblue" => return Some(Color::from_rgb(176, 224, 230)),
        "skyblue" => return Some(Color::from_rgb(135, 206, 235)),
        "lightskyblue" => return Some(Color::from_rgb(135, 206, 250)),
        "deepskyblue" => return Some(Color::from_rgb(0, 191, 255)),
        "dodgerblue" => return Some(Color::from_rgb(30, 144, 255)),
        "cornflowerblue" => return Some(Color::from_rgb(100, 149, 237)),
        "royalblue" => return Some(Color::from_rgb(65, 105, 225)),
        "mediumblue" => return Some(Color::from_rgb(0, 0, 205)),
        "midnightblue" => return Some(Color::from_rgb(25, 25, 112)),
        "slateblue" => return Some(Color::from_rgb(106, 90, 205)),
        "darkslateblue" => return Some(Color::from_rgb(72, 61, 139)),
        "mediumslateblue" => return Some(Color::from_rgb(123, 104, 238)),
        "mediumpurple" => return Some(Color::from_rgb(147, 112, 219)),
        "blueviolet" => return Some(Color::from_rgb(138, 43, 226)),
        "darkorchid" => return Some(Color::from_rgb(153, 50, 204)),
        "darkviolet" => return Some(Color::from_rgb(148, 0, 211)),
        "mediumorchid" => return Some(Color::from_rgb(186, 85, 211)),
        "orchid" => return Some(Color::from_rgb(218, 112, 214)),
        "plum" => return Some(Color::from_rgb(221, 160, 221)),
        "violet" => return Some(Color::from_rgb(238, 130, 238)),
        "thistle" => return Some(Color::from_rgb(216, 191, 216)),
        "lavender" => return Some(Color::from_rgb(230, 230, 250)),
        "mistyrose" => return Some(Color::from_rgb(255, 228, 225)),
        "antiquewhite" => return Some(Color::from_rgb(250, 235, 215)),
        "linen" => return Some(Color::from_rgb(250, 240, 230)),
        "oldlace" => return Some(Color::from_rgb(253, 245, 230)),
        "papayawhip" => return Some(Color::from_rgb(255, 239, 213)),
        "seashell" => return Some(Color::from_rgb(255, 245, 238)),
        "mintcream" => return Some(Color::from_rgb(245, 255, 250)),
        "slategray" | "slategrey" => return Some(Color::from_rgb(112, 128, 144)),
        "lightslategray" | "lightslategrey" => return Some(Color::from_rgb(119, 136, 153)),
        "gainsboro" => return Some(Color::from_rgb(220, 220, 220)),
        "whitesmoke" => return Some(Color::from_rgb(245, 245, 245)),
        "floralwhite" => return Some(Color::from_rgb(255, 250, 240)),
        "ghostwhite" => return Some(Color::from_rgb(248, 248, 255)),
        "honeydew" => return Some(Color::from_rgb(240, 255, 240)),
        "azure" => return Some(Color::from_rgb(240, 255, 255)),
        "aliceblue" => return Some(Color::from_rgb(240, 248, 255)),
        "snow" => return Some(Color::from_rgb(255, 250, 250)),
        "darkcyan" => return Some(Color::from_rgb(0, 139, 139)),
        "darkmagenta" => return Some(Color::from_rgb(139, 0, 139)),
        "darkorange" => return Some(Color::from_rgb(255, 140, 0)),
        "darksalmon" => return Some(Color::from_rgb(233, 150, 122)),
        "darkseagreen" => return Some(Color::from_rgb(143, 188, 143)),
        "darkslategray" | "darkslategrey" => return Some(Color::from_rgb(47, 79, 79)),
        "deeppink" => return Some(Color::from_rgb(255, 20, 147)),
        "hotpink" => return Some(Color::from_rgb(255, 105, 180)),
        "mediumvioletred" => return Some(Color::from_rgb(199, 21, 133)),
        "palevioletred" => return Some(Color::from_rgb(219, 112, 147)),
        "rosybrown" => return Some(Color::from_rgb(188, 143, 143)),
        "saddlebrown" => return Some(Color::from_rgb(139, 69, 19)),
        "yellowgreen" => return Some(Color::from_rgb(154, 205, 50)),
        "olivedrab" => return Some(Color::from_rgb(107, 142, 35)),
        "darkolivegreen" => return Some(Color::from_rgb(85, 107, 47)),
        "mediumaquamarine" => return Some(Color::from_rgb(102, 205, 170)),
        "lightcyan" => return Some(Color::from_rgb(224, 255, 255)),
        "paleturquoise" => return Some(Color::from_rgb(175, 238, 238)),
        "lightseagreen" => return Some(Color::from_rgb(32, 178, 170)),
        "cornsilk" => return Some(Color::from_rgb(255, 248, 220)),
        "blanchedalmond" => return Some(Color::from_rgb(255, 235, 205)),
        "bisque" => return Some(Color::from_rgb(255, 228, 196)),
        "navajowhite" => return Some(Color::from_rgb(255, 222, 173)),
        "moccasin" => return Some(Color::from_rgb(255, 228, 181)),
        "peachpuff" => return Some(Color::from_rgb(255, 218, 185)),
        "burlywood" => return Some(Color::from_rgb(222, 184, 135)),
        "lavenderblush" => return Some(Color::from_rgb(255, 240, 245)),
        "currentcolor" => return None, // Special case - needs context
        "inherit" => return None,      // Special case - needs context
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

    // hsl() / hsla()
    if value.starts_with("hsl") {
        let inner = value
            .trim_start_matches("hsla(")
            .trim_start_matches("hsl(")
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() >= 3 {
            let h = parts[0]
                .trim()
                .trim_end_matches("deg")
                .parse::<f32>()
                .ok()?;
            let s = parts[1].trim().trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let l = parts[2].trim().trim_end_matches('%').parse::<f32>().ok()? / 100.0;
            let a = if parts.len() >= 4 {
                parts[3].trim().parse::<f32>().ok()?
            } else {
                1.0
            };

            // HSL to RGB conversion
            let (r, g, b) = hsl_to_rgb(h, s, l);
            return Some(Color::new(r, g, b, a));
        }
    }

    None
}

/// Convert HSL to RGB
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (u8, u8, u8) {
    let s = s.clamp(0.0, 1.0);
    let l = l.clamp(0.0, 1.0);

    if s < 0.0001 {
        // Achromatic (gray)
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }

    // Wrap hue into [0, 360) — hsl(-120, …) and hsl(480, …) are valid CSS.
    let h = ((h % 360.0) + 360.0) % 360.0 / 360.0;
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    let r = hue_to_rgb(p, q, h + 1.0 / 3.0);
    let g = hue_to_rgb(p, q, h);
    let b = hue_to_rgb(p, q, h - 1.0 / 3.0);

    (
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }

    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

/// Parse a length value.
pub fn parse_length(value: &str) -> Option<Length> {
    let value = value.trim();

    if value == "auto" {
        return Some(Length::Auto);
    }
    if value == "fit-content" {
        return Some(Length::FitContent);
    }
    if value == "0" {
        return Some(Length::Zero);
    }

    // Handle min(), max(), clamp() CSS math functions
    if value.starts_with("min(") && value.ends_with(')') {
        let inner = &value[4..value.len() - 1];
        let args = split_css_function_args(inner);
        if args.len() >= 2 {
            let a = parse_length(args[0])?;
            let b = parse_length(args[1])?;
            return Some(Length::Min(Box::new((a, b))));
        }
        return None;
    }

    if value.starts_with("max(") && value.ends_with(')') {
        let inner = &value[4..value.len() - 1];
        let args = split_css_function_args(inner);
        if args.len() >= 2 {
            let a = parse_length(args[0])?;
            let b = parse_length(args[1])?;
            return Some(Length::Max(Box::new((a, b))));
        }
        return None;
    }

    if value.starts_with("clamp(") && value.ends_with(')') {
        let inner = &value[6..value.len() - 1];
        let args = split_css_function_args(inner);
        if args.len() >= 3 {
            let min = parse_length(args[0])?;
            let preferred = parse_length(args[1])?;
            let max = parse_length(args[2])?;
            return Some(Length::Clamp(Box::new((min, preferred, max))));
        }
        return None;
    }

    if value.starts_with("calc(") && value.ends_with(')') {
        let inner = value[5..value.len() - 1].trim();
        // A single value wrapped in `calc()` is still that value, and stayed
        // on its own `Length` variant before this parser existed. Kept first
        // so the collapse in `CalcSum::into_length` is never the only thing
        // holding that up.
        if let Some(len) = parse_length(inner) {
            return Some(len);
        }
        return parse_calc_sum(inner).map(CalcSum::into_length);
    }

    if value.ends_with("px") {
        let num = value.trim_end_matches("px").parse::<f32>().ok()?;
        return Some(Length::Px(num));
    }
    // rem MUST be checked before em: "2rem".ends_with("em") is true, and
    // the em arm's "2r".parse() then fails -> None. The engine's deleted
    // duplicate carried this exact warning; the canonical copy had the rem
    // arm dead below the em arm. (Duplication audit P0, proven by test.)
    if value.ends_with("rem") {
        let num = value.trim_end_matches("rem").parse::<f32>().ok()?;
        return Some(Length::Rem(num));
    }
    if value.ends_with("em") {
        let num = value.trim_end_matches("em").parse::<f32>().ok()?;
        return Some(Length::Em(num));
    }
    if value.ends_with("vh") {
        let num = value.trim_end_matches("vh").parse::<f32>().ok()?;
        return Some(Length::Vh(num));
    }
    if value.ends_with("vw") {
        let num = value.trim_end_matches("vw").parse::<f32>().ok()?;
        return Some(Length::Vw(num));
    }
    if value.ends_with("vmin") {
        let num = value.trim_end_matches("vmin").parse::<f32>().ok()?;
        return Some(Length::Vmin(num));
    }
    if value.ends_with("vmax") {
        let num = value.trim_end_matches("vmax").parse::<f32>().ok()?;
        return Some(Length::Vmax(num));
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

/// Parse the inside of a `calc()` into css-values-3 §8.1 normal form.
///
/// Grammar, exactly the spec's:
/// ```text
///   sum     := product ( S ('+' | '-') S product )*
///   product := unit ( ('*' number) | ('/' number) )*   |   number '*' unit
///   unit    := <length> | <percentage> | <number> | '(' sum ')'
/// ```
/// `+` and `-` REQUIRE surrounding whitespace (css-values-3 §8.1: without it
/// `10px -5px` is one token, a signed length, not a subtraction) — this is why
/// the sum splitter looks at the neighbouring characters rather than at `-`
/// alone. `*` and `/` do not.
///
/// Returns `None` for anything outside that grammar, including a `*` or `/`
/// whose operand is not a plain number, which the spec makes invalid rather
/// than approximate.
fn parse_calc_sum(input: &str) -> Option<CalcSum> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();

    // Split on top-level ' + ' / ' - ', right to left, so the left operand
    // keeps its own additions and the sign applies to one product.
    let mut depth = 0i32;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => depth -= 1,
            b'+' | b'-' if depth == 0 => {
                let prev = bytes.get(i.wrapping_sub(1)).copied();
                let next = bytes.get(i + 1).copied();
                let spaced = matches!(prev, Some(b' ') | Some(b'\t'))
                    && matches!(next, Some(b' ') | Some(b'\t'));
                if !spaced || i == 0 {
                    continue;
                }
                let lhs = parse_calc_sum(&s[..i])?;
                let rhs = parse_calc_product(&s[i + 1..])?;
                let sign = if bytes[i] == b'+' { 1.0 } else { -1.0 };
                return Some(lhs.add(rhs, sign));
            }
            _ => {}
        }
    }
    parse_calc_product(s)
}

/// One `product` of the calc grammar: a unit scaled by plain numbers.
fn parse_calc_product(input: &str) -> Option<CalcSum> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    let bytes = s.as_bytes();

    let mut depth = 0i32;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b')' => depth += 1,
            b'(' => depth -= 1,
            b'*' | b'/' if depth == 0 => {
                let lhs = &s[..i];
                let rhs = &s[i + 1..];
                if bytes[i] == b'/' {
                    // css-values-3 §8.1: the right side of `/` must be a number.
                    let divisor = parse_plain_number(rhs)?;
                    if divisor == 0.0 {
                        return None;
                    }
                    return Some(parse_calc_product(lhs)?.scaled(1.0 / divisor));
                }
                // `*` takes a number on exactly one side.
                if let Some(k) = parse_plain_number(rhs) {
                    return Some(parse_calc_product(lhs)?.scaled(k));
                }
                if let Some(k) = parse_plain_number(lhs) {
                    return Some(parse_calc_product(rhs)?.scaled(k));
                }
                return None;
            }
            _ => {}
        }
    }
    parse_calc_unit(s)
}

/// One `unit`: a parenthesised sum, or a single length/percentage/number.
fn parse_calc_unit(input: &str) -> Option<CalcSum> {
    let s = input.trim();
    if let Some(stripped) = s.strip_prefix('(') {
        let inner = stripped.strip_suffix(')')?;
        return parse_calc_sum(inner);
    }
    if s.starts_with("calc(") && s.ends_with(')') {
        return parse_calc_sum(&s[5..s.len() - 1]);
    }
    let mut sum = CalcSum::default();
    let (num, unit) = split_number_and_unit(s)?;
    match unit {
        "px" | "" => sum.px = num,
        "%" => sum.percent = num,
        "em" => sum.em = num,
        "rem" => sum.rem = num,
        "vw" => sum.vw = num,
        "vh" => sum.vh = num,
        "vmin" => sum.vmin = num,
        "vmax" => sum.vmax = num,
        _ => return None,
    }
    Some(sum)
}

/// A bare number, with no unit. `None` for anything else — including a length,
/// which is what makes `100px * 2px` invalid rather than silently 200px.
fn parse_plain_number(input: &str) -> Option<f32> {
    let s = input.trim();
    match split_number_and_unit(s) {
        Some((num, "")) => Some(num),
        _ => None,
    }
}

/// Split `"-84px"` into `(-84.0, "px")`. The unit is lower-cased by the
/// caller's input already being lower-cased in `parse_length`; `%` is a unit
/// here, not punctuation.
fn split_number_and_unit(s: &str) -> Option<(f32, &str)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let split = s
        .char_indices()
        .position(|(_, c)| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e'))
        .unwrap_or(s.len());
    // `e` is only exponent notation when it sits between digits; a bare `em`
    // must not eat its own `e`.
    let split = if split > 0 && s.as_bytes()[split - 1] == b'e' {
        split - 1
    } else {
        split
    };
    let (num, unit) = s.split_at(split);
    let value = num.parse::<f32>().ok()?;
    if !value.is_finite() {
        return None;
    }
    Some((value, unit.trim()))
}

/// Split CSS function arguments, handling nested parentheses.
fn split_css_function_args(args: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;

    for (i, c) in args.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                result.push(args[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }

    // Add the last argument
    let last = args[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }

    result
}

/// Parse display value.
///
/// Besides the legacy single keywords, this takes css-display-3's
/// `<display-outside> || <display-inside>` plus `list-item` (`flow-root`,
/// `inline flex`, `block flow list-item`, ...), which were dropped before, so
/// the element kept its previous display. `flow-root` lays out as a block
/// (its new formatting context only matters for floats and margin collapse);
/// `list-item` lays out as its outer display, with no marker. `contents`,
/// `table*` and `ruby` stay unsupported (`None`).
pub fn parse_display(value: &str) -> Option<Display> {
    let value = value.trim().to_lowercase();
    match value.as_str() {
        "block" => return Some(Display::Block),
        "inline" => return Some(Display::Inline),
        "inline-block" => return Some(Display::InlineBlock),
        "flex" => return Some(Display::Flex),
        "inline-flex" => return Some(Display::InlineFlex),
        "grid" => return Some(Display::Grid),
        "inline-grid" => return Some(Display::InlineGrid),
        "none" => return Some(Display::None),
        _ => {}
    }
    let (mut outer, mut inner, mut list_item) = (None, None, false);
    for token in value.split_whitespace() {
        match token {
            "block" | "inline" if outer.is_none() => outer = Some(token),
            "flow" | "flow-root" | "flex" | "grid" if inner.is_none() => inner = Some(token),
            "list-item" if !list_item => list_item = true,
            _ => return None,
        }
    }
    if outer.is_none() && inner.is_none() && !list_item {
        return None;
    }
    // `list-item` only combines with a flow inner display.
    if list_item && !matches!(inner, None | Some("flow") | Some("flow-root")) {
        return None;
    }
    let inline = outer == Some("inline");
    Some(match (inline, inner.unwrap_or("flow")) {
        (false, "flex") => Display::Flex,
        (true, "flex") => Display::InlineFlex,
        (false, "grid") => Display::Grid,
        (true, "grid") => Display::InlineGrid,
        (true, "flow-root") => Display::InlineBlock,
        (true, _) => Display::Inline,
        (false, _) => Display::Block,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_takes_flow_root_list_item_and_two_value_syntax() {
        let cases: &[(&str, Option<Display>)] = &[
            ("flow-root", Some(Display::Block)),
            ("list-item", Some(Display::Block)),
            ("flow", Some(Display::Block)),
            ("block flow-root", Some(Display::Block)),
            ("inline flow-root", Some(Display::InlineBlock)),
            ("inline flow", Some(Display::Inline)),
            ("block flex", Some(Display::Flex)),
            ("inline flex", Some(Display::InlineFlex)),
            ("grid inline", Some(Display::InlineGrid)),
            ("block flow list-item", Some(Display::Block)),
            ("inline list-item", Some(Display::Inline)),
            ("Flow-Root", Some(Display::Block)),
            // Legacy keywords are unchanged.
            ("inline-block", Some(Display::InlineBlock)),
            ("none", Some(Display::None)),
            // Unsupported or invalid: still ignored.
            ("contents", None),
            ("table", None),
            ("block block", None),
            ("flex list-item", None),
            ("block wobble", None),
            ("", None),
        ];
        for (value, want) in cases {
            assert_eq!(parse_display(value), *want, "display: {value:?}");
        }
    }

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
        // Extended names must resolve — rustkit-engine delegates here now
        assert_eq!(parse_color("coral"), Some(Color::from_rgb(255, 127, 80)));
        assert_eq!(parse_color("tomato"), Some(Color::from_rgb(255, 99, 71)));
    }

    #[test]
    fn test_parse_color_hsl_hue_wraps() {
        // hsl(-120) ≡ hsl(240), hsl(480) ≡ hsl(120) — hue is a circle
        assert_eq!(
            parse_color("hsl(-120, 50%, 50%)"),
            parse_color("hsl(240, 50%, 50%)")
        );
        assert_eq!(
            parse_color("hsl(480, 100%, 50%)"),
            parse_color("hsl(120, 100%, 50%)")
        );
    }

    #[test]
    fn test_parse_color_hsl() {
        // Pure red: hsl(0, 100%, 50%)
        let red = parse_color("hsl(0, 100%, 50%)");
        assert!(red.is_some(), "HSL red should parse");
        let red = red.unwrap();
        assert_eq!(red.r, 255, "HSL red R component");
        assert_eq!(red.g, 0, "HSL red G component");
        assert_eq!(red.b, 0, "HSL red B component");

        // Pure green: hsl(120, 100%, 50%)
        let green = parse_color("hsl(120, 100%, 50%)");
        assert!(green.is_some(), "HSL green should parse");
        let green = green.unwrap();
        assert_eq!(green.r, 0, "HSL green R component");
        assert_eq!(green.g, 255, "HSL green G component");
        assert_eq!(green.b, 0, "HSL green B component");

        // Pure blue: hsl(240, 100%, 50%)
        let blue = parse_color("hsl(240, 100%, 50%)");
        assert!(blue.is_some(), "HSL blue should parse");
        let blue = blue.unwrap();
        assert_eq!(blue.r, 0, "HSL blue R component");
        assert_eq!(blue.g, 0, "HSL blue G component");
        assert_eq!(blue.b, 255, "HSL blue B component");
    }

    #[test]
    fn test_parse_length() {
        assert_eq!(parse_length("10px"), Some(Length::Px(10.0)));
        assert_eq!(parse_length("1.5em"), Some(Length::Em(1.5)));
        assert_eq!(parse_length("50%"), Some(Length::Percent(50.0)));
        assert_eq!(parse_length("auto"), Some(Length::Auto));
    }

    /// `fit-content` must parse, and it must NOT parse as `auto`.
    ///
    /// Returning `None` here is what shipped: the declaration was dropped and
    /// `height` kept its `auto` initial value, so `height: fit-content` made a
    /// grid item stretch — the one thing the keyword is written to prevent.
    /// Mapping it to `Length::Auto` would be the same defect with a parse
    /// result attached, which is why this asserts the variant and not just
    /// `is_some()`.
    #[test]
    fn fit_content_parses_and_is_not_auto() {
        assert_eq!(parse_length("fit-content"), Some(Length::FitContent));
        assert_eq!(parse_length("  fit-content  "), Some(Length::FitContent));
        assert_ne!(parse_length("fit-content"), Some(Length::Auto));
        // Content-sized like auto everywhere that resolves a used value.
        assert_eq!(Length::FitContent.to_px(16.0, 16.0, 500.0), 0.0);
    }

    #[test]
    fn test_parse_length_math_functions() {
        // Test min()
        let min_result = parse_length("min(700px, 100%)");
        assert!(min_result.is_some());
        if let Some(Length::Min(pair)) = min_result {
            assert_eq!(pair.0, Length::Px(700.0));
            assert_eq!(pair.1, Length::Percent(100.0));
        } else {
            panic!("Expected Length::Min");
        }

        // Test max()
        let max_result = parse_length("max(50%, 300px)");
        assert!(max_result.is_some());
        if let Some(Length::Max(pair)) = max_result {
            assert_eq!(pair.0, Length::Percent(50.0));
            assert_eq!(pair.1, Length::Px(300.0));
        } else {
            panic!("Expected Length::Max");
        }

        // Test clamp()
        let clamp_result = parse_length("clamp(200px, 50%, 800px)");
        assert!(clamp_result.is_some());
        if let Some(Length::Clamp(triple)) = clamp_result {
            assert_eq!(triple.0, Length::Px(200.0));
            assert_eq!(triple.1, Length::Percent(50.0));
            assert_eq!(triple.2, Length::Px(800.0));
        } else {
            panic!("Expected Length::Clamp");
        }
    }

    fn calc_of(value: &str) -> CalcSum {
        match parse_length(value) {
            Some(Length::Calc(sum)) => *sum,
            other => panic!("expected Length::Calc for {value:?}, got {other:?}"),
        }
    }

    #[test]
    fn a_calc_mixing_a_percentage_and_a_length_keeps_both_terms() {
        // chrome_rustkit's `.sidebar`. Before `Length::Calc` existed this
        // parsed to `None`, the declaration was dropped, and the box fell back
        // to its content height — 203px against Chrome's 16.
        let sum = calc_of("calc(100% - 84px)");
        assert_eq!(sum.percent, 100.0);
        assert_eq!(sum.px, -84.0);
        assert_eq!(
            Length::Calc(Box::new(sum)).to_px_with_viewport(16.0, 16.0, 100.0, 1280.0, 100.0),
            16.0
        );
    }

    #[test]
    fn a_calc_that_uses_one_unit_stays_on_that_units_variant() {
        // The blast-radius guard: every site that matches `Length::Px` or
        // `Length::Percent` must keep seeing these. A `Calc` here would make
        // those sites fall through to their `_` arm, i.e. to `auto`.
        assert_eq!(parse_length("calc(100px)"), Some(Length::Px(100.0)));
        assert_eq!(parse_length("calc(2 * 50px)"), Some(Length::Px(100.0)));
        assert_eq!(parse_length("calc(100px / 4)"), Some(Length::Px(25.0)));
        assert_eq!(parse_length("calc(100px - 40px)"), Some(Length::Px(60.0)));
        match parse_length("calc(100% / 3)") {
            Some(Length::Percent(pct)) => assert!((pct - 100.0 / 3.0).abs() < 1e-4, "{pct}"),
            other => panic!("expected Length::Percent, got {other:?}"),
        }
        assert_eq!(parse_length("calc(50% + 50%)"), Some(Length::Percent(100.0)));
        // A calc that cancels to nothing is a definite ZERO length, not the
        // `Length::Zero` default: `Zero` is what an unset property holds.
        assert_eq!(parse_length("calc(10px - 10px)"), Some(Length::Px(0.0)));
    }

    #[test]
    fn calc_sums_every_unit_against_its_own_basis() {
        let sum = calc_of("calc(50% + 2em + 1rem + 10vw + 10vh - 5px)");
        assert_eq!(sum.percent, 50.0);
        assert_eq!(sum.em, 2.0);
        assert_eq!(sum.rem, 1.0);
        assert_eq!(sum.vw, 10.0);
        assert_eq!(sum.vh, 10.0);
        assert_eq!(sum.px, -5.0);
        // font 20, root 16, container 200, viewport 1000x500:
        // 100 + 40 + 16 + 100 + 50 - 5
        assert_eq!(
            Length::Calc(Box::new(sum)).to_px_with_viewport(20.0, 16.0, 200.0, 1000.0, 500.0),
            301.0
        );
    }

    #[test]
    fn calc_multiplication_and_division_scale_every_term() {
        let sum = calc_of("calc((100% - 20px) / 2)");
        assert_eq!(sum.percent, 50.0);
        assert_eq!(sum.px, -10.0);
        let sum = calc_of("calc(2 * (50% + 5px))");
        assert_eq!(sum.percent, 100.0);
        assert_eq!(sum.px, 10.0);
    }

    #[test]
    fn calc_subtraction_is_left_associative() {
        // Right-associative folding reads `100px - 30px - 20px` as
        // 100 - (30 - 20) = 90 instead of 50.
        assert_eq!(
            parse_length("calc(100px - 30px - 20px)"),
            Some(Length::Px(50.0))
        );
        let sum = calc_of("calc(100% - 30px - 20px)");
        assert_eq!(sum.px, -50.0);
    }

    #[test]
    fn calc_rejects_what_the_spec_rejects() {
        // `+` and `-` need whitespace on both sides (css-values-3 §8.1);
        // without it the token is a signed length and the sum is malformed.
        assert_eq!(parse_length("calc(100% -84px)"), None);
        // `*` and `/` take a plain NUMBER, never a second length.
        assert_eq!(parse_length("calc(100% * 2px)"), None);
        assert_eq!(parse_length("calc(100% / 2px)"), None);
        assert_eq!(parse_length("calc(100% / 0)"), None);
        assert_eq!(parse_length("calc(100% - )"), None);
        assert_eq!(parse_length("calc(100% - 10foo)"), None);
        assert_eq!(parse_length("calc()"), None);
    }

    #[test]
    fn test_parse_length_viewport_units() {
        assert_eq!(parse_length("100vh"), Some(Length::Vh(100.0)));
        assert_eq!(parse_length("50vw"), Some(Length::Vw(50.0)));
        assert_eq!(parse_length("10vmin"), Some(Length::Vmin(10.0)));
        assert_eq!(parse_length("20vmax"), Some(Length::Vmax(20.0)));
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

    // Grid template expansion tests
    #[test]
    fn test_expand_tracks_no_repeat() {
        // Template without any repeats should return tracks unchanged
        let template = GridTemplate {
            tracks: vec![
                TrackDefinition::simple(TrackSize::Fr(1.0)),
                TrackDefinition::simple(TrackSize::Fr(1.0)),
            ],
            repeats: vec![],
            final_line_names: vec![],
        };

        let (expanded, auto_repeat) = template.expand_tracks();
        assert_eq!(expanded.len(), 2);
        assert!(auto_repeat.is_none());
    }

    #[test]
    fn test_expand_tracks_repeat_count() {
        // repeat(3, 1fr) should expand to 3 tracks
        let template = GridTemplate {
            tracks: vec![],
            repeats: vec![(
                0,
                TrackRepeat::Count(3, vec![TrackDefinition::simple(TrackSize::Fr(1.0))]),
            )],
            final_line_names: vec![],
        };

        let (expanded, auto_repeat) = template.expand_tracks();
        assert_eq!(expanded.len(), 3);
        assert!(auto_repeat.is_none());
        for track in &expanded {
            assert_eq!(track.size, TrackSize::Fr(1.0));
        }
    }

    #[test]
    fn test_expand_tracks_repeat_multiple_tracks() {
        // repeat(2, 100px 1fr) should expand to 4 tracks: 100px, 1fr, 100px, 1fr
        let template = GridTemplate {
            tracks: vec![],
            repeats: vec![(
                0,
                TrackRepeat::Count(
                    2,
                    vec![
                        TrackDefinition::simple(TrackSize::Px(100.0)),
                        TrackDefinition::simple(TrackSize::Fr(1.0)),
                    ],
                ),
            )],
            final_line_names: vec![],
        };

        let (expanded, auto_repeat) = template.expand_tracks();
        assert_eq!(expanded.len(), 4);
        assert_eq!(expanded[0].size, TrackSize::Px(100.0));
        assert_eq!(expanded[1].size, TrackSize::Fr(1.0));
        assert_eq!(expanded[2].size, TrackSize::Px(100.0));
        assert_eq!(expanded[3].size, TrackSize::Fr(1.0));
    }

    #[test]
    fn test_expand_tracks_mixed() {
        // 100px repeat(2, 1fr) 200px -> 100px 1fr 1fr 200px
        let template = GridTemplate {
            tracks: vec![
                TrackDefinition::simple(TrackSize::Px(100.0)),
                TrackDefinition::simple(TrackSize::Px(200.0)),
            ],
            repeats: vec![(
                1, // Insert at position 1 (after first track)
                TrackRepeat::Count(2, vec![TrackDefinition::simple(TrackSize::Fr(1.0))]),
            )],
            final_line_names: vec![],
        };

        let (expanded, auto_repeat) = template.expand_tracks();
        assert_eq!(expanded.len(), 4);
        assert_eq!(expanded[0].size, TrackSize::Px(100.0));
        assert_eq!(expanded[1].size, TrackSize::Fr(1.0));
        assert_eq!(expanded[2].size, TrackSize::Fr(1.0));
        assert_eq!(expanded[3].size, TrackSize::Px(200.0));
    }

    #[test]
    fn test_expand_tracks_auto_fill_returns_unexpanded() {
        // auto-fill should be marked for layout-time expansion
        let template = GridTemplate {
            tracks: vec![],
            repeats: vec![(
                0,
                TrackRepeat::AutoFill(vec![TrackDefinition::simple(TrackSize::Px(200.0))]),
            )],
            final_line_names: vec![],
        };

        let (expanded, auto_repeat) = template.expand_tracks();
        assert_eq!(expanded.len(), 0); // No tracks expanded yet
        assert!(auto_repeat.is_some());
        match auto_repeat.unwrap() {
            TrackRepeat::AutoFill(_) => {}
            _ => panic!("Expected AutoFill"),
        }
    }

    #[test]
    fn test_expand_tracks_auto_fit_returns_unexpanded() {
        // auto-fit should be marked for layout-time expansion
        let template = GridTemplate {
            tracks: vec![],
            repeats: vec![(
                0,
                TrackRepeat::AutoFit(vec![TrackDefinition::simple(TrackSize::Px(200.0))]),
            )],
            final_line_names: vec![],
        };

        let (expanded, auto_repeat) = template.expand_tracks();
        assert_eq!(expanded.len(), 0);
        assert!(auto_repeat.is_some());
        match auto_repeat.unwrap() {
            TrackRepeat::AutoFit(_) => {}
            _ => panic!("Expected AutoFit"),
        }
    }

    #[test]
    fn test_expand_tracks_with_line_names() {
        // Named lines should be preserved during expansion
        let track_with_names = TrackDefinition {
            size: TrackSize::Fr(1.0),
            line_names: vec!["col-start".to_string()],
        };

        let template = GridTemplate {
            tracks: vec![],
            repeats: vec![(0, TrackRepeat::Count(2, vec![track_with_names]))],
            final_line_names: vec![],
        };

        let (expanded, _) = template.expand_tracks();
        assert_eq!(expanded.len(), 2);
        assert_eq!(expanded[0].line_names, vec!["col-start".to_string()]);
        assert_eq!(expanded[1].line_names, vec!["col-start".to_string()]);
    }
}


#[cfg(test)]
mod object_fit_initial_value_tests {
    use super::*;

    /// CSS Images 3 §5.5 pins the initial value of `object-fit` to `fill`.
    ///
    /// We shipped `contain`, which letterboxes every image that does not set
    /// the property — nearly all of them — so sized images rendered smaller
    /// than their box with gaps (live session 2026-08-07). A default that is
    /// "reasonable looking" but not the spec value is the shape that makes a
    /// whole class of pages subtly wrong while every test passes.
    #[test]
    fn object_fit_initial_value_is_fill() {
        assert_eq!(ComputedStyle::new().object_fit, "fill");
    }
}

// ── ported from hiwave-windows (#37, #49): a shadow with no visible colour or
//    no geometry is not visible, so paint never spends a command on it. ──
#[cfg(test)]
mod windows_shadow_pins {
    use super::*;


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
}
