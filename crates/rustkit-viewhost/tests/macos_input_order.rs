//! Inputs reach the app in arrival order, tagged with the content view they
//! were delivered to, through one queue (threads plan M2, #575).
//!
//! Until 2026-10-14 the content view recorded clicks, keys and the wheel in
//! three separate queues, and the app drained them one after another, so a
//! click made after a wheel burst reached the page before the burst, and a
//! key typed before a click after it. Now every input goes to one queue with
//! a sequence number and the view's id, and `drain_input` hands them out in
//! that order.
//!
//! This builds the app's nesting (a tao window, two content views made by
//! `ViewHost::create_view`) and hands real `NSEvent`s to the view AppKit's
//! own hit test picks: a wheel burst, a click, more wheel, then a wheel on
//! the other view. It asserts the drained order and the view tags, and that
//! destroying a view drops what was queued for it.
//!
//! Runs without the test harness: tao's event loop must own the main
//! thread. The window is never shown.

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
    use rustkit_viewhost::{drain_input, Bounds, InputEvent, InputKind, PointerInput, ViewHost};
    use std::time::{Duration, Instant};
    use tao::dpi::LogicalSize;
    use tao::event::Event;
    use tao::event_loop::{ControlFlow, EventLoop};
    use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS, WindowExtMacOS};
    use tao::platform::run_return::EventLoopExtRunReturn;
    use tao::window::WindowBuilder;

    const UNIT_PIXEL: u32 = 0;
    const LEFT_MOUSE_DOWN: u64 = 1;
    const LEFT_MOUSE_UP: u64 = 2;

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

    // Two content views under an 80px chrome strip: the upper half and the
    // lower half of the content area.
    let host = ViewHost::new();
    let parent = window.window_handle().expect("window handle").as_raw();
    let upper = host.create_view(parent, Bounds::new(0, 80, 1280, 360)).expect("upper view");
    let lower = host.create_view(parent, Bounds::new(0, 440, 1280, 360)).expect("lower view");
    assert_ne!(upper, lower);

    let tao_view = window.ns_view() as id;
    let ns_window = window.ns_window() as id;
    let window_number: isize = unsafe { msg_send![ns_window, windowNumber] };

    let wheel_event = |amount: i32| -> id {
        unsafe {
            let cg = CGEventCreateScrollWheelEvent(std::ptr::null(), UNIT_PIXEL, 1, amount);
            assert!(!cg.is_null(), "a scroll CGEvent was made");
            let wheel: id = msg_send![class!(NSEvent), eventWithCGEvent: cg];
            assert!(wheel != nil, "the CGEvent became an NSEvent");
            wheel
        }
    };
    let mouse_event = |kind: u64, at: NSPoint| -> id {
        unsafe {
            msg_send![class!(NSEvent),
                mouseEventWithType: kind
                location: at
                modifierFlags: 0u64
                timestamp: 0.0f64
                windowNumber: window_number
                context: nil
                eventNumber: 0isize
                clickCount: 1isize
                pressure: 1.0f32]
        }
    };

    // Window coordinates have a bottom-left origin: the middle of the upper
    // view is 800 - (80 + 180), the middle of the lower view 800 - (440 + 180).
    let upper_mid = NSPoint::new(640.0, 540.0);
    let lower_mid = NSPoint::new(640.0, 180.0);

    let mut sent_at: Option<Instant> = None;
    let mut first: Vec<InputEvent> = Vec::new();
    let mut after_destroy: Vec<InputEvent> = Vec::new();
    let mut hit_classes = (String::new(), String::new());
    event_loop.run_return(|event, _, control_flow| {
        *control_flow = ControlFlow::Poll;
        if let Event::MainEventsCleared = event {
            match sent_at {
                None => unsafe {
                    let frame_view: id = msg_send![tao_view, superview];
                    let hit_upper: id = msg_send![frame_view, hitTest: upper_mid];
                    let hit_lower: id = msg_send![frame_view, hitTest: lower_mid];
                    let class_name = |view: id| -> String {
                        let name: id = msg_send![view, className];
                        let utf8: *const std::os::raw::c_char = msg_send![name, UTF8String];
                        std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
                    };
                    hit_classes = (class_name(hit_upper), class_name(hit_lower));
                    assert_ne!(hit_upper, hit_lower, "two different views under the two points");

                    // A wheel burst on the upper view, a click, two more
                    // wheel steps, then a wheel on the lower view.
                    let _: () = msg_send![hit_upper, scrollWheel: wheel_event(-100)];
                    let _: () = msg_send![hit_upper, scrollWheel: wheel_event(-100)];
                    let _: () = msg_send![hit_upper, scrollWheel: wheel_event(-50)];
                    let _: () = msg_send![hit_upper, mouseDown: mouse_event(LEFT_MOUSE_DOWN, upper_mid)];
                    let _: () = msg_send![hit_upper, mouseUp: mouse_event(LEFT_MOUSE_UP, upper_mid)];
                    let _: () = msg_send![hit_upper, scrollWheel: wheel_event(-10)];
                    let _: () = msg_send![hit_upper, scrollWheel: wheel_event(-10)];
                    let _: () = msg_send![hit_lower, scrollWheel: wheel_event(-30)];
                    first = drain_input();

                    // A wheel queued for a view that is then destroyed is
                    // dropped; the other view's input stays.
                    let _: () = msg_send![hit_lower, scrollWheel: wheel_event(-40)];
                    let _: () = msg_send![hit_upper, scrollWheel: wheel_event(-5)];
                    host.destroy_view(lower).expect("destroy the lower view");
                    after_destroy = drain_input();
                    sent_at = Some(Instant::now());
                },
                Some(at) if at.elapsed() > Duration::from_millis(100) => {
                    *control_flow = ControlFlow::Exit;
                }
                Some(_) => {}
            }
        }
    });

    assert_eq!(hit_classes.0, "RustKitContentView", "the upper content view is under its point");
    assert_eq!(hit_classes.1, "RustKitContentView", "the lower content view is under its point");

    let describe = |e: &InputEvent| -> String {
        let view = if e.view == upper { "upper" } else if e.view == lower { "lower" } else { "other" };
        match &e.kind {
            InputKind::Wheel(w) => format!("wheel({view}, dy {})", w.dy),
            InputKind::Pointer(p) => match p.input {
                PointerInput::Button => format!("{}({view})", if p.down { "press" } else { "release" }),
                PointerInput::Move => format!("move({view})"),
                PointerInput::Leave => format!("leave({view})"),
                PointerInput::Secondary => format!("secondary({view})"),
            },
            InputKind::Key(k) => format!("key({view}, {:?})", k.text),
            other => format!("{other:?}({view})"),
        }
    };
    let got: Vec<String> = first.iter().map(describe).collect();
    assert_eq!(
        got,
        [
            "wheel(upper, dy -250)",
            "press(upper)",
            "release(upper)",
            "wheel(upper, dy -20)",
            "wheel(lower, dy -30)",
        ],
        "arrival order, bursts summed only between neighbours, each tagged with its view"
    );
    assert!(first.windows(2).all(|w| w[0].seq < w[1].seq), "sequence numbers rise: {first:?}");

    let got: Vec<String> = after_destroy.iter().map(describe).collect();
    assert_eq!(
        got,
        ["wheel(upper, dy -5)"],
        "the destroyed view's wheel is dropped; the other view's stays"
    );

    println!("ok: inputs drain in arrival order, tagged by view; a destroyed view's input is dropped");
}
