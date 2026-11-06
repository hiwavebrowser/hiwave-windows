//! macOS ViewHost implementation using NSView and Cocoa
//!
//! This module provides the macOS-specific implementation of ViewHost,
//! using NSView for rendering surfaces and TAO window handles.

use crate::{Bounds, ViewHostError, ViewId};
use raw_window_handle::RawWindowHandle;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{debug, info, warn};

#[cfg(target_os = "macos")]
use cocoa::{
    base::{id, nil},
};
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};

/// macOS-specific view state
#[cfg(target_os = "macos")]
struct MacOSViewState {
    _id: ViewId,
    view: id, // NSView
    bounds: Bounds,
    dpi: u32,
    visible: bool,
    _focused: bool,
}

/// macOS ViewHost implementation
#[cfg(target_os = "macos")]
pub struct MacOSViewHost {
    views: RwLock<HashMap<ViewId, Arc<Mutex<MacOSViewState>>>>,
}

/// The inputs the content view records go to one ordered, view-tagged
/// queue (`crate::input_queue`, threads plan M2): the app drains it with
/// `drain_input`. The three drains below take ONE kind out and leave the
/// rest queued, for the viewhost's own event tests; the app does not use
/// them, since draining kind by kind is what lost the arrival order (#575).
#[cfg(target_os = "macos")]
use crate::input_queue::{drain_input_kind, push_input, retire_view_input, InputKind};
#[cfg(target_os = "macos")]
use crate::input_queue::{PendingClick, PendingKey, PendingScroll, PointerInput};
#[cfg(target_os = "macos")]
use crate::input_queue::{text_input_state, EditAction, ImeInput};

/// The name of the ivar each `RustKitContentView` carries its [`ViewId`] in.
#[cfg(target_os = "macos")]
pub(crate) const VIEW_ID_IVAR: &str = "rkViewId";

/// The ivar holding the `NSCursor` the view shows (CH2.E4); 0 for the arrow.
#[cfg(target_os = "macos")]
const CURSOR_IVAR: &str = "rkCursor";

#[cfg(target_os = "macos")]
pub fn drain_pending_clicks() -> Vec<PendingClick> {
    drain_input_kind(|k| matches!(k, InputKind::Pointer(_)))
        .into_iter()
        .filter_map(|e| match e.kind {
            InputKind::Pointer(c) => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn drain_pending_keys() -> Vec<PendingKey> {
    drain_input_kind(|k| matches!(k, InputKind::Key(_)))
        .into_iter()
        .filter_map(|e| match e.kind {
            InputKind::Key(k) => Some(k),
            _ => None,
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub fn drain_pending_scrolls() -> Vec<PendingScroll> {
    drain_input_kind(|k| matches!(k, InputKind::Wheel(_)))
        .into_iter()
        .filter_map(|e| match e.kind {
            InputKind::Wheel(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// The [`ViewId`] a content view was created with, from its ivar.
#[cfg(target_os = "macos")]
fn view_id_of(this: &objc::runtime::Object) -> ViewId {
    let raw: u64 = unsafe { *this.get_ivar::<u64>(VIEW_ID_IVAR) };
    ViewId::from_raw(raw)
}

// ── text input (CH2.E3) ────────────────────────────────────────────────────

/// `NSNotFound`.
#[cfg(target_os = "macos")]
const NOT_FOUND: u64 = i64::MAX as u64;

/// `NSRange`, with the encoding the runtime wants for a method that takes
/// or returns one (the cocoa crate's has none).
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RkRange {
    location: u64,
    length: u64,
}

#[cfg(target_os = "macos")]
unsafe impl objc::Encode for RkRange {
    fn encode() -> objc::Encoding {
        unsafe { objc::Encoding::from_str("{_NSRange=QQ}") }
    }
}

/// What the input method did with the key `keyDown:` handed it.
#[cfg(target_os = "macos")]
struct KeyDownContext {
    view: ViewId,
    /// It marked or committed composed text: the key was the input
    /// method's, not the page's.
    composed: bool,
    /// Plain text it inserted with nothing marked: the key's text.
    inserted: Option<String>,
}

#[cfg(target_os = "macos")]
thread_local! {
    static KEY_DOWN: std::cell::RefCell<Option<KeyDownContext>> = const { std::cell::RefCell::new(None) };
    /// Each view's marked text, set the moment the input method marks it:
    /// (start, length) in UTF-16 units of the field's value. The input
    /// method asks `hasMarkedText`/`markedRange` right after it marks, long
    /// before the page could answer.
    static MARKED: std::cell::RefCell<HashMap<ViewId, (usize, usize)>> = std::cell::RefCell::new(HashMap::new());
}

/// Drop `view`'s marked text: its field lost the focus (CH2.E3, #918 R1).
/// Main thread.
#[cfg(target_os = "macos")]
pub(crate) fn forget_marked(view: ViewId) {
    MARKED.with(|m| m.borrow_mut().remove(&view));
}

/// Cmd chords the view took for the page, by token: the retained `NSEvent`
/// that goes on to the menu when the page does not claim it.
#[cfg(target_os = "macos")]
static KEY_EQUIVALENTS: Mutex<Vec<(u64, usize)>> = Mutex::new(Vec::new());
#[cfg(target_os = "macos")]
static NEXT_KEY_EQUIVALENT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A string the input method passed: an `NSString` or an
/// `NSAttributedString` (marked text with its underline).
#[cfg(target_os = "macos")]
unsafe fn text_of(obj: id) -> String {
    if obj == nil {
        return String::new();
    }
    let attributed = objc::runtime::Class::get("NSAttributedString").expect("NSAttributedString");
    let is_attributed: bool = msg_send![obj, isKindOfClass: attributed];
    let string: id = if is_attributed { msg_send![obj, string] } else { obj };
    let utf8: *const std::os::raw::c_char = msg_send![string, UTF8String];
    if utf8.is_null() {
        String::new()
    } else {
        std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
    }
}

/// The Cmd chords Chromium keeps from the page (`IsReservedCommandOrKey`):
/// new/close tab and window, reopen tab, tab switching, quit. Chrome 143
/// on this seat: a page's `preventDefault()` does not stop Cmd+T or Cmd+N,
/// and the page hears no keydown for them; it does hear Cmd+L, Cmd+D and
/// Cmd+K (tools/real_window, `e3_cmd_key_to_page`'s probe).
///
/// `chars` is the event's `charactersIgnoringModifiers`: letters and
/// brackets match by the character the key types, as the menu's key
/// equivalents do, so Cmd+W is the W of a Dvorak or AZERTY layout, not the
/// US W key's position. The arrows, and a key whose character is not
/// ASCII (a Cyrillic or Greek layout, where AppKit matches the menu by the
/// US key), go by `keycode`.
pub fn is_reserved_chord(chars: &str, keycode: u16, cmd: bool, shift: bool, alt: bool) -> bool {
    const Q: u16 = 12;
    const W: u16 = 13;
    const T: u16 = 17;
    const N: u16 = 45;
    const BRACKET_RIGHT: u16 = 30;
    const BRACKET_LEFT: u16 = 33;
    const LEFT: u16 = 123;
    const RIGHT: u16 = 124;
    if !cmd {
        return false;
    }
    if let LEFT | RIGHT = keycode {
        return alt && !shift;
    }
    let mut it = chars.chars();
    let ch = match (it.next(), it.next()) {
        (Some(c), None) if c.is_ascii() => Some(c.to_ascii_lowercase()),
        _ => None,
    };
    let ch = ch.or(match keycode {
        Q => Some('q'),
        W => Some('w'),
        T => Some('t'),
        N => Some('n'),
        BRACKET_LEFT => Some('['),
        BRACKET_RIGHT => Some(']'),
        _ => None,
    });
    match ch {
        Some('q' | 'w' | 't' | 'n') => !alt,
        Some('[' | ']' | '{' | '}') => shift && !alt,
        _ => false,
    }
}

/// The page did (`claimed`) or did not take the Cmd chord `token`
/// (`InputKind::KeyEquivalent`). One it did not goes on to the main menu,
/// Chromium's order: page first, then the menu. Call on the main thread.
#[cfg(target_os = "macos")]
pub fn finish_key_equivalent(token: u64, claimed: bool) {
    let event = {
        let mut held = KEY_EQUIVALENTS.lock().unwrap_or_else(|e| e.into_inner());
        let Some(at) = held.iter().position(|(t, _)| *t == token) else {
            return;
        };
        held.remove(at).1 as id
    };
    unsafe {
        if !claimed {
            let app: id = msg_send![objc::runtime::Class::get("NSApplication").expect("NSApplication"), sharedApplication];
            let menu: id = msg_send![app, mainMenu];
            if menu != nil {
                let taken: bool = msg_send![menu, performKeyEquivalent: event];
                debug!(token, taken, "Cmd chord the page did not claim went to the menu");
            }
        }
        let _: () = msg_send![event, release];
    }
}

/// The NSView subclass that hosts RustKit content.
///
/// A stock NSView was measured to be a dead end for input: hitTest correctly
/// routes clicks to it, but events delivered to it NEVER surface as tao
/// window events — a synthetic mouseDown through `window sendEvent:` produced
/// nothing at the event loop (2026-08-07 probe). So the view records clicks,
/// moves, keys and, since 2026-10-06, the wheel itself. The wheel was left to
/// the responder chain on a 2026-08-05 reading that it reached tao's
/// `WindowEvent::MouseWheel`; in the app as built since the view took a
/// tracking area and first-responder status (#515, #523) it no longer does:
/// a real-window run on 2026-10-06 posted eight wheel events at the content
/// view's centre and the app logged none, while hover, press and keys from
/// the same tool arrived (Z lane I0, hand-test item H6). The wheel is
/// consumed here, not passed to super, so the window loop cannot scroll the
/// page a second time if the chain ever carries it again.
#[cfg(target_os = "macos")]
pub fn rustkit_content_view_class() -> &'static objc::runtime::Class {
    use objc::declare::ClassDecl;
    use objc::runtime::{Class, Object, Sel};
    static REGISTER: std::sync::Once = std::sync::Once::new();
    REGISTER.call_once(|| {
        let superclass = Class::get("NSView").expect("NSView class");
        let mut decl =
            ClassDecl::new("RustKitContentView", superclass).expect("register RustKitContentView");
        // Each view carries the ViewId it was created with, so what it
        // records is tagged with the view it was delivered to.
        decl.add_ivar::<u64>(VIEW_ID_IVAR);
        decl.add_ivar::<usize>(CURSOR_IVAR);

        extern "C" fn record(this: &Object, event: id, down: bool, input: PointerInput) {
            if input == PointerInput::Button {
                tracing::info!(down, "RustKitContentView mouse event handler entered");
            }
            unsafe {
                // locationInWindow is window coords (bottom-left origin);
                // convertPoint gives view-local, then flip to top-left.
                let wpt: cocoa::foundation::NSPoint = msg_send![event, locationInWindow];
                let lpt: cocoa::foundation::NSPoint =
                    msg_send![this, convertPoint: wpt fromView: nil];
                let frame: cocoa::foundation::NSRect = msg_send![this, frame];
                let (clicks, shift) = if matches!(input, PointerInput::Button | PointerInput::Secondary) {
                    let count: isize = msg_send![event, clickCount];
                    let flags: u64 = msg_send![event, modifierFlags];
                    (count.max(1) as u32, flags & (1 << 17) != 0) // NSEventModifierFlagShift
                } else {
                    (1, false)
                };
                let click = PendingClick {
                    x: lpt.x,
                    y: frame.size.height - lpt.y,
                    down,
                    input,
                    clicks,
                    shift,
                };
                // A move right after a move replaces it in the queue: the
                // page needs where the pointer is, not every point it
                // crossed since the last turn of the loop.
                push_input(view_id_of(this), InputKind::Pointer(click));
            }
        }
        extern "C" fn mouse_down(this: &Object, _sel: Sel, event: id) {
            // A press mid-composition finishes it first (#918 R1): the input
            // method forgets its marked text, the page keeps it (unmark:
            // compositionend), before the press can move the focus. Chrome
            // keeps the text when the focus leaves a composing field.
            let view = view_id_of(this);
            if MARKED.with(|m| m.borrow_mut().remove(&view)).is_some() {
                unsafe {
                    let context: id = msg_send![this, inputContext];
                    if context != nil {
                        let _: () = msg_send![context, discardMarkedText];
                    }
                }
                push_input(view, InputKind::Ime(ImeInput::Unmark));
            }
            record(this, event, true, PointerInput::Button);
        }
        extern "C" fn mouse_up(this: &Object, _sel: Sel, event: id) {
            record(this, event, false, PointerInput::Button);
        }
        // `mouseMoved:` with no button held, `mouseDragged:` with the
        // primary button held: both are a move to the page.
        extern "C" fn mouse_moved(this: &Object, _sel: Sel, event: id) {
            record(this, event, false, PointerInput::Move);
        }
        // CH2.E4: the secondary button is the page's `contextmenu`. Its
        // drag is a move (the page is not told the button is held).
        extern "C" fn right_mouse_down(this: &Object, _sel: Sel, event: id) {
            record(this, event, true, PointerInput::Secondary);
        }
        extern "C" fn right_mouse_up(this: &Object, _sel: Sel, event: id) {
            record(this, event, false, PointerInput::Secondary);
        }
        // The cursor the page asked for (`set_view_cursor`), when the
        // pointer comes into the view and whenever AppKit rebuilds the
        // window's cursor rects.
        extern "C" fn cursor_update(this: &Object, _sel: Sel, _event: id) {
            unsafe {
                let _: () = msg_send![view_cursor(this), set];
            }
        }
        extern "C" fn reset_cursor_rects(this: &Object, _sel: Sel) {
            unsafe {
                let visible: cocoa::foundation::NSRect = msg_send![this, visibleRect];
                let _: () = msg_send![this, addCursorRect: visible cursor: view_cursor(this)];
            }
        }
        extern "C" fn mouse_exited(this: &Object, _sel: Sel, event: id) {
            record(this, event, false, PointerInput::Leave);
        }
        // AppKit sends `mouseMoved:` and `mouseExited:` only to a view with
        // a tracking area that asks for them. `InVisibleRect` keeps the
        // area on the view's visible rect through every resize.
        extern "C" fn update_tracking_areas(this: &Object, _sel: Sel) {
            const MOUSE_ENTERED_AND_EXITED: u64 = 0x01;
            const MOUSE_MOVED: u64 = 0x02;
            const CURSOR_UPDATE: u64 = 0x04;
            const ACTIVE_ALWAYS: u64 = 0x80;
            const IN_VISIBLE_RECT: u64 = 0x200;
            unsafe {
                let superclass = Class::get("NSView").expect("NSView class");
                let _: () = msg_send![super(this, superclass), updateTrackingAreas];
                let areas: id = msg_send![this, trackingAreas];
                let count: usize = msg_send![areas, count];
                if count > 0 {
                    return;
                }
                let zero = cocoa::foundation::NSRect::new(
                    cocoa::foundation::NSPoint::new(0.0, 0.0),
                    cocoa::foundation::NSSize::new(0.0, 0.0),
                );
                let area: id = msg_send![Class::get("NSTrackingArea").expect("NSTrackingArea class"), alloc];
                let area: id = msg_send![area,
                    initWithRect: zero
                    options: MOUSE_ENTERED_AND_EXITED | MOUSE_MOVED | CURSOR_UPDATE | ACTIVE_ALWAYS | IN_VISIBLE_RECT
                    owner: this as *const Object as id
                    userInfo: nil];
                let _: () = msg_send![this, addTrackingArea: area];
                let _: () = msg_send![area, release];
            }
        }
        extern "C" fn scroll_wheel(this: &Object, _sel: Sel, event: id) {
            unsafe {
                let wpt: cocoa::foundation::NSPoint = msg_send![event, locationInWindow];
                let lpt: cocoa::foundation::NSPoint =
                    msg_send![this, convertPoint: wpt fromView: nil];
                let frame: cocoa::foundation::NSRect = msg_send![this, frame];
                let raw_dx: f64 = msg_send![event, scrollingDeltaX];
                let raw_dy: f64 = msg_send![event, scrollingDeltaY];
                let precise: bool = msg_send![event, hasPreciseScrollingDeltas];
                // Lines (an external wheel) at 40px a line; a trackpad
                // reports pixels. AppKit's horizontal sign is the inverse
                // of tao's, which is the sign the app's scroll code speaks.
                let scale = if precise { 1.0 } else { 40.0 };
                let scroll = PendingScroll {
                    x: lpt.x,
                    y: frame.size.height - lpt.y,
                    dx: -raw_dx * scale,
                    dy: raw_dy * scale,
                };
                // A wheel right after a wheel is summed into it in the
                // queue: a trackpad flick is ~50 events a second and the
                // page needs where it ends up, not every step. A click in
                // between keeps its place after the steps before it.
                push_input(view_id_of(this), InputKind::Wheel(scroll));
            }
        }
        extern "C" fn accepts_first_responder(_this: &Object, _sel: Sel) -> bool {
            // Without this, makeFirstResponder: refuses the view and macOS
            // keeps routing keys to whoever held focus before — observed
            // live 2026-08-07: click focused a page textarea (engine-side)
            // while typed characters went to the chrome URL bar, because
            // ENGINE focus and APPKIT first-responder are different systems
            // and only one was wired.
            true
        }
        unsafe fn pending_key(event: id, up: bool, text: Option<String>) -> PendingKey {
                let text = match text {
                    Some(text) => text,
                    None => text_of(msg_send![event, characters]),
                };
                let keycode: u16 = msg_send![event, keyCode];
                let flags: u64 = msg_send![event, modifierFlags];
                PendingKey {
                    text,
                    mac_keycode: keycode,
                    up,
                    ctrl: flags & (1 << 18) != 0,   // NSEventModifierFlagControl
                    cmd: flags & (1 << 20) != 0,    // NSEventModifierFlagCommand
                    shift: flags & (1 << 17) != 0,  // NSEventModifierFlagShift
                    alt: flags & (1 << 19) != 0,    // NSEventModifierFlagOption
                }
        }
        extern "C" fn record_key(this: &Object, event: id, up: bool, text: Option<String>) {
            unsafe {
                push_input(view_id_of(this), InputKind::Key(pending_key(event, up, text)));
            }
        }
        extern "C" fn key_down(this: &Object, _sel: Sel, event: id) {
            // Deliberately NOT calling super: consuming here is what keeps a
            // keystroke from ALSO reaching whatever else might interpret it.
            unsafe {
                let flags: u64 = msg_send![event, modifierFlags];
                if flags & (1 << 20) != 0 {
                    // A Cmd chord no menu item took (performKeyEquivalent:
                    // passed it on): the page's, as a key.
                    return record_key(this, event, false, None);
                }
                // The input method first (CH2.E3): a dead key or an IME
                // marks and commits through setMarkedText:/insertText:,
                // a plain key comes back as insertText: or
                // doCommandBySelector:. Only with a text field focused
                // (#918 R1): with nothing editable the view has no input
                // context, as Chromium has none for TEXT_INPUT_TYPE_NONE,
                // so a dead key or an IME's key is a plain key the page's
                // shortcuts hear.
                let view = view_id_of(this);
                if text_input_state(view).is_none() {
                    forget_marked(view);
                    return record_key(this, event, false, None);
                }
                let marked_before = MARKED.with(|m| m.borrow().contains_key(&view));
                KEY_DOWN.with(|k| {
                    *k.borrow_mut() = Some(KeyDownContext { view, composed: false, inserted: None })
                });
                let context: id = msg_send![this, inputContext];
                if context != nil {
                    let _: bool = msg_send![context, handleEvent: event];
                }
                let done = KEY_DOWN.with(|k| k.borrow_mut().take());
                let Some(done) = done else { return };
                if done.composed || marked_before {
                    // The input method's: the page hears the composition.
                    return;
                }
                // A plain key: the page hears it as one, with the text the
                // input method inserted for it (an Option+letter character
                // is the key's own text), and the window maps the specials
                // (arrows, delete, return) from the keyCode as before.
                record_key(this, event, false, done.inserted);
            }
        }
        extern "C" fn key_up(this: &Object, _sel: Sel, event: id) {
            record_key(this, event, true, None);
        }
        // ---- NSTextInputClient (CH2.E3) ----
        extern "C" fn set_marked_text(this: &Object, _sel: Sel, string: id, _selected: RkRange, replacement: RkRange) {
            let view = view_id_of(this);
            let text = unsafe { text_of(string) };
            let units = text.encode_utf16().count();
            MARKED.with(|m| {
                let mut m = m.borrow_mut();
                if units == 0 {
                    m.remove(&view);
                } else if let Some(marked) = m.get_mut(&view) {
                    marked.1 = units;
                } else {
                    let start = if replacement.location != NOT_FOUND {
                        replacement.location as usize
                    } else {
                        text_input_state(view).map_or(0, |s| s.selected.0)
                    };
                    m.insert(view, (start, units));
                }
            });
            KEY_DOWN.with(|k| {
                if let Some(k) = k.borrow_mut().as_mut().filter(|k| k.view == view) {
                    k.composed = true;
                }
            });
            push_input(view, InputKind::Ime(ImeInput::SetMarked(text)));
        }
        extern "C" fn insert_text_replacing(this: &Object, _sel: Sel, string: id, replacement: RkRange) {
            let view = view_id_of(this);
            let text = unsafe { text_of(string) };
            let had_marked = MARKED.with(|m| m.borrow_mut().remove(&view).is_some());
            let replace = (!had_marked && replacement.location != NOT_FOUND)
                .then_some((replacement.location as usize, replacement.length as usize));
            let deferred = KEY_DOWN.with(|k| {
                let mut k = k.borrow_mut();
                let Some(k) = k.as_mut().filter(|k| k.view == view) else {
                    return false;
                };
                if had_marked || replace.is_some() || k.composed {
                    k.composed = true;
                    return false;
                }
                // A plain key's text: it goes with the key itself.
                k.inserted.get_or_insert_with(String::new).push_str(&text);
                true
            });
            if !deferred {
                push_input(view, InputKind::Ime(ImeInput::Insert { text, replace }));
            }
        }
        extern "C" fn insert_text(this: &Object, sel: Sel, string: id) {
            insert_text_replacing(this, sel, string, RkRange { location: NOT_FOUND, length: 0 });
        }
        extern "C" fn unmark_text(this: &Object, _sel: Sel) {
            let view = view_id_of(this);
            if MARKED.with(|m| m.borrow_mut().remove(&view).is_some()) {
                push_input(view, InputKind::Ime(ImeInput::Unmark));
            }
        }
        // No input context while nothing editable has focus (#918 R1):
        // Chromium's view answers nil for TEXT_INPUT_TYPE_NONE, so no input
        // method composes keys that have nowhere to go.
        extern "C" fn input_context(this: &Object, _sel: Sel) -> id {
            if text_input_state(view_id_of(this)).is_none() {
                return nil;
            }
            unsafe {
                let superclass = Class::get("NSView").expect("NSView class");
                msg_send![super(this, superclass), inputContext]
            }
        }
        extern "C" fn has_marked_text(this: &Object, _sel: Sel) -> bool {
            let view = view_id_of(this);
            MARKED.with(|m| m.borrow().contains_key(&view))
        }
        extern "C" fn marked_range(this: &Object, _sel: Sel) -> RkRange {
            let view = view_id_of(this);
            match MARKED.with(|m| m.borrow().get(&view).copied()) {
                Some((start, len)) => RkRange { location: start as u64, length: len as u64 },
                None => RkRange { location: NOT_FOUND, length: 0 },
            }
        }
        extern "C" fn selected_range(this: &Object, _sel: Sel) -> RkRange {
            let view = view_id_of(this);
            if let Some((start, len)) = MARKED.with(|m| m.borrow().get(&view).copied()) {
                return RkRange { location: (start + len) as u64, length: 0 };
            }
            match text_input_state(view) {
                Some(s) => RkRange {
                    location: s.selected.0 as u64,
                    length: s.selected.1.saturating_sub(s.selected.0) as u64,
                },
                None => RkRange { location: NOT_FOUND, length: 0 },
            }
        }
        extern "C" fn valid_attributes_for_marked_text(_this: &Object, _sel: Sel) -> id {
            unsafe { msg_send![objc::runtime::Class::get("NSArray").expect("NSArray"), array] }
        }
        extern "C" fn attributed_substring(_this: &Object, _sel: Sel, _range: RkRange, _actual: *mut std::ffi::c_void) -> id {
            // The page's text is not mirrored here; the input method works
            // without it (reconversion and the dictionary panel do not).
            nil
        }
        extern "C" fn character_index_for_point(_this: &Object, _sel: Sel, _point: cocoa::foundation::NSPoint) -> u64 {
            NOT_FOUND
        }
        // Where the IME's candidate window goes: under the caret the page
        // last reported, in screen coordinates.
        extern "C" fn first_rect_for_range(this: &Object, _sel: Sel, range: RkRange, actual: *mut std::ffi::c_void) -> cocoa::foundation::NSRect {
            use cocoa::foundation::{NSPoint, NSRect, NSSize};
            unsafe {
                if !actual.is_null() {
                    *(actual as *mut RkRange) = range;
                }
                let frame: NSRect = msg_send![this, frame];
                let (x, top, height) = text_input_state(view_id_of(this))
                    .and_then(|s| s.caret)
                    .unwrap_or((0.0, 0.0, 0.0));
                // The view is not flipped: its y runs up from the bottom.
                let local = NSRect::new(NSPoint::new(x, frame.size.height - top - height), NSSize::new(0.0, height));
                let in_window: NSRect = msg_send![this, convertRect: local toView: nil];
                let window: id = msg_send![this, window];
                if window == nil {
                    return in_window;
                }
                msg_send![window, convertRectToScreen: in_window]
            }
        }
        extern "C" fn do_command_by_selector(this: &Object, _sel: Sel, _command: Sel) {
            // A key the input method did not turn into text (arrows,
            // delete, return, emacs chords): the key itself goes to the
            // page from keyDown:, which maps it from its keyCode.
            let _ = this;
        }
        // ---- the Edit menu (CH2.E3) ----
        // muda's predefined Edit items send these down the responder chain.
        // They reach the page as edit commands, with no keydown, as a menu
        // click in Chrome does. A Cmd chord reaches them only when the page
        // did not claim it (performKeyEquivalent: below), so they never
        // loop back into a key. (#912's selectAll: queued a Cmd+A press;
        // with the chord now going to the page first that press would come
        // back here unclaimed, so it is an edit action too.)
        fn edit(this: &Object, action: EditAction) {
            push_input(view_id_of(this), InputKind::Edit(action));
        }
        extern "C" fn copy(this: &Object, _sel: Sel, _sender: id) {
            edit(this, EditAction::Copy)
        }
        extern "C" fn cut(this: &Object, _sel: Sel, _sender: id) {
            edit(this, EditAction::Cut)
        }
        extern "C" fn paste(this: &Object, _sel: Sel, _sender: id) {
            edit(this, EditAction::Paste)
        }
        extern "C" fn undo(this: &Object, _sel: Sel, _sender: id) {
            edit(this, EditAction::Undo)
        }
        extern "C" fn redo(this: &Object, _sel: Sel, _sender: id) {
            edit(this, EditAction::Redo)
        }
        extern "C" fn select_all(this: &Object, _sel: Sel, _sender: id) {
            edit(this, EditAction::SelectAll)
        }
        // A Cmd chord while this view has the keyboard: the page hears its
        // keydown before the menu (Chromium's order, CH2.E3). AppKit asks
        // every view of the key window before the main menu; the view takes
        // the chord, and the app hands back the ones the page did not
        // claim (`finish_key_equivalent`). Chromium's reserved chords
        // (Cmd+T/N/W/Q, ...) go straight to the menu, as in Chrome.
        extern "C" fn perform_key_equivalent(this: &Object, _sel: Sel, event: id) -> bool {
            unsafe {
                let superclass = Class::get("NSView").expect("NSView class");
                let pass = |event: id| -> bool { msg_send![super(this, superclass), performKeyEquivalent: event] };
                let kind: u64 = msg_send![event, type];
                let flags: u64 = msg_send![event, modifierFlags];
                if kind != 10 || flags & (1 << 20) == 0 {
                    return pass(event);
                }
                let window: id = msg_send![this, window];
                let responder: id = if window == nil { nil } else { msg_send![window, firstResponder] };
                if responder != this as *const Object as id {
                    return pass(event);
                }
                let key = pending_key(event, false, None);
                let plain = text_of(msg_send![event, charactersIgnoringModifiers]);
                if is_reserved_chord(&plain, key.mac_keycode, key.cmd, key.shift, key.alt) {
                    return pass(event);
                }
                let token = NEXT_KEY_EQUIVALENT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let _: id = msg_send![event, retain];
                KEY_EQUIVALENTS
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((token, event as usize));
                push_input(view_id_of(this), InputKind::KeyEquivalent { key, token });
                true
            }
        }
        extern "C" fn accepts_first_mouse(_this: &Object, _sel: Sel, _event: id) -> bool {
            // A click on an inactive window should reach the page (this is
            // what browsers do for links), and the synthetic probe runs
            // before the window is ever key.
            true
        }
        // CH3.A3: a view with a region (the RustKit chrome stacked over the
        // page) is absent outside it, so AppKit goes on to the view below.
        // A view without one keeps NSView's hit test.
        extern "C" fn hit_test(this: &Object, _sel: Sel, point: cocoa::foundation::NSPoint) -> id {
            unsafe {
                let superclass = Class::get("NSView").expect("NSView class");
                let hit: id = msg_send![super(this, superclass), hitTest: point];
                if hit == nil {
                    return nil;
                }
                let id = view_id_of(this).raw();
                let outside = REGIONS.with_region(id, |region| {
                    let superview: id = msg_send![this, superview];
                    let local: cocoa::foundation::NSPoint =
                        msg_send![this, convertPoint: point fromView: superview];
                    let frame: cocoa::foundation::NSRect = msg_send![this, frame];
                    !crate::region::region_contains(region, local.x, frame.size.height - local.y)
                });
                if outside == Some(true) {
                    return nil;
                }
                hit
            }
        }
        unsafe {
            decl.add_method(
                sel!(hitTest:),
                hit_test as extern "C" fn(&Object, Sel, cocoa::foundation::NSPoint) -> id,
            );
            decl.add_method(
                sel!(acceptsFirstMouse:),
                accepts_first_mouse as extern "C" fn(&Object, Sel, id) -> bool,
            );
            decl.add_method(
                sel!(acceptsFirstResponder),
                accepts_first_responder as extern "C" fn(&Object, Sel) -> bool,
            );
            decl.add_method(sel!(keyDown:), key_down as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(keyUp:), key_up as extern "C" fn(&Object, Sel, id));
            decl.add_method(
                sel!(setMarkedText:selectedRange:replacementRange:),
                set_marked_text as extern "C" fn(&Object, Sel, id, RkRange, RkRange),
            );
            decl.add_method(
                sel!(insertText:replacementRange:),
                insert_text_replacing as extern "C" fn(&Object, Sel, id, RkRange),
            );
            decl.add_method(sel!(insertText:), insert_text as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(unmarkText), unmark_text as extern "C" fn(&Object, Sel));
            decl.add_method(sel!(hasMarkedText), has_marked_text as extern "C" fn(&Object, Sel) -> bool);
            decl.add_method(sel!(inputContext), input_context as extern "C" fn(&Object, Sel) -> id);
            decl.add_method(sel!(markedRange), marked_range as extern "C" fn(&Object, Sel) -> RkRange);
            decl.add_method(sel!(selectedRange), selected_range as extern "C" fn(&Object, Sel) -> RkRange);
            decl.add_method(
                sel!(validAttributesForMarkedText),
                valid_attributes_for_marked_text as extern "C" fn(&Object, Sel) -> id,
            );
            decl.add_method(
                sel!(attributedSubstringForProposedRange:actualRange:),
                attributed_substring as extern "C" fn(&Object, Sel, RkRange, *mut std::ffi::c_void) -> id,
            );
            decl.add_method(
                sel!(characterIndexForPoint:),
                character_index_for_point as extern "C" fn(&Object, Sel, cocoa::foundation::NSPoint) -> u64,
            );
            decl.add_method(
                sel!(firstRectForCharacterRange:actualRange:),
                first_rect_for_range
                    as extern "C" fn(&Object, Sel, RkRange, *mut std::ffi::c_void) -> cocoa::foundation::NSRect,
            );
            decl.add_method(
                sel!(doCommandBySelector:),
                do_command_by_selector as extern "C" fn(&Object, Sel, Sel),
            );
            decl.add_method(sel!(copy:), copy as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(cut:), cut as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(paste:), paste as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(undo:), undo as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(redo:), redo as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(selectAll:), select_all as extern "C" fn(&Object, Sel, id));
            decl.add_method(
                sel!(performKeyEquivalent:),
                perform_key_equivalent as extern "C" fn(&Object, Sel, id) -> bool,
            );
            decl.add_method(
                sel!(mouseDown:),
                mouse_down as extern "C" fn(&Object, Sel, id),
            );
            decl.add_method(sel!(mouseUp:), mouse_up as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(mouseMoved:), mouse_moved as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(mouseDragged:), mouse_moved as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(mouseExited:), mouse_exited as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(rightMouseDown:), right_mouse_down as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(rightMouseUp:), right_mouse_up as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(rightMouseDragged:), mouse_moved as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(cursorUpdate:), cursor_update as extern "C" fn(&Object, Sel, id));
            decl.add_method(sel!(resetCursorRects), reset_cursor_rects as extern "C" fn(&Object, Sel));
            decl.add_method(sel!(scrollWheel:), scroll_wheel as extern "C" fn(&Object, Sel, id));
            decl.add_method(
                sel!(updateTrackingAreas),
                update_tracking_areas as extern "C" fn(&Object, Sel),
            );
        }
        // The input context exists only for a view that says it is a
        // text input client (`-[NSView inputContext]`).
        if let Some(protocol) = objc::runtime::Protocol::get("NSTextInputClient") {
            decl.add_protocol(protocol);
        }
        decl.register();
    });
    Class::get("RustKitContentView").expect("RustKitContentView registered")
}

/// The regions set with [`set_raw_view_region`], by the raw [`ViewId`] of
/// the view (CH3.A3). Read by `hitTest:` on the main thread.
#[cfg(target_os = "macos")]
struct RegionStore(std::sync::OnceLock<Mutex<HashMap<u64, Vec<crate::region::RegionRect>>>>);

#[cfg(target_os = "macos")]
impl RegionStore {
    fn map(&self) -> &Mutex<HashMap<u64, Vec<crate::region::RegionRect>>> {
        self.0.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn with_region<R>(&self, id: u64, f: impl FnOnce(&[crate::region::RegionRect]) -> R) -> Option<R> {
        // Copied out so no lock is held across the AppKit calls in `f`.
        let region = self.map().lock().ok()?.get(&id).cloned()?;
        Some(f(&region))
    }
}

#[cfg(target_os = "macos")]
static REGIONS: RegionStore = RegionStore(std::sync::OnceLock::new());

/// Make a `RustKitContentView` a transparent view (CH3.A3c, the alpha
/// route): its layer is not opaque and its region, when it has one, is
/// only its hit region (`hitTest:`), not a layer mask, so its own pixels
/// decide what covers the views below. Takes effect at the next
/// [`set_raw_view_region`]. No-op for a view of another class.
#[cfg(target_os = "macos")]
pub fn set_raw_view_transparent(view: isize, transparent: bool) {
    use objc::runtime::{Class, Object};
    if view == 0 {
        return;
    }
    let view = view as id;
    unsafe {
        let class: *const Class = msg_send![view, class];
        if class != rustkit_content_view_class() as *const Class {
            return;
        }
        let id = view_id_of(&*(view as *const Object)).raw();
        if let Ok(mut set) = TRANSPARENT_VIEWS.get_or_init(|| Mutex::new(std::collections::HashSet::new())).lock() {
            if transparent {
                set.insert(id);
            } else {
                set.remove(&id);
            }
        }
        let layer: id = msg_send![view, layer];
        if layer != nil {
            // wgpu's CAMetalLayer is a sublayer; its own `opaque` follows the
            // surface's alpha mode (`Compositor::set_surface_transparent`).
            let _: () = msg_send![layer, setOpaque: !transparent];
        }
    }
}

/// `RustKitContentView`s made transparent (CH3.A3c): their region is a hit
/// region only.
#[cfg(target_os = "macos")]
static TRANSPARENT_VIEWS: std::sync::OnceLock<Mutex<std::collections::HashSet<u64>>> = std::sync::OnceLock::new();

#[cfg(target_os = "macos")]
fn is_transparent_view(id: u64) -> bool {
    TRANSPARENT_VIEWS
        .get()
        .and_then(|set| set.lock().ok().map(|set| set.contains(&id)))
        .unwrap_or(false)
}

/// Give a `RustKitContentView` a region (CH3.A3): outside the rects (view
/// coordinates, top-left origin) the view is not composited (a mask on its
/// layer, which clips wgpu's Metal sublayer too) and not hit (`hitTest:`
/// returns nil). `None` removes both. No-op for a view of another class.
#[cfg(target_os = "macos")]
pub fn set_raw_view_region(view: isize, region: Option<Vec<crate::region::RegionRect>>) {
    use objc::runtime::{Class, Object};
    if view == 0 {
        return;
    }
    let view = view as id;
    unsafe {
        let class: *const Class = msg_send![view, class];
        if class != rustkit_content_view_class() as *const Class {
            return;
        }
        let id = view_id_of(&*(view as *const Object)).raw();
        if let Ok(mut map) = REGIONS.map().lock() {
            match &region {
                Some(rects) => {
                    map.insert(id, rects.clone());
                }
                None => {
                    map.remove(&id);
                }
            }
        }
        let layer: id = msg_send![view, layer];
        if layer == nil {
            return;
        }
        let transaction = Class::get("CATransaction").expect("CATransaction class");
        let _: () = msg_send![transaction, begin];
        let _: () = msg_send![transaction, setDisableActions: true];
        // A transparent view's pixels decide what covers the views below
        // (CH3.A3c): its region is for `hitTest:` only, never a mask.
        let region = if is_transparent_view(id) { None } else { region };
        match region {
            None => {
                let _: () = msg_send![layer, setMask: nil];
            }
            Some(rects) => {
                let bounds: cocoa::foundation::NSRect = msg_send![layer, bounds];
                let flipped: bool = msg_send![layer, isGeometryFlipped];
                let path = core_graphics_path(&rects, bounds.size.height, flipped);
                let mut mask: id = msg_send![layer, mask];
                let shape = Class::get("CAShapeLayer").expect("CAShapeLayer class");
                let is_shape: bool = mask != nil && msg_send![mask, isKindOfClass: shape];
                if !is_shape {
                    mask = msg_send![shape, layer];
                    let _: () = msg_send![layer, setMask: mask];
                }
                let _: () = msg_send![mask, setFrame: bounds];
                let _: () = msg_send![mask, setPath: path];
                CGPathRelease(path);
            }
        }
        let _: () = msg_send![transaction, commit];
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathCreateMutable() -> *mut std::ffi::c_void;
    fn CGPathAddRect(path: *mut std::ffi::c_void, m: *const std::ffi::c_void, rect: cocoa::foundation::NSRect);
    fn CGPathRelease(path: *mut std::ffi::c_void);
}

/// The union of `rects` (top-left origin) as a CGPath in the layer's
/// coordinates: bottom-left origin unless the layer's geometry is flipped.
/// A shape layer fills overlapping subpaths with the non-zero rule, so the
/// union is filled once.
#[cfg(target_os = "macos")]
unsafe fn core_graphics_path(
    rects: &[crate::region::RegionRect],
    height: f64,
    flipped: bool,
) -> *mut std::ffi::c_void {
    let path = CGPathCreateMutable();
    for r in rects {
        let y = if flipped { r.y } else { height - r.y - r.height };
        let rect = cocoa::foundation::NSRect::new(
            cocoa::foundation::NSPoint::new(r.x, y),
            cocoa::foundation::NSSize::new(r.width, r.height),
        );
        CGPathAddRect(path, std::ptr::null(), rect);
    }
    path
}

/// Re-add `view` to its superview directly above `below` (CH3.A3: the
/// RustKit chrome over the page). Both must share a superview.
#[cfg(target_os = "macos")]
pub fn raise_raw_view_above(view: isize, below: isize) {
    if view == 0 || below == 0 {
        return;
    }
    let (view, below) = (view as id, below as id);
    unsafe {
        let superview: id = msg_send![below, superview];
        let own: id = msg_send![view, superview];
        if superview == nil || own != superview {
            warn!("raise_raw_view_above: the views do not share a superview");
            return;
        }
        // NSWindowAbove = 1.
        let _: () = msg_send![superview, addSubview: view positioned: 1i64 relativeTo: below];
    }
}

/// Make `view` the frontmost of its superview's subviews, when it is not
/// already (CH3.A3b: the RustKit chrome over panels built after it). The
/// window's first responder is kept: a view re-added to its superview can
/// lose it. True when the view moved.
#[cfg(target_os = "macos")]
pub fn raise_raw_view_to_front(view: isize) -> bool {
    if view == 0 {
        return false;
    }
    let view = view as id;
    unsafe {
        let superview: id = msg_send![view, superview];
        if superview == nil {
            return false;
        }
        let subviews: id = msg_send![superview, subviews];
        let last: id = msg_send![subviews, lastObject];
        if last == view {
            return false;
        }
        let window: id = msg_send![view, window];
        let responder: id = if window != nil { msg_send![window, firstResponder] } else { nil };
        // NSWindowAbove = 1; relative to nil: above every sibling.
        let _: () = msg_send![superview, addSubview: view positioned: 1i64 relativeTo: nil];
        if window != nil && responder != nil {
            let now: id = msg_send![window, firstResponder];
            if now != responder {
                let _: bool = msg_send![window, makeFirstResponder: responder];
            }
        }
        true
    }
}

/// The cursor a content view shows: the one `set_view_cursor` stored, else
/// the arrow.
#[cfg(target_os = "macos")]
fn view_cursor(this: &objc::runtime::Object) -> id {
    let stored: usize = unsafe { *this.get_ivar::<usize>(CURSOR_IVAR) };
    if stored != 0 {
        return stored as id;
    }
    unsafe { msg_send![objc::runtime::Class::get("NSCursor").expect("NSCursor class"), arrowCursor] }
}

/// `raw` as a RustKit content view, or `None` for any other view.
#[cfg(target_os = "macos")]
fn content_view(raw: isize) -> Option<id> {
    if raw == 0 {
        return None;
    }
    let view = raw as id;
    let is_ours: bool = unsafe { msg_send![view, isKindOfClass: rustkit_content_view_class()] };
    is_ours.then_some(view)
}

/// Show the cursor for the CSS keyword the page reported under the pointer
/// (`PageEvent::Cursor`, CH2.E4) over the content view `raw`: set at once
/// when the pointer is over the view, and kept in the view's cursor rect so
/// AppKit restores it when the pointer comes back.
#[cfg(target_os = "macos")]
pub fn set_view_cursor(raw: isize, css: &str) {
    use objc::runtime::{Class, Sel};
    let Some(view) = content_view(raw) else {
        return;
    };
    let selector = crate::cursor::ns_cursor_selector(css);
    unsafe {
        let class = Class::get("NSCursor").expect("NSCursor class");
        let sel = Sel::register(selector);
        let known: bool = msg_send![class, respondsToSelector: sel];
        let cursor: id = if known { msg_send![class, performSelector: sel] } else { nil };
        let cursor: id = if cursor == nil { msg_send![class, arrowCursor] } else { cursor };
        (*view).set_ivar::<usize>(CURSOR_IVAR, cursor as usize);
        let window: id = msg_send![view, window];
        if window != nil {
            let at: cocoa::foundation::NSPoint = msg_send![window, mouseLocationOutsideOfEventStream];
            let local: cocoa::foundation::NSPoint = msg_send![view, convertPoint: at fromView: nil];
            let bounds: cocoa::foundation::NSRect = msg_send![view, bounds];
            let inside = local.x >= bounds.origin.x
                && local.y >= bounds.origin.y
                && local.x < bounds.origin.x + bounds.size.width
                && local.y < bounds.origin.y + bounds.size.height;
            if inside {
                let _: () = msg_send![cursor, set];
            }
            let _: () = msg_send![window, invalidateCursorRectsForView: view];
        }
    }
    debug!(css, ns = selector, "content view cursor set");
}

/// The content view's tooltip: the nearest `title` under the pointer
/// (`PageEvent::Tooltip`, CH2.E4), `None` for none. AppKit shows it when
/// the pointer rests.
#[cfg(target_os = "macos")]
pub fn set_view_tooltip(raw: isize, title: Option<&str>) {
    let Some(view) = content_view(raw) else {
        return;
    };
    unsafe {
        let text: id = match title {
            Some(t) => {
                let s = cocoa::foundation::NSString::alloc(nil);
                let s: id = cocoa::foundation::NSString::init_str(s, t);
                msg_send![s, autorelease]
            }
            None => nil,
        };
        let _: () = msg_send![view, setToolTip: text];
    }
    debug!(tooltip = ?title, "content view tooltip set");
}

#[cfg(target_os = "macos")]
impl MacOSViewHost {
    pub fn new() -> Self {
        Self {
            views: RwLock::new(HashMap::new()),
        }
    }

    /// Convert top-left origin bounds to Cocoa's bottom-left origin coordinate system.
    ///
    /// HiWave/Wry uses top-left origin (y=0 at top, increasing downward).
    /// Cocoa uses bottom-left origin (y=0 at bottom, increasing upward).
    ///
    /// Formula: y_cocoa = parent_height - bounds.y - bounds.height
    fn convert_to_cocoa_frame(bounds: Bounds, parent_height: f64) -> cocoa::foundation::NSRect {
        let y_cocoa = parent_height - bounds.y as f64 - bounds.height as f64;
        cocoa::foundation::NSRect::new(
            cocoa::foundation::NSPoint::new(bounds.x as f64, y_cocoa),
            cocoa::foundation::NSSize::new(bounds.width as f64, bounds.height as f64),
        )
    }

    /// Create a view from a TAO window handle
    pub fn create_view_from_window(
        &self,
        window_handle: RawWindowHandle,
        bounds: Bounds,
    ) -> Result<ViewId, ViewHostError> {
        let view_id = ViewId::new();
        debug!(?view_id, ?bounds, "Creating macOS view");

        // Extract NSWindow from raw window handle
        // In raw-window-handle 0.6, AppKitHandle contains ns_view, not ns_window
        // We need to get the window from the view
        let ns_view = match window_handle {
            RawWindowHandle::AppKit(handle) => {
                handle.ns_view.as_ptr() as id
            }
            _ => {
                return Err(ViewHostError::InvalidParent);
            }
        };
        
        if ns_view == nil {
            return Err(ViewHostError::InvalidParent);
        }
        
        // Get the window from the view
        let ns_window: id = unsafe { msg_send![ns_view, window] };

        if ns_window == nil {
            return Err(ViewHostError::InvalidParent);
        }

        // Get the content view of the window
        let content_view: id = unsafe { msg_send![ns_window, contentView] };
        if content_view == nil {
            return Err(ViewHostError::WindowCreation(
                "Window has no content view".to_string(),
            ));
        }

        // Get the content view's frame to get parent height for coordinate conversion
        let parent_frame: cocoa::foundation::NSRect = unsafe { msg_send![content_view, frame] };
        let parent_height = parent_frame.size.height;

        debug!(parent_height, "Got parent content view height");

        // Create a new NSView for our content
        // Convert from top-left origin (HiWave/Wry) to bottom-left origin (Cocoa)
        let frame = Self::convert_to_cocoa_frame(bounds, parent_height);
        debug!(?bounds, cocoa_y = frame.origin.y, "Converted bounds to Cocoa coordinates");

        let view: id = unsafe {
            let view_class = rustkit_content_view_class();
            let view: id = msg_send![view_class, alloc];
            msg_send![view, initWithFrame: frame]
        };

        if view == nil {
            return Err(ViewHostError::WindowCreation(
                "Failed to create NSView".to_string(),
            ));
        }
        unsafe {
            (*view).set_ivar::<u64>(VIEW_ID_IVAR, view_id.raw());
        }

        // Configure the view for layer-backed rendering
        // NOTE: Don't manually create CAMetalLayer - let wgpu manage it
        // wgpu will create and configure its own Metal layer when the surface is created
        unsafe {
            // Enable layer-backed rendering (required for wgpu)
            let wants_layer: bool = true;
            let _: () = msg_send![view, setWantsLayer: wants_layer];
        }

        // Add view to content view
        unsafe {
            let _: () = msg_send![content_view, addSubview: view];
            debug!(?view_id, "Added RustKit view as subview");
        }

        // Get DPI (backing scale factor)
        let dpi = unsafe {
            let scale: f64 = msg_send![ns_window, backingScaleFactor];
            (scale * 96.0) as u32
        };

        let state = Arc::new(Mutex::new(MacOSViewState {
            _id: view_id,
            view,
            bounds,
            dpi,
            visible: true,
            _focused: false,
        }));

        {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in create_view_from_window: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views.insert(view_id, state);
        }

        info!(?view_id, dpi, "macOS view created");
        Ok(view_id)
    }

    /// Get the NSView for a view ID
    pub fn get_view(&self, view_id: ViewId) -> Result<id, ViewHostError> {
        let state_arc = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in get_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
                .clone() // Clone the Arc to extend lifetime
        }; // views lock is released here
        let view = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_view: {}", e);
            ViewHostError::LockPoisoned
        })?.view;
        Ok(view)
    }

    /// Get the raw window handle for a view
    pub fn get_raw_window_handle(&self, view_id: ViewId) -> Result<RawWindowHandle, ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in get_raw_window_handle: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;
        let view = state.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_raw_window_handle: {}", e);
            ViewHostError::LockPoisoned
        })?.view;

        // Get the window from the view
        let window: id = unsafe { msg_send![view, window] };
        if window == nil {
            warn!(?view_id, "View has no window attached");
            return Err(ViewHostError::ViewNotFound(view_id));
        }

        // Verify view state
        unsafe {
            let is_hidden: bool = msg_send![view, isHidden];
            let superview: id = msg_send![view, superview];
            let has_superview = superview != nil;
            let frame: cocoa::foundation::NSRect = msg_send![view, frame];
            info!(
                ?view_id,
                is_hidden,
                has_superview,
                frame_x = frame.origin.x,
                frame_y = frame.origin.y,
                frame_w = frame.size.width,
                frame_h = frame.size.height,
                "Getting raw window handle - view state"
            );
        }

        // Create raw window handle
        // AppKitWindowHandle::new() expects NonNull<c_void>
        use std::ptr::NonNull;
        let handle = RawWindowHandle::AppKit(
            raw_window_handle::AppKitWindowHandle::new(
                NonNull::new(view as *mut std::ffi::c_void)
                    .expect("View pointer is null")
            )
        );

        Ok(handle)
    }

    /// Set view bounds
    pub fn set_bounds(&self, view_id: ViewId, bounds: Bounds) -> Result<(), ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in set_bounds: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Record the new bounds under the lock, then release it before any
        // AppKit call: `setFrame:` runs layout callbacks synchronously on a
        // subclassed view. See `focus` for the full rationale.
        let view: id = {
            let mut guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in set_bounds: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.bounds = bounds;
            guard.view
        };

        unsafe {
            // Get the superview to determine parent height for coordinate conversion
            let superview: id = msg_send![view, superview];
            let parent_height = if superview != nil {
                let parent_frame: cocoa::foundation::NSRect = msg_send![superview, frame];
                parent_frame.size.height
            } else {
                // Fallback: try to get window content view height
                let window: id = msg_send![view, window];
                if window != nil {
                    let content_view: id = msg_send![window, contentView];
                    if content_view != nil {
                        let content_frame: cocoa::foundation::NSRect = msg_send![content_view, frame];
                        content_frame.size.height
                    } else {
                        bounds.height as f64 + bounds.y as f64 // Fallback
                    }
                } else {
                    bounds.height as f64 + bounds.y as f64 // Fallback
                }
            };

            // Convert from top-left origin to Cocoa's bottom-left origin
            let frame = Self::convert_to_cocoa_frame(bounds, parent_height);
            let _: () = msg_send![view, setFrame: frame];
            let layer: id = msg_send![view, layer];
            if layer != nil {
                let masks: bool = true;
                let _: () = msg_send![layer, setMasksToBounds: masks];
            }
        }

        debug!(?view_id, ?bounds, "View bounds updated");
        Ok(())
    }

    /// Get view bounds
    pub fn get_bounds(&self, view_id: ViewId) -> Result<Bounds, ViewHostError> {
        let state_arc = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in get_bounds: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
                .clone() // Clone the Arc to extend lifetime
        }; // views lock is released here
        let bounds = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_bounds: {}", e);
            ViewHostError::LockPoisoned
        })?.bounds;
        Ok(bounds)
    }

    /// Set view visibility
    pub fn set_visible(&self, view_id: ViewId, visible: bool) -> Result<(), ViewHostError> {
        let views = self.views.read().map_err(|e| {
            tracing::error!("Views RwLock poisoned in set_visible: {}", e);
            ViewHostError::LockPoisoned
        })?;
        let state = views
            .get(&view_id)
            .ok_or(ViewHostError::ViewNotFound(view_id))?;

        // Mutate state under the lock, then release before AppKit:
        // `setHidden:` runs viewDidHide/viewDidUnhide synchronously on a
        // subclassed view. See `focus` for the full rationale.
        let view: id = {
            let mut guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in set_visible: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.visible = visible;
            guard.view
        };

        unsafe {
            let hidden: bool = !visible;
            let _: () = msg_send![view, setHidden: hidden];
        }

        debug!(?view_id, visible, "View visibility changed");
        Ok(())
    }

    /// Focus a view
    ///
    /// COPY THE VIEW POINTER, DROP EVERY LOCK, *THEN* CALL APPKIT.
    ///
    /// `makeFirstResponder:` is synchronous and re-enters the responder
    /// chain — `resignFirstResponder` / `becomeFirstResponder` and any focus
    /// notification run before it returns. The moment this NSView gains
    /// responder overrides that call back into the ViewHost (the next unit:
    /// content keyboard input), holding the per-view `Mutex` across that call
    /// would deadlock the process forever on a non-reentrant lock, with no
    /// error and no log past this line.
    ///
    /// Athena hit exactly this on Windows (`SetFocus` dispatching
    /// `WM_SETFOCUS` into our own wnd_proc, hiwave-windows#85): every focus
    /// call hung the process from the day it was written, and nothing found
    /// it because nothing ever called it. An unused API is not a working API,
    /// it is an untested one — this method has zero callers here too.
    pub fn focus(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let view: id = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in focus: {}", e);
                ViewHostError::LockPoisoned
            })?;
            let state = views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?;
            let guard = state.lock().map_err(|e| {
                tracing::error!("ViewState lock poisoned in focus: {}", e);
                ViewHostError::LockPoisoned
            })?;
            guard.view
        }; // both guards released here, before any AppKit call

        unsafe {
            let window: id = msg_send![view, window];
            if window != nil {
                let _: () = msg_send![window, makeFirstResponder: view];
            }
        }

        debug!(?view_id, "View focused");
        Ok(())
    }

    /// Get DPI for a view
    pub fn get_dpi(&self, view_id: ViewId) -> Result<u32, ViewHostError> {
        let state_arc = {
            let views = self.views.read().map_err(|e| {
                tracing::error!("Views RwLock poisoned in get_dpi: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .get(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
                .clone() // Clone the Arc to extend lifetime
        }; // views lock is released here
        let dpi = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in get_dpi: {}", e);
            ViewHostError::LockPoisoned
        })?.dpi;
        Ok(dpi)
    }

    /// Destroy a view
    pub fn destroy_view(&self, view_id: ViewId) -> Result<(), ViewHostError> {
        let state_arc = {
            let mut views = self.views.write().map_err(|e| {
                tracing::error!("Views RwLock poisoned in destroy_view: {}", e);
                ViewHostError::LockPoisoned
            })?;
            views
                .remove(&view_id)
                .ok_or(ViewHostError::ViewNotFound(view_id))?
        }; // views lock is released here

        let view = state_arc.lock().map_err(|e| {
            tracing::error!("ViewState lock poisoned in destroy_view: {}", e);
            ViewHostError::LockPoisoned
        })?.view;

        unsafe {
            let _: () = msg_send![view, removeFromSuperview];
        }
        // An input recorded for this view must not reach whatever view
        // comes next.
        retire_view_input(view_id);

        debug!(?view_id, "View destroyed");
        Ok(())
    }

    /// Pump macOS event loop (stub for now)
    pub fn pump_messages(&self) -> bool {
        // TODO: Implement proper event loop pumping
        // For now, this is a no-op as TAO handles the event loop
        true
    }
}

#[cfg(not(target_os = "macos"))]
pub struct MacOSViewHost;

#[cfg(not(target_os = "macos"))]
impl MacOSViewHost {
    pub fn new() -> Self {
        Self
    }
}

