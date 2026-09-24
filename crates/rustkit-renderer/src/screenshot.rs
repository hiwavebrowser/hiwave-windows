//! Screenshot capture via GPU readback.
//!
//! Provides functionality to capture rendered frames to PNG/PPM files
//! for testing and debugging purposes.

use std::path::Path;

/// Error type for screenshot operations.
#[derive(Debug, thiserror::Error)]
pub enum ScreenshotError {
    #[error("Buffer mapping failed")]
    BufferMapFailed,
    
    #[error("Image encoding failed: {0}")]
    ImageEncoding(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Screenshot metadata for test verification.
#[derive(Debug, Clone)]
pub struct ScreenshotMetadata {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// GPU adapter name.
    pub adapter: String,
    /// Texture format used.
    pub format: String,
    /// Timestamp of capture.
    pub timestamp: String,
    /// Number of display commands rendered.
    pub command_count: usize,
}

/// GPU readback buffer for capturing rendered frames.
pub struct GpuReadbackBuffer {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    bytes_per_row: u32,
}

impl GpuReadbackBuffer {
    /// Create a new readback buffer for the given dimensions.
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        // RGBA8/BGRA8 = 4 bytes per pixel, aligned to 256 bytes
        let bytes_per_row = (width * 4 + 255) & !255;
        let buffer_size = bytes_per_row * height;
        
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Screenshot Readback Buffer"),
            size: buffer_size as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        
        Self {
            buffer,
            width,
            height,
            bytes_per_row,
        }
    }
    
    /// Copy from a texture to this readback buffer.
    pub fn copy_from_texture(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) {
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.bytes_per_row),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
    }
    
    /// Read buffer data synchronously (blocks current thread).
    pub fn read_data_sync(&self, device: &wgpu::Device) -> Result<Vec<u8>, ScreenshotError> {
        let buffer_slice = self.buffer.slice(..);
        
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        
        device.poll(wgpu::Maintain::Wait);
        
        rx.recv()
            .map_err(|_| ScreenshotError::BufferMapFailed)?
            .map_err(|_| ScreenshotError::BufferMapFailed)?;
        
        let data = buffer_slice.get_mapped_range();
        
        // Remove row padding
        let mut result = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height {
            let start = (y * self.bytes_per_row) as usize;
            let end = start + (self.width * 4) as usize;
            result.extend_from_slice(&data[start..end]);
        }
        
        drop(data);
        self.buffer.unmap();
        
        Ok(result)
    }
    
    /// Get dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

/// Save BGRA pixel data as PPM (converts to RGB).
pub fn save_ppm(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    bgra_data: &[u8],
) -> Result<(), ScreenshotError> {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    
    // PPM header
    writeln!(writer, "P6")?;
    writeln!(writer, "{} {}", width, height)?;
    writeln!(writer, "255")?;
    
    // Convert BGRA to RGB
    let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
    for chunk in bgra_data.chunks_exact(4) {
        rgb_data.push(chunk[2]); // R (from B position in BGRA)
        rgb_data.push(chunk[1]); // G
        rgb_data.push(chunk[0]); // B (from R position in BGRA)
    }
    
    writer.write_all(&rgb_data)?;
    
    Ok(())
}

/// Save RGBA pixel data as PPM (converts to RGB).
pub fn save_rgba_as_ppm(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    rgba_data: &[u8],
) -> Result<(), ScreenshotError> {
    use std::fs::File;
    use std::io::{BufWriter, Write};
    
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    
    // PPM header
    writeln!(writer, "P6")?;
    writeln!(writer, "{} {}", width, height)?;
    writeln!(writer, "255")?;
    
    // Convert RGBA to RGB (just drop alpha)
    let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
    for chunk in rgba_data.chunks_exact(4) {
        rgb_data.push(chunk[0]); // R
        rgb_data.push(chunk[1]); // G
        rgb_data.push(chunk[2]); // B
    }
    
    writer.write_all(&rgb_data)?;
    
    Ok(())
}

/// Create an offscreen render target for screenshot capture.
pub fn create_offscreen_target(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Offscreen Screenshot Target"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    
    (texture, view)
}

/// Compare two images pixel-by-pixel with a tolerance.
/// Returns the number of pixels that differ beyond the tolerance.
/// Input should be RGBA data (4 bytes per pixel).
pub fn compare_images(
    expected: &[u8],
    actual: &[u8],
    tolerance: u8,
) -> usize {
    if expected.len() != actual.len() {
        return expected.len().max(actual.len()) / 4;
    }
    
    let mut diff_count = 0;
    
    // Compare RGBA pixels (4 bytes each)
    for (e_chunk, a_chunk) in expected.chunks(4).zip(actual.chunks(4)) {
        let r_diff = (e_chunk[0] as i16 - a_chunk[0] as i16).unsigned_abs() as u8;
        let g_diff = (e_chunk[1] as i16 - a_chunk[1] as i16).unsigned_abs() as u8;
        let b_diff = (e_chunk[2] as i16 - a_chunk[2] as i16).unsigned_abs() as u8;
        
        // If any channel differs beyond tolerance, count the pixel
        if r_diff > tolerance || g_diff > tolerance || b_diff > tolerance {
            diff_count += 1;
        }
    }
    
    diff_count
}

/// Generate a diff image highlighting differences.
pub fn generate_diff_image(
    expected: &[u8],
    actual: &[u8],
    _width: u32,
    _height: u32,
    tolerance: u8,
) -> Vec<u8> {
    let mut diff = Vec::with_capacity(expected.len());
    
    // Process as RGBA
    for (e_chunk, a_chunk) in expected.chunks(4).zip(actual.chunks(4)) {
        let r_diff = (e_chunk.get(0).unwrap_or(&0).wrapping_sub(*a_chunk.get(0).unwrap_or(&0))) as i16;
        let g_diff = (e_chunk.get(1).unwrap_or(&0).wrapping_sub(*a_chunk.get(1).unwrap_or(&0))) as i16;
        let b_diff = (e_chunk.get(2).unwrap_or(&0).wrapping_sub(*a_chunk.get(2).unwrap_or(&0))) as i16;
        
        let max_diff = r_diff.abs().max(g_diff.abs()).max(b_diff.abs()) as u8;
        
        if max_diff > tolerance {
            // Highlight differences in red
            diff.push(255); // R
            diff.push(0);   // G
            diff.push(0);   // B
            diff.push(255); // A
        } else {
            // Show original (dimmed)
            diff.push(a_chunk.get(0).unwrap_or(&0) / 2);
            diff.push(a_chunk.get(1).unwrap_or(&0) / 2);
            diff.push(a_chunk.get(2).unwrap_or(&0) / 2);
            diff.push(255);
        }
    }
    
    diff
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_compare_identical_images() {
        let img = vec![255, 0, 0, 255, 0, 255, 0, 255]; // 2 RGBA pixels
        assert_eq!(compare_images(&img, &img, 0), 0);
    }
    
    #[test]
    fn test_compare_different_images() {
        let img1 = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let img2 = vec![0, 0, 0, 255, 0, 0, 0, 255];
        assert!(compare_images(&img1, &img2, 0) > 0);
    }
    
    #[test]
    fn test_compare_within_tolerance() {
        let img1 = vec![100, 100, 100, 255];
        let img2 = vec![105, 105, 105, 255];
        assert_eq!(compare_images(&img1, &img2, 10), 0);
    }
}

// ── Windows capture path (native shell screenshot harness) ─────────────────
//
// The PPM `ScreenshotMetadata` above belongs to the parity-capture
// instrument. The native-win32 shell captures a view to PNG with a JSON
// sidecar and reads the vertex counts and adapter back out of it; that is
// this type. Windows-only so the crate is behaviour-identical on macOS.

/// Metadata written next to a PNG capture by `Renderer::execute_and_capture`.
#[cfg(windows)]
#[derive(Debug, Clone)]
pub struct CaptureMetadata {
    pub width: u32,
    pub height: u32,
    /// GPU adapter name.
    pub adapter: String,
    /// Texture format used.
    pub format: String,
    /// Timestamp of capture.
    pub timestamp: String,
    /// Number of color vertices rendered.
    pub color_vertex_count: usize,
    /// Number of texture vertices rendered (text/images).
    pub texture_vertex_count: usize,
}

/// Save RGBA8 pixels as a PNG.
#[cfg(windows)]
pub fn save_png(
    path: impl AsRef<Path>,
    width: u32,
    height: u32,
    rgba_data: &[u8],
) -> Result<(), ScreenshotError> {
    use std::fs::File;
    use std::io::BufWriter;

    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png_writer = encoder
        .write_header()
        .map_err(|e| ScreenshotError::ImageEncoding(e.to_string()))?;
    png_writer
        .write_image_data(rgba_data)
        .map_err(|e| ScreenshotError::ImageEncoding(e.to_string()))?;
    Ok(())
}

/// Save capture metadata as a small JSON document (no serde dependency).
#[cfg(windows)]
pub fn save_capture_metadata(
    path: impl AsRef<Path>,
    metadata: &CaptureMetadata,
) -> Result<(), ScreenshotError> {
    fn esc(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }
    let json = format!(
        concat!(
            "{{\n",
            "  \"width\": {},\n",
            "  \"height\": {},\n",
            "  \"adapter\": \"{}\",\n",
            "  \"format\": \"{}\",\n",
            "  \"timestamp\": \"{}\",\n",
            "  \"color_vertex_count\": {},\n",
            "  \"texture_vertex_count\": {}\n",
            "}}\n"
        ),
        metadata.width,
        metadata.height,
        esc(&metadata.adapter),
        esc(&metadata.format),
        esc(&metadata.timestamp),
        metadata.color_vertex_count,
        metadata.texture_vertex_count,
    );
    std::fs::write(path, json)?;
    Ok(())
}


#[cfg(all(test, windows))]
mod windows_capture_metadata_pins {
    use super::*;

    /// The JSON sidecar written next to a PNG capture names every field the
    /// native shell reads back (ported from the Windows serde round-trip test).
    #[test]
    fn capture_metadata_sidecar_names_every_field() {
        let metadata = CaptureMetadata {
            width: 800,
            height: 600,
            adapter: "Test Adapter".to_string(),
            format: "Rgba8UnormSrgb".to_string(),
            timestamp: "2025-01-04T12:00:00Z".to_string(),
            color_vertex_count: 100,
            texture_vertex_count: 50,
        };
        let dir = std::env::temp_dir().join(format!("rk-capture-meta-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("meta.json");
        save_capture_metadata(&path, &metadata).unwrap();
        let json = std::fs::read_to_string(&path).unwrap();
        for needle in ["\"width\": 800", "\"height\": 600", "Test Adapter", "Rgba8UnormSrgb",
                       "\"color_vertex_count\": 100", "\"texture_vertex_count\": 50"] {
            assert!(json.contains(needle), "sidecar missing {needle}: {json}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
