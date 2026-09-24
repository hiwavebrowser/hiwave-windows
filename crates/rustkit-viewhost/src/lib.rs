//! # RustKit ViewHost
//!
//! Platform window hosting layer for the RustKit browser engine.
//! Handles child window/view creation, resize events, DPI changes, focus, visibility,
//! and input event translation (mouse, keyboard).
//!
//! ## Design Goals
//!
//! 1. **Multi-view support**: Each view has isolated state, no global singletons
//! 2. **Resize correctness**: Platform resize events trigger surface resize immediately
//! 3. **DPI awareness**: Per-monitor DPI scaling
//! 4. **Focus management**: Proper focus chain for keyboard events
//! 5. **Input handling**: Platform messages translated to platform-agnostic events
//! 6. **Platform abstraction**: Trait-based design for cross-platform support

// Allow Arc with non-Send/Sync types - intentional for Win32 HWND handling
#![allow(clippy::arc_with_non_send_sync)]

mod traits;

#[cfg(target_os = "macos")]
mod macos;

pub use traits::{ViewHostTrait, WindowHandle};

#[cfg(target_os = "macos")]
pub use macos::MacOSViewHost;
#[cfg(target_os = "macos")]
pub use macos::{drain_pending_clicks, drain_pending_keys, PendingClick, PendingKey};

// Screenshot capture (Windows: GPU readback of a hosted view)
#[cfg(windows)]
pub mod screenshot;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use thiserror::Error;
use tracing::{debug, info, trace, warn};
// `error!` is only used inside the Win32 window-creation paths; keep the
// import platform-gated so macOS builds stay warning-free.
#[cfg(windows)]
use tracing::error;

#[cfg(windows)]
use rustkit_core::{
    FocusEvent, FocusEventType, InputEvent, KeyCode, KeyEvent, KeyEventType, KeyboardState,
    Modifiers, MouseButton, MouseEvent, MouseEventType, MouseState, Point,
};

#[cfg(target_os = "macos")]
use cocoa::{
    base::{id, nil},
};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};

#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::Gdi::{
            BeginPaint, EndPaint, InvalidateRect, ScreenToClient, UpdateWindow, HBRUSH,
            PAINTSTRUCT,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{
                GetDpiForWindow, SetProcessDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            },
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, SetFocus, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
                VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
            },
            WindowsAndMessaging::*,
        },
    },
};

/// Win32 message constants.
#[cfg(windows)]
const WM_MOUSELEAVE_MSG: u32 = 0x02A3;

/// Unique identifier for a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ViewId(u64);

impl ViewId {
    /// Create a new unique ViewId.
    pub fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        Self(COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

impl Default for ViewId {
    fn default() -> Self {
        Self::new()
    }
}

/// Rectangle representing view bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Bounds {
    pub fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn zero() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
        }
    }
}

/// Errors that can occur in the ViewHost.
#[derive(Error, Debug)]
pub enum ViewHostError {
    #[error("Failed to create window: {0}")]
    WindowCreation(String),

    #[error("View not found: {0:?}")]
    ViewNotFound(ViewId),

    #[error("Invalid parent window")]
    InvalidParent,

    #[error("Windows API error: {0}")]
    WindowsApi(String),

    #[error("Lock poisoned - thread panicked while holding lock")]
    LockPoisoned,
}

/// Events emitted by the ViewHost.
#[derive(Debug, Clone)]
pub enum ViewEvent {
    /// View bounds changed (includes DPI-aware dimensions).
    Resized {
        view_id: ViewId,
        bounds: Bounds,
        dpi: u32,
    },
    /// View received focus.
    Focused { view_id: ViewId },
    /// View lost focus.
    Blurred { view_id: ViewId },
    /// View visibility changed.
    VisibilityChanged { view_id: ViewId, visible: bool },
    /// DPI changed for the view.
    DpiChanged { view_id: ViewId, dpi: u32 },
    /// View is being destroyed.
    Destroyed { view_id: ViewId },
    /// Input event from the view (Windows only).
    #[cfg(windows)]
    Input { view_id: ViewId, event: InputEvent },
}

/// Callback for view events.
pub type EventCallback = Arc<dyn Fn(ViewEvent) + Send + Sync>;

/// Per-view state. Stores HWND as isize for thread safety.
#[allow(dead_code)]
struct ViewState {
    id: ViewId,
    /// HWND stored as isize for Send + Sync safety.
    hwnd_raw: isize,
    bounds: Bounds,
    dpi: u32,
    visible: bool,
    focused: bool,
    #[cfg(windows)]
    keyboard_state: KeyboardState,
    #[cfg(windows)]
    mouse_state: MouseState,
    #[cfg(windows)]
    last_click_time: u64,
    #[cfg(windows)]
    last_click_pos: Point,
    #[cfg(windows)]
    click_count: u32,
    #[cfg(windows)]
    tracking_mouse: bool,
}

/// Global view registry for window procedure lookups.
#[cfg(windows)]
static VIEW_REGISTRY: std::sync::LazyLock<RwLock<ViewRegistry>> =
    std::sync::LazyLock::new(|| RwLock::new(ViewRegistry::new()));

#[cfg(windows)]
struct ViewRegistry {
    hwnd_to_state: HashMap<isize, Arc<Mutex<ViewState>>>,
    event_callback: Option<EventCallback>,
}

#[cfg(windows)]
impl ViewRegistry {
    fn new() -> Self {
        Self {
            hwnd_to_state: HashMap::new(),
            event_callback: None,
        }
    }

    fn register(&mut self, hwnd_raw: isize, state: Arc<Mutex<ViewState>>) {
        self.hwnd_to_state.insert(hwnd_raw, state);
    }

    fn unregister(&mut self, hwnd_raw: isize) {
        self.hwnd_to_state.remove(&hwnd_raw);
    }

    fn get(&self, hwnd_raw: isize) -> Option<Arc<Mutex<ViewState>>> {
        self.hwnd_to_state.get(&hwnd_raw).cloned()
    }

    fn set_callback(&mut self, callback: EventCallback) {
        self.event_callback = Some(callback);
    }

    fn emit(&self, event: ViewEvent) {
        if let Some(ref cb) = self.event_callback {
            cb(event);
        }
    }
}

/// Configuration for creating a main window.
#[derive(Debug, Clone)]
pub struct MainWindowConfig {
    /// Window title.
    pub title: String,
    /// Initial width.
    pub width: u32,
    /// Initial height.
    pub height: u32,
    /// Whether the window is resizable.
    pub resizable: bool,
    /// Whether to center the window on screen.
    pub centered: bool,
}

impl Default for MainWindowConfig {
    fn default() -> Self {
        Self {
            title: "RustKit Window".to_string(),
            width: 1280,
            height: 800,
            resizable: true,
            centered: true,
        }
    }
}

impl MainWindowConfig {
    /// Create a new config with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            ..Default::default()
        }
    }

    /// Set the window dimensions.
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }
}

/// The main ViewHost that manages all views.
pub struct ViewHost {
    views: RwLock<HashMap<ViewId, Arc<Mutex<ViewState>>>>,
    /// Main window HWND (if created via create_main_window).
    #[cfg(windows)]
    main_hwnd: RwLock<Option<isize>>,
}

impl ViewHost {
    /// Create a new ViewHost.
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            // Enable per-monitor DPI awareness
            unsafe {
                let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
            }
        }

        Self {
            views: RwLock::new(HashMap::new()),
            #[cfg(windows)]
            main_hwnd: RwLock::new(None),
        }
    }

    /// Create a top-level main window.
    ///
    /// Returns the HWND of the created window. This can be used as a parent
    /// for child views created with `create_view`.
    #[cfg(windows)]
    pub fn create_main_window(&self, config: MainWindowConfig) -> Result<HWND, ViewHostError> {
        use std::ffi::OsStr;
        use std::os::windows::ffi::OsStrExt;

        info!(?config, "Creating main window");

        // Register the main window class
        let class_name = Self::register_main_class()?;

        // Convert title to wide string
        let title_wide: Vec<u16> = OsStr::new(&config.title)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // Calculate window position
        let (x, y) = if config.centered {
            let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
            let screen_height = unsafe { GetSystemMetrics(SM_CYSCREEN) };
            (
                (screen_width - config.width as i32) / 2,
                (screen_height - config.height as i32) / 2,
            )
        } else {
            (CW_USEDEFAULT, CW_USEDEFAULT)
        };

        // Window style
        let mut style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
        if !config.resizable {
            style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_CLIPCHILDREN;
        }

        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                PCWSTR::from_raw(title_wide.as_ptr()),
                style,
                x,
                y,
                config.width as i32,
                config.height as i32,
                None,
                None,
                GetModuleHandleW(None).unwrap_or_default(),
                None,
            )
        };

        let hwnd = hwnd.map_err(|e| ViewHostError::WindowCreation(e.to_string()))?;

        if hwnd.0.is_null() {
            let err = std::io::Error::last_os_error();
            error!(?err, "Failed to create main window");
            return Err(ViewHostError::WindowCreation(err.to_string()));
        }

        // Store the main HWND
        *self.main_hwnd.write().map_err(|e| {
            tracing::error!("main_hwnd RwLock poisoned in create_main_window: {}", e);
            ViewHostError::LockPoisoned
        })? = Some(hwnd.0 as isize);

        // Show the window
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = UpdateWindow(hwnd);
        }

        info!(?hwnd, "Main window created");
        Ok(hwnd)
    }

    /// Get the main window HWND if one was created.
    #[cfg(windows)]
    pub fn get_main_hwnd(&self) -> Option<HWND> {
        self.main_hwnd
            .read()
            .map_err(|e| {
                tracing::error!("main_hwnd RwLock poisoned in get_main_hwnd: {}", e);
                e
            })
            .ok()?
            .map(|raw| HWND(raw as *mut _))
    }

    /// Register the main window class (Windows only).
    #[cfg(windows)]
    fn register_main_class() -> Result<PCWSTR, ViewHostError> {
        use std::sync::Once;

        static REGISTER: Once = Once::new();
        static MAIN_CLASS_NAME: &[u16] = &[
            b'H' as u16,
            b'i' as u16,
            b'W' as u16,
            b'a' as u16,
            b'v' as u16,
            b'e' as u16,
            b'M' as u16,
            b'a' as u16,
            b'i' as u16,
            b'n' as u16,
            b'W' as u16,
            b'i' as u16,
            b'n' as u16,
            b'd' as u16,
            b'o' as u16,
            b'w' as u16,
            0,
        ];

        REGISTER.call_once(|| unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(Self::main_wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                hIcon: HICON::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR::from_raw(MAIN_CLASS_NAME.as_ptr()),
                hIconSm: HICON::default(),
            };

            let _ = RegisterClassExW(&wc);
        });

        Ok(PCWSTR::from_raw(MAIN_CLASS_NAME.as_ptr()))
    }

    /// Main window procedure.
    #[cfg(windows)]
    unsafe extern "system" fn main_wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        match msg {
            WM_SIZE => {
                // Emit resize event for the main window
                let width = (lparam.0 & 0xFFFF) as u32;
                let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                trace!(?hwnd, width, height, "Main window WM_SIZE");

                // Broadcast resize to all child views
                if let Ok(registry) = VIEW_REGISTRY.read() {
                    registry.emit(ViewEvent::Resized {
                        view_id: ViewId(0), // Special ID for main window
                        bounds: Bounds::new(0, 0, width, height),
                        dpi: GetDpiForWindow(hwnd),
                    });
                }
            }

            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                return LRESULT(0);
            }

            WM_DESTROY => {
                PostQuitMessage(0);
                return LRESULT(0);
            }

            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _hdc = BeginPaint(hwnd, &mut ps);
                let _ = EndPaint(hwnd, &ps);
                return LRESULT(0);
            }

            WM_ERASEBKGND => {
                // Prevent flicker - views handle their own backgrounds
                return LRESULT(1);
            }

            _ => {}
        }

        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// Run the Win32 message loop until WM_QUIT is received.
    ///
    /// This is a blocking call that processes all Windows messages.
    /// Returns when the window is closed.
    #[cfg(windows)]
    pub fn run_message_loop(&self) {
        info!("Starting Win32 message loop");

        unsafe {
            let mut msg = std::mem::zeroed::<MSG>();

            loop {
                let result = GetMessageW(&mut msg, None, 0, 0);
                if result.0 <= 0 {
                    // 0 = WM_QUIT, -1 = error
                    break;
                }

                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        info!("Message loop ended");
    }

    /// Process pending Win32 messages without blocking.
    ///
    /// Returns true if there are more messages to process, false if the message
    /// loop should exit (WM_QUIT received).
    #[cfg(windows)]
    pub fn pump_messages(&self) -> bool {
        unsafe {
            let mut msg = std::mem::zeroed::<MSG>();

            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return false;
                }

                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            true
        }
    }

    /// Set the event callback for all views.
    #[cfg(windows)]
    pub fn set_event_callback(&self, callback: EventCallback) {
        let mut registry = match VIEW_REGISTRY.write() {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("VIEW_REGISTRY lock poisoned in set_event_callback: {}", e);
                return;
            }
        };
        registry.set_callback(callback);
    }

    /// Set the event callback (non-Windows stub).
    #[cfg(not(windows))]
    pub fn set_event_callback(&self, _callback: EventCallback) {
        // No-op on non-Windows
    }

    /// Create a new child view under the given parent HWND.
    #[cfg(windows)]
    pub fn create_view(
        &self,
        parent: HWND,
        initial_bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        if parent.0.is_null() {
            return Err(ViewHostError::InvalidParent);
        }

        let view_id = ViewId::new();
        debug!(?view_id, ?initial_bounds, "Creating view");

        // Get DPI for the parent window
        let dpi = unsafe { GetDpiForWindow(parent) };
        let dpi = if dpi == 0 { 96 } else { dpi };

        // Create child window
        let hwnd = unsafe {
            let class_name = Self::register_class()?;

            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class_name,
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                initial_bounds.x,
                initial_bounds.y,
                initial_bounds.width as i32,
                initial_bounds.height as i32,
                parent,
                None,
                GetModuleHandleW(None).unwrap_or_default(),
                None,
            )
        };

        let hwnd = hwnd.map_err(|e| ViewHostError::WindowCreation(e.to_string()))?;

        if hwnd.0.is_null() {
            let err = std::io::Error::last_os_error();
            error!(?err, "Failed to create child window");
            return Err(ViewHostError::WindowCreation(err.to_string()));
        }

        let hwnd_raw = hwnd.0 as isize;

        let state = Arc::new(Mutex::new(ViewState {
            id: view_id,
            hwnd_raw,
            bounds: initial_bounds,
            dpi,
            visible: true,
            focused: false,
            keyboard_state: KeyboardState::new(),
            mouse_state: MouseState::new(),
            last_click_time: 0,
            last_click_pos: Point::zero(),
            click_count: 0,
            tracking_mouse: false,
        }));

        // Store in local views map
        {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in create_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views.insert(view_id, state.clone());
        }

        // Register in global registry for window proc
        {
            let mut registry = VIEW_REGISTRY.write().map_err(|e| {
                tracing::error!("VIEW_REGISTRY lock poisoned in create_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            registry.register(hwnd_raw, state);
        }

        info!(?view_id, ?hwnd, dpi, "View created");
        Ok(view_id)
    }

    /// Create a new view (macOS implementation).
    #[cfg(target_os = "macos")]
    pub fn create_view(
        &self,
        parent: WindowHandle,
        initial_bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        let view_id = ViewId::new();
        debug!(?view_id, ?initial_bounds, "Creating macOS view");

        // Extract NSWindow from raw window handle
        let raw_handle = match parent {
            raw_window_handle::RawWindowHandle::AppKit(handle) => handle,
            _ => {
                return Err(ViewHostError::InvalidParent);
            }
        };

        // In raw-window-handle 0.6, AppKitHandle contains ns_view
        let ns_view = raw_handle.ns_view.as_ptr() as id;
        if ns_view == nil {
            return Err(ViewHostError::InvalidParent);
        }
        
        // Get the window from the view
        let ns_window: id = unsafe { msg_send![ns_view, window] };
        if ns_window == nil {
            return Err(ViewHostError::InvalidParent);
        }

        // Get the content view of the window
        let content_view: id = unsafe { msg_send![ns_window, contentView] };
        if content_view == nil {
            return Err(ViewHostError::WindowCreation(
                "Window has no content view".to_string(),
            ));
        }

        // Get parent height for coordinate conversion
        let parent_height: f64 = unsafe {
            let parent_frame: cocoa::foundation::NSRect = msg_send![content_view, frame];
            parent_frame.size.height
        };

        // Convert from top-left origin (HiWave/Wry) to bottom-left origin (Cocoa)
        // Formula: y_cocoa = parent_height - bounds.y - bounds.height
        let y_cocoa = parent_height - initial_bounds.y as f64 - initial_bounds.height as f64;
        debug!(
            ?view_id,
            initial_y = initial_bounds.y,
            parent_height,
            y_cocoa,
            "Converting coordinates from top-left to bottom-left"
        );

        // RustKitContentView, not a stock NSView: a stock view was measured
        // to be an input dead end (hitTest routes clicks to it; they never
        // surface as tao window events — synthetic sendEvent probe,
        // 2026-08-07). The subclass records clicks into a queue the app
        // drains each loop turn.
        //
        // NOTE: macos.rs carries a TWIN of this whole function
        // (`MacOSViewHost::create_view_from_window`) with zero callers — the
        // first version of this fix patched that one and changed nothing.
        // The orphan-law twin-stack case, in the fix for an orphan.
        let view: id = unsafe {
            let view_class = crate::macos::rustkit_content_view_class();
            let view: id = msg_send![view_class, alloc];
            let frame = cocoa::foundation::NSRect::new(
                cocoa::foundation::NSPoint::new(initial_bounds.x as f64, y_cocoa),
                cocoa::foundation::NSSize::new(initial_bounds.width as f64, initial_bounds.height as f64),
            );
            msg_send![view, initWithFrame: frame]
        };

        if view == nil {
            return Err(ViewHostError::WindowCreation(
                "Failed to create NSView".to_string(),
            ));
        }

        // Configure the view for layer-backed rendering
        // NOTE: Don't manually create CAMetalLayer - let wgpu manage it
        // wgpu will create and configure its own Metal layer when the surface is created
        unsafe {
            // Enable layer-backed rendering (required for wgpu)
            let wants_layer: bool = true;
            let _: () = msg_send![view, setWantsLayer: wants_layer];
        }

        // Add view to content view
        unsafe {
            let _: () = msg_send![content_view, addSubview: view];
        }

        // Get DPI (backing scale factor)
        let dpi = unsafe {
            let scale: f64 = msg_send![ns_window, backingScaleFactor];
            (scale * 96.0) as u32
        };

        let state = Arc::new(Mutex::new(ViewState {
            id: view_id,
            hwnd_raw: view as isize, // Store NSView pointer as isize
            bounds: initial_bounds,
            dpi,
            visible: true,
            focused: false,
        }));

        {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in create_view (macOS): {}", e);
                ViewHostError::LockPoisoned
            })?;
            views.insert(view_id, state);
        }

        info!(?view_id, dpi, "macOS view created");
        Ok(view_id)
    }

    /// Create a new view (non-macOS, non-Windows stub).
    #[cfg(not(any(windows, target_os = "macos")))]
    pub fn create_view(
        &self,
        _parent: (),
        initial_bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        let view_id = ViewId::new();
        let state = Arc::new(Mutex::new(ViewState {
            id: view_id,
            hwnd_raw: 0,
            bounds: initial_bounds,
            dpi: 96,
            visible: true,
            focused: false,
        }));
        self.views.write().map_err(|e| {
            tracing::error!("Views RwLock poisoned in create_view (non-mac/win): {}", e);
            ViewHostError::LockPoisoned
        })?.insert(view_id, state);
        Ok(view_id)
    }

    /// Set the bounds of a view.
    pub fn set_bounds(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError> {
        let views = self.views.read().unwrap();
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Record under the lock, release, THEN call the platform:
        // SetWindowPos dispatches WM_SIZE synchronously back into wnd_proc,
        // which re-locks this same mutex (same class as focus above).
        let hwnd_raw = {
            let mut guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in set_bounds: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.bounds = bounds;
            guard.hwnd_raw
        };
        drop(views);
        let _ = hwnd_raw;

        #[cfg(windows)]
        {
            let hwnd = HWND(hwnd_raw as *mut _);
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    bounds.x,
                    bounds.y,
                    bounds.width as i32,
                    bounds.height as i32,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );

                // Force repaint
                let _ = InvalidateRect(hwnd, None, false);
            }
        }

        trace!(?view_id, ?bounds, "Bounds updated");
        Ok(())
    }

    /// Get the current bounds of a view.
    pub fn get_bounds(&self, view_id: ViewId) -> Result<Bounds, ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in get_bounds: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let bounds = state.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_bounds: {}", e);
            ViewHostError::LockPoisoned
        })?.bounds;
        Ok(bounds)
    }

    /// Set view visibility.
    pub fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in set_visible: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Same copy-drop-call shape as focus/set_bounds: ShowWindow
        // dispatches WM_SHOWWINDOW synchronously.
        let hwnd_raw = {
            let mut guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in set_visible: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.visible = visible;
            guard.hwnd_raw
        };
        drop(views);
        let _ = hwnd_raw;

        #[cfg(windows)]
        {
            let hwnd = HWND(hwnd_raw as *mut _);
            unsafe {
                let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
            }
        }

        debug!(?view_id, visible, "Visibility changed");
        Ok(())
    }

    /// Focus a view.
    pub fn focus(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in focus: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Copy the handle, DROP BOTH GUARDS, then call the platform.
        // SetFocus dispatches WM_SETFOCUS synchronously into our own
        // wnd_proc (hiwave-windows#85: hung forever from the day it was
        // written); makeFirstResponder re-enters the responder chain the
        // moment the view has responder overrides — which, as of #115's
        // RustKitContentView, it now does. #108 fixed exactly this shape in
        // macos.rs, but on the ORPHAN TWIN of this function; this is the
        // live one.
        let hwnd_raw = state
            .lock()
            .map_err(|e| {
                tracing::error!("ViewState lock poisoned in focus: {}", e);
                ViewHostError::LockPoisoned
            })?
            .hwnd_raw;
        drop(views);

        #[cfg(windows)]
        {
            let hwnd = HWND(hwnd_raw as *mut _);
            unsafe {
                let _ = SetFocus(hwnd);
            }
        }

        #[cfg(target_os = "macos")]
        {
            let view = hwnd_raw as id;
            unsafe {
                let window: id = msg_send![view, window];
                if window != nil {
                    let _: () = msg_send![window, makeFirstResponder: view];
                }
            }
        }

        debug!(?view_id, "Focus requested");
        Ok(())
    }

    /// Get the HWND for a view.
    #[cfg(windows)]
    pub fn get_hwnd(&self, view_id: ViewId) -> Result<HWND, ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in get_hwnd: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let hwnd_raw = state.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_hwnd: {}", e);
            ViewHostError::LockPoisoned
        })?.hwnd_raw;
        Ok(HWND(hwnd_raw as *mut _))
    }

    /// Get the DPI for a view.
    pub fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in get_dpi: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let dpi = state.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_dpi: {}", e);
            ViewHostError::LockPoisoned
        })?.dpi;
        Ok(dpi)
    }

    /// Destroy a view.
    pub fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let state = {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in destroy_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views.remove(&view_id)
        };

        if let Some(state) = state {
            let state_lock = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in destroy_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            #[cfg(windows)]
            let hwnd_raw = state_lock.hwnd_raw;
            drop(state_lock);

            #[cfg(windows)]
            {
                // Unregister from global registry
                {
                    let mut registry = VIEW_REGISTRY.write().map_err(|e| {
                        tracing::error!("VIEW_REGISTRY lock poisoned in destroy_view: {}", e);
                        ViewHostError::LockPoisoned
                    })?;
                    registry.unregister(hwnd_raw);
                }

                let hwnd = HWND(hwnd_raw as *mut _);
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }

            info!(?view_id, "View destroyed");
            Ok(())
        } else {
            Err(ViewHostError::ViewNotFound(view_id))
        }
    }

    /// Get the number of active views.
    pub fn view_count(&self) -> usize {
        self.views.read().map(|v| v.len()).unwrap_or(0)
    }

    /// Register the window class (Windows only).
    #[cfg(windows)]
    fn register_class() -> Result<PCWSTR, ViewHostError> {
        use std::sync::Once;

        static REGISTER: Once = Once::new();
        static CLASS_NAME: &[u16] = &[
            b'R' as u16,
            b'u' as u16,
            b's' as u16,
            b't' as u16,
            b'K' as u16,
            b'i' as u16,
            b't' as u16,
            b'V' as u16,
            b'i' as u16,
            b'e' as u16,
            b'w' as u16,
            b'H' as u16,
            b'o' as u16,
            b's' as u16,
            b't' as u16,
            0,
        ];

        REGISTER.call_once(|| unsafe {
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
                lpfnWndProc: Some(Self::wnd_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                hIcon: HICON::default(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hbrBackground: HBRUSH::default(),
                lpszMenuName: PCWSTR::null(),
                lpszClassName: PCWSTR::from_raw(CLASS_NAME.as_ptr()),
                hIconSm: HICON::default(),
            };

            let _ = RegisterClassExW(&wc);
        });

        Ok(PCWSTR::from_raw(CLASS_NAME.as_ptr()))
    }

    /// Get current timestamp in milliseconds.
    #[cfg(windows)]
    fn timestamp() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Get current modifier state.
    #[cfg(windows)]
    fn get_modifiers() -> Modifiers {
        unsafe {
            Modifiers {
                ctrl: GetAsyncKeyState(VK_CONTROL.0 as i32) < 0,
                alt: GetAsyncKeyState(VK_MENU.0 as i32) < 0,
                shift: GetAsyncKeyState(VK_SHIFT.0 as i32) < 0,
                meta: GetAsyncKeyState(VK_LWIN.0 as i32) < 0
                    || GetAsyncKeyState(VK_RWIN.0 as i32) < 0,
            }
        }
    }

    /// Translate Win32 mouse button.
    #[cfg(windows)]
    fn translate_mouse_button(msg: u32) -> MouseButton {
        match msg {
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_LBUTTONDBLCLK => MouseButton::Primary,
            WM_RBUTTONDOWN | WM_RBUTTONUP | WM_RBUTTONDBLCLK => MouseButton::Secondary,
            WM_MBUTTONDOWN | WM_MBUTTONUP | WM_MBUTTONDBLCLK => MouseButton::Auxiliary,
            WM_XBUTTONDOWN | WM_XBUTTONUP | WM_XBUTTONDBLCLK => MouseButton::Back,
            _ => MouseButton::Primary,
        }
    }

    /// Window procedure for view windows.
    #[cfg(windows)]
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let hwnd_raw = hwnd.0 as isize;

        // Helper to get view state
        let get_state = || -> Option<Arc<Mutex<ViewState>>> {
            let registry = VIEW_REGISTRY.read().ok()?;
            registry.get(hwnd_raw)
        };

        // Helper to emit event
        let emit = |event: ViewEvent| {
            if let Ok(registry) = VIEW_REGISTRY.read() {
                registry.emit(event);
            }
        };

        match msg {
            // === Mouse Events ===
            WM_MOUSEMOVE => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_MOUSEMOVE: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let x = (lparam.0 & 0xFFFF) as i16 as f64;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
                    let pos = Point::new(x, y);

                    state.mouse_state.set_position(pos);

                    // Start mouse tracking for WM_MOUSELEAVE
                    if !state.tracking_mouse {
                        let mut tme = TRACKMOUSEEVENT {
                            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                            dwFlags: TME_LEAVE,
                            hwndTrack: hwnd,
                            dwHoverTime: 0,
                        };
                        let _ = TrackMouseEvent(&mut tme);
                        state.tracking_mouse = true;
                    }

                    let view_id = state.id;
                    let buttons = state.mouse_state.buttons;
                    drop(state);

                    let event = MouseEvent::new(MouseEventType::MouseMove, pos)
                        .with_buttons(buttons)
                        .with_modifiers(Self::get_modifiers())
                        .with_timestamp(Self::timestamp());

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Mouse(event),
                    });
                }
            }

            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_*BUTTONDOWN: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let x = (lparam.0 & 0xFFFF) as i16 as f64;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
                    let pos = Point::new(x, y);
                    let button = Self::translate_mouse_button(msg);
                    let timestamp = Self::timestamp();

                    state.mouse_state.button_down(button);

                    // Detect double-click (within 500ms and 5 pixels)
                    let double_click_time = 500;
                    let double_click_dist = 5.0;
                    if timestamp - state.last_click_time < double_click_time
                        && (pos.x - state.last_click_pos.x).abs() < double_click_dist
                        && (pos.y - state.last_click_pos.y).abs() < double_click_dist
                    {
                        state.click_count += 1;
                    } else {
                        state.click_count = 1;
                    }
                    state.last_click_time = timestamp;
                    state.last_click_pos = pos;

                    let view_id = state.id;
                    let buttons = state.mouse_state.buttons;
                    let click_count = state.click_count;
                    drop(state);

                    let event = MouseEvent::new(MouseEventType::MouseDown, pos)
                        .with_button(button)
                        .with_buttons(buttons)
                        .with_click_count(click_count)
                        .with_modifiers(Self::get_modifiers())
                        .with_timestamp(timestamp);

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Mouse(event),
                    });
                }
            }

            WM_LBUTTONUP | WM_RBUTTONUP | WM_MBUTTONUP => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_*BUTTONUP: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let x = (lparam.0 & 0xFFFF) as i16 as f64;
                    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f64;
                    let pos = Point::new(x, y);
                    let button = Self::translate_mouse_button(msg);

                    state.mouse_state.button_up(button);

                    let view_id = state.id;
                    let buttons = state.mouse_state.buttons;
                    let click_count = state.click_count;
                    drop(state);

                    let event = MouseEvent::new(MouseEventType::MouseUp, pos)
                        .with_button(button)
                        .with_buttons(buttons)
                        .with_click_count(click_count)
                        .with_modifiers(Self::get_modifiers())
                        .with_timestamp(Self::timestamp());

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Mouse(event),
                    });
                }
            }

            WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                if let Some(state) = get_state() {
                    let state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_MOUSEWHEEL: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let view_id = state.id;
                    drop(state);

                    // Convert screen coords to client coords
                    let mut pt = POINT {
                        x: (lparam.0 & 0xFFFF) as i16 as i32,
                        y: ((lparam.0 >> 16) & 0xFFFF) as i16 as i32,
                    };
                    let _ = ScreenToClient(hwnd, &mut pt);
                    let pos = Point::new(pt.x as f64, pt.y as f64);

                    let delta_raw = (wparam.0 >> 16) as i16 as f64;
                    let delta = if msg == WM_MOUSEWHEEL {
                        Point::new(0.0, delta_raw / 120.0)
                    } else {
                        Point::new(delta_raw / 120.0, 0.0)
                    };

                    let event = MouseEvent::new(MouseEventType::Wheel, pos)
                        .with_delta(delta)
                        .with_modifiers(Self::get_modifiers())
                        .with_timestamp(Self::timestamp());

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Mouse(event),
                    });
                }
            }

            m if m == WM_MOUSELEAVE_MSG => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_MOUSELEAVE: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    state.tracking_mouse = false;
                    let view_id = state.id;
                    let pos = state.mouse_state.position;
                    drop(state);

                    let event = MouseEvent::new(MouseEventType::MouseLeave, pos)
                        .with_timestamp(Self::timestamp());

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Mouse(event),
                    });
                }
            }

            // === Keyboard Events ===
            WM_KEYDOWN | WM_SYSKEYDOWN => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_KEYDOWN: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let vk = wparam.0 as u32;
                    let key_code = KeyCode::from_vk(vk);

                    let repeat = state.keyboard_state.key_down(key_code);
                    let modifiers = state.keyboard_state.modifiers();
                    let view_id = state.id;
                    drop(state);

                    let event = KeyEvent::new(KeyEventType::KeyDown, key_code, modifiers)
                        .with_repeat(repeat)
                        .with_timestamp(Self::timestamp());

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Key(event),
                    });
                }
            }

            WM_KEYUP | WM_SYSKEYUP => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_KEYUP: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let vk = wparam.0 as u32;
                    let key_code = KeyCode::from_vk(vk);

                    state.keyboard_state.key_up(key_code);
                    let modifiers = state.keyboard_state.modifiers();
                    let view_id = state.id;
                    drop(state);

                    let event = KeyEvent::new(KeyEventType::KeyUp, key_code, modifiers)
                        .with_timestamp(Self::timestamp());

                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Key(event),
                    });
                }
            }

            WM_CHAR => {
                if let Some(state) = get_state() {
                    let state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_CHAR: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let view_id = state.id;
                    drop(state);

                    // wparam contains the UTF-16 code unit
                    let ch = char::from_u32(wparam.0 as u32).unwrap_or('\0');
                    if !ch.is_control() || ch == '\r' || ch == '\t' {
                        let event = KeyEvent::input(ch).with_timestamp(Self::timestamp());

                        emit(ViewEvent::Input {
                            view_id,
                            event: InputEvent::Key(event),
                        });
                    }
                }
            }

            // === Focus Events ===
            WM_SETFOCUS => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_SETFOCUS: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    state.focused = true;
                    let view_id = state.id;
                    drop(state);

                    let event =
                        FocusEvent::new(FocusEventType::Focus).with_timestamp(Self::timestamp());

                    emit(ViewEvent::Focused { view_id });
                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Focus(event),
                    });
                }
            }

            WM_KILLFOCUS => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_KILLFOCUS: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    state.focused = false;
                    let view_id = state.id;
                    drop(state);

                    let event =
                        FocusEvent::new(FocusEventType::Blur).with_timestamp(Self::timestamp());

                    emit(ViewEvent::Blurred { view_id });
                    emit(ViewEvent::Input {
                        view_id,
                        event: InputEvent::Focus(event),
                    });
                }
            }

            // === Window Events ===
            WM_SIZE => {
                if let Some(state) = get_state() {
                    let state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_SIZE: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let width = (lparam.0 & 0xFFFF) as u32;
                    let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
                    let view_id = state.id;
                    let bounds = Bounds::new(state.bounds.x, state.bounds.y, width, height);
                    let dpi = state.dpi;
                    drop(state);

                    trace!(?view_id, width, height, "WM_SIZE received");
                    emit(ViewEvent::Resized {
                        view_id,
                        bounds,
                        dpi,
                    });
                }
            }

            WM_DPICHANGED => {
                if let Some(state) = get_state() {
                    let mut state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_DPICHANGED: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let new_dpi = (wparam.0 & 0xFFFF) as u32;
                    state.dpi = new_dpi;
                    let view_id = state.id;
                    drop(state);

                    let suggested_rect = lparam.0 as *const RECT;
                    if !suggested_rect.is_null() {
                        let rect = &*suggested_rect;
                        let _ = SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            rect.top,
                            rect.right - rect.left,
                            rect.bottom - rect.top,
                            SWP_NOZORDER | SWP_NOACTIVATE,
                        );
                    }

                    trace!(?view_id, new_dpi, "WM_DPICHANGED");
                    emit(ViewEvent::DpiChanged {
                        view_id,
                        dpi: new_dpi,
                    });
                }
            }

            WM_PAINT => {
                let mut ps = PAINTSTRUCT::default();
                let _hdc = BeginPaint(hwnd, &mut ps);
                // Compositor handles actual painting
                let _ = EndPaint(hwnd, &ps);
                return LRESULT(0);
            }

            WM_ERASEBKGND => {
                // Prevent flicker - compositor handles background
                return LRESULT(1);
            }

            WM_DESTROY => {
                if let Some(state) = get_state() {
                    let state = match state.lock() {
                        Ok(s) => s,
                        Err(e) => {
                            tracing::error!("ViewState lock poisoned in WM_DESTROY: {}", e);
                            return DefWindowProcW(hwnd, msg, wparam, lparam);
                        }
                    };
                    let view_id = state.id;
                    drop(state);

                    trace!(?view_id, "WM_DESTROY");
                    emit(ViewEvent::Destroyed { view_id });
                }
            }

            _ => {}
        }

        DefWindowProcW(hwnd, msg, wparam, lparam)
    }
}

impl Default for ViewHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ViewHost {
    fn drop(&mut self) {
        // Destroy all views
        let view_ids: Vec<_> = self.views.read()
            .map(|views| views.keys().copied().collect())
            .unwrap_or_else(|e| {
                tracing::error!("Views RwLock poisoned in ViewHost::drop: {}", e);
                Vec::new()
            });
        for view_id in view_ids {
            let _ = self.destroy_view(view_id);
        }
    }
}

// ============================================================================
// ViewHostTrait Implementation
// ============================================================================

#[cfg(target_os = "windows")]
impl ViewHostTrait for ViewHost {
    fn create_view(
        &self,
        parent: WindowHandle,
        bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        self.create_view(parent, bounds)
    }

    fn resize_view(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError> {
        self.set_bounds(view_id, bounds)
    }

    fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        self.destroy_view(view_id)
    }

    fn get_hwnd(&self, view_id: ViewId) -> Result<windows::Win32::Foundation::HWND, ViewHostError> {
        self.get_hwnd(view_id)
    }

    fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError> {
        self.set_visible(view_id, visible)
    }

    fn focus_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        self.focus(view_id)
    }

    fn pump_messages(&self) -> bool {
        // macOS doesn't need message pumping like Windows
        // Events are handled by the Cocoa event loop
        false
    }

    fn get_bounds(&self, view_id: ViewId) -> Result<Bounds, ViewHostError> {
        self.get_bounds(view_id)
    }

    fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError> {
        self.get_dpi(view_id)
    }
}

#[cfg(not(target_os = "windows"))]
impl ViewHostTrait for ViewHost {
    fn create_view(
        &self,
        parent: WindowHandle,
        bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        self.create_view(parent, bounds)
    }

    fn resize_view(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError> {
        self.set_bounds(view_id, bounds)
    }

    fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        self.destroy_view(view_id)
    }

    #[cfg(target_os = "macos")]
    fn get_raw_window_handle(&self, view_id: ViewId) -> Result<raw_window_handle::RawWindowHandle, ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in get_raw_window_handle: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let view = state.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_raw_window_handle: {}", e);
            ViewHostError::LockPoisoned
        })?.hwnd_raw as id;

        // Get the window from the view
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            warn!(?view_id, "View has no window attached");
            return Err(ViewHostError::ViewNotFound(view_id));
        }

        // Create raw window handle for raw-window-handle 0.6
        // In version 0.6, AppKitWindowHandle uses ns_view field
        use raw_window_handle::{RawWindowHandle, AppKitWindowHandle};
        use std::ptr::NonNull;
        // AppKitWindowHandle::new() expects NonNull<c_void>
        let handle = RawWindowHandle::AppKit(
            AppKitWindowHandle::new(
                NonNull::new(view as *mut std::ffi::c_void)
                    .expect("View pointer is null")
            )
        );

        Ok(handle)
    }

    fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError> {
        self.set_visible(view_id, visible)
    }

    fn focus_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        self.focus(view_id)
    }

    fn pump_messages(&self) -> bool {
        // macOS doesn't need message pumping like Windows
        // Events are handled by the Cocoa event loop
        false
    }

    fn get_bounds(&self, view_id: ViewId) -> Result<Bounds, ViewHostError> {
        self.get_bounds(view_id)
    }

    fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError> {
        self.get_dpi(view_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_view_id_uniqueness() {
        let id1 = ViewId::new();
        let id2 = ViewId::new();
        let id3 = ViewId::new();

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_bounds() {
        let bounds = Bounds::new(10, 20, 800, 600);
        assert_eq!(bounds.x, 10);
        assert_eq!(bounds.y, 20);
        assert_eq!(bounds.width, 800);
        assert_eq!(bounds.height, 600);
    }

    #[test]
    fn test_viewhost_creation() {
        let host = ViewHost::new();
        assert_eq!(host.view_count(), 0);
    }

    #[test]
    fn test_lock_poisoning_handling() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        // Create a mutex that we'll intentionally poison
        let poisoned_lock: Arc<Mutex<i32>> = Arc::new(Mutex::new(42));
        let poisoned_lock_clone = poisoned_lock.clone();

        // Spawn a thread that panics while holding the lock
        let _ = thread::spawn(move || {
            let _guard = poisoned_lock_clone.lock().unwrap();
            panic!("Intentional panic to poison the lock");
        })
        .join();

        // Now the lock is poisoned - attempting to acquire it should fail
        let result = poisoned_lock.lock();
        assert!(result.is_err(), "Lock should be poisoned");

        // Verify we can handle the poisoned lock gracefully
        match result {
            Ok(_) => panic!("Expected poisoned lock"),
            Err(e) => {
                // This demonstrates our error handling pattern
                tracing::error!("Lock poisoned (expected in test): {}", e);
                // In the real code, we would return ViewHostError::LockPoisoned
            }
        }
    }

    #[test]
    fn test_bounds_zero_size() {
        let bounds = Bounds::new(0, 0, 0, 0);
        assert_eq!(bounds.width, 0);
        assert_eq!(bounds.height, 0);
    }

    #[test]
    fn test_bounds_negative_position() {
        // Negative positions are valid (multi-monitor setups)
        let bounds = Bounds::new(-100, -50, 800, 600);
        assert_eq!(bounds.x, -100);
        assert_eq!(bounds.y, -50);
    }

    #[test]
    fn test_bounds_large_values() {
        // Test with 4K resolution
        let bounds = Bounds::new(0, 0, 3840, 2160);
        assert_eq!(bounds.width, 3840);
        assert_eq!(bounds.height, 2160);
    }

    #[test]
    fn test_bounds_equality() {
        let b1 = Bounds::new(10, 20, 800, 600);
        let b2 = Bounds::new(10, 20, 800, 600);
        let b3 = Bounds::new(10, 20, 1024, 768);

        assert_eq!(b1, b2);
        assert_ne!(b1, b3);
    }

    #[test]
    fn test_view_id_raw_value() {
        let id = ViewId::new();
        let raw = id.raw();

        // Raw value should be non-zero
        assert!(raw > 0, "ViewId raw value should be non-zero");
    }

    #[test]
    fn test_view_id_many_unique() {
        // Generate many IDs and verify uniqueness
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let id = ViewId::new();
            assert!(ids.insert(id), "ViewId should be unique");
        }
        assert_eq!(ids.len(), 1000);
    }

    #[test]
    fn test_viewhost_view_count_initial() {
        let host = ViewHost::new();
        assert_eq!(host.view_count(), 0, "New ViewHost should have 0 views");
    }

    #[test]
    fn test_viewhost_error_display() {
        // Test that error messages are formatted correctly
        let err = ViewHostError::ViewNotFound(ViewId::new());
        let msg = format!("{}", err);
        assert!(msg.contains("View not found"), "Error message should be descriptive");

        let err = ViewHostError::WindowCreation("test error".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Failed to create window"), "Error message should be descriptive");
        assert!(msg.contains("test error"), "Error message should include details");
    }

    #[test]
    fn test_viewhost_error_lock_poisoned() {
        let err = ViewHostError::LockPoisoned;
        let msg = format!("{}", err);
        assert!(msg.contains("poisoned"), "Error should mention lock poisoning");
    }

    #[test]
    fn test_viewhost_multiple_instances() {
        // Verify multiple ViewHost instances can coexist
        let host1 = ViewHost::new();
        let host2 = ViewHost::new();
        let host3 = ViewHost::new();

        assert_eq!(host1.view_count(), 0);
        assert_eq!(host2.view_count(), 0);
        assert_eq!(host3.view_count(), 0);
    }

    #[test]
    fn test_view_id_debug_format() {
        let id = ViewId::new();
        let debug_str = format!("{:?}", id);

        // Debug format should include "ViewId"
        assert!(debug_str.contains("ViewId"), "Debug format should be descriptive");
    }

    #[test]
    fn test_bounds_debug_format() {
        let bounds = Bounds::new(10, 20, 800, 600);
        let debug_str = format!("{:?}", bounds);

        // Debug format should show all values
        assert!(debug_str.contains("10"));
        assert!(debug_str.contains("20"));
        assert!(debug_str.contains("800"));
        assert!(debug_str.contains("600"));
    }

    #[test]
    fn test_bounds_clone() {
        let b1 = Bounds::new(10, 20, 800, 600);
        let b2 = b1.clone();

        assert_eq!(b1, b2);
        assert_eq!(b1.x, b2.x);
        assert_eq!(b1.y, b2.y);
        assert_eq!(b1.width, b2.width);
        assert_eq!(b1.height, b2.height);
    }

    #[test]
    fn test_view_id_clone() {
        let id1 = ViewId::new();
        let id2 = id1.clone();

        // Cloned IDs should be equal
        assert_eq!(id1, id2);
        assert_eq!(id1.raw(), id2.raw());
    }

    #[test]
    fn test_viewhost_error_invalid_parent() {
        let err = ViewHostError::InvalidParent;
        let msg = format!("{}", err);
        assert!(msg.contains("Invalid parent window"), "Error message should be descriptive");
    }

    #[test]
    fn test_viewhost_error_windows_api() {
        let err = ViewHostError::WindowsApi("test API failure".to_string());
        let msg = format!("{}", err);
        assert!(msg.contains("Windows API error"), "Error message should be descriptive");
        assert!(msg.contains("test API failure"), "Error message should include details");
    }

    // Note: Full view lifecycle tests (create_view, destroy_view, resize_view)
    // require valid window handles and are tested in hiwave-app integration tests.
    // The tests above cover all testable logic that doesn't require platform windows.
}
