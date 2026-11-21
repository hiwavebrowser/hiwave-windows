//! Cmd chords offered to the page first, and how long they wait for it (M1e).
//!
//! A Cmd chord Chromium does not reserve goes to the page's keydown before
//! the menu (CH2.E3): the view holds the chord, the page answers whether it
//! claimed it, and an unclaimed one goes on to the menu. The page answers
//! from its own thread, which a load can hold for seconds (an image the
//! server held 8 s kept Cmd+L from the address bar for 6.4 s, M1d #1012).
//! So a chord waits [`CHORD_ANSWER_TIMEOUT`] at most: one the page has not
//! answered by then is unclaimed and goes to the menu, and the page's late
//! answer, claimed or not, is dropped. The menu runs once, and a claim
//! that comes too late does not take the chord back.
//!
//! One actor (M1g, #1015 delta R1 5835356948): when the page's thread
//! reaches a chord whose wait already ran out, it does not dispatch the
//! chord's keydown at all ([`HeldChords::begin`]). Otherwise a page with
//! its own handler for the chord (a web app that switches panes on Cmd+2)
//! would act as well as the menu, and after a Cmd+digit tab switch it would
//! act in the tab the menu just left. This is a divergence from Chrome on
//! the timeout path only: Chrome has no timeout and always delivers the
//! keydown before the menu sees the chord. The cost is that a page's
//! observe-only listener misses a key the user pressed while its thread was
//! held for more than 100 ms. A chord whose keydown the page has started
//! is the page's to answer: its wait running out then sends nothing to the
//! menu, and the page's answer (claimed or not) decides, as in Chrome.
//!
//! Two bounds on that (#1023 R1): a started chord that is never answered
//! (the page's thread died, the view was torn down mid-dispatch) is given
//! up after [`STARTED_CHORD_GIVE_UP`] and goes to the menu, so it neither
//! stays held nor leaks its `NSEvent`; a listener that just runs long is
//! stopped first by the app's 5 s live turn budget, whose end lets the
//! page answer. And later chords keep their order: one whose wait runs out
//! while an earlier chord's keydown is still running in the page (it
//! cannot have been reached) waits for that chord's answer and then goes
//! to the menu ([`HeldChords::release_due`]), rather than running ahead of
//! it.
//!
//! Why 100 ms: it is the response budget a press has before the delay is
//! felt (RAIL: respond to input within 100 ms), and far above what an idle
//! page thread takes to run a keydown listener and answer (the in-time
//! cases in `tests/macos_chord_wait.rs` answer in 10-20 ms). A page whose
//! thread is free keeps every chord it cancels, as in Chrome; only a page
//! whose thread is held past the budget loses it to the menu. The
//! real-window gate (`m1d_cmd_l_while_loading`) allows 250 ms from the
//! press to the address bar, the timeout plus the menu action and the
//! chrome's focus.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a Cmd chord waits for the page to say whether it claimed it.
pub const CHORD_ANSWER_TIMEOUT: Duration = Duration::from_millis(100);

/// How long a chord whose keydown the page has started ([`HeldChords::begin`])
/// waits for the page's answer before the view gives up on it
/// ([`HeldChords::give_up`]) and sends it to the menu. The answer normally
/// comes as soon as the listeners return, and the app stops an input
/// event's listeners at its live turn budget (5 s, `LIVE_TURN_BUDGET_MS` in
/// hiwave-app), so this is set past that: it is for a page that never
/// answers at all (its thread died, its view was torn down mid-dispatch),
/// whose chord would otherwise stay held and its `NSEvent` retained for
/// good (#1023 R1 item 1).
pub const STARTED_CHORD_GIVE_UP: Duration = Duration::from_secs(6);

/// What became of a held chord.
#[derive(Debug, PartialEq, Eq)]
pub enum ChordFate<E> {
    /// The page did not claim it, or did not answer in time: to the menu.
    ToMenu(E),
    /// The page claimed it in time: the page keeps it.
    PageKept(E),
}

/// How many timed-out chords are remembered one by one until the page's
/// late answer comes for them. One that falls out is still known to have
/// timed out ([`HeldChords::forgotten_through`]).
const RAN_OUT_KEPT: usize = 64;

#[derive(Debug)]
struct Held<E> {
    token: u64,
    event: E,
    /// When the view took it and asked the page.
    taken: Instant,
    /// The page has started the chord's own default ([`HeldChords::claim`]).
    page_has_it: bool,
    /// The page has started the chord's keydown ([`HeldChords::begin`]).
    page_started: bool,
    /// Its wait ran out while an earlier chord's keydown was still running
    /// in the page: it goes to the menu after that chord is answered
    /// ([`HeldChords::release_due`]), keeping the presses in order.
    deferred: bool,
}

/// The chords the view holds for the page, by token, with when each was
/// taken. `E` is the native event (a retained `NSEvent` on macOS).
///
/// A chord has one owner. While it is held the page may take it
/// ([`claim`](Self::claim), then its answer); once its wait has run out the
/// menu has it, and the page, when it gets to the chord, is told so and
/// leaves the chord's default alone (R1 nit 1: a Cmd+V that timed out was
/// pasted by the menu's Paste and then again by the page's late keydown).
#[derive(Debug)]
pub struct HeldChords<E> {
    held: Vec<Held<E>>,
    /// Tokens whose wait ran out, until the page's answer for them comes.
    ran_out: VecDeque<u64>,
    /// The highest token that fell out of `ran_out` (0: none). The view
    /// issues tokens in increasing order and holds each one before the page
    /// can hear of it, so a token at or below this mark that is not held
    /// either timed out or was answered; the page asks about a token only
    /// before it answers, so for the page it timed out. Forgetting fails
    /// safe (#1015 delta R1 (b)): a page more than `RAN_OUT_KEPT` timed-out
    /// chords behind skips them, as it skips a remembered one. A mark, not
    /// an age bound: the page thread can be held for any length of time
    /// (an 8 s image), and no age is safe to forget at, while the mark
    /// costs one integer and needs no clock.
    forgotten_through: u64,
}

impl<E> Default for HeldChords<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> HeldChords<E> {
    pub const fn new() -> Self {
        Self { held: Vec::new(), ran_out: VecDeque::new(), forgotten_through: 0 }
    }

    /// The view took `event` for the page at `at`, the moment it asks the
    /// page. The wait is counted from here and not from the key event's own
    /// stamp (R1 nit 3): the 100 ms is the page's time to answer. A press
    /// that reached the view late (the main thread was busy for 150 ms)
    /// would otherwise arrive with its wait already spent, and a page that
    /// cancels the chord would lose it to the menu without having been
    /// asked. `since_press_ms` in the log is the other clock, press to
    /// menu, and is what the real-window gate reads.
    pub fn hold(&mut self, token: u64, event: E, at: Instant) {
        self.held.push(Held { token, event, taken: at, page_has_it: false, page_started: false, deferred: false });
    }

    /// When `token`'s wait ends, while it is held. The timer is armed for
    /// this.
    pub fn deadline(&self, token: u64) -> Option<Instant> {
        self.held.iter().find(|h| h.token == token).map(|h| h.taken + CHORD_ANSWER_TIMEOUT)
    }

    /// The page asks, from its own thread and after the chord's keydown
    /// went uncancelled, whether the chord's default (the edit in a text
    /// field) is still its to run. Held: yes, and from here the chord is
    /// the page's; if the wait runs out before the answer arrives, it does
    /// not go to the menu. Timed out: no, the menu has it. A token this
    /// view never held (a test, an embedder with no menu) is the page's.
    pub fn claim(&mut self, token: u64) -> bool {
        if let Some(held) = self.held.iter_mut().find(|h| h.token == token) {
            held.page_has_it = true;
            return true;
        }
        !self.timed_out(token)
    }

    /// The page, from its own thread, is about to dispatch the chord's
    /// keydown: whether it may. Held: yes, and from here the page answers
    /// for the chord; its wait running out no longer sends it to the menu.
    /// Timed out (remembered or forgotten): no, the menu has it and the
    /// page skips the keydown and the default (M1g: one actor). A token
    /// this view never held is the page's.
    pub fn begin(&mut self, token: u64) -> bool {
        if let Some(held) = self.held.iter_mut().find(|h| h.token == token) {
            held.page_started = true;
            return true;
        }
        !self.timed_out(token)
    }

    /// `token` is not held and went to the menu at its deadline.
    fn timed_out(&self, token: u64) -> bool {
        self.ran_out.contains(&token) || token <= self.forgotten_through
    }

    /// The page's answer for `token`, with how long it waited. `None`: the
    /// chord is no longer held (its wait ran out and it went to the menu),
    /// so the answer is dropped.
    pub fn answer(&mut self, token: u64, claimed: bool, now: Instant) -> Option<(ChordFate<E>, Duration)> {
        let Some(at) = self.held.iter().position(|h| h.token == token) else {
            self.ran_out.retain(|t| *t != token);
            return None;
        };
        let held = self.held.remove(at);
        let waited = now.saturating_duration_since(held.taken);
        let fate = if claimed || held.page_has_it { ChordFate::PageKept(held.event) } else { ChordFate::ToMenu(held.event) };
        Some((fate, waited))
    }

    /// `token`'s wait ran out. If the page has not answered, the chord goes
    /// to the menu, unless the page already took it ([`claim`](Self::claim))
    /// and its answer is on the way: then the page keeps it. `None`: the
    /// page answered first, or it is running the chord's keydown
    /// ([`begin`](Self::begin)) and its answer, which comes as soon as the
    /// listeners return, releases the chord. The timer that calls this was armed for the
    /// deadline; it does not check the clock, so a timer that fires a hair
    /// early still releases the chord.
    pub fn expire(&mut self, token: u64) -> Option<ChordFate<E>> {
        let at = self.held.iter().position(|h| h.token == token)?;
        // The page is running the chord's keydown: its answer decides (or,
        // if it never comes, `give_up`).
        if self.held[at].page_started {
            return None;
        }
        // An earlier chord's keydown is still running in the page, so the
        // page cannot have reached this one. Sending it to the menu now
        // would run it ahead of the earlier chord's answer (Cmd+L with a
        // slow listener, then Cmd+D: the bookmark before the address bar,
        // #1023 R1 item 2). It waits for that answer; Chrome too handles a
        // window's key events one after the other.
        if self.held[..at].iter().any(|h| h.page_started) {
            self.held[at].deferred = true;
            return None;
        }
        let held = self.held.remove(at);
        if held.page_has_it {
            return Some(ChordFate::PageKept(held.event));
        }
        self.remember_ran_out(token);
        Some(ChordFate::ToMenu(held.event))
    }

    /// When `token` is given up on if the page never answers it: the timer
    /// for [`give_up`](Self::give_up) is armed for this when the chord's
    /// 100 ms timer finds it still held, whether the page has started it
    /// or it is held back behind an earlier started chord. A held-back
    /// chord the page starts later has no other timer left (#1023 delta
    /// HOLD 5836613705: without this it stayed held for good, and every
    /// later timed-out chord waited behind it). `None`: not held, or
    /// neither started nor held back.
    pub fn give_up_at(&self, token: u64) -> Option<Instant> {
        self.held
            .iter()
            .find(|h| h.token == token && (h.page_started || h.deferred))
            .map(|h| h.taken + STARTED_CHORD_GIVE_UP)
    }

    /// The page started `token`'s keydown and has not answered in
    /// [`STARTED_CHORD_GIVE_UP`]: the chord goes to the menu (or stays the
    /// page's if it took the default), and a late answer or claim for it
    /// is refused like a timed-out chord's. `None`: it was answered, or it
    /// is held back and not started, which `release_due` releases when the
    /// earlier chord it waits behind is answered or given up (that chord
    /// was taken first, so its give-up comes first). Call
    /// [`release_due`](Self::release_due) after it.
    pub fn give_up(&mut self, token: u64) -> Option<ChordFate<E>> {
        let at = self.held.iter().position(|h| h.token == token && h.page_started)?;
        let held = self.held.remove(at);
        if held.page_has_it {
            return Some(ChordFate::PageKept(held.event));
        }
        self.remember_ran_out(token);
        Some(ChordFate::ToMenu(held.event))
    }

    /// Chords whose wait ran out behind an earlier started chord
    /// ([`expire`](Self::expire)) and that nothing earlier holds back any
    /// more, oldest first: to the menu. Call after an answer or a give-up.
    pub fn release_due(&mut self) -> Vec<(u64, ChordFate<E>)> {
        let mut out = Vec::new();
        loop {
            let Some(at) = self.held.iter().position(|h| h.deferred && !h.page_started) else {
                break;
            };
            if self.held[..at].iter().any(|h| h.page_started) {
                break;
            }
            let held = self.held.remove(at);
            self.remember_ran_out(held.token);
            out.push((held.token, ChordFate::ToMenu(held.event)));
        }
        out
    }

    fn remember_ran_out(&mut self, token: u64) {
        if self.ran_out.len() == RAN_OUT_KEPT {
            if let Some(forgotten) = self.ran_out.pop_front() {
                self.forgotten_through = self.forgotten_through.max(forgotten);
            }
        }
        self.ran_out.push_back(token);
    }

    pub fn len(&self) -> usize {
        self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    /// The main thread's loop against a fake page thread that answers
    /// after `after` (or never): page answers are taken as they come, and
    /// the chord's wait runs out at its deadline. What the menu got, and
    /// whether the page kept it.
    fn drive(claimed: bool, after: Option<Duration>) -> (Vec<&'static str>, Vec<&'static str>, Duration) {
        let mut chords = HeldChords::new();
        let start = Instant::now();
        chords.hold(7, "cmd-l", start);
        let deadline = chords.deadline(7).expect("held");
        let (tx, rx) = mpsc::channel::<(u64, bool)>();
        thread::spawn(move || {
            if let Some(after) = after {
                thread::sleep(after);
                let _ = tx.send((7, claimed));
            } else {
                thread::sleep(Duration::from_secs(2));
            }
        });
        let (mut menu, mut kept) = (Vec::new(), Vec::new());
        let mut menu_at = Duration::ZERO;
        let end = start + after.unwrap_or_default().max(CHORD_ANSWER_TIMEOUT) + Duration::from_millis(150);
        while Instant::now() < end {
            while let Ok((token, claimed)) = rx.try_recv() {
                match chords.answer(token, claimed, Instant::now()) {
                    Some((ChordFate::ToMenu(e), _)) => {
                        menu.push(e);
                        menu_at = start.elapsed();
                    }
                    Some((ChordFate::PageKept(e), _)) => kept.push(e),
                    None => {}
                }
            }
            if Instant::now() >= deadline {
                if let Some(ChordFate::ToMenu(e)) = chords.expire(7) {
                    menu.push(e);
                    menu_at = start.elapsed();
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(chords.is_empty(), "nothing left held");
        (menu, kept, menu_at)
    }

    #[test]
    fn answered_in_time_unclaimed_goes_to_the_menu_once() {
        let (menu, kept, at) = drive(false, Some(Duration::from_millis(5)));
        assert_eq!(menu, ["cmd-l"]);
        assert!(kept.is_empty());
        assert!(at < CHORD_ANSWER_TIMEOUT, "at the answer, not the deadline: {at:?}");
    }

    #[test]
    fn answered_in_time_claimed_the_page_keeps_it() {
        // Chromium: IDC_FOCUS_LOCATION / IDC_SELECT_TAB_* are not reserved.
        let (menu, kept, _) = drive(true, Some(Duration::from_millis(5)));
        assert!(menu.is_empty());
        assert_eq!(kept, ["cmd-l"]);
    }

    #[test]
    fn never_answered_goes_to_the_menu_at_the_deadline() {
        let (menu, kept, at) = drive(false, None);
        assert_eq!(menu, ["cmd-l"]);
        assert!(kept.is_empty());
        assert!(at >= CHORD_ANSWER_TIMEOUT && at < Duration::from_millis(250), "{at:?}");
    }

    #[test]
    fn a_late_unclaimed_answer_does_not_run_the_menu_again() {
        let (menu, kept, at) = drive(false, Some(Duration::from_millis(300)));
        assert_eq!(menu, ["cmd-l"], "once");
        assert!(kept.is_empty());
        assert!(at < Duration::from_millis(250), "at the deadline, not the answer: {at:?}");
    }

    #[test]
    fn a_late_claim_is_dropped() {
        let (menu, kept, _) = drive(true, Some(Duration::from_millis(300)));
        assert_eq!(menu, ["cmd-l"]);
        assert!(kept.is_empty(), "too late to take it back");
    }

    #[test]
    fn answer_and_expire_each_release_a_chord_once() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'a', t0);
        chords.hold(2, 'b', t0);
        assert_eq!(chords.deadline(2), Some(t0 + CHORD_ANSWER_TIMEOUT));
        assert_eq!(chords.expire(1), Some(ChordFate::ToMenu('a')));
        assert_eq!(chords.answer(1, false, t0), None, "late answer for 1 dropped");
        assert_eq!(chords.expire(1), None);
        let (fate, waited) = chords.answer(2, true, t0 + Duration::from_millis(30)).expect("held");
        assert_eq!(fate, ChordFate::PageKept('b'));
        assert_eq!(waited, Duration::from_millis(30));
        assert_eq!(chords.expire(2), None, "an answered chord's timer finds nothing");
        assert_eq!(chords.deadline(2), None);
        assert!(chords.is_empty());
    }

    // R1 nit 1: the chord's default (a paste, an undo) has one owner.

    #[test]
    fn a_timed_out_chord_is_not_the_pages_to_edit_with() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'v', t0);
        assert_eq!(chords.expire(1), Some(ChordFate::ToMenu('v')));
        assert!(!chords.claim(1), "the menu has it: the page's late keydown does not paste");
        assert!(!chords.claim(1), "and stays so until the page answers");
        assert_eq!(chords.answer(1, false, t0), None, "the late answer is dropped");
        assert!(chords.ran_out.is_empty(), "and the token is forgotten");
    }

    #[test]
    fn a_chord_the_page_took_in_time_does_not_go_to_the_menu_at_the_deadline() {
        // The page starts its paste at 99 ms; the timer fires before the
        // answer crosses back to the main thread.
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'v', t0);
        assert!(chords.claim(1), "held: the page's");
        assert_eq!(chords.expire(1), Some(ChordFate::PageKept('v')), "no menu run");
        assert_eq!(chords.answer(1, true, t0), None);
        assert!(chords.ran_out.is_empty());
        // And answered before the timer: kept, whatever the answer says.
        chords.hold(2, 'z', t0);
        assert!(chords.claim(2));
        assert_eq!(chords.answer(2, false, t0).map(|(f, _)| f), Some(ChordFate::PageKept('z')));
        assert_eq!(chords.expire(2), None);
    }

    #[test]
    fn a_token_the_view_never_held_is_the_pages() {
        let mut chords = HeldChords::<char>::new();
        assert!(chords.claim(99));
    }

    #[test]
    fn only_so_many_timed_out_tokens_are_remembered() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        for token in 0..(RAN_OUT_KEPT as u64 + 5) {
            chords.hold(token, (), t0);
            chords.expire(token);
        }
        assert_eq!(chords.ran_out.len(), RAN_OUT_KEPT);
        assert!(!chords.claim(RAN_OUT_KEPT as u64 + 4), "the newest is remembered");
    }

    // M1g: one actor. The page does not hear a chord the menu has.

    #[test]
    fn a_timed_out_chord_is_not_the_pages_to_hear() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'v', t0);
        assert_eq!(chords.expire(1), Some(ChordFate::ToMenu('v')));
        assert!(!chords.begin(1), "the menu has it: no keydown");
        assert!(!chords.claim(1), "and no edit");
        assert_eq!(chords.answer(1, false, t0), None);
    }

    #[test]
    fn a_chord_whose_keydown_has_started_waits_for_the_pages_answer() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        // The page started the keydown at 99 ms, its listener is running
        // when the wait runs out: nothing goes to the menu at the deadline.
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        assert_eq!(chords.expire(1), None, "no menu run while the page is on it");
        assert_eq!(chords.len(), 1, "still held for the answer");
        // Not cancelled, no edit default (Cmd+L): the answer sends it to the
        // menu, after the keydown, as in Chrome. Once.
        assert_eq!(chords.answer(1, false, t0).map(|(f, _)| f), Some(ChordFate::ToMenu('l')));
        assert_eq!(chords.expire(1), None);
        // Cancelled by the page: the page keeps it.
        chords.hold(2, 'l', t0);
        assert!(chords.begin(2));
        assert_eq!(chords.expire(2), None);
        assert_eq!(chords.answer(2, true, t0).map(|(f, _)| f), Some(ChordFate::PageKept('l')));
        // Started, then took the default, then the wait ran out: the page's,
        // released by its answer.
        chords.hold(3, 'v', t0);
        assert!(chords.begin(3));
        assert!(chords.claim(3));
        assert_eq!(chords.expire(3), None);
        assert_eq!(chords.answer(3, true, t0).map(|(f, _)| f), Some(ChordFate::PageKept('v')));
        assert!(chords.is_empty());
        assert!(chords.ran_out.is_empty(), "none of these timed out");
    }

    #[test]
    fn a_forgotten_timed_out_token_is_not_the_pages_to_hear() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        for token in 1..=(RAN_OUT_KEPT as u64 + 1) {
            chords.hold(token, (), t0);
            chords.expire(token);
        }
        assert!(!chords.begin(1), "forgotten, still timed out");
        assert!(chords.begin(RAN_OUT_KEPT as u64 + 2), "a later token the view never held is the page's");
    }

    // #1023 R1 item 1: a started chord that is never answered is released.

    #[test]
    fn a_started_chord_never_answered_is_given_up_to_the_menu() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        assert_eq!(chords.expire(1), None, "the page is on it at 100 ms");
        assert_eq!(chords.len(), 1, "held for the answer");
        assert_eq!(chords.give_up_at(1), Some(t0 + STARTED_CHORD_GIVE_UP));
        // No answer ever comes (the page's thread died).
        assert_eq!(chords.give_up(1), Some(ChordFate::ToMenu('l')), "given up: to the menu, once");
        assert!(chords.is_empty(), "nothing stays held");
        assert_eq!(chords.give_up(1), None);
        assert!(!chords.claim(1), "a late claim is refused");
        assert_eq!(chords.answer(1, true, t0), None, "and a late answer dropped");
        // A chord the page answered is not given up.
        chords.hold(2, 'l', t0);
        assert!(chords.begin(2));
        assert!(chords.answer(2, false, t0).is_some());
        assert_eq!(chords.give_up_at(2), None);
        assert_eq!(chords.give_up(2), None);
    }

    // #1023 R1 item 2: presses reach the menu in order.

    #[test]
    fn a_chord_behind_a_started_one_waits_for_its_answer() {
        // Cmd+L reaches the page in time, its listener runs long; Cmd+D
        // comes 30 ms later and cannot be reached while the listener runs.
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        chords.hold(2, 'd', t0 + Duration::from_millis(30));
        assert_eq!(chords.expire(1), None);
        assert_eq!(chords.expire(2), None, "Cmd+D does not run ahead of Cmd+L's answer");
        assert!(chords.release_due().is_empty(), "nothing due while Cmd+L's listener runs");
        // Cmd+L's listener returns without cancelling: Cmd+L to the menu,
        // then Cmd+D.
        assert_eq!(chords.answer(1, false, t0).map(|(f, _)| f), Some(ChordFate::ToMenu('l')));
        assert_eq!(chords.release_due(), vec![(2, ChordFate::ToMenu('d'))]);
        assert!(chords.is_empty());
        assert!(!chords.begin(2), "Cmd+D went to the menu: the page does not hear it");
    }

    #[test]
    fn a_waiting_chord_the_page_reaches_is_the_pages() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        chords.hold(2, 'd', t0);
        assert_eq!(chords.expire(2), None, "waits behind Cmd+L");
        // The page answers Cmd+L and reaches Cmd+D before the answer
        // crosses to the main thread.
        assert!(chords.begin(2), "still held: the page hears it");
        assert_eq!(chords.answer(1, true, t0).map(|(f, _)| f), Some(ChordFate::PageKept('l')));
        assert!(chords.release_due().is_empty(), "Cmd+D is the page's to answer");
        assert_eq!(chords.answer(2, false, t0).map(|(f, _)| f), Some(ChordFate::ToMenu('d')));
    }

    #[test]
    fn a_give_up_releases_the_chords_waiting_behind_it() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        chords.hold(2, 'd', t0);
        chords.hold(3, 't', t0);
        assert_eq!(chords.expire(2), None);
        assert_eq!(chords.expire(3), None);
        assert_eq!(chords.give_up(1), Some(ChordFate::ToMenu('l')));
        assert_eq!(chords.release_due(), vec![(2, ChordFate::ToMenu('d')), (3, ChordFate::ToMenu('t'))], "in press order");
        assert!(chords.is_empty());
    }

    /// #1023 delta HOLD 5836613705: a chord held back behind a started one,
    /// which the page then starts and never answers, must still be given
    /// up at its own deadline; otherwise it stays held for good and every
    /// later timed-out chord waits behind it (Cmd shortcuts dead app-wide).
    #[test]
    fn a_deferred_chord_the_page_starts_and_never_answers_is_given_up() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        // A: started, its listener slow.
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        assert_eq!(chords.expire(1), None);
        // B: its wait runs out behind A, so it is held back. Its give-up
        // timer is armed now, when its 100 ms timer fires.
        chords.hold(2, 'd', t0 + Duration::from_millis(30));
        assert_eq!(chords.expire(2), None, "B waits behind A");
        assert_eq!(
            chords.give_up_at(2),
            Some(t0 + Duration::from_millis(30) + STARTED_CHORD_GIVE_UP),
            "the 100 ms timer finds a give-up deadline for the held-back chord"
        );
        // A's listener returns; the page reaches B before A's answer crosses.
        assert!(chords.begin(2), "B is the page's");
        assert_eq!(chords.answer(1, false, t0).map(|(f, _)| f), Some(ChordFate::ToMenu('l')));
        assert!(chords.release_due().is_empty(), "B is started: the page answers it");
        // B is never answered (the page's thread died). At B's give-up:
        assert_eq!(chords.give_up(2), Some(ChordFate::ToMenu('d')), "B given up, once");
        assert!(chords.is_empty(), "nothing stays held");
        // A later chord is not held back by anything.
        chords.hold(3, 't', t0 + Duration::from_secs(7));
        assert_eq!(chords.expire(3), Some(ChordFate::ToMenu('t')), "C still reaches the menu at its deadline");
    }

    /// A held-back chord whose give-up fires while it is still not started
    /// is left to `release_due` (its blocker's answer or give-up, which
    /// comes first, releases it).
    #[test]
    fn a_give_up_on_a_held_back_chord_not_started_waits_for_release_due() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        chords.hold(1, 'l', t0);
        assert!(chords.begin(1));
        chords.hold(2, 'd', t0);
        assert_eq!(chords.expire(2), None);
        assert_eq!(chords.give_up(2), None, "not started: not the give-up's");
        assert_eq!(chords.give_up(1), Some(ChordFate::ToMenu('l')));
        assert_eq!(chords.release_due(), vec![(2, ChordFate::ToMenu('d'))]);
    }

    /// #1015 delta R1 (b): a timed-out token that fell out of the memory is
    /// still the menu's. Forgetting fails safe: the page skips the edit.
    #[test]
    fn a_forgotten_timed_out_token_is_still_not_the_pages() {
        let mut chords = HeldChords::new();
        let t0 = Instant::now();
        for token in 1..=(RAN_OUT_KEPT as u64 + 5) {
            chords.hold(token, (), t0);
            chords.expire(token);
        }
        for token in 1..=5 {
            assert!(!chords.claim(token), "forgotten token {token} is not claimable");
        }
        assert!(chords.claim(RAN_OUT_KEPT as u64 + 100), "a token the view never held is still the page's");
    }
}
