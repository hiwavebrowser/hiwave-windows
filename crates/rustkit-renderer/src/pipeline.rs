//! Render pipeline creation.

use crate::{ColorVertex, TextureVertex};

// ==================== Gradient Pipeline ====================

/// Gradient parameters uniform structure (must match gradient.wgsl).
/// Total size: 80 bytes (20 values)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GradientParams {
    /// Rectangle x position in pixels
    pub rect_x: f32,
    /// Rectangle y position in pixels
    pub rect_y: f32,
    /// Rectangle width in pixels
    pub rect_width: f32,
    /// Rectangle height in pixels
    pub rect_height: f32,
    /// Gradient-specific param 0 (linear: angle_rad, radial: rx, conic: start_angle)
    pub param0: f32,
    /// Gradient-specific param 1 (radial: ry)
    pub param1: f32,
    /// Gradient-specific param 2 (radial: cx normalized 0-1)
    pub param2: f32,
    /// Gradient-specific param 3 (radial: cy normalized 0-1)
    pub param3: f32,
    /// Gradient type: 0 = linear, 1 = radial, 2 = conic
    pub gradient_type: u32,
    /// Repeating flag: 0 = clamp, 1 = repeat
    pub repeating: u32,
    /// Repeat length (position of last stop)
    pub repeat_length: f32,
    /// Number of color stops
    pub num_stops: u32,
    /// Border radius: top-left
    pub radius_tl: f32,
    /// Border radius: top-right
    pub radius_tr: f32,
    /// Border radius: bottom-right
    pub radius_br: f32,
    /// Border radius: bottom-left
    pub radius_bl: f32,
    /// Debug mode: 0=normal, 1=t-value, 2=direction, 3=position, 4=coverage, 5=raw-t
    pub debug_mode: u32,
    /// Padding for 16-byte alignment
    pub _padding0: u32,
    pub _padding1: u32,
    pub _padding2: u32,
}

impl Default for GradientParams {
    fn default() -> Self {
        Self {
            rect_x: 0.0,
            rect_y: 0.0,
            rect_width: 100.0,
            rect_height: 100.0,
            param0: 0.0,
            param1: 0.0,
            param2: 0.5,
            param3: 0.5,
            gradient_type: 0,
            repeating: 0,
            repeat_length: 1.0,
            num_stops: 0,
            radius_tl: 0.0,
            radius_tr: 0.0,
            radius_br: 0.0,
            radius_bl: 0.0,
            debug_mode: 0,
            _padding0: 0,
            _padding1: 0,
            _padding2: 0,
        }
    }
}

/// Color stop structure for gradient storage buffer (must match gradient.wgsl).
/// Total size: 20 bytes (5 f32 values)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GradientColorStop {
    /// Position along gradient (0.0 to 1.0)
    pub position: f32,
    /// Red component (0.0 to 1.0)
    pub r: f32,
    /// Green component (0.0 to 1.0)
    pub g: f32,
    /// Blue component (0.0 to 1.0)
    pub b: f32,
    /// Alpha component (0.0 to 1.0)
    pub a: f32,
}

/// Gradient rendering pipeline and resources.
pub struct GradientPipeline {
    pub pipeline: wgpu::RenderPipeline,
    pub uniform_buffer: wgpu::Buffer,
    pub stops_buffer: wgpu::Buffer,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
    /// Maximum number of color stops supported (buffer size)
    pub max_stops: usize,
}

/// Filter parameters uniform structure (must match WGSL).
/// Uses explicit padding to avoid vec3 alignment issues.
/// Total size: 32 bytes (8 f32 values)
#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct FilterParams {
    pub blur_radius: f32,
    pub filter_type: u32,
    pub filter_amount: f32,
    pub texture_width: f32,
    pub texture_height: f32,
    pub _padding0: f32,
    pub _padding1: f32,
    pub _padding2: f32,
}

impl Default for FilterParams {
    fn default() -> Self {
        Self {
            blur_radius: 0.0,
            filter_type: 0,
            filter_amount: 1.0,
            texture_width: 0.0,
            texture_height: 0.0,
            _padding0: 0.0,
            _padding1: 0.0,
            _padding2: 0.0,
        }
    }
}

/// Backdrop filter pipelines and resources.
pub struct BackdropFilterPipelines {
    pub blur_h_pipeline: wgpu::ComputePipeline,
    pub blur_v_pipeline: wgpu::ComputePipeline,
    pub color_filter_pipeline: wgpu::ComputePipeline,
    pub bind_group_layout: wgpu::BindGroupLayout,
    pub uniform_buffer: wgpu::Buffer,
}

/// Create the backdrop filter compute pipelines.
pub fn create_backdrop_filter_pipelines(device: &wgpu::Device) -> BackdropFilterPipelines {
    // Load the compute shader
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Backdrop Filter Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/backdrop_filter.wgsl").into()),
    });

    // Create bind group layout for filter operations
    // Binding 0: Uniform buffer with filter params
    // Binding 1: Input texture (read)
    // Binding 2: Output texture (write/storage)
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Backdrop Filter Bind Group Layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Texture {
                    multisampled: false,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
        ],
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Backdrop Filter Pipeline Layout"),
        bind_group_layouts: &[&bind_group_layout],
        push_constant_ranges: &[],
    });

    // Create the three compute pipelines (blur horizontal, blur vertical, color filter)
    let blur_h_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Blur Horizontal Pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("blur_horizontal"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let blur_v_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Blur Vertical Pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("blur_vertical"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    let color_filter_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("Color Filter Pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("apply_color_filter"),
        compilation_options: wgpu::PipelineCompilationOptions::default(),
        cache: None,
    });

    // Create uniform buffer for filter parameters
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Filter Params Buffer"),
        size: std::mem::size_of::<FilterParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    BackdropFilterPipelines {
        blur_h_pipeline,
        blur_v_pipeline,
        color_filter_pipeline,
        bind_group_layout,
        uniform_buffer,
    }
}

/// Create the color rendering pipeline.
pub fn create_color_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Color Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/color.wgsl").into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Color Pipeline Layout"),
        bind_group_layouts: &[uniform_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Color Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[ColorVertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

/// Create the texture rendering pipeline.
pub fn create_texture_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Texture Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/texture.wgsl").into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Texture Pipeline Layout"),
        bind_group_layouts: &[uniform_bind_group_layout, texture_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Texture Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[TextureVertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

/// Create the blit pipeline for copying RGBA textures.
/// Unlike the texture pipeline (for glyph rendering), this properly samples all 4 channels.
pub fn create_blit_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Blit Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Blit Pipeline Layout"),
        bind_group_layouts: &[uniform_bind_group_layout, texture_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Blit Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[TextureVertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                // Use REPLACE blend (no blending) for proper texture copying
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

/// Create the color-glyph (emoji) pipeline: the blit shader (samples real
/// RGBA), but with PREMULTIPLIED-alpha blending so translucent emoji composite
/// over existing content instead of REPLACE-ing it (which paints transparent
/// pixels as opaque black). CoreGraphics gives us premultiplied RGBA, so
/// premultiplied-over (`One, OneMinusSrcAlpha`) is the matching blend.
pub fn create_color_glyph_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
    texture_bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Color Glyph Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/blit.wgsl").into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Color Glyph Pipeline Layout"),
        bind_group_layouts: &[uniform_bind_group_layout, texture_bind_group_layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Color Glyph Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[TextureVertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    })
}

/// Create the gradient rendering pipeline with uniform and storage buffers.
pub fn create_gradient_pipeline(
    device: &wgpu::Device,
    surface_format: wgpu::TextureFormat,
    uniform_bind_group_layout: &wgpu::BindGroupLayout,
) -> GradientPipeline {
    // Load the gradient shader
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Gradient Shader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("shaders/gradient.wgsl").into()),
    });

    // Maximum number of color stops (should be plenty for any gradient)
    let max_stops: usize = 32;

    // Create uniform buffer for gradient parameters
    let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Gradient Params Buffer"),
        size: std::mem::size_of::<GradientParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Create storage buffer for color stops
    let stops_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Gradient Stops Buffer"),
        size: (max_stops * std::mem::size_of::<GradientColorStop>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Create bind group layout for gradient-specific data
    let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("Gradient Bind Group Layout"),
        entries: &[
            // Binding 0: Gradient parameters uniform
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            // Binding 1: Color stops storage buffer
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    // Create bind group
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Gradient Bind Group"),
        layout: &bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: stops_buffer.as_entire_binding(),
            },
        ],
    });

    // Create pipeline layout (group 0: viewport uniforms, group 1: gradient data)
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("Gradient Pipeline Layout"),
        bind_group_layouts: &[uniform_bind_group_layout, &bind_group_layout],
        push_constant_ranges: &[],
    });

    // Create render pipeline
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Gradient Pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            // Use ColorVertex layout (we ignore the color but need the position)
            buffers: &[crate::ColorVertex::LAYOUT],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: surface_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 1,
            mask: !0,
            alpha_to_coverage_enabled: false,
        },
        multiview: None,
        cache: None,
    });

    GradientPipeline {
        pipeline,
        uniform_buffer,
        stops_buffer,
        bind_group_layout,
        bind_group,
        max_stops,
    }
}
