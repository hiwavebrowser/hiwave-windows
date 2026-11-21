//! `ViewHost::set_bounds` must move and size the content NSView (Z lane I0,
//! 2026-10-03).
//!
//! Pete's live testing: a bigger window made the page smaller and a small
//! window made the text huge. The engine resized its drawable and laid out
//! at the new size, but the NSView it draws into kept its first frame, so
//! the new drawable was stretched into the old rectangle.
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
    use rustkit_viewhost::{Bounds, ViewHost};
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
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 800.0)),
            NSWindowStyleMask::NSTitledWindowMask | NSWindowStyleMask::NSResizableWindowMask,
            NSBackingStoreType::NSBackingStoreBuffered,
            NO,
        );
        let content_view: id = msg_send![window, contentView];
        let parent = RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::new(content_view as *mut std::ffi::c_void).expect("content view"),
        ));

        // The NSView's frame as (x, y, width, height), in Cocoa's
        // bottom-left coordinates.
        let frame = || -> (f64, f64, f64, f64) {
            let subviews: id = msg_send![content_view, subviews];
            let view: id = msg_send![subviews, lastObject];
            let f: NSRect = msg_send![view, frame];
            (f.origin.x, f.origin.y, f.size.width, f.size.height)
        };

        // Content below an 80px chrome strip, as hiwave-app lays it out.
        let host = ViewHost::new();
        let view = host
            .create_view(parent, Bounds::new(0, 80, 1280, 720))
            .expect("create_view");
        assert_eq!(frame(), (0.0, 0.0, 1280.0, 720.0), "created");

        // The window grows; the app gives the content the new size.
        window.setContentSize_(NSSize::new(1600.0, 1000.0));
        host.set_bounds(view, Bounds::new(0, 80, 1600, 920)).expect("set_bounds");
        assert_eq!(frame(), (0.0, 0.0, 1600.0, 920.0), "window grown");

        // A 240px sidebar opens and a 20px shelf appears: the view moves.
        host.set_bounds(view, Bounds::new(240, 80, 1360, 900)).expect("set_bounds");
        assert_eq!(frame(), (240.0, 20.0, 1360.0, 900.0), "sidebar and shelf");

        // The window shrinks.
        window.setContentSize_(NSSize::new(800.0, 500.0));
        host.set_bounds(view, Bounds::new(0, 80, 800, 420)).expect("set_bounds");
        assert_eq!(frame(), (0.0, 0.0, 800.0, 420.0), "window shrunk");

        assert_eq!(host.get_bounds(view).expect("bounds"), Bounds::new(0, 80, 800, 420));
        println!("ok: set_bounds moves and sizes the content NSView");
    }
}
