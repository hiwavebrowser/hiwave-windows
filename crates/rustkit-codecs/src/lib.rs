//! # RustKit Codecs
//!
//! Minimal image codec layer used by RustKit to remove the large `image` crate dependency.
//!
//! Current support:
//! - PNG (via `png` crate)
//! - JPEG (via `jpeg-decoder` crate)
//! - GIF (static + animated via `gif` crate)
//!
//! Planned:
//! - WebP
//! - BMP/ICO

use thiserror::Error;

/// Supported image formats (detected by magic bytes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    WebP,
    Bmp,
    Ico,
    Unknown,
}

/// Errors that can occur during decoding.
#[derive(Error, Debug)]
pub enum CodecError {
    #[error("Unsupported image format: {0:?}")]
    Unsupported(ImageFormat),

    #[error("Invalid image data: {0}")]
    Invalid(String),

    #[error("Decode error: {0}")]
    Decode(String),
}

/// A simple RGBA8 image buffer.
#[derive(Debug, Clone)]
pub struct RgbaImage {
    width: u32,
    height: u32,
    data: Vec<u8>, // RGBA8, row-major
}

impl RgbaImage {
    pub fn new(width: u32, height: u32) -> Self {
        let len = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        Self {
            width,
            height,
            data: vec![0u8; len],
        }
    }

    pub fn from_rgba8(width: u32, height: u32, data: Vec<u8>) -> Result<Self, CodecError> {
        let expected = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(4);
        if data.len() != expected {
            return Err(CodecError::Invalid(format!(
                "RGBA buffer length mismatch: got {}, expected {}",
                data.len(),
                expected
            )));
        }
        Ok(Self { width, height, data })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn pixels_mut(&mut self) -> impl Iterator<Item = &mut [u8]> {
        self.data.chunks_exact_mut(4)
    }
}

/// One decoded animation frame.
#[derive(Debug, Clone)]
pub struct Frame {
    pub image: RgbaImage,
    pub delay_ms: u32,
}

/// Decoded output (static or animated).
#[derive(Debug, Clone)]
pub enum Decoded {
    Static(RgbaImage),
    Animated(Vec<Frame>),
}

/// Detect image format by magic bytes (best-effort).
pub fn detect_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.len() >= 8 && &bytes[..8] == b"\x89PNG\r\n\x1a\n" {
        return Some(ImageFormat::Png);
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.len() >= 6 && (&bytes[..6] == b"GIF87a" || &bytes[..6] == b"GIF89a") {
        return Some(ImageFormat::Gif);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::WebP);
    }
    if bytes.len() >= 2 && &bytes[..2] == b"BM" {
        return Some(ImageFormat::Bmp);
    }
    if bytes.len() >= 4 && &bytes[..4] == b"\x00\x00\x01\x00" {
        return Some(ImageFormat::Ico);
    }
    None
}

/// Decode image bytes into RGBA8 (static or animated).
pub fn decode_any(bytes: &[u8]) -> Result<Decoded, CodecError> {
    let fmt = detect_format(bytes).unwrap_or(ImageFormat::Unknown);
    match fmt {
        ImageFormat::Png => Ok(Decoded::Static(decode_png(bytes)?)),
        ImageFormat::Jpeg => Ok(Decoded::Static(decode_jpeg(bytes)?)),
        ImageFormat::Gif => Ok(Decoded::Animated(decode_gif(bytes)?)),
        ImageFormat::WebP => Ok(Decoded::Static(decode_webp(bytes)?)),
        ImageFormat::Bmp | ImageFormat::Ico | ImageFormat::Unknown => {
            Err(CodecError::Unsupported(fmt))
        }
    }
}

pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, CodecError> {
    let mut decoder = png::Decoder::new(bytes);
    // Expand palette/gray to RGB, add alpha, strip 16-bit.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);

    let mut reader = decoder
        .read_info()
        .map_err(|e| CodecError::Decode(e.to_string()))?;

    let mut buf = vec![0; reader.output_buffer_size()];
    let output = reader
        .next_frame(&mut buf)
        .map_err(|e| CodecError::Decode(e.to_string()))?;

    let buf = buf[..output.buffer_size()].to_vec();

    let width = output.width;
    let height = output.height;

    let rgba = match output.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => rgb_to_rgba(buf, 255),
        png::ColorType::GrayscaleAlpha => gray_alpha_to_rgba(buf),
        png::ColorType::Grayscale => gray_to_rgba(buf, 255),
        // After EXPAND, Indexed should not appear, but keep a guard.
        png::ColorType::Indexed => {
            return Err(CodecError::Decode(
                "Indexed PNG remained after EXPAND".to_string(),
            ))
        }
    };

    RgbaImage::from_rgba8(width, height, rgba)
}

/// Decode a WebP still image (lossy or lossless) to RGBA8.
///
/// Detection existed without decoding: `detect_format` returned `WebP` and
/// `decode_any` then returned `Unsupported` — so every WebP on the web
/// failed with a message that read like a missing feature rather than a
/// missing decoder. eBay serves every listing photo as WebP; a live session
/// (2026-08-07) produced 82 of these on one page.
///
/// Animated WebP decodes its first frame here rather than erroring: a still
/// frame is closer to right than a hole, and the animation path belongs with
/// the GIF-style `Decoded::Animated` work, not this bundle.
pub fn decode_webp(bytes: &[u8]) -> Result<RgbaImage, CodecError> {
    let mut decoder = image_webp::WebPDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| CodecError::Decode(e.to_string()))?;
    let (width, height) = decoder.dimensions();
    let has_alpha = decoder.has_alpha();

    let mut buf = vec![0u8; decoder.output_buffer_size().ok_or_else(|| {
        CodecError::Decode("WebP dimensions overflow the output buffer size".to_string())
    })?];
    decoder
        .read_image(&mut buf)
        .map_err(|e| CodecError::Decode(e.to_string()))?;

    // image-webp emits RGB8 or RGBA8 depending on the alpha channel; the
    // engine's texture path is RGBA8 only.
    let rgba = if has_alpha { buf } else { rgb_to_rgba(buf, 255) };
    RgbaImage::from_rgba8(width, height, rgba)
}

pub fn decode_jpeg(bytes: &[u8]) -> Result<RgbaImage, CodecError> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    let pixels = decoder
        .decode()
        .map_err(|e| CodecError::Decode(e.to_string()))?;
    let info = decoder
        .info()
        .ok_or_else(|| CodecError::Decode("Missing JPEG info".into()))?;

    let width = info.width as u32;
    let height = info.height as u32;

    // jpeg-decoder outputs RGB (or grayscale). Treat grayscale as RGB.
    let rgba = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => rgb_to_rgba(pixels, 255),
        jpeg_decoder::PixelFormat::L8 => gray_to_rgba(pixels, 255),
        other => {
            return Err(CodecError::Decode(format!(
                "Unsupported JPEG pixel format: {:?}",
                other
            )))
        }
    };

    RgbaImage::from_rgba8(width, height, rgba)
}

pub fn decode_gif(bytes: &[u8]) -> Result<Vec<Frame>, CodecError> {
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::RGBA);
    let mut decoder = opts
        .read_info(std::io::Cursor::new(bytes))
        .map_err(|e| CodecError::Decode(e.to_string()))?;

    let mut frames = Vec::new();
    while let Some(frame) = decoder
        .read_next_frame()
        .map_err(|e| CodecError::Decode(e.to_string()))?
    {
        let width = frame.width as u32;
        let height = frame.height as u32;
        let rgba = frame.buffer.to_vec(); // already RGBA
        let image = RgbaImage::from_rgba8(width, height, rgba)?;
        // Delay is in 1/100s units. Convert to ms, minimum 10ms.
        let delay_ms = (frame.delay as u32).saturating_mul(10).max(10);
        frames.push(Frame { image, delay_ms });
    }

    if frames.is_empty() {
        return Err(CodecError::Decode("GIF has no frames".into()));
    }

    Ok(frames)
}

fn rgb_to_rgba(rgb: Vec<u8>, alpha: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgb.len() / 3 * 4);
    for chunk in rgb.chunks_exact(3) {
        out.push(chunk[0]);
        out.push(chunk[1]);
        out.push(chunk[2]);
        out.push(alpha);
    }
    out
}

fn gray_to_rgba(gray: Vec<u8>, alpha: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(gray.len() * 4);
    for g in gray {
        out.push(g);
        out.push(g);
        out.push(g);
        out.push(alpha);
    }
    out
}

fn gray_alpha_to_rgba(ga: Vec<u8>) -> Vec<u8> {
    let mut out = Vec::with_capacity(ga.len() / 2 * 4);
    for chunk in ga.chunks_exact(2) {
        let g = chunk[0];
        out.push(g);
        out.push(g);
        out.push(g);
        out.push(chunk[1]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_format_png() {
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend_from_slice(&[0u8; 16]);
        assert_eq!(detect_format(&bytes), Some(ImageFormat::Png));
    }

    #[test]
    fn test_detect_format_jpeg() {
        let bytes = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0];
        assert_eq!(detect_format(&bytes), Some(ImageFormat::Jpeg));
    }

    #[test]
    fn test_detect_format_gif() {
        let bytes = b"GIF89a....";
        assert_eq!(detect_format(bytes), Some(ImageFormat::Gif));
    }
}


