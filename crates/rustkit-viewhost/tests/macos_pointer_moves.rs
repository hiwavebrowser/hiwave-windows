//! The pointer moving over the content NSView, and leaving it, must reach
//! the app (Z lane I0, 2026-10-04).
//!
//! The view recorded presses and releases only, so the page was never told
//! where the mouse was: no `mousemove`, no hover.
//!
//! The events are real `NSEvent`s handed to the view AppKit's own hit test
//! picks, as in `macos_click_point`. Runs without the test harness: AppKit
//! only makes windows on the main thread. The window is never shown, so
//! this does not show AppKit itself sending `mouseMoved:`; it shows the
//! view asks for it (the tracking area) and records it when it comes.

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
    use rustkit_viewhost::{drain_pending_clicks, Bounds, PointerInput, ViewHost};
    use std::ptr::NonNull;

    const LEFT_MOUSE_DOWN: u64 = 1;
    const LEFT_MOUSE_UP: u64 = 2;
    const MOUSE_MOVED: u64 = 5;
    const LEFT_MOUSE_DRAGGED: u64 = 6;
    // NSTrackingAreaOptions
    const TRACK_ENTERED_AND_EXITED: u64 = 0x01;
    const TRACK_MOUSE_MOVED: u64 = 0x02;
    const TRACK_IN_VISIBLE_RECT: u64 = 0x200;

    #[derive(Clone, Copy)]
    enum Send {
        Moved,
        Dragged,
        Exited,
        Down,
        Up,
    }

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

        // Content below an 80px chrome strip, as hiwave-app lays it out.
        let host = ViewHost::new();
        let _view = host
            .create_view(parent, Bounds::new(0, 80, 1280, 720))
            .expect("create_view");
        let frame_view: id = msg_send![content_view, superview];
        let view_at =
            |x: f64, y: f64| -> id { msg_send![frame_view, hitTest: NSPoint::new(x, 800.0 - y)] };

        // The point is given the way the app thinks of it: from the
        // top-left corner of the window's content area.
        let send = |what: Send, x: f64, y: f64| {
            let kind = match what {
                Send::Moved | Send::Exited => MOUSE_MOVED,
                Send::Dragged => LEFT_MOUSE_DRAGGED,
                Send::Down => LEFT_MOUSE_DOWN,
                Send::Up => LEFT_MOUSE_UP,
            };
            let event: id = msg_send![class!(NSEvent),
                mouseEventWithType: kind
                location: NSPoint::new(x, 800.0 - y)
                modifierFlags: 0u64
                timestamp: 0.0f64
                windowNumber: window_number
                context: nil
                eventNumber: 0isize
                clickCount: 1isize
                pressure: 1.0f32];
            let hit = view_at(x, y);
            if hit == nil {
                return;
            }
            // A stock NSView passes these up the responder chain; the
            // content view records them.
            match what {
                Send::Moved => {
                    let _: () = msg_send![hit, mouseMoved: event];
                }
                Send::Dragged => {
                    let _: () = msg_send![hit, mouseDragged: event];
                }
                Send::Exited => {
                    let _: () = msg_send![hit, mouseExited: event];
                }
                Send::Down => {
                    let _: () = msg_send![hit, mouseDown: event];
                }
                Send::Up => {
                    let _: () = msg_send![hit, mouseUp: event];
                }
            }
        };
        let drained = || -> Vec<(PointerInput, f64, f64, bool)> {
            drain_pending_clicks()
                .iter()
                .map(|c| (c.input, c.x, c.y, c.down))
                .collect()
        };

        // The view asks AppKit for moves, entries and exits over its
        // visible rect, whatever size it is given later.
        let page = view_at(10.0, 80.0 + 15.0);
        assert!(page != nil, "the content view is under the point");
        let _: () = msg_send![page, updateTrackingAreas];
        let _: () = msg_send![page, updateTrackingAreas];
        let areas: id = msg_send![page, trackingAreas];
        let count: usize = msg_send![areas, count];
        assert_eq!(count, 1, "one tracking area, however often AppKit asks");
        let area: id = msg_send![areas, objectAtIndex: 0usize];
        let options: u64 = msg_send![area, options];
        for (bit, name) in [
            (TRACK_MOUSE_MOVED, "mouse moved"),
            (TRACK_ENTERED_AND_EXITED, "entered and exited"),
            (TRACK_IN_VISIBLE_RECT, "in visible rect"),
        ] {
            assert!(options & bit != 0, "the tracking area asks for: {name}");
        }
        let owner: id = msg_send![area, owner];
        assert_eq!(owner, page, "the view is the tracking area's owner");

        // Three moves before the loop turns: the page gets where the
        // pointer is, in the view's own top-left coordinates.
        send(Send::Moved, 10.0, 80.0 + 15.0);
        send(Send::Moved, 20.0, 80.0 + 25.0);
        send(Send::Moved, 30.0, 80.0 + 35.0);
        assert_eq!(
            drained(),
            vec![(PointerInput::Move, 30.0, 35.0, false)],
            "moves"
        );

        // A move stays in order with the press and release around it, and
        // a move with the button held is a move.
        send(Send::Moved, 40.0, 80.0 + 40.0);
        send(Send::Down, 40.0, 80.0 + 40.0);
        send(Send::Dragged, 50.0, 80.0 + 60.0);
        send(Send::Dragged, 60.0, 80.0 + 70.0);
        send(Send::Up, 60.0, 80.0 + 70.0);
        send(Send::Exited, 60.0, 80.0 + 70.0);
        assert_eq!(
            drained(),
            vec![
                (PointerInput::Move, 40.0, 40.0, false),
                (PointerInput::Button, 40.0, 40.0, true),
                (PointerInput::Move, 60.0, 70.0, false),
                (PointerInput::Button, 60.0, 70.0, false),
                (PointerInput::Leave, 60.0, 70.0, false),
            ],
            "a drag, then the pointer leaves"
        );

        // A move over the chrome strip is not the page's.
        send(Send::Moved, 10.0, 40.0);
        assert_eq!(drained(), vec![], "chrome strip");

        println!("ok: pointer moves and exits are recorded by the content view");
    }
}
