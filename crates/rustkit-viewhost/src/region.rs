//! A view's *region* (CH3.A3): the rects, in view coordinates with a
//! top-left origin, where the view paints and takes the pointer. Outside
//! them the view is absent: not composited (a layer mask) and not hit
//! (`hitTest:` returns nil, so the view below gets the pointer).
//!
//! The RustKit chrome is the whole window, stacked over the page. Its
//! region is the window less the page's column (the *hole*), plus the
//! boxes of the chrome's popups that are showing (*overlays*). See
//! `trench/chrome/A3-composition.md`.

/// A rect in view coordinates, top-left origin, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RegionRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RegionRect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self { x, y, width, height }
    }

    fn is_empty(&self) -> bool {
        self.width <= 0.0 || self.height <= 0.0
    }

    /// Half-open: a point on the right or bottom edge is outside.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }

    fn intersect(&self, other: &RegionRect) -> RegionRect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = (self.x + self.width).min(other.x + other.width);
        let y1 = (self.y + self.height).min(other.y + other.height);
        RegionRect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
    }
}

/// The region of a `width` x `height` view that is absent over `hole` and
/// present over each of `overlays`: the view less the hole (at most four
/// rects: above, below, left and right of it) and the overlays, each
/// clipped to the view. Empty rects are left out.
pub fn compose_region(
    width: f64,
    height: f64,
    hole: Option<RegionRect>,
    overlays: &[RegionRect],
) -> Vec<RegionRect> {
    let view = RegionRect::new(0.0, 0.0, width, height);
    let mut region = Vec::new();
    match hole.map(|h| h.intersect(&view)).filter(|h| !h.is_empty()) {
        None => region.push(view),
        Some(h) => {
            let (right, bottom) = (h.x + h.width, h.y + h.height);
            // Full-width bands above and below, then the sides of the
            // hole's own rows.
            region.push(RegionRect::new(0.0, 0.0, width, h.y));
            region.push(RegionRect::new(0.0, bottom, width, height - bottom));
            region.push(RegionRect::new(0.0, h.y, h.x, h.height));
            region.push(RegionRect::new(right, h.y, width - right, h.height));
        }
    }
    region.extend(overlays.iter().map(|o| o.intersect(&view)));
    region.retain(|r| !r.is_empty());
    region
}

/// Whether the point is in the region.
pub fn region_contains(region: &[RegionRect], x: f64, y: f64) -> bool {
    region.iter().any(|r| r.contains(x, y))
}


/// The region of a view whose overlays come from its own frames
/// (CH3.A3b, the RustKit chrome): the layout gives the view's size and the
/// hole, and each frame, as it is presented, gives the boxes it paints over
/// the hole. The view applies what [`present`](Self::present) returns in
/// the same call that presents the frame, so the region (the layer mask
/// and `hitTest:`) and the pixels change on the same turn of the window
/// thread: no message from the document, no frame shown under the previous
/// frame's region.
#[derive(Debug, Default)]
pub struct FrameRegion {
    size: (f64, f64),
    hole: Option<RegionRect>,
    overlays: Vec<RegionRect>,
    /// The frame the region was last derived from, or `None` when the next
    /// present must derive again (a new layout).
    applied: Option<u64>,
}

impl FrameRegion {
    /// The view was laid out at `width` x `height` with `hole` left to the
    /// views below. The frame on screen is derived again on its next
    /// present, against the new hole.
    pub fn lay_out(&mut self, width: f64, height: f64, hole: Option<RegionRect>) {
        self.size = (width, height);
        self.hole = hole;
        self.applied = None;
    }

    /// The hole of the last layout.
    pub fn hole(&self) -> Option<RegionRect> {
        self.hole
    }

    /// Frame `frame` is being presented. For a frame not yet applied,
    /// `overlays(hole)` gives the boxes it paints over the hole, and the
    /// region to apply now is returned; `None` when the region stands (the
    /// same frame, or no layout yet). A `modal` frame (an open drop-down
    /// that takes every press) is hit over the whole view; its overlays
    /// are still derived and reported.
    pub fn present(
        &mut self,
        frame: u64,
        modal: bool,
        overlays: impl FnOnce(RegionRect) -> Vec<RegionRect>,
    ) -> Option<Vec<RegionRect>> {
        let hole = self.hole?;
        if self.applied == Some(frame) {
            return None;
        }
        self.overlays = overlays(hole);
        self.applied = Some(frame);
        if modal {
            // A modal popup (an open `<select>` drop-down, CH2.E6b) takes
            // every press: the view is hit everywhere until it closes.
            return Some(compose_region(self.size.0, self.size.1, None, &[]));
        }
        Some(compose_region(self.size.0, self.size.1, Some(hole), &self.overlays))
    }

    /// The boxes the applied frame paints over the hole.
    pub fn overlays(&self) -> &[RegionRect] {
        &self.overlays
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: f64, y: f64, w: f64, h: f64) -> RegionRect {
        RegionRect::new(x, y, w, h)
    }

    // The chrome at 1280x800: toolbar 104 high, sidebar 220 wide, no right
    // sidebar. The page column is the hole.
    const HOLE: RegionRect = RegionRect { x: 220.0, y: 104.0, width: 1060.0, height: 696.0 };

    #[test]
    fn the_toolbar_and_sidebar_are_in_the_region_and_the_page_column_is_not() {
        let region = compose_region(1280.0, 800.0, Some(HOLE), &[]);
        assert!(region_contains(&region, 600.0, 50.0), "toolbar");
        assert!(region_contains(&region, 100.0, 500.0), "sidebar");
        assert!(region_contains(&region, 0.0, 0.0), "top-left corner");
        assert!(!region_contains(&region, 600.0, 400.0), "page");
        assert!(!region_contains(&region, 220.0, 104.0), "the hole's top-left point");
        assert!(region_contains(&region, 219.9, 104.0), "just left of the hole");
        assert!(region_contains(&region, 220.0, 103.9), "just above the hole");
        assert!(!region_contains(&region, 1279.0, 799.0), "bottom-right corner is page");
    }

    #[test]
    fn the_view_less_a_hole_is_at_most_four_rects_and_none_is_empty() {
        let region = compose_region(1280.0, 800.0, Some(HOLE), &[]);
        assert!(region.len() <= 4, "{region:?}");
        assert!(region.iter().all(|r| r.width > 0.0 && r.height > 0.0), "{region:?}");
        // The hole touches the right and bottom edges: above and left only.
        assert_eq!(region.len(), 2, "{region:?}");
    }

    #[test]
    fn a_hole_inside_the_view_leaves_a_frame_around_it() {
        let hole = r(220.0, 104.0, 800.0, 600.0);
        let region = compose_region(1280.0, 800.0, Some(hole), &[]);
        assert_eq!(region.len(), 4, "{region:?}");
        assert!(region_contains(&region, 1100.0, 400.0), "right sidebar");
        assert!(region_contains(&region, 600.0, 750.0), "below the hole");
        assert!(!region_contains(&region, 600.0, 400.0));
    }

    #[test]
    fn an_open_popup_over_the_page_is_in_the_region_and_the_page_beside_it_is_not() {
        // The downloads panel: 320 wide, right-aligned, under the toolbar.
        let panel = r(942.0, 98.0, 320.0, 240.0);
        let region = compose_region(1280.0, 800.0, Some(HOLE), &[panel]);
        assert!(region_contains(&region, 1000.0, 200.0), "inside the panel, over the page");
        assert!(!region_contains(&region, 600.0, 200.0), "page left of the panel");
        assert!(!region_contains(&region, 1000.0, 400.0), "page below the panel");
    }

    #[test]
    fn an_overlay_is_clipped_to_the_view_and_an_empty_one_is_left_out() {
        let region = compose_region(
            1280.0,
            800.0,
            Some(HOLE),
            &[r(1200.0, 700.0, 300.0, 300.0), r(500.0, 500.0, 0.0, 40.0)],
        );
        assert!(region.iter().all(|q| q.x + q.width <= 1280.0 && q.y + q.height <= 800.0), "{region:?}");
        assert!(region_contains(&region, 1250.0, 750.0));
        assert!(!region_contains(&region, 500.0, 520.0));
        assert_eq!(region.len(), 3, "{region:?}");
    }

    #[test]
    fn no_hole_is_the_whole_view() {
        let region = compose_region(1280.0, 800.0, None, &[]);
        assert_eq!(region, vec![r(0.0, 0.0, 1280.0, 800.0)]);
    }

    #[test]
    fn a_hole_that_covers_the_view_leaves_only_the_overlays() {
        let region = compose_region(400.0, 300.0, Some(r(-10.0, -10.0, 500.0, 500.0)), &[r(10.0, 10.0, 20.0, 20.0)]);
        assert_eq!(region, vec![r(10.0, 10.0, 20.0, 20.0)]);
    }

    // CH3.A3b: the region follows the frames as they are presented.

    #[test]
    fn a_presented_frame_brings_its_overlays_into_the_region_in_the_same_call() {
        let mut fr = FrameRegion::default();
        fr.lay_out(1280.0, 800.0, Some(HOLE));
        let menu = r(600.0, 104.0, 194.0, 100.0);
        let region = fr.present(7, false, |hole| {
            assert_eq!(hole, HOLE, "the overlays are derived against the current hole");
            vec![menu]
        });
        let region = region.expect("a new frame changes the region now, not on a later message");
        assert!(region_contains(&region, 650.0, 150.0), "the menu over the page");
        assert!(!region_contains(&region, 650.0, 300.0), "the page below it");
        assert_eq!(fr.overlays(), &[menu]);
    }

    #[test]
    fn the_frame_already_applied_is_not_derived_again() {
        let mut fr = FrameRegion::default();
        fr.lay_out(1280.0, 800.0, Some(HOLE));
        assert!(fr.present(7, false, |_| vec![]).is_some());
        assert!(fr.present(7, false, |_| panic!("derived twice")).is_none());
    }

    #[test]
    fn a_frame_that_closes_the_menu_takes_it_out_of_the_region() {
        let mut fr = FrameRegion::default();
        fr.lay_out(1280.0, 800.0, Some(HOLE));
        fr.present(7, false, |_| vec![r(600.0, 104.0, 194.0, 100.0)]);
        let region = fr.present(8, false, |_| vec![]).expect("frame 8");
        assert!(!region_contains(&region, 650.0, 150.0));
        assert!(fr.overlays().is_empty());
    }

    #[test]
    fn a_new_layout_derives_the_frame_on_screen_again_against_the_new_hole() {
        let mut fr = FrameRegion::default();
        fr.lay_out(1280.0, 800.0, Some(HOLE));
        fr.present(7, false, |_| vec![]);
        let narrower = r(220.0, 104.0, 760.0, 696.0);
        fr.lay_out(1280.0, 800.0, Some(narrower));
        assert_eq!(fr.hole(), Some(narrower));
        let region = fr.present(7, false, |hole| {
            assert_eq!(hole, narrower);
            vec![]
        });
        let region = region.expect("the same frame, a new hole: derived again");
        assert!(region_contains(&region, 1100.0, 400.0), "the right sidebar is the chrome's");
    }

    #[test]
    fn no_region_before_the_first_layout() {
        let mut fr = FrameRegion::default();
        assert!(fr.present(1, false, |_| panic!("no hole to derive against")).is_none());
    }

    // CH2.E6b: an open `<select>` drop-down takes every press, as the
    // platform popup does: a press on the page column closes it and the
    // page does not get it. The view is hit everywhere while it is open.
    #[test]
    fn a_frame_with_a_modal_popup_is_hit_over_the_whole_view() {
        let mut fr = FrameRegion::default();
        fr.lay_out(1280.0, 800.0, Some(HOLE));
        let popup = r(220.0, 104.0, 164.0, 37.0);
        let region = fr.present(7, true, |_| vec![popup]).expect("frame 7");
        assert!(region_contains(&region, 600.0, 400.0), "the page column, away from the popup: {region:?}");
        assert!(region_contains(&region, 1279.0, 799.0), "the far corner: {region:?}");
        assert_eq!(fr.overlays(), &[popup], "the popup's paint over the page is still reported");
        // Closed: the page column is the page's again.
        let region = fr.present(8, false, |_| vec![]).expect("frame 8");
        assert!(!region_contains(&region, 600.0, 400.0), "{region:?}");
    }
}
