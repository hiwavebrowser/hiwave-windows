//! CH3.A3c (the alpha route): a transparent view stacked over the page
//! keeps its region for `hitTest:` but is not masked by it, so its pixels
//! (a popup's shadow, a rounded corner, a dimming overlay) decide what
//! covers the view below; its layer is not opaque. A view that is not
//! transparent keeps #909/#925's mask.
//!
//! Runs without the test harness: AppKit only makes windows on the main
//! thread. The window is never shown.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use cocoa::appkit::{
        NSApp, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow,
        NSWindowStyleMask,
    };
    use cocoa::base::{id, nil, NO};
    use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};
    use rustkit_viewhost::{set_raw_view_region, set_raw_view_transparent, Bounds, RegionRect, ViewHost};
    use std::ptr::NonNull;

    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let screens: id = msg_send![class!(NSScreen), screens];
        let screen_count: usize = if screens == nil { 0 } else { msg_send![screens, count] };
        if screen_count == 0 {
            println!("skipped: no display in this session");
            return;
        }
        let app = NSApp();
        app.setActivationPolicy_(NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory);
        let window = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0)),
            NSWindowStyleMask::NSTitledWindowMask,
            NSBackingStoreType::NSBackingStoreBuffered,
            NO,
        );
        let content_view: id = msg_send![window, contentView];
        let parent = RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::new(content_view as *mut std::ffi::c_void).expect("content view"),
        ));
        let host = ViewHost::new();
        // The chrome: the whole window; the page column is x 0..800, y 100..600.
        let region = vec![RegionRect::new(0.0, 0.0, 800.0, 100.0), RegionRect::new(300.0, 100.0, 200.0, 80.0)];
        let make = || {
            let id = host.create_view(parent, Bounds::new(0, 0, 800, 600)).expect("create_view");
            host.raw_view(id)
        };
        // Window points (bottom-left origin) for view points (top-left).
        let hit = |x: f64, y: f64| -> id { msg_send![content_view, hitTest: NSPoint::new(x, 600.0 - y)] };

        let masked = make();
        set_raw_view_region(masked, Some(region.clone()));
        let layer: id = msg_send![masked as id, layer];
        let mask: id = msg_send![layer, mask];
        assert!(mask != nil, "an opaque view's region is a layer mask (#909)");
        let _: () = msg_send![masked as id, removeFromSuperview];

        let chrome = make();
        set_raw_view_transparent(chrome, true);
        set_raw_view_region(chrome, Some(region.clone()));
        let layer: id = msg_send![chrome as id, layer];
        let mask: id = msg_send![layer, mask];
        assert!(mask == nil, "a transparent view is not masked: its pixels decide");
        let opaque: bool = msg_send![layer, isOpaque];
        assert!(!opaque, "a transparent view's layer is not opaque");
        assert_eq!(hit(400.0, 50.0), chrome as id, "the toolbar strip is the chrome's");
        assert_eq!(hit(400.0, 150.0), chrome as id, "a popup over the page is the chrome's");
        assert_eq!(hit(100.0, 400.0), content_view, "the page column goes on to the view below");

        // Back to opaque: the mask returns with the next region.
        set_raw_view_transparent(chrome, false);
        set_raw_view_region(chrome, Some(region));
        let mask: id = msg_send![layer, mask];
        assert!(mask != nil, "opaque again: masked again");
        println!("ok: a transparent view keeps its hit region and drops the mask");
    }
}
