//! A wheel over the content NSView must reach the app (Z lane I0, hand-test
//! item H6, 2026-10-06).
//!
//! Until this test, hiwave-app scrolled the page from tao's
//! `WindowEvent::MouseWheel`, and a test of this name's predecessor
//! (`macos_wheel_reaches_window`) showed the responder chain carrying a wheel
//! from the content view up to tao's view in this nesting. The built app
//! disagreed: a real-window run posted eight wheel events at the content
//! view's centre and the app logged none, while hover, press and keys from
//! the same tool arrived. So the view records the wheel itself, as it
//! records clicks, moves and keys, and the app drains the queue each turn.
//!
//! This builds the app's nesting (a tao window, the content view made by
//! `ViewHost::create_view` from the window's handle) and hands real scroll
//! `NSEvent`s to the view AppKit's own hit test picks. It asserts the queue
//! has the scroll, with the sign and units the app's scroll code speaks, and
//! that the window loop does NOT also hear it (one scroll per event).
//!
//! Runs without the test harness: tao's event loop must own the main
//! thread. The window is never shown, so this does not show AppKit itself
//! routing a wheel to the view under the pointer; it shows what the view
//! does once it has the wheel.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreateScrollWheelEvent(
        source: *const std::ffi::c_void,
        units: u32,
        wheel_count: u32,
        wheel1: i32,
        ...
    ) -> *mut std::ffi::c_void;
}

#[cfg(target_os = "macos")]
fn main() {
    use cocoa::base::{id, nil};
    use cocoa::foundation::NSPoint;
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::HasWindowHandle;
    use rustkit_viewhost::{drain_pending_scrolls, Bounds, ViewHost};
    use std::time::{Duration, Instant};
    use tao::dpi::LogicalSize;
    use tao::event::{Event, MouseScrollDelta, WindowEvent};
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS, WindowExtMacOS};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::window::WindowBuilder;

    /// kCGScrollEventUnitPixel, kCGScrollEventUnitLine
    const UNIT_PIXEL: u32 = 0;
    const UNIT_LINE: u32 = 1;

    let screen_count: usize = unsafe {
        let screens: id = msg_send![class!(NSScreen), screens];
        if screens == nil {
            0
        } else {
            msg_send![screens, count]
        }
    };
    if screen_count == 0 {
        println!("skipped: no display in this session");
        return;
    }

    let mut event_loop = EventLoop::new();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let window = WindowBuilder::new()
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_visible(false)
        .build(&event_loop)
        .expect("tao window");

    // Content below an 80px chrome strip, as hiwave-app lays it out.
    let host = ViewHost::new();
    let parent = window.window_handle().expect("window handle").as_raw();
    let _view = host
        .create_view(parent, Bounds::new(0, 80, 1280, 720))
        .expect("create_view");

    let tao_view = window.ns_view() as id;
    let class_name = |view: id| -> String {
        if view == nil {
            return "nil".to_string();
        }
        unsafe {
            let name: id = msg_send![view, className];
            let utf8: *const std::os::raw::c_char = msg_send![name, UTF8String];
            std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
        }
    };
    // A real scroll NSEvent, as AppKit makes one from the HID event the
    // driver posts (`hwdrive scroll`, pixel units) or a wheel mouse (lines).
    let wheel_event = |units: u32, amount: i32| -> id {
        unsafe {
            let cg = CGEventCreateScrollWheelEvent(std::ptr::null(), units, 1, amount);
            assert!(!cg.is_null(), "a scroll CGEvent was made");
            let wheel: id = msg_send![class!(NSEvent), eventWithCGEvent: cg];
            assert!(wheel != nil, "the CGEvent became an NSEvent");
            wheel
        }
    };

    let mut sent_at: Option<Instant> = None;
    let mut window_loop_heard: Option<MouseScrollDelta> = None;
    let mut hit_class = String::new();
    let mut recorded = Vec::new();
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Poll;
        match event {
            Event::WindowEvent {
                event: WindowEvent::MouseWheel { delta, .. },
                ..
            } => {
                window_loop_heard = Some(delta);
            }
            Event::MainEventsCleared => match sent_at {
                None => unsafe {
                    // The middle of the content area, in the window's
                    // coordinates (origin bottom-left).
                    let frame_view: id = msg_send![tao_view, superview];
                    let hit: id = msg_send![frame_view, hitTest: NSPoint::new(640.0, 360.0)];
                    hit_class = class_name(hit);
                    if hit != nil {
                        // One notch down in pixels, then two more in the
                        // same turn, then a line wheel, then a horizontal
                        // swipe.
                        let _: () = msg_send![hit, scrollWheel: wheel_event(UNIT_PIXEL, -120)];
                        let _: () = msg_send![hit, scrollWheel: wheel_event(UNIT_PIXEL, -100)];
                        let _: () = msg_send![hit, scrollWheel: wheel_event(UNIT_PIXEL, -30)];
                        recorded.push(drain_pending_scrolls());
                        let _: () = msg_send![hit, scrollWheel: wheel_event(UNIT_LINE, -3)];
                        recorded.push(drain_pending_scrolls());
                        let cg = CGEventCreateScrollWheelEvent(std::ptr::null(), UNIT_PIXEL, 2, 0i32, -50i32);
                        let swipe: id = msg_send![class!(NSEvent), eventWithCGEvent: cg];
                        let _: () = msg_send![hit, scrollWheel: swipe];
                        recorded.push(drain_pending_scrolls());
                    }
                    sent_at = Some(Instant::now());
                },
                // Long enough for a MouseWheel the chain might still carry
                // to come through the loop.
                Some(at) if at.elapsed() > Duration::from_millis(500) => {
                    *control_flow = ControlFlow::Exit;
                }
                Some(_) => {}
            },
            _ => {}
        }
    });

    assert_eq!(hit_class, "RustKitContentView", "the content view is under the point");
    assert_eq!(recorded.len(), 3, "three drains");

    // Pixel deltas are summed over a turn; down is negative, as the app's
    // scroll code has always read tao's deltas.
    assert_eq!(recorded[0].len(), 1, "three wheel events in one turn are one scroll: {:?}", recorded[0]);
    assert_eq!((recorded[0][0].dx, recorded[0][0].dy), (0.0, -250.0), "pixels down: {:?}", recorded[0]);

    // A wheel that reports lines is 40px a line, as the window-loop arm counts.
    assert_eq!(recorded[1].len(), 1, "line wheel: {:?}", recorded[1]);
    assert_eq!((recorded[1][0].dx, recorded[1][0].dy), (0.0, -120.0), "three lines down: {:?}", recorded[1]);

    // AppKit's horizontal sign is the inverse of tao's: a swipe whose
    // scrollingDeltaX is -50 is dx = +50 to the app (the page moves right).
    assert_eq!(recorded[2].len(), 1, "swipe: {:?}", recorded[2]);
    assert_eq!((recorded[2][0].dx, recorded[2][0].dy), (50.0, 0.0), "swipe right: {:?}", recorded[2]);

    assert!(
        window_loop_heard.is_none(),
        "the view consumes the wheel; the window loop must not scroll the page a second time, heard {window_loop_heard:?}"
    );
    println!("ok: the content view records the wheel; the window loop does not hear it twice");
}
