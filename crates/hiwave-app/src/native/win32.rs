//! Native Win32 Entry Point for HiWave
//!
//! This module provides a pure Win32 entry point for HiWave that uses RustKit
//! for all rendering, completely bypassing wry/tao.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────┐
//! │  Win32 Main Window (ViewHost)           │
//! ├─────────────────────────────────────────┤
//! │  RustKit Chrome View (tabs, toolbar)    │
//! ├─────────────────────────────────────────┤
//! │  RustKit Content View (web pages)       │
//! ├─────────────────────────────────────────┤
//! │  RustKit Shelf View (command palette)   │
//! └─────────────────────────────────────────┘
//! ```
//!
//! # Feature Flag
//!
//! This module is only compiled when the `native-win32` feature is enabled:
//!
//! ```bash
//! cargo build --features native-win32
//! ```

use super::tabs::{TabModel, TabSwitch};
use rustkit_engine::{Engine, EngineBuilder, EngineViewId, IpcMessage};
use rustkit_viewhost::{Bounds, MainWindowConfig, ViewEvent, ViewHost};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{debug, error, info, trace, warn};
use windows::Win32::Foundation::HWND;

use crate::state::AppState;

// Chrome UI constants
const CHROME_HEIGHT_DEFAULT: u32 = 104;
const CHROME_HEIGHT_EXPANDED: u32 = 460;
const SHELF_HEIGHT_DEFAULT: u32 = 0;
const SHELF_HEIGHT_EXPANDED: u32 = 280;
const SIDEBAR_WIDTH: u32 = 220;

/// HTML content embedded at compile time
const CHROME_HTML: &str = include_str!("../ui/chrome.html");
const SHELF_HTML: &str = include_str!("../ui/shelf.html");
const ABOUT_HTML: &str = include_str!("../ui/about.html");

/// View types in the browser
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewType {
    Chrome,
    Content,
    Shelf,
}

/// State for the native Win32 browser
pub struct NativeBrowser {
    /// ViewHost manages all Win32 windows
    viewhost: ViewHost,
    /// RustKit engine for rendering
    engine: RefCell<Engine>,
    /// Map of view types to engine view IDs
    views: HashMap<ViewType, EngineViewId>,
    /// Reverse map: engine view ID to view type (for IPC routing)
    engine_view_types: HashMap<EngineViewId, ViewType>,
    /// Tab strip. `None` until the first content view exists — the model
    /// cannot represent a zero-tab window, so it is not constructed until
    /// there is a real view to put in it (see `native/tabs.rs`).
    tabs: Option<TabModel<EngineViewId>>,
    /// Application state
    #[allow(dead_code)]
    app_state: Arc<AppState>,
    /// Current window dimensions
    window_width: u32,
    window_height: u32,
    /// Layout state
    chrome_height: u32,
    shelf_height: u32,
    sidebar_width: u32,
    sidebar_open: bool,
}

impl NativeBrowser {
    /// Create a new native browser instance.
    pub fn new() -> Result<Self, String> {
        info!("Initializing native Win32 browser");

        // Create the engine
        let engine = EngineBuilder::new()
            .user_agent(
                // Finding 4 from the macOS live demo, ported: the bare
                // "HiWave/1.0 RustKit/1.0" UA is scraper-shaped — Wikimedia
                // 429'd it and eBay challenged it. Safari-shaped with an
                // HONEST Windows platform string plus the HiWave token: sites
                // get a recognisable engine family, and we stay identifiable
                // rather than impersonating Chrome outright.
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/605.1.15                  (KHTML, like Gecko) Version/17.4 Safari/605.1.15 HiWave/1.0",
            )
            .javascript_enabled(true)
            .cookies_enabled(true)
            .build()
            .map_err(|e| format!("Failed to create engine: {}", e))?;

        // Initialize app state
        let app_state = Arc::new(
            AppState::with_defaults()
                .map_err(|e| format!("Failed to initialize app state: {}", e))?,
        );

        Ok(Self {
            viewhost: ViewHost::new(),
            engine: RefCell::new(engine),
            views: HashMap::new(),
            engine_view_types: HashMap::new(),
            tabs: None,
            app_state,
            window_width: 1280,
            window_height: 800,
            chrome_height: CHROME_HEIGHT_DEFAULT,
            shelf_height: SHELF_HEIGHT_DEFAULT,
            sidebar_width: SIDEBAR_WIDTH,
            sidebar_open: true,
        })
    }

    /// Initialize the browser window and views.
    pub fn init(&mut self) -> Result<(), String> {
        // Create the main window
        let config = MainWindowConfig::new("HiWave").with_size(1280, 800);
        let hwnd = self
            .viewhost
            .create_main_window(config)
            .map_err(|e| format!("Failed to create window: {}", e))?;

        // Set up event callback
        let _callback = Arc::new(move |event: ViewEvent| {
            // Events are handled in the message loop
            debug!(?event, "View event received");
        });

        // Create the three views
        self.create_views(hwnd)?;

        // Load initial content
        self.load_initial_content()?;

        info!("Native browser initialized");
        Ok(())
    }

    /// Create Chrome, Content, and Shelf views.
    fn create_views(&mut self, parent: HWND) -> Result<(), String> {
        let mut engine = self.engine.borrow_mut();

        // Calculate initial bounds
        // Chrome view covers the sidebar area (left column, full height)
        // The chrome.html contains the sidebar UI
        let chrome_bounds = Bounds::new(0, 0, self.sidebar_width, self.window_height);
        let content_bounds = self.calculate_content_bounds();
        let shelf_bounds = self.calculate_shelf_bounds();

        // Create Chrome view (tabs, address bar, toolbar)
        let chrome_id = engine
            .create_view(parent, chrome_bounds)
            .map_err(|e| format!("Failed to create Chrome view: {}", e))?;
        self.views.insert(ViewType::Chrome, chrome_id);
        self.engine_view_types.insert(chrome_id, ViewType::Chrome);
        debug!(?chrome_id, "Chrome view created");

        // Create Content view (web page rendering)
        let content_id = engine
            .create_view(parent, content_bounds)
            .map_err(|e| format!("Failed to create Content view: {}", e))?;
        self.views.insert(ViewType::Content, content_id);
        self.engine_view_types.insert(content_id, ViewType::Content);
        self.tabs = Some(TabModel::new(content_id));
        debug!(?content_id, "Content view created");

        // Create Shelf view (command palette, hidden by default)
        let shelf_id = engine
            .create_view(parent, shelf_bounds)
            .map_err(|e| format!("Failed to create Shelf view: {}", e))?;
        self.views.insert(ViewType::Shelf, shelf_id);
        self.engine_view_types.insert(shelf_id, ViewType::Shelf);
        debug!(?shelf_id, "Shelf view created");
        
        info!(chrome = ?chrome_id, content = ?content_id, shelf = ?shelf_id, "All views created");

        Ok(())
    }

    /// Load initial HTML content into views.
    fn load_initial_content(&self) -> Result<(), String> {
        let mut engine = self.engine.borrow_mut();

        // Load Chrome UI
        if let Some(&chrome_id) = self.views.get(&ViewType::Chrome) {
            engine
                .load_html(chrome_id, CHROME_HTML)
                .map_err(|e| format!("Failed to load Chrome HTML: {}", e))?;
            debug!(bytes = CHROME_HTML.len(), "Chrome HTML loaded");
        }

        // Load Shelf UI
        if let Some(&shelf_id) = self.views.get(&ViewType::Shelf) {
            engine
                .load_html(shelf_id, SHELF_HTML)
                .map_err(|e| format!("Failed to load Shelf HTML: {}", e))?;
            debug!(bytes = SHELF_HTML.len(), "Shelf HTML loaded");
        }

        // Load About page in content view
        if let Some(&content_id) = self.views.get(&ViewType::Content) {
            engine
                .load_html(content_id, ABOUT_HTML)
                .map_err(|e| format!("Failed to load About HTML: {}", e))?;
            debug!(bytes = ABOUT_HTML.len(), "About page loaded");
        }

        info!("Initial content loaded into all views");
        Ok(())
    }

    /// Calculate bounds for the content view.
    fn calculate_content_bounds(&self) -> Bounds {
        let x = if self.sidebar_open {
            self.sidebar_width as i32
        } else {
            0
        };
        let y = 0; // Content starts at top (no toolbar in native mode)
        let width = if self.sidebar_open {
            self.window_width.saturating_sub(self.sidebar_width)
        } else {
            self.window_width
        };
        let height = self
            .window_height
            .saturating_sub(self.shelf_height);

        Bounds::new(x, y, width, height)
    }

    /// Calculate bounds for the shelf view.
    fn calculate_shelf_bounds(&self) -> Bounds {
        let x = if self.sidebar_open {
            self.sidebar_width as i32
        } else {
            0
        };
        let y = (self.window_height - self.shelf_height) as i32;
        let width = if self.sidebar_open {
            self.window_width.saturating_sub(self.sidebar_width)
        } else {
            self.window_width
        };

        Bounds::new(x, y, width, self.shelf_height)
    }

    /// Update layout after window resize or layout change.
    fn update_layout(&self) {
        let mut engine = self.engine.borrow_mut();

        // Update Chrome view (sidebar area - left column, full height)
        if let Some(&chrome_id) = self.views.get(&ViewType::Chrome) {
            let width = if self.sidebar_open { self.sidebar_width } else { 0 };
            let bounds = Bounds::new(0, 0, width, self.window_height);
            if let Err(e) = engine.resize_view(chrome_id, bounds) {
                warn!(error = %e, "Failed to resize Chrome view");
            }
        }

        // Update Content view
        if let Some(&content_id) = self.views.get(&ViewType::Content) {
            let bounds = self.calculate_content_bounds();
            if let Err(e) = engine.resize_view(content_id, bounds) {
                warn!(error = %e, "Failed to resize Content view");
            }
        }

        // Update Shelf view
        if let Some(&shelf_id) = self.views.get(&ViewType::Shelf) {
            let bounds = self.calculate_shelf_bounds();
            if let Err(e) = engine.resize_view(shelf_id, bounds) {
                warn!(error = %e, "Failed to resize Shelf view");
            }
        }
    }

    /// Handle window resize event.
    #[allow(dead_code)]
    pub fn on_resize(&mut self, width: u32, height: u32) {
        self.window_width = width;
        self.window_height = height;
        self.update_layout();
    }

    /// Expand the Chrome UI (for overlays).
    pub fn expand_chrome(&mut self) {
        self.chrome_height = CHROME_HEIGHT_EXPANDED;
        self.update_layout();
    }

    /// Collapse the Chrome UI to default height.
    pub fn collapse_chrome(&mut self) {
        self.chrome_height = CHROME_HEIGHT_DEFAULT;
        self.update_layout();
    }

    /// Expand the Shelf (command palette).
    pub fn expand_shelf(&mut self) {
        self.shelf_height = SHELF_HEIGHT_EXPANDED;
        self.update_layout();
    }

    /// Collapse the Shelf.
    pub fn collapse_shelf(&mut self) {
        self.shelf_height = SHELF_HEIGHT_DEFAULT;
        self.update_layout();
    }

    /// Toggle sidebar visibility.
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_open = !self.sidebar_open;
        self.update_layout();
    }

    /// Navigate to a URL.
    /// Stop the content view's in-flight navigation.
    ///
    /// This is the first of the five rented capabilities to become OURS. The
    /// hybrid shell implements Stop as `evaluate_script("window.stop()")` —
    /// a JavaScript call asking Chromium to cancel its own load. This asks
    /// nobody: `Engine::stop` marks the navigation superseded so the engine
    /// will not apply its result.
    ///
    /// Honest limit, same as the engine's: the socket is not aborted, because
    /// `rustkit-net::fetch` has no cancellation surface yet. The bytes arrive
    /// and are discarded. Stop-as-observed, not stop-as-transport.
    pub fn stop_loading(&self) {
        if let Some(&content_id) = self.views.get(&ViewType::Content) {
            let mut engine = self.engine.borrow_mut();
            if engine.stop(content_id) {
                info!("Navigation stopped");
            } else {
                warn!("Stop requested for a content view that does not exist");
            }
        }
    }

    /// History traversal — back, forward, reload — on OUR engine.
    ///
    /// The hybrid shell rents all three from Chromium via
    /// `evaluate_script("history.back()")` etc. These run against RustKit's
    /// SessionHistory (canonical per the 2026-08-05 pin) and our own loader.
    ///
    /// Same block_on shape as `navigate` — one current-thread runtime per
    /// call. That is the known engine-thread soft spot (Atlas's follow-up
    /// unit, inherited by this tree), not a new decision made here.
    /// Apply a tab switch to the Win32 side.
    ///
    /// THE INVARIANT THIS MAINTAINS: `views[ViewType::Content]` always names
    /// the active tab's view. Every existing code path — navigate, traverse,
    /// resize, render, IPC routing — reads Content and keeps working without
    /// knowing tabs exist. Tabs became a list of content views plus this one
    /// rule, rather than a rewrite of the shell.
    fn apply_switch(&mut self, switch: TabSwitch<EngineViewId>) {
        {
            let engine = self.engine.borrow();
            if let Err(e) = engine.set_view_visible(switch.previous, false) {
                warn!(error = %e, "Failed to hide the outgoing tab");
            }
            if let Err(e) = engine.set_view_visible(switch.current, true) {
                warn!(error = %e, "Failed to show the incoming tab");
            }
        }
        self.views.insert(ViewType::Content, switch.current);
        self.engine_view_types
            .insert(switch.current, ViewType::Content);
    }

    /// Open a new tab and select it.
    fn new_tab(&mut self) {
        let Some(parent) = self.viewhost.get_main_hwnd() else {
            warn!("No main window — cannot open a tab");
            return;
        };
        let bounds = self.calculate_content_bounds();
        let created = self.engine.borrow_mut().create_view(parent, bounds);
        let view = match created {
            Ok(v) => v,
            Err(e) => {
                error!(error = %e, "Failed to create a view for the new tab");
                return;
            }
        };
        let Some(tabs) = self.tabs.as_mut() else {
            warn!("Tab model not initialised — cannot open a tab");
            return;
        };
        let switch = tabs.push(view);
        let count = tabs.count();
        self.apply_switch(switch);
        info!(?view, count, "New tab");
    }

    /// Close the active tab.
    ///
    /// Closing the LAST tab is deliberately a no-op here rather than a window
    /// close: quitting on Ctrl+W is a product decision, and the tab strip
    /// refusing to empty itself (`close_active` -> None) is what keeps
    /// `active_view()` total. Wire the quit at the window layer when the
    /// product calls for it.
    fn close_active_tab(&mut self) {
        let Some(tabs) = self.tabs.as_mut() else {
            return;
        };
        let Some((closed, switch)) = tabs.close_active() else {
            debug!("Refusing to close the last tab");
            return;
        };
        let count = tabs.count();
        self.apply_switch(switch);
        // Destroy AFTER the switch: the replacement is already visible, so
        // there is no frame where the window has nothing to show.
        if let Err(e) = self.engine.borrow_mut().destroy_view(closed) {
            warn!(error = %e, ?closed, "Failed to destroy the closed tab's view");
        }
        self.engine_view_types.remove(&closed);
        info!(?closed, count, "Tab closed");
    }

    /// Select a tab by index (Ctrl+1..9).
    fn activate_tab_by_index(&mut self, index: usize) {
        let Some(tabs) = self.tabs.as_mut() else {
            return;
        };
        match tabs.activate(index) {
            Some(switch) => {
                self.apply_switch(switch);
                debug!(index, "Tab activated");
            }
            // Out of range or already active: nothing to show or hide.
            None => debug!(index, "Tab activation: nothing to do"),
        }
    }

    fn traverse(&self, direction: &str) {
        let Some(&content_id) = self.views.get(&ViewType::Content) else {
            return;
        };
        let mut engine = self.engine.borrow_mut();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create runtime");
        rt.block_on(async {
            let result = match direction {
                "back" => engine.go_back(content_id).await,
                "forward" => engine.go_forward(content_id).await,
                "reload" => engine.reload(content_id).await,
                _ => return,
            };
            match result {
                Ok(true) => info!(direction, "History traversal complete"),
                // Nowhere to go is a no-op, not an error — mashing Back on
                // the first page must stay silent.
                Ok(false) => debug!(direction, "History traversal: nothing to do"),
                Err(e) => error!(error = %e, direction, "History traversal failed"),
            }
        });
    }

    pub fn navigate(&self, url: &str) {
        if let Some(&content_id) = self.views.get(&ViewType::Content) {
            match url::Url::parse(url) {
                Ok(parsed) => {
                    let mut engine = self.engine.borrow_mut();
                    // Create a runtime for the async operation
                    let rt = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .expect("Failed to create runtime");

                    rt.block_on(async {
                        if let Err(e) = engine.load_url(content_id, parsed).await {
                            error!(error = %e, url, "Failed to navigate");
                        }
                    });
                }
                Err(e) => {
                    warn!(error = %e, url, "Invalid URL");
                }
            }
        }
    }

    /// Execute JavaScript in a view.
    #[allow(dead_code)]
    pub fn execute_script(&self, view_type: ViewType, script: &str) -> Option<String> {
        if let Some(&view_id) = self.views.get(&view_type) {
            let mut engine = self.engine.borrow_mut();
            match engine.execute_script(view_id, script) {
                Ok(result) => Some(result),
                Err(e) => {
                    debug!(error = %e, "Script execution failed");
                    None
                }
            }
        } else {
            None
        }
    }

    /// Run the browser's main message loop with IPC processing.
    pub fn run(&mut self) {
        info!("Starting native browser message loop");

        loop {
            // Process Win32 messages (non-blocking)
            if !self.viewhost.pump_messages() {
                // WM_QUIT received, exit the loop
                break;
            }

            // Render all views
            self.engine.borrow_mut().render_all_views();

            // Process any IPC messages from views
            self.process_ipc_messages();

            // Small sleep to prevent busy-waiting (target ~60fps)
            std::thread::sleep(std::time::Duration::from_millis(16));
        }

        info!("Browser message loop ended");
    }

    /// Process IPC messages from all views.
    fn process_ipc_messages(&mut self) {
        let messages = self.engine.borrow().drain_ipc_messages();

        for (view_id, ipc_msg) in messages {
            let view_type = self.engine_view_types.get(&view_id).copied();
            self.handle_ipc_message(view_type, ipc_msg);
        }
    }

    /// Handle a single IPC message.
    fn handle_ipc_message(&mut self, view_type: Option<ViewType>, msg: IpcMessage) {
        // Parse the JSON payload
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(&msg.payload);
        let json = match parsed {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, payload_len = msg.payload.len(), "Failed to parse IPC message JSON");
                return;
            }
        };

        // Extract the command
        let cmd = match json.get("cmd").and_then(|v| v.as_str()) {
            Some(c) => c,
            None => {
                trace!("IPC message missing 'cmd' field (likely data payload)");
                return;
            }
        };

        // Log at trace level for high-frequency commands, debug for others
        match cmd {
            // High-frequency, log at trace only
            "log" | "sync_tabs" | "sync_workspaces" | "sync_downloads" => {
                trace!(?view_type, cmd, "IPC");
            }
            // Low-frequency, log at debug
            _ => {
                debug!(?view_type, cmd, "IPC command");
            }
        }

        // Handle the command
        match cmd {
            "navigate" => {
                if let Some(url) = json.get("url").and_then(|v| v.as_str()) {
                    info!(url, "Navigation started");
                    self.navigate(url);
                }
            }
            "go_back" => {
                self.traverse("back");
            }
            "go_forward" => {
                self.traverse("forward");
            }
            "new_tab" => self.new_tab(),
            "close_active_tab" => self.close_active_tab(),
            "activate_tab_by_index" => {
                match json.get("index").and_then(|v| v.as_u64()) {
                    Some(i) => self.activate_tab_by_index(i as usize),
                    None => warn!("activate_tab_by_index without an index"),
                }
            }
            "reload" => {
                self.traverse("reload");
            }
            "stop" => {
                self.stop_loading();
            }
            "expand_chrome" => {
                trace!("Layout: chrome expanded");
                self.expand_chrome();
            }
            "collapse_chrome" => {
                trace!("Layout: chrome collapsed");
                self.collapse_chrome();
            }
            "expand_shelf" | "open_command_palette" => {
                debug!("Shelf opened");
                self.expand_shelf();
            }
            "collapse_shelf" | "close_command_palette" => {
                trace!("Shelf closed");
                self.collapse_shelf();
            }
            "toggle_sidebar" => {
                debug!(open = !self.sidebar_open, "Sidebar toggled");
                self.toggle_sidebar();
            }
            "chrome_ready" => {
                info!("Chrome UI initialized and ready");
                // TODO: Send initial state to Chrome UI
            }
            "log" => {
                // JavaScript console log forwarding
                let level = json.get("level").and_then(|v| v.as_str()).unwrap_or("info");
                let message = json.get("message").and_then(|v| v.as_str()).unwrap_or("");
                match level {
                    "error" => error!(source = "js", "{}", message),
                    "warn" => warn!(source = "js", "{}", message),
                    "debug" => trace!(source = "js", "{}", message), // Downgrade JS debug to trace
                    _ => trace!(source = "js", "{}", message), // JS info → trace (very frequent)
                }
            }
            _ => {
                trace!(cmd, "Unhandled IPC command (may be hybrid-only)");
            }
        }
    }
}

impl Drop for NativeBrowser {
    fn drop(&mut self) {
        info!("Shutting down native browser");

        // Destroy all views
        let mut engine = self.engine.borrow_mut();
        for (&view_type, &view_id) in &self.views {
            if let Err(e) = engine.destroy_view(view_id) {
                warn!(?view_type, error = %e, "Failed to destroy view");
            }
        }
    }
}

/// Print startup banner with build info.
fn print_startup_banner() {
    info!("=== HiWave Browser ===");
    info!(
        version = env!("CARGO_PKG_VERSION"),
        engine = "RustKit",
        mode = "native-win32",
        "Startup"
    );
    info!(
        target_os = std::env::consts::OS,
        target_arch = std::env::consts::ARCH,
        "Platform"
    );
    
    // Get DPI scale if available
    #[cfg(windows)]
    {
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        let dpi = unsafe { GetDpiForSystem() };
        let scale = dpi as f64 / 96.0;
        info!(dpi, scale, "Display");
    }
}

/// Install panic hook for structured crash logging.
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        // Log the panic with tracing
        let location = panic_info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()));
        let message = panic_info.payload().downcast_ref::<&str>().map(|s| *s)
            .or_else(|| panic_info.payload().downcast_ref::<String>().map(|s| s.as_str()));
        
        error!(
            location = location.as_deref(),
            message = message,
            "PANIC - Browser crashed"
        );
        
        // Call the default hook for normal panic behavior
        default_hook(panic_info);
    }));
}

/// Entry point for native Win32 mode.
///
/// This function is called from main.rs when the `native-win32` feature is enabled.
pub fn run_native() -> Result<(), String> {
    install_panic_hook();
    print_startup_banner();

    // Check for screenshot test mode
    if let Some(config) = super::screenshot_harness::parse_screenshot_args() {
        info!("Running in screenshot test mode");
        return run_screenshot_mode(config);
    }

    let mut browser = NativeBrowser::new()?;
    browser.init()?;
    browser.run();

    info!("HiWave shutdown complete");
    Ok(())
}

/// Run in screenshot test mode.
fn run_screenshot_mode(config: super::screenshot_harness::ScreenshotConfig) -> Result<(), String> {
    use super::screenshot_harness::{get_test_scene_html, CaptureMetadata, GpuCaptureInfo, OsCaptureInfo, RenderStatsInfo};
    use std::fs;
    
    info!(
        output_dir = %config.output_dir.display(),
        scene = ?config.scene,
        url = ?config.url,
        frames = config.wait_frames,
        "Screenshot test configuration"
    );
    
    // Create output directory
    fs::create_dir_all(&config.output_dir)
        .map_err(|e| format!("Failed to create output directory: {}", e))?;
    
    // Create browser instance
    let mut browser = NativeBrowser::new()?;
    browser.init()?;
    
    // Determine what content to load
    let scene_name = if let Some(ref scene) = config.scene {
        // Load test scene HTML
        if let Some(html) = get_test_scene_html(scene) {
            info!(scene, "Loading test scene");
            if let Some(&content_id) = browser.views.get(&ViewType::Content) {
                browser.engine.borrow_mut()
                    .load_html(content_id, html)
                    .map_err(|e| format!("Failed to load scene HTML: {}", e))?;
            }
            scene.clone()
        } else {
            return Err(format!("Unknown test scene: {}. Available: {:?}", 
                scene, super::screenshot_harness::list_test_scenes()));
        }
    } else if let Some(ref url) = config.url {
        info!(url, "Loading URL");
        browser.navigate(url);
        url.replace(['/', ':', '.'], "_")
    } else {
        // Default to about page
        "about".to_string()
    };
    
    // Wait for frames to render
    info!(frames = config.wait_frames, "Rendering frames");
    for frame in 0..config.wait_frames {
        // Process messages and render
        browser.viewhost.pump_messages();
        browser.engine.borrow_mut().render_all_views();
        browser.process_ipc_messages();
        
        debug!(frame, "Frame rendered");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    
    // One more render pass to ensure content is ready
    browser.viewhost.pump_messages();
    browser.engine.borrow_mut().render_all_views();
    
    // Capture screenshots
    let timestamp = chrono_lite_timestamp();
    let mut gpu_captures: Vec<GpuCaptureInfo> = Vec::new();
    let mut os_captures: Vec<OsCaptureInfo> = Vec::new();
    
    // Get render stats
    let stats = browser.engine.borrow().get_render_stats();
    let render_stats = Some(RenderStatsInfo {
        color_vertex_count: stats.color_vertex_count,
        texture_vertex_count: stats.texture_vertex_count,
        color_index_count: stats.color_index_count,
        texture_index_count: stats.texture_index_count,
    });
    
    // GPU readback capture (per-view)
    if config.gpu_capture {
        for (view_type, &view_id) in browser.views.iter() {
            let view_name = match view_type {
                ViewType::Chrome => "chrome",
                ViewType::Content => "content",
                ViewType::Shelf => "shelf",
            };
            let gpu_path = config
                .output_dir
                .join(format!("{}_gpu_{}.png", scene_name, view_name));
            info!(view = view_name, path = %gpu_path.display(), "Capturing GPU readback screenshot");

            match browser.engine.borrow_mut().capture_view_screenshot(view_id, &gpu_path) {
                Ok(metadata) => {
                    info!(
                        view = view_name,
                        width = metadata.width,
                        height = metadata.height,
                        color_vertices = metadata.color_vertex_count,
                        texture_vertices = metadata.texture_vertex_count,
                        "GPU screenshot captured"
                    );
                    gpu_captures.push(GpuCaptureInfo {
                        view: view_name.to_string(),
                        path: gpu_path.to_string_lossy().to_string(),
                        adapter: metadata.adapter,
                        format: metadata.format,
                    });
                }
                Err(e) => {
                    error!(view = view_name, error = %e, "GPU screenshot capture failed");
                }
            }
        }
    }
    
    // OS window capture (main window + per-view child HWND)
    if config.os_capture {
        // Main window
        let os_main_path = config.output_dir.join(format!("{}_os_main.png", scene_name));
        info!(path = %os_main_path.display(), "Capturing OS window screenshot (main)");
        if let Some(hwnd) = browser.viewhost.get_main_hwnd() {
            match super::screenshot_harness::capture_os_window(hwnd, &os_main_path) {
                Ok((width, height)) => {
                    info!(width, height, "OS screenshot captured (main)");
                    os_captures.push(OsCaptureInfo {
                        view: "main".to_string(),
                        path: os_main_path.to_string_lossy().to_string(),
                        capture_method: "BitBlt".to_string(),
                    });
                }
                Err(e) => {
                    error!(error = %e, "OS screenshot capture failed (main)");
                }
            }
        }

        // Per-view child windows
        for (view_type, &view_id) in browser.views.iter() {
            let view_name = match view_type {
                ViewType::Chrome => "chrome",
                ViewType::Content => "content",
                ViewType::Shelf => "shelf",
            };
            let os_path = config
                .output_dir
                .join(format!("{}_os_{}.png", scene_name, view_name));
            info!(view = view_name, path = %os_path.display(), "Capturing OS window screenshot (view)");

            let hwnd = match browser.engine.borrow().get_view_hwnd(view_id) {
                Ok(h) => h,
                Err(e) => {
                    error!(view = view_name, error = %e, "Failed to get view HWND");
                    continue;
                }
            };

            match super::screenshot_harness::capture_os_window(hwnd, &os_path) {
                Ok((width, height)) => {
                    info!(view = view_name, width, height, "OS screenshot captured (view)");
                    os_captures.push(OsCaptureInfo {
                        view: view_name.to_string(),
                        path: os_path.to_string_lossy().to_string(),
                        capture_method: "BitBlt".to_string(),
                    });
                }
                Err(e) => {
                    error!(view = view_name, error = %e, "OS screenshot capture failed (view)");
                }
            }
        }
    }
    
    // Save metadata
    let metadata = CaptureMetadata {
        timestamp,
        scene: scene_name.clone(),
        width: browser.window_width,
        height: browser.window_height,
        wait_frames: config.wait_frames,
        gpu_captures,
        os_captures,
        render_stats,
    };
    
    let metadata_path = config.output_dir.join(format!("{}_metadata.json", scene_name));
    let json = serde_json::to_string_pretty(&metadata)
        .map_err(|e| format!("Failed to serialize metadata: {}", e))?;
    fs::write(&metadata_path, json)
        .map_err(|e| format!("Failed to write metadata: {}", e))?;
    
    info!(path = %metadata_path.display(), "Metadata written");
    info!("Screenshot capture complete");
    
    Ok(())
}

/// Simple timestamp without chrono dependency.
fn chrono_lite_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
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

