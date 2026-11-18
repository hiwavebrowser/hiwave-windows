//! M1e: a Cmd chord offered to the page first waits for the page's answer
//! only so long.
//!
//! With the keyboard in a page, a Cmd chord Chromium does not reserve
//! (Cmd+L, Cmd+F, Cmd+1..9, ...) goes to the page before the menu
//! (`performKeyEquivalent:`, CH2.E3); the app hands the page's answer back
//! (`finish_key_equivalent`). While a load held the page's thread (an image
//! the server held 8 s) Cmd+L reached the menu 6.4 s after the press, and
//! users pressed twice (M1d #1012's real-window measurement). The view now
//! waits `CHORD_ANSWER_TIMEOUT` (100 ms) for the answer; a page that has not
//! answered by then has not claimed the chord, and it goes to the menu. A
//! late answer is dropped: the menu does not run twice, and a page that
//! claims too late does not get it back.
//!
//! A page that answers in time keeps what it claims (Chromium: only
//! `kReservedCommandIds` -- close/new tab and window, reopen tab, tab
//! switching, quit -- skip the page; IDC_FOCUS_LOCATION and
//! IDC_SELECT_TAB_0..7 are not among them), so a page that cancels Cmd+L
//! or Cmd+2 in time keeps it.
//!
//! The page thread here is a fake: a thread that answers after a delay, or
//! never. The main menu's items count what reaches them. Runs without the
//! test harness: AppKit only makes windows on the main thread. The window is
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
    use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
    use objc::declare::ClassDecl;
    use objc::runtime::{Object, Sel};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};
    use rustkit_viewhost::{drain_input, finish_key_equivalent, Bounds, InputKind, ViewHost};
    use std::ptr::NonNull;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    static FOCUS_URL: AtomicUsize = AtomicUsize::new(0);
    static SELECT_TAB_2: AtomicUsize = AtomicUsize::new(0);
    // What the menu ran, in order, with when.
    static RAN: std::sync::Mutex<Vec<(&'static str, Instant)>> = std::sync::Mutex::new(Vec::new());
    extern "C" fn focus_url(_this: &Object, _sel: Sel, _sender: id) {
        FOCUS_URL.fetch_add(1, Ordering::SeqCst);
        RAN.lock().unwrap().push(("l", Instant::now()));
    }
    extern "C" fn select_tab_2(_this: &Object, _sel: Sel, _sender: id) {
        SELECT_TAB_2.fetch_add(1, Ordering::SeqCst);
        RAN.lock().unwrap().push(("2", Instant::now()));
    }

    const CMD: u64 = 1 << 20;
    // The answer bound the view keeps, and the real-window gate's 250 ms.
    const BOUND: Duration = Duration::from_millis(100);
    const GATE: Duration = Duration::from_millis(250);

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

        // The menu: Cmd+L focuses the address bar, Cmd+2 selects tab 2.
        let mut decl = ClassDecl::new("M1eMenuTarget", class!(NSObject)).expect("class");
        decl.add_method(sel!(focusUrl:), focus_url as extern "C" fn(&Object, Sel, id));
        decl.add_method(sel!(selectTab2:), select_tab_2 as extern "C" fn(&Object, Sel, id));
        let target_class = decl.register();
        let target: id = msg_send![target_class, new];
        let string = |s: &str| -> id { NSString::alloc(nil).init_str(s) };
        let menu: id = msg_send![class!(NSMenu), alloc];
        let menu: id = msg_send![menu, initWithTitle: string("Main")];
        let sub: id = msg_send![class!(NSMenu), alloc];
        let sub: id = msg_send![sub, initWithTitle: string("File")];
        for (title, action, key) in [("Open Location", sel!(focusUrl:), "l"), ("Tab 2", sel!(selectTab2:), "2")] {
            let item: id = msg_send![class!(NSMenuItem), alloc];
            let item: id = msg_send![item, initWithTitle: string(title) action: action keyEquivalent: string(key)];
            let _: () = msg_send![item, setTarget: target];
            let _: () = msg_send![sub, addItem: item];
        }
        let top: id = msg_send![class!(NSMenuItem), alloc];
        let top: id = msg_send![top, initWithTitle: string("File") action: nil keyEquivalent: string("")];
        let _: () = msg_send![top, setSubmenu: sub];
        let _: () = msg_send![menu, addItem: top];
        let _: () = msg_send![app, setMainMenu: menu];

        let window = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
            NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(1280.0, 800.0)),
            NSWindowStyleMask::NSTitledWindowMask | NSWindowStyleMask::NSResizableWindowMask,
            NSBackingStoreType::NSBackingStoreBuffered,
            NO,
        );
        let content_view: id = msg_send![window, contentView];
        let parent = RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::new(content_view as *mut std::ffi::c_void).expect("content view"),
        ));
        let host = ViewHost::new();
        let _view_id = host.create_view(parent, Bounds::new(0, 80, 1280, 720)).expect("create_view");
        let frame_view: id = msg_send![content_view, superview];
        let page: id = msg_send![frame_view, hitTest: NSPoint::new(10.0, 800.0 - 95.0)];
        assert!(page != nil, "the content view is under the point");
        let _: bool = msg_send![window, makeFirstResponder: page];
        let responder: id = msg_send![window, firstResponder];
        assert!(responder == page, "the content view has the keyboard");

        let chord = |keycode: u16, ch: &str| -> id {
            msg_send![class!(NSEvent),
                keyEventWithType: 10u64 // NSEventTypeKeyDown
                location: NSPoint::new(0.0, 0.0)
                modifierFlags: CMD
                timestamp: 0.0f64
                windowNumber: 0isize
                context: nil
                characters: string(ch)
                charactersIgnoringModifiers: string(ch)
                isARepeat: NO
                keyCode: keycode]
        };
        // The window's turn: AppKit offers the chord to the key view first.
        let press = |keycode: u16, ch: &str| -> u64 {
            drain_input();
            let taken: bool = msg_send![page, performKeyEquivalent: chord(keycode, ch)];
            assert!(taken, "the view takes the chord for the page");
            let tokens: Vec<u64> = drain_input()
                .into_iter()
                .filter_map(|e| match e.kind {
                    InputKind::KeyEquivalent { token, .. } => Some(token),
                    _ => None,
                })
                .collect();
            assert_eq!(tokens.len(), 1, "one chord to the page: {tokens:?}");
            tokens[0]
        };
        // The main thread's loop for `span`: the run loop turns, and the
        // fake page thread's answers come back as the app hands them over.
        let run = |span: Duration, answers: &mpsc::Receiver<(u64, bool)>| {
            let until = Instant::now() + span;
            while Instant::now() < until {
                let _: () = msg_send![class!(NSRunLoop), currentRunLoop];
                let date: id = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 0.002f64];
                let rl: id = msg_send![class!(NSRunLoop), currentRunLoop];
                let _: () = msg_send![rl, runUntilDate: date];
                while let Ok((token, claimed)) = answers.try_recv() {
                    finish_key_equivalent(token, claimed);
                }
            }
        };
        // Run until `counter` moves (or `limit`); how long it took.
        let run_until_moves = |counter: &AtomicUsize, limit: Duration, answers: &mpsc::Receiver<(u64, bool)>| -> Option<Duration> {
            let start = Instant::now();
            let before = counter.load(Ordering::SeqCst);
            while start.elapsed() < limit {
                run(Duration::from_millis(2), answers);
                if counter.load(Ordering::SeqCst) != before {
                    return Some(start.elapsed());
                }
            }
            None
        };
        // The fake page thread: answers `token` after `after`, or never.
        let page_thread = |token: u64, claimed: bool, after: Option<Duration>| -> mpsc::Receiver<(u64, bool)> {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || match after {
                Some(after) => {
                    std::thread::sleep(after);
                    let _ = tx.send((token, claimed));
                }
                None => {
                    // Never answers: the load holds the thread past the test.
                    std::thread::sleep(Duration::from_secs(5));
                    drop(tx);
                }
            });
            rx
        };
        let count = |c: &AtomicUsize| c.load(Ordering::SeqCst);
        let mut failures: Vec<String> = Vec::new();
        let mut check = |ok: bool, what: String| {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            if !ok {
                failures.push(what);
            }
        };

        // 1. In time, not claimed: the menu runs once, at the answer.
        let before = count(&FOCUS_URL);
        let answers = page_thread(press(37, "l"), false, Some(Duration::from_millis(10)));
        let took = run_until_moves(&FOCUS_URL, Duration::from_secs(1), &answers);
        run(Duration::from_millis(300), &answers);
        check(
            count(&FOCUS_URL) == before + 1 && took.is_some_and(|t| t < BOUND),
            format!("Cmd+L answered unclaimed in 10 ms: the menu ran once, after {took:?} ({} runs)", count(&FOCUS_URL) - before),
        );

        // 2. In time, claimed: the page keeps Cmd+L (not reserved in Chromium).
        let before = count(&FOCUS_URL);
        let answers = page_thread(press(37, "l"), true, Some(Duration::from_millis(10)));
        run(Duration::from_millis(400), &answers);
        check(
            count(&FOCUS_URL) == before,
            format!("Cmd+L claimed in 10 ms: the page keeps it ({} menu runs)", count(&FOCUS_URL) - before),
        );

        // 3. Never answered (the load holds the page's thread): the menu
        //    runs within the bound, once.
        let before = count(&FOCUS_URL);
        let answers = page_thread(press(37, "l"), false, None);
        let took = run_until_moves(&FOCUS_URL, Duration::from_secs(2), &answers);
        run(Duration::from_millis(300), &answers);
        check(
            took.is_some_and(|t| t >= BOUND - Duration::from_millis(5) && t < GATE) && count(&FOCUS_URL) == before + 1,
            format!("Cmd+L never answered: the menu ran after {took:?} (bound {BOUND:?}, gate {GATE:?}), {} runs", count(&FOCUS_URL) - before),
        );

        // 4. Answered unclaimed late (400 ms): the menu ran at the bound;
        //    the late answer does not run it again.
        let before = count(&FOCUS_URL);
        let answers = page_thread(press(37, "l"), false, Some(Duration::from_millis(400)));
        let took = run_until_moves(&FOCUS_URL, Duration::from_secs(2), &answers);
        run(Duration::from_millis(500), &answers);
        check(
            took.is_some_and(|t| t < GATE) && count(&FOCUS_URL) == before + 1,
            format!("Cmd+L answered unclaimed at 400 ms: menu after {took:?}, {} runs (late answer dropped)", count(&FOCUS_URL) - before),
        );

        // 5. Answered claimed late: the menu ran at the bound; the late claim
        //    is dropped (it cannot take the chord back).
        let before = count(&FOCUS_URL);
        let answers = page_thread(press(37, "l"), true, Some(Duration::from_millis(400)));
        let took = run_until_moves(&FOCUS_URL, Duration::from_secs(2), &answers);
        run(Duration::from_millis(500), &answers);
        check(
            took.is_some_and(|t| t < GATE) && count(&FOCUS_URL) == before + 1,
            format!("Cmd+L claimed at 400 ms: menu after {took:?}, {} runs (late claim dropped)", count(&FOCUS_URL) - before),
        );

        // 6. Every page-first chord, not only Cmd+L: Cmd+2 kept in time,
        //    and bounded when the page is held.
        let before = count(&SELECT_TAB_2);
        let answers = page_thread(press(19, "2"), true, Some(Duration::from_millis(20)));
        run(Duration::from_millis(400), &answers);
        check(
            count(&SELECT_TAB_2) == before,
            format!("Cmd+2 claimed in 20 ms: the page keeps it ({} menu runs)", count(&SELECT_TAB_2) - before),
        );
        let before = count(&SELECT_TAB_2);
        let answers = page_thread(press(19, "2"), false, Some(Duration::from_secs(1)));
        let took = run_until_moves(&SELECT_TAB_2, Duration::from_secs(2), &answers);
        run(Duration::from_millis(1100), &answers);
        check(
            took.is_some_and(|t| t < GATE) && count(&SELECT_TAB_2) == before + 1,
            format!("Cmd+2 answered at 1 s: menu after {took:?}, {} runs", count(&SELECT_TAB_2) - before),
        );

        // 7. Two chords held at once each get their own bound and one menu run.
        let before = count(&FOCUS_URL);
        let a = press(37, "l");
        let b = press(37, "l");
        let answers = page_thread(a, false, None);
        let took = run_until_moves(&FOCUS_URL, Duration::from_secs(2), &answers);
        run(Duration::from_millis(300), &answers);
        finish_key_equivalent(b, false);
        finish_key_equivalent(a, true);
        check(
            took.is_some_and(|t| t < GATE) && count(&FOCUS_URL) == before + 2,
            format!("two held Cmd+L: first menu run after {took:?}, {} runs in all", count(&FOCUS_URL) - before),
        );

        // 8. Two quick presses while the page's thread is held (a second
        //    Cmd+2 30 ms after the first, then a Cmd+L): each waits out its
        //    own bound, and the menu runs them in the order pressed, none
        //    before its own bound and each inside the gate (#1015 R1
        //    nit 4). A later one can run some tens of ms past its bound:
        //    its timer waits on the main thread behind the menu run before
        //    it (measured 102 / 129 / 151 ms after the three presses). The
        //    page's answers, when they come, run nothing more.
        RAN.lock().unwrap().clear();
        let gap = Duration::from_millis(30);
        let (_, quiet) = mpsc::channel::<(u64, bool)>();
        let first_at = Instant::now();
        let first = press(19, "2");
        run(gap, &quiet);
        let second_at = Instant::now();
        let second = press(19, "2");
        run(gap, &quiet);
        let third_at = Instant::now();
        let third = press(37, "l");
        run(Duration::from_millis(400), &quiet);
        let ran = RAN.lock().unwrap().clone();
        let names: Vec<&str> = ran.iter().map(|(name, _)| *name).collect();
        let after: Vec<Duration> = ran
            .iter()
            .zip([first_at, second_at, third_at])
            .map(|((_, at), pressed)| at.saturating_duration_since(pressed))
            .collect();
        let slack = Duration::from_millis(5);
        check(
            names == ["2", "2", "l"]
                && after.iter().all(|t| *t >= BOUND - slack && *t < GATE)
                && ran.windows(2).all(|w| w[1].1 >= w[0].1 + gap - slack),
            format!("Cmd+2, Cmd+2, Cmd+L 30 ms apart, page held: the menu ran {names:?} in press order, each {after:?} after its own press"),
        );
        for token in [third, second, first] {
            finish_key_equivalent(token, false);
        }
        finish_key_equivalent(first, true);
        let runs = RAN.lock().unwrap().len();
        check(runs == 3, format!("their late answers run nothing more ({runs} runs)"));

        // 9. The page took the chord (it is running the chord's default)
        //    before the bound, and its answer comes after: the menu does
        //    not run (#1015 R1 nit 1). A chord whose wait ran out is no
        //    longer the page's to take.
        let before = count(&FOCUS_URL);
        let token = press(37, "l");
        run(Duration::from_millis(40), &quiet);
        let took = rustkit_viewhost::claim_key_equivalent(token);
        run(Duration::from_millis(300), &quiet);
        finish_key_equivalent(token, true);
        check(
            took && count(&FOCUS_URL) == before,
            format!("a chord the page took at 40 ms and answered at 340 ms: the page's ({} menu runs)", count(&FOCUS_URL) - before),
        );
        let before = count(&FOCUS_URL);
        let token = press(37, "l");
        run(Duration::from_millis(300), &quiet);
        let took = rustkit_viewhost::claim_key_equivalent(token);
        finish_key_equivalent(token, false);
        check(
            !took && count(&FOCUS_URL) == before + 1,
            format!("a chord whose wait ran out is the menu's, not the page's to take ({} menu runs)", count(&FOCUS_URL) - before),
        );

        assert!(failures.is_empty(), "{} failed: {failures:#?}", failures.len());
        println!("ok: a Cmd chord waits at most {BOUND:?} for the page's answer");
    }
}
