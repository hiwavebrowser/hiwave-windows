//! Chrome/Skia-compatible ordered dithering for gradient rendering.
//!
//! This module implements the same ordered dithering patterns used by Chrome/Skia
//! when quantizing gradients to 8-bit channels. The dither matrices reduce banding
//! artifacts that occur when smooth gradients are displayed on 8-bit displays.
//!
//! # Algorithm
//!
//! The dithering process works by adding a small offset to color values before
//! truncating to 8-bit integers. The offset is determined by a Bayer matrix lookup
//! based on the pixel's (x, y) position:
//!
//! ```text
//! For 4x4 matrix:
//!   idx = (y & 3) * 4 + (x & 3)
//!   dither = (matrix[idx] + 0.5) / 16.0
//!
//! For 8x8 matrix:
//!   idx = (y & 7) * 8 + (x & 7)
//!   dither = (matrix[idx] + 0.5) / 64.0
//! ```
//!
//! The dither value is then added to each color channel (in 0-255 space) before
//! truncation to u8.
//!
//! # Matrix Selection
//!
//! Chrome/Skia selects between different dither matrices based on the gradient's
//! "slope" - how many device pixels correspond to one 8-bit color step:
//!
//! - **BAYER_4X4**: Small gradients (bucket <= 64)
//! - **BAYER_8X8_XY**: Very gentle ramps (>= 1024 pixels per step)
//! - **BAYER_8X8_MEDIUM_XY**: Medium ramps (>= 512 pixels per step)
//! - **BAYER_8X8_CLASSIC_XY**: Steep ramps (< 512 pixels per step)

/// 4x4 Bayer ordered dither matrix used by Chrome/Skia for small gradients.
///
/// Values are in the range 0-15. The floating dither offset is:
/// `dither = (matrix[idx] + 0.5) / 16.0`
///
/// Indexing: `idx = (y & 3) * 4 + (x & 3)`
pub const BAYER_4X4: [u8; 16] = [
    0, 12, 3, 15,
    8, 4, 11, 7,
    2, 14, 1, 13,
    10, 6, 9, 5,
];

/// 8x8 Bayer ordered dither matrix for very gentle gradient ramps.
///
/// Used when there are >= 1024 device pixels per 8-bit color step.
/// Values are in the range 0-63. The floating dither offset is:
/// `dither = (matrix[idx] + 0.5) / 64.0`
///
/// Indexing: `idx = (y & 7) * 8 + (x & 7)`
pub const BAYER_8X8_XY: [u8; 64] = [
    15, 48, 0, 60, 12, 51, 3, 63,
    44, 19, 35, 31, 47, 16, 32, 28,
    7, 56, 8, 52, 4, 59, 11, 55,
    36, 27, 43, 23, 39, 24, 40, 20,
    13, 50, 2, 62, 14, 49, 1, 61,
    46, 17, 33, 29, 45, 18, 34, 30,
    5, 58, 10, 54, 6, 57, 9, 53,
    38, 25, 41, 21, 37, 26, 42, 22,
];

/// 8x8 Bayer ordered dither matrix for medium gradient ramps.
///
/// Used when there are >= 512 but < 1024 device pixels per 8-bit color step.
/// Extracted from Chrome output for a 0→1 ramp over 512px.
pub const BAYER_8X8_MEDIUM_XY: [u8; 64] = [
    12, 48, 3, 60, 15, 51, 0, 63,
    32, 28, 44, 19, 35, 31, 47, 16,
    4, 56, 11, 52, 7, 59, 8, 55,
    40, 20, 36, 27, 43, 23, 39, 24,
    14, 50, 1, 62, 13, 49, 2, 61,
    34, 30, 46, 17, 33, 29, 45, 18,
    6, 58, 9, 54, 5, 57, 10, 53,
    42, 22, 38, 25, 41, 21, 37, 26,
];

/// Classic 8x8 Bayer matrix for steep gradient ramps.
///
/// Used when there are < 512 device pixels per 8-bit color step.
/// This is the standard Bayer 8x8 ordered dither pattern.
pub const BAYER_8X8_CLASSIC_XY: [u8; 64] = [
    0, 48, 12, 60, 3, 51, 15, 63,
    32, 16, 44, 28, 35, 19, 47, 31,
    8, 56, 4, 52, 11, 59, 7, 55,
    40, 24, 36, 20, 43, 27, 39, 23,
    2, 50, 14, 62, 1, 49, 13, 61,
    34, 18, 46, 30, 33, 17, 45, 29,
    10, 58, 6, 54, 9, 57, 5, 53,
    42, 26, 38, 22, 41, 25, 37, 21,
];

/// Dither matrix type for GPU shader selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DitherMatrix {
    /// 4x4 Bayer matrix for small gradients
    Bayer4x4,
    /// 8x8 Bayer matrix for gentle ramps (>= 1024 px/step)
    Bayer8x8,
    /// 8x8 medium matrix for moderate ramps (>= 512 px/step)
    Bayer8x8Medium,
    /// 8x8 classic matrix for steep ramps (< 512 px/step)
    Bayer8x8Classic,
}

impl DitherMatrix {
    /// Select the appropriate dither matrix based on gradient characteristics.
    ///
    /// # Arguments
    /// * `gradient_length` - The length of the gradient in device pixels
    /// * `max_color_delta` - Maximum per-channel color difference (0.0-1.0) across the gradient
    ///
    /// # Returns
    /// The recommended dither matrix for this gradient.
    pub fn select(gradient_length: f32, max_color_delta: f32) -> DitherMatrix {
        if !gradient_length.is_finite() || gradient_length <= 0.0 {
            return DitherMatrix::Bayer8x8;
        }

        // Calculate pixels per 8-bit step (delta is in 0-1 range, convert to 0-255)
        let delta_255 = max_color_delta * 255.0;
        if delta_255 <= 0.0001 {
            // Flat gradient, use gentlest dither
            return DitherMatrix::Bayer8x8;
        }

        let pixels_per_step = gradient_length / delta_255;

        // Small gradients use 4x4 matrix
        if gradient_length <= 64.0 {
            return DitherMatrix::Bayer4x4;
        }

        // Select 8x8 variant based on gradient slope
        if pixels_per_step >= 4.0 {
            // Very gentle: >= 1024 px across full 0-255 range
            DitherMatrix::Bayer8x8
        } else if pixels_per_step >= 2.0 {
            // Medium: >= 512 px across full range
            DitherMatrix::Bayer8x8Medium
        } else {
            // Steep: < 512 px across full range
            DitherMatrix::Bayer8x8Classic
        }
    }

    /// Get the dither value for a pixel position.
    ///
    /// Returns a value in the range [0.03125, 0.96875] for 4x4 or [0.0078125, 0.9921875] for 8x8.
    /// This value should be added to each color channel (in 0-255 space) before truncation.
    pub fn dither_value(&self, x: u32, y: u32) -> f32 {
        match self {
            DitherMatrix::Bayer4x4 => {
                let idx = ((y & 3) * 4 + (x & 3)) as usize;
                let m = BAYER_4X4[idx] as f32;
                (m + 0.5) / 16.0  // Range: 0.03125 to 0.96875
            }
            DitherMatrix::Bayer8x8 => {
                let idx = ((y & 7) * 8 + (x & 7)) as usize;
                let m = BAYER_8X8_XY[idx] as f32;
                (m + 0.5) / 64.0  // Range: 0.0078125 to 0.9921875
            }
            DitherMatrix::Bayer8x8Medium => {
                let idx = ((y & 7) * 8 + (x & 7)) as usize;
                let m = BAYER_8X8_MEDIUM_XY[idx] as f32;
                (m + 0.5) / 64.0
            }
            DitherMatrix::Bayer8x8Classic => {
                let idx = ((y & 7) * 8 + (x & 7)) as usize;
                let m = BAYER_8X8_CLASSIC_XY[idx] as f32;
                (m + 0.5) / 64.0
            }
        }
    }

    /// Convert to shader-compatible enum value.
    pub fn to_shader_value(&self) -> u32 {
        match self {
            DitherMatrix::Bayer4x4 => 0,
            DitherMatrix::Bayer8x8 => 1,
            DitherMatrix::Bayer8x8Medium => 2,
            DitherMatrix::Bayer8x8Classic => 3,
        }
    }
}

/// Quantize a floating-point color value (0-255 range) to u8 with ordered dithering.
///
/// This matches Chrome/Skia's `quantize_dither` function.
#[inline(always)]
pub fn quantize_dither(value: f32, dither: f32) -> u8 {
    ((value + dither) as i32).clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bayer_4x4_first_entry() {
        // First entry should be 0, giving dither of 0.03125
        let dither = DitherMatrix::Bayer4x4.dither_value(0, 0);
        assert!((dither - 0.03125).abs() < 0.0001);
    }

    #[test]
    fn test_bayer_4x4_wrapping() {
        // Position (4, 4) should wrap to (0, 0)
        let d1 = DitherMatrix::Bayer4x4.dither_value(0, 0);
        let d2 = DitherMatrix::Bayer4x4.dither_value(4, 4);
        assert!((d1 - d2).abs() < 0.0001);
    }

    #[test]
    fn test_bayer_8x8_range() {
        // Check all values are in expected range
        for y in 0..8 {
            for x in 0..8 {
                let d = DitherMatrix::Bayer8x8.dither_value(x, y);
                assert!(d >= 0.0078125 && d <= 0.9921875, "d = {}", d);
            }
        }
    }

    #[test]
    fn test_matrix_selection() {
        // Small gradient
        assert_eq!(DitherMatrix::select(50.0, 1.0), DitherMatrix::Bayer4x4);

        // Large gentle gradient
        assert_eq!(DitherMatrix::select(2000.0, 0.5), DitherMatrix::Bayer8x8);

        // Steep gradient
        assert_eq!(DitherMatrix::select(100.0, 1.0), DitherMatrix::Bayer8x8Classic);
    }

    #[test]
    fn test_quantize_dither() {
        // Value 127.4 with dither 0.5 should round to 127
        assert_eq!(quantize_dither(127.4, 0.5), 127);

        // Value 127.6 with dither 0.5 should round to 128
        assert_eq!(quantize_dither(127.6, 0.5), 128);

        // Clamping at boundaries
        assert_eq!(quantize_dither(-10.0, 0.5), 0);
        assert_eq!(quantize_dither(300.0, 0.5), 255);
    }
}
