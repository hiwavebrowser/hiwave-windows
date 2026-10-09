//! A click in a real window must reach the link under it, at any window
//! size (Z lane I0, 2026-10-03).
//!
//! Pete's live testing: plain link clicks failed, and a resized window
//! showed the page at the wrong scale. This walks the path a click takes in
//! hiwave-app, without the app: AppKit's hit test picks the content NSView,
//! the view queues the point, and the point goes to `Engine::click_at_point`
//! as the app's loop passes it. The page is a column of full-width links 40px
//! tall, so the link a click lands on says where the engine thinks it fell.
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
    use rustkit_engine::{Engine, EngineConfig};
    use rustkit_viewhost::{drain_pending_clicks, Bounds};
    use std::ptr::NonNull;

    const LEFT_MOUSE_DOWN: u64 = 1;
    const LEFT_MOUSE_UP: u64 = 2;
    /// The chrome strip above the content, as hiwave-app lays it out.
    const CHROME: f64 = 80.0;

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

        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_view(parent, Bounds::new(0, CHROME as i32, 1280, 720))
            .expect("create_view");
        let mut html = String::from(r#"<html><body style="margin:0">"#);
        for row in 0..30 {
            html.push_str(&format!(
                r#"<a id="r{row}" href="https://example.com/row-{row}" style="display:block;height:40px">row {row}</a>"#
            ));
        }
        html.push_str("</body></html>");
        engine.load_html(view, &html).expect("load_html");
        // Row 1 is a script-driven control: its listener keeps the page where
        // it is and counts the click.
        engine
            .execute_script(
                view,
                "window.presses = 0; document.getElementById('r1').addEventListener('click', \
                 function (e) { e.preventDefault(); window.presses++; });",
            )
            .expect("listener");

        // Press and release at a point in the page (the content view's
        // top-left is 0,0), the way a hardware click arrives: AppKit's hit
        // test picks the view and the view is handed the event. (`-[NSWindow
        // sendEvent:]` drops mouse events for a window that is not on
        // screen, so its two steps are done here.) Then the app's loop:
        // drain the view's queue into the engine. Returns where the click
        // would navigate.
        let click = |engine: &mut Engine, x: f64, y: f64, content_height: f64| -> Option<String> {
            let frame_view: id = msg_send![content_view, superview];
            for kind in [LEFT_MOUSE_DOWN, LEFT_MOUSE_UP] {
                let at = NSPoint::new(x, content_height - CHROME - y);
                let event: id = msg_send![class!(NSEvent),
                    mouseEventWithType: kind
                    location: at
                    modifierFlags: 0u64
                    timestamp: 0.0f64
                    windowNumber: window_number
                    context: nil
                    eventNumber: 0isize
                    clickCount: 1isize
                    pressure: 1.0f32];
                let hit: id = msg_send![frame_view, hitTest: at];
                if hit != nil && kind == LEFT_MOUSE_DOWN {
                    let _: () = msg_send![hit, mouseDown: event];
                } else if hit != nil {
                    let _: () = msg_send![hit, mouseUp: event];
                }
            }
            let mut navigate = None;
            for c in drain_pending_clicks() {
                if c.down {
                    engine.mouse_down_at_point(view, c.x as f32, c.y as f32);
                } else {
                    navigate = engine.click_at_point(view, c.x as f32, c.y as f32).navigate;
                }
            }
            navigate
        };
        let row = |n: u32| Some(format!("https://example.com/row-{n}"));

        // As created: 1280x720 below the chrome strip.
        assert_eq!(
            click(&mut engine, 1250.0, 20.0, 800.0),
            row(0),
            "created: right end of row 0"
        );
        assert_eq!(
            click(&mut engine, 10.0, 700.0, 800.0),
            row(17),
            "created: row 17 at the bottom"
        );

        // The script-driven control hears the click and cancels the link.
        assert_eq!(
            click(&mut engine, 10.0, 60.0, 800.0),
            None,
            "row 1's listener cancels the link"
        );
        assert_eq!(
            engine
                .execute_script(view, "window.presses")
                .expect("presses"),
            format!("{:?}", rustkit_js::JsValue::Number(1.0)),
            "row 1's listener ran once"
        );

        // The window grows and the app gives the content the new size. A
        // point that was outside the old view is on the page now, and it is
        // the row at that height, not a scaled one.
        window.setContentSize_(NSSize::new(1600.0, 1000.0));
        engine
            .resize_view(view, Bounds::new(0, CHROME as i32, 1600, 920))
            .expect("resize_view");
        assert_eq!(
            click(&mut engine, 1500.0, 900.0, 1000.0),
            row(22),
            "grown: row 22, past the old view"
        );
        assert_eq!(
            click(&mut engine, 10.0, 20.0, 1000.0),
            row(0),
            "grown: row 0"
        );

        // The window shrinks.
        window.setContentSize_(NSSize::new(800.0, 500.0));
        engine
            .resize_view(view, Bounds::new(0, CHROME as i32, 800, 420))
            .expect("resize_view");
        assert_eq!(
            click(&mut engine, 700.0, 20.0, 500.0),
            row(0),
            "shrunk: row 0"
        );
        assert_eq!(
            click(&mut engine, 700.0, 380.0, 500.0),
            row(9),
            "shrunk: row 9 at the bottom"
        );

        println!("ok: clicks in a real window reach the link under them at every size");
    }
}
