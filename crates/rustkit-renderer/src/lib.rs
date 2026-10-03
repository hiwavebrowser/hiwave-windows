//! # RustKit Renderer
//!
//! GPU display list renderer for the RustKit browser engine.
//!
//! This crate takes a `DisplayList` from `rustkit-layout` and executes it
//! via wgpu to produce actual rendered output.
//!
//! ## Architecture
//!
//! ```text
//! DisplayList
//!     │
//!     ▼
//! ┌─────────────────────────────────────┐
//! │           Renderer                  │
//! │  ┌─────────────────────────────┐    │
//! │  │   Command Processing        │    │
//! │  │   - Solid colors            │    │
//! │  │   - Borders                 │    │
//! │  │   - Text (via GlyphCache)   │    │
//! │  │   - Images (via TextureCache)│   │
//! │  └─────────────────────────────┘    │
//! │              │                      │
//! │              ▼                      │
//! │  ┌─────────────────────────────┐    │
//! │  │   Vertex Batching           │    │
//! │  │   - ColorVertex             │    │
//! │  │   - TextureVertex           │    │
//! │  └─────────────────────────────┘    │
//! │              │                      │
//! │              ▼                      │
//! │  ┌─────────────────────────────┐    │
//! │  │   Render Pipelines (wgpu)   │    │
//! │  │   - Color pipeline          │    │
//! │  │   - Texture pipeline        │    │
//! │  └─────────────────────────────┘    │
//! └─────────────────────────────────────┘
//!                 │
//!                 ▼
//!            GPU Output
//! ```

use bytemuck::{Pod, Zeroable};
use hashbrown::HashMap;
use rustkit_css::Color;
use rustkit_layout::{BackgroundRepeat, BackgroundSize, DisplayCommand, Rect};
use std::sync::Arc;
use thiserror::Error;
use wgpu::util::DeviceExt;

pub mod dither;
mod glyph;
mod pipeline;
pub mod screenshot;
#[cfg(windows)]
pub use screenshot::CaptureMetadata;
mod shaders;

pub use glyph::*;
pub use pipeline::*;
pub use screenshot::*;

// ==================== Errors ====================

/// Errors that can occur during rendering.
#[derive(Error, Debug)]
pub enum RendererError {
    #[error("Failed to create render pipeline: {0}")]
    PipelineCreation(String),

    #[error("Failed to create buffer: {0}")]
    BufferCreation(String),

    #[error("Buffer size {0} bytes exceeds maximum allowed size of {1} bytes")]
    BufferTooLarge(u64, u64),

    #[error("Texture upload failed: {0}")]
    TextureUpload(String),

    #[error("Glyph rasterization failed: {0}")]
    GlyphRasterization(String),

    #[error("Surface error: {0}")]
    Surface(#[from] wgpu::SurfaceError),
}

// ==================== Constants ====================

/// Maximum GPU buffer size (256 MB) - prevents OOM on pathological inputs
const MAX_BUFFER_SIZE: u64 = 256 * 1024 * 1024;

// ==================== Vertex Types ====================

/// Vertex for solid color rendering.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ColorVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
}

impl ColorVertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ColorVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

/// Vertex for textured rendering (images, glyphs).
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct TextureVertex {
    pub position: [f32; 2],
    pub tex_coords: [f32; 2],
    pub color: [f32; 4],
}

impl TextureVertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TextureVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                offset: 0,
                shader_location: 0,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 8,
                shader_location: 1,
                format: wgpu::VertexFormat::Float32x2,
            },
            wgpu::VertexAttribute {
                offset: 16,
                shader_location: 2,
                format: wgpu::VertexFormat::Float32x4,
            },
        ],
    };
}

/// Uniform buffer for viewport transformation.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Uniforms {
    pub viewport_size: [f32; 2],
    pub _padding: [f32; 2],
}

// ==================== Texture Cache ====================

/// Cached texture entry.
pub struct CachedTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub bind_group: wgpu::BindGroup,
    pub width: u32,
    pub height: u32,
}

/// Shrink an RGBA image so neither side exceeds `limit`, keeping its aspect.
/// Box filter: each output pixel is the mean of the source pixels it covers.
/// Returns `(width, height, pixels)`.
pub(crate) fn downscale_rgba_to_fit(width: u32, height: u32, data: &[u8], limit: u32) -> (u32, u32, Vec<u8>) {
    let limit = limit.max(1);
    let scale = (limit as f64 / width as f64).min(limit as f64 / height as f64).min(1.0);
    let out_w = ((width as f64 * scale).floor() as u32).clamp(1, limit);
    let out_h = ((height as f64 * scale).floor() as u32).clamp(1, limit);
    let mut out = Vec::with_capacity((out_w * out_h * 4) as usize);
    for oy in 0..out_h {
        let y0 = (oy as u64 * height as u64 / out_h as u64) as u32;
        let y1 = (((oy + 1) as u64 * height as u64 / out_h as u64) as u32).max(y0 + 1).min(height);
        for ox in 0..out_w {
            let x0 = (ox as u64 * width as u64 / out_w as u64) as u32;
            let x1 = (((ox + 1) as u64 * width as u64 / out_w as u64) as u32).max(x0 + 1).min(width);
            let mut acc = [0u64; 4];
            for y in y0..y1 {
                let row = (y as usize * width as usize + x0 as usize) * 4;
                for px in data[row..row + (x1 - x0) as usize * 4].chunks_exact(4) {
                    for c in 0..4 {
                        acc[c] += px[c] as u64;
                    }
                }
            }
            let n = ((y1 - y0) as u64 * (x1 - x0) as u64).max(1);
            out.extend(acc.iter().map(|&v| ((v + n / 2) / n) as u8));
        }
    }
    (out_w, out_h, out)
}

/// Texture cache for images.
pub struct TextureCache {
    textures: HashMap<String, CachedTexture>,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl TextureCache {
    /// Create a new texture cache.
    pub fn new(device: &wgpu::Device, bind_group_layout: wgpu::BindGroupLayout) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        Self {
            textures: HashMap::new(),
            sampler,
            bind_group_layout,
        }
    }

    /// Get or create a texture from RGBA data.
    pub fn get_or_create(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: &str,
        width: u32,
        height: u32,
        data: &[u8],
    ) -> &CachedTexture {
        if !self.textures.contains_key(key) {
            // An image larger than the device allows in either direction
            // (8192 under wgpu's default limits; hulu ships an 11501 px
            // sprite) is downscaled to fit. `create_texture` would otherwise
            // raise a validation error that takes the whole page down. The
            // cache entry keeps the intrinsic size: background sizing reads
            // it, and drawing samples by UV so the smaller texture is
            // stretched back over the same rect.
            let limit = device.limits().max_texture_dimension_2d;
            let scaled;
            let (tex_w, tex_h, data) = if width > limit || height > limit {
                let (w, h, px) = downscale_rgba_to_fit(width, height, data, limit);
                scaled = px;
                (w, h, scaled.as_slice())
            } else {
                (width, height, data)
            };
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(key),
                size: wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // Rgba8Unorm, NOT Rgba8UnormSrgb: the render targets are linear
                // formats and every other pipeline writes sRGB bytes through
                // unconverted. An Srgb view would linearize on sample and paint
                // images darker than the rest of the page.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * tex_w),
                    rows_per_image: Some(tex_h),
                },
                wgpu::Extent3d {
                    width: tex_w,
                    height: tex_h,
                    depth_or_array_layers: 1,
                },
            );

            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                layout: &self.bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
                label: Some(&format!("{}_bind_group", key)),
            });

            self.textures.insert(key.to_string(), CachedTexture {
                texture,
                view,
                bind_group,
                width,
                height,
            });
        }

        self.textures.get(key).unwrap()
    }

    /// Check if a texture exists.
    pub fn contains(&self, key: &str) -> bool {
        self.textures.contains_key(key)
    }

    /// Get an existing texture.
    pub fn get(&self, key: &str) -> Option<&CachedTexture> {
        self.textures.get(key)
    }

    /// Clear all cached textures.
    pub fn clear(&mut self) {
        self.textures.clear();
    }
    
    /// Remove a specific texture.
    pub fn remove(&mut self, key: &str) {
        self.textures.remove(key);
    }
}

// ==================== Renderer ====================

/// The main display list renderer.
pub struct Renderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,

    // Pipelines
    color_pipeline: wgpu::RenderPipeline,
    texture_pipeline: wgpu::RenderPipeline,
    // Texture pipeline for Rgba8Unorm targets (used for blitting to filter textures)
    // NOTE: Currently unused, kept for potential future use
    _texture_pipeline_rgba: wgpu::RenderPipeline,
    // Blit pipeline for copying RGBA textures (unlike texture_pipeline which treats R as alpha)
    blit_pipeline: wgpu::RenderPipeline,
    color_glyph_pipeline: wgpu::RenderPipeline,
    // Image pipeline: the blit shader composited source-over (straight alpha)
    image_pipeline: wgpu::RenderPipeline,
    // Blit pipeline for Rgba8Unorm targets (for blitting to filter textures)
    blit_pipeline_rgba: wgpu::RenderPipeline,

    // Backdrop filter pipelines (compute shaders for blur + color filters)
    backdrop_filter_pipelines: pipeline::BackdropFilterPipelines,

    // GPU gradient pipeline
    gradient_pipeline: pipeline::GradientPipeline,

    /// Enable GPU gradient rendering (controlled by RUSTKIT_GPU_GRADIENTS env var)
    /// When disabled, uses cell-by-cell rendering for gradients
    gpu_gradients_enabled: bool,

    // Uniform buffer
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    viewport_size: (u32, u32),

    // Vertex batching
    color_vertices: Vec<ColorVertex>,
    color_indices: Vec<u32>,
    texture_vertices: Vec<TextureVertex>,
    texture_indices: Vec<u32>,
    // Color-glyph (emoji) batch — RGBA quads sampling the color atlas, drawn
    // with the passthrough blit pipeline after the grayscale glyph batch. Empty
    // on pages without emoji, so the normal text path pays nothing.
    color_glyph_vertices: Vec<TextureVertex>,
    color_glyph_indices: Vec<u32>,
    // Image quads batch separately from glyphs: the glyph batch binds the
    // glyph atlas for the whole draw, while each image quad must bind its
    // own texture (per-URL runs, drawn between colors and text).
    image_vertices: Vec<TextureVertex>,
    image_indices: Vec<u32>,
    image_runs: Vec<(String, u32)>,

    // GPU gradient queues for batched rendering
    gradient_queue: Vec<QueuedLinearGradient>,
    radial_gradient_queue: Vec<QueuedRadialGradient>,
    conic_gradient_queue: Vec<QueuedConicGradient>,

    // State stacks
    clip_stack: Vec<ClipEntry>,
    /// Scratch buffer for `draw_clipped_quad`. Lives on the renderer so the
    /// hot path allocates once, not once per quad.
    clip_pieces: Vec<(Rect, f32)>,
    stacking_contexts: Vec<StackingContext>,
    /// Stack of 2D transform matrices and their origins.
    /// Each entry is (matrix [a,b,c,d,e,f], origin (x,y)).
    transform_stack: Vec<([f32; 6], (f32, f32))>,

    // Caches
    texture_cache: TextureCache,
    glyph_cache: GlyphCache,

    // Texture bind group layout (for sharing)
    texture_bind_group_layout: wgpu::BindGroupLayout,

    // Intermediate render texture for backdrop filter operations
    // Created lazily when needed, resized to match viewport
    intermediate_texture: Option<wgpu::Texture>,
    intermediate_view: Option<wgpu::TextureView>,
    intermediate_size: (u32, u32),

    // Sampler for drawing filtered textures back to screen
    filter_sampler: wgpu::Sampler,

    // Surface format for creating compatible textures
    surface_format: wgpu::TextureFormat,
}

/// A stacking context for z-ordering.
#[derive(Debug, Clone)]
pub struct StackingContext {
    pub z_index: i32,
    pub rect: Rect,
}

/// A queued linear gradient to be rendered with the GPU shader.
/// Enable GPU gradients via RUSTKIT_GPU_GRADIENTS=1 environment variable.
#[derive(Debug, Clone)]
struct QueuedLinearGradient {
    rect: Rect,
    angle_rad: f32,
    stops: Vec<(f32, rustkit_css::ColorF32)>,
    repeating: bool,
    border_radius: rustkit_layout::BorderRadius,
}

/// A queued radial gradient to be rendered with the GPU shader.
#[derive(Debug, Clone)]
struct QueuedRadialGradient {
    rect: Rect,
    /// X radius in pixels
    rx: f32,
    /// Y radius in pixels
    ry: f32,
    /// Center position (0-1 normalized within rect)
    center: (f32, f32),
    stops: Vec<(f32, rustkit_css::ColorF32)>,
    repeating: bool,
    border_radius: rustkit_layout::BorderRadius,
}

/// A queued conic gradient to be rendered with the GPU shader.
#[derive(Debug, Clone)]
struct QueuedConicGradient {
    rect: Rect,
    /// Starting angle in radians
    from_angle_rad: f32,
    /// Center position (0-1 normalized within rect)
    center: (f32, f32),
    stops: Vec<(f32, rustkit_css::ColorF32)>,
    repeating: bool,
    border_radius: rustkit_layout::BorderRadius,
}

impl Renderer {
    /// Create a new renderer.
    pub fn new(
        device: Arc<wgpu::Device>,
        queue: Arc<wgpu::Queue>,
        surface_format: wgpu::TextureFormat,
    ) -> Result<Self, RendererError> {
        // Create uniform buffer
        let uniforms = Uniforms {
            viewport_size: [800.0, 600.0],
            _padding: [0.0; 2],
        };

        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Uniform Buffer"),
            contents: bytemuck::cast_slice(&[uniforms]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
                label: Some("uniform_bind_group_layout"),
            });

        let uniform_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &uniform_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
            label: Some("uniform_bind_group"),
        });

        // Texture bind group layout
        let texture_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            multisampled: false,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
                label: Some("texture_bind_group_layout"),
            });

        // Create pipelines
        let color_pipeline = create_color_pipeline(
            &device,
            surface_format,
            &uniform_bind_group_layout,
        );

        let texture_pipeline = create_texture_pipeline(
            &device,
            surface_format,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
        );

        // Create texture pipeline for Rgba8Unorm targets (blitting to filter textures)
        let texture_pipeline_rgba = create_texture_pipeline(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
        );

        // Create blit pipeline for copying RGBA textures (properly samples all 4 channels)
        let blit_pipeline = pipeline::create_blit_pipeline(
            &device,
            surface_format,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
        );

        // Color-glyph (emoji) pipeline: blit shader + premultiplied-alpha blend.
        let color_glyph_pipeline = pipeline::create_color_glyph_pipeline(
            &device,
            surface_format,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
        );

        // Image pipeline: blit shader + source-over blend for straight alpha.
        let image_pipeline = pipeline::create_image_pipeline(
            &device,
            surface_format,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
        );

        // Create blit pipeline for Rgba8Unorm targets (blitting to filter textures)
        let blit_pipeline_rgba = pipeline::create_blit_pipeline(
            &device,
            wgpu::TextureFormat::Rgba8Unorm,
            &uniform_bind_group_layout,
            &texture_bind_group_layout,
        );

        // Create backdrop filter pipelines (compute shaders for blur + color filters)
        let backdrop_filter_pipelines = pipeline::create_backdrop_filter_pipelines(&device);

        // Create GPU gradient pipeline
        let gradient_pipeline = pipeline::create_gradient_pipeline(
            &device,
            surface_format,
            &uniform_bind_group_layout,
        );

        // Create caches
        let texture_cache = TextureCache::new(&device, texture_bind_group_layout.clone());
        let glyph_cache = GlyphCache::new(&device, &queue, texture_bind_group_layout.clone())?;

        // Create sampler for drawing filtered textures
        let filter_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        // GPU gradients: the flag is READ but the path is NOT IMPLEMENTED.
        //
        // The queues below are pushed to and cleared, never drained:
        // `render_linear_gradient_gpu` has zero callers, and draw_linear_gradient
        // does `push(...); return;` — skipping the CPU path. Honouring this flag
        // therefore DELETED EVERY GRADIENT ON THE PAGE, silently, with the CPU
        // renderer bypassed and nothing drawn in its place.
        //
        // Forced off until the queues are actually drained. The flag still logs
        // loudly so an operator who sets it learns it did nothing, rather than
        // debugging vanished gradients. Do NOT flip this to `is_ok()` without
        // first wiring flush_to to consume the three queues.
        let gpu_gradients_requested = std::env::var("RUSTKIT_GPU_GRADIENTS").is_ok();
        if gpu_gradients_requested {
            tracing::warn!(
                "RUSTKIT_GPU_GRADIENTS is set but GPU gradient rendering is NOT implemented \
                 (the queues are never drained). Ignoring the flag and using the CPU path. \
                 Honouring it would render no gradients at all."
            );
        }
        let gpu_gradients_enabled = false;

        Ok(Self {
            device,
            queue,
            color_pipeline,
            texture_pipeline,
            _texture_pipeline_rgba: texture_pipeline_rgba,
            blit_pipeline,
            color_glyph_pipeline,
            image_pipeline,
            blit_pipeline_rgba,
            backdrop_filter_pipelines,
            gradient_pipeline,
            gpu_gradients_enabled,
            uniform_buffer,
            uniform_bind_group,
            viewport_size: (800, 600),
            color_vertices: Vec::with_capacity(4096),
            color_indices: Vec::with_capacity(8192),
            texture_vertices: Vec::with_capacity(4096),
            texture_indices: Vec::with_capacity(8192),
            color_glyph_vertices: Vec::new(),
            color_glyph_indices: Vec::new(),
            image_vertices: Vec::with_capacity(256),
            image_indices: Vec::with_capacity(512),
            image_runs: Vec::with_capacity(64),
            gradient_queue: Vec::with_capacity(64),
            radial_gradient_queue: Vec::with_capacity(16),
            conic_gradient_queue: Vec::with_capacity(16),
            clip_stack: Vec::new(),
            clip_pieces: Vec::new(),
            stacking_contexts: Vec::new(),
            transform_stack: Vec::new(),
            texture_cache,
            glyph_cache,
            texture_bind_group_layout,
            intermediate_texture: None,
            intermediate_view: None,
            intermediate_size: (0, 0),
            filter_sampler,
            surface_format,
        })
    }

    /// Validate buffer size to prevent GPU memory exhaustion.
    /// Returns Ok(size) if size is within limits, Err otherwise.
    fn validate_buffer_size(&self, size: u64, label: &str) -> Result<u64, RendererError> {
        if size > MAX_BUFFER_SIZE {
            tracing::error!(
                "Buffer '{}' size {} bytes exceeds maximum {} bytes",
                label,
                size,
                MAX_BUFFER_SIZE
            );
            return Err(RendererError::BufferTooLarge(size, MAX_BUFFER_SIZE));
        }
        Ok(size)
    }

    /// Set the viewport size.
    /// Render `commands` to an offscreen target and save it as PNG plus a
    /// JSON sidecar. The native-win32 shell's screenshot harness and
    /// hiwave-smoke drive this; parity-capture uses the PPM path instead.
    #[cfg(windows)]
    pub fn execute_and_capture(
        &mut self,
        commands: &[DisplayCommand],
        output_path: impl AsRef<std::path::Path>,
    ) -> Result<CaptureMetadata, RendererError> {
        let (width, height) = self.viewport_size;
        let capture_format = self.surface_format;

        let (texture, view) =
            screenshot::create_offscreen_target(&self.device, width, height, capture_format);
        self.execute(commands, &view)?;

        let readback = screenshot::GpuReadbackBuffer::new(&self.device, width, height);
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Screenshot Copy Encoder"),
            });
        readback.copy_from_texture(&mut encoder, &texture);
        self.queue.submit(std::iter::once(encoder.finish()));

        let mut pixels = readback
            .read_data_sync(&self.device)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))?;

        // A BGRA capture target is swizzled to RGBA for PNG encoding.
        match capture_format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                for px in pixels.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
            _ => {}
        }

        screenshot::save_png(&output_path, width, height, &pixels)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))?;

        let metadata = CaptureMetadata {
            width,
            height,
            adapter: "Unknown".to_string(),
            format: format!("{:?}", capture_format),
            timestamp: chrono_lite_timestamp(),
            color_vertex_count: self.color_vertices.len(),
            texture_vertex_count: self.texture_vertices.len(),
        };
        let metadata_path = output_path.as_ref().with_extension("json");
        screenshot::save_capture_metadata(&metadata_path, &metadata)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))?;
        Ok(metadata)
    }

    /// Batch sizes and stack depths of the last executed frame (shell
    /// diagnostics).
    pub fn get_render_stats(&self) -> RenderStats {
        RenderStats {
            color_vertex_count: self.color_vertices.len(),
            color_index_count: self.color_indices.len(),
            texture_vertex_count: self.texture_vertices.len(),
            texture_index_count: self.texture_indices.len(),
            clip_stack_depth: self.clip_stack.len(),
            stacking_context_depth: self.stacking_contexts.len(),
        }
    }

    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        self.viewport_size = (width, height);

        let uniforms = Uniforms {
            viewport_size: [width as f32, height as f32],
            _padding: [0.0; 2],
        };

        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
    }

    /// Create an intermediate texture for backdrop filter operations.
    /// Returns (texture, view) pair. The texture supports both reading and storage writes.
    fn create_filter_texture(&self, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Filter Intermediate Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        (texture, view)
    }

    /// Ensure intermediate render texture exists and matches viewport size.
    /// Uses surface format (typically Bgra8Unorm) for compatibility with render pipelines.
    /// Returns the texture view for rendering.
    fn ensure_intermediate_texture(&mut self) -> &wgpu::TextureView {
        let (width, height) = self.viewport_size;

        // Recreate if size changed or doesn't exist
        if self.intermediate_texture.is_none() || self.intermediate_size != (width, height) {
            // Use surface format so we can render with existing pipelines
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Intermediate Render Texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.surface_format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });

            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.intermediate_texture = Some(texture);
            self.intermediate_view = Some(view);
            self.intermediate_size = (width, height);
        }

        self.intermediate_view.as_ref().unwrap()
    }

    /// Flush current batched vertices to the target without clearing.
    /// Used for incremental rendering when backdrop filters are present.
    fn flush_batches_to(&mut self, target: &wgpu::TextureView, clear: bool) -> Result<(), RendererError> {
        if self.color_vertices.is_empty()
            && self.texture_vertices.is_empty()
            && self.image_vertices.is_empty()
            && self.color_glyph_vertices.is_empty()
        {
            return Ok(());
        }

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Batch Flush Encoder"),
        });

        {
            let load_op = if clear {
                wgpu::LoadOp::Clear(wgpu::Color::WHITE)
            } else {
                wgpu::LoadOp::Load
            };

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Batch Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw solid colors
            if !self.color_vertices.is_empty() {
                // Validate buffer sizes before allocation
                let vertex_size = (self.color_vertices.len() * std::mem::size_of::<ColorVertex>()) as u64;
                let index_size = (self.color_indices.len() * std::mem::size_of::<u32>()) as u64;

                self.validate_buffer_size(vertex_size, "Color Vertex Buffer")?;
                self.validate_buffer_size(index_size, "Color Index Buffer")?;

                let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Color Vertex Buffer"),
                    contents: bytemuck::cast_slice(&self.color_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Color Index Buffer"),
                    contents: bytemuck::cast_slice(&self.color_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                render_pass.set_pipeline(&self.color_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.color_indices.len() as u32, 0, 0..1);
            }

            // Draw images (own textures) between backgrounds and text
            self.draw_image_batch(&mut render_pass);

            // Draw textured quads
            if !self.texture_vertices.is_empty() {
                // Validate buffer sizes before allocation
                let vertex_size = (self.texture_vertices.len() * std::mem::size_of::<TextureVertex>()) as u64;
                let index_size = (self.texture_indices.len() * std::mem::size_of::<u32>()) as u64;

                self.validate_buffer_size(vertex_size, "Texture Vertex Buffer")?;
                self.validate_buffer_size(index_size, "Texture Index Buffer")?;

                let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Texture Vertex Buffer"),
                    contents: bytemuck::cast_slice(&self.texture_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Texture Index Buffer"),
                    contents: bytemuck::cast_slice(&self.texture_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                render_pass.set_pipeline(&self.texture_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, self.glyph_cache.bind_group(), &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.texture_indices.len() as u32, 0, 0..1);
            }

            // Color glyphs (emoji) drawn on top via the RGBA atlas + blit pipeline.
            self.draw_color_glyph_batch(&mut render_pass);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Clear batches after flushing
        self.color_vertices.clear();
        self.color_indices.clear();
        self.texture_vertices.clear();
        self.texture_indices.clear();
        self.color_glyph_vertices.clear();
        self.color_glyph_indices.clear();
        self.image_vertices.clear();
        self.image_indices.clear();
        self.image_runs.clear();
        Ok(())
    }

    /// Flush batched vertices before rendering a GPU gradient.
    /// This ensures correct z-order: batched content renders before the gradient.
    fn flush_batches_for_gradient(&mut self, target: &wgpu::TextureView, clear: bool) -> Result<(), RendererError> {
        if self.color_vertices.is_empty()
            && self.texture_vertices.is_empty()
            && self.image_vertices.is_empty()
        {
            // Nothing to flush, but if this is the first call we still need to clear
            if clear {
                let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Clear Encoder"),
                });
                {
                    let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Clear Pass"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: target,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                }
                self.queue.submit(std::iter::once(encoder.finish()));
            }
            return Ok(());
        }

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Gradient Interleave Flush"),
        });

        {
            let load_op = if clear {
                wgpu::LoadOp::Clear(wgpu::Color::WHITE)
            } else {
                wgpu::LoadOp::Load
            };

            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Batched Content Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw solid colors
            if !self.color_vertices.is_empty() {
                // Validate buffer sizes before allocation
                let vertex_size = (self.color_vertices.len() * std::mem::size_of::<ColorVertex>()) as u64;
                let index_size = (self.color_indices.len() * std::mem::size_of::<u32>()) as u64;

                self.validate_buffer_size(vertex_size, "Color Vertex Buffer")?;
                self.validate_buffer_size(index_size, "Color Index Buffer")?;

                let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Color Vertex Buffer"),
                    contents: bytemuck::cast_slice(&self.color_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Color Index Buffer"),
                    contents: bytemuck::cast_slice(&self.color_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                render_pass.set_pipeline(&self.color_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.color_indices.len() as u32, 0, 0..1);
            }

            // Draw images (own textures) between backgrounds and text
            self.draw_image_batch(&mut render_pass);

            // Draw textured quads
            if !self.texture_vertices.is_empty() {
                // Validate buffer sizes before allocation
                let vertex_size = (self.texture_vertices.len() * std::mem::size_of::<TextureVertex>()) as u64;
                let index_size = (self.texture_indices.len() * std::mem::size_of::<u32>()) as u64;

                self.validate_buffer_size(vertex_size, "Texture Vertex Buffer")?;
                self.validate_buffer_size(index_size, "Texture Index Buffer")?;

                let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Texture Vertex Buffer"),
                    contents: bytemuck::cast_slice(&self.texture_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Texture Index Buffer"),
                    contents: bytemuck::cast_slice(&self.texture_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                render_pass.set_pipeline(&self.texture_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, self.glyph_cache.bind_group(), &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.texture_indices.len() as u32, 0, 0..1);
            }

            // Color glyphs (emoji) drawn on top via the RGBA atlas + blit pipeline.
            self.draw_color_glyph_batch(&mut render_pass);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Clear batches after flushing
        self.color_vertices.clear();
        self.color_indices.clear();
        self.texture_vertices.clear();
        self.texture_indices.clear();
        self.color_glyph_vertices.clear();
        self.color_glyph_indices.clear();
        self.image_vertices.clear();
        self.image_indices.clear();
        self.image_runs.clear();
        Ok(())
    }

    /// Draw a textured quad from a filtered texture to the render target immediately.
    /// This renders with a custom bind group, bypassing the batch system.
    fn draw_filtered_texture_to(
        &self,
        texture_view: &wgpu::TextureView,
        target: &wgpu::TextureView,
        rect: Rect,
    ) {
        // Create bind group for this texture
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Filtered Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.filter_sampler),
                },
            ],
        });

        // Create vertices for the quad - normalized tex coords to sample region
        let x = rect.x;
        let y = rect.y;
        let w = rect.width;
        let h = rect.height;

        // Calculate tex coords based on position in viewport
        let (vw, vh) = self.viewport_size;
        let u0 = rect.x / vw as f32;
        let v0 = rect.y / vh as f32;
        let u1 = (rect.x + rect.width) / vw as f32;
        let v1 = (rect.y + rect.height) / vh as f32;

        let white = [1.0, 1.0, 1.0, 1.0];

        let vertices = [
            TextureVertex { position: [x, y], tex_coords: [u0, v0], color: white },
            TextureVertex { position: [x + w, y], tex_coords: [u1, v0], color: white },
            TextureVertex { position: [x + w, y + h], tex_coords: [u1, v1], color: white },
            TextureVertex { position: [x, y + h], tex_coords: [u0, v1], color: white },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Filtered Quad Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Filtered Quad Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Filtered Texture Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Filtered Texture Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Use blit_pipeline to properly sample RGBA texture
            // (texture_pipeline treats red channel as alpha for glyph rendering)
            render_pass.set_pipeline(&self.blit_pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Create a bind group for backdrop filter compute shader operations.
    fn create_filter_bind_group(
        &self,
        input_view: &wgpu::TextureView,
        output_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Filter Bind Group"),
            layout: &self.backdrop_filter_pipelines.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.backdrop_filter_pipelines.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(input_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(output_view),
                },
            ],
        })
    }

    /// Run Gaussian blur on a texture using compute shaders.
    /// Performs two passes: horizontal then vertical blur.
    fn run_blur_compute(
        &self,
        source_view: &wgpu::TextureView,
        intermediate_view: &wgpu::TextureView,
        dest_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        blur_radius: f32,
    ) {
        // Update filter params uniform
        let params = FilterParams {
            blur_radius,
            filter_type: 0, // Not used for blur
            filter_amount: 1.0,
            texture_width: width as f32,
            texture_height: height as f32,
            _padding0: 0.0,
            _padding1: 0.0,
            _padding2: 0.0,
        };
        self.queue.write_buffer(
            &self.backdrop_filter_pipelines.uniform_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        // Calculate workgroup counts (16x16 workgroups)
        let workgroups_x = (width + 15) / 16;
        let workgroups_y = (height + 15) / 16;

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Blur Compute Encoder"),
        });

        // Pass 1: Horizontal blur (source -> intermediate)
        {
            let bind_group = self.create_filter_bind_group(source_view, intermediate_view);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Horizontal Blur Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.backdrop_filter_pipelines.blur_h_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        // Pass 2: Vertical blur (intermediate -> dest)
        {
            let bind_group = self.create_filter_bind_group(intermediate_view, dest_view);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Vertical Blur Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.backdrop_filter_pipelines.blur_v_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Run a color filter (grayscale, sepia, brightness) on a texture.
    fn run_color_filter_compute(
        &self,
        source_view: &wgpu::TextureView,
        dest_view: &wgpu::TextureView,
        width: u32,
        height: u32,
        filter_type: u32,
        amount: f32,
    ) {
        // Update filter params uniform
        let params = FilterParams {
            blur_radius: 0.0,
            filter_type,
            filter_amount: amount,
            texture_width: width as f32,
            texture_height: height as f32,
            _padding0: 0.0,
            _padding1: 0.0,
            _padding2: 0.0,
        };
        self.queue.write_buffer(
            &self.backdrop_filter_pipelines.uniform_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        // Calculate workgroup counts
        let workgroups_x = (width + 15) / 16;
        let workgroups_y = (height + 15) / 16;

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Color Filter Compute Encoder"),
        });

        {
            let bind_group = self.create_filter_bind_group(source_view, dest_view);
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("Color Filter Pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.backdrop_filter_pipelines.color_filter_pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(workgroups_x, workgroups_y, 1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Execute a display list and render to a target.
    pub fn execute(
        &mut self,
        commands: &[DisplayCommand],
        target: &wgpu::TextureView,
    ) -> Result<(), RendererError> {
        // Clear batches
        self.color_vertices.clear();
        self.color_indices.clear();
        self.texture_vertices.clear();
        self.texture_indices.clear();
        self.color_glyph_vertices.clear();
        self.color_glyph_indices.clear();
        self.image_vertices.clear();
        self.image_indices.clear();
        self.image_runs.clear();
        self.gradient_queue.clear();
        self.radial_gradient_queue.clear();
        self.conic_gradient_queue.clear();
        self.clip_stack.clear();
        self.stacking_contexts.clear();
        self.transform_stack.clear();

        // Check if there are any blur backdrop filters that need GPU processing
        let has_blur_filters = commands.iter().any(|cmd| {
            matches!(cmd, DisplayCommand::BackdropFilter {
                filter: rustkit_css::BackdropFilter::Blur(r), ..
            } if *r > 0.0)
        });

        // Check if there are any GPU gradients that need z-order aware rendering
        let has_gpu_gradients = self.gpu_gradients_enabled && commands.iter().any(|cmd| {
            matches!(cmd,
                DisplayCommand::LinearGradient { .. } |
                DisplayCommand::RadialGradient { .. } |
                DisplayCommand::ConicGradient { .. }
            )
        });

        if has_blur_filters {
            // Use GPU blur path - render to intermediate texture with GPU blur processing
            self.execute_with_gpu_blur(commands, target)?;
        } else if has_gpu_gradients {
            // Use GPU gradient path - flush batches before each gradient for correct z-order
            self.execute_with_gpu_gradients(commands, target)?;
        } else {
            // Fast path - no backdrop blur or GPU gradients, process normally.
            // One z-order hazard remains: the batch flush draws ALL color
            // quads before ALL glyph quads, so a solid fill that arrives
            // AFTER glyphs are batched — e.g. a positioned box's background
            // painting above in-flow text per CSS 2.1 Appendix E — would be
            // drawn UNDER that text. Flush first so paint follows command
            // order (same discipline as the GPU-gradient path).
            let mut flushed_mid_stream = false;
            for cmd in commands {
                if self.solid_fill_occludes_batched_glyphs(cmd) {
                    self.flush_batches_to(target, !flushed_mid_stream)?;
                    flushed_mid_stream = true;
                }
                self.process_command(cmd);
            }
            if flushed_mid_stream {
                self.flush_batches_to(target, false)?;
            } else {
                self.flush_to(target)?;
            }
        }

        Ok(())
    }

    /// True when `cmd` is a solid fill whose rect overlaps a glyph quad
    /// already sitting in the batch — the case where flush order (colors
    /// before glyphs) would contradict command order. Batched glyph
    /// positions are already transformed, so the rect's corners get the
    /// same transform before the overlap test.
    fn solid_fill_occludes_batched_glyphs(&self, cmd: &DisplayCommand) -> bool {
        if self.texture_vertices.is_empty() && self.color_glyph_vertices.is_empty() {
            return false;
        }
        let rect = match cmd {
            DisplayCommand::SolidColor(color, rect) if color.a > 0.0 => rect,
            DisplayCommand::RoundedRect { color, rect, .. } if color.a > 0.0 => rect,
            _ => return false,
        };
        let (ax, ay) = self.transform_point(rect.x, rect.y);
        let (bx, by) = self.transform_point(rect.x + rect.width, rect.y + rect.height);
        let (rx0, rx1) = (ax.min(bx), ax.max(bx));
        let (ry0, ry1) = (ay.min(by), ay.max(by));

        let overlaps = |verts: &[TextureVertex]| {
            verts.chunks_exact(4).any(|quad| {
                let mut qx0 = f32::MAX;
                let mut qy0 = f32::MAX;
                let mut qx1 = f32::MIN;
                let mut qy1 = f32::MIN;
                for v in quad {
                    qx0 = qx0.min(v.position[0]);
                    qy0 = qy0.min(v.position[1]);
                    qx1 = qx1.max(v.position[0]);
                    qy1 = qy1.max(v.position[1]);
                }
                rx0 < qx1 && rx1 > qx0 && ry0 < qy1 && ry1 > qy0
            })
        };
        overlaps(&self.texture_vertices) || overlaps(&self.color_glyph_vertices)
    }

    /// Execute commands with GPU blur support for backdrop filters.
    fn execute_with_gpu_blur(
        &mut self,
        commands: &[DisplayCommand],
        target: &wgpu::TextureView,
    ) -> Result<(), RendererError> {
        // Ensure intermediate texture exists
        let _ = self.ensure_intermediate_texture();
        let intermediate_view = self.intermediate_view.as_ref().unwrap().clone();

        let mut is_first_flush = true;

        for cmd in commands {
            // Check if this is a blur backdrop filter
            if let DisplayCommand::BackdropFilter {
                rect,
                border_radius: _,
                filter: rustkit_css::BackdropFilter::Blur(radius),
            } = cmd
            {
                if *radius > 0.0 {
                    // Flush current batches to intermediate texture
                    self.flush_batches_to(&intermediate_view, is_first_flush)?;
                    is_first_flush = false;

                    // Apply GPU blur
                    self.apply_gpu_blur(&intermediate_view, *rect, *radius);

                    continue;
                }
            }

            // Process command normally (including non-blur backdrop filters)
            self.process_command(cmd);
        }

        // Flush remaining batches to intermediate
        if !self.color_vertices.is_empty()
            || !self.texture_vertices.is_empty()
            || !self.image_vertices.is_empty()
        {
            self.flush_batches_to(&intermediate_view, is_first_flush)?;
        }

        // Copy intermediate to final target
        self.copy_texture_to_target(&intermediate_view, target);

        Ok(())
    }

    /// Execute commands with GPU gradient support for correct z-order.
    ///
    /// This method flushes batched content BEFORE each gradient to ensure
    /// parent gradients render behind child content (correct DOM z-order).
    fn execute_with_gpu_gradients(
        &mut self,
        commands: &[DisplayCommand],
        target: &wgpu::TextureView,
    ) -> Result<(), RendererError> {
        let mut is_first_flush = true;

        for cmd in commands {
            // Check if this is a GPU gradient command
            let is_gpu_gradient = matches!(cmd,
                DisplayCommand::LinearGradient { .. } |
                DisplayCommand::RadialGradient { .. } |
                DisplayCommand::ConicGradient { .. }
            );

            if is_gpu_gradient {
                // Flush batched content FIRST (before gradient)
                // This ensures children render before their parent's gradient
                self.flush_batches_for_gradient(target, is_first_flush)?;
                is_first_flush = false;

                // Render the gradient directly (inline, not queued)
                self.render_gpu_gradient_inline(cmd, target);
            } else {
                // Process command normally (batched)
                self.process_command(cmd);
            }
        }

        // Flush any remaining batched content
        if !self.color_vertices.is_empty()
            || !self.texture_vertices.is_empty()
            || !self.image_vertices.is_empty()
        {
            self.flush_batches_for_gradient(target, is_first_flush)?;
        }

        Ok(())
    }

    /// Render a GPU gradient inline (immediately, not queued).
    /// Called from execute_with_gpu_gradients() for correct z-order.
    fn render_gpu_gradient_inline(&mut self, cmd: &DisplayCommand, target: &wgpu::TextureView) {
        match cmd {
            DisplayCommand::LinearGradient { rect, direction, stops, repeating, border_radius } => {
                self.render_linear_gradient_inline(*rect, *direction, stops, *repeating, *border_radius, target);
            }
            DisplayCommand::RadialGradient { rect, shape, size, center, stops, repeating, border_radius } => {
                self.render_radial_gradient_inline(*rect, *shape, *size, *center, stops, *repeating, *border_radius, target);
            }
            DisplayCommand::ConicGradient { rect, from_angle, center, stops, repeating, border_radius } => {
                self.render_conic_gradient_inline(*rect, *from_angle, *center, stops, *repeating, *border_radius, target);
            }
            _ => {}
        }
    }

    /// Render a linear gradient directly to the target (inline GPU path).
    fn render_linear_gradient_inline(
        &self,
        rect: Rect,
        direction: rustkit_css::GradientDirection,
        stops: &[rustkit_css::ColorStop],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
        target: &wgpu::TextureView,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Convert direction to angle in radians
        let angle_deg = direction.to_degrees();
        let angle_rad = angle_deg.to_radians();

        // Calculate gradient geometry
        let (sin_a, cos_a) = (angle_rad.sin(), angle_rad.cos());
        let half_width = rect.width / 2.0;
        let half_height = rect.height / 2.0;
        let gradient_half_length = (half_width * sin_a.abs() + half_height * cos_a.abs()).max(0.001);

        // Check if any stop uses pixel positions
        let has_pixel_positions = stops.iter().any(|s| {
            s.position.as_ref().map(|p| p.is_pixels()).unwrap_or(false)
        });

        // Calculate repeat length for pixel-based repeating gradients
        let repeat_length_pixels = if repeating && has_pixel_positions {
            stops.last()
                .and_then(|s| s.position.as_ref())
                .map(|p| match p {
                    rustkit_css::StopPosition::Pixels(px) => *px,
                    rustkit_css::StopPosition::Percent(pct) => *pct * gradient_half_length * 2.0,
                })
                .unwrap_or(gradient_half_length * 2.0)
                .max(0.001)
        } else {
            gradient_half_length * 2.0
        };

        // Normalize stops
        let normalized_stops: Vec<(f32, rustkit_css::ColorF32)> = stops.iter().enumerate()
            .map(|(i, stop)| {
                let pos = match &stop.position {
                    Some(p) => {
                        if has_pixel_positions && repeating {
                            match p {
                                rustkit_css::StopPosition::Pixels(px) => *px / repeat_length_pixels,
                                rustkit_css::StopPosition::Percent(pct) => *pct,
                            }
                        } else {
                            match p {
                                rustkit_css::StopPosition::Percent(pct) => *pct,
                                rustkit_css::StopPosition::Pixels(px) => *px / (gradient_half_length * 2.0),
                            }
                        }
                    }
                    None => {
                        if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
                    }
                };
                (pos, rustkit_css::ColorF32::from_color(stop.color))
            })
            .collect();

        // Render using GPU
        self.render_linear_gradient_gpu_with_clear(
            target,
            rect,
            angle_rad,
            &normalized_stops,
            repeating,
            border_radius,
            None, // LoadOp::Load to preserve previous content
        );
    }

    /// Render a radial gradient directly to the target (inline GPU path).
    fn render_radial_gradient_inline(
        &self,
        rect: Rect,
        shape: rustkit_css::RadialShape,
        size: rustkit_css::RadialSize,
        center: (f32, f32),
        stops: &[rustkit_css::ColorStop],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
        target: &wgpu::TextureView,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate radii based on shape and size
        let (rx, ry) = self.calculate_radial_radii(rect, shape, size, center);

        // Check if any stop uses pixel positions
        let has_pixel_positions = stops.iter().any(|s| {
            s.position.as_ref().map(|p| p.is_pixels()).unwrap_or(false)
        });

        // Calculate repeat length for pixel-based repeating gradients
        let repeat_length_pixels = if repeating && has_pixel_positions {
            stops.last()
                .and_then(|s| s.position.as_ref())
                .map(|p| match p {
                    rustkit_css::StopPosition::Pixels(px) => *px,
                    rustkit_css::StopPosition::Percent(pct) => *pct * rx.max(ry),
                })
                .unwrap_or(rx.max(ry))
                .max(0.001)
        } else {
            rx.max(ry)
        };

        // Normalize stops
        let normalized_stops: Vec<(f32, rustkit_css::ColorF32)> = stops.iter().enumerate()
            .map(|(i, stop)| {
                let pos = match &stop.position {
                    Some(p) => {
                        if has_pixel_positions && repeating {
                            match p {
                                rustkit_css::StopPosition::Pixels(px) => *px / repeat_length_pixels,
                                rustkit_css::StopPosition::Percent(pct) => *pct,
                            }
                        } else {
                            match p {
                                rustkit_css::StopPosition::Percent(pct) => *pct,
                                rustkit_css::StopPosition::Pixels(px) => *px / rx.max(ry).max(0.001),
                            }
                        }
                    }
                    None => {
                        if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
                    }
                };
                (pos, rustkit_css::ColorF32::from_color(stop.color))
            })
            .collect();

        // Render using GPU
        self.render_radial_gradient_gpu(
            target,
            rect,
            rx, ry,
            center,
            &normalized_stops,
            repeating,
            border_radius,
        );
    }

    /// Render a conic gradient directly to the target (inline GPU path).
    fn render_conic_gradient_inline(
        &self,
        rect: Rect,
        from_angle: f32,
        center: (f32, f32),
        stops: &[rustkit_css::ColorStop],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
        target: &wgpu::TextureView,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Conic gradients use angular positions (0-360 degrees or 0-1 normalized)
        let normalized_stops: Vec<(f32, rustkit_css::ColorF32)> = stops.iter().enumerate()
            .map(|(i, stop)| {
                let pos = match &stop.position {
                    Some(p) => match p {
                        rustkit_css::StopPosition::Percent(pct) => *pct,
                        rustkit_css::StopPosition::Pixels(deg) => *deg / 360.0, // Degrees to 0-1
                    },
                    None => {
                        if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
                    }
                };
                (pos, rustkit_css::ColorF32::from_color(stop.color))
            })
            .collect();

        // Convert from_angle to radians
        let from_rad = from_angle.to_radians();

        // Render using GPU
        self.render_conic_gradient_gpu(
            target,
            rect,
            from_rad,
            center,
            &normalized_stops,
            repeating,
            border_radius,
        );
    }

    /// Calculate radial gradient radii based on shape and size.
    fn calculate_radial_radii(
        &self,
        rect: Rect,
        shape: rustkit_css::RadialShape,
        size: rustkit_css::RadialSize,
        center: (f32, f32),
    ) -> (f32, f32) {
        let cx = rect.x + rect.width * center.0;
        let cy = rect.y + rect.height * center.1;

        // Distances to each edge from center
        let dist_left = (cx - rect.x).abs();
        let dist_right = (rect.x + rect.width - cx).abs();
        let dist_top = (cy - rect.y).abs();
        let dist_bottom = (rect.y + rect.height - cy).abs();

        // Distances to corners
        let corner_tl = ((cx - rect.x).powi(2) + (cy - rect.y).powi(2)).sqrt();
        let corner_tr = ((rect.x + rect.width - cx).powi(2) + (cy - rect.y).powi(2)).sqrt();
        let corner_bl = ((cx - rect.x).powi(2) + (rect.y + rect.height - cy).powi(2)).sqrt();
        let corner_br = ((rect.x + rect.width - cx).powi(2) + (rect.y + rect.height - cy).powi(2)).sqrt();

        let (rx, ry) = match size {
            rustkit_css::RadialSize::ClosestSide => {
                let dx = dist_left.min(dist_right);
                let dy = dist_top.min(dist_bottom);
                match shape {
                    rustkit_css::RadialShape::Circle => {
                        let r = dx.min(dy);
                        (r, r)
                    }
                    rustkit_css::RadialShape::Ellipse => (dx, dy),
                }
            }
            rustkit_css::RadialSize::FarthestSide => {
                let dx = dist_left.max(dist_right);
                let dy = dist_top.max(dist_bottom);
                match shape {
                    rustkit_css::RadialShape::Circle => {
                        let r = dx.max(dy);
                        (r, r)
                    }
                    rustkit_css::RadialShape::Ellipse => (dx, dy),
                }
            }
            rustkit_css::RadialSize::ClosestCorner => {
                let min_corner = corner_tl.min(corner_tr).min(corner_bl).min(corner_br);
                match shape {
                    rustkit_css::RadialShape::Circle => (min_corner, min_corner),
                    rustkit_css::RadialShape::Ellipse => {
                        // css-images-3 §3.3.3: the corner ellipse has the
                        // SAME ASPECT as the closest-side ellipse and passes
                        // through the closest corner — exactly the per-axis
                        // side distances scaled by sqrt(2). (Was: Euclidean
                        // corner distance as rx with ry from the box aspect,
                        // which made every corner-sized ellipse too small.)
                        let dx = dist_left.min(dist_right);
                        let dy = dist_top.min(dist_bottom);
                        (dx * std::f32::consts::SQRT_2, dy * std::f32::consts::SQRT_2)
                    }
                }
            }
            rustkit_css::RadialSize::FarthestCorner => {
                let max_corner = corner_tl.max(corner_tr).max(corner_bl).max(corner_br);
                match shape {
                    rustkit_css::RadialShape::Circle => (max_corner, max_corner),
                    rustkit_css::RadialShape::Ellipse => {
                        // css-images-3 §3.3.3 — see ClosestCorner. Verified
                        // against Chrome 148: 150x100 box, center position,
                        // Chrome's ramp gives rx = 106.1 = 75·sqrt(2).
                        let dx = dist_left.max(dist_right);
                        let dy = dist_top.max(dist_bottom);
                        (dx * std::f32::consts::SQRT_2, dy * std::f32::consts::SQRT_2)
                    }
                }
            }
            rustkit_css::RadialSize::Explicit(w, h) => {
                match shape {
                    rustkit_css::RadialShape::Circle => (w, w),
                    rustkit_css::RadialShape::Ellipse => (w, h),
                }
            }
        };

        (rx.max(0.001), ry.max(0.001))
    }

    /// Blit from intermediate texture (surface format) to a filter texture (Rgba8Unorm).
    /// This performs format conversion during the render pass.
    fn blit_to_filter_texture(&self, dest_view: &wgpu::TextureView) {
        let (vw, vh) = self.viewport_size;

        // Create bind group for sampling the intermediate texture
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Blit Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(
                        self.intermediate_view.as_ref().unwrap(),
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.filter_sampler),
                },
            ],
        });

        // Full-screen quad vertices
        let vertices = [
            TextureVertex { position: [0.0, 0.0], tex_coords: [0.0, 0.0], color: [1.0, 1.0, 1.0, 1.0] },
            TextureVertex { position: [vw as f32, 0.0], tex_coords: [1.0, 0.0], color: [1.0, 1.0, 1.0, 1.0] },
            TextureVertex { position: [vw as f32, vh as f32], tex_coords: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0] },
            TextureVertex { position: [0.0, vh as f32], tex_coords: [0.0, 1.0], color: [1.0, 1.0, 1.0, 1.0] },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Blit Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Blit Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Blit to Filter Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Blit to Filter Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: dest_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Use blit_pipeline_rgba for rendering to Rgba8Unorm target
            // (properly samples all 4 RGBA channels, unlike texture_pipeline which treats R as alpha)
            render_pass.set_pipeline(&self.blit_pipeline_rgba);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Apply GPU Gaussian blur to a region of the intermediate texture.
    fn apply_gpu_blur(
        &self,
        render_target: &wgpu::TextureView,
        rect: Rect,
        blur_radius: f32,
    ) {
        let (vw, vh) = self.viewport_size;

        // Create filter textures for the blur passes (full viewport size for simplicity)
        let (_filter_tex_a, filter_view_a) = self.create_filter_texture(vw, vh);
        let (_filter_tex_b, filter_view_b) = self.create_filter_texture(vw, vh);

        // Blit from intermediate texture (Bgra8Unorm) to filter texture A (Rgba8Unorm)
        // This performs format conversion via the blit_pipeline_rgba
        self.blit_to_filter_texture(&filter_view_a);

        // Run the blur compute passes: A -> B (horizontal), B -> A (vertical)
        self.run_blur_compute(
            &filter_view_a,
            &filter_view_b,
            &filter_view_a,
            vw,
            vh,
            blur_radius,
        );

        // Draw the blurred result back to the render target at the specified rect
        self.draw_filtered_texture_to(&filter_view_a, render_target, rect);
    }

    /// Copy the intermediate texture to the final target.
    fn copy_texture_to_target(
        &self,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
    ) {
        let (vw, vh) = self.viewport_size;

        // Draw the entire intermediate texture to the target
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Copy Texture Bind Group"),
            layout: &self.texture_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.filter_sampler),
                },
            ],
        });

        let vertices = [
            TextureVertex { position: [0.0, 0.0], tex_coords: [0.0, 0.0], color: [1.0, 1.0, 1.0, 1.0] },
            TextureVertex { position: [vw as f32, 0.0], tex_coords: [1.0, 0.0], color: [1.0, 1.0, 1.0, 1.0] },
            TextureVertex { position: [vw as f32, vh as f32], tex_coords: [1.0, 1.0], color: [1.0, 1.0, 1.0, 1.0] },
            TextureVertex { position: [0.0, vh as f32], tex_coords: [0.0, 1.0], color: [1.0, 1.0, 1.0, 1.0] },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Copy Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Copy Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Copy to Target Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Copy to Target Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Use blit_pipeline instead of texture_pipeline to properly sample RGBA
            // (texture_pipeline treats red channel as alpha for glyph rendering)
            render_pass.set_pipeline(&self.blit_pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Process a single display command.
    fn process_command(&mut self, cmd: &DisplayCommand) {
        match cmd {
            DisplayCommand::SolidColor(color, rect) => {
                self.draw_solid_rect(*rect, *color);
            }

            DisplayCommand::RoundedRect { color, rect, radius } => {
                if radius.is_zero() {
                    self.draw_solid_rect(*rect, *color);
                } else {
                    // Draw rounded rect using SDF-based pixel rendering
                    self.draw_rounded_rect(*rect, *color, *radius);
                }
            }

            DisplayCommand::Border {
                color,
                rect,
                top,
                right,
                bottom,
                left,
            } => {
                self.draw_border(*rect, *color, *top, *right, *bottom, *left);
            }

            DisplayCommand::RoundedBorder { rect, widths, colors, radius } => {
                self.draw_rounded_border(*rect, *widths, *colors, *radius);
            }

            DisplayCommand::Text {
                text,
                x,
                y,
                color,
                font_size,
                font_family,
                font_weight,
                font_style,
                advances,
                ascent,
                run,
            } => {
                // SHAPED-RUN CONTRACT (S0): a command that carries its run
                // is painted from the run. The family-list path below is
                // for commands without one, and for a run whose face the
                // rasterizer no longer holds.
                let painted_from_run = run
                    .as_deref()
                    .is_some_and(|run| self.draw_glyph_run(run, *x, *y, *color, *ascent));
                if !painted_from_run {
                    self.draw_text_with_metrics(
                        text,
                        *x,
                        *y,
                        *color,
                        *font_size,
                        font_family,
                        *font_weight,
                        *font_style,
                        advances.as_deref(),
                        *ascent,
                    );
                }
            }

            DisplayCommand::TextDecoration {
                x,
                y,
                width,
                thickness,
                color,
                style: _,
            } => {
                // Draw as a solid rect
                self.draw_solid_rect(
                    Rect::new(*x, *y, *width, *thickness),
                    *color,
                );
            }

            DisplayCommand::Image {
                url,
                src_rect: _,
                dest_rect,
                object_fit: _,
                opacity: _,
                current_color: _,
            } => {
                self.draw_image(url, *dest_rect);
            }

            DisplayCommand::BackgroundImage {
                url,
                rect,
                size,
                position,
                repeat,
            } => {
                self.draw_background_image(url, *rect, size, *position, repeat);
            }

            DisplayCommand::BoxShadow {
                offset_x,
                offset_y,
                blur_radius,
                spread_radius,
                color,
                rect,
                border_radius,
                inset,
            } => {
                self.draw_box_shadow(
                    *rect,
                    *border_radius,
                    *offset_x,
                    *offset_y,
                    *blur_radius,
                    *spread_radius,
                    *color,
                    *inset,
                );
            }

            DisplayCommand::BackdropFilter { rect, border_radius, filter } => {
                self.apply_backdrop_filter(*rect, *border_radius, *filter);
            }

            DisplayCommand::LinearGradient { rect, direction, stops, repeating, border_radius } => {
                self.draw_linear_gradient(*rect, *direction, stops, *repeating, *border_radius);
            }

            DisplayCommand::RadialGradient { rect, shape, size, center, stops, repeating, border_radius } => {
                self.draw_radial_gradient(*rect, *shape, *size, *center, stops, *repeating, *border_radius);
            }

            DisplayCommand::ConicGradient { rect, from_angle, center, stops, repeating, border_radius } => {
                self.draw_conic_gradient(*rect, *from_angle, *center, stops, *repeating, *border_radius);
            }

            DisplayCommand::TextInput {
                rect,
                value,
                placeholder,
                font_size,
                text_color,
                placeholder_color,
                background_color,
                border_color,
                border_width,
                focused,
                caret_position,
                font_family,
                font_weight,
                padding,
                kind,
            } => {
                self.draw_text_input(
                    *rect,
                    value,
                    placeholder,
                    *font_size,
                    *text_color,
                    *placeholder_color,
                    *background_color,
                    *border_color,
                    *border_width,
                    *focused,
                    *caret_position,
                    font_family,
                    *font_weight,
                    *padding,
                    *kind,
                );
            }

            DisplayCommand::ListBox {
                rect,
                options,
                selected,
                row_height,
                font_size,
                font_family,
                font_weight,
                text_color,
                background_color,
                border_color,
                border_width,
            } => {
                self.draw_list_box(
                    *rect,
                    options,
                    selected,
                    *row_height,
                    *font_size,
                    font_family,
                    *font_weight,
                    *text_color,
                    *background_color,
                    *border_color,
                    *border_width,
                );
            }

            DisplayCommand::Button {
                rect,
                label,
                font_size,
                text_color,
                background_color,
                border_color,
                border_width,
                border_radius,
                pressed,
                focused,
                font_family,
                font_weight,
                padding,
            } => {
                self.draw_button(
                    *rect,
                    label,
                    *font_size,
                    *text_color,
                    *background_color,
                    *border_color,
                    *border_width,
                    *border_radius,
                    *pressed,
                    *focused,
                    font_family,
                    *font_weight,
                    *padding,
                );
            }

            DisplayCommand::FocusRing { rect, color, width, offset } => {
                self.draw_focus_ring(*rect, *color, *width, *offset);
            }

            DisplayCommand::Caret { x, y, height, color } => {
                self.draw_caret(*x, *y, *height, *color);
            }

            DisplayCommand::PushClip(rect) => {
                self.push_clip(*rect);
            }

            DisplayCommand::PushClipRounded { rect, radius } => {
                self.push_clip_rounded(*rect, *radius);
            }

            DisplayCommand::PopClip => {
                self.pop_clip();
            }

            DisplayCommand::PushStackingContext { z_index, rect } => {
                self.stacking_contexts.push(StackingContext {
                    z_index: *z_index,
                    rect: *rect,
                });
            }

            DisplayCommand::PopStackingContext => {
                self.stacking_contexts.pop();
            }

            // SVG primitives
            DisplayCommand::FillRect { rect, color } => {
                self.draw_solid_rect(*rect, *color);
            }

            DisplayCommand::StrokeRect { rect, color, width } => {
                // Draw as 4 lines forming a rectangle
                self.draw_border(*rect, *color, *width, *width, *width, *width);
            }

            DisplayCommand::FillCircle { cx, cy, radius, color } => {
                // Render circle using triangle fan
                self.draw_fill_circle(*cx, *cy, *radius, *color);
            }

            DisplayCommand::StrokeCircle { cx, cy, radius, color, width } => {
                // A stroke is centred on the geometry (SVG 2 §13.4): the ring
                // spans r ± w/2. Painted as an annulus so the interior stays
                // whatever is underneath — the old two-disc trick filled it
                // with opaque white, which put a white disc inside every
                // `fill="none"` icon ring on a dark toolbar.
                let half = width * 0.5;
                self.draw_ring(*cx, *cy, radius + half, (radius - half).max(0.0), *color);
            }

            DisplayCommand::FillEllipse { rect, color } => {
                // Render ellipse using triangle fan with parametric equations
                self.draw_fill_ellipse(*rect, *color);
            }

            DisplayCommand::Line { x1, y1, x2, y2, color, width } => {
                // Draw as thin rectangle
                let dx = x2 - x1;
                let dy = y2 - y1;
                let len = (dx * dx + dy * dy).sqrt();
                if len > 0.0 {
                    // Calculate perpendicular offset for width
                    let nx = -dy / len * width * 0.5;
                    let ny = dx / len * width * 0.5;
                    
                    let c = [
                        color.r as f32 / 255.0,
                        color.g as f32 / 255.0,
                        color.b as f32 / 255.0,
                        color.a,
                    ];
                    
                    let base = self.color_vertices.len() as u32;
                    self.color_vertices.extend_from_slice(&[
                        ColorVertex { position: [x1 + nx, y1 + ny], color: c },
                        ColorVertex { position: [x2 + nx, y2 + ny], color: c },
                        ColorVertex { position: [x2 - nx, y2 - ny], color: c },
                        ColorVertex { position: [x1 - nx, y1 - ny], color: c },
                    ]);
                    self.color_indices.extend_from_slice(&[
                        base, base + 1, base + 2,
                        base, base + 2, base + 3,
                    ]);
                }
            }

            DisplayCommand::Polyline { points, color, width } => {
                // Draw as series of lines
                for i in 0..points.len().saturating_sub(1) {
                    let (x1, y1) = points[i];
                    let (x2, y2) = points[i + 1];
                    self.process_command(&DisplayCommand::Line {
                        x1, y1, x2, y2,
                        color: *color,
                        width: *width,
                    });
                }
            }

            DisplayCommand::FillPolygon { points, color } => {
                // Simple triangle fan for convex polygons
                if points.len() >= 3 {
                    let c = [
                        color.r as f32 / 255.0,
                        color.g as f32 / 255.0,
                        color.b as f32 / 255.0,
                        color.a,
                    ];
                    
                    let base = self.color_vertices.len() as u32;
                    for (x, y) in points {
                        self.color_vertices.push(ColorVertex {
                            position: [*x, *y],
                            color: c,
                        });
                    }
                    
                    // Triangle fan
                    for i in 1..points.len() as u32 - 1 {
                        self.color_indices.extend_from_slice(&[base, base + i, base + i + 1]);
                    }
                }
            }

            DisplayCommand::StrokePolygon { points, color, width } => {
                // Draw as closed polyline
                if !points.is_empty() {
                    let mut closed_points = points.clone();
                    closed_points.push(points[0]);
                    self.process_command(&DisplayCommand::Polyline {
                        points: closed_points,
                        color: *color,
                        width: *width,
                    });
                }
            }

            DisplayCommand::PushTransform { matrix, origin } => {
                self.push_transform(*matrix, *origin);
            }

            DisplayCommand::PopTransform => {
                self.pop_transform();
            }

            DisplayCommand::GradientText {
                text,
                x,
                y,
                font_size,
                font_family,
                font_weight,
                font_style,
                gradient,
                rect,
                advances,
                ascent,
            } => {
                // Glyph quads are alpha-textured and tinted per VERTEX, so
                // gradient text needs no offscreen mask: each glyph's left
                // and right vertex pairs take the gradient color sampled at
                // those x positions and the GPU interpolates between them.
                // The sweep is sampled horizontally across `rect` — the
                // vertical component of an angled gradient is ignored, an
                // approximation that is exact for to-right/to-left and close
                // for the diagonal hero-text cases this feature serves.
                // (This replaces a hardcoded PURPLE debug fallback that
                // painted every background-clip:text run violet.)
                self.draw_text_gradient(
                    text,
                    *x,
                    *y,
                    gradient,
                    rect,
                    *font_size,
                    font_family,
                    *font_weight,
                    *font_style,
                    advances.as_deref(),
                    *ascent,
                );
            }
        }
    }

    /// Draw a solid color rectangle.
    fn draw_solid_rect(&mut self, rect: Rect, color: Color) {
        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];
        self.draw_clipped_quad(rect, c);
    }

    /// Draw a solid color rectangle using high-precision color.
    /// This is the preferred internal method for gradient rendering.
    fn draw_solid_rect_f32(&mut self, rect: Rect, color: rustkit_css::ColorF32) {
        // Color already in normalized f32 format - no conversion needed
        self.draw_clipped_quad(rect, color.to_array());
    }

    /// Clip a quad against the current clip and emit what survives.
    ///
    /// The rectangular half is unchanged from before rounded clips existed. The
    /// rounded half only runs when a rounded clip is actually on the stack, so
    /// a page without one emits exactly the vertices it always did.
    ///
    /// The clip stack is in SCREEN space (see `push_clip_rounded`), so the quad
    /// is taken to screen space first and clipped where it actually lands.
    /// Until this, the quad was clipped in document space and transformed
    /// afterwards, so a transformed descendant escaped its ancestor's
    /// `overflow: hidden`: `.btn::before { inset: 0; transform:
    /// translateX(-100%) }` painted as a shine bar LEFT of the button Chrome
    /// clips it inside (about, n46).
    fn draw_clipped_quad(&mut self, rect: Rect, color: [f32; 4]) {
        // Borrowed out and put back so the immutable borrow of `clip_stack`
        // inside `collect_clipped_pieces` does not collide with the mutable
        // borrow the emit loop needs. Reused rather than freshly allocated
        // because gradients call this once per cell — up to 100k times a frame.
        let mut pieces = std::mem::take(&mut self.clip_pieces);
        pieces.clear();
        let space = clip_quad_under(
            self.current_transform(),
            self.clip_stack.last(),
            rect,
            &mut pieces,
        );

        for &(piece, coverage) in &pieces {
            let mut faded = color;
            faded[3] *= coverage;
            match space {
                QuadSpace::Screen => self.push_screen_quad(piece, faded),
                QuadSpace::Document => self.push_color_quad(piece, faded),
            }
        }

        self.clip_pieces = pieces;
    }

    /// Append one transformed quad to the color batch. No clipping — callers
    /// have already done it.
    fn push_color_quad(&mut self, rect: Rect, c: [f32; 4]) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Apply transform to corners
        let (x0, y0) = self.transform_point(rect.x, rect.y);
        let (x1, y1) = self.transform_point(rect.x + rect.width, rect.y);
        let (x2, y2) = self.transform_point(rect.x + rect.width, rect.y + rect.height);
        let (x3, y3) = self.transform_point(rect.x, rect.y + rect.height);
        self.push_screen_corners([[x0, y0], [x1, y1], [x2, y2], [x3, y3]], c);
    }

    /// Append one quad that is ALREADY in screen space — no transform, no
    /// clipping.
    fn push_screen_quad(&mut self, rect: Rect, c: [f32; 4]) {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }
        let (x0, y0) = (rect.x, rect.y);
        let (x1, y1) = (rect.x + rect.width, rect.y + rect.height);
        self.push_screen_corners([[x0, y0], [x1, y0], [x1, y1], [x0, y1]], c);
    }

    fn push_screen_corners(&mut self, p: [[f32; 2]; 4], c: [f32; 4]) {
        let base = self.color_vertices.len() as u32;
        let [[x0, y0], [x1, y1], [x2, y2], [x3, y3]] = p;

        self.color_vertices.extend_from_slice(&[
            ColorVertex { position: [x0, y0], color: c },
            ColorVertex { position: [x1, y1], color: c },
            ColorVertex { position: [x2, y2], color: c },
            ColorVertex { position: [x3, y3], color: c },
        ]);

        self.color_indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
        ]);
    }

    /// Draw a rounded rectangle using SDF-based rendering.
    ///
    /// Each corner is a quarter ellipse `(h, v)`; the interior is painted as
    /// horizontal bands split wherever a corner starts or ends, so no two
    /// quads overlap whatever the four corners are (a translucent fill must
    /// not be painted twice).
    fn draw_rounded_rect(&mut self, rect: Rect, color: Color, radius: rustkit_layout::BorderRadius) {
        // For small radii or very small rects, fall back to solid rect
        let radius = radius.fitted(rect.width, rect.height);
        let max_radius = radius.top_left.h.max(radius.top_right.h).max(radius.bottom_left.h).max(radius.bottom_right.h);
        if max_radius < 1.0 || rect.width < 4.0 || rect.height < 4.0 {
            self.draw_solid_rect(rect, color);
            return;
        }

        // A corner under one pixel on either axis is painted square: it has
        // no pixel of its own to antialias.
        let square_if_tiny = |c: rustkit_layout::CornerRadius| {
            if c.h < 1.0 || c.v < 1.0 {
                rustkit_layout::CornerRadius::default()
            } else {
                c
            }
        };
        // Quadrant order: 0=top-left, 1=top-right, 2=bottom-right, 3=bottom-left
        let corners = [
            square_if_tiny(radius.top_left),
            square_if_tiny(radius.top_right),
            square_if_tiny(radius.bottom_right),
            square_if_tiny(radius.bottom_left),
        ];
        let [tl, tr, br, bl] = corners;

        // Interior bands between the corner boxes.
        let mut cuts = [
            0.0,
            tl.v,
            tr.v,
            rect.height - bl.v,
            rect.height - br.v,
            rect.height,
        ];
        cuts.sort_by(|a, b| a.total_cmp(b));
        for band in cuts.windows(2) {
            let (y0, y1) = (band[0], band[1]);
            if y1 <= y0 {
                continue;
            }
            let left = if y1 <= tl.v {
                tl.h
            } else if y0 >= rect.height - bl.v {
                bl.h
            } else {
                0.0
            };
            let right = if y1 <= tr.v {
                tr.h
            } else if y0 >= rect.height - br.v {
                br.h
            } else {
                0.0
            };
            if rect.width > left + right {
                self.draw_solid_rect(
                    Rect::new(
                        rect.x + left,
                        rect.y + y0,
                        rect.width - left - right,
                        y1 - y0,
                    ),
                    color,
                );
            }
        }

        // Draw corners using SDF
        for quadrant in 0..4u8 {
            self.draw_rounded_corner(rect, corners, quadrant, color);
        }
    }

    /// Smoothstep interpolation function matching WGSL's smoothstep.
    /// Performs Hermite interpolation between 0 and 1 when x is in [edge0, edge1].
    #[inline]
    fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
        let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// Check if a point is inside a rounded rectangle and return the alpha coverage.
    /// Returns 1.0 if fully inside, 0.0 if fully outside, values in between for AA at corners.
    /// Uses smoothstep for antialiasing to match the GPU shader implementation.
    #[inline]
    fn point_in_rounded_rect(
        px: f32,
        py: f32,
        rect: Rect,
        radius: rustkit_layout::BorderRadius,
    ) -> f32 {
        // Quick check: outside bounding rect
        if px < rect.x || px > rect.x + rect.width || py < rect.y || py > rect.y + rect.height {
            return 0.0;
        }

        // If no border radius, point is inside
        if radius.is_zero() {
            return 1.0;
        }

        let radius = radius.fitted(rect.width, rect.height);

        // Distance of the point from each of the rect's four edges
        let local_x = px - rect.x;
        let local_y = py - rect.y;
        let right_x = rect.width - local_x;
        let bottom_y = rect.height - local_y;

        // Each corner the point falls in: smoothstep SDF antialiasing to
        // match the GPU shader. Two diagonally opposite corners can both
        // hold the point when their boxes overlap; it must be inside both.
        let mut coverage = 1.0_f32;
        for (corner, ex, ey) in [
            (radius.top_left, local_x, local_y),
            (radius.top_right, right_x, local_y),
            (radius.bottom_right, right_x, bottom_y),
            (radius.bottom_left, local_x, bottom_y),
        ] {
            if ex < corner.h && ey < corner.v {
                let sdf = -ellipse_edge_distance(corner.h - ex, corner.v - ey, corner.h, corner.v);
                coverage = coverage.min(1.0 - Self::smoothstep(-0.5, 0.5, sdf));
            }
        }
        coverage
    }

    /// Draw one corner of a rounded rect, pixel by pixel, with anti-aliasing.
    ///
    /// `corners` are the fitted radii in quadrant order (0=top-left,
    /// 1=top-right, 2=bottom-right, 3=bottom-left). The corner's box is
    /// `h` x `v` and its curve is the quarter ellipse centred on the box's
    /// inner corner.
    fn draw_rounded_corner(
        &mut self,
        rect: Rect,
        corners: [rustkit_layout::CornerRadius; 4],
        quadrant: u8,
        color: Color,
    ) {
        let corner = corners[quadrant as usize];
        if corner.is_zero() {
            return;
        }
        let (h, v) = (corner.h, corner.v);
        let (x, y) = corner_box_origin(rect, corner, quadrant);

        // Calculate center of the corner ellipse
        let (cx, cy) = match quadrant {
            0 => (x + h, y + v), // top-left: center is inside
            1 => (x, y + v),     // top-right: center is to the left
            2 => (x, y),         // bottom-right: center is up-left
            3 => (x + h, y),     // bottom-left: center is up
            _ => return,
        };
        // Only the diagonally opposite corner's box can reach into this one
        // (adjacent corners are fitted not to).
        let opposite = (quadrant + 2) % 4;
        let overlaps_opposite = corner_boxes_overlap(rect, corner, corners[opposite as usize]);

        // Draw corner using small rectangles with AA
        let step = 1.0;
        let columns = corner_cells(x, h, quadrant == 0 || quadrant == 3);
        for (py, cell_y, cell_h) in corner_cells(y, v, quadrant == 0 || quadrant == 1) {
            for &(px, cell_x, cell_w) in &columns {
                // Calculate distance from pixel center to corner center
                let dx = match quadrant {
                    0 | 3 => cx - (px + step / 2.0), // left corners: measure from right edge
                    _ => (px + step / 2.0) - cx,    // right corners: measure from left edge
                };
                let dy = match quadrant {
                    0 | 1 => cy - (py + step / 2.0), // top corners: measure from bottom edge
                    _ => (py + step / 2.0) - cy,    // bottom corners: measure from top edge
                };

                // Use signed distance field for anti-aliasing
                // Distance to edge (positive = inside, negative = outside)
                let signed_dist = ellipse_edge_distance(dx, dy, h, v);
                let mut coverage = corner_coverage(signed_dist);

                if overlaps_opposite {
                    let (pcx, pcy) = (px + step / 2.0, py + step / 2.0);
                    match corner_distance_at(rect, corners[opposite as usize], opposite, pcx, pcy) {
                        // The pixel is in both boxes. The lower quadrant
                        // paints it, with the smaller of the two coverages.
                        Some(_) if opposite < quadrant => coverage = 0.0,
                        Some(other) => coverage = coverage.min(corner_coverage(other)),
                        None => {}
                    }
                }

                let cell = Rect::new(cell_x, cell_y, cell_w, cell_h);
                if coverage >= 1.0 {
                    // Fully inside
                    self.draw_solid_rect(cell, color);
                } else if coverage > 0.01 {
                    // Edge pixel - apply anti-aliasing
                    let aa_color = Color::new(color.r, color.g, color.b, color.a * coverage);
                    self.draw_solid_rect(cell, aa_color);
                }
                // else: outside, don't draw
            }
        }
    }

    /// Draw a filled circle using triangle fan.
    fn draw_fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: Color) {
        if radius <= 0.0 {
            return;
        }

        // Determine number of segments based on radius for smooth appearance
        let segments = ((radius / 2.0).sqrt() * 8.0).round().max(16.0).min(64.0) as u32;

        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];

        let base = self.color_vertices.len() as u32;

        // Center vertex
        let (center_x, center_y) = self.transform_point(cx, cy);
        self.color_vertices.push(ColorVertex {
            position: [center_x, center_y],
            color: c,
        });

        // Generate vertices around the circumference
        use std::f32::consts::PI;
        for i in 0..=segments {
            let angle = 2.0 * PI * (i as f32) / (segments as f32);
            let px = cx + radius * angle.cos();
            let py = cy + radius * angle.sin();
            let (x, y) = self.transform_point(px, py);
            self.color_vertices.push(ColorVertex {
                position: [x, y],
                color: c,
            });
        }

        // Generate triangle fan indices
        for i in 0..segments {
            self.color_indices.extend_from_slice(&[
                base,         // Center
                base + i + 1, // Current point on circumference
                base + i + 2, // Next point on circumference
            ]);
        }
    }

    /// Draw an annulus between `outer` and `inner` radii as a triangle strip.
    /// `inner` of zero degrades to a plain disc.
    fn draw_ring(&mut self, cx: f32, cy: f32, outer: f32, inner: f32, color: Color) {
        if outer <= 0.0 {
            return;
        }
        if inner <= 0.0 {
            self.draw_fill_circle(cx, cy, outer, color);
            return;
        }

        let segments = ((outer / 2.0).sqrt() * 8.0).round().max(16.0).min(64.0) as u32;
        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];
        let base = self.color_vertices.len() as u32;

        use std::f32::consts::PI;
        // Vertex pairs (outer, inner) around the circumference, closed by
        // repeating the first pair at i == segments.
        for i in 0..=segments {
            let angle = 2.0 * PI * (i as f32) / (segments as f32);
            let (cos, sin) = (angle.cos(), angle.sin());
            let (ox, oy) = self.transform_point(cx + outer * cos, cy + outer * sin);
            let (ix, iy) = self.transform_point(cx + inner * cos, cy + inner * sin);
            self.color_vertices.push(ColorVertex { position: [ox, oy], color: c });
            self.color_vertices.push(ColorVertex { position: [ix, iy], color: c });
        }
        for i in 0..segments {
            let o0 = base + 2 * i;
            let i0 = o0 + 1;
            let o1 = o0 + 2;
            let i1 = o0 + 3;
            self.color_indices
                .extend_from_slice(&[o0, i0, o1, i0, i1, o1]);
        }
    }

    /// Draw a filled ellipse using triangle fan.
    fn draw_fill_ellipse(&mut self, rect: Rect, color: Color) {
        let cx = rect.x + rect.width / 2.0;
        let cy = rect.y + rect.height / 2.0;
        let rx = rect.width / 2.0;
        let ry = rect.height / 2.0;

        if rx <= 0.0 || ry <= 0.0 {
            return;
        }

        // Use average of radii to determine segment count
        let avg_radius = (rx + ry) / 2.0;
        let segments = ((avg_radius / 2.0).sqrt() * 8.0).round().max(16.0).min(64.0) as u32;

        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];

        let base = self.color_vertices.len() as u32;

        // Center vertex
        let (center_x, center_y) = self.transform_point(cx, cy);
        self.color_vertices.push(ColorVertex {
            position: [center_x, center_y],
            color: c,
        });

        // Generate vertices around the ellipse using parametric equations
        use std::f32::consts::PI;
        for i in 0..=segments {
            let angle = 2.0 * PI * (i as f32) / (segments as f32);
            let px = cx + rx * angle.cos();
            let py = cy + ry * angle.sin();
            let (x, y) = self.transform_point(px, py);
            self.color_vertices.push(ColorVertex {
                position: [x, y],
                color: c,
            });
        }

        // Generate triangle fan indices
        for i in 0..segments {
            self.color_indices.extend_from_slice(&[
                base,         // Center
                base + i + 1, // Current point on ellipse
                base + i + 2, // Next point on ellipse
            ]);
        }
    }

    /// Draw a border.
    fn draw_border(&mut self, rect: Rect, color: Color, top: f32, right: f32, bottom: f32, left: f32) {
        // Top border
        if top > 0.0 {
            self.draw_solid_rect(
                Rect::new(rect.x, rect.y, rect.width, top),
                color,
            );
        }

        // Right border
        if right > 0.0 {
            self.draw_solid_rect(
                Rect::new(rect.x + rect.width - right, rect.y + top, right, rect.height - top - bottom),
                color,
            );
        }

        // Bottom border
        if bottom > 0.0 {
            self.draw_solid_rect(
                Rect::new(rect.x, rect.y + rect.height - bottom, rect.width, bottom),
                color,
            );
        }

        // Left border
        if left > 0.0 {
            self.draw_solid_rect(
                Rect::new(rect.x, rect.y + top, left, rect.height - top - bottom),
                color,
            );
        }
    }
    
    /// Draw solid borders whose corners are rounded.
    ///
    /// `widths`/`colors` are `[top, right, bottom, left]`. Radii are fitted
    /// exactly as `draw_rounded_rect` fits the background, so the ring and
    /// the fill it sits on share one outer curve. Each corner box is
    /// `max(radius, side width)` on each axis and is painted per pixel:
    /// coverage = outer curve − inner (padding-edge) curve, where the outer
    /// curve is the corner's ellipse `(h, v)` and the inner one the ellipse
    /// `(h − vertical width, v − horizontal width)` about the same centre (CSS Backgrounds 3 §5.2), square when either
    /// is ≤ 0. Between corner boxes each side is a plain strip. A corner
    /// pixel takes the colour of the side on its half of the line from the
    /// outer corner to the inner corner.
    fn draw_rounded_border(
        &mut self,
        rect: Rect,
        widths: [f32; 4],
        colors: [Color; 4],
        radius: rustkit_layout::BorderRadius,
    ) {
        let [t, r, b, l] = widths;
        if rect.width < 4.0 || rect.height < 4.0 {
            let sides = [
                (t, colors[0], Rect::new(rect.x, rect.y, rect.width, t)),
                (r, colors[1], Rect::new(rect.x + rect.width - r, rect.y, r, rect.height)),
                (b, colors[2], Rect::new(rect.x, rect.y + rect.height - b, rect.width, b)),
                (l, colors[3], Rect::new(rect.x, rect.y, l, rect.height)),
            ];
            for (w, c, s) in sides {
                if w > 0.0 {
                    self.draw_solid_rect(s, c);
                }
            }
            return;
        }

        let radius = radius.fitted(rect.width, rect.height);
        let half_w = rect.width / 2.0;
        let half_h = rect.height / 2.0;
        // (radii, vertical side width, horizontal side width,
        //  vertical colour, horizontal colour, corner index)
        let corners = [
            (radius.top_left, l, t, colors[3], colors[0], 0u8),
            (radius.top_right, r, t, colors[1], colors[0], 1u8),
            (radius.bottom_right, r, b, colors[1], colors[2], 2u8),
            (radius.bottom_left, l, b, colors[3], colors[2], 3u8),
        ];
        // Corner box extents: width along x, height along y. A side wider
        // than half the box stops at the middle; a fitted radius is already
        // kept clear of its neighbour.
        let cw = |rad: rustkit_layout::CornerRadius, vw: f32| rad.h.max(vw.min(half_w));
        let ch = |rad: rustkit_layout::CornerRadius, hw: f32| rad.v.max(hw.min(half_h));
        let (tl_w, tl_h) = (cw(corners[0].0, l), ch(corners[0].0, t));
        let (tr_w, tr_h) = (cw(corners[1].0, r), ch(corners[1].0, t));
        let (br_w, br_h) = (cw(corners[2].0, r), ch(corners[2].0, b));
        let (bl_w, bl_h) = (cw(corners[3].0, l), ch(corners[3].0, b));

        // Straight strips between the corner boxes.
        let right = rect.x + rect.width;
        let bottom = rect.y + rect.height;
        if t > 0.0 && rect.width > tl_w + tr_w {
            self.draw_solid_rect(
                Rect::new(rect.x + tl_w, rect.y, rect.width - tl_w - tr_w, t),
                colors[0],
            );
        }
        if b > 0.0 && rect.width > bl_w + br_w {
            self.draw_solid_rect(
                Rect::new(rect.x + bl_w, bottom - b, rect.width - bl_w - br_w, b),
                colors[2],
            );
        }
        if l > 0.0 && rect.height > tl_h + bl_h {
            self.draw_solid_rect(
                Rect::new(rect.x, rect.y + tl_h, l, rect.height - tl_h - bl_h),
                colors[3],
            );
        }
        if r > 0.0 && rect.height > tr_h + br_h {
            self.draw_solid_rect(
                Rect::new(right - r, rect.y + tr_h, r, rect.height - tr_h - br_h),
                colors[1],
            );
        }

        let boxes = [(tl_w, tl_h), (tr_w, tr_h), (br_w, br_h), (bl_w, bl_h)];
        for ((rad, vw, hw, vcol, hcol, q), (bw, bh)) in corners.into_iter().zip(boxes) {
            if bw <= 0.0 || bh <= 0.0 {
                continue;
            }
            let box_x = if q == 0 || q == 3 { rect.x } else { right - bw };
            let box_y = if q == 0 || q == 1 { rect.y } else { bottom - bh };
            let (rx, ry) = (rad.h - vw, rad.v - hw);
            let columns = corner_cells(box_x, bw, q == 0 || q == 3);
            for (_, py, ph) in corner_cells(box_y, bh, q == 0 || q == 1) {
                for &(_, px, pw) in &columns {
                    let (cx, cy) = (px + pw * 0.5, py + ph * 0.5);
                    // Distances from the corner's two outer edges.
                    let ex = if q == 0 || q == 3 { cx - rect.x } else { right - cx };
                    let ey = if q == 0 || q == 1 { cy - rect.y } else { bottom - cy };

                    let outer = if !rad.is_zero() && ex < rad.h && ey < rad.v {
                        let d = ellipse_edge_distance(rad.h - ex, rad.v - ey, rad.h, rad.v);
                        (d * 0.5 + 0.5).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    let inner = if rx > 0.0 && ry > 0.0 && ex < rad.h && ey < rad.v {
                        let k = (((rad.h - ex) / rx).powi(2) + ((rad.v - ey) / ry).powi(2)).sqrt();
                        ((1.0 - k) * rx.min(ry) * 0.5 + 0.5).clamp(0.0, 1.0)
                    } else {
                        (ex - vw + 0.5).clamp(0.0, 1.0) * (ey - hw + 0.5).clamp(0.0, 1.0)
                    };
                    let coverage = (outer - inner).clamp(0.0, 1.0);
                    if coverage > 0.01 {
                        // Horizontal side owns the pixel when it lies on the
                        // edge side of the outer→inner corner diagonal.
                        let horizontal = vw <= 0.0 || (hw > 0.0 && ey * vw < ex * hw);
                        let c = if horizontal { hcol } else { vcol };
                        self.draw_solid_rect(
                            Rect::new(px, py, pw, ph),
                            Color::new(c.r, c.g, c.b, c.a * coverage),
                        );
                    }
                }
            }
        }
    }

    /// Draw a box shadow.
    /// 
    /// For now, this uses a simplified approach:
    /// - Outer shadows: semi-transparent rectangles with increasing offsets,
    ///   clipped to outside the border box (`outer_shadow_paint_rects`)
    /// - Inset shadows: Draw gradient-like rectangles inside the box
    ///
    /// `radius` is the box's border-box corner radii. An outer shadow takes
    /// its shape from them; an inset shadow does not use them yet.
    #[allow(clippy::too_many_arguments)]
    fn draw_box_shadow(
        &mut self,
        rect: Rect,
        radius: rustkit_layout::BorderRadius,
        offset_x: f32,
        offset_y: f32,
        blur_radius: f32,
        spread_radius: f32,
        color: Color,
        inset: bool,
    ) {
        if color.a == 0.0 {
            return;
        }
        
        // Calculate shadow rectangle
        let shadow_rect = if inset {
            // Inset shadow is inside the box
            Rect::new(
                rect.x + offset_x.max(0.0),
                rect.y + offset_y.max(0.0),
                rect.width - spread_radius * 2.0 - offset_x.abs(),
                rect.height - spread_radius * 2.0 - offset_y.abs(),
            )
        } else {
            // Outer shadow is outside the box
            Rect::new(
                rect.x + offset_x - spread_radius,
                rect.y + offset_y - spread_radius,
                rect.width + spread_radius * 2.0,
                rect.height + spread_radius * 2.0,
            )
        };
        
        if shadow_rect.width <= 0.0 || shadow_rect.height <= 0.0 {
            return;
        }

        if !inset {
            let shadow_radius = radius.spread(spread_radius);
            for (r, alpha) in Self::outer_shadow_paint_rects(
                rect,
                radius,
                shadow_rect,
                shadow_radius,
                blur_radius,
                color.a,
            ) {
                self.draw_solid_rect(r, Color::new(color.r, color.g, color.b, alpha));
            }
            return;
        }

        // Inset: for blur, draw multiple layers with decreasing opacity, shrinking
        // inward. This is a simplified approximation - real blur would use GPU shaders
        if blur_radius > 0.0 {
            let steps = (blur_radius / 2.0).ceil().max(1.0) as u32;
            let step_size = blur_radius / steps as f32;

            for i in 0..steps {
                let layer = steps - i; // Draw outer layers first
                let expansion = step_size * layer as f32;
                let layer_alpha = color.a / (steps as f32 * 1.5); // Fade out

                let layer_rect = Rect::new(
                    shadow_rect.x + expansion,
                    shadow_rect.y + expansion,
                    shadow_rect.width - expansion * 2.0,
                    shadow_rect.height - expansion * 2.0,
                );

                if layer_rect.width > 0.0 && layer_rect.height > 0.0 {
                    let layer_color = Color::new(color.r, color.g, color.b, layer_alpha);
                    self.draw_solid_rect(layer_rect, layer_color);
                }
            }
        } else {
            // No blur - just draw solid shadow
            self.draw_solid_rect(shadow_rect, color);
        }
    }

    /// The rects (with alpha) that paint an outer box shadow. `shadow_rect`
    /// is the border box moved by the offset and grown by the spread; blur
    /// is approximated by expanding layers, outermost first. Each layer is
    /// clipped to outside the border box (CSS Backgrounds 3 §7.1), so a
    /// transparent box shows what is behind it, not its own shadow.
    ///
    /// `box_radius` is the border box's corner radii and `shadow_radius` the
    /// shadow shape's (the box's, spread). The hole follows the box's curve
    /// and each layer its own, grown with the layer; a square box gets
    /// exactly the rects it always did.
    fn outer_shadow_paint_rects(
        border_box: Rect,
        box_radius: rustkit_layout::BorderRadius,
        shadow_rect: Rect,
        shadow_radius: rustkit_layout::BorderRadius,
        blur_radius: f32,
        alpha: f32,
    ) -> Vec<(Rect, f32)> {
        let mut layers = Vec::new();
        if blur_radius > 0.0 {
            let steps = (blur_radius / 2.0).ceil().max(1.0) as u32;
            let step_size = blur_radius / steps as f32;
            for i in 0..steps {
                let expansion = step_size * (steps - i) as f32;
                layers.push((
                    Rect::new(
                        shadow_rect.x - expansion,
                        shadow_rect.y - expansion,
                        shadow_rect.width + expansion * 2.0,
                        shadow_rect.height + expansion * 2.0,
                    ),
                    shadow_radius.spread(expansion),
                    alpha / (steps as f32 * 1.5),
                ));
            }
        } else {
            layers.push((shadow_rect, shadow_radius, alpha));
        }
        layers
            .into_iter()
            .flat_map(|(r, radius, a)| {
                rounded_difference_pieces(r, radius, border_box, box_radius)
                    .into_iter()
                    .map(move |(p, coverage)| (p, a * coverage))
            })
            .collect()
    }

    /// `a` minus `b`, as up to four disjoint rects: full-width bands above
    /// and below `b`, then the left and right pieces beside it.
    fn rect_minus(a: Rect, b: Rect) -> Vec<Rect> {
        let Some(hole) = a.intersect(&b) else {
            return vec![a];
        };
        let (a_right, a_bottom) = (a.x + a.width, a.y + a.height);
        let (h_right, h_bottom) = (hole.x + hole.width, hole.y + hole.height);
        [
            Rect::new(a.x, a.y, a.width, hole.y - a.y),
            Rect::new(a.x, h_bottom, a.width, a_bottom - h_bottom),
            Rect::new(a.x, hole.y, hole.x - a.x, hole.height),
            Rect::new(h_right, hole.y, a_right - h_right, hole.height),
        ]
        .into_iter()
        .filter(|r| r.width > 0.0 && r.height > 0.0)
        .collect()
    }

    /// Apply a backdrop filter (blur, grayscale, etc.) to pixels behind the element.
    ///
    /// ## GPU Infrastructure (Available)
    ///
    /// The following GPU compute pipeline infrastructure is in place:
    /// - `backdrop_filter_pipelines`: Compute pipelines for blur (horizontal/vertical) and color filters
    /// - `create_filter_texture()`: Creates storage textures for compute operations
    /// - `run_blur_compute()`: Executes Gaussian blur via 2-pass separable filter
    /// - `run_color_filter_compute()`: Executes grayscale/sepia/brightness filters
    ///
    /// ## Current Limitation
    ///
    /// Full GPU backdrop filter requires render-to-texture support:
    /// 1. Rendering commands up to this point to an intermediate texture
    /// 2. Copying the backdrop region
    /// 3. Running compute shader passes
    /// 4. Drawing the filtered result
    ///
    /// The current architecture batches all commands and renders once at the end of `execute()`,
    /// making mid-frame capture non-trivial. For now, we use overlay approximations.
    ///
    /// ## Future Integration Path
    ///
    /// To enable true GPU filters:
    /// 1. Modify `execute()` to split rendering at BackdropFilter commands
    /// 2. Create intermediate render texture with `COPY_SRC` usage
    /// 3. Flush batched commands before each backdrop filter
    /// 4. Copy region, run compute, draw result, continue batching
    fn apply_backdrop_filter(
        &mut self,
        rect: Rect,
        border_radius: rustkit_layout::BorderRadius,
        filter: rustkit_css::BackdropFilter,
    ) {
        use rustkit_css::BackdropFilter;

        // Apply clipping
        let rect = if let Some(clip) = self.current_clip() {
            if let Some(clipped) = rect.intersect(&clip) {
                clipped
            } else {
                return; // Fully clipped
            }
        } else {
            rect
        };

        if rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        match filter {
            BackdropFilter::None => {}

            BackdropFilter::Blur(radius) => {
                // Proper backdrop blur requires render-to-texture and compute shaders.
                // For now, we simulate it with a semi-transparent white/gray overlay
                // which approximates the "frosted glass" effect.
                if radius > 0.0 {
                    // The heavier the blur, the more opaque the overlay (0.0-1.0 range)
                    let opacity = (radius / 20.0).min(0.5) * 0.3;
                    let overlay_color = Color::new(255, 255, 255, opacity);

                    if border_radius.is_zero() {
                        self.draw_solid_rect(rect, overlay_color);
                    } else {
                        self.draw_rounded_rect(rect, overlay_color, border_radius);
                    }
                }
            }

            BackdropFilter::Grayscale(amount) => {
                // Approximate grayscale by drawing a gray overlay
                // This isn't accurate but provides visual feedback
                if amount > 0.0 {
                    let gray_value = 128;
                    // Alpha in 0.0-1.0 range
                    let overlay_color = Color::new(gray_value, gray_value, gray_value, amount * 0.4);

                    if border_radius.is_zero() {
                        self.draw_solid_rect(rect, overlay_color);
                    } else {
                        self.draw_rounded_rect(rect, overlay_color, border_radius);
                    }
                }
            }

            BackdropFilter::Brightness(amount) => {
                // Brightness > 1.0 = lighter, < 1.0 = darker
                if amount != 1.0 {
                    let color = if amount > 1.0 {
                        // Lighten with white overlay (alpha in 0.0-1.0 range)
                        let intensity = ((amount - 1.0) * 0.4).min(0.8);
                        Color::new(255, 255, 255, intensity)
                    } else {
                        // Darken with black overlay (alpha in 0.0-1.0 range)
                        let intensity = ((1.0 - amount) * 0.8).min(0.8);
                        Color::new(0, 0, 0, intensity)
                    };

                    if border_radius.is_zero() {
                        self.draw_solid_rect(rect, color);
                    } else {
                        self.draw_rounded_rect(rect, color, border_radius);
                    }
                }
            }

            BackdropFilter::Contrast(_) => {
                // Contrast adjustment would require per-pixel operations
                // No simple overlay approximation exists
            }

            BackdropFilter::Saturate(_) => {
                // Saturation adjustment would require per-pixel color manipulation
                // No simple overlay approximation exists
            }

            BackdropFilter::Sepia(amount) => {
                // Approximate sepia with a brownish overlay (alpha in 0.0-1.0 range)
                if amount > 0.0 {
                    let sepia_color = Color::new(112, 66, 20, amount * 0.3);

                    if border_radius.is_zero() {
                        self.draw_solid_rect(rect, sepia_color);
                    } else {
                        self.draw_rounded_rect(rect, sepia_color, border_radius);
                    }
                }
            }
        }
    }

    /// Render a linear gradient using the GPU shader.
    /// This method renders the gradient immediately using a separate render pass.
    /// Note: This may cause z-ordering issues with other content.
    /// Enable via RUSTKIT_GPU_GRADIENTS=1 environment variable.
    fn render_linear_gradient_gpu(
        &self,
        target: &wgpu::TextureView,
        rect: Rect,
        angle_rad: f32,
        stops: &[(f32, rustkit_css::ColorF32)],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate repeat length for repeating gradients
        let repeat_length = if repeating && !stops.is_empty() {
            stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

        // Update gradient parameters uniform buffer
        let params = pipeline::GradientParams {
            rect_x: rect.x,
            rect_y: rect.y,
            rect_width: rect.width,
            rect_height: rect.height,
            param0: angle_rad,  // linear gradient uses angle in radians
            param1: 0.0,
            param2: 0.5,
            param3: 0.5,
            gradient_type: 0,  // 0 = linear
            repeating: if repeating { 1 } else { 0 },
            repeat_length,
            num_stops: stops.len().min(self.gradient_pipeline.max_stops) as u32,
            radius_tl: border_radius.top_left.h,
            radius_tr: border_radius.top_right.h,
            radius_br: border_radius.bottom_right.h,
            radius_bl: border_radius.bottom_left.h,
            debug_mode: std::env::var("RUSTKIT_GPU_DEBUG")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0),
            _padding0: 0,
            _padding1: 0,
            _padding2: 0,
        };

        self.queue.write_buffer(
            &self.gradient_pipeline.uniform_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        // Update color stops storage buffer
        let gpu_stops: Vec<pipeline::GradientColorStop> = stops
            .iter()
            .take(self.gradient_pipeline.max_stops)
            .map(|(pos, color)| pipeline::GradientColorStop {
                position: *pos,
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            })
            .collect();

        if !gpu_stops.is_empty() {
            self.queue.write_buffer(
                &self.gradient_pipeline.stops_buffer,
                0,
                bytemuck::cast_slice(&gpu_stops),
            );
        }

        // Create vertices for the gradient quad
        // We use ColorVertex but the fragment shader ignores the color
        let dummy_color = [0.0f32, 0.0, 0.0, 1.0];
        let vertices = [
            ColorVertex { position: [rect.x, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y + rect.height], color: dummy_color },
            ColorVertex { position: [rect.x, rect.y + rect.height], color: dummy_color },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Gradient Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Gradient Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Create command encoder and render pass
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Gradient Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GPU Gradient Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,  // Don't clear, preserve existing content
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.gradient_pipeline.pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &self.gradient_pipeline.bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Render a linear gradient using the GPU shader with optional clear.
    /// When clear_color is Some, the target is cleared before rendering.
    /// When clear_color is None, existing content is preserved (LoadOp::Load).
    fn render_linear_gradient_gpu_with_clear(
        &self,
        target: &wgpu::TextureView,
        rect: Rect,
        angle_rad: f32,
        stops: &[(f32, rustkit_css::ColorF32)],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
        clear_color: Option<wgpu::Color>,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate repeat length for repeating gradients
        let repeat_length = if repeating && !stops.is_empty() {
            stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

        // Update gradient parameters uniform buffer
        let params = pipeline::GradientParams {
            rect_x: rect.x,
            rect_y: rect.y,
            rect_width: rect.width,
            rect_height: rect.height,
            param0: angle_rad,
            param1: 0.0,
            param2: 0.5,
            param3: 0.5,
            gradient_type: 0,  // 0 = linear
            repeating: if repeating { 1 } else { 0 },
            repeat_length,
            num_stops: stops.len().min(self.gradient_pipeline.max_stops) as u32,
            radius_tl: border_radius.top_left.h,
            radius_tr: border_radius.top_right.h,
            radius_br: border_radius.bottom_right.h,
            radius_bl: border_radius.bottom_left.h,
            // Debug mode: 0=normal, 1=t-value, 2=direction, 3=position, 4=coverage, 5=raw-t
            //            6=first-stop, 7=num-stops, 8=interp-color
            // Set via RUSTKIT_GPU_DEBUG=N environment variable
            debug_mode: std::env::var("RUSTKIT_GPU_DEBUG")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0),
            _padding0: 0,
            _padding1: 0,
            _padding2: 0,
        };

        // Debug output for GPU gradient parameters
        if std::env::var("RUSTKIT_GPU_TRACE").is_ok() {
            eprintln!("GPU Gradient (with_clear): rect=({:.1}, {:.1}, {:.1}, {:.1}), angle={:.2}rad, num_stops={}, repeating={}",
                params.rect_x, params.rect_y, params.rect_width, params.rect_height,
                params.param0, params.num_stops, params.repeating);
            for (i, (pos, color)) in stops.iter().enumerate() {
                eprintln!("  Stop {}: pos={:.3}, RGBA=({:.3}, {:.3}, {:.3}, {:.3})",
                    i, pos, color.r, color.g, color.b, color.a);
            }
        }

        self.queue.write_buffer(
            &self.gradient_pipeline.uniform_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        // Update color stops storage buffer
        let gpu_stops: Vec<pipeline::GradientColorStop> = stops
            .iter()
            .take(self.gradient_pipeline.max_stops)
            .map(|(pos, color)| pipeline::GradientColorStop {
                position: *pos,
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            })
            .collect();

        if !gpu_stops.is_empty() {
            self.queue.write_buffer(
                &self.gradient_pipeline.stops_buffer,
                0,
                bytemuck::cast_slice(&gpu_stops),
            );
        }

        // Create vertices for the gradient quad
        let dummy_color = [0.0f32, 0.0, 0.0, 1.0];

        let vertices = [
            ColorVertex { position: [rect.x, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y + rect.height], color: dummy_color },
            ColorVertex { position: [rect.x, rect.y + rect.height], color: dummy_color },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Gradient Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Gradient Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Create command encoder and render pass
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Gradient Encoder (with_clear)"),
        });

        let load_op = match clear_color {
            Some(color) => wgpu::LoadOp::Clear(color),
            None => wgpu::LoadOp::Load,
        };

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GPU Gradient Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.gradient_pipeline.pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &self.gradient_pipeline.bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Render a radial gradient using the GPU shader.
    fn render_radial_gradient_gpu(
        &self,
        target: &wgpu::TextureView,
        rect: Rect,
        rx: f32,
        ry: f32,
        center: (f32, f32),
        stops: &[(f32, rustkit_css::ColorF32)],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate repeat length for repeating gradients
        let repeat_length = if repeating && !stops.is_empty() {
            stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

        // Update gradient parameters uniform buffer
        let params = pipeline::GradientParams {
            rect_x: rect.x,
            rect_y: rect.y,
            rect_width: rect.width,
            rect_height: rect.height,
            param0: rx,  // radial: x radius in pixels
            param1: ry,  // radial: y radius in pixels
            param2: center.0,  // radial: center x (0-1)
            param3: center.1,  // radial: center y (0-1)
            gradient_type: 1,  // 1 = radial
            repeating: if repeating { 1 } else { 0 },
            repeat_length,
            num_stops: stops.len().min(self.gradient_pipeline.max_stops) as u32,
            radius_tl: border_radius.top_left.h,
            radius_tr: border_radius.top_right.h,
            radius_br: border_radius.bottom_right.h,
            radius_bl: border_radius.bottom_left.h,
            debug_mode: std::env::var("RUSTKIT_GPU_DEBUG")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0),
            _padding0: 0,
            _padding1: 0,
            _padding2: 0,
        };

        self.queue.write_buffer(
            &self.gradient_pipeline.uniform_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        // Update color stops storage buffer
        let gpu_stops: Vec<pipeline::GradientColorStop> = stops
            .iter()
            .take(self.gradient_pipeline.max_stops)
            .map(|(pos, color)| pipeline::GradientColorStop {
                position: *pos,
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            })
            .collect();

        if !gpu_stops.is_empty() {
            self.queue.write_buffer(
                &self.gradient_pipeline.stops_buffer,
                0,
                bytemuck::cast_slice(&gpu_stops),
            );
        }

        // Create vertices for the gradient quad
        let dummy_color = [0.0f32, 0.0, 0.0, 1.0];
        let vertices = [
            ColorVertex { position: [rect.x, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y + rect.height], color: dummy_color },
            ColorVertex { position: [rect.x, rect.y + rect.height], color: dummy_color },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Radial Gradient Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Radial Gradient Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Create command encoder and render pass (LoadOp::Load to preserve existing content)
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Radial Gradient Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GPU Radial Gradient Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.gradient_pipeline.pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &self.gradient_pipeline.bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Render a conic gradient using the GPU shader.
    fn render_conic_gradient_gpu(
        &self,
        target: &wgpu::TextureView,
        rect: Rect,
        from_angle_rad: f32,
        center: (f32, f32),
        stops: &[(f32, rustkit_css::ColorF32)],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate repeat length for repeating gradients
        let repeat_length = if repeating && !stops.is_empty() {
            stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

        // Update gradient parameters uniform buffer
        let params = pipeline::GradientParams {
            rect_x: rect.x,
            rect_y: rect.y,
            rect_width: rect.width,
            rect_height: rect.height,
            param0: from_angle_rad,  // conic: starting angle in radians
            param1: 0.0,
            param2: center.0,  // conic: center x (0-1)
            param3: center.1,  // conic: center y (0-1)
            gradient_type: 2,  // 2 = conic
            repeating: if repeating { 1 } else { 0 },
            repeat_length,
            num_stops: stops.len().min(self.gradient_pipeline.max_stops) as u32,
            radius_tl: border_radius.top_left.h,
            radius_tr: border_radius.top_right.h,
            radius_br: border_radius.bottom_right.h,
            radius_bl: border_radius.bottom_left.h,
            debug_mode: std::env::var("RUSTKIT_GPU_DEBUG")
                .ok()
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0),
            _padding0: 0,
            _padding1: 0,
            _padding2: 0,
        };

        self.queue.write_buffer(
            &self.gradient_pipeline.uniform_buffer,
            0,
            bytemuck::cast_slice(&[params]),
        );

        // Update color stops storage buffer
        let gpu_stops: Vec<pipeline::GradientColorStop> = stops
            .iter()
            .take(self.gradient_pipeline.max_stops)
            .map(|(pos, color)| pipeline::GradientColorStop {
                position: *pos,
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            })
            .collect();

        if !gpu_stops.is_empty() {
            self.queue.write_buffer(
                &self.gradient_pipeline.stops_buffer,
                0,
                bytemuck::cast_slice(&gpu_stops),
            );
        }

        // Create vertices for the gradient quad
        let dummy_color = [0.0f32, 0.0, 0.0, 1.0];
        let vertices = [
            ColorVertex { position: [rect.x, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y], color: dummy_color },
            ColorVertex { position: [rect.x + rect.width, rect.y + rect.height], color: dummy_color },
            ColorVertex { position: [rect.x, rect.y + rect.height], color: dummy_color },
        ];
        let indices: [u32; 6] = [0, 1, 2, 0, 2, 3];

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Conic Gradient Vertex Buffer"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });

        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Conic Gradient Index Buffer"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // Create command encoder and render pass (LoadOp::Load to preserve existing content)
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("GPU Conic Gradient Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("GPU Conic Gradient Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            render_pass.set_pipeline(&self.gradient_pipeline.pipeline);
            render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
            render_pass.set_bind_group(1, &self.gradient_pipeline.bind_group, &[]);
            render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
            render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..6, 0, 0..1);
        }

        self.queue.submit(std::iter::once(encoder.finish()));
    }

    /// Draw a linear gradient with optional border-radius clipping.
    fn draw_linear_gradient(
        &mut self,
        rect: Rect,
        direction: rustkit_css::GradientDirection,
        stops: &[rustkit_css::ColorStop],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Convert direction to angle in radians
        let angle_deg = direction.to_degrees();
        let angle_rad = angle_deg.to_radians();

        // Calculate gradient direction vector
        let (sin_a, cos_a) = (angle_rad.sin(), angle_rad.cos());

        // Calculate gradient geometry
        let half_width = rect.width / 2.0;
        let half_height = rect.height / 2.0;
        let gradient_half_length = (half_width * sin_a.abs() + half_height * cos_a.abs()).max(0.001);

        // Check if any stop uses pixel positions (for repeating gradients)
        let has_pixel_positions = stops.iter().any(|s| {
            s.position.as_ref().map(|p| p.is_pixels()).unwrap_or(false)
        });

        // For repeating gradients with pixel positions, get the repeat length in pixels
        let repeat_length_pixels = if repeating && has_pixel_positions {
            stops.last()
                .and_then(|s| s.position.as_ref())
                .map(|p| match p {
                    rustkit_css::StopPosition::Pixels(px) => *px,
                    rustkit_css::StopPosition::Percent(pct) => *pct * gradient_half_length * 2.0,
                })
                .unwrap_or(gradient_half_length * 2.0)
                .max(0.001)
        } else {
            gradient_half_length * 2.0 // Full gradient length
        };

        // For pixel-based repeating gradients, normalize stops to the repeat length (not full gradient)
        // For percentage-based gradients, normalize to 0-1
        let mut normalized_stops: Vec<(f32, rustkit_css::ColorF32)> = Vec::with_capacity(stops.len());
        for (i, stop) in stops.iter().enumerate() {
            let pos = match &stop.position {
                Some(p) => {
                    if has_pixel_positions && repeating {
                        // For pixel-based repeating gradients, normalize to repeat length
                        match p {
                            rustkit_css::StopPosition::Pixels(px) => *px / repeat_length_pixels,
                            rustkit_css::StopPosition::Percent(pct) => *pct,
                        }
                    } else {
                        // For non-repeating or percentage-based, normalize to 0-1 using gradient line
                        match p {
                            rustkit_css::StopPosition::Percent(pct) => *pct,
                            rustkit_css::StopPosition::Pixels(px) => *px / (gradient_half_length * 2.0),
                        }
                    }
                }
                None => {
                    // Auto-position: distribute evenly
                    if stops.len() == 1 {
                        0.5
                    } else {
                        i as f32 / (stops.len() - 1) as f32
                    }
                }
            };
            normalized_stops.push((pos, rustkit_css::ColorF32::from_color(stop.color)));
        }

        // For repeating gradients, the repeat length is 1.0 (since we normalized stops to it)
        // For non-repeating, use the last stop position
        let repeat_length = if repeating {
            1.0 // Stops are already normalized to repeat length
        } else {
            normalized_stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        };

        // GPU gradient path: queue for deferred rendering
        // Enable via RUSTKIT_GPU_GRADIENTS=1 environment variable
        if self.gpu_gradients_enabled {
            self.gradient_queue.push(QueuedLinearGradient {
                rect,
                angle_rad,
                stops: normalized_stops,
                repeating,
                border_radius,
            });
            return; // GPU will render during flush_to
        }

        // CPU path: cell-by-cell rendering (default)

        // Helper to apply repeating logic to t value
        let apply_t = |t: f32| -> f32 {
            if repeating {
                // Scale t to repeat length and use modulo for repeating
                (t.rem_euclid(repeat_length)).min(repeat_length)
            } else {
                t.clamp(0.0, 1.0)
            }
        };

        // Check for axis-aligned gradients (more efficient rendering)
        let is_horizontal = (angle_deg - 90.0).abs() < 0.1 || (angle_deg - 270.0).abs() < 0.1;
        let is_vertical = angle_deg.abs() < 0.1 || (angle_deg - 180.0).abs() < 0.1;
        let has_radius = !border_radius.is_zero();

        // If we have border-radius, we need cell-by-cell rendering for proper clipping
        if !has_radius && is_horizontal {
            // Horizontal gradient (left to right or right to left) - fast path
            let reverse = angle_deg > 180.0;
            let step_count = rect.width.max(2.0) as usize;
            let strip_width = rect.width / step_count as f32;

            let vp_w = self.viewport_size.0 as f32;
            let (first, last) = self.visible_strip_range(rect.x, rect.width, step_count, vp_w);
            for i in first..last {
                let t = if reverse {
                    1.0 - (i as f32 + 0.5) / step_count as f32
                } else {
                    (i as f32 + 0.5) / step_count as f32
                };
                let t_final = apply_t(t);
                let color = Self::interpolate_color_f32(&normalized_stops, t_final);
                let x_pos = rect.x + i as f32 * strip_width;
                self.draw_solid_rect_f32(Rect::new(x_pos, rect.y, strip_width + 0.5, rect.height), color);
            }
        } else if !has_radius && is_vertical {
            // Vertical gradient (top to bottom or bottom to top) - fast path
            let reverse = angle_deg < 90.0 || angle_deg > 270.0;
            let step_count = rect.height.max(2.0) as usize;
            let strip_height = rect.height / step_count as f32;

            let vp_h = self.viewport_size.1 as f32;
            let (first, last) = self.visible_strip_range(rect.y, rect.height, step_count, vp_h);
            for i in first..last {
                let t = if reverse {
                    1.0 - (i as f32 + 0.5) / step_count as f32
                } else {
                    (i as f32 + 0.5) / step_count as f32
                };
                let t_final = apply_t(t);
                let color = Self::interpolate_color_f32(&normalized_stops, t_final);
                let y_pos = rect.y + i as f32 * strip_height;
                self.draw_solid_rect_f32(Rect::new(rect.x, y_pos, rect.width, strip_height + 0.5), color);
            }
        } else {
            // Diagonal gradient or gradient with border-radius - cell-by-cell rendering
            // Uses the CSS gradient spec algorithm for proper corner-to-corner diagonal
            // (half_width, half_height, gradient_half_length are calculated at function start)

            // Adaptive step sizing to prevent GPU buffer overflow for large gradients
            // while maintaining 1px quality for small UI elements
            let area = rect.width * rect.height;
            let max_cells: f32 = 100_000.0; // Limit cells to prevent buffer overflow
            let cell_size: f32 = if area > max_cells {
                (area / max_cells).sqrt().ceil()
            } else {
                1.0
            };
            let cols = (rect.width / cell_size).ceil() as usize;
            let rows = (rect.height / cell_size).ceil() as usize;

            let center_x = rect.x + half_width;
            let center_y = rect.y + half_height;

            let (vp_w, vp_h) = (self.viewport_size.0 as f32, self.viewport_size.1 as f32);
            let (row_first, row_last) = self.visible_cell_range(rect.y, rect.height, cell_size, rows, vp_h);
            let (col_first, col_last) = self.visible_cell_range(rect.x, rect.width, cell_size, cols, vp_w);

            for row in row_first..row_last {
                for col in col_first..col_last {
                    let cell_x = rect.x + col as f32 * cell_size;
                    let cell_y = rect.y + row as f32 * cell_size;
                    let cell_center_x = cell_x + cell_size * 0.5;
                    let cell_center_y = cell_y + cell_size * 0.5;

                    // Check bounds
                    if cell_x >= rect.x + rect.width || cell_y >= rect.y + rect.height {
                        continue;
                    }

                    // Check border-radius clipping
                    if has_radius {
                        let coverage = Self::point_in_rounded_rect(cell_center_x, cell_center_y, rect, border_radius);
                        if coverage <= 0.0 {
                            continue; // Skip cells outside the rounded corners
                        }
                    }

                    // Position relative to rect center
                    let px = cell_center_x - center_x;
                    let py = cell_center_y - center_y;

                    // Project onto gradient direction (sin_a, -cos_a)
                    // projection ranges from -gradient_half_length to +gradient_half_length
                    let projection = px * sin_a + py * (-cos_a);

                    // Calculate t value
                    let t = if repeating && has_pixel_positions {
                        // For pixel-based repeating gradients, the 0 position is at the center
                        // of the gradient line, and the pattern repeats in both directions.
                        // projection is already centered at 0, so use it directly.
                        projection / repeat_length_pixels
                    } else {
                        // For non-repeating or percentage-based, normalize to 0-1
                        (projection / gradient_half_length + 1.0) / 2.0
                    };
                    let t_final = apply_t(t);

                    let mut color = Self::interpolate_color_f32(&normalized_stops, t_final);

                    // Apply alpha coverage for antialiased edges at rounded corners
                    if has_radius {
                        let coverage = Self::point_in_rounded_rect(cell_center_x, cell_center_y, rect, border_radius);
                        if coverage < 1.0 {
                            color = rustkit_css::ColorF32::new(color.r, color.g, color.b, color.a * coverage);
                        }
                    }

                    // Clamp cell to rect bounds
                    let cell_w = cell_size.min(rect.x + rect.width - cell_x);
                    let cell_h = cell_size.min(rect.y + rect.height - cell_y);

                    self.draw_solid_rect_f32(Rect::new(cell_x, cell_y, cell_w, cell_h), color);
                }
            }
        }
    }
    
    /// Draw a radial gradient with optional border-radius clipping.
    fn draw_radial_gradient(
        &mut self,
        rect: Rect,
        shape: rustkit_css::RadialShape,
        size: rustkit_css::RadialSize,
        center: (f32, f32),
        stops: &[rustkit_css::ColorStop],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate center position in pixels
        let cx = rect.x + rect.width * center.0;
        let cy = rect.y + rect.height * center.1;
        
        // Calculate radius based on size keyword
        let (rx, ry) = match size {
            rustkit_css::RadialSize::ClosestSide => {
                let dx = center.0.min(1.0 - center.0) * rect.width;
                let dy = center.1.min(1.0 - center.1) * rect.height;
                match shape {
                    rustkit_css::RadialShape::Circle => (dx.min(dy), dx.min(dy)),
                    rustkit_css::RadialShape::Ellipse => (dx, dy),
                }
            }
            rustkit_css::RadialSize::FarthestSide => {
                let dx = center.0.max(1.0 - center.0) * rect.width;
                let dy = center.1.max(1.0 - center.1) * rect.height;
                match shape {
                    rustkit_css::RadialShape::Circle => (dx.max(dy), dx.max(dy)),
                    rustkit_css::RadialShape::Ellipse => (dx, dy),
                }
            }
            rustkit_css::RadialSize::ClosestCorner => {
                // Distance to closest corner
                let corners = [
                    (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)
                ];
                let mut min_dist = f32::INFINITY;
                for (cx_frac, cy_frac) in corners {
                    let dx = (cx_frac - center.0).abs() * rect.width;
                    let dy = (cy_frac - center.1).abs() * rect.height;
                    let dist = (dx * dx + dy * dy).sqrt();
                    min_dist = min_dist.min(dist);
                }
                match shape {
                    rustkit_css::RadialShape::Circle => (min_dist, min_dist),
                    rustkit_css::RadialShape::Ellipse => {
                        // css-images-3 §3.3.3: side distances scaled by
                        // sqrt(2) — see the GPU path for the derivation.
                        let dx = center.0.min(1.0 - center.0) * rect.width;
                        let dy = center.1.min(1.0 - center.1) * rect.height;
                        (dx * std::f32::consts::SQRT_2, dy * std::f32::consts::SQRT_2)
                    }
                }
            }
            rustkit_css::RadialSize::FarthestCorner => {
                // Distance to farthest corner
                let corners = [
                    (0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)
                ];
                let mut max_dist = 0.0f32;
                for (cx_frac, cy_frac) in corners {
                    let dx = (cx_frac - center.0).abs() * rect.width;
                    let dy = (cy_frac - center.1).abs() * rect.height;
                    let dist = (dx * dx + dy * dy).sqrt();
                    max_dist = max_dist.max(dist);
                }
                match shape {
                    rustkit_css::RadialShape::Circle => (max_dist, max_dist),
                    rustkit_css::RadialShape::Ellipse => {
                        // css-images-3 §3.3.3: side distances scaled by
                        // sqrt(2) — see the GPU path for the derivation.
                        let dx = center.0.max(1.0 - center.0) * rect.width;
                        let dy = center.1.max(1.0 - center.1) * rect.height;
                        (dx * std::f32::consts::SQRT_2, dy * std::f32::consts::SQRT_2)
                    }
                }
            }
            rustkit_css::RadialSize::Explicit(r1, r2) => (r1, r2),
        };

        // Radial gradient line length is the maximum radius
        let radial_gradient_length = rx.max(ry);

        // Check if any stop uses pixel positions
        let has_pixel_positions = stops.iter().any(|s| {
            s.position.as_ref().map(|p| p.is_pixels()).unwrap_or(false)
        });

        // Normalize color stops using high-precision colors
        // For pixel positions, convert to normalized using the radial gradient length
        let mut normalized_stops: Vec<(f32, rustkit_css::ColorF32)> = Vec::with_capacity(stops.len());
        for (i, stop) in stops.iter().enumerate() {
            let pos = match &stop.position {
                Some(p) => p.to_normalized(radial_gradient_length),
                None => {
                    // Auto-position: distribute evenly
                    if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
                }
            };
            normalized_stops.push((pos, rustkit_css::ColorF32::from_color(stop.color)));
        }

        // For repeating gradients, calculate repeat length
        let repeat_length = if repeating && !normalized_stops.is_empty() {
            if has_pixel_positions {
                stops.last()
                    .and_then(|s| s.position.as_ref())
                    .map(|p| p.to_normalized(radial_gradient_length))
                    .unwrap_or(1.0)
                    .max(0.001)
            } else {
                normalized_stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
            }
        } else {
            1.0
        };

        // GPU radial gradient path: queue for deferred rendering
        if self.gpu_gradients_enabled {
            self.radial_gradient_queue.push(QueuedRadialGradient {
                rect,
                rx,
                ry,
                center,
                stops: normalized_stops,
                repeating,
                border_radius,
            });
            return; // GPU will render during flush_to
        }

        // CPU path: cell-by-cell rendering

        // Adaptive step sizing to prevent GPU buffer overflow for large gradients
        // while maintaining 1px quality for small UI elements
        let area = rect.width * rect.height;
        let max_cells: f32 = 100_000.0; // Limit cells to prevent buffer overflow
        let step_size: f32 = if area > max_cells {
            (area / max_cells).sqrt().ceil()
        } else {
            1.0
        };
        let (vp_w, vp_h) = (self.viewport_size.0 as f32, self.viewport_size.1 as f32);
        let rows = (rect.height / step_size).ceil().max(1.0) as usize;
        let cols = (rect.width / step_size).ceil().max(1.0) as usize;
        let (row_first, row_last) = self.visible_cell_range(rect.y, rect.height, step_size, rows, vp_h);
        let (col_first, col_last) = self.visible_cell_range(rect.x, rect.width, step_size, cols, vp_w);
        let y_end = (rect.y + row_last as f32 * step_size).min(rect.y + rect.height);
        let x_end = (rect.x + col_last as f32 * step_size).min(rect.x + rect.width);
        let mut y = rect.y + row_first as f32 * step_size;
        while y < y_end {
            let row_height = step_size.min(rect.y + rect.height - y);
            let mut x = rect.x + col_first as f32 * step_size;
            while x < x_end {
                let col_width = step_size.min(rect.x + rect.width - x);
                let cell_center_x = x + col_width / 2.0;
                let cell_center_y = y + row_height / 2.0;

                // Check border-radius clipping
                let alpha_coverage = Self::point_in_rounded_rect(
                    cell_center_x,
                    cell_center_y,
                    rect,
                    border_radius,
                );

                if alpha_coverage > 0.0 {
                    // Calculate distance from center (normalized to ellipse)
                    let dx = (cell_center_x - cx) / rx.max(0.001);
                    let dy = (cell_center_y - cy) / ry.max(0.001);
                    let t = (dx * dx + dy * dy).sqrt();

                    // Apply repeating logic
                    let t_final = if repeating {
                        t.rem_euclid(repeat_length)
                    } else {
                        t.clamp(0.0, 1.0)
                    };

                    // Get color at this distance
                    let mut color = Self::interpolate_color_f32(&normalized_stops, t_final);

                    // Apply border-radius alpha
                    if alpha_coverage < 1.0 {
                        color = rustkit_css::ColorF32::new(color.r, color.g, color.b, color.a * alpha_coverage);
                    }

                    // Only draw if not fully transparent
                    if color.a > 0.0 {
                        self.draw_solid_rect_f32(Rect::new(x, y, col_width, row_height), color);
                    }
                }

                x += step_size;
            }
            y += step_size;
        }
    }

    /// Draw a conic gradient with optional border-radius clipping.
    fn draw_conic_gradient(
        &mut self,
        rect: Rect,
        from_angle: f32,
        center: (f32, f32),
        stops: &[rustkit_css::ColorStop],
        repeating: bool,
        border_radius: rustkit_layout::BorderRadius,
    ) {
        if stops.is_empty() || rect.width <= 0.0 || rect.height <= 0.0 {
            return;
        }

        // Calculate center position in pixels
        let cx = rect.x + rect.width * center.0;
        let cy = rect.y + rect.height * center.1;

        // Convert from_angle to radians (CSS conic gradients: 0deg = up, clockwise)
        let from_rad = (from_angle - 90.0).to_radians();

        // Normalize color stops using high-precision colors
        // For conic gradients, positions are typically percentages (0-1) of the full sweep
        // Pixel positions are treated as percentages for conic gradients
        let mut normalized_stops: Vec<(f32, rustkit_css::ColorF32)> = Vec::with_capacity(stops.len());
        for (i, stop) in stops.iter().enumerate() {
            let pos = match &stop.position {
                Some(p) => {
                    // For conic gradients, use raw value as percentage
                    // (pixel positions don't make sense for conic, treat them as normalized)
                    match p {
                        rustkit_css::StopPosition::Percent(pct) => *pct,
                        rustkit_css::StopPosition::Pixels(px) => *px / 360.0, // Treat as degrees
                    }
                }
                None => {
                    if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
                }
            };
            normalized_stops.push((pos, rustkit_css::ColorF32::from_color(stop.color)));
        }

        // For repeating gradients, get the repeat length from the last stop
        let repeat_length = if repeating && !normalized_stops.is_empty() {
            normalized_stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

        // GPU conic gradient path: queue for deferred rendering
        if self.gpu_gradients_enabled {
            self.conic_gradient_queue.push(QueuedConicGradient {
                rect,
                from_angle_rad: from_rad,
                center,
                stops: normalized_stops,
                repeating,
                border_radius,
            });
            return; // GPU will render during flush_to
        }

        // CPU path: cell-by-cell rendering

        // Function to apply repeating logic to t value
        let apply_t = |t: f32| -> f32 {
            if repeating {
                t.rem_euclid(repeat_length)
            } else {
                t
            }
        };

        // Adaptive step sizing to prevent GPU buffer overflow
        let area = rect.width * rect.height;
        let max_cells: f32 = 100_000.0;
        let step_size: f32 = if area > max_cells {
            (area / max_cells).sqrt().ceil()
        } else {
            1.0
        };

        let (vp_w, vp_h) = (self.viewport_size.0 as f32, self.viewport_size.1 as f32);
        let rows = (rect.height / step_size).ceil().max(1.0) as usize;
        let cols = (rect.width / step_size).ceil().max(1.0) as usize;
        let (row_first, row_last) = self.visible_cell_range(rect.y, rect.height, step_size, rows, vp_h);
        let (col_first, col_last) = self.visible_cell_range(rect.x, rect.width, step_size, cols, vp_w);
        let y_end = (rect.y + row_last as f32 * step_size).min(rect.y + rect.height);
        let x_end = (rect.x + col_last as f32 * step_size).min(rect.x + rect.width);
        let mut y = rect.y + row_first as f32 * step_size;
        while y < y_end {
            let row_height = step_size.min(rect.y + rect.height - y);
            let mut x = rect.x + col_first as f32 * step_size;
            while x < x_end {
                let col_width = step_size.min(rect.x + rect.width - x);
                let cell_center_x = x + col_width / 2.0;
                let cell_center_y = y + row_height / 2.0;

                // Check border-radius clipping
                let alpha_coverage = Self::point_in_rounded_rect(
                    cell_center_x,
                    cell_center_y,
                    rect,
                    border_radius,
                );

                if alpha_coverage > 0.0 {
                    // Calculate angle from center
                    let dx = cell_center_x - cx;
                    let dy = cell_center_y - cy;
                    let angle = dy.atan2(dx) - from_rad;

                    // Normalize angle to 0-1 range
                    let normalized_angle = ((angle + std::f32::consts::PI) / (2.0 * std::f32::consts::PI)) % 1.0;
                    let raw_t = if normalized_angle < 0.0 { normalized_angle + 1.0 } else { normalized_angle };

                    // Apply repeating logic
                    let t = apply_t(raw_t);

                    // Get color at this angle
                    let mut color = Self::interpolate_color_f32(&normalized_stops, t);

                    // Apply border-radius alpha
                    if alpha_coverage < 1.0 {
                        color = rustkit_css::ColorF32::new(color.r, color.g, color.b, color.a * alpha_coverage);
                    }

                    if color.a > 0.0 {
                        self.draw_solid_rect_f32(Rect::new(x, y, col_width, row_height), color);
                    }
                }

                x += step_size;
            }
            y += step_size;
        }
    }

    /// Convert sRGB to linear space for interpolation.
    #[inline]
    fn srgb_to_linear(c: f32) -> f32 {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    /// Convert linear to sRGB space after interpolation.
    #[inline]
    fn linear_to_srgb(c: f32) -> f32 {
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    }

    /// Convert linear RGB to oklab color space.
    /// Returns (L, a, b) where L is lightness, a is green-red, b is blue-yellow.
    #[inline]
    fn linear_rgb_to_oklab(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
        // Convert to LMS (long, medium, short cone response)
        let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
        let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
        let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

        // Apply cube root (non-linear response)
        let l_ = l.cbrt();
        let m_ = m.cbrt();
        let s_ = s.cbrt();

        // Convert to oklab
        let ok_l = 0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_;
        let ok_a = 1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_;
        let ok_b = 0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_;

        (ok_l, ok_a, ok_b)
    }

    /// Convert oklab to linear RGB color space.
    #[inline]
    fn oklab_to_linear_rgb(ok_l: f32, ok_a: f32, ok_b: f32) -> (f32, f32, f32) {
        // Convert from oklab to LMS (cube root space)
        let l_ = ok_l + 0.3963377774 * ok_a + 0.2158037573 * ok_b;
        let m_ = ok_l - 0.1055613458 * ok_a - 0.0638541728 * ok_b;
        let s_ = ok_l - 0.0894841775 * ok_a - 1.2914855480 * ok_b;

        // Cube to get linear LMS
        let l = l_ * l_ * l_;
        let m = m_ * m_ * m_;
        let s = s_ * s_ * s_;

        // Convert LMS to linear RGB
        let r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
        let g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
        let b = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s;

        (r, g, b)
    }

    /// Interpolate between color stops using oklab color space.
    /// This provides perceptually uniform gradients but doesn't match Chrome's default.
    /// Use for CSS `linear-gradient(in oklab, ...)` when that syntax is supported.
    #[allow(dead_code)]
    fn interpolate_color_oklab(stops: &[(f32, Color)], t: f32) -> Color {
        if stops.is_empty() {
            return Color::TRANSPARENT;
        }
        if stops.len() == 1 || t <= stops[0].0 {
            return stops[0].1;
        }
        if t >= stops[stops.len() - 1].0 {
            return stops[stops.len() - 1].1;
        }

        // Find the two stops surrounding t
        for i in 0..stops.len() - 1 {
            let (pos0, color0) = stops[i];
            let (pos1, color1) = stops[i + 1];
            if t >= pos0 && t <= pos1 {
                let local_t = if (pos1 - pos0).abs() < 0.0001 {
                    0.0
                } else {
                    (t - pos0) / (pos1 - pos0)
                };

                // Convert sRGB to linear RGB
                let r0 = Self::srgb_to_linear(color0.r as f32 / 255.0);
                let g0 = Self::srgb_to_linear(color0.g as f32 / 255.0);
                let b0 = Self::srgb_to_linear(color0.b as f32 / 255.0);

                let r1 = Self::srgb_to_linear(color1.r as f32 / 255.0);
                let g1 = Self::srgb_to_linear(color1.g as f32 / 255.0);
                let b1 = Self::srgb_to_linear(color1.b as f32 / 255.0);

                // Convert to oklab
                let (l0, a0, b0_ok) = Self::linear_rgb_to_oklab(r0, g0, b0);
                let (l1, a1, b1_ok) = Self::linear_rgb_to_oklab(r1, g1, b1);

                // Interpolate in oklab space
                let l_interp = (1.0 - local_t) * l0 + local_t * l1;
                let a_interp = (1.0 - local_t) * a0 + local_t * a1;
                let b_interp = (1.0 - local_t) * b0_ok + local_t * b1_ok;

                // Convert back to linear RGB
                let (r_lin, g_lin, b_lin) = Self::oklab_to_linear_rgb(l_interp, a_interp, b_interp);

                // Clamp to valid range and convert to sRGB
                let r = (Self::linear_to_srgb(r_lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
                let g = (Self::linear_to_srgb(g_lin.clamp(0.0, 1.0)) * 255.0).round() as u8;
                let b = (Self::linear_to_srgb(b_lin.clamp(0.0, 1.0)) * 255.0).round() as u8;

                // Alpha is interpolated linearly
                let a = (1.0 - local_t) * color0.a + local_t * color1.a;

                return Color::new(r, g, b, a);
            }
        }
        stops[stops.len() - 1].1
    }

    /// Interpolate between color stops using high-precision floating point.
    /// Returns ColorF32 to preserve precision through the pipeline.
    /// This function keeps all color math in f32 and only quantizes at final render.
    fn interpolate_color_f32(stops: &[(f32, rustkit_css::ColorF32)], t: f32) -> rustkit_css::ColorF32 {
        if stops.is_empty() {
            return rustkit_css::ColorF32::TRANSPARENT;
        }
        if stops.len() == 1 || t <= stops[0].0 {
            return stops[0].1;
        }
        if t >= stops[stops.len() - 1].0 {
            return stops[stops.len() - 1].1;
        }

        // Find the two stops surrounding t
        for i in 0..stops.len() - 1 {
            let (pos0, color0) = &stops[i];
            let (pos1, color1) = &stops[i + 1];
            if t >= *pos0 && t <= *pos1 {
                let local_t = if (pos1 - pos0).abs() < 0.0001 {
                    0.0
                } else {
                    (t - pos0) / (pos1 - pos0)
                };

                // Premultiplied alpha interpolation in sRGB space
                // This matches Chrome's default gradient interpolation
                return color0.lerp(color1, local_t);
            }
        }
        stops[stops.len() - 1].1
    }

    /// Draw a text input field.
    #[allow(clippy::too_many_arguments)]
    fn draw_text_input(
        &mut self,
        rect: Rect,
        value: &str,
        placeholder: &str,
        font_size: f32,
        text_color: Color,
        placeholder_color: Color,
        background_color: Color,
        border_color: Color,
        border_width: f32,
        focused: bool,
        caret_position: Option<usize>,
        font_family: &str,
        font_weight: u16,
        padding: [f32; 4],
        kind: rustkit_layout::TextControlKind,
    ) {
        let menu_list = kind == rustkit_layout::TextControlKind::MenuList;

        // Draw background
        self.draw_solid_rect(rect, background_color);

        // Draw border
        let border_rect = rect;
        self.draw_solid_rect(
            Rect::new(rect.x, rect.y, rect.width, border_width),
            border_color,
        );
        self.draw_solid_rect(
            Rect::new(rect.x, rect.y + rect.height - border_width, rect.width, border_width),
            border_color,
        );
        self.draw_solid_rect(
            Rect::new(rect.x, rect.y, border_width, rect.height),
            border_color,
        );
        self.draw_solid_rect(
            Rect::new(rect.x + rect.width - border_width, rect.y, border_width, rect.height),
            border_color,
        );
        
        // Draw text or placeholder, seated like Chrome's inner editor (see
        // form_text_seat for what the old formula got wrong).
        let (text_x, text_top, ascent, descent) =
            Self::form_text_seat(rect, border_width, padding, font_family, font_size);
        // A drop-down's label sits 4px inside its padding edge (Chrome
        // CfT-148: "Option 1" at border + 4 on a bare select, at border +
        // padding + 4 on a padded one).
        let text_x = if menu_list {
            text_x + MENU_LIST_LABEL_INSET
        } else {
            text_x
        };

        let (display_text, display_color) = if value.is_empty() {
            (placeholder, placeholder_color)
        } else {
            (value, text_color)
        };

        if menu_list {
            self.draw_menu_list_arrow(rect, text_color);
        }

        if !display_text.is_empty() {
            self.draw_text_with_metrics(
                display_text,
                text_x,
                text_top,
                display_color,
                font_size,
                font_family,
                font_weight,
                0,
                None,
                Some(ascent),
            );
        }

        // Draw focus ring if focused
        if focused {
            self.draw_focus_ring(border_rect, Color::new(0, 122, 255, 1.0), 2.0, 2.0);
        }

        // Draw caret if focused and position is set: after the measured
        // width of the value's first `pos` chars, spanning the text line.
        if focused {
            if let Some(pos) = caret_position {
                let prefix: String = value.chars().take(pos).collect();
                let caret_x = text_x + Self::measure_run_width(&prefix, font_family, font_size);
                self.draw_caret(caret_x, text_top, ascent + descent, text_color);
            }
        }
    }
    
    /// The drop-down arrow of a `<select>`: a chevron in the control's text
    /// colour, centred vertically, at a fixed distance from the right edge.
    /// Chrome CfT-148 measures the same at 13.333px and at 18px with
    /// `padding: 8px 12px`: about 7.5 wide and 4 tall, its centre 8.75px
    /// inside the border box, whatever the author padding or font size.
    fn draw_menu_list_arrow(&mut self, rect: Rect, color: Color) {
        let (cx, cy) = menu_list_arrow_centre(rect);
        self.process_command(&DisplayCommand::Polyline {
            points: vec![(cx - 3.75, cy - 2.0), (cx, cy + 2.0), (cx + 3.75, cy - 2.0)],
            color,
            width: 1.75,
        });
    }

    /// Draw a list box: the frame, then one row per option, clipped to the
    /// inside of the frame (a list box with more options than rows scrolls;
    /// the rows past its height are not painted).
    #[allow(clippy::too_many_arguments)]
    fn draw_list_box(
        &mut self,
        rect: Rect,
        options: &[String],
        selected: &[usize],
        row_height: f32,
        font_size: f32,
        font_family: &str,
        font_weight: u16,
        text_color: Color,
        background_color: Color,
        border_color: Color,
        border_width: f32,
    ) {
        self.draw_solid_rect(rect, background_color);
        self.draw_border(
            rect,
            border_color,
            border_width,
            border_width,
            border_width,
            border_width,
        );

        let inner = Rect::new(
            rect.x + border_width,
            rect.y + border_width,
            (rect.width - 2.0 * border_width).max(0.0),
            (rect.height - 2.0 * border_width).max(0.0),
        );
        let (ascent, _descent) = Self::fallback_run_metrics(font_family, font_size);
        self.push_clip(inner);
        for (index, label) in options.iter().enumerate() {
            let row = list_box_row_rect(inner, row_height, index);
            if row.y >= inner.y + inner.height {
                break;
            }
            // Chrome CfT-148, list box without focus: a selected row is
            // rgb(206,206,206) with rgb(16,16,16) text.
            let is_selected = selected.contains(&index);
            if is_selected {
                self.draw_solid_rect(row, LIST_BOX_SELECTED_ROW);
            }
            self.draw_text_with_metrics(
                label,
                row.x + LIST_BOX_OPTION_INSET,
                row.y,
                if is_selected {
                    LIST_BOX_SELECTED_TEXT
                } else {
                    text_color
                },
                font_size,
                font_family,
                font_weight,
                0,
                None,
                Some(ascent),
            );
        }
        self.pop_clip();
    }

    /// Draw a button.
    #[allow(clippy::too_many_arguments)]
    fn draw_button(
        &mut self,
        rect: Rect,
        label: &str,
        font_size: f32,
        text_color: Color,
        background_color: Color,
        border_color: Color,
        border_width: f32,
        _border_radius: f32,
        pressed: bool,
        focused: bool,
        font_family: &str,
        font_weight: u16,
        padding: [f32; 4],
    ) {
        // Adjust colors for pressed state
        let bg = if pressed {
            Color::new(
                (background_color.r as i32 - 20).max(0) as u8,
                (background_color.g as i32 - 20).max(0) as u8,
                (background_color.b as i32 - 20).max(0) as u8,
                background_color.a,
            )
        } else {
            background_color
        };
        
        // Draw background
        self.draw_solid_rect(rect, bg);
        
        // Draw border
        self.draw_solid_rect(
            Rect::new(rect.x, rect.y, rect.width, border_width),
            border_color,
        );
        self.draw_solid_rect(
            Rect::new(rect.x, rect.y + rect.height - border_width, rect.width, border_width),
            border_color,
        );
        self.draw_solid_rect(
            Rect::new(rect.x, rect.y, border_width, rect.height),
            border_color,
        );
        self.draw_solid_rect(
            Rect::new(rect.x + rect.width - border_width, rect.y, border_width, rect.height),
            border_color,
        );
        
        // Draw label: measured width centred in the content box, line box
        // centred vertically (form_text_seat — the old seat painted ~0.8em
        // low and centred a `bytes * 0.5em` guess of the width).
        if !label.is_empty() {
            let (_left, text_top, ascent, _descent) =
                Self::form_text_seat(rect, border_width, padding, font_family, font_size);
            let label_width = Self::measure_run_width(label, font_family, font_size);
            let inner_x = rect.x + border_width + padding[3];
            let inner_w = (rect.width - 2.0 * border_width - padding[1] - padding[3]).max(0.0);
            let text_x = inner_x + (inner_w - label_width) / 2.0;
            self.draw_text_with_metrics(
                label,
                text_x,
                text_top,
                text_color,
                font_size,
                font_family,
                font_weight,
                0,
                None,
                Some(ascent),
            );
        }
        
        // Draw focus ring if focused
        if focused {
            self.draw_focus_ring(rect, Color::new(0, 122, 255, 1.0), 2.0, 2.0);
        }
    }
    
    /// Draw a focus ring around an element.
    fn draw_focus_ring(&mut self, rect: Rect, color: Color, width: f32, offset: f32) {
        let outer = Rect::new(
            rect.x - offset,
            rect.y - offset,
            rect.width + offset * 2.0,
            rect.height + offset * 2.0,
        );
        
        // Top
        self.draw_solid_rect(
            Rect::new(outer.x, outer.y, outer.width, width),
            color,
        );
        // Bottom
        self.draw_solid_rect(
            Rect::new(outer.x, outer.y + outer.height - width, outer.width, width),
            color,
        );
        // Left
        self.draw_solid_rect(
            Rect::new(outer.x, outer.y, width, outer.height),
            color,
        );
        // Right
        self.draw_solid_rect(
            Rect::new(outer.x + outer.width - width, outer.y, width, outer.height),
            color,
        );
    }
    
    /// Draw a text caret (cursor).
    fn draw_caret(&mut self, x: f32, y: f32, height: f32, color: Color) {
        self.draw_solid_rect(
            Rect::new(x, y, 2.0, height),
            color,
        );
    }

    /// Draw text.
    /// Draw text filled with a gradient (background-clip: text). The
    /// gradient is sampled horizontally across `rect`; each glyph quad gets
    /// the sampled color on its left and right vertex pairs and the GPU
    /// interpolates across the glyph.
    #[allow(clippy::too_many_arguments)]
    fn draw_text_gradient(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        gradient: &rustkit_css::Gradient,
        rect: &Rect,
        font_size: f32,
        font_family: &str,
        font_weight: u16,
        font_style: u8,
        layout_advances: Option<&[f32]>,
        layout_ascent: Option<f32>,
    ) {
        let stops = match gradient {
            rustkit_css::Gradient::Linear(g) => &g.stops,
            rustkit_css::Gradient::Radial(g) => &g.stops,
            rustkit_css::Gradient::Conic(g) => &g.stops,
        };
        if stops.is_empty() {
            return;
        }

        // Resolve stop positions to 0..1: explicit values kept (pixels
        // normalized by the sweep width), first/last default to 0/1, and
        // runs of None distribute evenly between resolved neighbors.
        let span = rect.width.max(1.0);
        let n = stops.len();
        let mut pos: Vec<Option<f32>> = stops
            .iter()
            .map(|s| {
                s.position.as_ref().map(|p| match p {
                    rustkit_css::StopPosition::Percent(v) => *v,
                    rustkit_css::StopPosition::Pixels(px) => px / span,
                })
            })
            .collect();
        if pos[0].is_none() {
            pos[0] = Some(0.0);
        }
        if pos[n - 1].is_none() {
            pos[n - 1] = Some(1.0);
        }
        let mut i = 0;
        while i < n {
            if pos[i].is_none() {
                let start = i - 1; // pos[0] is Some, so start >= 0 is resolved
                let mut end = i;
                while pos[end].is_none() {
                    end += 1;
                }
                let a = pos[start].unwrap();
                let b = pos[end].unwrap();
                let gap = (end - start) as f32;
                for (k, p) in pos.iter_mut().enumerate().take(end).skip(start + 1) {
                    *p = Some(a + (b - a) * (k - start) as f32 / gap);
                }
            }
            i += 1;
        }

        let sample = |t: f32| -> [f32; 4] {
            let t = t.clamp(0.0, 1.0);
            let mut prev = 0usize;
            for (k, p) in pos.iter().enumerate() {
                if p.unwrap() <= t {
                    prev = k;
                } else {
                    break;
                }
            }
            let next = (prev + 1).min(n - 1);
            let (p0, p1) = (pos[prev].unwrap(), pos[next].unwrap());
            let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 0.0 };
            let (c0, c1) = (&stops[prev].color, &stops[next].color);
            [
                (c0.r as f32 + (c1.r as f32 - c0.r as f32) * f) / 255.0,
                (c0.g as f32 + (c1.g as f32 - c0.g as f32) * f) / 255.0,
                (c0.b as f32 + (c1.b as f32 - c0.b as f32) * f) / 255.0,
                c0.a + (c1.a - c0.a) * f,
            ]
        };

        let mut cursor_x = x;
        let atlas_size = self.glyph_cache.atlas_size() as f32;
        let web_face = GlyphKey::web_face_for(font_family, font_weight, font_style);
        // Glyph entries are baseline-relative (ADVANCE CONTRACT): layout's
        // ascent when shipped, one per-run fallback otherwise.
        let baseline = y
            + layout_ascent.unwrap_or_else(|| Self::fallback_run_ascent(font_family, font_size));

        for (char_idx, ch) in text.chars().enumerate() {
            let key = GlyphKey {
                // FROZEN AT 0 until the rasterizer can draw at a phase --
                // see GlyphKey::subpixel_phase. Pixels are bit-identical to
                // before this field existed.
                subpixel_phase: 0,
                codepoint: ch,
                font_family: font_family.to_string(),
                font_size: (font_size * 10.0) as u32,
                font_weight,
                font_style,
                web_face,
            };

            if let Some(entry) = self.glyph_cache.get_or_rasterize(&self.device, &self.queue, &key) {
                let glyph_x = cursor_x + entry.offset[0];
                let glyph_y = baseline + entry.offset[1];
                let glyph_w = (entry.tex_coords[2] - entry.tex_coords[0]) * atlas_size;
                let glyph_h = (entry.tex_coords[3] - entry.tex_coords[1]) * atlas_size;

                let c_left = sample((glyph_x - rect.x) / span);
                let c_right = sample((glyph_x + glyph_w - rect.x) / span);

                let (x0, y0) = self.transform_point(glyph_x, glyph_y);
                let (x1, y1) = self.transform_point(glyph_x + glyph_w, glyph_y);
                let (x2, y2) = self.transform_point(glyph_x + glyph_w, glyph_y + glyph_h);
                let (x3, y3) = self.transform_point(glyph_x, glyph_y + glyph_h);

                let base = self.texture_vertices.len() as u32;
                self.texture_vertices.extend_from_slice(&[
                    TextureVertex {
                        position: [x0, y0],
                        tex_coords: [entry.tex_coords[0], entry.tex_coords[1]],
                        color: c_left,
                    },
                    TextureVertex {
                        position: [x1, y1],
                        tex_coords: [entry.tex_coords[2], entry.tex_coords[1]],
                        color: c_right,
                    },
                    TextureVertex {
                        position: [x2, y2],
                        tex_coords: [entry.tex_coords[2], entry.tex_coords[3]],
                        color: c_right,
                    },
                    TextureVertex {
                        position: [x3, y3],
                        tex_coords: [entry.tex_coords[0], entry.tex_coords[3]],
                        color: c_left,
                    },
                ]);
                self.texture_indices.extend_from_slice(&[
                    base,
                    base + 1,
                    base + 2,
                    base,
                    base + 2,
                    base + 3,
                ]);

                cursor_x += layout_advances
                    .and_then(|a| a.get(char_idx).copied())
                    .unwrap_or(entry.advance);
            }
        }
    }

    /// One-per-run ascent fallback for legacy callers that ship no layout
    /// ascent — same metric source the deleted per-glyph lookup used.
    fn fallback_run_ascent(font_family: &str, font_size: f32) -> f32 {
        Self::fallback_run_metrics(font_family, font_size).0
    }

    /// `(ascent, descent)` of the run font — the renderer-side metric source
    /// for callers that ship no layout metrics (form-control text).
    #[cfg(target_os = "macos")]
    fn fallback_run_metrics(font_family: &str, font_size: f32) -> (f32, f32) {
        let m = Self::run_shaper(font_family, font_size).get_metrics();
        (m.ascent, m.descent)
    }

    #[cfg(not(target_os = "macos"))]
    fn fallback_run_metrics(_font_family: &str, font_size: f32) -> (f32, f32) {
        (font_size * 0.8, font_size * 0.2)
    }

    /// Advance width of `text` in the run font. Form-control callers use it
    /// to centre a button label / place a caret; the old code guessed
    /// `chars * 0.5em`.
    #[cfg(target_os = "macos")]
    fn measure_run_width(text: &str, font_family: &str, font_size: f32) -> f32 {
        Self::run_shaper(font_family, font_size)
            .shape(text)
            .map(|shaped| shaped.advances.iter().sum())
            .unwrap_or_else(|_| text.chars().count() as f32 * font_size * 0.5)
    }

    #[cfg(not(target_os = "macos"))]
    fn measure_run_width(text: &str, _font_family: &str, font_size: f32) -> f32 {
        text.chars().count() as f32 * font_size * 0.5
    }

    #[cfg(target_os = "macos")]
    fn run_shaper(font_family: &str, font_size: f32) -> rustkit_text::macos::TextShaper {
        let family = if font_family.is_empty() { "Helvetica" } else { font_family };
        rustkit_text::macos::TextShaper::new(family, font_size as f64)
            .unwrap_or_else(|_| rustkit_text::macos::TextShaper::with_system_font(font_size as f64))
    }

    /// Where a form control's text line goes. Chrome centres the inner
    /// editor's line box inside the control's CONTENT box (border-box minus
    /// border and padding); returns `(text_x, text_top, ascent, descent)` so
    /// the caller hands `text_top` + `Some(ascent)` to draw_text_with_metrics.
    ///
    /// The old seat was `rect.y + (h + fs)/2 - 0.2fs` — a BASELINE formula —
    /// handed to draw_text, which treats y as the line TOP and adds the
    /// ascent AGAIN: every input/button label painted ~0.8em too low (a bare
    /// 19px control drew its text below its own bottom border), 6px from the
    /// left regardless of border/padding, in a hardcoded sans-serif.
    fn form_text_seat(
        rect: Rect,
        border_width: f32,
        padding: [f32; 4],
        font_family: &str,
        font_size: f32,
    ) -> (f32, f32, f32, f32) {
        let (ascent, descent) = Self::fallback_run_metrics(font_family, font_size);
        let inner_top = rect.y + border_width + padding[0];
        let inner_h = (rect.height - 2.0 * border_width - padding[0] - padding[2]).max(0.0);
        let text_top = inner_top + (inner_h - (ascent + descent)) / 2.0;
        let text_x = rect.x + border_width + padding[3];
        (text_x, text_top, ascent, descent)
    }

    /// Paint a shaped run (SHAPED-RUN CONTRACT, slice S0): the run's glyph
    /// ids, from the run's face, at the run's advances and offsets. Nothing
    /// here reads a character or a `font-family` list.
    ///
    /// Seating is the family-list path's: the baseline is `y` plus layout's
    /// ascent, snapped to a whole device row (INTEGER-BASELINE CONTRACT),
    /// and glyphs are rasterized at phase 0.
    ///
    /// Returns `false`, having drawn nothing, when the rasterizer does not
    /// hold the run's face; the caller then paints the command the old way.
    fn draw_glyph_run(
        &mut self,
        run: &rustkit_layout::GlyphRun,
        x: f32,
        y: f32,
        color: Color,
        layout_ascent: Option<f32>,
    ) -> bool {
        // FROZEN AT 0, as on the character path (GlyphKey::subpixel_phase).
        let key_for = |glyph_id: u16| RunGlyphKey {
            face: run.face.id,
            glyph_id,
            font_size: (run.font_size * 10.0) as u32,
            subpixel_phase: 0,
        };

        // Every glyph's bitmap first, so a missing face is known before a
        // single quad is emitted.
        let mut entries = Vec::with_capacity(run.glyphs.len());
        for glyph in &run.glyphs {
            match self.glyph_cache.get_or_rasterize_run_glyph(
                &self.queue,
                &key_for(glyph.glyph_id),
                run.font_size,
            ) {
                Some(entry) => entries.push(entry),
                None => return false,
            }
        }

        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];
        let baseline = (y + layout_ascent.unwrap_or(run.ascent)).round();
        let atlas_size = self.glyph_cache.atlas_size() as f32;

        for ((glyph, pen_x), entry) in run
            .glyphs
            .iter()
            .zip(run.pen_positions(x))
            .zip(entries)
        {
            let glyph_x = pen_x + glyph.x_offset + entry.offset[0];
            let glyph_y = baseline + glyph.y_offset + entry.offset[1];
            let glyph_w = (entry.tex_coords[2] - entry.tex_coords[0]) * atlas_size;
            let glyph_h = (entry.tex_coords[3] - entry.tex_coords[1]) * atlas_size;

            // `overflow: hidden` clips glyphs like everything else.
            for ([[x0, y0], [x1, y1], [x2, y2], [x3, y3]], tex, coverage) in self
                .textured_pieces(
                    Rect::new(glyph_x, glyph_y, glyph_w, glyph_h),
                    entry.tex_coords,
                )
            {
                let c = [c[0], c[1], c[2], c[3] * coverage];
                let base = self.texture_vertices.len() as u32;
                self.texture_vertices.extend_from_slice(&[
                    TextureVertex {
                        position: [x0, y0],
                        tex_coords: [tex[0], tex[1]],
                        color: c,
                    },
                    TextureVertex {
                        position: [x1, y1],
                        tex_coords: [tex[2], tex[1]],
                        color: c,
                    },
                    TextureVertex {
                        position: [x2, y2],
                        tex_coords: [tex[2], tex[3]],
                        color: c,
                    },
                    TextureVertex {
                        position: [x3, y3],
                        tex_coords: [tex[0], tex[3]],
                        color: c,
                    },
                ]);
                self.texture_indices.extend_from_slice(&[
                    base, base + 1, base + 2,
                    base, base + 2, base + 3,
                ]);
            }
        }
        true
    }

    /// Draw text honoring the ADVANCE CONTRACT: when layout ships per-char
    /// advances and an ascent, glyphs are placed at layout's advances and
    /// the baseline sits at y + layout_ascent — the renderer's own advance
    /// derivation and per-glyph ascent shaper (two extra text stacks) are
    /// bypassed. Legacy callers pass None and keep the old behavior.
    #[allow(clippy::too_many_arguments)]
    fn draw_text_with_metrics(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        font_size: f32,
        font_family: &str,
        font_weight: u16,
        font_style: u8,
        layout_advances: Option<&[f32]>,
        layout_ascent: Option<f32>,
    ) {
        let mut cursor_x = x;
        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];

        // Baseline: layout's ascent when the command carries one (ADVANCE
        // CONTRACT), else ONE per-run fallback from the same source the old
        // per-glyph lookup used. Glyph entries are baseline-relative.
        //
        // SNAPPED TO A WHOLE DEVICE ROW (INTEGER-BASELINE CONTRACT, pairs
        // with rustkit_text::macos::baseline_seat): the atlas bitmaps are
        // rasterized with their baseline on an integer row and report an
        // integer bearing_y, so a whole-row baseline here means every glyph
        // on the line lands pixel-aligned with NO vertical resampling. This
        // is what Skia does for horizontal text (subpixel x, rounded y);
        // a fractional baseline smeared every glyph across two rows.
        let baseline = (y
            + layout_ascent.unwrap_or_else(|| Self::fallback_run_ascent(font_family, font_size)))
        .round();

        // PAINT-0 seating probe (RUSTKIT_PAINT_PROBE=1): paint half of the
        // seating chain — pairs with the layout-side y_cmd log so a flat vs
        // metrics A/B can attribute score deltas to seating float shifts
        // (forensics 2026-07-16-paint0-glyph-seat §4.2 P0a).
        if crate::paint0_probe() {
            eprintln!(
                "PAINT0 paint text={:?} fs={} y_cmd={} layout_ascent={:?} baseline={}",
                text.chars().take(16).collect::<String>(),
                font_size,
                y,
                layout_ascent,
                baseline
            );
        }

        // Get atlas size before the loop to avoid borrow issues
        let atlas_size = self.glyph_cache.atlas_size() as f32;
        let web_face = GlyphKey::web_face_for(font_family, font_weight, font_style);

        for (char_idx, ch) in text.chars().enumerate() {
            let key = GlyphKey {
                // FROZEN AT 0 until the rasterizer can draw at a phase --
                // see GlyphKey::subpixel_phase. Pixels are bit-identical to
                // before this field existed.
                subpixel_phase: 0,
                codepoint: ch,
                font_family: font_family.to_string(),
                font_size: (font_size * 10.0) as u32,
                font_weight,
                font_style,
                web_face,
            };

            // Color-glyph (emoji) path: paint the real color-bitmap artwork via
            // the RGBA atlas + blit pipeline, not the grayscale coverage mask
            // the normal path would tint into a flat blob. Falls through to the
            // grayscale path if the char isn't a color glyph or has no color
            // artwork (e.g. non-macOS).
            #[cfg(any(target_os = "macos", windows))]
            let is_color = rustkit_text::is_emoji(ch);
            #[cfg(not(any(target_os = "macos", windows)))]
            let is_color = false;
            if is_color {
                if let Some(entry) =
                    self.glyph_cache.get_or_rasterize_color(&self.device, &self.queue, &key)
                {
                    let glyph_x = cursor_x + entry.offset[0];
                    let glyph_y = baseline + entry.offset[1];
                    let glyph_w = (entry.tex_coords[2] - entry.tex_coords[0]) * atlas_size;
                    let glyph_h = (entry.tex_coords[3] - entry.tex_coords[1]) * atlas_size;

                    // The advance is owed whether or not the glyph survives
                    // the clip — a clipped-away glyph still occupies its run.
                    cursor_x += layout_advances
                        .and_then(|a| a.get(char_idx).copied())
                        .unwrap_or(entry.advance);

                    for ([[x0, y0], [x1, y1], [x2, y2], [x3, y3]], tex, coverage) in self
                        .textured_pieces(
                            Rect::new(glyph_x, glyph_y, glyph_w, glyph_h),
                            entry.tex_coords,
                        )
                    {
                        // White vertex color: the blit pipeline multiplies, so this
                        // passes the emoji's own colors through untinted. Preserve
                        // the run's alpha for opacity/fade. The emoji is
                        // premultiplied, so a rounded clip's partial coverage
                        // scales every channel.
                        let cw = [coverage, coverage, coverage, color.a * coverage];
                        let base = self.color_glyph_vertices.len() as u32;
                        self.color_glyph_vertices.extend_from_slice(&[
                            TextureVertex { position: [x0, y0], tex_coords: [tex[0], tex[1]], color: cw },
                            TextureVertex { position: [x1, y1], tex_coords: [tex[2], tex[1]], color: cw },
                            TextureVertex { position: [x2, y2], tex_coords: [tex[2], tex[3]], color: cw },
                            TextureVertex { position: [x3, y3], tex_coords: [tex[0], tex[3]], color: cw },
                        ]);
                        self.color_glyph_indices.extend_from_slice(&[
                            base, base + 1, base + 2,
                            base, base + 2, base + 3,
                        ]);
                    }
                    continue;
                }
            }

            // Clone the entry to avoid borrow issues
            if let Some(entry) = self.glyph_cache.get_or_rasterize(&self.device, &self.queue, &key) {
                let glyph_x = cursor_x + entry.offset[0];
                let glyph_y = baseline + entry.offset[1];

                // PAINT-0: sample chars only — x (ex-height), H (cap), g
                // (descender) cover the three seating regimes.
                if matches!(ch, 'x' | 'H' | 'g') && crate::paint0_probe() {
                    eprintln!(
                        "PAINT0 glyph ch={:?} fs={} baseline={} bearing_y={} glyph_y={}",
                        ch,
                        font_size,
                        baseline,
                        -entry.offset[1],
                        glyph_y
                    );
                }
                let glyph_w = (entry.tex_coords[2] - entry.tex_coords[0]) * atlas_size;
                let glyph_h = (entry.tex_coords[3] - entry.tex_coords[1]) * atlas_size;

                // ADVANCE CONTRACT: layout's advance wins when present so
                // painted ink tracks measured width 1:1; the atlas advance
                // is the fallback for legacy callers. Owed before the clip
                // check — a clipped-away glyph still occupies its run.
                cursor_x += layout_advances
                    .and_then(|a| a.get(char_idx).copied())
                    .unwrap_or(entry.advance);

                // `overflow: hidden` clips glyphs like everything else.
                for ([[x0, y0], [x1, y1], [x2, y2], [x3, y3]], tex, coverage) in self
                    .textured_pieces(
                        Rect::new(glyph_x, glyph_y, glyph_w, glyph_h),
                        entry.tex_coords,
                    )
                {
                    let c = [c[0], c[1], c[2], c[3] * coverage];
                    let base = self.texture_vertices.len() as u32;

                    self.texture_vertices.extend_from_slice(&[
                        TextureVertex {
                            position: [x0, y0],
                            tex_coords: [tex[0], tex[1]],
                            color: c,
                        },
                        TextureVertex {
                            position: [x1, y1],
                            tex_coords: [tex[2], tex[1]],
                            color: c,
                        },
                        TextureVertex {
                            position: [x2, y2],
                            tex_coords: [tex[2], tex[3]],
                            color: c,
                        },
                        TextureVertex {
                            position: [x3, y3],
                            tex_coords: [tex[0], tex[3]],
                            color: c,
                        },
                    ]);

                    self.texture_indices.extend_from_slice(&[
                        base, base + 1, base + 2,
                        base, base + 2, base + 3,
                    ]);
                }
            } else {
                // Fallback: advance by estimated width (or layout's, if given)
                cursor_x += layout_advances
                    .and_then(|a| a.get(char_idx).copied())
                    .unwrap_or(font_size * 0.6);
            }
        }
    }

    /// Draw an image.
    fn draw_image(&mut self, url: &str, rect: Rect) {
        if self.texture_cache.contains(url) {
            // `overflow: hidden` clips replaced content like everything else.
            for ([[x0, y0], [x1, y1], [x2, y2], [x3, y3]], tex, coverage) in
                self.textured_pieces(rect, [0.0, 0.0, 1.0, 1.0])
            {
                let color = [1.0, 1.0, 1.0, coverage];
                self.push_image_quad(
                    url,
                    [
                        TextureVertex {
                            position: [x0, y0],
                            tex_coords: [tex[0], tex[1]],
                            color,
                        },
                        TextureVertex {
                            position: [x1, y1],
                            tex_coords: [tex[2], tex[1]],
                            color,
                        },
                        TextureVertex {
                            position: [x2, y2],
                            tex_coords: [tex[2], tex[3]],
                            color,
                        },
                        TextureVertex {
                            position: [x3, y3],
                            tex_coords: [tex[0], tex[3]],
                            color,
                        },
                    ],
                );
            }
        }
        // If image not loaded, skip (async loading handled elsewhere)
    }

    /// Append a quad to the image batch, extending the current run when the
    /// previous quad used the same texture so consecutive tiles stay one draw.
    fn push_image_quad(&mut self, url: &str, corners: [TextureVertex; 4]) {
        let base = self.image_vertices.len() as u32;
        self.image_vertices.extend_from_slice(&corners);
        self.image_indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
        ]);

        match self.image_runs.last_mut() {
            Some((last_url, count)) if last_url == url => *count += 6,
            _ => self.image_runs.push((url.to_string(), 6)),
        }
    }

    /// Draw a background image with proper size, position, and repeat handling.
    fn draw_background_image(
        &mut self,
        url: &str,
        container: Rect,
        size: &BackgroundSize,
        position: (f32, f32),
        repeat: &BackgroundRepeat,
    ) {
        // Get the texture to retrieve image dimensions
        let (image_width, image_height) = if let Some(cached) = self.texture_cache.get(url) {
            (cached.width as f32, cached.height as f32)
        } else {
            // Image not loaded yet, skip
            return;
        };

        if image_width == 0.0 || image_height == 0.0 {
            return;
        }

        for tile in rustkit_layout::background_tiles(container, size, position, *repeat, image_width, image_height) {
            self.draw_background_image_tile(url, tile, container);
        }
    }

    /// Draw a single tile of a background image, clipped to the container bounds.
    fn draw_background_image_tile(&mut self, url: &str, tile_rect: Rect, container: Rect) {
        if !self.texture_cache.contains(url) {
            return;
        }

        // Clip tile to container bounds
        let clip_left = (container.x - tile_rect.x).max(0.0);
        let clip_top = (container.y - tile_rect.y).max(0.0);
        let clip_right = (tile_rect.x + tile_rect.width - container.x - container.width).max(0.0);
        let clip_bottom = (tile_rect.y + tile_rect.height - container.y - container.height).max(0.0);

        let draw_rect = Rect {
            x: tile_rect.x + clip_left,
            y: tile_rect.y + clip_top,
            width: tile_rect.width - clip_left - clip_right,
            height: tile_rect.height - clip_top - clip_bottom,
        };

        if draw_rect.width <= 0.0 || draw_rect.height <= 0.0 {
            return;
        }

        // Calculate texture coordinates for the clipped portion
        let tex_left = clip_left / tile_rect.width;
        let tex_top = clip_top / tile_rect.height;
        let tex_right = 1.0 - clip_right / tile_rect.width;
        let tex_bottom = 1.0 - clip_bottom / tile_rect.height;

        // Then the overflow clip on top of the container clip.
        for ([[x0, y0], [x1, y1], [x2, y2], [x3, y3]], [tex_left, tex_top, tex_right, tex_bottom], coverage) in
            self.textured_pieces(draw_rect, [tex_left, tex_top, tex_right, tex_bottom])
        {
            let color = [1.0, 1.0, 1.0, coverage];
            self.push_image_quad(
                url,
                [
                    TextureVertex {
                        position: [x0, y0],
                        tex_coords: [tex_left, tex_top],
                        color,
                    },
                    TextureVertex {
                        position: [x1, y1],
                        tex_coords: [tex_right, tex_top],
                        color,
                    },
                    TextureVertex {
                        position: [x2, y2],
                        tex_coords: [tex_right, tex_bottom],
                        color,
                    },
                    TextureVertex {
                        position: [x3, y3],
                        tex_coords: [tex_left, tex_bottom],
                        color,
                    },
                ],
            );
        }
    }

    /// Upload an image to the texture cache.
    /// 
    /// Call this to upload decoded image data (RGBA format) to the GPU.
    /// Once uploaded, the image can be drawn using its URL as the key.
    pub fn upload_image(
        &mut self,
        url: &str,
        width: u32,
        height: u32,
        rgba_data: &[u8],
    ) -> Result<(), RendererError> {
        if rgba_data.len() != (width * height * 4) as usize {
            return Err(RendererError::TextureUpload(format!(
                "Invalid image data size: expected {} bytes, got {}",
                width * height * 4,
                rgba_data.len()
            )));
        }
        
        self.texture_cache.get_or_create(
            &self.device,
            &self.queue,
            url,
            width,
            height,
            rgba_data,
        );
        
        Ok(())
    }
    
    /// Check if an image is already uploaded.
    pub fn has_image(&self, url: &str) -> bool {
        self.texture_cache.contains(url)
    }
    
    /// Remove an image from the cache.
    pub fn remove_image(&mut self, url: &str) {
        self.texture_cache.remove(url);
    }


    /// Push a clipping rectangle.
    fn push_clip(&mut self, rect: Rect) {
        self.push_clip_rounded(rect, rustkit_layout::BorderRadius::default());
    }

    /// Push a clipping rectangle whose corners may be rounded.
    ///
    /// The rect half intersects as it always did. The rounded half accumulates:
    /// a nested rounded clip does not replace its parent, because a point has to
    /// be inside both.
    ///
    /// The entry is stored in SCREEN space: the command's rect is in document
    /// space and is mapped through the transform in force when the clip is
    /// pushed, so a clip inside a transformed box moves with the box, and a
    /// descendant transformed AFTER the clip was pushed is clipped where it
    /// lands rather than where it would have been without its transform.
    fn push_clip_rounded(&mut self, rect: Rect, radius: rustkit_layout::BorderRadius) {
        let entry = clip_entry_under(self.clip_stack.last(), self.current_transform(), rect, radius);
        self.clip_stack.push(entry);
    }

    /// Pop the current clipping rectangle.
    fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Get the current clip rectangle (screen space).
    fn current_clip(&self) -> Option<Rect> {
        self.clip_stack.last().map(|entry| entry.rect)
    }

    /// A textured quad (glyph, image) cut to the current clip and taken to
    /// screen space: the four corner positions in emit order (top-left,
    /// top-right, bottom-right, bottom-left) and the texture coordinates of
    /// the surviving part. `None` when nothing survives. One rule for every
    /// textured site so text, images and tiles are clipped under a transform
    /// exactly as color quads are.
    ///
    /// A quad that a rounded clip's corner cuts comes back as several
    /// pieces, each with the coverage its vertices' alpha is to be scaled
    /// by; any other quad is one piece with coverage 1. Empty when nothing
    /// survives.
    fn textured_pieces(&self, rect: Rect, tex: [f32; 4]) -> Vec<([[f32; 2]; 4], [f32; 4], f32)> {
        let mut pieces = Vec::with_capacity(1);
        let space = clip_textured_pieces_under(
            self.current_transform(),
            self.clip_stack.last(),
            rect,
            tex,
            &mut pieces,
        );
        pieces
            .into_iter()
            .map(|(g, tex, coverage)| {
                let corners = match space {
                    QuadSpace::Screen => [
                        [g.x, g.y],
                        [g.x + g.width, g.y],
                        [g.x + g.width, g.y + g.height],
                        [g.x, g.y + g.height],
                    ],
                    QuadSpace::Document => {
                        let (x0, y0) = self.transform_point(g.x, g.y);
                        let (x1, y1) = self.transform_point(g.x + g.width, g.y);
                        let (x2, y2) = self.transform_point(g.x + g.width, g.y + g.height);
                        let (x3, y3) = self.transform_point(g.x, g.y + g.height);
                        [[x0, y0], [x1, y1], [x2, y2], [x3, y3]]
                    }
                };
                (corners, tex, coverage)
            })
            .collect()
    }


    /// Push a 2D transform matrix onto the stack.
    fn push_transform(&mut self, matrix: [f32; 6], origin: (f32, f32)) {
        self.transform_stack.push((matrix, origin));
    }

    /// Pop the current transform from the stack.
    fn pop_transform(&mut self) {
        self.transform_stack.pop();
    }

    /// Get the current combined transform matrix.
    /// Returns identity matrix [1, 0, 0, 1, 0, 0] if no transforms are active.
    fn current_transform(&self) -> [f32; 6] {
        if self.transform_stack.is_empty() {
            return [1.0, 0.0, 0.0, 1.0, 0.0, 0.0]; // Identity
        }

        // Compose all transforms on the stack: an outer (earlier) transform
        // applies to what an inner one produces, so the page-space affine is
        // `outer · inner` in the column-vector convention `multiply_matrices_2d`
        // uses.
        let mut result = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        for (matrix, origin) in &self.transform_stack {
            result = multiply_matrices_2d(result, affine_about_origin(*matrix, *origin));
        }
        result
    }

    /// Apply the current transform to a point.
    /// The index range of gradient strips that can possibly reach pixels,
    /// given the current clip and the viewport. Strips are generated one per
    /// CSS pixel along the gradient axis, so an unculled loop is O(document
    /// extent): a single 3,000,000px-tall gradient emits 3M quads = 288MB of
    /// color vertices and the frame dies with BufferTooLarge — every frame,
    /// forever, which is exactly the repeating "Buffer 'Color Vertex Buffer'
    /// size ... exceeds maximum" seen on real tab pages (2026-08-05).
    ///
    /// Only valid when no transform is active: with a transform on the stack
    /// the strip's document position no longer predicts its screen position,
    /// so we fall back to the full range rather than wrongly cull content a
    /// transform moves into view. Cap stays either way as the last line.
    /// The index range of gradient CELLS (row or column) that can reach the
    /// viewport along one axis, on the same law as `visible_strip_range`: a
    /// cell's document position predicts its screen position only while no
    /// transform is active, so with a transform on the stack the full range
    /// comes back. Indices stay grid-aligned (floor/ceil of the viewport
    /// bounds in cell units), so a culled render paints the identical pixels
    /// for every cell that survives.
    ///
    /// Without this, the cell paths (linear-with-radius/diagonal, radial,
    /// conic) emit up to `max_cells` quads per gradient over the element's
    /// FULL rect — a per-element cap that composes into millions of quads on
    /// a long page of offscreen gradient cards, dying with BufferTooLarge
    /// every frame (2026-09-08, autotrader smoke).
    fn visible_cell_range(
        &self,
        axis_start: f32,
        _axis_len: f32,
        cell_size: f32,
        count: usize,
        viewport_extent: f32,
    ) -> (usize, usize) {
        cell_range_for_viewport(
            axis_start,
            cell_size,
            count,
            viewport_extent,
            !self.transform_stack.is_empty(),
        )
    }

    fn visible_strip_range(
        &self,
        axis_start: f32,
        axis_len: f32,
        step_count: usize,
        viewport_extent: f32,
    ) -> (usize, usize) {
        const MAX_STRIPS: usize = 32_768;
        if !self.transform_stack.is_empty() {
            return (0, step_count.min(MAX_STRIPS));
        }
        // Visible window along this axis is the viewport [0, extent).
        // Per-strip clip culling still happens inside draw_solid_rect_f32;
        // this range only bounds the LOOP, which is what the vertex budget
        // needs — a strip inside the viewport but outside a clip costs one
        // rejected call, not a quad.
        let (lo, hi) = (0.0_f32, viewport_extent);
        let strip = axis_len / step_count as f32;
        let first = (((lo - axis_start) / strip).floor().max(0.0)) as usize;
        let last = ((((hi - axis_start) / strip).ceil()).max(0.0) as usize).min(step_count);
        let first = first.min(last);
        (first, last.min(first + MAX_STRIPS))
    }

    fn transform_point(&self, x: f32, y: f32) -> (f32, f32) {
        let m = self.current_transform();
        // [a, b, c, d, e, f] where:
        // x' = a*x + c*y + e
        // y' = b*x + d*y + f
        let x_prime = m[0] * x + m[2] * y + m[4];
        let y_prime = m[1] * x + m[3] * y + m[5];
        (x_prime, y_prime)
    }

    /// Flush all batched vertices to the target.
    /// Draw the batched image quads into an open render pass: one draw call
    /// per (texture, run) pair so every image samples its own texture rather
    /// than the glyph atlas the shared texture batch binds.
    fn draw_image_batch(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        if self.image_vertices.is_empty() {
            return;
        }

        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Image Vertex Buffer"),
            contents: bytemuck::cast_slice(&self.image_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Image Index Buffer"),
            contents: bytemuck::cast_slice(&self.image_indices),
            usage: wgpu::BufferUsages::INDEX,
        });

        // image_pipeline, not texture_pipeline: the texture shader treats the
        // sampled R channel as glyph-atlas alpha; the blit shader samples real
        // RGBA. Not blit_pipeline either: its blend is REPLACE, which paints an
        // image's transparent texels as their own colour (black, for most PNGs).
        render_pass.set_pipeline(&self.image_pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);

        let mut start = 0u32;
        for (url, count) in &self.image_runs {
            if let Some(cached) = self.texture_cache.get(url) {
                render_pass.set_bind_group(1, &cached.bind_group, &[]);
                render_pass.draw_indexed(start..start + count, 0, 0..1);
            }
            start += count;
        }
    }

    /// Draw the color-glyph (emoji) batch: RGBA quads sampling the color atlas
    /// via the passthrough blit pipeline (blit samples real RGBA; the grayscale
    /// texture pipeline would treat R as alpha and mangle the artwork). Empty on
    /// pages without emoji, so this is a no-op for normal text.
    fn draw_color_glyph_batch(&self, render_pass: &mut wgpu::RenderPass<'_>) {
        if self.color_glyph_vertices.is_empty() {
            return;
        }
        let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Color Glyph Vertex Buffer"),
            contents: bytemuck::cast_slice(&self.color_glyph_vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Color Glyph Index Buffer"),
            contents: bytemuck::cast_slice(&self.color_glyph_indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        render_pass.set_pipeline(&self.color_glyph_pipeline);
        render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        render_pass.set_bind_group(1, self.glyph_cache.color_bind_group(), &[]);
        render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
        render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
        render_pass.draw_indexed(0..self.color_glyph_indices.len() as u32, 0, 0..1);
    }

    fn flush_to(&mut self, target: &wgpu::TextureView) -> Result<(), RendererError> {
        // Check for debug visual mode (RUSTKIT_DEBUG_VISUAL=1)
        // When enabled, clear to magenta to prove pixels are hitting the screen
        let debug_visual = std::env::var("RUSTKIT_DEBUG_VISUAL")
            .map(|v| v == "1" || v.to_lowercase() == "true")
            .unwrap_or(false);

        let clear_color = if debug_visual {
            // Magenta - very visible, proves rendering works
            wgpu::Color {
                r: 1.0,
                g: 0.0,
                b: 1.0,
                a: 1.0,
            }
        } else {
            // Normal white background
            wgpu::Color {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }
        };

        // Clear gradient queues (safety measure - they should already be empty)
        // GPU gradients are now rendered inline via execute_with_gpu_gradients() for correct z-order
        self.gradient_queue.clear();
        self.radial_gradient_queue.clear();
        self.conic_gradient_queue.clear();

        // Render batched content
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        // Always clear on first pass
        let load_op = wgpu::LoadOp::Clear(clear_color);

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: load_op,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // In debug mode, draw a test rectangle at (10,10) to prove draw commands work
            if debug_visual && self.color_vertices.is_empty() {
                // If no commands were issued, add a test rectangle
                let test_rect = Rect::new(10.0, 10.0, 100.0, 100.0);
                let test_color = Color::new(0, 255, 0, 1.0); // Green
                let c = [
                    test_color.r as f32 / 255.0,
                    test_color.g as f32 / 255.0,
                    test_color.b as f32 / 255.0,
                    test_color.a,
                ];
                let x = test_rect.x;
                let y = test_rect.y;
                let w = test_rect.width;
                let h = test_rect.height;

                self.color_vertices.extend_from_slice(&[
                    ColorVertex { position: [x, y], color: c },
                    ColorVertex { position: [x + w, y], color: c },
                    ColorVertex { position: [x + w, y + h], color: c },
                    ColorVertex { position: [x, y + h], color: c },
                ]);
                self.color_indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
            }

            // Draw solid colors
            if !self.color_vertices.is_empty() {
                // Validate buffer sizes before allocation
                let vertex_size = (self.color_vertices.len() * std::mem::size_of::<ColorVertex>()) as u64;
                let index_size = (self.color_indices.len() * std::mem::size_of::<u32>()) as u64;

                self.validate_buffer_size(vertex_size, "Color Vertex Buffer")?;
                self.validate_buffer_size(index_size, "Color Index Buffer")?;

                let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Color Vertex Buffer"),
                    contents: bytemuck::cast_slice(&self.color_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Color Index Buffer"),
                    contents: bytemuck::cast_slice(&self.color_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                render_pass.set_pipeline(&self.color_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.color_indices.len() as u32, 0, 0..1);
            }

            // Draw images (own textures) between backgrounds and text
            self.draw_image_batch(&mut render_pass);

            // Draw textured quads (glyphs)
            if !self.texture_vertices.is_empty() {
                let vertex_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Texture Vertex Buffer"),
                    contents: bytemuck::cast_slice(&self.texture_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });

                let index_buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Texture Index Buffer"),
                    contents: bytemuck::cast_slice(&self.texture_indices),
                    usage: wgpu::BufferUsages::INDEX,
                });

                render_pass.set_pipeline(&self.texture_pipeline);
                render_pass.set_bind_group(0, &self.uniform_bind_group, &[]);
                render_pass.set_bind_group(1, self.glyph_cache.bind_group(), &[]);
                render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                render_pass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                render_pass.draw_indexed(0..self.texture_indices.len() as u32, 0, 0..1);
            }

            // Color glyphs (emoji) drawn on top via the RGBA atlas + blit pipeline.
            self.draw_color_glyph_batch(&mut render_pass);
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        // Note: GPU gradients are now rendered inline via execute_with_gpu_gradients()
        // for correct z-order (gradients render in DOM order, not all-at-end)

        Ok(())
    }

    /// Get access to the texture cache for external image loading.
    pub fn texture_cache(&mut self) -> &mut TextureCache {
        &mut self.texture_cache
    }

    /// Get access to the glyph cache.
    pub fn glyph_cache(&mut self) -> &mut GlyphCache {
        &mut self.glyph_cache
    }
}

// ==================== Rect Extension ====================

trait RectExt {
    fn intersect(&self, other: &Rect) -> Option<Rect>;
}

impl RectExt for Rect {
    fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let right = (self.x + self.width).min(other.x + other.width);
        let bottom = (self.y + self.height).min(other.y + other.height);

        if right > x && bottom > y {
            Some(Rect::new(x, y, right - x, bottom - y))
        } else {
            None
        }
    }
}

// ==================== Rounded clipping ====================

/// One entry on the clip stack.
///
/// `rect` is the intersection of every clip pushed so far, exactly as the old
/// `Vec<Rect>` stack held it. `rounded` carries the clips that also round their
/// corners, each with its OWN rect — a rounded corner is a property of the box
/// that pushed it, so intersecting the rects would move the arc centres and
/// round the wrong place.
///
/// Both are needed. `rect` alone is what shipped before this: `overflow: hidden`
/// on a 12px-radius card clipped nothing at the corners, so a child's background
/// painted square into the notch. Gate B named 51 of those as `missing_clip`
/// discrete structural failures on 2026-08-08 (image-gallery 17, sticky-scroll
/// 12, new_tab 10).
#[derive(Debug, Clone, Default)]
struct ClipEntry {
    rect: Rect,
    /// Rounded constraints still in force, outermost first. A point must be
    /// inside every one of them.
    rounded: Vec<(Rect, rustkit_layout::BorderRadius)>,
}

/// The clip entry a `PushClip`/`PushClipRounded` produces on top of `current`.
///
/// Pure so it can be tested: the stack lives on `Renderer`, which needs a wgpu
/// device, and a device is not available on every machine that runs these
/// tests. Keeping the rule here rather than in the method means a mutation to
/// the rule is caught rather than merely compiled.
fn clip_entry_for(
    current: Option<&ClipEntry>,
    rect: Rect,
    radius: rustkit_layout::BorderRadius,
) -> ClipEntry {
    let (clip, mut rounded) = match current {
        Some(current) => (
            current
                .rect
                .intersect(&rect)
                .unwrap_or_else(|| Rect::new(0.0, 0.0, 0.0, 0.0)), // Empty clip
            current.rounded.clone(),
        ),
        None => (rect, Vec::new()),
    };
    // Accumulate, never replace: a point under two rounded clips has to be
    // inside both, and the outer arc does not stop existing because an inner
    // box pushed its own.
    if !radius.is_zero() {
        rounded.push((rect, radius));
    }
    ClipEntry {
        rect: clip,
        rounded,
    }
}

const IDENTITY_2D: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

/// Which space a clipped piece comes back in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuadSpace {
    /// Already mapped through the transform; emit the corners as they are.
    Screen,
    /// Still in document space; the emitter applies the transform.
    Document,
}

/// `rect` mapped through `m` when `m` has no rotation or skew — the mapped
/// rect is still a rect, so it can be clipped exactly. `None` otherwise.
fn map_rect_axis_aligned(m: [f32; 6], rect: Rect) -> Option<Rect> {
    if m[1] != 0.0 || m[2] != 0.0 {
        return None;
    }
    let x0 = m[0] * rect.x + m[4];
    let x1 = m[0] * (rect.x + rect.width) + m[4];
    let y0 = m[3] * rect.y + m[5];
    let y1 = m[3] * (rect.y + rect.height) + m[5];
    Some(Rect::new(x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs()))
}

/// The bounding box of `rect`'s corners mapped through `m`.
fn map_rect_bounds(m: [f32; 6], rect: Rect) -> Rect {
    let corners = [
        (rect.x, rect.y),
        (rect.x + rect.width, rect.y),
        (rect.x + rect.width, rect.y + rect.height),
        (rect.x, rect.y + rect.height),
    ];
    let mut x0 = f32::MAX;
    let mut y0 = f32::MAX;
    let mut x1 = f32::MIN;
    let mut y1 = f32::MIN;
    for (x, y) in corners {
        let px = m[0] * x + m[2] * y + m[4];
        let py = m[1] * x + m[3] * y + m[5];
        x0 = x0.min(px);
        y0 = y0.min(py);
        x1 = x1.max(px);
        y1 = y1.max(py);
    }
    Rect::new(x0, y0, x1 - x0, y1 - y0)
}

/// Inverse of a 2D affine matrix, `None` when it is singular (a zero scale).
fn invert_matrix_2d(m: [f32; 6]) -> Option<[f32; 6]> {
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-12 {
        return None;
    }
    let inv_det = 1.0 / det;
    let a = m[3] * inv_det;
    let b = -m[1] * inv_det;
    let c = -m[2] * inv_det;
    let d = m[0] * inv_det;
    Some([a, b, c, d, -(a * m[4] + c * m[5]), -(b * m[4] + d * m[5])])
}

/// The clip entry a `PushClip`/`PushClipRounded` issued under transform `m`
/// produces on top of `current`. The command's `rect` is in document space;
/// the entry is in screen space, so a clip and the quads drawn under it are
/// compared where they both land.
///
/// Under a rotation or skew the clip's rounded part is kept only as a
/// bounding box (a rotated rounded rect is not a rounded rect) — a ledgered
/// approximation; no board case rotates an `overflow: hidden` box.
fn clip_entry_under(
    current: Option<&ClipEntry>,
    m: [f32; 6],
    rect: Rect,
    radius: rustkit_layout::BorderRadius,
) -> ClipEntry {
    if m == IDENTITY_2D {
        return clip_entry_for(current, rect, radius);
    }
    match map_rect_axis_aligned(m, rect) {
        Some(screen) => {
            // Axis-aligned, so each axis scales its own radii: a circular
            // corner under a non-uniform scale is an ellipse on screen.
            clip_entry_for(current, screen, radius.scaled(m[0].abs(), m[3].abs()))
        }
        None => clip_entry_for(current, map_rect_bounds(m, rect), radius),
    }
}

/// Everything a document-space `rect` drawn under transform `m` becomes under
/// the screen-space `clip`, appended to `out` as `(piece, coverage)`; the
/// return value says which space the pieces are in.
///
/// Without a transform this is `collect_clipped_pieces` and emits exactly the
/// vertices it always did. With an axis-aligned transform the quad is mapped
/// first and clipped where it lands. With a rotation or skew the quad cannot be
/// clipped as a rect after mapping, so the clip's rectangular part is brought
/// back to document space (as a bounding box) and the quad is clipped before
/// the transform — the pre-existing behaviour, kept as the fallback.
fn clip_quad_under(
    m: [f32; 6],
    clip: Option<&ClipEntry>,
    rect: Rect,
    out: &mut Vec<(Rect, f32)>,
) -> QuadSpace {
    if m == IDENTITY_2D {
        collect_clipped_pieces(clip, rect, out);
        return QuadSpace::Screen;
    }
    if let Some(screen) = map_rect_axis_aligned(m, rect) {
        collect_clipped_pieces(clip, screen, out);
        return QuadSpace::Screen;
    }
    match clip {
        None => out.push((rect, 1.0)),
        Some(entry) => {
            if let Some(inv) = invert_matrix_2d(m) {
                let fallback = ClipEntry {
                    rect: map_rect_bounds(inv, entry.rect),
                    rounded: Vec::new(),
                };
                collect_clipped_pieces(Some(&fallback), rect, out);
            }
            // A singular transform paints nothing visible.
        }
    }
    QuadSpace::Document
}

/// `clip_textured_rect` under transform `m`, on the same law as
/// `clip_quad_under`: the surviving rect, its texture coordinates, and the
/// space the rect is in.
fn clip_textured_under(
    m: [f32; 6],
    clip: Option<Rect>,
    rect: Rect,
    tex: [f32; 4],
) -> Option<(Rect, [f32; 4], QuadSpace)> {
    if m == IDENTITY_2D {
        let (r, t) = clip_textured_rect(clip, rect, tex)?;
        return Some((r, t, QuadSpace::Screen));
    }
    if let Some(screen) = map_rect_axis_aligned(m, rect) {
        // A negative scale flips the texels; keep them in the same order as
        // the mapped corners by flipping the coordinates too.
        let tex = [
            if m[0] < 0.0 { tex[2] } else { tex[0] },
            if m[3] < 0.0 { tex[3] } else { tex[1] },
            if m[0] < 0.0 { tex[0] } else { tex[2] },
            if m[3] < 0.0 { tex[1] } else { tex[3] },
        ];
        let (r, t) = clip_textured_rect(clip, screen, tex)?;
        return Some((r, t, QuadSpace::Screen));
    }
    let doc_clip = match clip {
        None => None,
        Some(c) => Some(map_rect_bounds(invert_matrix_2d(m)?, c)),
    };
    let (r, t) = clip_textured_rect(doc_clip, rect, tex)?;
    Some((r, t, QuadSpace::Document))
}

/// A textured quad (glyph, image tile) cut to the rectangular part of the
/// current clip: the surviving rect and its texture coordinates, scaled so the
/// texels stay where they were. `None` when nothing survives.
///
/// Textured quads used to bypass the clip stack entirely — only color quads
/// went through `collect_clipped_pieces` — so `overflow: hidden` clipped a
/// box's background but never its text (n35). The rounded part of the clip
/// is applied on top of this by `clip_textured_pieces_under`.
fn clip_textured_rect(
    clip: Option<Rect>,
    rect: Rect,
    tex: [f32; 4],
) -> Option<(Rect, [f32; 4])> {
    let clip = match clip {
        Some(clip) => clip,
        None => return Some((rect, tex)),
    };
    if rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    let clipped = rect.intersect(&clip)?;
    if clipped.width <= 0.0 || clipped.height <= 0.0 {
        return None;
    }
    let u_per_px = (tex[2] - tex[0]) / rect.width;
    let v_per_px = (tex[3] - tex[1]) / rect.height;
    let u0 = tex[0] + (clipped.x - rect.x) * u_per_px;
    let v0 = tex[1] + (clipped.y - rect.y) * v_per_px;
    let u1 = u0 + clipped.width * u_per_px;
    let v1 = v0 + clipped.height * v_per_px;
    Some((clipped, [u0, v0, u1, v1]))
}

/// Everything `rect` becomes under `clip`, appended to `out` as
/// `(piece, coverage)`.
///
/// The whole clipping decision lives here — rectangular intersection, the
/// no-rounding fast path, and the rounded decomposition — for the same reason
/// as `clip_entry_for`: a rule inside a `Renderer` method cannot be
/// mutation-checked on a machine without a GPU, and a guard that cannot fail is
/// not a guard.
fn collect_clipped_pieces(clip: Option<&ClipEntry>, rect: Rect, out: &mut Vec<(Rect, f32)>) {
    let rect = match clip {
        Some(entry) => match rect.intersect(&entry.rect) {
            Some(clipped) => clipped,
            None => return, // Fully clipped
        },
        None => rect,
    };

    let rounded = clip.map(|entry| entry.rounded.as_slice()).unwrap_or(&[]);
    if rounded.is_empty() {
        out.push((rect, 1.0));
        return;
    }
    out.extend(clip_quad_to_rounded(rect, rounded));
}

/// How far inside the ellipse with semi-axes `(h, v)` the point `(dx, dy)`
/// from its centre lies, in px: positive inside, negative outside.
///
/// A circle is exact. An ellipse has no closed-form distance, so it uses the
/// first-order estimate `(1 - k) / |grad k|` for `k = sqrt((dx/h)^2 +
/// (dy/v)^2)`, which is exact on the curve and good to a fraction of a pixel
/// in the one-pixel band the antialiasing reads.
///
/// Every rounded rasteriser (fill, border, gradient cells) measures its
/// corners with this, so they agree on where a corner's edge is.
fn ellipse_edge_distance(dx: f32, dy: f32, h: f32, v: f32) -> f32 {
    if h == v {
        return h - (dx * dx + dy * dy).sqrt();
    }
    let (nx, ny) = (dx / h, dy / v);
    let k = (nx * nx + ny * ny).sqrt();
    if k <= f32::EPSILON {
        return h.min(v);
    }
    let grad = ((nx / h).powi(2) + (ny / v).powi(2)).sqrt() / k;
    (1.0 - k) / grad
}

/// Pixel coverage for a signed distance to a corner's edge: half covered on
/// the edge, a two-pixel ramp across it.
fn corner_coverage(signed_dist: f32) -> f32 {
    if signed_dist >= 1.0 {
        1.0
    } else if signed_dist > -1.0 {
        (signed_dist * 0.5 + 0.5).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// The one-pixel cells a corner box is painted in along one axis, as
/// `(grid start, drawn start, drawn length)`.
///
/// The box runs `len` from `box_start`. Its grid is laid from the rect's
/// OUTER edge inward (`outer_at_start` says which end that is), so the cell
/// centres `grid start + 0.5` sit where the straight edge beside the corner
/// puts its pixels, whatever the radius. Laying it from the box's inner
/// corner put a right or bottom corner with a fractional radius (`25%` of
/// 150px) half a pixel off the grid: the pixels along the box's outer edge
/// were measured on the curve instead of inside it and came out half
/// transparent, a notch in the side.
///
/// The cell that crosses the box's inner end is drawn only up to it; what
/// is past it belongs to the strip beside the corner. A whole-pixel box
/// gets whole cells, the same ones from either end.
fn corner_cells(box_start: f32, len: f32, outer_at_start: bool) -> Vec<(f32, f32, f32)> {
    let box_end = box_start + len;
    let mut cells = Vec::new();
    let mut grid = if outer_at_start {
        box_start
    } else {
        box_start - (len.ceil() - len)
    };
    while grid < box_end - 0.001 {
        let start = grid.max(box_start);
        let length = (grid + 1.0).min(box_end) - start;
        if length > 0.0 {
            cells.push((grid, start, length));
        }
        grid += 1.0;
    }
    cells
}

/// Top-left of the `h` x `v` box a corner occupies in `rect`.
/// quadrant: 0=top-left, 1=top-right, 2=bottom-right, 3=bottom-left
fn corner_box_origin(rect: Rect, corner: rustkit_layout::CornerRadius, quadrant: u8) -> (f32, f32) {
    let x = if quadrant == 0 || quadrant == 3 {
        rect.x
    } else {
        rect.x + rect.width - corner.h
    };
    let y = if quadrant == 0 || quadrant == 1 {
        rect.y
    } else {
        rect.y + rect.height - corner.v
    };
    (x, y)
}

/// Whether the boxes of two diagonally opposite corners of `rect` overlap.
/// Fitting keeps adjacent corners apart, not opposite ones: `80px 0` on a
/// 100px square is a leaf whose two corner boxes share the middle.
fn corner_boxes_overlap(
    rect: Rect,
    a: rustkit_layout::CornerRadius,
    b: rustkit_layout::CornerRadius,
) -> bool {
    !a.is_zero() && !b.is_zero() && a.h + b.h > rect.width && a.v + b.v > rect.height
}

/// Signed distance of the point `(px, py)` to the edge of `corner`
/// (`quadrant` of `rect`), or `None` when the point is outside that
/// corner's box and the corner says nothing about it.
fn corner_distance_at(
    rect: Rect,
    corner: rustkit_layout::CornerRadius,
    quadrant: u8,
    px: f32,
    py: f32,
) -> Option<f32> {
    if corner.is_zero() {
        return None;
    }
    let ex = if quadrant == 0 || quadrant == 3 {
        px - rect.x
    } else {
        rect.x + rect.width - px
    };
    let ey = if quadrant == 0 || quadrant == 1 {
        py - rect.y
    } else {
        rect.y + rect.height - py
    };
    if ex >= corner.h || ey >= corner.v {
        return None;
    }
    Some(ellipse_edge_distance(
        corner.h - ex,
        corner.v - ey,
        corner.h,
        corner.v,
    ))
}

/// How far a corner's arc reaches from its centre line on the row `dy` away
/// from it: the ellipse's `(h / v) * sqrt(v^2 - dy^2)`.
fn corner_row_reach(corner: rustkit_layout::CornerRadius, dy: f32) -> f32 {
    (corner.h / corner.v) * (corner.v * corner.v - dy * dy).max(0.0).sqrt()
}

/// The horizontal span of a rounded rect at height `y`, or `None` when the row
/// is outside it entirely.
///
/// Analytic rather than sampled: for a row crossing a corner, the ellipse
/// gives `dx = (h / v) * sqrt(v^2 - dy^2)` and the span shrinks by exactly
/// that much. The left
/// bound takes the tighter of the two left corners and the right bound the
/// tighter of the two right ones, so a row crossing both a top-left and a
/// bottom-left arc is handled without special-casing.
fn rounded_row_span(rect: Rect, radius: rustkit_layout::BorderRadius, y: f32) -> Option<(f32, f32)> {
    if y < rect.y || y > rect.bottom() {
        return None;
    }
    let radius = radius.fitted(rect.width, rect.height);
    let (tl, tr, br, bl) = (
        radius.top_left,
        radius.top_right,
        radius.bottom_right,
        radius.bottom_left,
    );
    let mut left = rect.x;
    let mut right = rect.right();

    if !tl.is_zero() && y < rect.y + tl.v {
        let dx = corner_row_reach(tl, (rect.y + tl.v) - y);
        left = left.max(rect.x + tl.h - dx);
    }
    if !bl.is_zero() && y > rect.bottom() - bl.v {
        let dx = corner_row_reach(bl, y - (rect.bottom() - bl.v));
        left = left.max(rect.x + bl.h - dx);
    }
    if !tr.is_zero() && y < rect.y + tr.v {
        let dx = corner_row_reach(tr, (rect.y + tr.v) - y);
        right = right.min(rect.right() - tr.h + dx);
    }
    if !br.is_zero() && y > rect.bottom() - br.v {
        let dx = corner_row_reach(br, y - (rect.bottom() - br.v));
        right = right.min(rect.right() - br.h + dx);
    }

    if right > left {
        Some((left, right))
    } else {
        None
    }
}

/// Push one row's span as up to three pieces: a fully covered interior and an
/// antialiased cell at each end THE ARC ACTUALLY CUT.
///
/// The partial cells are what keep a clipped corner from reading as a hard
/// staircase. They use the same "coverage multiplies alpha" convention as
/// `draw_rounded_corner`, so a clipped corner and a painted rounded corner
/// antialias the same way.
///
/// `left_cut`/`right_cut` say whether that end came from an arc or is the
/// quad's own edge, and only a cut end is snapped. An uncut end must pass
/// through exactly as the no-clip path would emit it — `collect_clipped_pieces`
/// returns `(rect, 1.0)` when there is no rounding at all, and a rounded clip
/// somewhere else on the box is not a reason for this edge to move.
///
/// Snapping an uncut end is invisible on a quad whose edge is a real edge, and
/// wrong on a quad that TILES: a gradient paints as a grid of cells, and two
/// neighbouring cells' partial-coverage slivers each blend against what is
/// under them instead of summing to one. Measured on gradient-backgrounds'
/// `.linear-6`, that seamed 2164 interior pixels of a 227x180 card — a stipple
/// every cell across the rows the arc band covers, 1309 of them out of Gate B's
/// tolerance — while the corner notches the clip exists to cut were 353.
fn push_row_pieces(
    out: &mut Vec<(Rect, f32)>,
    left: f32,
    right: f32,
    y: f32,
    height: f32,
    left_cut: bool,
    right_cut: bool,
) {
    if height <= 0.0 || right <= left {
        return;
    }
    let inner_left = if left_cut { left.ceil() } else { left };
    let inner_right = if right_cut { right.floor() } else { right };

    if inner_right <= inner_left {
        // Span narrower than one pixel column: one cell carrying its coverage.
        out.push((Rect::new(left, y, right - left, height), (right - left).min(1.0)));
        return;
    }
    if inner_left > left {
        out.push((
            Rect::new(left.floor(), y, 1.0, height),
            (inner_left - left).min(1.0),
        ));
    }
    out.push((
        Rect::new(inner_left, y, inner_right - inner_left, height),
        1.0,
    ));
    if right > inner_right {
        out.push((
            Rect::new(inner_right, y, 1.0, height),
            (right - inner_right).min(1.0),
        ));
    }
}

/// Decompose `quad` into the pieces of it that survive every rounded clip.
///
/// Returns `(piece, coverage)` pairs; coverage multiplies the quad's alpha.
/// `quad` must already be intersected with the clip stack's rectangle — this
/// function only removes the corner notches.
///
/// Pure on purpose. Every other clipping path in this renderer lives on
/// `Renderer`, which needs a wgpu device, so it can only be exercised on a
/// machine with an adapter. This one is a free function over plain geometry and
/// its tests run anywhere.
/// Grid-aligned cell-index window along one axis that can reach the viewport
/// `[0, viewport_extent)`. Free function so the math is testable without a GPU
/// device; `Renderer::visible_cell_range` supplies the transform guard.
fn cell_range_for_viewport(
    axis_start: f32,
    cell_size: f32,
    count: usize,
    viewport_extent: f32,
    transform_active: bool,
) -> (usize, usize) {
    if transform_active {
        return (0, count);
    }
    let first = (((0.0 - axis_start) / cell_size).floor().max(0.0) as usize).min(count);
    let last = ((((viewport_extent - axis_start) / cell_size).ceil()).max(0.0) as usize).min(count);
    (first.min(last), last)
}

fn clip_quad_to_rounded(
    quad: Rect,
    rounded: &[(Rect, rustkit_layout::BorderRadius)],
) -> Vec<(Rect, f32)> {
    if quad.width <= 0.0 || quad.height <= 0.0 {
        return Vec::new();
    }
    if rounded.is_empty() {
        return vec![(quad, 1.0)];
    }

    // Below `top_limit` and above `bottom_limit` no corner of any constraint is
    // active, so that band passes through whole. Without this a full-page
    // rounded container would emit one quad per scanline for its entire height.
    let mut top_limit = f32::NEG_INFINITY;
    let mut bottom_limit = f32::INFINITY;
    for (rect, radius) in rounded {
        let fitted = radius.fitted(rect.width, rect.height);
        top_limit = top_limit.max(rect.y + fitted.top_left.v.max(fitted.top_right.v));
        bottom_limit =
            bottom_limit.min(rect.bottom() - fitted.bottom_left.v.max(fitted.bottom_right.v));
    }
    if quad.y >= top_limit && quad.bottom() <= bottom_limit {
        return vec![(quad, 1.0)];
    }

    let mut out = Vec::new();
    let mut emit_rows = |out: &mut Vec<(Rect, f32)>, from: f32, to: f32| {
        let mut y = from;
        while y < to {
            let height = 1.0_f32.min(to - y);
            let centre = y + height * 0.5;
            let mut left = quad.x;
            let mut right = quad.right();
            let (mut left_cut, mut right_cut) = (false, false);
            let mut inside = true;
            for (rect, radius) in rounded {
                match rounded_row_span(*rect, *radius, centre) {
                    Some((l, r)) => {
                        if l > left {
                            left = l;
                            left_cut = true;
                        }
                        if r < right {
                            right = r;
                            right_cut = true;
                        }
                    }
                    None => {
                        inside = false;
                        break;
                    }
                }
            }
            if inside {
                push_row_pieces(out, left, right, y, height, left_cut, right_cut);
            }
            y += height;
        }
    };

    let top_end = quad.bottom().min(top_limit.max(quad.y));
    emit_rows(&mut out, quad.y, top_end);

    let middle_start = top_end.max(quad.y);
    let middle_end = quad.bottom().min(bottom_limit.max(middle_start));
    if middle_end > middle_start {
        out.push((
            Rect::new(quad.x, middle_start, quad.width, middle_end - middle_start),
            1.0,
        ));
    }

    emit_rows(&mut out, middle_end.max(quad.y), quad.bottom());
    out
}

/// Whether `quad` lies wholly inside every rounded constraint. A rounded rect
/// is convex, so a quad whose top and bottom edges are both inside it is
/// inside it everywhere.
fn quad_inside_rounded(quad: Rect, rounded: &[(Rect, rustkit_layout::BorderRadius)]) -> bool {
    rounded.iter().all(|(rect, radius)| {
        [quad.y, quad.bottom()].into_iter().all(|y| {
            matches!(
                rounded_row_span(*rect, *radius, y),
                Some((left, right)) if left <= quad.x && right >= quad.right()
            )
        })
    })
}

/// A textured quad (glyph, image, background tile) drawn under transform `m`
/// and cut to `clip`, rounded corners included: each surviving piece with its
/// texture coordinates and coverage, appended to `out`. The return value says
/// which space the pieces are in.
///
/// The rectangular half is `clip_textured_under`. The rounded half is the
/// decomposition colour quads get (`clip_quad_to_rounded`), with each piece
/// taking the texels that were under it, so an `<img>` in a rounded
/// `overflow: hidden` box (an avatar, a card's cover photo) loses its corners
/// the way the box's background does. A quad clear of every corner comes back
/// as the one piece it always was.
///
/// Under a rotation or skew only the rectangular part applies, as for colour
/// quads.
fn clip_textured_pieces_under(
    m: [f32; 6],
    clip: Option<&ClipEntry>,
    rect: Rect,
    tex: [f32; 4],
    out: &mut Vec<(Rect, [f32; 4], f32)>,
) -> QuadSpace {
    let Some((quad, tex, space)) = clip_textured_under(m, clip.map(|entry| entry.rect), rect, tex)
    else {
        return QuadSpace::Screen;
    };
    let rounded = match (space, clip) {
        (QuadSpace::Screen, Some(entry)) => entry.rounded.as_slice(),
        _ => &[],
    };
    if rounded.is_empty() || quad_inside_rounded(quad, rounded) {
        out.push((quad, tex, 1.0));
        return space;
    }

    let u_per_px = (tex[2] - tex[0]) / quad.width;
    let v_per_px = (tex[3] - tex[1]) / quad.height;
    for (piece, coverage) in clip_quad_to_rounded(quad, rounded) {
        // An antialiased end cell is a whole pixel and can start before the
        // quad does; a piece only has texels where the quad is.
        let Some(piece) = piece.intersect(&quad) else {
            continue;
        };
        if piece.width <= 0.0 || piece.height <= 0.0 {
            continue;
        }
        let u0 = tex[0] + (piece.x - quad.x) * u_per_px;
        let v0 = tex[1] + (piece.y - quad.y) * v_per_px;
        out.push((
            piece,
            [u0, v0, u0 + piece.width * u_per_px, v0 + piece.height * v_per_px],
            coverage,
        ));
    }
    space
}

/// How much of the pixel centred on `(px, py)` the rounded rect covers: 1
/// inside, 0 outside, and the fill's own ramp across a corner's curve
/// (`corner_coverage`), so a shape cut out with this and the same shape
/// filled by `draw_rounded_rect` meet on the curve. `radius` must be fitted.
fn rounded_rect_coverage(rect: Rect, radius: rustkit_layout::BorderRadius, px: f32, py: f32) -> f32 {
    if px < rect.x || px >= rect.right() || py < rect.y || py >= rect.bottom() {
        return 0.0;
    }
    let mut coverage = 1.0_f32;
    for (quadrant, corner) in [
        radius.top_left,
        radius.top_right,
        radius.bottom_right,
        radius.bottom_left,
    ]
    .into_iter()
    .enumerate()
    {
        if let Some(distance) = corner_distance_at(rect, corner, quadrant as u8, px, py) {
            coverage = coverage.min(corner_coverage(distance));
        }
    }
    coverage
}

/// The part of the rounded rect `shape` that is outside the rounded rect
/// `hole`, as `(piece, coverage)`: an outer box shadow's layer with the box
/// cut out of it (CSS Backgrounds 3 §6.1).
///
/// Two square shapes give `rect_minus` exactly. Otherwise the plane is cut
/// along every edge of both rects and of their corner boxes: a cell outside
/// every corner box is wholly painted or wholly not, and a cell in one is
/// painted a pixel at a time, runs of fully covered pixels joined.
///
/// Pure for the same reason as `clip_quad_to_rounded`.
fn rounded_difference_pieces(
    shape: Rect,
    shape_radius: rustkit_layout::BorderRadius,
    hole: Rect,
    hole_radius: rustkit_layout::BorderRadius,
) -> Vec<(Rect, f32)> {
    if shape.width <= 0.0 || shape.height <= 0.0 {
        return Vec::new();
    }
    if shape_radius.is_zero() && hole_radius.is_zero() {
        return Renderer::rect_minus(shape, hole)
            .into_iter()
            .map(|r| (r, 1.0))
            .collect();
    }
    let shape_radius = shape_radius.fitted(shape.width, shape.height);
    let hole_radius = hole_radius.fitted(hole.width, hole.height);
    let coverage = |x: f32, y: f32| {
        rounded_rect_coverage(shape, shape_radius, x, y)
            * (1.0 - rounded_rect_coverage(hole, hole_radius, x, y))
    };

    // The corner boxes of both shapes, and the cuts along both axes.
    let mut corner_boxes = Vec::new();
    let mut xs = vec![shape.x, shape.right()];
    let mut ys = vec![shape.y, shape.bottom()];
    for (rect, radius) in [(shape, shape_radius), (hole, hole_radius)] {
        xs.extend([rect.x, rect.right()]);
        ys.extend([rect.y, rect.bottom()]);
        for (quadrant, corner) in [
            radius.top_left,
            radius.top_right,
            radius.bottom_right,
            radius.bottom_left,
        ]
        .into_iter()
        .enumerate()
        {
            if corner.is_zero() {
                continue;
            }
            let (x, y) = corner_box_origin(rect, corner, quadrant as u8);
            corner_boxes.push(Rect::new(x, y, corner.h, corner.v));
            xs.extend([x, x + corner.h]);
            ys.extend([y, y + corner.v]);
        }
    }
    let cuts = |mut values: Vec<f32>, from: f32, to: f32| {
        for v in values.iter_mut() {
            *v = v.clamp(from, to);
        }
        values.sort_by(|a, b| a.total_cmp(b));
        values.dedup();
        values
    };
    let xs = cuts(xs, shape.x, shape.right());
    let ys = cuts(ys, shape.y, shape.bottom());

    let mut out: Vec<(Rect, f32)> = Vec::new();
    for band in ys.windows(2) {
        let (y0, y1) = (band[0], band[1]);
        // The whole cell of this band still open on its right, if any: cells
        // side by side in a band join into one rect.
        let mut open: Option<usize> = None;
        for span in xs.windows(2) {
            let (x0, x1) = (span[0], span[1]);
            let (cx, cy) = ((x0 + x1) * 0.5, (y0 + y1) * 0.5);
            if !corner_boxes.iter().any(|b| b.contains(cx, cy)) {
                if coverage(cx, cy) <= 0.0 {
                    open = None;
                } else if let Some(index) = open {
                    out[index].0.width = x1 - out[index].0.x;
                } else {
                    open = Some(out.len());
                    out.push((Rect::new(x0, y0, x1 - x0, y1 - y0), 1.0));
                }
                continue;
            }
            open = None;

            // A cell in a corner box: one pixel at a time on the pixel grid,
            // each drawn only as far as the cell reaches.
            let mut row = y0.floor();
            while row < y1 {
                let top = row.max(y0);
                let height = (row + 1.0).min(y1) - top;
                let mut run: Option<usize> = None;
                let mut column = x0.floor();
                while column < x1 {
                    let left = column.max(x0);
                    let width = (column + 1.0).min(x1) - left;
                    let c = coverage(column + 0.5, row + 0.5);
                    if c >= 1.0 {
                        match run {
                            Some(index) => out[index].0.width = left + width - out[index].0.x,
                            None => {
                                run = Some(out.len());
                                out.push((Rect::new(left, top, width, height), 1.0));
                            }
                        }
                    } else {
                        run = None;
                        if c > 0.01 {
                            out.push((Rect::new(left, top, width, height), c));
                        }
                    }
                    column += 1.0;
                }
                row += 1.0;
            }
        }
    }
    out
}

// ==================== Transform Helpers ====================

/// The page-space affine a `PushTransform { matrix, origin }` command means:
/// `matrix` applied about `origin` (css-transforms-1 §6), which is
/// `T(origin) · matrix · T(-origin)` — move the origin to (0,0), transform,
/// move it back — in the column-vector convention `multiply_matrices_2d` uses
/// (`a · b` applies `b` first). The origin is a fixed point of the result.
///
/// Until n48 the two translations were composed the other way round,
/// `T(-origin) · matrix · T(origin)`, so a point went `p ↦ M·(p + o) − o`
/// instead of `M·(p − o) + o`. Translations commute with each other, so every
/// `translate()` on every board case was unaffected and the bug hid for the
/// whole campaign; a `scale()` or `rotate()` landed its box at `M·o − o` away
/// from where it belonged — a 60×20 card at (110, 20) with `scale(2);
/// transform-origin: 0 0` painted at (330, 60), and n47's repro section E
/// "painted nothing" because its box went to y = 900, off the frame. The
/// engine's geometry oracle (`own_transform_affine`) had the right order all
/// along, so the exported layout rect and the painted pixels disagreed for
/// every scaled or rotated box.
fn affine_about_origin(matrix: [f32; 6], origin: (f32, f32)) -> [f32; 6] {
    let to_origin = [1.0, 0.0, 0.0, 1.0, -origin.0, -origin.1];
    let from_origin = [1.0, 0.0, 0.0, 1.0, origin.0, origin.1];
    multiply_matrices_2d(from_origin, multiply_matrices_2d(matrix, to_origin))
}

/// Multiply two 2D affine matrices.
/// Matrix format: [a, b, c, d, e, f] representing:
/// | a c e |
/// | b d f |
/// | 0 0 1 |
fn multiply_matrices_2d(a: [f32; 6], b: [f32; 6]) -> [f32; 6] {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_oversized_image_is_downscaled_to_the_limit_keeping_its_aspect() {
        // 20x10 solid red, limit 8: longest side becomes 8, the other 4.
        let data = vec![255u8, 0, 0, 255].repeat(20 * 10);
        let (w, h, px) = super::downscale_rgba_to_fit(20, 10, &data, 8);
        assert_eq!((w, h), (8, 4));
        assert_eq!(px.len(), 8 * 4 * 4);
        assert!(px.chunks_exact(4).all(|p| p == [255, 0, 0, 255]));
    }

    #[test]
    fn downscaling_averages_the_source_pixels_it_covers() {
        // 2x1: black and white, to 1x1: mid grey.
        let data = vec![0, 0, 0, 255, 255, 255, 255, 255];
        let (w, h, px) = super::downscale_rgba_to_fit(2, 1, &data, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(px, vec![128, 128, 128, 255]);
    }

    #[test]
    fn an_image_within_the_limit_is_left_alone() {
        let data = vec![7u8; 3 * 2 * 4];
        let (w, h, px) = super::downscale_rgba_to_fit(3, 2, &data, 8);
        assert_eq!((w, h, px), (3, 2, data));
    }

    use super::*;

    // ==================== Transform origin (n48) ====================

    fn map(m: [f32; 6], x: f32, y: f32) -> (f32, f32) {
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    /// A rect's corners mapped through an axis-aligned affine, as
    /// `(x, y, width, height)`.
    fn map_box(m: [f32; 6], x: f32, y: f32, w: f32, h: f32) -> (f32, f32, f32, f32) {
        let (x0, y0) = map(m, x, y);
        let (x1, y1) = map(m, x + w, y + h);
        (x0.min(x1), y0.min(y1), (x1 - x0).abs(), (y1 - y0).abs())
    }

    #[test]
    fn a_scale_keeps_its_transform_origin_fixed() {
        // A 60x20 card at (110, 20) with `scale(2); transform-origin: 0 0`
        // (n47's repro section E, scale-variants row 1): the top-left corner
        // is the origin and must not move; the far corner doubles away from
        // it. The old order sent the box to (330, 60) — and section E to
        // y = 900, off the frame.
        let m = affine_about_origin([2.0, 0.0, 0.0, 2.0, 0.0, 0.0], (110.0, 20.0));
        assert_eq!(map(m, 110.0, 20.0), (110.0, 20.0));
        assert_eq!(map(m, 170.0, 40.0), (230.0, 60.0));
        assert_eq!(map_box(m, 110.0, 20.0, 60.0, 20.0), (110.0, 20.0, 120.0, 40.0));
    }

    #[test]
    fn a_scale_about_the_centre_grows_evenly() {
        // `transform-origin: 50% 50%` (the default): the centre is fixed and
        // the box grows the same amount on every side.
        let m = affine_about_origin([2.0, 0.0, 0.0, 2.0, 0.0, 0.0], (140.0, 30.0));
        assert_eq!(map_box(m, 110.0, 20.0, 60.0, 20.0), (80.0, 10.0, 120.0, 40.0));
    }

    #[test]
    fn a_rotation_turns_about_its_origin() {
        // rotate(90deg) about (100, 100): (100, 0) — straight above the
        // origin — goes to (200, 100), straight to its right.
        let m = affine_about_origin([0.0, 1.0, -1.0, 0.0, 0.0, 0.0], (100.0, 100.0));
        let (x, y) = map(m, 100.0, 0.0);
        assert!((x - 200.0).abs() < 1e-4 && (y - 100.0).abs() < 1e-4, "{x} {y}");
    }

    #[test]
    fn a_translate_ignores_its_origin() {
        // Translations commute, so the origin never mattered for them — the
        // pixels every translate() board case emitted stay exactly the same.
        let t = [1.0, 0.0, 0.0, 1.0, 40.0, -7.0];
        assert_eq!(affine_about_origin(t, (0.0, 0.0)), t);
        assert_eq!(affine_about_origin(t, (123.0, 456.0)), t);
    }

    #[test]
    fn nested_transforms_apply_inner_first() {
        // A scaled child inside a translated parent: the child scales about
        // its own origin in page space, then the parent's translate moves the
        // result — `outer · inner`, as `current_transform` composes the stack.
        let outer = affine_about_origin([1.0, 0.0, 0.0, 1.0, 50.0, 0.0], (0.0, 0.0));
        let inner = affine_about_origin([2.0, 0.0, 0.0, 2.0, 0.0, 0.0], (110.0, 20.0));
        let m = multiply_matrices_2d(outer, inner);
        assert_eq!(map(m, 110.0, 20.0), (160.0, 20.0));
        assert_eq!(map(m, 170.0, 40.0), (280.0, 60.0));
    }

    // ==================== Textured-quad clipping (n35) ====================

    #[test]
    fn no_clip_leaves_a_textured_quad_untouched() {
        let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
        let tex = [0.1, 0.2, 0.3, 0.4];
        let (r, t) = clip_textured_rect(None, rect, tex).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (10.0, 20.0, 30.0, 40.0));
        assert_eq!(t, tex);
    }

    #[test]
    fn a_glyph_below_an_overflow_hidden_box_is_dropped_entirely() {
        // overflow-wrap-anywhere-002's shape: a 1em-tall clipper, the second
        // line's glyphs start at its bottom edge.
        let clip = Rect::new(8.0, 60.0, 40.0, 16.0);
        let glyph = Rect::new(8.0, 76.0, 10.0, 14.0);
        assert!(clip_textured_rect(Some(clip), glyph, [0.0, 0.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn a_glyph_straddling_the_clip_edge_keeps_only_the_inside_and_its_texels() {
        // Clip cuts the glyph's lower half: the surviving quad is the top
        // half and its v range is the top half of the original, so the atlas
        // texels do not stretch to fill the smaller quad.
        let clip = Rect::new(0.0, 0.0, 100.0, 20.0);
        let glyph = Rect::new(4.0, 10.0, 10.0, 20.0);
        let tex = [0.5, 0.0, 0.6, 0.4];
        let (r, t) = clip_textured_rect(Some(clip), glyph, tex).unwrap();
        assert_eq!((r.x, r.y, r.width, r.height), (4.0, 10.0, 10.0, 10.0));
        assert!((t[0] - 0.5).abs() < 1e-6 && (t[2] - 0.6).abs() < 1e-6, "u untouched: {t:?}");
        assert!((t[1] - 0.0).abs() < 1e-6 && (t[3] - 0.2).abs() < 1e-6, "v halved: {t:?}");
    }

    #[test]
    fn a_left_cut_shifts_the_u_origin() {
        let clip = Rect::new(5.0, 0.0, 100.0, 100.0);
        let glyph = Rect::new(0.0, 0.0, 10.0, 10.0);
        let (r, t) = clip_textured_rect(Some(clip), glyph, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!((r.x, r.width), (5.0, 5.0));
        assert!((t[0] - 0.5).abs() < 1e-6 && (t[2] - 1.0).abs() < 1e-6, "{t:?}");
    }

    #[test]
    fn test_color_vertex_size() {
        assert_eq!(std::mem::size_of::<ColorVertex>(), 24);
    }

    #[test]
    fn test_texture_vertex_size() {
        assert_eq!(std::mem::size_of::<TextureVertex>(), 32);
    }

    #[test]
    fn test_uniforms_size() {
        assert_eq!(std::mem::size_of::<Uniforms>(), 16);
    }

    #[test]
    fn test_rect_intersect() {
        let a = Rect::new(0.0, 0.0, 100.0, 100.0);
        let b = Rect::new(50.0, 50.0, 100.0, 100.0);

        let result = a.intersect(&b).unwrap();
        assert_eq!(result.x, 50.0);
        assert_eq!(result.y, 50.0);
        assert_eq!(result.width, 50.0);
        assert_eq!(result.height, 50.0);
    }

    #[test]
    fn test_rect_no_intersect() {
        let a = Rect::new(0.0, 0.0, 50.0, 50.0);
        let b = Rect::new(100.0, 100.0, 50.0, 50.0);

        assert!(a.intersect(&b).is_none());
    }

    // ==================== Rounded clip ====================
    //
    // These exercise `clip_quad_to_rounded` directly. It is a free function
    // over geometry precisely so these run without a wgpu adapter — every
    // other clipping path needs a `Renderer`, which needs a device, which this
    // runner does not have.

    fn radius(r: f32) -> rustkit_layout::BorderRadius {
        rustkit_layout::BorderRadius::uniform(r)
    }

    /// Total area the pieces cover, weighted by coverage.
    fn covered_area(pieces: &[(Rect, f32)]) -> f32 {
        pieces
            .iter()
            .map(|(r, cov)| r.width * r.height * cov)
            .sum()
    }

    /// Does any piece put paint at (x, y) with coverage above `floor`?
    fn painted_at(pieces: &[(Rect, f32)], x: f32, y: f32, floor: f32) -> bool {
        pieces
            .iter()
            .any(|(r, cov)| *cov > floor && r.contains(x, y))
    }

    fn pieces_under(clip: Option<&ClipEntry>, rect: Rect) -> Vec<(Rect, f32)> {
        let mut out = Vec::new();
        collect_clipped_pieces(clip, rect, &mut out);
        out
    }

    #[test]
    fn a_rounded_clip_reaches_the_quads_drawn_under_it() {
        // The wiring, not the geometry. Before this, `overflow: hidden` pushed
        // nothing and every quad under a rounded box came out square; a
        // decomposition nobody calls fixes nothing.
        let box_rect = Rect::new(0.0, 0.0, 200.0, 200.0);
        let entry = clip_entry_for(None, box_rect, radius(12.0));
        let pieces = pieces_under(Some(&entry), box_rect);
        assert!(
            !painted_at(&pieces, 1.0, 1.0, 0.0),
            "a quad drawn under a rounded clip must lose its corner"
        );
        assert!(
            pieces.len() > 1,
            "a rounded clip must decompose the quad, got {} piece(s)",
            pieces.len()
        );
    }

    #[test]
    fn a_square_clip_leaves_the_quad_whole() {
        // The fast path has to stay a fast path: one piece, full coverage, the
        // same vertices the renderer emitted before rounded clips existed.
        let entry = clip_entry_for(None, Rect::new(0.0, 0.0, 200.0, 200.0), radius(0.0));
        let pieces = pieces_under(Some(&entry), Rect::new(10.0, 10.0, 50.0, 50.0));
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].1, 1.0);
        assert_eq!(pieces[0].0.width, 50.0);
    }

    #[test]
    fn a_quad_outside_the_clip_rect_emits_nothing() {
        let entry = clip_entry_for(None, Rect::new(0.0, 0.0, 100.0, 100.0), radius(0.0));
        assert!(pieces_under(Some(&entry), Rect::new(200.0, 200.0, 10.0, 10.0)).is_empty());
    }

    #[test]
    fn no_clip_at_all_emits_the_quad_unchanged() {
        let pieces = pieces_under(None, Rect::new(5.0, 6.0, 7.0, 8.0));
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].0.x, 5.0);
        assert_eq!(pieces[0].1, 1.0);
    }

    #[test]
    fn pushing_a_clip_intersects_the_rect_and_keeps_the_outer_arc() {
        // Both halves of the stack rule at once: rects intersect, rounded
        // constraints accumulate. Dropping the outer arc is invisible in the
        // rect and shows up only at the corner.
        let outer = clip_entry_for(None, Rect::new(0.0, 0.0, 200.0, 200.0), radius(20.0));
        let inner = clip_entry_for(Some(&outer), Rect::new(0.0, 0.0, 100.0, 100.0), radius(0.0));

        assert_eq!(inner.rect.width, 100.0, "rects must intersect");
        assert_eq!(
            inner.rounded.len(),
            1,
            "the outer arc must survive an inner square clip"
        );

        let pieces = pieces_under(Some(&inner), Rect::new(0.0, 0.0, 100.0, 100.0));
        assert!(
            !painted_at(&pieces, 1.0, 1.0, 0.0),
            "the outer box's corner must still cut quads drawn under the inner clip"
        );
    }

    #[test]
    fn a_square_clip_records_no_rounded_constraint() {
        // Control for the accumulation rule: pushing squares must not grow the
        // rounded list, or every clip in the corpus would take the slow path.
        let entry = clip_entry_for(None, Rect::new(0.0, 0.0, 200.0, 200.0), radius(0.0));
        assert!(entry.rounded.is_empty());
    }

    #[test]
    fn a_clip_that_intersects_to_nothing_clips_everything_away() {
        let outer = clip_entry_for(None, Rect::new(0.0, 0.0, 50.0, 50.0), radius(0.0));
        let inner = clip_entry_for(Some(&outer), Rect::new(500.0, 500.0, 50.0, 50.0), radius(0.0));
        assert_eq!(inner.rect.width, 0.0);
        assert!(pieces_under(Some(&inner), Rect::new(0.0, 0.0, 50.0, 50.0)).is_empty());
    }

    // ==================== Clip vs transform order ====================
    //
    // The clip stack is in screen space and quads are mapped before they are
    // clipped. These pin the order with the about page's own idiom:
    // `.sponsor-btn { overflow: hidden }` holding
    // `::before { inset: 0; transform: translateX(-100%) }`.

    fn translate(x: f32, y: f32) -> [f32; 6] {
        [1.0, 0.0, 0.0, 1.0, x, y]
    }

    fn quad_pieces_under(m: [f32; 6], clip: Option<&ClipEntry>, rect: Rect) -> (Vec<(Rect, f32)>, QuadSpace) {
        let mut out = Vec::new();
        let space = clip_quad_under(m, clip, rect, &mut out);
        (out, space)
    }

    #[test]
    fn a_descendant_translated_out_of_its_clipper_paints_nothing() {
        // The shine bar: the button clips to its own box under identity, the
        // pseudo box is the button's size and translated a full width left.
        // Before this it was clipped in document space (fully inside) and
        // then moved — a 230x50 bar painted left of the button.
        let button = Rect::new(300.0, 240.0, 230.0, 50.0);
        let clip = clip_entry_under(None, IDENTITY_2D, button, radius(8.0));
        let (pieces, _) = quad_pieces_under(translate(-230.0, 0.0), Some(&clip), button);
        assert!(
            pieces.is_empty(),
            "a quad translated wholly outside its clipper must emit nothing, got {pieces:?}"
        );
    }

    #[test]
    fn a_partly_translated_descendant_keeps_only_the_part_inside_and_in_screen_space() {
        let button = Rect::new(0.0, 0.0, 200.0, 50.0);
        let clip = clip_entry_under(None, IDENTITY_2D, button, radius(0.0));
        let (pieces, space) = quad_pieces_under(translate(-150.0, 0.0), Some(&clip), button);
        assert_eq!(space, QuadSpace::Screen);
        assert_eq!(pieces.len(), 1);
        let (piece, cov) = pieces[0];
        assert_eq!(cov, 1.0);
        assert_eq!((piece.x, piece.width), (0.0, 50.0), "screen-space piece: the 50px that overlap");
    }

    #[test]
    fn a_clip_pushed_under_a_transform_moves_with_it() {
        // The other half of the rule: a transformed box that clips its own
        // children clips them where the box is, not where it was laid out.
        let entry = clip_entry_under(None, translate(100.0, 20.0), Rect::new(0.0, 0.0, 50.0, 50.0), radius(0.0));
        assert_eq!((entry.rect.x, entry.rect.y), (100.0, 20.0));
        // A child drawn under the same transform lands inside it.
        let (pieces, _) = quad_pieces_under(translate(100.0, 20.0), Some(&entry), Rect::new(10.0, 10.0, 10.0, 10.0));
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0].0.x, pieces[0].0.y), (110.0, 30.0));
    }

    #[test]
    fn a_scaled_clip_scales_its_corner_radius() {
        let entry = clip_entry_under(None, [2.0, 0.0, 0.0, 2.0, 0.0, 0.0], Rect::new(0.0, 0.0, 50.0, 50.0), radius(10.0));
        assert_eq!(entry.rect.width, 100.0);
        assert_eq!(entry.rounded.len(), 1);
        assert_eq!(entry.rounded[0].1.top_left, rustkit_layout::CornerRadius::circular(20.0));
    }

    #[test]
    fn a_clip_scaled_unevenly_scales_each_axis_of_its_radius() {
        // A circular corner under scale(2, 3) is an ellipse on screen. One
        // scalar per corner could only take the geometric mean for both.
        let entry = clip_entry_under(
            None,
            [2.0, 0.0, 0.0, 3.0, 0.0, 0.0],
            Rect::new(0.0, 0.0, 50.0, 50.0),
            radius(10.0),
        );
        assert_eq!((entry.rect.width, entry.rect.height), (100.0, 150.0));
        assert_eq!(
            entry.rounded[0].1.top_left,
            rustkit_layout::CornerRadius { h: 20.0, v: 30.0 }
        );
    }

    #[test]
    fn without_a_transform_the_pieces_are_exactly_the_old_ones() {
        // The no-transform page must emit the vertices it always did.
        let clip = clip_entry_for(None, Rect::new(0.0, 0.0, 100.0, 100.0), radius(12.0));
        let rect = Rect::new(-10.0, -10.0, 60.0, 60.0);
        let old = pieces_under(Some(&clip), rect);
        let (new, space) = quad_pieces_under(IDENTITY_2D, Some(&clip), rect);
        assert_eq!(space, QuadSpace::Screen);
        assert_eq!(old.len(), new.len());
        for (a, b) in old.iter().zip(new.iter()) {
            assert_eq!((a.0.x, a.0.y, a.0.width, a.0.height, a.1), (b.0.x, b.0.y, b.0.width, b.0.height, b.1));
        }
    }

    #[test]
    fn a_rotated_quad_falls_back_to_document_space_clipping() {
        // 90 degrees about the origin: not axis-aligned, so the quad is
        // clipped against the clip's document-space bounds and handed back for
        // the emitter to transform — no worse than before this existed.
        let rot = [0.0, 1.0, -1.0, 0.0, 0.0, 0.0];
        let clip = clip_entry_under(None, IDENTITY_2D, Rect::new(-100.0, 0.0, 100.0, 100.0), radius(0.0));
        // x' = -y, y' = x: the screen clip maps back to x in [0, 100],
        // y in [0, 100]. A quad at y in [-200, -100] misses it entirely.
        let (pieces, space) = quad_pieces_under(rot, Some(&clip), Rect::new(0.0, -200.0, 50.0, 100.0));
        assert_eq!(space, QuadSpace::Document);
        assert!(pieces.is_empty(), "{pieces:?}");
        // A quad at y in [-50, 50] keeps its [0, 50] half.
        let (pieces, _) = quad_pieces_under(rot, Some(&clip), Rect::new(0.0, -50.0, 50.0, 100.0));
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0].0.y, pieces[0].0.height), (0.0, 50.0));
    }

    #[test]
    fn invert_matrix_2d_round_trips() {
        let m = [2.0, 0.5, -0.25, 3.0, 40.0, -7.0];
        let inv = invert_matrix_2d(m).unwrap();
        let id = multiply_matrices_2d(m, inv);
        for (a, b) in id.iter().zip(IDENTITY_2D.iter()) {
            assert!((a - b).abs() < 1e-5, "{id:?}");
        }
        assert!(invert_matrix_2d([0.0, 0.0, 0.0, 0.0, 1.0, 1.0]).is_none());
    }

    #[test]
    fn a_translated_glyph_is_clipped_where_it_lands() {
        // Text under a transformed descendant follows the same law as color.
        let clip = Some(Rect::new(0.0, 0.0, 100.0, 50.0));
        let glyph = Rect::new(90.0, 10.0, 20.0, 20.0);
        // Untransformed: half the glyph survives.
        let (r, _, space) = clip_textured_under(IDENTITY_2D, clip, glyph, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(space, QuadSpace::Screen);
        assert_eq!(r.width, 10.0);
        // Moved 20px right: nothing does.
        assert!(clip_textured_under(translate(20.0, 0.0), clip, glyph, [0.0, 0.0, 1.0, 1.0]).is_none());
        // Moved 20px left: whole glyph, in screen space, texels intact.
        let (r, t, space) = clip_textured_under(translate(-20.0, 0.0), clip, glyph, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(space, QuadSpace::Screen);
        assert_eq!((r.x, r.width), (70.0, 20.0));
        assert_eq!(t, [0.0, 0.0, 1.0, 1.0]);
    }

    // ==================== Rounded clip on textured quads ====================

    fn textured_pieces_under(
        clip: Option<&ClipEntry>,
        rect: Rect,
        tex: [f32; 4],
    ) -> Vec<(Rect, [f32; 4], f32)> {
        let mut out = Vec::new();
        clip_textured_pieces_under(IDENTITY_2D, clip, rect, tex, &mut out);
        out
    }

    /// An avatar: an image filling a circular `overflow: hidden` box. Its
    /// corners used to survive, because only the clip's rect reached a
    /// textured quad.
    #[test]
    fn an_image_under_a_round_clip_loses_its_corners_and_keeps_its_texels() {
        let box_rect = Rect::new(20.0, 10.0, 100.0, 100.0);
        let entry = clip_entry_for(None, box_rect, radius(50.0));
        let pieces = textured_pieces_under(Some(&entry), box_rect, [0.0, 0.0, 1.0, 1.0]);
        assert!(pieces.len() > 1, "the image was not decomposed");

        let holds = |x: f32, y: f32| {
            pieces
                .iter()
                .any(|(r, _, coverage)| *coverage > 0.0 && r.contains(x, y))
        };
        assert!(!holds(22.0, 12.0), "the top-left corner is still painted");
        assert!(!holds(118.0, 108.0), "the bottom-right corner is still painted");
        assert!(holds(70.0, 60.0), "the centre is gone");

        // Every piece samples the texels that were under it in the whole quad.
        for (r, tex, coverage) in &pieces {
            assert!(*coverage > 0.0 && *coverage <= 1.0);
            let expected = [
                (r.x - 20.0) / 100.0,
                (r.y - 10.0) / 100.0,
                (r.right() - 20.0) / 100.0,
                (r.bottom() - 10.0) / 100.0,
            ];
            for (got, want) in tex.iter().zip(expected) {
                assert!((got - want).abs() < 1e-4, "{r:?}: tex {tex:?}, expected {expected:?}");
                assert!((-1e-4..=1.0 + 1e-4).contains(got), "texel {got} outside the image");
            }
        }

        // What survives is the disc.
        let area: f32 = pieces.iter().map(|(r, _, c)| r.width * r.height * c).sum();
        let disc = std::f32::consts::PI * 50.0 * 50.0;
        assert!((area - disc).abs() < disc * 0.02, "area {area}, disc {disc}");
    }

    /// A sub-rect of an atlas keeps its own texel range through the split.
    #[test]
    fn a_split_glyph_stays_inside_its_atlas_cell() {
        let box_rect = Rect::new(0.0, 0.0, 100.0, 40.0);
        let entry = clip_entry_for(None, box_rect, radius(20.0));
        let glyph = Rect::new(0.5, 4.0, 20.0, 30.0);
        let cell = [0.25, 0.5, 0.35, 0.65];
        let pieces = textured_pieces_under(Some(&entry), glyph, cell);
        assert!(pieces.len() > 1);
        for (r, tex, _) in &pieces {
            assert!(r.x >= glyph.x - 1e-4 && r.right() <= glyph.right() + 1e-4, "{r:?}");
            assert!(tex[0] >= cell[0] - 1e-5 && tex[2] <= cell[2] + 1e-5, "{tex:?}");
            assert!(tex[1] >= cell[1] - 1e-5 && tex[3] <= cell[3] + 1e-5, "{tex:?}");
        }
    }

    /// Text in a pill sits in the band the corners occupy but clear of both
    /// arcs: it is one quad, as it was, not one per scanline.
    #[test]
    fn a_glyph_between_the_arcs_of_a_pill_is_not_split() {
        let box_rect = Rect::new(0.0, 0.0, 150.0, 40.0);
        let entry = clip_entry_for(None, box_rect, radius(20.0));
        let glyph = Rect::new(60.0, 5.0, 20.0, 30.0);
        let tex = [0.1, 0.2, 0.3, 0.4];
        let pieces = textured_pieces_under(Some(&entry), glyph, tex);
        assert_eq!(pieces.len(), 1);
        let (r, t, coverage) = pieces[0];
        assert_eq!((r.x, r.y, r.width, r.height), (60.0, 5.0, 20.0, 30.0));
        assert_eq!(t, tex);
        assert_eq!(coverage, 1.0);
    }

    /// No rounded clip, no change: the single piece `clip_textured_under`
    /// always gave.
    #[test]
    fn a_textured_quad_under_a_square_clip_is_the_one_piece_it_was() {
        let entry = clip_entry_for(None, Rect::new(0.0, 0.0, 50.0, 50.0), radius(0.0));
        let glyph = Rect::new(40.0, 10.0, 20.0, 20.0);
        let pieces = textured_pieces_under(Some(&entry), glyph, [0.0, 0.0, 1.0, 1.0]);
        let (r, t, _) = clip_textured_under(IDENTITY_2D, Some(entry.rect), glyph, [0.0, 0.0, 1.0, 1.0]).unwrap();
        assert_eq!(pieces.len(), 1);
        assert_eq!((pieces[0].0.x, pieces[0].0.width, pieces[0].1, pieces[0].2), (r.x, r.width, t, 1.0));
        assert!(textured_pieces_under(Some(&entry), Rect::new(60.0, 0.0, 10.0, 10.0), [0.0, 0.0, 1.0, 1.0]).is_empty());
    }

    /// Images are composited source-over. With the blit pipeline's REPLACE a
    /// transparent PNG painted black where it should show the page.
    #[test]
    fn images_are_blended_not_copied() {
        assert_eq!(pipeline::IMAGE_BLEND, wgpu::BlendState::ALPHA_BLENDING);
        assert_ne!(pipeline::IMAGE_BLEND, wgpu::BlendState::REPLACE);
    }

    #[test]
    fn no_rounded_constraint_passes_the_quad_through_untouched() {
        let quad = Rect::new(10.0, 10.0, 100.0, 50.0);
        let pieces = clip_quad_to_rounded(quad, &[]);
        assert_eq!(pieces.len(), 1);
        assert_eq!(pieces[0].1, 1.0);
        assert_eq!(pieces[0].0.width, 100.0);
        assert_eq!(pieces[0].0.height, 50.0);
    }

    #[test]
    fn the_corner_notch_is_not_painted() {
        // image-gallery's shape: a 12px-radius box whose child fills it exactly.
        // Before rounded clipping the child painted the full square and the
        // notch carried the child's fill — Gate B's `missing_clip`.
        let clip = Rect::new(0.0, 0.0, 200.0, 200.0);
        let pieces = clip_quad_to_rounded(clip, &[(clip, radius(12.0))]);

        // (1,1) is deep inside the top-left arc: 12 - sqrt(12^2 - 11^2) ~= 7.2,
        // so x must be >= ~7.2 at that row. Nothing may paint there.
        assert!(
            !painted_at(&pieces, 1.0, 1.0, 0.0),
            "top-left notch was painted"
        );
        assert!(
            !painted_at(&pieces, 198.5, 1.0, 0.0),
            "top-right notch was painted"
        );
        assert!(
            !painted_at(&pieces, 1.0, 198.5, 0.0),
            "bottom-left notch was painted"
        );
        assert!(
            !painted_at(&pieces, 198.5, 198.5, 0.0),
            "bottom-right notch was painted"
        );
    }

    #[test]
    fn the_interior_is_still_fully_painted() {
        // The other half of the failure mode: a clip that eats paint it should
        // have kept is worse than no clip at all.
        let clip = Rect::new(0.0, 0.0, 200.0, 200.0);
        let pieces = clip_quad_to_rounded(clip, &[(clip, radius(12.0))]);

        for (x, y) in [
            (100.0, 100.0), // centre
            (0.5, 100.0),   // left edge, no corner active
            (199.5, 100.0), // right edge
            (100.0, 0.5),   // top edge between the corners
            (100.0, 199.5), // bottom edge
            (12.5, 12.5),   // just inside the top-left arc
        ] {
            assert!(
                painted_at(&pieces, x, y, 0.999),
                "({x}, {y}) should be fully painted"
            );
        }
    }

    // ---------- an end the arc did not cut is not the arc's to move ----------
    //
    // A gradient paints as a grid of cells. Put a rounded clip over one and
    // every cell inside the arc band got both its ends snapped to the pixel
    // grid and re-emitted as partial-coverage slivers — so two neighbours'
    // slivers each blended against what was under them instead of summing to
    // one, and the card stippled every cell. On gradient-backgrounds'
    // `.linear-6` that was 2164 interior pixels against 353 in the notches the
    // clip exists to cut.

    /// One cell of a tiled paint source, sitting well inside the arc's span.
    const TILE_W: f32 = 3.0;

    #[test]
    fn an_end_the_arc_did_not_cut_keeps_the_quads_own_edge() {
        let clip = Rect::new(0.0, 0.0, 200.0, 200.0);
        // A row inside the top arc band (y < 20), but an x-span the arc at
        // that row does not reach: the corner only bites the first ~20px.
        let tile = Rect::new(100.3, 4.0, TILE_W, 1.0);
        let pieces = clip_quad_to_rounded(tile, &[(clip, radius(20.0))]);

        assert_eq!(
            pieces.len(),
            1,
            "an uncut row must emit ONE piece, not a snapped interior plus two \
             slivers: {pieces:?}"
        );
        let (rect, cov) = pieces[0];
        assert_eq!(cov, 1.0, "an uncut row is fully covered");
        assert!(
            (rect.x - tile.x).abs() < 1e-4 && (rect.width - tile.width).abs() < 1e-4,
            "the quad's own fractional edges must survive: {rect:?} vs {tile:?}"
        );
    }

    #[test]
    fn neighbouring_tiles_under_a_rounded_clip_do_not_seam() {
        // The defect in the form it shipped: adjacent cells must tile exactly,
        // with no partial-coverage pixel between them. Their pieces sum to the
        // full area of both, and nothing lands at less than full coverage.
        let clip = Rect::new(0.0, 0.0, 200.0, 200.0);
        let a = Rect::new(100.3, 4.0, TILE_W, 1.0);
        let b = Rect::new(100.3 + TILE_W, 4.0, TILE_W, 1.0);

        let mut pieces = clip_quad_to_rounded(a, &[(clip, radius(20.0))]);
        pieces.extend(clip_quad_to_rounded(b, &[(clip, radius(20.0))]));

        assert!(
            pieces.iter().all(|(_, cov)| *cov == 1.0),
            "no cell of an uncut row may carry partial coverage: {pieces:?}"
        );
        let area: f32 = pieces.iter().map(|(r, cov)| r.width * r.height * cov).sum();
        assert!(
            (area - 2.0 * TILE_W).abs() < 1e-3,
            "the two tiles must cover exactly their own area, got {area}"
        );
    }

    #[test]
    fn a_cut_end_is_still_antialiased_at_both_ends() {
        // The control the fix must not buy its way out of: where the arc DOES
        // cut the row, the sliver stays, or a clipped corner reads as a
        // staircase again.
        //
        // Asserted per END, not "some piece is partial". A row crossing both
        // top arcs is cut twice, so a check that only asks whether ANY partial
        // piece exists is satisfied by whichever end still works — and it
        // passed with the left end's antialiasing deleted, and again with the
        // right end's. This campaign's recurring survivor shape: the guard
        // written against the example rather than against the rule.
        let clip = Rect::new(0.0, 0.0, 200.0, 200.0);
        let pieces = clip_quad_to_rounded(Rect::new(0.0, 4.0, 200.0, 1.0), &[(clip, radius(20.0))]);

        let (left, right) =
            rounded_row_span(clip, radius(20.0), 4.5).expect("row crosses the rounded rect");
        let partial_at = |x: f32| {
            pieces
                .iter()
                .any(|(r, cov)| *cov > 0.0 && *cov < 1.0 && r.contains(x, 4.5))
        };
        assert!(
            partial_at(left + 0.01),
            "the arc-cut LEFT end must stay antialiased: {pieces:?}"
        );
        assert!(
            partial_at(right - 0.01),
            "the arc-cut RIGHT end must stay antialiased: {pieces:?}"
        );
    }

    #[test]
    fn the_clipped_area_matches_the_rounded_rect_area() {
        // A rounded rect loses (4 - pi) * r^2 to its corners. If the
        // decomposition drifts from that, it is either eating paint or leaking
        // it, and the notch tests alone would not say which.
        let clip = Rect::new(0.0, 0.0, 200.0, 120.0);
        let r = 20.0_f32;
        let pieces = clip_quad_to_rounded(clip, &[(clip, radius(r))]);

        let expected = 200.0 * 120.0 - (4.0 - std::f32::consts::PI) * r * r;
        let actual = covered_area(&pieces);
        assert!(
            (actual - expected).abs() < 2.0,
            "area {actual} should be within 2px^2 of the rounded-rect area {expected}"
        );
    }

    #[test]
    fn offscreen_gradient_cells_are_culled_to_the_viewport() {
        // The 2026-09-08 autotrader kill: a gradient card at y=20_000 on an
        // 800px viewport must contribute ZERO cells, not its full per-element
        // cell budget — per-element caps compose into BufferTooLarge.
        let (first, last) = cell_range_for_viewport(20_000.0, 1.0, 180, 800.0, false);
        assert_eq!(first, last, "fully offscreen rect must yield an empty range");
        // A rect straddling the viewport bottom keeps only the visible rows,
        // grid-aligned so the surviving cells paint identical pixels.
        let (first, last) = cell_range_for_viewport(700.0, 2.0, 180, 800.0, false);
        assert_eq!(first, 0);
        assert_eq!(last, 50, "(800-700)/2 = 50 cells remain");
        // A transform on the stack disables the cull rather than wrongly
        // dropping content the transform moves into view.
        let (first, last) = cell_range_for_viewport(20_000.0, 1.0, 180, 800.0, true);
        assert_eq!((first, last), (0, 180));
    }

    #[test]
    fn a_quad_clear_of_every_corner_is_not_split_into_rows() {
        // The band optimisation. Without it a full-page rounded container emits
        // one quad per scanline for its whole height.
        let clip = Rect::new(0.0, 0.0, 200.0, 600.0);
        let quad = Rect::new(0.0, 100.0, 200.0, 400.0);
        let pieces = clip_quad_to_rounded(quad, &[(clip, radius(12.0))]);
        assert_eq!(
            pieces.len(),
            1,
            "a quad between the corner bands must stay one quad, got {}",
            pieces.len()
        );
    }

    #[test]
    fn nested_rounded_clips_both_apply() {
        // A rounded clip inside another does not replace it — a point has to be
        // inside both. The inner box's own top-left is square here, so if the
        // outer constraint were dropped the notch would come back.
        let outer = Rect::new(0.0, 0.0, 200.0, 200.0);
        let inner = Rect::new(0.0, 0.0, 100.0, 100.0);
        let pieces = clip_quad_to_rounded(
            inner,
            &[(outer, radius(20.0)), (inner, radius(0.0))],
        );
        assert!(
            !painted_at(&pieces, 1.0, 1.0, 0.0),
            "the outer clip's corner must still cut the inner box"
        );
    }

    #[test]
    fn a_quad_outside_the_rounded_shape_entirely_yields_nothing() {
        // A row that clears no span must emit no pieces, not a zero-width one.
        let clip = Rect::new(0.0, 0.0, 40.0, 40.0);
        let quad = Rect::new(0.0, 0.0, 2.0, 2.0);
        let pieces = clip_quad_to_rounded(quad, &[(clip, radius(20.0))]);
        assert!(
            covered_area(&pieces) < 0.5,
            "the corner of a pill should be empty, covered {}",
            covered_area(&pieces)
        );
    }

    #[test]
    fn radii_are_clamped_so_opposite_corners_cannot_overlap() {
        // 100px radius on a 40px-tall box is a pill, not a negative span.
        let clip = Rect::new(0.0, 0.0, 200.0, 40.0);
        let pieces = clip_quad_to_rounded(clip, &[(clip, radius(100.0))]);
        assert!(
            painted_at(&pieces, 100.0, 20.0, 0.999),
            "the middle of a pill must still be painted"
        );
        let expected = 200.0 * 40.0 - (4.0 - std::f32::consts::PI) * 20.0 * 20.0;
        assert!(
            (covered_area(&pieces) - expected).abs() < 2.0,
            "clamped radius should give a 20px pill, covered {}",
            covered_area(&pieces)
        );
    }

    fn ellipse(h: f32, v: f32) -> rustkit_layout::BorderRadius {
        let corner = rustkit_layout::CornerRadius { h, v };
        rustkit_layout::BorderRadius {
            top_left: corner,
            top_right: corner,
            bottom_right: corner,
            bottom_left: corner,
        }
    }

    #[test]
    fn top_only_radii_as_tall_as_the_box_are_not_cut_to_half_of_it() {
        // `40px 40px 0 0` on a 200x40 tab: nothing on the left or right side
        // overlaps, so the corners stay 40px (CSS Backgrounds 3 §5.5). Each
        // radius used to be cut to half the shorter side, 20px.
        let clip = Rect::new(0.0, 0.0, 200.0, 40.0);
        let tab = rustkit_layout::BorderRadius {
            top_left: rustkit_layout::CornerRadius::circular(40.0),
            top_right: rustkit_layout::CornerRadius::circular(40.0),
            ..Default::default()
        };
        let pieces = clip_quad_to_rounded(clip, &[(clip, tab)]);
        let expected = 200.0 * 40.0 - (2.0 - std::f32::consts::FRAC_PI_2) * 40.0 * 40.0;
        assert!(
            (covered_area(&pieces) - expected).abs() < 2.0,
            "two 40px quarter circles, covered {} expected {expected}",
            covered_area(&pieces)
        );
        // 29.5px above its centre line a 40px corner has come in
        // 40 - sqrt(40^2 - 29.5^2) = 12.99px; a 20px one is not on this row
        // the same way (it would be 2.6px in).
        let (left, _) = rounded_row_span(clip, tab, 10.5).expect("row crosses the tab");
        assert!(
            (left - 12.986).abs() < 0.01,
            "left edge at y=10.5 was {left}"
        );
    }

    #[test]
    fn an_elliptical_clip_follows_the_ellipse() {
        // 200x100 with 100x50 corners is one whole ellipse.
        let clip = Rect::new(0.0, 0.0, 200.0, 100.0);
        let (left, right) =
            rounded_row_span(clip, ellipse(100.0, 50.0), 25.0).expect("row crosses");
        // dy = 25 of 50: dx = 100 * sqrt(1 - 0.25) = 86.60
        assert!((left - 13.397).abs() < 0.01, "left {left}");
        assert!((right - 186.603).abs() < 0.01, "right {right}");

        let pieces = clip_quad_to_rounded(clip, &[(clip, ellipse(100.0, 50.0))]);
        // One sample per row: the rows nearest the flat top and bottom are
        // where the span changes fastest, so allow them a few px^2.
        let expected = std::f32::consts::PI * 100.0 * 50.0;
        assert!(
            (covered_area(&pieces) - expected).abs() < 8.0,
            "ellipse area {expected}, covered {}",
            covered_area(&pieces)
        );
        // Inside the old circular corner (a 50px circle at (50, 50)), outside
        // the ellipse.
        assert!(
            !painted_at(&pieces, 30.0, 10.5, 0.01),
            "(30, 10.5) is outside the ellipse"
        );
        assert!(
            painted_at(&pieces, 100.0, 2.5, 0.999),
            "the top of the ellipse is painted"
        );
    }

    #[test]
    fn an_elliptical_corner_loses_its_own_area_to_the_clip() {
        // Each corner loses (1 - pi/4) * h * v.
        let clip = Rect::new(0.0, 0.0, 200.0, 120.0);
        let pieces = clip_quad_to_rounded(clip, &[(clip, ellipse(60.0, 30.0))]);
        let expected = 200.0 * 120.0 - (4.0 - std::f32::consts::PI) * 60.0 * 30.0;
        assert!(
            (covered_area(&pieces) - expected).abs() < 6.0,
            "covered {} expected {expected}",
            covered_area(&pieces)
        );
    }

    #[test]
    fn the_edge_distance_is_exact_for_a_circle_and_close_for_an_ellipse() {
        // Circle: the plain radius-minus-distance.
        assert_eq!(ellipse_edge_distance(3.0, 4.0, 10.0, 10.0), 5.0);
        assert_eq!(ellipse_edge_distance(6.0, 8.0, 10.0, 10.0), 0.0);

        // Ellipse (60, 30): walk the curve; half a pixel along the normal on
        // either side must read as half a pixel, to well under the width of
        // the antialiasing ramp.
        let (h, v) = (60.0_f32, 30.0_f32);
        for step in 0..=20 {
            let t = step as f32 / 20.0 * std::f32::consts::FRAC_PI_2;
            let (x, y) = (h * t.cos(), v * t.sin());
            assert!(
                ellipse_edge_distance(x, y, h, v).abs() < 1e-3,
                "on the curve at t={t}"
            );
            let (nx, ny) = (x / (h * h), y / (v * v));
            let len = (nx * nx + ny * ny).sqrt();
            let (nx, ny) = (nx / len, ny / len);
            let outside = ellipse_edge_distance(x + 0.5 * nx, y + 0.5 * ny, h, v);
            let inside = ellipse_edge_distance(x - 0.5 * nx, y - 0.5 * ny, h, v);
            assert!(
                (outside + 0.5).abs() < 0.05,
                "t={t}: half a pixel outside read {outside}"
            );
            assert!(
                (inside - 0.5).abs() < 0.05,
                "t={t}: half a pixel inside read {inside}"
            );
        }
        // The centre is as far inside as the shorter axis.
        assert_eq!(ellipse_edge_distance(0.0, 0.0, h, v), 30.0);
    }

    #[test]
    fn the_fill_and_the_clip_agree_on_an_elliptical_corner() {
        // The fill paints corner pixels from `ellipse_edge_distance`; the clip
        // cuts rows with `rounded_row_span`. If they drew different curves the
        // clip would eat the fill's edge or leave a rim outside it.
        let rect = Rect::new(0.0, 0.0, 200.0, 120.0);
        let radius = ellipse(60.0, 30.0);
        let corner = radius.top_left;
        // Whether the clip keeps the point, read from its row.
        let kept = |px: f32, py: f32| {
            py >= 0.0
                && rounded_row_span(rect, radius, py)
                    .map(|(left, _)| px >= left)
                    .unwrap_or(false)
        };
        let mut edge_pixels = 0;
        for row in 0..30 {
            let py = row as f32 + 0.5;
            for col in 0..60 {
                let px = col as f32 + 0.5;
                let fill = corner_distance_at(rect, corner, 0, px, py)
                    .map(corner_coverage)
                    .unwrap_or(1.0);
                // A pixel and a half clear of the curve along both axes
                // (the curve is nearly flat along the top, so one axis alone
                // says little there).
                if !kept(px + 1.5, py) && !kept(px, py + 1.5) {
                    assert!(
                        fill < 0.01,
                        "({px}, {py}) is well outside the clip but filled {fill}"
                    );
                }
                if kept(px - 1.5, py) && kept(px, py - 1.5) {
                    assert!(
                        fill > 0.99,
                        "({px}, {py}) is well inside the clip but filled {fill}"
                    );
                }
                if fill > 0.01 && fill < 0.99 {
                    edge_pixels += 1;
                }
            }
        }
        assert!(
            edge_pixels > 60,
            "the corner has an antialiased edge, saw {edge_pixels} partial pixels"
        );
    }

    #[test]
    fn a_fractional_corner_is_sampled_on_the_grid_of_its_outer_edge() {
        // `25%` of a 150px box is 37.5px. The right-hand corner's box starts
        // at 112.5; its cells must still be centred where the box's right
        // edge (150) puts pixels: 112.5, 113.5 ... 149.5.
        let from_right = corner_cells(112.5, 37.5, false);
        assert_eq!(from_right.len(), 38);
        assert_eq!(from_right[0], (112.0, 112.5, 0.5), "the inner cell is cut at the box");
        assert_eq!(from_right[37], (149.0, 149.0, 1.0), "the outer cell is whole");
        // The left-hand corner: laid from 0, cut at 37.5.
        let from_left = corner_cells(0.0, 37.5, true);
        assert_eq!(from_left.len(), 38);
        assert_eq!(from_left[0], (0.0, 0.0, 1.0));
        assert_eq!(from_left[37], (37.0, 37.0, 0.5));

        // Mirror images: the outermost column of a right corner reads the
        // same coverage as the outermost column of a left one. Laid from the
        // box's inner corner the right one sat half a pixel out and read
        // one-half or less down the whole side.
        let rect = Rect::new(0.0, 0.0, 150.0, 100.0);
        let corner = rustkit_layout::CornerRadius { h: 37.5, v: 25.0 };
        for row in 0..25 {
            let py = row as f32 + 0.5;
            let left = corner_distance_at(rect, corner, 0, from_left[0].0 + 0.5, py).map(corner_coverage);
            let right = corner_distance_at(rect, corner, 1, from_right[37].0 + 0.5, py).map(corner_coverage);
            let (left, right) = (left.expect("in the box"), right.expect("in the box"));
            assert!((left - right).abs() < 1e-4, "row {row}: left {left} right {right}");
        }
        let side = corner_distance_at(rect, corner, 1, from_right[37].0 + 0.5, 24.5).map(corner_coverage);
        assert!(side.unwrap() > 0.7, "the side just above the corner's end is covered, read {side:?}");

        // A whole-pixel box has the same whole cells from either end.
        assert_eq!(corner_cells(20.0, 12.0, true), corner_cells(20.0, 12.0, false));
        assert_eq!(corner_cells(20.0, 12.0, true).len(), 12);
        assert!(corner_cells(20.0, 12.0, true).iter().all(|c| c.0 == c.1 && c.2 == 1.0));
    }

    #[test]
    fn opposite_corners_that_share_the_middle_both_cut_it() {
        // `80px 0` on a 100px square: a leaf. The top-left and bottom-right
        // boxes overlap in the middle and a point there must be inside both
        // curves. Adjacent corners never overlap once fitted.
        let rect = Rect::new(0.0, 0.0, 100.0, 100.0);
        let big = rustkit_layout::CornerRadius::circular(80.0);
        let leaf = rustkit_layout::BorderRadius {
            top_left: big,
            bottom_right: big,
            ..Default::default()
        };
        assert_eq!(leaf.fitted(100.0, 100.0), leaf, "nothing adjacent overlaps");
        assert!(corner_boxes_overlap(rect, big, big));
        assert!(!corner_boxes_overlap(
            rect,
            rustkit_layout::CornerRadius::circular(50.0),
            rustkit_layout::CornerRadius::circular(50.0)
        ));

        // (25, 75): inside the top-left box and curve region's box overlap?
        // It is in both boxes; the bottom-right curve (centre (20, 20),
        // r 80) is 55 from its centre on one axis and 5 on the other: inside.
        // (22, 22) is in both boxes too and outside the top-left curve
        // (centre (80, 80), distance 82).
        assert_eq!(Renderer::point_in_rounded_rect(50.0, 50.0, rect, leaf), 1.0);
        assert_eq!(Renderer::point_in_rounded_rect(22.0, 22.0, rect, leaf), 0.0);
        assert_eq!(Renderer::point_in_rounded_rect(78.0, 78.0, rect, leaf), 0.0);
        // The square corners are whole.
        assert_eq!(Renderer::point_in_rounded_rect(99.0, 1.0, rect, leaf), 1.0);

        // Row 10 is cut on the left by the top-left curve only.
        let (left, right) = rounded_row_span(rect, leaf, 10.0).expect("row crosses");
        assert!(left > 40.0 && left < 42.0, "left {left}");
        assert_eq!(right, 100.0);
    }

    #[test]
    fn a_gradient_cell_is_tested_against_the_ellipse() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let whole = ellipse(100.0, 50.0);
        assert_eq!(
            Renderer::point_in_rounded_rect(30.0, 10.0, rect, whole),
            0.0
        );
        assert_eq!(
            Renderer::point_in_rounded_rect(100.0, 3.0, rect, whole),
            1.0
        );
        assert_eq!(
            Renderer::point_in_rounded_rect(15.0, 50.0, rect, whole),
            1.0
        );
        // On the curve: half covered.
        let on_curve = Renderer::point_in_rounded_rect(100.0 - 86.603, 25.0, rect, whole);
        assert!(
            (on_curve - 0.5).abs() < 0.02,
            "on the curve read {on_curve}"
        );
    }

    #[test]
    fn the_corner_edge_is_antialiased_rather_than_a_staircase() {
        // Partial coverage at the arc boundary. Without it the clipped corner
        // is a hard staircase while a painted rounded corner next to it is
        // smooth, and the two disagree along every shared edge.
        let clip = Rect::new(0.0, 0.0, 200.0, 200.0);
        let pieces = clip_quad_to_rounded(clip, &[(clip, radius(12.0))]);
        let partial = pieces.iter().filter(|(_, c)| *c > 0.0 && *c < 1.0).count();
        assert!(
            partial >= 8,
            "expected antialiased cells along the arcs, found {partial}"
        );
    }

    // ==================== Gradient Coordinate Tests ====================
    // These tests verify the gradient math matches between CPU and GPU implementations.

    /// Test gradient half-length calculation for various angles.
    /// CSS spec: gradient line extends from corner to corner through center.
    /// Formula: |sin(angle)| * half_width + |cos(angle)| * half_height
    #[test]
    fn test_gradient_half_length() {
        let half_w = 50.0_f32;
        let half_h = 50.0_f32;

        // 0deg (to top): sin=0, cos=1 -> half_h = 50
        let angle_rad = 0.0_f32.to_radians();
        let result = (half_w * angle_rad.sin().abs() + half_h * angle_rad.cos().abs()).max(0.001);
        assert!((result - 50.0).abs() < 0.01, "0deg: expected 50, got {}", result);

        // 90deg (to right): sin=1, cos=0 -> half_w = 50
        let angle_rad = 90.0_f32.to_radians();
        let result = (half_w * angle_rad.sin().abs() + half_h * angle_rad.cos().abs()).max(0.001);
        assert!((result - 50.0).abs() < 0.01, "90deg: expected 50, got {}", result);

        // 45deg: sin=0.707, cos=0.707 -> 0.707*50 + 0.707*50 = 70.7
        let angle_rad = 45.0_f32.to_radians();
        let result = (half_w * angle_rad.sin().abs() + half_h * angle_rad.cos().abs()).max(0.001);
        assert!((result - 70.71).abs() < 0.1, "45deg: expected 70.71, got {}", result);

        // 180deg (to bottom): sin=0, cos=-1 -> |cos|=1 -> half_h = 50
        let angle_rad = 180.0_f32.to_radians();
        let result = (half_w * angle_rad.sin().abs() + half_h * angle_rad.cos().abs()).max(0.001);
        assert!((result - 50.0).abs() < 0.01, "180deg: expected 50, got {}", result);
    }

    /// Test gradient direction vector follows CSS convention.
    /// CSS: 0deg = "to top", 90deg = "to right", etc.
    /// Direction vector: (sin(angle), -cos(angle))
    #[test]
    fn test_gradient_direction_vector() {
        // 0deg (to top): direction = (0, -1) -> points UP
        let angle_rad = 0.0_f32.to_radians();
        let dir = (angle_rad.sin(), -angle_rad.cos());
        assert!((dir.0 - 0.0).abs() < 0.001, "0deg dir.x: expected 0, got {}", dir.0);
        assert!((dir.1 - (-1.0)).abs() < 0.001, "0deg dir.y: expected -1, got {}", dir.1);

        // 90deg (to right): direction = (1, 0) -> points RIGHT
        let angle_rad = 90.0_f32.to_radians();
        let dir = (angle_rad.sin(), -angle_rad.cos());
        assert!((dir.0 - 1.0).abs() < 0.001, "90deg dir.x: expected 1, got {}", dir.0);
        assert!((dir.1 - 0.0).abs() < 0.001, "90deg dir.y: expected 0, got {}", dir.1);

        // 180deg (to bottom): direction = (0, 1) -> points DOWN
        let angle_rad = 180.0_f32.to_radians();
        let dir = (angle_rad.sin(), -angle_rad.cos());
        assert!((dir.0 - 0.0).abs() < 0.001, "180deg dir.x: expected 0, got {}", dir.0);
        assert!((dir.1 - 1.0).abs() < 0.001, "180deg dir.y: expected 1, got {}", dir.1);

        // 270deg (to left): direction = (-1, 0) -> points LEFT
        let angle_rad = 270.0_f32.to_radians();
        let dir = (angle_rad.sin(), -angle_rad.cos());
        assert!((dir.0 - (-1.0)).abs() < 0.001, "270deg dir.x: expected -1, got {}", dir.0);
        assert!((dir.1 - 0.0).abs() < 0.001, "270deg dir.y: expected 0, got {}", dir.1);
    }

    /// Test t-value calculation for a 0deg gradient on a 100x100 rect.
    /// 0deg = "to top": red at BOTTOM (t=0), blue at TOP (t=1)
    #[test]
    fn test_gradient_t_value_vertical() {
        let rect_x = 0.0_f32;
        let rect_y = 0.0_f32;
        let rect_width = 100.0_f32;
        let rect_height = 100.0_f32;
        let angle_deg = 0.0_f32;
        let angle_rad = angle_deg.to_radians();

        let (sin_a, cos_a) = (angle_rad.sin(), angle_rad.cos());
        let half_w = rect_width / 2.0;
        let half_h = rect_height / 2.0;
        let gradient_half_length = (half_w * sin_a.abs() + half_h * cos_a.abs()).max(0.001);
        let center_x = rect_x + half_w;
        let center_y = rect_y + half_h;

        // At top center (50, 0): should be t=1.0 (blue end)
        let px = 50.0 - center_x;
        let py = 0.0 - center_y; // py = -50
        let projection = px * sin_a + py * (-cos_a); // 0 + (-50)*(-1) = 50
        let t = (projection / gradient_half_length + 1.0) / 2.0;
        assert!((t - 1.0).abs() < 0.01, "Top center t: expected 1.0, got {}", t);

        // At bottom center (50, 100): should be t=0.0 (red end)
        let px = 50.0 - center_x;
        let py = 100.0 - center_y; // py = 50
        let projection = px * sin_a + py * (-cos_a); // 0 + 50*(-1) = -50
        let t = (projection / gradient_half_length + 1.0) / 2.0;
        assert!((t - 0.0).abs() < 0.01, "Bottom center t: expected 0.0, got {}", t);

        // At center (50, 50): should be t=0.5
        let px = 50.0 - center_x;
        let py = 50.0 - center_y;
        let projection = px * sin_a + py * (-cos_a);
        let t = (projection / gradient_half_length + 1.0) / 2.0;
        assert!((t - 0.5).abs() < 0.01, "Center t: expected 0.5, got {}", t);
    }

    /// Test t-value calculation for a 90deg gradient on a 100x100 rect.
    /// 90deg = "to right": red at LEFT (t=0), blue at RIGHT (t=1)
    #[test]
    fn test_gradient_t_value_horizontal() {
        let rect_x = 0.0_f32;
        let rect_y = 0.0_f32;
        let rect_width = 100.0_f32;
        let rect_height = 100.0_f32;
        let angle_deg = 90.0_f32;
        let angle_rad = angle_deg.to_radians();

        let (sin_a, cos_a) = (angle_rad.sin(), angle_rad.cos());
        let half_w = rect_width / 2.0;
        let half_h = rect_height / 2.0;
        let gradient_half_length = (half_w * sin_a.abs() + half_h * cos_a.abs()).max(0.001);
        let center_x = rect_x + half_w;
        let center_y = rect_y + half_h;

        // At left center (0, 50): should be t=0.0 (red end)
        let px = 0.0 - center_x; // px = -50
        let py = 50.0 - center_y;
        let projection = px * sin_a + py * (-cos_a); // -50*1 + 0 = -50
        let t = (projection / gradient_half_length + 1.0) / 2.0;
        assert!((t - 0.0).abs() < 0.01, "Left center t: expected 0.0, got {}", t);

        // At right center (100, 50): should be t=1.0 (blue end)
        let px = 100.0 - center_x; // px = 50
        let py = 50.0 - center_y;
        let projection = px * sin_a + py * (-cos_a); // 50*1 + 0 = 50
        let t = (projection / gradient_half_length + 1.0) / 2.0;
        assert!((t - 1.0).abs() < 0.01, "Right center t: expected 1.0, got {}", t);
    }

    /// Test GradientParams struct size matches what GPU expects.
    #[test]
    fn test_gradient_params_size() {
        // GradientParams should be 80 bytes (20 x 4-byte values)
        assert_eq!(
            std::mem::size_of::<crate::pipeline::GradientParams>(),
            80,
            "GradientParams size mismatch"
        );
    }

    /// Test GradientColorStop struct size matches what GPU expects.
    #[test]
    fn test_gradient_color_stop_size() {
        // GradientColorStop should be 20 bytes (5 f32 values)
        assert_eq!(
            std::mem::size_of::<crate::pipeline::GradientColorStop>(),
            20,
            "GradientColorStop size mismatch"
        );
    }

    /// Test buffer size validation logic (unit test without GPU).
    #[test]
    fn test_buffer_size_validation_logic() {
        // Test the validation logic directly
        // This tests the error handling without needing a real GPU device

        const MAX_SIZE: u64 = 256 * 1024 * 1024; // 256 MB

        // Simulate validation function
        let validate = |size: u64| -> Result<u64, String> {
            if size > MAX_SIZE {
                Err(format!("Buffer size {} exceeds maximum {}", size, MAX_SIZE))
            } else {
                Ok(size)
            }
        };

        // Test valid buffer sizes
        assert!(validate(1024).is_ok());
        assert!(validate(1024 * 1024).is_ok());
        assert!(validate(100 * 1024 * 1024).is_ok());

        // Test buffer size at limit
        assert!(validate(MAX_SIZE).is_ok());

        // Test buffer size exceeding limit
        assert!(validate(MAX_SIZE + 1).is_err());
        assert!(validate(MAX_SIZE * 2).is_err());

        // Test pathological gradient scenario (10K stops)
        let pathological_stops = 10_000;
        let stop_size = std::mem::size_of::<crate::pipeline::GradientColorStop>() as u64; // 20 bytes
        let total_size = pathological_stops * stop_size;
        // 10K stops = 200KB, well within limits
        assert!(validate(total_size).is_ok(), "10K gradient stops should fit in buffer");

        // Test extreme case (1M stops would be 20MB, still within 256MB limit)
        let extreme_stops = 1_000_000;
        let extreme_size = extreme_stops * stop_size;
        assert!(validate(extreme_size).is_ok(), "1M stops should fit");

        // Test truly pathological case (1B stops = 20GB, should exceed limit)
        let truly_pathological = 1_000_000_000;
        let pathological_size = truly_pathological * stop_size;
        assert!(validate(pathological_size).is_err(), "1B stops should exceed limit");
    }

    #[test]
    fn test_gradient_stops_clamping() {
        // Test that gradient stops are properly clamped to max_stops (32)
        // This is important for GPU buffer allocation safety
        const MAX_STOPS: usize = 32;

        // Simulate what happens with many stops
        let input_stops = 100;
        let clamped = input_stops.min(MAX_STOPS);
        assert_eq!(clamped, MAX_STOPS, "Should clamp to 32 stops");

        // Edge case: exactly at limit
        assert_eq!(MAX_STOPS.min(MAX_STOPS), MAX_STOPS);

        // Edge case: under limit
        assert_eq!(10_usize.min(MAX_STOPS), 10);
    }

    #[test]
    fn test_circle_rendering_triangle_count() {
        // Test that circle rendering uses a reasonable triangle count
        // Circles are rendered as triangle fans
        const MIN_SEGMENTS: u32 = 16; // Minimum for visual smoothness
        const MAX_SEGMENTS: u32 = 64; // Maximum to avoid GPU overload

        // For a small circle (radius 10px), use minimum segments
        let small_radius = 10.0;
        let small_segments = estimate_circle_segments(small_radius);
        assert!(small_segments >= MIN_SEGMENTS, "Small circle needs minimum segments");
        assert!(small_segments <= MAX_SEGMENTS, "Small circle shouldn't exceed max");

        // For a large circle (radius 500px), might use more segments
        let large_radius = 500.0;
        let large_segments = estimate_circle_segments(large_radius);
        assert!(large_segments >= MIN_SEGMENTS);
        assert!(large_segments <= MAX_SEGMENTS, "Large circle should be clamped");

        // Helper function to estimate segments (simplified version)
        fn estimate_circle_segments(radius: f32) -> u32 {
            // A simple heuristic: use more segments for larger circles
            let base = (radius / 10.0).sqrt() as u32;
            base.max(16).min(64)
        }
    }

    #[test]
    fn test_ellipse_aspect_ratio() {
        // Test that ellipse rendering maintains correct aspect ratio
        let width = 200.0;
        let height = 100.0;
        let aspect = width / height;
        assert_eq!(aspect, 2.0, "Aspect ratio should be 2:1");

        // Edge case: circle (aspect 1:1)
        let circle_width = 100.0;
        let circle_height = 100.0;
        let circle_aspect = circle_width / circle_height;
        assert_eq!(circle_aspect, 1.0, "Circle has 1:1 aspect");

        // Edge case: very elongated ellipse
        let narrow_width = 500.0;
        let narrow_height = 10.0;
        let narrow_aspect = narrow_width / narrow_height;
        assert_eq!(narrow_aspect, 50.0, "Narrow ellipse has 50:1 aspect");
    }

    #[test]
    fn test_background_repeat_space_calculation() {
        // Test background-repeat: space calculation
        // Should evenly distribute images with spacing between them
        let container_width = 800.0_f32;
        let image_width = 100.0_f32;

        // Calculate how many full images fit
        let fit_count = (container_width / image_width).floor() as u32;
        assert_eq!(fit_count, 8, "8 images of 100px fit in 800px");

        // Calculate spacing for 'space' mode
        // With 8 images, we need 7 gaps to distribute evenly
        let gaps = if fit_count > 1 { fit_count - 1 } else { 1 };
        let total_image_width = fit_count as f32 * image_width;
        let remaining_space = container_width - total_image_width;
        let gap_size = remaining_space / gaps as f32;

        assert!(gap_size >= 0.0_f32, "Gap size should be non-negative");
        assert_eq!(gap_size, 0.0_f32, "With perfect fit, gap is 0");

        // Test with imperfect fit
        let container_width_2 = 850.0_f32;
        let fit_count_2 = (container_width_2 / image_width).floor() as u32;
        let gaps_2 = fit_count_2 - 1;
        let total_image_width_2 = fit_count_2 as f32 * image_width;
        let remaining_space_2 = container_width_2 - total_image_width_2;
        let gap_size_2 = remaining_space_2 / gaps_2 as f32;

        assert!(gap_size_2 > 0.0_f32, "Imperfect fit should have gaps");
        assert!((gap_size_2 - 7.14_f32).abs() < 0.1_f32, "Gap should be ~7.14px");
    }

    #[test]
    fn test_background_repeat_round_scaling() {
        // Test background-repeat: round calculation
        // Should scale images to fit container with integer repetitions
        let container_width = 850.0_f32;
        let image_width = 100.0_f32;

        // Calculate integer repetitions
        let repetitions = (container_width / image_width).round() as u32;
        assert_eq!(repetitions, 9, "Should round to 9 repetitions");

        // Calculate scaled image size
        let scaled_width = container_width / repetitions as f32;
        assert!((scaled_width - 94.44_f32).abs() < 0.1_f32, "Scaled image should be ~94.44px");

        // Edge case: exact fit (no scaling needed)
        let exact_container = 800.0_f32;
        let exact_reps = (exact_container / image_width).round() as u32;
        let exact_scaled = exact_container / exact_reps as f32;
        assert_eq!(exact_scaled, 100.0_f32, "Exact fit should not scale");
    }

    #[test]
    fn test_gradient_color_interpolation() {
        // Test gradient color interpolation between stops
        // Linear interpolation between two colors
        let color1 = [1.0, 0.0, 0.0, 1.0]; // Red
        let color2 = [0.0, 0.0, 1.0, 1.0]; // Blue
        let t = 0.5; // Halfway

        let interpolated = [
            color1[0] * (1.0 - t) + color2[0] * t,
            color1[1] * (1.0 - t) + color2[1] * t,
            color1[2] * (1.0 - t) + color2[2] * t,
            color1[3] * (1.0 - t) + color2[3] * t,
        ];

        assert_eq!(interpolated[0], 0.5, "Red channel should be 0.5");
        assert_eq!(interpolated[1], 0.0, "Green channel should be 0");
        assert_eq!(interpolated[2], 0.5, "Blue channel should be 0.5");
        assert_eq!(interpolated[3], 1.0, "Alpha should be 1.0");

        // Edge case: t=0 should return first color
        let t0 = 0.0;
        let at_start = [
            color1[0] * (1.0 - t0) + color2[0] * t0,
            color1[1] * (1.0 - t0) + color2[1] * t0,
            color1[2] * (1.0 - t0) + color2[2] * t0,
            color1[3] * (1.0 - t0) + color2[3] * t0,
        ];
        assert_eq!(at_start, color1);

        // Edge case: t=1 should return second color
        let t1 = 1.0;
        let at_end = [
            color1[0] * (1.0 - t1) + color2[0] * t1,
            color1[1] * (1.0 - t1) + color2[1] * t1,
            color1[2] * (1.0 - t1) + color2[2] * t1,
            color1[3] * (1.0 - t1) + color2[3] * t1,
        ];
        assert_eq!(at_end, color2);
    }

    #[test]
    fn test_gradient_radial_center_calculation() {
        // Test that radial gradients correctly calculate center position
        let rect_x = 100.0;
        let rect_y = 200.0;
        let rect_width = 400.0;
        let rect_height = 300.0;

        // Default center is at 50% 50%
        let center_x = rect_x + rect_width / 2.0;
        let center_y = rect_y + rect_height / 2.0;

        assert_eq!(center_x, 300.0, "Center X should be at 300");
        assert_eq!(center_y, 350.0, "Center Y should be at 350");

        // Test with offset center (e.g., at 25% 75%)
        let offset_center_x = rect_x + rect_width * 0.25;
        let offset_center_y = rect_y + rect_height * 0.75;

        assert_eq!(offset_center_x, 200.0, "Offset center X");
        assert_eq!(offset_center_y, 425.0, "Offset center Y");
    }

    #[test]
    fn test_gradient_conic_angle_normalization() {
        // Test that conic gradient angles are normalized to 0-360 range
        let angle_450 = 450.0_f32;
        let normalized_450 = angle_450 % 360.0;
        assert_eq!(normalized_450, 90.0, "450° should normalize to 90°");

        let angle_neg90 = -90.0_f32;
        let normalized_neg = (angle_neg90 % 360.0 + 360.0) % 360.0;
        assert_eq!(normalized_neg, 270.0, "-90° should normalize to 270°");

        let angle_720 = 720.0_f32;
        let normalized_720 = angle_720 % 360.0;
        assert_eq!(normalized_720, 0.0, "720° should normalize to 0°");
    }

    #[test]
    fn test_buffer_vertex_capacity() {
        // Test that vertex buffers can hold expected number of vertices
        const VERTICES_PER_RECT: usize = 4;
        const INDICES_PER_RECT: usize = 6;

        // Typical batch size
        let batch_size = 100;
        let total_vertices = batch_size * VERTICES_PER_RECT;
        let total_indices = batch_size * INDICES_PER_RECT;

        assert_eq!(total_vertices, 400, "100 rects need 400 vertices");
        assert_eq!(total_indices, 600, "100 rects need 600 indices");

        // Check buffer size
        let vertex_size = std::mem::size_of::<ColorVertex>();
        let total_vertex_bytes = total_vertices * vertex_size;

        assert_eq!(total_vertex_bytes, 400 * 24, "Total vertex buffer size");
        assert!(total_vertex_bytes < 256 * 1024 * 1024, "Should fit in max buffer");
    }

    #[test]
    fn test_rect_fully_contains() {
        // Test rect containment logic
        let outer = Rect::new(0.0, 0.0, 200.0, 200.0);
        let inner = Rect::new(50.0, 50.0, 100.0, 100.0);

        // Check if inner is fully inside outer
        let contains = inner.x >= outer.x
            && inner.y >= outer.y
            && (inner.x + inner.width) <= (outer.x + outer.width)
            && (inner.y + inner.height) <= (outer.y + outer.height);

        assert!(contains, "Inner rect should be fully contained");

        // Edge case: same rect
        let same = Rect::new(0.0, 0.0, 200.0, 200.0);
        let contains_self = same.x >= outer.x
            && same.y >= outer.y
            && (same.x + same.width) <= (outer.x + outer.width)
            && (same.y + same.height) <= (outer.y + outer.height);

        assert!(contains_self, "Rect should contain itself");
    }

    #[test]
    fn test_rect_area_calculation() {
        // Test rectangle area calculations
        let rect = Rect::new(0.0, 0.0, 100.0, 50.0);
        let area = rect.width * rect.height;
        assert_eq!(area, 5000.0, "Area should be 5000 square pixels");

        // Edge case: zero area
        let zero_width = Rect::new(0.0, 0.0, 0.0, 100.0);
        let zero_area = zero_width.width * zero_width.height;
        assert_eq!(zero_area, 0.0, "Zero width means zero area");

        // Edge case: very small rect
        let tiny = Rect::new(0.0, 0.0, 0.1, 0.1);
        let tiny_area = tiny.width * tiny.height;
        assert!(tiny_area < 0.02, "Tiny rect has tiny area");
    }
}

/// PAINT-0 seating probe gate (forensics 2026-07-16-paint0-glyph-seat).
/// RUSTKIT_PAINT_PROBE=1 logs the paint half of the glyph seating chain
/// (baseline, bearing_y, glyph_y) plus per-glyph atlas bitmap hashes, so a
/// flat-1.2 vs metrics-normal A/B can attribute score deltas to seating
/// float shifts vs raster differences. Zero cost when off.
pub(crate) fn paint0_probe() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTKIT_PAINT_PROBE").as_deref() == Ok("1"))
}


#[cfg(test)]
mod outer_shadow_tests {
    use super::*;

    fn area(rects: &[(Rect, f32)]) -> f32 {
        rects.iter().map(|(r, _)| r.width * r.height).sum()
    }

    const SQUARE: rustkit_layout::BorderRadius = rustkit_layout::BorderRadius {
        top_left: rustkit_layout::CornerRadius { h: 0.0, v: 0.0 },
        top_right: rustkit_layout::CornerRadius { h: 0.0, v: 0.0 },
        bottom_right: rustkit_layout::CornerRadius { h: 0.0, v: 0.0 },
        bottom_left: rustkit_layout::CornerRadius { h: 0.0, v: 0.0 },
    };

    /// Paint at `(x, y)`: the summed alpha of the pieces holding the point.
    fn alpha_at(rects: &[(Rect, f32)], x: f32, y: f32) -> f32 {
        rects
            .iter()
            .filter(|(r, _)| x >= r.x && x < r.right() && y >= r.y && y < r.bottom())
            .map(|(_, a)| *a)
            .sum()
    }

    /// The area painted, weighted by alpha.
    fn weighted_area(rects: &[(Rect, f32)]) -> f32 {
        rects.iter().map(|(r, a)| r.width * r.height * a).sum()
    }

    /// `box-shadow: 0 0 0 6px` on a 100px box with 20px corners is a ring
    /// between two rounded rects (radii 26 and 20), not a square frame.
    #[test]
    fn a_rounded_box_gets_a_rounded_ring() {
        let border_box = Rect::new(50.0, 50.0, 100.0, 100.0);
        let radius = rustkit_layout::BorderRadius::uniform(20.0);
        let shadow = Rect::new(44.0, 44.0, 112.0, 112.0);
        let rects = Renderer::outer_shadow_paint_rects(
            border_box,
            radius,
            shadow,
            radius.spread(6.0),
            0.0,
            1.0,
        );

        // On the ring's straight side, and in the gap between the two curves.
        assert_eq!(alpha_at(&rects, 100.5, 46.5), 1.0);
        assert_eq!(alpha_at(&rects, 53.5, 53.5), 1.0);
        // Outside the shadow's own curve: the corner of its bounding square.
        assert_eq!(alpha_at(&rects, 44.5, 44.5), 0.0);
        // Inside the box: its middle, and just inside its curve.
        assert_eq!(alpha_at(&rects, 100.5, 100.5), 0.0);
        assert_eq!(alpha_at(&rects, 58.5, 58.5), 0.0);

        // No pixel is painted twice, and none outside the shadow's rect.
        for (r, a) in &rects {
            assert!(*a > 0.0 && *a <= 1.0, "alpha {a}");
            assert!(r.x >= 44.0 && r.right() <= 156.0 && r.y >= 44.0 && r.bottom() <= 156.0, "{r:?}");
        }
        for y in 44..156 {
            for x in 44..156 {
                let a = alpha_at(&rects, x as f32 + 0.5, y as f32 + 0.5);
                assert!(a <= 1.0 + 1e-6, "({x}, {y}) painted {a}");
            }
        }

        // The ring's area: the difference of the two rounded rects.
        let rounded_area = |side: f32, r: f32| side * side - (4.0 - std::f32::consts::PI) * r * r;
        let ring = rounded_area(112.0, 26.0) - rounded_area(100.0, 20.0);
        let painted = weighted_area(&rects);
        assert!((painted - ring).abs() < ring * 0.02, "painted {painted}, ring {ring}");
    }

    /// The hole is the box's curve even when the shadow is only moved: the
    /// notch between the box's square corner and its curve shows the shadow.
    #[test]
    fn an_offset_shadow_shows_through_the_corner_notch_of_its_box() {
        let border_box = Rect::new(0.0, 0.0, 100.0, 100.0);
        let radius = rustkit_layout::BorderRadius::uniform(30.0);
        let shadow = Rect::new(10.0, 10.0, 100.0, 100.0);
        let rects = Renderer::outer_shadow_paint_rects(border_box, radius, shadow, radius, 0.0, 1.0);
        // Bottom-right notch of the box: outside its curve, inside the shadow.
        assert_eq!(alpha_at(&rects, 97.5, 97.5), 1.0);
        // The shadow's own bottom-right corner is cut.
        assert_eq!(alpha_at(&rects, 108.5, 108.5), 0.0);
        // Under the box.
        assert_eq!(alpha_at(&rects, 50.5, 50.5), 0.0);
    }

    /// Each blur layer is the shadow's shape grown, corners included.
    #[test]
    fn blurred_layers_of_a_rounded_shadow_are_rounded() {
        let border_box = Rect::new(20.0, 20.0, 60.0, 60.0);
        let radius = rustkit_layout::BorderRadius::uniform(30.0);
        let rects = Renderer::outer_shadow_paint_rects(border_box, radius, border_box, radius, 8.0, 0.9);
        assert!(!rects.is_empty());
        // The outermost layer reaches 8px out on the axis, and its bounding
        // square's corner stays clear.
        assert!(alpha_at(&rects, 50.5, 13.5) > 0.0);
        assert_eq!(alpha_at(&rects, 13.5, 13.5), 0.0);
        assert_eq!(alpha_at(&rects, 50.5, 50.5), 0.0);
    }

    /// A square box takes the path it always did, rect for rect.
    #[test]
    fn a_square_box_gets_exactly_the_old_rects() {
        let border_box = Rect::new(40.0, 40.0, 89.0, 38.0);
        let shadow = Rect::new(39.0, 39.0, 91.0, 40.0);
        let rects = rounded_difference_pieces(shadow, SQUARE, border_box, SQUARE);
        let old: Vec<(Rect, f32)> = Renderer::rect_minus(shadow, border_box)
            .into_iter()
            .map(|r| (r, 1.0))
            .collect();
        assert_eq!(rects.len(), old.len());
        for ((a, ca), (b, cb)) in rects.iter().zip(&old) {
            assert_eq!((a.x, a.y, a.width, a.height, *ca), (b.x, b.y, b.width, b.height, *cb));
        }
    }

    /// The hole's edge takes the fill's coverage ramp, so the two sum to one
    /// across the box's curve.
    #[test]
    fn the_hole_and_the_fill_share_the_corner_ramp() {
        let rect = Rect::new(0.0, 0.0, 80.0, 80.0);
        let radius = rustkit_layout::BorderRadius::uniform(24.0);
        // A shape that covers everything, minus the box.
        let outside = rounded_difference_pieces(Rect::new(-10.0, -10.0, 100.0, 100.0), SQUARE, rect, radius);
        let mut partial = 0;
        for y in 0..24 {
            for x in 0..24 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let fill = rounded_rect_coverage(rect, radius, px, py);
                let hole = alpha_at(&outside, px, py);
                // A coverage under 0.01 is not emitted, as in the fill.
                assert!((fill + hole - 1.0).abs() <= 0.011, "({x}, {y}): fill {fill} + hole {hole}");
                if fill > 0.0 && fill < 1.0 {
                    partial += 1;
                }
            }
        }
        assert!(partial > 10, "only {partial} antialiased pixels along a 24px arc");
    }

    fn assert_outside(rects: &[(Rect, f32)], border_box: Rect) {
        for (r, _) in rects {
            assert!(
                r.intersect(&border_box).is_none(),
                "shadow rect {r:?} paints inside the border box {border_box:?}"
            );
        }
    }

    /// CSS Backgrounds 3 §7.1: an outer shadow is clipped to outside the
    /// border box. linkedin's "Sign in" is `box-shadow: 0 0 0 1px blue` on a
    /// transparent background with blue text: a 1px ring, not a filled box.
    #[test]
    fn spread_only_ring_does_not_fill_the_box() {
        let border_box = Rect::new(40.0, 40.0, 89.0, 38.0);
        let shadow = Rect::new(39.0, 39.0, 91.0, 40.0);
        let rects = Renderer::outer_shadow_paint_rects(border_box, SQUARE, shadow, SQUARE, 0.0, 1.0);
        assert_outside(&rects, border_box);
        let ring = 91.0 * 40.0 - 89.0 * 38.0;
        assert!((area(&rects) - ring).abs() < 1e-3, "area {} != ring {ring}", area(&rects));
    }

    /// An offset shadow shows only where it sticks out past the box.
    #[test]
    fn offset_shadow_shows_only_outside_the_box() {
        let border_box = Rect::new(40.0, 40.0, 200.0, 60.0);
        let shadow = Rect::new(48.0, 48.0, 200.0, 60.0);
        let rects = Renderer::outer_shadow_paint_rects(border_box, SQUARE, shadow, SQUARE, 0.0, 1.0);
        assert_outside(&rects, border_box);
        let visible = 200.0 * 60.0 - 192.0 * 52.0;
        assert!((area(&rects) - visible).abs() < 1e-3);
    }

    /// Blurred layers are clipped the same way, and keep their alpha.
    #[test]
    fn blurred_layers_are_clipped_too() {
        let border_box = Rect::new(10.0, 10.0, 50.0, 50.0);
        let rects = Renderer::outer_shadow_paint_rects(border_box, SQUARE, border_box, SQUARE, 4.0, 0.9);
        assert!(!rects.is_empty());
        assert_outside(&rects, border_box);
        assert!(rects.iter().all(|(_, a)| (*a - 0.9 / 3.0).abs() < 1e-6));
    }

    /// A shadow entirely hidden behind its box paints nothing.
    #[test]
    fn shadow_under_the_box_paints_nothing() {
        let border_box = Rect::new(0.0, 0.0, 100.0, 100.0);
        let shadow = Rect::new(10.0, 10.0, 80.0, 80.0);
        assert!(Renderer::outer_shadow_paint_rects(border_box, SQUARE, shadow, SQUARE, 0.0, 1.0).is_empty());
    }
}

/// How far inside its padding edge a drop-down `<select>` seats its label.
const MENU_LIST_LABEL_INSET: f32 = 4.0;
/// Horizontal padding of a list-box option row (Chrome's `option` padding).
const LIST_BOX_OPTION_INSET: f32 = 2.0;
/// A selected list-box row, and its text, in a list box without focus.
const LIST_BOX_SELECTED_ROW: Color = Color {
    r: 206,
    g: 206,
    b: 206,
    a: 1.0,
};
const LIST_BOX_SELECTED_TEXT: Color = Color {
    r: 16,
    g: 16,
    b: 16,
    a: 1.0,
};

/// Centre of a drop-down's arrow in its border box `rect`.
fn menu_list_arrow_centre(rect: Rect) -> (f32, f32) {
    (rect.x + rect.width - 8.75, rect.y + rect.height / 2.0)
}

/// Row `index` of a list box whose frame encloses `inner`.
fn list_box_row_rect(inner: Rect, row_height: f32, index: usize) -> Rect {
    Rect::new(
        inner.x,
        inner.y + row_height * index as f32,
        inner.width,
        row_height,
    )
}

#[cfg(test)]
mod select_paint_tests {
    use super::*;

    /// Chrome CfT-148, bare `<select>` at (156.19, 875) 137x19: the chevron
    /// spans x 280.5..288.5 and y 882..886 — centred on the control's
    /// height, clear of the right border.
    #[test]
    fn the_drop_down_arrow_sits_inside_the_right_edge_at_mid_height() {
        let rect = Rect::new(156.1875, 875.0, 137.0, 19.0);
        let (cx, cy) = menu_list_arrow_centre(rect);
        assert!((cx - 284.5).abs() <= 0.5, "arrow centre x {cx}");
        assert_eq!(cy, 884.5);
        // The same distance from the right edge on a padded 155x40 select.
        let big = Rect::new(239.45, 167.0, 155.0, 40.0);
        let (bx, by) = menu_list_arrow_centre(big);
        assert_eq!(big.x + big.width - bx, rect.x + rect.width - cx);
        assert_eq!(by, 187.0);
    }

    /// Chrome's option rows stack from the inside of the frame: 16px rows
    /// at y = 906, 922, 938 in a list box whose border box starts at 905.
    #[test]
    fn list_box_rows_stack_from_the_inside_of_the_frame() {
        let inner = Rect::new(157.1875, 906.0, 37.05, 48.0);
        let ys: Vec<f32> = (0..4)
            .map(|i| list_box_row_rect(inner, 16.0, i).y)
            .collect();
        assert_eq!(ys, vec![906.0, 922.0, 938.0, 954.0]);
        // The fourth option starts at the frame's inner bottom: not painted.
        assert!(ys[3] >= inner.y + inner.height);
        assert_eq!(list_box_row_rect(inner, 16.0, 1).width, inner.width);
    }
}

#[cfg(test)]
mod form_text_seat_tests {
    use super::*;

    /// Chrome centres the inner editor's line box in the CONTENT box. On the
    /// form-controls Chrome baseline a bare input is Arial 13.333px, padding
    /// 0, border 2px, 19px border-box: content 15px, Arial line box ~14.9px,
    /// so the text top sits ~2px below the border-box top and the baseline
    /// ~2 + ascent. The old seat put the baseline at
    /// rect.y + (h+fs)/2 - 0.2fs + ascent ≈ rect.y + 23.8 — under the box.
    #[test]
    fn bare_input_text_line_is_centred_in_the_content_box() {
        let rect = Rect::new(156.0, 145.0, 149.0, 19.0);
        let (text_x, text_top, ascent, descent) =
            Renderer::form_text_seat(rect, 2.0, [0.0; 4], "Arial", 13.333);
        assert!(ascent > 0.0 && descent > 0.0);
        let line = ascent + descent;
        let content_top = rect.y + 2.0;
        let content_h = rect.height - 4.0;
        // Centred: equal slack above and below the line box.
        let slack_above = text_top - content_top;
        let slack_below = (content_top + content_h) - (text_top + line);
        assert!(
            (slack_above - slack_below).abs() < 1e-3,
            "line box not centred: above {slack_above}, below {slack_below}"
        );
        // Baseline lands INSIDE the box, not under its bottom border.
        let baseline = text_top + ascent;
        assert!(baseline < rect.y + rect.height - 2.0, "baseline {baseline} is under the box");
        assert!(baseline > rect.y + 2.0);
        // Text starts after the border (padding 0), not at a hardcoded 6px.
        assert_eq!(text_x, rect.x + 2.0);
    }

    #[test]
    fn author_padding_moves_the_seat_and_the_box_agrees() {
        // input { padding: 8px 16px; border: 2px } → Chrome builds
        // (fs+1) + 16 + 4 = 35 tall (layout_form_control's DIG-1 compose).
        let fs = 14.0;
        let rect = Rect::new(0.0, 0.0, 200.0, (fs + 1.0) + 16.0 + 4.0);
        let padding = [8.0, 16.0, 8.0, 16.0];
        let (text_x, text_top, ascent, descent) =
            Renderer::form_text_seat(rect, 2.0, padding, "Arial", fs);
        assert_eq!(text_x, 2.0 + 16.0);
        let content_top = 2.0 + 8.0;
        let content_h = rect.height - 4.0 - 16.0;
        let centred_top = content_top + (content_h - (ascent + descent)) / 2.0;
        assert!((text_top - centred_top).abs() < 1e-3);
        // Compose says the content is fs+1 tall; the font's line box must
        // fit within ~a pixel of that or the two formulas disagree.
        assert!(((ascent + descent) - content_h).abs() <= 1.5,
            "line box {} vs composed content {}", ascent + descent, content_h);
    }
}

/// Statistics about the last render pass (shell diagnostics).
#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    pub color_vertex_count: usize,
    pub color_index_count: usize,
    pub texture_vertex_count: usize,
    pub texture_index_count: usize,
    pub clip_stack_depth: usize,
    pub stacking_context_depth: usize,
}

/// ISO8601 timestamp for the capture sidecar, without a chrono dependency.
#[cfg(windows)]
fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    utc_timestamp_from_secs(secs)
}

/// Render seconds since the Unix epoch as `YYYY-MM-DDTHH:MM:SSZ`.
///
/// The previous version counted 365-day years and 30-day months, so every
/// sidecar written in 2026 was stamped 17 days into the future (a
/// 2026-09-25 capture read 2026-10-12), and the drift grows by about five
/// days a year. This is the proleptic-Gregorian days-to-civil conversion
/// (Howard Hinnant's algorithm): exact for every date a `u64` can express,
/// no dependency, no leap seconds (neither has `SystemTime`).
#[allow(dead_code)] // only the Windows capture path calls it; the test runs everywhere
fn utc_timestamp_from_secs(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let z = days + 719_468; // shift the epoch from 1970-01-01 to 0000-03-01
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era [0, 146096]
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365; // year of era [0, 399]
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year, March-based [0, 365]
    let mp = (5 * doy + 2) / 153; // March = 0 ... February = 11
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    let hours = (secs % 86_400) / 3_600;
    let minutes = (secs % 3_600) / 60;
    let seconds = secs % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

#[cfg(test)]
mod sidecar_timestamp_tests {
    use super::utc_timestamp_from_secs;

    /// Values cross-checked against Python's `datetime.fromtimestamp(..., UTC)`.
    #[test]
    fn the_sidecar_timestamp_is_a_real_civil_date() {
        assert_eq!(utc_timestamp_from_secs(0), "1970-01-01T00:00:00Z");
        // The capture that exposed the bug: the old formula turns this
        // second into 2026-10-12T18:35:17Z.
        assert_eq!(utc_timestamp_from_secs(1_790_361_317), "2026-09-25T18:35:17Z");
        // Leap days: an ordinary leap year and a century year that is one.
        assert_eq!(utc_timestamp_from_secs(1_709_208_000), "2024-02-29T12:00:00Z");
        assert_eq!(utc_timestamp_from_secs(951_782_400), "2000-02-29T00:00:00Z");
        // Last second of 2099.
        assert_eq!(utc_timestamp_from_secs(4_102_444_799), "2099-12-31T23:59:59Z");
    }
}

/// SHAPED-RUN CONTRACT, slice S0 (docs/SHAPED_RUN_CONTRACT_2026-09-30.md
/// §7), paint side. Device-free: these read the bitmaps the atlas is filled
/// from. The character path is the oracle.
#[cfg(all(test, target_os = "macos"))]
mod shaped_run_paint_tests {
    use super::*;
    use rustkit_css::{ComputedStyle, Length};

    const GEORGIA_LIST: &str = "Georgia, 'Times New Roman', serif";

    fn run_in(family: &str, weight: u16, text: &str, size: f32) -> rustkit_layout::GlyphRun {
        let mut style = ComputedStyle::new();
        style.font_family = family.to_string();
        style.font_size = Length::Px(size);
        style.font_weight = rustkit_css::FontWeight(weight);
        rustkit_layout::shape_line_run(text, &style, size, 0.0).expect("a Latin line has a run")
    }

    fn key(run: &rustkit_layout::GlyphRun, glyph_id: u16) -> RunGlyphKey {
        RunGlyphKey {
            face: run.face.id,
            glyph_id,
            font_size: (run.font_size * 10.0) as u32,
            subpixel_phase: 0,
        }
    }

    /// "Face identity, macOS": painting the run resolves no family list,
    /// and what it draws is, byte for byte, what the character path draws
    /// when that path resolves the same list to the same face.
    #[test]
    fn run_glyphs_are_drawn_from_the_run_face_with_no_family_lookup() {
        for (weight, text, size) in [(400u16, "Wave To", 24.0f32), (700, "Type", 17.6)] {
            let run = run_in(GEORGIA_LIST, weight, text, size);

            let before = rustkit_text::macos::css_list_resolutions();
            let from_run: Vec<_> = run
                .glyphs
                .iter()
                .map(|g| rasterize_run_glyph(&key(&run, g.glyph_id), run.font_size))
                .collect();
            assert_eq!(
                rustkit_text::macos::css_list_resolutions(),
                before,
                "painting a run must not resolve a font-family list"
            );

            // The character path, at the size its key quantizes to.
            let old = rustkit_text::macos::GlyphRasterizer::with_style(
                GEORGIA_LIST,
                (size * 10.0) as u32 as f32 / 10.0,
                weight,
                false,
            );
            assert!(rustkit_text::macos::css_list_resolutions() > before);
            for (ch, drawn) in text.chars().zip(from_run) {
                let drawn = drawn.expect("the run's face is held");
                assert_eq!(Some(drawn), old.rasterize_char(ch, 0.0), "{ch:?} at {size}px");
            }
        }
    }

    /// Two faces, one glyph id each for the same letter: the key tells them
    /// apart, so regular and bold do not share a bitmap.
    #[test]
    fn the_key_is_the_face_and_the_glyph_not_the_family() {
        let regular = run_in(GEORGIA_LIST, 400, "W", 24.0);
        let bold = run_in(GEORGIA_LIST, 700, "W", 24.0);
        let k_regular = key(&regular, regular.glyphs[0].glyph_id);
        let k_bold = key(&bold, bold.glyphs[0].glyph_id);
        assert_ne!(k_regular, k_bold);
        assert_ne!(
            rasterize_run_glyph(&k_regular, 24.0).expect("regular"),
            rasterize_run_glyph(&k_bold, 24.0).expect("bold"),
        );

        // The same face reached through another list is the same key.
        let walked = run_in("No Such Family 9f2c, Georgia", 400, "W", 24.0);
        assert_eq!(key(&walked, walked.glyphs[0].glyph_id), k_regular);
    }

    /// A face the rasterizer does not hold draws nothing: the caller paints
    /// the command through the family-list path instead.
    #[test]
    fn a_face_that_is_not_held_is_not_drawn() {
        let run = run_in(GEORGIA_LIST, 400, "W", 24.0);
        let mut unknown = key(&run, run.glyphs[0].glyph_id);
        unknown.face ^= 0x5a5a;
        assert!(rasterize_run_glyph(&unknown, 24.0).is_none());
        // Held per size: a size layout never shaped at is not held either.
        assert!(rasterize_run_glyph(&key(&run, run.glyphs[0].glyph_id), 23.0).is_none());
    }
}

/// Where S0 changes a frame (campaign case `css-selectors`, "Lang prefix |=
/// (should be italic)"): layout measures italic system-font text in the
/// system italic, and the character path, resolving the same family list
/// again, drew another face at those advances. A run is drawn in the face
/// that was measured.
#[cfg(all(test, target_os = "macos"))]
mod shaped_run_measured_face_tests {
    use super::*;
    use rustkit_css::{ComputedStyle, Length};

    #[test]
    fn italic_system_text_is_drawn_in_the_face_layout_measured() {
        let mut style = ComputedStyle::new();
        style.font_family = "-apple-system, BlinkMacSystemFont, sans-serif".to_string();
        style.font_size = Length::Px(14.0);
        style.font_style = rustkit_css::FontStyle::Italic;
        let run = rustkit_layout::shape_line_run("Lang", &style, 14.0, 0.0).expect("run");
        let name = &run.face.postscript_name;
        assert!(
            name.starts_with(".SFNS") && name.contains("Italic"),
            "layout measures the system italic, got {name}"
        );

        // The rasterizer is handed that face, and draws the run's glyph.
        let held = rustkit_text::macos::face_font(run.face.id, 14.0).expect("the face is held");
        assert_eq!(&held.postscript_name(), name);
        let key = RunGlyphKey {
            face: run.face.id,
            glyph_id: run.glyphs[0].glyph_id,
            font_size: 140,
            subpixel_phase: 0,
        };
        let (bitmap, ..) = rasterize_run_glyph(&key, 14.0).expect("drawn");
        assert!(bitmap.iter().any(|&v| v > 128), "the glyph has ink");
    }
}

/// SHAPED-RUN CONTRACT, slice S0, paint side on Windows. Device-free: these
/// read the bitmaps the atlas is filled from. The character path is the
/// oracle.
#[cfg(all(test, windows))]
mod shaped_run_windows_paint_tests {
    use super::*;
    use rustkit_css::{ComputedStyle, Length};

    const GEORGIA_LIST: &str = "Georgia, 'Times New Roman', serif";

    fn run_in(family: &str, weight: u16, text: &str, size: f32) -> rustkit_layout::GlyphRun {
        let mut style = ComputedStyle::new();
        style.font_family = family.to_string();
        style.font_size = Length::Px(size);
        style.font_weight = rustkit_css::FontWeight(weight);
        rustkit_layout::shape_line_run(text, &style, size, 0.0).expect("a Latin line has a run")
    }

    fn key(run: &rustkit_layout::GlyphRun, glyph_id: u16) -> RunGlyphKey {
        RunGlyphKey {
            face: run.face.id,
            glyph_id,
            font_size: (run.font_size * 10.0) as u32,
            subpixel_phase: 0,
        }
    }

    /// What the run draws is, byte for byte, what the character path draws
    /// when it resolves the same list to the same face.
    #[test]
    fn run_glyphs_match_the_character_path() {
        for (weight, text, size) in [(400u16, "Wave", 24.0f32), (700, "Type", 17.6)] {
            let run = run_in(GEORGIA_LIST, weight, text, size);
            for (glyph, c) in run.glyphs.iter().zip(text.chars()) {
                let from_run = rasterize_run_glyph(&key(&run, glyph.glyph_id), run.font_size)
                    .expect("the run's face is held");
                let from_char = glyph::rasterize_char_for_test(c, "Georgia", size, weight)
                    .expect("the character path draws it");
                assert_eq!(from_run, from_char, "{c:?} at weight {weight}");
            }
        }
    }

    /// A space is a glyph with an advance and no ink: it must not fail the
    /// run, or every line with a space would fall back to the family path.
    #[test]
    fn a_space_in_a_run_is_blank_not_a_failure() {
        let run = run_in(GEORGIA_LIST, 400, "a b", 24.0);
        let space = run.glyphs[1].glyph_id;
        let (bitmap, w, h, advance, _, _) =
            rasterize_run_glyph(&key(&run, space), 24.0).expect("a space rasterizes");
        assert_eq!((bitmap.as_slice(), w, h), (&[0u8][..], 1, 1));
        assert!(advance > 0.0);
    }

    #[test]
    fn the_key_is_the_face_and_the_glyph_not_the_family() {
        let regular = run_in(GEORGIA_LIST, 400, "W", 24.0);
        let bold = run_in(GEORGIA_LIST, 700, "W", 24.0);
        let k_regular = key(&regular, regular.glyphs[0].glyph_id);
        let k_bold = key(&bold, bold.glyphs[0].glyph_id);
        assert_ne!(k_regular, k_bold);
        assert_ne!(
            rasterize_run_glyph(&k_regular, 24.0).expect("regular"),
            rasterize_run_glyph(&k_bold, 24.0).expect("bold"),
        );
        let walked = run_in("No Such Family 9f2c, Georgia", 400, "W", 24.0);
        assert_eq!(key(&walked, walked.glyphs[0].glyph_id), k_regular);
    }

    /// A face the rasterizer does not hold draws nothing: the caller paints
    /// the command through the family-list path instead.
    #[test]
    fn a_face_that_is_not_held_is_not_drawn() {
        let run = run_in(GEORGIA_LIST, 400, "W", 24.0);
        let mut unknown = key(&run, run.glyphs[0].glyph_id);
        unknown.face ^= 0x5a5a;
        assert!(rasterize_run_glyph(&unknown, 24.0).is_none());
    }
}
