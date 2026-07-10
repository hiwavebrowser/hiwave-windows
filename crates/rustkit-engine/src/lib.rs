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
        let mut root_box = self.build_layout_from_document(&document);

        // Count children for debugging
        let child_count = root_box.children.len();
        info!(?id, child_count, "Layout: built tree from DOM");

        // Layout
        root_box.layout(&containing_block);

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
        // Create root layout box for the document
        let mut root_style = ComputedStyle::new();
        root_style.background_color = rustkit_css::Color::WHITE;
        let mut root_box = LayoutBox::new(BoxType::Block, root_style);

        // Collect and parse author stylesheets from <style> elements so the
        // cascade can apply them (previously only UA defaults + inline applied,
        // so every `<style>` rule — backgrounds, class selectors, sizes — was
        // silently ignored).
        let mut css_text = String::new();
        self.collect_style_text(&document.root(), &mut css_text);
        let sheet = Stylesheet::parse(&css_text).unwrap_or_else(|_| Stylesheet::new());
        info!(rule_count = sheet.rules.len(), css_len = css_text.len(), "CSS: author stylesheet parsed");
        let root_inherited = ComputedStyle::new();

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
            
            let body_box = self.build_layout_from_node(&body, &sheet, &root_inherited, &[]);
            info!(
                layout_children = body_box.children.len(),
                "Layout: body box built"
            );
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
                    let style = ComputedStyle::inherit_from(parent);
                    LayoutBox::new(BoxType::Text(trimmed.to_string()), style)
                }
            }
            _ => {
                // For other node types (Document, Comment, etc.), return empty box
                LayoutBox::new(BoxType::Block, ComputedStyle::inherit_from(parent))
            }
        }
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
            "td" | "th" => {
                style.flex_grow = 1.0;
                style.flex_basis = rustkit_css::FlexBasis::Length(0.0);
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
        for (_, i) in matched {
            for decl in &sheet.rules[i].declarations {
                if let PropertyValue::Specified(v) = &decl.value {
                    self.apply_declaration(&mut style, &decl.property, v);
                }
            }
        }

        // 4. Inline style attribute (highest priority).
        if let Some(style_attr) = attributes.get("style") {
            self.apply_inline_style(&mut style, style_attr);
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
        let mut best: Option<u32> = None;
        for group in selector.split(',') {
            let group = group.trim();
            if group.is_empty() {
                continue;
            }
            // Split into compound selectors, dropping `>` combinator tokens.
            let compounds: Vec<&str> = group
                .split_whitespace()
                .filter(|t| *t != ">")
                .collect();
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
            for sel in ancestor_sels.iter().rev() {
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
    fn apply_inline_style(&self, style: &mut ComputedStyle, style_attr: &str) {
        for declaration in style_attr.split(';') {
            let declaration = declaration.trim();
            if declaration.is_empty() {
                continue;
            }
            if let Some((property, value)) = declaration.split_once(':') {
                self.apply_declaration(style, property.trim(), value.trim());
            }
        }
    }

    /// Apply a single `property: value` declaration to a computed style.
    /// Shared by the inline-style and author-stylesheet paths.
    fn apply_declaration(&self, style: &mut ComputedStyle, property: &str, value: &str) {
        let property = property.to_lowercase();
        let value = value.trim();
        match property.as_str() {
            "color" => {
                if let Some(c) = parse_color(value) {
                    style.color = c;
                }
            }
            "background-color" | "background" => {
                // `background` shorthand: use the first token that parses as a color.
                let color = if property == "background" {
                    value.split_whitespace().find_map(parse_color)
                } else {
                    parse_color(value)
                };
                if let Some(c) = color {
                    style.background_color = c;
                }
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
                if let Ok(n) = value.parse::<f32>() {
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
            "margin" => self.apply_box_shorthand(value, |s, l| {
                s.margin_top = l;
                s.margin_right = l;
                s.margin_bottom = l;
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
            "padding" => self.apply_box_shorthand(value, |s, l| {
                s.padding_top = l;
                s.padding_right = l;
                s.padding_bottom = l;
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
                style.border_top_width = w;
                style.border_right_width = w;
                style.border_bottom_width = w;
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
                    style.border_top_width = l;
                    style.border_right_width = l;
                    style.border_bottom_width = l;
                    style.border_left_width = l;
                }
            }
            "display" => {
                if let Some(d) = rustkit_css::parse_display(value) {
                    style.display = d;
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
                    style.row_gap = l;
                    style.column_gap = l;
                }
            }
            _ => {}
        }
    }

    /// Apply a 1-value box shorthand (margin/padding). Only the common
    /// single-value form is handled; multi-value forms fall back to the
    /// first value on all sides.
    fn apply_box_shorthand(
        &self,
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

/// Parse a length value from CSS.
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
            compositor: Compositor::new().expect("Failed to create compositor"),
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
            compositor: Compositor::new().expect("Failed to create compositor"),
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
            compositor: Compositor::new().expect("compositor"),
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
