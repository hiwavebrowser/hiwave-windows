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
use tracing::{debug, error, info, trace};

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

        // Create wgpu instance
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::DX12 | wgpu::Backends::VULKAN,
            ..Default::default()
        });

        // Request adapter
        let adapter = pollster::block_on(async {
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: config.power_preference,
                    compatible_surface: None,
                    force_fallback_adapter: false,
                })
                .await
        })
        .ok_or_else(|| CompositorError::DeviceCreation("No suitable GPU adapter found".into()))?;

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
        let surfaces = self.surfaces.read().unwrap();
        let state = surfaces
            .get(&view_id)
            .ok_or(CompositorError::SurfaceNotFound(view_id))?;

        let output = state.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        Ok((output, view))
    }

    /// Present a surface texture.
    pub fn present(&self, output: wgpu::SurfaceTexture) {
        output.present();
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
    }

    // Note: GPU tests require a display and are typically run manually
    // or in integration test environments with GPU access.
}
