//! The W-gate log (threads plan `THREADS_PLAN_2026-10.md` §4 invariant 9
//! and §5 "The W-gate"): every window-thread hold of 50 ms or more, by
//! stage, and input-to-present latency.
//!
//! The Presenter stays on the window thread until M9, so the window can
//! still stall on the page's build, layout and paint, and on the
//! presenter's own surface acquire, execute and present. The W-gate reads
//! this log at M4 on eBay search, cnn and the Wikipedia portal: a stage
//! that holds the window 50 ms or more, or input-to-present over 32 ms at
//! p95, moves M9 (the render thread) ahead of M5.
//!
//! Two measurements, both on the window thread, both always on (two
//! `Instant` reads per stage and one per entry; nothing is logged under
//! the threshold):
//!
//! 1. **Holds.** Every entry the window thread makes into the engine that
//!    can run page code or draw ([`WindowHold`]) is timed; the outermost
//!    one reports if it lasted [`HOLD_THRESHOLD_MS`] or more, with the
//!    milliseconds each stage took inside it:
//!
//!    ```text
//!    Window thread hold entry=live_turn ms=612 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 acquire_ms=0.0 execute_ms=0.0 present_ms=0.0 other_ms=612.0
//!    Content thread hold entry=live_turn ms=612 ...
//!
//!    The word is `Content` when the entry runs on the page thread
//!    (named `page`). Flag-off entries stay `Window thread hold`.
//!    ```
//!
//!    Between the stages and `other_ms` the line names the script turns
//!    (`script_ms`, `script_turns`, `script_longest_ms`, `script_stopped`:
//!    those the watchdog stopped) and, on a load, what it waited for
//!    (`parse_ms`, `style_ms`, `doc_fetch_ms`, `sheets_ms`, `fonts_ms`,
//!    `images_ms`, `script_fetch_ms`, `script_net_ms`; [`HoldBucket`]).
//!    One moment is charged once. A wait is wall clock, whatever else the
//!    thread did between its polls.
//!
//!    `other_ms` is what none of those claims: the hit test, a live turn's
//!    network settling. The presenter's three stages run inside `render`
//!    and are its breakdown, so `other` is the hold less the five relayout
//!    stages and the buckets. A nested entry (a relayout inside a click)
//!    is counted by the outer one and reports nothing itself.
//!
//! 2. **Input to present (LAT1c).** Every input the user makes on a
//!    RustKit view (page, chrome or panel) is stamped with the native
//!    event's own time (`NSEvent.timestamp`, so time spent waiting in the
//!    window server's queue while the window thread was busy counts) and
//!    its kind ([`InputMark`], [`InputClass`]). The app notes it to the
//!    view's engine before it dispatches it. Every mark noted since the
//!    view's last commit rides the next [`super::presenter::FrameCommit`],
//!    and the presenter takes one sample per mark when it presents that
//!    frame: the time from the native event to the return of the
//!    surface's present.
//!
//!    **What "reflects" means here.** The frame measured is the first
//!    frame presented after the input's turn completed whose content
//!    changed (a new display list, a new scroll offset or a new
//!    navigation). An input whose turn changed no frame (a key into
//!    nothing, a press on blank space) is not charged to a later,
//!    unrelated frame: it is counted as `no_frame`. So is one whose
//!    document a navigation replaced before it was drawn. The present
//!    is the swap chain's present call; the time the compositor then
//!    takes to put it on the glass (up to a refresh interval) is not
//!    seen. Moves are sampled: at most one per frame (the earliest since
//!    the last commit), reported only in `by_kind`, never in the headline
//!    figures, which cover keys, presses, releases, wheels and edits.
//!
//!    One line per visit (a frame of a different view or host ends it,
//!    `at=navigation`), one per [`LATENCY_SUMMARY_EVERY`] inputs
//!    (`at=every_100`), and one when the presenter goes (`at=drop`):
//!
//!    ```text
//!    Input to present view=4 host=en.wikipedia.org n=100 p50_ms=12.0 p95_ms=38.0 max_ms=612.0 by_kind=key:60:20.0,wheel:30:38.0,down:5:90.0,up:5:612.0,move:41:30.0 over_100=1 over_100_ms=612.0 no_frame=7 role=page at=every_100
//!    ```
//!
//!    `role=chrome` is the RustKit chrome or a panel (the app's own
//!    views); `role=page` is a page. The line carries no page content.

//! 3. **Hold summary (LAT1, per visit since LAT1b).** Every outermost
//!    entry on the content thread, whatever its length, is one sample of
//!    the current visit's summary; one line covers at most
//!    [`HOLD_SUMMARY_EVERY`] entries, and a navigation ([`begin_visit`],
//!    at the start of a `load` or `load_html` entry) or the engine's drop
//!    ends it early. The line names the visit's view and its host
//!    (hostname only; `-` for an HTML load or a URL without one):
//!
//!    ```text
//!    Content thread holds view=3 host=en.wikipedia.org n=100 p50_ms=0.4 p95_ms=38.2 max_ms=612.0 over_50=3 over_50_ms=612.0,96.0,51.2 by_entry=move:80:12.1,live_turn:20:96.0 at=every_100
//!    ```
//!
//!    `over_50_ms` lists the entries of 50 ms or more, longest first (`-`
//!    for none), and `by_entry` is `entry:n:p95_ms`, most entries first.
//!    So the summaries alone give the exact p95 over all entries whenever
//!    it is 50 ms or more (`tools/latency/baseline_from_logs.py`), with no
//!    hold line placed by where it fell in the log. The line carries no page
//!    content, only the view id, the hostname, entry names and
//!    milliseconds. Window-thread entries (the flag-off page, a headless
//!    capture's load and render on its worker) are no content visit and
//!    make no summary: a whole load summarised at the drop as `n=2` was
//!    the p95 of nothing the exit metric means.
//!
//! Limits: the hold log sees entries into the engine, not the app's own
//! work between them (chrome messages, the UI WebView); `window_stall.rs`
//! in the app measures those from outside. A hold inside a single
//! `Instant` read's resolution is 0.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

/// The shortest hold that is reported, in milliseconds (the W-gate's
/// figure).
pub const HOLD_THRESHOLD_MS: f64 = 50.0;

/// How many entries make one hold summary line at most (LAT1).
pub const HOLD_SUMMARY_EVERY: usize = 100;

/// How many input-to-present samples of one kind (headline inputs, or
/// sampled moves) make one summary line at most.
pub const LATENCY_SUMMARY_EVERY: usize = 100;

/// The input-to-present line lists the headline inputs that took this
/// long or longer (the exit metric's p95 bound, `trench/latency`).
pub const INPUT_THRESHOLD_MS: f64 = 100.0;

/// What the window thread was doing, for the breakdown of a hold. The
/// first five are the relayout stages (`relayout_cause::Stage`); the
/// last three are the presenter's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldStage {
    Build,
    Layout,
    DisplayList,
    Snapshot,
    Render,
    /// Waiting for the surface's next texture (`get_surface_texture`):
    /// vsync and the swap chain.
    Acquire,
    /// Image upload and `Renderer::execute`.
    Execute,
    /// `Compositor::present`.
    Present,
}

const STAGES: usize = 8;
/// The first five of [`HoldStage`]: the ones that add up to a hold.
const RELAYOUT_STAGES: usize = 5;

/// What else a hold spends its time on (dev-tools T12): the load's script
/// turns and what it waits for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HoldBucket {
    /// A script turn: parsing, compiling and running the script or callback,
    /// and its microtasks.
    Script,
    /// `Document::parse_html`.
    Parse,
    /// Parsing a fetched stylesheet.
    Style,
    /// The document's own fetch and body.
    DocFetch,
    /// External stylesheets (their fetches, less `Style`).
    Sheets,
    /// `@font-face` fetches.
    Fonts,
    /// Image fetches and decodes.
    Images,
    /// The page's scripts still arriving after the subresources were done.
    ScriptFetch,
    /// The script phase awaiting the network: fetch/XHR, imports and inserted
    /// scripts, less the script turns those run.
    ScriptNet,
}

const BUCKETS: usize = 9;

/// Milliseconds a hold spent in each stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoldMs {
    ms: [f64; STAGES],
    buckets: [f64; BUCKETS],
    script_turns: u32,
    script_longest_ms: f64,
    script_stopped: u32,
}

impl Default for HoldMs {
    fn default() -> Self {
        HOLD_MS_ZERO
    }
}

const HOLD_MS_ZERO: HoldMs =
    HoldMs { ms: [0.0; STAGES], buckets: [0.0; BUCKETS], script_turns: 0, script_longest_ms: 0.0, script_stopped: 0 };

/// Charges the time since the last charge to the bucket in progress.
fn charge() {
    ACTIVE.with(|active| {
        if let Some((bucket, since)) = active.get() {
            let now = Instant::now();
            HOLD_MS.with(|c| {
                let mut hold = c.get();
                hold.buckets[bucket as usize] += now.duration_since(since).as_secs_f64() * 1000.0;
                c.set(hold);
            });
            active.set(Some((bucket, now)));
        }
    });
}

/// Times one [`HoldBucket`] until dropped. Buckets nest and one moment is
/// charged once, to the innermost: it may be held across an `.await`, which
/// is the point (a wait is a bucket), as long as what is polled in between
/// holds no bucket of its own.
pub(crate) struct BucketTimer {
    around: Option<HoldBucket>,
}

pub(crate) fn bucket(bucket: HoldBucket) -> BucketTimer {
    charge();
    let around = ACTIVE.with(|active| active.replace(Some((bucket, Instant::now()))));
    BucketTimer { around: around.map(|(bucket, _)| bucket) }
}

impl Drop for BucketTimer {
    fn drop(&mut self) {
        charge();
        ACTIVE.with(|active| active.set(self.around.map(|bucket| (bucket, Instant::now()))));
    }
}

/// One script turn until dropped. A turn inside a turn (a nested
/// `with_script_js`, a timer run from a script) is part of the outer one.
pub(crate) struct ScriptTurn {
    timer: Option<BucketTimer>,
    spent_before: f64,
}

pub(crate) fn script_turn() -> ScriptTurn {
    let nested = ACTIVE.with(|active| matches!(active.get(), Some((HoldBucket::Script, _))));
    if nested {
        return ScriptTurn { timer: None, spent_before: 0.0 };
    }
    let timer = bucket(HoldBucket::Script);
    let spent_before = HOLD_MS.with(|c| c.get().buckets[HoldBucket::Script as usize]);
    ScriptTurn { timer: Some(timer), spent_before }
}

impl Drop for ScriptTurn {
    fn drop(&mut self) {
        // Dropping the timer charges the turn, less any relayout inside it.
        if self.timer.take().is_some() {
            HOLD_MS.with(|c| {
                let mut hold = c.get();
                let spent = hold.buckets[HoldBucket::Script as usize] - self.spent_before;
                hold.script_turns += 1;
                hold.script_longest_ms = hold.script_longest_ms.max(spent);
                c.set(hold);
            });
        }
    }
}

/// The turn in progress was stopped by the watchdog.
pub(crate) fn script_stopped() {
    HOLD_MS.with(|c| {
        let mut hold = c.get();
        hold.script_stopped += 1;
        c.set(hold);
    });
}

impl HoldMs {
    fn add(&mut self, stage: HoldStage, ms: f64) {
        self.ms[stage as usize] += ms;
    }

    /// What the stages claim: the five relayout stages only. The
    /// presenter's three run inside `render` (`Stage::Render` times all
    /// of `Engine::render`), so they are its breakdown, not more time.
    fn staged(&self) -> f64 {
        self.ms[..RELAYOUT_STAGES].iter().sum()
    }

    /// What no stage or bucket claims, of a hold that took `total_ms`.
    pub fn other_ms(&self, total_ms: f64) -> f64 {
        (total_ms - self.staged() - self.buckets.iter().sum::<f64>()).max(0.0)
    }

    #[cfg(test)]
    fn of(&self, stage: HoldStage) -> f64 {
        self.ms[stage as usize]
    }

    #[cfg(test)]
    pub(crate) fn in_bucket(&self, bucket: HoldBucket) -> f64 {
        self.buckets[bucket as usize]
    }

    #[cfg(test)]
    pub(crate) fn turns(&self) -> (u32, f64, u32) {
        (self.script_turns, self.script_longest_ms, self.script_stopped)
    }
}

impl fmt::Display for HoldMs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "build_ms={:.1} layout_ms={:.1} display_list_ms={:.1} snapshot_ms={:.1} render_ms={:.1} acquire_ms={:.1} execute_ms={:.1} present_ms={:.1} \
             script_ms={:.1} script_turns={} script_longest_ms={:.1} script_stopped={} parse_ms={:.1} style_ms={:.1} doc_fetch_ms={:.1} \
             sheets_ms={:.1} fonts_ms={:.1} images_ms={:.1} script_fetch_ms={:.1} script_net_ms={:.1}",
            self.ms[0],
            self.ms[1],
            self.ms[2],
            self.ms[3],
            self.ms[4],
            self.ms[5],
            self.ms[6],
            self.ms[7],
            self.buckets[HoldBucket::Script as usize],
            self.script_turns,
            self.script_longest_ms,
            self.script_stopped,
            self.buckets[HoldBucket::Parse as usize],
            self.buckets[HoldBucket::Style as usize],
            self.buckets[HoldBucket::DocFetch as usize],
            self.buckets[HoldBucket::Sheets as usize],
            self.buckets[HoldBucket::Fonts as usize],
            self.buckets[HoldBucket::Images as usize],
            self.buckets[HoldBucket::ScriptFetch as usize],
            self.buckets[HoldBucket::ScriptNet as usize],
        )
    }
}

thread_local! {
    /// The stage milliseconds of the hold in progress on this thread.
    static HOLD_MS: Cell<HoldMs> = const { Cell::new(HOLD_MS_ZERO) };
    /// The bucket being timed on this thread, and since when its time was
    /// last charged.
    static ACTIVE: Cell<Option<(HoldBucket, Instant)>> = const { Cell::new(None) };
    /// How many [`WindowHold`] guards are open; only the outermost
    /// reports.
    static HOLD_DEPTH: Cell<u32> = const { Cell::new(0) };
    /// Every outermost content-thread entry since the last hold summary
    /// line: its name and milliseconds (LAT1).
    static HOLD_SUMMARY: RefCell<Vec<(&'static str, f64)>> = const { RefCell::new(Vec::new()) };
    /// The visit those entries belong to: its view id and hostname (LAT1b).
    static VISIT: RefCell<Option<(u64, String)>> = const { RefCell::new(None) };
    /// Whether this is the content thread, read once per thread.
    static ON_CONTENT: bool = hold_thread_name() == "Content";
    /// The last hold summary line this thread logged, for tests.
    #[cfg(test)]
    static LAST_SUMMARY: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The last hold the outermost guard reported, for tests.
    #[cfg(test)]
    static LAST_HOLD: Cell<Option<(&'static str, f64)>> = const { Cell::new(None) };
    /// The last outermost entry that closed, reported or not, for tests.
    #[cfg(test)]
    static LAST_ENTRY: Cell<Option<&'static str>> = const { Cell::new(None) };
    /// The stages of the last outermost entry that closed, for tests.
    #[cfg(test)]
    static LAST_STAGES: Cell<HoldMs> = const { Cell::new(HOLD_MS_ZERO) };
}

/// The stages of the last outermost entry that closed on this thread, for
/// tests.
#[cfg(test)]
pub(crate) fn last_stages() -> HoldMs {
    LAST_STAGES.with(|c| c.get())
}

/// The last hold the outermost guard reported on this thread (entry,
/// milliseconds), for tests.
#[cfg(test)]
pub(crate) fn last_hold() -> Option<(&'static str, f64)> {
    LAST_HOLD.with(|c| c.get())
}

/// The last outermost entry that closed on this thread, whatever its
/// length, for tests.
#[cfg(test)]
pub(crate) fn last_entry() -> Option<&'static str> {
    LAST_ENTRY.with(|c| c.get())
}

/// Forgets both, for tests.
#[cfg(test)]
pub(crate) fn clear_last() {
    LAST_HOLD.with(|c| c.set(None));
    LAST_ENTRY.with(|c| c.set(None));
}

/// Adds `ms` of `stage` to the hold in progress (called by the relayout
/// stage timers and the presenter's).
pub(crate) fn add(stage: HoldStage, ms: f64) {
    charge();
    let around = ACTIVE.with(|active| active.get()).map(|(bucket, _)| bucket);
    HOLD_MS.with(|c| {
        let mut hold = c.get();
        hold.add(stage, ms);
        // A relayout stage ran inside the bucket in progress (a forced layout
        // in a script turn). The presenter's are already inside `render`.
        if let (Some(bucket), true) = (around, (stage as usize) < RELAYOUT_STAGES) {
            let charged = &mut hold.buckets[bucket as usize];
            *charged = (*charged - ms).max(0.0);
        }
        c.set(hold);
    });
}

/// Times one presenter stage until dropped.
pub(crate) struct StageTimer {
    stage: HoldStage,
    started: Instant,
}

pub(crate) fn time_stage(stage: HoldStage) -> StageTimer {
    StageTimer {
        stage,
        started: Instant::now(),
    }
}

impl Drop for StageTimer {
    fn drop(&mut self) {
        add(self.stage, self.started.elapsed().as_secs_f64() * 1000.0);
    }
}

/// One entry of the window thread into the engine. Dropping it ends the
/// hold; the outermost guard reports a hold of [`HOLD_THRESHOLD_MS`] or
/// more.
pub(crate) struct WindowHold {
    entry: &'static str,
    started: Instant,
    outermost: bool,
}

/// Starts timing an entry named `entry` (`live_turn`, `relayout`,
/// `scroll`, `click`, ...).
pub(crate) fn hold(entry: &'static str) -> WindowHold {
    let outermost = HOLD_DEPTH.with(|d| {
        let depth = d.get();
        d.set(depth + 1);
        depth == 0
    });
    if outermost {
        charge();
        HOLD_MS.with(|c| c.set(HoldMs::default()));
    }
    WindowHold {
        entry,
        started: Instant::now(),
        outermost,
    }
}

impl Drop for WindowHold {
    fn drop(&mut self) {
        HOLD_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        if !self.outermost {
            return;
        }
        let total_ms = self.started.elapsed().as_secs_f64() * 1000.0;
        charge();
        let stages = HOLD_MS.with(|c| c.replace(HoldMs::default()));
        #[cfg(test)]
        LAST_ENTRY.with(|c| c.set(Some(self.entry)));
        #[cfg(test)]
        LAST_STAGES.with(|c| c.set(stages));
        if let Some(line) = hold_line(hold_thread_name(), self.entry, total_ms, &stages) {
            #[cfg(test)]
            LAST_HOLD.with(|c| c.set(Some((self.entry, total_ms))));
            tracing::warn!("{line}");
        }
        if !ON_CONTENT.with(|on| *on) {
            return;
        }
        let full = HOLD_SUMMARY.with(|summary| {
            let mut summary = summary.borrow_mut();
            if summary.capacity() == 0 {
                summary.reserve_exact(HOLD_SUMMARY_EVERY);
            }
            // f64, as the hold line compares it: `over_50` counts exactly the
            // entries that logged a hold line.
            summary.push((self.entry, total_ms));
            summary.len() >= HOLD_SUMMARY_EVERY
        });
        if full {
            flush_hold_summary("every_100");
        }
    }
}

/// A navigation of `view` to `host` on this thread (LAT1b): logs the
/// visit before it, and the entries from here on are the new visit's.
/// `host` is the URL's hostname, `None` for an HTML load.
pub(crate) fn begin_visit(view: u64, host: Option<&str>) {
    flush_hold_summary("navigation");
    VISIT.with(|visit| *visit.borrow_mut() = Some((view, host.unwrap_or("-").to_string())));
}

/// The engine is going: logs the last visit and forgets it, so an engine
/// built later on this thread starts untagged.
pub(crate) fn end_visits() {
    flush_hold_summary("drop");
    VISIT.with(|visit| *visit.borrow_mut() = None);
}

/// Logs this thread's hold summary line for the entries since the last
/// one, if any, and starts a new one. `at` says what ended it:
/// `every_100`, `navigation` or `drop` (the engine's).
pub(crate) fn flush_hold_summary(at: &str) {
    let line = HOLD_SUMMARY.with(|summary| {
        let mut summary = summary.borrow_mut();
        let line = VISIT.with(|visit| {
            let visit = visit.borrow();
            hold_summary_line(visit.as_ref().map(|(view, host)| (*view, host.as_str())), &summary, at)
        });
        summary.clear();
        line
    });
    if let Some(line) = line {
        #[cfg(test)]
        LAST_SUMMARY.with(|c| *c.borrow_mut() = Some(line.clone()));
        tracing::info!("{line}");
    }
}

/// The last hold summary line this thread logged, taken, for tests.
#[cfg(test)]
pub(crate) fn take_last_summary() -> Option<String> {
    LAST_SUMMARY.with(|c| c.borrow_mut().take())
}

/// Nearest-rank percentile `p` (0..=1) of sorted `values`; 0 when empty.
fn nearest_rank(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let k = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[k - 1]
}

/// The content thread's hold summary line of `samples` (entry, ms) for
/// `visit` (view id, hostname; `-` for both before the first navigation),
/// or `None` when there are none. Pure, so its shape is testable without a
/// clock.
pub(crate) fn hold_summary_line(visit: Option<(u64, &str)>, samples: &[(&'static str, f64)], at: &str) -> Option<String> {
    if samples.is_empty() {
        return None;
    }
    let mut all: Vec<f64> = samples.iter().map(|&(_, ms)| ms).collect();
    all.sort_unstable_by(f64::total_cmp);
    let mut by_entry: Vec<(&str, Vec<f64>)> = Vec::new();
    for &(entry, ms) in samples {
        match by_entry.iter_mut().find(|(name, _)| *name == entry) {
            Some((_, values)) => values.push(ms),
            None => by_entry.push((entry, vec![ms])),
        }
    }
    for (_, values) in &mut by_entry {
        values.sort_unstable_by(f64::total_cmp);
    }
    by_entry.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));
    let by_entry = by_entry
        .iter()
        .map(|(entry, values)| format!("{entry}:{}:{:.1}", values.len(), nearest_rank(values, 0.95)))
        .collect::<Vec<_>>()
        .join(",");
    let over: Vec<String> = all
        .iter()
        .rev()
        .take_while(|&&ms| ms >= HOLD_THRESHOLD_MS)
        .map(|ms| format!("{ms:.1}"))
        .collect();
    let over_ms = if over.is_empty() { "-".to_string() } else { over.join(",") };
    let (view, host) = match visit {
        Some((view, host)) => (view.to_string(), host),
        None => ("-".to_string(), "-"),
    };
    Some(format!(
        "Content thread holds view={view} host={host} n={} p50_ms={:.1} p95_ms={:.1} max_ms={:.1} over_{}={} over_{}_ms={} by_entry={} at={}",
        all.len(),
        nearest_rank(&all, 0.5),
        nearest_rank(&all, 0.95),
        all.last().copied().unwrap_or(0.0),
        HOLD_THRESHOLD_MS as u32,
        over.len(),
        HOLD_THRESHOLD_MS as u32,
        over_ms,
        by_entry,
        at
    ))
}

/// `Content` on the page thread, `Window` everywhere else. The content
/// thread is spawned with the name `page`.
pub(crate) fn hold_thread_name() -> &'static str {
    match std::thread::current().name() {
        Some("page") => "Content",
        _ => "Window",
    }
}

/// The log line of a hold, or `None` under the threshold. Pure, so the
/// decision is testable without a clock. `thread` is [`hold_thread_name`].
pub(crate) fn hold_line(thread: &str, entry: &str, total_ms: f64, stages: &HoldMs) -> Option<String> {
    if total_ms < HOLD_THRESHOLD_MS {
        return None;
    }
    Some(format!(
        "{thread} thread hold entry={entry} ms={:.0} {stages} other_ms={:.1}",
        total_ms,
        stages.other_ms(total_ms)
    ))
}

/// What kind of input a mark stands for (LAT1c): the summary line's
/// `by_kind`. A click is a `down` and an `up`, each measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputClass {
    /// A key press or release, a Cmd chord the page heard first, or what
    /// the input method committed or composed.
    Key,
    /// A primary or secondary button press.
    Down,
    /// A primary or secondary button release.
    Up,
    /// A wheel turn or trackpad swipe (a burst queued in one turn is one
    /// input, measured from its first event).
    Wheel,
    /// An Edit menu action (copy, paste, undo, ...).
    Edit,
    /// The pointer moving over the view or leaving it. Sampled, not
    /// counted: at most one per frame (the earliest since the last
    /// commit), and outside the headline `n`/`p50`/`p95`, which are the
    /// discrete inputs the user waits on.
    Move,
}

impl InputClass {
    /// The class of a recorded input.
    pub fn of(kind: &rustkit_viewhost::InputKind) -> Self {
        use rustkit_viewhost::{InputKind, PointerInput};
        match kind {
            InputKind::Pointer(click) => match click.input {
                PointerInput::Button | PointerInput::Secondary if click.down => InputClass::Down,
                PointerInput::Button | PointerInput::Secondary => InputClass::Up,
                PointerInput::Move | PointerInput::Leave => InputClass::Move,
            },
            InputKind::Key(_) | InputKind::KeyEquivalent { .. } | InputKind::Ime(_) => InputClass::Key,
            InputKind::Wheel(_) => InputClass::Wheel,
            InputKind::Edit(_) => InputClass::Edit,
        }
    }

    /// Its name in `by_kind`.
    pub fn name(self) -> &'static str {
        match self {
            InputClass::Key => "key",
            InputClass::Down => "down",
            InputClass::Up => "up",
            InputClass::Wheel => "wheel",
            InputClass::Edit => "edit",
            InputClass::Move => "move",
        }
    }

    /// Sampled (moves) rather than counted in the headline.
    pub fn sampled(self) -> bool {
        self == InputClass::Move
    }
}

/// When the user made an input, its sequence number in the app's queue
/// (`rustkit_viewhost::InputEvent`), and what kind it was. `at` is the
/// native event's own time (`NSEvent.timestamp` on macOS, converted to an
/// `Instant`), so the time the event waited in the window server's queue
/// while the window thread was busy is counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMark {
    pub at: Instant,
    pub seq: u64,
    pub kind: InputClass,
}

/// The visit a frame belongs to (LAT1c): the page's view id and hostname,
/// as the `Content thread holds` line names them (`-` for an HTML load),
/// whether the view is the app's own (the RustKit chrome or a panel:
/// `role=chrome`) or a page (`role=page`), and the navigation (`nav_id`),
/// so a second visit to the same host is a visit of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisitTag {
    pub view: u64,
    pub host: Arc<str>,
    pub chrome: bool,
    pub nav: u64,
}

impl VisitTag {
    /// No visit yet: view 0, host `-`.
    pub fn none() -> Self {
        thread_local! {
            static DASH: Arc<str> = Arc::from("-");
        }
        VisitTag { view: 0, host: DASH.with(Arc::clone), chrome: false, nav: 0 }
    }
}

/// The presenter's input-to-present samples for the visit on screen
/// (LAT1c): one line per visit, and one per [`LATENCY_SUMMARY_EVERY`]
/// samples of either the headline inputs or the sampled moves, and one
/// when the presenter goes (`at=drop`).
#[derive(Debug, Default)]
pub(crate) struct InputLatency {
    visit: Option<VisitTag>,
    window: Vec<(InputClass, f64)>,
    headline: usize,
    moves: usize,
    /// Inputs whose turn presented nothing (the frame did not change, or
    /// a navigation replaced the document first), since the last line.
    no_frame: u64,
    /// Samples taken since the presenter was built (tests read it).
    pub(crate) samples: u64,
    /// Every sample taken, in ms (tests).
    #[cfg(test)]
    pub(crate) taken: Vec<(InputClass, f64)>,
    /// Every line logged (tests).
    #[cfg(test)]
    pub(crate) lines: Vec<String>,
}

impl InputLatency {
    /// The frame about to be presented belongs to `visit`: a different
    /// visit ends the line of the one before (`at=navigation`).
    pub(crate) fn begin(&mut self, visit: &VisitTag) {
        if self.visit.as_ref() == Some(visit) {
            return;
        }
        if self.visit.is_some() {
            self.flush("navigation");
        }
        self.visit = Some(visit.clone());
    }

    /// One input presented `ms` after it was made.
    pub(crate) fn record(&mut self, kind: InputClass, ms: f64) {
        self.samples += 1;
        #[cfg(test)]
        self.taken.push((kind, ms));
        if self.window.capacity() == 0 {
            self.window.reserve_exact(2 * LATENCY_SUMMARY_EVERY);
        }
        self.window.push((kind, ms));
        if kind.sampled() {
            self.moves += 1;
        } else {
            self.headline += 1;
        }
        if self.headline >= LATENCY_SUMMARY_EVERY || self.moves >= LATENCY_SUMMARY_EVERY {
            self.flush("every_100");
        }
    }

    /// `n` inputs of the visit on screen presented no frame of their own.
    pub(crate) fn unpresented(&mut self, n: u64) {
        self.no_frame += n;
    }

    /// Logs the line for the samples since the last one, if any.
    pub(crate) fn flush(&mut self, at: &str) {
        let line = input_summary_line(self.visit.as_ref(), &self.window, self.no_frame, at);
        self.window.clear();
        self.headline = 0;
        self.moves = 0;
        self.no_frame = 0;
        if let Some(line) = line {
            #[cfg(test)]
            self.lines.push(line.clone());
            tracing::info!("{line}");
        }
    }
}

impl Drop for InputLatency {
    fn drop(&mut self) {
        self.flush("drop");
    }
}

/// The input-to-present line of `samples` (class, ms) for `visit`, or
/// `None` when there is nothing to say. Pure, so its shape is testable
/// without a clock. `n`, `p50_ms`, `p95_ms`, `max_ms` and `over_100_ms` are
/// over the headline inputs (every class but `move`); `by_kind` is
/// `kind:n:p95_ms` for every class, most samples first, `move` included;
/// `over_100_ms` lists the headline samples of 100 ms or more, longest
/// first, so the exact p95 over many lines can be had whenever it is
/// 100 ms or more (`tools/latency/baseline_from_logs.py`).
pub(crate) fn input_summary_line(
    visit: Option<&VisitTag>,
    samples: &[(InputClass, f64)],
    no_frame: u64,
    at: &str,
) -> Option<String> {
    if samples.is_empty() && no_frame == 0 {
        return None;
    }
    let mut all: Vec<f64> = samples.iter().filter(|(k, _)| !k.sampled()).map(|&(_, ms)| ms).collect();
    all.sort_unstable_by(f64::total_cmp);
    let mut by_kind: Vec<(InputClass, Vec<f64>)> = Vec::new();
    for &(kind, ms) in samples {
        match by_kind.iter_mut().find(|(k, _)| *k == kind) {
            Some((_, values)) => values.push(ms),
            None => by_kind.push((kind, vec![ms])),
        }
    }
    for (_, values) in &mut by_kind {
        values.sort_unstable_by(f64::total_cmp);
    }
    by_kind.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.name().cmp(b.0.name())));
    let by_kind = if by_kind.is_empty() {
        "-".to_string()
    } else {
        by_kind
            .iter()
            .map(|(kind, values)| format!("{}:{}:{:.1}", kind.name(), values.len(), nearest_rank(values, 0.95)))
            .collect::<Vec<_>>()
            .join(",")
    };
    let over: Vec<String> = all
        .iter()
        .rev()
        .take_while(|&&ms| ms >= INPUT_THRESHOLD_MS)
        .map(|ms| format!("{ms:.1}"))
        .collect();
    let over_ms = if over.is_empty() { "-".to_string() } else { over.join(",") };
    let none = VisitTag::none();
    let visit = visit.unwrap_or(&none);
    let view = if visit.view == 0 { "-".to_string() } else { visit.view.to_string() };
    Some(format!(
        "Input to present view={view} host={} n={} p50_ms={:.1} p95_ms={:.1} max_ms={:.1} by_kind={by_kind} over_{}={} over_{}_ms={over_ms} no_frame={no_frame} role={} at={at}",
        visit.host,
        all.len(),
        nearest_rank(&all, 0.5),
        nearest_rank(&all, 0.95),
        all.last().copied().unwrap_or(0.0),
        INPUT_THRESHOLD_MS as u32,
        over.len(),
        INPUT_THRESHOLD_MS as u32,
        if visit.chrome { "chrome" } else { "page" },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hold_under_the_threshold_is_not_reported() {
        assert_eq!(hold_line("Window", "scroll", 49.9, &HoldMs::default()), None);
        let line = hold_line("Window", "scroll", 50.0, &HoldMs::default()).expect("reported");
        assert!(line.starts_with("Window thread hold entry=scroll ms=50 "), "{line}");
        assert!(line.ends_with(" other_ms=50.0"), "{line}");
    }

    #[test]
    fn a_hold_on_the_content_thread_is_not_labelled_as_the_window() {
        assert_eq!(hold_thread_name(), "Window");
        let label = std::thread::Builder::new()
            .name("page".into())
            .spawn(hold_thread_name)
            .expect("spawn")
            .join()
            .expect("join");
        assert_eq!(label, "Content");
        let line = hold_line("Content", "live_turn", 80.0, &HoldMs::default()).expect("reported");
        assert!(
            line.starts_with("Content thread hold entry=live_turn ms=80 "),
            "{line}"
        );
    }

    #[test]
    fn a_hold_names_what_each_stage_took_and_what_nothing_claims() {
        let mut stages = HoldMs::default();
        stages.add(HoldStage::Layout, 300.0);
        stages.add(HoldStage::Render, 40.0);
        stages.add(HoldStage::Acquire, 16.0);
        stages.add(HoldStage::Acquire, 16.0);
        let line = hold_line("Window", "live_turn", 612.0, &stages).expect("reported");
        // The presenter's three stages are a breakdown of `render`, so
        // they are not counted again: `other` is 612 - 300 - 40.
        assert_eq!(
            line,
            "Window thread hold entry=live_turn ms=612 build_ms=0.0 layout_ms=300.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=40.0 acquire_ms=32.0 execute_ms=0.0 present_ms=0.0 script_ms=0.0 script_turns=0 script_longest_ms=0.0 script_stopped=0 parse_ms=0.0 style_ms=0.0 doc_fetch_ms=0.0 sheets_ms=0.0 fonts_ms=0.0 images_ms=0.0 script_fetch_ms=0.0 script_net_ms=0.0 other_ms=272.0"
        );
        // Without the layout: a 100 ms hold with a 40 ms render whose
        // acquire took 32 has 60 ms of `other`, not 28.
        let mut draw = HoldMs::default();
        draw.add(HoldStage::Render, 40.0);
        draw.add(HoldStage::Acquire, 32.0);
        assert_eq!(draw.other_ms(100.0), 60.0, "40 ms of render, 60 of script");
        // A 100 ms turn with 40 ms of script and a 60 ms render of which
        // 55 was the execute (the review's case): the script is `other`.
        let mut turn = HoldMs::default();
        turn.add(HoldStage::Render, 60.0);
        turn.add(HoldStage::Execute, 55.0);
        assert_eq!(turn.other_ms(100.0), 40.0);
        // Nothing can push `other` below zero.
        assert_eq!(turn.other_ms(50.0), 0.0);
    }

    /// T12: `other_ms` was 88% of a 36.8 s microsoft load hold, because no
    /// stage named script or the network. They are named; `other_ms` is what
    /// is left, and it stays the line's last field.
    #[test]
    fn a_hold_names_its_buckets_and_other_is_what_is_left() {
        let mut stages = HoldMs::default();
        stages.add(HoldStage::Layout, 100.0);
        stages.buckets[HoldBucket::Script as usize] = 200.0;
        stages.script_turns = 3;
        stages.script_longest_ms = 150.0;
        stages.script_stopped = 1;
        stages.buckets[HoldBucket::Parse as usize] = 10.0;
        stages.buckets[HoldBucket::Style as usize] = 20.0;
        stages.buckets[HoldBucket::DocFetch as usize] = 30.0;
        stages.buckets[HoldBucket::Sheets as usize] = 40.0;
        stages.buckets[HoldBucket::Fonts as usize] = 50.0;
        stages.buckets[HoldBucket::Images as usize] = 60.0;
        stages.buckets[HoldBucket::ScriptFetch as usize] = 70.0;
        stages.buckets[HoldBucket::ScriptNet as usize] = 80.0;
        let line = hold_line("Content", "load", 700.0, &stages).expect("reported");
        assert_eq!(
            line,
            "Content thread hold entry=load ms=700 build_ms=0.0 layout_ms=100.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 acquire_ms=0.0 execute_ms=0.0 present_ms=0.0 script_ms=200.0 script_turns=3 script_longest_ms=150.0 script_stopped=1 parse_ms=10.0 style_ms=20.0 doc_fetch_ms=30.0 sheets_ms=40.0 fonts_ms=50.0 images_ms=60.0 script_fetch_ms=70.0 script_net_ms=80.0 other_ms=40.0"
        );
    }

    fn sleep(ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn ms_since(started: Instant) -> f64 {
        started.elapsed().as_secs_f64() * 1000.0
    }

    /// One moment is charged once: a nested bucket takes the time from the one
    /// around it, which gets it back when the nested one ends. A sleep can
    /// overrun on a loaded machine, so the upper bound is the wall clock.
    #[test]
    fn a_nested_bucket_takes_its_time_from_the_one_around_it() {
        let started = Instant::now();
        {
            let _hold = hold("load");
            let _outer = bucket(HoldBucket::Sheets);
            sleep(20);
            {
                let _inner = bucket(HoldBucket::Style);
                sleep(40);
            }
            sleep(20);
        }
        let wall = ms_since(started);
        let stages = last_stages();
        let (sheets, style) = (stages.in_bucket(HoldBucket::Sheets), stages.in_bucket(HoldBucket::Style));
        assert!(sheets >= 39.0, "{stages}");
        assert!(style >= 39.0, "{stages}");
        assert!(sheets + style <= wall, "{wall}: {stages}");
    }

    /// A relayout inside a script turn (a forced layout) is `layout_ms`, not
    /// script as well.
    #[test]
    fn a_relayout_stage_inside_a_bucket_is_not_counted_twice() {
        let started = Instant::now();
        {
            let _hold = hold("load");
            let _script = bucket(HoldBucket::Script);
            let timed = Instant::now();
            sleep(50);
            add(HoldStage::Layout, ms_since(timed));
            sleep(20);
        }
        let wall = ms_since(started);
        let stages = last_stages();
        assert!(stages.of(HoldStage::Layout) >= 50.0, "{stages}");
        let script = stages.in_bucket(HoldBucket::Script);
        assert!(script >= 19.0, "{stages}");
        assert!(script + stages.of(HoldStage::Layout) <= wall, "{wall}: {stages}");
        // The presenter's stages are a breakdown of `render`, which is what
        // comes off. They take nothing away themselves.
        {
            let _hold = hold("load");
            let _script = bucket(HoldBucket::Script);
            sleep(30);
            add(HoldStage::Acquire, 25.0);
        }
        let stages = last_stages();
        assert!(stages.in_bucket(HoldBucket::Script) >= 29.0, "{stages}");
    }

    /// A turn is counted once however deep, by what it spent itself.
    #[test]
    fn script_turns_are_counted_with_the_longest_and_the_stopped() {
        {
            let _hold = hold("load");
            {
                let _turn = script_turn();
                sleep(10);
                let _nested = script_turn();
                sleep(10);
            }
            {
                let _turn = script_turn();
                sleep(40);
                script_stopped();
            }
        }
        let stages = last_stages();
        let (turns, longest, stopped) = stages.turns();
        assert_eq!((turns, stopped), (2, 1), "{stages}");
        let script = stages.in_bucket(HoldBucket::Script);
        assert!(longest >= 39.0, "{stages}");
        assert!(script >= 59.0, "{stages}");
        assert!(longest <= script - 19.0, "the longest turn, not the sum: {stages}");
    }

    #[test]
    fn every_entry_records_its_name_and_only_a_long_one_is_reported() {
        clear_last();
        {
            let _h = hold("render_view");
            add(HoldStage::Render, 1.0);
        }
        assert_eq!(last_entry(), Some("render_view"), "recorded whatever its length");
        assert_eq!(last_hold(), None, "under the threshold: not reported");
    }

    #[test]
    fn only_the_outermost_entry_reports_and_it_counts_the_nested_stages() {
        {
            let _outer = hold("click");
            {
                let _inner = hold("relayout");
                add(HoldStage::Layout, 70.0);
                let _timed = time_stage(HoldStage::Present);
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
            // The inner guard reported nothing: its stages are still the
            // outer hold's.
            assert_eq!(LAST_HOLD.with(|c| c.get()), None);
            let stages = HOLD_MS.with(|c| c.get());
            assert_eq!(stages.of(HoldStage::Layout), 70.0);
            assert!(stages.of(HoldStage::Present) >= 60.0, "{stages}");
        }
        let (entry, ms) = LAST_HOLD.with(|c| c.take()).expect("the outer hold reported");
        assert_eq!(entry, "click");
        assert!(ms >= 60.0, "{ms}");
        assert_eq!(HOLD_DEPTH.with(|d| d.get()), 0);
        assert_eq!(HOLD_MS.with(|c| c.get()), HoldMs::default(), "cleared for the next entry");
    }

    #[test]
    fn a_short_entry_reports_nothing() {
        LAST_HOLD.with(|c| c.set(None));
        {
            let _h = hold("move");
            add(HoldStage::Build, 1.0);
        }
        assert_eq!(LAST_HOLD.with(|c| c.get()), None);
    }

    /// LAT1: ten cheap clicks and one slow one are eleven entries with a
    /// p50 under 50 ms, where the hold log alone shows only the slow one.
    #[test]
    fn the_hold_summary_counts_every_entry_by_kind() {
        let mut samples: Vec<(&'static str, f64)> = vec![("click", 5.0); 10];
        samples.push(("click", 200.0));
        samples.push(("move", 1.5));
        samples.push(("move", 2.5));
        let line = hold_summary_line(Some((7, "a.example")), &samples, "navigation").expect("a line");
        assert_eq!(
            line,
            "Content thread holds view=7 host=a.example n=13 p50_ms=5.0 p95_ms=200.0 max_ms=200.0 over_50=1 over_50_ms=200.0 by_entry=click:11:200.0,move:2:2.5 at=navigation"
        );
        assert_eq!(hold_summary_line(None, &[], "drop"), None, "no entries, no line");
        // The boundary is the hold line's: 49.99999 ms logs no hold line, so
        // it is not over (it would round to 50.0 in f32).
        let line = hold_summary_line(None, &[("move", 49.999_99), ("move", 50.0)], "drop").unwrap();
        assert!(line.contains(" over_50=1 over_50_ms=50.0 "), "{line}");
        // LAT1b: the line lists its entries of 50 ms or more, longest first,
        // so the p95 needs no hold line attributed by its log position.
        let line = hold_summary_line(None, &[("move", 60.0), ("move", 1.0), ("click", 612.0)], "drop").unwrap();
        assert!(line.contains(" over_50=2 over_50_ms=612.0,60.0 "), "{line}");
        let line = hold_summary_line(None, &[("move", 1.0)], "drop").unwrap();
        assert!(line.contains(" over_50=0 over_50_ms=- "), "{line}");
        assert!(hold_line("Content", "move", 49.999_99, &HoldMs::default()).is_none());
        // Ties in count sort by name, so the line is stable.
        let line = hold_summary_line(None, &[("scroll", 1.0), ("key", 2.0)], "drop").unwrap();
        assert!(line.contains(" by_entry=key:1:2.0,scroll:1:1.0 "), "{line}");
        // Entries before any navigation are no site's.
        assert!(line.starts_with("Content thread holds view=- host=- n=2 "), "{line}");
    }

    /// Runs `f` on a thread named as the content thread is (`page`).
    fn on_page_thread(f: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new().name("page".into()).spawn(f).expect("spawn").join().expect("join");
    }

    #[test]
    fn the_hold_summary_is_logged_every_hundred_entries_and_at_a_navigation() {
        on_page_thread(|| {
            end_visits();
            let _ = take_last_summary();
            begin_visit(5, Some("a.example"));
            assert_eq!(take_last_summary(), None, "no entries yet, no line");
            for i in 0..HOLD_SUMMARY_EVERY {
                {
                    let _h = hold(if i % 2 == 0 { "move" } else { "scroll" });
                }
                if i + 1 < HOLD_SUMMARY_EVERY {
                    assert_eq!(take_last_summary(), None, "entry {i}");
                }
            }
            let line = take_last_summary().expect("the hundredth entry summarises");
            assert!(line.starts_with("Content thread holds view=5 host=a.example n=100 "), "{line}");
            assert!(line.contains(" by_entry=move:50:"), "{line}");
            assert!(line.contains(",scroll:50:"), "{line}");
            assert!(line.ends_with(" at=every_100"), "{line}");
            // Nested entries are part of the outer one, not samples of their own.
            {
                let _outer = hold("click");
                let _inner = hold("relayout");
            }
            begin_visit(5, Some("b.example"));
            let line = take_last_summary().expect("a navigation ends the visit");
            assert!(line.starts_with("Content thread holds view=5 host=a.example n=1 "), "{line}");
            assert!(line.contains(" by_entry=click:1:"), "{line}");
            assert!(line.ends_with(" at=navigation"), "{line}");
            {
                let _load = hold("load");
            }
            begin_visit(5, None);
            let line = take_last_summary().expect("the load was the new visit's first entry");
            assert!(line.starts_with("Content thread holds view=5 host=b.example n=1 "), "{line}");
            assert!(line.contains(" by_entry=load:1:"), "{line}");
            {
                let _load = hold("load_html");
            }
            end_visits();
            let line = take_last_summary().expect("the drop ends the last visit");
            assert!(line.starts_with("Content thread holds view=5 host=- n=1 "), "{line}");
            assert!(line.contains(" by_entry=load_html:1:") && line.ends_with(" at=drop"), "{line}");
        });
    }

    /// LAT1b: the window thread's entries make no summary line.
    #[test]
    fn the_window_thread_makes_no_hold_summary() {
        end_visits();
        let _ = take_last_summary();
        begin_visit(1, Some("a.example"));
        for _ in 0..HOLD_SUMMARY_EVERY {
            let _h = hold("move");
        }
        end_visits();
        assert_eq!(take_last_summary(), None);
    }

    fn tag(view: u64, host: &str, chrome: bool) -> VisitTag {
        VisitTag { view, host: Arc::from(host), chrome, nav: 1 }
    }

    /// LAT1c: the line names the visit and the role, gives the headline
    /// figures over the discrete inputs only, and `by_kind` for every
    /// class, moves included.
    #[test]
    fn the_input_line_names_its_visit_and_keeps_moves_out_of_the_headline() {
        let samples = [
            (InputClass::Key, 20.0),
            (InputClass::Move, 900.0),
            (InputClass::Key, 40.0),
            (InputClass::Wheel, 120.0),
            (InputClass::Down, 10.0),
            (InputClass::Move, 30.0),
        ];
        let line = input_summary_line(Some(&tag(4, "a.example", false)), &samples, 2, "navigation").expect("line");
        assert_eq!(
            line,
            "Input to present view=4 host=a.example n=4 p50_ms=20.0 p95_ms=120.0 max_ms=120.0 \
             by_kind=key:2:40.0,move:2:900.0,down:1:10.0,wheel:1:120.0 over_100=1 over_100_ms=120.0 \
             no_frame=2 role=page at=navigation"
        );
        let chrome = input_summary_line(Some(&tag(3, "-", true)), &[(InputClass::Up, 5.0)], 0, "drop").expect("line");
        assert_eq!(
            chrome,
            "Input to present view=3 host=- n=1 p50_ms=5.0 p95_ms=5.0 max_ms=5.0 by_kind=up:1:5.0 \
             over_100=0 over_100_ms=- no_frame=0 role=chrome at=drop"
        );
        assert_eq!(input_summary_line(None, &[], 0, "drop"), None, "nothing to say");
        let only_misses = input_summary_line(None, &[], 3, "drop").expect("no_frame alone is said");
        assert!(only_misses.starts_with("Input to present view=- host=- n=0 p50_ms=0.0 "), "{only_misses}");
    }

    /// LAT1c: a line per hundred headline inputs (moves do not push the
    /// headline over), a line when the visit changes, a line at the drop.
    #[test]
    fn the_input_summary_flushes_every_hundred_at_a_new_visit_and_at_the_drop() {
        let a = tag(5, "a.example", false);
        let b = tag(5, "b.example", false);
        let mut latency = InputLatency::default();
        latency.begin(&a);
        for i in 0..LATENCY_SUMMARY_EVERY {
            latency.record(InputClass::Move, 1.0);
            if i + 1 < LATENCY_SUMMARY_EVERY {
                latency.record(InputClass::Key, (i + 1) as f64);
            }
        }
        // 100 moves flushed the line with 99 keys in it.
        assert_eq!(latency.lines.len(), 1);
        assert!(latency.lines[0].contains(" n=99 "), "{}", latency.lines[0]);
        assert!(latency.lines[0].contains("move:100:1.0"), "{}", latency.lines[0]);
        assert!(latency.lines[0].ends_with(" at=every_100"), "{}", latency.lines[0]);
        for _ in 0..LATENCY_SUMMARY_EVERY {
            latency.record(InputClass::Wheel, 7.0);
        }
        assert_eq!(latency.lines.len(), 2, "a hundred wheels");
        latency.record(InputClass::Key, 3.0);
        latency.unpresented(2);
        latency.begin(&a);
        assert_eq!(latency.lines.len(), 2, "the same visit");
        latency.begin(&b);
        assert_eq!(latency.lines.len(), 3, "a new visit ends the line");
        assert!(latency.lines[2].starts_with("Input to present view=5 host=a.example n=1 "), "{}", latency.lines[2]);
        assert!(latency.lines[2].ends_with(" no_frame=2 role=page at=navigation"), "{}", latency.lines[2]);
        latency.record(InputClass::Down, 9.0);
        latency.flush("drop");
        assert!(latency.lines[3].starts_with("Input to present view=5 host=b.example n=1 "), "{}", latency.lines[3]);
        assert_eq!(latency.samples, 2 * LATENCY_SUMMARY_EVERY as u64 + 99 + 1 + 1);
    }

    /// LAT1c: the class of each recorded input.
    #[test]
    fn every_recorded_input_has_a_class() {
        use rustkit_viewhost::{InputKind, PendingClick, PendingScroll, PointerInput};
        let click = |down, input| {
            InputKind::Pointer(PendingClick { x: 0.0, y: 0.0, down, input, clicks: 1, shift: false })
        };
        assert_eq!(InputClass::of(&click(true, PointerInput::Button)), InputClass::Down);
        assert_eq!(InputClass::of(&click(false, PointerInput::Button)), InputClass::Up);
        assert_eq!(InputClass::of(&click(true, PointerInput::Secondary)), InputClass::Down);
        assert_eq!(InputClass::of(&click(false, PointerInput::Move)), InputClass::Move);
        assert_eq!(InputClass::of(&click(false, PointerInput::Leave)), InputClass::Move);
        assert_eq!(
            InputClass::of(&InputKind::Wheel(PendingScroll { x: 0.0, y: 0.0, dx: 0.0, dy: 1.0 })),
            InputClass::Wheel
        );
        assert_eq!(InputClass::of(&InputKind::Edit(rustkit_viewhost::EditAction::Copy)), InputClass::Edit);
    }

    /// LAT1c overhead: recording a sample is a push and, every hundred,
    /// a sort of a hundred values and one line. Measured here so the PR
    /// can state it; the bound is loose (a debug build on a loaded Mac).
    #[test]
    fn recording_an_input_sample_is_cheap() {
        let a = tag(1, "a.example", false);
        let mut latency = InputLatency::default();
        latency.begin(&a);
        let n = 200_000u32;
        let started = Instant::now();
        for i in 0..n {
            latency.record(if i % 3 == 0 { InputClass::Move } else { InputClass::Key }, f64::from(i % 97));
        }
        let per = started.elapsed().as_nanos() as f64 / f64::from(n);
        latency.taken.clear();
        latency.lines.clear();
        eprintln!("LAT1c overhead: {per:.0} ns per recorded input (incl. a line every 100)");
        assert!(per < 20_000.0, "{per} ns per input");
    }
}
