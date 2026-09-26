//! macOS ViewHost implementation using NSView and Cocoa
//!
//! This module provides the macOS-specific implementation of ViewHost,
//! using NSView for rendering surfaces and TAO window handles.

use crate::{Bounds, ViewHostError, ViewId};
use raw_window_handle::RawWindowHandle;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{debug, info, warn};

#[cfg(target_os = "macos")]
use cocoa::{
    base::{id, nil},
};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};

/// macOS-specific view state
#[cfg(target_os = "macos")]
struct MacOSViewState {
    _id: ViewId,
    view: id, // NSView
    bounds: Bounds,
    dpi: u32,
    visible: bool,
    _focused: bool,
}

/// macOS ViewHost implementation
#[cfg(target_os = "macos")]
pub struct MacOSViewHost {
    views: RwLock<HashMap<ViewId, Arc<Mutex<MacOSViewState>>>>,
}

/// A content-view click, in VIEW-LOCAL TOP-LEFT coordinates — exactly the
/// viewport space the engine's hit testing speaks, no chrome/sidebar math.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy)]
pub struct PendingClick {
    pub x: f64,
    pub y: f64,
    pub down: bool,
}

/// Clicks captured by the RustKit NSView, drained by the app each loop turn.
///
/// A queue, not a callback: the handlers run inside AppKit's event dispatch,
/// and calling back into app/engine state from there is the re-entrancy trap
/// #108 exists to prevent. Push under a short lock, drain on the main loop.
#[cfg(target_os = "macos")]
static PENDING_CLICKS: Mutex<Vec<PendingClick>> = Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
pub fn drain_pending_clicks() -> Vec<PendingClick> {
    PENDING_CLICKS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

/// A key event captured by the content view while it is first responder.
///
/// `text` carries the typed characters (empty for pure control keys);
/// `mac_keycode` is the hardware-independent macOS keyCode for specials.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub struct PendingKey {
    pub text: String,
    pub mac_keycode: u16,
    pub ctrl: bool,
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
}

#[cfg(target_os = "macos")]
static PENDING_KEYS: Mutex<Vec<PendingKey>> = Mutex::new(Vec::new());

#[cfg(target_os = "macos")]
pub fn drain_pending_keys() -> Vec<PendingKey> {
    PENDING_KEYS.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default()
}

/// The NSView subclass that hosts RustKit content.
///
/// A stock NSView was measured to be a dead end for input: hitTest correctly
/// routes clicks to it, but events delivered to it NEVER surface as tao
/// window events — a synthetic mouseDown through `window sendEvent:` produced
/// nothing at the event loop (2026-08-07 probe). So the view records clicks
/// itself. Wheel is left alone: scroll DOES reach the window loop (verified
/// live 2026-08-05) via a different AppKit forwarding path.
#[cfg(target_os = "macos")]
pub fn rustkit_content_view_class() -> &'static objc::runtime::Class {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        let superclass = Class::get("NSView").expect("NSView class");
        let mut decl =
            ClassDecl::new("RustKitContentView", superclass).expect("register RustKitContentView");

        extern "C" fn record(this: &Object, event: id, down: bool) {
            tracing::info!(down, "RustKitContentView mouse event handler entered");
            unsafe {
                // locationInWindow is window coords (bottom-left origin);
                // convertPoint gives view-local, then flip to top-left.
                let wpt: cocoa::foundation::NSPoint = msg_send![event, locationInWindow];
                let lpt: cocoa::foundation::NSPoint =
                    msg_send![this, convertPoint: wpt fromView: nil];
                let frame: cocoa::foundation::NSRect = msg_send![this, frame];
                let click = PendingClick {
                    x: lpt.x,
                    y: frame.size.height - lpt.y,
                    down,
                };
                if let Ok(mut q) = PENDING_CLICKS.lock() {
                    q.push(click);
                }
            }
        }
        extern "C" fn mouse_down(this: &Object, _sel: Sel, event: id) {
            record(this, event, true);
        }
        extern "C" fn mouse_up(this: &Object, _sel: Sel, event: id) {
            record(this, event, false);
        }
        extern "C" fn accepts_first_responder(_this: &Object, _sel: Sel) -> bool {
            // Without this, makeFirstResponder: refuses the view and macOS
            // keeps routing keys to whoever held focus before — observed
            // live 2026-08-07: click focused a page textarea (engine-side)
            // while typed characters went to the chrome URL bar, because
            // ENGINE focus and APPKIT first-responder are different systems
            // and only one was wired.
            true
        }
        extern "C" fn key_down(this: &Object, _sel: Sel, event: id) {
            unsafe {
                let chars: id = msg_send![event, characters];
                let text = if chars != nil {
                    let utf8: *const std::os::raw::c_char = msg_send![chars, UTF8String];
                    if utf8.is_null() {
                        String::new()
                    } else {
                        std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
                    }
                } else {
                    String::new()
                };
                let keycode: u16 = msg_send![event, keyCode];
                let flags: u64 = msg_send![event, modifierFlags];
                let key = PendingKey {
                    text,
                    mac_keycode: keycode,
                    ctrl: flags & (1 << 18) != 0,   // NSEventModifierFlagControl
                    cmd: flags & (1 << 20) != 0,    // NSEventModifierFlagCommand
                    shift: flags & (1 << 17) != 0,  // NSEventModifierFlagShift
                    alt: flags & (1 << 19) != 0,    // NSEventModifierFlagOption
                };
                if let Ok(mut q) = PENDING_KEYS.lock() {
                    q.push(key);
                }
            }
            let _ = this;
            // Deliberately NOT calling super: consuming here is what keeps a
            // keystroke from ALSO reaching whatever else might interpret it.
            // Cmd-shortcuts still work: the menu system sees key equivalents
            // before the responder chain does.
        }
        extern "C" fn accepts_first_mouse(_this: &Object, _sel: Sel, _event: id) -> bool {
            // A click on an inactive window should reach the page (this is
            // what browsers do for links), and the synthetic probe runs
            // before the window is ever key.
            true
        }
        unsafe {
            decl.add_method(
                sel!(acceptsFirstMouse:),
                accepts_first_mouse as extern "C" fn(&Object, Sel, id) -> bool,
            );
            decl.add_method(
                sel!(acceptsFirstResponder),
                accepts_first_responder as extern "C" fn(&Object, Sel) -> bool,
            );
            decl.add_method(sel!(keyDown:), key_down as extern "C" fn(&Object, Sel, id));
            decl.add_method(
                sel!(mouseDown:),
                mouse_down as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(sel!(mouseUp:), mouse_up as extern "C" fn(&Object, Sel, id));
        }
        decl.register();
    });
    Class::get("RustKitContentView").expect("RustKitContentView registered")
}

#[cfg(target_os = "macos")]
impl MacOSViewHost {
    pub fn new() -> Self {
        Self {
            views: RwLock::new(HashMap::new()),
        }
    }

    /// Convert top-left origin bounds to Cocoa's bottom-left origin coordinate system.
    ///
    /// HiWave/Wry uses top-left origin (y=0 at top, increasing downward).
    /// Cocoa uses bottom-left origin (y=0 at bottom, increasing upward).
    ///
    /// Formula: y_cocoa = parent_height - bounds.y - bounds.height
    fn convert_to_cocoa_frame(bounds: Bounds, parent_height: f64) -> cocoa::foundation::NSRect {
        let y_cocoa = parent_height - bounds.y as f64 - bounds.height as f64;
        cocoa::foundation::NSRect::new(
            cocoa::foundation::NSPoint::new(bounds.x as f64, y_cocoa),
            cocoa::foundation::NSSize::new(bounds.width as f64, bounds.height as f64),
        )
    }

    /// Create a view from a TAO window handle
    pub fn create_view_from_window(
        &self,
        window_handle: RawWindowHandle,
        bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        let view_id = ViewId::new();
        debug!(?view_id, ?bounds, "Creating macOS view");

        // Extract NSWindow from raw window handle
        // In raw-window-handle 0.6, AppKitHandle contains ns_view, not ns_window
        // We need to get the window from the view
        let ns_view = match window_handle {
            RawWindowHandle::AppKit(handle) => {
                handle.ns_view.as_ptr() as id
            }
            _ => {
                return Err(ViewHostError::InvalidParent);
            }
        };
        
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

        // Get the content view's frame to get parent height for coordinate conversion
        let parent_frame: cocoa::foundation::NSRect = unsafe { msg_send![content_view, frame] };
        let parent_height = parent_frame.size.height;

        debug!(parent_height, "Got parent content view height");

        // Create a new NSView for our content
        // Convert from top-left origin (HiWave/Wry) to bottom-left origin (Cocoa)
        let frame = Self::convert_to_cocoa_frame(bounds, parent_height);
        debug!(?bounds, cocoa_y = frame.origin.y, "Converted bounds to Cocoa coordinates");

        let view: id = unsafe {
            let view_class = rustkit_content_view_class();
            let view: id = msg_send![view_class, alloc];
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
            debug!(?view_id, "Added RustKit view as subview");
        }

        // Get DPI (backing scale factor)
        let dpi = unsafe {
            let scale: f64 = msg_send![ns_window, backingScaleFactor];
            (scale * 96.0) as u32
        };

        let state = Arc::new(Mutex::new(MacOSViewState {
            _id: view_id,
            view,
            bounds,
            dpi,
            visible: true,
            _focused: false,
        }));

        {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in create_view_from_window: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views.insert(view_id, state);
        }

        info!(?view_id, dpi, "macOS view created");
        Ok(view_id)
    }

    /// Get the NSView for a view ID
    pub fn get_view(&self, view_id: ViewId) -> Result<id, ViewHostError> {
        let state_arc = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in get_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
                .clone() // Clone the Arc to extend lifetime
        }; // views lock is released here
        let view = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_view: {}", e);
            ViewHostError::LockPoisoned
        })?.view;
        Ok(view)
    }

    /// Get the raw window handle for a view
    pub fn get_raw_window_handle(&self, view_id: ViewId) -> Result<RawWindowHandle, ViewHostError> {
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
        })?.view;

        // Get the window from the view
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            warn!(?view_id, "View has no window attached");
            return Err(ViewHostError::ViewNotFound(view_id));
        }

        // Verify view state
        unsafe {
            let is_hidden: bool = msg_send![view, isHidden];
            let superview: id = msg_send![view, superview];
            let has_superview = superview != nil;
            let frame: cocoa::foundation::NSRect = msg_send![view, frame];
            info!(
                ?view_id,
                is_hidden,
                has_superview,
                frame_x = frame.origin.x,
                frame_y = frame.origin.y,
                frame_w = frame.size.width,
                frame_h = frame.size.height,
                "Getting raw window handle - view state"
            );
        }

        // Create raw window handle
        // AppKitWindowHandle::new() expects NonNull<c_void>
        use std::ptr::NonNull;
        let handle = RawWindowHandle::AppKit(
            raw_window_handle::AppKitWindowHandle::new(
                NonNull::new(view as *mut std::ffi::c_void)
                    .expect("View pointer is null")
            )
        );

        Ok(handle)
    }

    /// Set view bounds
    pub fn set_bounds(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in set_bounds: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Record the new bounds under the lock, then release it before any
        // AppKit call: `setFrame:` runs layout callbacks synchronously on a
        // subclassed view. See `focus` for the full rationale.
        let view: id = {
            let mut guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in set_bounds: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.bounds = bounds;
            guard.view
        };

        unsafe {
            // Get the superview to determine parent height for coordinate conversion
            let superview: id = msg_send![view, superview];
            let parent_height = if superview != nil {
                let parent_frame: cocoa::foundation::NSRect = msg_send![superview, frame];
                parent_frame.size.height
            } else {
                // Fallback: try to get window content view height
                let window: id = msg_send![view, window];
                if window != nil {
                    let content_view: id = msg_send![window, contentView];
                    if content_view != nil {
                        let content_frame: cocoa::foundation::NSRect = msg_send![content_view, frame];
                        content_frame.size.height
                    } else {
                        bounds.height as f64 + bounds.y as f64 // Fallback
                    }
                } else {
                    bounds.height as f64 + bounds.y as f64 // Fallback
                }
            };

            // Convert from top-left origin to Cocoa's bottom-left origin
            let frame = Self::convert_to_cocoa_frame(bounds, parent_height);
            let _: () = msg_send![view, setFrame: frame];
        }

        debug!(?view_id, ?bounds, "View bounds updated");
        Ok(())
    }

    /// Get view bounds
    pub fn get_bounds(&self, view_id: ViewId) -> Result<Bounds, ViewHostError> {
        let state_arc = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in get_bounds: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
                .clone() // Clone the Arc to extend lifetime
        }; // views lock is released here
        let bounds = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_bounds: {}", e);
            ViewHostError::LockPoisoned
        })?.bounds;
        Ok(bounds)
    }

    /// Set view visibility
    pub fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in set_visible: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Mutate state under the lock, then release before AppKit:
        // `setHidden:` runs viewDidHide/viewDidUnhide synchronously on a
        // subclassed view. See `focus` for the full rationale.
        let view: id = {
            let mut guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in set_visible: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.visible = visible;
            guard.view
        };

        unsafe {
            let hidden: bool = !visible;
            let _: () = msg_send![view, setHidden: hidden];
        }

        debug!(?view_id, visible, "View visibility changed");
        Ok(())
    }

    /// Focus a view
    ///
    /// COPY THE VIEW POINTER, DROP EVERY LOCK, *THEN* CALL APPKIT.
    ///
    /// `makeFirstResponder:` is synchronous and re-enters the responder
    /// chain — `resignFirstResponder` / `becomeFirstResponder` and any focus
    /// notification run before it returns. The moment this NSView gains
    /// responder overrides that call back into the ViewHost (the next unit:
    /// content keyboard input), holding the per-view `Mutex` across that call
    /// would deadlock the process forever on a non-reentrant lock, with no
    /// error and no log past this line.
    ///
    /// Athena hit exactly this on Windows (`SetFocus` dispatching
    /// `WM_SETFOCUS` into our own wnd_proc, hiwave-windows#85): every focus
    /// call hung the process from the day it was written, and nothing found
    /// it because nothing ever called it. An unused API is not a working API,
    /// it is an untested one — this method has zero callers here too.
    pub fn focus(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let view: id = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in focus: {}", e);
                ViewHostError::LockPoisoned
            })?;
            let state = views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?;
            let guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in focus: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.view
        }; // both guards released here, before any AppKit call

        unsafe {
            let window: id = msg_send![view, window];
            if window != nil {
                let _: () = msg_send![window, makeFirstResponder: view];
            }
        }

        debug!(?view_id, "View focused");
        Ok(())
    }

    /// Get DPI for a view
    pub fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError> {
        let state_arc = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in get_dpi: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
                .clone() // Clone the Arc to extend lifetime
        }; // views lock is released here
        let dpi = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_dpi: {}", e);
            ViewHostError::LockPoisoned
        })?.dpi;
        Ok(dpi)
    }

    /// Destroy a view
    pub fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let state_arc = {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in destroy_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .remove(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
        }; // views lock is released here

        let view = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in destroy_view: {}", e);
            ViewHostError::LockPoisoned
        })?.view;

        unsafe {
            let _: () = msg_send![view, removeFromSuperview];
        }

        debug!(?view_id, "View destroyed");
        Ok(())
    }

    /// Pump macOS event loop (stub for now)
    pub fn pump_messages(&self) -> bool {
        // TODO: Implement proper event loop pumping
        // For now, this is a no-op as TAO handles the event loop
        true
    }
}

#[cfg(not(target_os = "macos"))]
pub struct MacOSViewHost;

#[cfg(not(target_os = "macos"))]
impl MacOSViewHost {
    pub fn new() -> Self {
        Self
    }
}

