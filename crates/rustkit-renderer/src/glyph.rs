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
        #[cfg(not(target_os = "macos"))]
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
            // Windows fallback - use simple placeholder
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
        }
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
        };

        let key2 = GlyphKey {
            subpixel_phase: 0,
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
            subpixel_phase: 0,
            codepoint: 'A',
            font_family: "Arial".to_string(),
            font_size: 160,
            font_weight: 400,
            font_style: 0,
        };

        let key2 = GlyphKey {
            subpixel_phase: 0,
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
