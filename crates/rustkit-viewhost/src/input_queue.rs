//! The input queue: what the content view's AppKit handlers record, drained
//! by the app each turn of its loop (threads plan M2, #575).
//!
//! A queue, not a callback: the handlers run inside AppKit's event dispatch,
//! and calling back into app/engine state from there is the re-entrancy trap
//! #108 exists to prevent. Push under a short lock, drain on the main loop.
//!
//! The types here are plain data with no AppKit in them, so the queue and
//! its tests build on every OS; `macos.rs` fills it from the view's
//! selectors.

use crate::ViewId;
use std::sync::Mutex;
use std::time::Instant;

/// A content-view click, in VIEW-LOCAL TOP-LEFT coordinates: exactly the
/// viewport space the engine's hit testing speaks, no chrome/sidebar math.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingClick {
    pub x: f64,
    pub y: f64,
    pub down: bool,
    pub input: PointerInput,
    /// A press or release: which click of a multi-click it is (AppKit's
    /// `clickCount`, 1 for a single click). 1 for the rest.
    pub clicks: u32,
    /// Shift was held (a press extends the selection).
    pub shift: bool,
}

/// What a [`PendingClick`] records: a press or release of the primary
/// button (`down` says which), of the secondary button (the context
/// menu, CH2.E4), the pointer moving over the view (with a button held or
/// not: the page knows which), or the pointer leaving it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerInput {
    Button,
    Secondary,
    Move,
    Leave,
}

/// A key event captured by the content view while it is first responder.
///
/// `text` carries the typed characters (empty for pure control keys);
/// `mac_keycode` is the hardware-independent macOS keyCode for specials;
/// `up` is a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingKey {
    pub text: String,
    pub mac_keycode: u16,
    pub up: bool,
    pub ctrl: bool,
    pub cmd: bool,
    pub shift: bool,
    pub alt: bool,
}

/// A wheel turn or trackpad swipe over the content view.
///
/// `dx`, `dy` are in CSS pixels with the sign hiwave-app's window-loop arm
/// always used (tao's): `dy < 0` moves the page down, `dx > 0` moves it
/// right. A wheel that reports lines is counted at 40px a line, as the
/// window-loop arm counts it. `x`, `y` are where the pointer was, view-local
/// top-left, for an inner scroller one day; a page scroll does not read them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PendingScroll {
    pub x: f64,
    pub y: f64,
    pub dx: f64,
    pub dy: f64,
}

/// What the platform's input method handed the content view through
/// `NSTextInputClient` (CH2.E3): a dead key or an IME composing, and the
/// text it commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImeInput {
    /// `setMarkedText:` — compose `text`; an empty string cancels.
    SetMarked(String),
    /// `insertText:` — commit `text`. With text marked it replaces the
    /// marked text. With nothing marked, `replace` is the UTF-16 range
    /// (start, length) of the value it replaces, as the press-and-hold
    /// accent popover replaces the letter typed before it; `None` inserts
    /// at the selection.
    Insert { text: String, replace: Option<(usize, usize)> },
    /// `unmarkText` — keep what is marked and end the composition.
    Unmark,
}

/// An Edit menu item's action sent to the content view, the first
/// responder (`copy:`, `cut:`, `paste:`, `undo:`, `redo:`, `selectAll:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Copy,
    Cut,
    Paste,
    Undo,
    Redo,
    SelectAll,
}

/// One recorded input.
#[derive(Debug, Clone, PartialEq)]
pub enum InputKind {
    Pointer(PendingClick),
    Key(PendingKey),
    Wheel(PendingScroll),
    /// Marked or committed text from the input method (CH2.E3).
    Ime(ImeInput),
    /// An Edit menu item (CH2.E3).
    Edit(EditAction),
    /// A Cmd chord the view took in `performKeyEquivalent:` so the page
    /// hears its keydown before the menu does (CH2.E3, Chromium's order).
    /// The app hands `token` to `finish_key_equivalent`: a chord the page
    /// did not claim goes on to the main menu.
    KeyEquivalent { key: PendingKey, token: u64 },
}

/// A focused text field's state as the page last reported it, kept for the
/// input method's synchronous questions (`selectedRange`, `markedRange`,
/// `firstRectForCharacterRange:`), which AppKit asks inside its own event
/// dispatch where the page cannot be asked. Offsets are UTF-16; the caret
/// is view-local top-left points: x, y (top) and height.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextInputMirror {
    pub marked: Option<(usize, usize)>,
    pub selected: (usize, usize),
    pub caret: Option<(f64, f64, f64)>,
}

static TEXT_INPUT: Mutex<Vec<(ViewId, TextInputMirror)>> = Mutex::new(Vec::new());

/// Record what the page reported for `view`'s focused text field; `None`
/// when no text field has focus.
pub fn set_text_input_state(view: ViewId, state: Option<TextInputMirror>) {
    let mut all = TEXT_INPUT.lock().unwrap_or_else(|e| e.into_inner());
    all.retain(|(v, _)| *v != view);
    if let Some(state) = state {
        all.push((view, state));
    } else {
        drop(all);
        // The field lost the focus: its marked text goes with it (#918 R1).
        #[cfg(target_os = "macos")]
        crate::macos::forget_marked(view);
    }
}

/// The last state recorded for `view`.
pub fn text_input_state(view: ViewId) -> Option<TextInputMirror> {
    let all = TEXT_INPUT.lock().unwrap_or_else(|e| e.into_inner());
    all.iter().find(|(v, _)| *v == view).map(|(_, s)| *s)
}

/// An input as the app drains it: `seq` is its place in arrival order
/// across every kind and every view, `at` when the view recorded it (for
/// a coalesced burst, its first event), and `view` the content view it
/// was delivered to.
#[derive(Debug, Clone, PartialEq)]
pub struct InputEvent {
    pub seq: u64,
    pub at: Instant,
    pub view: ViewId,
    pub kind: InputKind,
}

/// The queue itself; one process-wide instance behind [`push_input`] and
/// [`drain_input`], and a value for tests.
#[derive(Debug, Default)]
pub struct InputQueue {
    events: Vec<InputEvent>,
    next_seq: u64,
}

impl InputQueue {
    /// Record `kind` for `view`. Returns the event's `seq`.
    ///
    /// Coalescing, within one view: a pointer move replaces the queued
    /// pointer move (the page needs where the pointer is, not every point
    /// it crossed since the last turn), and a wheel is summed into the
    /// queued wheel (a trackpad flick is ~50 events a second and the page
    /// needs where it ends up). A move and a wheel commute: a wheel turned
    /// while the pointer drifts queues as `W M W M`, and a drain must still
    /// cost the page one hover and one scroll, not one of each per event
    /// (#693 review). Anything else between them (a press, a release, the
    /// pointer leaving, a key, another view's event) keeps both sides apart,
    /// so a click after a burst stays after it.
    pub fn push(&mut self, view: ViewId, kind: InputKind) -> u64 {
        if let Some(at) = self.coalesces_into(view, &kind) {
            let queued = &mut self.events[at];
            match (&mut queued.kind, &kind) {
                (InputKind::Pointer(prev), InputKind::Pointer(next)) => *prev = *next,
                (InputKind::Wheel(prev), InputKind::Wheel(next)) => {
                    prev.dx += next.dx;
                    prev.dy += next.dy;
                    prev.x = next.x;
                    prev.y = next.y;
                }
                _ => unreachable!("coalesces_into pairs like with like"),
            }
            // `at` stays the first event's: input-to-present is measured
            // from when the user acted, not from the last tick of a burst
            // queued while the window was held.
            return queued.seq;
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        self.events.push(InputEvent { seq, at: Instant::now(), view, kind });
        seq
    }

    /// Where `kind` merges, if anywhere: the nearest queued event of its own
    /// kind for `view`, looking back past that view's moves and wheels only.
    fn coalesces_into(&self, view: ViewId, kind: &InputKind) -> Option<usize> {
        fn is_move(kind: &InputKind) -> bool {
            matches!(kind, InputKind::Pointer(p) if p.input == PointerInput::Move)
        }
        fn is_wheel(kind: &InputKind) -> bool {
            matches!(kind, InputKind::Wheel(_))
        }
        if !is_move(kind) && !is_wheel(kind) {
            return None;
        }
        for (at, queued) in self.events.iter().enumerate().rev() {
            if queued.view != view || !(is_move(&queued.kind) || is_wheel(&queued.kind)) {
                return None;
            }
            if is_move(&queued.kind) == is_move(kind) {
                return Some(at);
            }
        }
        None
    }

    /// Take everything recorded so far, in arrival (`seq`) order across
    /// every kind and view. Until 2026-10-14 the app drained three queues
    /// one after another (clicks, then wheels, then keys), so a click made
    /// after a wheel burst reached the page before the burst, and a key
    /// typed before a click after it (#575).
    pub fn drain(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.events)
    }

    /// Take the recorded events of one kind, leaving the others queued (for
    /// a caller that only handles that kind; the app drains everything).
    pub fn drain_kind(&mut self, keep: impl Fn(&InputKind) -> bool) -> Vec<InputEvent> {
        let (taken, kept): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.events).into_iter().partition(|e| keep(&e.kind));
        self.events = kept;
        taken
    }

    /// Take the recorded events of one view, in arrival order, leaving the
    /// other views' events queued: the RustKit chrome's input is handled
    /// while the page's waits for its load (CH3.A2 step 3b).
    pub fn drain_view(&mut self, view: ViewId) -> Vec<InputEvent> {
        let (taken, kept): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.events).into_iter().partition(|e| e.view == view);
        self.events = kept;
        taken
    }

    /// Drop what is queued for `view`: the view is gone, and an input
    /// recorded for it must not reach whatever view comes next.
    pub fn retire(&mut self, view: ViewId) {
        self.events.retain(|e| e.view != view);
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

static INPUT: Mutex<InputQueue> = Mutex::new(InputQueue { events: Vec::new(), next_seq: 0 });

/// Record an input for `view` (called from the content view's handlers).
pub fn push_input(view: ViewId, kind: InputKind) -> u64 {
    INPUT.lock().map(|mut q| q.push(view, kind)).unwrap_or(0)
}

/// Everything recorded since the last drain, for the app's loop turn.
pub fn drain_input() -> Vec<InputEvent> {
    INPUT.lock().map(|mut q| q.drain()).unwrap_or_default()
}

/// The recorded events of one kind, leaving the others queued.
pub fn drain_input_kind(keep: impl Fn(&InputKind) -> bool) -> Vec<InputEvent> {
    INPUT.lock().map(|mut q| q.drain_kind(keep)).unwrap_or_default()
}

/// The recorded events of one view, leaving the other views' queued.
pub fn drain_input_view(view: ViewId) -> Vec<InputEvent> {
    INPUT.lock().map(|mut q| q.drain_view(view)).unwrap_or_default()
}

/// Drop what is queued for a view that no longer exists.
pub fn retire_view_input(view: ViewId) {
    if let Ok(mut q) = INPUT.lock() {
        q.retire(view);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(dy: f64) -> InputKind {
        InputKind::Wheel(PendingScroll { x: 10.0, y: 10.0, dx: 0.0, dy })
    }

    fn click(down: bool) -> InputKind {
        InputKind::Pointer(PendingClick { x: 5.0, y: 5.0, down, input: PointerInput::Button, clicks: 1, shift: false })
    }

    fn mv(x: f64) -> InputKind {
        InputKind::Pointer(PendingClick { x, y: 5.0, down: false, input: PointerInput::Move, clicks: 1, shift: false })
    }

    fn key(text: &str) -> InputKind {
        InputKind::Key(PendingKey {
            text: text.into(),
            mac_keycode: 0,
            up: false,
            ctrl: false,
            cmd: false,
            shift: false,
            alt: false,
        })
    }

    fn kinds(events: &[InputEvent]) -> Vec<&'static str> {
        events
            .iter()
            .map(|e| match e.kind {
                InputKind::Pointer(_) => "pointer",
                InputKind::Wheel(_) => "wheel",
                InputKind::Key(_) => "key",
                _ => "other",
            })
            .collect()
    }

    #[test]
    fn a_click_after_a_wheel_burst_is_delivered_after_it() {
        let view = ViewId::new();
        let mut q = InputQueue::default();
        for _ in 0..5 {
            q.push(view, wheel(-10.0));
        }
        q.push(view, click(true));
        q.push(view, click(false));
        let out = q.drain();
        assert_eq!(kinds(&out), ["wheel", "pointer", "pointer"], "the burst is one wheel, then the click");
        assert!(out.windows(2).all(|w| w[0].seq < w[1].seq), "seq order");
        match out[0].kind {
            InputKind::Wheel(w) => assert_eq!(w.dy, -50.0, "the burst was summed"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn a_key_typed_before_a_click_is_delivered_before_it() {
        let view = ViewId::new();
        let mut q = InputQueue::default();
        q.push(view, key("a"));
        q.push(view, click(true));
        q.push(view, click(false));
        q.push(view, key("b"));
        assert_eq!(kinds(&q.drain()), ["key", "pointer", "pointer", "key"]);
    }

    #[test]
    fn draining_one_view_leaves_the_other_views_input_queued() {
        let chrome = ViewId::new();
        let page = ViewId::new();
        let mut q = InputQueue::default();
        q.push(page, click(true));
        q.push(chrome, key("a"));
        q.push(page, click(false));
        q.push(chrome, key("b"));
        let taken = q.drain_view(chrome);
        assert_eq!(kinds(&taken), ["key", "key"], "the chrome's two keys");
        assert!(taken.iter().all(|e| e.view == chrome));
        assert!(taken[0].seq < taken[1].seq, "in arrival order");
        let rest = q.drain();
        assert_eq!(kinds(&rest), ["pointer", "pointer"], "the page's click is still queued");
        assert!(rest.iter().all(|e| e.view == page));
    }

    #[test]
    fn a_wheel_between_two_clicks_keeps_its_place() {
        let view = ViewId::new();
        let mut q = InputQueue::default();
        q.push(view, click(true));
        q.push(view, click(false));
        q.push(view, wheel(-10.0));
        q.push(view, wheel(-10.0));
        q.push(view, click(true));
        q.push(view, click(false));
        q.push(view, wheel(-5.0));
        let out = q.drain();
        assert_eq!(
            kinds(&out),
            ["pointer", "pointer", "wheel", "pointer", "pointer", "wheel"],
            "wheels sum only with the wheel right before them"
        );
    }

    #[test]
    fn moves_coalesce_only_when_adjacent_and_in_the_same_view() {
        let a = ViewId::new();
        let b = ViewId::new();
        let mut q = InputQueue::default();
        q.push(a, mv(1.0));
        q.push(a, mv(2.0));
        q.push(a, click(true));
        q.push(a, mv(3.0));
        q.push(b, mv(4.0));
        let out = q.drain();
        assert_eq!(out.len(), 4, "two moves replaced, one click, one move per view");
        match out[0].kind {
            InputKind::Pointer(p) => assert_eq!(p.x, 2.0, "the later move replaced the earlier"),
            _ => unreachable!(),
        }
        assert_eq!(out[3].view, b);
    }

    #[test]
    fn a_wheel_burst_interleaved_with_moves_is_one_wheel_and_one_move() {
        // A wheel turned while the pointer drifts: `W M W M W M`. The page
        // must hear one scroll (the sum) and one hover (the last point).
        let view = ViewId::new();
        let mut q = InputQueue::default();
        for step in 1..=3 {
            q.push(view, wheel(-10.0));
            q.push(view, mv(step as f64));
        }
        let out = q.drain();
        assert_eq!(kinds(&out), ["wheel", "pointer"], "one wheel, one move");
        match out[0].kind {
            InputKind::Wheel(w) => assert_eq!(w.dy, -30.0, "the burst was summed across the moves"),
            _ => unreachable!(),
        }
        match out[1].kind {
            InputKind::Pointer(p) => assert_eq!(p.x, 3.0, "the last move wins"),
            _ => unreachable!(),
        }
        assert!(out[0].seq < out[1].seq, "the wheel keeps its earlier place");
    }

    #[test]
    fn a_coalesced_burst_keeps_the_arrival_time_of_its_first_event() {
        // The W-gate's input-to-present figure is measured from `at`. A
        // trackpad flick queued while the window is held coalesces into one
        // wheel; it must be timed from the first tick, not the last (#704
        // review), or a 600 ms hold reads as a few ms.
        let view = ViewId::new();
        let mut q = InputQueue::default();
        q.push(view, wheel(-10.0));
        q.push(view, mv(1.0));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let later = Instant::now();
        q.push(view, wheel(-10.0));
        q.push(view, mv(2.0));
        let out = q.drain();
        assert_eq!(kinds(&out), ["wheel", "pointer"]);
        assert!(out[0].at < later, "the wheel burst is timed from its first tick");
        assert!(out[1].at < later, "the move burst is timed from its first move");
    }

    #[test]
    fn a_click_or_another_view_ends_the_coalescing_of_moves_and_wheels() {
        let view = ViewId::new();
        let other = ViewId::new();
        let mut q = InputQueue::default();
        q.push(view, wheel(-10.0));
        q.push(view, mv(1.0));
        q.push(view, click(true));
        q.push(view, wheel(-5.0));
        assert_eq!(kinds(&q.drain()), ["wheel", "pointer", "pointer", "wheel"], "a press keeps the sides apart");
        q.push(view, wheel(-10.0));
        q.push(other, mv(1.0));
        q.push(view, wheel(-5.0));
        let out = q.drain();
        assert_eq!(kinds(&out), ["wheel", "pointer", "wheel"], "another view's move keeps the sides apart");
        q.push(view, mv(1.0));
        q.push(view, InputKind::Pointer(PendingClick { x: 0.0, y: 0.0, down: false, input: PointerInput::Leave, clicks: 1, shift: false }));
        q.push(view, mv(2.0));
        assert_eq!(q.drain().len(), 3, "the pointer leaving keeps the sides apart");
    }

    #[test]
    fn a_wheel_queued_for_a_gone_view_is_dropped() {
        let gone = ViewId::new();
        let live = ViewId::new();
        let mut q = InputQueue::default();
        q.push(gone, wheel(-10.0));
        q.push(live, wheel(-20.0));
        q.push(gone, click(true));
        q.retire(gone);
        let out = q.drain();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].view, live);
        assert!(q.is_empty());
    }

    #[test]
    fn drain_kind_leaves_the_rest_queued_in_order() {
        let view = ViewId::new();
        let mut q = InputQueue::default();
        q.push(view, key("a"));
        q.push(view, wheel(-10.0));
        q.push(view, key("b"));
        let keys = q.drain_kind(|k| matches!(k, InputKind::Key(_)));
        assert_eq!(keys.len(), 2);
        assert_eq!(q.len(), 1);
        assert_eq!(kinds(&q.drain()), ["wheel"]);
    }

    #[test]
    fn the_process_queue_is_shared_and_sequenced() {
        let view = ViewId::new();
        let first = push_input(view, key("x"));
        let second = push_input(view, click(true));
        assert!(second > first);
        retire_view_input(view);
        assert!(drain_input().iter().all(|e| e.view != view));
    }
}
