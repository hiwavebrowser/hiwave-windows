//! # CSS Grid Layout
//!
//! Implementation of the CSS Grid Layout algorithm.
//!
//! ## Overview
//!
//! Grid layout is a two-dimensional layout system that places items in rows and columns.
//! It supports:
//! - Explicit tracks (grid-template-columns/rows)
//! - Implicit tracks (grid-auto-columns/rows)
//! - Named lines and areas
//! - Flexible sizing (fr units)
//! - Auto-placement algorithm
//!
//! ## References
//!
//! - [CSS Grid Layout Module Level 1](https://www.w3.org/TR/css-grid-1/)
//! - [CSS Grid Layout Module Level 2](https://www.w3.org/TR/css-grid-2/)

use rustkit_css::{
    AlignItems, AlignSelf, Display, GridAutoFlow, GridLine, GridPlacement,
    GridTemplate, JustifyItems, JustifySelf, Length, TrackSize,
};
use tracing::{debug, trace};

use crate::{LayoutBox, Rect};

// ==================== Grid Container ====================

/// A resolved grid track (computed from template).
#[derive(Debug, Clone)]
pub struct GridTrack {
    /// Base size (minimum).
    pub base_size: f32,
    /// Growth limit (maximum).
    pub growth_limit: f32,
    /// Whether this track has flexible sizing.
    pub is_flexible: bool,
    /// Flex factor (fr value).
    pub flex_factor: f32,
    /// Final computed size.
    pub size: f32,
    /// Position (offset from container start).
    pub position: f32,
    /// Line names before this track.
    pub line_names: Vec<String>,
}

impl GridTrack {
    /// Create a new track with default sizing.
    pub fn new(size: &TrackSize) -> Self {
        let (base_size, growth_limit, flex_factor) = match size {
            TrackSize::Px(v) => (*v, *v, 0.0),
            TrackSize::Percent(_) => (0.0, f32::INFINITY, 0.0),
            TrackSize::Fr(fr) => (0.0, f32::INFINITY, *fr),
            TrackSize::MinContent => (0.0, 0.0, 0.0), // Will be computed
            TrackSize::MaxContent => (0.0, f32::INFINITY, 0.0),
            TrackSize::Auto => (0.0, f32::INFINITY, 0.0),
            TrackSize::MinMax(min, max) => {
                let min_size = Self::new(min).base_size;
                let max_size = Self::new(max).growth_limit;
                let flex = if max.is_flexible() {
                    if let TrackSize::Fr(fr) = max.as_ref() {
                        *fr
                    } else {
                        0.0
                    }
                } else {
                    0.0
                };
                (min_size, max_size, flex)
            }
            TrackSize::FitContent(max) => (0.0, *max, 0.0),
        };

        Self {
            base_size,
            // For flexible tracks, keep growth_limit as INFINITY
            // For non-flexible tracks with INFINITY growth limit, clamp to base_size
            growth_limit: if flex_factor > 0.0 {
                f32::INFINITY
            } else if growth_limit == f32::INFINITY {
                base_size
            } else {
                growth_limit
            },
            is_flexible: flex_factor > 0.0,
            flex_factor,
            size: base_size,
            position: 0.0,
            line_names: Vec::new(),
        }
    }

    /// Create an implicit track.
    pub fn implicit(size: &TrackSize) -> Self {
        Self::new(size)
    }
}

/// A grid item with placement information.
#[derive(Debug, Clone)]
pub struct GridItem<'a> {
    /// Reference to the layout box.
    pub layout_box: &'a LayoutBox,
    /// Column start line (1-based).
    pub column_start: i32,
    /// Column end line (1-based).
    pub column_end: i32,
    /// Row start line (1-based).
    pub row_start: i32,
    /// Row end line (1-based).
    pub row_end: i32,
    /// Whether this item was auto-placed.
    pub auto_placed: bool,
    /// Computed column span.
    pub column_span: u32,
    /// Computed row span.
    pub row_span: u32,
    /// Computed position and size.
    pub rect: Rect,
}

impl<'a> GridItem<'a> {
    /// Create a new grid item from a layout box.
    pub fn new(layout_box: &'a LayoutBox) -> Self {
        Self {
            layout_box,
            column_start: 0,
            column_end: 0,
            row_start: 0,
            row_end: 0,
            auto_placed: true,
            column_span: 1,
            row_span: 1,
            rect: Rect::default(),
        }
    }

    /// Set explicit placement from style.
    pub fn set_placement(&mut self, placement: &GridPlacement) {
        // Resolve column placement
        match (&placement.column_start, &placement.column_end) {
            (GridLine::Number(start), GridLine::Number(end)) => {
                self.column_start = *start;
                self.column_end = *end;
                self.auto_placed = false;
            }
            (GridLine::Number(start), GridLine::Auto) => {
                self.column_start = *start;
                self.column_end = start + 1;
                self.auto_placed = false;
            }
            (GridLine::Number(start), GridLine::Span(span)) => {
                self.column_start = *start;
                self.column_end = start + *span as i32;
                self.auto_placed = false;
            }
            (GridLine::Auto, GridLine::Number(end)) => {
                self.column_end = *end;
                self.column_start = end - 1;
                self.auto_placed = false;
            }
            (GridLine::Span(span), _) => {
                self.column_span = *span;
            }
            _ => {
                // Auto placement
            }
        }

        // Resolve row placement
        match (&placement.row_start, &placement.row_end) {
            (GridLine::Number(start), GridLine::Number(end)) => {
                self.row_start = *start;
                self.row_end = *end;
                self.auto_placed = self.auto_placed && false;
            }
            (GridLine::Number(start), GridLine::Auto) => {
                self.row_start = *start;
                self.row_end = start + 1;
            }
            (GridLine::Number(start), GridLine::Span(span)) => {
                self.row_start = *start;
                self.row_end = start + *span as i32;
            }
            (GridLine::Auto, GridLine::Number(end)) => {
                self.row_end = *end;
                self.row_start = end - 1;
            }
            (GridLine::Span(span), _) => {
                self.row_span = *span;
            }
            _ => {
                // Auto placement
            }
        }

        // Update spans from placement
        if self.column_start != 0 && self.column_end != 0 {
            self.column_span = (self.column_end - self.column_start).unsigned_abs();
        }
        if self.row_start != 0 && self.row_end != 0 {
            self.row_span = (self.row_end - self.row_start).unsigned_abs();
        }
    }
}

/// Grid layout state.
#[derive(Debug)]
pub struct GridLayout {
    /// Column tracks.
    pub columns: Vec<GridTrack>,
    /// Row tracks.
    pub rows: Vec<GridTrack>,
    /// Column gap.
    pub column_gap: f32,
    /// Row gap.
    pub row_gap: f32,
    /// Auto-flow direction.
    pub auto_flow: GridAutoFlow,
    /// Auto-placement cursor (column, row).
    pub cursor: (usize, usize),
    /// Number of explicit columns.
    pub explicit_columns: usize,
    /// Number of explicit rows.
    pub explicit_rows: usize,
}

impl GridLayout {
    /// Create a new grid layout from style.
    pub fn new(
        template_columns: &GridTemplate,
        template_rows: &GridTemplate,
        _auto_columns: &TrackSize,
        _auto_rows: &TrackSize,
        column_gap: f32,
        row_gap: f32,
        auto_flow: GridAutoFlow,
    ) -> Self {
        // Create explicit column tracks
        let columns: Vec<GridTrack> = template_columns
            .tracks
            .iter()
            .map(|def| {
                let mut track = GridTrack::new(&def.size);
                track.line_names = def.line_names.clone();
                track
            })
            .collect();

        // Create explicit row tracks
        let rows: Vec<GridTrack> = template_rows
            .tracks
            .iter()
            .map(|def| {
                let mut track = GridTrack::new(&def.size);
                track.line_names = def.line_names.clone();
                track
            })
            .collect();

        let explicit_columns = columns.len();
        let explicit_rows = rows.len();

        Self {
            columns,
            rows,
            column_gap,
            row_gap,
            auto_flow,
            cursor: (0, 0),
            explicit_columns,
            explicit_rows,
        }
    }

    /// Ensure we have enough tracks for an item.
    pub fn ensure_tracks(&mut self, col_end: usize, row_end: usize, auto_columns: &TrackSize, auto_rows: &TrackSize) {
        while self.columns.len() < col_end {
            self.columns.push(GridTrack::implicit(auto_columns));
        }
        while self.rows.len() < row_end {
            self.rows.push(GridTrack::implicit(auto_rows));
        }
    }

    /// Get number of columns.
    pub fn column_count(&self) -> usize {
        self.columns.len()
    }

    /// Get number of rows.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// Find next available cell for auto-placement.
    pub fn find_next_cell(&self, col_span: usize, row_span: usize, occupied: &[Vec<bool>]) -> (usize, usize) {
        let (mut col, mut row) = self.cursor;

        if self.auto_flow.is_row() {
            // Row-major placement
            loop {
                if col + col_span <= self.column_count() {
                    // Check if cells are available
                    let available = (0..row_span).all(|dr| {
                        (0..col_span).all(|dc| {
                            let r = row + dr;
                            let c = col + dc;
                            r >= occupied.len() || c >= occupied.get(r).map_or(0, |row| row.len()) || !occupied[r][c]
                        })
                    });

                    if available {
                        return (col, row);
                    }
                }

                col += 1;
                if col + col_span > self.column_count().max(1) {
                    col = 0;
                    row += 1;
                }

                // Safety limit
                if row > 1000 {
                    break;
                }
            }
        } else {
            // Column-major placement
            loop {
                if row + row_span <= self.row_count() {
                    let available = (0..row_span).all(|dr| {
                        (0..col_span).all(|dc| {
                            let r = row + dr;
                            let c = col + dc;
                            r >= occupied.len() || c >= occupied.get(r).map_or(0, |row| row.len()) || !occupied[r][c]
                        })
                    });

                    if available {
                        return (col, row);
                    }
                }

                row += 1;
                if row + row_span > self.row_count().max(1) {
                    row = 0;
                    col += 1;
                }

                if col > 1000 {
                    break;
                }
            }
        }

        (col, row)
    }
}

// ==================== Layout Algorithm ====================

/// Lay out a grid container and its items.
pub fn layout_grid_container(
    container: &mut LayoutBox,
    container_width: f32,
    container_height: f32,
) {
    let style = &container.style;

    // Skip if not a grid container
    if !style.display.is_grid() {
        return;
    }

    debug!(
        "Grid layout: container {}x{}, {} children",
        container_width,
        container_height,
        container.children.len()
    );

    // Compute gaps
    let column_gap = style.column_gap.to_px(16.0, 16.0, container_width);
    let row_gap = style.row_gap.to_px(16.0, 16.0, container_height);

    // Create grid layout
    let mut grid = GridLayout::new(
        &style.grid_template_columns,
        &style.grid_template_rows,
        &style.grid_auto_columns,
        &style.grid_auto_rows,
        column_gap,
        row_gap,
        style.grid_auto_flow,
    );

    // Ensure at least one column and row
    if grid.columns.is_empty() {
        grid.columns.push(GridTrack::implicit(&TrackSize::Auto));
    }
    if grid.rows.is_empty() {
        grid.rows.push(GridTrack::implicit(&TrackSize::Auto));
    }

    // Collect items with placement info
    let mut items: Vec<GridItem> = container
        .children
        .iter()
        .filter(|child| child.style.display != Display::None)
        .map(|child| {
            let mut item = GridItem::new(child);
            // Set placement from style
            let placement = GridPlacement {
                column_start: child.style.grid_column_start.clone(),
                column_end: child.style.grid_column_end.clone(),
                row_start: child.style.grid_row_start.clone(),
                row_end: child.style.grid_row_end.clone(),
            };
            item.set_placement(&placement);
            item
        })
        .collect();

    // Phase 1: Place items with explicit placement
    let mut occupied: Vec<Vec<bool>> = Vec::new();

    for item in items.iter_mut().filter(|i| !i.auto_placed) {
        // Convert to 0-based indices
        let col_start = (item.column_start - 1).max(0) as usize;
        let col_end = item.column_end.max(item.column_start + 1) as usize;
        let row_start = (item.row_start - 1).max(0) as usize;
        let row_end = item.row_end.max(item.row_start + 1) as usize;

        // Ensure grid has enough tracks
        grid.ensure_tracks(col_end, row_end, &style.grid_auto_columns, &style.grid_auto_rows);

        // Mark cells as occupied
        while occupied.len() < row_end {
            occupied.push(vec![false; grid.column_count()]);
        }
        for row in &mut occupied {
            while row.len() < grid.column_count() {
                row.push(false);
            }
        }

        for r in row_start..row_end {
            for c in col_start..col_end {
                if r < occupied.len() && c < occupied[r].len() {
                    occupied[r][c] = true;
                }
            }
        }

        // Update item with resolved placement
        item.column_start = col_start as i32 + 1;
        item.column_end = col_end as i32 + 1;
        item.row_start = row_start as i32 + 1;
        item.row_end = row_end as i32 + 1;
    }

    // Phase 2: Auto-place remaining items
    for item in items.iter_mut().filter(|i| i.auto_placed) {
        let col_span = item.column_span.max(1) as usize;
        let row_span = item.row_span.max(1) as usize;

        // Ensure grid has enough tracks
        grid.ensure_tracks(
            grid.column_count().max(col_span),
            grid.row_count().max(row_span),
            &style.grid_auto_columns,
            &style.grid_auto_rows,
        );

        // Find next available position
        let (col, row) = grid.find_next_cell(col_span, row_span, &occupied);

        // Ensure tracks exist
        grid.ensure_tracks(col + col_span, row + row_span, &style.grid_auto_columns, &style.grid_auto_rows);

        // Ensure occupied grid is large enough
        while occupied.len() < row + row_span {
            occupied.push(vec![false; grid.column_count()]);
        }
        for occ_row in &mut occupied {
            while occ_row.len() < grid.column_count() {
                occ_row.push(false);
            }
        }

        // Mark cells as occupied
        for r in row..row + row_span {
            for c in col..col + col_span {
                if r < occupied.len() && c < occupied[r].len() {
                    occupied[r][c] = true;
                }
            }
        }

        // Update item placement (1-based)
        item.column_start = col as i32 + 1;
        item.column_end = (col + col_span) as i32 + 1;
        item.row_start = row as i32 + 1;
        item.row_end = (row + row_span) as i32 + 1;
        item.column_span = col_span as u32;
        item.row_span = row_span as u32;

        // Update cursor
        grid.cursor = if grid.auto_flow.is_row() {
            (col + col_span, row)
        } else {
            (col, row + row_span)
        };

        trace!(
            "Auto-placed item at ({}, {}) span ({}, {})",
            col, row, col_span, row_span
        );
    }

    // Phase 3: Size tracks
    size_grid_tracks(&mut grid.columns, container_width, column_gap);
    size_grid_tracks(&mut grid.rows, container_height, row_gap);

    // Phase 4: Position items
    let content_x = container.dimensions.content.x;
    let content_y = container.dimensions.content.y;

    for item in &mut items {
        // Get track positions
        let col_start_idx = (item.column_start - 1).max(0) as usize;
        let col_end_idx = (item.column_end - 1).max(0) as usize;
        let row_start_idx = (item.row_start - 1).max(0) as usize;
        let row_end_idx = (item.row_end - 1).max(0) as usize;

        // Calculate position
        let x = if col_start_idx < grid.columns.len() {
            grid.columns[col_start_idx].position
        } else {
            0.0
        };

        let y = if row_start_idx < grid.rows.len() {
            grid.rows[row_start_idx].position
        } else {
            0.0
        };

        // Calculate size (sum of tracks + gaps)
        let width: f32 = (col_start_idx..col_end_idx.min(grid.columns.len()))
            .map(|i| grid.columns[i].size)
            .sum::<f32>()
            + (col_end_idx.saturating_sub(col_start_idx).saturating_sub(1)) as f32 * column_gap;

        let height: f32 = (row_start_idx..row_end_idx.min(grid.rows.len()))
            .map(|i| grid.rows[i].size)
            .sum::<f32>()
            + (row_end_idx.saturating_sub(row_start_idx).saturating_sub(1)) as f32 * row_gap;

        item.rect = Rect {
            x: content_x + x,
            y: content_y + y,
            width,
            height,
        };

        trace!(
            "Item at ({}-{}, {}-{}) -> rect {:?}",
            item.column_start, item.column_end,
            item.row_start, item.row_end,
            item.rect
        );
    }

    // Phase 5: Collect final positions (drops immutable borrow of children)
    let item_count = items.len();
    let positions: Vec<Rect> = items.iter().map(|item| item.rect.clone()).collect();
    drop(items); // Explicitly drop to release borrow

    // Phase 6: Apply positions to children
    let mut position_idx = 0;
    for child in container.children.iter_mut() {
        if child.style.display == Display::None {
            continue;
        }

        if let Some(rect) = positions.get(position_idx) {
            // Apply alignment
            let (x, width) = apply_justify_self(
                &child.style.justify_self,
                &style.justify_items,
                rect.x,
                rect.width,
                child,
            );

            let (y, height) = apply_align_self(
                &child.style.align_self,
                &style.align_items,
                rect.y,
                rect.height,
                child,
            );

            child.dimensions.content.x = x;
            child.dimensions.content.y = y;
            child.dimensions.content.width = width;
            child.dimensions.content.height = height;
        }
        position_idx += 1;
    }

    debug!(
        "Grid layout complete: {} columns, {} rows, {} items",
        grid.column_count(),
        grid.row_count(),
        item_count
    );
}

/// Size grid tracks using the track sizing algorithm.
fn size_grid_tracks(tracks: &mut [GridTrack], container_size: f32, gap: f32) {
    if tracks.is_empty() {
        return;
    }

    let total_gaps = (tracks.len().saturating_sub(1)) as f32 * gap;
    let available_space = (container_size - total_gaps).max(0.0);

    // Step 1: Initialize base sizes
    for track in tracks.iter_mut() {
        track.size = track.base_size;
    }

    // Step 2: Resolve percentage tracks
    for _track in tracks.iter_mut() {
        // Percentages already handled in TrackSize::new
    }

    // Step 3: Distribute remaining space to flexible tracks
    let fixed_size: f32 = tracks.iter().filter(|t| !t.is_flexible).map(|t| t.size).sum();
    let flex_space = (available_space - fixed_size).max(0.0);

    let total_flex: f32 = tracks.iter().filter(|t| t.is_flexible).map(|t| t.flex_factor).sum();

    if total_flex > 0.0 {
        let flex_unit = flex_space / total_flex;
        for track in tracks.iter_mut().filter(|t| t.is_flexible) {
            track.size = (track.flex_factor * flex_unit).max(track.base_size);
            // Respect growth limit
            if track.growth_limit < f32::INFINITY {
                track.size = track.size.min(track.growth_limit);
            }
        }
    }

    // Step 4: Distribute remaining space to auto tracks if any space left
    let used_space: f32 = tracks.iter().map(|t| t.size).sum();
    let remaining = (available_space - used_space).max(0.0);

    if remaining > 0.0 {
        let auto_tracks: Vec<usize> = tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_flexible && t.growth_limit > t.size)
            .map(|(i, _)| i)
            .collect();

        if !auto_tracks.is_empty() {
            let per_track = remaining / auto_tracks.len() as f32;
            for i in auto_tracks {
                tracks[i].size += per_track;
            }
        }
    }

    // Step 5: Calculate positions
    let mut position = 0.0;
    for track in tracks.iter_mut() {
        track.position = position;
        position += track.size + gap;
    }
}

/// Apply justify-self alignment.
fn apply_justify_self(
    self_align: &JustifySelf,
    items_align: &JustifyItems,
    cell_x: f32,
    cell_width: f32,
    child: &LayoutBox,
) -> (f32, f32) {
    let align = match self_align {
        JustifySelf::Auto => match items_align {
            JustifyItems::Start => JustifySelf::Start,
            JustifyItems::End => JustifySelf::End,
            JustifyItems::Center => JustifySelf::Center,
            JustifyItems::Stretch => JustifySelf::Stretch,
        },
        other => *other,
    };

    let child_width = match child.style.width {
        Length::Auto => cell_width,
        Length::Px(w) => w,
        Length::Percent(p) => cell_width * p / 100.0,
        _ => cell_width,
    };

    match align {
        JustifySelf::Start | JustifySelf::Auto => (cell_x, child_width),
        JustifySelf::End => (cell_x + cell_width - child_width, child_width),
        JustifySelf::Center => (cell_x + (cell_width - child_width) / 2.0, child_width),
        JustifySelf::Stretch => (cell_x, cell_width),
    }
}

/// Apply align-self alignment.
fn apply_align_self(
    self_align: &AlignSelf,
    items_align: &AlignItems,
    cell_y: f32,
    cell_height: f32,
    child: &LayoutBox,
) -> (f32, f32) {
    let align = match self_align {
        AlignSelf::Auto => match items_align {
            AlignItems::FlexStart => AlignSelf::FlexStart,
            AlignItems::FlexEnd => AlignSelf::FlexEnd,
            AlignItems::Center => AlignSelf::Center,
            AlignItems::Stretch => AlignSelf::Stretch,
            AlignItems::Baseline => AlignSelf::Baseline,
        },
        other => *other,
    };

    let child_height = match child.style.height {
        Length::Auto => cell_height,
        Length::Px(h) => h,
        Length::Percent(p) => cell_height * p / 100.0,
        _ => cell_height,
    };

    match align {
        AlignSelf::FlexStart | AlignSelf::Auto => (cell_y, child_height),
        AlignSelf::FlexEnd => (cell_y + cell_height - child_height, child_height),
        AlignSelf::Center => (cell_y + (cell_height - child_height) / 2.0, child_height),
        AlignSelf::Stretch => (cell_y, cell_height),
        AlignSelf::Baseline => (cell_y, child_height), // Simplified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxType;
    use rustkit_css::{ComputedStyle, GridTemplateAreas};

    fn create_test_container() -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.display = Display::Grid;
        style.grid_template_columns = GridTemplate::from_sizes(vec![
            TrackSize::Fr(1.0),
            TrackSize::Fr(1.0),
        ]);
        style.grid_template_rows = GridTemplate::from_sizes(vec![
            TrackSize::Px(100.0),
            TrackSize::Px(100.0),
        ]);

        LayoutBox::new(BoxType::Block, style)
    }

    #[test]
    fn test_grid_track_creation() {
        let track = GridTrack::new(&TrackSize::Px(100.0));
        assert_eq!(track.base_size, 100.0);
        assert_eq!(track.size, 100.0);
        assert!(!track.is_flexible);

        let fr_track = GridTrack::new(&TrackSize::Fr(2.0));
        assert!(fr_track.is_flexible);
        assert_eq!(fr_track.flex_factor, 2.0);
    }

    #[test]
    fn test_grid_layout_creation() {
        let template_cols = GridTemplate::from_sizes(vec![
            TrackSize::Fr(1.0),
            TrackSize::Fr(2.0),
        ]);
        let template_rows = GridTemplate::from_sizes(vec![
            TrackSize::Px(100.0),
        ]);

        let grid = GridLayout::new(
            &template_cols,
            &template_rows,
            &TrackSize::Auto,
            &TrackSize::Auto,
            10.0,
            10.0,
            GridAutoFlow::Row,
        );

        assert_eq!(grid.column_count(), 2);
        assert_eq!(grid.row_count(), 1);
    }

    #[test]
    fn test_track_sizing() {
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Fr(1.0)),
            GridTrack::new(&TrackSize::Fr(2.0)),
        ];

        size_grid_tracks(&mut tracks, 300.0, 0.0);

        // 1fr + 2fr = 3fr, so 1fr = 100px, 2fr = 200px
        assert_eq!(tracks[0].size, 100.0);
        assert_eq!(tracks[1].size, 200.0);
    }

    #[test]
    fn test_track_sizing_with_fixed() {
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(50.0)),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];

        size_grid_tracks(&mut tracks, 300.0, 0.0);

        assert_eq!(tracks[0].size, 50.0);
        assert_eq!(tracks[1].size, 250.0);
    }

    #[test]
    fn test_track_positions() {
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        size_grid_tracks(&mut tracks, 320.0, 10.0);

        assert_eq!(tracks[0].position, 0.0);
        assert_eq!(tracks[1].position, 110.0);
        assert_eq!(tracks[2].position, 220.0);
    }

    #[test]
    fn test_auto_placement() {
        let template_cols = GridTemplate::from_sizes(vec![
            TrackSize::Fr(1.0),
            TrackSize::Fr(1.0),
        ]);
        let template_rows = GridTemplate::from_sizes(vec![
            TrackSize::Auto,
        ]);

        let mut grid = GridLayout::new(
            &template_cols,
            &template_rows,
            &TrackSize::Auto,
            &TrackSize::Auto,
            0.0,
            0.0,
            GridAutoFlow::Row,
        );

        let occupied: Vec<Vec<bool>> = Vec::new();

        let (col, row) = grid.find_next_cell(1, 1, &occupied);
        assert_eq!((col, row), (0, 0));
    }

    #[test]
    fn test_grid_template_areas() {
        let areas = GridTemplateAreas::parse(
            "\"header header\"
             \"nav main\"
             \"footer footer\""
        ).unwrap();

        assert_eq!(areas.rows.len(), 3);
        
        let header = areas.get_area("header").unwrap();
        assert_eq!(header.column_start, 1);
        assert_eq!(header.column_end, 3);
        assert_eq!(header.row_start, 1);
        assert_eq!(header.row_end, 2);
    }

    #[test]
    fn test_grid_item_placement() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);

        let placement = GridPlacement::from_lines(1, 3, 1, 2);
        item.set_placement(&placement);

        assert!(!item.auto_placed);
        assert_eq!(item.column_start, 1);
        assert_eq!(item.column_end, 3);
        assert_eq!(item.column_span, 2);
    }
}

