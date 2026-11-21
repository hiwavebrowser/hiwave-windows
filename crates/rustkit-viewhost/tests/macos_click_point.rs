//! A click in the content NSView must reach the app in the view's own
//! top-left coordinates, at any view size and position (Z lane I0,
//! 2026-10-03).
//!
//! Pete's live testing: plain link clicks failed. The app hit-tests the
//! page at the point the view reports, so that point has to be the page's
//! viewport point: unscaled, and measured from the view's top-left corner
//! wherever the sidebar, the shelf or a window resize has put the view.
//!
//! The events are real `NSEvent`s, routed by AppKit's own hit test to the
//! view that would get a hardware click there. Runs without the test
//! harness: AppKit only makes windows on the main thread. The window is
//! never shown.

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
    use rustkit_viewhost::{drain_pending_clicks, Bounds, ViewHost};
    use std::ptr::NonNull;

    const LEFT_MOUSE_DOWN: u64 = 1;
    const LEFT_MOUSE_UP: u64 = 2;

    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let screens: id = msg_send![class!(NSScreen), screens];
        let screen_count: usize = if screens == nil {
            0
        } else {
            msg_send![screens, count]
        };
        if screen_count == 0 {
            println!("skipped: no display in this session");
            return;
        }
        let app = NSApp();
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
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
        let window_number: isize = msg_send![window, windowNumber];

        // Press and release at a point given the way the app thinks of it:
        // from the top-left corner of the window's content area. Returns what
        // the content view queued, as (x, y, down).
        let click = |x: f64, y: f64, content_height: f64| -> Vec<(f64, f64, bool)> {
            for kind in [LEFT_MOUSE_DOWN, LEFT_MOUSE_UP] {
                let event: id = msg_send![class!(NSEvent),
                    mouseEventWithType: kind
                    location: NSPoint::new(x, content_height - y)
                    modifierFlags: 0u64
                    timestamp: 0.0f64
                    windowNumber: window_number
                    context: nil
                    eventNumber: 0isize
                    clickCount: 1isize
                    pressure: 1.0f32];
                // `-[NSWindow sendEvent:]` drops mouse events for a window
                // that is not on screen, so do its two steps here: AppKit's
                // hit test picks the view, and the view is handed the event.
                let frame_view: id = msg_send![content_view, superview];
                let hit: id = msg_send![frame_view, hitTest: NSPoint::new(x, content_height - y)];
                if hit != nil && kind == LEFT_MOUSE_DOWN {
                    let _: () = msg_send![hit, mouseDown: event];
                } else if hit != nil {
                    let _: () = msg_send![hit, mouseUp: event];
                }
            }
            drain_pending_clicks()
                .iter()
                .map(|c| (c.x, c.y, c.down))
                .collect()
        };

        // Content below an 80px chrome strip, as hiwave-app lays it out.
        let host = ViewHost::new();
        let view = host
            .create_view(parent, Bounds::new(0, 80, 1280, 720))
            .expect("create_view");
        assert_eq!(
            click(10.0, 80.0 + 15.0, 800.0),
            vec![(10.0, 15.0, true), (10.0, 15.0, false)],
            "created: 10,15 inside the view"
        );
        assert_eq!(
            click(10.0, 40.0, 800.0),
            vec![],
            "a click on the chrome strip is not the page's"
        );

        // The window grows and the app gives the content the new size: a
        // point near the new bottom-right corner is that point, not a scaled
        // one.
        window.setContentSize_(NSSize::new(1600.0, 1000.0));
        host.set_bounds(view, Bounds::new(0, 80, 1600, 920))
            .expect("set_bounds");
        assert_eq!(
            click(1500.0, 80.0 + 900.0, 1000.0),
            vec![(1500.0, 900.0, true), (1500.0, 900.0, false)],
            "window grown"
        );

        // A 240px sidebar opens and a 20px shelf appears: the view moves, and
        // its top-left corner is still the page's 0,0.
        host.set_bounds(view, Bounds::new(240, 80, 1360, 900))
            .expect("set_bounds");
        assert_eq!(
            click(240.0 + 30.0, 80.0 + 50.0, 1000.0),
            vec![(30.0, 50.0, true), (30.0, 50.0, false)],
            "sidebar and shelf"
        );
        assert_eq!(
            click(100.0, 80.0 + 50.0, 1000.0),
            vec![],
            "a click on the sidebar is not the page's"
        );

        // The window shrinks. (Not a point in the last few pixels of the
        // corner: a resizable window's own resize area wins the hit test there.)
        window.setContentSize_(NSSize::new(800.0, 500.0));
        host.set_bounds(view, Bounds::new(0, 80, 800, 420))
            .expect("set_bounds");
        assert_eq!(
            click(700.0, 80.0 + 400.0, 500.0),
            vec![(700.0, 400.0, true), (700.0, 400.0, false)],
            "window shrunk"
        );

        println!("ok: content clicks arrive in view-local top-left coordinates at every size");
    }
}
