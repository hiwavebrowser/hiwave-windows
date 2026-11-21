//! FRAME-STAGES: what each live turn spent, by stage, for a stress run's
//! stage split (docs/diagnostics/2026-11-17/motionmark_stage_split.md).
//!
//! The W-gate's hold breakdown (`wgate`) already times every stage, but
//! per hold and only for holds of 50 ms or more. These are the same
//! figures kept as per-thread running totals that are never reset, so a
//! caller (parity-capture's `--frame-stages`) reads them before and after
//! a turn and logs the difference. Nothing here reads a clock of its own:
//! the stage timers and the script bucket charge it as they charge the
//! hold.
//!
//! Script time includes the canvas raster the page's draw calls made
//! (tiny-skia, inside the host calls); `canvas_raster_ms` is that part.
//! SCRIPT-SPLIT: `host_ms` is the part of script time spent inside host
//! functions (DOM, CSSOM, canvas and the other natives, `rustkit_js`'s
//! host call timing), the canvas raster included; script less host is the
//! engine's JS execution (the bindings' own JS shims included).
//! A relayout stage that ran inside a script turn (a forced layout) is
//! the stage's time and is taken out of the script's, as the hold does.

use std::cell::Cell;

/// Running totals on one thread (see the module docs).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStageTotals {
    /// Script turns: timers, animation frame callbacks, responses, with
    /// their microtasks, less the relayout stages run inside them.
    pub script_ms: f64,
    /// Of `script_ms`: the time inside host functions (outermost calls),
    /// less any relayout stage run inside one; and how many host calls.
    pub host_ms: f64,
    pub host_calls: u64,
    /// Of `host_ms`: the canvas draws rasterized into their bitmaps.
    pub canvas_raster_ms: f64,
    pub canvas_draws: u64,
    /// The box tree from the DOM, styles included (RustKit computes style
    /// and builds boxes in one walk; Chrome's `UpdateLayoutTree`).
    pub build_ms: f64,
    pub layout_ms: f64,
    pub display_list_ms: f64,
    /// The geometry and computed-style maps script reads.
    pub snapshot_ms: f64,
    /// Inline SVG's pre-pass before the build.
    pub svg_ms: f64,
    /// `Engine::render`: the commit and the presenter's draw. Its
    /// breakdown follows (not more time).
    pub render_ms: f64,
    pub acquire_ms: f64,
    pub execute_ms: f64,
    pub present_ms: f64,
    pub renders: u64,
}

impl FrameStageTotals {
    /// What was added since `before` (an earlier read on the same thread).
    pub fn since(&self, before: &Self) -> Self {
        Self {
            script_ms: self.script_ms - before.script_ms,
            host_ms: self.host_ms - before.host_ms,
            host_calls: self.host_calls - before.host_calls,
            canvas_raster_ms: self.canvas_raster_ms - before.canvas_raster_ms,
            canvas_draws: self.canvas_draws - before.canvas_draws,
            build_ms: self.build_ms - before.build_ms,
            layout_ms: self.layout_ms - before.layout_ms,
            display_list_ms: self.display_list_ms - before.display_list_ms,
            snapshot_ms: self.snapshot_ms - before.snapshot_ms,
            svg_ms: self.svg_ms - before.svg_ms,
            render_ms: self.render_ms - before.render_ms,
            acquire_ms: self.acquire_ms - before.acquire_ms,
            execute_ms: self.execute_ms - before.execute_ms,
            present_ms: self.present_ms - before.present_ms,
            renders: self.renders - before.renders,
        }
    }

    /// The time the stages claim of a turn: script and the relayout
    /// stages (render's breakdown is inside render).
    fn claimed_ms(&self) -> f64 {
        self.script_ms
            + self.build_ms
            + self.layout_ms
            + self.display_list_ms
            + self.snapshot_ms
            + self.svg_ms
            + self.render_ms
    }
}

thread_local! {
    static TOTALS: Cell<FrameStageTotals> = const {
        Cell::new(FrameStageTotals {
            script_ms: 0.0,
            host_ms: 0.0,
            host_calls: 0,
            canvas_raster_ms: 0.0,
            canvas_draws: 0,
            build_ms: 0.0,
            layout_ms: 0.0,
            display_list_ms: 0.0,
            snapshot_ms: 0.0,
            svg_ms: 0.0,
            render_ms: 0.0,
            acquire_ms: 0.0,
            execute_ms: 0.0,
            present_ms: 0.0,
            renders: 0,
        })
    };
}

fn update(f: impl FnOnce(&mut FrameStageTotals)) {
    TOTALS.with(|t| {
        let mut v = t.get();
        f(&mut v);
        t.set(v);
    });
}

/// `ms` more of the script bucket (negative: a relayout stage inside it).
pub(crate) fn add_script(ms: f64) {
    update(|t| t.script_ms += ms);
}

/// `ms` more of a W-gate stage.
pub(crate) fn add_stage(stage: crate::wgate::HoldStage, ms: f64) {
    use crate::wgate::HoldStage;
    update(|t| match stage {
        HoldStage::Build => t.build_ms += ms,
        HoldStage::Layout => t.layout_ms += ms,
        HoldStage::DisplayList => t.display_list_ms += ms,
        HoldStage::Snapshot => t.snapshot_ms += ms,
        HoldStage::Render => {
            t.render_ms += ms;
            t.renders += 1;
        }
        HoldStage::Acquire => t.acquire_ms += ms,
        HoldStage::Execute => t.execute_ms += ms,
        HoldStage::Present => t.present_ms += ms,
    });
}

/// `ms` more of the inline SVG pre-pass (not a W-gate stage).
pub(crate) fn add_svg(ms: f64) {
    update(|t| t.svg_ms += ms);
}

/// Turns on (`on`) or off this thread's timing that only the stage line
/// needs: the canvas raster's per-draw clock and draw count, and the host
/// call clock and count (and its per-function rows,
/// `rustkit_js::host_call_rows`). Off by default, so a page nobody is
/// measuring pays one thread-local read per draw or host call and no
/// clock; `host_*` and `canvas_*` stay 0 while it is off. The stage timers
/// and the script bucket are the W-gate's own and are kept either way.
pub fn set_frame_stage_timing(on: bool) {
    rustkit_bindings::canvas::set_raster_timing(on);
    rustkit_js::set_host_call_timing(on);
}

/// This thread's totals so far, the canvas raster's included.
pub fn frame_stage_totals() -> FrameStageTotals {
    let mut totals = TOTALS.with(|t| t.get());
    let raster = rustkit_bindings::canvas::raster_totals();
    let host = rustkit_js::host_call_totals();
    totals.host_ms = host.ms;
    totals.host_calls = host.calls;
    totals.canvas_raster_ms = raster.ms;
    totals.canvas_draws = raster.draws;
    totals
}

/// The fields of [`frame_stage_line`], in order.
pub const FRAME_STAGE_FIELDS: &[&str] = &[
    "turn",
    "t_ms",
    "busy_ms",
    "wait_ms",
    "script_ms",
    "host_ms",
    "host_calls",
    "canvas_raster_ms",
    "canvas_draws",
    "build_ms",
    "layout_ms",
    "display_list_ms",
    "snapshot_ms",
    "svg_ms",
    "render_ms",
    "acquire_ms",
    "execute_ms",
    "present_ms",
    "renders",
    "other_ms",
];

/// One turn's line: turn `turn`, begun `t_ms` after the live loop began,
/// busy for `busy_ms`, after a wait of `wait_ms` since the turn before;
/// `d` is what the totals moved by in it. `other_ms` is the busy time no
/// stage claims (the network, the frame tree's bookkeeping), at least 0.
/// Numbers and names only: no page content.
pub fn frame_stage_line(turn: u64, t_ms: f64, busy_ms: f64, wait_ms: f64, d: &FrameStageTotals) -> String {
    format!(
        "Frame stages turn={turn} t_ms={t_ms:.3} busy_ms={busy_ms:.3} wait_ms={wait_ms:.3} script_ms={:.3} \
         host_ms={:.3} host_calls={} canvas_raster_ms={:.3} canvas_draws={} build_ms={:.3} layout_ms={:.3} display_list_ms={:.3} \
         snapshot_ms={:.3} svg_ms={:.3} render_ms={:.3} acquire_ms={:.3} execute_ms={:.3} present_ms={:.3} \
         renders={} other_ms={:.3}",
        d.script_ms,
        d.host_ms,
        d.host_calls,
        d.canvas_raster_ms,
        d.canvas_draws,
        d.build_ms,
        d.layout_ms,
        d.display_list_ms,
        d.snapshot_ms,
        d.svg_ms,
        d.render_ms,
        d.acquire_ms,
        d.execute_ms,
        d.present_ms,
        d.renders,
        (busy_ms - d.claimed_ms()).max(0.0),
    )
}

/// The thread's running host call rows as one line (SCRIPT-SPLIT):
/// `Host calls turn=N rows=name/op:calls:ms,...`, the most time first.
/// Names are host functions and the operations the embedder's own script
/// names (`rustkit_js::host_call_rows`): no page content.
pub fn host_call_rows_line(turn: u64) -> String {
    let rows: Vec<String> = rustkit_js::host_call_rows()
        .iter()
        .filter(|r| r.calls > 0)
        .map(|r| format!("{}/{}:{}:{:.3}", r.name, r.op, r.calls, r.ms))
        .collect();
    format!("Host calls turn={turn} rows={}", rows.join(","))
}
