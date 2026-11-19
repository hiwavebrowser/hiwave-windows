//! Headless frame capture tool for parity testing.
//!
//! This tool renders HTML files using RustKit's headless mode and exports:
//! - PPM frame capture
//! - Layout tree JSON
//! - Performance metrics
//!
//! Unlike hiwave-smoke, this does NOT require a display and can run in CI.

mod inspector;
mod report;

use clap::Parser;
use rustkit_engine::inspector::{CollectingSink, JsonLinesSink};
use rustkit_engine::{EngineBuilder, EngineConfig, Inspector, InspectorSink, InspectorSummary, ScriptOutcome, ScriptRecord};
use std::sync::Arc;
use rustkit_viewhost::Bounds;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use tracing::{error, warn};
use url::Url;

#[derive(Parser, Debug, Clone)]
#[command(name = "parity-capture")]
#[command(about = "Headless frame capture for parity testing")]
#[command(group(clap::ArgGroup::new("source").required(true).args(["html_file", "url", "inspector_dump"])))]
struct Args {
    /// Path to HTML file to render
    #[arg(long)]
    html_file: Option<String>,

    /// Live URL to load through `Engine::load_url` (document + subresources
    /// over the network), for the real-site board
    #[arg(long)]
    url: Option<String>,

    /// Collect a WPT completion record from the loopback test server.
    #[arg(long, requires = "url", conflicts_with_all = ["shield", "replay_proxy", "prelude_url"])]
    wpt: bool,

    /// Hard wall-clock limit for the whole capture, in milliseconds. On
    /// expiry the process prints a `timeout` result and exits 3.
    #[arg(long, default_value = "30000")]
    timeout_ms: u64,

    /// Output path for display-list JSON (paint commands, including text runs)
    #[arg(long)]
    dump_display_list: Option<String>,

    /// Output path for the page's script log (URL mode: one record per
    /// `<script>`, plus exceptions from lifecycle listeners and timers)
    #[arg(long)]
    dump_scripts: Option<String>,

    /// Viewport width
    #[arg(long, default_value = "1280")]
    width: u32,

    /// Viewport height
    #[arg(long, default_value = "800")]
    height: u32,

    /// After loading at --width x --height, resize the view to `WxH` (through
    /// `Engine::resize_view`, as a window resize does) and capture at the new
    /// size. Compare against a fresh load at `WxH` to find layout that stays
    /// stale across a resize.
    #[arg(long, value_parser = parse_size)]
    resize_to: Option<(u32, u32)>,

    /// Output path for PPM frame
    #[arg(long)]
    dump_frame: Option<String>,

    /// Output path for layout JSON
    #[arg(long)]
    dump_layout: Option<String>,

    /// JSON string or path to JSON file defining interaction sequence
    /// (wait/click/hover/key/resize/capture; `key` takes `modifiers`)
    #[arg(long)]
    actions: Option<String>,

    /// Directory for action capture frames
    #[arg(long)]
    actions_out_dir: Option<String>,

    /// Optional HTTP replay proxy URL (e.g. http://127.0.0.1:8989) for deterministic HAR replay.
    /// Test-only; routes outbound requests to the local replay server while preserving document origin.
    #[arg(long)]
    replay_proxy: Option<String>,

    /// Virtual timer clock horizon in milliseconds (default: 5000).
    #[arg(long)]
    timer_horizon_ms: Option<u64>,

    /// Wall-clock budget for the page's scripts in milliseconds (default: 5000, the
    /// engine's and the board's). The live app runs pages at 60000; a capture taken
    /// with another budget is not comparable with the board and must be labelled.
    #[arg(long)]
    script_budget_ms: Option<u64>,

    /// The live turn's watchdog, in ms (`EngineConfig::live_turn_budget_ms`, 5000 by default): how long
    /// one callback's JavaScript may run before it is stopped, and also the turn's whole allowance of
    /// script time, so a raised value paces frames differently even when no single callback is long.
    /// Needs `--interrupt-scripts`; 1 to 3600000 (an hour). For stress-suite runs only (tools/stress,
    /// `HIWAVE_STRESS_TURN_BUDGET_MS`): MotionMark's end-of-test analysis runs longer than the default and
    /// the stop ends the suite. A capture taken with it is not comparable with the board or the app: it says
    /// so on stderr, and tools/stress records the budget in the run.
    #[arg(long, requires = "interrupt_scripts", value_parser = clap::value_parser!(u64).range(1..=MAX_LIVE_TURN_BUDGET_MS))]
    live_turn_budget_ms: Option<u64>,

    /// Stop a script that is still running when the script budget is spent, as the
    /// live app does (off by default: the board lets a running script finish). A
    /// capture taken with it is not comparable with the board and must be labelled.
    #[arg(long)]
    interrupt_scripts: bool,

    /// After the load, turn the live loop as the app does for this many
    /// milliseconds of real time (timers, animation frames, observers,
    /// script requests, relayouts) and report what the turns did as
    /// `live_stats`. With `--html-file` it also runs the file's scripts, as
    /// `--run-inline-scripts` does. Off by default; a frame taken with it
    /// is not comparable with the board.
    #[arg(long)]
    live_ms: Option<u64>,

    /// Pace `--live-ms` as the app's window thread does with the content thread off
    /// (`HIWAVE_CONTENT_THREAD=0`, `hiwave-app` `process_events`): every wait for a timer or a request
    /// at least as long as the last turn took, so after one long turn the next frame waits as long
    /// again. Without it the loop waits as the app's default loop, the content thread's, waits
    /// (`rustkit_engine::content_turn_wait`): until the next timer, 2 to 50 ms, whatever the last turn
    /// took. Only the wait is the app's: the capture's turn is not the content thread's turn (that loop
    /// also drops each turn's sub-millisecond remainder and publishes frames), so a headless live
    /// measurement approximates the app's pacing; it does not copy it. The content thread's wait does
    /// not wake when a fetch arrives, so with no timer set a response is handled up to 50 ms later than
    /// with this flag. Recorded in `live_stats.pacing`.
    #[arg(long)]
    window_thread_pacing: bool,

    /// With `--live-ms`: print one `Frame stages ...` line per live turn to stderr
    /// (`rustkit_engine::frame_stage_line`): the turn's busy time and the wait before it, and what
    /// its script, canvas raster, build, layout, display list, render and the rest took. For a
    /// stress run's stage split (tools/stress `--frame-stages`); numbers and names only. Every 5
    /// turns and at the end, a `Host calls turn=N rows=...` line: the thread's running host call
    /// rows (host function / operation : calls : ms), for the split of script into JS and host calls.
    #[arg(long, requires = "live_ms")]
    frame_stages: bool,

    /// Load as the app does: what the page asks of the network after its
    /// `load` event is the live loop's (`--live-ms`), not the load's
    /// (`EngineConfig::post_load_requests_live`).
    #[arg(long)]
    post_load_live: bool,

    /// Run the page's CSS animations and transitions as the app does (the
    /// parity config holds them still so a frame is reproducible). For
    /// `--live-ms` measurements of animation frames; a frame taken with it
    /// is not comparable with the board.
    #[arg(long)]
    animations: bool,

    /// Step the page clock in a load-only capture: `performance.now()`, frame and event timestamps are
    /// virtual time plus 0.1 ms per read, with no real time in them, so two captures of a page read the
    /// same values. Off by default: the page reads develop's real-flowing clock (batch 75: under the
    /// stepped clock squarespace's capture drew a blank frame). Never with `--live-ms`.
    #[arg(long)]
    deterministic_clock: bool,

    /// Before the `--url` load, load this URL in the same view and turn the
    /// live loop for `--prelude-live-ms`, as a page visited earlier in a
    /// browsing session would have been; then navigate to `--url`. For
    /// measuring what one document leaves to the next (LAT14: a Web
    /// Animation of the previous page). A frame taken with it is not
    /// comparable with the board.
    #[arg(long)]
    prelude_url: Option<String>,

    /// How long the prelude page's live loop turns, in milliseconds [default: 500]
    #[arg(long)]
    prelude_live_ms: Option<u64>,

    /// Write the view's Inspector event log for the whole capture to this
    /// file, as JSON Lines (one record per line; see
    /// docs/INSPECTOR_EVENT_LOG.md). URLs lose their query and fragment.
    #[arg(long)]
    event_log: Option<String>,

    /// Add `event_summary` to the result: counts over the Inspector events
    /// (console calls by level, script errors, unsupported APIs by member,
    /// requests by type and outcome, failures by stage).
    #[arg(long)]
    event_summary: bool,

    /// Keep query strings and fragments in the event log's URLs. They can
    /// carry session ids and tokens: never for a log that will be shared.
    #[arg(long)]
    event_log_keep_query: bool,

    /// Keep the text of console messages in the event log (by default a
    /// message is its arguments' types and string lengths). Pages log
    /// cookies and form values: never for a log that will be shared.
    #[arg(long)]
    event_log_console_text: bool,

    /// Keep the message text of the page's errors in the event log (by
    /// default an error is its type, a standard name and its message's
    /// length). Pages throw cookies and form values: never for a log that
    /// will be shared.
    #[arg(long)]
    event_log_error_text: bool,

    /// Append the page's console calls, uncaught exceptions and unhandled
    /// rejections to this file as JSON Lines, with the text the page wrote
    /// (the command-line form of `HIWAVE_CONSOLE_LOG=<path>`; see
    /// docs/INSPECTOR_EVENT_LOG.md). The file holds page content (cookies,
    /// tokens, form values): never for a log that will be shared.
    #[arg(long, value_name = "PATH")]
    console_log: Option<std::path::PathBuf>,

    /// Filter subresources through the app's shield (EasyList, EasyPrivacy
    /// and the interim domain floor), so a block shows in the event log as
    /// `failure.blocked_by`. Off by default: the board runs unfiltered.
    #[arg(long)]
    shield: bool,

    /// With `--shield`: wait up to this long for the filter lists to be
    /// built before loading. Until they are, blocks come from the domain
    /// floor (`blocked_by.list = "domain-floor"`).
    #[arg(long, default_value_t = 30_000, value_name = "MS")]
    shield_wait_ms: u64,

    /// Print the element diagnostics record (JSON) of the first element
    /// matching this CSS selector. Repeatable.
    #[arg(long, value_name = "SELECTOR")]
    diagnose: Vec<String>,

    /// Print the diagnostics record of the element hit at viewport point
    /// `X,Y` (CSS px). Repeatable.
    #[arg(long, value_name = "X,Y", value_parser = parse_point_arg)]
    diagnose_at: Vec<(f32, f32)>,

    /// Write a diagnostics record for every element (with or without a
    /// layout box) to this file.
    #[arg(long)]
    dump_diagnostics: Option<String>,

    /// Cap on the number of records `--dump-diagnostics` writes; the dump
    /// says `truncated: true` when the page had more elements.
    #[arg(long, default_value_t = rustkit_engine::DEFAULT_MAX_ELEMENTS)]
    diagnostics_max_elements: usize,

    /// Keep the page URL's query and fragment in diagnostics records
    /// (stripped by default).
    #[arg(long)]
    diagnostics_keep_query: bool,

    /// Write the page report (JSON): page state, covering boxes, content
    /// per viewport band and a ranked "most likely cause" list, from the
    /// final layout (dev-tools T4).
    #[arg(long, value_name = "FILE")]
    report: Option<String>,

    /// Write the page report as one page of text.
    #[arg(long, value_name = "FILE")]
    report_text: Option<String>,

    /// Turn script on for an --html-file load, so the load runs the page's
    /// inline classic scripts (fixture mode otherwise runs none), as for a
    /// built-in page a host loads directly. A script that throws fails the
    /// capture.
    #[arg(long)]
    run_inline_scripts: bool,

    /// Render the HiWave Inspector for this capture's diagnostics dump into
    /// this directory: inspector.html (the page with its input),
    /// inspector.png and inspector.display-list.json.
    #[arg(long, value_name = "DIR")]
    inspector_out: Option<String>,

    /// Show this dump (as --dump-diagnostics writes it) in the Inspector
    /// instead of capturing a page. Needs --inspector-out.
    #[arg(long, value_name = "FILE", requires = "inspector_out", conflicts_with_all = ["html_file", "url"])]
    inspector_dump: Option<String>,

    /// An inspector_diff.py result (tools/inspector) for the Inspector to
    /// mark and compare.
    #[arg(long, value_name = "FILE", requires = "inspector_out")]
    inspector_diff: Option<String>,

    /// Select the captured page's element at viewport point `X,Y` (CSS px)
    /// in the Inspector, as click-to-inspect will.
    #[arg(long, value_name = "X,Y", value_parser = parse_point_arg, requires = "inspector_out", conflicts_with = "inspector_dump")]
    inspector_select_at: Option<(f32, f32)>,

    /// Inspector viewport, `WxH`.
    #[arg(long, value_name = "WxH", value_parser = parse_size, default_value = "1280x800")]
    inspector_size: (u32, u32),

    /// Enable verbose output
    #[arg(long, short)]
    verbose: bool,
}

#[derive(Serialize, Deserialize)]
struct CaptureResult {
    status: String,
    html_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    url: Option<String>,
    width: u32,
    height: u32,
    frame_path: Option<String>,
    layout_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    display_list_path: Option<String>,
    layout_stats: Option<LayoutStats>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    script_stats: Option<ScriptStats>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    live_stats: Option<LiveStats>,
    /// A stepped capture's settle: how many requests the page still had open when the settle stopped
    /// holding virtual time for them (its real-time cap ran out). A count only, never a URL; absent when none.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    settle_held_open: Option<usize>,
    /// A stepped capture's settle, what it cost in real time (shapes only): turns, wall and wait time,
    /// the slowest turn, timer callbacks and relayouts.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    settle_stats: Option<SettleStats>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    elapsed_ms: Option<u64>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    captures: Option<Vec<StepCapture>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    action_results: Option<Vec<ActionResult>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    event_log_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    event_summary: Option<InspectorSummary>,
    /// `--diagnose` / `--diagnose-at` records, in argument order; `null`
    /// where nothing matched.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    diagnostics: Option<Vec<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    diagnostics_path: Option<String>,
    /// `--inspector-out`
    #[serde(skip_serializing_if = "Option::is_none", default)]
    inspector: Option<inspector::InspectorOutput>,
    /// `--report`: the layout half, built while the view exists; the event
    /// rules are added once the load's records are in (`run_capture`).
    #[serde(skip)]
    page_report: Option<report::Report>,
    /// `--live-ms`: milliseconds from the load's start to the window's end.
    #[serde(skip)]
    capture_ms: Option<f64>,
    /// True when this capture ran on the content thread (`HIWAVE_CONTENT_THREAD`).
    /// Absent means an older capture tool. `false` is the flag-off worker.
    #[serde(default)]
    content_thread: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActionItem {
    #[serde(default)]
    step: Option<usize>,
    #[serde(rename = "type")]
    action_type: String,
    #[serde(default)]
    ms: Option<u64>,
    #[serde(default)]
    selector: Option<String>,
    #[serde(default)]
    x: Option<f32>,
    #[serde(default)]
    y: Option<f32>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    frame: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// `key` only: modifier names held while the key is pressed, as
    /// Playwright spells them (`Control`, `Shift`, `Alt`, `Meta`).
    #[serde(default)]
    modifiers: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActionsPayload {
    #[serde(default)]
    actions: Vec<ActionItem>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct StepCapture {
    step: usize,
    label: String,
    frame: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct ActionResult {
    step: usize,
    #[serde(rename = "type")]
    action_type: String,
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    frame: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selector_used: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<String>,
    elapsed_ms: u64,
}

impl CaptureResult {
    fn new(args: &Args) -> Self {
        CaptureResult {
            status: "ok".to_string(),
            html_file: args.html_file.clone(),
            url: args.url.clone(),
            width: args.width,
            height: args.height,
            frame_path: None,
            layout_path: None,
            display_list_path: None,
            layout_stats: None,
            script_stats: None,
            live_stats: None,
            settle_held_open: None,
            settle_stats: None,
            elapsed_ms: None,
            error: None,
            captures: None,
            action_results: None,
            event_log_path: None,
            event_summary: None,
            diagnostics: None,
            diagnostics_path: None,
            inspector: None,
            page_report: None,
            capture_ms: None,
            content_thread: false,
        }
    }

    fn failed(mut self, status: &str, error: String) -> Self {
        self.status = status.to_string();
        self.error = Some(error);
        self
    }
}

/// Totals over a URL capture's script log.
#[derive(Serialize, Deserialize)]
struct ScriptStats {
    ran: u32,
    threw: u32,
    skipped: u32,
    fetch_failed: u32,
    over_budget: u32,
    bytes: u64,
    elapsed_ms: u64,
}

/// What the live loop did after the load (`--live-ms`).
#[derive(Serialize, Deserialize, Default)]
struct LiveStats {
    /// Real time the loop was given.
    wall_ms: u64,
    /// Of it, time spent inside turns: the window thread's time in the app.
    busy_ms: u64,
    turns: u32,
    timer_callbacks: u64,
    requests: u64,
    relayouts: u32,
    /// The page had no timer set and nothing in flight when the loop ended.
    idle_at_end: bool,
    /// Whose wait the loop used between turns: "content-thread" (the content thread's wait, the
    /// app's default) or "window-thread" (`--window-thread-pacing`). Empty in a record from before PACE1, which paced
    /// as the window thread.
    #[serde(default)]
    pacing: String,
}

/// Wakes the live loop's sleep when work arrives off the page thread, as the
/// app's event loop is woken (`rustkit_engine::set_live_wake`): an inserted
/// script's fetch finishing is not `in_flight`, so without it a page with no
/// timer set slept the rest of `--live-ms` and its inserted scripts ran only
/// on the last turn.
#[derive(Default)]
struct LiveWake {
    woken: std::sync::Mutex<bool>,
    arrived: std::sync::Condvar,
}

impl LiveWake {
    /// The process's one waker, installed in the engine on first use.
    fn installed() -> Arc<LiveWake> {
        static WAKE: std::sync::OnceLock<Arc<LiveWake>> = std::sync::OnceLock::new();
        WAKE.get_or_init(|| {
            let wake = Arc::new(LiveWake::default());
            let engine_side = Arc::clone(&wake);
            rustkit_engine::set_live_wake(Arc::new(move || engine_side.wake()));
            wake
        })
        .clone()
    }

    fn wake(&self) {
        *self.woken.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.arrived.notify_one();
    }

    /// Sleep up to `limit`, less if woken (or woken since the last sleep).
    fn sleep(&self, limit: Duration) {
        let woken = self.woken.lock().unwrap_or_else(|e| e.into_inner());
        let (mut woken, _) = self
            .arrived
            .wait_timeout_while(woken, limit, |woken| !*woken)
            .unwrap_or_else(|e| e.into_inner());
        *woken = false;
    }
}

/// Which of the app's live loops `run_live` takes its wait between turns from.
#[derive(Clone, Copy, PartialEq, Debug)]
enum LivePacing {
    /// The content thread's wait (`page_protocol` `PageHalf::run`, the app's default loop): after a
    /// turn, wait for the next timer, 2 to 50 ms (`rustkit_engine::content_turn_wait`), however long
    /// the turn took. The engine's wake is not consulted, as in that loop.
    ContentThread,
    /// The window thread's loop with the content thread off (`hiwave-app` `process_events`): a turn
    /// when the next timer is due, a request is out or the engine wakes the loop. Timer and request
    /// waits are never shorter than the last turn took (nor `MIN_LIVE_TURN`), so the page holds the
    /// window's thread at most half the time. The engine's wake skips that floor and ends the wait at
    /// once, as the app's poke does, so a page that keeps waking the loop gets back-to-back turns.
    WindowThread,
}

impl LivePacing {
    fn of(args: &Args) -> Self {
        if args.window_thread_pacing {
            LivePacing::WindowThread
        } else {
            LivePacing::ContentThread
        }
    }

    fn name(self) -> &'static str {
        match self {
            LivePacing::ContentThread => "content-thread",
            LivePacing::WindowThread => "window-thread",
        }
    }
}

/// How long a stepped capture lets the page run after its load, in virtual time. The Chrome reference
/// waits this long between `load` and its screenshot (`CHROME_SETTLE_MS` in scripts/realsite_board.py, the
/// default of tools/parity_oracle/realsite.mjs), so a timer the page sets after the load's timer pass (an
/// A/B test settled by `setTimeout`, an entrance reveal) has run by the time ours is taken. The same span on
/// every run: the page clock in a stepped capture is virtual.
const STEPPED_SETTLE_MS: u64 = 5_000;

/// The most real time a stepped settle may take: the reference's own wait (5 s of real time). A settle
/// whose turns are slow (a page that relays itself out on every step, at up to a second a turn) stops
/// between turns once this is spent, and reports how far its virtual time got (batch 74's A/B: github's
/// settle took over 77 s, squarespace's 36 s). Fast pages finish their virtual span well inside it.
const SETTLE_REAL_BUDGET: Duration = Duration::from_secs(5);

/// Real time a capture keeps, after its settle, for the layout, the frame and the dumps within
/// `--timeout-ms`: the settle's budget shrinks when the load has left less than this plus the budget.
const SETTLE_TIMEOUT_MARGIN: Duration = Duration::from_secs(5);

/// The capture's outputs as the load left the page, taken before a stepped settle: if the settle is cut
/// short, these replace the outputs taken after it, so a capture never shows a page part-way through its
/// own reveal (batch 75: squarespace's content hidden by the first settle turn, its reveal never reached).
/// Each entry is (side file, requested output path).
struct PreSettle {
    files: Vec<(String, String)>,
    layout: Option<String>,
}

impl PreSettle {
    /// Render the view and write each requested output (frame, layout, display list) beside its path.
    /// `None` when nothing is requested or a step fails: there is nothing to fall back to, and the side
    /// files already written are removed (`Drop`).
    fn take(engine: &mut rustkit_engine::Engine, view_id: rustkit_engine::EngineViewId, args: &Args) -> Option<Self> {
        if args.dump_frame.is_none() && args.dump_layout.is_none() && args.dump_display_list.is_none() {
            return None;
        }
        engine.render_view(view_id).ok()?;
        let side = |path: &String| format!("{path}.pre-settle");
        // Built as it goes, so a failure part-way drops it and its side files with it.
        let mut pre = Self { files: Vec::new(), layout: args.dump_layout.clone() };
        if let Some(ref path) = args.dump_frame {
            pre.files.push((side(path), path.clone()));
            engine.capture_frame(view_id, &side(path)).ok()?;
        }
        if let Some(ref path) = args.dump_layout {
            pre.files.push((side(path), path.clone()));
            engine.export_layout_json(view_id, &side(path)).ok()?;
        }
        if let Some(ref path) = args.dump_display_list {
            pre.files.push((side(path), path.clone()));
            engine.export_display_list_json(view_id, &side(path)).ok()?;
        }
        Some(pre)
    }

    /// The settle was cut short: the outputs taken before it replace the ones taken after. True only when
    /// every one was restored; the result's paths and layout stats then describe the restored files.
    fn restore(self, result: &mut CaptureResult, args: &Args) -> bool {
        let mut all = true;
        for (side, path) in &self.files {
            if let Err(e) = fs::rename(side, path) {
                error!("Failed to restore the pre-settle output: {:?}", e);
                all = false;
            }
        }
        if all {
            result.frame_path = args.dump_frame.clone().or(result.frame_path.take());
            result.layout_path = args.dump_layout.clone().or(result.layout_path.take());
            result.display_list_path = args.dump_display_list.clone().or(result.display_list_path.take());
            if let Some(ref path) = self.layout {
                if let Ok(layout_json) = fs::read_to_string(path) {
                    result.layout_stats = analyze_layout_json(&layout_json);
                }
            }
        }
        all
    }
}

impl Drop for PreSettle {
    /// Whatever side file is still there (the settle finished, a restore failed, or the capture ended
    /// early) is removed.
    fn drop(&mut self) {
        for (side, _) in &self.files {
            let _ = fs::remove_file(side);
        }
    }
}

/// What a stepped settle cost (`CaptureResult::settle_stats`).
#[derive(Serialize, Deserialize, Default, Clone, Debug)]
struct SettleStats {
    /// Turns pumped (steps and 0 ms waiting turns).
    turns: u32,
    /// Real time the settle took, all of it.
    wall_ms: u64,
    /// Real time spent waiting for open requests (bounded by the cap).
    wait_ms: u64,
    /// The slowest single turn.
    max_turn_ms: u64,
    /// Turns that relaid the page out.
    relayouts: u32,
    /// Timer callbacks that ran.
    timer_callbacks: u64,
    /// Requests still open once the cap ran out.
    held_open: usize,
    /// How far the page's virtual time got (`STEPPED_SETTLE_MS` unless cut short).
    virtual_ms: u64,
    /// The settle stopped at its real-time budget before `STEPPED_SETTLE_MS` of virtual time.
    cut_short: bool,
    /// The settle was cut short, so the frame, display list and layout are the ones taken before it (the
    /// page as the load left it), not a state part-way through the page's own reveal.
    fell_back: bool,
}

/// Run `STEPPED_SETTLE_MS` of virtual time in fixed steps. While a request the page started is open,
/// virtual time stands still: turns of 0 ms (real waiting, bounded by a cap across the settle) until it
/// answers, then the steps go on. So the answer lands at the same virtual time whatever the server's
/// speed, and a timer the page sets from it (fetch config, then `setTimeout(reveal, c.delay)`) is due at
/// the same virtual time on every run. Past the cap the request is left open and the settle goes on.
///
/// It also stops, between turns, when `budget` of real time is spent or the next turn would likely end
/// past it (the time spent plus the slowest turn so far). `budget` is `SETTLE_REAL_BUDGET`, less when the
/// capture's timeout has less left. A page whose turns are slow gets a bounded settle, cut short and
/// reported (`cut_short`, `virtual_ms`); a page whose span fits in the budget settles the same every run.
///
/// Returns what it cost (`SettleStats`), including how many requests were still open once the cap ran
/// out: a page with a request that never answers (a long poll, an event stream) pays the cap once.
fn settle_stepped(
    rt: &tokio::runtime::Runtime,
    engine: &mut rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    budget: Duration,
) -> SettleStats {
    const STEP_MS: u64 = 50;
    const REQUEST_WAIT: Duration = Duration::from_millis(5);
    const REQUEST_WAIT_CAP: Duration = Duration::from_secs(3);
    let began = Instant::now();
    let mut stats = SettleStats::default();
    let mut left = STEPPED_SETTLE_MS;
    // Real time spent waiting, measured: a 5 ms sleep can last about 15 ms (Windows), and the pump counts.
    let mut waited = Duration::ZERO;
    let mut turn = |stats: &mut SettleStats, elapsed_ms: u64| {
        let t = Instant::now();
        let out = rt.block_on(engine.pump_live(view_id, elapsed_ms));
        stats.turns += 1;
        stats.max_turn_ms = stats.max_turn_ms.max(t.elapsed().as_millis() as u64);
        stats.relayouts += out.relaid_out as u32;
        stats.timer_callbacks += out.timers_ran as u64;
        out.in_flight
    };
    // No budget (the load used the capture's timeout down to the margin): no turn at all.
    if budget.is_zero() {
        stats.cut_short = true;
        return stats;
    }
    // A request the load left open counts too: a 0 ms turn reports it without moving the clock.
    let mut in_flight = turn(&mut stats, 0);
    while left > 0 {
        while in_flight > 0
            && waited < REQUEST_WAIT_CAP
            && began.elapsed() + Duration::from_millis(stats.max_turn_ms) < budget
        {
            let t = Instant::now();
            std::thread::sleep(REQUEST_WAIT);
            in_flight = turn(&mut stats, 0);
            waited += t.elapsed();
        }
        if in_flight > 0 {
            stats.held_open = stats.held_open.max(in_flight);
        }
        // Between turns only: a turn already running is not interrupted, so a turn that would likely end
        // past the budget (the time spent plus the slowest turn so far) is not started.
        if began.elapsed() + Duration::from_millis(stats.max_turn_ms) > budget {
            stats.cut_short = true;
            break;
        }
        let step = STEP_MS.min(left);
        in_flight = turn(&mut stats, step);
        left -= step;
    }
    stats.virtual_ms = STEPPED_SETTLE_MS - left;
    stats.wait_ms = waited.as_millis() as u64;
    stats.wall_ms = began.elapsed().as_millis() as u64;
    stats
}

/// Turn the live loop for `live_ms` of real time, paced as one of the app's loops (`LivePacing`).
fn run_live(
    rt: &tokio::runtime::Runtime,
    engine: &mut rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    live_ms: u64,
    pacing: LivePacing,
) -> LiveStats {
    const MIN_LIVE_TURN: Duration = Duration::from_millis(4);
    const LIVE_REQUEST_POLL: Duration = Duration::from_millis(10);
    let limit = Duration::from_millis(live_ms);
    let began = Instant::now();
    // The app's content-thread clock: whole ms a turn, the fraction kept, and the next turn due
    // from the turn's time (PACE2). The window-thread arm keeps its own wait.
    let mut clock = rustkit_engine::LiveClock::new(began);
    let mut stats = LiveStats {
        pacing: pacing.name().to_string(),
        ..LiveStats::default()
    };
    let wake = LiveWake::installed();
    tracing::info!(live_ms, "Live loop started");
    let frame_stages = FRAME_STAGES.load(std::sync::atomic::Ordering::Relaxed);
    if frame_stages {
        // The canvas raster's per-draw clock, off unless a stage line is printed.
        rustkit_engine::set_frame_stage_timing(true);
    }
    let mut last_end: Option<Instant> = None;
    loop {
        let started = Instant::now();
        let elapsed_ms = clock.turn(started);
        let totals_before = frame_stages.then(rustkit_engine::frame_stage_totals);
        // Real time has passed (`elapsed_ms`), as in the app's loops (CLOCK3).
        let turn = rt.block_on(engine.pump_live_real(view_id, elapsed_ms));
        if let Some(before) = totals_before {
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            let wait = last_end.map_or(0.0, |end| ms(started.saturating_duration_since(end)));
            let d = rustkit_engine::frame_stage_totals().since(&before);
            eprintln!(
                "{}",
                rustkit_engine::frame_stage_line(stats.turns as u64 + 1, ms(started - began), ms(started.elapsed()), wait, &d)
            );
            // The running host call rows, every HOST_ROWS_EVERY turns (SCRIPT-SPLIT): the stage split
            // reads the two lines around its steady frames.
            if (stats.turns as u64 + 1) % HOST_ROWS_EVERY == 0 {
                eprintln!("{}", rustkit_engine::host_call_rows_line(stats.turns as u64 + 1));
            }
            last_end = Some(Instant::now());
        }
        stats.turns += 1;
        stats.timer_callbacks += turn.timers_ran as u64;
        stats.requests += turn.requests as u64;
        stats.relayouts += turn.relaid_out as u32;
        stats.busy_ms += started.elapsed().as_millis() as u64;
        let timer = turn.next_timer_ms.map(Duration::from_millis);
        let next = if turn.in_flight > 0 {
            Some(timer.map_or(LIVE_REQUEST_POLL, |t| t.min(LIVE_REQUEST_POLL)))
        } else {
            timer
        };
        stats.idle_at_end = next.is_none();
        let left = limit.saturating_sub(began.elapsed());
        if left.is_zero() {
            if frame_stages {
                eprintln!("{}", rustkit_engine::host_call_rows_line(stats.turns as u64));
            }
            break;
        }
        match pacing {
            // The content thread turns again when the next timer is due, counted from the turn's
            // time on the page's clock (at least 2 and at most 50 ms on), and nothing but a
            // command (none here) ends its wait early.
            LivePacing::ContentThread => {
                let now = Instant::now();
                let due = clock.deadline(now, rustkit_engine::content_turn_wait(turn.next_timer_ms));
                std::thread::sleep(due.saturating_duration_since(now).min(left))
            }
            // An idle page still gets its time: in the app only input, or the
            // engine's wake, ends the wait.
            LivePacing::WindowThread => {
                let wait = next.map_or(left, |wait| wait.max(started.elapsed()).max(MIN_LIVE_TURN));
                wake.sleep(wait.min(left));
            }
        }
    }
    stats.wall_ms = began.elapsed().as_millis() as u64;
    stats
}

#[derive(Serialize, Deserialize)]
struct LayoutStats {
    total_boxes: u32,
    sized: u32,
    zero_size: u32,
    positioned: u32,
    at_origin: u32,
    sizing_rate: f32,
    positioning_rate: f32,
}

/// The most `--live-turn-budget-ms` takes: an hour, far below what the engine's deadline arithmetic
/// (`Instant + Duration`) can hold.
const MAX_LIVE_TURN_BUDGET_MS: u64 = 3_600_000;

/// With `--frame-stages`: how many live turns apart the running host call rows are printed.
const HOST_ROWS_EVERY: u64 = 5;

/// `--frame-stages`, for the live loop (`run_live`): set once in `main`.
static FRAME_STAGES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn main() {
    let args = Args::parse();
    FRAME_STAGES.store(args.frame_stages, std::sync::atomic::Ordering::Relaxed);
    if let Some(budget) = args.live_turn_budget_ms {
        eprintln!(
            "WARNING: the live turn's script watchdog is {budget} ms, not the engine's {} ms \
             (--live-turn-budget-ms, a stress-suite run): this capture is not comparable with the board or the app",
            EngineConfig::for_parity_testing().live_turn_budget_ms
        );
    }
    if let Some(path) = &args.console_log {
        rustkit_engine::console_file::set_path(path);
    }

    // Initialize tracing - respect RUST_LOG if set, otherwise use defaults
    let default_filter = if args.verbose { "info" } else { "warn" };
    let filter = std::env::var("RUST_LOG").unwrap_or_else(|_| default_filter.to_string());
    tracing_subscriber::fmt()
        .with_env_filter(&filter)
        .with_writer(std::io::stderr)
        .init();

    // Hard wall-clock limit. A live page can stall the engine anywhere
    // (network, layout, paint) and nothing inside the engine can be trusted
    // to give up, so the limit is enforced from outside the capture thread.
    {
        let limit = Duration::from_millis(args.timeout_ms);
        let timeout_result = CaptureResult::new(&args).failed(
            "timeout",
            format!("capture exceeded {} ms", args.timeout_ms),
        );
        std::thread::spawn(move || {
            std::thread::sleep(limit);
            println!("{}", serde_json::to_string(&timeout_result).unwrap());
            std::process::exit(3);
        });
    }

    // The Chrome baselines are captured with `--hide-scrollbars`
    // (Playwright's default): styled scrollbars take no space there, so
    // they take none here (CH2.E7). RUSTKIT_SHOW_SCROLLBARS=1 keeps them.
    if std::env::var("RUSTKIT_SHOW_SCROLLBARS").map_or(true, |v| v != "1") {
        rustkit_layout::scroll::set_scrollbars_hidden(true);
    }

    let started = Instant::now();
    let args_clone = args.clone();
    // Flag off: today's worker. Flag on: the page thread's name and stack,
    // so a 26-case run with HIWAVE_CONTENT_THREAD set is not the flag-off
    // capture again. The tool has no window, so the headless presenter stays
    // with the page on that thread.
    let (worker_name, worker_stack) = capture_worker_spec(rustkit_engine::content_thread_enabled());
    let handler = std::thread::Builder::new()
        .name(worker_name.into())
        .stack_size(worker_stack)
        .spawn(move || {
            // Half the stack is the native-recursion budget: the 1 MiB fallback would throw on
            // pages Chrome renders and show up as false diffs.
            rustkit_engine::set_thread_native_stack_budget(Some(worker_stack / 2));
            run_capture(&args_clone)
        })
        .expect("failed to spawn capture worker thread");
    let mut result = match handler.join() {
        Ok(r) => r,
        Err(_) if args.wpt => std::process::exit(4),
        Err(_) => CaptureResult::new(&args).failed("error", "worker thread panicked".to_string()),
    };
    result.elapsed_ms = Some(started.elapsed().as_millis() as u64);

    // Output JSON result
    println!("{}", serde_json::to_string(&result).unwrap());

    // Exit with appropriate code
    if result.status == "ok" {
        std::process::exit(0);
    } else {
        std::process::exit(1);
    }
}

/// Where a capture runs. `HIWAVE_CONTENT_THREAD` selects the content
/// thread (the name the page thread uses, and its stack). Anything else
/// keeps the capture worker the campaign has always used.
fn capture_worker_spec(content_thread: bool) -> (&'static str, usize) {
    if content_thread {
        ("page", rustkit_engine::content_stack_bytes())
    } else {
        ("capture-worker", 16 * 1024 * 1024)
    }
}

/// The user agent the shipping RustKit content view sends
/// (hiwave-app/src/webview_rustkit.rs). Live sites branch on it, so a URL
/// capture must present as the product does, not as a test tool.
const PRODUCT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15 HiWave/1.0";

/// The capture, then its Inspector event log and summary, whichever way
/// the capture ended.
fn run_capture(args: &Args) -> CaptureResult {
    let wants_report = args.report.is_some() || args.report_text.is_some();
    let wants_events = args.event_log.is_some() || args.event_summary || wants_report;
    let sink = wants_events.then(CollectingSink::default);
    let mut inspector: Option<Arc<Inspector>> = None;
    let mut result = capture(args, sink.as_ref(), &mut inspector);
    result.content_thread = std::thread::current().name() == Some("page");
    if let Some(sink) = sink {
        let records = sink.records();
        if let Some(mut page) = result.page_report.take() {
            let json: Vec<serde_json::Value> =
                records.iter().filter_map(|r| serde_json::to_value(r).ok()).collect();
            let facts = report::CaptureFacts { shield: args.shield, capture_ms: result.capture_ms };
            report::apply_events(&mut page, &json, facts);
            write_report_files(args, &page);
        }
        if let Some(ref path) = args.event_log {
            match write_event_log(path, &records) {
                Ok(()) => result.event_log_path = Some(path.clone()),
                Err(e) => error!("Failed to write event log: {:?}", e),
            }
        }
        if args.event_summary {
            let unsupported = inspector.map(|i| i.unsupported_apis()).unwrap_or_default();
            result.event_summary = Some(InspectorSummary::from_records(&records, &unsupported));
        }
    }
    result
}

fn write_event_log(path: &str, records: &[rustkit_engine::InspectorRecord]) -> std::io::Result<()> {
    let file = std::io::BufWriter::new(fs::File::create(path)?);
    let mut sink = JsonLinesSink::new(file);
    for record in records {
        sink.record(record);
    }
    sink.flush();
    Ok(())
}

fn capture(args: &Args, sink: Option<&CollectingSink>, inspector: &mut Option<Arc<Inspector>>) -> CaptureResult {
    let capture_began = Instant::now();
    let mut result = CaptureResult::new(args);

    if let Some(ref dump_path) = args.inspector_dump {
        let dump = match fs::read_to_string(dump_path) {
            Ok(d) => d,
            Err(e) => return result.failed("error", format!("Failed to read {dump_path}: {e}")),
        };
        if let Err(e) = write_inspector(args, &dump, None, &mut result) {
            return result.failed("error", e);
        }
        return result;
    }

    // Read HTML file (URL mode fetches inside the engine instead)
    let html_content = match &args.html_file {
        Some(html_file) => match fs::read_to_string(html_file) {
            Ok(content) => Some(preprocess_html(&content, Path::new(html_file))),
            Err(e) => {
                return result.failed("error", format!("Failed to read HTML file: {}", e));
            }
        },
        None => None,
    };
    let url = match &args.url {
        Some(raw) => match Url::parse(raw) {
            Ok(u) => Some(u),
            Err(e) => return result.failed("error", format!("Invalid URL: {}", e)),
        },
        None => None,
    };

    // Create engine with parity testing config (animations disabled).
    // Fixture mode keeps its historical test-tool UA; URL mode sends the
    // product's. URL mode runs the page's scripts, as the browser does;
    // fixture mode does not (the campaign fixtures are static pages).
    let user_agent = if url.is_some() {
        PRODUCT_USER_AGENT
    } else {
        "ParityCapture/1.0"
    };

    let replay_proxy = match &args.replay_proxy {
        Some(raw) => match Url::parse(raw) {
            Ok(u) => Some(u),
            Err(e) => return result.failed("error", format!("Invalid replay proxy URL: {}", e)),
        },
        None => None,
    };

    let mut builder = EngineBuilder::new()
        .with_config(capture_config(args, replay_proxy))
        .user_agent(user_agent)
        .javascript_enabled(
            url.is_some() || args.actions.is_some() || args.run_inline_scripts || args.live_ms.is_some(),
        );
    if args.wpt {
        let origin = match &url {
            Some(u) if u.scheme() == "http" && u.host_str() == Some("127.0.0.1") => u.origin(),
            _ => return result.failed("error", "WPT requires a loopback HTTP URL".into()),
        };
        let mut interceptor = rustkit_net::RequestInterceptor::new();
        interceptor.set_default_action(rustkit_net::InterceptAction::Block);
        interceptor.allow(rustkit_net::intercept::UrlPattern::prefix(
            &format!("{}/", origin.ascii_serialization()),
        ));
        interceptor.restrict_origin(origin);
        builder = builder.request_interceptor(interceptor);
    } else if let Some(interceptor) = shield_interceptor(args) {
        builder = builder.request_interceptor(interceptor);
    }
    let engine_result = builder.build();

    let mut engine = match engine_result {
        Ok(e) => e,
        Err(e) => return result.failed("error", format!("Failed to create engine: {:?}", e)),
    };

    // Create headless view
    let bounds = Bounds {
        x: 0,
        y: 0,
        width: args.width,
        height: args.height,
    };

    let view_id = match engine.create_headless_view(bounds) {
        Ok(id) => id,
        Err(e) => {
            return result.failed("error", format!("Failed to create headless view: {:?}", e));
        }
    };
    // Before the load: the whole load goes to the sink, none of it is
    // dropped by the view's ring buffer.
    if let Some(sink) = sink {
        let _ = engine.add_inspector_sink(view_id, Box::new(sink.clone()));
        *inspector = engine.inspector(view_id);
    }

    // Load: either the fixture HTML, or the live URL through the same
    // navigation path the browser uses (document fetch, then stylesheets,
    // fonts and images via load_subresources).
    // The outputs as the load left the page, kept while a stepped settle runs (see `PreSettle`).
    let mut pre_settle: Option<PreSettle> = None;
    if let Some(url) = url {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => return result.failed("error", format!("Failed to create runtime: {}", e)),
        };
        if let Some(prelude) = &args.prelude_url {
            let prelude = match Url::parse(prelude) {
                Ok(u) => u,
                Err(e) => return result.failed("error", format!("Invalid prelude URL: {}", e)),
            };
            if let Err(e) = rt.block_on(engine.load_url(view_id, prelude)) {
                return result.failed("error", format!("Failed to load prelude URL: {:?}", e));
            }
            run_live(
                &rt,
                &mut engine,
                view_id,
                args.prelude_live_ms.unwrap_or(500),
                LivePacing::of(args),
            );
        }
        let load_began = Instant::now();
        if let Err(e) = rt.block_on(engine.load_url(view_id, url)) {
            return result.failed("error", format!("Failed to load URL: {:?}", e));
        }
        // The engine shows a server's error page as the page, which is what
        // the app's user should see. A capture is of the site, not of its
        // error page: it fails as it did when the engine refused the
        // response, with the same message first, so no board counts a 403
        // as a load.
        if let Some(status) = engine
            .http_status(view_id)
            .filter(|s| !(200..300).contains(s))
        {
            return result.failed(
                "error",
                format!("Failed to load URL: NavigationError(\"HTTP error\") (HTTP {status}; the engine rendered its body)"),
            );
        }
        if args.wpt {
            if engine.execute_script(view_id, "typeof add_completion_callback").ok().as_deref() != Some("String(\"function\")") {
                return result.failed("error", "WPT harness unavailable".into());
            }
            let mut clock = rustkit_engine::LiveClock::new(Instant::now());
            loop {
                match engine.execute_script(view_id, "globalThis.__hiwaveWptResult") {
                    Ok(value) if value.starts_with("String(\"") && value.ends_with("\")") => {
                        println!("WPT_ENCODED {}", &value[8..value.len() - 2]);
                        return result;
                    }
                    Err(err) => return result.failed("error", format!("WPT result read failed: {err}")),
                    _ => {}
                }
                rt.block_on(engine.pump_live_real(view_id, clock.turn(Instant::now())));
                std::thread::sleep(Duration::from_millis(4));
            }
        }
        // With no live window the page gets the reference's settle time after the load: virtual steps in a
        // stepped capture, a real live phase when animations run on the real clock.
        if args.live_ms.is_none() {
            // The reference's 5 s, or what the capture's timeout leaves after a margin for the frame.
            let left_in_timeout = Duration::from_millis(args.timeout_ms)
                .saturating_sub(capture_began.elapsed())
                .saturating_sub(SETTLE_TIMEOUT_MARGIN);
            let budget = SETTLE_REAL_BUDGET.min(left_in_timeout);
            if args.animations {
                // The real clock: live loop turns for the same span, in real time, within the same budget.
                if !budget.is_zero() {
                    run_live(&rt, &mut engine, view_id, budget.as_millis() as u64, LivePacing::of(args));
                }
            } else {
                // Actions and a resize define the state after the settle on purpose: nothing to fall back to.
                if !budget.is_zero() && args.actions.is_none() && args.resize_to.is_none() {
                    pre_settle = PreSettle::take(&mut engine, view_id, args);
                }
                let mut stats = settle_stepped(&rt, &mut engine, view_id, budget);
                stats.fell_back = stats.cut_short && pre_settle.is_some();
                if !stats.fell_back {
                    // The settle finished its span: the outputs taken after it stand.
                    pre_settle = None;
                }
                result.settle_held_open = (stats.held_open > 0).then_some(stats.held_open);
                result.settle_stats = Some(stats);
            }
        }
        if let Some(log) = engine.script_log(view_id) {
            result.script_stats = Some(script_stats(log));
            if let Some(ref path) = args.dump_scripts {
                if let Err(e) = fs::write(path, script_log_json(log).to_string()) {
                    error!("Failed to write script log: {:?}", e);
                }
            }
        }
        if let Some(live_ms) = args.live_ms {
            result.live_stats = Some(run_live(
                &rt,
                &mut engine,
                view_id,
                live_ms,
                LivePacing::of(args),
            ));
            result.capture_ms = Some(load_began.elapsed().as_secs_f64() * 1000.0);
        }
    } else if let Some(html) = html_content {
        let load_began = Instant::now();
        if let Err(e) = engine.load_html(view_id, &html) {
            return result.failed("error", format!("Failed to load HTML: {:?}", e));
        }
        // `--run-inline-scripts` and `--live-ms` both turn script on for a
        // file (T21). A script that threw is a failed capture either way,
        // the same check the inline-scripts flag already made.
        if args.run_inline_scripts || args.live_ms.is_some() {
            let log = engine.script_log(view_id).unwrap_or_default();
            if let Some(failed) = inline_script_failure(&log) {
                return result.failed(
                    "error",
                    format!("Inline script failed: {} {:?}", failed.source, failed.outcome),
                );
            }
        }
        // `load_html` runs no timer horizon: what a file page does after its
        // inline scripts (a `setTimeout`, an animation frame, a
        // ResizeObserver callback) runs only in the live loop, the one a URL
        // capture turns (T21).
        if let Some(live_ms) = args.live_ms {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => return result.failed("error", format!("Failed to create runtime: {}", e)),
            };
            result.live_stats = Some(run_live(
                &rt,
                &mut engine,
                view_id,
                live_ms,
                LivePacing::of(args),
            ));
            result.capture_ms = Some(load_began.elapsed().as_secs_f64() * 1000.0);
        }
    }

    if let Some((width, height)) = args.resize_to {
        let bounds = Bounds {
            x: 0,
            y: 0,
            width,
            height,
        };
        if let Err(e) = engine.resize_view(view_id, bounds) {
            return result.failed("error", format!("Failed to resize view: {:?}", e));
        }
        result.width = width;
        result.height = height;
    }

    if let Some(ref actions_raw) = args.actions {
        let actions = match parse_actions(actions_raw) {
            Ok(acts) => acts,
            Err(e) => {
                let _ = engine.destroy_view(view_id);
                return result.failed("error", e);
            }
        };
        execute_actions(
            &mut engine,
            view_id,
            &actions,
            args.actions_out_dir.as_deref(),
            &mut result,
        );
    }

    // Render final view
    if let Err(e) = engine.render_view(view_id) {
        return result.failed("error", format!("Failed to render: {:?}", e));
    }

    // Capture frame if requested
    if let Some(ref path) = args.dump_frame {
        if let Err(e) = engine.capture_frame(view_id, path) {
            error!("Failed to capture frame: {:?}", e);
        } else {
            result.frame_path = Some(path.clone());
        }
    }

    // Export layout if requested
    if let Some(ref path) = args.dump_layout {
        match engine.export_layout_json(view_id, path) {
            Ok(()) => {
                result.layout_path = Some(path.clone());
                // Read back the file to analyze
                match fs::read_to_string(path) {
                    Ok(layout_json) => result.layout_stats = analyze_layout_json(&layout_json),
                    Err(e) => error!("Failed to read layout file: {:?}", e),
                }
            }
            Err(e) => error!("Failed to export layout: {:?}", e),
        }
    }

    // Export display list if requested
    if let Some(ref path) = args.dump_display_list {
        match engine.export_display_list_json(view_id, path) {
            Ok(()) => result.display_list_path = Some(path.clone()),
            Err(e) => error!("Failed to export display list: {:?}", e),
        }
    }

    // A settle cut short: the outputs as the load left the page replace the half-way ones.
    // `fell_back` stays true only if every output was restored. Diagnostics and reports still describe the
    // state after the settle; `fell_back` marks them as not the frame's state.
    if let Some(pre) = pre_settle.take() {
        let restored = pre.restore(&mut result, args);
        if let Some(stats) = result.settle_stats.as_mut() {
            stats.fell_back = restored;
        }
    }

    write_diagnostics(&engine, view_id, args, &mut result);

    if args.inspector_out.is_some() {
        let opts = rustkit_engine::DiagnosticsOptions {
            include_url_query: args.diagnostics_keep_query,
            max_elements: args.diagnostics_max_elements,
        };
        let dump = engine
            .all_element_diagnostics(view_id, &opts)
            .map_err(|e| format!("Failed to build diagnostics: {e:?}"))
            .and_then(|d| serde_json::to_string(&d).map_err(|e| e.to_string()));
        let select = match args.inspector_select_at {
            Some((x, y)) => match engine.inspector_selection_at(view_id, x, y) {
                Ok(sel) => sel.map(|s| s.node_id),
                Err(e) => return result.failed("error", format!("Inspector selection failed: {e:?}")),
            },
            None => None,
        };
        if let Err(e) = dump.and_then(|d| write_inspector(args, &d, select, &mut result)) {
            return result.failed("error", e);
        }
    }

    // Clean up
    let _ = engine.destroy_view(view_id);

    result
}

/// `--diagnose`, `--diagnose-at` and `--dump-diagnostics`: records read from
/// the final layout, after any actions and the final render.
fn write_diagnostics(
    engine: &rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    args: &Args,
    result: &mut CaptureResult,
) {
    use rustkit_engine::{DiagnosticTarget, DiagnosticsOptions};
    let opts = DiagnosticsOptions {
        include_url_query: args.diagnostics_keep_query,
        max_elements: args.diagnostics_max_elements,
    };
    let targets: Vec<DiagnosticTarget> = args
        .diagnose
        .iter()
        .map(|s| DiagnosticTarget::Selector(s.clone()))
        .chain(args.diagnose_at.iter().map(|&(x, y)| DiagnosticTarget::Point { x, y }))
        .collect();
    if !targets.is_empty() {
        let records = targets
            .iter()
            .map(|t| match engine.element_diagnostics(view_id, t, &opts) {
                Ok(r) => serde_json::to_value(r).unwrap_or(serde_json::Value::Null),
                Err(e) => serde_json::json!({ "error": format!("{e:?}") }),
            })
            .collect();
        result.diagnostics = Some(records);
    }
    if let Some(ref path) = args.dump_diagnostics {
        match engine.all_element_diagnostics(view_id, &opts) {
            Ok(dump) => {
                if dump.truncated {
                    warn!(
                        total = dump.total_elements,
                        written = dump.elements.len(),
                        "diagnostics dump truncated at --diagnostics-max-elements"
                    );
                }
                match serde_json::to_string(&dump).map(|json| fs::write(path, json)) {
                    Ok(Ok(())) => result.diagnostics_path = Some(path.clone()),
                    Ok(Err(e)) => error!("Failed to write diagnostics: {:?}", e),
                    Err(e) => error!("Failed to serialise diagnostics: {:?}", e),
                }
            }
            Err(e) => error!("Failed to build diagnostics: {:?}", e),
        }
    }
    if args.report.is_some() || args.report_text.is_some() {
        result.page_report = layout_report(engine, view_id, &opts);
    }
}

/// The most elements the page report reads. A consent dialog is often the
/// last thing in the body, so the report reads past the dump's default cap.
const REPORT_MAX_ELEMENTS: usize = 20_000;

/// The layout half of `--report`, from the final layout.
fn layout_report(
    engine: &rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    opts: &rustkit_engine::DiagnosticsOptions,
) -> Option<report::Report> {
    use rustkit_engine::{DiagnosticTarget, DiagnosticsOptions};
    let opts = DiagnosticsOptions { max_elements: opts.max_elements.max(REPORT_MAX_ELEMENTS), ..opts.clone() };
    let dump = match engine.all_element_diagnostics(view_id, &opts).map(serde_json::to_value) {
        Ok(Ok(dump)) => dump,
        Ok(Err(e)) => {
            error!("Failed to serialise the report's diagnostics: {:?}", e);
            return None;
        }
        Err(e) => {
            error!("Failed to build the report's diagnostics: {:?}", e);
            return None;
        }
    };
    let (w, h) = (
        dump["page"]["viewport"][0].as_f64().unwrap_or(0.0) as f32,
        dump["page"]["viewport"][1].as_f64().unwrap_or(0.0) as f32,
    );
    let centre = engine
        .element_diagnostics(view_id, &DiagnosticTarget::Point { x: w / 2.0, y: h / 2.0 }, &opts)
        .ok()
        .flatten()
        .and_then(|r| serde_json::to_value(r).ok());
    let mut page = report::page_report(&dump, centre.as_ref());
    if dump["truncated"].as_bool() == Some(true) {
        page.not_checked.push(format!("elements past the first {} (the page has {})", opts.max_elements, page.elements));
    }
    Some(page)
}

/// `--report` and `--report-text`.
fn write_report_files(args: &Args, page: &report::Report) {
    if let Some(ref path) = args.report {
        match serde_json::to_string_pretty(&page).map(|json| fs::write(path, json)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => error!("Failed to write the report: {:?}", e),
            Err(e) => error!("Failed to serialise the report: {:?}", e),
        }
    }
    if let Some(ref path) = args.report_text {
        if let Err(e) = fs::write(path, report::report_text(&page)) {
            error!("Failed to write the report text: {:?}", e);
        }
    }
}

/// `--inspector-out`: the Inspector for `dump`, with `--inspector-diff`.
fn write_inspector(args: &Args, dump: &str, select: Option<usize>, result: &mut CaptureResult) -> Result<(), String> {
    let Some(ref out) = args.inspector_out else { return Ok(()) };
    let diff = match args.inspector_diff {
        Some(ref p) => Some(fs::read_to_string(p).map_err(|e| format!("Failed to read {p}: {e}"))?),
        None => None,
    };
    let written = inspector::render_inspector(dump, diff.as_deref(), select, args.inspector_size, Path::new(out))
        .map_err(|e| format!("Inspector: {e}"))?;
    result.inspector = Some(written);
    Ok(())
}

/// `X,Y` in CSS px.
fn parse_point_arg(s: &str) -> Result<(f32, f32), String> {
    let (x, y) = s
        .split_once(',')
        .ok_or_else(|| format!("expected X,Y, got {s:?}"))?;
    let x = x.trim().parse().map_err(|e| format!("x {x:?}: {e}"))?;
    let y = y.trim().parse().map_err(|e| format!("y {y:?}: {e}"))?;
    Ok((x, y))
}

fn parse_actions(raw: &str) -> Result<Vec<ActionItem>, String> {
    let trimmed = raw.trim();
    let content = if trimmed.starts_with('[') || trimmed.starts_with('{') {
        trimmed.to_string()
    } else {
        fs::read_to_string(trimmed).map_err(|e| format!("failed to read actions file: {e}"))?
    };
    let content = content.trim();
    if content.starts_with('[') {
        serde_json::from_str::<Vec<ActionItem>>(content)
            .map_err(|e| format!("failed to parse actions JSON array: {e}"))
    } else {
        serde_json::from_str::<ActionsPayload>(content)
            .map(|p| p.actions)
            .map_err(|e| format!("failed to parse actions JSON object: {e}"))
    }
}

fn parse_point(eval_res: &str) -> Option<(f32, f32)> {
    let idx = eval_res.find("point:")?;
    let rest = &eval_res[idx + 6..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let x_part = parts[0].trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
    let y_part = parts[1].trim_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != '-');
    let x = x_part.parse::<f32>().ok()?;
    let y = y_part.parse::<f32>().ok()?;
    Some((x, y))
}

/// The focused control's value, for telling an engine-typed key from one the
/// page swallowed.
const ACTIVE_VALUE_JS: &str =
    "(function(){ var el = document.activeElement; return String(el && 'value' in el ? el.value : ''); })()";
/// A one-shot capture listener: did the engine's key reach the page? The
/// listener is kept on `window` so a read for a key the page never heard
/// can remove it; otherwise it would fire on the next key before that key
/// was pressed.
const KEY_SEEN_ARM_JS: &str = "(function(){ \
    if (window.__parityCaptureKeyListener) { \
        document.removeEventListener('keydown', window.__parityCaptureKeyListener, true); } \
    window.__parityCaptureKeySeen = false; \
    window.__parityCaptureKeyListener = function(){ window.__parityCaptureKeySeen = true; window.__parityCaptureKeyListener = null; }; \
    document.addEventListener('keydown', window.__parityCaptureKeyListener, { once: true, capture: true }); })()";
/// Two answers that are not substrings of each other: `execute_script`
/// returns the value's debug form (`String("key-heard")`), so the read is a
/// `contains`, and 'unseen' contains 'seen' (#898 R1).
const KEY_SEEN_READ_JS: &str = "(function(){ \
    if (window.__parityCaptureKeySeen) return 'key-heard'; \
    if (window.__parityCaptureKeyListener) { \
        document.removeEventListener('keydown', window.__parityCaptureKeyListener, true); \
        window.__parityCaptureKeyListener = null; } \
    return 'key-silent'; })()";

/// Did the page hear the engine's key? Reads what `KEY_SEEN_READ_JS` returned.
fn key_was_seen(read: &str) -> bool {
    read.contains("key-heard")
}

/// A `selector` field may name several selectors to try in order, comma
/// separated, as `click` has always read it. Top-level commas only: a comma
/// inside `:is(a, b)`, `[title="x, y"]` or quotes belongs to the selector.
fn split_selector_list(selector: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for ch in selector.chars() {
        match quote {
            Some(q) => {
                current.push(ch);
                if ch == q {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => {
                    quote = Some(ch);
                    current.push(ch);
                }
                '(' | '[' => {
                    depth += 1;
                    current.push(ch);
                }
                ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    out.push(current.trim().to_string());
                    current.clear();
                }
                _ => current.push(ch),
            },
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn execute_actions(
    engine: &mut rustkit_engine::Engine,
    view_id: rustkit_engine::EngineViewId,
    actions: &[ActionItem],
    actions_out_dir: Option<&str>,
    result: &mut CaptureResult,
) {
    let mut captures = Vec::new();
    let mut action_results = Vec::new();

    for (i, a) in actions.iter().enumerate() {
        let step = a.step.unwrap_or(i);
        let start = Instant::now();
        let mut a_res = ActionResult {
            step,
            action_type: a.action_type.clone(),
            status: "ok".to_string(),
            error: None,
            frame: None,
            label: a.label.clone(),
            selector_used: None,
            method: None,
            elapsed_ms: 0,
        };

        match a.action_type.as_str() {
            "wait" => {
                if let Some(ref sel) = a.selector {
                    let timeout = Duration::from_millis(a.timeout_ms.or(a.ms).unwrap_or(2000));
                    let wait_start = Instant::now();
                    let sel_json = serde_json::to_string(sel).unwrap_or_default();
                    let check_js = format!(
                        "Boolean(document.querySelector({})) ? 'found' : 'missing'",
                        sel_json
                    );
                    let mut found = false;
                    while wait_start.elapsed() < timeout {
                        if let Ok(eval_res) = engine.execute_script(view_id, &check_js) {
                            if eval_res.contains("found") && !eval_res.contains("missing") {
                                found = true;
                                break;
                            }
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    if !found {
                        a_res.status = "timeout".to_string();
                        a_res.error = Some(format!("timed out waiting for selector {}", sel));
                    }
                } else {
                    let ms = a.ms.unwrap_or(500);
                    std::thread::sleep(Duration::from_millis(ms));
                }
            }
            "click" => {
                if let Some(ref sel) = a.selector {
                    let selectors = split_selector_list(sel);
                    let mut clicked = false;
                    for s in selectors.iter().map(String::as_str) {
                        let s_json = serde_json::to_string(s).unwrap_or_default();
                        let query_js = format!(
                            r#"(function() {{
                                try {{
                                    var el = document.querySelector({0});
                                    if (!el) return 'missing';
                                    if (typeof el.getBoundingClientRect === 'function') {{
                                        var r = el.getBoundingClientRect();
                                        if (r && r.width > 0 && r.height > 0) {{
                                            var cx = r.left + r.width / 2.0;
                                            var cy = r.top + r.height / 2.0;
                                            return 'point:' + cx + ':' + cy;
                                        }}
                                    }}
                                    if (typeof el.focus === 'function') {{
                                        try {{ el.focus(); }} catch (e) {{}}
                                    }}
                                    if (typeof el.click === 'function') {{
                                        try {{ el.click(); }} catch (e) {{}}
                                    }} else {{
                                        try {{ el.dispatchEvent(new Event('click', {{ bubbles: true, cancelable: true }})); }} catch (e) {{}}
                                    }}
                                    return 'fallback_clicked';
                                }} catch (e) {{
                                    return 'error:' + e;
                                }}
                            }})()"#,
                            s_json
                        );
                        if let Ok(eval_res) = engine.execute_script(view_id, &query_js) {
                            if let Some((cx, cy)) = parse_point(&eval_res) {
                                engine.mouse_move_at_point(view_id, cx, cy);
                                let _ = engine.mouse_down_at_point(view_id, cx, cy);
                                let _ = engine.click_at_point(view_id, cx, cy);
                                let _ = engine.relayout(view_id);
                                clicked = true;
                                a_res.selector_used = Some(s.to_string());
                                a_res.method = Some(format!("engine_point({:.1}, {:.1})", cx, cy));
                                break;
                            }
                            if eval_res.contains("fallback_clicked") {
                                let _ = engine.relayout(view_id);
                                clicked = true;
                                a_res.selector_used = Some(s.to_string());
                                a_res.method = Some("synthetic_fallback".to_string());
                                if a_res.label.is_none() {
                                    a_res.label = Some("fallback".to_string());
                                }
                                break;
                            }
                        }
                    }
                    if !clicked {
                        a_res.status = "selector_not_found".to_string();
                        a_res.error =
                            Some(format!("no matching element found for selector {}", sel));
                    }
                } else if let (Some(x), Some(y)) = (a.x, a.y) {
                    engine.mouse_move_at_point(view_id, x, y);
                    let _ = engine.mouse_down_at_point(view_id, x, y);
                    let _ = engine.click_at_point(view_id, x, y);
                    let _ = engine.relayout(view_id);
                    a_res.method = Some(format!("engine_point({:.1}, {:.1})", x, y));
                } else {
                    a_res.status = "error".to_string();
                    a_res.error = Some("click action requires selector or (x, y)".to_string());
                }
            }
            "hover" => {
                // A pointer move to the element's centre and nothing else: the
                // page sees mousemove/mouseover and `:hover` applies, no press.
                let point = match (a.selector.as_ref(), a.x, a.y) {
                    (Some(sel), _, _) => split_selector_list(sel).into_iter().find_map(|s| {
                        let sel_json = serde_json::to_string(&s).unwrap_or_default();
                        let query_js = format!(
                            r#"(function() {{
                                var el = document.querySelector({0});
                                if (!el || typeof el.getBoundingClientRect !== 'function') return 'missing';
                                var r = el.getBoundingClientRect();
                                if (!r || !(r.width > 0 && r.height > 0)) return 'missing';
                                return 'point:' + (r.left + r.width / 2.0) + ':' + (r.top + r.height / 2.0);
                            }})()"#,
                            sel_json
                        );
                        let found = engine
                            .execute_script(view_id, &query_js)
                            .ok()
                            .and_then(|res| parse_point(&res));
                        if found.is_some() {
                            a_res.selector_used = Some(s);
                        }
                        found
                    }),
                    (None, Some(x), Some(y)) => Some((x, y)),
                    _ => None,
                };
                match point {
                    Some((cx, cy)) => {
                        engine.mouse_move_at_point(view_id, cx, cy);
                        let _ = engine.relayout(view_id);
                        a_res.method = Some(format!("engine_point({:.1}, {:.1})", cx, cy));
                    }
                    None if a.selector.is_some() => {
                        a_res.status = "selector_not_found".to_string();
                        a_res.error = Some(format!(
                            "no matching element found for selector {}",
                            a.selector.as_deref().unwrap_or("")
                        ));
                    }
                    None => {
                        a_res.status = "error".to_string();
                        a_res.error = Some("hover action requires selector or (x, y)".to_string());
                    }
                }
            }
            "key" => {
                let held = |name: &str| {
                    a.modifiers
                        .as_ref()
                        .map(|m| m.iter().any(|x| x.eq_ignore_ascii_case(name)))
                        .unwrap_or(false)
                };
                let (ctrl, shift, alt, meta) =
                    (held("Control"), held("Shift"), held("Alt"), held("Meta"));
                if let Some(ref sel) = a.selector {
                    let sel_json = serde_json::to_string(sel).unwrap_or_default();
                    let focus_js = format!(
                        "var el = document.querySelector({}); if (el && typeof el.focus === 'function') el.focus();",
                        sel_json
                    );
                    let _ = engine.execute_script(view_id, &focus_js);
                }
                if let Some(ref text) = a.text {
                    // Typed through the engine's keyboard, a key at a time, as
                    // Playwright's keyboard.type does: the page hears a keydown
                    // per character and the focused control edits its own
                    // value (selection replaced, caret moved). The value is
                    // written from script only when no control took the keys.
                    let before = engine
                        .execute_script(view_id, ACTIVE_VALUE_JS)
                        .unwrap_or_default();
                    for ch in text.chars() {
                        let key = ch.to_string();
                        let _ = engine.handle_text_key(view_id, 0, &key, false, false, false);
                    }
                    let after = engine
                        .execute_script(view_id, ACTIVE_VALUE_JS)
                        .unwrap_or_default();
                    if after == before {
                        let text_json = serde_json::to_string(text).unwrap_or_default();
                        let key_js = format!(
                            r#"(function() {{
                                var el = document.activeElement;
                                if (el && 'value' in el) {{
                                    el.value = (el.value || '') + {0};
                                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                                }}
                            }})()"#,
                            text_json
                        );
                        let _ = engine.execute_script(view_id, &key_js);
                        a_res.method = Some("script_value".to_string());
                    } else {
                        a_res.method = Some("engine_keys".to_string());
                    }
                    let _ = engine.relayout(view_id);
                } else if let Some(ref key) = a.key {
                    let key_code = match key.as_str() {
                        "Enter" => 13,
                        "Escape" => 27,
                        "Backspace" => 8,
                        "Tab" => 9,
                        _ => 0,
                    };
                    // The engine fires the DOM keydown itself (fire_key). The
                    // synthetic event below is only for a key the engine did
                    // not route to the page; sending both made every listener
                    // hear the key twice and a toggle undo itself.
                    let _ = engine.execute_script(view_id, KEY_SEEN_ARM_JS);
                    // The engine's keyboard has no meta flag; Meta is Ctrl's
                    // equivalent on macOS and the chrome takes either.
                    let _ = engine.handle_text_key(view_id, key_code, key, ctrl || meta, shift, alt);
                    let seen = engine
                        .execute_script(view_id, KEY_SEEN_READ_JS)
                        .map(|r| key_was_seen(&r))
                        .unwrap_or(false);
                    if seen {
                        a_res.method = Some("engine_key".to_string());
                    } else {
                        let key_json = serde_json::to_string(key).unwrap_or_default();
                        let event_js = format!(
                            r#"(function() {{
                                var el = document.activeElement || document.body;
                                if (el) {{
                                    var init = {{ key: {0}, bubbles: true, cancelable: true,
                                                  ctrlKey: {1}, shiftKey: {2}, altKey: {3}, metaKey: {4} }};
                                    el.dispatchEvent(new KeyboardEvent('keydown', init));
                                    el.dispatchEvent(new KeyboardEvent('keyup', init));
                                }}
                            }})()"#,
                            key_json, ctrl, shift, alt, meta
                        );
                        let _ = engine.execute_script(view_id, &event_js);
                        a_res.method = Some("synthetic_key".to_string());
                    }
                    let _ = engine.relayout(view_id);
                }
            }
            "resize" => {
                if let (Some(w), Some(h)) = (a.width, a.height) {
                    let bounds = Bounds {
                        x: 0,
                        y: 0,
                        width: w,
                        height: h,
                    };
                    if let Err(e) = engine.resize_view(view_id, bounds) {
                        a_res.status = "error".to_string();
                        a_res.error = Some(format!("resize failed: {:?}", e));
                    } else {
                        result.width = w;
                        result.height = h;
                        let _ = engine.relayout(view_id);
                    }
                } else {
                    a_res.status = "error".to_string();
                    a_res.error = Some("resize requires width and height".to_string());
                }
            }
            "capture" => {
                if let Some(ref frame) = a.frame {
                    let frame_path = match actions_out_dir {
                        Some(out_dir) => {
                            Path::new(out_dir).join(frame).to_string_lossy().to_string()
                        }
                        None => frame.clone(),
                    };
                    if let Some(parent) = Path::new(&frame_path).parent() {
                        let _ = fs::create_dir_all(parent);
                    }
                    let _ = engine.render_view(view_id);
                    if let Err(e) = engine.capture_frame(view_id, &frame_path) {
                        a_res.status = "error".to_string();
                        a_res.error = Some(format!("capture_frame failed: {:?}", e));
                    } else {
                        let label = a.label.clone().unwrap_or_else(|| format!("step_{}", step));
                        captures.push(StepCapture {
                            step,
                            label: label.clone(),
                            frame: frame_path.clone(),
                        });
                        a_res.frame = Some(frame_path);
                        a_res.label = Some(label);
                    }
                } else {
                    a_res.status = "error".to_string();
                    a_res.error = Some("capture requires frame path".to_string());
                }
            }
            other => {
                a_res.status = "error".to_string();
                a_res.error = Some(format!("unknown action type: {}", other));
            }
        }

        a_res.elapsed_ms = start.elapsed().as_millis() as u64;
        action_results.push(a_res);
    }

    result.captures = Some(captures);
    result.action_results = Some(action_results);
}

/// `WxH`, e.g. `1024x768`.
fn parse_size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s
        .split_once('x')
        .ok_or_else(|| format!("expected WxH, got {s:?}"))?;
    let w = w.parse().map_err(|e| format!("width {w:?}: {e}"))?;
    let h = h.parse().map_err(|e| format!("height {h:?}: {e}"))?;
    Ok((w, h))
}

fn script_stats(log: &[ScriptRecord]) -> ScriptStats {
    let mut stats = ScriptStats {
        ran: 0,
        threw: 0,
        skipped: 0,
        fetch_failed: 0,
        over_budget: 0,
        bytes: 0,
        elapsed_ms: 0,
    };
    for record in log {
        match record.outcome {
            ScriptOutcome::Ran => stats.ran += 1,
            ScriptOutcome::Threw(_) => stats.threw += 1,
            ScriptOutcome::Skipped(_) | ScriptOutcome::Nomodule => stats.skipped += 1,
            ScriptOutcome::FetchFailed(_) => stats.fetch_failed += 1,
            ScriptOutcome::OverBudget => stats.over_budget += 1,
            ScriptOutcome::EngineFailed(_) => stats.threw += 1,
        }
        stats.bytes += record.bytes as u64;
        stats.elapsed_ms += record.elapsed_ms;
    }
    stats
}

fn script_log_json(log: &[ScriptRecord]) -> serde_json::Value {
    let records: Vec<_> = log
        .iter()
        .map(|r| {
            let (outcome, detail) = match &r.outcome {
                ScriptOutcome::Ran => ("ran", None),
                ScriptOutcome::Threw(m) => ("threw", Some(m.clone())),
                ScriptOutcome::Skipped(why) => ("skipped", Some(why.to_string())),
                ScriptOutcome::Nomodule => (
                    "skipped",
                    Some("nomodule (skipped, as in module-capable browsers)".to_string()),
                ),
                ScriptOutcome::FetchFailed(m) => ("fetch_failed", Some(m.to_string())),
                ScriptOutcome::OverBudget => ("over_budget", None),
                // Reported as before the engine marked its own failures.
                ScriptOutcome::EngineFailed(failure) => ("threw", Some(failure.to_string())),
            };
            serde_json::json!({
                "source": r.source,
                "bytes": r.bytes,
                "elapsed_ms": r.elapsed_ms,
                "outcome": outcome,
                "detail": detail,
            })
        })
        .collect();
    serde_json::json!({ "scripts": records })
}

/// Mirror the Chrome capture pipeline's CSS inputs for a file loaded via
/// `Engine::load_html`, which uses a synthetic about:blank base URL and never
/// fetches subresources:
///
/// 1. Inline every `<link rel="stylesheet" href="...">` whose href resolves to
///    a file relative to the HTML file's directory (Chrome loads these
///    natively over file://).
/// 2. For micro-suite fixtures, inject `baselines/common/parity-reset.css` as
///    the FIRST style in `<head>` — Chrome's capture injects it via an init
///    script before the fixture's own styles, so fixture rules win ties there
///    and must win ties here too.
/// 3. Inline local image files as `data:` URLs (`inline_local_images`).
fn preprocess_html(html: &str, html_path: &Path) -> String {
    let base_dir = html_path.parent().unwrap_or_else(|| Path::new("."));
    let mut out = inline_local_images(&inline_stylesheet_links(html, base_dir), base_dir);

    if is_micro_suite_path(html_path) {
        if let Some(reset_css) = read_repo_parity_reset(html_path) {
            out = inject_style_first_in_head(&out, "data-parity-reset=\"1\"", &reset_css);
        } else {
            warn!("micro-suite fixture but baselines/common/parity-reset.css not found");
        }
    }

    out
}

fn is_micro_suite_path(html_path: &Path) -> bool {
    // Canonicalize first: the separator-delimited patterns below require a
    // LEADING separator, so a repo-relative invocation ("websuite/micro/x")
    // never matched and the reset was silently skipped — while CI passes
    // absolute paths and always matched. Local captures therefore rendered
    // micro cases WITHOUT the reset Chrome's oracle applies (line-height
    // 1.5 vs metrics-normal, ~5px per line), an invisible capture-
    // environment asymmetry between every local board and CI.
    let canon = html_path
        .canonicalize()
        .unwrap_or_else(|_| html_path.to_path_buf());
    let p = canon.to_string_lossy();
    p.contains("/websuite/micro/") || p.contains("\\websuite\\micro\\")
}

/// Walk up from the HTML file to the repo root (the directory containing
/// `baselines/common/parity-reset.css`) and read the reset, matching
/// deterministic.mjs's RESET_CSS_PATH.
fn read_repo_parity_reset(html_path: &Path) -> Option<String> {
    let mut dir = html_path.parent()?;
    loop {
        let candidate = dir.join("baselines/common/parity-reset.css");
        if candidate.is_file() {
            return fs::read_to_string(candidate).ok();
        }
        dir = dir.parent()?;
    }
}

/// Replace `<link rel="stylesheet" href="...">` tags with inline `<style>`
/// blocks holding the referenced file's contents, preserving document order
/// so the cascade is unchanged. Absolute (scheme://) hrefs and unreadable
/// files keep the original tag and produce a warning.
fn inline_stylesheet_links(html: &str, base_dir: &Path) -> String {
    let lower = html.to_ascii_lowercase();
    let mut out = String::with_capacity(html.len());
    let mut pos = 0;

    while let Some(rel_start) = lower[pos..].find("<link") {
        let tag_start = pos + rel_start;
        let Some(rel_end) = lower[tag_start..].find('>') else {
            break;
        };
        let tag_end = tag_start + rel_end + 1;
        let tag = &html[tag_start..tag_end];

        out.push_str(&html[pos..tag_start]);
        match stylesheet_replacement_for_link_tag(tag, base_dir) {
            Some(style_block) => out.push_str(&style_block),
            None => out.push_str(tag),
        }
        pos = tag_end;
    }
    out.push_str(&html[pos..]);
    out
}

/// If the tag is a resolvable relative stylesheet link, build its inline
/// `<style>` replacement; otherwise None (keep the tag as-is).
fn stylesheet_replacement_for_link_tag(tag: &str, base_dir: &Path) -> Option<String> {
    let rel = extract_attr(tag, "rel")?;
    if !rel
        .split_ascii_whitespace()
        .any(|t| t.eq_ignore_ascii_case("stylesheet"))
    {
        return None;
    }
    let href = extract_attr(tag, "href")?;
    if href.contains("://") || href.starts_with("//") {
        warn!(href, "leaving non-local stylesheet link unresolved");
        return None;
    }

    let css_path = base_dir.join(&href);
    let css = match fs::read_to_string(&css_path) {
        Ok(css) => css,
        Err(e) => {
            warn!(href, ?css_path, ?e, "failed to read linked stylesheet");
            return None;
        }
    };
    if css.to_ascii_lowercase().contains("</style") {
        warn!(href, "stylesheet contains '</style' — cannot inline safely");
        return None;
    }

    // A sheet's url()s are relative to the sheet, not to the page.
    let css = inline_local_images(&css, css_path.parent().unwrap_or(base_dir));
    let media = extract_attr(tag, "media").unwrap_or_default();
    let needs_media_wrap = !media.is_empty() && !media.eq_ignore_ascii_case("all");
    let body = if needs_media_wrap {
        format!("@media {} {{\n{}\n}}", media, css)
    } else {
        css
    };
    Some(format!(
        "<style data-inlined-href=\"{}\">\n{}\n</style>",
        href, body
    ))
}

/// Inline the page's local image files as `data:` URLs: CSS `url(...)`
/// (backgrounds, masks, ...) and `<img src>`. `load_html` fetches no image
/// (about:blank base, http(s) only), while the oracle serves the page over
/// http as wptserve does, so a page that masks or paints with
/// `support/icon.svg` was captured without it. A `data:` image takes the
/// engine's own decode path (`cache_data_svg` for SVG). Left alone: any URL
/// with a scheme, `//host`, `#id` and `file.svg#id` (an element of another
/// document, not an image), a query, and a path that is not a readable
/// image file.
fn inline_local_images(text: &str, base_dir: &Path) -> String {
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    loop {
        let url_at = lower[pos..].find("url(").map(|i| (pos + i, true));
        let src_at = find_img_src(&lower, pos).map(|i| (i, false));
        let next = match (url_at, src_at) {
            (Some(a), Some(b)) => if a.0 <= b.0 { a } else { b },
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let (start, is_css) = next;
        // The value's span: inside `url( ... )`, or the quoted/unquoted
        // attribute value after `src=`.
        let (value_start, value_end, after) = if is_css {
            let open = start + 4;
            let Some(close) = text[open..].find(')').map(|i| open + i) else {
                break;
            };
            (open, close, close)
        } else {
            let eq = start + lower[start..].find('=').unwrap() + 1;
            let ws = text[eq..].len() - text[eq..].trim_start().len();
            let v = eq + ws;
            match text[v..].chars().next() {
                Some(q @ ('"' | '\'')) => match text[v + 1..].find(q) {
                    Some(e) => (v, v + 1 + e + 1, v + 1 + e + 1),
                    None => break,
                },
                _ => {
                    let e = text[v..]
                        .find(|c: char| c.is_ascii_whitespace() || c == '>')
                        .map_or(text.len(), |i| v + i);
                    (v, e, e)
                }
            }
        };
        let raw = text[value_start..value_end].trim();
        let reference = raw.trim_matches(|c| c == '"' || c == '\'');
        match local_image_data_url(reference, base_dir) {
            // A CSS url() is written unquoted: it may sit in a quoted
            // `style` attribute, and either quote could close it (#966 R1).
            // base64 needs no quoting there. An `<img src>` value is
            // replaced with its own quotes, so it is quoted.
            Some(data) if is_css => {
                out.push_str(&text[pos..value_start]);
                out.push_str(&data);
                pos = after;
            }
            Some(data) => {
                out.push_str(&text[pos..value_start]);
                out.push('"');
                out.push_str(&data);
                out.push('"');
                pos = after;
            }
            None => {
                let step = if is_css { value_start } else { after.max(start + 1) };
                out.push_str(&text[pos..step]);
                pos = step;
            }
        }
    }
    out.push_str(&text[pos..]);
    out
}

/// The byte offset of the next `src` attribute of an `<img>` tag at or after
/// `from` in the lowercased text, pointing at the `s` of `src`.
fn find_img_src(lower: &str, from: usize) -> Option<usize> {
    let mut at = from;
    while let Some(i) = lower[at..].find("<img") {
        let tag_start = at + i;
        let tag_end = lower[tag_start..].find('>').map_or(lower.len(), |e| tag_start + e);
        let tag = &lower[tag_start..tag_end];
        let mut search = 0;
        while let Some(j) = tag[search..].find("src") {
            let k = search + j;
            let before_ok = tag[..k].chars().next_back().is_some_and(|c| c.is_ascii_whitespace());
            let rest = tag[k + 3..].trim_start();
            if before_ok && rest.starts_with('=') {
                return Some(tag_start + k);
            }
            search = k + 3;
        }
        at = tag_end.max(tag_start + 4);
    }
    None
}

/// `reference` as a `data:` URL when it names a readable image file
/// relative to `base_dir` (or an absolute path, as the WPT runner stages
/// root-absolute references).
fn local_image_data_url(reference: &str, base_dir: &Path) -> Option<String> {
    use base64::Engine as _;
    if reference.is_empty()
        || reference.starts_with('#')
        || reference.starts_with("//")
        || reference.contains('#')
        || reference.contains('?')
        || Url::parse(reference).is_ok_and(|u| u.scheme().len() > 1)
    {
        return None;
    }
    let path = base_dir.join(reference);
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "gif" => "image/gif",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = fs::read(&path).ok()?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Crude attribute extraction: name="value" or name='value' (fixtures are
/// controlled inputs; unquoted values are not used there).
fn extract_attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let needle = format!("{}=", name);
    let mut search_from = 0;
    while let Some(found) = lower[search_from..].find(&needle) {
        let idx = search_from + found;
        // Must be preceded by whitespace to be an attribute name boundary.
        let boundary_ok = idx > 0
            && lower[..idx]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_whitespace());
        let val_start = idx + needle.len();
        if boundary_ok {
            let rest = &tag[val_start..];
            let mut chars = rest.chars();
            return match chars.next() {
                Some(q @ ('"' | '\'')) => {
                    let inner = &rest[1..];
                    inner.find(q).map(|end| inner[..end].to_string())
                }
                _ => None,
            };
        }
        search_from = val_start;
    }
    None
}

/// Insert a `<style {attrs}>` block as the first child of `<head>`, falling
/// back to just after `<html...>` or the start of the document.
fn inject_style_first_in_head(html: &str, attrs: &str, css: &str) -> String {
    let style_block = format!("<style {}>\n{}\n</style>", attrs, css);
    let lower = html.to_ascii_lowercase();

    let insert_at = ["<head", "<html"].iter().find_map(|open| {
        lower
            .find(open)
            .and_then(|start| lower[start..].find('>').map(|end| start + end + 1))
    });

    match insert_at {
        Some(at) => format!("{}{}{}", &html[..at], style_block, &html[at..]),
        None => format!("{}{}", style_block, html),
    }
}

fn analyze_layout_json(json_str: &str) -> Option<LayoutStats> {
    let data: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let mut stats = LayoutStats {
        total_boxes: 0,
        sized: 0,
        zero_size: 0,
        positioned: 0,
        at_origin: 0,
        sizing_rate: 0.0,
        positioning_rate: 0.0,
    };

    fn walk(node: &serde_json::Value, stats: &mut LayoutStats) {
        if let Some(rect) = node.get("content_rect").or(node.get("rect")) {
            let x = rect.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = rect.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let w = rect.get("width").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let h = rect.get("height").and_then(|v| v.as_f64()).unwrap_or(0.0);

            stats.total_boxes += 1;

            if x != 0.0 || y != 0.0 {
                stats.positioned += 1;
            } else {
                stats.at_origin += 1;
            }

            if w > 0.0 && h > 0.0 {
                stats.sized += 1;
            } else {
                stats.zero_size += 1;
            }
        }

        if let Some(children) = node.get("children").and_then(|v| v.as_array()) {
            for child in children {
                walk(child, stats);
            }
        }
    }

    if let Some(root) = data.get("root") {
        walk(root, &mut stats);
    }

    let total = stats.total_boxes.max(1) as f32;
    stats.sizing_rate = stats.sized as f32 / total;
    stats.positioning_rate = stats.positioned as f32 / total;

    Some(stats)
}

/// The first script of an `--html-file` load that failed: it threw, the
/// engine failed, the budget stopped it, or it was skipped though the load
/// meant to run it (too large, or the engine panicked earlier). A script the
/// HTML load does not run (an external `<script src>`, a module, `nomodule`)
/// is not a failure.
fn inline_script_failure(log: &[ScriptRecord]) -> Option<&ScriptRecord> {
    log.iter().find(|r| match r.outcome {
        ScriptOutcome::Threw(_) | ScriptOutcome::EngineFailed(_) | ScriptOutcome::OverBudget => true,
        ScriptOutcome::Skipped(why) => why != rustkit_engine::HTML_LOAD_RUNS_INLINE_CLASSIC_ONLY,
        _ => false,
    })
}

/// The engine configuration a capture runs with: the parity defaults, plus
/// whatever the command line overrides.
/// The app's shield as the engine's request interceptor, with `--shield`
/// only, once its filter lists are built or `--shield-wait-ms` has passed.
fn shield_interceptor(args: &Args) -> Option<rustkit_net::RequestInterceptor> {
    if !args.shield {
        return None;
    }
    // The event log names the list and rule of a block.
    hiwave_shield_intercept::name_matched_rules();
    let ready = hiwave_shield_intercept::wait_until_ready(Duration::from_millis(args.shield_wait_ms));
    eprintln!("shield: on, filter lists {}", if ready { "ready" } else { "not ready (domain floor only)" });
    Some(hiwave_shield_intercept::create_shield_interceptor())
}

fn capture_config(args: &Args, replay_proxy: Option<Url>) -> EngineConfig {
    let mut config = EngineConfig::for_parity_testing();
    if let Some(horizon) = args.timer_horizon_ms {
        config.timer_horizon_ms = horizon;
    }
    if let Some(budget) = args.script_budget_ms {
        config.script_budget_ms = budget;
    }
    if let Some(budget) = args.live_turn_budget_ms {
        config.live_turn_budget_ms = budget;
    }
    config.interrupt_scripts_at_budget = args.interrupt_scripts;
    config.post_load_requests_live = args.post_load_live || args.wpt;
    if args.animations {
        config.disable_animations = false;
    }
    // The stepped clock only when asked for, and only for a stepped capture; a live run reads a clock
    // that flows.
    config.deterministic_page_clock = args.deterministic_clock && args.live_ms.is_none();
    // Grid frame stamps whenever there is no --live-ms (TIMELINE1): the timeline, and so every script
    // animation, reads exact grid times, the same every run. That includes the live turns of an
    // --animations settle and of a prelude, which run on the real clock: there too only the
    // timeline is on the grid; frame callbacks' timestamps and performance.now() flow as in a
    // --live-ms run.
    config.grid_frame_stamps = args.live_ms.is_none();
    // The replay transport restricts sockets (including redirects) to this
    // loopback endpoint. The WPT interceptor also checks the requested origin.
    config.replay_proxy = if args.wpt {
        args.url.as_ref().and_then(|raw| Url::parse(raw).ok())
    } else {
        replay_proxy
    };
    config.inspector.keep_url_query = args.event_log_keep_query;
    config.inspector.keep_console_text = args.event_log_console_text;
    config.inspector.keep_error_text = args.event_log_error_text;
    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// #799 review (3): `--run-inline-scripts` fails a capture only on a
    /// script that threw, failed the engine or ran out of budget. An external
    /// `<script src>` or a module the HTML load skips is not one.
    #[test]
    fn run_inline_scripts_fails_only_on_scripts_that_failed() {
        let log_of = |html: &str| {
            let mut engine = EngineBuilder::new().javascript_enabled(true).build().expect("engine");
            let view = engine
                .create_headless_view(Bounds { x: 0, y: 0, width: 200, height: 100 })
                .expect("view");
            engine.load_html(view, html).expect("load_html");
            engine.script_log(view).unwrap_or_default().to_vec()
        };
        let skipped = log_of(
            "<html><body><script src=\"/app.js\"></script>\
<script type=\"module\">window.m = 1;</script><script>var ok = 1;</script></body></html>",
        );
        assert!(skipped.iter().any(|r| matches!(r.outcome, ScriptOutcome::Skipped(_))), "{skipped:?}");
        assert!(inline_script_failure(&skipped).is_none(), "{skipped:?}");
        let threw = log_of("<html><body><script src=\"/app.js\"></script><script>throw new Error('x');</script></body></html>");
        let failed = inline_script_failure(&threw).expect("a thrown script fails the capture");
        assert_eq!(failed.source, "inline#2", "{threw:?}");
        // #799 nit: an inline classic script past the size cap was meant to
        // run and did not.
        let big = format!("<html><body><script>/*{}*/</script></body></html>", "x".repeat(4 * 1024 * 1024 + 1));
        let too_large = log_of(&big);
        let failed = inline_script_failure(&too_large).expect("a too-large inline script fails the capture");
        assert!(matches!(failed.outcome, ScriptOutcome::Skipped(why) if why.starts_with("too large")), "{too_large:?}");
    }

    #[test]
    fn console_log_is_off_unless_a_path_is_given() {
        assert_eq!(parsed(&[]).console_log, None);
        assert_eq!(
            parsed(&["--console-log", "/tmp/console.jsonl"]).console_log,
            Some(std::path::PathBuf::from("/tmp/console.jsonl"))
        );
    }

    fn parsed(extra: &[&str]) -> Args {
        let mut argv = vec!["parity-capture", "--url", "https://example.test/"];
        argv.extend_from_slice(extra);
        Args::try_parse_from(argv).expect("the command line parses")
    }

    // The live loop is the app's, not the board's: a capture turns it only
    // when asked.
    // The board runs unfiltered; `--shield` puts the app's shield in front
    // of every subresource, and a block names its list and rule.
    #[test]
    fn the_shield_filters_requests_only_with_the_flag() {
        assert!(shield_interceptor(&parsed(&[])).is_none());
        let interceptor = shield_interceptor(&parsed(&["--shield", "--shield-wait-ms", "0"])).expect("an interceptor");
        let request = |url: &str| {
            let mut request = rustkit_net::Request::get(Url::parse(url).unwrap());
            request.destination = rustkit_net::RequestDestination::Script;
            request.referrer = Some(Url::parse("https://news.example/").unwrap());
            request
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        match rt.block_on(interceptor.intercept(&request("https://doubleclick.net/pagead.js"))) {
            rustkit_net::InterceptAction::BlockedBy(reason) => {
                assert!(!reason.list.is_empty() && reason.rule.is_some(), "{reason:?}")
            }
            other => panic!("not blocked with a reason: {other:?}"),
        }
        assert!(matches!(
            rt.block_on(interceptor.intercept(&request("https://news.example/app.js"))),
            rustkit_net::InterceptAction::Allow
        ));
    }

    #[test]
    fn the_live_loop_is_turned_only_when_asked() {
        assert_eq!(parsed(&[]).live_ms, None);
        assert_eq!(parsed(&["--live-ms", "20000"]).live_ms, Some(20_000));
        assert!(!capture_config(&parsed(&[]), None).post_load_requests_live);
        assert!(capture_config(&parsed(&["--post-load-live"]), None).post_load_requests_live);
    }

    /// FRAME-STAGES: the per-turn stage lines are asked for, and only with a live loop to turn.
    #[test]
    fn frame_stage_lines_are_printed_only_when_asked_and_only_for_a_live_loop() {
        assert!(!parsed(&["--live-ms", "1000"]).frame_stages);
        assert!(parsed(&["--live-ms", "1000", "--frame-stages"]).frame_stages);
        let argv = ["parity-capture", "--url", "https://example.test/", "--frame-stages"];
        assert!(Args::try_parse_from(argv).is_err(), "--frame-stages without --live-ms");
    }

    /// The heights of every box in a `--dump-layout` file, rounded.
    fn layout_heights(layout: &str) -> Vec<i64> {
        fn walk(v: &serde_json::Value, out: &mut Vec<i64>) {
            match v {
                serde_json::Value::Object(map) => {
                    for (k, v) in map {
                        if k == "height" {
                            if let Some(h) = v.as_f64() {
                                out.push(h.round() as i64);
                            }
                        }
                        walk(v, out);
                    }
                }
                serde_json::Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        walk(&serde_json::from_str(layout).expect("layout JSON"), &mut out);
        out
    }

    /// T21 (board 11-04, 11-05): a `--html-file` page that finishes on a
    /// timer, an animation frame or a ResizeObserver callback (Chart.js,
    /// the Inspector) captured without them. With `--live-ms` the file is
    /// turned through the same live loop as `--url`; each box below gets
    /// its height only from its own kind of callback.
    const LATE_CALLBACK_PAGE: &str = "<!doctype html><html><head><style>\
        body{margin:0} div{width:50px;height:10px}\
        .t{height:137px} .f{height:149px} .r{height:163px}\
        </style></head><body><div id=t></div><div id=f></div><div id=r></div><script>\
        setTimeout(function(){ document.getElementById('t').className='t'; }, 0);\
        requestAnimationFrame(function(){ document.getElementById('f').className='f'; });\
        new ResizeObserver(function(){ document.getElementById('r').className='r'; })\
            .observe(document.getElementById('r'));\
        </script></body></html>";

    fn capture_file_page(name: &str, extra: &[&str]) -> (CaptureResult, Vec<i64>) {
        capture_html(name, LATE_CALLBACK_PAGE, extra)
    }

    fn capture_html(name: &str, html: &str, extra: &[&str]) -> (CaptureResult, Vec<i64>) {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("parity-capture-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (page, layout) = (dir.join("page.html"), dir.join("layout.json"));
        fs::write(&page, html).unwrap();
        let mut argv = vec![
            "parity-capture",
            "--html-file",
            page.to_str().unwrap(),
            "--dump-layout",
            layout.to_str().unwrap(),
        ];
        argv.extend_from_slice(extra);
        let result = run_capture(&Args::try_parse_from(argv).unwrap());
        let heights = fs::read_to_string(&layout).map(|l| layout_heights(&l)).unwrap_or_default();
        let _ = fs::remove_dir_all(&dir);
        (result, heights)
    }

    #[test]
    fn a_file_capture_with_live_ms_runs_timers_frames_and_resize_observers() {
        let (result, heights) = capture_file_page("t21-live", &["--live-ms", "600"]);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        for (what, h) in [("setTimeout(0)", 137), ("requestAnimationFrame", 149), ("ResizeObserver", 163)] {
            assert!(heights.contains(&h), "{what} callback did not run: heights {heights:?}");
        }
        let live = result.live_stats.as_ref().expect("a file capture with --live-ms reports live_stats");
        assert!(live.timer_callbacks >= 2, "timer + frame callbacks: {}", live.timer_callbacks);
        assert!(result.capture_ms.is_some_and(|ms| ms > 0.0), "{:?}", result.capture_ms);
    }

    /// PACE1: one frame runs 2000 ms (`docs/diagnostics/2026-11-15/long_turn_resume.html`), then the
    /// page records how long the next frame took to come (the box of height 10000 + ms) and how many
    /// came in the second after (20000 + count). By default the live loop paces as the app's content
    /// thread does (`rustkit_engine::content_turn_wait`, the app's default loop): the next frame is
    /// due at once, as in Chrome for Testing 148 (0-1 ms, 60-61 frames; `long_turn_resume_cft148.json`).
    /// `--window-thread-pacing` keeps the wait of the app's window thread with the content thread off:
    /// at least as long as the last turn took, so about 2000 ms. The bounds are wall-clock bounds with
    /// wide margins on either side of the two behaviours (measured 5-15 ms and 2010-2017 ms;
    /// `long_turn_resume_engine.json`): a 2 to 50 ms wait would have to run 450 ms late to fail the
    /// first, and no load makes a 2000 ms wait shorter.
    const LONG_TURN_PAGE: &str =
        include_str!("../../../docs/diagnostics/2026-11-15/long_turn_resume.html");

    fn after_long_turn(name: &str, extra: &[&str]) -> (i64, i64, String) {
        let mut argv = vec!["--live-ms", "7000"];
        argv.extend_from_slice(extra);
        let (result, heights) = capture_html(name, LONG_TURN_PAGE, &argv);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let pick = |lo: i64| {
            heights
                .iter()
                .find(|h| (lo..lo + 10_000).contains(*h))
                .map(|h| h - lo)
        };
        let first =
            pick(10_000).unwrap_or_else(|| panic!("the page did not finish: heights {heights:?}"));
        let frames = pick(20_000).expect("frames after");
        let pacing = result.live_stats.map(|l| l.pacing).unwrap_or_default();
        // One line per run for `long_turn_resume_engine.py`.
        eprintln!(
            "PACE1-RESULT {}",
            serde_json::json!({ "arm": name, "firstFrameAfterMs": first, "afterFrames": frames, "pacing": pacing })
        );
        (first, frames, pacing)
    }

    #[test]
    fn after_a_long_frame_the_live_loop_runs_the_next_frame_when_it_is_due() {
        let (first, frames, pacing) = after_long_turn("pace1-content", &[]);
        assert_eq!(pacing, "content-thread");
        assert!(
            first < 500,
            "the next frame came {first} ms after a 2000 ms frame ({frames} frames after)"
        );
        assert!(
            frames >= 10,
            "{frames} frames in the second after the long frame (first after {first} ms)"
        );
    }

    #[test]
    fn window_thread_pacing_waits_as_long_as_the_long_frame() {
        let (first, _, pacing) = after_long_turn("pace1-window", &["--window-thread-pacing"]);
        assert_eq!(pacing, "window-thread");
        assert!(
            first >= 1500,
            "the next frame came {first} ms after a 2000 ms frame"
        );
    }

    /// `--live-ms` turns script on by itself. A file whose inline script
    /// throws fails the capture, the same as `--run-inline-scripts`.
    #[test]
    fn a_file_capture_with_live_ms_fails_when_an_inline_script_throws() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("parity-capture-t21-throw-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let page = dir.join("page.html");
        fs::write(&page, "<!doctype html><script>throw new Error('nope')</script>").unwrap();
        let result = run_capture(&Args::try_parse_from([
            "parity-capture",
            "--html-file",
            page.to_str().unwrap(),
            "--live-ms",
            "200",
        ]).unwrap());
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(result.status, "error", "{:?}", result.error);
        let err = result.error.unwrap_or_default();
        assert!(err.contains("Inline script failed"), "{err}");
    }

    /// The board's fixture capture is unchanged: without `--live-ms` a file
    /// runs no script and turns no loop.
    #[test]
    fn a_file_capture_without_live_ms_is_the_boards_static_capture() {
        let (result, heights) = capture_file_page("t21-static", &[]);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        assert!(result.live_stats.is_none());
        for h in [137, 149, 163] {
            assert!(!heights.contains(&h), "no script ran, but a box is {h}px: {heights:?}");
        }
    }

    /// With `--run-inline-scripts` (the board's script pages) and no
    /// `--live-ms`, the inline scripts run but nothing after the load does:
    /// what the board captured before T21.
    #[test]
    fn a_file_capture_with_inline_scripts_only_is_unchanged() {
        let (result, heights) = capture_file_page("t21-inline", &["--run-inline-scripts"]);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        assert!(result.live_stats.is_none());
        assert!(!heights.contains(&137), "{heights:?}");
    }

    // The board is measured at the engine's 5 s script budget, and the live
    // app runs pages at 60 s (#573). `--script-budget-ms` lets one labelled
    // run be taken at the app's budget; without the flag nothing changes.
    #[test]
    fn the_script_budget_is_the_engines_default_without_the_flag() {
        let config = capture_config(&parsed(&[]), None);
        assert_eq!(config.script_budget_ms, 5_000);
        assert_eq!(
            config.script_budget_ms,
            EngineConfig::for_parity_testing().script_budget_ms
        );
        assert_eq!(
            config.timer_horizon_ms,
            EngineConfig::for_parity_testing().timer_horizon_ms
        );
    }

    /// LAT14: animations run in a capture only with `--animations`; the
    /// board's frames stay still.
    #[test]
    fn animations_run_only_with_the_flag() {
        assert!(capture_config(&parsed(&[]), None).disable_animations);
        assert!(capture_config(&parsed(&["--live-ms", "1000"]), None).disable_animations);
        assert!(!capture_config(&parsed(&["--animations"]), None).disable_animations);
        // The stepped page clock only with --deterministic-clock, and never in a live-ms run.
        assert!(!capture_config(&parsed(&[]), None).deterministic_page_clock);
        assert!(!capture_config(&parsed(&["--animations"]), None).deterministic_page_clock);
        assert!(capture_config(&parsed(&["--deterministic-clock"]), None).deterministic_page_clock);
        assert!(!capture_config(&parsed(&["--deterministic-clock", "--live-ms", "1000"]), None).deterministic_page_clock);
        // Grid frame stamps in every load-only capture (script animations reproduce), never in a
        // live-ms run (TIMELINE1).
        assert!(capture_config(&parsed(&[]), None).grid_frame_stamps);
        assert!(!capture_config(&parsed(&["--live-ms", "1000"]), None).grid_frame_stamps);
    }

    /// LAT14: the prelude page and how long it runs live before the
    /// `--url` load (the fixture path has no live loop and ignores it).
    #[test]
    fn a_prelude_is_parsed_with_its_live_time() {
        assert_eq!(parsed(&["--prelude-url", "http://p/"]).prelude_url.as_deref(), Some("http://p/"));
        assert_eq!(parsed(&[]).prelude_live_ms, None);
        assert_eq!(parsed(&["--prelude-live-ms", "250"]).prelude_live_ms, Some(250));
    }

    /// A stress-suite run may raise the live turn's watchdog (`--live-turn-budget-ms`) so a long computation
    /// finishes and the next blocker shows; without the flag the engine's default stands.
    #[test]
    fn the_live_turn_budget_is_raised_only_with_the_flag() {
        let default = EngineConfig::for_parity_testing().live_turn_budget_ms;
        assert_eq!(capture_config(&parsed(&["--interrupt-scripts"]), None).live_turn_budget_ms, default);
        let raised = capture_config(&parsed(&["--interrupt-scripts", "--live-turn-budget-ms", "600000"]), None);
        assert_eq!(raised.live_turn_budget_ms, 600_000);
        assert!(raised.interrupt_scripts_at_budget);
        // Without the stop the flag would change nothing; past an hour, or zero, it is refused.
        for bad in [
            &["--live-turn-budget-ms", "60000"][..],
            &["--interrupt-scripts", "--live-turn-budget-ms", "0"][..],
            &["--interrupt-scripts", "--live-turn-budget-ms", "3600001"][..],
            &["--interrupt-scripts", "--live-turn-budget-ms", "18446744073709551615"][..],
        ] {
            let mut argv = vec!["parity-capture", "--url", "http://a/"];
            argv.extend_from_slice(bad);
            assert!(Args::try_parse_from(&argv).is_err(), "accepted {bad:?}");
        }
        let most = capture_config(&parsed(&["--interrupt-scripts", "--live-turn-budget-ms", "3600000"]), None);
        assert_eq!(most.live_turn_budget_ms, 3_600_000);
    }

    #[test]
    fn a_running_script_is_stopped_at_the_budget_only_with_the_flag() {
        assert!(!capture_config(&parsed(&[]), None).interrupt_scripts_at_budget);
        assert!(!capture_config(&parsed(&["--script-budget-ms", "60000"]), None).interrupt_scripts_at_budget);
        let config = capture_config(&parsed(&["--interrupt-scripts"]), None);
        assert!(config.interrupt_scripts_at_budget);
        assert_eq!(config.script_budget_ms, 5_000);
    }

    #[test]
    fn event_log_urls_lose_their_query_unless_asked() {
        assert!(!capture_config(&parsed(&["--event-log", "x.jsonl"]), None).inspector.keep_url_query);
        assert!(capture_config(&parsed(&["--event-log-keep-query"]), None).inspector.keep_url_query);
        assert!(!capture_config(&parsed(&["--event-log", "x.jsonl"]), None).inspector.keep_console_text);
        assert!(capture_config(&parsed(&["--event-log-console-text"]), None).inspector.keep_console_text);
        assert!(!capture_config(&parsed(&["--event-log", "x.jsonl"]), None).inspector.keep_error_text);
        assert!(capture_config(&parsed(&["--event-log-error-text"]), None).inspector.keep_error_text);
        // The flags add a sink; the buffer is the tab ring (T6b), on from
        // the start only with HIWAVE_DEVTOOLS_RING=1.
        let config = capture_config(&parsed(&["--event-summary"]), None);
        assert_eq!(config.inspector.capacity, rustkit_engine::InspectorConfig::tab().capacity);
    }

    /// A fixture capture with both flags: the log is JSON Lines of the
    /// whole load, and the summary counts it.
    #[test]
    fn an_event_log_and_summary_cover_the_whole_load() {
        let dir = std::env::temp_dir().join(format!("parity-capture-events-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let html = dir.join("page.html");
        fs::write(&html, "<html><body>x</body></html>").unwrap();
        let log = dir.join("events.jsonl");
        let args = Args::try_parse_from([
            "parity-capture",
            "--html-file",
            html.to_str().unwrap(),
            "--event-log",
            log.to_str().unwrap(),
            "--event-summary",
        ])
        .unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let text = fs::read_to_string(&log).unwrap();
        let kinds: Vec<String> = text
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["type"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(kinds.first().map(String::as_str), Some("navigation_start"));
        assert!(kinds.contains(&"load_finished".to_string()), "{kinds:?}");
        let summary = result.event_summary.expect("a summary");
        assert_eq!(summary.events as usize, kinds.len());
        assert_eq!(summary.schema, rustkit_engine::inspector::SUMMARY_SCHEMA);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A page whose only pending work is an inserted script's fetch (no
    /// timer set, nothing `in_flight`) still runs the script when its fetch
    /// arrives, not on the live window's last turn. With the window thread's
    /// wait only the engine's wake (`LiveWake`) ends the sleep; the content
    /// thread's wait turns at least every 50 ms and never consults it.
    #[test]
    fn an_inserted_script_runs_when_its_fetch_arrives_not_at_the_end_of_the_window() {
        inserted_script_runs_when_its_fetch_arrives(&[]);
    }

    #[test]
    fn with_window_thread_pacing_the_engines_wake_runs_an_inserted_script_when_its_fetch_arrives() {
        inserted_script_runs_when_its_fetch_arrives(&["--window-thread-pacing"]);
    }

    fn inserted_script_runs_when_its_fetch_arrives(extra: &[&str]) {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let (ctype, body) = if line.starts_with("GET /late.js") {
                        std::thread::sleep(Duration::from_millis(300));
                        ("text/javascript", "console.log('late ran');".to_string())
                    } else {
                        let page = "<html><body>x<script>setTimeout(function () { var s = document.createElement('script'); s.src = '/late.js'; document.body.appendChild(s); }, 50);</script></body></html>";
                        ("text/html", page.to_string())
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-wake-{}-{}", extra.len(), std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let log = dir.join("events.jsonl");
        let url = format!("http://127.0.0.1:{port}/");
        let mut argv = vec!["parity-capture", "--url", url.as_str(), "--live-ms", "6000", "--event-log", log.to_str().unwrap()];
        argv.extend_from_slice(extra);
        let result = run_capture(&Args::try_parse_from(argv).unwrap());
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let pacing = result.live_stats.as_ref().map(|l| l.pacing.as_str());
        let expected = if extra.is_empty() { "content-thread" } else { "window-thread" };
        assert_eq!(pacing, Some(expected));
        let records: Vec<serde_json::Value> = fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let at = |pred: &dyn Fn(&serde_json::Value) -> bool| records.iter().find(|r| pred(r)).map(|r| r["t_ms"].as_f64().unwrap());
        let load = at(&|r| r["type"] == "load_finished").expect("load_finished");
        let ran = at(&|r| r["type"] == "console").expect("late.js logged");
        assert!(ran - load < 2_000.0, "late.js ran {:.0} ms after load (the window is 6000 ms)", ran - load);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_script_budget_flag_sets_the_engines_script_budget_and_nothing_else() {
        let config = capture_config(&parsed(&["--script-budget-ms", "60000"]), None);
        assert_eq!(config.script_budget_ms, 60_000);
        assert_eq!(
            config.timer_horizon_ms,
            EngineConfig::for_parity_testing().timer_horizon_ms
        );
    }

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    #[test]
    fn inlines_relative_stylesheet_link_in_place() {
        let dir = std::env::temp_dir().join("pc-test-inline");
        let _ = fs::remove_dir_all(&dir);
        write_file(&dir, "common/reset.css", "h1 { color: red; }");
        let html = r#"<html><head><link rel="stylesheet" href="common/reset.css"><style>h1{color:blue}</style></head></html>"#;

        let out = inline_stylesheet_links(html, &dir);

        assert!(!out.contains("<link"), "link tag should be replaced");
        assert!(out.contains("h1 { color: red; }"));
        // Order preserved: inlined sheet before the fixture's own <style>.
        let inlined = out.find("color: red").unwrap();
        let fixture = out.find("color:blue").unwrap();
        assert!(inlined < fixture);
    }

    /// The WPT masking reftests mask a box with an image file beside the
    /// page (`mask-image: url(support/50x50-opaque-blue.svg)`), served by
    /// wptserve. `load_html` fetches no image (about:blank base, http(s)
    /// only), so the box painted nothing and 15 of the 30 css-masking-mask
    /// cases failed without measuring the mask. Chromium 148 on this page
    /// served over http: purple at (10, 10), white at (58, 10) and (10, 100).
    /// (Over file:// Chromium paints nothing either: a mask image is a
    /// CORS-mode fetch and a file:// page has an opaque origin.)
    #[test]
    fn an_html_file_capture_masks_with_an_image_file_beside_the_page() {
        let dir = std::env::temp_dir().join(format!("pc-test-mask-file-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_file(
            &dir,
            "support/blue.svg",
            r#"<svg width="50" height="50" xmlns="http://www.w3.org/2000/svg"><rect width="50" height="50" fill="blue"/></svg>"#,
        );
        write_file(
            &dir,
            "page.html",
            "<!DOCTYPE html><style>body { margin: 0 } div { width: 64px; height: 128px; \
background-color: purple; mask-image: url(support/blue.svg); mask-repeat: no-repeat; }</style><div></div>",
        );
        let frame = dir.join("frame.ppm");
        let args = Args::try_parse_from([
            "parity-capture",
            "--html-file",
            dir.join("page.html").to_str().unwrap(),
            "--width",
            "100",
            "--height",
            "160",
            "--dump-frame",
            frame.to_str().unwrap(),
        ])
        .expect("the command line parses");
        let result = capture(&args, None, &mut None);
        assert!(result.error.is_none(), "{:?}", result.error);
        let ppm = fs::read(&frame).unwrap();
        let header_end = ppm.windows(4).position(|w| w == b"255\n").unwrap() + 4;
        let pixel = |x: usize, y: usize| {
            let i = header_end + (y * 100 + x) * 3;
            [ppm[i], ppm[i + 1], ppm[i + 2]]
        };
        assert_eq!(pixel(10, 10), [128, 0, 128], "under the mask image");
        assert_eq!(pixel(58, 10), [255, 255, 255], "right of the 50px mask image");
        assert_eq!(pixel(10, 100), [255, 255, 255], "below it (no-repeat)");
        let _ = fs::remove_dir_all(&dir);
    }

    /// #966 R1: a url() inside a double-quoted `style` attribute. Writing the
    /// data URL as `url("data:...")` closed the attribute and the element
    /// lost its whole style (border and size too). Chrome for Testing 148 on
    /// this page over http: the 4px green border at (1, 1) and (66, 30),
    /// white at (70, 30), the blue image inside it at (30, 30).
    #[test]
    fn an_image_in_a_double_quoted_style_attribute_keeps_the_rest_of_the_style() {
        let dir = std::env::temp_dir().join(format!("pc-test-style-attr-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_file(
            &dir,
            "support/blue.svg",
            r#"<svg width="60" height="60" xmlns="http://www.w3.org/2000/svg"><rect width="60" height="60" fill="blue"/></svg>"#,
        );
        write_file(
            &dir,
            "page.html",
            r#"<!DOCTYPE html><body style="margin:0"><div style="background-image:url(support/blue.svg);width:60px;height:60px;border:4px solid green;background-repeat:no-repeat;background-position:4px 4px"></div>"#,
        );
        let frame = dir.join("frame.ppm");
        let args = Args::try_parse_from([
            "parity-capture",
            "--html-file",
            dir.join("page.html").to_str().unwrap(),
            "--width",
            "100",
            "--height",
            "100",
            "--dump-frame",
            frame.to_str().unwrap(),
        ])
        .expect("the command line parses");
        let result = capture(&args, None, &mut None);
        assert!(result.error.is_none(), "{:?}", result.error);
        let ppm = fs::read(&frame).unwrap();
        let header_end = ppm.windows(4).position(|w| w == b"255\n").unwrap() + 4;
        let pixel = |x: usize, y: usize| {
            let i = header_end + (y * 100 + x) * 3;
            [ppm[i], ppm[i + 1], ppm[i + 2]]
        };
        assert_eq!(pixel(1, 1), [0, 128, 0], "the border survives");
        assert_eq!(pixel(66, 30), [0, 128, 0], "and the 60px width: right border at 64..68");
        assert_eq!(pixel(70, 30), [255, 255, 255], "nothing past the border box");
        // Not pinned: the image itself. Chromium paints it blue at (30, 30);
        // the engine splits a `style` attribute at every `;`, so the
        // `;base64,` of any data: URL there cuts the declaration (an engine
        // gap, C8 queue; a <style> block is not affected).
        let _ = fs::remove_dir_all(&dir);
    }

    /// What is inlined and what is left: a sheet's url() is relative to the
    /// sheet; scheme, fragment and missing references are untouched.
    #[test]
    fn inlines_local_image_files_and_leaves_other_references() {
        let dir = std::env::temp_dir().join(format!("pc-test-img-inline-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        write_file(&dir, "a.svg", "<svg/>");
        write_file(&dir, "css/b.png", "PNG");
        write_file(&dir, "css/s.css", "p { background: url( 'b.png' ) }");
        let html = concat!(
            r#"<link rel="stylesheet" href="css/s.css">"#,
            r#"<style>div { mask: url(a.svg) no-repeat; x: url(#m); y: url(a.svg#m); z: url(missing.svg); w: url(https://h/a.svg); }</style>"#,
            r#"<img alt=x src=a.svg><img src="data:image/png;base64,AA"><img data-src="a.svg">"#,
        );
        let out = preprocess_html(html, &dir.join("page.html"));
        let svg = "url(data:image/svg+xml;base64,PHN2Zy8+)";
        assert!(out.contains(&format!("mask: {svg} no-repeat")), "{out}");
        assert!(out.contains("background: url(data:image/png;base64,UE5H)"), "{out}");
        assert!(out.contains(r#"<img alt=x src="data:image/svg+xml;base64,PHN2Zy8+">"#), "{out}");
        for kept in ["url(#m)", "url(a.svg#m)", "url(missing.svg)", "url(https://h/a.svg)", r#"src="data:image/png;base64,AA""#, r#"data-src="a.svg""#] {
            assert!(out.contains(kept), "{kept} should be left alone: {out}");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leaves_remote_and_missing_links_alone() {
        let dir = std::env::temp_dir().join("pc-test-remote");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let html = concat!(
            r#"<link rel="stylesheet" href="https://example.com/a.css">"#,
            r#"<link rel="stylesheet" href="missing.css">"#,
            r#"<link rel="icon" href="fav.ico">"#,
        );

        let out = inline_stylesheet_links(html, &dir);
        assert_eq!(out, html);
    }

    #[test]
    fn wraps_media_scoped_links() {
        let dir = std::env::temp_dir().join("pc-test-media");
        let _ = fs::remove_dir_all(&dir);
        write_file(&dir, "print.css", "body { display: none; }");
        let html = r#"<link rel="stylesheet" href="print.css" media="print">"#;

        let out = inline_stylesheet_links(html, &dir);
        assert!(out.contains("@media print {"));
    }

    #[test]
    fn injects_reset_first_in_head() {
        let html = "<html><head><style>b{}</style></head></html>";
        let out = inject_style_first_in_head(html, "data-parity-reset=\"1\"", "x{}");
        let reset = out.find("data-parity-reset").unwrap();
        let fixture = out.find("<style>b{}").unwrap();
        assert!(reset < fixture);
    }

    #[test]
    fn micro_suite_detection_accepts_relative_paths() {
        // The predicate must not depend on how the caller spelled the path:
        // CI passes absolute, humans pass relative, and the reset injection
        // silently diverging between them is a capture-environment asymmetry
        // (see the canonicalize comment on is_micro_suite_path).
        // Canonicalization needs a real file, so use one from the tree.
        let real = Path::new("websuite/micro/gradients/index.html");
        if real.exists() {
            assert!(
                is_micro_suite_path(real),
                "relative micro path must be detected"
            );
        }
        assert!(!is_micro_suite_path(Path::new(
            "websuite/cases/x/index.html"
        )));
    }

    #[test]
    fn micro_suite_detection_matches_deterministic_mjs() {
        assert!(is_micro_suite_path(Path::new(
            "/repo/websuite/micro/bg-solid/index.html"
        )));
        assert!(!is_micro_suite_path(Path::new(
            "/repo/websuite/pages/blog/index.html"
        )));
    }

    #[test]
    fn inspector_flags_parse() {
        let argv = |extra: &[&str]| {
            let mut v = vec!["parity-capture"];
            v.extend_from_slice(extra);
            Args::try_parse_from(v)
        };
        // A dump file is a source of its own; it needs an output directory
        // and no page.
        let a = argv(&["--inspector-dump", "d.json", "--inspector-out", "o"]).unwrap();
        assert_eq!((a.inspector_dump.as_deref(), a.inspector_size), (Some("d.json"), (1280, 800)));
        assert!(argv(&["--inspector-dump", "d.json"]).is_err());
        assert!(argv(&["--inspector-dump", "d.json", "--inspector-out", "o", "--html-file", "x.html"]).is_err());
        // A point selects in a captured page, not in a dump file.
        assert!(argv(&["--inspector-dump", "d.json", "--inspector-out", "o", "--inspector-select-at", "1,2"]).is_err());
        let b = parsed(&["--inspector-out", "o", "--inspector-select-at", "3,4.5", "--inspector-size", "800x600"]);
        assert_eq!((b.inspector_select_at, b.inspector_size), (Some((3.0, 4.5)), (800, 600)));
        assert!(Args::try_parse_from(["parity-capture", "--url", "https://e.test/", "--inspector-diff", "x"]).is_err());
        assert!(!parsed(&[]).run_inline_scripts);
        assert!(parsed(&["--run-inline-scripts"]).run_inline_scripts);
    }

    /// The fixture dump and diff (fixtures/inspector), rendered as the
    /// parity case's page is.
    #[test]
    fn the_inspector_renders_a_dump_file_to_png_and_display_list() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/inspector");
        let dump = fs::read_to_string(root.join("target.planted.dump.json")).unwrap();
        let diff = fs::read_to_string(root.join("f2_planted.diff.json")).unwrap();
        let out = std::env::temp_dir().join(format!("parity-capture-inspector-{}", std::process::id()));
        let written = inspector::render_inspector(&dump, Some(&diff), None, (1280, 800), &out).unwrap();
        assert_eq!((written.elements, written.truncated), (21, false));
        assert_eq!(fs::read_to_string(&written.html_path).unwrap(), fs::read_to_string(root.join("inspector_page.html")).unwrap());
        let png = fs::read(&written.png_path).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!((u32::from_be_bytes(png[16..20].try_into().unwrap()), u32::from_be_bytes(png[20..24].try_into().unwrap())), (1280, 800));
        let list: serde_json::Value = serde_json::from_str(&fs::read_to_string(&written.display_list_path).unwrap()).unwrap();
        let texts: Vec<&str> = list["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| (c["op"] == "text").then(|| c["text"].as_str().unwrap().trim()))
            .collect();
        for want in ["HiWave Inspector", "first difference", "Difference from Chromium (STYLE)", "flex-grow"] {
            assert!(texts.contains(&want), "{want:?} not painted: {texts:?}");
        }
        assert!(!out.join("inspector.ppm").exists(), "the intermediate frame is removed");
        let _ = fs::remove_dir_all(&out);
    }

    #[test]
    fn diagnostics_flags_parse() {
        let args = parsed(&["--diagnose", "#a", "--diagnose", "div > p", "--diagnose-at", "10,20.5"]);
        assert_eq!(args.diagnose, vec!["#a".to_string(), "div > p".to_string()]);
        assert_eq!(args.diagnose_at, vec![(10.0, 20.5)]);
        assert_eq!(args.diagnostics_max_elements, 5000);
        assert!(args.dump_diagnostics.is_none());
        assert!(Args::try_parse_from(["parity-capture", "--url", "https://e.test/", "--diagnose-at", "10"]).is_err());
    }

    #[test]
    fn parse_point_extracts_coordinates_from_js_string() {
        assert_eq!(
            parse_point("String(\"point:100:200\")"),
            Some((100.0, 200.0))
        );
        assert_eq!(parse_point("point:12.5:30.25"), Some((12.5, 30.25)));
        assert_eq!(parse_point("point:-10.0:40.5"), Some((-10.0, 40.5)));
        assert_eq!(parse_point("String(\"fallback_clicked\")"), None);
        assert_eq!(parse_point("missing"), None);
    }

    #[test]
    fn the_content_thread_flag_selects_the_page_worker() {
        let (name, stack) = capture_worker_spec(false);
        assert_eq!((name, stack), ("capture-worker", 16 * 1024 * 1024));
        let (name, stack) = capture_worker_spec(true);
        assert_eq!(name, "page");
        assert!(stack >= 8 * 1024 * 1024, "content stack {stack}");
        let got = std::thread::Builder::new()
            .name(name.into())
            .stack_size(stack)
            .spawn(|| std::thread::current().name().unwrap().to_string())
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(got, "page");
    }

    /// `set_live_wake` is one slot per process: a second live window in the
    /// same test binary takes the first one's wake.
    static LIVE_WAKE_IS_GLOBAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `--report` and `--report-text` on a fixture: the report JSON and text.
    fn report_for(name: &str, html: &str) -> (serde_json::Value, String) {
        report_with(name, html, &[])
    }

    fn report_with(name: &str, html: &str, extra: &[&str]) -> (serde_json::Value, String) {
        let dir = std::env::temp_dir().join(format!("parity-capture-report-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let page = dir.join("page.html");
        fs::write(&page, html).unwrap();
        let (json, text) = (dir.join("report.json"), dir.join("report.txt"));
        let mut argv = vec![
            "parity-capture",
            "--html-file",
            page.to_str().unwrap(),
            "--report",
            json.to_str().unwrap(),
            "--report-text",
            text.to_str().unwrap(),
        ];
        argv.extend_from_slice(extra);
        let args = Args::try_parse_from(argv).unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let report = serde_json::from_str(&fs::read_to_string(&json).expect("report written")).unwrap();
        let text = fs::read_to_string(&text).expect("report text written");
        let _ = fs::remove_dir_all(&dir);
        (report, text)
    }

    fn causes(report: &serde_json::Value) -> Vec<String> {
        report["ranked"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| format!("{} {}", c["rule"].as_str().unwrap(), c["line"].as_str().unwrap()))
            .collect()
    }

    #[test]
    fn a_fixed_dialog_over_a_locked_body_is_the_first_cause() {
        let (report, text) = report_for(
            "lock",
            "<html><head><style>body { margin: 0; overflow: hidden } .story { height: 3000px }
#consent { position: fixed; top: 0; left: 0; right: 0; bottom: 0; z-index: 9999; background: white }</style></head>
<body><div class=\"story\"><p>story</p></div><div id=\"consent\" class=\"ot-sdk banner\"><p>cookies</p></div></body></html>",
        );
        assert_eq!(
            causes(&report),
            vec![
                "R1 scroll locked (body overflow-y:hidden) by overlay: fixed div#consent.ot-sdk.banner (z 9999) covers 100% of the viewport",
                // The fixture's story is one empty 3000px box.
                "R9 below the fold is empty from y=800 (document 3016px tall): no text or image starts in 800..1600",
            ],
            "{text}"
        );
        assert!(text.contains("1. [R1] scroll locked"), "{text}");
    }

    /// cnn, 2026-10-24: the consent script makes the body `position: fixed`
    /// with `overflow: hidden`. The body is the lock, not the overlay, and
    /// its content is still the page's; a bottom bar smaller than the
    /// covering share is what the lock names.
    #[test]
    fn a_fixed_body_is_the_lock_and_the_largest_fixed_box_is_named() {
        let (report, text) = report_for(
            "fixed-body",
            "<html><head><style>body { margin: 0; position: fixed; overflow: hidden; width: 100% }
p { height: 300px; margin: 0 } #bar { position: fixed; left: 0; right: 0; bottom: 0; height: 160px; z-index: 50; background: white }</style></head>
<body><p>one</p><p>two</p><p>three</p><p>four</p><p>five</p><p>six</p><div style=\"position: fixed; left: 0; right: 0; bottom: 0; height: 160px; z-index: 50; background: white\"><p>cookies</p></div></body></html>",
        );
        // No id or class: named by its structural selector's last steps.
        assert_eq!(
            causes(&report),
            vec!["R1 scroll locked (body overflow-y:hidden) by overlay: fixed body:nth-child(2) > div:nth-child(7) (z 50) covers 20% of the viewport"],
            "{text}"
        );
        assert_eq!(report["covering"], serde_json::json!([]), "{text}");
        assert_eq!(report["bands"][1]["text"], 3, "{text}");
    }

    #[test]
    fn an_empty_viewport_band_below_content_is_r9() {
        let (report, text) = report_for(
            "band",
            "<html><body style=\"margin: 0\"><p>top</p><div style=\"height: 2400px\"></div><p>end</p></body></html>",
        );
        assert_eq!(
            causes(&report),
            vec!["R9 below the fold is empty from y=800 (document 2500px tall): no text or image starts in 800..1600"],
            "{text}"
        );
    }

    #[test]
    fn a_page_with_nothing_to_read_in_the_first_viewport_is_r10() {
        let (report, text) = report_for("blank", "<html><body><div style=\"height:10px\"></div></body></html>");
        let causes = causes(&report);
        assert_eq!(causes.len(), 1, "{text}");
        assert!(causes[0].starts_with("R10 the first viewport has no text and no image ("), "{text}");
    }

    #[test]
    fn a_short_plain_page_fires_no_rule_and_says_what_it_did_not_check() {
        let (report, text) = report_for("plain", "<html><body><p>a</p><img src=\"data:,\" width=\"10\" height=\"10\"></body></html>");
        assert_eq!(causes(&report), Vec::<String>::new(), "{text}");
        assert!(text.contains("(no rule fired)"), "{text}");
        // develop has no T2/T3 records: those rules are named as skipped.
        let skipped: Vec<&str> = report["not_checked"].as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
        assert!(skipped.iter().any(|n| n.starts_with("R3, R4")) && skipped.iter().any(|n| n.starts_with("R7")), "{skipped:?}");
    }

    /// T9 gap 1: yahoo's feed was grey rows and the bands read as content.
    /// Ten rows count (colour or gradient); what follows them does not: a dot,
    /// a hairline, a transparent box, a box with a picture for a background,
    /// a form control, a hidden box, a positioned scrim and a box holding text.
    #[test]
    fn a_band_of_empty_painted_boxes_is_named_as_a_possible_skeleton() {
        let rows = "<div style=\"height:20px;margin:4px;background:#eee\"></div>".repeat(8)
            + &"<div style=\"height:20px;margin:4px;background:linear-gradient(#eee,#ddd)\"></div>".repeat(2);
        let others = "<div style=\"width:8px;height:8px;background:#eee\"></div>
<div style=\"height:1px;background:#eee\"></div>
<div style=\"height:20px\"></div>
<div style=\"height:20px;background:url(pic.png)\"></div>
<input style=\"background:#eee\">
<div style=\"height:20px;background:#eee;visibility:hidden\"></div>
<div style=\"height:20px;background:#eee;opacity:0\"></div>
<div style=\"position:absolute;left:0;width:100px;height:20px;background:linear-gradient(#eee,#ddd)\"></div>
<div style=\"height:20px;background:#eee\">words</div>";
        let html = format!(
            "<html><body style=\"margin:0\"><p>top</p><div style=\"height:800px\"></div><p>mid</p><div style=\"height:800px\"></div>{rows}{others}</body></html>"
        );
        let (report, text) = report_for("skeleton", &html);
        assert_eq!(report["bands"][2]["empty_painted"], 10, "{text}");
        assert_eq!(report["bands"][0]["empty_painted"], 0, "{text}");
        assert_eq!(causes(&report), vec!["R11 y 1600..2400: 10 painted boxes with no text and no image (skeleton?)"], "{text}");
        assert!(text.contains("  y 1600..2400: text 1, images 0, empty painted 10, overlapping 0, past right edge 0\n"), "{text}");
    }

    /// T9 gap 6, through a real layout: three rows pulled over the one before,
    /// three pushed past the right edge. A clipped strip of slides and an
    /// out-of-flow label on a row count as neither.
    #[test]
    fn overlap_and_overflow_below_the_fold_are_named_from_a_real_layout() {
        let over = "<div style=\"height:40px;margin-top:-30px\">row</div>".repeat(3);
        let wide = "<div style=\"width:1500px;height:20px\">wide</div>".repeat(3);
        let clipped = "<div style=\"overflow:hidden;height:40px\"><div style=\"width:3000px;height:40px\">slides</div></div>";
        let label = "<div style=\"position:relative;height:40px\">row<div style=\"position:absolute;top:0;left:0\">ad</div></div>";
        let html = format!(
            "<html><body style=\"margin:0\"><p>top</p><div style=\"height:800px\"></div><p>mid</p><div style=\"height:800px\"></div><div style=\"height:40px\">first</div>{over}<div style=\"height:40px\"></div>{wide}{clipped}{label}</body></html>"
        );
        let (report, text) = report_for("misplaced", &html);
        assert_eq!(report["bands"][2]["overlapping"], 3, "{text}");
        assert_eq!(report["bands"][2]["past_right"], 3, "{text}");
        assert_eq!(causes(&report), vec!["R12 y 1600..2400: 3 boxes overlap other content, 3 run past the right edge"], "{text}");
    }

    /// A few empty boxes are a page's decoration, not a placeholder feed.
    #[test]
    fn a_few_empty_painted_boxes_fire_no_rule() {
        let rows = "<div style=\"height:20px;margin:4px;background:#eee\"></div>".repeat(7);
        let (report, text) = report_for("few-empty", &format!("<html><body style=\"margin:0\"><p>top</p>{rows}</body></html>"));
        assert_eq!(report["bands"][0]["empty_painted"], 7, "{text}");
        assert_eq!(causes(&report), Vec::<String>::new(), "{text}");
    }

    /// The flag reaches the report: T9's three runs had no shield and the
    /// report could not say so.
    #[test]
    fn the_report_says_whether_the_shield_was_installed() {
        const LINE: &str = "R6 shield: not installed, nothing could be blocked";
        let page = "<html><body><p>a</p></body></html>";
        let (without, text) = report_for("no-shield", page);
        assert_eq!(without["shield"], false, "{text}");
        assert!(text.contains(LINE), "{text}");
        let (with, text) = report_with("shield", page, &["--shield", "--shield-wait-ms", "0"]);
        assert_eq!(with["shield"], true, "{text}");
        assert!(!text.contains(LINE), "{text}");
    }

    /// A load-only capture (no `--live-ms`) of a page whose reveal timer is due just past the load's
    /// timer pass (Squarespace's homepage settles an A/B test with a timer it sets after the pass: the
    /// timer never ran in a stepped capture, and the hero stayed hidden). The Chrome reference waits
    /// 5 s after load before its screenshot (`CHROME_SETTLE_MS` in scripts/realsite_board.py, the
    /// default of tools/parity_oracle/realsite.mjs); the stepped capture gives the page the same
    /// span of virtual time after the load.
    #[test]
    fn a_stepped_load_only_capture_gives_the_page_the_references_settle_time() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    // The last timer the load's pass (5 s of virtual time) runs schedules the reveal
                    // 200 ms later: due just past the pass, as a timer set after the pass would be.
                    let body = "<html><body><p>x</p><script>setTimeout(function () { setTimeout(function () { var d = document.createElement('div'); d.id = 'revealed-by-timer'; d.style.height = '77px'; d.textContent = 'revealed'; document.body.appendChild(d); }, 200); }, 4990);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-settle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let layout = dir.join("layout.json");
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--dump-layout",
            layout.to_str().unwrap(),
        ])
        .unwrap();
        let began = Instant::now();
        let result = run_capture(&args);
        let took = began.elapsed();
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let dumped = fs::read_to_string(&layout).unwrap();
        assert!(dumped.contains("revealed-by-timer"), "the timer never ran: {}", &dumped[..dumped.len().min(400)]);
        // The settle is 5 s of virtual time: a plain capture of a page with nothing to wait for must not
        // spend 5 s of real time on it (every board capture would).
        assert!(took < Duration::from_millis(3_000), "a plain capture took {took:?}: the settle waited in real time");
    }

    /// An `--animations` capture runs on the real clock, so its settle is a real 5000 ms live phase (live
    /// loop turns), not virtual steps: every capture mode gives the page the reference's 5 s after load.
    /// The page is the first settle test's (a reveal timer due just past the load's timer pass); without
    /// the phase the capture ended at the load and the timer never ran.
    #[test]
    fn an_animations_capture_gets_the_settle_as_a_real_live_phase() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><p>x</p><script>setTimeout(function () { setTimeout(function () { var d = document.createElement('div'); d.id = 'revealed-by-timer'; d.style.height = '77px'; d.textContent = 'revealed'; document.body.appendChild(d); }, 200); }, 4990);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK
Content-Type: text/html
Content-Length: {}
Connection: close

{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-settle-anim-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let layout = dir.join("layout.json");
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--animations",
            "--dump-layout",
            layout.to_str().unwrap(),
        ])
        .unwrap();
        let began = Instant::now();
        let result = run_capture(&args);
        let took = began.elapsed();
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let dumped = fs::read_to_string(&layout).unwrap();
        assert!(dumped.contains("revealed-by-timer"), "the timer never ran: {}", &dumped[..dumped.len().min(400)]);
        assert!(took >= Duration::from_millis(4_500), "the settle was not a real-time phase: the capture took {took:?}");
    }

    /// The settle's virtual span must not depend on how fast the server answers (R1 on #1104). A page
    /// starts a fetch just after the load's 5000 ms timer pass (a timer the pass sets for 5190 ms, so the request is open during the settle) and sets its reveal from the answer
    /// (`fetch config`, then `setTimeout(reveal, c.delay)`). While the request is open the settle
    /// held no virtual time still: it advanced 50 ms a turn and waited 5 ms of real time, so a server
    /// that took 1.5 s put the answer thousands of ms into the settle (a 5 ms wait a turn is 50 ms of
    /// virtual time per 5 ms, and a Windows sleep of 5 ms lasts about 15 ms), the reveal past its 5000 ms
    /// span, and the capture differed from the one against a fast server. The reveal is due 3000 ms
    /// after the answer; with virtual time held until the answer it is always inside the span.
    #[test]
    fn the_settle_holds_virtual_time_while_a_request_is_open() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let (kind, body) = if line.contains("/cfg") {
                        std::thread::sleep(Duration::from_millis(1500));
                        ("application/json", "{\"delay\":3000}".to_string())
                    } else {
                        (
                            "text/html",
                            "<html><body><p>x</p><script>setTimeout(function () { setTimeout(function () { fetch('/cfg').then(function (r) { return r.json(); }).then(function (c) { setTimeout(function () { var d = document.createElement('div'); d.id = 'revealed-by-config'; d.style.height = '77px'; d.textContent = 'revealed'; document.body.appendChild(d); }, c.delay); }); }, 200); }, 4990);</script></body></html>"
                                .to_string(),
                        )
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-settle-slow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let layout = dir.join("layout.json");
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--dump-layout",
            layout.to_str().unwrap(),
        ])
        .unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let dumped = fs::read_to_string(&layout).unwrap();
        assert!(
            dumped.contains("revealed-by-config"),
            "the answer landed too late in virtual time for its timer: {}",
            &dumped[..dumped.len().min(400)]
        );
    }

    /// The settle's wait for an open request is capped in real time, measured, not counted in nominal
    /// sleeps (R1 on #1104: a 5 ms sleep lasts about 15 ms on Windows, so 600 of them were 9 s, not 3 s).
    /// A request that never answers (a long poll, an event stream) costs the cap once, and the result
    /// says how many were still open, as a count.
    #[test]
    fn a_request_that_never_answers_costs_the_settle_its_cap_once_and_is_counted() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    if line.contains("/hang") {
                        // Never answer; hold the connection past the capture.
                        std::thread::sleep(Duration::from_secs(30));
                        return;
                    }
                    let body = "<html><body><p>x</p><script>setTimeout(function () { setTimeout(function () { fetch('/hang'); }, 200); }, 4990);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from(["parity-capture", "--url", url.as_str()]).unwrap();
        let began = Instant::now();
        let result = run_capture(&args);
        let took = began.elapsed();
        assert_eq!(result.status, "ok", "{:?}", result.error);
        assert_eq!(result.settle_held_open, Some(1), "the open request is not counted");
        // The 3 s cap, the load and the stepped rest of the settle; 9 s of nominal sleeps fails it.
        // Windows: 7.5 s. The hosted runner (a CPU rasteriser under load) took 6.02 s; 3.9 s on a GPU
        // machine. 9 s of nominal sleeps still fails it.
        let bound = if cfg!(windows) { 7_500 } else { 6_000 };
        assert!(took < Duration::from_millis(bound), "the settle's cap is not real time: the capture took {took:?}");
    }

    /// The settle is bounded in real time as well (batch 74's A/B: github's settle took over 77 s and
    /// squarespace's 36 s, because nearly every turn relaid the page out at up to a second each). It
    /// stops between turns once its real-time budget (the reference's 5 s, less if the capture's timeout
    /// has less left) is spent, and says how far virtual time got. Here every turn costs about 150 ms,
    /// so 100 turns would take 15 s.
    ///
    /// The bound is on the settle's own figures, not on the whole capture's wall time (FLK7: at load
    /// 7 to 12.5 the capture took 9.5 s against a 9 s limit, the settle itself having stopped at
    /// 4828 ms as designed): the settle stops between turns, so it ends past its budget by at most
    /// its slowest turn, however slow the machine makes the turns.
    #[test]
    fn a_settle_whose_turns_are_slow_stops_at_its_real_time_budget() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><p>x</p><script>setTimeout(function () { setInterval(function () { var t = Date.now(); while (Date.now() - t < 150) {} document.body.style.paddingTop = (document.body.style.paddingTop === '1px' ? '2px' : '1px'); }, 16); }, 4990);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from(["parity-capture", "--url", url.as_str()]).unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let stats = result.settle_stats.clone().expect("a stepped capture reports its settle");
        assert!(stats.cut_short, "the settle was not cut short: {stats:?}");
        assert!(stats.virtual_ms > 0 && stats.virtual_ms < STEPPED_SETTLE_MS, "virtual time reached: {stats:?}");
        let budget = SETTLE_REAL_BUDGET.as_millis() as u64;
        assert!(
            stats.wall_ms <= budget + stats.max_turn_ms,
            "the settle ran past its budget by more than its slowest turn: {stats:?}"
        );
    }

    /// The settle does not start a turn that would likely end past its budget: it stops when the time spent
    /// plus the slowest turn so far would pass it (R1 LOW on #1104: a turn is not interrupted, so a slow
    /// one started just inside the budget overran it). Here every turn costs about 900 ms: the sixth
    /// would start at about 4.5 s and end near 5.4 s, past the 5 s budget.
    #[test]
    fn the_settle_does_not_start_a_turn_that_would_end_past_its_budget() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><p>x</p><script>setTimeout(function () { setInterval(function () { var t = Date.now(); while (Date.now() - t < 900) {} }, 16); }, 4990);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from(["parity-capture", "--url", url.as_str()]).unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let stats = result.settle_stats.clone().expect("a stepped capture reports its settle");
        assert!(stats.cut_short, "{stats:?}");
        assert!(
            stats.wall_ms <= SETTLE_REAL_BUDGET.as_millis() as u64,
            "a turn started that ended past the budget: {stats:?}"
        );
    }

    /// A settle cut short must not freeze a page part-way through its own reveal (batch 75's A/B:
    /// squarespace's settle ran one 3.7 s turn, whose scripts hid the content, and was cut before the
    /// timers that reveal it, so the frame was the header on black where the load-only capture had the
    /// page). When the settle is cut short, the capture's frame, display list and layout are the ones
    /// taken after the load, before the settle. Here a timer early in the settle takes the content out,
    /// every turn after it costs about 900 ms, and the content comes back only 3 s later.
    #[test]
    fn a_settle_cut_short_falls_back_to_the_page_as_the_load_left_it() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><div id='content-from-the-load' style='height:120px'>content</div><script>setTimeout(function () { setTimeout(function () { var c = document.getElementById('content-from-the-load'); c.remove(); setInterval(function () { var t = Date.now(); while (Date.now() - t < 900) {} }, 16); setTimeout(function () { document.body.appendChild(c); }, 3000); }, 200); }, 4990);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-settle-fallback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let layout = dir.join("layout.json");
        let display_list = dir.join("display-list.json");
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--dump-layout",
            layout.to_str().unwrap(),
            "--dump-display-list",
            display_list.to_str().unwrap(),
        ])
        .unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let stats = result.settle_stats.clone().expect("a stepped capture reports its settle");
        assert!(stats.cut_short, "the settle was not cut short: {stats:?}");
        assert!(stats.fell_back, "a cut-short settle did not fall back: {stats:?}");
        let dumped = fs::read_to_string(&layout).unwrap();
        assert!(dumped.contains("content-from-the-load"), "the layout is the half-revealed state: {}", &dumped[..dumped.len().min(300)]);
        let drawn = fs::read_to_string(&display_list).unwrap();
        assert!(drawn.contains("\"content\""), "the display list is the half-revealed state");
    }

    /// A load-only capture runs the page on develop's real-flowing clock unless `--deterministic-clock`
    /// asks for the stepped one (batch 75: under the stepped clock squarespace's load-only capture drew
    /// a blank frame where develop's draws the page; with the clock flowing it is develop's capture). The
    /// page reads `performance.now()` around 40 ms of real time and names what it saw.
    #[test]
    fn a_load_only_capture_reads_a_flowing_clock_by_default() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><script>var a = performance.now(), t = Date.now(); while (Date.now() - t < 40) {} var d = performance.now() - a; var e = document.createElement('div'); e.id = d >= 35 ? 'clock-flows' : 'clock-stepped'; e.style.height = '10px'; document.body.appendChild(e);</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-clock-default-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let layout = dir.join("layout.json");
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from(["parity-capture", "--url", url.as_str(), "--dump-layout", layout.to_str().unwrap()]).unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let dumped = fs::read_to_string(&layout).unwrap();
        assert!(dumped.contains("clock-flows"), "a plain capture stepped the page clock: {}", &dumped[..dumped.len().min(300)]);
        // Asked for, the stepped clock is there.
        let stepped = dir.join("layout-stepped.json");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--deterministic-clock",
            "--dump-layout",
            stepped.to_str().unwrap(),
        ])
        .unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let dumped = fs::read_to_string(&stepped).unwrap();
        assert!(dumped.contains("clock-stepped"), "--deterministic-clock did not step the clock");
    }

    /// When the load has used the capture's timeout down to the margin the frame needs, the settle does not
    /// start at all, not even its first turn: a heavy page (walmart's load is most of the 30 s on the Mac)
    /// must not be pushed past its timeout by the settle.
    #[test]
    fn no_settle_turn_runs_when_the_timeout_leaves_no_budget() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><p>x</p></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        // A timeout no larger than the margin: nothing is left for the settle.
        let timeout = SETTLE_TIMEOUT_MARGIN.as_millis().to_string();
        let args = Args::try_parse_from(["parity-capture", "--url", url.as_str(), "--timeout-ms", timeout.as_str()]).unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let stats = result.settle_stats.clone().expect("a stepped capture reports its settle");
        assert_eq!(stats.turns, 0, "a settle turn ran with no budget: {stats:?}");
        assert!(stats.cut_short && stats.virtual_ms == 0, "{stats:?}");
    }

    /// `--live-ms` end to end: the window's end is the capture's end. A request
    /// still open at the end is pinned in `report.rs`: here a fetch started
    /// inside the load holds the load, so it answers before the window opens.
    #[test]
    fn a_live_capture_reports_whether_the_page_settled() {
        let _one_at_a_time = LIVE_WAKE_IS_GLOBAL.lock().unwrap_or_else(|e| e.into_inner());
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                std::thread::spawn(move || {
                    let mut line = String::new();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let _ = reader.read_line(&mut line);
                    loop {
                        let mut header = String::new();
                        if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                            break;
                        }
                    }
                    let body = "<html><body><p>x</p></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-settled-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let json = dir.join("report.json");
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--live-ms",
            "4000",
            "--report",
            json.to_str().unwrap(),
        ])
        .unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let report: serde_json::Value = serde_json::from_str(&fs::read_to_string(&json).unwrap()).unwrap();
        let settled = &report["settled"];
        assert_eq!(settled["changing"], false, "{settled}");
        assert_eq!(settled["open_requests"], 0, "{settled}");
        assert_eq!(settled["navigated"], false, "{settled}");
        // The layout came with the load, the whole window before the end.
        let before = settled["last_relayout_ms_before"].as_f64().expect("a layout was recorded");
        assert!(before >= 3000.0, "{settled}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// T9 gap 7, through the engine's own console record: a string logged by
    /// the page's inline script reaches R5, and its text does not.
    #[test]
    fn a_string_console_error_from_the_page_reaches_r5_as_a_shape() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                std::thread::spawn(move || {
                    let mut stream = stream;
                    let mut request = [0u8; 2048];
                    let _ = stream.read(&mut request);
                    let body = "<html><body><p>top</p><script>console.error('SECRET-TEXT');</script></body></html>";
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                });
            }
        });
        let dir = std::env::temp_dir().join(format!("parity-capture-string-error-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let (json, log) = (dir.join("report.json"), dir.join("events.jsonl"));
        let url = format!("http://127.0.0.1:{port}/");
        let args = Args::try_parse_from([
            "parity-capture",
            "--url",
            url.as_str(),
            "--event-log",
            log.to_str().unwrap(),
            "--report",
            json.to_str().unwrap(),
        ])
        .unwrap();
        let result = run_capture(&args);
        assert_eq!(result.status, "ok", "{:?}", result.error);
        let written = fs::read_to_string(&json).unwrap();
        assert!(!written.contains("SECRET-TEXT"), "{written}");
        let report: serde_json::Value = serde_json::from_str(&written).unwrap();
        let lines = causes(&report);
        assert!(
            lines.iter().any(|l| l.starts_with("R5 the page's own script: logged string in inline#1:1:9;")),
            "{lines:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    // CH3.A1 (chrome board): the board drives HiWave's own chrome pages
    // through the same action list in RustKit and in Chromium. Two states
    // need actions the list did not have: "hover on the second tab" (a
    // pointer move with no press) and "focus mode" (Ctrl+Shift+F — a key
    // with modifiers). Playwright has `page.hover` and `Control+Shift+F`;
    // these tests pin the RustKit side.
    fn board_view(html: &str) -> (rustkit_engine::Engine, rustkit_engine::EngineViewId) {
        let mut engine = EngineBuilder::new().javascript_enabled(true).build().expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
            .expect("view");
        engine.load_html(view, html).expect("load_html");
        (engine, view)
    }

    #[test]
    fn a_hover_action_moves_the_pointer_over_the_selector_without_pressing() {
        let (mut engine, view) = board_view(
            "<html><head><style>#t{width:100px;height:40px}</style></head><body>\
             <div id=\"t\"></div><script>\
             document.addEventListener('mousedown', function(){ document.body.setAttribute('data-pressed','1'); });\
             document.addEventListener('click', function(){ document.body.setAttribute('data-pressed','1'); });\
             </script></body></html>",
        );
        let actions = parse_actions(r##"[{"type":"hover","selector":"#t"}]"##).expect("actions parse");
        let mut result = CaptureResult::new(&parsed(&[]));
        execute_actions(&mut engine, view, &actions, None, &mut result);
        let res = &result.action_results.as_ref().expect("action results")[0];
        assert_eq!(res.status, "ok", "hover is an action: {res:?}");
        assert!(
            res.method.as_deref().unwrap_or("").starts_with("engine_point("),
            "hover moves the engine pointer to the element's centre: {res:?}"
        );
        let pressed = engine
            .execute_script(view, "String(document.body.getAttribute('data-pressed'))")
            .unwrap_or_default();
        assert!(!pressed.contains('1'), "a hover never presses: {pressed}");
    }

    #[test]
    fn a_key_action_carries_its_modifiers_into_the_keyboard_event() {
        let (mut engine, view) = board_view(
            "<html><body><script>\
             document.addEventListener('keydown', function(e){\
               document.body.setAttribute('data-k', (e.ctrlKey?'c':'') + (e.shiftKey?'s':'') + (e.metaKey?'m':'') + (e.altKey?'a':'') + e.key);\
             });</script></body></html>",
        );
        let actions = parse_actions(
            r#"[{"type":"key","key":"F","modifiers":["Control","Shift"]}]"#,
        )
        .expect("actions parse");
        let mut result = CaptureResult::new(&parsed(&[]));
        execute_actions(&mut engine, view, &actions, None, &mut result);
        let seen = engine
            .execute_script(view, "String(document.body.getAttribute('data-k'))")
            .unwrap_or_default();
        assert!(seen.contains("csF"), "the page's keydown listener saw Ctrl+Shift+F: {seen}");
    }

    // The engine's `handle_text_key` fires the DOM keydown itself (fire_key),
    // and the action then dispatched a synthetic one on top: a page listener
    // heard every key twice, and a toggle (the chrome's Ctrl+Shift+F focus
    // mode) switched on and straight off again (chrome board, 2026-11-05).
    #[test]
    fn a_key_action_reaches_the_page_listener_once() {
        let (mut engine, view) = board_view(
            "<html><body><script>\
             var n = 0;\
             document.addEventListener('keydown', function(e){ n += 1; document.body.setAttribute('data-n', String(n)); });\
             </script></body></html>",
        );
        let actions = parse_actions(r#"[{"type":"key","key":"F3"}]"#).expect("actions parse");
        let mut result = CaptureResult::new(&parsed(&[]));
        execute_actions(&mut engine, view, &actions, None, &mut result);
        let seen = engine
            .execute_script(view, "String(document.body.getAttribute('data-n'))")
            .unwrap_or_default();
        assert!(seen.contains('1') && !seen.contains('2'), "one keydown per key action: {seen}");
    }

    // `key` with `text` wrote the string onto the control's value from
    // script: no keydown, no selection replaced. Playwright types it. The
    // chrome's address bar selects all on focus, so Chromium showed the typed
    // URL and RustKit the old one with the typed URL appended (chrome board,
    // 2026-11-05). Typed text goes through the engine's keyboard, a key at a
    // time, as a user's does.
    #[test]
    fn typed_text_goes_through_the_engines_keyboard_a_key_at_a_time() {
        let (mut engine, view) = board_view(
            "<html><body><input id=\"i\" value=\"abc\"><script>\
             var keys = [];\
             document.addEventListener('keydown', function(e){ keys.push(e.key); document.body.setAttribute('data-keys', keys.join('')); });\
             </script></body></html>",
        );
        let actions = parse_actions(r##"[{"type":"key","selector":"#i","text":"xy"}]"##).expect("actions parse");
        let mut result = CaptureResult::new(&parsed(&[]));
        execute_actions(&mut engine, view, &actions, None, &mut result);
        let keys = engine
            .execute_script(view, "String(document.body.getAttribute('data-keys'))")
            .unwrap_or_default();
        assert!(keys.contains("xy"), "the page heard a keydown per typed character: {keys}");
        let value = engine
            .execute_script(view, "String(document.getElementById('i').value)")
            .unwrap_or_default();
        assert!(value.contains("abcxy"), "the text landed in the focused control: {value}");
    }

    // #898 R1 (Prometheus): `KEY_SEEN_READ_JS` answers 'unseen' and the check
    // was `contains("seen")` — true either way, so the synthetic-key fallback
    // was dead and every key was labelled engine_key.
    #[test]
    fn a_key_the_page_did_not_hear_reads_as_not_seen() {
        assert!(!key_was_seen("String(\"unseen\")"), "'unseen' is not seen");
        assert!(!key_was_seen("String(\"key-silent\")"), "the silent answer is not seen");
        assert!(key_was_seen("String(\"key-heard\")"), "the heard answer is seen");
    }

    // The one-shot listener armed for a key the page never heard must not
    // stay armed: the next key would read as seen before it was pressed.
    #[test]
    fn an_unheard_key_leaves_no_armed_listener_behind() {
        let (mut engine, view) = board_view("<html><body></body></html>");
        let _ = engine.execute_script(view, KEY_SEEN_ARM_JS);
        let read = engine.execute_script(view, KEY_SEEN_READ_JS).unwrap_or_default();
        assert!(!key_was_seen(&read), "nothing pressed: {read}");
        let later = engine
            .execute_script(
                view,
                "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'x', bubbles: true })); \
                 String(window.__parityCaptureKeySeen)",
            )
            .unwrap_or_default();
        assert!(later.contains("false"), "the stale listener was cleared: {later}");
    }

    // Meta is Ctrl's equivalent on macOS and the chrome's shortcuts take either
    // (`e.ctrlKey || e.metaKey`); the engine's keyboard has no meta flag, so a
    // Meta chord goes through as Ctrl.
    #[test]
    fn a_meta_chord_reaches_the_page_as_a_control_chord() {
        let (mut engine, view) = board_view(
            "<html><body><script>\
             document.addEventListener('keydown', function(e){\
               document.body.setAttribute('data-k', (e.ctrlKey||e.metaKey?'c':'') + (e.shiftKey?'s':'') + e.key);\
             });</script></body></html>",
        );
        let actions = parse_actions(r#"[{"type":"key","key":"F","modifiers":["Meta","Shift"]}]"#)
            .expect("actions parse");
        let mut result = CaptureResult::new(&parsed(&[]));
        execute_actions(&mut engine, view, &actions, None, &mut result);
        let seen = engine
            .execute_script(view, "String(document.body.getAttribute('data-k'))")
            .unwrap_or_default();
        assert!(seen.contains("csF"), "Meta+Shift+F reached the page as a modifier chord: {seen}");
    }

    // A selector list splits on top-level commas only: `:is(a, b)` is one
    // selector, not two broken ones.
    #[test]
    fn a_selector_list_splits_on_top_level_commas_only() {
        assert_eq!(split_selector_list("#a, .b"), vec!["#a", ".b"]);
        assert_eq!(
            split_selector_list(":is(#a, .b) > span, [title=\"x, y\"]"),
            vec![":is(#a, .b) > span", "[title=\"x, y\"]"]
        );
    }
}
