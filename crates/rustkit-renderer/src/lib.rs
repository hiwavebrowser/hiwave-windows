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
use rustkit_layout::{DisplayCommand, Rect};
use std::sync::Arc;
use thiserror::Error;
use wgpu::util::DeviceExt;

mod glyph;
mod pipeline;
mod shaders;
pub mod screenshot;

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

    #[error("Texture upload failed: {0}")]
    TextureUpload(String),

    #[error("Glyph rasterization failed: {0}")]
    GlyphRasterization(String),

    #[error("Surface error: {0}")]
    Surface(#[from] wgpu::SurfaceError),
}

// ==================== Render Statistics ====================

/// Statistics about the last render pass.
#[derive(Debug, Clone, Default)]
pub struct RenderStats {
    pub color_vertex_count: usize,
    pub color_index_count: usize,
    pub texture_vertex_count: usize,
    pub texture_index_count: usize,
    pub clip_stack_depth: usize,
    pub stacking_context_depth: usize,
}

/// Generate a simple ISO8601-ish timestamp without external dependencies.
fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple conversion (approximate, doesn't handle leap seconds etc.)
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let remaining = (days % 365) as u32;
    let month = remaining / 30 + 1;
    let day = remaining % 30 + 1;
    let hours = (secs % 86400) / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        years, month, day, hours, minutes, seconds
    )
}

// ==================== Vertex Types ====================

/// Vertex for solid color rendering.
#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct ColorVertex {
    pub position: [f32; 2],
    pub color: [f32; 4],
}

/// Convert one 8-bit sRGB channel to linear-light [0,1]. CSS colors are sRGB,
/// but the render targets are sRGB-encoded (`*UnormSrgb`), so the GPU re-encodes
/// whatever the shader outputs on write. Uploading raw sRGB values therefore
/// double-encodes them (0.102 → 0.352 = a dark navy #1a1a2e showing as #5a5a76).
/// Converting to linear here means the target's sRGB encode reproduces the
/// original color exactly. Darks are shifted most, which is why dark UIs looked
/// washed out while near-white pages seemed fine.
fn srgb_channel_to_linear(u: u8) -> f32 {
    let s = u as f32 / 255.0;
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

/// A color as linear-light RGBA for upload to an sRGB render target.
fn color_to_linear(c: Color) -> [f32; 4] {
    [
        srgb_channel_to_linear(c.r),
        srgb_channel_to_linear(c.g),
        srgb_channel_to_linear(c.b),
        c.a,
    ]
}

/// Linearly interpolate two colors in gamma sRGB space (the raw 0–255 channels)
/// — matching Chrome's default gradient interpolation, so parity holds.
fn lerp_color(a: Color, b: Color, f: f32) -> Color {
    let f = f.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * f).round() as u8;
    Color {
        r: mix(a.r, b.r),
        g: mix(a.g, b.g),
        b: mix(a.b, b.b),
        a: a.a + (b.a - a.a) * f,
    }
}

/// Evaluate a gradient's color at position `t` (0.0–1.0) across its stops.
fn eval_gradient_stops(stops: &[rustkit_css::GradientStop], t: f32) -> Color {
    if stops.is_empty() {
        return Color::TRANSPARENT;
    }
    let t = t.clamp(0.0, 1.0);
    let first = &stops[0];
    let last = &stops[stops.len() - 1];
    if t <= first.position {
        return first.color;
    }
    if t >= last.position {
        return last.color;
    }
    for pair in stops.windows(2) {
        if t >= pair[0].position && t <= pair[1].position {
            let span = (pair[1].position - pair[0].position).max(f32::EPSILON);
            return lerp_color(pair[0].color, pair[1].color, (t - pair[0].position) / span);
        }
    }
    last.color
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
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some(key),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
                    bytes_per_row: Some(4 * width),
                    rows_per_image: Some(height),
                },
                wgpu::Extent3d {
                    width,
                    height,
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
}

// ==================== Renderer ====================

/// The main display list renderer.
pub struct Renderer {
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surface_format: wgpu::TextureFormat,

    // Pipelines
    color_pipeline: wgpu::RenderPipeline,
    texture_pipeline: wgpu::RenderPipeline,

    // Uniform buffer
    uniform_buffer: wgpu::Buffer,
    uniform_bind_group: wgpu::BindGroup,
    viewport_size: (u32, u32),

    // Vertex batching
    color_vertices: Vec<ColorVertex>,
    color_indices: Vec<u32>,
    texture_vertices: Vec<TextureVertex>,
    texture_indices: Vec<u32>,

    // State stacks
    clip_stack: Vec<Rect>,
    stacking_contexts: Vec<StackingContext>,

    // Caches
    texture_cache: TextureCache,
    glyph_cache: GlyphCache,

    // Texture bind group layout (for sharing)
    texture_bind_group_layout: wgpu::BindGroupLayout,
}

/// A stacking context for z-ordering.
#[derive(Debug, Clone)]
pub struct StackingContext {
    pub z_index: i32,
    pub rect: Rect,
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

        // Create caches
        let texture_cache = TextureCache::new(&device, texture_bind_group_layout.clone());
        let glyph_cache = GlyphCache::new(&device, &queue, texture_bind_group_layout.clone())?;

        Ok(Self {
            device,
            queue,
            surface_format,
            color_pipeline,
            texture_pipeline,
            uniform_buffer,
            uniform_bind_group,
            viewport_size: (800, 600),
            color_vertices: Vec::with_capacity(4096),
            color_indices: Vec::with_capacity(8192),
            texture_vertices: Vec::with_capacity(4096),
            texture_indices: Vec::with_capacity(8192),
            clip_stack: Vec::new(),
            stacking_contexts: Vec::new(),
            texture_cache,
            glyph_cache,
            texture_bind_group_layout,
        })
    }

    /// Set the viewport size.
    pub fn set_viewport_size(&mut self, width: u32, height: u32) {
        self.viewport_size = (width, height);

        let uniforms = Uniforms {
            viewport_size: [width as f32, height as f32],
            _padding: [0.0; 2],
        };

        self.queue.write_buffer(&self.uniform_buffer, 0, bytemuck::cast_slice(&[uniforms]));
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
        self.clip_stack.clear();
        self.stacking_contexts.clear();

        // Process commands
        for cmd in commands {
            self.process_command(cmd);
        }

        // Render
        self.flush_to(target)?;

        Ok(())
    }

    /// Process a single display command.
    fn process_command(&mut self, cmd: &DisplayCommand) {
        match cmd {
            DisplayCommand::SolidColor(color, rect) => {
                self.draw_solid_rect(*rect, *color);
            }

            DisplayCommand::LinearGradient {
                rect,
                angle_deg,
                stops,
            } => {
                self.draw_linear_gradient(*rect, *angle_deg, stops);
            }

            DisplayCommand::RadialGradient {
                rect,
                cx,
                cy,
                circle,
                stops,
            } => {
                self.draw_radial_gradient(*rect, *cx, *cy, *circle, stops);
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

            DisplayCommand::Text {
                text,
                x,
                y,
                color,
                font_size,
                font_family,
                font_weight,
                font_style,
                gradient,
                gradient_rect,
            } => {
                self.draw_text(
                    text,
                    *x,
                    *y,
                    *color,
                    *font_size,
                    font_family,
                    *font_weight,
                    *font_style,
                    gradient.as_deref(),
                    *gradient_rect,
                );
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
            } => {
                self.draw_image(url, *dest_rect);
            }

            DisplayCommand::BackgroundImage {
                url,
                rect,
                size: _,
                position: _,
                repeat: _,
            } => {
                self.draw_image(url, *rect);
            }

            DisplayCommand::PushClip(rect) => {
                self.push_clip(*rect);
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
                // Approximate circle with a square for now
                // TODO: Implement proper circle rendering with triangles
                self.draw_solid_rect(
                    Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0),
                    *color,
                );
            }

            DisplayCommand::StrokeCircle { cx, cy, radius, color, width } => {
                // Approximate with a square border
                let outer = Rect::new(cx - radius, cy - radius, radius * 2.0, radius * 2.0);
                self.draw_border(outer, *color, *width, *width, *width, *width);
            }

            DisplayCommand::FillEllipse { rect, color } => {
                // Approximate with rectangle
                self.draw_solid_rect(*rect, *color);
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
                    
                    let c = color_to_linear(*color);
                    
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
                    let c = color_to_linear(*color);
                    
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
        }
    }

    /// Draw a solid color rectangle.
    fn draw_solid_rect(&mut self, rect: Rect, color: Color) {
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

        let c = color_to_linear(color);

        let base = self.color_vertices.len() as u32;

        self.color_vertices.extend_from_slice(&[
            ColorVertex { position: [rect.x, rect.y], color: c },
            ColorVertex { position: [rect.x + rect.width, rect.y], color: c },
            ColorVertex { position: [rect.x + rect.width, rect.y + rect.height], color: c },
            ColorVertex { position: [rect.x, rect.y + rect.height], color: c },
        ]);

        self.color_indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
        ]);
    }

    /// Draw a linear gradient by emitting per-vertex-colored quads that the GPU
    /// interpolates. Axis-aligned gradients (to top/bottom/left/right) are drawn
    /// as one exact strip per adjacent stop pair — correct for any number of
    /// stops. Angled gradients fall back to a single quad whose four corners are
    /// evaluated along the gradient axis (exact for two stops; a smooth
    /// approximation for more).
    fn draw_linear_gradient(
        &mut self,
        rect: Rect,
        angle_deg: f32,
        stops: &[rustkit_css::GradientStop],
    ) {
        let rect = if let Some(clip) = self.current_clip() {
            match rect.intersect(&clip) {
                Some(r) => r,
                None => return,
            }
        } else {
            rect
        };
        if stops.len() < 2 {
            return;
        }
        let a = ((angle_deg % 360.0) + 360.0) % 360.0;
        const EPS: f32 = 1.0;
        let vertical = (a - 0.0).abs() < EPS || (a - 180.0).abs() < EPS || (a - 360.0).abs() < EPS;
        let horizontal = (a - 90.0).abs() < EPS || (a - 270.0).abs() < EPS;

        if vertical || horizontal {
            // `f` runs 0..1 across the box; for `to bottom`(180)/`to right`(90)
            // t=0 is the top/left edge. Subdivide the axis into fine segments and
            // colour each boundary from the stops in gamma sRGB
            // (eval_gradient_stops). The GPU interpolates the linear-light vertex
            // colours *within* a segment, but because the boundaries follow the
            // gamma curve, the whole run approximates Chrome's gamma-space
            // interpolation — a single strip per stop pair would interpolate in
            // linear light and read too bright at the midpoint (188 vs 127 for
            // red->blue). 64 segments keeps the midpoint within ~1 level.
            let reversed = (a - 0.0).abs() < EPS || (a - 360.0).abs() < EPS || (a - 270.0).abs() < EPS;
            const SEG: usize = 64;
            for i in 0..SEG {
                let f0 = i as f32 / SEG as f32;
                let f1 = (i + 1) as f32 / SEG as f32;
                let t0 = if reversed { 1.0 - f0 } else { f0 };
                let t1 = if reversed { 1.0 - f1 } else { f1 };
                let c0 = eval_gradient_stops(stops, t0);
                let c1 = eval_gradient_stops(stops, t1);
                if vertical {
                    let y0 = rect.y + rect.height * f0;
                    let y1 = rect.y + rect.height * f1;
                    self.push_gradient_quad(
                        [rect.x, y0], [rect.x + rect.width, y0],
                        [rect.x + rect.width, y1], [rect.x, y1],
                        c0, c0, c1, c1,
                    );
                } else {
                    let x0 = rect.x + rect.width * f0;
                    let x1 = rect.x + rect.width * f1;
                    self.push_gradient_quad(
                        [x0, rect.y], [x1, rect.y],
                        [x1, rect.y + rect.height], [x0, rect.y + rect.height],
                        c0, c1, c1, c0,
                    );
                }
            }
        } else {
            // Angled: project the four corners onto the gradient axis and colour
            // each by its normalised position. 0deg points up in CSS.
            let rad = a.to_radians();
            let (dx, dy) = (rad.sin(), -rad.cos());
            let corners = [
                [rect.x, rect.y],
                [rect.x + rect.width, rect.y],
                [rect.x + rect.width, rect.y + rect.height],
                [rect.x, rect.y + rect.height],
            ];
            let proj: Vec<f32> = corners.iter().map(|c| c[0] * dx + c[1] * dy).collect();
            let (mut tmin, mut tmax) = (f32::INFINITY, f32::NEG_INFINITY);
            for &p in &proj {
                tmin = tmin.min(p);
                tmax = tmax.max(p);
            }
            let span = (tmax - tmin).max(f32::EPSILON);
            let cols: Vec<Color> = proj
                .iter()
                .map(|p| eval_gradient_stops(stops, (p - tmin) / span))
                .collect();
            self.push_gradient_quad(
                corners[0], corners[1], corners[2], corners[3],
                cols[0], cols[1], cols[2], cols[3],
            );
        }
    }

    /// Draw a radial gradient by tessellating the rect into a fine grid and
    /// colouring each vertex by its normalised distance from the center
    /// (farthest-corner size). `circle` keeps the radius equal on both axes;
    /// `ellipse` (default) scales per-axis so the ellipse passes through the
    /// farthest corner (radii = sqrt(2) x extent). The GPU interpolates within
    /// each cell; a 64x64 grid keeps the interpolation close to CSS's, matching
    /// the density draw_linear_gradient uses along its axis.
    fn draw_radial_gradient(
        &mut self,
        rect: Rect,
        cx: f32,
        cy: f32,
        circle: bool,
        stops: &[rustkit_css::GradientStop],
    ) {
        let rect = if let Some(clip) = self.current_clip() {
            match rect.intersect(&clip) {
                Some(r) => r,
                None => return,
            }
        } else {
            rect
        };
        if stops.len() < 2 {
            return;
        }

        let center = [rect.x + cx * rect.width, rect.y + cy * rect.height];
        // Farthest-corner extents from the center along each axis.
        let ext_x = cx.max(1.0 - cx) * rect.width;
        let ext_y = cy.max(1.0 - cy) * rect.height;
        let (rx, ry) = if circle {
            let r = (ext_x * ext_x + ext_y * ext_y).sqrt().max(f32::EPSILON);
            (r, r)
        } else {
            // farthest-corner ellipse: radii scaled so it passes through the
            // farthest corner while keeping the ext_x:ext_y aspect ratio.
            let s = std::f32::consts::SQRT_2;
            ((ext_x * s).max(f32::EPSILON), (ext_y * s).max(f32::EPSILON))
        };

        let t_at = |px: f32, py: f32| -> f32 {
            let dx = (px - center[0]) / rx;
            let dy = (py - center[1]) / ry;
            (dx * dx + dy * dy).sqrt().min(1.0)
        };

        const GRID: usize = 64;
        for gy in 0..GRID {
            let fy0 = gy as f32 / GRID as f32;
            let fy1 = (gy + 1) as f32 / GRID as f32;
            let y0 = rect.y + rect.height * fy0;
            let y1 = rect.y + rect.height * fy1;
            for gx in 0..GRID {
                let fx0 = gx as f32 / GRID as f32;
                let fx1 = (gx + 1) as f32 / GRID as f32;
                let x0 = rect.x + rect.width * fx0;
                let x1 = rect.x + rect.width * fx1;
                let c00 = eval_gradient_stops(stops, t_at(x0, y0));
                let c10 = eval_gradient_stops(stops, t_at(x1, y0));
                let c11 = eval_gradient_stops(stops, t_at(x1, y1));
                let c01 = eval_gradient_stops(stops, t_at(x0, y1));
                self.push_gradient_quad(
                    [x0, y0], [x1, y0], [x1, y1], [x0, y1],
                    c00, c10, c11, c01,
                );
            }
        }
    }

    /// Emit a quad (two triangles) with a colour at each corner.
    fn push_gradient_quad(
        &mut self,
        p0: [f32; 2], p1: [f32; 2], p2: [f32; 2], p3: [f32; 2],
        c0: Color, c1: Color, c2: Color, c3: Color,
    ) {
        let cv = color_to_linear;
        let base = self.color_vertices.len() as u32;
        self.color_vertices.extend_from_slice(&[
            ColorVertex { position: p0, color: cv(c0) },
            ColorVertex { position: p1, color: cv(c1) },
            ColorVertex { position: p2, color: cv(c2) },
            ColorVertex { position: p3, color: cv(c3) },
        ]);
        self.color_indices.extend_from_slice(&[
            base, base + 1, base + 2,
            base, base + 2, base + 3,
        ]);
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

    /// Draw text.
    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        color: Color,
        font_size: f32,
        font_family: &str,
        font_weight: u16,
        font_style: u8,
        gradient: Option<&[rustkit_css::GradientStop]>,
        gradient_rect: Rect,
    ) {
        let mut cursor_x = x;
        let c_solid = color_to_linear(color);

        // Get atlas size before the loop to avoid borrow issues
        let atlas_size = self.glyph_cache.atlas_size() as f32;

        for ch in text.chars() {
            let key = GlyphKey {
                codepoint: ch,
                font_family: font_family.to_string(),
                font_size: (font_size * 10.0) as u32,
                font_weight,
                font_style,
            };

            // Clone the entry to avoid borrow issues
            if let Some(entry) = self.glyph_cache.get_or_rasterize(&self.device, &self.queue, &key) {
                let glyph_x = cursor_x + entry.offset[0];
                let glyph_y = y + entry.offset[1];
                let glyph_w = (entry.tex_coords[2] - entry.tex_coords[0]) * atlas_size;
                let glyph_h = (entry.tex_coords[3] - entry.tex_coords[1]) * atlas_size;

                // background-clip:text — tint this glyph by the gradient sampled
                // at the glyph's horizontal centre within the text box.
                let c = match gradient {
                    Some(stops) if gradient_rect.width > 0.0 => {
                        let t = (glyph_x + glyph_w * 0.5 - gradient_rect.x) / gradient_rect.width;
                        color_to_linear(eval_gradient_stops(stops, t))
                    }
                    _ => c_solid,
                };

                let base = self.texture_vertices.len() as u32;

                self.texture_vertices.extend_from_slice(&[
                    TextureVertex {
                        position: [glyph_x, glyph_y],
                        tex_coords: [entry.tex_coords[0], entry.tex_coords[1]],
                        color: c,
                    },
                    TextureVertex {
                        position: [glyph_x + glyph_w, glyph_y],
                        tex_coords: [entry.tex_coords[2], entry.tex_coords[1]],
                        color: c,
                    },
                    TextureVertex {
                        position: [glyph_x + glyph_w, glyph_y + glyph_h],
                        tex_coords: [entry.tex_coords[2], entry.tex_coords[3]],
                        color: c,
                    },
                    TextureVertex {
                        position: [glyph_x, glyph_y + glyph_h],
                        tex_coords: [entry.tex_coords[0], entry.tex_coords[3]],
                        color: c,
                    },
                ]);

                self.texture_indices.extend_from_slice(&[
                    base, base + 1, base + 2,
                    base, base + 2, base + 3,
                ]);

                cursor_x += entry.advance;
            } else {
                // Fallback: advance by estimated width
                cursor_x += font_size * 0.6;
            }
        }
    }

    /// Draw an image.
    fn draw_image(&mut self, url: &str, rect: Rect) {
        if self.texture_cache.contains(url) {
            let base = self.texture_vertices.len() as u32;

            self.texture_vertices.extend_from_slice(&[
                TextureVertex {
                    position: [rect.x, rect.y],
                    tex_coords: [0.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
                TextureVertex {
                    position: [rect.x + rect.width, rect.y],
                    tex_coords: [1.0, 0.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
                TextureVertex {
                    position: [rect.x + rect.width, rect.y + rect.height],
                    tex_coords: [1.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
                TextureVertex {
                    position: [rect.x, rect.y + rect.height],
                    tex_coords: [0.0, 1.0],
                    color: [1.0, 1.0, 1.0, 1.0],
                },
            ]);

            self.texture_indices.extend_from_slice(&[
                base, base + 1, base + 2,
                base, base + 2, base + 3,
            ]);
        }
        // If image not loaded, skip (async loading handled elsewhere)
    }


    /// Push a clipping rectangle.
    fn push_clip(&mut self, rect: Rect) {
        let clip = if let Some(current) = self.clip_stack.last() {
            if let Some(intersected) = current.intersect(&rect) {
                intersected
            } else {
                Rect::new(0.0, 0.0, 0.0, 0.0) // Empty clip
            }
        } else {
            rect
        };
        self.clip_stack.push(clip);
    }

    /// Pop the current clipping rectangle.
    fn pop_clip(&mut self) {
        self.clip_stack.pop();
    }

    /// Get the current clip rectangle.
    fn current_clip(&self) -> Option<Rect> {
        self.clip_stack.last().copied()
    }

    /// Flush all batched vertices to the target.
    fn flush_to(&mut self, target: &wgpu::TextureView) -> Result<(), RendererError> {
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Render Encoder"),
        });

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Main Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 1.0,
                            b: 1.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // Draw solid colors
            if !self.color_vertices.is_empty() {
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

            // Draw textured quads (images and glyphs)
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
        }

        self.queue.submit(std::iter::once(encoder.finish()));

        Ok(())
    }

    /// Execute a display list and capture the result to a PNG file.
    ///
    /// This renders to an offscreen texture and reads back the pixels.
    pub fn execute_and_capture(
        &mut self,
        commands: &[DisplayCommand],
        output_path: impl AsRef<std::path::Path>,
    ) -> Result<screenshot::ScreenshotMetadata, RendererError> {
        let (width, height) = self.viewport_size;
        let capture_format = self.surface_format;
        
        // Create offscreen target
        let (texture, view) = screenshot::create_offscreen_target(
            &self.device,
            width,
            height,
            capture_format,
        );
        
        // Render to offscreen target
        self.execute(commands, &view)?;
        
        // Create readback buffer
        let readback = screenshot::GpuReadbackBuffer::new(&self.device, width, height);
        
        // Copy texture to readback buffer
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("Screenshot Copy Encoder"),
        });
        readback.copy_from_texture(&mut encoder, &texture);
        self.queue.submit(std::iter::once(encoder.finish()));
        
        // Read back the data
        let mut pixels = readback
            .read_data_sync(&self.device)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))?;

        // If the capture target is BGRA, swizzle to RGBA for PNG encoding.
        match capture_format {
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                for px in pixels.chunks_exact_mut(4) {
                    px.swap(0, 2);
                }
            }
            _ => {}
        }
        
        // Save PNG
        screenshot::save_png(&output_path, width, height, &pixels)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))?;
        
        // Create and save metadata
        let metadata = screenshot::ScreenshotMetadata {
            width,
            height,
            adapter: "Unknown".to_string(), // TODO: Get actual adapter name
            format: format!("{:?}", capture_format),
            timestamp: chrono_lite_timestamp(),
            color_vertex_count: self.color_vertices.len(),
            texture_vertex_count: self.texture_vertices.len(),
        };
        
        let metadata_path = output_path.as_ref().with_extension("json");
        screenshot::save_metadata(&metadata_path, &metadata)
            .map_err(|e| RendererError::TextureUpload(e.to_string()))?;
        
        Ok(metadata)
    }

    /// Get render statistics for the last frame.
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

    /// Get access to the texture cache for external image loading.
    pub fn texture_cache(&mut self) -> &mut TextureCache {
        &mut self.texture_cache
    }

    /// Get access to the glyph cache.
    pub fn glyph_cache(&mut self) -> &mut GlyphCache {
        &mut self.glyph_cache
    }

    /// Dump the glyph atlas (CPU mirror) to a PNG for debugging.
    pub fn dump_glyph_atlas_png(&self, path: impl AsRef<std::path::Path>) -> Result<(), RendererError> {
        self.glyph_cache.dump_cpu_atlas_png(path)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_vertex_size() {
        assert_eq!(std::mem::size_of::<ColorVertex>(), 24);
    }

    #[test]
    fn test_texture_vertex_size() {
        assert_eq!(std::mem::size_of::<TextureVertex>(), 32);
    }

    #[test]
    fn test_gradient_interp_is_gamma_space() {
        // Chrome interpolates legacy gradients in gamma sRGB: the midpoint of
        // red and blue is the raw-channel average (127,0,127), NOT a linear
        // blend (~188). eval_gradient_stops must lerp the raw 0-255 channels.
        let stops = [
            rustkit_css::GradientStop { color: Color::from_rgb(255, 0, 0), position: 0.0 },
            rustkit_css::GradientStop { color: Color::from_rgb(0, 0, 255), position: 1.0 },
        ];
        let mid = eval_gradient_stops(&stops, 0.5);
        assert!((mid.r as i32 - 127).abs() <= 1, "r={}", mid.r);
        assert_eq!(mid.g, 0);
        assert!((mid.b as i32 - 128).abs() <= 1, "b={}", mid.b);
        // Endpoints are exact.
        assert_eq!(eval_gradient_stops(&stops, 0.0), Color::from_rgb(255, 0, 0));
        assert_eq!(eval_gradient_stops(&stops, 1.0), Color::from_rgb(0, 0, 255));
    }

    #[test]
    fn test_srgb_linear_roundtrip_dark() {
        // The gamma double-encode guard as a unit: a dark sRGB channel converted
        // to linear then back to sRGB must return itself (the #1a1a2e = 26 case
        // that rendered as 90 before color_to_linear).
        for u in [0u8, 26, 46, 128, 200, 255] {
            let lin = srgb_channel_to_linear(u);
            // sRGB encode of the linear value (inverse EOTF).
            let enc = if lin <= 0.0031308 {
                lin * 12.92
            } else {
                1.055 * lin.powf(1.0 / 2.4) - 0.055
            };
            let back = (enc * 255.0).round() as u8;
            assert!((back as i32 - u as i32).abs() <= 1, "u={u} back={back}");
        }
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
}

