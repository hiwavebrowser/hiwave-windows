//! CH2.E35: a destroyed host view leaves its window. `destroy_view` (and a
//! dropped `ViewHost`) forgot the view but left the NSView in the window's
//! content view, still drawing its last frame: each closed tab left one.
//! Creates and destroys 50 views and counts the content view's subviews.
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
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0)),
            NSWindowStyleMask::NSTitledWindowMask,
            NSBackingStoreType::NSBackingStoreBuffered,
            NO,
        );
        let content_view: id = msg_send![window, contentView];
        let parent = RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::new(content_view as *mut std::ffi::c_void).expect("content view"),
        ));
        let subviews = || -> usize {
            let list: id = msg_send![content_view, subviews];
            msg_send![list, count]
        };
        let baseline = subviews();

        // destroy_view, 50 times.
        let host = ViewHost::new();
        for _ in 0..50 {
            let pool = NSAutoreleasePool::new(nil);
            let view = host.create_view(parent, Bounds::new(0, 0, 800, 600)).expect("create_view");
            host.destroy_view(view).expect("destroy_view");
            assert_eq!(host.raw_view(view), 0, "the view host still knows the destroyed view");
            let _: () = msg_send![pool, drain];
        }
        assert_eq!(host.view_count(), 0);
        let after_destroy = subviews();
        println!("destroy_view x50: subviews {baseline} -> {after_destroy}");
        assert_eq!(
            after_destroy, baseline,
            "50 destroyed views are still in the window ({} left)",
            after_destroy - baseline
        );

        // A dropped ViewHost takes its views with it.
        let host = ViewHost::new();
        for _ in 0..50 {
            host.create_view(parent, Bounds::new(0, 0, 800, 600)).expect("create_view");
        }
        assert_eq!(subviews(), baseline + 50);
        drop(host);
        let after_drop = subviews();
        println!("ViewHost drop with 50 views: subviews {} -> {after_drop}", baseline + 50);
        assert_eq!(after_drop, baseline, "a dropped view host left {} views in the window", after_drop - baseline);
        println!("ok: destroyed and dropped host views leave the window");
    }
}
