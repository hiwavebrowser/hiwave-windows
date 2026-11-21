//! A key pressed and released in the content NSView must reach the app as
//! a press and a release (Z lane I0, 2026-10-03).
//!
//! The view recorded `keyDown:` only, so the page could never be told a
//! key was released (`keyup`).
//!
//! The events are real `NSEvent`s handed to the view AppKit's own hit test
//! picks, as in `macos_click_point`. Runs without the test harness: AppKit
//! only makes windows on the main thread. The window is never shown.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use cocoa::appkit::{
        NSApp, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSWindow,
        NSWindowStyleMask,
    };
    use cocoa::base::{id, nil, NO};
    use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};
    use rustkit_viewhost::{drain_pending_keys, Bounds, ViewHost};
    use std::ptr::NonNull;

    const KEY_DOWN: u64 = 10;
    const KEY_UP: u64 = 11;
    const SHIFT: u64 = 1 << 17;

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

        let host = ViewHost::new();
        let _view = host
            .create_view(parent, Bounds::new(0, 80, 1280, 720))
            .expect("create_view");
        // The view a hardware click at 10,15 of the page would reach.
        let frame_view: id = msg_send![content_view, superview];
        let page: id = msg_send![frame_view, hitTest: NSPoint::new(10.0, 800.0 - 95.0)];
        assert!(page != nil, "the content view is under the point");

        // Press and release one key in the page's view. Returns what the
        // view queued, as (text, keyCode, up, shift).
        let press = |text: &str, keycode: u16, flags: u64| -> Vec<(String, u16, bool, bool)> {
            for kind in [KEY_DOWN, KEY_UP] {
                let chars = NSString::alloc(nil).init_str(text);
                let event: id = msg_send![class!(NSEvent),
                    keyEventWithType: kind
                    location: NSPoint::new(0.0, 0.0)
                    modifierFlags: flags
                    timestamp: 0.0f64
                    windowNumber: window_number
                    context: nil
                    characters: chars
                    charactersIgnoringModifiers: chars
                    isARepeat: NO
                    keyCode: keycode];
                if kind == KEY_DOWN {
                    let _: () = msg_send![page, keyDown: event];
                } else {
                    let _: () = msg_send![page, keyUp: event];
                }
            }
            drain_pending_keys()
                .iter()
                .map(|k| (k.text.clone(), k.mac_keycode, k.up, k.shift))
                .collect()
        };

        assert_eq!(
            press("j", 38, 0),
            vec![
                ("j".to_string(), 38, false, false),
                ("j".to_string(), 38, true, false)
            ],
            "a character key: press, then release"
        );
        assert_eq!(
            press("?", 44, SHIFT),
            vec![
                ("?".to_string(), 44, false, true),
                ("?".to_string(), 44, true, true)
            ],
            "a shifted character keeps its modifier on both"
        );
        assert_eq!(
            press("\u{1b}", 53, 0),
            vec![
                ("\u{1b}".to_string(), 53, false, false),
                ("\u{1b}".to_string(), 53, true, false)
            ],
            "escape"
        );

        println!("ok: the content view records key presses and releases");
    }
}
