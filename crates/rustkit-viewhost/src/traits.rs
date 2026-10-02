//! Platform-agnostic ViewHost trait for RustKit
//!
//! This trait abstracts over platform-specific window/view management,
//! allowing the engine to work on both Windows and macOS.

use crate::{Bounds, ViewHostError, ViewId};

/// Platform-agnostic window handle type
#[cfg(target_os = "windows")]
pub type WindowHandle = windows::Win32::Foundation::HWND;

#[cfg(target_os = "macos")]
pub type WindowHandle = raw_window_handle::RawWindowHandle;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub type WindowHandle = ();

/// Platform-agnostic trait for view hosting operations
///
/// This trait abstracts over the differences between Windows (HWND) and
/// macOS (NSView) window management, allowing the engine to work on both platforms.
pub trait ViewHostTrait: Send + Sync {
    /// Create a new view under the given parent window
    fn create_view(
        &self,
        parent: WindowHandle,
        bounds: Bounds,
    ) -> Result<ViewId, ViewHostError>;

    /// Resize a view to new bounds
    fn resize_view(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError>;

    /// Destroy a view
    fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError>;

    /// Get the window handle for a view (platform-specific)
    #[cfg(target_os = "windows")]
    fn get_hwnd(&self, view_id: ViewId) -> Result<windows::Win32::Foundation::HWND, ViewHostError>;

    /// Get the window handle for a view (macOS)
    #[cfg(target_os = "macos")]
    fn get_raw_window_handle(&self, view_id: ViewId) -> Result<raw_window_handle::RawWindowHandle, ViewHostError>;

    /// Set view visibility
    fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError>;

    /// Focus a view
    fn focus_view(&self, view_id: ViewId) -> Result<(), ViewHostError>;

    /// Pump platform message loop (returns false on quit)
    fn pump_messages(&self) -> bool;

    /// Get view bounds
    fn get_bounds(&self, view_id: ViewId) -> Result<Bounds, ViewHostError>;

    /// Get DPI for a view
    fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError>;
}

