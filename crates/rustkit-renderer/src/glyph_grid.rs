//! A uniform grid over the image, glyph and color-glyph quads batched since
//! the last flush, so the fast path's "would this command be drawn under
//! something batched before it?" check (`Renderer::occludes_batched`)
//! looks at the quads near the command instead of every quad batched so
//! far.
//!
//! The answer is a linear scan's, exactly: each quad's bounding box is
//! filed under every cell it touches, a query looks in every cell its rect
//! touches, and the same strict overlap test decides. Two boxes that
//! overlap share an interior point, and that point's cell is in both
//! ranges, so no overlapping quad is missed. A rect too large for the
//! grid to help (the page background) is answered by scanning the boxes
//! in order.

use hashbrown::HashMap;

/// Cell edge in screen px: a few lines of text.
const CELL: f32 = 64.0;
/// A box spanning more cells than this on an axis is not filed in cells.
const MAX_SPAN: i64 = 32;
/// Cells kept, emptied, across a flush: a 4K screen is about 2,000.
const MAX_KEPT_CELLS: usize = 4096;

type Box2 = [f32; 4];

/// The batches a flush draws after the color batch, in draw order:
/// colors, then `Image`, then `Glyph`, then `ColorGlyph`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Batch {
    Image = 0,
    Glyph = 1,
    ColorGlyph = 2,
}

#[derive(Debug, Default)]
pub(crate) struct GlyphGrid {
    /// Vertices of each batch filed so far, by `Batch`.
    filed: [usize; 3],
    /// Boxes filed from each batch.
    count: [usize; 3],
    /// Every filed box and its batch, in filing order.
    boxes: Vec<(Box2, Batch)>,
    /// Tests: answer every query by scanning `boxes` in order.
    pub(crate) scan_only: bool,
    /// Indices into `boxes`, by cell.
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// Boxes too large or not finite enough to file in cells: always tested.
    wide: Vec<u32>,
}

fn strictly_overlap(r: Box2, q: Box2) -> bool {
    r[0] < q[2] && r[2] > q[0] && r[1] < q[3] && r[3] > q[1]
}

/// The cell range a box touches, or `None` when it is too large or not
/// finite.
fn cell_range(b: Box2) -> Option<(i32, i32, i32, i32)> {
    if !b.iter().all(|v| v.is_finite()) {
        return None;
    }
    let cell = |v: f32| (v / CELL).floor() as i64;
    let (cx0, cy0, cx1, cy1) = (cell(b[0]), cell(b[1]), cell(b[2]), cell(b[3]));
    if cx1 - cx0 > MAX_SPAN || cy1 - cy0 > MAX_SPAN {
        return None;
    }
    let fit = |v: i64| i32::try_from(v).ok();
    Some((fit(cx0)?, fit(cy0)?, fit(cx1)?, fit(cy1)?))
}

impl GlyphGrid {
    /// Forget everything: the batches were flushed.
    ///
    /// The cells' vectors are emptied in place so the next batch reuses
    /// them, unless a transform has spread them over more cells than a
    /// screen holds.
    pub(crate) fn clear(&mut self) {
        self.filed = [0, 0, 0];
        self.count = [0, 0, 0];
        self.boxes.clear();
        if self.cells.len() > MAX_KEPT_CELLS {
            self.cells.clear();
        } else {
            self.cells.values_mut().for_each(Vec::clear);
        }
        self.wide.clear();
    }

    /// File the quads of `batch` added since the last call; `verts` is the
    /// whole batch, four vertices per quad, and `pos` reads a vertex's
    /// screen position.
    pub(crate) fn extend<V>(&mut self, batch: Batch, verts: &[V], pos: impl Fn(&V) -> [f32; 2]) {
        let which = batch as usize;
        if verts.len() < self.filed[which] {
            // The batch was cleared without `clear` (defensive): start over.
            self.clear();
        }
        let from = self.filed[which];
        for quad in verts[from..].chunks_exact(4) {
            let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for v in quad {
                let [x, y] = pos(v);
                b[0] = b[0].min(x);
                b[1] = b[1].min(y);
                b[2] = b[2].max(x);
                b[3] = b[3].max(y);
            }
            let id = self.boxes.len() as u32;
            self.boxes.push((b, batch));
            self.count[which] += 1;
            match cell_range(b) {
                Some((cx0, cy0, cx1, cy1)) => {
                    for cy in cy0..=cy1 {
                        for cx in cx0..=cx1 {
                            self.cells.entry((cx, cy)).or_default().push(id);
                        }
                    }
                }
                None => self.wide.push(id),
            }
        }
        self.filed[which] = from + (verts.len() - from) / 4 * 4;
    }

    /// True when a box from `from` or a later batch is filed.
    pub(crate) fn any_from(&self, from: Batch) -> bool {
        self.count[from as usize..].iter().any(|&n| n > 0)
    }

    /// True when `r` strictly overlaps a box filed from `from` or a later
    /// batch; `compared` counts the boxes tested.
    pub(crate) fn overlaps(&self, r: Box2, from: Batch, compared: &mut u64) -> bool {
        if !self.any_from(from) {
            return false;
        }
        if self.scan_only {
            return self.scan(r, from, compared);
        }
        let mut test = |id: &u32| {
            let (q, batch) = self.boxes[*id as usize];
            batch >= from && {
                *compared += 1;
                strictly_overlap(r, q)
            }
        };
        if self.wide.iter().any(&mut test) {
            return true;
        }
        let Some((cx0, cy0, cx1, cy1)) = cell_range(r) else {
            return self.scan(r, from, compared);
        };
        let cells = (cx1 - cx0 + 1) as usize * (cy1 - cy0 + 1) as usize;
        if cells > self.cells.len() {
            // Fewer occupied cells than cells to visit: walk the occupied.
            return self
                .cells
                .iter()
                .filter(|((cx, cy), _)| (cx0..=cx1).contains(cx) && (cy0..=cy1).contains(cy))
                .any(|(_, ids)| ids.iter().any(&mut test));
        }
        (cy0..=cy1).any(|cy| {
            (cx0..=cx1).any(|cx| {
                self.cells
                    .get(&(cx, cy))
                    .is_some_and(|ids| ids.iter().any(&mut test))
            })
        })
    }

    /// Every box in order: the linear scan.
    fn scan(&self, r: Box2, from: Batch, compared: &mut u64) -> bool {
        self.boxes.iter().any(|&(q, batch)| {
            batch >= from && {
                *compared += 1;
                strictly_overlap(r, q)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(x0: f32, y0: f32, x1: f32, y1: f32) -> [[f32; 2]; 4] {
        [[x0, y0], [x1, y0], [x1, y1], [x0, y1]]
    }

    fn grid(quads: &[[[f32; 2]; 4]]) -> GlyphGrid {
        let mut g = GlyphGrid::default();
        let pos: Vec<[f32; 2]> = quads.iter().flatten().copied().collect();
        g.extend(Batch::Glyph, &pos, |p| *p);
        g
    }

    /// The old check: each quad's box by `min`/`max` over its corners
    /// (which pass over a NaN), tested in order.
    fn scan_answer(quads: &[[[f32; 2]; 4]], r: Box2) -> bool {
        quads.iter().any(|q| {
            let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for [x, y] in q {
                b = [b[0].min(*x), b[1].min(*y), b[2].max(*x), b[3].max(*y)];
            }
            strictly_overlap(r, b)
        })
    }

    #[test]
    fn the_grid_answers_exactly_what_the_linear_scan_answers() {
        // Glyph-sized quads over a page, some straddling cell edges, one
        // huge and one not finite.
        let mut quads = Vec::new();
        for row in 0..40 {
            for col in 0..30 {
                let (x, y) = (col as f32 * 31.7 - 5.0, row as f32 * 20.3 - 3.0);
                quads.push(quad(x, y, x + 9.0, y + 14.0));
            }
        }
        quads.push(quad(-5000.0, 3000.0, 9000.0, 3010.0));
        quads.push(quad(f32::NAN, 0.0, 1.0, 1.0));
        let g = grid(&quads);
        let mut compared = 0;
        let mut n = 0;
        for y in (-40..900).step_by(7) {
            for x in (-40..1000).step_by(13) {
                for (w, h) in [
                    (1.0, 1.0),
                    (5.0, 3.0),
                    (64.0, 64.0),
                    (300.0, 18.0),
                    (5000.0, 5000.0),
                ] {
                    let r = [
                        x as f32 + 0.5,
                        y as f32 + 0.25,
                        x as f32 + 0.5 + w,
                        y as f32 + 0.25 + h,
                    ];
                    assert_eq!(
                        g.overlaps(r, Batch::Image, &mut compared),
                        scan_answer(&quads, r),
                        "{r:?}"
                    );
                    n += 1;
                }
            }
        }
        assert!(
            compared < n * quads.len() as u64 / 20,
            "the grid should test few boxes: {compared}"
        );
    }

    #[test]
    fn edges_that_only_touch_do_not_overlap() {
        let g = grid(&[quad(64.0, 64.0, 128.0, 80.0)]);
        let mut c = 0;
        assert!(!g.overlaps([0.0, 0.0, 64.0, 64.0], Batch::Image, &mut c));
        assert!(!g.overlaps([128.0, 64.0, 200.0, 80.0], Batch::Image, &mut c));
        assert!(g.overlaps([127.9, 79.9, 200.0, 90.0], Batch::Image, &mut c));
    }

    #[test]
    fn extend_files_only_new_quads_and_clear_forgets() {
        let mut g = GlyphGrid::default();
        let q1 = quad(0.0, 0.0, 10.0, 10.0);
        let q2 = quad(100.0, 0.0, 110.0, 10.0);
        g.extend(Batch::Glyph, &q1, |p| *p);
        let both: Vec<[f32; 2]> = q1.into_iter().chain(q2).collect();
        g.extend(Batch::Glyph, &both, |p| *p);
        assert_eq!(g.boxes.len(), 2);
        let mut c = 0;
        assert!(g.overlaps([105.0, 5.0, 106.0, 6.0], Batch::Image, &mut c));
        g.clear();
        assert!(!g.overlaps([105.0, 5.0, 106.0, 6.0], Batch::Image, &mut c));
        // The emptied cells are reused, and hold only the new batch.
        assert!(!g.cells.is_empty());
        g.extend(Batch::Glyph, &q1, |p| *p);
        assert!(g.overlaps([5.0, 5.0, 6.0, 6.0], Batch::Image, &mut c));
        assert!(!g.overlaps([105.0, 5.0, 106.0, 6.0], Batch::Image, &mut c));
    }
}
