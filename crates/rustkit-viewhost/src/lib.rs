//! # RustKit ViewHost
//!
//! Cross-platform window hosting layer for the RustKit browser engine.
//! Provides platform-native windowing without external frameworks like tao/wry.
//!
//! ## Platform Support
//!
//! - **Windows**: Win32 API (HWND, WM_* messages)
//! - **macOS**: Cocoa/AppKit (NSWindow, NSView)
//! - **Linux**: X11 (initial), Wayland (planned)
//!
//! ## Design Goals
//!
//! 1. **Multi-view support**: Each view has isolated state, no global singletons
//! 2. **Resize correctness**: Platform resize events trigger surface resize immediately
//! 3. **DPI awareness**: Per-monitor DPI scaling on all platforms
//! 4. **Focus management**: Proper focus chain for keyboard events
//! 5. **Input handling**: Platform messages translated to cross-platform events

// Allow Arc with non-Send/Sync types - intentional for native handle handling
#![allow(clippy::arc_with_non_send_sync)]

// Platform-specific modules
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

// Screenshot capture
pub mod screenshot;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use thiserror::Error;
use tracing::{debug, error, info, trace};

#[cfg(windows)]
use rustkit_core::{
    FocusEvent, FocusEventType, InputEvent, KeyCode, KeyEvent, KeyEventType, KeyboardState,
    Modifiers, MouseButton, MouseEvent, MouseEventType, MouseState, Point,
};

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

    /// The child window that should receive keyboard focus.
    ///
    /// The most recently focused visible view, falling back to the first
    /// visible one. Deliberately NOT "the first view created": that is the
    /// chrome, and forwarding keys there would send every shortcut to the
    /// browser UI instead of the page.
    fn focus_target(&self) -> Option<isize> {
        self.hwnd_to_state
            .values()
            .find(|v| {
                let s = v.lock().ok();
                s.map(|s| s.focused && s.visible).unwrap_or(false)
            })
            .or_else(|| {
                self.hwnd_to_state
                    .values()
                    .find(|v| v.lock().ok().map(|s| s.visible).unwrap_or(false))
            })
            .and_then(|v| v.lock().ok().map(|s| s.hwnd_raw))
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
        *self.main_hwnd.write().unwrap() = Some(hwnd.0 as isize);

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
            .unwrap()
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

            // KEYBOARD INPUT DEPENDS ON THIS. Win32 delivers WM_KEYDOWN to the
            // FOCUSED window, and only the CHILD view proc emits key events --
            // this proc handles none. Focusing a child at startup is not
            // enough: focus set BEFORE the top-level window is first activated
            // is discarded by Windows on activation, so every keypress went to
            // a window that dropped it. Measured by injecting real chords with
            // SendInput and reading the log: OS accepted them, shell saw
            // nothing.
            //
            // Forwarding on WM_SETFOCUS is the standard remedy: whenever the
            // frame takes focus, hand it to the child that actually wants
            // keys.
            WM_SETFOCUS => {
                let child = {
                    let registry = VIEW_REGISTRY.read().ok();
                    registry.and_then(|r| r.focus_target())
                };
                if let Some(hwnd_raw) = child {
                    unsafe {
                        let _ = SetFocus(HWND(hwnd_raw as *mut _));
                    }
                    trace!(hwnd_raw, "Frame focus forwarded to child view");
                }
                return LRESULT(0);
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
        let mut registry = VIEW_REGISTRY.write().unwrap();
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
            let mut views = self.views.write().unwrap();
            views.insert(view_id, state.clone());
        }

        // Register in global registry for window proc
        {
            let mut registry = VIEW_REGISTRY.write().unwrap();
            registry.register(hwnd_raw, state);
        }

        info!(?view_id, ?hwnd, dpi, "View created");
        Ok(view_id)
    }

    /// Create a new view (non-Windows stub).
    #[cfg(not(windows))]
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
        self.views.write().unwrap().insert(view_id, state);
        Ok(view_id)
    }

    /// Set the bounds of a view.
    pub fn set_bounds(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError> {
        let views = self.views.read().unwrap();
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        let mut state = state.lock().unwrap();
        state.bounds = bounds;

        #[cfg(windows)]
        {
            let hwnd = HWND(state.hwnd_raw as *mut _);
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
        let views = self.views.read().unwrap();
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let bounds = state.lock().unwrap().bounds;
        Ok(bounds)
    }

    /// Set view visibility.
    pub fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError> {
        let views = self.views.read().unwrap();
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        let mut state = state.lock().unwrap();
        state.visible = visible;

        #[cfg(windows)]
        {
            let hwnd = HWND(state.hwnd_raw as *mut _);
            unsafe {
                let _ = ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
            }
        }

        debug!(?view_id, visible, "Visibility changed");
        Ok(())
    }

    /// Focus a view.
    pub fn focus(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        // SELF-DEADLOCK, FIXED 2026-08-06: this used to hold the per-view
        // Mutex across SetFocus. SetFocus is SYNCHRONOUS — it dispatches
        // WM_SETFOCUS straight into our own `wnd_proc`, which locks the SAME
        // per-view Mutex to update `focused`. std Mutex is not reentrant, so
        // the process hung forever on the FIRST focus call. Every call did
        // this; the API was simply never used, which is why nothing found it
        // until the native shell tried to focus its content view so that
        // keyboard shortcuts could work.
        //
        // The rule this encodes: never hold a lock across a synchronous
        // platform call that can re-enter your own window procedure. Copy
        // what you need, drop the guards, then call.
        #[allow(unused_variables)]
        let hwnd_raw = {
            let views = self.views.read().unwrap();
            let state = views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?;
            let state = state.lock().unwrap();
            state.hwnd_raw
            // both guards drop here, BEFORE SetFocus
        };

        #[cfg(windows)]
        unsafe {
            let _ = SetFocus(HWND(hwnd_raw as *mut _));
        }

        debug!(?view_id, "Focus requested");
        Ok(())
    }

    /// Get the HWND for a view.
    #[cfg(windows)]
    pub fn get_hwnd(&self, view_id: ViewId) -> Result<HWND, ViewHostError> {
        let views = self.views.read().unwrap();
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let hwnd_raw = state.lock().unwrap().hwnd_raw;
        Ok(HWND(hwnd_raw as *mut _))
    }

    /// Get the DPI for a view.
    pub fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError> {
        let views = self.views.read().unwrap();
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let dpi = state.lock().unwrap().dpi;
        Ok(dpi)
    }

    /// Destroy a view.
    pub fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let state = {
            let mut views = self.views.write().unwrap();
            views.remove(&view_id)
        };

        if let Some(state) = state {
            let state_lock = state.lock().unwrap();
            let hwnd_raw = state_lock.hwnd_raw;
            drop(state_lock);

            #[cfg(windows)]
            {
                // Unregister from global registry
                {
                    let mut registry = VIEW_REGISTRY.write().unwrap();
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
        self.views.read().unwrap().len()
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
                    let mut state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let state = state.lock().unwrap();
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
                    let mut state = state.lock().unwrap();
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
                    let state = state.lock().unwrap();
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
        let view_ids: Vec<_> = self.views.read().unwrap().keys().copied().collect();
        for view_id in view_ids {
            let _ = self.destroy_view(view_id);
        }
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

    #[cfg(not(windows))]
    #[test]
    fn test_view_lifecycle_stub() {
        let host = ViewHost::new();
        let bounds = Bounds::new(0, 0, 800, 600);

        let view_id = host.create_view((), bounds).unwrap();
        assert_eq!(host.view_count(), 1);

        assert_eq!(host.get_bounds(view_id).unwrap(), bounds);

        let new_bounds = Bounds::new(10, 10, 1024, 768);
        host.set_bounds(view_id, new_bounds).unwrap();
        assert_eq!(host.get_bounds(view_id).unwrap(), new_bounds);

        host.destroy_view(view_id).unwrap();
        assert_eq!(host.view_count(), 0);
    }
}
