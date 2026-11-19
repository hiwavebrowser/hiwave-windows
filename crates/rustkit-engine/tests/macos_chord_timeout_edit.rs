//! M1e (#1015 R1 nit 1): a Cmd+V / Cmd+Z / Cmd+X / Cmd+C whose wait for
//! the page ran out edits once, not twice, and (M1g) the page does not
//! hear its keydown: the menu is the chord's one actor.
//!
//! With the keyboard in a text field, Cmd+V goes to the page first
//! (`performKeyEquivalent:`); the page's keydown runs and, uncancelled, the
//! field pastes and the page answers "claimed". M1e bounds the wait for
//! that answer at 100 ms: a page whose thread is held gets the chord taken
//! to the menu instead. The menu's Paste item then sends `paste:` to the
//! view, which queues an edit command for the page. When the page's thread
//! comes free it finds both: the chord's keydown, then the menu's paste.
//! The chord's keydown is not dispatched (M1g, #1015 delta R1 5835356948:
//! exactly one actor; a page that heard it would act on a key the menu also
//! took, e.g. in a tab the menu just left), and its editing default does not
//! run: the menu has the chord, and its edit command is the one edit.
//!
//! This walks that path with the real pieces: an NSWindow, the engine's
//! content view, a main menu with Paste and Undo, the view's input queue
//! and the page protocol (`PageInput::CommandKey`, `PageInput::Edit`) into
//! a real engine. The "held page thread" is this test not draining the
//! queue until the wait has run out. The menu items target the view
//! directly: the window is never shown, so it is not key and a nil-target
//! action would have no responder chain to walk. The clipboard is the
//! engine's in-memory one.
//!
//! Runs without the test harness: AppKit only makes windows on the main
//! thread. Needs a display and a GPU.

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
    use rustkit_engine::{
        connect, EditCommand, Engine, EngineConfig, PageCmd, PageEndpoint, PageEvent, PageHandle, PageInput,
    };
    use rustkit_viewhost::{drain_input, finish_key_equivalent, Bounds, EditAction, InputKind};
    use std::ptr::NonNull;
    use std::time::{Duration, Instant};

    const CMD: u64 = 1 << 20;
    const CHROME: f64 = 80.0;

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
            NSRect::new(NSPoint::new(100.0, 100.0), NSSize::new(1280.0, 800.0)),
            NSWindowStyleMask::NSTitledWindowMask | NSWindowStyleMask::NSResizableWindowMask,
            NSBackingStoreType::NSBackingStoreBuffered,
            NO,
        );
        let content_view: id = msg_send![window, contentView];
        let parent = RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::new(content_view as *mut std::ffi::c_void).expect("content view"),
        ));

        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_view(parent, Bounds::new(0, CHROME as i32, 1280, 720))
            .expect("create_view");
        engine
            .load_html(
                view,
                r#"<html><body style="margin:0"><input id="q" value="hello">
                <script>window.keys = [];
                document.addEventListener('keydown', function (e) { if (e.metaKey) keys.push(e.key); });</script>
                </body></html>"#,
            )
            .expect("load_html");
        engine
            .execute_script(view, "var q = document.getElementById('q'); q.focus(); q.setSelectionRange(99, 99); 0")
            .expect("focus");
        assert!(engine.focused_node(view).is_some(), "the field has focus");
        engine.clipboard().write_text("XY");
        let clipboard = engine.clipboard();

        let frame_view: id = msg_send![content_view, superview];
        let page: id = msg_send![frame_view, hitTest: NSPoint::new(10.0, 800.0 - 95.0)];
        assert!(page != nil, "the content view is under the point");
        let _: bool = msg_send![window, makeFirstResponder: page];
        let responder: id = msg_send![window, firstResponder];
        assert!(responder == page, "the content view has the keyboard");

        // The menu: Edit > Paste (Cmd+V), Undo (Cmd+Z), Cut (Cmd+X) and
        // Copy (Cmd+C), to the view.
        let string = |s: &str| -> id { NSString::alloc(nil).init_str(s) };
        let menu: id = msg_send![class!(NSMenu), alloc];
        let menu: id = msg_send![menu, initWithTitle: string("Main")];
        let sub: id = msg_send![class!(NSMenu), alloc];
        let sub: id = msg_send![sub, initWithTitle: string("Edit")];
        for (title, action, key) in [
            ("Paste", sel!(paste:), "v"),
            ("Undo", sel!(undo:), "z"),
            ("Cut", sel!(cut:), "x"),
            ("Copy", sel!(copy:), "c"),
        ] {
            let item: id = msg_send![class!(NSMenuItem), alloc];
            let item: id = msg_send![item, initWithTitle: string(title) action: action keyEquivalent: string(key)];
            let _: () = msg_send![item, setTarget: page];
            let _: () = msg_send![sub, addItem: item];
        }
        let top: id = msg_send![class!(NSMenuItem), alloc];
        let top: id = msg_send![top, initWithTitle: string("Edit") action: nil keyEquivalent: string("")];
        let _: () = msg_send![top, setSubmenu: sub];
        let _: () = msg_send![menu, addItem: top];
        let _: () = msg_send![app, setMainMenu: menu];

        let (handle, mut endpoint) = connect(engine, view);

        // A Cmd chord as the window server stamps it, offered to the key view.
        let press = |keycode: u16, ch: &str| {
            let info: id = msg_send![class!(NSProcessInfo), processInfo];
            let uptime: f64 = msg_send![info, systemUptime];
            let event: id = msg_send![class!(NSEvent),
                keyEventWithType: 10u64 // NSEventTypeKeyDown
                location: NSPoint::new(0.0, 0.0)
                modifierFlags: CMD
                timestamp: uptime
                windowNumber: 0isize
                context: nil
                characters: string(ch)
                charactersIgnoringModifiers: string(ch)
                isARepeat: NO
                keyCode: keycode];
            let taken: bool = msg_send![page, performKeyEquivalent: event];
            assert!(taken, "the view takes Cmd+{ch} for the page");
        };
        // The main run loop turns for `span`; nothing drains the view's queue.
        let turn = |span: Duration| {
            let until = Instant::now() + span;
            while Instant::now() < until {
                let date: id = msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 0.002f64];
                let rl: id = msg_send![class!(NSRunLoop), currentRunLoop];
                let _: () = msg_send![rl, runUntilDate: date];
            }
        };
        // The app's loop turn, then the page's: the view's queue goes to the
        // page in order, the page applies it, and its chord answers go back
        // to the view. What the queue held.
        let serve = |handle: &PageHandle, endpoint: &mut PageEndpoint, ch: &str| -> Vec<&'static str> {
            let mut seen = Vec::new();
            for event in drain_input() {
                match event.kind {
                    InputKind::KeyEquivalent { token, .. } => {
                        seen.push("chord");
                        handle.send(PageCmd::Input(PageInput::CommandKey {
                            key_code: 0,
                            key: ch.to_string(),
                            shift: false,
                            alt: false,
                            token,
                        }));
                    }
                    InputKind::Edit(action) => {
                        seen.push(match action {
                            EditAction::Paste => "menu paste",
                            EditAction::Undo => "menu undo",
                            EditAction::Cut => "menu cut",
                            EditAction::Copy => "menu copy",
                            _ => "menu other",
                        });
                        handle.send(PageCmd::Input(PageInput::Edit(match action {
                            EditAction::Paste => EditCommand::Paste,
                            EditAction::Undo => EditCommand::Undo,
                            EditAction::Cut => EditCommand::Cut,
                            EditAction::Copy => EditCommand::Copy,
                            _ => panic!("unexpected edit action"),
                        })));
                    }
                    _ => {}
                }
            }
            endpoint.drain(None);
            for event in handle.drain_events() {
                if let PageEvent::KeyEquivalentDone { token, claimed } = event {
                    finish_key_equivalent(token, claimed);
                }
            }
            seen
        };
        let eval = |handle: &PageHandle, endpoint: &mut PageEndpoint, script: &str| -> String {
            let (reply, mut answer) = tokio::sync::oneshot::channel();
            handle.send(PageCmd::Eval { script: script.to_string(), reply });
            endpoint.drain(None);
            let out = answer.try_recv().expect("eval answered").expect("eval ran");
            // A string comes back as `String("...")`.
            out.strip_prefix("String(\"").and_then(|s| s.strip_suffix("\")")).unwrap_or(&out).to_string()
        };
        let value = |handle: &PageHandle, endpoint: &mut PageEndpoint| eval(handle, endpoint, "document.getElementById('q').value");
        let keys = |handle: &PageHandle, endpoint: &mut PageEndpoint| eval(handle, endpoint, "var k = keys.join(','); keys = []; k");

        let mut failures: Vec<String> = Vec::new();
        let mut check = |ok: bool, what: String| {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
            if !ok {
                failures.push(what);
            }
        };
        drain_input();

        // 1. The page answers in time: Cmd+V pastes once, in the page, and
        //    the menu never hears it.
        press(9, "v");
        let seen = serve(&handle, &mut endpoint, "v");
        turn(Duration::from_millis(300));
        let after = serve(&handle, &mut endpoint, "v");
        let v = value(&handle, &mut endpoint);
        check(
            v == "helloXY" && seen == ["chord"] && after.is_empty(),
            format!("Cmd+V answered in time: one paste, by the page: value {v:?}, queue {seen:?} then {after:?}"),
        );
        let heard = keys(&handle, &mut endpoint);
        check(heard == "v", format!("Cmd+V answered in time: the page heard its keydown: {heard:?}"));

        // 2. The page's thread is held past the wait: the chord goes to the
        //    menu, whose Paste reaches the view. The page then finds the
        //    chord and the menu's paste: one paste, and the chord's keydown
        //    is not dispatched (M1g: one actor).
        let before = value(&handle, &mut endpoint);
        press(9, "v");
        turn(Duration::from_millis(300));
        let seen = serve(&handle, &mut endpoint, "v");
        let v = value(&handle, &mut endpoint);
        let heard = keys(&handle, &mut endpoint);
        check(
            seen == ["chord", "menu paste"],
            format!("Cmd+V unanswered for 300 ms: the menu's Paste reached the view after the chord: {seen:?}"),
        );
        check(
            v == format!("{before}XY"),
            format!("Cmd+V unanswered for 300 ms: pasted once: {before:?} -> {v:?}"),
        );
        check(heard.is_empty(), format!("the page does not hear a timed-out chord's keydown: {heard:?}"));

        // 3. The same for undo. Two pastes in time are two undo steps; a
        //    Cmd+Z whose wait ran out takes back one of them, not both.
        let v0 = value(&handle, &mut endpoint);
        press(9, "v");
        serve(&handle, &mut endpoint, "v");
        let v1 = value(&handle, &mut endpoint);
        press(9, "v");
        serve(&handle, &mut endpoint, "v");
        let v2 = value(&handle, &mut endpoint);
        assert!(v1 == format!("{v0}XY") && v2 == format!("{v1}XY"), "two pastes in time: {v0:?} {v1:?} {v2:?}");
        turn(Duration::from_millis(300));
        assert!(serve(&handle, &mut endpoint, "v").is_empty(), "in-time pastes left nothing for the menu");
        let _ = keys(&handle, &mut endpoint);
        press(6, "z");
        turn(Duration::from_millis(300));
        let seen = serve(&handle, &mut endpoint, "z");
        let v = value(&handle, &mut endpoint);
        check(
            seen == ["chord", "menu undo"] && v == v1,
            format!("Cmd+Z unanswered for 300 ms: one step undone: {v2:?} -> {v:?} (want {v1:?}), queue {seen:?}"),
        );

        let heard = keys(&handle, &mut endpoint);
        check(heard.is_empty(), format!("nor a timed-out Cmd+Z's: {heard:?}"));

        // 4. Cut (#1015 delta R1 (c)): the field holds "abcdef" with "cd"
        //    selected. A Cmd+X whose wait ran out is the menu's Cut: "cd" goes
        //    to the clipboard and leaves the field once, the page hears no
        //    keydown, and its late default cuts nothing more.
        eval(&handle, &mut endpoint, "q.value = 'abcdef'; q.setSelectionRange(2, 4); 0");
        let _ = keys(&handle, &mut endpoint);
        press(7, "x");
        turn(Duration::from_millis(300));
        let seen = serve(&handle, &mut endpoint, "x");
        let v = value(&handle, &mut endpoint);
        let clip = clipboard.read_text();
        let heard = keys(&handle, &mut endpoint);
        check(
            seen == ["chord", "menu cut"] && v == "abef" && clip.as_deref() == Some("cd"),
            format!("Cmd+X unanswered for 300 ms: cut once by the menu: value {v:?}, clipboard {clip:?}, queue {seen:?}"),
        );
        check(heard.is_empty(), format!("the page does not hear the timed-out Cmd+X: {heard:?}"));

        // 5. Copy: a Cmd+C whose wait ran out is the menu's Copy; the field is
        //    unchanged and the page hears no keydown.
        eval(&handle, &mut endpoint, "q.value = 'ghijkl'; q.setSelectionRange(1, 3); 0");
        let _ = keys(&handle, &mut endpoint);
        press(8, "c");
        turn(Duration::from_millis(300));
        let seen = serve(&handle, &mut endpoint, "c");
        let v = value(&handle, &mut endpoint);
        let clip = clipboard.read_text();
        let heard = keys(&handle, &mut endpoint);
        check(
            seen == ["chord", "menu copy"] && v == "ghijkl" && clip.as_deref() == Some("hi"),
            format!("Cmd+C unanswered for 300 ms: copied by the menu, field unchanged: value {v:?}, clipboard {clip:?}, queue {seen:?}"),
        );
        check(heard.is_empty(), format!("the page does not hear the timed-out Cmd+C: {heard:?}"));

        assert!(failures.is_empty(), "{} failed: {failures:#?}", failures.len());
        println!("ok: a Cmd edit chord whose wait ran out edits once");
    }
}
