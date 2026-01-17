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

            DisplayCommand::LinearGradient {
                rect,
                direction,
                stops,
                repeating,
                border_radius,
            } => {
                self.draw_linear_gradient(*rect, *direction, stops, *repeating, *border_radius);
            }

            DisplayCommand::RadialGradient {
                rect,
                shape,
                size,
                center,
                stops,
                repeating,
                border_radius,
            } => {
                self.draw_radial_gradient(*rect, *shape, *size, *center, stops, *repeating, *border_radius);
            }

            DisplayCommand::ConicGradient {
                rect,
                from_angle,
                center,
                stops,
                repeating,
                border_radius,
            } => {
                self.draw_conic_gradient(*rect, *from_angle, *center, stops, *repeating, *border_radius);
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

        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];

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
    ) {
        let mut cursor_x = x;
        let c = [
            color.r as f32 / 255.0,
            color.g as f32 / 255.0,
            color.b as f32 / 255.0,
            color.a,
        ];

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

    // ==================== Gradient Rendering ====================

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

        // Normalize color stop positions
        let mut normalized_stops: Vec<(f32, Color)> = Vec::with_capacity(stops.len());
        for (i, stop) in stops.iter().enumerate() {
            let pos = stop.position.unwrap_or_else(|| {
                if stops.len() == 1 {
                    0.5
                } else {
                    i as f32 / (stops.len() - 1) as f32
                }
            });
            normalized_stops.push((pos, stop.color));
        }

        // For repeating gradients, get the repeat length (last stop position)
        let repeat_length = if repeating && !normalized_stops.is_empty() {
            normalized_stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

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

            for i in 0..step_count {
                let t = if reverse {
                    1.0 - (i as f32 + 0.5) / step_count as f32
                } else {
                    (i as f32 + 0.5) / step_count as f32
                };
                let t_final = apply_t(t);
                let color = Self::interpolate_color(&normalized_stops, t_final);
                let x_pos = rect.x + i as f32 * strip_width;
                self.draw_solid_rect(Rect::new(x_pos, rect.y, strip_width + 0.5, rect.height), color);
            }
        } else if !has_radius && is_vertical {
            // Vertical gradient (top to bottom or bottom to top) - fast path
            let reverse = angle_deg < 90.0 || angle_deg > 270.0;
            let step_count = rect.height.max(2.0) as usize;
            let strip_height = rect.height / step_count as f32;

            for i in 0..step_count {
                let t = if reverse {
                    1.0 - (i as f32 + 0.5) / step_count as f32
                } else {
                    (i as f32 + 0.5) / step_count as f32
                };
                let t_final = apply_t(t);
                let color = Self::interpolate_color(&normalized_stops, t_final);
                let y_pos = rect.y + i as f32 * strip_height;
                self.draw_solid_rect(Rect::new(rect.x, y_pos, rect.width, strip_height + 0.5), color);
            }
        } else {
            // Diagonal gradient or gradient with border-radius - render using cells
            // Use adaptive cell sizing to prevent GPU buffer overflow for large gradients
            // while maintaining 1px quality for small UI elements
            let area = rect.width * rect.height;
            let max_cells: f32 = 100_000.0; // Limit cells to prevent buffer overflow
            let cell_size: f32 = if area > max_cells {
                (area / max_cells).sqrt().ceil()
            } else {
                1.0
            };

            // Calculate the gradient line length (diagonal of rectangle projected onto gradient line)
            // For CSS gradients, the gradient line goes through the center and extends to the corners
            let half_width = rect.width / 2.0;
            let half_height = rect.height / 2.0;

            // The gradient length is the distance along the gradient line from corner to corner
            let gradient_half_length = (half_width * sin_a.abs() + half_height * cos_a.abs()).max(0.001);

            let mut y = rect.y;
            while y < rect.y + rect.height {
                let cell_h = cell_size.min(rect.y + rect.height - y);
                let mut x = rect.x;

                while x < rect.x + rect.width {
                    let cell_w = cell_size.min(rect.x + rect.width - x);
                    let cell_center_x = x + cell_w / 2.0;
                    let cell_center_y = y + cell_h / 2.0;

                    // Check border-radius clipping
                    let alpha_coverage = Self::point_in_rounded_rect(
                        cell_center_x,
                        cell_center_y,
                        rect,
                        border_radius,
                    );

                    if alpha_coverage > 0.0 {
                        // Calculate position relative to center of rect
                        let px = cell_center_x - rect.x - half_width;
                        let py = cell_center_y - rect.y - half_height;

                        // Project point onto gradient line
                        // Gradient direction: (sin_a, -cos_a) where angle 0 is "to top"
                        let projection = px * sin_a + py * (-cos_a);

                        // Normalize to 0-1 range
                        let t = (projection / gradient_half_length + 1.0) / 2.0;
                        let t_final = apply_t(t);

                        let mut color = Self::interpolate_color(&normalized_stops, t_final);

                        // Apply border-radius alpha
                        if alpha_coverage < 1.0 {
                            color = Color::new(color.r, color.g, color.b, color.a * alpha_coverage);
                        }

                        if color.a > 0.0 {
                            self.draw_solid_rect(Rect::new(x, y, cell_w, cell_h), color);
                        }
                    }

                    x += cell_size;
                }
                y += cell_size;
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
                        let aspect = rect.width / rect.height.max(1.0);
                        (min_dist, min_dist / aspect)
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
                        let aspect = rect.width / rect.height.max(1.0);
                        (max_dist, max_dist / aspect)
                    }
                }
            }
            rustkit_css::RadialSize::Explicit(r1, r2) => (r1, r2),
        };

        // Normalize color stops
        let mut normalized_stops: Vec<(f32, Color)> = Vec::with_capacity(stops.len());
        for (i, stop) in stops.iter().enumerate() {
            let pos = stop.position.unwrap_or_else(|| {
                if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
            });
            normalized_stops.push((pos, stop.color));
        }

        // For repeating gradients, get the repeat length (last stop position)
        let repeat_length = if repeating && !normalized_stops.is_empty() {
            normalized_stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

        // Adaptive step sizing to prevent GPU buffer overflow for large gradients
        // while maintaining 1px quality for small UI elements
        let area = rect.width * rect.height;
        let max_cells: f32 = 100_000.0; // Limit cells to prevent buffer overflow
        let step_size: f32 = if area > max_cells {
            (area / max_cells).sqrt().ceil()
        } else {
            1.0
        };
        let mut y = rect.y;
        while y < rect.y + rect.height {
            let row_height = step_size.min(rect.y + rect.height - y);
            let mut x = rect.x;
            while x < rect.x + rect.width {
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
                    let mut color = Self::interpolate_color(&normalized_stops, t_final);

                    // Apply border-radius alpha
                    if alpha_coverage < 1.0 {
                        color = Color::new(color.r, color.g, color.b, color.a * alpha_coverage);
                    }

                    // Only draw if not fully transparent
                    if color.a > 0.0 {
                        self.draw_solid_rect(Rect::new(x, y, col_width, row_height), color);
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

        // Normalize color stops
        let mut normalized_stops: Vec<(f32, Color)> = Vec::with_capacity(stops.len());
        for (i, stop) in stops.iter().enumerate() {
            let pos = stop.position.unwrap_or_else(|| {
                if stops.len() == 1 { 0.5 } else { i as f32 / (stops.len() - 1) as f32 }
            });
            normalized_stops.push((pos, stop.color));
        }

        // For repeating gradients, get the repeat length from the last stop
        let repeat_length = if repeating && !normalized_stops.is_empty() {
            normalized_stops.last().map(|(pos, _)| *pos).unwrap_or(1.0).max(0.001)
        } else {
            1.0
        };

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

        let mut y = rect.y;
        while y < rect.y + rect.height {
            let row_height = step_size.min(rect.y + rect.height - y);
            let mut x = rect.x;
            while x < rect.x + rect.width {
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
                    let mut color = Self::interpolate_color(&normalized_stops, t);

                    // Apply border-radius alpha
                    if alpha_coverage < 1.0 {
                        color = Color::new(color.r, color.g, color.b, color.a * alpha_coverage);
                    }

                    if color.a > 0.0 {
                        self.draw_solid_rect(Rect::new(x, y, col_width, row_height), color);
                    }
                }

                x += step_size;
            }
            y += step_size;
        }
    }

    // ==================== Gradient Helper Methods ====================

    /// Check if a point is inside a rounded rectangle (with anti-aliasing).
    /// Returns alpha coverage (0.0-1.0).
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

        // Clamp radii to half the rect dimensions
        let max_r = (rect.width / 2.0).min(rect.height / 2.0);
        let r_tl = radius.top_left.min(max_r);
        let r_tr = radius.top_right.min(max_r);
        let r_br = radius.bottom_right.min(max_r);
        let r_bl = radius.bottom_left.min(max_r);

        // Check each corner
        let local_x = px - rect.x;
        let local_y = py - rect.y;
        let right_x = rect.width - local_x;
        let bottom_y = rect.height - local_y;

        // Top-left corner
        if local_x < r_tl && local_y < r_tl {
            let dx = r_tl - local_x;
            let dy = r_tl - local_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r_tl + 0.5 {
                return 0.0;
            } else if dist > r_tl - 0.5 {
                return 1.0 - (dist - (r_tl - 0.5));
            }
        }

        // Top-right corner
        if right_x < r_tr && local_y < r_tr {
            let dx = r_tr - right_x;
            let dy = r_tr - local_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r_tr + 0.5 {
                return 0.0;
            } else if dist > r_tr - 0.5 {
                return 1.0 - (dist - (r_tr - 0.5));
            }
        }

        // Bottom-right corner
        if right_x < r_br && bottom_y < r_br {
            let dx = r_br - right_x;
            let dy = r_br - bottom_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r_br + 0.5 {
                return 0.0;
            } else if dist > r_br - 0.5 {
                return 1.0 - (dist - (r_br - 0.5));
            }
        }

        // Bottom-left corner
        if local_x < r_bl && bottom_y < r_bl {
            let dx = r_bl - local_x;
            let dy = r_bl - bottom_y;
            let dist = (dx * dx + dy * dy).sqrt();
            if dist > r_bl + 0.5 {
                return 0.0;
            } else if dist > r_bl - 0.5 {
                return 1.0 - (dist - (r_bl - 0.5));
            }
        }

        1.0
    }

    /// Interpolate color at position t between gradient stops.
    /// Uses sRGB interpolation to match browser behavior.
    fn interpolate_color(stops: &[(f32, Color)], t: f32) -> Color {
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

                // Interpolate directly in sRGB space (browser default behavior)
                let r = ((1.0 - local_t) * color0.r as f32 + local_t * color1.r as f32).round() as u8;
                let g = ((1.0 - local_t) * color0.g as f32 + local_t * color1.g as f32).round() as u8;
                let b = ((1.0 - local_t) * color0.b as f32 + local_t * color1.b as f32).round() as u8;

                // Alpha is interpolated linearly
                let a = (1.0 - local_t) * color0.a + local_t * color1.a;

                return Color::new(r, g, b, a);
            }
        }
        stops[stops.len() - 1].1
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

