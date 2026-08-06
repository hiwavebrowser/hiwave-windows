//! Native platform entry points for HiWave.
//!
//! This module provides platform-native browser implementations that do not
//! depend on wry, tao, or any WebView abstraction layers. Each platform
//! implementation uses only platform-native APIs:
//!
//! - **Windows**: Win32 + RustKit (via `win32.rs`) - IMPLEMENTED
//! - **macOS**: Cocoa + RustKit (via `macos.rs`) - STUB
//! - **Linux**: X11/Wayland + RustKit (via `linux.rs`) - STUB

#[cfg(target_os = "windows")]
mod tabs;
#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
pub mod screenshot_harness;

#[cfg(target_os = "windows")]
pub use win32::run_native;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "macos")]
pub use macos::run_native;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::run_native;

