//! CH2.E35 (Athena, from hiwave-windows #124): a dropped `WindowSurface`
//! takes its host view with it. A closed content-thread tab drops its
//! surface; with no teardown its NSView stayed in the window, with its last
//! frame, one per closed tab. The GPU surface goes first, then the host
//! view (Pollux on #124: the swapchain is released before the view under
//! it).
//!
//! Creates and drops 50 surfaces and counts the window's subviews; one
//! view is retained here so its release can be read after the drop.
//!
//! Runs without the test harness: AppKit only makes windows on the main
//! thread. The window is never shown. Needs a display and a GPU.

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
    use rustkit_engine::WindowSurface;
    use rustkit_viewhost::Bounds;
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
        let bounds = Bounds::new(0, 80, 800, 520);

        let mut kept: id = nil;
        for i in 0..50 {
            let pool = NSAutoreleasePool::new(nil);
            let surface = match WindowSurface::attach(parent, bounds) {
                Ok(surface) => surface,
                Err(e) if i == 0 => {
                    println!("skipped: no GPU surface in this session ({e})");
                    return;
                }
                Err(e) => panic!("attach {i}: {e}"),
            };
            let view = surface.raw_view() as id;
            assert!(view != nil, "attach made no host view");
            if i == 0 {
                kept = msg_send![view, retain];
            }
            drop(surface);
            let _: () = msg_send![pool, drain];
        }
        let after = subviews();
        println!("WindowSurface attach+drop x50: subviews {baseline} -> {after}");
        assert_eq!(after, baseline, "50 dropped surfaces left {} host views in the window", after - baseline);

        let superview: id = msg_send![kept, superview];
        assert!(superview == nil, "a dropped surface's view is still in a window");
        // Only this test's retain is left: neither the window, the view
        // host nor the GPU surface still holds the view.
        let retains: usize = msg_send![kept, retainCount];
        println!("the first surface's view after its drop: retainCount {retains}");
        assert_eq!(retains, 1, "something still retains a dropped surface's view");
        let _: () = msg_send![kept, release];
        println!("ok: a dropped WindowSurface takes its host view with it");
    }
}
