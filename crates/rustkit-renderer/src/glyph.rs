//! Glyph cache for text rendering.
//!
//! Caches rasterized glyphs in a GPU texture atlas.

use crate::RendererError;
use hashbrown::HashMap;
#[cfg(windows)]
use rustkit_text::{
    FontCollection as RkFontCollection, FontStretch as RkFontStretch, FontStyle as RkFontStyle,
    FontWeight as RkFontWeight,
};
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
    /// Horizontal subpixel phase, `0..SUBPIXEL_QUANTIZE`.
    ///
    /// WHY THIS EXISTS: every glyph was rasterized ONCE at phase 0 into an
    /// integer-sized atlas bitmap, then drawn at arbitrary FRACTIONAL device
    /// positions and bilinearly resampled. Chrome rasterizes AT the phase.
    /// Measured baselines on fixtures/typography.html land at .081/.280/.120/
    /// .960/.200/.441/.880 -- arbitrary phases on every line -- which is the
    /// mechanism behind the bimodal text diff tail.
    ///
    /// PRODUCTION IS FROZEN AT PHASE 0 IN THIS COMMIT, DELIBERATELY. The
    /// rasterizer still draws a phase-0 bitmap for every phase, so emitting
    /// multi-phase keys now would mint up to SUBPIXEL_QUANTIZE BYTE-IDENTICAL
    /// atlas entries per glyph: more memory, more eviction pressure, and not
    /// one pixel different. The call-site flip belongs in the same commit as
    /// the rasterizer that can honor it.
    pub subpixel_phase: u8,
    /// Which document-registered (`@font-face`) file `font_family` resolves
    /// to, from [`GlyphKey::web_face_for`]; 0 when it is a platform font.
    ///
    /// WHY THIS EXISTS: a family name does not say which face draws it. The
    /// first paint of a page runs before its web fonts arrive, so the
    /// fallback's bitmaps were cached under the web font's NAME and every
    /// later frame reused them: text measured with the web font and drawn
    /// with Helvetica's glyphs. The same collision let a second document
    /// that declares the same family name with another file reuse the first
    /// document's glyphs.
    pub web_face: u64,
}

impl GlyphKey {
    /// The `web_face` of a run: one registry lookup per run, not per glyph.
    pub fn web_face_for(font_family: &str, font_weight: u16, font_style: u8) -> u64 {
        rustkit_text::webfonts::face_id(font_family, font_weight, font_style == 1)
    }
}

/// Key for a glyph of a SHAPED RUN (`rustkit_layout::GlyphRun`): the face
/// layout selected and the glyph id it shaped. No character and no family
/// name: which face draws a character is layout's decision, made once, and
/// a cache keyed by family name would be keyed by a question, not an answer.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct RunGlyphKey {
    /// `rustkit_layout::FaceIdentity::id`.
    pub face: u64,
    pub glyph_id: u16,
    pub font_size: u32, // Fixed-point (size * 10), as `GlyphKey`
    /// As `GlyphKey::subpixel_phase`, and frozen at 0 for the same reason.
    pub subpixel_phase: u8,
}

impl RunGlyphKey {
    /// The size this key's bitmap is rasterized at.
    pub fn raster_size(&self) -> f32 {
        self.font_size as f32 / 10.0
    }
}

/// Rasterize one glyph of a shaped run: glyph `key.glyph_id` of the face
/// layout shaped with, at the key's size. `shaped_size` is the size layout
/// shaped at, which is how the face's font is found. Returns `None` when
/// the face is not held any more (or on a platform with no face table);
/// the caller then paints the command through the family-list path.
///
/// No CSS family list is resolved here.
pub fn rasterize_run_glyph(
    key: &RunGlyphKey,
    shaped_size: f32,
) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
    #[cfg(target_os = "macos")]
    {
        let font = rustkit_text::macos::face_font(key.face, shaped_size)?;
        rustkit_text::macos::GlyphRasterizer::for_face(font, key.raster_size())
            .rasterize_glyph_id(key.glyph_id, 0.0)
    }
    #[cfg(windows)]
    {
        // DirectWrite faces are size-independent: the face recorded for the
        // run is drawn at the key's size.
        let _ = shaped_size;
        let Some(face) = rustkit_text::face_by_id(key.face) else {
            // Loud, not silent: layout recorded this id when it shaped the run, so a
            // miss means the table was bounded out or an id was made up. The caller
            // still paints the command through the family-list path (no crash).
            tracing::error!(face = key.face, glyph = key.glyph_id, "shaped run names a face the rasterizer does not hold");
            return None;
        };
        rasterize_face_glyph(face.raw(), key.glyph_id, key.raster_size(), false, true)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (key, shaped_size);
        None
    }
}

/// Number of horizontal subpixel phases a glyph may be rasterized at.
///
/// 4 (quarter-pixel) is the industry default: it is the point where added
/// positional accuracy stops being visible at normal text sizes while atlas
/// cost still grows linearly. 3 is the LCD-subpixel-triad choice and belongs
/// to a different rendering mode, not to this key.
pub const SUBPIXEL_QUANTIZE: u8 = 4;

/// Quantize a fractional device X into a phase bucket.
///
/// Takes the FRACTIONAL part, so it is correct for any x including negatives:
/// `-0.25` and `0.75` are the same phase, because what a rasterizer needs is
/// the offset within the pixel, not the pixel.
pub fn subpixel_phase_for(x: f32) -> u8 {
    let frac = x - x.floor();
    let phase = (frac * SUBPIXEL_QUANTIZE as f32).floor() as i32;
    phase.clamp(0, SUBPIXEL_QUANTIZE as i32 - 1) as u8
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
    /// Glyphs of shaped runs, in the same atlas as `entries`.
    run_entries: HashMap<RunGlyphKey, GlyphEntry>,
    next_x: u32,
    next_y: u32,
    row_height: u32,
    // Parallel RGBA atlas for COLOR glyphs (emoji). The grayscale atlas above
    // is R8 and the renderer tints it; color-bitmap emoji need real RGBA, drawn
    // with the passthrough (blit) pipeline. Kept separate so the grayscale text
    // hot path is untouched — this atlas stays empty on pages without emoji.
    color_atlas: wgpu::Texture,
    _color_atlas_view: wgpu::TextureView,
    color_bind_group: wgpu::BindGroup,
    color_entries: HashMap<GlyphKey, GlyphEntry>,
    color_next_x: u32,
    color_next_y: u32,
    color_row_height: u32,
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

        // Parallel RGBA color-glyph atlas (emoji). Same bind-group layout —
        // Rgba8Unorm is float-sampleable like R8Unorm.
        let color_atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Color Glyph Atlas"),
            size: wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let color_empty = vec![0u8; (atlas_size * atlas_size * 4) as usize];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &color_atlas,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &color_empty,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(atlas_size * 4),
                rows_per_image: Some(atlas_size),
            },
            wgpu::Extent3d {
                width: atlas_size,
                height: atlas_size,
                depth_or_array_layers: 1,
            },
        );
        let color_atlas_view = color_atlas.create_view(&wgpu::TextureViewDescriptor::default());
        let color_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&color_atlas_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
            label: Some("color_glyph_atlas_bind_group"),
        });

        Ok(Self {
            atlas,
            _atlas_view: atlas_view,
            bind_group,
            atlas_size,
            entries: HashMap::new(),
            run_entries: HashMap::new(),
            next_x: 1, // Start at 1 to avoid edge artifacts
            next_y: 1,
            row_height: 0,
            color_atlas,
            _color_atlas_view: color_atlas_view,
            color_bind_group,
            color_entries: HashMap::new(),
            color_next_x: 1,
            color_next_y: 1,
            color_row_height: 0,
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

    /// Get the bind group for the RGBA color-glyph atlas.
    pub fn color_bind_group(&self) -> &wgpu::BindGroup {
        &self.color_bind_group
    }

    /// Get or rasterize a COLOR glyph (emoji) into the RGBA atlas. Returns the
    /// atlas entry (tex_coords into the color atlas), or None if the platform
    /// or font can't produce a color glyph for this codepoint.
    #[allow(unused_variables)]
    pub fn get_or_rasterize_color(
        &mut self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &GlyphKey,
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.color_entries.get(key) {
            return Some(entry.clone());
        }

        #[cfg(target_os = "macos")]
        let raster = {
            let italic = key.font_style == 1;
            let family = if key.font_family.is_empty() {
                "Helvetica"
            } else {
                key.font_family.as_str()
            };
            let rasterizer = rustkit_text::macos::GlyphRasterizer::with_style(
                family,
                key.font_size as f32 / 10.0,
                key.font_weight,
                italic,
            );
            rasterizer.rasterize_char_color(key.codepoint)
        };
        #[cfg(windows)]
        let raster = rasterize_glyph_directwrite_color(key, key.font_size as f32 / 10.0);
        #[cfg(not(any(target_os = "macos", windows)))]
        let raster: Option<(Vec<u8>, u32, u32, f32, f32, f32)> = None;

        let (rgba, gw, gh, advance, bearing_x, bearing_y) = raster?;
        let gw = gw.max(1).min(256);
        let gh = gh.max(1).min(256);

        let (ax, ay) = self.allocate_color_space(gw + 2, gh + 2)?;

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color_atlas,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: ax + 1,
                    y: ay + 1,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(gw * 4),
                rows_per_image: Some(gh),
            },
            wgpu::Extent3d {
                width: gw,
                height: gh,
                depth_or_array_layers: 1,
            },
        );

        let u0 = (ax + 1) as f32 / self.atlas_size as f32;
        let v0 = (ay + 1) as f32 / self.atlas_size as f32;
        let u1 = (ax + 1 + gw) as f32 / self.atlas_size as f32;
        let v1 = (ay + 1 + gh) as f32 / self.atlas_size as f32;

        let entry = GlyphEntry {
            tex_coords: [u0, v0, u1, v1],
            offset: [bearing_x, -bearing_y],
            advance,
        };
        self.color_entries.insert(key.clone(), entry.clone());
        Some(entry)
    }

    /// Allocate space in the COLOR atlas (separate cursor from the grayscale one).
    fn allocate_color_space(&mut self, width: u32, height: u32) -> Option<(u32, u32)> {
        if self.color_next_x + width > self.atlas_size {
            self.color_next_x = 1;
            self.color_next_y += self.color_row_height + 1;
            self.color_row_height = 0;
        }
        if self.color_next_y + height > self.atlas_size {
            tracing::warn!("Color glyph atlas full, clearing cache");
            self.color_entries.clear();
            self.color_next_x = 1;
            self.color_next_y = 1;
            self.color_row_height = 0;
        }
        let x = self.color_next_x;
        let y = self.color_next_y;
        self.color_next_x += width + 1;
        self.color_row_height = self.color_row_height.max(height);
        Some((x, y))
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

        // Rasterize using fallback (simple rectangle placeholder)
        self.rasterize_glyph_fallback(queue, key)
    }

    /// Rasterize a glyph using platform-specific text rendering.
    fn rasterize_glyph_fallback(
        &mut self,
        queue: &wgpu::Queue,
        key: &GlyphKey,
    ) -> Option<GlyphEntry> {
        let font_size = key.font_size as f32 / 10.0;

        // Use platform-specific glyph rasterization
        #[cfg(target_os = "macos")]
        let raster_result = {
            let italic = key.font_style == 1;
            // Map font families for parity testing
            let family = if key.font_family.is_empty() {
                "Helvetica"
            } else {
                // Map ParityTest to Noto Sans for consistent cross-platform rendering
                match key.font_family.as_str() {
                    "ParityTest" | "'ParityTest'" => "Noto Sans",
                    "Noto Sans" | "'Noto Sans'" => "Noto Sans",
                    other => other,
                }
            };
            let rasterizer = rustkit_text::macos::GlyphRasterizer::with_style(
                family,
                font_size,
                key.font_weight,
                italic,
            );
            rasterizer.rasterize_char(key.codepoint, 0.0)
        };

        #[cfg(windows)]
        let raster_result = {
            // DirectWrite rasterization (ported from hiwave-windows glyph.rs,
            // including the July-2026 ClearType fallback: NATURAL rendering
            // mode reports EMPTY aliased bounds for most glyphs, which used to
            // turn every glyph into a tofu box). The bordered-box placeholder
            // below is the last resort when DirectWrite cannot produce a
            // bitmap for this glyph.
            rasterize_glyph_directwrite(key, font_size).or_else(|| {
                let (glyph_width, glyph_height) = estimate_glyph_size(key.codepoint, font_size);
                let glyph_width = glyph_width.max(1).min(256);
                let glyph_height = glyph_height.max(1).min(256);

                let mut bitmap = vec![0u8; (glyph_width * glyph_height) as usize];
                if key.codepoint.is_ascii_graphic() || key.codepoint.is_alphabetic() {
                    for y in 0..glyph_height {
                        for x in 0..glyph_width {
                            let idx = (y * glyph_width + x) as usize;
                            let border =
                                x == 0 || x == glyph_width - 1 || y == 0 || y == glyph_height - 1;
                            bitmap[idx] = if border { 255 } else { 200 };
                        }
                    }
                }
                Some((
                    bitmap,
                    glyph_width,
                    glyph_height,
                    glyph_width as f32,
                    0.0f32,
                    font_size * 0.8,
                ))
            })
        };

        #[cfg(not(any(target_os = "macos", windows)))]
        let raster_result: Option<(Vec<u8>, u32, u32, f32, f32, f32)> = {
            // Fallback for other platforms
            let (glyph_width, glyph_height) = estimate_glyph_size(key.codepoint, font_size);
            let glyph_width = glyph_width.max(1).min(256);
            let glyph_height = glyph_height.max(1).min(256);

            let mut bitmap = vec![0u8; (glyph_width * glyph_height) as usize];
            if key.codepoint.is_ascii_graphic() || key.codepoint.is_alphabetic() {
                for y in 0..glyph_height {
                    for x in 0..glyph_width {
                        let idx = (y * glyph_width + x) as usize;
                        let border =
                            x == 0 || x == glyph_width - 1 || y == 0 || y == glyph_height - 1;
                        bitmap[idx] = if border { 255 } else { 200 };
                    }
                }
            }
            Some((
                bitmap,
                glyph_width,
                glyph_height,
                glyph_width as f32,
                0.0f32,
                font_size * 0.8,
            ))
        };

        let (bitmap, glyph_width, glyph_height, advance, bearing_x, bearing_y) = raster_result?;

        let glyph_width = glyph_width.max(1).min(256);
        let glyph_height = glyph_height.max(1).min(256);

        // PAINT-0 (P0c atlas A/B): FNV-1a over the rasterized bitmap. If the
        // metrics-normal build produces identical hashes to flat-1.2, the
        // bitmaps are byte-identical and any pixel delta is pure seating.
        if crate::paint0_probe() {
            let mut hash: u64 = 0xcbf29ce484222325;
            for &b in &bitmap {
                hash ^= b as u64;
                hash = hash.wrapping_mul(0x100000001b3);
            }
            eprintln!(
                "PAINT0 atlas cp={:?} fs={} w={} h={} bearing_x={} bearing_y={} advance={} hash={:016x}",
                key.codepoint, key.font_size, glyph_width, glyph_height, bearing_x, bearing_y, advance, hash
            );
        }

        // Allocate space in the atlas
        let (atlas_x, atlas_y) = self.allocate_space(glyph_width + 2, glyph_height + 2)?;

        // Upload to atlas
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

        // ADVANCE CONTRACT (2026-07-11): entries are BASELINE-relative.
        // offset[1] = -bearing_y (glyph top relative to the baseline); the
        // draw path decides where the baseline is — from layout's ascent
        // when the display command carries one, else one per-run fallback.
        // The old code built a THIRD TextShaper here PER GLYPH just to get
        // an ascent, and its metrics disagreed with layout's by 2-3px —
        // every glyph on every page painted low.
        let y_offset = -bearing_y;

        // x_offset: horizontal bearing adjustment
        let x_offset = bearing_x;

        let entry = GlyphEntry {
            tex_coords: [u0, v0, u1, v1],
            offset: [x_offset, y_offset],
            advance,
        };

        self.entries.insert(key.clone(), entry.clone());
        Some(entry)
    }

    /// Get or rasterize a glyph of a shaped run (see [`RunGlyphKey`]).
    /// `shaped_size` is the run's size as layout shaped it. `None` when the
    /// run's face is not available to the rasterizer; nothing is cached
    /// then, so the caller can paint the command the old way.
    pub fn get_or_rasterize_run_glyph(
        &mut self,
        queue: &wgpu::Queue,
        key: &RunGlyphKey,
        shaped_size: f32,
    ) -> Option<GlyphEntry> {
        if let Some(entry) = self.run_entries.get(key) {
            return Some(entry.clone());
        }

        let (bitmap, glyph_width, glyph_height, advance, bearing_x, bearing_y) =
            rasterize_run_glyph(key, shaped_size)?;
        let glyph_width = glyph_width.max(1).min(256);
        let glyph_height = glyph_height.max(1).min(256);

        let (atlas_x, atlas_y) = self.allocate_space(glyph_width + 2, glyph_height + 2)?;
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

        // Baseline-relative, as the character entries are.
        let entry = GlyphEntry {
            tex_coords: [u0, v0, u1, v1],
            offset: [bearing_x, -bearing_y],
            advance,
        };
        self.run_entries.insert(key.clone(), entry.clone());
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
            self.run_entries.clear();
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
        self.run_entries.clear();
        self.next_x = 1;
        self.next_y = 1;
        self.row_height = 0;
        self.color_entries.clear();
        self.color_next_x = 1;
        self.color_next_y = 1;
        self.color_row_height = 0;
    }
}

/// Estimate glyph size based on character and font size.
#[allow(dead_code)]
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

/// Rasterize one glyph with DirectWrite into an 8-bit coverage bitmap.
///
/// Returns `(bitmap, width, height, advance, bearing_x, bearing_y)` in the
/// same BASELINE-relative contract the macOS path uses: `bearing_y` is the
/// distance from the baseline UP to the bitmap's top row, so the shared
/// upload code below sets `offset[1] = -bearing_y`. DirectWrite reports the
/// texture bounds relative to a baseline origin of (0, 0) with y down, so
/// `bearing_y = -bounds.top` and `bearing_x = bounds.left`.
///
/// The rendering-mode dance is the load-bearing part (hiwave-windows #7,
/// 2026-07-10): `CreateGlyphRunAnalysis` in NATURAL mode returns SUCCESS with
/// an EMPTY rect from `GetAlphaTextureBounds(ALIASED_1x1)` for most glyphs;
/// only the CLEARTYPE_3x1 texture is populated, and the alpha texture must
/// be read in the SAME mode the bounds came from (an aliased read of a
/// ClearType analysis returns success-but-zeros, i.e. invisible text).
///
/// Subpixel phase is not applied on this path: production is frozen at
/// phase 0 (see `GlyphKey::subpixel_phase`), and DirectWrite already
/// positions at the integer baseline origin we pass.
#[cfg(windows)]
fn rasterize_glyph_directwrite(
    key: &GlyphKey,
    font_size: f32,
) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    unsafe {
        // COM must be initialised on this thread; a repeat call is harmless.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let factory: IDWriteFactory =
            match DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("Failed to create DWrite factory: {:?}", e);
                    return None;
                }
            };

        let mut collection: Option<IDWriteFontCollection> = None;
        if factory.GetSystemFontCollection(&mut collection, false).is_err() {
            return None;
        }
        let collection = collection?;

        // Family lookup with the same fallback ladder the Windows tree used.
        let family_wide: Vec<u16> =
            key.font_family.encode_utf16().chain(std::iter::once(0)).collect();
        let mut index: u32 = 0;
        let mut exists = windows::core::BOOL(0);
        let direct = !key.font_family.is_empty()
            && collection
                .FindFamilyName(PCWSTR(family_wide.as_ptr()), &mut index, &mut exists)
                .is_ok()
            && exists.as_bool();
        if !direct {
            let mut found = false;
            for fallback in ["Segoe UI", "Arial", "Tahoma"] {
                let fb_wide: Vec<u16> =
                    fallback.encode_utf16().chain(std::iter::once(0)).collect();
                if collection
                    .FindFamilyName(PCWSTR(fb_wide.as_ptr()), &mut index, &mut exists)
                    .is_ok()
                    && exists.as_bool()
                {
                    found = true;
                    break;
                }
            }
            if !found {
                return None;
            }
        }

        let family = collection.GetFontFamily(index).ok()?;
        let dw_weight = DWRITE_FONT_WEIGHT(key.font_weight as i32);
        let dw_stretch = DWRITE_FONT_STRETCH(5); // Normal
        let dw_style = if key.font_style == 1 {
            DWRITE_FONT_STYLE_ITALIC
        } else {
            DWRITE_FONT_STYLE_NORMAL
        };
        let font = family
            .GetFirstMatchingFont(dw_weight, dw_stretch, dw_style)
            .ok()?;
        let face = font.CreateFontFace().ok()?;

        let codepoint = key.codepoint as u32;
        let mut glyph_indices = [0u16; 1];
        if face
            .GetGlyphIndices(&codepoint as *const u32, 1, glyph_indices.as_mut_ptr())
            .is_err()
        {
            return None;
        }
        let glyph_index = glyph_indices[0];
        if glyph_index == 0 {
            return None;
        }

        // Whitespace has an advance but no ink.
        rasterize_face_glyph(&face, glyph_index, font_size, key.codepoint.is_whitespace(), false)
    }
}

/// The character path's bitmap for `c`, for tests that compare it with the
/// run path's.
#[cfg(all(test, windows))]
pub(crate) fn rasterize_char_for_test(
    c: char,
    family: &str,
    size: f32,
    weight: u16,
) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
    let key = GlyphKey {
        codepoint: c,
        font_family: family.to_string(),
        font_size: (size * 10.0) as u32,
        font_weight: weight,
        font_style: 0,
        subpixel_phase: 0,
        web_face: 0,
    };
    rasterize_glyph_directwrite(&key, size)
}

/// Rasterize glyph `glyph_index` of `face` at `font_size`, into the contract
/// `rasterize_glyph_directwrite` documents.
///
/// `blank` says the glyph is known to have no ink (whitespace by character):
/// it returns a 1x1 empty bitmap with the advance. `blank_if_no_ink` makes a
/// glyph that turns out to have no ink the same (a shaped run names glyphs,
/// not characters, so a space reaches here as an id): the character path
/// leaves that case `None`, so its callers skip the glyph.
#[cfg(windows)]
fn rasterize_face_glyph(
    face: &IDWriteFontFace,
    glyph_index: u16,
    font_size: f32,
    blank: bool,
    blank_if_no_ink: bool,
) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
    unsafe {
        let factory: IDWriteFactory =
            match DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("Failed to create DWrite factory: {:?}", e);
                    return None;
                }
            };

        let mut font_metrics = DWRITE_FONT_METRICS::default();
        face.GetMetrics(&mut font_metrics);
        let design_units_per_em = font_metrics.designUnitsPerEm as f32;
        if design_units_per_em <= 0.0 {
            return None;
        }

        let mut glyph_metrics = [DWRITE_GLYPH_METRICS::default()];
        if face
            .GetDesignGlyphMetrics(&glyph_index, 1, glyph_metrics.as_mut_ptr(), false)
            .is_err()
        {
            return None;
        }
        let advance_width = glyph_metrics[0].advanceWidth as f32 * font_size / design_units_per_em;

        // A 1x1 empty bitmap keeps the shared upload path happy and paints
        // nothing.
        if blank {
            return Some((vec![0u8; 1], 1, 1, advance_width, 0.0, 0.0));
        }

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
        // Every early return below must release the ManuallyDrop face.
        let release = |run: DWRITE_GLYPH_RUN| {
            std::mem::ManuallyDrop::into_inner(run.fontFace);
        };

        let analysis: IDWriteGlyphRunAnalysis = match factory.CreateGlyphRunAnalysis(
            &glyph_run,
            1.0, // pixels per DIP
            None,
            DWRITE_RENDERING_MODE_NATURAL,
            DWRITE_MEASURING_MODE_NATURAL,
            0.0, // baseline origin x
            0.0, // baseline origin y
        ) {
            Ok(a) => a,
            Err(_) => {
                release(glyph_run);
                return None;
            }
        };

        let non_empty =
            |b: &windows::Win32::Foundation::RECT| b.right > b.left && b.bottom > b.top;
        let (bounds, use_cleartype) =
            match analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_ALIASED_1x1) {
                Ok(b) if non_empty(&b) => (b, false),
                _ => match analysis.GetAlphaTextureBounds(DWRITE_TEXTURE_CLEARTYPE_3x1) {
                    Ok(b) if non_empty(&b) => (b, true),
                    _ => {
                        release(glyph_run);
                        return if blank_if_no_ink {
                            Some((vec![0u8; 1], 1, 1, advance_width, 0.0, 0.0))
                        } else {
                            None
                        };
                    }
                },
            };

        let tex_width = (bounds.right - bounds.left) as u32;
        let tex_height = (bounds.bottom - bounds.top) as u32;
        if tex_width == 0 || tex_height == 0 || tex_width > 256 || tex_height > 256 {
            release(glyph_run);
            return None;
        }

        let mut alpha_values = vec![0u8; (tex_width * tex_height) as usize];
        let tex_ok = if use_cleartype {
            let mut ct_values = vec![0u8; (tex_width * tex_height * 3) as usize];
            let ok = analysis
                .CreateAlphaTexture(DWRITE_TEXTURE_CLEARTYPE_3x1, &bounds, ct_values.as_mut_slice())
                .is_ok();
            if ok {
                for i in 0..(tex_width * tex_height) as usize {
                    let r = ct_values[i * 3] as u32;
                    let g = ct_values[i * 3 + 1] as u32;
                    let b = ct_values[i * 3 + 2] as u32;
                    alpha_values[i] = ((r + g + b) / 3) as u8;
                }
            }
            ok
        } else {
            analysis
                .CreateAlphaTexture(DWRITE_TEXTURE_ALIASED_1x1, &bounds, alpha_values.as_mut_slice())
                .is_ok()
        };
        release(glyph_run);
        if !tex_ok {
            return None;
        }

        let bearing_x = bounds.left as f32;
        let bearing_y = -(bounds.top as f32);
        Some((alpha_values, tex_width, tex_height, advance_width, bearing_x, bearing_y))
    }
}

/// Rasterize a colour glyph (emoji) to premultiplied RGBA, the Windows
/// counterpart of `rasterize_char_color` on macOS.
///
/// Chrome on Windows draws emoji from **Segoe UI Emoji**, so that family is
/// used whatever the run's `font-family` says: the point of the colour path
/// is to paint the artwork the baseline shows. The drawing goes through
/// Direct2D's `DrawTextLayout` with `ENABLE_COLOR_FONT`, which renders every
/// colour glyph format the OS knows (COLRv0 layers, and on Windows 11 the
/// COLRv1 paint trees Segoe UI Emoji now ships), into a WIC bitmap that is
/// then cropped to its ink. `IDWriteFactory2::TranslateColorGlyphRun` was
/// tried first: it only yields the flat COLRv0 layers, which differ from
/// Chrome's COLRv1 rendering more than the missing glyph did.
///
/// Returns `(rgba, width, height, advance, bearing_x, bearing_y)` with the
/// same bitmap-edge contract as the grayscale rasterizer: `bearing_x` and
/// `bearing_y` place the bitmap's top-left at `(pen + bearing_x,
/// baseline - bearing_y)`. `None` means "nothing painted" and the caller
/// falls back to the grayscale path.
#[cfg(windows)]
fn rasterize_glyph_directwrite_color(
    key: &GlyphKey,
    font_size: f32,
) -> Option<(Vec<u8>, u32, u32, f32, f32, f32)> {
    use windows::core::w;
    use windows::Win32::Graphics::Direct2D::Common::*;
    use windows::Win32::Graphics::Direct2D::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };

    if font_size <= 0.0 || !font_size.is_finite() {
        return None;
    }

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let dwrite: IDWriteFactory =
            DWriteCreateFactory::<IDWriteFactory>(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let format = dwrite
            .CreateTextFormat(
                w!("Segoe UI Emoji"),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                font_size,
                w!("en-us"),
            )
            .ok()?;
        // No wrapping: the layout is one glyph, and the box below is sized
        // to hold any emoji at this size with room for overhang.
        let _ = format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
        let mut text: Vec<u16> = [0u16; 2].to_vec();
        let n = key.codepoint.encode_utf16(&mut text).len();
        text.truncate(n);
        let pad = (font_size * 0.5).ceil();
        let box_w = (font_size * 2.0 + pad * 2.0).ceil();
        let box_h = (font_size * 2.0 + pad * 2.0).ceil();
        let layout = dwrite.CreateTextLayout(&text, &format, box_w, box_h).ok()?;

        // Where DirectWrite puts the baseline inside the layout, and the
        // glyph's advance.
        let mut line_metrics = [DWRITE_LINE_METRICS::default()];
        let mut line_count = 0u32;
        let _ = layout.GetLineMetrics(Some(&mut line_metrics), &mut line_count);
        if line_count == 0 {
            return None;
        }
        let baseline_in_layout = line_metrics[0].baseline;
        let mut text_metrics = DWRITE_TEXT_METRICS::default();
        layout.GetMetrics(&mut text_metrics).ok()?;
        let advance = text_metrics.widthIncludingTrailingWhitespace;

        // A transparent premultiplied BGRA bitmap for Direct2D to draw into.
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let (bw, bh) = (box_w as u32, box_h as u32);
        let bitmap = wic
            .CreateBitmap(bw, bh, &GUID_WICPixelFormat32bppPBGRA, WICBitmapCacheOnDemand)
            .ok()?;
        let d2d: ID2D1Factory =
            D2D1CreateFactory::<ID2D1Factory>(D2D1_FACTORY_TYPE_SINGLE_THREADED, None).ok()?;
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT {
                format: DXGI_FORMAT_B8G8R8A8_UNORM,
                alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
            },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        let rt = d2d.CreateWicBitmapRenderTarget(&bitmap, &props).ok()?;
        // Grayscale antialiasing: there is no opaque background to ClearType
        // against, and the atlas is sampled with plain alpha blending.
        rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        let black = D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 };
        let brush = rt.CreateSolidColorBrush(&black, None).ok()?;
        let origin = windows_numerics::Vector2 { X: pad, Y: pad };
        rt.BeginDraw();
        rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        // The brush only colours layers that ask for the text colour; the
        // rest is the font's own palette.
        rt.DrawTextLayout(origin, &layout, &brush, D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT);
        rt.EndDraw(None, None).ok()?;

        let stride = bw * 4;
        let mut pixels = vec![0u8; (stride * bh) as usize];
        bitmap.CopyPixels(std::ptr::null(), stride, &mut pixels).ok()?;

        // Crop to the ink. Everything is relative to `origin` (the pen) and
        // the baseline row inside the box.
        let (mut x0, mut y0, mut x1, mut y1) = (bw, bh, 0u32, 0u32);
        for y in 0..bh {
            for x in 0..bw {
                if pixels[((y * bw + x) * 4 + 3) as usize] != 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let width = x1 - x0;
        let height = y1 - y0;
        if width > 256 || height > 256 {
            return None;
        }
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in y0..y1 {
            for x in x0..x1 {
                let o = ((y * bw + x) * 4) as usize;
                // PBGRA to premultiplied RGBA.
                rgba.extend_from_slice(&[pixels[o + 2], pixels[o + 1], pixels[o], pixels[o + 3]]);
            }
        }
        let bearing_x = x0 as f32 - origin.X;
        let bearing_y = (origin.Y + baseline_in_layout) - y0 as f32;
        Some((rgba, width, height, advance, bearing_x, bearing_y))
    }
}

#[cfg(test)]
mod tests {

    fn key_at(phase: u8) -> GlyphKey {
        GlyphKey {
            codepoint: 'a',
            font_family: "Helvetica".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
            subpixel_phase: phase,
            web_face: 0,
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn a_web_font_that_arrives_later_does_not_reuse_the_fallbacks_glyphs() {
        use rustkit_text::webfonts::{self, WebFontFace};
        use std::sync::Arc;
        let face = |bytes: &[u8]| WebFontFace {
            family: "GlyphKeyLateFont".to_string(),
            weight: 400,
            italic: false,
            data: Arc::new(bytes.to_vec()),
        };
        let key = || GlyphKey {
            web_face: GlyphKey::web_face_for("GlyphKeyLateFont, sans-serif", 400, 0),
            font_family: "GlyphKeyLateFont, sans-serif".to_string(),
            ..key_at(0)
        };

        // First paint: the document's font has not arrived, the fallback draws.
        webfonts::clear();
        let before = key();
        assert_eq!(before.web_face, 0);

        // The font arrives. Same family string, another cache entry.
        let ttf = include_bytes!("../../rustkit-text/tests/fixtures/Ahem.ttf");
        webfonts::install("glyph-key-a", &[face(ttf)]);
        let loaded = key();
        assert_ne!(
            loaded, before,
            "the fallback's bitmap was reused for the web font"
        );
        assert_eq!(
            key(),
            loaded,
            "one face is one entry however often it is drawn"
        );

        // Another document, the same family name, another file.
        let woff2 = include_bytes!("../../rustkit-text/tests/fixtures/Ahem.woff2");
        webfonts::install("glyph-key-b", &[face(woff2)]);
        assert_ne!(
            key(),
            loaded,
            "another document's file drew this document's text"
        );

        // Back on the first document its glyphs are still cached.
        webfonts::install("glyph-key-c", &[face(ttf)]);
        assert_eq!(key(), loaded);
        webfonts::clear();
    }

    #[test]
    fn one_glyph_occupies_at_most_quantize_cache_slots() {
        // ATLAS GROWTH BOUND (Argos's soft note on #131). The phase field
        // multiplies cache entries per glyph, and this cache has NO eviction
        // -- `clear()` is the only reset -- so the growth FACTOR is the whole
        // safety story. It must be exactly SUBPIXEL_QUANTIZE, not "however
        // many distinct fractions a page happens to produce".
        use std::collections::HashSet;
        let mut keys = HashSet::new();
        // Sweep far more x positions than there are phases; the bucket count,
        // not the position count, must bound the entries.
        for i in 0..500 {
            let x = i as f32 * 0.013;
            keys.insert(key_at(subpixel_phase_for(x)));
        }
        assert_eq!(
            keys.len(),
            SUBPIXEL_QUANTIZE as usize,
            "500 distinct x positions must collapse to exactly {} cache slots",
            SUBPIXEL_QUANTIZE
        );
    }

    #[test]
    fn the_growth_bound_is_the_only_thing_this_unit_guarantees() {
        // Deliberate documentation-as-test. Paying 4x atlas for a glyph is
        // only worth it if the four phases produce four DIFFERENT bitmaps --
        // and Atlas measured that CoreGraphics grid-fits glyph origins and
        // rounds the offset away by default: at 36px, phases .25 and .50 gave
        // a 0.00px and a 1.00px shift, i.e. TWO bitmaps in FOUR slots. That is
        // fixed in the rasterizer half (#132, subpixel positioning on,
        // subpixel quantization off), NOT here.
        //
        // This test exists so a reader of THIS file learns that the key alone
        // does not buy distinct rendering, and does not mistake a green suite
        // here for a working subpixel pipeline.
        assert_eq!(SUBPIXEL_QUANTIZE, 4);
    }

    #[test]
    fn glyphs_at_different_phases_are_different_cache_entries() {
        // THE POINT OF THE WHOLE UNIT. Before the phase field, a glyph at
        // x=10.0 and the same glyph at x=10.5 collided on one key, so both got
        // the phase-0 bitmap and the .5 one was resampled into blur.
        assert_ne!(key_at(0), key_at(2));
    }

    #[test]
    fn the_same_phase_is_the_same_entry() {
        // The other direction: phases must still SHARE, or the cache degrades
        // into one entry per draw and the atlas grows without bound.
        assert_eq!(key_at(2), key_at(2));
    }

    #[test]
    fn phase_quantization_buckets_the_fraction() {
        assert_eq!(subpixel_phase_for(10.0), 0);
        assert_eq!(subpixel_phase_for(10.24), 0);
        assert_eq!(subpixel_phase_for(10.25), 1);
        assert_eq!(subpixel_phase_for(10.5), 2);
        assert_eq!(subpixel_phase_for(10.75), 3);
        assert_eq!(subpixel_phase_for(10.999), 3, "never reaches QUANTIZE");
    }

    #[test]
    fn a_negative_x_phases_by_its_fraction_not_its_sign() {
        // Text can be laid out at a negative device X (scrolled, or a run that
        // starts left of the viewport). Using the raw value rather than the
        // fractional part would produce a negative bucket and panic on cast.
        assert_eq!(subpixel_phase_for(-0.25), 3, "-0.25 sits at .75 of a pixel");
        assert_eq!(subpixel_phase_for(-1.0), 0);
    }

    #[test]
    fn every_phase_is_in_range() {
        for i in 0..400 {
            let x = i as f32 * 0.017 - 3.0;
            let p = subpixel_phase_for(x);
            assert!(p < SUBPIXEL_QUANTIZE, "phase {p} out of range for x={x}");
        }
    }
    use super::*;

    #[test]
    fn test_glyph_key_hash() {
        let key1 = GlyphKey {
            subpixel_phase: 0,
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
            web_face: 0,
        };

        let key2 = GlyphKey {
            subpixel_phase: 0,
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
            web_face: 0,
        };

        assert_eq!(key1, key2);
    }

    #[test]
    fn test_glyph_key_different() {
        let key1 = GlyphKey {
            subpixel_phase: 0,
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
            web_face: 0,
        };

        let key2 = GlyphKey {
            subpixel_phase: 0,
            codepoint: 'B',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
            web_face: 0,
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
