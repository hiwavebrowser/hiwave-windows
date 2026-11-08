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
//! 2. **Input to present.** The app notes each input event's arrival
//!    time as it drains its queue ([`InputMark`]); the first unpresented
//!    one rides the next [`super::presenter::FrameCommit`] and the
//!    presenter logs the time from arrival to the surface present:
//!
//!    ```text
//!    Input to present ms=41 seq=1207
//!    Input to present p50_ms=12 p95_ms=38 max_ms=612 n=100   (every 100 inputs)
//!    ```
//!
//!    An input that changed no frame (a move over nothing) is dropped at
//!    the next `render_changed_views`, not charged to the next frame.
//!
//! Limits: the hold log sees entries into the engine, not the app's own
//! work between them (chrome messages, the UI WebView); `window_stall.rs`
//! in the app measures those from outside. A hold inside a single
//! `Instant` read's resolution is 0.

use std::cell::Cell;
use std::fmt;
use std::time::Instant;

/// The shortest hold that is reported, in milliseconds (the W-gate's
/// figure).
pub const HOLD_THRESHOLD_MS: f64 = 50.0;

/// How many input-to-present samples make one summary line.
pub const LATENCY_SUMMARY_EVERY: usize = 100;

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
    }
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

/// When an input event arrived at the app's queue, and its sequence
/// number there (`rustkit_viewhost::InputEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputMark {
    pub at: Instant,
    pub seq: u64,
}

/// Input-to-present samples, summarised every [`LATENCY_SUMMARY_EVERY`].
#[derive(Debug, Default)]
pub(crate) struct InputLatency {
    window: Vec<u32>,
    /// Samples taken since the presenter was built (tests read it).
    pub(crate) samples: u64,
}

impl InputLatency {
    /// Records one sample; returns the summary line when the window is
    /// full.
    pub(crate) fn record(&mut self, ms: u32) -> Option<String> {
        self.samples += 1;
        self.window.push(ms);
        if self.window.len() < LATENCY_SUMMARY_EVERY {
            return None;
        }
        let line = summary_line(&self.window);
        self.window.clear();
        Some(line)
    }
}

/// `p50_ms p95_ms max_ms n` of `samples` (nearest-rank percentiles).
pub(crate) fn summary_line(samples: &[u32]) -> String {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = |p: f64| -> u32 {
        if sorted.is_empty() {
            return 0;
        }
        let k = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
        sorted[k - 1]
    };
    format!(
        "Input to present p50_ms={} p95_ms={} max_ms={} n={}",
        rank(0.5),
        rank(0.95),
        sorted.last().copied().unwrap_or(0),
        sorted.len()
    )
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

    #[test]
    fn latency_summarises_every_hundred_samples_with_nearest_rank_percentiles() {
        let mut latency = InputLatency::default();
        for i in 1..LATENCY_SUMMARY_EVERY as u32 {
            assert_eq!(latency.record(i), None, "sample {i}");
        }
        let line = latency.record(612).expect("the hundredth sample summarises");
        assert_eq!(line, "Input to present p50_ms=50 p95_ms=95 max_ms=612 n=100");
        assert_eq!(latency.samples, 100);
        assert_eq!(latency.record(3), None, "a new window");
        assert_eq!(summary_line(&[]), "Input to present p50_ms=0 p95_ms=0 max_ms=0 n=0");
        assert_eq!(summary_line(&[7]), "Input to present p50_ms=7 p95_ms=7 max_ms=7 n=1");
    }
}
