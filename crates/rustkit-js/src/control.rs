//! Stopping a runaway script from another thread (threads plan M1,
//! invariant 6; the design is `THREAD_OWNERSHIP_2026-10.md` section 3,
//! "Interrupt word: lifecycle").
//!
//! Two control blocks, atomics only, so they are `Send + Sync`:
//!
//! - [`ThreadStamp`], one per page thread and written only by it, says which
//!   turn of script that thread is running, since when, for which view and
//!   with what budget.
//! - [`ViewControl`], one per view, holds the interrupt word the vendored
//!   Boa reads at every call and loop back-edge
//!   (`third_party/boa_engine/PATCHES.md`), and the host-call check reads
//!   too. A script stops when the word equals its turn id, or [`ALL`].
//!
//! The [`Watchdog`] is its own thread: before M4 the thread that would
//! otherwise notice an overrun is the one spinning. It sleeps while no turn
//! is open, wakes when one is published, and sets the word of a turn that
//! has run past its budget.
//!
//! Turn ids come from one counter, only grow and are never reused, so a
//! word naming a turn that has ended cannot stop the next one, and writers
//! never need to clear a turn-targeted word: every writer uses `fetch_max`.

use std::collections::HashMap;
use std::sync::atomic::{fence, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

/// The interrupt word's "every turn": this view runs no more script until
/// a new document clears it ([`ViewControl::clear_all`]).
pub const ALL: u64 = u64::MAX;

/// What is left of a turn after its script was stopped may run this long
/// (`JsRuntime`'s grace turn): the promise jobs the turn's finished
/// callbacks queued, and the embedder's own script.
pub const STOP_GRACE: Duration = Duration::from_millis(50);

/// The longest the watchdog sleeps while a turn is open. It wakes sooner
/// when an open turn's budget ends first.
const TICK: Duration = Duration::from_millis(250);

/// The next turn id. Starts at 1: 0 is "no turn" and "no interrupt".
static NEXT_TURN: AtomicU64 = AtomicU64::new(1);
static NEXT_VIEW: AtomicU64 = AtomicU64::new(1);

/// Microseconds since the first call, the clock turn start times are on.
/// Finer than the budgets so a turn is never judged by a rounded clock.
pub fn mono_us() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_micros() as u64
}

fn now_us() -> u64 {
    mono_us()
}

const SETTER_NONE: u8 = 0;
const SETTER_WATCHDOG: u8 = 1;
const SETTER_STOP_ALL: u8 = 2;
const SETTER_CANCEL: u8 = 3;
const SETTER_COMMIT: u8 = 4;
const SETTER_CLEAR: u8 = 5;

fn setter_name(tag: u8) -> &'static str {
    match tag {
        SETTER_WATCHDOG => "watchdog",
        SETTER_STOP_ALL => "stop_all",
        SETTER_CANCEL => "cancel",
        SETTER_COMMIT => "commit",
        SETTER_CLEAR => "clear",
        _ => "-",
    }
}

/// One view's stop state.
///
/// Created with the view and dropped with it. View ids are never reused,
/// so a new view never inherits an old view's [`ALL`].
#[derive(Debug)]
pub struct ViewControl {
    id: u64,
    /// A turn id to stop, or [`ALL`]; 0 = none. An `Arc` of its own
    /// because Boa holds it ([`boa_engine::Context::set_interrupt_word`]).
    interrupt: Arc<AtomicU64>,
    /// Cancel every load whose navigation id is at or below this; 0 =
    /// none. Only grows ([`ViewControl::cancel_nav`]).
    pub cancel_nav: AtomicU64,
    /// The view's newest navigation id; 0 = none yet (M3b).
    nav: AtomicU64,
    /// The navigation whose load is in progress; 0 = none.
    loading_nav: AtomicU64,
    /// The newest navigation whose document has replaced the view's; 0 =
    /// none. Until `loading_nav` commits, the word belongs to the previous
    /// document's script, which a cancel of the new load must not stop.
    committed_nav: AtomicU64,
    /// Held while `loading_nav` is read or written together with the
    /// interrupt word, so a cancel for a load that just ended cannot set
    /// [`ALL`] on the next document after it cleared the word.
    nav_lock: Mutex<()>,
    /// Bumped on Stop and Close, never lowered. Not read yet: navigations a
    /// stopped page raised are dropped by it from M4.
    pub nav_epoch: AtomicU64,
    /// Who last wrote [`Self::interrupt`], for the console-log line. The
    /// opcode check does not read these.
    setter_tag: AtomicU8,
    setter_at_us: AtomicU64,
    setter_target: AtomicU64,
}

impl ViewControl {
    fn new() -> Self {
        Self {
            id: NEXT_VIEW.fetch_add(1, Ordering::Relaxed),
            interrupt: Arc::new(AtomicU64::new(0)),
            cancel_nav: AtomicU64::new(0),
            nav: AtomicU64::new(0),
            loading_nav: AtomicU64::new(0),
            committed_nav: AtomicU64::new(0),
            nav_lock: Mutex::new(()),
            nav_epoch: AtomicU64::new(0),
            setter_tag: AtomicU8::new(SETTER_NONE),
            setter_at_us: AtomicU64::new(0),
            setter_target: AtomicU64::new(0),
        }
    }

    fn note_setter(&self, tag: u8, target: u64) {
        self.setter_target.store(target, Ordering::Relaxed);
        // 0 means "nobody has written". A write in the first microsecond
        // of the clock still has to be distinguishable from that.
        self.setter_at_us.store(now_us().max(1), Ordering::Relaxed);
        self.setter_tag.store(tag, Ordering::Release);
    }

    /// The word, and who last wrote it: tag, monotonic time, target turn.
    pub fn word_writer(&self) -> (u64, &'static str, u64, u64) {
        let tag = self.setter_tag.load(Ordering::Acquire);
        (
            self.interrupt.load(Ordering::Relaxed),
            setter_name(tag),
            self.setter_at_us.load(Ordering::Relaxed),
            self.setter_target.load(Ordering::Relaxed),
        )
    }

    /// This view's id in [`ThreadStamp`] (`turn_view`).
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Stop `turn` if it is this view's running turn. Never lowers the
    /// word: a later turn's id, or [`ALL`], stays.
    pub fn interrupt_turn(&self, turn: u64) {
        self.note_setter(SETTER_WATCHDOG, turn);
        self.interrupt.fetch_max(turn, Ordering::Relaxed);
    }

    /// Stop every turn of this view until [`ViewControl::clear_all`].
    pub fn stop_all(&self) {
        self.write_all(SETTER_STOP_ALL);
    }

    /// [`ViewControl::stop_all`] from a navigation cancel.
    pub fn stop_cancelled(&self) {
        self.write_all(SETTER_CANCEL);
    }

    fn write_all(&self, tag: u8) {
        self.note_setter(tag, ALL);
        self.interrupt.store(ALL, Ordering::Relaxed);
    }

    /// Lift [`ALL`]; done by the page thread only, when a new document
    /// commits in the view. A turn id in the word is left: it names a turn
    /// that has ended. Whether `ALL` was set.
    pub fn clear_all(&self) -> bool {
        let cleared = self
            .interrupt
            .compare_exchange(ALL, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok();
        if cleared {
            self.note_setter(SETTER_CLEAR, 0);
        }
        cleared
    }

    /// Whether the word stops `turn`.
    pub fn stops(&self, turn: u64) -> bool {
        let word = self.interrupt.load(Ordering::Relaxed);
        word != 0 && (word == ALL || word == turn)
    }

    /// Whether the word is [`ALL`]: the view runs no script until a new
    /// document commits (the window asked to leave this one, or stopped
    /// it). A turn opened now is stopped at its first opcode, so an
    /// embedder that can tell beforehand does not open one.
    pub fn stops_all(&self) -> bool {
        self.interrupt.load(Ordering::Relaxed) == ALL
    }

    /// Navigation `nav_id` has started and its load is in progress. Called
    /// by the page thread only, before the load's first await; ids grow.
    pub fn begin_nav(&self, nav_id: u64) {
        let _held = self.nav_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.nav.store(nav_id, Ordering::SeqCst);
        self.loading_nav.store(nav_id, Ordering::SeqCst);
    }

    /// Navigation `nav_id`'s document has replaced the view's: from here
    /// the word is its script's. Lifts [`ALL`], which ended with the
    /// previous document, unless `nav_id` was cancelled already, whose
    /// script then never starts. Called by the page thread only.
    pub fn commit_nav(&self, nav_id: u64) {
        let _held = self.nav_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.committed_nav.store(nav_id, Ordering::SeqCst);
        if self.nav_cancelled(nav_id) {
            self.write_all(SETTER_COMMIT);
        } else {
            self.clear_all();
        }
    }

    /// Whether `nav_id`'s document has replaced the view's.
    pub fn nav_committed(&self, nav_id: u64) -> bool {
        nav_id != 0 && self.committed_nav.load(Ordering::SeqCst) == nav_id
    }

    /// The load of `nav_id` is over. Whether it ended uncancelled: a
    /// cancel that arrives after this finds no load and leaves the word
    /// alone, and one that arrived before it is reported here.
    pub fn end_load(&self, nav_id: u64) -> bool {
        let _held = self.nav_lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = self
            .loading_nav
            .compare_exchange(nav_id, 0, Ordering::SeqCst, Ordering::SeqCst);
        !self.nav_cancelled(nav_id)
    }

    /// Cancel navigation `nav_id` and every earlier one, from any thread.
    /// If its load is in progress and has committed its document, the
    /// interrupt word is set to [`ALL`], so a script of that load that is
    /// running now stops at its next call or loop back-edge, and none of
    /// its later scripts starts; the next document clears it. Before the
    /// commit the word is the previous document's and is left alone: the
    /// load ends at its next await. Whether a load in progress was
    /// cancelled.
    pub fn cancel_nav(&self, nav_id: u64) -> bool {
        if nav_id == 0 {
            return false;
        }
        let _held = self.nav_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.cancel_nav.fetch_max(nav_id, Ordering::SeqCst);
        let loading = self.loading_nav.load(Ordering::SeqCst);
        let hit = loading != 0 && loading <= nav_id;
        if hit && self.committed_nav.load(Ordering::SeqCst) == loading {
            self.write_all(SETTER_CANCEL);
        }
        hit
    }

    /// Navigation `nav_id` ended without committing a document (a refused
    /// connection, a fetch error, a 204). The document still on screen gets
    /// its script back: lift [`ALL`], which the window raised when it
    /// started `nav_id`. Not when a newer navigation or a Stop cancelled
    /// it; that one raised ALL for its own reason. Whether ALL was lifted
    /// for this navigation. Held under the nav lock, as `cancel_nav`, so a
    /// cancel cannot land between the check and the lift.
    pub fn end_uncommitted(&self, nav_id: u64) -> bool {
        let _held = self.nav_lock.lock().unwrap_or_else(|e| e.into_inner());
        if nav_id == 0 || self.nav_committed(nav_id) || self.nav_cancelled(nav_id) {
            return false;
        }
        self.clear_all();
        true
    }

    /// Whether `nav_id` has been cancelled.
    pub fn nav_cancelled(&self, nav_id: u64) -> bool {
        nav_id != 0 && self.cancel_nav.load(Ordering::SeqCst) >= nav_id
    }

    /// The view's newest navigation id; 0 = none yet.
    pub fn current_nav(&self) -> u64 {
        self.nav.load(Ordering::SeqCst)
    }

    /// The navigation whose load is in progress; 0 = none.
    pub fn loading_nav(&self) -> u64 {
        self.loading_nav.load(Ordering::SeqCst)
    }

    /// The word itself, for Boa.
    #[cfg_attr(not(feature = "boa"), allow(dead_code))]
    pub(crate) fn word(&self) -> &Arc<AtomicU64> {
        &self.interrupt
    }
}

/// What one page thread is running.
///
/// Written only by its thread; read by the watchdog. A turn is published
/// seqlock-style: the start time, view and budget first, then `turn`; the
/// watchdog reads `turn` on both sides of them and uses them only if it
/// did not change.
#[derive(Debug)]
pub struct ThreadStamp {
    /// The open turn's id; 0 = idle.
    turn: AtomicU64,
    turn_started_us: AtomicU64,
    turn_view: AtomicU64,
    turn_budget_us: AtomicU64,
    /// Open turns, counting nested ones; only the outermost publishes.
    depth: AtomicU32,
    watchdog: Weak<Shared>,
}

/// A turn as the watchdog read it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OpenTurn {
    turn: u64,
    started_us: u64,
    view: u64,
    budget_us: u64,
}

impl ThreadStamp {
    /// Open a turn of script for `view` that may run for `budget`, and
    /// return its id; the watchdog stops it past that. A turn opened while
    /// another is open on this thread joins it and returns its id: the
    /// outer turn's budget governs.
    pub fn begin_turn(&self, view: &ViewControl, budget: Duration) -> u64 {
        if self.depth.fetch_add(1, Ordering::Relaxed) > 0 {
            return self.turn.load(Ordering::Relaxed);
        }
        let id = NEXT_TURN.fetch_add(1, Ordering::Relaxed);
        // The previous turn's `turn = 0` is ordered before these: a watchdog
        // that reads them sees `turn` change and skips the tick.
        fence(Ordering::Release);
        let started = now_us();
        let budget_us = budget.as_micros().min(u64::MAX as u128) as u64;
        self.turn_view.store(view.id, Ordering::Relaxed);
        self.turn_started_us.store(started, Ordering::Relaxed);
        self.turn_budget_us.store(budget_us, Ordering::Relaxed);
        self.turn.store(id, Ordering::Release);
        // Wake the watchdog if it would sleep past this turn's budget (it
        // may be sleeping a whole tick after stopping another turn), or is
        // awake and may have looked before this turn was published. The
        // lock orders the publish above before the watchdog's next look.
        if let Some(watchdog) = self.watchdog.upgrade() {
            watchdog
                .sleep
                .turn_opened(started.saturating_add(budget_us));
        }
        id
    }

    /// Close the turn [`ThreadStamp::begin_turn`] opened.
    pub fn end_turn(&self) {
        let depth = self.depth.load(Ordering::Relaxed);
        debug_assert!(depth > 0, "end_turn without begin_turn");
        if depth == 0 {
            return;
        }
        self.depth.store(depth - 1, Ordering::Relaxed);
        if depth == 1 {
            self.turn.store(0, Ordering::Release);
        }
    }

    /// The open turn's id; 0 = idle.
    pub fn current_turn(&self) -> u64 {
        self.turn.load(Ordering::Relaxed)
    }

    /// The open turn's id, how long it has been open, and its budget, in
    /// microseconds. Zeros when idle.
    pub fn timing(&self) -> (u64, u64, u64) {
        let turn = self.turn.load(Ordering::Relaxed);
        if turn == 0 {
            return (0, 0, 0);
        }
        let started = self.turn_started_us.load(Ordering::Relaxed);
        let budget = self.turn_budget_us.load(Ordering::Relaxed);
        (turn, now_us().saturating_sub(started), budget)
    }

    /// The open turn, if the read was not torn by a turn ending or
    /// starting in between.
    fn read(&self) -> Option<OpenTurn> {
        let first = self.turn.load(Ordering::Acquire);
        if first == 0 {
            return None;
        }
        let started_us = self.turn_started_us.load(Ordering::Relaxed);
        let view = self.turn_view.load(Ordering::Relaxed);
        let budget_us = self.turn_budget_us.load(Ordering::Relaxed);
        // Keeps the three loads above from moving below the second load of
        // `turn`, which two Acquire loads alone do not.
        fence(Ordering::Acquire);
        let second = self.turn.load(Ordering::Relaxed);
        (first == second).then_some(OpenTurn {
            turn: first,
            started_us,
            view,
            budget_us,
        })
    }
}

struct Shared {
    /// The longest this watchdog sleeps while a turn is open ([`TICK`];
    /// a test may lengthen it to tell a missed wake from a slow runner).
    tick: Duration,
    stamps: Mutex<Vec<Weak<ThreadStamp>>>,
    views: Mutex<HashMap<u64, Weak<ViewControl>>>,
    /// The watchdog's sleep. The thread holds it strongly and `Shared`
    /// only weakly, so it can sleep without keeping the watchdog alive.
    sleep: Arc<Sleep>,
    /// Turns the watchdog has stopped (a diagnostic).
    stopped: AtomicU64,
}

impl Drop for Shared {
    fn drop(&mut self) {
        // The thread holds only a `Weak`: woken, it finds it dead and ends.
        self.sleep.wake();
    }
}

/// The watchdog's sleep and the wake a newly opened turn sends it.
///
/// One lock for the watchdog's decision to sleep and for a turn's check of
/// that decision, so no wake is lost between them: a turn opened before the
/// watchdog takes the lock to sleep makes it look again, and a turn opened
/// after reads the time it will wake. A turn takes this lock once, when it
/// opens; the watchdog takes it about once a tick.
#[derive(Default)]
struct Sleep {
    state: Mutex<SleepState>,
    woken: Condvar,
}

#[derive(Debug)]
struct SleepState {
    /// When the sleeping watchdog wakes on its own (`now_us`). `u64::MAX`
    /// while it sleeps with no turn open, and while it is awake: a turn
    /// opened then always reaches it.
    wake_at_us: u64,
    /// A turn opened, or the watchdog was dropped, since the watchdog last
    /// woke: it looks again before it sleeps.
    kicked: bool,
    /// The watchdog is waiting on [`Sleep::woken`]. A diagnostic: the
    /// timing tests wait for it so that the turn they open finds the
    /// watchdog asleep.
    sleeping: bool,
}

impl Default for SleepState {
    fn default() -> Self {
        Self {
            wake_at_us: u64::MAX,
            kicked: false,
            sleeping: false,
        }
    }
}

impl Sleep {
    fn lock(&self) -> MutexGuard<'_, SleepState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A turn whose budget ends at `ends_us` has been published: wake the
    /// watchdog unless it already wakes by then.
    fn turn_opened(&self, ends_us: u64) {
        let mut state = self.lock();
        if ends_us < state.wake_at_us {
            state.kicked = true;
            drop(state);
            self.woken.notify_one();
        }
    }

    /// Wake the watchdog whatever it is waiting for.
    fn wake(&self) {
        self.lock().kicked = true;
        self.woken.notify_one();
    }

    /// Called by the watchdog after a look that found the next budget ends
    /// in `wait` (`None`: no turn open). Sleeps until then, or until a turn
    /// that ends sooner opens. Returns at once if a turn opened during the
    /// look, which may have missed it.
    fn sleep(&self, wait: Option<Duration>) {
        let mut state = self.lock();
        if !state.kicked {
            // The time published is the time slept to, from the same look.
            let wake_at = wait.map_or(u64::MAX, |wait| {
                now_us().saturating_add(wait.as_micros() as u64)
            });
            state.wake_at_us = wake_at;
            state.sleeping = true;
            while !state.kicked {
                if wake_at == u64::MAX {
                    state = self.woken.wait(state).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                let now = now_us();
                if now >= wake_at {
                    break;
                }
                state = self
                    .woken
                    .wait_timeout(state, Duration::from_micros(wake_at - now))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        }
        // Awake: every turn opened from here on kicks, until the next sleep.
        state.kicked = false;
        state.sleeping = false;
        state.wake_at_us = u64::MAX;
    }
}

impl Shared {
    /// Stops every open turn past its budget. Returns how long until the
    /// next open turn's budget ends, at most [`TICK`]; `None` with no turn
    /// open.
    fn scan(&self) -> Option<Duration> {
        let now = now_us();
        let mut due: Option<Duration> = None;
        let mut over: Vec<OpenTurn> = Vec::new();
        {
            let mut stamps = self.stamps.lock().unwrap_or_else(|e| e.into_inner());
            stamps.retain(|stamp| {
                let Some(stamp) = stamp.upgrade() else {
                    return false;
                };
                if let Some(open) = stamp.read() {
                    let ends = open.started_us.saturating_add(open.budget_us);
                    let wait = if now >= ends {
                        over.push(open);
                        self.tick
                    } else {
                        Duration::from_micros(ends - now).min(self.tick)
                    };
                    due = Some(due.map_or(wait, |d| d.min(wait)));
                }
                true
            });
        }
        if !over.is_empty() {
            let mut views = self.views.lock().unwrap_or_else(|e| e.into_inner());
            views.retain(|_, view| view.strong_count() > 0);
            for open in over {
                let Some(view) = views.get(&open.view).and_then(Weak::upgrade) else {
                    continue;
                };
                if !view.stops(open.turn) {
                    self.stopped.fetch_add(1, Ordering::Relaxed);
                    tracing::warn!(
                        turn = open.turn,
                        ran_ms = now.saturating_sub(open.started_us) / 1000,
                        budget_ms = open.budget_us / 1000,
                        "Watchdog: a script turn ran past its budget; interrupting it"
                    );
                }
                view.interrupt_turn(open.turn);
            }
        }
        due
    }
}

/// The thread that stops script turns that run past their budget.
///
/// Cheap to clone; the thread ends when the last clone is dropped (stamps
/// and controls do not keep it).
#[derive(Clone)]
pub struct Watchdog {
    shared: Arc<Shared>,
}

impl Watchdog {
    /// A watchdog with a thread of its own.
    pub fn spawn() -> Self {
        Self::spawn_with_tick(TICK)
    }

    /// [`Watchdog::spawn`] with its own longest sleep. Tests use a long
    /// one: a wake the watchdog misses then costs the whole tick, which no
    /// loaded runner is slow enough to hide.
    fn spawn_with_tick(tick: Duration) -> Self {
        let shared = Arc::new(Shared {
            tick,
            stamps: Mutex::default(),
            views: Mutex::default(),
            sleep: Arc::default(),
            stopped: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&shared);
        let sleep = Arc::clone(&shared.sleep);
        std::thread::Builder::new()
            .name("watchdog".into())
            .spawn(move || run(weak, sleep))
            .expect("spawn the watchdog thread");
        Self { shared }
    }

    /// The process's watchdog, started on first use.
    pub fn shared() -> &'static Watchdog {
        static SHARED: OnceLock<Watchdog> = OnceLock::new();
        SHARED.get_or_init(Watchdog::spawn)
    }

    /// A stamp for the calling page thread, watched by this watchdog.
    pub fn thread_stamp(&self) -> Arc<ThreadStamp> {
        let stamp = Arc::new(ThreadStamp {
            turn: AtomicU64::new(0),
            turn_started_us: AtomicU64::new(0),
            turn_view: AtomicU64::new(0),
            turn_budget_us: AtomicU64::new(0),
            depth: AtomicU32::new(0),
            watchdog: Arc::downgrade(&self.shared),
        });
        self.shared
            .stamps
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::downgrade(&stamp));
        stamp
    }

    /// The control block for a new view, watched by this watchdog.
    pub fn view_control(&self) -> Arc<ViewControl> {
        let control = Arc::new(ViewControl::new());
        self.shared
            .views
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(control.id, Arc::downgrade(&control));
        control
    }

    /// How many turns this watchdog has stopped.
    pub fn turns_stopped(&self) -> u64 {
        self.shared.stopped.load(Ordering::Relaxed)
    }
}

fn run(shared: Weak<Shared>, sleep: Arc<Sleep>) {
    loop {
        let Some(live) = shared.upgrade() else {
            return;
        };
        let wait = live.scan();
        drop(live);
        sleep.sleep(wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_racing_the_end_of_an_uncommitted_navigation_keeps_its_stop() {
        // #730 follow-up (b): the page lowered ALL for a navigation that
        // never committed after reading "not cancelled", unlocked. A newer
        // navigation's cancel and stop landing between the two was undone,
        // and the old document's script ran on under the new load.
        let control = Arc::new(ViewControl::new());
        control.begin_nav(1);
        control.commit_nav(1);
        assert!(control.end_load(1));
        control.stop_all();
        control.begin_nav(2);
        let held = control.nav_lock.lock().unwrap();
        let ending = {
            let control = Arc::clone(&control);
            std::thread::spawn(move || control.end_uncommitted(2))
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        // A newer navigation, as `ContentThread::navigate`: cancel, then stop.
        control.cancel_nav.fetch_max(2, Ordering::SeqCst);
        control.stop_all();
        drop(held);
        let lifted = ending.join().unwrap();
        assert!(!lifted, "ALL was lifted for a navigation already cancelled");
        assert!(control.stops(42), "the newer navigation's stop was undone");
    }

    #[test]
    fn an_uncommitted_navigation_gives_the_shown_document_its_script_back() {
        let control = ViewControl::new();
        control.begin_nav(1);
        control.commit_nav(1);
        assert!(control.end_load(1));
        control.stop_all();
        control.begin_nav(2);
        assert!(control.end_load(2));
        assert!(control.end_uncommitted(2));
        assert!(!control.stops(42));
        assert!(!control.end_uncommitted(1), "a committed navigation is not ended again");
    }

    #[test]
    fn a_cancel_stops_the_running_load_and_every_earlier_one() {
        let control = ViewControl::new();
        control.begin_nav(3);
        assert!(!control.nav_cancelled(3));
        control.commit_nav(3);
        assert!(control.cancel_nav(3), "the load in progress was hit");
        assert!(control.nav_cancelled(3) && control.nav_cancelled(2));
        assert!(control.stops(42), "its running script stops");
        assert!(!control.end_load(3), "the load reports the cancel");
    }

    #[test]
    fn a_cancel_before_the_commit_spares_the_previous_document() {
        let control = ViewControl::new();
        control.begin_nav(1);
        control.commit_nav(1);
        assert!(control.end_load(1));
        control.begin_nav(2);
        assert!(control.cancel_nav(2), "the load in progress was hit");
        assert!(control.nav_cancelled(2));
        assert!(!control.stops(42), "the shown document keeps its script");
        assert!(!control.end_load(2), "the load reports the cancel");
    }

    #[test]
    fn a_commit_after_the_cancel_stops_the_new_documents_script() {
        let control = ViewControl::new();
        control.begin_nav(1);
        assert!(control.cancel_nav(1));
        assert!(!control.stops(42));
        control.commit_nav(1);
        assert!(
            control.stops(42),
            "none of the cancelled load's script runs"
        );
    }

    #[test]
    fn a_cancel_after_the_load_ended_leaves_the_word_alone() {
        let control = ViewControl::new();
        control.begin_nav(1);
        assert!(control.end_load(1));
        assert!(!control.cancel_nav(1), "no load in progress");
        assert!(!control.stops(7), "the finished document keeps its script");
        assert!(!control.cancel_nav(0), "0 names no navigation");
    }

    #[test]
    fn a_late_cancel_for_an_older_navigation_spares_the_newer_one() {
        let control = ViewControl::new();
        control.begin_nav(1);
        control.begin_nav(2);
        assert!(!control.cancel_nav(1), "navigation 2 is the one loading");
        assert!(!control.nav_cancelled(2));
        assert!(!control.stops(5));
        assert_eq!(control.current_nav(), 2);
        assert_eq!(control.loading_nav(), 2);
    }

    #[test]
    fn writers_never_lower_the_word() {
        let control = ViewControl::new();
        control.interrupt_turn(5);
        control.interrupt_turn(3);
        assert!(control.stops(5));
        assert!(
            !control.stops(3),
            "an older id does not replace a newer one"
        );
        control.stop_all();
        control.interrupt_turn(9);
        assert!(
            control.stops(9) && control.stops(10),
            "a turn id never replaces ALL"
        );
        assert!(control.clear_all());
        assert!(!control.stops(10));
        control.interrupt_turn(11);
        assert!(!control.clear_all(), "clear_all leaves a turn id");
        assert!(control.stops(11));
    }

    #[test]
    fn the_last_writer_of_the_word_is_recorded() {
        let control = ViewControl::new();
        control.interrupt_turn(4);
        let (word, tag, at, target) = control.word_writer();
        assert_eq!((word, tag, target), (4, "watchdog", 4));
        assert!(at > 0);
        control.begin_nav(1);
        control.commit_nav(1);
        assert!(control.cancel_nav(1));
        let (word, tag, _, target) = control.word_writer();
        assert_eq!((word, tag, target), (ALL, "cancel", ALL));
        assert!(control.clear_all());
        let (word, tag, _, target) = control.word_writer();
        assert_eq!((word, tag, target), (0, "clear", 0));
        control.begin_nav(2);
        control.cancel_nav(2);
        control.commit_nav(2);
        assert_eq!(control.word_writer().1, "commit");
    }

    #[test]
    fn nested_turns_join_the_outer_one() {
        let watchdog = Watchdog::spawn();
        let stamp = watchdog.thread_stamp();
        let view = watchdog.view_control();
        let outer = stamp.begin_turn(&view, Duration::from_secs(60));
        assert_eq!(stamp.begin_turn(&view, Duration::from_secs(1)), outer);
        stamp.end_turn();
        assert_eq!(
            stamp.current_turn(),
            outer,
            "the inner end leaves the outer turn open"
        );
        stamp.end_turn();
        assert_eq!(stamp.current_turn(), 0);
        assert!(
            stamp.begin_turn(&view, Duration::from_secs(1)) > outer,
            "ids only grow"
        );
        stamp.end_turn();
    }

    #[test]
    fn a_read_torn_by_a_turn_change_is_not_acted_on() {
        // A stepped version of the watchdog's read: the turn ends and the
        // next starts between its two loads of `turn`.
        let watchdog = Watchdog::spawn();
        let stamp = watchdog.thread_stamp();
        let view = watchdog.view_control();
        let first = stamp.begin_turn(&view, Duration::from_millis(0));
        let t1 = stamp.turn.load(Ordering::Acquire);
        stamp.end_turn();
        let second = stamp.begin_turn(&view, Duration::from_secs(60));
        let t2 = stamp.turn.load(Ordering::Relaxed);
        assert_eq!((t1, t2), (first, second));
        assert_ne!(t1, t2, "the reader skips this tick");
        // An untorn read sees the new turn with its own clock and budget.
        let open = stamp.read().expect("a turn is open");
        assert_eq!(
            (open.turn, open.budget_us, open.view),
            (second, 60_000_000, view.id())
        );
        stamp.end_turn();
        assert_eq!(stamp.read(), None);
    }

    // The three tests that bound how soon the watchdog thread stops a turn.
    // Cargo runs the suite in parallel. On a loaded macOS runner the short
    // turn below was stopped at 275 ms, and once at 396 ms, against a 200 ms
    // ceiling, while these tests spun beside it. They share one lock.
    fn one_watchdog_timing_test() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        // A panicking test must not poison the other two.
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[test]
    fn the_watchdog_stops_a_turn_past_its_budget_and_not_the_next() {
        let _timing = one_watchdog_timing_test();
        let watchdog = Watchdog::spawn();
        let stamp = watchdog.thread_stamp();
        let view = watchdog.view_control();
        let started = Instant::now();
        let turn = stamp.begin_turn(&view, Duration::from_millis(100));
        while !view.stops(turn) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the watchdog never fired"
            );
            std::thread::yield_now();
        }
        let fired = started.elapsed();
        assert!(
            fired >= Duration::from_millis(100),
            "fired early: {fired:?}"
        );
        assert!(fired < Duration::from_millis(350), "fired late: {fired:?}");
        stamp.end_turn();
        assert_eq!(watchdog.turns_stopped(), 1);

        // The next turn, within its budget, is left alone.
        let next = stamp.begin_turn(&view, Duration::from_secs(60));
        std::thread::sleep(Duration::from_millis(300));
        assert!(!view.stops(next));
        stamp.end_turn();
    }

    /// After stopping a turn the watchdog sleeps a whole tick; a turn whose
    /// budget ends sooner (the grace turn after a stop) wakes it. A missed
    /// wake waits out that sleep, so the stop lands at about a tick after
    /// the short turn opened. With the process's 250 ms tick that is too
    /// close to what a loaded runner adds to a 30 ms turn (275 ms and
    /// 396 ms were seen on CI): the watchdog here gets a tick of its own,
    /// long enough that a missed wake lands far past any runner's slack,
    /// and the ceiling sits between the two.
    #[test]
    fn a_short_turn_opened_while_the_watchdog_sleeps_a_tick_is_stopped_on_time() {
        const LONG_TICK: Duration = Duration::from_secs(2);
        const SHORT_TURN: Duration = Duration::from_millis(30);
        // Half a tick: a missed wake costs the whole tick, a slow runner
        // has had up to about 970 ms on top of the 30 ms turn.
        const CEILING: Duration = Duration::from_secs(1);
        let _timing = one_watchdog_timing_test();
        let watchdog = Watchdog::spawn_with_tick(LONG_TICK);
        let stamp = watchdog.thread_stamp();
        let view = watchdog.view_control();
        let first = stamp.begin_turn(&view, Duration::from_millis(0));
        let started = Instant::now();
        while !view.stops(first) {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the first stop never came"
            );
            std::thread::yield_now();
        }
        // With the stopped turn still open (the script is unwinding), wait
        // for the watchdog to be in its tick-long sleep: its last look saw
        // that turn, so it sleeps a tick, not until woken, and only the
        // short turn's own wake can bring it back before the tick ends.
        // Ending the turn first would let the watchdog sleep with nothing
        // open, until any turn wakes it, which tests nothing.
        let started = Instant::now();
        while !watchdog.shared.sleep.lock().sleeping {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the watchdog never went to sleep after the first stop"
            );
            std::thread::yield_now();
        }
        stamp.end_turn();
        let started = Instant::now();
        let short = stamp.begin_turn(&view, SHORT_TURN);
        while !view.stops(short) {
            assert!(
                started.elapsed() < LONG_TICK + Duration::from_secs(2),
                "the short turn was never stopped"
            );
            std::thread::yield_now();
        }
        let took = started.elapsed();
        stamp.end_turn();
        assert!(took >= SHORT_TURN, "stopped early: {took:?}");
        assert!(
            took < CEILING,
            "stopped after {took:?}: the short turn did not wake the watchdog from its {LONG_TICK:?} tick"
        );
    }

    #[test]
    fn the_watchdog_ends_with_its_last_owner_and_its_stamps_still_work() {
        let _timing = one_watchdog_timing_test();
        let watchdog = Watchdog::spawn();
        let weak = Arc::downgrade(&watchdog.shared);
        let stamp = watchdog.thread_stamp();
        let view = watchdog.view_control();
        drop(watchdog);
        let started = Instant::now();
        // The thread may hold it for the length of one scan.
        while weak.upgrade().is_some() {
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "the watchdog outlived its owner"
            );
            std::thread::yield_now();
        }
        stamp.begin_turn(&view, Duration::from_millis(1));
        stamp.end_turn();
    }
}
