//! # RustKit Engine
//!
//! Browser engine orchestration layer that integrates all RustKit components
//! to provide a complete multi-view browser engine.
//!
//! ## Design Goals
//!
//! 1. **Multi-view support**: Manage multiple independent browser views
//! 2. **Unified API**: Single entry point for all browser functionality
//! 3. **Event coordination**: Route events between views and host
//! 4. **Resource sharing**: Share compositor and network resources

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rustkit_bindings::DomBindings;
// Re-export types for external use
pub use rustkit_bindings::IpcMessage;
pub use rustkit_renderer::{RenderStats, ScreenshotMetadata};
use rustkit_compositor::Compositor;
use rustkit_core::{LoadEvent, NavigationRequest, NavigationStateMachine};
use rustkit_css::{ComputedStyle, Length, PropertyValue, Stylesheet};
use rustkit_dom::{Document, Node, NodeType};
use rustkit_image::ImageManager;
use rustkit_js::JsRuntime;
use rustkit_layout::{BoxType, Dimensions, DisplayList, LayoutBox, Rect};
use rustkit_net::{LoaderConfig, NetError, Request, ResourceLoader};
use rustkit_renderer::Renderer;
use rustkit_viewhost::{Bounds, ViewHost, ViewId};
use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, trace, warn};
use url::Url;
#[cfg(windows)]
use windows::Win32::Foundation::HWND;

/// Errors that can occur in the engine.
#[derive(Error, Debug)]
pub enum EngineError {
    #[error("View error: {0}")]
    ViewError(String),

    #[error("Network error: {0}")]
    NetworkError(#[from] NetError),

    #[error("Navigation error: {0}")]
    NavigationError(String),

    #[error("Render error: {0}")]
    RenderError(String),

    #[error("JS error: {0}")]
    JsError(String),

    #[error("View not found: {0:?}")]
    ViewNotFound(EngineViewId),
}

/// Unique identifier for an engine view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EngineViewId(u64);

impl EngineViewId {
    fn new() -> Self {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    pub fn raw(&self) -> u64 {
        self.0
    }
}

/// Engine events emitted to the host application.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// Navigation started.
    NavigationStarted { view_id: EngineViewId, url: Url },
    /// Navigation committed (first bytes received).
    NavigationCommitted { view_id: EngineViewId, url: Url },
    /// Page fully loaded.
    PageLoaded {
        view_id: EngineViewId,
        url: Url,
        title: Option<String>,
    },
    /// Navigation failed.
    NavigationFailed {
        view_id: EngineViewId,
        url: Url,
        error: String,
    },
    /// Title changed.
    TitleChanged {
        view_id: EngineViewId,
        title: String,
    },
    /// Console message from JavaScript.
    ConsoleMessage {
        view_id: EngineViewId,
        level: String,
        message: String,
    },
    /// View resized.
    ViewResized {
        view_id: EngineViewId,
        width: u32,
        height: u32,
    },
    /// View received focus.
    ViewFocused { view_id: EngineViewId },
    /// Download started.
    DownloadStarted { url: Url, filename: String },
    /// Image loaded.
    ImageLoaded {
        view_id: EngineViewId,
        url: Url,
        width: u32,
        height: u32,
    },
    /// Image failed to load.
    ImageError {
        view_id: EngineViewId,
        url: Url,
        error: String,
    },
    /// Favicon detected.
    FaviconDetected {
        view_id: EngineViewId,
        url: Url,
    },
}

/// View state.
#[allow(dead_code)]
struct ViewState {
    id: EngineViewId,
    viewhost_id: ViewId,
    url: Option<Url>,
    title: Option<String>,
    document: Option<Rc<Document>>,
    #[allow(dead_code)]
    layout: Option<LayoutBox>,
    #[allow(dead_code)]
    display_list: Option<DisplayList>,
    #[allow(dead_code)]
    bindings: Option<DomBindings>,
    navigation: NavigationStateMachine,
    #[allow(dead_code)]
    nav_event_rx: mpsc::UnboundedReceiver<LoadEvent>,
    /// Currently focused DOM node.
    focused_node: Option<rustkit_dom::NodeId>,
    /// Whether the view itself has focus.
    view_focused: bool,
    /// Headless bounds (only set for headless views, None for window-based views).
    headless_bounds: Option<Bounds>,
    /// CSS text of every successfully fetched `<link rel="stylesheet">`, in
    /// document order. Held on the view rather than re-fetched at layout time
    /// because `relayout` is synchronous and runs on every resize.
    external_css: String,
}

/// Engine configuration.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// User agent string.
    pub user_agent: String,
    /// Enable JavaScript.
    pub javascript_enabled: bool,
    /// Enable cookies.
    pub cookies_enabled: bool,
    /// Default background color.
    pub background_color: [f64; 4],
    /// Disable animations and transitions for deterministic parity captures.
    pub disable_animations: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            user_agent: "RustKit/1.0 HiWave/1.0".to_string(),
            javascript_enabled: true,
            cookies_enabled: true,
            background_color: [1.0, 1.0, 1.0, 1.0], // White
            disable_animations: false,
        }
    }
}

impl EngineConfig {
    /// Create a configuration for parity testing (animations disabled).
    pub fn for_parity_testing() -> Self {
        Self {
            disable_animations: true,
            ..Default::default()
        }
    }
}

/// The main browser engine.
pub struct Engine {
    config: EngineConfig,
    viewhost: ViewHost,
    compositor: Compositor,
    renderer: Option<Renderer>,
    loader: Arc<ResourceLoader>,
    image_manager: Arc<ImageManager>,
    views: HashMap<EngineViewId, ViewState>,
    event_tx: mpsc::UnboundedSender<EngineEvent>,
    event_rx: Option<mpsc::UnboundedReceiver<EngineEvent>>,
}

/// Minimal identity of an ancestor element, captured while walking the DOM so
/// descendant selectors (`.card p`) can verify the ancestor chain instead of
/// matching on the subject alone.
struct ElementCtx {
    tag: String,
    classes: Vec<String>,
    id: Option<String>,
}

impl Engine {
    /// Create a new browser engine.
    pub fn new(config: EngineConfig) -> Result<Self, EngineError> {
        Self::with_interceptor(config, None)
    }

    /// Create a new browser engine with an optional request interceptor.
    pub fn with_interceptor(
        config: EngineConfig,
        interceptor: Option<rustkit_net::RequestInterceptor>,
    ) -> Result<Self, EngineError> {
        info!("Initializing RustKit Engine");

        // Initialize ViewHost
        let viewhost = ViewHost::new();

        // Initialize Compositor
        let compositor = Compositor::new().map_err(|e| EngineError::RenderError(e.to_string()))?;

        // Initialize ResourceLoader
        let loader_config = LoaderConfig {
            user_agent: config.user_agent.clone(),
            cookies_enabled: config.cookies_enabled,
            ..Default::default()
        };
        let loader = if let Some(interceptor) = interceptor {
            info!("ResourceLoader initialized with request interceptor");
            Arc::new(
                ResourceLoader::with_interceptor(loader_config, interceptor)
                    .map_err(EngineError::NetworkError)?,
            )
        } else {
            Arc::new(ResourceLoader::new(loader_config).map_err(EngineError::NetworkError)?)
        };

        // Initialize ImageManager
        let image_manager = Arc::new(ImageManager::new());

        // Initialize Renderer
        let renderer = Renderer::new(
            compositor.device_arc(),
            compositor.queue_arc(),
            compositor.surface_format(),
        ).map_err(|e| EngineError::RenderError(e.to_string()))?;

        // Event channel
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        info!(
            adapter = ?compositor.adapter_info().name,
            "Engine initialized with GPU renderer"
        );

        Ok(Self {
            config,
            viewhost,
            compositor,
            renderer: Some(renderer),
            loader,
            image_manager,
            views: HashMap::new(),
            event_tx,
            event_rx: Some(event_rx),
        })
    }

    /// Take the event receiver.
    pub fn take_event_receiver(&mut self) -> Option<mpsc::UnboundedReceiver<EngineEvent>> {
        self.event_rx.take()
    }

    /// Create a new view.
    #[cfg(windows)]
    pub fn create_view(
        &mut self,
        parent: HWND,
        bounds: Bounds,
    ) -> Result<EngineViewId, EngineError> {
        let id = EngineViewId::new();

        debug!(?id, ?bounds, "Creating view");

        // Create viewhost view
        let viewhost_id = self
            .viewhost
            .create_view(parent, bounds)
            .map_err(|e| EngineError::ViewError(e.to_string()))?;

        // Create compositor surface
        let hwnd = self
            .viewhost
            .get_hwnd(viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))?;

        unsafe {
            self.compositor
                .create_surface_for_hwnd(viewhost_id, hwnd, bounds.width, bounds.height)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;
        }

        // Create navigation state machine
        let (nav_tx, nav_rx) = mpsc::unbounded_channel();
        let navigation = NavigationStateMachine::new(nav_tx);

        // Create view state
        let view_state = ViewState {
            id,
            viewhost_id,
            url: None,
            title: None,
            document: None,
            layout: None,
            display_list: None,
            bindings: None,
            navigation,
            nav_event_rx: nav_rx,
            focused_node: None,
            view_focused: false,
            headless_bounds: None,
            external_css: String::new(),
        };

        self.views.insert(id, view_state);

        // Render initial background
        self.compositor
            .render_solid_color(viewhost_id, self.config.background_color)
            .map_err(|e| EngineError::RenderError(e.to_string()))?;

        info!(?id, "View created");
        Ok(id)
    }

    #[cfg(not(windows))]
    pub fn create_view(
        &mut self,
        _parent: usize,
        _bounds: Bounds,
    ) -> Result<EngineViewId, EngineError> {
        Err(EngineError::RenderError("create_view is only supported on Windows".to_string()))
    }

    /// Create a headless view for offscreen rendering (testing/CI mode).
    ///
    /// This creates a view without requiring a window, perfect for unit tests
    /// and CI environments.
    pub fn create_headless_view(
        &mut self,
        bounds: Bounds,
    ) -> Result<EngineViewId, EngineError> {
        let id = EngineViewId::new();
        let viewhost_id = ViewId::new();

        debug!(?id, ?bounds, "Creating headless view");

        // Create headless texture instead of surface
        self.compositor
            .create_headless_texture(viewhost_id, bounds.width, bounds.height)
            .map_err(|e| EngineError::RenderError(e.to_string()))?;

        // Create navigation state machine
        let (nav_tx, nav_rx) = mpsc::unbounded_channel();
        let navigation = NavigationStateMachine::new(nav_tx);

        let view_state = ViewState {
            id,
            viewhost_id,
            url: None,
            title: None,
            document: None,
            layout: None,
            display_list: None,
            bindings: None,
            navigation,
            nav_event_rx: nav_rx,
            focused_node: None,
            view_focused: false,
            headless_bounds: Some(bounds),
            external_css: String::new(),
        };

        self.views.insert(id, view_state);

        // Render initial background to headless texture
        self.compositor
            .render_solid_color(viewhost_id, self.config.background_color)
            .map_err(|e| EngineError::RenderError(e.to_string()))?;

        info!(?id, "Headless view created");
        Ok(id)
    }

    /// Destroy a view.
    pub fn destroy_view(&mut self, id: EngineViewId) -> Result<(), EngineError> {
        let view = self
            .views
            .remove(&id)
            .ok_or(EngineError::ViewNotFound(id))?;

        // Destroy compositor surface
        let _ = self.compositor.destroy_surface(view.viewhost_id);

        // Destroy viewhost view
        let _ = self.viewhost.destroy_view(view.viewhost_id);

        info!(?id, "View destroyed");
        Ok(())
    }

    /// Resize a view.
    pub fn resize_view(&mut self, id: EngineViewId, bounds: Bounds) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;

        debug!(?id, ?bounds, "Resizing view");

        // Resize viewhost
        self.viewhost
            .set_bounds(view.viewhost_id, bounds)
            .map_err(|e| EngineError::ViewError(e.to_string()))?;

        // Resize compositor surface
        self.compositor
            .resize_surface(view.viewhost_id, bounds.width, bounds.height)
            .map_err(|e| EngineError::RenderError(e.to_string()))?;

        // Re-layout if we have content
        if self.views.get(&id).unwrap().document.is_some() {
            self.relayout(id)?;
        }

        // Emit event
        let _ = self.event_tx.send(EngineEvent::ViewResized {
            view_id: id,
            width: bounds.width,
            height: bounds.height,
        });

        Ok(())
    }

    /// Focus a view.
    pub fn focus_view(&self, id: EngineViewId) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;

        debug!(?id, "Focusing view");

        self.viewhost
            .focus(view.viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))?;

        Ok(())
    }

    /// Set view visibility.
    pub fn set_view_visible(&self, id: EngineViewId, visible: bool) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;

        debug!(?id, visible, "Setting view visibility");

        self.viewhost
            .set_visible(view.viewhost_id, visible)
            .map_err(|e| EngineError::ViewError(e.to_string()))?;

        Ok(())
    }

    /// Load a URL in a view.
    pub async fn load_url(&mut self, id: EngineViewId, url: Url) -> Result<(), EngineError> {
        let view = self
            .views
            .get_mut(&id)
            .ok_or(EngineError::ViewNotFound(id))?;

        info!(?id, %url, "Loading URL");

        // Start navigation
        let request = NavigationRequest::new(url.clone());
        view.navigation
            .start_navigation(request)
            .map_err(|e| EngineError::NavigationError(e.to_string()))?;

        // Emit event
        let _ = self.event_tx.send(EngineEvent::NavigationStarted {
            view_id: id,
            url: url.clone(),
        });

        // Fetch the URL
        let request = Request::get(url.clone());
        let response = self.loader.fetch(request).await?;

        if !response.ok() {
            let error = format!("HTTP {}", response.status);
            let view = self.views.get_mut(&id).unwrap();
            view.navigation
                .fail_navigation(error.clone())
                .map_err(|e| EngineError::NavigationError(e.to_string()))?;

            let _ = self.event_tx.send(EngineEvent::NavigationFailed {
                view_id: id,
                url,
                error,
            });

            return Err(EngineError::NavigationError("HTTP error".into()));
        }

        // Commit navigation
        let view = self.views.get_mut(&id).unwrap();
        view.navigation
            .commit_navigation()
            .map_err(|e| EngineError::NavigationError(e.to_string()))?;

        let _ = self.event_tx.send(EngineEvent::NavigationCommitted {
            view_id: id,
            url: url.clone(),
        });

        // Parse HTML
        let html = response.text().await?;
        let document =
            Document::parse_html(&html).map_err(|e| EngineError::RenderError(e.to_string()))?;
        let document = Rc::new(document);

        // Get title
        let title = document.title();

        // Store in view
        let view = self.views.get_mut(&id).unwrap();
        view.url = Some(url.clone());
        view.document = Some(document.clone());
        view.title = title.clone();
        // A new document starts with NO external CSS. Without this, a view
        // navigating from a styled page to one with no <link> (or to
        // load_html, which fetches no subresources at all) keeps the previous
        // document's stylesheet and renders the new page with the old page's
        // styles.
        view.external_css.clear();

        // Initialize JavaScript if enabled
        if self.config.javascript_enabled {
            let js_runtime = JsRuntime::new().map_err(|e| EngineError::JsError(e.to_string()))?;

            let bindings =
                DomBindings::new(js_runtime).map_err(|e| EngineError::JsError(e.to_string()))?;

            bindings
                .set_document(document.clone())
                .map_err(|e| EngineError::JsError(e.to_string()))?;

            bindings
                .set_location(&url)
                .map_err(|e| EngineError::JsError(e.to_string()))?;

            let view = self.views.get_mut(&id).unwrap();
            view.bindings = Some(bindings);
        }

        // Fetch <link rel="stylesheet"> BEFORE layout, so external rules take
        // part in the very first cascade instead of appearing on a later
        // repaint (a flash of unstyled content).
        self.load_external_stylesheets(id, &document, &url).await;

        // And the images, also before layout: an <img> contributes its
        // INTRINSIC size to layout, so loading after the first pass would lay
        // the page out with zero-sized images and then reflow.
        self.load_document_images(id, &document, &url).await;

        // Layout and render
        self.relayout(id)?;

        // Finish navigation
        let view = self.views.get_mut(&id).unwrap();
        view.navigation
            .finish_navigation()
            .map_err(|e| EngineError::NavigationError(e.to_string()))?;

        // Emit events
        if let Some(ref title) = title {
            let _ = self.event_tx.send(EngineEvent::TitleChanged {
                view_id: id,
                title: title.clone(),
            });
        }

        let _ = self.event_tx.send(EngineEvent::PageLoaded {
            view_id: id,
            url,
            title: view.title.clone(),
        });

        Ok(())
    }

    /// Load HTML content directly into a view.
    ///
    /// This is used for loading inline HTML content like the Chrome UI,
    /// without making an HTTP request.
    pub fn load_html(&mut self, id: EngineViewId, html: &str) -> Result<(), EngineError> {
        let view = self
            .views
            .get_mut(&id)
            .ok_or(EngineError::ViewNotFound(id))?;

        info!(?id, len = html.len(), "HTML: loading content");
        
        // Log first 100 chars of HTML for debugging
        let preview: String = html.chars().take(100).collect();
        info!(?id, preview = %preview, "HTML: preview");

        // Use a synthetic about:blank URL for inline content
        let url = Url::parse("about:blank").unwrap();

        // Start navigation
        let request = NavigationRequest::new(url.clone());
        view.navigation
            .start_navigation(request)
            .map_err(|e| EngineError::NavigationError(e.to_string()))?;

        // Emit event
        let _ = self.event_tx.send(EngineEvent::NavigationStarted {
            view_id: id,
            url: url.clone(),
        });

        // Commit navigation
        view.navigation
            .commit_navigation()
            .map_err(|e| EngineError::NavigationError(e.to_string()))?;

        let _ = self.event_tx.send(EngineEvent::NavigationCommitted {
            view_id: id,
            url: url.clone(),
        });

        // Parse HTML
        let document =
            Document::parse_html(html).map_err(|e| EngineError::RenderError(e.to_string()))?;
        let document = Rc::new(document);

        // Get title
        let title = document.title();

        // Store in view
        let view = self.views.get_mut(&id).unwrap();
        view.url = Some(url.clone());
        view.document = Some(document.clone());
        view.title = title.clone();
        // A new document starts with NO external CSS. Without this, a view
        // navigating from a styled page to one with no <link> (or to
        // load_html, which fetches no subresources at all) keeps the previous
        // document's stylesheet and renders the new page with the old page's
        // styles.
        view.external_css.clear();

        // Initialize JavaScript if enabled
        if self.config.javascript_enabled {
            let js_runtime = JsRuntime::new().map_err(|e| EngineError::JsError(e.to_string()))?;

            let bindings =
                DomBindings::new(js_runtime).map_err(|e| EngineError::JsError(e.to_string()))?;

            bindings
                .set_document(document.clone())
                .map_err(|e| EngineError::JsError(e.to_string()))?;

            bindings
                .set_location(&url)
                .map_err(|e| EngineError::JsError(e.to_string()))?;

            let view = self.views.get_mut(&id).unwrap();
            view.bindings = Some(bindings);
        }

        // Layout and render
        self.relayout(id)?;

        // Finish navigation
        let view = self.views.get_mut(&id).unwrap();
        view.navigation
            .finish_navigation()
            .map_err(|e| EngineError::NavigationError(e.to_string()))?;

        // Emit events
        if let Some(ref title) = title {
            let _ = self.event_tx.send(EngineEvent::TitleChanged {
                view_id: id,
                title: title.clone(),
            });
        }

        let _ = self.event_tx.send(EngineEvent::PageLoaded {
            view_id: id,
            url,
            title: view.title.clone(),
        });

        Ok(())
    }

    /// Re-layout a view.
    fn relayout(&mut self, id: EngineViewId) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;

        let document = view
            .document
            .as_ref()
            .ok_or(EngineError::RenderError("No document".into()))?
            .clone();

        // Get view bounds - use headless_bounds if set (for offscreen rendering),
        // otherwise query the viewhost
        let bounds = if let Some(headless_bounds) = view.headless_bounds {
            headless_bounds
        } else {
            self.viewhost
                .get_bounds(view.viewhost_id)
                .map_err(|e| EngineError::ViewError(e.to_string()))?
        };

        info!(
            ?id,
            width = bounds.width,
            height = bounds.height,
            "Layout: starting"
        );

        // Create containing block
        // NOTE: content.height is used as a cursor for vertical positioning, so it starts at 0.
        // The available viewport size is stored in the rect's width/height.
        let containing_block = Dimensions {
            content: Rect::new(0.0, 0.0, bounds.width as f32, 0.0), // height=0 means cursor at top
            ..Default::default()
        };

        // Build layout tree from DOM
        let external_css = view.external_css.clone();
        let mut root_box = self.build_layout_with_external_css(&document, &external_css);

        // Count children for debugging
        let child_count = root_box.children.len();
        info!(?id, child_count, "Layout: built tree from DOM");

        // Layout
        root_box.layout(&containing_block);

        // The canvas fills the viewport: stretch the root box so its propagated
        // background (CSS §14.2) paints the whole viewport, not just the content
        // height. Grow-only, so tall pages keep their full scroll height.
        let (vw, vh) = (bounds.width as f32, bounds.height as f32);
        if root_box.dimensions.content.width < vw {
            root_box.dimensions.content.width = vw;
        }
        if root_box.dimensions.content.height < vh {
            root_box.dimensions.content.height = vh;
        }

        // Generate display list
        let display_list = DisplayList::build(&root_box);

        // Count command types for debugging
        let mut solid_count = 0;
        let mut text_count = 0;
        let mut border_count = 0;
        let mut other_count = 0;
        for cmd in &display_list.commands {
            match cmd {
                rustkit_layout::DisplayCommand::SolidColor(_, _) => solid_count += 1,
                rustkit_layout::DisplayCommand::Text { .. } => text_count += 1,
                rustkit_layout::DisplayCommand::Border { .. } => border_count += 1,
                _ => other_count += 1,
            }
        }
        
        info!(
            ?id,
            num_commands = display_list.commands.len(),
            solid_count,
            text_count,
            border_count,
            other_count,
            "Layout: generated display list"
        );
        
        // Print first few text commands for debugging
        for (i, cmd) in display_list.commands.iter().enumerate() {
            if let rustkit_layout::DisplayCommand::Text { text, x, y, font_size, .. } = cmd {
                if i < 5 {
                    info!(
                        ?id,
                        index = i,
                        text = %text,
                        x = x,
                        y = y,
                        font_size = font_size,
                        "Layout: text command"
                    );
                }
            }
        }

        // Store
        let view = self.views.get_mut(&id).unwrap();
        view.layout = Some(root_box);
        view.display_list = Some(display_list);

        // Render
        self.render(id)?;

        Ok(())
    }

    /// Build a layout tree from a DOM document.
    fn build_layout_from_document(&self, document: &Document) -> LayoutBox {
        self.build_layout_with_external_css(document, "")
    }

    /// Same, plus the CSS text of any fetched external stylesheets.
    ///
    /// `external_css` is placed BEFORE the inline `<style>` text so that an
    /// inline rule wins over an external one at equal specificity. That is the
    /// common authoring order (`<link>` then `<style>` in `<head>`) but it is a
    /// SIMPLIFICATION: true CSS cascade order follows the document position of
    /// each element, so a `<style>` that appears before a `<link>` is applied
    /// in the wrong order here. Stated rather than glossed; fixing it needs
    /// per-element source ordering, which this change does not add.
    fn build_layout_with_external_css(
        &self,
        document: &Document,
        external_css: &str,
    ) -> LayoutBox {
        // Create root layout box for the document
        let mut root_style = ComputedStyle::new();
        root_style.background_color = rustkit_css::Color::WHITE;
        let mut root_box = LayoutBox::new(BoxType::Block, root_style);

        // Collect and parse author stylesheets from <style> elements so the
        // cascade can apply them (previously only UA defaults + inline applied,
        // so every `<style>` rule — backgrounds, class selectors, sizes — was
        // silently ignored).
        let mut css_text = String::new();
        if !external_css.is_empty() {
            css_text.push_str(external_css);
            css_text.push('\n');
        }
        self.collect_style_text(&document.root(), &mut css_text);
        let sheet = Stylesheet::parse(&css_text).unwrap_or_else(|_| Stylesheet::new());
        info!(rule_count = sheet.rules.len(), css_len = css_text.len(), "CSS: author stylesheet parsed");
        let mut root_inherited = ComputedStyle::new();
        // Seed custom properties defined at the document root. The layout tree
        // is built from <body>, so a `:root { --x: ... }` block (the design-token
        // pattern every builtin UI uses) would otherwise never be computed and
        // every `var(--x)` would resolve to nothing — a blank page.
        let root_vars = Self::collect_root_custom_properties(&sheet);
        if !root_vars.is_empty() {
            root_inherited.custom_properties = Arc::new(root_vars);
        }

        // Debug: print root children to understand DOM structure
        let root_children = document.root().children();
        info!(
            root_children = root_children.len(),
            "DOM: document root children count"
        );
        for (i, child) in root_children.iter().take(5).enumerate() {
            if let NodeType::Element { tag_name, .. } = &child.node_type {
                info!(index = i, tag = %tag_name, "DOM: root child");
                // Print grandchildren too
                for (j, grandchild) in child.children().iter().take(3).enumerate() {
                    if let NodeType::Element { tag_name, .. } = &grandchild.node_type {
                        info!(index = j, tag = %tag_name, "DOM: grandchild of root");
                    }
                }
            } else if let NodeType::DocumentType { name, .. } = &child.node_type {
                info!(index = i, name = %name, "DOM: root child (doctype)");
            }
        }

        // Get the body element and build layout from it
        if let Some(body) = document.body() {
            // Debug: count body's children
            let body_children = body.children();
            info!(
                body_children = body_children.len(),
                "DOM: body element found"
            );
            
            // Debug: print first few children tags
            for (i, child) in body_children.iter().take(5).enumerate() {
                if let NodeType::Element { tag_name, .. } = &child.node_type {
                    info!(index = i, tag = %tag_name, "DOM: body child");
                } else if let NodeType::Text(text) = &child.node_type {
                    let preview: String = text.chars().take(30).collect();
                    info!(index = i, text = %preview, "DOM: body child (text)");
                }
            }
            
            let mut body_box = self.build_layout_from_node(&body, &sheet, &root_inherited, &[]);
            info!(
                layout_children = body_box.children.len(),
                "Layout: body box built"
            );
            // CSS §14.2 canvas background: the body's background propagates to the
            // canvas (the whole viewport), not just the body box. Without this a
            // short page paints its background only behind its content and leaves
            // the rest of the viewport the canvas default (white). Move it onto
            // the root/canvas box and clear it from the body so it paints once.
            if body_box.style.background_color.a > 0.0
                || body_box.style.background_gradient.is_some()
                || body_box.style.background_radial_gradient.is_some()
            {
                root_box.style.background_color = body_box.style.background_color;
                root_box.style.background_gradient = body_box.style.background_gradient.clone();
                root_box.style.background_radial_gradient =
                    body_box.style.background_radial_gradient.clone();
                body_box.style.background_color = rustkit_css::Color::TRANSPARENT;
                body_box.style.background_gradient = None;
                body_box.style.background_radial_gradient = None;
            }
            root_box.children.push(body_box);
        } else if let Some(html) = document.document_element() {
            // Fallback: use html element if no body
            info!("DOM: no body found, using html element");
            // Debug: print html's children
            let html_children = html.children();
            info!(html_children = html_children.len(), "DOM: html element children");
            for (i, child) in html_children.iter().take(5).enumerate() {
                if let NodeType::Element { tag_name, .. } = &child.node_type {
                    info!(index = i, tag = %tag_name, "DOM: html child");
                }
            }
            let html_box = self.build_layout_from_node(&html, &sheet, &root_inherited, &[]);
            root_box.children.push(html_box);
        } else {
            warn!("DOM: no body or html element found");
        }

        root_box
    }

    /// Build a layout box from a DOM node.
    fn build_layout_from_node(
        &self,
        node: &Rc<Node>,
        sheet: &Stylesheet,
        parent: &ComputedStyle,
        ancestors: &[ElementCtx],
    ) -> LayoutBox {
        match &node.node_type {
            NodeType::Element { tag_name, attributes, .. } => {
                // Determine box type based on tag
                let is_inline = matches!(
                    tag_name.to_lowercase().as_str(),
                    "a" | "span" | "strong" | "b" | "em" | "i" | "u" | "code" | "small" | "big" | "sub" | "sup" | "abbr" | "cite" | "q" | "mark" | "label"
                );

                // Skip rendering for certain elements
                let is_hidden = matches!(
                    tag_name.to_lowercase().as_str(),
                    "head" | "title" | "meta" | "link" | "script" | "style" | "noscript"
                );

                if is_hidden {
                    // Return an empty block for hidden elements
                    return LayoutBox::new(BoxType::Block, ComputedStyle::new());
                }

                let box_type = if is_inline {
                    BoxType::Inline
                } else {
                    BoxType::Block
                };

                // Create computed style: inheritance + UA defaults + author
                // stylesheet (selector-matched) + inline.
                let style =
                    self.compute_style_for_element(tag_name, attributes, sheet, parent, ancestors);

                let mut layout_box = LayoutBox::new(box_type, style);

                // Feed the positioned-layout path. Without this the layout
                // crate's Position::Absolute / Fixed branches are unreachable
                // no matter what the author wrote.
                // rustkit_css::Position and rustkit_layout::Position are two
                // separate enums with identical variants and no From impl
                // between them (flagged as a duplication smell; not merged
                // here). Matched EXHAUSTIVELY on purpose: adding a variant to
                // either enum then breaks this build instead of silently
                // mapping to Static.
                layout_box.position = match layout_box.style.position {
                    rustkit_css::Position::Static => rustkit_layout::Position::Static,
                    rustkit_css::Position::Relative => rustkit_layout::Position::Relative,
                    rustkit_css::Position::Absolute => rustkit_layout::Position::Absolute,
                    rustkit_css::Position::Fixed => rustkit_layout::Position::Fixed,
                    rustkit_css::Position::Sticky => rustkit_layout::Position::Sticky,
                };
                layout_box.z_index = layout_box.style.z_index;
                {
                    // Resolve offsets to px. Percentages resolve against the
                    // CONTAINING BLOCK, which is not known while the tree is
                    // still being built, so a percentage offset yields None
                    // (treated as `auto`) rather than a wrong number. Same
                    // restriction as the macOS reference; stated rather than
                    // silently approximated.
                    let fs = match &layout_box.style.font_size {
                        rustkit_css::Length::Px(v) => *v,
                        _ => 16.0,
                    };
                    let px = |l: &Option<rustkit_css::Length>| -> Option<f32> {
                        match l {
                            Some(rustkit_css::Length::Px(v)) => Some(*v),
                            Some(rustkit_css::Length::Em(v)) => Some(v * fs),
                            Some(rustkit_css::Length::Rem(v)) => Some(v * 16.0),
                            _ => None,
                        }
                    };
                    let (t, r, b, l) = (
                        px(&layout_box.style.top),
                        px(&layout_box.style.right),
                        px(&layout_box.style.bottom),
                        px(&layout_box.style.left),
                    );
                    layout_box.set_offsets(t, r, b, l);
                }

                // Get DOM children for processing
                let dom_children = node.children();
                trace!(tag = %tag_name, dom_children = dom_children.len(), "Processing element");

                // Extend the ancestor chain with this element so descendant
                // selectors evaluated on our children can see us.
                let mut child_ancestors: Vec<ElementCtx> = Vec::with_capacity(ancestors.len() + 1);
                for a in ancestors {
                    child_ancestors.push(ElementCtx {
                        tag: a.tag.clone(),
                        classes: a.classes.clone(),
                        id: a.id.clone(),
                    });
                }
                child_ancestors.push(ElementCtx {
                    tag: tag_name.to_string(),
                    classes: attributes
                        .get("class")
                        .map(|c| c.split_whitespace().map(|s| s.to_string()).collect())
                        .unwrap_or_default(),
                    id: attributes.get("id").cloned(),
                });

                // Process children, inheriting from this element's computed style.
                for child in dom_children {
                    // Whitespace-only text between elements (e.g. the newlines
                    // between sibling <div>s) must not generate a box. Otherwise
                    // each gap becomes an empty anonymous block that a flex
                    // container counts as a phantom flex item — stealing width
                    // from the real items and blowing out the gaps.
                    if let NodeType::Text(ref t) = child.node_type {
                        if t.trim().is_empty() {
                            continue;
                        }
                    }
                    let child_box =
                        self.build_layout_from_node(&child, sheet, &layout_box.style, &child_ancestors);
                    layout_box.children.push(child_box);
                }

                // Basic list marker: prepend a disc bullet to the first text run
                // of an <li> so lists render with markers (the <ul>/<ol> UA
                // padding-left already provides the indent).
                if tag_name.eq_ignore_ascii_case("li") {
                    for child in layout_box.children.iter_mut() {
                        if let BoxType::Text(ref mut t) = child.box_type {
                            *t = format!("\u{2022}  {t}");
                            break;
                        }
                    }
                }

                // <input>/<textarea> are void of DOM text; surface the `value`
                // (or `placeholder`) as a text child so the field shows content.
                // Password fields are masked with bullets.
                if matches!(
                    tag_name.to_lowercase().as_str(),
                    "input" | "textarea"
                ) {
                    let value = attributes
                        .get("value")
                        .filter(|v| !v.is_empty())
                        .or_else(|| attributes.get("placeholder"))
                        .cloned()
                        .unwrap_or_default();
                    if !value.is_empty() {
                        let is_password = attributes
                            .get("type")
                            .map(|t| t.eq_ignore_ascii_case("password"))
                            .unwrap_or(false);
                        let shown = if is_password {
                            "\u{2022}".repeat(value.chars().count())
                        } else {
                            value
                        };
                        let text_style = ComputedStyle::inherit_from(&layout_box.style);
                        layout_box
                            .children
                            .push(LayoutBox::new(BoxType::Text(shown), text_style));
                    }
                }

                layout_box
            }
            NodeType::Text(text) => {
                // Create text box for non-empty text. Text inherits its parent's
                // font/color so headings, colored spans, etc. reach the glyphs
                // (a fresh default here rendered every run at 16px black).
                let trimmed = text.trim();
                if trimmed.is_empty() {
                    LayoutBox::new(BoxType::Block, ComputedStyle::inherit_from(parent))
                } else {
                    let mut style = ComputedStyle::inherit_from(parent);
                    // background-clip:text fills the glyphs with the element's
                    // background instead of the box. Neither clip nor gradient
                    // inherits, so carry them onto this text run explicitly (the
                    // gradient-text effect, e.g. the HIWAVE logo).
                    if parent.background_clip == rustkit_css::BackgroundClip::Text {
                        style.background_clip = rustkit_css::BackgroundClip::Text;
                        style.background_gradient = parent.background_gradient.clone();
                    }
                    // text-decoration does NOT inherit - and that is correct,
                    // which is exactly why it needs handling here. CSS Text
                    // Decoration distinguishes INHERITANCE from PROPAGATION: the
                    // line is drawn across in-flow descendants of the element
                    // that declared it. `inherit_from` re-initialises these four
                    // fields (correctly), and the display list reads them off
                    // the TEXT box, so without this an underlined <p> computed
                    // its decoration and painted nothing - the property was
                    // writable but still unreachable.
                    if parent.text_decoration_line.underline
                        || parent.text_decoration_line.overline
                        || parent.text_decoration_line.line_through
                    {
                        style.text_decoration_line = parent.text_decoration_line;
                        style.text_decoration_color = parent.text_decoration_color;
                        style.text_decoration_style = parent.text_decoration_style;
                        style.text_decoration_thickness =
                            parent.text_decoration_thickness.clone();
                    }
                    LayoutBox::new(BoxType::Text(trimmed.to_string()), style)
                }
            }
            _ => {
                // For other node types (Document, Comment, etc.), return empty box
                LayoutBox::new(BoxType::Block, ComputedStyle::inherit_from(parent))
            }
        }
    }

    /// Gather custom properties (`--x: value`) defined by selectors that apply
    /// to the document root — `:root`, `html`, or `*` — into a base map that
    /// seeds inheritance for the whole tree. Element-scoped custom properties
    /// are layered on later, per element, in `compute_style_for_element`.
    fn collect_root_custom_properties(sheet: &Stylesheet) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = HashMap::new();
        for rule in &sheet.rules {
            let applies = rule.selector.split(',').any(|s| {
                let s = s.trim();
                s == ":root" || s == "html" || s == "*"
            });
            if !applies {
                continue;
            }
            for decl in &rule.declarations {
                if decl.property.starts_with("--") {
                    if let PropertyValue::Specified(v) = &decl.value {
                        let resolved = resolve_var_refs(v, &map);
                        map.insert(decl.property.clone(), resolved);
                    }
                }
            }
        }
        map
    }

    /// Compute a basic style for an element based on its tag and attributes.
    fn compute_style_for_element(
        &self,
        tag_name: &str,
        attributes: &std::collections::HashMap<String, String>,
        sheet: &Stylesheet,
        parent: &ComputedStyle,
        ancestors: &[ElementCtx],
    ) -> ComputedStyle {
        // 1. Start from inherited properties (color, font-*, line-height, ...).
        let mut style = ComputedStyle::inherit_from(parent);

        // 2. Apply tag-specific user-agent default styles
        match tag_name.to_lowercase().as_str() {
            "body" => {
                style.background_color = rustkit_css::Color::WHITE;
                style.margin_top = rustkit_css::Length::Px(8.0);
                style.margin_right = rustkit_css::Length::Px(8.0);
                style.margin_bottom = rustkit_css::Length::Px(8.0);
                style.margin_left = rustkit_css::Length::Px(8.0);
            }
            "h1" => {
                style.font_size = rustkit_css::Length::Px(32.0);
                style.font_weight = rustkit_css::FontWeight::BOLD;
                style.margin_top = rustkit_css::Length::Px(21.44);
                style.margin_bottom = rustkit_css::Length::Px(21.44);
            }
            "h2" => {
                style.font_size = rustkit_css::Length::Px(24.0);
                style.font_weight = rustkit_css::FontWeight::BOLD;
                style.margin_top = rustkit_css::Length::Px(19.92);
                style.margin_bottom = rustkit_css::Length::Px(19.92);
            }
            "h3" => {
                style.font_size = rustkit_css::Length::Px(18.72);
                style.font_weight = rustkit_css::FontWeight::BOLD;
                style.margin_top = rustkit_css::Length::Px(18.72);
                style.margin_bottom = rustkit_css::Length::Px(18.72);
            }
            // h4-h6 had NO UA defaults on this tree at all: an <h4> rendered
            // at body size, unbolded, with no margins. Values are the
            // reference's, which are the Chrome UA sheet's em ratios resolved
            // against a 16px root (1em / 0.83em / 0.67em).
            "h4" => {
                style.font_size = rustkit_css::Length::Px(16.0); // 1em
                style.font_weight = rustkit_css::FontWeight::BOLD;
                style.margin_top = rustkit_css::Length::Px(21.28); // 1.33em
                style.margin_bottom = rustkit_css::Length::Px(21.28);
            }
            "h5" => {
                style.font_size = rustkit_css::Length::Px(13.28); // 0.83em
                style.font_weight = rustkit_css::FontWeight::BOLD;
                style.margin_top = rustkit_css::Length::Px(22.17); // 1.67em
                style.margin_bottom = rustkit_css::Length::Px(22.17);
            }
            "h6" => {
                style.font_size = rustkit_css::Length::Px(10.72); // 0.67em
                style.font_weight = rustkit_css::FontWeight::BOLD;
                style.margin_top = rustkit_css::Length::Px(25.0); // 2.33em
                style.margin_bottom = rustkit_css::Length::Px(25.0);
            }
            "p" => {
                style.margin_top = rustkit_css::Length::Px(16.0);
                style.margin_bottom = rustkit_css::Length::Px(16.0);
            }
            "div" => {
                // Block element with no special styling
            }
            "a" => {
                style.color = rustkit_css::Color::new(0, 0, 238, 1.0); // Blue
            }
            "strong" | "b" => {
                style.font_weight = rustkit_css::FontWeight::BOLD;
            }
            "em" | "i" => {
                style.font_style = rustkit_css::FontStyle::Italic;
            }
            "pre" | "code" => {
                style.font_family = "monospace".to_string();
            }
            "ul" | "ol" => {
                style.margin_top = rustkit_css::Length::Px(16.0);
                style.margin_bottom = rustkit_css::Length::Px(16.0);
                style.padding_left = rustkit_css::Length::Px(40.0);
            }
            "li" => {
                // List items are blocks
            }
            "blockquote" => {
                style.margin_top = rustkit_css::Length::Px(16.0);
                style.margin_bottom = rustkit_css::Length::Px(16.0);
                style.margin_left = rustkit_css::Length::Px(40.0);
                style.margin_right = rustkit_css::Length::Px(40.0);
            }
            "hr" => {
                style.border_top_width = rustkit_css::Length::Px(1.0);
                style.border_top_color = rustkit_css::Color::new(128, 128, 128, 1.0);
                style.margin_top = rustkit_css::Length::Px(8.0);
                style.margin_bottom = rustkit_css::Length::Px(8.0);
            }
            // Scoped table support: model rows as flex rows and cells as
            // equal-growing flex items, so tables lay out as a grid via the
            // existing flex engine (full table sizing is a follow-up).
            "tr" => {
                style.display = rustkit_css::Display::Flex;
                // Don't stretch cells to the row's ambient cross height; let the
                // row hug its tallest cell's content.
                style.align_items = rustkit_css::AlignItems::FlexStart;
            }
            "td" => {
                style.flex_grow = 1.0;
                style.flex_basis = rustkit_css::FlexBasis::Length(0.0);
            }
            "th" => {
                style.flex_grow = 1.0;
                style.flex_basis = rustkit_css::FlexBasis::Length(0.0);
                // A header cell is BOLD. td and th shared one arm here, so
                // every <th> rendered at normal weight - the reference bolds
                // it, so this is a defect rather than a deliberate difference.
                style.font_weight = rustkit_css::FontWeight::BOLD;
            }
            // Form controls do NOT inherit the document font in Chrome's UA
            // sheet — they reset to the system control font (~13.333px, normal
            // weight/style). Inheriting the page font makes every unstyled
            // control the wrong size, sliding whole sections off (Atlas macOS
            // #42: this was the real css-selectors residual, not box-model
            // compose alone).
            "button" | "input" | "select" | "textarea" => {
                // Form controls are replaced-ish: they lay out as one atomic
                // inline-block, so siblings share a line instead of stacking
                // (css-selectors §6; macOS #55). Author `display` still wins in
                // the cascade below.
                style.display = rustkit_css::Display::InlineBlock;
                style.font_size = rustkit_css::Length::Px(13.333);
                style.font_family = "sans-serif".to_string();
                style.font_weight = rustkit_css::FontWeight::NORMAL;
                style.font_style = rustkit_css::FontStyle::Normal;

                // W55-B: UA intrinsic border-box sizes calibrated to Chrome
                // CfT-148 at the 13.333px control font (macOS #55). Windows
                // lacks the macOS FormControlType/layout_form_control
                // subsystem, so the oracle sizes are applied as UA
                // width/height here; author width/height still win in the
                // cascade below, and the inline layout honors these for
                // inline-block boxes (a bare control has no content to size).
                let tag = tag_name.to_lowercase();
                let itype = attributes.get("type").map(|s| s.to_lowercase());
                match tag.as_str() {
                    "input" => match itype.as_deref() {
                        Some("hidden") => {}
                        Some("checkbox") | Some("radio") => {
                            style.width = rustkit_css::Length::Px(13.0);
                            style.height = rustkit_css::Length::Px(13.0);
                        }
                        Some("range") => {
                            style.width = rustkit_css::Length::Px(129.0);
                            style.height = rustkit_css::Length::Px(16.0);
                        }
                        Some("color") => {
                            style.width = rustkit_css::Length::Px(50.0);
                            style.height = rustkit_css::Length::Px(27.0);
                        }
                        _ => {
                            style.width = rustkit_css::Length::Px(160.0);
                            style.height = rustkit_css::Length::Px(19.0);
                        }
                    },
                    "textarea" => {
                        // Default rows=2 / cols=20 (HTML), but an author-set
                        // rows=1 must stay one row — no min-floor above the
                        // default (Prometheus review nit on #27).
                        let rows = attributes
                            .get("rows")
                            .and_then(|r| r.trim().parse::<f32>().ok())
                            .filter(|&r| r >= 1.0)
                            .unwrap_or(2.0);
                        let cols = attributes
                            .get("cols")
                            .and_then(|c| c.trim().parse::<f32>().ok())
                            .filter(|&c| c >= 1.0)
                            .unwrap_or(20.0);
                        style.width = rustkit_css::Length::Px(13.333 * 0.6 * cols);
                        style.height = rustkit_css::Length::Px(15.0 * rows + 2.0);
                    }
                    "select" => {
                        let size = attributes
                            .get("size")
                            .and_then(|s| s.trim().parse::<u32>().ok())
                            .unwrap_or(0);
                        let size = if attributes.contains_key("multiple") && size == 0 {
                            4
                        } else {
                            size
                        };
                        style.width = rustkit_css::Length::Px(133.0);
                        style.height = if size > 1 {
                            rustkit_css::Length::Px(16.0 * size as f32 + 2.0)
                        } else {
                            rustkit_css::Length::Px(19.0)
                        };
                    }
                    "button" => {
                        // Width hugs the label (content); pin the line height.
                        style.height = rustkit_css::Length::Px(19.0);
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        // 3. Author stylesheet rules, selector-matched, applied by
        //    (specificity, source order) so later/more-specific rules win.
        let classes: Vec<&str> = attributes
            .get("class")
            .map(|c| c.split_whitespace().collect())
            .unwrap_or_default();
        let id = attributes.get("id").map(|s| s.as_str());
        let mut matched: Vec<(u32, usize)> = Vec::new();
        for (i, rule) in sheet.rules.iter().enumerate() {
            if let Some(spec) =
                Self::selector_matches(&rule.selector, tag_name, &classes, id, ancestors)
            {
                matched.push((spec, i));
            }
        }
        matched.sort_by_key(|&(spec, i)| (spec, i));

        // 3a. Collect this element's own custom properties (`--x`) first, in
        //     cascade order over the inherited set, so var() lookups in this
        //     element's other declarations (and in descendants) see them.
        for &(_, i) in &matched {
            for decl in &sheet.rules[i].declarations {
                if decl.property.starts_with("--") {
                    if let PropertyValue::Specified(v) = &decl.value {
                        let resolved = resolve_var_refs(v, &style.custom_properties);
                        Arc::make_mut(&mut style.custom_properties)
                            .insert(decl.property.clone(), resolved);
                    }
                }
            }
        }

        // 3b. Apply normal declarations, resolving any var(--x) references.
        for (_, i) in matched {
            for decl in &sheet.rules[i].declarations {
                if decl.property.starts_with("--") {
                    continue;
                }
                if let PropertyValue::Specified(v) = &decl.value {
                    if v.contains("var(") {
                        let resolved = resolve_var_refs(v, &style.custom_properties);
                        Self::apply_declaration(&mut style, &decl.property, &resolved);
                    } else {
                        Self::apply_declaration(&mut style, &decl.property, v);
                    }
                }
            }
        }

        // 4. Inline style attribute (highest priority).
        if let Some(style_attr) = attributes.get("style") {
            Self::apply_inline_style(&mut style, style_attr);
        }

        style
    }

    /// Test whether a CSS selector matches an element, returning its
    /// specificity if so. Supports comma groups, `*`, type, `.class`, `#id`,
    /// and compounds (`div.foo`, `h1#bar`). Descendant selectors (`.card p`)
    /// are matched against the ancestor chain: the subject (rightmost compound)
    /// must match the element, and every ancestor compound must match some
    /// ancestor, in order, walking outward. Specificity is the sum of all
    /// compounds. The child combinator `>` is treated as descendant (a mild
    /// over-match, but far closer than ignoring ancestors entirely).
    fn selector_matches(
        selector: &str,
        tag: &str,
        classes: &[&str],
        id: Option<&str>,
        ancestors: &[ElementCtx],
    ) -> Option<u32> {
        Self::selector_matches_inner(selector, tag, classes, id, ancestors)
    }

    /// Split one selector group into compound selectors, recording which
    /// boundaries are `>` child combinators.
    ///
    /// Returns `(compounds, child_of_next)` where `child_of_next[i]` is true
    /// when `compounds[i]` must be the IMMEDIATE PARENT of `compounds[i + 1]`.
    /// `child_of_next` always has the same length as `compounds`; the entry for
    /// the subject (the last compound) is meaningless and is always false.
    ///
    /// Splits on whitespace AND on `>`, so `.nav>li`, `.nav > li` and
    /// `.nav >li` all tokenise identically - authors write all three.
    ///
    /// Returns `None` for a group with a leading or trailing `>`, which cannot
    /// mean anything. Refusing beats guessing, but note the failure direction:
    /// over-refusing silently drops rules that should have applied, so this
    /// returns None ONLY for those two shapes and never for an ordinary
    /// selector.
    fn split_selector_group(group: &str) -> Option<(Vec<String>, Vec<bool>)> {
        let mut compounds: Vec<String> = Vec::new();
        let mut child_of_next: Vec<bool> = Vec::new();
        let mut cur = String::new();

        for ch in group.chars() {
            if ch == '>' {
                if !cur.is_empty() {
                    compounds.push(std::mem::take(&mut cur));
                    child_of_next.push(false);
                }
                // The compound to the LEFT of this `>` must be the immediate
                // parent of the next one. A `>` with nothing to its left is
                // malformed.
                match child_of_next.last_mut() {
                    Some(flag) => *flag = true,
                    None => return None,
                }
            } else if ch.is_whitespace() {
                if !cur.is_empty() {
                    compounds.push(std::mem::take(&mut cur));
                    child_of_next.push(false);
                }
            } else {
                cur.push(ch);
            }
        }
        if !cur.is_empty() {
            compounds.push(cur);
            child_of_next.push(false);
        }

        // A trailing `>` leaves the child flag set on the SUBJECT, which has
        // nothing to its right.
        if child_of_next.last().copied().unwrap_or(false) {
            return None;
        }
        if compounds.is_empty() {
            return None;
        }
        Some((compounds, child_of_next))
    }

    fn selector_matches_inner(
        selector: &str,
        tag: &str,
        classes: &[&str],
        id: Option<&str>,
        ancestors: &[ElementCtx],
    ) -> Option<u32> {
        let mut best: Option<u32> = None;
        for group in selector.split(',') {
            let group = group.trim();
            if group.is_empty() {
                continue;
            }
            // Split into compound selectors, KEEPING the `>` child combinator.
            //
            // The previous version dropped `>` tokens, which was wrong in both
            // directions: `.nav > li` silently became `.nav li` (over-match -
            // one menu level's rule also styled every nested submenu item), and
            // `.nav>li` never split at all, so the compound's type part was the
            // literal string "nav>li", matched no tag, and the whole rule was
            // silently DEAD. Both spellings are ordinary authoring style.
            let Some((compounds, child_of_next)) = Self::split_selector_group(group) else {
                // Malformed (leading or trailing `>`). Refuse the group rather
                // than guessing at what the author meant.
                continue;
            };
            let Some((subject, ancestor_sels)) = compounds.split_last() else {
                continue;
            };
            // Subject must match the element itself.
            let Some(subject_spec) = Self::simple_selector_match(subject, tag, classes, id) else {
                continue;
            };
            // Each ancestor compound (right-to-left) must match some ancestor,
            // searching from nearest to farthest and consuming as we go.
            let mut spec = subject_spec;
            let mut idx = ancestors.len();
            let mut matched_all = true;
            for (k, sel) in ancestor_sels.iter().enumerate().rev() {
                let a_classes_of = |a: &ElementCtx| -> Vec<String> { a.classes.clone() };
                if child_of_next[k] {
                    // `>`: this compound must be the IMMEDIATE PARENT of
                    // whatever matched to its right. Consume exactly one
                    // ancestor and require it to match - no searching upward.
                    if idx == 0 {
                        matched_all = false;
                        break;
                    }
                    idx -= 1;
                    let a = &ancestors[idx];
                    let owned = a_classes_of(a);
                    let a_classes: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
                    match Self::simple_selector_match(sel, &a.tag, &a_classes, a.id.as_deref()) {
                        Some(s) => spec += s,
                        None => {
                            matched_all = false;
                            break;
                        }
                    }
                    continue;
                }
                // Descendant: search from nearest to farthest, consuming as we
                // go. Greedy with no backtracking - `.a > .b .c` can
                // false-NEGATIVE where an outer candidate would have matched.
                // The macOS reference has the SAME cursor, so this is left
                // alone deliberately: an out-of-band better matcher here would
                // be a divergence and would make parity comparison meaningless.
                // Talos raised it as a shared cross-tree item for Prometheus.
                let mut found = false;
                while idx > 0 {
                    idx -= 1;
                    let a = &ancestors[idx];
                    let a_classes: Vec<&str> = a.classes.iter().map(|s| s.as_str()).collect();
                    if let Some(s) =
                        Self::simple_selector_match(sel, &a.tag, &a_classes, a.id.as_deref())
                    {
                        spec += s;
                        found = true;
                        break;
                    }
                }
                if !found {
                    matched_all = false;
                    break;
                }
            }
            if matched_all {
                best = Some(best.map_or(spec, |b| b.max(spec)));
            }
        }
        best
    }

    /// Match a single compound selector (no combinators) like `div.foo#bar`.
    fn simple_selector_match(
        sel: &str,
        tag: &str,
        classes: &[&str],
        id: Option<&str>,
    ) -> Option<u32> {
        let mut spec = 0u32;
        // Leading type selector (up to the first '.' or '#').
        let first_special = sel.find(['.', '#']).unwrap_or(sel.len());
        let type_sel = &sel[..first_special];
        if !type_sel.is_empty() && type_sel != "*" {
            if !type_sel.eq_ignore_ascii_case(tag) {
                return None;
            }
            spec += 1;
        }
        // Remaining `.class` / `#id` segments.
        let rest = &sel[first_special..];
        let bytes = rest.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let kind = bytes[i];
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j] != b'.' && bytes[j] != b'#' {
                j += 1;
            }
            let name = &rest[start..j];
            if name.is_empty() {
                return None;
            }
            match kind {
                b'.' => {
                    if !classes.iter().any(|c| *c == name) {
                        return None;
                    }
                    spec += 10;
                }
                b'#' => {
                    if id != Some(name) {
                        return None;
                    }
                    spec += 100;
                }
                _ => return None,
            }
            i = j;
        }
        Some(spec)
    }

    /// Discover every `<link rel="stylesheet">` href in the document, resolved
    /// against the document's own URL.
    ///
    /// Takes no `self` - it is a pure function of the document, so the tests
    /// exercise the real discovery path without constructing an Engine (and
    /// therefore without a GPU adapter). Same reasoning as #52.
    ///
    /// Relative hrefs need `base_url`; with no base, only absolute hrefs
    /// resolve. An href that will not parse is SKIPPED, not guessed at.
    fn discover_external_stylesheets(document: &Document, base_url: Option<&Url>) -> Vec<Url> {
        let mut urls = Vec::new();
        for link in document.get_elements_by_tag_name("link") {
            // `rel` is an unordered set in HTML ("stylesheet", "alternate
            // stylesheet"); match any token rather than the whole attribute,
            // and case-insensitively - `REL="StyleSheet"` is legal.
            let is_stylesheet = link
                .get_attribute("rel")
                .map(|rel| {
                    rel.split_whitespace()
                        .any(|t| t.eq_ignore_ascii_case("stylesheet"))
                })
                .unwrap_or(false);
            if !is_stylesheet {
                continue;
            }
            let Some(href) = link.get_attribute("href") else {
                continue;
            };
            if href.trim().is_empty() {
                continue;
            }
            let resolved = match base_url {
                Some(base) => base.join(href).ok(),
                None => Url::parse(href).ok(),
            };
            match resolved {
                Some(url) => urls.push(url),
                None => warn!(href, "stylesheet href did not resolve; skipping"),
            }
        }
        urls
    }

    /// Discover every `<img src>` in the document, resolved against the
    /// document's own URL.
    ///
    /// Returns `(src_attribute, resolved_url)` — the raw attribute is kept
    /// because the renderer identifies an image box by the `src` it was
    /// authored with, not by the resolved URL.
    ///
    /// Takes no `self`, same reasoning as `discover_external_stylesheets`:
    /// tests exercise the real path without building an Engine.
    ///
    /// `srcset` / `<picture>` are NOT handled — only the plain `src`. Stated
    /// rather than left to be discovered: a responsive-image page will load
    /// its fallback here, not its best candidate.
    fn discover_images(document: &Document, base_url: Option<&Url>) -> Vec<(String, Url)> {
        let mut images = Vec::new();
        for img in document.get_elements_by_tag_name("img") {
            let Some(src) = img.get_attribute("src") else {
                continue;
            };
            if src.trim().is_empty() {
                continue;
            }
            let resolved = match base_url {
                Some(base) => base.join(src).ok(),
                None => Url::parse(src).ok(),
            };
            match resolved {
                Some(url) => images.push((src.to_string(), url)),
                None => warn!(src, "image src did not resolve; skipping"),
            }
        }
        images
    }

    /// Fetch every `<img>` in the document into the image cache.
    ///
    /// Delegates each URL to `load_image`, which already fetches, decodes,
    /// caches and emits ImageLoaded / ImageError. Reusing it rather than
    /// reimplementing the loop means there is exactly ONE image-load path to
    /// keep correct.
    ///
    /// FAIL-SOFT PER IMAGE: a broken `<img>` must not fail the navigation —
    /// that is what a real browser does, and it is what `alt` text is for.
    /// Every failure is logged with its URL. Returns how many are in cache.
    async fn load_document_images(
        &self,
        id: EngineViewId,
        document: &Document,
        base_url: &Url,
    ) -> usize {
        let images = Self::discover_images(document, Some(base_url));
        if images.is_empty() {
            return 0;
        }
        let mut loaded = 0usize;
        for (_src, url) in images {
            if self.image_manager.is_cached(&url) {
                loaded += 1;
                continue;
            }
            match self.load_image(id, url.clone()).await {
                Ok(()) => loaded += 1,
                // load_image has already emitted ImageError; this is the
                // operator-visible half. A silently missing image looks
                // identical to a page that was authored without one.
                Err(e) => warn!(%url, error = %e, "image failed to load"),
            }
        }
        info!(?id, loaded, "document images loaded");
        loaded
    }

    /// Recursively gather the text content of every `<style>` element.
    fn collect_style_text(&self, node: &Rc<Node>, out: &mut String) {
        if let NodeType::Element { tag_name, .. } = &node.node_type {
            if tag_name.eq_ignore_ascii_case("style") {
                for child in node.children() {
                    if let NodeType::Text(t) = &child.node_type {
                        out.push_str(t);
                        out.push('\n');
                    }
                }
                return;
            }
        }
        for child in node.children() {
            self.collect_style_text(&child, out);
        }
    }

    /// Apply inline style attribute to computed style.
    ///
    /// Takes no `self`: the cascade never needed the Engine, so tests can
    /// exercise the real path without constructing one (and therefore without
    /// initialising a GPU adapter). See `apply_declaration` below.
    fn apply_inline_style(style: &mut ComputedStyle, style_attr: &str) {
        for declaration in style_attr.split(';') {
            let declaration = declaration.trim();
            if declaration.is_empty() {
                continue;
            }
            if let Some((property, value)) = declaration.split_once(':') {
                let property = property.trim();
                let value = value.trim();
                if property.starts_with("--") {
                    let resolved = resolve_var_refs(value, &style.custom_properties);
                    Arc::make_mut(&mut style.custom_properties)
                        .insert(property.to_string(), resolved);
                    continue;
                }
                if value.contains("var(") {
                    let resolved = resolve_var_refs(value, &style.custom_properties);
                    Self::apply_declaration(style, property, &resolved);
                } else {
                    Self::apply_declaration(style, property, value);
                }
            }
        }
    }

    /// Apply a single `property: value` declaration to a computed style.
    /// Shared by the inline-style and author-stylesheet paths.
    ///
    /// Takes no `self`. The body never read a field - the only `self` uses
    /// were calls to `apply_box_shorthand`, which itself took `&self` and
    /// never touched it. Dropping the receiver lets the cascade wire tests
    /// call the REAL production path directly instead of building an Engine
    /// (and with it a `Compositor`, i.e. a wgpu adapter) per test. That
    /// concurrent adapter init is what SIGSEGVd on Linux (hiwave-linux #21).
    fn apply_declaration(style: &mut ComputedStyle, property: &str, value: &str) {
        let property = property.to_lowercase();
        let value = value.trim();
        match property.as_str() {
            // Transform family WIRE (engine Slice-1 / Cluster A). The types
            // landed INERT in #36; these arms are what make the properties
            // compute. Renderer does not consume style.transform yet - see the
            // PR body's dead-code note.
            "transform" => {
                if let Some(list) = parse_transform(value) {
                    style.transform = list;
                }
            }
            "transform-origin" => {
                if let Some(origin) = parse_transform_origin(value) {
                    style.transform_origin = origin;
                }
            }
            // Shadow/Filter family WIRE (Cluster A2). BoxShadow landed INERT
            // in #37. `none` clears the list rather than pushing nothing, so a
            // later rule can cancel an earlier shadow - otherwise
            // `box-shadow: none` would silently leave the inherited-cascade
            // value in place.
            "box-shadow" => {
                if value.trim() == "none" {
                    style.box_shadows.clear();
                } else if let Some(shadow) = parse_box_shadow(value) {
                    style.box_shadows.push(shadow);
                }
            }
            // Animation/transition family WIRE (Cluster A3). Enums landed
            // INERT in #40. PARSED, NOT EXECUTED - nothing animates as a
            // result; the values simply survive the cascade.
            "transition-property" => {
                style.transition_property = value.trim().to_string();
            }
            "transition-duration" => {
                if let Some(dur) = parse_time(value) {
                    style.transition_duration = dur;
                }
            }
            "transition-timing-function" => {
                style.transition_timing_function = parse_timing_function(value);
            }
            "transition-delay" => {
                if let Some(delay) = parse_time(value) {
                    style.transition_delay = delay;
                }
            }
            "animation-name" => {
                style.animation_name = value.trim().to_string();
            }
            "animation-duration" => {
                if let Some(dur) = parse_time(value) {
                    style.animation_duration = dur;
                }
            }
            "animation-timing-function" => {
                style.animation_timing_function = parse_timing_function(value);
            }
            "animation-delay" => {
                if let Some(delay) = parse_time(value) {
                    style.animation_delay = delay;
                }
            }
            "animation-iteration-count" => {
                let v = value.trim();
                if v == "infinite" {
                    style.animation_iteration_count =
                        rustkit_css::AnimationIterationCount::Infinite;
                } else if let Ok(n) = v.parse::<f32>() {
                    style.animation_iteration_count =
                        rustkit_css::AnimationIterationCount::Count(n);
                }
            }
            "animation-direction" => {
                style.animation_direction = match value.trim() {
                    "normal" => rustkit_css::AnimationDirection::Normal,
                    "reverse" => rustkit_css::AnimationDirection::Reverse,
                    "alternate" => rustkit_css::AnimationDirection::Alternate,
                    "alternate-reverse" => rustkit_css::AnimationDirection::AlternateReverse,
                    _ => rustkit_css::AnimationDirection::Normal,
                };
            }
            "animation-fill-mode" => {
                style.animation_fill_mode = match value.trim() {
                    "none" => rustkit_css::AnimationFillMode::None,
                    "forwards" => rustkit_css::AnimationFillMode::Forwards,
                    "backwards" => rustkit_css::AnimationFillMode::Backwards,
                    "both" => rustkit_css::AnimationFillMode::Both,
                    _ => rustkit_css::AnimationFillMode::None,
                };
            }
            "animation-play-state" => {
                style.animation_play_state = match value.trim() {
                    "running" => rustkit_css::AnimationPlayState::Running,
                    "paused" => rustkit_css::AnimationPlayState::Paused,
                    _ => rustkit_css::AnimationPlayState::Running,
                };
            }
            "color" => {
                if let Some(c) = parse_color(value) {
                    style.color = c;
                }
            }
            "background-color" | "background" => {
                if property == "background" {
                    // Shorthand: capture a linear-gradient layer (painted over
                    // the base) and the solid base color separately, so a
                    // gradient's first stop no longer masquerades as the base.
                    if value.contains("linear-gradient(") {
                        style.background_gradient = parse_linear_gradient(value);
                    }
                    if value.contains("radial-gradient(") {
                        style.background_radial_gradient = parse_radial_gradient(value);
                    }
                    if let Some(c) = background_base_color(value) {
                        style.background_color = c;
                    }
                } else if let Some(c) = parse_color(value) {
                    style.background_color = c;
                }
            }
            "background-image" => {
                if value.contains("linear-gradient(") {
                    style.background_gradient = parse_linear_gradient(value);
                }
                if value.contains("radial-gradient(") {
                    style.background_radial_gradient = parse_radial_gradient(value);
                }
            }
            "background-clip" | "-webkit-background-clip" => {
                style.background_clip = match value.trim().to_lowercase().as_str() {
                    "text" => rustkit_css::BackgroundClip::Text,
                    "padding-box" => rustkit_css::BackgroundClip::PaddingBox,
                    "content-box" => rustkit_css::BackgroundClip::ContentBox,
                    _ => rustkit_css::BackgroundClip::BorderBox,
                };
            }
            "font-size" => {
                if let Some(l) = parse_length(value) {
                    style.font_size = l;
                }
            }
            "font-weight" => {
                if matches!(value, "bold" | "600" | "700" | "800" | "900") {
                    style.font_weight = rustkit_css::FontWeight::BOLD;
                } else {
                    style.font_weight = rustkit_css::FontWeight::NORMAL;
                }
            }
            "font-style" => {
                if value == "italic" || value == "oblique" {
                    style.font_style = rustkit_css::FontStyle::Italic;
                }
            }
            "font-family" => {
                let fam = value.split(',').next().unwrap_or(value).trim();
                let fam = fam.trim_matches(['"', '\'']);
                if !fam.is_empty() {
                    style.font_family = fam.to_string();
                }
            }
            "line-height" => {
                if value.trim().eq_ignore_ascii_case("normal") {
                    // Normal sentinel: layout derives it from font metrics (W56).
                    style.line_height = 0.0;
                } else if let Ok(n) = value.parse::<f32>() {
                    style.line_height = n;
                } else if let Some(Length::Px(px)) = parse_length(value) {
                    if let Length::Px(fs) = style.font_size {
                        if fs > 0.0 {
                            style.line_height = px / fs;
                        }
                    }
                }
            }
            "text-align" => {
                style.text_align = match value {
                    "center" => rustkit_css::TextAlign::Center,
                    "right" => rustkit_css::TextAlign::Right,
                    "justify" => rustkit_css::TextAlign::Justify,
                    _ => rustkit_css::TextAlign::Left,
                };
            }
            "width" => {
                if let Some(l) = parse_length(value) {
                    style.width = l;
                }
            }
            "height" => {
                if let Some(l) = parse_length(value) {
                    style.height = l;
                }
            }
            "min-width" => {
                if let Some(l) = parse_length(value) {
                    style.min_width = l;
                }
            }
            "max-width" => {
                if let Some(l) = parse_length(value) {
                    style.max_width = l;
                }
            }
            "min-height" => {
                if let Some(l) = parse_length(value) {
                    style.min_height = l;
                }
            }
            "max-height" => {
                if let Some(l) = parse_length(value) {
                    style.max_height = l;
                }
            }
            "margin" => Self::apply_box_shorthand(value, |s, l| {
                s.margin_top = l.clone();
                s.margin_right = l.clone();
                s.margin_bottom = l.clone();
                s.margin_left = l;
            }, style),
            "margin-top" => {
                if let Some(l) = parse_length(value) { style.margin_top = l; }
            }
            "margin-right" => {
                if let Some(l) = parse_length(value) { style.margin_right = l; }
            }
            "margin-bottom" => {
                if let Some(l) = parse_length(value) { style.margin_bottom = l; }
            }
            "margin-left" => {
                if let Some(l) = parse_length(value) { style.margin_left = l; }
            }
            "padding" => Self::apply_box_shorthand(value, |s, l| {
                s.padding_top = l.clone();
                s.padding_right = l.clone();
                s.padding_bottom = l.clone();
                s.padding_left = l;
            }, style),
            "padding-top" => {
                if let Some(l) = parse_length(value) { style.padding_top = l; }
            }
            "padding-right" => {
                if let Some(l) = parse_length(value) { style.padding_right = l; }
            }
            "padding-bottom" => {
                if let Some(l) = parse_length(value) { style.padding_bottom = l; }
            }
            "padding-left" => {
                if let Some(l) = parse_length(value) { style.padding_left = l; }
            }
            "border" => {
                // `<width> <style> <color>` in any order; ignore the line style.
                let mut w = None;
                let mut c = None;
                for tok in value.split_whitespace() {
                    if let Some(l) = parse_length(tok) {
                        w = Some(l);
                    } else if let Some(col) = parse_color(tok) {
                        c = Some(col);
                    }
                }
                let w = w.unwrap_or(Length::Px(1.0));
                style.border_top_width = w.clone();
                style.border_right_width = w.clone();
                style.border_bottom_width = w.clone();
                style.border_left_width = w;
                if let Some(col) = c {
                    style.border_top_color = col;
                    style.border_right_color = col;
                    style.border_bottom_color = col;
                    style.border_left_color = col;
                }
            }
            "border-color" => {
                if let Some(c) = parse_color(value) {
                    style.border_top_color = c;
                    style.border_right_color = c;
                    style.border_bottom_color = c;
                    style.border_left_color = c;
                }
            }
            "border-width" => {
                if let Some(l) = parse_length(value) {
                    style.border_top_width = l.clone();
                    style.border_right_width = l.clone();
                    style.border_bottom_width = l.clone();
                    style.border_left_width = l;
                }
            }
            "display" => {
                if let Some(d) = rustkit_css::parse_display(value) {
                    style.display = d;
                }
            }
            // POSITION FAMILY. rustkit-layout already implements positioned
            // layout - 24 Position:: references plus out-of-flow handling in
            // flex.rs and grid.rs - but NOTHING could set style.position, so
            // every page rendered position:static and all of that code was
            // unreachable. Ported from the macOS reference, which has these
            // arms.
            "position" => {
                style.position = match value.trim() {
                    "static" => rustkit_css::Position::Static,
                    "relative" => rustkit_css::Position::Relative,
                    "absolute" => rustkit_css::Position::Absolute,
                    "fixed" => rustkit_css::Position::Fixed,
                    "sticky" => rustkit_css::Position::Sticky,
                    // Unknown keyword falls back to the CSS initial rather
                    // than leaving whatever a previous rule set.
                    _ => rustkit_css::Position::Static,
                };
            }
            "top" => {
                if let Some(length) = parse_length(value) {
                    style.top = Some(length);
                }
            }
            "right" => {
                if let Some(length) = parse_length(value) {
                    style.right = Some(length);
                }
            }
            "bottom" => {
                if let Some(length) = parse_length(value) {
                    style.bottom = Some(length);
                }
            }
            "left" => {
                if let Some(length) = parse_length(value) {
                    style.left = Some(length);
                }
            }
            "z-index" => {
                // `auto` is stored as 0, matching LayoutBox::z_index and the
                // macOS reference. A non-numeric value is ignored rather than
                // silently becoming 0, which would flatten an authored stack.
                let v = value.trim();
                if v.eq_ignore_ascii_case("auto") {
                    style.z_index = 0;
                } else if let Ok(z) = v.parse::<i32>() {
                    style.z_index = z;
                }
            }
            // OVERFLOW / WHITE-SPACE / TEXT-DECORATION.
            //
            // All three had implemented consumers and no producer: grid.rs
            // sizes auto-min tracks off overflow_x and honours white_space
            // Nowrap/Pre, and the display list emits TextDecoration commands -
            // none of it reachable, because no arm could set the field. Ported
            // from the macOS reference, which has these arms.
            "overflow" => {
                let o = parse_overflow(value);
                style.overflow_x = o;
                style.overflow_y = o;
            }
            "overflow-x" => {
                style.overflow_x = parse_overflow(value);
            }
            "overflow-y" => {
                style.overflow_y = parse_overflow(value);
            }
            "white-space" => {
                style.white_space = match value.trim().to_lowercase().as_str() {
                    "pre" => rustkit_css::WhiteSpace::Pre,
                    "nowrap" => rustkit_css::WhiteSpace::Nowrap,
                    "pre-wrap" => rustkit_css::WhiteSpace::PreWrap,
                    "pre-line" => rustkit_css::WhiteSpace::PreLine,
                    "break-spaces" => rustkit_css::WhiteSpace::BreakSpaces,
                    _ => rustkit_css::WhiteSpace::Normal,
                };
            }
            "text-decoration" | "text-decoration-line" => {
                // `text-decoration` is a shorthand and MAY carry a colour and
                // style as well as the line. Only the line is read here; a
                // shorthand that also names a colour still sets the line
                // correctly rather than being dropped whole, which is what
                // matching the entire value against fixed keywords would do.
                let mut line = rustkit_css::TextDecorationLine::NONE;
                let mut saw_line_keyword = false;
                for part in value.split_whitespace() {
                    match part.to_lowercase().as_str() {
                        "underline" => {
                            line.underline = true;
                            saw_line_keyword = true;
                        }
                        "overline" => {
                            line.overline = true;
                            saw_line_keyword = true;
                        }
                        "line-through" => {
                            line.line_through = true;
                            saw_line_keyword = true;
                        }
                        "none" => {
                            line = rustkit_css::TextDecorationLine::NONE;
                            saw_line_keyword = true;
                        }
                        _ => {}
                    }
                }
                // A value naming no line keyword at all (e.g. a colour on its
                // own) leaves the existing line alone instead of clearing it.
                if saw_line_keyword {
                    style.text_decoration_line = line;
                }
            }
            "text-decoration-color" => {
                if let Some(c) = parse_color(value) {
                    style.text_decoration_color = Some(c);
                }
            }
            "text-decoration-style" => {
                style.text_decoration_style = match value.trim().to_lowercase().as_str() {
                    "double" => rustkit_css::TextDecorationStyle::Double,
                    "dotted" => rustkit_css::TextDecorationStyle::Dotted,
                    "dashed" => rustkit_css::TextDecorationStyle::Dashed,
                    "wavy" => rustkit_css::TextDecorationStyle::Wavy,
                    _ => rustkit_css::TextDecorationStyle::Solid,
                };
            }
            // FLEX ITEM / ALIGNMENT FAMILY.
            //
            // Four of these came straight off the reachability list
            // (align_content, align_self, flex_shrink, order). The other three
            // - flex-grow, flex-basis and the `flex` shorthand - are a DECLARED
            // EXPANSION: flex-shrink alone is not usable, and `flex: 1` is the
            // form authors actually write. Shipping shrink without grow would
            // close a metric entry while leaving the family unusable, which is
            // the kind of win-on-paper this metric exists to prevent.
            "align-content" => {
                style.align_content = match value.trim() {
                    "flex-start" | "start" => rustkit_css::AlignContent::FlexStart,
                    "flex-end" | "end" => rustkit_css::AlignContent::FlexEnd,
                    "center" => rustkit_css::AlignContent::Center,
                    "space-between" => rustkit_css::AlignContent::SpaceBetween,
                    "space-around" => rustkit_css::AlignContent::SpaceAround,
                    "space-evenly" => rustkit_css::AlignContent::SpaceEvenly,
                    _ => rustkit_css::AlignContent::Stretch,
                };
            }
            "align-self" => {
                style.align_self = match value.trim() {
                    "flex-start" | "start" => rustkit_css::AlignSelf::FlexStart,
                    "flex-end" | "end" => rustkit_css::AlignSelf::FlexEnd,
                    "center" => rustkit_css::AlignSelf::Center,
                    "baseline" => rustkit_css::AlignSelf::Baseline,
                    "stretch" => rustkit_css::AlignSelf::Stretch,
                    _ => rustkit_css::AlignSelf::Auto,
                };
            }
            "order" => {
                // A non-numeric value is IGNORED rather than reset to 0.
                // Flattening to 0 would silently reorder a flex line.
                if let Ok(order) = value.trim().parse::<i32>() {
                    style.order = order;
                }
            }
            "flex-grow" => {
                if let Ok(grow) = value.trim().parse::<f32>() {
                    style.flex_grow = grow;
                }
            }
            "flex-shrink" => {
                if let Ok(shrink) = value.trim().parse::<f32>() {
                    style.flex_shrink = shrink;
                }
            }
            "flex-basis" => {
                style.flex_basis = parse_flex_basis(value);
            }
            "flex" => {
                // Shorthand: flex: <grow> [<shrink>] [<basis>].
                //
                // CSS also allows `flex: <grow> <basis>` (two values where the
                // second is a length). Treating position 2 as shrink
                // unconditionally would read `flex: 1 200px` as shrink=200,
                // which is silently wrong rather than merely unsupported - so
                // a second value that does NOT parse as a bare number is
                // treated as the basis.
                let parts: Vec<&str> = value.split_whitespace().collect();
                if let Some(first) = parts.first() {
                    if let Ok(grow) = first.parse::<f32>() {
                        style.flex_grow = grow;
                        // `flex: <number>` sets basis to 0, not auto - that is
                        // what makes `flex: 1` divide the container rather than
                        // sizing to content.
                        if parts.len() == 1 {
                            style.flex_shrink = 1.0;
                            style.flex_basis = rustkit_css::FlexBasis::Length(0.0);
                        }
                    }
                }
                if parts.len() >= 2 {
                    match parts[1].parse::<f32>() {
                        Ok(shrink) => {
                            style.flex_shrink = shrink;
                            style.flex_basis = rustkit_css::FlexBasis::Length(0.0);
                        }
                        Err(_) => {
                            style.flex_shrink = 1.0;
                            style.flex_basis = parse_flex_basis(parts[1]);
                        }
                    }
                }
                if parts.len() >= 3 {
                    style.flex_basis = parse_flex_basis(parts[2]);
                }
            }
            "flex-direction" => {
                style.flex_direction = match value {
                    "column" => rustkit_css::FlexDirection::Column,
                    "row-reverse" => rustkit_css::FlexDirection::RowReverse,
                    "column-reverse" => rustkit_css::FlexDirection::ColumnReverse,
                    _ => rustkit_css::FlexDirection::Row,
                };
            }
            "flex-wrap" => {
                style.flex_wrap = match value.trim() {
                    "wrap" => rustkit_css::FlexWrap::Wrap,
                    "wrap-reverse" => rustkit_css::FlexWrap::WrapReverse,
                    _ => rustkit_css::FlexWrap::NoWrap,
                };
            }
            "flex-flow" => {
                // Shorthand for flex-direction + flex-wrap.
                for part in value.split_whitespace() {
                    match part {
                        "row" => style.flex_direction = rustkit_css::FlexDirection::Row,
                        "column" => style.flex_direction = rustkit_css::FlexDirection::Column,
                        "row-reverse" => {
                            style.flex_direction = rustkit_css::FlexDirection::RowReverse
                        }
                        "column-reverse" => {
                            style.flex_direction = rustkit_css::FlexDirection::ColumnReverse
                        }
                        "wrap" => style.flex_wrap = rustkit_css::FlexWrap::Wrap,
                        "wrap-reverse" => style.flex_wrap = rustkit_css::FlexWrap::WrapReverse,
                        "nowrap" => style.flex_wrap = rustkit_css::FlexWrap::NoWrap,
                        _ => {}
                    }
                }
            }
            "justify-content" => {
                style.justify_content = match value {
                    "center" => rustkit_css::JustifyContent::Center,
                    "flex-end" | "end" => rustkit_css::JustifyContent::FlexEnd,
                    "space-between" => rustkit_css::JustifyContent::SpaceBetween,
                    "space-around" => rustkit_css::JustifyContent::SpaceAround,
                    "space-evenly" => rustkit_css::JustifyContent::SpaceEvenly,
                    _ => rustkit_css::JustifyContent::FlexStart,
                };
            }
            "align-items" => {
                style.align_items = match value {
                    "center" => rustkit_css::AlignItems::Center,
                    "flex-end" | "end" => rustkit_css::AlignItems::FlexEnd,
                    "stretch" => rustkit_css::AlignItems::Stretch,
                    _ => rustkit_css::AlignItems::FlexStart,
                };
            }
            "gap" => {
                if let Some(l) = parse_length(value) {
                    style.row_gap = l.clone();
                    style.column_gap = l;
                }
            }
            "row-gap" => {
                if let Some(l) = parse_length(value) {
                    style.row_gap = l;
                }
            }
            "column-gap" => {
                if let Some(l) = parse_length(value) {
                    style.column_gap = l;
                }
            }
            "box-sizing" => {
                style.box_sizing = match value.trim() {
                    "border-box" => rustkit_css::BoxSizing::BorderBox,
                    _ => rustkit_css::BoxSizing::ContentBox,
                };
            }
            "grid-template-columns" => {
                if let Some(t) = parse_grid_template(value) {
                    style.grid_template_columns = t;
                }
            }
            "grid-template-rows" => {
                if let Some(t) = parse_grid_template(value) {
                    style.grid_template_rows = t;
                }
            }
            "grid-auto-columns" => {
                if let Some(s) = parse_track_size(value) {
                    style.grid_auto_columns = s;
                }
            }
            "grid-auto-rows" => {
                if let Some(s) = parse_track_size(value) {
                    style.grid_auto_rows = s;
                }
            }
            "grid-auto-flow" => {
                style.grid_auto_flow = match value.trim() {
                    "column" => rustkit_css::GridAutoFlow::Column,
                    "row dense" | "dense row" => rustkit_css::GridAutoFlow::RowDense,
                    "column dense" | "dense column" => rustkit_css::GridAutoFlow::ColumnDense,
                    "dense" => rustkit_css::GridAutoFlow::RowDense,
                    _ => rustkit_css::GridAutoFlow::Row,
                };
            }
            "grid-column" => {
                if let Some((start, end)) = parse_grid_line_shorthand(value) {
                    style.grid_column_start = start;
                    style.grid_column_end = end;
                }
            }
            "grid-column-start" => {
                if let Some(line) = parse_grid_line(value) {
                    style.grid_column_start = line;
                }
            }
            "grid-column-end" => {
                if let Some(line) = parse_grid_line(value) {
                    style.grid_column_end = line;
                }
            }
            "grid-row" => {
                if let Some((start, end)) = parse_grid_line_shorthand(value) {
                    style.grid_row_start = start;
                    style.grid_row_end = end;
                }
            }
            "grid-row-start" => {
                if let Some(line) = parse_grid_line(value) {
                    style.grid_row_start = line;
                }
            }
            "grid-row-end" => {
                if let Some(line) = parse_grid_line(value) {
                    style.grid_row_end = line;
                }
            }
            _ => {}
        }
    }

    /// Apply a 1-value box shorthand (margin/padding). Only the common
    /// single-value form is handled; multi-value forms fall back to the
    /// first value on all sides.
    /// Takes no `self` - the receiver was never used in the body.
    fn apply_box_shorthand(
        value: &str,
        set: impl Fn(&mut ComputedStyle, Length),
        style: &mut ComputedStyle,
    ) {
        if let Some(first) = value.split_whitespace().next() {
            if let Some(l) = parse_length(first) {
                set(style, l);
            }
        }
    }

    /// Render a view (public API for continuous rendering).
    pub fn render_view(&mut self, id: EngineViewId) -> Result<(), EngineError> {
        self.render(id)
    }

    /// Render all views.
    pub fn render_all_views(&mut self) {
        let view_ids: Vec<_> = self.views.keys().copied().collect();
        for id in view_ids {
            if let Err(e) = self.render(id) {
                trace!(?id, error = %e, "Failed to render view");
            }
        }
    }

    /// Get render statistics from the renderer.
    pub fn get_render_stats(&self) -> RenderStats {
        self.renderer
            .as_ref()
            .map(|r| r.get_render_stats())
            .unwrap_or_default()
    }

    /// Capture a screenshot of a view to a PNG file.
    ///
    /// This renders the view to an offscreen texture and reads back the pixels.
    pub fn capture_view_screenshot(
        &mut self,
        id: EngineViewId,
        output_path: &std::path::Path,
    ) -> Result<ScreenshotMetadata, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let display_list = view.display_list.as_ref();
        let viewhost_id = view.viewhost_id;

        // Get view bounds for viewport - use headless_bounds if set
        let bounds = if let Some(headless_bounds) = view.headless_bounds {
            headless_bounds
        } else {
            self.viewhost
                .get_bounds(viewhost_id)
                .map_err(|e| EngineError::ViewError(e.to_string()))?
        };

        if bounds.width == 0 || bounds.height == 0 {
            return Err(EngineError::RenderError(format!(
                "Cannot capture screenshot of zero-sized view: {}x{}",
                bounds.width, bounds.height
            )));
        }

        if let Some(renderer) = &mut self.renderer {
            // Update viewport size
            renderer.set_viewport_size(bounds.width, bounds.height);

            // Get commands from display list or use empty
            let commands = display_list
                .map(|dl| dl.commands.as_slice())
                .unwrap_or(&[]);

            // Capture to file
            renderer
                .execute_and_capture(commands, output_path)
                .map_err(|e| EngineError::RenderError(e.to_string()))
        } else {
            Err(EngineError::RenderError("No renderer available".to_string()))
        }
    }

    /// Get the native window handle (HWND) for a view.
    #[cfg(windows)]
    pub fn get_view_hwnd(&self, id: EngineViewId) -> Result<HWND, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        self.viewhost
            .get_hwnd(view.viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))
    }

    /// Render a view (internal).
    fn render(&mut self, id: EngineViewId) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let viewhost_id = view.viewhost_id;
        let display_list = view.display_list.as_ref();
        let is_headless = view.headless_bounds.is_some();

        trace!(?id, is_headless, "Rendering view");

        // Get view bounds for viewport - use headless_bounds if set
        let bounds = if let Some(headless_bounds) = view.headless_bounds {
            headless_bounds
        } else {
            self.viewhost
                .get_bounds(viewhost_id)
                .map_err(|e| EngineError::ViewError(e.to_string()))?
        };

        // For headless views, use headless texture; for windowed views, use surface texture
        if is_headless {
            // Headless rendering path
            let texture_view = self.compositor
                .get_headless_texture_view(viewhost_id)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;

            // Render using display list if available
            if let (Some(renderer), Some(display_list)) = (&mut self.renderer, display_list) {
                renderer.set_viewport_size(bounds.width, bounds.height);
                renderer.execute(&display_list.commands, &texture_view)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
            } else if let Some(renderer) = &mut self.renderer {
                renderer.set_viewport_size(bounds.width, bounds.height);
                renderer.execute(&[], &texture_view)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
            } else {
                self.compositor
                    .render_solid_color(viewhost_id, self.config.background_color)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
            }
            // No present needed for headless textures
        } else {
            // Windowed rendering path
            let (output, texture_view) = self.compositor
                .get_surface_texture(viewhost_id)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;

            // Render using display list if available, otherwise just clear to background
            if let (Some(renderer), Some(display_list)) = (&mut self.renderer, display_list) {
                renderer.set_viewport_size(bounds.width, bounds.height);
                renderer.execute(&display_list.commands, &texture_view)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
            } else if let Some(renderer) = &mut self.renderer {
                renderer.set_viewport_size(bounds.width, bounds.height);
                renderer.execute(&[], &texture_view)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
            } else {
                drop(output);
                self.compositor
                    .render_solid_color(viewhost_id, self.config.background_color)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
                return Ok(());
            }

            // Present
            self.compositor.present(output);
        }

        Ok(())
    }

    /// Execute JavaScript in a view.
    pub fn execute_script(
        &mut self,
        id: EngineViewId,
        script: &str,
    ) -> Result<String, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;

        let bindings = view
            .bindings
            .as_ref()
            .ok_or(EngineError::JsError("JavaScript not initialized".into()))?;

        let result = bindings
            .evaluate(script)
            .map_err(|e| EngineError::JsError(e.to_string()))?;

        Ok(format!("{:?}", result))
    }

    /// Get the current URL of a view.
    pub fn get_url(&self, id: EngineViewId) -> Option<Url> {
        self.views.get(&id).and_then(|v| v.url.clone())
    }

    /// Get the title of a view.
    pub fn get_title(&self, id: EngineViewId) -> Option<String> {
        self.views.get(&id).and_then(|v| v.title.clone())
    }

    /// Check if a view can go back.
    pub fn can_go_back(&self, id: EngineViewId) -> bool {
        self.views
            .get(&id)
            .map(|v| v.navigation.can_go_back())
            .unwrap_or(false)
    }

    /// Check if a view can go forward.
    pub fn can_go_forward(&self, id: EngineViewId) -> bool {
        self.views
            .get(&id)
            .map(|v| v.navigation.can_go_forward())
            .unwrap_or(false)
    }

    /// Get the number of views.
    pub fn view_count(&self) -> usize {
        self.views.len()
    }

    /// Get the download manager.
    pub fn download_manager(&self) -> Arc<rustkit_net::DownloadManager> {
        self.loader.download_manager()
    }

    /// Get GPU info.
    pub fn gpu_info(&self) -> String {
        format!("{:?}", self.compositor.adapter_info())
    }

    /// Handle a view event from the viewhost.
    #[cfg(windows)]
    pub fn handle_view_event(&mut self, event: rustkit_viewhost::ViewEvent) {
        use rustkit_viewhost::ViewEvent;

        match event {
            ViewEvent::Resized {
                view_id: viewhost_id,
                bounds,
                dpi: _,
            } => {
                // Find engine view id for this viewhost id
                if let Some((id, _)) = self
                    .views
                    .iter()
                    .find(|(_, v)| v.viewhost_id == viewhost_id)
                {
                    let id = *id;
                    let _ = self.resize_view(
                        id,
                        rustkit_viewhost::Bounds::new(
                            bounds.x,
                            bounds.y,
                            bounds.width,
                            bounds.height,
                        ),
                    );
                }
            }
            ViewEvent::Focused {
                view_id: viewhost_id,
            } => {
                if let Some((id, view)) = self
                    .views
                    .iter_mut()
                    .find(|(_, v)| v.viewhost_id == viewhost_id)
                {
                    view.view_focused = true;
                    let _ = self
                        .event_tx
                        .send(EngineEvent::ViewFocused { view_id: *id });
                }
            }
            ViewEvent::Blurred {
                view_id: viewhost_id,
            } => {
                if let Some(view) = self
                    .views
                    .values_mut()
                    .find(|v| v.viewhost_id == viewhost_id)
                {
                    view.view_focused = false;
                }
            }
            ViewEvent::Input {
                view_id: viewhost_id,
                event: input_event,
            } => {
                self.handle_input_event(viewhost_id, input_event);
            }
            _ => {}
        }
    }

    /// Handle an input event.
    #[cfg(windows)]
    fn handle_input_event(&mut self, viewhost_id: ViewId, event: rustkit_core::InputEvent) {
        use rustkit_core::InputEvent;

        // Find the view
        let engine_id = self
            .views
            .iter()
            .find(|(_, v)| v.viewhost_id == viewhost_id)
            .map(|(id, _)| *id);

        let Some(engine_id) = engine_id else {
            return;
        };

        match event {
            InputEvent::Mouse(mouse_event) => {
                self.handle_mouse_event(engine_id, mouse_event);
            }
            InputEvent::Key(key_event) => {
                self.handle_key_event(engine_id, key_event);
            }
            InputEvent::Focus(focus_event) => {
                // Focus events are handled via ViewEvent::Focused/Blurred
                let _ = focus_event;
            }
        }
    }

    /// Handle a mouse event.
    #[cfg(windows)]
    fn handle_mouse_event(&mut self, view_id: EngineViewId, event: rustkit_core::MouseEvent) {
        use rustkit_core::MouseEventType;
        use rustkit_dom::MouseEventData;

        let view = match self.views.get_mut(&view_id) {
            Some(v) => v,
            None => return,
        };

        // Perform hit testing if we have layout
        let hit_result = view
            .layout
            .as_ref()
            .and_then(|layout| layout.hit_test(event.position.x as f32, event.position.y as f32));

        // Convert to DOM event
        let dom_event_type = match event.event_type {
            MouseEventType::MouseDown => "mousedown",
            MouseEventType::MouseUp => "mouseup",
            MouseEventType::MouseMove => "mousemove",
            MouseEventType::MouseEnter => "mouseenter",
            MouseEventType::MouseLeave => "mouseleave",
            MouseEventType::Wheel => "wheel",
            MouseEventType::ContextMenu => "contextmenu",
        };

        let _mouse_data = MouseEventData {
            client_x: event.position.x,
            client_y: event.position.y,
            screen_x: event.screen_position.x,
            screen_y: event.screen_position.y,
            offset_x: hit_result.as_ref().map(|r| r.local_x as f64).unwrap_or(0.0),
            offset_y: hit_result.as_ref().map(|r| r.local_y as f64).unwrap_or(0.0),
            button: event.button.button_index(),
            buttons: event.buttons,
            ctrl_key: event.modifiers.ctrl,
            alt_key: event.modifiers.alt,
            shift_key: event.modifiers.shift,
            meta_key: event.modifiers.meta,
            related_target: None,
        };

        // If we have a hit and a document, dispatch the event
        if let (Some(_hit), Some(_document)) = (hit_result, &view.document) {
            // TODO: Map hit result to DOM node and dispatch event
            // For now, just log
            trace!(?view_id, event_type = dom_event_type, "Mouse event");
        }

        // Handle click focus change
        if event.event_type == MouseEventType::MouseDown {
            // TODO: Focus the clicked element if focusable
        }
    }

    /// Handle a keyboard event.
    #[cfg(windows)]
    fn handle_key_event(&mut self, view_id: EngineViewId, event: rustkit_core::KeyEvent) {
        use rustkit_core::{KeyCode, KeyEventType};

        let view = match self.views.get_mut(&view_id) {
            Some(v) => v,
            None => return,
        };

        // Only process keyboard events if the view has focus
        if !view.view_focused {
            return;
        }

        trace!(?view_id, key = ?event.key_code, event_type = ?event.event_type, "Key event");

        // Handle Tab key for focus navigation
        if event.event_type == KeyEventType::KeyDown && event.key_code == KeyCode::Tab {
            // TODO: Implement Tab navigation between focusable elements
        }

        // Dispatch to focused element via DOM events
        // TODO: Dispatch KeyboardEvent to focused DOM node
    }

    /// Focus a DOM node in a view.
    pub fn focus_element(
        &mut self,
        view_id: EngineViewId,
        node_id: rustkit_dom::NodeId,
    ) -> Result<(), EngineError> {
        let view = self
            .views
            .get_mut(&view_id)
            .ok_or(EngineError::ViewNotFound(view_id))?;

        let old_focused = view.focused_node;
        view.focused_node = Some(node_id);

        // TODO: Dispatch blur event to old focused element
        // TODO: Dispatch focus event to new focused element

        debug!(?view_id, ?node_id, ?old_focused, "Focus changed");
        Ok(())
    }

    /// Blur the currently focused element.
    pub fn blur_element(&mut self, view_id: EngineViewId) -> Result<(), EngineError> {
        let view = self
            .views
            .get_mut(&view_id)
            .ok_or(EngineError::ViewNotFound(view_id))?;

        let old_focused = view.focused_node.take();

        // TODO: Dispatch blur event to old focused element

        debug!(?view_id, ?old_focused, "Element blurred");
        Ok(())
    }

    /// Get the currently focused node in a view.
    pub fn get_focused_element(&self, view_id: EngineViewId) -> Option<rustkit_dom::NodeId> {
        self.views.get(&view_id).and_then(|v| v.focused_node)
    }

    /// Load an image from a URL.
    /// Fetch every `<link rel="stylesheet">` in the document and store the CSS
    /// on the view so the next layout can cascade it.
    ///
    /// FAIL-SOFT PER SHEET, DELIBERATELY: one 404 stylesheet must not fail the
    /// whole navigation - that is how a real browser behaves. But each failure
    /// is logged at warn with its URL, because a silently-dropped stylesheet
    /// looks exactly like a page that renders wrong for no reason. Returns the
    /// number of sheets that actually loaded.
    async fn load_external_stylesheets(
        &mut self,
        id: EngineViewId,
        document: &Document,
        base_url: &Url,
    ) -> usize {
        // NO early return on an empty list. A document with no <link> must
        // ASSIGN an empty string, not skip the assignment - skipping leaves the
        // PREVIOUS document's stylesheet on the view, so navigating from a
        // styled page to an unstyled one silently keeps the old page's CSS.
        let urls = Self::discover_external_stylesheets(document, Some(base_url));
        let mut css = String::new();
        let mut loaded = 0usize;
        for url in urls {
            match self.loader.fetch(Request::get(url.clone())).await {
                Ok(response) if response.ok() => match response.text().await {
                    Ok(text) => {
                        css.push_str(&text);
                        css.push('\n');
                        loaded += 1;
                    }
                    Err(e) => warn!(%url, error = %e, "stylesheet body was not readable"),
                },
                Ok(response) => {
                    warn!(%url, status = %response.status, "stylesheet fetch returned non-OK")
                }
                Err(e) => warn!(%url, error = %e, "stylesheet fetch failed"),
            }
        }
        if let Some(view) = self.views.get_mut(&id) {
            view.external_css = css;
        }
        info!(?id, loaded, "external stylesheets loaded");
        loaded
    }

    pub async fn load_image(&self, view_id: EngineViewId, url: Url) -> Result<(), EngineError> {
        let image_manager = self.image_manager.clone();
        let event_tx = self.event_tx.clone();

        match image_manager.load(url.clone()).await {
            Ok(image) => {
                let _ = event_tx.send(EngineEvent::ImageLoaded {
                    view_id,
                    url,
                    width: image.natural_width,
                    height: image.natural_height,
                });
                Ok(())
            }
            Err(e) => {
                let error = e.to_string();
                let _ = event_tx.send(EngineEvent::ImageError {
                    view_id,
                    url: url.clone(),
                    error: error.clone(),
                });
                Err(EngineError::RenderError(format!("Image load failed: {}", error)))
            }
        }
    }

    /// Preload an image (non-blocking).
    pub fn preload_image(&self, url: Url) {
        self.image_manager.preload(url);
    }

    /// Check if an image is cached.
    pub fn is_image_cached(&self, url: &Url) -> bool {
        self.image_manager.is_cached(url)
    }

    /// Get a cached image's dimensions.
    pub fn get_image_dimensions(&self, url: &Url) -> Option<(u32, u32)> {
        self.image_manager
            .get_cached(url)
            .map(|img| (img.natural_width, img.natural_height))
    }

    /// Get the image manager for direct access.
    pub fn image_manager(&self) -> Arc<ImageManager> {
        self.image_manager.clone()
    }

    /// Clear the image cache.
    pub fn clear_image_cache(&self) {
        self.image_manager.clear_cache();
    }

    /// Drain IPC messages from all views.
    ///
    /// Returns a Vec of (EngineViewId, IpcMessage) tuples for messages received
    /// via `window.ipc.postMessage()` from JavaScript in any view.
    ///
    /// This should be called periodically (e.g., during the message loop) to
    /// process IPC messages from the Chrome UI, Shelf, and Content views.
    pub fn drain_ipc_messages(&self) -> Vec<(EngineViewId, IpcMessage)> {
        let mut messages = Vec::new();

        for (&view_id, view_state) in &self.views {
            if let Some(ref bindings) = view_state.bindings {
                for ipc_msg in bindings.drain_ipc_queue() {
                    messages.push((view_id, ipc_msg));
                }
            }
        }

        messages
    }

    /// Check if any view has pending IPC messages.
    pub fn has_pending_ipc(&self) -> bool {
        self.views.values().any(|v| {
            v.bindings
                .as_ref()
                .map(|b| b.has_pending_ipc())
                .unwrap_or(false)
        })
    }

    /// Capture a frame from a view to a PPM file.
    ///
    /// This renders the current display list to an offscreen texture and saves it.
    /// This is useful for deterministic testing and visual debugging.
    pub fn capture_frame(&mut self, id: EngineViewId, path: &str) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let viewhost_id = view.viewhost_id;
        let has_display_list = view.display_list.is_some();

        info!(?id, path, "Capturing frame");

        // Get surface size
        let (width, height) = self.compositor
            .get_surface_size(viewhost_id)
            .map_err(|e| EngineError::RenderError(e.to_string()))?;

        if width == 0 || height == 0 {
            return Err(EngineError::RenderError("Cannot capture zero-size frame".into()));
        }

        // If we have a display list and renderer, render to offscreen texture
        if has_display_list && self.renderer.is_some() {
            let view = self.views.get(&id).unwrap();
            let display_list = view.display_list.as_ref().unwrap();
            let renderer = self.renderer.as_mut().unwrap();

            // Update viewport size for correct coordinate transforms
            renderer.set_viewport_size(width, height);

            // Capture with actual display list rendering
            self.compositor
                .capture_frame_with_renderer(viewhost_id, path, renderer, &display_list.commands)
                .map_err(|e| EngineError::RenderError(e.to_string()))
        } else {
            // Fallback to magenta test pattern if no display list
            self.compositor
                .capture_frame_to_file(viewhost_id, path)
                .map_err(|e| EngineError::RenderError(e.to_string()))
        }
    }

    /// Export the layout tree for a view as JSON.
    ///
    /// This exports the current layout tree with dimensions for each box,
    /// which can be compared against Chromium's DOMRect data for layout parity testing.
    pub fn export_layout_json(&self, id: EngineViewId, path: &str) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;

        let layout = view.layout.as_ref().ok_or_else(|| {
            EngineError::RenderError("No layout tree available".into())
        })?;

        // Convert layout tree to JSON-serializable structure
        fn layout_box_to_json(layout_box: &LayoutBox) -> serde_json::Value {
            let dims = &layout_box.dimensions;
            let content = &dims.content;
            let margin_box = dims.margin_box();
            let padding_box = dims.padding_box();
            let border_box = dims.border_box();

            let box_type = match &layout_box.box_type {
                BoxType::Block => "block",
                BoxType::Inline => "inline",
                BoxType::AnonymousBlock => "anonymous_block",
                BoxType::Text(t) => return serde_json::json!({
                    "type": "text",
                    "text": t.chars().take(50).collect::<String>(),
                    "rect": {
                        "x": content.x,
                        "y": content.y,
                        "width": content.width,
                        "height": content.height
                    }
                }),
            };

            let children: Vec<serde_json::Value> = layout_box.children
                .iter()
                .map(layout_box_to_json)
                .collect();

            serde_json::json!({
                "type": box_type,
                "content_rect": {
                    "x": content.x,
                    "y": content.y,
                    "width": content.width,
                    "height": content.height
                },
                "padding_rect": {
                    "x": padding_box.x,
                    "y": padding_box.y,
                    "width": padding_box.width,
                    "height": padding_box.height
                },
                "border_rect": {
                    "x": border_box.x,
                    "y": border_box.y,
                    "width": border_box.width,
                    "height": border_box.height
                },
                "margin_rect": {
                    "x": margin_box.x,
                    "y": margin_box.y,
                    "width": margin_box.width,
                    "height": margin_box.height
                },
                "children": children
            })
        }

        let json = layout_box_to_json(layout);

        // Write to file
        let file = std::fs::File::create(path)
            .map_err(|e| EngineError::RenderError(format!("Failed to create file: {}", e)))?;

        serde_json::to_writer_pretty(file, &json)
            .map_err(|e| EngineError::RenderError(format!("Failed to write JSON: {}", e)))?;

        info!(?id, path, "Layout JSON exported");
        Ok(())
    }
}

/// Builder for Engine.
pub struct EngineBuilder {
    config: EngineConfig,
    interceptor: Option<rustkit_net::RequestInterceptor>,
}

impl EngineBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            config: EngineConfig::default(),
            interceptor: None,
        }
    }

    /// Set a request interceptor for filtering network requests.
    pub fn request_interceptor(mut self, interceptor: rustkit_net::RequestInterceptor) -> Self {
        self.interceptor = Some(interceptor);
        self
    }

    /// Set the user agent.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.config.user_agent = user_agent.into();
        self
    }

    /// Enable or disable JavaScript.
    pub fn javascript_enabled(mut self, enabled: bool) -> Self {
        self.config.javascript_enabled = enabled;
        self
    }

    /// Enable or disable cookies.
    pub fn cookies_enabled(mut self, enabled: bool) -> Self {
        self.config.cookies_enabled = enabled;
        self
    }

    /// Set the default background color.
    pub fn background_color(mut self, color: [f64; 4]) -> Self {
        self.config.background_color = color;
        self
    }

    /// Set the entire configuration at once.
    pub fn with_config(mut self, config: EngineConfig) -> Self {
        self.config = config;
        self
    }

    /// Disable animations for deterministic parity testing.
    pub fn disable_animations(mut self, disable: bool) -> Self {
        self.config.disable_animations = disable;
        self
    }

    /// Build the engine.
    pub fn build(self) -> Result<Engine, EngineError> {
        Engine::with_interceptor(self.config, self.interceptor)
    }
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve `var(--name)` / `var(--name, fallback)` references in a CSS value
/// against the in-scope custom properties. Handles nested parens inside the
/// fallback and resolves recursively (a variable whose value is itself a
/// `var()`). Unknown variables with no fallback resolve to empty (matching CSS,
/// which then treats the declaration as invalid — good enough here).
fn resolve_var_refs(value: &str, vars: &HashMap<String, String>) -> String {
    if !value.contains("var(") {
        return value.to_string();
    }
    let mut out = String::new();
    let mut rest = value;
    while let Some(pos) = rest.find("var(") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 4..];
        // Find the matching close paren, tracking one level of nested parens
        // (e.g. a fallback that contains rgb(...)).
        let bytes = after.as_bytes();
        let mut depth = 1usize;
        let mut i = 0usize;
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        let inner = &after[..i.saturating_sub(1)];
        rest = &after[i..];
        let (name, fallback) = match inner.find(',') {
            Some(c) => (inner[..c].trim(), Some(inner[c + 1..].trim())),
            None => (inner.trim(), None),
        };
        let resolved = match vars.get(name) {
            Some(v) => resolve_var_refs(v, vars),
            None => fallback.map(|f| resolve_var_refs(f, vars)).unwrap_or_default(),
        };
        out.push_str(&resolved);
    }
    out.push_str(rest);
    out
}

/// Split a CSS value on top-level commas, ignoring commas inside parentheses
/// (so `rgb(1, 2, 3)` and nested gradients survive as one segment).
fn split_top_level_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for ch in s.chars() {
        match ch {
            '(' => {
                depth += 1;
                cur.push(ch);
            }
            ')' => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// The solid base color of a `background` shorthand: the last comma-separated
/// layer that is a plain color (gradient/image layers are skipped). This is the
/// color painted under any gradient — e.g. `radial-gradient(...), #0f172a`
/// yields `#0f172a`, not the gradient's first stop.
fn background_base_color(value: &str) -> Option<rustkit_css::Color> {
    let mut base = None;
    for layer in split_top_level_commas(value) {
        if layer.contains("gradient(") || layer.contains("url(") {
            continue;
        }
        if let Some(c) = layer.split_whitespace().find_map(parse_color) {
            base = Some(c);
        }
    }
    base
}

/// Parse a `linear-gradient(...)` into an angle and color stops. Supports an
/// optional leading angle (`135deg`) or direction keyword (`to right`), then a
/// list of `color [position%]` stops. Positions default to an even spread.
fn parse_linear_gradient(value: &str) -> Option<rustkit_css::LinearGradient> {
    let start = value.find("linear-gradient(")?;
    let after = &value[start + "linear-gradient(".len()..];
    // Find the matching close paren.
    let mut depth = 1i32;
    let mut end = after.len();
    for (i, ch) in after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let parts = split_top_level_commas(&after[..end]);
    if parts.is_empty() {
        return None;
    }

    let mut idx = 0;
    let mut angle_deg = 180.0f32; // default: to bottom
    let first = parts[0].to_lowercase();
    if let Some(deg) = first.strip_suffix("deg") {
        if let Ok(a) = deg.trim().parse::<f32>() {
            angle_deg = a;
            idx = 1;
        }
    } else if first.starts_with("to ") {
        angle_deg = match first.as_str() {
            "to top" => 0.0,
            "to right" => 90.0,
            "to bottom" => 180.0,
            "to left" => 270.0,
            "to top right" | "to right top" => 45.0,
            "to bottom right" | "to right bottom" => 135.0,
            "to bottom left" | "to left bottom" => 225.0,
            "to top left" | "to left top" => 315.0,
            _ => 180.0,
        };
        idx = 1;
    }

    let stop_parts = &parts[idx..];
    let n = stop_parts.len();
    if n < 2 {
        return None;
    }
    let denom = (n - 1).max(1) as f32;
    let mut stops = Vec::with_capacity(n);
    for (i, seg) in stop_parts.iter().enumerate() {
        let seg = seg.trim();
        // A trailing `N%` token is the position; the rest is the color (which
        // may itself contain spaces/commas, e.g. rgb(1, 2, 3)).
        let (color_str, pos) = match seg.rfind(char::is_whitespace) {
            Some(sp) if seg[sp + 1..].trim().ends_with('%') => {
                let p = seg[sp + 1..].trim().trim_end_matches('%').parse::<f32>().ok();
                (seg[..sp].trim(), p.map(|p| p / 100.0))
            }
            _ => (seg, None),
        };
        let color = parse_color(color_str)?;
        let position = pos.unwrap_or(i as f32 / denom);
        stops.push(rustkit_css::GradientStop { color, position });
    }
    Some(rustkit_css::LinearGradient { angle_deg, stops })
}

/// Parse a `radial-gradient(...)` into shape, center, and color stops. Handles
/// an optional leading config (`circle`|`ellipse` and/or `at <position>`), then
/// a `color [position%]` stop list. Size is treated as farthest-corner (the CSS
/// default); center defaults to 50% 50%. When the value carries several
/// comma-separated radial layers, the first is captured (MVP).
fn parse_radial_gradient(value: &str) -> Option<rustkit_css::RadialGradient> {
    let start = value.find("radial-gradient(")?;
    let after = &value[start + "radial-gradient(".len()..];
    let mut depth = 1i32;
    let mut end = after.len();
    for (i, ch) in after.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let parts = split_top_level_commas(&after[..end]);
    if parts.is_empty() {
        return None;
    }

    let mut idx = 0;
    let mut shape = rustkit_css::RadialShape::Ellipse;
    let (mut cx, mut cy) = (0.5f32, 0.5f32);
    // The first segment is config (not a stop) when it names a shape or an
    // `at <position>` clause — i.e. it is not itself a color.
    let first = parts[0].trim().to_lowercase();
    let is_config = first.starts_with("circle")
        || first.starts_with("ellipse")
        || first.starts_with("at ")
        || first.contains(" at ");
    if is_config {
        if first.contains("circle") {
            shape = rustkit_css::RadialShape::Circle;
        }
        if let Some(at) = first.find("at ") {
            let (x, y) = parse_radial_position(first[at + 3..].trim());
            cx = x;
            cy = y;
        }
        idx = 1;
    }

    let stop_parts = &parts[idx..];
    let n = stop_parts.len();
    if n < 2 {
        return None;
    }
    let denom = (n - 1).max(1) as f32;
    let mut stops = Vec::with_capacity(n);
    for (i, seg) in stop_parts.iter().enumerate() {
        let seg = seg.trim();
        let (color_str, pos) = match seg.rfind(char::is_whitespace) {
            Some(sp) if seg[sp + 1..].trim().ends_with('%') => {
                let p = seg[sp + 1..].trim().trim_end_matches('%').parse::<f32>().ok();
                (seg[..sp].trim(), p.map(|p| p / 100.0))
            }
            _ => (seg, None),
        };
        let color = parse_color(color_str)?;
        let position = pos.unwrap_or(i as f32 / denom);
        stops.push(rustkit_css::GradientStop { color, position });
    }
    Some(rustkit_css::RadialGradient { shape, cx, cy, stops })
}

/// Parse a radial-gradient `<position>` (the tokens after `at`) into (x, y)
/// center fractions 0..1. Keywords are axis-specific and order-independent
/// (`top left` == `left top`); `center` and percentages are positional
/// (first→x, second→y). A single value sets the horizontal axis (vertical
/// stays centered), matching CSS.
fn parse_radial_position(pos: &str) -> (f32, f32) {
    let pct = |t: &str| t.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()).map(|p| p / 100.0);
    let toks: Vec<&str> = pos.split_whitespace().collect();
    let (mut cx, mut cy) = (0.5f32, 0.5f32);
    match toks.as_slice() {
        [a] => match *a {
            "left" => cx = 0.0,
            "right" => cx = 1.0,
            "top" => cy = 0.0,
            "bottom" => cy = 1.0,
            "center" => {}
            other => {
                if let Some(p) = pct(other) {
                    cx = p;
                }
            }
        },
        [a, b] => {
            for (i, t) in [*a, *b].iter().enumerate() {
                match *t {
                    "left" => cx = 0.0,
                    "right" => cx = 1.0,
                    "top" => cy = 0.0,
                    "bottom" => cy = 1.0,
                    "center" => {}
                    other => {
                        if let Some(p) = pct(other) {
                            if i == 0 {
                                cx = p;
                            } else {
                                cy = p;
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    (cx, cy)
}

/// Parse a color value from CSS.
fn parse_color(value: &str) -> Option<rustkit_css::Color> {
    let value = value.trim().to_lowercase();

    // Named colors
    match value.as_str() {
        "black" => return Some(rustkit_css::Color::BLACK),
        "white" => return Some(rustkit_css::Color::WHITE),
        "red" => return Some(rustkit_css::Color::new(255, 0, 0, 1.0)),
        "green" => return Some(rustkit_css::Color::new(0, 128, 0, 1.0)),
        "blue" => return Some(rustkit_css::Color::new(0, 0, 255, 1.0)),
        "yellow" => return Some(rustkit_css::Color::new(255, 255, 0, 1.0)),
        "cyan" => return Some(rustkit_css::Color::new(0, 255, 255, 1.0)),
        "magenta" => return Some(rustkit_css::Color::new(255, 0, 255, 1.0)),
        "gray" | "grey" => return Some(rustkit_css::Color::new(128, 128, 128, 1.0)),
        "transparent" => return Some(rustkit_css::Color::TRANSPARENT),
        _ => {}
    }

    // Hex colors
    if let Some(hex) = value.strip_prefix('#') {
        let (r, g, b) = match hex.len() {
            3 => {
                let r = u8::from_str_radix(&hex[0..1], 16).ok()? * 17;
                let g = u8::from_str_radix(&hex[1..2], 16).ok()? * 17;
                let b = u8::from_str_radix(&hex[2..3], 16).ok()? * 17;
                (r, g, b)
            }
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                (r, g, b)
            }
            _ => return None,
        };
        return Some(rustkit_css::Color::from_rgb(r, g, b));
    }

    // rgb() and rgba()
    if value.starts_with("rgb(") || value.starts_with("rgba(") {
        let inner = value
            .trim_start_matches("rgba(")
            .trim_start_matches("rgb(")
            .trim_end_matches(')');
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() >= 3 {
            let r: u8 = parts[0].trim().parse().ok()?;
            let g: u8 = parts[1].trim().parse().ok()?;
            let b: u8 = parts[2].trim().parse().ok()?;
            let a: f32 = if parts.len() >= 4 {
                parts[3].trim().parse().ok()?
            } else {
                1.0
            };
            return Some(rustkit_css::Color::new(r, g, b, a));
        }
    }

    None
}

/// Find the position of the matching closing parenthesis (depth starts at 1,
/// i.e. `s` is the text after an opening paren).
fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 1;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Parse a single grid track size (`1fr`, `100px`, `auto`, `min-content`,
/// `max-content`, `minmax(...)`, `fit-content(...)`, `N%`).
fn parse_track_size(value: &str) -> Option<rustkit_css::TrackSize> {
    let value = value.trim();
    if value == "auto" {
        return Some(rustkit_css::TrackSize::Auto);
    }
    if value == "min-content" {
        return Some(rustkit_css::TrackSize::MinContent);
    }
    if value == "max-content" {
        return Some(rustkit_css::TrackSize::MaxContent);
    }
    if let Some(fr_str) = value.strip_suffix("fr") {
        if let Ok(fr) = fr_str.trim().parse::<f32>() {
            return Some(rustkit_css::TrackSize::Fr(fr));
        }
    }
    if let Some(px_str) = value.strip_suffix("px") {
        if let Ok(px) = px_str.trim().parse::<f32>() {
            return Some(rustkit_css::TrackSize::Px(px));
        }
    }
    if let Some(pct_str) = value.strip_suffix('%') {
        if let Ok(pct) = pct_str.trim().parse::<f32>() {
            return Some(rustkit_css::TrackSize::Percent(pct));
        }
    }
    if value.starts_with("minmax(") {
        if let Some(close) = find_matching_paren(&value[7..]) {
            let content = &value[7..7 + close];
            if let Some(comma) = content.find(',') {
                let min_str = content[..comma].trim();
                let max_str = content[comma + 1..].trim();
                if let (Some(min), Some(max)) =
                    (parse_track_size(min_str), parse_track_size(max_str))
                {
                    return Some(rustkit_css::TrackSize::MinMax(Box::new(min), Box::new(max)));
                }
            }
        }
    }
    if value.starts_with("fit-content(") {
        if let Some(close) = find_matching_paren(&value[12..]) {
            let content = &value[12..12 + close];
            if let Some(length) = parse_length(content) {
                return Some(rustkit_css::TrackSize::FitContent(length.to_px(16.0, 16.0, 0.0)));
            }
        }
    }
    None
}

/// Parse `grid-template-columns`/`-rows` into a GridTemplate, expanding
/// `repeat(N, <track>)` into explicit tracks (auto-fill/-fit default to 4).
fn parse_grid_template(value: &str) -> Option<rustkit_css::GridTemplate> {
    let value = value.trim();
    if value == "none" || value.is_empty() {
        return Some(rustkit_css::GridTemplate::none());
    }
    let mut tracks = Vec::new();
    if let Some(repeat_start) = value.find("repeat(") {
        let after_repeat = &value[repeat_start + 7..];
        if let Some(close_paren) = find_matching_paren(after_repeat) {
            let repeat_content = &after_repeat[..close_paren];
            if let Some(comma_pos) = repeat_content.find(',') {
                let count_str = repeat_content[..comma_pos].trim();
                let track_str = repeat_content[comma_pos + 1..].trim();
                let count: Option<u32> = if count_str == "auto-fill" || count_str == "auto-fit" {
                    Some(4)
                } else {
                    count_str.parse().ok()
                };
                if let (Some(count), Some(track_size)) = (count, parse_track_size(track_str)) {
                    for _ in 0..count {
                        tracks.push(rustkit_css::TrackDefinition::simple(track_size.clone()));
                    }
                }
            }
        }
    } else {
        for part in value.split_whitespace() {
            if let Some(track_size) = parse_track_size(part) {
                tracks.push(rustkit_css::TrackDefinition::simple(track_size));
            }
        }
    }
    if tracks.is_empty() {
        return None;
    }
    Some(rustkit_css::GridTemplate {
        tracks,
        repeats: Vec::new(),
        final_line_names: Vec::new(),
    })
}

/// Parse a single grid line (`auto`, `span N`, a number, or a named line
/// treated as auto).
fn parse_grid_line(value: &str) -> Option<rustkit_css::GridLine> {
    let value = value.trim();
    if value == "auto" {
        return Some(rustkit_css::GridLine::Auto);
    }
    if let Some(span_str) = value.strip_prefix("span") {
        if let Ok(span) = span_str.trim().parse::<u32>() {
            return Some(rustkit_css::GridLine::Span(span));
        }
    }
    if let Ok(num) = value.parse::<i32>() {
        return Some(rustkit_css::GridLine::Number(num));
    }
    Some(rustkit_css::GridLine::Auto)
}

/// Parse a `grid-column`/`grid-row` shorthand (`1 / 3`, `span 2`).
fn parse_grid_line_shorthand(
    value: &str,
) -> Option<(rustkit_css::GridLine, rustkit_css::GridLine)> {
    let value = value.trim();
    if let Some(slash_pos) = value.find('/') {
        let start = parse_grid_line(value[..slash_pos].trim())?;
        let end = parse_grid_line(value[slash_pos + 1..].trim())?;
        return Some((start, end));
    }
    let start = parse_grid_line(value)?;
    Some((start, rustkit_css::GridLine::Auto))
}

/// Parse a length value from CSS.
/// Parse a CSS transform value into a TransformList.
fn parse_transform(value: &str) -> Option<rustkit_css::TransformList> {
    let value = value.trim();
    if value == "none" {
        return Some(rustkit_css::TransformList::none());
    }

    let mut ops = Vec::new();
    let mut remaining = value;

    while !remaining.is_empty() {
        remaining = remaining.trim_start();

        // Find the function name
        if let Some(paren_pos) = remaining.find('(') {
            let func_name = &remaining[..paren_pos];
            let after_paren = &remaining[paren_pos + 1..];

            // Find matching closing paren
            if let Some(close_pos) = find_matching_paren(after_paren) {
                let args = &after_paren[..close_pos];
                remaining = &after_paren[close_pos + 1..];

                if let Some(op) = parse_transform_op(func_name, args) {
                    ops.push(op);
                }
            } else {
                break;
            }
        } else {
            break;
        }
    }

    if ops.is_empty() {
        None
    } else {
        Some(rustkit_css::TransformList { ops })
    }
}

/// Parse a single transform operation.
fn parse_transform_op(func: &str, args: &str) -> Option<rustkit_css::TransformOp> {
    let args = args.trim();
    let parts: Vec<&str> = args.split(',').map(|s| s.trim()).collect();

    match func.trim() {
        "translate" => {
            let x = parse_length(parts.first()?)?;
            let y = parts
                .get(1)
                .and_then(|s| parse_length(s))
                .unwrap_or(rustkit_css::Length::Zero);
            Some(rustkit_css::TransformOp::Translate(x, y))
        }
        "translateX" => {
            let x = parse_length(parts.first()?)?;
            Some(rustkit_css::TransformOp::TranslateX(x))
        }
        "translateY" => {
            let y = parse_length(parts.first()?)?;
            Some(rustkit_css::TransformOp::TranslateY(y))
        }
        "scale" => {
            let sx = parts.first()?.parse::<f32>().ok()?;
            let sy = parts
                .get(1)
                .and_then(|s| s.parse::<f32>().ok())
                .unwrap_or(sx);
            Some(rustkit_css::TransformOp::Scale(sx, sy))
        }
        "scaleX" => {
            let s = parts.first()?.parse::<f32>().ok()?;
            Some(rustkit_css::TransformOp::ScaleX(s))
        }
        "scaleY" => {
            let s = parts.first()?.parse::<f32>().ok()?;
            Some(rustkit_css::TransformOp::ScaleY(s))
        }
        "rotate" => {
            let angle = parse_angle(parts.first()?)?;
            Some(rustkit_css::TransformOp::Rotate(angle))
        }
        "skew" => {
            let ax = parse_angle(parts.first()?)?;
            let ay = parts.get(1).and_then(|s| parse_angle(s)).unwrap_or(0.0);
            Some(rustkit_css::TransformOp::Skew(ax, ay))
        }
        "skewX" => {
            let angle = parse_angle(parts.first()?)?;
            Some(rustkit_css::TransformOp::SkewX(angle))
        }
        "skewY" => {
            let angle = parse_angle(parts.first()?)?;
            Some(rustkit_css::TransformOp::SkewY(angle))
        }
        "matrix" => {
            if parts.len() >= 6 {
                let a = parts[0].parse::<f32>().ok()?;
                let b = parts[1].parse::<f32>().ok()?;
                let c = parts[2].parse::<f32>().ok()?;
                let d = parts[3].parse::<f32>().ok()?;
                let e = parts[4].parse::<f32>().ok()?;
                let f = parts[5].parse::<f32>().ok()?;
                Some(rustkit_css::TransformOp::Matrix(a, b, c, d, e, f))
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse a CSS angle value (e.g., "45deg", "1rad", "0.5turn") into degrees.
fn parse_angle(value: &str) -> Option<f32> {
    let value = value.trim();
    // Suffixes are tested LONGEST-FIRST because they overlap: "grad" ends with
    // "rad". Testing "rad" first made the grad branch below unreachable - it
    // stripped "rad" from "200grad", leaving "200g", which fails to parse, and
    // the `?`-style .ok() dropped the whole declaration. Identical shape to the
    // rem-before-em bug fixed in #38: an overlapping-suffix chain in the wrong
    // order silently discards the longer unit.
    if value.ends_with("grad") {
        value[..value.len() - 4]
            .parse::<f32>()
            .ok()
            .map(|g| g * 0.9)
    } else if value.ends_with("turn") {
        value[..value.len() - 4]
            .parse::<f32>()
            .ok()
            .map(|t| t * 360.0)
    } else if value.ends_with("deg") {
        value[..value.len() - 3].parse().ok()
    } else if value.ends_with("rad") {
        value[..value.len() - 3]
            .parse::<f32>()
            .ok()
            .map(|r| r.to_degrees())
    } else {
        // Try parsing as number (defaults to degrees)
        value.parse().ok()
    }
}

/// Parse transform-origin value.
fn parse_transform_origin(value: &str) -> Option<rustkit_css::TransformOrigin> {
    let parts: Vec<&str> = value.split_whitespace().collect();

    let parse_component = |s: &str| -> Option<rustkit_css::Length> {
        match s {
            "left" => Some(rustkit_css::Length::Percent(0.0)),
            "center" => Some(rustkit_css::Length::Percent(50.0)),
            "right" => Some(rustkit_css::Length::Percent(100.0)),
            "top" => Some(rustkit_css::Length::Percent(0.0)),
            "bottom" => Some(rustkit_css::Length::Percent(100.0)),
            _ => parse_length(s),
        }
    };

    match parts.len() {
        1 => {
            let x = parse_component(parts[0])?;
            Some(rustkit_css::TransformOrigin {
                x,
                y: rustkit_css::Length::Percent(50.0),
            })
        }
        2 | 3 => {
            let x = parse_component(parts[0])?;
            let y = parse_component(parts[1])?;
            Some(rustkit_css::TransformOrigin { x, y })
        }
        _ => None,
    }
}

/// Supports: offset-x offset-y [blur [spread]] color [inset]
fn parse_box_shadow(value: &str) -> Option<rustkit_css::BoxShadow> {
    let value = value.trim();
    if value.is_empty() || value == "none" {
        return None;
    }

    let mut shadow = rustkit_css::BoxShadow::new();

    // Check for "inset" keyword
    let (value, inset) = if value.starts_with("inset") {
        // SAFETY: strip_prefix will succeed because we just checked starts_with("inset")
        (value.strip_prefix("inset").unwrap().trim(), true)
    } else if value.ends_with("inset") {
        // SAFETY: strip_suffix will succeed because we just checked ends_with("inset")
        (value.strip_suffix("inset").unwrap().trim(), true)
    } else {
        (value, false)
    };
    shadow.inset = inset;

    // Split into tokens, being careful about rgba() which contains commas
    let mut parts: Vec<&str> = Vec::new();
    let mut current_start = 0;
    let mut paren_depth = 0;

    for (i, ch) in value.char_indices() {
        match ch {
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            ' ' if paren_depth == 0 => {
                let part = value[current_start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
                current_start = i + 1;
            }
            _ => {}
        }
    }
    // Don't forget the last part
    let last_part = value[current_start..].trim();
    if !last_part.is_empty() {
        parts.push(last_part);
    }

    // Parse parts: expect at least 2 lengths + 1 color
    // Format: offset-x offset-y [blur [spread]] color
    let mut lengths: Vec<f32> = Vec::new();
    let mut color_value = None;

    for part in parts {
        // Try as length first
        if let Some(length) = parse_length(part) {
            lengths.push(length.to_px(16.0, 16.0, 0.0));
        } else {
            // Must be a color
            if let Some(c) = parse_color(part) {
                color_value = Some(c);
            }
        }
    }

    // Assign lengths
    if lengths.len() >= 2 {
        shadow.offset_x = lengths[0];
        shadow.offset_y = lengths[1];
    } else {
        return None; // Need at least offset-x and offset-y
    }

    if lengths.len() >= 3 {
        shadow.blur_radius = lengths[2].max(0.0);
    }

    if lengths.len() >= 4 {
        shadow.spread_radius = lengths[3];
    }

    // Set color
    shadow.color = color_value.unwrap_or(rustkit_css::Color::new(0, 0, 0, 0.5));

    Some(shadow)
}

/// Parse a CSS time value (e.g., "0.3s", "300ms") into seconds.
fn parse_time(value: &str) -> Option<f32> {
    let value = value.trim();
    if value.ends_with("ms") {
        value[..value.len() - 2]
            .parse::<f32>()
            .ok()
            .map(|v| v / 1000.0)
    } else if value.ends_with('s') {
        value[..value.len() - 1].parse::<f32>().ok()
    } else {
        None
    }
}

/// Parse a CSS timing function.
fn parse_timing_function(value: &str) -> rustkit_css::TimingFunction {
    let value = value.trim();
    match value {
        "ease" => rustkit_css::TimingFunction::Ease,
        "linear" => rustkit_css::TimingFunction::Linear,
        "ease-in" => rustkit_css::TimingFunction::EaseIn,
        "ease-out" => rustkit_css::TimingFunction::EaseOut,
        "ease-in-out" => rustkit_css::TimingFunction::EaseInOut,
        "step-start" => rustkit_css::TimingFunction::StepStart,
        "step-end" => rustkit_css::TimingFunction::StepEnd,
        _ if value.starts_with("cubic-bezier(") => {
            // Parse cubic-bezier(x1, y1, x2, y2)
            let inner = value
                .trim_start_matches("cubic-bezier(")
                .trim_end_matches(')');
            let parts: Vec<f32> = inner
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();
            if parts.len() == 4 {
                rustkit_css::TimingFunction::CubicBezier(parts[0], parts[1], parts[2], parts[3])
            } else {
                rustkit_css::TimingFunction::Ease
            }
        }
        _ if value.starts_with("steps(") => {
            // Parse steps(count, jump-start|jump-end)
            let inner = value.trim_start_matches("steps(").trim_end_matches(')');
            let parts: Vec<&str> = inner.split(',').map(|s| s.trim()).collect();
            if let Some(count) = parts.first().and_then(|s| s.parse::<u32>().ok()) {
                let jump_start = parts
                    .get(1)
                    .map(|s| *s == "jump-start" || *s == "start")
                    .unwrap_or(false);
                rustkit_css::TimingFunction::Steps(count, jump_start)
            } else {
                rustkit_css::TimingFunction::StepEnd
            }
        }
        _ => rustkit_css::TimingFunction::Ease,
    }
}

/// Parse an `overflow` keyword.
///
/// An unknown keyword yields the CSS initial (`visible`) rather than leaving a
/// previous value in place, matching how the other keyword arms behave.
fn parse_overflow(value: &str) -> rustkit_css::Overflow {
    match value.trim().to_lowercase().as_str() {
        "hidden" => rustkit_css::Overflow::Hidden,
        "scroll" => rustkit_css::Overflow::Scroll,
        "auto" => rustkit_css::Overflow::Auto,
        "clip" => rustkit_css::Overflow::Clip,
        _ => rustkit_css::Overflow::Visible,
    }
}

/// Parse a `flex-basis` value.
///
/// `em` is REFUSED rather than approximated: `FlexBasis::Length` holds a bare
/// f32 with no unit, so silently storing `2em` as `2px` would be a wrong number
/// that looks like a measurement. Refusing leaves the previous value, which is
/// visible as "the property did nothing" rather than as a subtly wrong layout.
fn parse_flex_basis(value: &str) -> rustkit_css::FlexBasis {
    let v = value.trim();
    if v.eq_ignore_ascii_case("auto") {
        return rustkit_css::FlexBasis::Auto;
    }
    if v.eq_ignore_ascii_case("content") {
        return rustkit_css::FlexBasis::Content;
    }
    match parse_length(v) {
        Some(rustkit_css::Length::Px(px)) => rustkit_css::FlexBasis::Length(px),
        Some(rustkit_css::Length::Percent(pct)) => rustkit_css::FlexBasis::Percent(pct),
        _ => rustkit_css::FlexBasis::Auto,
    }
}

fn parse_length(value: &str) -> Option<rustkit_css::Length> {
    let value = value.trim();

    if value == "0" || value == "auto" {
        return Some(if value == "auto" {
            rustkit_css::Length::Auto
        } else {
            rustkit_css::Length::Zero
        });
    }

    if value.ends_with("px") {
        let num: f32 = value.trim_end_matches("px").trim().parse().ok()?;
        return Some(rustkit_css::Length::Px(num));
    }

    // Check "rem" before "em" since "rem" ends with "em"
    if value.ends_with("rem") {
        let num: f32 = value.trim_end_matches("rem").trim().parse().ok()?;
        return Some(rustkit_css::Length::Rem(num));
    }

    if value.ends_with("em") {
        let num: f32 = value.trim_end_matches("em").trim().parse().ok()?;
        return Some(rustkit_css::Length::Em(num));
    }

    if value.ends_with('%') {
        let num: f32 = value.trim_end_matches('%').trim().parse().ok()?;
        return Some(rustkit_css::Length::Percent(num));
    }

    // Bare number (treat as pixels)
    if let Ok(num) = value.parse::<f32>() {
        return Some(rustkit_css::Length::Px(num));
    }

    None
}

/// Construct a `Compositor` for tests with **initialisation serialised**.
///
/// `Compositor::new()` performs wgpu adapter init. cargo runs tests in
/// parallel, so many of these can execute concurrently - which SIGSEGVs on
/// Linux (hiwave-linux #21, found by Argos) and is merely tolerated on
/// Windows. The lock is held across the init ONLY and released when this
/// function returns, so the tests themselves still run in parallel.
/// Poison-tolerant: one panicking test must not brick every later one.
///
/// The cascade wire suites no longer need this at all - they call
/// `Engine::apply_declaration` directly and allocate no GPU. This exists for
/// the tests that genuinely need a real Engine.
#[cfg(test)]
fn test_compositor() -> Compositor {
    static ENGINE_INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _init_guard = ENGINE_INIT.lock().unwrap_or_else(|e| e.into_inner());
    Compositor::new().expect("failed to create compositor for test")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_view_id_uniqueness() {
        let id1 = EngineViewId::new();
        let id2 = EngineViewId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_engine_config_default() {
        let config = EngineConfig::default();
        assert!(config.javascript_enabled);
        assert!(config.cookies_enabled);
    }

    #[test]
    fn test_engine_builder() {
        let builder = EngineBuilder::new()
            .user_agent("Test/1.0")
            .javascript_enabled(false);

        assert_eq!(builder.config.user_agent, "Test/1.0");
        assert!(!builder.config.javascript_enabled);
    }

    #[test]
    fn test_layout_tree_from_document() {
        // Parse a simple HTML document
        let html = r#"<!DOCTYPE html>
            <html>
            <head><title>Test</title></head>
            <body>
                <h1>Hello World</h1>
                <p>This is a paragraph.</p>
            </body>
            </html>"#;
        
        let document = Document::parse_html(html).expect("Failed to parse HTML");
        let document = Rc::new(document);
        
        // Verify document structure
        assert!(document.body().is_some(), "Document should have a body");
        
        // Create a dummy engine using the new() constructor pattern
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("Failed to create loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        
        // Build layout tree from document
        let layout = engine.build_layout_from_document(&document);
        
        // Verify layout tree is not empty
        assert!(!layout.children.is_empty(), "Layout tree should have children from body");
        
        // The body should contain h1 and p elements
        let body_box = &layout.children[0];
        
        // Count text boxes (h1 content "Hello World" and p content "This is a paragraph.")
        fn count_text_boxes(layout_box: &LayoutBox) -> usize {
            let mut count = if matches!(layout_box.box_type, BoxType::Text(_)) {
                1
            } else {
                0
            };
            for child in &layout_box.children {
                count += count_text_boxes(child);
            }
            count
        }
        
        let text_count = count_text_boxes(body_box);
        assert!(text_count >= 2, "Should have at least 2 text boxes (h1 and p content), got {}", text_count);
    }

    #[test]
    fn test_display_list_generation() {
        // Parse a document with styled content
        let html = r#"<!DOCTYPE html>
            <html>
            <body style="background-color: white">
                <h1>Title</h1>
            </body>
            </html>"#;
        
        let document = Document::parse_html(html).expect("Failed to parse HTML");
        let document = Rc::new(document);
        
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("Failed to create loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        
        let mut layout = engine.build_layout_from_document(&document);
        
        // Perform layout with a containing block
        let containing_block = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout.layout(&containing_block);
        
        // Generate display list
        let display_list = DisplayList::build(&layout);
        
        // Display list should have commands (at least background colors)
        assert!(!display_list.commands.is_empty(), "Display list should have commands, got {:?}", display_list.commands);
    }

    // W55-A (port of macOS #55): bare form controls compute a UA
    // `display: inline-block`, so three sibling <button>s share ONE line.
    // Pre-fix they inherit the Block default and stack vertically, making the
    // body ~3 button-rows tall; inline-block collapses them to a single row.
    #[test]
    fn test_button_ua_display_inline_block_one_line() {
        let html = r#"<!DOCTYPE html><html><body><button>A</button><button>B</button><button>C</button></body></html>"#;
        let document = Document::parse_html(html).expect("Failed to parse HTML");
        let document = Rc::new(document);

        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("Failed to create loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };

        let mut layout = engine.build_layout_from_document(&document);
        let containing_block = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout.layout(&containing_block);

        let body = &layout.children[0];
        let body_h = body.dimensions.content.height;

        // Every button must carry the UA inline-block display (the arm fired).
        fn buttons_are_inline_block(b: &LayoutBox, ib: &mut usize, total: &mut usize) {
            if matches!(b.box_type, BoxType::Inline | BoxType::Block) {
                // A button box is one styled inline-block; count via display.
            }
            for c in &b.children {
                buttons_are_inline_block(c, ib, total);
            }
            if b.style.display == rustkit_css::Display::InlineBlock {
                *ib += 1;
            }
        }
        let (mut ib, mut total) = (0usize, 0usize);
        buttons_are_inline_block(body, &mut ib, &mut total);
        assert!(ib >= 3, "expected >=3 inline-block form-control boxes, got {ib}");

        // One line: the body hugs a single button row, not three stacked. A
        // single bare button is well under 60px tall, so one line is < 60 and
        // three stacked is comfortably over it.
        assert!(
            body_h < 60.0,
            "three bare buttons should share one line (body height {body_h} \
             indicates vertical stacking)"
        );
    }

    // W55-B (port of macOS #55): bare form controls get Chrome-oracle
    // intrinsic border-box sizes at the UA control font (13.333px):
    // single-line input 19px tall, checkbox/radio 13x13, textarea 15*rows+2.
    // Pre-fix a bare control hugs line-height (~16px) with no width.
    fn first_form_control_dims(html: &str) -> (f32, f32) {
        let document = Document::parse_html(html).expect("Failed to parse HTML");
        let document = Rc::new(document);
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("Failed to create loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let mut layout = engine.build_layout_from_document(&document);
        let containing_block = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout.layout(&containing_block);
        fn find(b: &LayoutBox) -> Option<(f32, f32)> {
            if b.style.display == rustkit_css::Display::InlineBlock {
                return Some((b.dimensions.content.width, b.dimensions.content.height));
            }
            for c in &b.children {
                if let Some(d) = find(c) {
                    return Some(d);
                }
            }
            None
        }
        find(&layout).expect("no inline-block form control found")
    }

    #[test]
    fn test_bare_form_control_heights_match_chrome() {
        let (_iw, ih) = first_form_control_dims(
            r#"<!DOCTYPE html><html><body><input></body></html>"#);
        assert!((ih - 19.0).abs() < 0.5, "bare input height {ih}, expected ~19");

        let (cw, ch) = first_form_control_dims(
            r#"<!DOCTYPE html><html><body><input type="checkbox"></body></html>"#);
        assert!((cw - 13.0).abs() < 0.5 && (ch - 13.0).abs() < 0.5,
            "checkbox {cw}x{ch}, expected 13x13");

        let (_tw, th) = first_form_control_dims(
            r#"<!DOCTYPE html><html><body><textarea></textarea></body></html>"#);
        // rows default 2 -> 15*2 + 2 = 32.
        assert!((th - 32.0).abs() < 0.5, "bare textarea height {th}, expected ~32");

        // Author rows=1 is honored (not floored to the default 2): 15*1+2=17.
        let (_tw1, th1) = first_form_control_dims(
            r#"<!DOCTYPE html><html><body><textarea rows="1"></textarea></body></html>"#);
        assert!((th1 - 17.0).abs() < 0.5, "rows=1 textarea height {th1}, expected ~17");
    }

    // W56 (port of macOS #56): `line-height: normal` derives from font metrics
    // (Blink `ascent + descent + line_gap`; the Windows TextMetrics estimate is
    // ~1.15*font_size), NOT a hardcoded 1.2 ratio. A single line of text is
    // exactly one line-height tall.
    fn first_text_box_height(html: &str) -> f32 {
        let document = Document::parse_html(html).expect("Failed to parse HTML");
        let document = Rc::new(document);
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("Failed to create loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let mut layout = engine.build_layout_from_document(&document);
        let containing_block = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };
        layout.layout(&containing_block);
        fn find(b: &LayoutBox) -> Option<f32> {
            if matches!(b.box_type, BoxType::Text(_)) {
                return Some(b.dimensions.content.height);
            }
            for c in &b.children {
                if let Some(h) = find(c) {
                    return Some(h);
                }
            }
            None
        }
        find(&layout).expect("no text box found")
    }

    #[test]
    fn test_line_height_normal_from_metrics() {
        // Default font 16px, line-height:normal -> metrics 1.15*16 = 18.4,
        // NOT the old flat 1.2*16 = 19.2.
        let h = first_text_box_height(
            r#"<!DOCTYPE html><html><body><p>one line of text</p></body></html>"#);
        assert!((h - 18.4).abs() < 0.6,
            "normal line-height {h}, expected ~18.4 (metrics-derived, not flat 19.2)");

        // An explicit numeric line-height is still an exact ratio of font-size.
        let h2 = first_text_box_height(
            r#"<!DOCTYPE html><html><body><p style="line-height: 2">two</p></body></html>"#);
        assert!((h2 - 32.0).abs() < 0.5,
            "line-height:2 should be 2*16=32, got {h2}");
    }

    #[test]
    fn test_parse_color() {
        // Test named colors
        assert_eq!(parse_color("black"), Some(rustkit_css::Color::BLACK));
        assert_eq!(parse_color("white"), Some(rustkit_css::Color::WHITE));
        
        // Test hex colors
        assert_eq!(parse_color("#fff"), Some(rustkit_css::Color::from_rgb(255, 255, 255)));
        assert_eq!(parse_color("#000000"), Some(rustkit_css::Color::from_rgb(0, 0, 0)));
        assert_eq!(parse_color("#ff0000"), Some(rustkit_css::Color::from_rgb(255, 0, 0)));
        
        // Test rgb colors
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some(rustkit_css::Color::new(255, 0, 0, 1.0)));
    }

    #[test]
    fn test_resolve_var_refs() {
        let mut vars = HashMap::new();
        vars.insert("--bg".to_string(), "#0f172a".to_string());
        vars.insert("--accent".to_string(), "#06b6d4".to_string());
        vars.insert("--alias".to_string(), "var(--accent)".to_string());

        assert_eq!(resolve_var_refs("var(--bg)", &vars), "#0f172a");
        assert_eq!(
            resolve_var_refs("1px solid var(--accent)", &vars),
            "1px solid #06b6d4"
        );
        assert_eq!(resolve_var_refs("var(--missing, red)", &vars), "red");
        assert_eq!(resolve_var_refs("var(--bg, red)", &vars), "#0f172a");
        assert_eq!(resolve_var_refs("var(--alias)", &vars), "#06b6d4");
        assert_eq!(
            resolve_var_refs("var(--missing, rgb(1, 2, 3))", &vars),
            "rgb(1, 2, 3)"
        );
        assert_eq!(resolve_var_refs("#fff", &vars), "#fff");
    }

    #[test]
    fn test_parse_linear_gradient() {
        // Direction keyword + two stops.
        let g = parse_linear_gradient("linear-gradient(to right, #ff0000, #0000ff)").unwrap();
        assert_eq!(g.angle_deg, 90.0);
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].color, rustkit_css::Color::from_rgb(255, 0, 0));
        assert_eq!(g.stops[0].position, 0.0);
        assert_eq!(g.stops[1].position, 1.0);

        // Explicit angle + explicit stop positions, including an rgb() stop with
        // internal commas (must not be split).
        let g = parse_linear_gradient(
            "linear-gradient(135deg, #667eea 0%, rgb(118, 75, 162) 100%)",
        )
        .unwrap();
        assert_eq!(g.angle_deg, 135.0);
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[1].color, rustkit_css::Color::from_rgb(118, 75, 162));

        // A layered background's base color is the solid layer, not a gradient stop.
        assert_eq!(
            background_base_color("radial-gradient(circle, #fff, #000), #1a1a2e"),
            Some(rustkit_css::Color::from_rgb(0x1a, 0x1a, 0x2e))
        );
    }

    #[test]
    fn test_parse_radial_gradient() {
        use rustkit_css::RadialShape;

        // circle + `at center` + explicit stop positions.
        let g = parse_radial_gradient(
            "radial-gradient(circle at center, #667eea 0%, #764ba2 100%)",
        )
        .unwrap();
        assert_eq!(g.shape, RadialShape::Circle);
        assert_eq!((g.cx, g.cy), (0.5, 0.5));
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].color, rustkit_css::Color::from_rgb(0x66, 0x7e, 0xea));
        assert_eq!(g.stops[0].position, 0.0);
        assert_eq!(g.stops[1].position, 1.0);

        // ellipse (default kept) + corner keyword position (order-independent).
        let g = parse_radial_gradient(
            "radial-gradient(ellipse at bottom right, #f093fb 0%, #f5576c 100%)",
        )
        .unwrap();
        assert_eq!(g.shape, RadialShape::Ellipse);
        assert_eq!((g.cx, g.cy), (1.0, 1.0));

        // No config prefix → defaults (ellipse, center); stops start immediately.
        let g = parse_radial_gradient("radial-gradient(#fff, #000)").unwrap();
        assert_eq!(g.shape, RadialShape::Ellipse);
        assert_eq!((g.cx, g.cy), (0.5, 0.5));
        assert_eq!(g.stops.len(), 2);

        // Percentage position + an rgba() stop with internal commas (not split).
        let g = parse_radial_gradient(
            "radial-gradient(circle at 20% 80%, rgba(255,255,255,0.3) 0%, transparent 50%)",
        )
        .unwrap();
        assert_eq!((g.cx, g.cy), (0.2, 0.8));
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[1].position, 0.5);
    }

    #[test]
    fn test_parse_radial_position() {
        // Keyword pairs are order-independent; single keyword sets its own axis.
        assert_eq!(parse_radial_position("center"), (0.5, 0.5));
        assert_eq!(parse_radial_position("top left"), (0.0, 0.0));
        assert_eq!(parse_radial_position("left top"), (0.0, 0.0));
        assert_eq!(parse_radial_position("bottom right"), (1.0, 1.0));
        assert_eq!(parse_radial_position("top"), (0.5, 0.0));
        assert_eq!(parse_radial_position("right"), (1.0, 0.5));
        // Percentages are positional (first→x, second→y).
        assert_eq!(parse_radial_position("20% 80%"), (0.2, 0.8));
        assert_eq!(parse_radial_position("30%"), (0.3, 0.5));
    }

    #[test]
    fn test_background_clip_text_propagates_to_text_run() {
        // background-clip:text + a gradient on an element must reach the child
        // text run (neither property inherits) so the glyphs get the gradient.
        let html = "<html><head><style>.logo{background:linear-gradient(90deg,#ff0000,#0000ff);\
                    -webkit-background-clip:text;background-clip:text;color:transparent}</style></head>\
                    <body><h1 class=\"logo\">HIWAVE</h1></body></html>";
        let document = Rc::new(Document::parse_html(html).expect("parse"));
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let layout = engine.build_layout_from_document(&document);
        fn find_gradient_text(b: &rustkit_layout::LayoutBox) -> bool {
            let is_grad_text = matches!(b.box_type, rustkit_layout::BoxType::Text(_))
                && b.style.background_clip == rustkit_css::BackgroundClip::Text
                && b.style.background_gradient.is_some();
            is_grad_text || b.children.iter().any(find_gradient_text)
        }
        assert!(
            find_gradient_text(&layout),
            "the HIWAVE text run should carry background-clip:text and the gradient"
        );
    }

    #[test]
    fn test_body_background_propagates_to_canvas() {
        // CSS §14.2: the body's background becomes the canvas (viewport)
        // background — it must move onto the root box and be cleared from the
        // body, so a short page doesn't leave the viewport the canvas default.
        let html = "<html><head><style>body{background:#1a1a2e}</style></head>\
                    <body><p>x</p></body></html>";
        let document = Rc::new(Document::parse_html(html).expect("parse"));
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let root = engine.build_layout_from_document(&document);
        assert_eq!(
            root.style.background_color,
            rustkit_css::Color::from_rgb(0x1a, 0x1a, 0x2e),
            "canvas (root) should carry the body background"
        );
        let body = &root.children[0];
        assert_eq!(
            body.style.background_color,
            rustkit_css::Color::TRANSPARENT,
            "body background should be cleared after propagating to the canvas"
        );
    }

    #[test]
    fn test_root_custom_properties_reach_body() {
        // `:root { --x }` must flow to a <body> descendant via inheritance even
        // though the tree is built from <body> (the html element is not walked).
        let html = "<html><head><style>:root{--brand:#123456}</style></head>\
                    <body><p style=\"color: var(--brand)\">hi</p></body></html>";
        let document = Rc::new(Document::parse_html(html).expect("parse"));
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let layout = engine.build_layout_from_document(&document);
        fn find_colored(b: &rustkit_layout::LayoutBox) -> bool {
            b.style.color == rustkit_css::Color::from_rgb(0x12, 0x34, 0x56)
                || b.children.iter().any(find_colored)
        }
        assert!(
            find_colored(&layout),
            "var(--brand) from :root should resolve on a body descendant"
        );
    }

    #[test]
    fn test_text_align_inherits_to_block_child() {
        // A block child inherits its containing block's text-align unless it
        // sets its own (CSS cascade). Windows inherits uniformly via
        // ComputedStyle::inherit_from, so `<div style=text-align:center><h1>`
        // must center the h1. Portable contract from hiwave-macos #47; Windows
        // already satisfies it (inherit_from carries text_align, and h1's UA
        // defaults do not reset it) — this locks the contract against regression.
        let html = "<html><body><div style=\"text-align:center\"><h1>Hi</h1></div></body></html>";
        let document = Rc::new(Document::parse_html(html).expect("parse"));
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let layout = engine.build_layout_from_document(&document);
        // Identify the h1 block by its UA default font-size (32px) and assert it
        // inherited text-align:center from the containing div.
        fn find_h1_align(b: &rustkit_layout::LayoutBox) -> Option<rustkit_css::TextAlign> {
            if b.style.font_size == rustkit_css::Length::Px(32.0) {
                return Some(b.style.text_align);
            }
            for c in &b.children {
                if let Some(a) = find_h1_align(c) {
                    return Some(a);
                }
            }
            None
        }
        assert_eq!(
            find_h1_align(&layout),
            Some(rustkit_css::TextAlign::Center),
            "h1 should inherit text-align:center from its containing div"
        );
    }

    #[test]
    fn test_parse_length() {
        assert_eq!(parse_length("0"), Some(rustkit_css::Length::Zero));
        assert_eq!(parse_length("auto"), Some(rustkit_css::Length::Auto));
        assert_eq!(parse_length("10px"), Some(rustkit_css::Length::Px(10.0)));
        assert_eq!(parse_length("1.5em"), Some(rustkit_css::Length::Em(1.5)));
        assert_eq!(parse_length("2rem"), Some(rustkit_css::Length::Rem(2.0)));
        assert_eq!(parse_length("50%"), Some(rustkit_css::Length::Percent(50.0)));
    }

    fn ctx(tag: &str, class: &str) -> ElementCtx {
        ElementCtx {
            tag: tag.to_string(),
            classes: if class.is_empty() {
                vec![]
            } else {
                vec![class.to_string()]
            },
            id: None,
        }
    }

    #[test]
    fn test_descendant_selector_requires_ancestor() {
        // `.hero p` matches a <p> inside .hero, but must NOT match a bare <p>
        // elsewhere (the over-match that leaked text-align:center onto cards).
        let inside = [ctx("div", "hero")];
        let outside: [ElementCtx; 0] = [];
        assert!(Engine::selector_matches(".hero p", "p", &[], None, &inside).is_some());
        assert!(Engine::selector_matches(".hero p", "p", &[], None, &outside).is_none());
        // Subject still has to match the element itself.
        assert!(Engine::selector_matches(".hero p", "span", &[], None, &inside).is_none());
    }

    #[test]
    fn test_descendant_selector_specificity_sums_compounds() {
        // `.hero p` = .hero (10) + p (1) = 11, proving ancestor compounds are
        // counted (a bare `p` subject alone would be 1).
        let inside = [ctx("div", "hero")];
        assert_eq!(
            Engine::selector_matches(".hero p", "p", &[], None, &inside),
            Some(11)
        );
        assert_eq!(Engine::selector_matches("p", "p", &[], None, &inside), Some(1));
    }

    #[test]
    fn test_descendant_matches_non_adjacent_ancestor() {
        // Descendant (not child) — the .hero ancestor may be any depth up.
        let chain = [ctx("div", "hero"), ctx("section", "")];
        assert!(Engine::selector_matches(".hero p", "p", &[], None, &chain).is_some());
    }

    #[test]
    fn test_whitespace_between_siblings_makes_no_boxes() {
        // Newlines/indentation between sibling elements must not become boxes
        // (else a flex container counts them as phantom items).
        let html = "<body><div id=\"row\"><div>a</div>\n  <div>b</div>\n  </div></body>";
        let document = Rc::new(Document::parse_html(html).expect("parse"));
        let (event_tx, event_rx) = tokio::sync::mpsc::unbounded_channel();
        let engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(
                ResourceLoader::new(LoaderConfig::default()).expect("loader"),
            ),
            image_manager: Arc::new(ImageManager::new()),
            event_tx,
            event_rx: Some(event_rx),
        };
        let layout = engine.build_layout_from_document(&document);
        // root -> body -> #row -> [div a, div b]  (no whitespace phantoms)
        let body = &layout.children[0];
        let row = &body.children[0];
        assert_eq!(
            row.children.len(),
            2,
            "row should have exactly two element children, got {}",
            row.children.len()
        );
    }
}

#[cfg(test)]
mod transform_wire_tests {
    use super::*;
    use rustkit_css::{Length, TransformOp};


    // ---- parser correctness -------------------------------------------------

    #[test]
    fn parses_none_as_identity() {
        let t = parse_transform("none").expect("none must parse");
        assert!(t.is_identity());
    }

    #[test]
    fn parses_a_multi_op_transform_in_source_order() {
        // Order matters for composition, so the ops must not be reordered or
        // deduplicated by the parser.
        let t = parse_transform("translate(10px, 20px) scale(2) rotate(45deg)")
            .expect("multi-op must parse");
        assert_eq!(t.ops.len(), 3);
        assert!(matches!(t.ops[0], TransformOp::Translate(..)));
        assert!(matches!(t.ops[1], TransformOp::Scale(..)));
        assert!(matches!(t.ops[2], TransformOp::Rotate(_)));
    }

    #[test]
    fn scale_with_one_arg_applies_to_both_axes() {
        let t = parse_transform("scale(3)").expect("parse");
        match t.ops[0] {
            TransformOp::Scale(x, y) => assert_eq!((x, y), (3.0, 3.0)),
            ref other => panic!("expected Scale, got {:?}", other),
        }
    }

    #[test]
    fn angle_units_all_convert_to_degrees() {
        // A parser that only handled `deg` would pass the common case and
        // silently mis-render rad/turn/grad.
        assert_eq!(parse_angle("90deg"), Some(90.0));
        assert_eq!(parse_angle("1turn"), Some(360.0));
        // REGRESSION: "200grad".ends_with("rad") is true. With rad tested
        // first, the grad branch was unreachable and every grad angle became
        // None - dropping the whole transform declaration. Same shape as
        // rem-before-em (#38).
        assert_eq!(parse_angle("200grad"), Some(180.0));
        assert_eq!(parse_angle("100grad"), Some(90.0));
        assert_eq!(parse_angle("45"), Some(45.0), "bare number defaults to deg");
        let rad = parse_angle("3.14159265rad").expect("rad must parse");
        assert!((rad - 180.0).abs() < 0.01, "1 pi rad == 180deg, got {}", rad);
    }

    #[test]
    fn transform_origin_keywords_map_to_percentages() {
        let o = parse_transform_origin("left top").expect("parse");
        assert_eq!(o.x, Length::Percent(0.0));
        assert_eq!(o.y, Length::Percent(0.0));
        let c = parse_transform_origin("center").expect("parse");
        assert_eq!(c.x, Length::Percent(50.0));
        assert_eq!(c.y, Length::Percent(50.0), "single value defaults y to 50%");
    }

    #[test]
    fn garbage_does_not_panic_and_yields_none_or_identity() {
        for bad in ["translate(", "rotate(abc)", "notafunction(1)", ""] {
            let _ = parse_transform(bad);
        }
    }

    // ---- the WIRE: properties must now COMPUTE ------------------------------

    #[test]
    fn transform_declaration_computes_into_style() {
        // THIS is the wire receipt. Before this PR the declaration was dropped
        // on the floor: apply_declaration had no "transform" arm and
        // ComputedStyle had no field to hold it.
        let mut style = ComputedStyle::default();
        assert!(style.transform.is_identity(), "default must be identity");

        Engine::apply_declaration(&mut style, "transform", "scale(2)");
        assert!(
            !style.transform.is_identity(),
            "transform: scale(2) must compute into ComputedStyle"
        );
        assert_eq!(style.transform.ops.len(), 1);
    }

    #[test]
    fn transform_origin_declaration_computes_into_style() {
        let mut style = ComputedStyle::default();
        Engine::apply_declaration(&mut style, "transform-origin", "left top");
        assert_eq!(style.transform_origin.x, Length::Percent(0.0));
        assert_eq!(style.transform_origin.y, Length::Percent(0.0));
    }

    #[test]
    fn an_invalid_transform_leaves_the_previous_value_untouched() {
        // CSS: an invalid declaration is ignored, not reset to initial.
        let mut style = ComputedStyle::default();
        Engine::apply_declaration(&mut style, "transform", "scale(2)");
        let before = style.transform.ops.len();
        Engine::apply_declaration(&mut style, "transform", "!!!garbage!!!");
        assert_eq!(
            style.transform.ops.len(), before,
            "invalid value must not clobber the computed transform"
        );
    }
}

#[cfg(test)]
mod shadow_wire_tests {
    use super::*;
    use rustkit_css::Color;


    #[test]
    fn parses_offsets_blur_and_colour() {
        let s = parse_box_shadow("2px 4px 6px rgb(255, 0, 0)").expect("must parse");
        assert_eq!((s.offset_x, s.offset_y, s.blur_radius), (2.0, 4.0, 6.0));
        assert_eq!(s.color.r, 255);
        assert!(!s.inset);
    }

    #[test]
    fn rgba_commas_do_not_split_the_token_list() {
        // The parser tracks paren depth precisely because rgba() contains
        // commas and spaces; a naive split would shred the colour into
        // fragments and lose it.
        let s = parse_box_shadow("1px 2px 3px rgba(0, 0, 0, 0.5)").expect("must parse");
        assert_eq!((s.offset_x, s.offset_y, s.blur_radius), (1.0, 2.0, 3.0));
        assert!(s.color.a < 1.0, "alpha must survive, got {}", s.color.a);
    }

    #[test]
    fn inset_keyword_is_recognised() {
        let s = parse_box_shadow("0 0 4px #000 inset").expect("must parse");
        assert!(s.inset);
    }

    #[test]
    fn none_and_empty_yield_no_shadow() {
        assert!(parse_box_shadow("none").is_none());
        assert!(parse_box_shadow("").is_none());
    }

    // ---- the WIRE ----------------------------------------------------------

    #[test]
    fn box_shadow_declaration_computes_into_style() {
        let mut style = ComputedStyle::default();
        assert!(style.box_shadows.is_empty(), "default has no shadows");
        Engine::apply_declaration(&mut style, "box-shadow", "2px 4px 6px #000");
        assert_eq!(style.box_shadows.len(), 1, "box-shadow must compute");
        assert_eq!(style.box_shadows[0].offset_x, 2.0);
    }

    #[test]
    fn box_shadow_none_clears_a_previously_computed_shadow() {
        // A later rule must be able to cancel an earlier one. If `none` were
        // simply "parse fails, push nothing", the earlier shadow would
        // survive and the element would keep a shadow the author removed.
        let mut style = ComputedStyle::default();
        Engine::apply_declaration(&mut style, "box-shadow", "2px 4px 6px #000");
        assert_eq!(style.box_shadows.len(), 1);
        Engine::apply_declaration(&mut style, "box-shadow", "none");
        assert!(style.box_shadows.is_empty(), "none must clear the list");
    }

    #[test]
    fn shadow_is_visible_predicate_agrees_with_the_parsed_value() {
        // Ties the wire back to the INERT type's own logic from #37.
        let mut style = ComputedStyle::default();
        Engine::apply_declaration(&mut style, "box-shadow", "0 0 0 rgba(0,0,0,0)");
        if let Some(s) = style.box_shadows.first() {
            assert!(!s.is_visible(), "fully transparent, zero geometry: not visible");
        }
        let mut style2 = ComputedStyle::default();
        Engine::apply_declaration(&mut style2, "box-shadow", "3px 3px 5px #000");
        assert!(style2.box_shadows[0].is_visible());
    }
}

#[cfg(test)]
mod animation_wire_tests {
    use super::*;
    use rustkit_css::{AnimationDirection, AnimationFillMode, AnimationIterationCount,
                      AnimationPlayState, TimingFunction};


    #[test]
    fn ms_and_s_both_convert_to_seconds() {
        // "500ms" ends with "s" too - parse_time must test the LONGER suffix
        // first. Verified correct on arrival (see #146 sweep), pinned here so
        // a future edit cannot reintroduce the grad/rad class of bug.
        assert_eq!(parse_time("500ms"), Some(0.5));
        assert_eq!(parse_time("2s"), Some(2.0));
        assert_eq!(parse_time("0.25s"), Some(0.25));
    }

    #[test]
    fn timing_function_keywords_and_cubic_bezier_parse() {
        assert_eq!(parse_timing_function("linear"), TimingFunction::Linear);
        assert_eq!(parse_timing_function("ease-in-out"), TimingFunction::EaseInOut);
        match parse_timing_function("cubic-bezier(0.25, 0.1, 0.25, 1)") {
            TimingFunction::CubicBezier(a, b, c, d) => {
                assert_eq!((a, b, c, d), (0.25, 0.1, 0.25, 1.0));
            }
            other => panic!("expected CubicBezier, got {:?}", other),
        }
    }

    #[test]
    fn an_unknown_timing_function_falls_back_to_the_css_initial() {
        assert_eq!(parse_timing_function("not-a-function"), TimingFunction::Ease);
    }

    // ---- the WIRE ----------------------------------------------------------

    #[test]
    fn animation_shorthand_longhands_compute() {
        let mut s = ComputedStyle::default();
        Engine::apply_declaration(&mut s, "animation-name", "slide");
        Engine::apply_declaration(&mut s, "animation-duration", "250ms");
        Engine::apply_declaration(&mut s, "animation-timing-function", "ease-in");
        Engine::apply_declaration(&mut s, "animation-delay", "1s");
        assert_eq!(s.animation_name, "slide");
        assert_eq!(s.animation_duration, 0.25, "ms must convert to seconds");
        assert_eq!(s.animation_timing_function, TimingFunction::EaseIn);
        assert_eq!(s.animation_delay, 1.0);
    }

    #[test]
    fn iteration_count_infinite_is_not_a_number() {
        let mut s = ComputedStyle::default();
        Engine::apply_declaration(&mut s, "animation-iteration-count", "infinite");
        assert_eq!(s.animation_iteration_count, AnimationIterationCount::Infinite);

        let mut s2 = ComputedStyle::default();
        Engine::apply_declaration(&mut s2, "animation-iteration-count", "2.5");
        assert_eq!(s2.animation_iteration_count, AnimationIterationCount::Count(2.5),
                   "fractional counts are legal CSS and must survive");
    }

    #[test]
    fn direction_fill_mode_and_play_state_compute() {
        let mut s = ComputedStyle::default();
        Engine::apply_declaration(&mut s, "animation-direction", "alternate-reverse");
        Engine::apply_declaration(&mut s, "animation-fill-mode", "both");
        Engine::apply_declaration(&mut s, "animation-play-state", "paused");
        assert_eq!(s.animation_direction, AnimationDirection::AlternateReverse);
        assert_eq!(s.animation_fill_mode, AnimationFillMode::Both);
        assert_eq!(s.animation_play_state, AnimationPlayState::Paused);
    }

    #[test]
    fn transition_longhands_compute_independently_of_animation() {
        // The two families share TimingFunction; a wire that crossed them
        // would be invisible unless both are asserted in one test.
        let mut s = ComputedStyle::default();
        Engine::apply_declaration(&mut s, "transition-property", "opacity");
        Engine::apply_declaration(&mut s, "transition-duration", "300ms");
        Engine::apply_declaration(&mut s, "transition-timing-function", "linear");
        assert_eq!(s.transition_property, "opacity");
        assert_eq!(s.transition_duration, 0.3);
        assert_eq!(s.transition_timing_function, TimingFunction::Linear);
        assert_eq!(s.animation_duration, 0.0, "animation must be untouched");
        assert_eq!(s.animation_timing_function, TimingFunction::Ease);
    }

    #[test]
    fn defaults_are_the_css_initial_values() {
        let s = ComputedStyle::default();
        assert_eq!(s.animation_duration, 0.0);
        assert_eq!(s.animation_iteration_count, AnimationIterationCount::One);
        assert_eq!(s.animation_play_state, AnimationPlayState::Running);
        assert_eq!(s.animation_fill_mode, AnimationFillMode::None);
    }
}

#[cfg(test)]
mod external_stylesheet_tests {
    use super::*;

    fn doc(html: &str) -> Document {
        Document::parse_html(html).expect("parse")
    }

    fn base() -> Url {
        Url::parse("https://example.com/dir/page.html").unwrap()
    }

    #[test]
    fn relative_href_resolves_against_the_document_url() {
        // The whole point of carrying base_url: "site.css" next to the page is
        // the overwhelmingly common authoring form.
        let d = doc(r#"<html><head><link rel="stylesheet" href="site.css"></head><body></body></html>"#);
        let urls = Engine::discover_external_stylesheets(&d, Some(&base()));
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://example.com/dir/site.css");
    }

    #[test]
    fn root_relative_and_absolute_hrefs_both_resolve() {
        let d = doc(
            r#"<html><head>
               <link rel="stylesheet" href="/a.css">
               <link rel="stylesheet" href="https://cdn.example.org/b.css">
               </head><body></body></html>"#,
        );
        let urls = Engine::discover_external_stylesheets(&d, Some(&base()));
        let got: Vec<&str> = urls.iter().map(|u| u.as_str()).collect();
        assert!(got.contains(&"https://example.com/a.css"), "got {got:?}");
        assert!(got.contains(&"https://cdn.example.org/b.css"), "got {got:?}");
    }

    #[test]
    fn non_stylesheet_links_are_ignored() {
        // <link> is used for icons, preconnect, manifests. Treating every
        // <link href> as CSS would fetch the favicon and try to parse it.
        let d = doc(
            r#"<html><head>
               <link rel="icon" href="favicon.ico">
               <link rel="preconnect" href="https://fonts.example.com">
               <link rel="manifest" href="app.webmanifest">
               </head><body></body></html>"#,
        );
        assert!(Engine::discover_external_stylesheets(&d, Some(&base())).is_empty());
    }

    #[test]
    fn rel_matching_is_case_insensitive_and_token_wise() {
        // `rel` is an unordered SET. "alternate stylesheet" contains the token,
        // and REL="StyleSheet" is legal HTML. A whole-attribute equality check
        // would silently drop both.
        let d = doc(
            r#"<html><head>
               <link rel="StyleSheet" href="a.css">
               <link rel="alternate stylesheet" href="b.css">
               </head><body></body></html>"#,
        );
        assert_eq!(Engine::discover_external_stylesheets(&d, Some(&base())).len(), 2);
    }

    #[test]
    fn missing_empty_and_unresolvable_hrefs_are_skipped_not_guessed() {
        let d = doc(
            r#"<html><head>
               <link rel="stylesheet">
               <link rel="stylesheet" href="">
               <link rel="stylesheet" href="   ">
               </head><body></body></html>"#,
        );
        assert!(Engine::discover_external_stylesheets(&d, Some(&base())).is_empty());
    }

    #[test]
    fn without_a_base_url_only_absolute_hrefs_resolve() {
        // load_html has no document URL. A relative href genuinely cannot be
        // resolved then - it must be dropped, never guessed relative to cwd.
        let d = doc(
            r#"<html><head>
               <link rel="stylesheet" href="relative.css">
               <link rel="stylesheet" href="https://cdn.example.org/abs.css">
               </head><body></body></html>"#,
        );
        let urls = Engine::discover_external_stylesheets(&d, None);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0].as_str(), "https://cdn.example.org/abs.css");
    }

    #[test]
    fn external_css_participates_in_the_cascade() {
        // The load-bearing test: before this change external CSS could not
        // reach the cascade at all, because only <style> text was collected.
        let e = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(
                ResourceLoader::new(LoaderConfig::default()).expect("loader"),
            ),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        };
        let d = doc(r#"<html><body><div id="t">x</div></body></html>"#);

        // Walk the tree: `#t` is a DESCENDANT, so asserting on the root box
        // would compare the document root's style and pass vacuously.
        fn widths(b: &LayoutBox, out: &mut Vec<String>) {
            out.push(format!("{:?}", b.style.width));
            for c in &b.children {
                widths(c, out);
            }
        }
        let mut without = Vec::new();
        widths(&e.build_layout_from_document(&d), &mut without);
        let mut with = Vec::new();
        widths(
            &e.build_layout_with_external_css(&d, "#t { width: 123px }"),
            &mut with,
        );

        assert_ne!(
            with, without,
            "external CSS must change the computed tree; if these match the sheet never reached the cascade"
        );
        assert!(
            with.iter().any(|w| w.contains("123")),
            "expected a box to compute width:123px from the external sheet, got {with:?}"
        );
    }

    #[test]
    fn empty_external_css_leaves_the_cascade_untouched() {
        let e = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(
                ResourceLoader::new(LoaderConfig::default()).expect("loader"),
            ),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        };
        let d = doc(r#"<html><head><style>#t{width:7px}</style></head><body><div id="t">x</div></body></html>"#);
        fn widths(b: &LayoutBox, out: &mut Vec<String>) {
            out.push(format!("{:?}", b.style.width));
            for c in &b.children {
                widths(c, out);
            }
        }
        let mut a = Vec::new();
        widths(&e.build_layout_from_document(&d), &mut a);
        let mut b = Vec::new();
        widths(&e.build_layout_with_external_css(&d, ""), &mut b);
        assert_eq!(a, b, "empty external CSS must be a no-op");
    }
}

#[cfg(test)]
mod image_subresource_tests {
    use super::*;

    fn doc(html: &str) -> Document {
        Document::parse_html(html).expect("parse")
    }

    fn base() -> Url {
        Url::parse("https://example.com/dir/page.html").unwrap()
    }

    #[test]
    fn relative_src_resolves_against_the_document_url() {
        let d = doc(r#"<html><body><img src="cat.png"></body></html>"#);
        let found = Engine::discover_images(&d, Some(&base()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, "cat.png", "raw src must be preserved");
        assert_eq!(found[0].1.as_str(), "https://example.com/dir/cat.png");
    }

    #[test]
    fn root_relative_and_absolute_both_resolve_and_order_is_document_order() {
        let d = doc(
            r#"<html><body>
               <img src="/a.png">
               <img src="https://cdn.example.org/b.png">
               </body></html>"#,
        );
        let found = Engine::discover_images(&d, Some(&base()));
        let urls: Vec<&str> = found.iter().map(|(_, u)| u.as_str()).collect();
        assert!(urls.contains(&"https://example.com/a.png"), "got {urls:?}");
        assert!(urls.contains(&"https://cdn.example.org/b.png"), "got {urls:?}");
    }

    #[test]
    fn missing_and_empty_src_are_skipped_not_guessed() {
        let d = doc(
            r#"<html><body>
               <img>
               <img src="">
               <img src="   ">
               </body></html>"#,
        );
        assert!(Engine::discover_images(&d, Some(&base())).is_empty());
    }

    #[test]
    fn without_a_base_url_only_absolute_src_resolves() {
        let d = doc(
            r#"<html><body>
               <img src="relative.png">
               <img src="https://cdn.example.org/abs.png">
               </body></html>"#,
        );
        let found = Engine::discover_images(&d, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.as_str(), "https://cdn.example.org/abs.png");
    }

    #[test]
    fn only_img_elements_are_collected() {
        // <link href> and <script src> are not images. Collecting any element
        // with a URL attribute would fetch the stylesheet twice and try to
        // decode JavaScript as a bitmap.
        let d = doc(
            r#"<html><head><link rel="stylesheet" href="a.css"></head>
               <body><script src="app.js"></script><img src="real.png"></body></html>"#,
        );
        let found = Engine::discover_images(&d, Some(&base()));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].1.as_str(), "https://example.com/dir/real.png");
    }

    #[test]
    fn stylesheet_and_image_discovery_do_not_collect_each_other() {
        // The two discovery passes run over the same document back to back.
        // If either matched by "has a URL attribute" instead of by tag, this
        // page would cross-contaminate and neither test alone would show it.
        let d = doc(
            r#"<html><head><link rel="stylesheet" href="s.css"></head>
               <body><img src="i.png"></body></html>"#,
        );
        let sheets = Engine::discover_external_stylesheets(&d, Some(&base()));
        let images = Engine::discover_images(&d, Some(&base()));
        assert_eq!(sheets.len(), 1);
        assert_eq!(images.len(), 1);
        assert!(sheets[0].as_str().ends_with("s.css"));
        assert!(images[0].1.as_str().ends_with("i.png"));
    }
}

#[cfg(test)]
mod external_css_lifetime_tests {
    use super::*;

    /// A view must NOT carry one document's external CSS into the next.
    ///
    /// The original #54 implementation early-returned from
    /// `load_external_stylesheets` when a document had no `<link>`, BEFORE
    /// assigning `view.external_css`. So navigating from a page with a
    /// stylesheet to a page without one left the first page's CSS applied to
    /// the second. `load_html` never fetches subresources at all, so it had
    /// the same leak by a shorter route.
    ///
    /// The failure is invisible on a single page load, which is exactly why it
    /// survived review: every #54 test loaded ONE document.
    #[test]
    fn a_new_document_does_not_inherit_the_previous_external_css() {
        let mut engine = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(
                ResourceLoader::new(LoaderConfig::default()).expect("loader"),
            ),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        };
        let id = engine
            .create_headless_view(Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            })
            .expect("headless view");

        // Simulate the state left behind by a previous navigation that DID
        // fetch a stylesheet.
        engine
            .views
            .get_mut(&id)
            .unwrap()
            .external_css
            .push_str("p { width: 123px }");

        // Load a new document into the same view.
        engine
            .load_html(id, "<html><body><p>next page</p></body></html>")
            .expect("load_html");

        assert_eq!(
            engine.views.get(&id).unwrap().external_css,
            "",
            "a new document must start with no external CSS; the previous \
             page's stylesheet leaked into it"
        );
    }
}

#[cfg(test)]
mod child_combinator_tests {
    use super::*;

    fn ctx(tag: &str, classes: &[&str], id: Option<&str>) -> ElementCtx {
        ElementCtx {
            tag: tag.to_string(),
            classes: classes.iter().map(|s| s.to_string()).collect(),
            id: id.map(|s| s.to_string()),
        }
    }

    #[test]
    fn child_matches_an_immediate_child() {
        let anc = [ctx("ul", &["nav"], None)];
        assert!(Engine::selector_matches(".nav > li", "li", &[], None, &anc).is_some());
    }

    #[test]
    fn child_does_not_match_a_deeper_descendant() {
        // THE OVER-MATCH BUG. `.nav > li` must not match a submenu li nested
        // under ul.nav > li > ul. Before the fix, `>` was discarded, so this
        // read as `.nav li` and every submenu item took the top-level rule --
        // the page renders wrong with no error anywhere.
        let anc = [
            ctx("ul", &["nav"], None),
            ctx("li", &[], None),
            ctx("ul", &["submenu"], None),
        ];
        assert!(
            Engine::selector_matches(".nav > li", "li", &[], None, &anc).is_none(),
            "child combinator must not match a descendant three levels down"
        );
    }

    #[test]
    fn unspaced_form_parses_identically() {
        // THE SILENTLY-DEAD BUG. `.nav>li` never split on whitespace, so the
        // compound type part was the literal "nav>li", matched no tag, and the
        // whole rule was dead. Authors write all of these spellings.
        let anc = [ctx("ul", &["nav"], None)];
        for sel in [".nav>li", ".nav > li", ".nav >li", ".nav> li"] {
            assert!(
                Engine::selector_matches(sel, "li", &[], None, &anc).is_some(),
                "{sel} must match an immediate child"
            );
        }
    }

    #[test]
    fn all_spellings_agree_on_the_negative_case_too() {
        // A spelling that parses but matches too much is as wrong as one that
        // matches nothing. Every spelling must REFUSE the nested case.
        let anc = [
            ctx("ul", &["nav"], None),
            ctx("li", &[], None),
            ctx("ul", &[], None),
        ];
        for sel in [".nav>li", ".nav > li", ".nav >li", ".nav> li"] {
            assert!(
                Engine::selector_matches(sel, "li", &[], None, &anc).is_none(),
                "{sel} must refuse a deeper descendant"
            );
        }
    }

    #[test]
    fn mixed_combinators_index_correctly_in_a_longer_chain() {
        // Talos attack point 1: is the "combinator to my right" indexing right
        // in general, or only for the chain lengths tested? Chain of four with
        // the child boundary in the MIDDLE, not at either end.
        let anc = [
            ctx("div", &["page"], None),
            ctx("section", &[], None),
            ctx("div", &["card"], None),
            ctx("div", &["body"], None),
            ctx("span", &[], None),
        ];
        assert!(
            Engine::selector_matches(".page .card > .body .title", "h2", &["title"], None, &anc)
                .is_some(),
            "middle child boundary with descendants on both sides must match"
        );

        // Break ONLY the child boundary: a wrapper between .card and .body.
        // Everything else is unchanged, so a failure here is the child
        // constraint doing its job rather than some other part of the chain.
        let broken = [
            ctx("div", &["page"], None),
            ctx("section", &[], None),
            ctx("div", &["card"], None),
            ctx("div", &["wrapper"], None),
            ctx("div", &["body"], None),
            ctx("span", &[], None),
        ];
        assert!(
            Engine::selector_matches(".page .card > .body .title", "h2", &["title"], None, &broken)
                .is_none(),
            "a wrapper between .card and .body must break the child boundary"
        );
    }

    #[test]
    fn ordinary_descendant_selectors_still_match() {
        // Talos attack point 2: over-refusing is the real regression risk --
        // the malformed-None path must not swallow selectors that should match.
        let anc = [
            ctx("body", &[], None),
            ctx("div", &["wrap"], None),
            ctx("ul", &["nav"], None),
        ];
        for sel in ["li", ".nav li", "body .nav li", "ul li", "div li"] {
            assert!(
                Engine::selector_matches(sel, "li", &[], None, &anc).is_some(),
                "{sel} is an ordinary descendant selector and must still match"
            );
        }
    }

    #[test]
    fn malformed_groups_are_refused_without_panicking() {
        let anc = [ctx("ul", &["nav"], None)];
        for sel in ["> li", ".nav >", ">", ""] {
            assert!(
                Engine::selector_matches(sel, "li", &[], None, &anc).is_none(),
                "{sel:?} is malformed and must not match"
            );
        }
    }

    #[test]
    fn a_malformed_group_does_not_kill_its_valid_siblings() {
        // Comma groups are independent. Refusing one must not refuse the rest,
        // or a single typo silently disables an entire rule.
        let anc = [ctx("ul", &["nav"], None)];
        assert!(
            Engine::selector_matches("> broken, .nav > li", "li", &[], None, &anc).is_some(),
            "a valid group must still match alongside a malformed one"
        );
    }

    #[test]
    fn specificity_is_unchanged_by_the_combinator() {
        // Combinators contribute nothing to specificity. `.nav > li` and
        // `.nav li` must score identically on an element both match.
        let anc = [ctx("ul", &["nav"], None)];
        let child = Engine::selector_matches(".nav > li", "li", &[], None, &anc);
        let desc = Engine::selector_matches(".nav li", "li", &[], None, &anc);
        assert_eq!(child, desc, "a combinator must not change specificity");
    }

    #[test]
    fn child_combinator_applies_through_the_real_layout_build() {
        // Receipt through the actual cascade, not just the matcher: the rule
        // must reach a computed style on the right element and skip the wrong
        // one.
        let e = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        };
        let html = "<html><head><style>.nav > li { width: 123px }</style></head>\
                    <body><ul class=\"nav\"><li>top<ul><li>nested</li></ul></li></ul></body></html>";
        let d = Document::parse_html(html).expect("parse");
        let layout = e.build_layout_from_document(&d);

        fn widths(b: &LayoutBox, out: &mut Vec<String>) {
            out.push(format!("{:?}", b.style.width));
            for c in &b.children {
                widths(c, out);
            }
        }
        let mut got = Vec::new();
        widths(&layout, &mut got);
        let hits = got.iter().filter(|w| w.contains("123")).count();
        assert_eq!(
            hits, 1,
            "exactly the ONE immediate-child li may take the rule; got {got:?}"
        );
    }
}

#[cfg(test)]
mod position_wire_tests {
    use super::*;

    fn engine() -> Engine {
        Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        }
    }

    fn find<'a>(b: &'a LayoutBox, pred: &dyn Fn(&LayoutBox) -> bool) -> Option<&'a LayoutBox> {
        if pred(b) {
            return Some(b);
        }
        for c in &b.children {
            if let Some(f) = find(c, pred) {
                return Some(f);
            }
        }
        None
    }

    #[test]
    fn all_five_position_keywords_compute() {
        let mut s = ComputedStyle::new();
        for (css, expected) in [
            ("static", rustkit_css::Position::Static),
            ("relative", rustkit_css::Position::Relative),
            ("absolute", rustkit_css::Position::Absolute),
            ("fixed", rustkit_css::Position::Fixed),
            ("sticky", rustkit_css::Position::Sticky),
        ] {
            Engine::apply_declaration(&mut s, "position", css);
            assert_eq!(s.position, expected, "position: {css}");
        }
    }

    #[test]
    fn an_unknown_keyword_falls_back_to_static_rather_than_keeping_the_old_value() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "position", "absolute");
        Engine::apply_declaration(&mut s, "position", "notakeyword");
        assert_eq!(
            s.position,
            rustkit_css::Position::Static,
            "an invalid keyword must reset to the CSS initial, not silently \
             leave the element absolutely positioned"
        );
    }

    #[test]
    fn offsets_compute_and_auto_stays_distinct_from_zero() {
        let mut s = ComputedStyle::new();
        assert_eq!(s.top, None, "initial top is auto");
        Engine::apply_declaration(&mut s, "top", "10px");
        Engine::apply_declaration(&mut s, "left", "0px");
        assert_eq!(s.top, Some(rustkit_css::Length::Px(10.0)));
        assert_eq!(
            s.left,
            Some(rustkit_css::Length::Px(0.0)),
            "left:0 must be Some(0), NOT None - `auto` and `0` mean different \
             things and collapsing them loses the distinction"
        );
        assert_eq!(s.right, None, "unset offsets stay auto");
    }

    #[test]
    fn z_index_parses_negatives_and_auto_but_ignores_garbage() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "z-index", "-3");
        assert_eq!(s.z_index, -3, "negative z-index is legal CSS");
        Engine::apply_declaration(&mut s, "z-index", "auto");
        assert_eq!(s.z_index, 0, "auto is stored as 0");
        Engine::apply_declaration(&mut s, "z-index", "5");
        Engine::apply_declaration(&mut s, "z-index", "banana");
        assert_eq!(
            s.z_index, 5,
            "a non-numeric value must be IGNORED, not flattened to 0 - \
             flattening would silently restack the page"
        );
    }

    #[test]
    fn position_reaches_the_layout_box_not_just_the_computed_style() {
        // The wire that was missing: nothing ever assigned layout_box.position,
        // so the layout crate's Absolute/Fixed branches were unreachable.
        let e = engine();
        let html = "<html><body><div id=\"a\" style=\"position: absolute\">x</div></body></html>";
        let d = Document::parse_html(html).expect("parse");
        let layout = e.build_layout_from_document(&d);
        let hit = find(&layout, &|b| b.position == rustkit_layout::Position::Absolute);
        assert!(
            hit.is_some(),
            "no LayoutBox carried Position::Absolute - the layout crate cannot \
             see the declaration"
        );
    }

    #[test]
    fn offsets_reach_the_layout_box_as_pixels() {
        // GEOMETRIC RECEIPT. Computing a value is not the same as the layout
        // engine receiving it; this asserts the resolved px landed on the box.
        let e = engine();
        let html = "<html><body><div style=\"position: absolute; top: 10px; left: 20px\">x</div></body></html>";
        let d = Document::parse_html(html).expect("parse");
        let layout = e.build_layout_from_document(&d);
        let hit = find(&layout, &|b| b.offsets.top.is_some())
            .expect("no LayoutBox received offsets");
        assert_eq!(hit.offsets.top, Some(10.0));
        assert_eq!(hit.offsets.left, Some(20.0));
        assert_eq!(
            hit.offsets.right, None,
            "an unset offset must stay None (auto), not become 0"
        );
    }

    #[test]
    fn em_offsets_resolve_against_font_size() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "top", "2em");
        assert_eq!(s.top, Some(rustkit_css::Length::Em(2.0)));
    }

    #[test]
    fn z_index_reaches_the_layout_box() {
        let e = engine();
        let html = "<html><body><div style=\"position: absolute; z-index: 7\">x</div></body></html>";
        let d = Document::parse_html(html).expect("parse");
        let layout = e.build_layout_from_document(&d);
        assert!(
            find(&layout, &|b| b.z_index == 7).is_some(),
            "z-index did not reach the layout box"
        );
    }

    #[test]
    fn a_percentage_offset_is_refused_rather_than_approximated() {
        // Percentages resolve against the CONTAINING BLOCK, which is unknown
        // while the tree is being built. Yielding None (auto) is honest;
        // guessing a pixel value would be a silently wrong position.
        let e = engine();
        let html = "<html><body><div style=\"position: absolute; top: 50%\">x</div></body></html>";
        let d = Document::parse_html(html).expect("parse");
        let layout = e.build_layout_from_document(&d);
        let positioned = find(&layout, &|b| b.position == rustkit_layout::Position::Absolute)
            .expect("element should still be absolutely positioned");
        assert_eq!(
            positioned.offsets.top, None,
            "a percentage offset must resolve to None, not an invented pixel value"
        );
    }
}

#[cfg(test)]
mod display_list_reftests {
    //! TIER 1 RENDERING TESTS: display-list reference tests.
    //!
    //! A reference test asserts that two DIFFERENT source documents produce the
    //! SAME rendering (`==`), or deliberately different renderings (`!=`).
    //!
    //! This compares DISPLAY LISTS, not pixels. That is a deliberate choice:
    //!
    //!  - It needs no GPU adapter and no window, so it runs on a hosted CI
    //!    runner exactly as it runs locally. Pixel comparison on a hosted
    //!    runner measures the runner, which is why this tree publishes
    //!    `parity: no data` rather than a number it cannot stand behind.
    //!  - It catches the entire class of defect found by the 2026-07-31
    //!    unreachable-capability audit. `position: absolute` doing nothing is
    //!    invisible to a computed-value test and obvious in a display list,
    //!    because the list is literally "what would be painted, where".
    //!
    //! What it does NOT catch: anything that goes wrong AFTER the display list
    //! - rasterisation, texture upload, blending, font rendering. Those need
    //! Tier 2 (pixel reftests on real hardware). Stated so this is not mistaken
    //! for proof that a page looks right.
    //!
    //! The previous harness in rustkit-test compared NORMALIZED HTML TEXT,
    //! which is inverted from what a reftest is for: a genuine pair (different
    //! markup, identical rendering) FAILS that comparison, and a trivially
    //! identical pair passes. The checked-in `color-red` pair is exactly such a
    //! genuine pair and would have been reported as a failure.

    use super::*;
    use rustkit_layout::{Dimensions, DisplayList, Rect};
    use std::path::{Path, PathBuf};

    const VIEWPORT_W: f32 = 800.0;
    const VIEWPORT_H: f32 = 600.0;

    fn reftest_dir() -> PathBuf {
        // CARGO_MANIFEST_DIR is crates/rustkit-engine.
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("wpt")
            .join("reftest")
    }

    fn engine() -> Engine {
        Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        }
    }

    /// Render one document to a display-list description.
    ///
    /// The containing block is fixed so the result is deterministic and
    /// independent of any real window.
    fn render_to_display_list(e: &Engine, html: &str) -> String {
        let document = Document::parse_html(html).expect("parse");
        let mut root = e.build_layout_from_document(&document);

        let mut containing = Dimensions::default();
        containing.content = Rect::new(0.0, 0.0, VIEWPORT_W, VIEWPORT_H);
        root.layout(&containing);

        let list = DisplayList::build(&root);
        // DisplayCommand derives Debug but not PartialEq, so the Debug
        // rendering IS the comparison surface. It is sensitive to float
        // formatting, which for a reftest is correct: two documents that should
        // render identically should produce byte-identical commands.
        list.commands
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    struct RefCase {
        should_match: bool,
        test: String,
        reference: String,
    }

    fn parse_reftest_list(text: &str) -> Vec<RefCase> {
        let mut cases = Vec::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() != 3 {
                continue;
            }
            let should_match = match parts[0] {
                "==" => true,
                "!=" => false,
                _ => continue,
            };
            cases.push(RefCase {
                should_match,
                test: parts[1].to_string(),
                reference: parts[2].to_string(),
            });
        }
        cases
    }

    /// SELF-CHECK. The comparison must be able to report a MISMATCH.
    ///
    /// A reftest harness whose comparison always returns "equal" passes every
    /// `==` case and looks perfect. That is the decorative-green shape this
    /// fleet has spent a week removing, and it is why Atlas's macOS WPT runner
    /// runs a negative control FIRST and aborts if it does not fail.
    #[test]
    fn the_comparison_can_actually_report_a_difference() {
        let e = engine();
        let red = render_to_display_list(
            &e,
            "<html><body><div style=\"background-color: #ff0000; width: 100px; height: 50px\"></div></body></html>",
        );
        let blue = render_to_display_list(
            &e,
            "<html><body><div style=\"background-color: #0000ff; width: 100px; height: 50px\"></div></body></html>",
        );
        assert_ne!(
            red, blue,
            "NEGATIVE CONTROL FAILED: two documents with different background \
             colours produced identical display lists. The comparison is inert, \
             so every == case below would pass vacuously. Do not trust any \
             reftest result from this run."
        );
    }

    /// A pair that SHOULD match must match — the positive control.
    #[test]
    fn equivalent_documents_produce_equal_display_lists() {
        let e = engine();
        let a = render_to_display_list(
            &e,
            "<html><body><div style=\"background-color: #ff0000; width: 100px\"></div></body></html>",
        );
        let b = render_to_display_list(
            &e,
            "<html><body><div style=\"background-color: rgb(255, 0, 0); width: 100px\"></div></body></html>",
        );
        assert_eq!(
            a, b,
            "#ff0000 and rgb(255,0,0) are the same colour and must produce the \
             same display list"
        );
    }

    /// The checked-in suite.
    #[test]
    fn checked_in_reftests_all_hold() {
        let dir = reftest_dir();
        let list_path = dir.join("reftest.list");
        let text = std::fs::read_to_string(&list_path).unwrap_or_else(|e| {
            panic!("cannot read {}: {e}", list_path.display());
        });
        let cases = parse_reftest_list(&text);

        // AN EMPTY SUITE IS A FAILURE, NOT A PASS. The previous runner returned
        // an empty summary when the directory did not exist, which reports
        // success for having tested nothing - a wrong path would have looked
        // identical to a green suite.
        assert!(
            !cases.is_empty(),
            "no reftest cases parsed from {} - a harness that finds zero tests \
             must fail rather than report success",
            list_path.display()
        );

        let e = engine();
        let mut failures = Vec::new();
        for case in &cases {
            let test_html = match std::fs::read_to_string(dir.join(&case.test)) {
                Ok(h) => h,
                Err(err) => {
                    failures.push(format!("{}: cannot read test file: {err}", case.test));
                    continue;
                }
            };
            let ref_html = match std::fs::read_to_string(dir.join(&case.reference)) {
                Ok(h) => h,
                Err(err) => {
                    failures.push(format!("{}: cannot read reference: {err}", case.reference));
                    continue;
                }
            };
            let got = render_to_display_list(&e, &test_html);
            let want = render_to_display_list(&e, &ref_html);

            // A pair that paints NOTHING compares equal to nothing and passes
            // every `==` case vacuously. An empty display list means the
            // document did not render, which is a failure however the case was
            // declared.
            if got.is_empty() || want.is_empty() {
                failures.push(format!(
                    "{} / {} : empty display list (test={} cmds, ref={} cmds) -                      a document that paints nothing cannot verify anything",
                    case.test,
                    case.reference,
                    got.lines().count(),
                    want.lines().count(),
                ));
                continue;
            }

            let equal = got == want;
            if equal != case.should_match {
                let op = if case.should_match { "==" } else { "!=" };
                failures.push(format!(
                    "{op} {} {} : expected {}, got {}",
                    case.test,
                    case.reference,
                    if case.should_match { "match" } else { "mismatch" },
                    if equal { "match" } else { "mismatch" },
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} reftest case(s) failed:\n{}",
            failures.len(),
            cases.len(),
            failures.join("\n")
        );
    }

    /// The audit class, made visible.
    ///
    /// Before the position wire, these two documents produced IDENTICAL display
    /// lists, because `position: absolute` could not be set and the offsets
    /// never reached the layout box. No computed-value test could see that; a
    /// display list shows it immediately.
    #[test]
    fn positioned_and_static_documents_differ_in_the_display_list() {
        let e = engine();
        let statik = render_to_display_list(
            &e,
            "<html><body><div style=\"background-color: #00ff00; width: 50px; height: 50px\"></div></body></html>",
        );
        let positioned = render_to_display_list(
            &e,
            "<html><body><div style=\"background-color: #00ff00; width: 50px; height: 50px; \
             position: absolute; top: 30px; left: 40px\"></div></body></html>",
        );
        assert_ne!(
            statik, positioned,
            "an absolutely positioned box must not paint identically to a \
             static one - if these match, `position` is not reaching layout"
        );
    }
}

#[cfg(test)]
mod overflow_whitespace_decoration_tests {
    //! Two groups, per the #62 receipt shape:
    //!   - COMPUTED-VALUE tests prove the arms parse.
    //!   - REACHING tests prove the value changes what would be painted.
    //! A wire unit needs both, or the receipt cannot distinguish "the property
    //! parses" from "the property does anything".
    use super::*;
    use rustkit_layout::{Dimensions, DisplayList, Rect};

    fn eng() -> Engine {
        Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        }
    }

    fn display_list_of(e: &Engine, html: &str) -> String {
        let d = Document::parse_html(html).expect("parse");
        let mut root = e.build_layout_from_document(&d);
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 800.0, 600.0);
        root.layout(&cb);
        DisplayList::build(&root)
            .commands
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---------- GROUP 1: computed values ----------

    #[test]
    fn overflow_keywords_compute() {
        let mut s = ComputedStyle::new();
        for (css, want) in [
            ("hidden", rustkit_css::Overflow::Hidden),
            ("scroll", rustkit_css::Overflow::Scroll),
            ("auto", rustkit_css::Overflow::Auto),
            ("clip", rustkit_css::Overflow::Clip),
            ("visible", rustkit_css::Overflow::Visible),
        ] {
            Engine::apply_declaration(&mut s, "overflow-x", css);
            assert_eq!(s.overflow_x, want, "overflow-x: {css}");
        }
    }

    #[test]
    fn the_overflow_shorthand_sets_BOTH_axes() {
        // A shorthand that set only one axis would leave the other at its
        // initial and be invisible in any single-axis assertion.
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "overflow", "hidden");
        assert_eq!(s.overflow_x, rustkit_css::Overflow::Hidden);
        assert_eq!(s.overflow_y, rustkit_css::Overflow::Hidden);
    }

    #[test]
    fn the_axis_longhands_are_independent() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "overflow-x", "scroll");
        Engine::apply_declaration(&mut s, "overflow-y", "hidden");
        assert_eq!(s.overflow_x, rustkit_css::Overflow::Scroll);
        assert_eq!(
            s.overflow_y,
            rustkit_css::Overflow::Hidden,
            "setting one axis must not clobber the other"
        );
    }

    #[test]
    fn white_space_keywords_compute() {
        let mut s = ComputedStyle::new();
        for (css, want) in [
            ("pre", rustkit_css::WhiteSpace::Pre),
            ("nowrap", rustkit_css::WhiteSpace::Nowrap),
            ("pre-wrap", rustkit_css::WhiteSpace::PreWrap),
            ("pre-line", rustkit_css::WhiteSpace::PreLine),
            ("break-spaces", rustkit_css::WhiteSpace::BreakSpaces),
            ("normal", rustkit_css::WhiteSpace::Normal),
        ] {
            Engine::apply_declaration(&mut s, "white-space", css);
            assert_eq!(s.white_space, want, "white-space: {css}");
        }
    }

    #[test]
    fn text_decoration_combines_multiple_lines() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "text-decoration", "underline line-through");
        assert!(s.text_decoration_line.underline);
        assert!(s.text_decoration_line.line_through);
        assert!(!s.text_decoration_line.overline);
    }

    #[test]
    fn a_shorthand_carrying_a_colour_still_sets_the_line() {
        // `text-decoration: underline red` is legal. Matching the WHOLE value
        // against fixed keywords would drop it entirely - the rule would be
        // silently dead, which is the defect class this whole campaign is
        // about.
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "text-decoration", "underline red");
        assert!(
            s.text_decoration_line.underline,
            "a shorthand naming a colour as well as a line must still set the line"
        );
    }

    #[test]
    fn a_value_naming_no_line_keyword_leaves_the_line_alone() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "text-decoration", "underline");
        Engine::apply_declaration(&mut s, "text-decoration", "red");
        assert!(
            s.text_decoration_line.underline,
            "a colour-only value must not clear an already-set line"
        );
    }

    #[test]
    fn none_clears_the_line() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "text-decoration", "underline");
        Engine::apply_declaration(&mut s, "text-decoration", "none");
        assert!(!s.text_decoration_line.underline);
    }

    #[test]
    fn decoration_style_computes() {
        // thickness deliberately NOT asserted: the reference has no
        // text-decoration-thickness arm, so wiring one here would be an
        // undeclared divergence. See the commit message.
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "text-decoration-style", "wavy");
        assert_eq!(s.text_decoration_style, rustkit_css::TextDecorationStyle::Wavy);
    }

    // ---------- GROUP 2: does it reach what gets painted? ----------

    #[test]
    fn text_decoration_changes_the_display_list() {
        // The reaching test. Before this wire the two documents produced
        // identical display lists, because nothing could set
        // text_decoration_line and the TextDecoration commands were
        // unreachable.
        let e = eng();
        let plain = display_list_of(
            &e,
            "<html><body><p>hello world</p></body></html>",
        );
        let underlined = display_list_of(
            &e,
            "<html><body><p style=\"text-decoration: underline\">hello world</p></body></html>",
        );
        assert_ne!(
            plain, underlined,
            "an underlined paragraph must not paint identically to a plain one \
             - if these match, text-decoration is not reaching the display list"
        );
    }

    #[test]
    fn white_space_nowrap_changes_the_display_list() {
        // nowrap suppresses line breaking, so a string long enough to wrap in a
        // narrow box must paint differently. If this ever stops differing, the
        // white_space consumer has become unreachable again.
        let e = eng();
        let wrapped = display_list_of(
            &e,
            "<html><body><div style=\"width: 60px\">aaa bbb ccc ddd eee fff</div></body></html>",
        );
        let nowrap = display_list_of(
            &e,
            "<html><body><div style=\"width: 60px; white-space: nowrap\">aaa bbb ccc ddd eee fff</div></body></html>",
        );
        assert_ne!(
            wrapped, nowrap,
            "white-space: nowrap must change how the text is laid out"
        );
    }
}

#[cfg(test)]
mod flex_item_property_tests {
    //! Two groups per the #62 shape: computed-value assertions, then a
    //! GEOMETRIC reaching assertion. The reaching test is the one that matters
    //! - on #64 the computed group passed while the property still painted
    //! nothing, and only the reaching group caught it.
    use super::*;
    use rustkit_layout::{Dimensions, Rect};

    fn eng() -> Engine {
        Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        }
    }

    // ---------- GROUP 1: computed values ----------

    #[test]
    fn align_content_and_align_self_keywords_compute() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "align-content", "space-between");
        Engine::apply_declaration(&mut s, "align-self", "center");
        assert_eq!(s.align_content, rustkit_css::AlignContent::SpaceBetween);
        assert_eq!(s.align_self, rustkit_css::AlignSelf::Center);
    }

    #[test]
    fn order_accepts_negatives_and_ignores_garbage() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "order", "-2");
        assert_eq!(s.order, -2, "negative order is legal CSS");
        Engine::apply_declaration(&mut s, "order", "banana");
        assert_eq!(
            s.order, -2,
            "a non-numeric value must be ignored, not flattened to 0 - \
             flattening would silently reorder the line"
        );
    }

    #[test]
    fn flex_longhands_compute() {
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "flex-grow", "3");
        Engine::apply_declaration(&mut s, "flex-shrink", "0");
        Engine::apply_declaration(&mut s, "flex-basis", "120px");
        assert_eq!(s.flex_grow, 3.0);
        assert_eq!(s.flex_shrink, 0.0);
        assert_eq!(s.flex_basis, rustkit_css::FlexBasis::Length(120.0));
    }

    #[test]
    fn the_single_number_shorthand_zeroes_the_basis() {
        // `flex: 1` must set basis to 0, not auto. With basis auto the item
        // sizes to content and the container is NOT divided - which is the
        // whole reason authors write `flex: 1`.
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "flex", "1");
        assert_eq!(s.flex_grow, 1.0);
        assert_eq!(s.flex_shrink, 1.0);
        assert_eq!(
            s.flex_basis,
            rustkit_css::FlexBasis::Length(0.0),
            "flex: 1 must zero the basis or the container is not divided"
        );
    }

    #[test]
    fn a_two_value_shorthand_distinguishes_shrink_from_basis() {
        // `flex: 1 200px` means grow 1, BASIS 200px - not shrink 200.
        // Reading position 2 as shrink unconditionally is silently wrong
        // rather than merely unsupported.
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "flex", "1 200px");
        assert_eq!(s.flex_grow, 1.0);
        assert_eq!(
            s.flex_basis,
            rustkit_css::FlexBasis::Length(200.0),
            "a length in position 2 is the BASIS"
        );

        let mut s2 = ComputedStyle::new();
        Engine::apply_declaration(&mut s2, "flex", "2 3");
        assert_eq!(s2.flex_grow, 2.0);
        assert_eq!(s2.flex_shrink, 3.0, "a bare number in position 2 is the SHRINK");
    }

    #[test]
    fn em_basis_is_refused_rather_than_stored_as_pixels() {
        // FlexBasis::Length holds a bare f32 with no unit. Storing 2em as 2px
        // would be a wrong number that looks like a measurement.
        let mut s = ComputedStyle::new();
        Engine::apply_declaration(&mut s, "flex-basis", "2em");
        assert_eq!(
            s.flex_basis,
            rustkit_css::FlexBasis::Auto,
            "an em basis must not be silently stored as pixels"
        );
    }

    // ---------- GROUP 2: does it reach the geometry? ----------

    #[test]
    fn flex_grow_actually_divides_the_container() {
        // GEOMETRIC RECEIPT, the shape from Talos's Linux #30: two items with
        // flex:1 and flex:3 must split the container 1:3. A computed-value
        // assertion cannot tell whether the number reached layout at all.
        let e = eng();
        let html = "<html><body>\
            <div style=\"display: flex; width: 400px\">\
            <div style=\"flex: 1\">a</div>\
            <div style=\"flex: 3\">b</div>\
            </div></body></html>";
        let d = Document::parse_html(html).expect("parse");
        let mut root = e.build_layout_from_document(&d);
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 800.0, 600.0);
        root.layout(&cb);

        fn widths(b: &LayoutBox, out: &mut Vec<f32>) {
            if b.style.flex_grow > 0.0 {
                out.push(b.dimensions.content.width);
            }
            for c in &b.children {
                widths(c, out);
            }
        }
        let mut got = Vec::new();
        widths(&root, &mut got);
        assert_eq!(
            got.len(),
            2,
            "expected two flex items to carry flex_grow; got {got:?}"
        );
        let (a, b) = (got[0], got[1]);
        assert!(
            a > 0.0 && b > 0.0,
            "both flex items must have non-zero width; got {a} and {b}"
        );
        let ratio = b / a;
        assert!(
            (ratio - 3.0).abs() < 0.2,
            "flex:1 and flex:3 must split the container 1:3 - got {a} and {b} \
             (ratio {ratio:.2}). If the ratio is 1.0 the grow factor never \
             reached layout."
        );
    }
}

#[cfg(test)]
mod ua_default_gap_tests {
    //! Gaps found by a PER-PROPERTY reference comparison, not by reading the
    //! code. Both are defects rather than divergences: the reference has them
    //! and this tree did not.
    use super::*;

    fn ua(tag: &str) -> ComputedStyle {
        let e = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        };
        let attrs = std::collections::HashMap::new();
        let sheet = Stylesheet::new();
        let parent = ComputedStyle::new();
        e.compute_style_for_element(tag, &attrs, &sheet, &parent, &[])
    }

    #[test]
    fn h4_h5_h6_have_ua_defaults() {
        // Before this, an <h4> rendered at body size, unbolded, with no
        // margins - visually indistinguishable from a <div>.
        for (tag, size) in [("h4", 16.0_f32), ("h5", 13.28), ("h6", 10.72)] {
            let s = ua(tag);
            assert_eq!(
                s.font_weight,
                rustkit_css::FontWeight::BOLD,
                "<{tag}> must be bold"
            );
            assert_eq!(s.font_size, rustkit_css::Length::Px(size), "<{tag}> font-size");
            assert_ne!(
                s.margin_top,
                rustkit_css::Length::Zero,
                "<{tag}> must have a top margin"
            );
        }
    }

    #[test]
    fn the_heading_scale_is_monotonically_decreasing() {
        // A per-tag assertion cannot catch two headings given the same size, or
        // h5 larger than h4. The ORDER is the property that makes a heading
        // scale a scale.
        let sizes: Vec<f32> = ["h1", "h2", "h3", "h4", "h5", "h6"]
            .iter()
            .map(|t| match ua(t).font_size {
                rustkit_css::Length::Px(px) => px,
                other => panic!("<{t}> font-size is {other:?}, expected Px"),
            })
            .collect();
        for w in sizes.windows(2) {
            assert!(
                w[0] > w[1],
                "heading sizes must strictly decrease; got {sizes:?}"
            );
        }
    }

    #[test]
    fn th_is_bold_but_td_is_not() {
        // td and th shared one arm, so every header cell rendered at normal
        // weight. That distinction is the entire visual point of <th>.
        let th = ua("th");
        let td = ua("td");
        assert_eq!(th.font_weight, rustkit_css::FontWeight::BOLD, "<th> is bold");
        assert_ne!(td.font_weight, rustkit_css::FontWeight::BOLD, "<td> is NOT bold");
        // Splitting the shared arm must not have dropped the table layout
        // behaviour from either cell type.
        assert_eq!(th.flex_grow, 1.0, "<th> keeps flex-grow");
        assert_eq!(td.flex_grow, 1.0, "<td> keeps flex-grow");
        assert_eq!(th.flex_basis, rustkit_css::FlexBasis::Length(0.0));
        assert_eq!(td.flex_basis, rustkit_css::FlexBasis::Length(0.0));
    }
}

#[cfg(test)]
mod box_shadow_paint_tests {
    //! box-shadow has PARSED since #49 and never drawn a pixel. Group A (the
    //! computed-value assertions from #49) passed the whole time, so a
    //! computed-value suite called box-shadow "supported" while a shadowed page
    //! rendered identically to an unshadowed one.
    use super::*;
    use rustkit_layout::{Dimensions, DisplayList, Rect};

    fn dl(html: &str) -> String {
        let e = Engine {
            config: EngineConfig::default(),
            views: HashMap::new(),
            viewhost: ViewHost::new(),
            compositor: test_compositor(),
            renderer: None,
            loader: Arc::new(ResourceLoader::new(LoaderConfig::default()).expect("loader")),
            image_manager: Arc::new(ImageManager::new()),
            event_tx: tokio::sync::mpsc::unbounded_channel().0,
            event_rx: None,
        };
        let d = Document::parse_html(html).expect("parse");
        let mut root = e.build_layout_from_document(&d);
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 800.0, 600.0);
        root.layout(&cb);
        DisplayList::build(&root)
            .commands
            .iter()
            .map(|c| format!("{c:?}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_shadowed_box_paints_differently_from_an_unshadowed_one() {
        let plain = dl("<html><body><div style=\"width:100px;height:50px;background-color:#fff\"></div></body></html>");
        let shadowed = dl("<html><body><div style=\"width:100px;height:50px;background-color:#fff;\
                           box-shadow: 4px 4px 0 #000\"></div></body></html>");
        assert_ne!(
            plain, shadowed,
            "a box-shadow must change what is painted; if these match the \
             shadow is parsed and never drawn"
        );
        assert!(
            shadowed.contains("BoxShadow"),
            "expected a BoxShadow command in the display list, got:\n{shadowed}"
        );
    }

    #[test]
    fn the_shadow_is_emitted_BEFORE_the_background() {
        // Paint order is the whole point of an outer shadow: emitted after the
        // background it would cover the box it is meant to sit behind.
        let s = dl("<html><body><div style=\"width:100px;height:50px;background-color:#ff0000;\
                    box-shadow: 4px 4px 0 #000\"></div></body></html>");
        let shadow_at = s.find("BoxShadow").expect("no BoxShadow command");
        let bg_at = s
            .find("SolidColor(Color { r: 255, g: 0, b: 0")
            .expect("no red background command");
        assert!(
            shadow_at < bg_at,
            "the outer shadow must be emitted before the background it sits behind"
        );
    }

    #[test]
    fn an_inset_shadow_is_emitted_AFTER_the_background() {
        let s = dl("<html><body><div style=\"width:100px;height:50px;background-color:#ff0000;\
                    box-shadow: inset 4px 4px 0 #000\"></div></body></html>");
        let shadow_at = s.find("BoxShadow").expect("no BoxShadow command");
        let bg_at = s
            .find("SolidColor(Color { r: 255, g: 0, b: 0")
            .expect("no red background command");
        assert!(
            shadow_at > bg_at,
            "an inset shadow must be emitted after the background so it paints over it"
        );
    }

    #[test]
    fn a_fully_transparent_shadow_emits_nothing() {
        // is_visible() gates emission. A transparent shadow that still emitted
        // a command would cost a draw call for nothing.
        let s = dl("<html><body><div style=\"width:100px;height:50px;\
                    box-shadow: 4px 4px 0 rgba(0,0,0,0)\"></div></body></html>");
        assert!(
            !s.contains("BoxShadow"),
            "a fully transparent shadow must not be emitted, got:\n{s}"
        );
    }
}
