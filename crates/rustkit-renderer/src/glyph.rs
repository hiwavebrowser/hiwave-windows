//! Glyph cache for text rendering.
//!
//! Caches rasterized glyphs in a GPU texture atlas.

use crate::RendererError;
use hashbrown::HashMap;
use std::path::Path;
use std::path::PathBuf;
use std::{env, fs};
#[cfg(windows)]
use rustkit_text::{FontCollection as RkFontCollection, FontStretch as RkFontStretch, FontStyle as RkFontStyle, FontWeight as RkFontWeight};
#[cfg(windows)]
use windows::Win32::Graphics::DirectWrite::*;

/// Key for identifying a specific glyph.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct GlyphKey {
    pub codepoint: char,
    pub font_family: String,
    pub font_size: u32, // Fixed-point (size * 10)
    pub font_weight: u16,
    pub font_style: u8, // 0 = normal, 1 = italic
}

/// Cached glyph entry.
#[derive(Debug, Clone)]
pub struct GlyphEntry {
    /// Texture coordinates in atlas [u0, v0, u1, v1].
    pub tex_coords: [f32; 4],
    /// Offset from cursor position.
    pub offset: [f32; 2],
    /// Horizontal advance.
    pub advance: f32,
}

/// Glyph atlas for caching rasterized glyphs.
pub struct GlyphCache {
    atlas: wgpu::Texture,
    _atlas_view: wgpu::TextureView,
    bind_group: wgpu::BindGroup,
    atlas_size: u32,
    entries: HashMap<GlyphKey, GlyphEntry>,
    /// CPU mirror of the atlas (R8 coverage). Used for deterministic debug dumps.
    cpu_atlas: Vec<u8>,
    next_x: u32,
    next_y: u32,
    row_height: u32,
}

impl GlyphCache {
    /// Default atlas size (2048x2048).
    pub const DEFAULT_ATLAS_SIZE: u32 = 2048;

    /// Create a new glyph cache.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bind_group_layout: wgpu::BindGroupLayout,
    ) -> Result<Self, RendererError> {
        let atlas_size = Self::DEFAULT_ATLAS_SIZE;

        // Create atlas texture
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Glyph Atlas"),
            size: wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });

        // Initialize with transparent
        let empty_data = vec![0u8; (atlas_size * atlas_size) as usize];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &empty_data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas_size),
                rows_per_image: Some(atlas_size),
            },
            wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
        );

        let atlas_view = atlas.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("glyph_atlas_bind_group"),
        });

        Ok(Self {
            atlas,
            _atlas_view: atlas_view,
            bind_group,
            atlas_size,
            entries: HashMap::new(),
            cpu_atlas: empty_data,
            next_x: 1, // Start at 1 to avoid edge artifacts
            next_y: 1,
            row_height: 0,
        })
    }

    /// Get the atlas size.
    pub fn atlas_size(&self) -> u32 {
        self.atlas_size
    }

    /// Get the bind group for the atlas texture.
    pub fn bind_group(&self) -> &wgpu::BindGroup {
        &self.bind_group
    }

    /// Dump the current glyph atlas (CPU mirror) to a PNG for debugging.
    ///
    /// The atlas is R8 coverage; we visualize it as grayscale (RGB) with alpha=255.
    pub fn dump_cpu_atlas_png(&self, path: impl AsRef<Path>) -> Result<(), RendererError> {
        let size = self.atlas_size as usize;
        let mut rgba = vec![0u8; size * size * 4];
        for (i, a) in self.cpu_atlas.iter().copied().enumerate() {
            let o = i * 4;
            rgba[o] = a;
            rgba[o + 1] = a;
            rgba[o + 2] = a;
            rgba[o + 3] = 255;
        }
        crate::screenshot::save_png(path, self.atlas_size, self.atlas_size, &rgba)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))
    }

    fn blit_into_cpu_atlas(&mut self, x: u32, y: u32, w: u32, h: u32, src: &[u8]) {
        let atlas_w = self.atlas_size as usize;
        let x0 = x as usize;
        let y0 = y as usize;
        let w_us = w as usize;
        for row in 0..h as usize {
            let src_off = row * w_us;
            let dst_off = (y0 + row) * atlas_w + x0;
            if src_off + w_us <= src.len() && dst_off + w_us <= self.cpu_atlas.len() {
                self.cpu_atlas[dst_off..dst_off + w_us].copy_from_slice(&src[src_off..src_off + w_us]);
            }
        }
    }

    fn maybe_dump_glyph_bitmap(&self, key: &GlyphKey, w: u32, h: u32, alpha: &[u8]) {
        let dump_dir = match env::var_os("RUSTKIT_GLYPH_DUMP_DIR") {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => return,
        };

        let chars = env::var("RUSTKIT_GLYPH_DUMP_CHARS").unwrap_or_else(|_| "A,g,#".to_string());
        let want = chars
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .any(|s| s.chars().next() == Some(key.codepoint));
        if !want {
            return;
        }

        if fs::create_dir_all(&dump_dir).is_err() {
            return;
        }

        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for (i, a) in alpha.iter().copied().enumerate() {
            let o = i * 4;
            rgba[o] = a;
            rgba[o + 1] = a;
            rgba[o + 2] = a;
            rgba[o + 3] = 255;
        }

        let cp = key.codepoint as u32;
        let path = dump_dir.join(format!("glyph_{:04X}_{}.png", cp, key.codepoint));
        let _ = crate::screenshot::save_png(path, w, h, &rgba);
    }

    /// Get or rasterize a glyph.
    pub fn get_or_rasterize(
        &mut self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &GlyphKey,
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.entries.get(key) {
            return Some(entry.clone());
        }

        // Use DirectWrite on Windows for proper glyph rendering
        #[cfg(windows)]
        {
            return self.rasterize_glyph_directwrite(queue, key);
        }
        
        // Fallback for non-Windows platforms
        #[cfg(not(windows))]
        {
            return self.rasterize_glyph_fallback(queue, key);
        }
    }

    /// Rasterize a glyph using DirectWrite.
    #[cfg(windows)]
    fn rasterize_glyph_directwrite(
        &mut self,
        queue: &wgpu::Queue,
        key: &GlyphKey,
    ) -> Option<GlyphEntry> {
        use windows::core::PCWSTR;
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

        tracing::trace!("Rasterizing glyph '{}' (U+{:04X}) size={} family={}",
            key.codepoint, key.codepoint as u32, key.font_size, key.font_family);

        // key.font_size is fixed-point (size * 10), so divide by 10.0
        let font_size = key.font_size as f32 / 10.0;

        // Resolve CSS generic font families to actual Windows fonts
        let resolved_family = match key.font_family.as_str() {
            "sans-serif" => "Segoe UI",
            "serif" => "Times New Roman",
            "monospace" => "Consolas",
            "cursive" => "Comic Sans MS",
            "fantasy" => "Impact",
            _ => &key.font_family,
        };

        unsafe {
            // Ensure COM is initialized
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

            // Create DirectWrite factory
            let factory: IDWriteFactory = match DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("Failed to create DWrite factory for '{}': {:?}", key.codepoint, e);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            };
            
            // Get system font collection
            let mut collection: Option<IDWriteFontCollection> = None;
            if factory.GetSystemFontCollection(&mut collection, false).is_err() {
                tracing::warn!("Failed to get font collection for '{}' family={}", key.codepoint, key.font_family);
                return self.rasterize_glyph_fallback(queue, key);
            }
            let collection = match collection {
                Some(c) => c,
                None => {
                    tracing::warn!("GetSystemFontCollection returned Ok but collection is None for '{}'", key.codepoint);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            };

            // Find font family
            let family_wide: Vec<u16> = resolved_family.encode_utf16().chain(std::iter::once(0)).collect();
            let mut index: u32 = 0;
            let mut exists = windows::core::BOOL(0);
            if collection.FindFamilyName(PCWSTR(family_wide.as_ptr()), &mut index, &mut exists).is_err() || !exists.as_bool() {
                tracing::trace!("FindFamilyName failed or font not found for '{}', trying fallbacks", resolved_family);
                // Try fallback fonts
                let fallbacks = ["Segoe UI", "Arial", "Tahoma"];
                let mut found = false;
                for fallback in fallbacks {
                    let fb_wide: Vec<u16> = fallback.encode_utf16().chain(std::iter::once(0)).collect();
                    if collection.FindFamilyName(PCWSTR(fb_wide.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool() {
                        tracing::trace!("Using fallback font '{}' for '{}'", fallback, key.codepoint);
                        found = true;
                        break;
                    }
                }
                if !found {
                    tracing::error!("No fallback font found for '{}' family={}", key.codepoint, key.font_family);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            } else {
            }

            // Get font family
            let family = match collection.GetFontFamily(index) {
                Ok(f) => {
                    f
                }
                Err(e) => {
                    tracing::error!("GetFontFamily FAILED: {:?}", e);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            };


            // Get matching font
            let dw_weight = DWRITE_FONT_WEIGHT(key.font_weight as i32);
            let dw_stretch = DWRITE_FONT_STRETCH(5); // Normal
            let dw_style = if key.font_style == 1 { DWRITE_FONT_STYLE_ITALIC } else { DWRITE_FONT_STYLE_NORMAL };

            let font = match family.GetFirstMatchingFont(dw_weight, dw_stretch, dw_style) {
                Ok(f) => {
                    f
                }
                Err(e) => {
                    tracing::error!("GetFirstMatchingFont FAILED: {:?}", e);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            };

            // Create font face
            let face = match font.CreateFontFace() {
                Ok(f) => {
                    f
                }
                Err(e) => {
                    tracing::error!("CreateFontFace FAILED: {:?}", e);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            };


            // Get glyph index for codepoint
            let codepoint = key.codepoint as u32;
            let mut glyph_indices = [0u16; 1];
            if face.GetGlyphIndices(&codepoint as *const u32, 1, glyph_indices.as_mut_ptr()).is_err() {
                tracing::error!("GetGlyphIndices FAILED (returned Err) for '{}' (U+{:04X})", key.codepoint, codepoint);
                return self.rasterize_glyph_fallback(queue, key);
            }

            let glyph_index = glyph_indices[0];
            if glyph_index == 0 {
                tracing::error!("GetGlyphIndices returned 0 (glyph not found in font) for '{}' (U+{:04X})", key.codepoint, codepoint);
                return self.rasterize_glyph_fallback(queue, key);
            }


            // Get font metrics for baseline calculation
            let mut font_metrics = DWRITE_FONT_METRICS::default();
            face.GetMetrics(&mut font_metrics);
            let design_units_per_em = font_metrics.designUnitsPerEm as f32;
            let ascent = font_metrics.ascent as f32 * font_size / design_units_per_em;

            // Get glyph metrics
            let mut glyph_metrics = [DWRITE_GLYPH_METRICS::default()];
            if face.GetDesignGlyphMetrics(&glyph_index, 1, glyph_metrics.as_mut_ptr(), false).is_err() {
                tracing::error!("GetDesignGlyphMetrics FAILED");
                return self.rasterize_glyph_fallback(queue, key);
            }

            let advance_width = glyph_metrics[0].advanceWidth as f32 * font_size / design_units_per_em;
            let left_bearing = glyph_metrics[0].leftSideBearing as f32 * font_size / design_units_per_em;
            let top_bearing = glyph_metrics[0].topSideBearing as f32 * font_size / design_units_per_em;
            let glyph_width_design = (glyph_metrics[0].advanceWidth as i32 - glyph_metrics[0].leftSideBearing - glyph_metrics[0].rightSideBearing) as f32;
            let glyph_height_design = (glyph_metrics[0].advanceHeight as i32 - glyph_metrics[0].topSideBearing - glyph_metrics[0].bottomSideBearing) as f32;
            
            let glyph_width = ((glyph_width_design * font_size / design_units_per_em).ceil() as u32).max(1).min(256);
            let glyph_height = ((glyph_height_design * font_size / design_units_per_em).ceil() as u32).max(1).min(256);
            
            // For whitespace, use minimal dimensions
            if key.codepoint.is_whitespace() {
                let (w, h) = estimate_glyph_size(key.codepoint, font_size);
                let (atlas_x, atlas_y) = self.allocate_space(w + 2, h + 2)?;
                let bitmap = vec![0u8; (w * h) as usize];
                
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.atlas,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x: atlas_x + 1, y: atlas_y + 1, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bitmap,
                    wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w), rows_per_image: Some(h) },
                    wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                );
                
                let u0 = (atlas_x + 1) as f32 / self.atlas_size as f32;
                let v0 = (atlas_y + 1) as f32 / self.atlas_size as f32;
                let u1 = (atlas_x + 1 + w) as f32 / self.atlas_size as f32;
                let v1 = (atlas_y + 1 + h) as f32 / self.atlas_size as f32;
                
                let entry = GlyphEntry {
                    tex_coords: [u0, v0, u1, v1],
                    offset: [0.0, 0.0],
                    advance: advance_width,
                };
                self.entries.insert(key.clone(), entry.clone());
                return Some(entry);
            }
            
            // Create glyph run for rendering
            // Note: glyphAdvances is null to let DirectWrite calculate advances automatically
            let glyph_run = DWRITE_GLYPH_RUN {
                fontFace: std::mem::ManuallyDrop::new(Some(face.clone())),
                fontEmSize: font_size,
                glyphCount: 1,
                glyphIndices: &glyph_index,
                glyphAdvances: std::ptr::null(),
                glyphOffsets: std::ptr::null(),
                isSideways: windows::core::BOOL(0),
                bidiLevel: 0,
            };
            
            // Create identity transform matrix
            let transform = DWRITE_MATRIX {
                m11: 1.0, m12: 0.0,
                m21: 0.0, m22: 1.0,
                dx: 0.0, dy: 0.0,
            };

            // Create glyph run analysis
            // Try ALIASED rendering mode instead of NATURAL
            let analysis: IDWriteGlyphRunAnalysis = match factory.CreateGlyphRunAnalysis(
                &glyph_run,
                1.0, // pixels per DIP
                Some(&transform), // Provide identity matrix instead of None
                DWRITE_RENDERING_MODE_ALIASED,
                DWRITE_MEASURING_MODE_NATURAL,
                0.0, // baseline origin x
                0.0, // baseline origin y
            ) {
                Ok(a) => {
                    a
                }
                Err(e) => {
                    tracing::warn!("CreateGlyphRunAnalysis failed for '{}': {:?}", key.codepoint, e);
                    // Clean up manually dropped face
                    std::mem::ManuallyDrop::into_inner(glyph_run.fontFace);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            };

            // Get texture bounds
            let bounds = match analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) {
                Ok(b) => {
                    tracing::trace!("Got aliased texture bounds for '{}': left={}, top={}, right={}, bottom={} ({}x{})",
                        key.codepoint, b.left, b.top, b.right, b.bottom, b.right - b.left, b.bottom - b.top);
                    b
                }
                Err(e1) => match analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1) {
                    Ok(b) => {
                        tracing::trace!("Got cleartype texture bounds for '{}': left={}, top={}, right={}, bottom={} ({}x{})",
                            key.codepoint, b.left, b.top, b.right, b.bottom, b.right - b.left, b.bottom - b.top);
                        b
                    }
                    Err(e2) => {
                        tracing::error!("GetAlphaTextureBounds FAILED for '{}': aliased={:?}, cleartype={:?}", key.codepoint, e1, e2);
                        std::mem::ManuallyDrop::into_inner(glyph_run.fontFace);
                        return self.rasterize_glyph_fallback(queue, key);
                    }
                },
            };

            let tex_width = (bounds.right - bounds.left) as u32;
            let tex_height = (bounds.bottom - bounds.top) as u32;
            tracing::trace!("Texture dimensions: {}x{}", tex_width, tex_height);

            if tex_width == 0 || tex_height == 0 {
                tracing::trace!("Texture dimensions are zero (whitespace glyph)");
                // Empty glyph (whitespace)
                std::mem::ManuallyDrop::into_inner(glyph_run.fontFace);
                return self.rasterize_glyph_fallback(queue, key);
            }

            
            // Allocate atlas space
            let (atlas_x, atlas_y) = match self.allocate_space(tex_width + 2, tex_height + 2) {
                Some(pos) => pos,
                None => {
                    std::mem::ManuallyDrop::into_inner(glyph_run.fontFace);
                    return None;
                }
            };
            
            // Get alpha texture (grayscale bitmap)
            let mut alpha_values = vec![0u8; (tex_width * tex_height) as usize];
            if analysis.CreateAlphaTexture(
                DWRITE_TEXTURE_ALIASED_1x1,
                &bounds,
                alpha_values.as_mut_slice(),
            ).is_err() {
                // Try cleartype and convert
                let mut ct_values = vec![0u8; (tex_width * tex_height * 3) as usize];
                if analysis.CreateAlphaTexture(
                    DWRITE_TEXTURE_CLEARTYPE_3x1,
                    &bounds,
                    ct_values.as_mut_slice(),
                ).is_ok() {
                    // Convert ClearType (3 bytes per pixel) to grayscale
                    for i in 0..(tex_width * tex_height) as usize {
                        let r = ct_values[i * 3] as u32;
                        let g = ct_values[i * 3 + 1] as u32;
                        let b = ct_values[i * 3 + 2] as u32;
                        alpha_values[i] = ((r + g + b) / 3) as u8;
                    }
                } else {
                    std::mem::ManuallyDrop::into_inner(glyph_run.fontFace);
                    return self.rasterize_glyph_fallback(queue, key);
                }
            }

            // Debug dump + CPU atlas mirror before upload.
            self.maybe_dump_glyph_bitmap(key, tex_width, tex_height, &alpha_values);
            self.blit_into_cpu_atlas(atlas_x + 1, atlas_y + 1, tex_width, tex_height, &alpha_values);
            
            // Clean up manually dropped face
            std::mem::ManuallyDrop::into_inner(glyph_run.fontFace);
            
            // Upload to atlas
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: atlas_x + 1, y: atlas_y + 1, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &alpha_values,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(tex_width),
                    rows_per_image: Some(tex_height),
                },
                wgpu::Extent3d { width: tex_width, height: tex_height, depth_or_array_layers: 1 },
            );
            
            let u0 = (atlas_x + 1) as f32 / self.atlas_size as f32;
            let v0 = (atlas_y + 1) as f32 / self.atlas_size as f32;
            let u1 = (atlas_x + 1 + tex_width) as f32 / self.atlas_size as f32;
            let v1 = (atlas_y + 1 + tex_height) as f32 / self.atlas_size as f32;
            
            // Calculate offset from cursor position to glyph origin
            // bounds.left/top are in pixels relative to the glyph run origin (baseline)
            // We need to position the glyph texture such that when drawn at (cursor_x, cursor_y),
            // the glyph appears correctly on the baseline.
            //
            // For text rendering:
            // - cursor_y is the TOP of the text line in our coordinate system (y increases downward)
            // - bounds.top is typically negative (glyph extends above baseline)
            // - We want to position glyphs relative to the text line top
            let offset_x = bounds.left as f32;
            let offset_y = ascent + bounds.top as f32; // Position relative to line top
            
            tracing::trace!(
                codepoint = ?key.codepoint,
                bounds_left = bounds.left,
                bounds_top = bounds.top,
                tex_width,
                tex_height,
                ascent,
                offset_x,
                offset_y,
                advance_width,
                "Glyph rasterized via DirectWrite"
            );
            
            let entry = GlyphEntry {
                tex_coords: [u0, v0, u1, v1],
                offset: [offset_x, offset_y],
                advance: advance_width,
            };
            
            self.entries.insert(key.clone(), entry.clone());
            Some(entry)
        }
    }
    
    /// Fallback glyph rasterization (creates placeholder rectangles).
    fn rasterize_glyph_fallback(
        &mut self,
        queue: &wgpu::Queue,
        key: &GlyphKey,
    ) -> Option<GlyphEntry> {
        tracing::warn!("Using fallback rectangle for glyph '{}' (U+{:04X}) size={} family={}",
            key.codepoint, key.codepoint as u32, key.font_size, key.font_family);

        let font_size = key.font_size as f32 / 10.0;
        
        #[cfg(windows)]
        {
            // Silence unused import warnings
            let _ = (RkFontCollection::system, RkFontWeight::from_u32, RkFontStretch::from_u32);
            let _ = |s: u8| match s { 0 => RkFontStyle::Normal, 1 => RkFontStyle::Italic, _ => RkFontStyle::Normal };
        }

        // Estimate glyph dimensions based on character (fallback)
        let (glyph_width, glyph_height) = estimate_glyph_size(key.codepoint, font_size);

        let glyph_width = glyph_width.max(1).min(256);
        let glyph_height = glyph_height.max(1).min(256);

        // Allocate space
        let (atlas_x, atlas_y) = self.allocate_space(glyph_width + 2, glyph_height + 2)?;

        // Create simple glyph bitmap (filled rectangle for fallback)
        let mut bitmap = vec![0u8; (glyph_width * glyph_height) as usize];
        
        // For printable characters, create a visible shape
        if key.codepoint.is_ascii_graphic() || key.codepoint.is_alphabetic() {
            for y in 0..glyph_height {
                for x in 0..glyph_width {
                    let idx = (y * glyph_width + x) as usize;
                    // Create a simple pattern
                    let border = x == 0 || x == glyph_width - 1 || y == 0 || y == glyph_height - 1;
                    bitmap[idx] = if border { 255 } else { 200 };
                }
            }
        }
        // Whitespace characters remain transparent

        // Upload
        self.maybe_dump_glyph_bitmap(key, glyph_width, glyph_height, &bitmap);
        self.blit_into_cpu_atlas(atlas_x + 1, atlas_y + 1, glyph_width, glyph_height, &bitmap);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: atlas_x + 1,
                    y: atlas_y + 1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &bitmap,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(glyph_width),
                rows_per_image: Some(glyph_height),
            },
            wgpu::Extent3d {
                width: glyph_width,
                height: glyph_height,
                depth_or_array_layers: 1,
            },
        );

        let u0 = (atlas_x + 1) as f32 / self.atlas_size as f32;
        let v0 = (atlas_y + 1) as f32 / self.atlas_size as f32;
        let u1 = (atlas_x + 1 + glyph_width) as f32 / self.atlas_size as f32;
        let v1 = (atlas_y + 1 + glyph_height) as f32 / self.atlas_size as f32;

        // For fallback glyphs, position them at the text line top
        // The glyph should start at y=0 relative to the line top
        let entry = GlyphEntry {
            tex_coords: [u0, v0, u1, v1],
            offset: [0.0, 0.0], // Start at line top
            advance: glyph_width as f32,
        };

        self.entries.insert(key.clone(), entry.clone());
        Some(entry)
    }

    /// Allocate space in the atlas.
    fn allocate_space(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        // Check if we need a new row
        if self.next_x + width > self.atlas_size {
            self.next_x = 1;
            self.next_y += self.row_height + 1;
            self.row_height = 0;
        }

        // Check if we've run out of space
        if self.next_y + height > self.atlas_size {
            tracing::warn!("Glyph atlas full, clearing cache");
            self.entries.clear();
            self.next_x = 1;
            self.next_y = 1;
            self.row_height = 0;
        }

        let x = self.next_x;
        let y = self.next_y;

        self.next_x += width + 1;
        self.row_height = self.row_height.max(height);

        Some((x, y))
    }

    /// Clear the cache.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.cpu_atlas.fill(0);
        self.next_x = 1;
        self.next_y = 1;
        self.row_height = 0;
    }
    
    /// Dump the glyph atlas to a PNG file for debugging.
    ///
    /// This reads back the atlas texture from the GPU and saves it as a grayscale PNG.
    pub fn dump_atlas_to_file(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output_path: impl AsRef<Path>,
    ) -> Result<(), crate::RendererError> {
        use std::fs::File;
        use std::io::BufWriter;
        
        let size = self.atlas_size;
        
        // Create readback buffer
        let bytes_per_row = (size + 255) & !255; // Align to 256 bytes
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Atlas Readback Buffer"),
            size: (bytes_per_row * size) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        
        // Copy texture to buffer
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Atlas Readback Encoder"),
        });
        
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(size),
                },
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
        
        queue.submit(std::iter::once(encoder.finish()));
        
        // Read back the data
        let buffer_slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
        
        device.poll(wgpu::Maintain::Wait);
        
        rx.recv()
            .map_err(|_| crate::RendererError::BufferCreation("Failed to receive buffer map result".into()))?
            .map_err(|_| crate::RendererError::BufferCreation("Buffer mapping failed".into()))?;
        
        let data = buffer_slice.get_mapped_range();
        
        // Remove row padding and convert to RGBA for PNG
        let mut rgba = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            let row_start = (y * bytes_per_row) as usize;
            for x in 0..size {
                let alpha = data[row_start + x as usize];
                // Convert grayscale to RGBA (white text on transparent background)
                rgba.push(255); // R
                rgba.push(255); // G
                rgba.push(255); // B
                rgba.push(alpha); // A
            }
        }
        
        drop(data);
        buffer.unmap();
        
        // Save as PNG
        let file = File::create(output_path)
            .map_err(|e| crate::RendererError::BufferCreation(format!("Failed to create file: {}", e)))?;
        let writer = BufWriter::new(file);
        
        let mut encoder = png::Encoder::new(writer, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        
        let mut png_writer = encoder.write_header()
            .map_err(|e| crate::RendererError::BufferCreation(format!("PNG header error: {}", e)))?;
        
        png_writer.write_image_data(&rgba)
            .map_err(|e| crate::RendererError::BufferCreation(format!("PNG write error: {}", e)))?;
        
        tracing::info!(
            entries = self.entries.len(),
            "Glyph atlas dumped to file"
        );
        
        Ok(())
    }
    
    /// Dump info about specific glyphs for debugging.
    pub fn dump_glyph_info(&self, codepoints: &[char]) -> Vec<String> {
        let mut info = Vec::new();
        
        for &cp in codepoints {
            let matching: Vec<_> = self.entries.iter()
                .filter(|(k, _)| k.codepoint == cp)
                .collect();
            
            if matching.is_empty() {
                info.push(format!("'{}' (U+{:04X}): NOT CACHED", cp, cp as u32));
            } else {
                for (key, entry) in matching {
                    info.push(format!(
                        "'{}' (U+{:04X}): family={}, size={}, tex_coords=[{:.4}, {:.4}, {:.4}, {:.4}], offset=[{:.1}, {:.1}], advance={:.1}",
                        cp, cp as u32,
                        key.font_family,
                        key.font_size as f32 / 10.0,
                        entry.tex_coords[0], entry.tex_coords[1], entry.tex_coords[2], entry.tex_coords[3],
                        entry.offset[0], entry.offset[1],
                        entry.advance
                    ));
                }
            }
        }
        
        info
    }
    
    /// Get statistics about the glyph cache.
    pub fn stats(&self) -> GlyphCacheStats {
        GlyphCacheStats {
            entries: self.entries.len(),
            atlas_size: self.atlas_size,
            next_x: self.next_x,
            next_y: self.next_y,
            row_height: self.row_height,
            estimated_usage_percent: 
                ((self.next_y as f64 * self.atlas_size as f64 + self.next_x as f64) / 
                 (self.atlas_size as f64 * self.atlas_size as f64) * 100.0),
        }
    }
}

/// Statistics about the glyph cache.
#[derive(Debug, Clone)]
pub struct GlyphCacheStats {
    pub entries: usize,
    pub atlas_size: u32,
    pub next_x: u32,
    pub next_y: u32,
    pub row_height: u32,
    pub estimated_usage_percent: f64,
}

/// Estimate glyph size based on character and font size.
fn estimate_glyph_size(ch: char, font_size: f32) -> (u32, u32) {
    let height = font_size.ceil() as u32;
    
    // Estimate width based on character type
    let width_factor = match ch {
        ' ' => 0.3,
        'i' | 'l' | '!' | '|' | '\'' => 0.3,
        'm' | 'w' | 'M' | 'W' => 0.9,
        _ if ch.is_ascii() => 0.6,
        _ => 0.8, // CJK and other wide characters
    };
    
    let width = (font_size * width_factor).ceil() as u32;
    (width.max(1), height.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glyph_key_hash() {
        let key1 = GlyphKey {
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
        };

        let key2 = GlyphKey {
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
        };

        assert_eq!(key1, key2);
    }

    #[test]
    fn test_glyph_key_different() {
        let key1 = GlyphKey {
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
        };

        let key2 = GlyphKey {
            codepoint: 'B',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
        };

        assert_ne!(key1, key2);
    }

    #[test]
    fn test_estimate_glyph_size() {
        let (w, h) = estimate_glyph_size('A', 16.0);
        assert!(w > 0);
        assert!(h > 0);
        
        let (narrow_w, _) = estimate_glyph_size('i', 16.0);
        let (wide_w, _) = estimate_glyph_size('M', 16.0);
        assert!(narrow_w < wide_w);
    }
}
