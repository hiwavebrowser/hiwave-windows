//! # RustKit Compositor
//!
//! GPU compositor with per-view swapchain support for the RustKit browser engine.
//!
//! ## Design Goals
//!
//! 1. **Per-view surfaces**: Each view has its own swapchain/surface
//! 2. **Resize correctness**: Swapchain recreated on WM_SIZE
//! 3. **Multi-view rendering**: No global state; views render independently
//! 4. **DirectComposition**: Smooth composition on Windows

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use thiserror::Error;
use tracing::{debug, info, trace, warn};

use rustkit_layout::DisplayCommand;
use rustkit_renderer::Renderer;
use rustkit_viewhost::{Bounds, ViewId};

/// Errors that can occur in the compositor.
#[derive(Error, Debug)]
pub enum CompositorError {
    #[error("Failed to create GPU device: {0}")]
    DeviceCreation(String),

    #[error("Failed to create surface: {0}")]
    SurfaceCreation(String),

    #[error("Surface not found for view: {0:?}")]
    SurfaceNotFound(ViewId),

    #[error("Swapchain error: {0}")]
    Swapchain(String),

    #[error("Render error: {0}")]
    Render(String),
}

/// Configuration for the compositor.
#[derive(Debug, Clone)]
pub struct CompositorConfig {
    /// Enable VSync.
    pub vsync: bool,
    /// Preferred surface format.
    pub format: wgpu::TextureFormat,
    /// Power preference for GPU selection.
    pub power_preference: wgpu::PowerPreference,
}

impl Default for CompositorConfig {
    fn default() -> Self {
        Self {
            vsync: true,
            // Use linear format to avoid double sRGB gamma correction.
            // CSS colors are already in sRGB space, so we don't want the GPU
            // to apply sRGB encoding when writing to the texture.
            format: wgpu::TextureFormat::Bgra8Unorm,
            power_preference: wgpu::PowerPreference::HighPerformance,
        }
    }
}

/// Per-view surface state.
pub struct SurfaceState {
    view_id: ViewId,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    width: u32,
    height: u32,
}

/// Headless texture state for offscreen rendering (used in testing/headless mode).
///
/// No `view_id` field: these live in `HashMap<ViewId, HeadlessState>`, so the
/// key already carries it. `SurfaceState` keeps its own copy because that one
/// is genuinely read.
pub struct HeadlessState {
    texture: wgpu::Texture,
    width: u32,
    height: u32,
}

impl SurfaceState {
    /// Resize the surface.
    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }

        if self.width == width && self.height == height {
            return;
        }

        self.width = width;
        self.height = height;
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(device, &self.config);

        trace!(view_id = ?self.view_id, width, height, "Surface resized");
    }

    /// Get the current texture for rendering.
    pub fn get_current_texture(&self) -> Result<wgpu::SurfaceTexture, CompositorError> {
        self.surface
            .get_current_texture()
            .map_err(|e| CompositorError::Swapchain(e.to_string()))
    }
}

/// The main compositor that manages GPU resources and surfaces.
pub struct Compositor {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    surfaces: RwLock<HashMap<ViewId, SurfaceState>>,
    headless_textures: RwLock<HashMap<ViewId, HeadlessState>>,
    config: CompositorConfig,
}

impl Compositor {
    /// Create a new compositor with default configuration.
    pub fn new() -> Result<Self, CompositorError> {
        Self::with_config(CompositorConfig::default())
    }

    /// Create a new compositor with custom configuration.
    pub fn with_config(config: CompositorConfig) -> Result<Self, CompositorError> {
        info!("Initializing compositor");

        // Create wgpu instance - use all backends to allow fallback options
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            // Use all available backends to maximize compatibility
            // On macOS this includes Metal and potentially software fallback
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        // Request adapter - try hardware first, then fall back to software
        let adapter = pollster::block_on(async {
            // First try hardware adapter
            let hardware = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: config.power_preference,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await;
            
            if hardware.is_some() {
                return hardware;
            }
            
            // Fall back to software adapter if hardware not available
            info!("No hardware GPU adapter found, trying software fallback");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: None,
                    force_fallback_adapter: true,
                })
                .await
        })
        .ok_or_else(|| CompositorError::DeviceCreation("No suitable GPU adapter found (tried hardware and software fallback)".into()))?;

        info!(adapter = ?adapter.get_info().name, "GPU adapter selected");

        // Create device and queue
        let (device, queue) = pollster::block_on(async {
            adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("RustKit Compositor Device"),
                        required_features: wgpu::Features::empty(),
                        required_limits: wgpu::Limits::default(),
                        memory_hints: wgpu::MemoryHints::Performance,
                    },
                    None,
                )
                .await
        })
        .map_err(|e| CompositorError::DeviceCreation(e.to_string()))?;

        Ok(Self {
            instance,
            adapter,
            device: Arc::new(device),
            queue: Arc::new(queue),
            surfaces: RwLock::new(HashMap::new()),
            headless_textures: RwLock::new(HashMap::new()),
            config,
        })
    }

    /// Create a surface for a view.
    ///
    /// # Safety
    ///
    /// The HWND must be valid and remain valid for the lifetime of the surface.
    #[cfg(windows)]
    pub unsafe fn create_surface_for_hwnd(
        &self,
        view_id: ViewId,
        hwnd: windows::Win32::Foundation::HWND,
        width: u32,
        height: u32,
    ) -> Result<(), CompositorError> {
        use raw_window_handle::{RawWindowHandle, Win32WindowHandle};

        debug!(?view_id, width, height, "Creating surface for HWND");

        // Create raw window handle
        let mut handle =
            Win32WindowHandle::new(std::num::NonZeroIsize::new(hwnd.0 as isize).unwrap());
        handle.hinstance = std::num::NonZeroIsize::new(
            windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
                .unwrap_or_default()
                .0 as isize,
        );

        // Create surface target
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: raw_window_handle::RawDisplayHandle::Windows(
                raw_window_handle::WindowsDisplayHandle::new(),
            ),
            raw_window_handle: RawWindowHandle::Win32(handle),
        };

        let surface = self
            .instance
            .create_surface_unsafe(target)
            .map_err(|e| CompositorError::SurfaceCreation(e.to_string()))?;

        // Configure the surface
        let surface_caps = surface.get_capabilities(&self.adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| **f == self.config.format)
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let present_mode = if self.config.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&self.device, &config);

        let state = SurfaceState {
            view_id,
            surface,
            config,
            width,
            height,
        };

        self.surfaces.write().unwrap().insert(view_id, state);

        info!(?view_id, "Surface created");
        Ok(())
    }

    /// Create a surface for a view (macOS implementation).
    ///
    /// # Safety
    ///
    /// The raw window handle must be valid and remain valid for the lifetime of the surface.
    #[cfg(target_os = "macos")]
    pub unsafe fn create_surface_for_raw_handle(
        &self,
        view_id: ViewId,
        raw_handle: raw_window_handle::RawWindowHandle,
        width: u32,
        height: u32,
    ) -> Result<(), CompositorError> {
        debug!(?view_id, width, height, "Creating surface for macOS view");

        // Create surface target
        let target = wgpu::SurfaceTargetUnsafe::RawHandle {
            raw_display_handle: raw_window_handle::RawDisplayHandle::AppKit(
                raw_window_handle::AppKitDisplayHandle::new(),
            ),
            raw_window_handle: raw_handle,
        };

        let surface = self
            .instance
            .create_surface_unsafe(target)
            .map_err(|e| CompositorError::SurfaceCreation(e.to_string()))?;

        // Configure the surface
        let surface_caps = surface.get_capabilities(&self.adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| **f == self.config.format)
            .copied()
            .unwrap_or(surface_caps.formats[0]);

        let present_mode = if self.config.vsync {
            wgpu::PresentMode::AutoVsync
        } else {
            wgpu::PresentMode::AutoNoVsync
        };

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode,
            alpha_mode: wgpu::CompositeAlphaMode::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };

        surface.configure(&self.device, &config);

        let state = SurfaceState {
            view_id,
            surface,
            config,
            width,
            height,
        };

        self.surfaces.write().unwrap().insert(view_id, state);

        info!(?view_id, width, height, ?format, "Surface created (macOS)");
        Ok(())
    }

    /// Create a headless texture for offscreen rendering (testing/headless mode).
    ///
    /// This creates an offscreen render target that doesn't require a window.
    /// Perfect for unit tests and CI environments.
    pub fn create_headless_texture(
        &self,
        view_id: ViewId,
        width: u32,
        height: u32,
    ) -> Result<(), CompositorError> {
        debug!(?view_id, width, height, "Creating headless texture");

        if width == 0 || height == 0 {
            return Err(CompositorError::SurfaceCreation(
                "Headless texture dimensions must be non-zero".into(),
            ));
        }

        // Create offscreen texture
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Headless Render Target"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let state = HeadlessState {
            texture,
            width,
            height,
        };

        self.headless_textures.write().unwrap().insert(view_id, state);

        info!(?view_id, width, height, "Headless texture created");
        Ok(())
    }

    /// Resize a surface.
    pub fn resize_surface(
        &self,
        view_id: ViewId,
        width: u32,
        height: u32,
    ) -> Result<(), CompositorError> {
        let mut surfaces = self.surfaces.write().unwrap();
        let state = surfaces
            .get_mut(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;

        state.resize(&self.device, width, height);
        Ok(())
    }

    /// Resize a surface from Bounds.
    pub fn resize_surface_from_bounds(
        &self,
        view_id: ViewId,
        bounds: Bounds,
    ) -> Result<(), CompositorError> {
        self.resize_surface(view_id, bounds.width, bounds.height)
    }

    /// Render a solid color to a surface (for testing).
    pub fn render_solid_color(
        &self,
        view_id: ViewId,
        color: [f64; 4],
    ) -> Result<(), CompositorError> {
        // Check if this is a headless texture first
        let headless = self.headless_textures.read().unwrap();
        if let Some(state) = headless.get(&view_id) {
            let view = state.texture.create_view(&wgpu::TextureViewDescriptor::default());

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Solid Color Encoder (Headless)"),
                });

            {
                let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Solid Color Pass (Headless)"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: color[0],
                                g: color[1],
                                b: color[2],
                                a: color[3],
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }

            self.queue.submit(std::iter::once(encoder.finish()));

            trace!(?view_id, "Rendered solid color to headless texture");
            return Ok(());
        }
        drop(headless);

        // Otherwise, render to regular surface
        let surfaces = self.surfaces.read().unwrap();
        let state = surfaces
            .get(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;

        let output = state.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Solid Color Encoder"),
            });

        {
            let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Solid Color Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: color[0],
                            g: color[1],
                            b: color[2],
                            a: color[3],
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        output.present();

        trace!(?view_id, "Rendered solid color");
        Ok(())
    }

    /// Destroy a surface.
    pub fn destroy_surface(&self, view_id: ViewId) -> Result<(), CompositorError> {
        let removed = self.surfaces.write().unwrap().remove(&view_id);
        if removed.is_some() {
            info!(?view_id, "Surface destroyed");
            Ok(())
        } else {
            Err(CompositorError::SurfaceNotFound(view_id))
        }
    }

    /// Destroy a headless texture.
    pub fn destroy_headless_texture(&self, view_id: ViewId) -> Result<(), CompositorError> {
        let removed = self.headless_textures.write().unwrap().remove(&view_id);
        if removed.is_some() {
            info!(?view_id, "Headless texture destroyed");
            Ok(())
        } else {
            Err(CompositorError::SurfaceNotFound(view_id))
        }
    }

    /// Get the number of active surfaces.
    pub fn surface_count(&self) -> usize {
        self.surfaces.read().unwrap().len()
    }

    /// Get the device.
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Get the device as Arc.
    pub fn device_arc(&self) -> Arc<wgpu::Device> {
        Arc::clone(&self.device)
    }

    /// Get the queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Get the queue as Arc.
    pub fn queue_arc(&self) -> Arc<wgpu::Queue> {
        Arc::clone(&self.queue)
    }

    /// Get the surface format.
    pub fn surface_format(&self) -> wgpu::TextureFormat {
        self.config.format
    }

    /// Get GPU adapter info.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.adapter.get_info()
    }

    /// Get surface texture for rendering.
    /// Returns the texture and presents it when dropped.
    pub fn get_surface_texture(
        &self,
        view_id: ViewId,
    ) -> Result<(wgpu::SurfaceTexture, wgpu::TextureView), CompositorError> {
        // Write lock: an Outdated/Lost surface must be reconfigured in place,
        // otherwise every subsequent acquire fails and the last presented
        // frame stays on screen indefinitely.
        let mut surfaces = self.surfaces.write().unwrap();
        let state = surfaces
            .get_mut(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;

        let output = match state.surface.get_current_texture() {
            Ok(output) => output,
            Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                warn!(?view_id, "Surface outdated/lost; reconfiguring and retrying acquire");
                state.surface.configure(&self.device, &state.config);
                state
                    .surface
                    .get_current_texture()
                    .map_err(|e| CompositorError::Swapchain(e.to_string()))?
            }
            Err(e) => return Err(CompositorError::Swapchain(e.to_string())),
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        Ok((output, view))
    }

    /// Get headless texture view for rendering (headless mode).
    /// Returns just the texture view - no presentation needed for headless.
    pub fn get_headless_texture_view(
        &self,
        view_id: ViewId,
    ) -> Result<wgpu::TextureView, CompositorError> {
        let headless = self.headless_textures.read().unwrap();
        let state = headless
            .get(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;

        let view = state.texture.create_view(&wgpu::TextureViewDescriptor::default());
        Ok(view)
    }

    /// Present a surface texture.
    pub fn present(&self, output: wgpu::SurfaceTexture) {
        trace!("Presenting surface texture");
        output.present();
    }

    /// Capture a frame to a PPM file.
    ///
    /// This creates a temporary render target, renders a solid color (or current state),
    /// and writes the result to a PPM file for deterministic testing.
    ///
    /// Note: This is primarily useful for testing/debugging. In production,
    /// the swapchain textures are presented directly and not readable.
    pub fn capture_frame_to_file(
        &self,
        view_id: ViewId,
        path: &str,
    ) -> Result<(), CompositorError> {
        let surfaces = self.surfaces.read().unwrap();
        let state = surfaces
            .get(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;

        let width = state.width;
        let height = state.height;

        if width == 0 || height == 0 {
            return Err(CompositorError::Render(
                "Cannot capture zero-size frame".into(),
            ));
        }

        info!(?view_id, width, height, path, "Capturing frame");

        // Create an offscreen texture for capture (COPY_SRC enabled)
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Capture Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm, // Linear format to match surface
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Render a test pattern (magenta with a small rectangle) to prove rendering works
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Capture Encoder"),
            });

        {
            let _render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Capture Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &texture_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Magenta background to prove capture works
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
                            g: 0.0,
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
        }

        // Create staging buffer for readback
        let bytes_per_pixel = 4u32; // RGBA8
        let padded_bytes_per_row = (width * bytes_per_pixel + 255) & !255; // Align to 256
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Capture Staging Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy texture to staging buffer
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map and read the buffer
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        self.device.poll(wgpu::Maintain::Wait);

        rx.recv()
            .map_err(|e| CompositorError::Render(format!("Failed to receive map result: {}", e)))?
            .map_err(|e| CompositorError::Render(format!("Failed to map buffer: {:?}", e)))?;

        let data = buffer_slice.get_mapped_range();

        // Write PPM file (simple portable format)
        let mut file = std::fs::File::create(path)
            .map_err(|e| CompositorError::Render(format!("Failed to create file: {}", e)))?;

        use std::io::Write;
        writeln!(file, "P6")
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM header: {}", e)))?;
        writeln!(file, "{} {}", width, height)
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM dimensions: {}", e)))?;
        writeln!(file, "255")
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM max value: {}", e)))?;

        // Convert RGBA to RGB and handle row padding
        let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            let row_start = (y * padded_bytes_per_row) as usize;
            for x in 0..width {
                let pixel_start = row_start + (x * bytes_per_pixel) as usize;
                // RGBA -> RGB
                rgb_data.push(data[pixel_start]); // R
                rgb_data.push(data[pixel_start + 1]); // G
                rgb_data.push(data[pixel_start + 2]); // B
            }
        }

        file.write_all(&rgb_data)
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM data: {}", e)))?;

        drop(data);
        staging_buffer.unmap();

        info!(?view_id, path, "Frame captured successfully");
        Ok(())
    }

    /// Capture a frame by rendering actual display list to an offscreen texture.
    ///
    /// This renders the provided display list commands to an offscreen texture
    /// and writes the result to a PPM file for deterministic visual testing.
    pub fn capture_frame_with_renderer(
        &self,
        view_id: ViewId,
        path: &str,
        renderer: &mut Renderer,
        commands: &[DisplayCommand],
    ) -> Result<(), CompositorError> {
        // Get dimensions from either headless texture or surface
        let (width, height) = {
            // Check headless textures first
            let headless = self.headless_textures.read().unwrap();
            if let Some(state) = headless.get(&view_id) {
                (state.width, state.height)
            } else {
                drop(headless);
                // Fall back to surfaces
                let surfaces = self.surfaces.read().unwrap();
                let state = surfaces
                    .get(&view_id)
                    .ok_or(CompositorError::SurfaceNotFound(view_id))?;
                (state.width, state.height)
            }
        };

        if width == 0 || height == 0 {
            return Err(CompositorError::Render(
                "Cannot capture zero-size frame".into(),
            ));
        }

        info!(?view_id, width, height, path, cmd_count = commands.len(), "Capturing frame with display list");

        // Create an offscreen texture for capture (RENDER_ATTACHMENT + COPY_SRC)
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Capture Texture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm, // Linear format to match surface
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });

        let texture_view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Render the display list to the offscreen texture
        renderer.execute(commands, &texture_view)
            .map_err(|e| CompositorError::Render(format!("Renderer error: {}", e)))?;

        // Create staging buffer for readback
        let bytes_per_pixel = 4u32; // BGRA8
        let padded_bytes_per_row = (width * bytes_per_pixel + 255) & !255; // Align to 256
        let buffer_size = (padded_bytes_per_row * height) as u64;

        let staging_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Capture Staging Buffer"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        // Copy texture to staging buffer
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Capture Copy Encoder"),
            });

        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &staging_buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );

        self.queue.submit(std::iter::once(encoder.finish()));

        // Map and read the buffer
        let buffer_slice = staging_buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });

        self.device.poll(wgpu::Maintain::Wait);

        rx.recv()
            .map_err(|e| CompositorError::Render(format!("Failed to receive map result: {}", e)))?
            .map_err(|e| CompositorError::Render(format!("Failed to map buffer: {:?}", e)))?;

        let data = buffer_slice.get_mapped_range();

        // Write PPM file (simple portable format)
        let mut file = std::fs::File::create(path)
            .map_err(|e| CompositorError::Render(format!("Failed to create file: {}", e)))?;

        use std::io::Write;
        writeln!(file, "P6")
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM header: {}", e)))?;
        writeln!(file, "{} {}", width, height)
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM dimensions: {}", e)))?;
        writeln!(file, "255")
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM max value: {}", e)))?;

        // Convert BGRA to RGB and handle row padding
        let mut rgb_data = Vec::with_capacity((width * height * 3) as usize);
        for y in 0..height {
            let row_start = (y * padded_bytes_per_row) as usize;
            for x in 0..width {
                let pixel_start = row_start + (x * bytes_per_pixel) as usize;
                // BGRA -> RGB
                rgb_data.push(data[pixel_start + 2]); // R (from B position in BGRA)
                rgb_data.push(data[pixel_start + 1]); // G
                rgb_data.push(data[pixel_start]);     // B (from R position in BGRA)
            }
        }

        file.write_all(&rgb_data)
            .map_err(|e| CompositorError::Render(format!("Failed to write PPM data: {}", e)))?;

        drop(data);
        staging_buffer.unmap();

        info!(?view_id, path, "Frame captured with display list successfully");
        Ok(())
    }

    /// Get the surface dimensions for a view (supports both surfaces and headless textures).
    pub fn get_surface_size(&self, view_id: ViewId) -> Result<(u32, u32), CompositorError> {
        // Check headless textures first
        let headless = self.headless_textures.read().unwrap();
        if let Some(state) = headless.get(&view_id) {
            return Ok((state.width, state.height));
        }
        drop(headless);

        // Fall back to surfaces
        let surfaces = self.surfaces.read().unwrap();
        let state = surfaces
            .get(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;
        Ok((state.width, state.height))
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        // Clear all surfaces
        self.surfaces.write().unwrap().clear();
        info!("Compositor dropped");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compositor_config_default() {
        let config = CompositorConfig::default();
        assert!(config.vsync);
        assert_eq!(config.format, wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(config.power_preference, wgpu::PowerPreference::HighPerformance);
    }

    #[test]
    fn test_compositor_config_custom() {
        let config = CompositorConfig {
            vsync: false,
            format: wgpu::TextureFormat::Rgba8Unorm,
            power_preference: wgpu::PowerPreference::LowPower,
        };
        assert!(!config.vsync);
        assert_eq!(config.format, wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(config.power_preference, wgpu::PowerPreference::LowPower);
    }

    #[test]
    fn test_compositor_creation() {
        // Test that compositor can be created with default config
        let result = Compositor::new();
        if result.is_err() {
            // Skip test if no GPU available (CI environments)
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();
        assert_eq!(compositor.surface_format(), wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(compositor.surface_count(), 0);
    }

    #[test]
    fn test_compositor_with_custom_config() {
        let config = CompositorConfig {
            vsync: false,
            format: wgpu::TextureFormat::Bgra8Unorm,
            power_preference: wgpu::PowerPreference::LowPower,
        };
        let result = Compositor::with_config(config.clone());
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();
        assert_eq!(compositor.surface_format(), config.format);
    }

    #[test]
    fn test_headless_texture_lifecycle() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        // Create headless texture
        let view_id = ViewId::new();
        compositor
            .create_headless_texture(view_id, 800, 600)
            .expect("Failed to create headless texture");

        // Verify texture exists and has correct size
        let size = compositor
            .get_surface_size(view_id)
            .expect("Failed to get surface size");
        assert_eq!(size, (800, 600));

        // Destroy texture
        compositor
            .destroy_headless_texture(view_id)
            .expect("Failed to destroy headless texture");

        // Verify texture is gone
        assert!(compositor.get_surface_size(view_id).is_err());
    }

    #[test]
    fn test_headless_texture_recreate_different_size() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();
        compositor
            .create_headless_texture(view_id, 800, 600)
            .expect("Failed to create headless texture");

        // To "resize" a headless texture, destroy and recreate it
        compositor
            .destroy_headless_texture(view_id)
            .expect("Failed to destroy");

        compositor
            .create_headless_texture(view_id, 1024, 768)
            .expect("Failed to recreate with new size");

        let size = compositor
            .get_surface_size(view_id)
            .expect("Failed to get surface size");
        assert_eq!(size, (1024, 768));

        compositor
            .destroy_headless_texture(view_id)
            .expect("Failed to destroy");
    }

    #[test]
    fn test_headless_texture_zero_size() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();

        // Creating zero-size texture should fail
        let result = compositor.create_headless_texture(view_id, 0, 0);

        // Zero-size textures are invalid, should error
        assert!(result.is_err(), "Zero-size texture should fail");
    }

    #[test]
    fn test_multiple_headless_textures() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        // Create multiple headless textures
        let view1 = ViewId::new();
        let view2 = ViewId::new();
        let view3 = ViewId::new();

        compositor
            .create_headless_texture(view1, 800, 600)
            .expect("Failed to create texture 1");
        compositor
            .create_headless_texture(view2, 1024, 768)
            .expect("Failed to create texture 2");
        compositor
            .create_headless_texture(view3, 640, 480)
            .expect("Failed to create texture 3");

        // Verify all sizes
        assert_eq!(compositor.get_surface_size(view1).unwrap(), (800, 600));
        assert_eq!(compositor.get_surface_size(view2).unwrap(), (1024, 768));
        assert_eq!(compositor.get_surface_size(view3).unwrap(), (640, 480));

        // Clean up
        compositor.destroy_headless_texture(view1).expect("Failed to destroy 1");
        compositor.destroy_headless_texture(view2).expect("Failed to destroy 2");
        compositor.destroy_headless_texture(view3).expect("Failed to destroy 3");
    }

    #[test]
    fn test_destroy_nonexistent_texture() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();

        // Destroying non-existent texture should error
        let result = compositor.destroy_headless_texture(view_id);
        assert!(result.is_err(), "Destroying non-existent texture should fail");
    }

    #[test]
    fn test_double_destroy() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();
        compositor
            .create_headless_texture(view_id, 800, 600)
            .expect("Failed to create texture");

        // First destroy should succeed
        compositor
            .destroy_headless_texture(view_id)
            .expect("First destroy should succeed");

        // Second destroy should fail
        let result = compositor.destroy_headless_texture(view_id);
        assert!(result.is_err(), "Second destroy should fail");
    }

    #[test]
    fn test_get_headless_texture_view() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();
        compositor
            .create_headless_texture(view_id, 800, 600)
            .expect("Failed to create texture");

        // Get texture view - should succeed and return a TextureView
        let _view = compositor
            .get_headless_texture_view(view_id)
            .expect("Failed to get texture view");

        // Getting texture view for non-existent surface should fail
        let bad_view_id = ViewId::new();
        assert!(compositor.get_headless_texture_view(bad_view_id).is_err());

        compositor.destroy_headless_texture(view_id).expect("Failed to destroy");
    }

    #[test]
    fn test_render_solid_color_headless() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();
        compositor
            .create_headless_texture(view_id, 100, 100)
            .expect("Failed to create texture");

        // Render solid red color
        compositor
            .render_solid_color(view_id, [1.0, 0.0, 0.0, 1.0])
            .expect("Failed to render solid color");

        compositor.destroy_headless_texture(view_id).expect("Failed to destroy");
    }

    #[test]
    fn test_adapter_info() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let info = compositor.adapter_info();
        // Verify we got adapter info (actual values depend on GPU)
        assert!(!info.name.is_empty(), "Adapter name should not be empty");
    }

    #[test]
    fn test_device_and_queue_access() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        // Test device access
        let _device = compositor.device();
        let device_arc = compositor.device_arc();
        assert!(Arc::strong_count(&device_arc) >= 1);

        // Test queue access
        let _queue = compositor.queue();
        let queue_arc = compositor.queue_arc();
        assert!(Arc::strong_count(&queue_arc) >= 1);
    }

    #[test]
    fn test_headless_texture_various_sizes() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        // Test creating textures at various sizes
        let view1 = ViewId::new();
        let view2 = ViewId::new();
        let view3 = ViewId::new();

        // Small size (1x1)
        compositor
            .create_headless_texture(view1, 1, 1)
            .expect("Failed to create 1x1 texture");
        assert_eq!(compositor.get_surface_size(view1).unwrap(), (1, 1));

        // Standard size (800x600)
        compositor
            .create_headless_texture(view2, 800, 600)
            .expect("Failed to create 800x600 texture");
        assert_eq!(compositor.get_surface_size(view2).unwrap(), (800, 600));

        // Large size (4K: 3840x2160)
        compositor
            .create_headless_texture(view3, 3840, 2160)
            .expect("Failed to create 4K texture");
        assert_eq!(compositor.get_surface_size(view3).unwrap(), (3840, 2160));

        // Clean up
        compositor.destroy_headless_texture(view1).expect("Failed to destroy");
        compositor.destroy_headless_texture(view2).expect("Failed to destroy");
        compositor.destroy_headless_texture(view3).expect("Failed to destroy");
    }

    #[test]
    fn test_resize_surface_error_for_headless() {
        let result = Compositor::new();
        if result.is_err() {
            println!("Skipping test: No GPU available");
            return;
        }
        let compositor = result.unwrap();

        let view_id = ViewId::new();
        compositor
            .create_headless_texture(view_id, 800, 600)
            .expect("Failed to create texture");

        // resize_surface_from_bounds should fail for headless textures
        // (they don't have surfaces, only textures)
        let result = compositor.resize_surface_from_bounds(view_id, Bounds::new(0, 0, 1024, 768));
        assert!(result.is_err(), "Resize should fail for headless textures");

        compositor.destroy_headless_texture(view_id).expect("Failed to destroy");
    }

    // Note: Full surface tests (non-headless) require a display and are typically
    // run manually or in integration test environments with window access.
    // The tests above cover the headless path which shares most of the compositor logic.
}
