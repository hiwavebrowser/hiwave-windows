//! Counters of one `Renderer::execute`: what a frame cost and where.
//!
//! They exist for the scroll-cost measurements (a scrolled frame of a
//! 20,000-command page took 426 ms on the Mac after threads P3, #703) and for
//! tests that pin how often a frame flushes. Counting is a few integer adds
//! per command; the parts that cost more (timing the overlap check, sorting
//! emitted quads into on- and off-screen) only run while
//! `Renderer::set_measure` is on.

use std::sync::atomic::{AtomicU64, Ordering};

/// What the last `Renderer::execute` did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrameStats {
    /// Commands in the list.
    pub commands: u64,
    /// Commands skipped as wholly outside the viewport (see `cull`).
    pub culled: u64,
    /// Solid fills that reached the glyph-overlap check with glyphs batched.
    pub overlap_checks: u64,
    /// Batched glyph quads those checks compared a fill against.
    pub overlap_comparisons: u64,
    /// Time spent in the overlap check, in nanoseconds (measure mode only).
    pub overlap_ns: u64,
    /// Flushes forced mid-list because a fill landed on batched glyphs.
    pub overlap_flushes: u64,
    /// Render passes that drew batched geometry.
    pub batch_flushes: u64,
    /// `queue.submit` calls.
    pub submits: u64,
    /// Quads handed to the GPU (colors, glyphs, images).
    pub quads: u64,
    /// Of those, quads with no pixel inside the viewport (measure mode only).
    pub quads_offscreen: u64,
}

/// `queue.submit` count: submissions happen behind `&self`, so the counter
/// is atomic rather than a plain field.
#[derive(Debug, Default)]
pub(crate) struct SubmitCounter(AtomicU64);

impl SubmitCounter {
    pub(crate) fn bump(&self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn take(&self) -> u64 {
        self.0.swap(0, Ordering::Relaxed)
    }
}

/// Quads in a 4-vertex-per-quad batch whose bounding box misses
/// `[0, w] x [0, h]`.
pub(crate) fn offscreen_quads<V>(verts: &[V], pos: impl Fn(&V) -> [f32; 2], w: f32, h: f32) -> u64 {
    verts
        .chunks_exact(4)
        .filter(|quad| {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for v in *quad {
                let [x, y] = pos(v);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            x1 <= 0.0 || y1 <= 0.0 || x0 >= w || y0 >= h
        })
        .count() as u64
}
