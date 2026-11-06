//! The devtools service thread (dev-tools lane T6a/T6b, `T6-design.md`).
//!
//! One `devtools` thread per process answers the Inspector panel's reads
//! of what a view has already recorded, from the view's
//! [`Inspector`](crate::inspector::Inspector) ring. The page thread is not
//! asked: before T6a every console read was a [`PageCmd::Devtools`]
//! (`crate::PageCmd`) the page answered between its turns, about four a
//! second while a panel was open on an idle page (#785's `CONSOLE_POLL`).
//!
//! Nothing polls (T6b). While a panel is open its view is watched: the
//! Inspector signals this thread after a record is pushed (and its lock is
//! released), and the thread wakes the window once, not again until the
//! panel has read. An idle page with the panel open costs no wakes at all.
//!
//! Records are redacted when they are recorded (`inspector.rs`), so the
//! service serves them as they are. The window never takes the Inspector
//! lock: sinks run under it, and a slow sink must not hold the UI.
//!
//! Live DOM and computed-style questions still go to the page.
//!
//! [`DevtoolsCounters`] counts what devtools costs the page thread: page
//! commands handled for devtools, and window wakes posted for a devtools
//! answer. `HIWAVE_DEVTOOLS_COUNTERS=1` logs the process totals once a
//! second from the service thread.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use crate::devtools::devtools_console_entries;
use crate::inspector::Inspector;
use crate::page_protocol::devtools_wakes_window;
use crate::DevtoolsAnswer;

/// What devtools has cost one page (or, for [`DevtoolsCounters::process`],
/// every page in the process).
#[derive(Debug, Default)]
pub struct DevtoolsCounters {
    page_commands: AtomicU64,
    window_wakes: AtomicU64,
}

/// A reading of [`DevtoolsCounters`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DevtoolsCount {
    /// [`crate::PageCmd::Devtools`] commands the page thread handled.
    pub page_commands: u64,
    /// Window wakes posted because a devtools answer had something new.
    pub window_wakes: u64,
}

impl DevtoolsCounters {
    /// The totals over every page in the process.
    pub fn process() -> &'static DevtoolsCounters {
        static PROCESS: DevtoolsCounters = DevtoolsCounters {
            page_commands: AtomicU64::new(0),
            window_wakes: AtomicU64::new(0),
        };
        &PROCESS
    }

    pub fn read(&self) -> DevtoolsCount {
        DevtoolsCount {
            page_commands: self.page_commands.load(Ordering::Relaxed),
            window_wakes: self.window_wakes.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn page_command(&self) {
        self.page_commands.fetch_add(1, Ordering::Relaxed);
        Self::process().page_commands.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn window_wake(&self) {
        self.window_wakes.fetch_add(1, Ordering::Relaxed);
        Self::process().window_wakes.fetch_add(1, Ordering::Relaxed);
    }
}

/// `HIWAVE_DEVTOOLS_COUNTERS=1` (or true/on/yes).
fn counters_logged() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        matches!(
            std::env::var("HIWAVE_DEVTOOLS_COUNTERS").ok().as_deref(),
            Some("1" | "true" | "on" | "yes")
        )
    })
}

/// A console (or network) read for the service.
struct ConsoleRead {
    inspector: Arc<Inspector>,
    since: u64,
    /// The current document's requests instead of console lines (T7).
    network: bool,
    reply: oneshot::Sender<DevtoolsAnswer>,
    /// Wake the window's loop when the answer has something new, as the
    /// content thread's page did. Counted here, not on the page.
    wake_window: Option<Arc<DevtoolsCounters>>,
}

/// An open panel's view: when it records past what the panel has read, the
/// service wakes the window once, and not again until the panel reads.
struct Watch {
    /// Weak: a closed tab's ring is freed even if its panel never said it
    /// closed (T6 R1 condition 3). A dead watch is dropped.
    inspector: Weak<Inspector>,
    counters: Arc<DevtoolsCounters>,
    /// The `next_seq` of the panel's last read.
    read_to: u64,
    /// A wake is out that the panel has not answered with a read.
    woken: bool,
    /// The view's held console count when the window was last told: a
    /// script that hangs after a hot loop holds calls and records nothing,
    /// so this is the only news the panel gets of them (#810 review).
    held_seen: usize,
}

#[derive(Default)]
struct Inbox {
    reads: Vec<ConsoleRead>,
    /// A watched view recorded since the service last looked.
    records: bool,
    watches: Vec<Watch>,
    started: bool,
}

struct Service {
    inbox: Mutex<Inbox>,
    ready: Condvar,
}

fn service() -> &'static Service {
    static SERVICE: OnceLock<Service> = OnceLock::new();
    SERVICE.get_or_init(|| Service {
        inbox: Mutex::new(Inbox::default()),
        ready: Condvar::new(),
    })
}

impl Service {
    fn inbox(&self) -> MutexGuard<'_, Inbox> {
        self.inbox.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Start the thread on first use. False when it could not be started.
    fn start(&'static self, inbox: &mut Inbox) -> bool {
        if !inbox.started {
            inbox.started = std::thread::Builder::new()
                .name("devtools".into())
                .spawn(move || self.run())
                .is_ok();
        }
        inbox.started
    }

    fn run(&self) {
        let logged = counters_logged();
        let mut next_log = Instant::now() + Duration::from_secs(1);
        loop {
            let reads = {
                let mut inbox = self.inbox();
                // Before sleeping, re-read every watched view's watermark
                // under this mutex: a push whose `watched` load raced the
                // panel opening sent no signal (T6 R1 condition 1). The
                // 1 s timeout is the backstop for anything else missed.
                while inbox.reads.is_empty() && !inbox.records && !unread(&inbox) {
                    #[cfg(test)]
                    before_wait();
                    if inbox.watches.is_empty() && !logged {
                        // No panel open: nothing to miss, no clock.
                        inbox = self.ready.wait(inbox).unwrap_or_else(|e| e.into_inner());
                        continue;
                    }
                    let wait = if logged {
                        next_log.saturating_duration_since(Instant::now()).min(BACKSTOP)
                    } else {
                        BACKSTOP
                    };
                    inbox = self.ready.wait_timeout(inbox, wait).unwrap_or_else(|e| e.into_inner()).0;
                    inbox.watches.retain(|w| w.inspector.strong_count() > 0);
                    if logged && Instant::now() >= next_log {
                        break;
                    }
                }
                inbox.records = false;
                std::mem::take(&mut inbox.reads)
            };
            for read in reads {
                let inspector = Arc::downgrade(&read.inspector);
                // A network read leaves the console's read mark alone.
                let Some(next_seq) = answer(read) else { continue };
                let mut inbox = self.inbox();
                if let Some(watch) = inbox.watches.iter_mut().find(|w| Weak::ptr_eq(&w.inspector, &inspector)) {
                    watch.read_to = watch.read_to.max(next_seq);
                    watch.woken = false;
                }
            }
            {
                // Every pass looks (cheap: one atomic load per open panel),
                // so a wake found by the re-read or the backstop is posted.
                let mut inbox = self.inbox();
                inbox.watches.retain(|w| w.inspector.strong_count() > 0);
                for watch in inbox.watches.iter_mut() {
                    let Some(inspector) = watch.inspector.upgrade() else { continue };
                    if !watch.woken && inspector.watermark() > watch.read_to {
                        watch.woken = true;
                        watch.held_seen = inspector.console_held();
                        watch.counters.window_wake();
                        crate::script_net::poke_live_turn();
                    }
                    // The held count moved: the panel reads it from the
                    // Inspector, so this wake asks for no read and leaves
                    // `woken` alone (a read only comes for new records).
                    let held = inspector.console_held();
                    if held != watch.held_seen {
                        watch.held_seen = held;
                        watch.counters.window_wake();
                    }
                }
            }
            if logged && Instant::now() >= next_log {
                let c = DevtoolsCounters::process().read();
                tracing::info!(
                    target: "devtools",
                    page_commands = c.page_commands,
                    window_wakes = c.window_wakes,
                    "devtools counters"
                );
                next_log = Instant::now() + Duration::from_secs(1);
            }
        }
    }
}

/// The longest the service sleeps while a panel is open, whatever it was
/// told: a missed signal costs at most this (T6 R1 condition 1).
const BACKSTOP: Duration = Duration::from_secs(1);

/// A watched view recorded past what its panel read, and no wake is out.
fn unread(inbox: &Inbox) -> bool {
    inbox.watches.iter().any(|w| {
        w.inspector.upgrade().is_some_and(|inspector| {
            (!w.woken && inspector.watermark() > w.read_to) || inspector.console_held() != w.held_seen
        })
    })
}

/// Test hook run on the service thread after the watermark re-read and
/// before the wait, with the inbox locked (so it must not signal).
#[cfg(test)]
pub(crate) static BEFORE_WAIT: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

#[cfg(test)]
fn before_wait() {
    let hook = BEFORE_WAIT.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(hook) = hook {
        hook();
    }
}

/// A watched view recorded. The Inspector calls this after its lock is
/// released, and only while a panel watches the view.
pub(crate) fn records_arrived() {
    let service = service();
    service.inbox().records = true;
    service.ready.notify_one();
}

/// A panel opened (`on`) or closed on the view `inspector` records. While
/// it is open, the window is woken when the view records something the
/// panel has not read (counted on `counters`). Both the window (at once)
/// and the page (when it handles the attach) call this, so a watch the
/// window could not make (the page booting, or holding its endpoint) is
/// still made; a second `on` keeps the first's read mark.
pub fn watch(inspector: &Arc<Inspector>, counters: &Arc<DevtoolsCounters>, on: bool) {
    let service = service();
    let mut inbox = service.inbox();
    let weak = Arc::downgrade(inspector);
    inbox.watches.retain(|w| w.inspector.strong_count() > 0);
    if on && inbox.watches.iter().any(|w| Weak::ptr_eq(&w.inspector, &weak)) {
        inspector.set_watched(true);
        return;
    }
    inbox.watches.retain(|w| !Weak::ptr_eq(&w.inspector, &weak));
    let watched = on && service.start(&mut inbox);
    if watched {
        inbox.watches.push(Watch {
            inspector: weak,
            counters: Arc::clone(counters),
            read_to: 0,
            woken: false,
            held_seen: 0,
        });
        // What the view already recorded is news to a panel that just opened.
        inbox.records = true;
        service.ready.notify_one();
    }
    inspector.set_watched(watched);
}

/// Answer `read`; returns the `next_seq` a console read answered up to.
fn answer(read: ConsoleRead) -> Option<u64> {
    if read.network {
        let (snapshot, doc) = read.inspector.network_for_current_doc();
        let mut network = crate::devtools_network(&snapshot, doc);
        // The second guard, as for console lines.
        for request in &mut network.requests {
            request.url = read.inspector.url_text(&request.url);
            request.failure = request.failure.as_deref().map(|f| read.inspector.scrub_text(f));
        }
        let _ = read.reply.send(DevtoolsAnswer::Network(network));
        return None;
    }
    let mut console = devtools_console_entries(&read.inspector.snapshot_since(read.since));
    console.page_crashed = read.inspector.page_crashed();
    console.held = read.inspector.console_held();
    guard(&read.inspector, &mut console);
    let next_seq = console.next_seq;
    let answer = DevtoolsAnswer::Console(console);
    let wake = devtools_wakes_window(&answer, false);
    let _ = read.reply.send(answer);
    if let (true, Some(counters)) = (wake, read.wake_window) {
        counters.window_wake();
        crate::script_net::poke_live_turn();
    }
    Some(next_seq)
}

/// The second guard (T6 R1 condition 7). Redaction happens when a record is
/// made (`inspector.rs`); this only re-applies the URL rule to what is
/// served, so a record pushed past it (`Inspector::emit` takes a built
/// event as it is) still leaves without a query string or fragment.
fn guard(inspector: &Inspector, console: &mut crate::DevtoolsConsole) {
    for entry in &mut console.entries {
        entry.message = inspector.scrub_text(&entry.message);
        for frame in &mut entry.stack {
            frame.file = inspector.url_text(&frame.file);
        }
    }
}

/// The console lines `inspector` recorded since `since`, answered on the
/// devtools thread. With `wake_window`, the window's live wake is posted
/// when the answer has something new (and counted there).
pub fn ask_console(
    inspector: Arc<Inspector>,
    since: u64,
    wake_window: Option<Arc<DevtoolsCounters>>,
) -> oneshot::Receiver<DevtoolsAnswer> {
    let (reply, rx) = oneshot::channel();
    submit(ConsoleRead {
        inspector,
        since,
        network: false,
        reply,
        wake_window,
    });
    rx
}

/// The current document's requests `inspector` recorded, answered on the
/// devtools thread: served while the page thread is blocked (T7).
pub fn ask_network(inspector: Arc<Inspector>) -> oneshot::Receiver<DevtoolsAnswer> {
    let (reply, rx) = oneshot::channel();
    submit(ConsoleRead {
        inspector,
        since: 0,
        network: true,
        reply,
        wake_window: None,
    });
    rx
}

fn submit(read: ConsoleRead) {
    let service = service();
    let mut inbox = service.inbox();
    if service.start(&mut inbox) {
        inbox.reads.push(read);
        service.ready.notify_one();
    } else {
        drop(inbox);
        answer_unserved(read);
    }
}

/// No devtools thread could be started. The caller is the window (or the
/// panel's) thread, which must not take the Inspector lock, so the read
/// gets an empty answer and a log line instead.
fn answer_unserved(read: ConsoleRead) {
    tracing::warn!(target: "devtools", "devtools thread did not start; console read answered empty");
    if read.network {
        let _ = read.reply.send(DevtoolsAnswer::Network(crate::DevtoolsNetwork::default()));
        return;
    }
    let console = crate::DevtoolsConsole {
        next_seq: read.since,
        ..Default::default()
    };
    let _ = read.reply.send(DevtoolsAnswer::Console(console));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inspector::{ConsoleLevel, InspectorConfig, InspectorEvent, SourceLocation, TAB_RING_CAPACITY};

    fn tab() -> Arc<Inspector> {
        Inspector::new(&InspectorConfig { buffer: true, capacity: TAB_RING_CAPACITY, ..Default::default() })
    }

    fn until(budget: Duration, done: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        done()
    }

    fn read(inspector: &Arc<Inspector>, since: u64) -> crate::DevtoolsConsole {
        match ask_console(Arc::clone(inspector), since, None).blocking_recv() {
            Ok(DevtoolsAnswer::Console(console)) => console,
            other => panic!("no console answer: {other:?}"),
        }
    }

    /// T6 R1 condition 1: a record pushed after the service last looked
    /// and before it sleeps, with no signal (its push read `watched` as
    /// it was being set), still wakes the window: the 1 s backstop.
    #[test]
    fn a_push_between_the_re_read_and_the_wait_still_wakes_the_panel() {
        let inspector = tab();
        let counters = Arc::new(DevtoolsCounters::default());
        watch(&inspector, &counters, true);
        inspector.console(ConsoleLevel::Log, "a", None);
        assert!(until(Duration::from_secs(3), || counters.read().window_wakes == 1), "first line: no wake");
        // The panel reads first. The hook is global and other tests' panels
        // run service passes too, so set before the read it could fire
        // early and push "b" before the panel had read "a".
        assert_eq!(read(&inspector, 0).next_seq, 1);
        let late = Arc::clone(&inspector);
        *BEFORE_WAIT.lock().unwrap_or_else(|e| e.into_inner()) = Some(Box::new(move || {
            late.set_watched(false);
            late.console(ConsoleLevel::Log, "b", None);
            late.set_watched(true);
        }));
        // The next pass (the backstop at the latest) re-reads, runs the
        // hook and waits; only the backstop finds "b".
        assert!(
            until(Duration::from_secs(5), || counters.read().window_wakes == 2),
            "the unsignalled line never woke the panel (wakes {:?}, watermark {})",
            counters.read(),
            inspector.watermark()
        );
        watch(&inspector, &counters, false);
    }

    /// T8: the network read clones only the current document's network
    /// records, and answers exactly what the whole-ring read answered: two
    /// documents, a full ring of console lines around their requests, a
    /// response, a failure, one left open. Prints the read's time on the full
    /// ring both ways (the lock is held for the clone).
    #[test]
    fn the_network_read_clones_only_the_current_documents_requests() {
        use crate::inspector::{FailureStage, ResourceType};
        let inspector = tab();
        let url = |p: &str| url::Url::parse(&format!("https://a.test/{p}?q=SECRET")).unwrap();
        let old = inspector.begin_request(&url("old"), "GET", ResourceType::Fetch, Some("fetch")).unwrap();
        old.response(200, Some("text/plain"), None, Some(1), false);
        inspector.navigation_started(Some(&url("next")));
        let console = TAB_RING_CAPACITY - 160;
        let mut open = Vec::new();
        for i in 0..150 {
            for _ in 0..console / 150 {
                inspector.console(ConsoleLevel::Log, "line", None);
            }
            let g = inspector.begin_request(&url(&format!("r{i}")), "GET", ResourceType::Fetch, Some("fetch")).unwrap();
            match i % 3 {
                0 => g.response(200, Some("text/plain"), None, Some(1), false),
                1 => g.fail(FailureStage::Connect, "refused"),
                _ => open.push(g),
            }
        }
        let started = Instant::now();
        let (whole, doc) = inspector.snapshot_and_doc(0);
        let before = crate::devtools_network(&whole, doc);
        let whole_took = started.elapsed();
        let started = Instant::now();
        let (network, now_doc) = inspector.network_for_current_doc();
        let after = crate::devtools_network(&network, now_doc);
        let network_took = started.elapsed();
        eprintln!(
            "T8 network read on a ring of {} records: whole ring {} cloned in {whole_took:?}, current doc's network {} cloned in {network_took:?}",
            whole.records.len(),
            whole.records.len(),
            network.records.len()
        );
        assert_eq!(after, before);
        assert_eq!(after.requests.len(), 150, "the old document's request is not listed");
        assert!(network.records.len() < whole.records.len() / 4, "{} of {}", network.records.len(), whole.records.len());
        assert!(network.records.iter().all(|r| r.doc == now_doc));
        drop(open);
    }

    /// T6 R1 condition 3: twenty tabs with an open panel each, full rings,
    /// closed without the panel saying so. The service keeps no ring alive.
    #[test]
    fn closed_tabs_free_their_rings_with_the_panel_still_open() {
        let counters = Arc::new(DevtoolsCounters::default());
        let mut weak = Vec::new();
        let mut ring_bytes = 0usize;
        for _ in 0..20 {
            let inspector = tab();
            watch(&inspector, &counters, true);
            for i in 0..TAB_RING_CAPACITY {
                inspector.console(ConsoleLevel::Log, &format!("line {i}"), None);
            }
            let snapshot = inspector.snapshot_since(0);
            assert_eq!(snapshot.records.len(), TAB_RING_CAPACITY);
            ring_bytes += snapshot.records.iter().map(|r| serde_json::to_vec(r).map_or(0, |v| v.len())).sum::<usize>();
            weak.push(Arc::downgrade(&inspector));
        }
        // Let the service look at every view at least once.
        std::thread::sleep(Duration::from_millis(100));
        let alive = weak.iter().filter(|w| w.strong_count() > 0).count();
        eprintln!(
            "T6 condition 3: 20 tabs x {TAB_RING_CAPACITY} records; ring payload {} KiB total ({} KiB per tab, \
             {} B per record struct); alive after close: {alive}",
            ring_bytes / 1024,
            ring_bytes / 1024 / 20,
            std::mem::size_of::<crate::inspector::InspectorRecord>()
        );
        assert_eq!(alive, 0, "the devtools service kept closed tabs' rings alive");
        assert!(until(Duration::from_secs(3), || {
            service().inbox().watches.iter().all(|w| !weak.iter().any(|d| Weak::ptr_eq(d, &w.inspector)))
        }));
    }

    /// T6 R1 condition 7: redaction is at record time; what the service
    /// serves passes the URL rule again. `emit` takes a built event as it
    /// is, so this one reaches the ring with its query and fragment.
    #[test]
    fn the_service_strips_a_query_that_got_past_record_time_redaction() {
        let inspector = tab();
        inspector.emit(InspectorEvent::Console {
            level: ConsoleLevel::Error,
            message: "see https://a.test/x?token=secret#frag".into(),
            truncated: false,
            location: Some(SourceLocation { url: "https://a.test/app.js?token=secret#frag".into(), line: 3, column: 9 }),
            stack: None,
            error_stack: None,
        });
        let console = read(&inspector, 0);
        let served = serde_json::to_string(&console.entries).unwrap();
        assert!(!served.contains("secret") && !served.contains("frag"), "{served}");
        assert!(served.contains("https://a.test/app.js"), "{served}");
        assert!(served.contains("https://a.test/x"), "{served}");
    }

    /// #803 review (2): the page repeats the window's watch when it handles
    /// the attach. The second keeps the panel's read mark: nothing it has
    /// read wakes the window again.
    #[test]
    fn the_page_repeating_a_watch_keeps_the_panels_read_mark() {
        let inspector = tab();
        let counters = Arc::new(DevtoolsCounters::default());
        watch(&inspector, &counters, true);
        inspector.console(ConsoleLevel::Log, "a", None);
        assert!(until(Duration::from_secs(3), || counters.read().window_wakes == 1), "first line: no wake");
        assert_eq!(read(&inspector, 0).next_seq, 1);
        // The service marks the read after it has answered.
        std::thread::sleep(Duration::from_millis(100));
        watch(&inspector, &counters, true);
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(counters.read().window_wakes, 1, "the repeated watch woke the window for a read line");
        inspector.console(ConsoleLevel::Log, "b", None);
        assert!(until(Duration::from_secs(3), || counters.read().window_wakes == 2), "a new line: no wake");
        watch(&inspector, &counters, false);
    }

    /// #810 review: a script that hangs after a hot loop holds console calls
    /// past the live cap and records nothing, so nothing woke the panel to
    /// show them. A held count that moves, with no new record, wakes the
    /// window (at the backstop at the latest), and the console answer
    /// carries it; a count that stays put wakes it no more.
    #[test]
    fn a_held_console_count_wakes_the_panel_without_a_record() {
        let inspector = tab();
        let counters = Arc::new(DevtoolsCounters::default());
        watch(&inspector, &counters, true);
        std::thread::sleep(Duration::from_millis(100));
        let from = counters.read().window_wakes;
        inspector.console_held_counter().store(100, std::sync::atomic::Ordering::SeqCst);
        assert!(
            until(Duration::from_secs(3), || counters.read().window_wakes > from),
            "a held count with no record never woke the panel"
        );
        assert_eq!(read(&inspector, 0).held, 100);
        let woken = counters.read().window_wakes;
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(counters.read().window_wakes, woken, "an unchanged count woke the window again");
        // The script ended: its flush clears the count, and the panel hears.
        inspector.console_held_counter().store(0, std::sync::atomic::Ordering::SeqCst);
        assert!(until(Duration::from_secs(3), || counters.read().window_wakes > woken), "the cleared count woke nothing");
        assert_eq!(crate::devtools_held_text(0), None);
        assert_eq!(crate::devtools_held_text(3).as_deref(), Some("3 console lines held by a running script"));
        watch(&inspector, &counters, false);
    }

    /// #803 review nit: with no devtools thread the read is answered empty
    /// on the caller's thread, without reading the view.
    #[test]
    fn an_unserved_console_read_is_answered_empty() {
        let inspector = tab();
        inspector.console(ConsoleLevel::Log, "a", None);
        let (reply, mut rx) = oneshot::channel();
        answer_unserved(ConsoleRead { inspector, since: 7, network: false, reply, wake_window: None });
        match rx.try_recv() {
            Ok(DevtoolsAnswer::Console(console)) => {
                assert!(console.entries.is_empty(), "{:?}", console.entries);
                assert_eq!(console.next_seq, 7);
            }
            other => panic!("no empty answer: {other:?}"),
        }
    }
}
