//! WebView abstraction for HiWave
//!
//! This module provides an abstraction layer over different WebView implementations,
//! allowing HiWave to use either WRY (WebView2 on Windows) or RustKit (pure Rust engine).
//!
//! # Architecture
//!
//! The `IWebView` trait defines the common interface that all WebView implementations
//! must provide. This includes operations like:
//! - Loading URLs and HTML content
//! - Evaluating JavaScript
//! - Setting bounds/position
//! - Handling zoom
//!
//! The actual implementation is selected at compile time via feature flags:
//! - Default: Uses WRY (WebView2 on Windows)
//! - `rustkit` feature: Uses RustKit engine (pure Rust)

use wry::dpi::{LogicalPosition, LogicalSize};
use wry::Rect;

// Re-export WRY types for convenience
pub use wry::WebView;

// ============================================================================
// HiWaveWebView - Unified type alias for WebView backends
// ============================================================================

/// Unified WebView type that works with all backends.
///
/// Note: This type is used for Chrome, Shelf, Settings, and other UI windows.
/// It always uses WRY/WebView2 for stability. The RustKit engine is only used
/// for the Content WebView when the `rustkit` feature is enabled.
///
/// - Default build: Uses `wry::WebView` (WebView2 on Windows)
/// - `rustkit` feature: Uses `RustKitView` for content (via separate API)
///
/// All types implement the `IWebView` trait for common operations.
pub type HiWaveWebView = wry::WebView;

// ============================================================================
// Helper functions
// ============================================================================

// These helper functions are part of the public API for external use
#[allow(dead_code)]
/// Convert logical position/size to WRY Rect
pub fn make_rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
    Rect {
        position: LogicalPosition::new(x, y).into(),
        size: LogicalSize::new(width, height).into(),
    }
}

#[allow(dead_code)]
/// Helper to safely evaluate a script on a WebView
/// Returns Ok(()) even if the script execution fails (fire-and-forget pattern)
pub fn eval_script(webview: &WebView, script: &str) {
    let _ = webview.evaluate_script(script);
}

#[allow(dead_code)]
/// Helper to safely set bounds on a WebView
pub fn set_webview_bounds(webview: &WebView, x: f64, y: f64, width: f64, height: f64) {
    let _ = webview.set_bounds(make_rect(x, y, width, height));
}

// ============================================================================
// IWebView Trait - Common interface for all WebView implementations
// ============================================================================

/// Common interface for WebView implementations
///
/// This trait abstracts over the differences between WRY (WebView2) and
/// RustKit, allowing main.rs to work with either backend.
///
/// Note: This trait is NOT Send because WebView handles are not thread-safe.
/// All WebView operations must be performed on the main/UI thread.
#[allow(dead_code)]
pub trait IWebView {
    /// Load a URL in the WebView
    fn load_url(&self, url: &str);

    /// Load HTML content directly
    fn load_html(&self, html: &str);

    /// Evaluate JavaScript in the WebView (fire-and-forget)
    fn evaluate_script(&self, script: &str);

    /// Set the bounds/position of the WebView
    fn set_bounds(&self, rect: Rect);

    /// Get the current URL (if available)
    fn url(&self) -> Option<String>;

    /// Set zoom level (1.0 = 100%)
    fn set_zoom(&self, level: f64);

    /// Print the current page
    fn print(&self);

    /// Focus the WebView
    fn focus(&self);

    /// Clear all browsing data
    fn clear_all_browsing_data(&self);

    /// Set the visibility of the WebView
    fn set_visible(&self, visible: bool);
}

// ============================================================================
// Arc wrapper implementation - allows Arc<WebView> to be used as IWebView
// ============================================================================

use std::sync::Arc;

impl<T: IWebView> IWebView for Arc<T> {
    fn load_url(&self, url: &str) {
        (**self).load_url(url)
    }

    fn load_html(&self, html: &str) {
        (**self).load_html(html)
    }

    fn evaluate_script(&self, script: &str) {
        (**self).evaluate_script(script)
    }

    fn set_bounds(&self, rect: Rect) {
        (**self).set_bounds(rect)
    }

    fn url(&self) -> Option<String> {
        (**self).url()
    }

    fn set_zoom(&self, level: f64) {
        (**self).set_zoom(level)
    }

    fn print(&self) {
        (**self).print()
    }

    fn focus(&self) {
        (**self).focus()
    }

    fn clear_all_browsing_data(&self) {
        (**self).clear_all_browsing_data()
    }

    fn set_visible(&self, visible: bool) {
        (**self).set_visible(visible)
    }
}

// ============================================================================
// WRY WebView implementation of IWebView
// ============================================================================

impl IWebView for wry::WebView {
    fn load_url(&self, url: &str) {
        let _ = self.load_url(url);
    }

    fn load_html(&self, html: &str) {
        let _ = self.load_html(html);
    }

    fn evaluate_script(&self, script: &str) {
        let _ = wry::WebView::evaluate_script(self, script);
    }

    fn set_bounds(&self, rect: Rect) {
        let _ = wry::WebView::set_bounds(self, rect);
    }

    fn url(&self) -> Option<String> {
        wry::WebView::url(self).ok()
    }

    fn set_zoom(&self, level: f64) {
        let _ = wry::WebView::zoom(self, level);
    }

    fn print(&self) {
        let _ = wry::WebView::print(self);
    }

    fn focus(&self) {
        let _ = wry::WebView::focus(self);
    }

    fn clear_all_browsing_data(&self) {
        let _ = wry::WebView::clear_all_browsing_data(self);
    }

    fn set_visible(&self, visible: bool) {
        let _ = wry::WebView::set_visible(self, visible);
    }
}

// ============================================================================
// Engine detection
// ============================================================================

/// Whether the RUSTKIT CRATES are linked into this build.
///
/// NOTE THE NARROWNESS, IT IS DELIBERATE. This says the engine is COMPILED IN.
/// It does NOT say the engine paints anything. In hybrid mode it does not:
/// `main.rs` builds the content webview unconditionally with
/// `WebViewBuilder::new()` and there is no `cfg(feature = "rustkit")` arm at
/// that site, so WebView2 paints all web content regardless of this flag.
///
/// Renamed from `is_rustkit_enabled`, which read as "RustKit is doing the
/// work" and was false on every default build.
#[allow(dead_code)]
pub fn rustkit_crates_linked() -> bool {
    cfg!(all(target_os = "windows", feature = "rustkit"))
}

/// The engine that ACTUALLY PAINTS WEB CONTENT in this build.
///
/// THIS FUNCTION USED TO LIE. It returned "RustKit" for
/// `cfg(all(windows, feature = "rustkit"))` — and `rustkit` is the DEFAULT
/// feature — so a stock `cargo run` printed "WebView engine: RustKit" at
/// startup while WebView2 painted every page. Measured: zero
/// `cfg(feature = "rustkit")` arms exist in main.rs (positive control: the
/// same pattern matches `native-win32` three times in that file), so enabling
/// the feature links crates and changes nothing about what renders.
///
/// This file is compiled ONLY into the hybrid (wry/tao) shell — `main.rs`
/// takes an early return into `native::run_native()` under
/// `feature = "native-win32"` before reaching any caller here. So within this
/// module the honest answer is unconditional: the hybrid shell paints content
/// with WebView2. The native shell prints its own banner.
///
/// The name is what a user reads to know what is rendering their web pages.
/// It reports the painter, not the dependency graph.
pub fn engine_name() -> &'static str {
    "WebView2 (WRY)"
}
