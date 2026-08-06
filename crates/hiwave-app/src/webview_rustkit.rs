//! RustKit WebView adapter for HiWave — **NOT WIRED. NOTHING CALLS THIS.**
//!
//! # STATUS: COMPILED DEAD CODE
//!
//! Measured 2026-08-05 on develop: `RustKitView` is never constructed outside
//! this file, and `create_rustkit_view` has ZERO callers. An orphan sweep over
//! every `mod` in this crate found exactly one module with no external
//! reference — this one:
//!
//! ```text
//! ipc 65 · import 17 · state 13 · platform 5 · webview 2 · native 1
//! shield_adapter 1 · webview_rustkit 0   <-- orphan
//! ```
//!
//! `main.rs` builds the content webview unconditionally with
//! `WebViewBuilder::new()`. There is no `cfg(feature = "rustkit")` arm at that
//! site, so enabling the `rustkit` feature compiles this file and changes
//! nothing about what paints. It type-checks on every build and renders
//! nothing — which is precisely how a decorative path survives review.
//!
//! # WHY IT IS KEPT RATHER THAN DELETED
//!
//! It is not junk: it is the UNFINISHED HALF of wiring RustKit into the hybrid
//! shell. `IWebView for RustKitView` is the adapter that a content-site
//! `cfg(feature = "rustkit")` arm would select. Deleting it would mean writing
//! it again.
//!
//! So it stays, LABELLED, until it is either wired or replaced. What was not
//! acceptable was leaving it undocumented, where a reader could reasonably
//! conclude from `mod webview_rustkit;` that RustKit renders content here.
//!
//! # WHERE RUSTKIT ACTUALLY PAINTS TODAY
//!
//! `--features native-win32` → `native::win32::NativeBrowser`, which owns a
//! `rustkit_engine::Engine` and a `ViewHost` and paints via the compositor.
//! Verified by run on 2026-08-05: fetched, laid out and GPU-painted
//! `https://example.com` with no wry in the path.
//!
//! # Thread Safety
//!
//! RustKit views are NOT thread-safe. All operations must be performed on the
//! main UI thread. This is consistent with WebView2 and WinCairo WebKit behavior.

use super::webview::IWebView;
use hiwave_core::HiWaveResult;
use std::cell::RefCell;
use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use tracing::{debug, error, info, warn};
use windows::Win32::Foundation::HWND;
use wry::Rect;

// Re-export types for convenience
pub use rustkit_engine::{Engine, EngineBuilder, EngineViewId};
pub use rustkit_viewhost::Bounds;

use super::shield_adapter::create_shield_interceptor_with_counter;

// ============================================================================
// RustKitView - Single-threaded wrapper around engine view
// ============================================================================

/// A RustKit-based WebView that implements IWebView.
///
/// # Thread Safety
///
/// This type is NOT Send or Sync. All operations must be performed on the main thread.
pub struct RustKitView {
    /// The engine managing this view.
    /// Each view currently has its own engine for thread-safety.
    engine: RefCell<Engine>,
    /// The view ID within the engine.
    view_id: EngineViewId,
    /// Current URL (cached).
    current_url: RefCell<Option<String>>,
    /// Current zoom level.
    zoom_level: RefCell<f64>,
    /// Visibility state.
    visible: RefCell<bool>,
}

impl RustKitView {
    /// Create a new RustKit view.
    pub fn new(parent: HWND, bounds: Bounds) -> HiWaveResult<Self> {
        Self::with_shield_counter(parent, bounds, None)
    }

    /// Create a new RustKit view with shield integration.
    ///
    /// The `blocked_counter` is an atomic counter that will be incremented
    /// for each blocked request, allowing the main thread to track stats.
    pub fn with_shield_counter(
        parent: HWND,
        bounds: Bounds,
        blocked_counter: Option<Arc<AtomicU64>>,
    ) -> HiWaveResult<Self> {
        info!("Creating RustKit view");

        // Create engine builder
        let mut builder = EngineBuilder::new()
            .user_agent("HiWave/1.0 RustKit/1.0")
            .javascript_enabled(true)
            .cookies_enabled(true);

        // Add shield interceptor if provided
        if let Some(counter) = blocked_counter {
            info!("RustKit view with shield integration");
            let interceptor = create_shield_interceptor_with_counter(counter);
            builder = builder.request_interceptor(interceptor);
        }

        let mut engine = builder
            .build()
            .map_err(|e| hiwave_core::HiWaveError::WebView(e.to_string()))?;

        let view_id = engine
            .create_view(parent, bounds)
            .map_err(|e| hiwave_core::HiWaveError::WebView(e.to_string()))?;

        info!(?view_id, "RustKit view created");

        Ok(Self {
            engine: RefCell::new(engine),
            view_id,
            current_url: RefCell::new(None),
            zoom_level: RefCell::new(1.0),
            visible: RefCell::new(true),
        })
    }

    /// Get the view ID.
    pub fn view_id(&self) -> EngineViewId {
        self.view_id
    }

    /// Set bounds directly using RustKit Bounds.
    pub fn set_rustkit_bounds(&self, bounds: Bounds) {
        if let Err(e) = self.engine.borrow_mut().resize_view(self.view_id, bounds) {
            warn!(error = %e, "Failed to resize RustKit view");
        }
    }

    /// Load a URL using a blocking runtime.
    fn load_url_blocking(&self, url: &str) {
        let parsed = match url::Url::parse(url) {
            Ok(u) => u,
            Err(e) => {
                error!(error = %e, url = url, "Invalid URL");
                return;
            }
        };

        // Create a single-threaded tokio runtime for this operation
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                error!(error = %e, "Failed to create runtime");
                return;
            }
        };

        let mut engine = self.engine.borrow_mut();
        rt.block_on(async {
            if let Err(e) = engine.load_url(self.view_id, parsed).await {
                error!(error = %e, "Failed to load URL");
            }
        });
    }

    /// Execute JavaScript synchronously.
    pub fn execute_script_sync(&self, script: &str) -> Option<String> {
        match self
            .engine
            .borrow_mut()
            .execute_script(self.view_id, script)
        {
            Ok(result) => Some(result),
            Err(e) => {
                debug!(error = %e, "Script execution failed");
                None
            }
        }
    }
}

impl Drop for RustKitView {
    fn drop(&mut self) {
        if let Err(e) = self.engine.borrow_mut().destroy_view(self.view_id) {
            warn!(error = %e, "Failed to destroy RustKit view");
        }
    }
}

// ============================================================================
// IWebView Implementation
// ============================================================================

impl IWebView for RustKitView {
    fn load_url(&self, url: &str) {
        // Update cached URL
        *self.current_url.borrow_mut() = Some(url.to_string());
        self.load_url_blocking(url);
    }

    fn load_html(&self, html: &str) {
        // Use the engine's direct HTML loading (no data URL encoding needed)
        let mut engine = self.engine.borrow_mut();
        if let Err(e) = engine.load_html(self.view_id, html) {
            error!(error = %e, "Failed to load HTML content");
        }
    }

    fn evaluate_script(&self, script: &str) {
        let _ = self.execute_script_sync(script);
    }

    fn set_bounds(&self, rect: Rect) {
        // Convert WRY Rect to RustKit Bounds
        let (x, y) = match rect.position {
            wry::dpi::Position::Logical(pos) => (pos.x as i32, pos.y as i32),
            wry::dpi::Position::Physical(pos) => (pos.x, pos.y),
        };
        let (width, height) = match rect.size {
            wry::dpi::Size::Logical(size) => (size.width as u32, size.height as u32),
            wry::dpi::Size::Physical(size) => (size.width, size.height),
        };

        self.set_rustkit_bounds(Bounds::new(x, y, width, height));
    }

    fn url(&self) -> Option<String> {
        // Return cached URL, or query engine
        if let Some(url) = self.current_url.borrow().clone() {
            return Some(url);
        }

        self.engine
            .borrow()
            .get_url(self.view_id)
            .map(|u| u.to_string())
    }

    fn set_zoom(&self, level: f64) {
        *self.zoom_level.borrow_mut() = level;
        // RustKit doesn't have zoom yet - would need to implement
        debug!(level, "Zoom level set (not yet implemented in RustKit)");
    }

    fn print(&self) {
        // Print not yet implemented in RustKit
        warn!("Print not yet implemented in RustKit");
    }

    fn focus(&self) {
        if let Err(e) = self.engine.borrow().focus_view(self.view_id) {
            warn!(error = %e, "Failed to focus view");
        }
    }

    fn clear_all_browsing_data(&self) {
        // Would need to implement in RustKit
        debug!("Clear browsing data requested");
    }

    fn set_visible(&self, visible: bool) {
        *self.visible.borrow_mut() = visible;
        if let Err(e) = self.engine.borrow().set_view_visible(self.view_id, visible) {
            warn!(error = %e, "Failed to set view visibility");
        }
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Create a RustKit view from a raw HWND.
pub fn create_rustkit_view(
    parent: windows::Win32::Foundation::HWND,
    bounds: Bounds,
) -> HiWaveResult<RustKitView> {
    RustKitView::new(parent, bounds)
}

/// Create a RustKit view with shield integration via a shared counter.
pub fn create_rustkit_view_with_shield(
    parent: windows::Win32::Foundation::HWND,
    bounds: Bounds,
    blocked_counter: Arc<AtomicU64>,
) -> HiWaveResult<RustKitView> {
    RustKitView::with_shield_counter(parent, bounds, Some(blocked_counter))
}

/// Helper to convert WRY Rect to RustKit Bounds.
pub fn rect_to_bounds(rect: &Rect) -> Bounds {
    let (x, y) = match rect.position {
        wry::dpi::Position::Logical(pos) => (pos.x as i32, pos.y as i32),
        wry::dpi::Position::Physical(pos) => (pos.x, pos.y),
    };
    let (width, height) = match rect.size {
        wry::dpi::Size::Logical(size) => (size.width as u32, size.height as u32),
        wry::dpi::Size::Physical(size) => (size.width, size.height),
    };
    Bounds::new(x, y, width, height)
}

/// Helper to create bounds from coordinates.
pub fn make_bounds(x: i32, y: i32, width: u32, height: u32) -> Bounds {
    Bounds::new(x, y, width, height)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use wry::dpi::{LogicalPosition, LogicalSize};

    #[test]
    fn test_rect_to_bounds() {
        let rect = Rect {
            position: LogicalPosition::new(10.0, 20.0).into(),
            size: LogicalSize::new(800.0, 600.0).into(),
        };

        let bounds = rect_to_bounds(&rect);
        assert_eq!(bounds.x, 10);
        assert_eq!(bounds.y, 20);
        assert_eq!(bounds.width, 800);
        assert_eq!(bounds.height, 600);
    }

    #[test]
    fn test_make_bounds() {
        let bounds = make_bounds(0, 0, 1920, 1080);
        assert_eq!(bounds.x, 0);
        assert_eq!(bounds.y, 0);
        assert_eq!(bounds.width, 1920);
        assert_eq!(bounds.height, 1080);
    }
}
