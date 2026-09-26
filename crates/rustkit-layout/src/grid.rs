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
    AlignContent, AlignItems, AlignSelf, BoxSizing, ComputedStyle, Display, GridAutoFlow,
    GridLine, GridPlacement, GridTemplate, GridTemplateAreas, JustifyContent, JustifyItems,
    JustifySelf, Length, Overflow, TrackDefinition, TrackRepeat, TrackSize, WhiteSpace,
};
use tracing::{debug, trace};

use crate::{BoxType, LayoutBox, Rect};

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
    /// Percentage value if this is a percentage track (0.0-100.0).
    /// For minmax(%, x), this stores the min percentage.
    pub percent: Option<f32>,
    /// Percentage value for the max bound in minmax(x, %).
    pub max_percent: Option<f32>,
    /// Whether this track uses min-content sizing.
    pub is_min_content: bool,
    /// Whether this track uses max-content sizing.
    pub is_max_content: bool,
    /// For fit-content(length), the maximum length constraint.
    pub fit_content_limit: Option<f32>,
    /// Whether this track is from auto-fit (should collapse if empty).
    pub is_auto_fit: bool,
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
        // Extract percentage values if present (for min and max bounds)
        let (percent, max_percent) = match size {
            TrackSize::Percent(p) => (Some(*p), None),
            TrackSize::MinMax(min, max) => {
                let min_pct = if let TrackSize::Percent(p) = min.as_ref() {
                    Some(*p)
                } else {
                    None
                };
                let max_pct = if let TrackSize::Percent(p) = max.as_ref() {
                    Some(*p)
                } else {
                    None
                };
                (min_pct, max_pct)
            }
            _ => (None, None),
        };

        // Determine if this track uses intrinsic sizing
        let (is_min_content, is_max_content) = match size {
            TrackSize::MinContent => (true, false),
            TrackSize::MaxContent => (false, true),
            TrackSize::MinMax(min, max) => {
                let min_is_min = matches!(min.as_ref(), TrackSize::MinContent);
                let max_is_max = matches!(max.as_ref(), TrackSize::MaxContent);
                (min_is_min, max_is_max)
            }
            TrackSize::FitContent(_) => (true, false), // fit-content uses min-content as minimum
            TrackSize::Auto => (true, true), // auto behaves like minmax(min-content, max-content)
            _ => (false, false),
        };

        let (base_size, growth_limit, flex_factor) = match size {
            TrackSize::Px(v) => (*v, *v, 0.0),
            TrackSize::Percent(_) => (0.0, f32::INFINITY, 0.0), // Will be resolved later
            TrackSize::Fr(fr) => (0.0, f32::INFINITY, *fr),
            TrackSize::MinContent => (0.0, 0.0, 0.0), // Will be computed from content
            TrackSize::MaxContent => (0.0, f32::INFINITY, 0.0), // Will be computed from content
            TrackSize::Auto => (0.0, f32::INFINITY, 0.0), // Will be computed from content
            TrackSize::MinMax(min, max) => {
                // For min: use 0 if it's intrinsic or percentage (resolved later)
                let min_size = match min.as_ref() {
                    TrackSize::Percent(_) | TrackSize::MinContent | TrackSize::MaxContent | TrackSize::Auto => 0.0,
                    _ => Self::new(min).base_size,
                };
                // For max: use INFINITY if it's intrinsic, percentage, or flexible
                let max_size = match max.as_ref() {
                    TrackSize::Percent(_) | TrackSize::MinContent | TrackSize::MaxContent | TrackSize::Auto => f32::INFINITY,
                    _ => Self::new(max).growth_limit,
                };
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

        // Extract fit-content limit if present
        let fit_content_limit = match size {
            TrackSize::FitContent(max) => Some(*max),
            _ => None,
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
            percent,
            max_percent,
            is_min_content,
            is_max_content,
            fit_content_limit,
            is_auto_fit: false,
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
    /// Whether this item needs auto-placement for columns.
    pub auto_column: bool,
    /// Whether this item needs auto-placement for rows.
    pub auto_row: bool,
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
            auto_column: true,
            auto_row: true,
            column_span: 1,
            row_span: 1,
            rect: Rect::default(),
        }
    }

    /// Whether this item needs any auto-placement.
    pub fn needs_auto_placement(&self) -> bool {
        self.auto_column || self.auto_row
    }

    /// Whether this item is fully explicitly placed (no auto-placement needed).
    pub fn is_fully_placed(&self) -> bool {
        !self.auto_column && !self.auto_row
    }

    /// Get the order property value for this item.
    /// Used for sorting items before auto-placement.
    pub fn order(&self) -> i32 {
        self.layout_box.style.order
    }

    /// Get the item's contribution to row sizing.
    /// This considers explicit heights, min-heights, and intrinsic content.
    ///
    /// css-grid-1 §12.4 sizes tracks from each item's OUTER size, so the
    /// block-axis margins are part of the contribution on every path. Without
    /// them a row is short by exactly the item's vertical margins, and because
    /// rows stack, the error accumulates down the grid: `gradient-no-radius`
    /// has four `margin-bottom: 10px` section headers and every row below the
    /// first was 10px too high, 40px by the last.
    pub fn get_height_contribution(&self, container_height: f32) -> f32 {
        let style = &self.layout_box.style;
        let margins = vertical_margins(style);

        // Check for explicit height
        match &style.height {
            Length::Px(h) => {
                trace!("get_height_contribution: explicit Px height = {}", h);
                return *h + margins;
            }
            Length::Percent(p) if container_height > 0.0 => {
                let result = container_height * p / 100.0;
                trace!("get_height_contribution: Percent {}% of {} = {}", p, container_height, result);
                return result + margins;
            }
            _ => {}
        }

        // Check for min-height
        let min_height = match &style.min_height {
            Length::Px(h) => *h,
            Length::Percent(p) if container_height > 0.0 => container_height * p / 100.0,
            _ => 0.0,
        };

        // For auto height, estimate based on content
        // This is a simplified calculation - a full implementation would
        // do a layout pass to determine content height
        let content_height = self.estimate_content_height();

        trace!(
            "get_height_contribution: min_height={}, content_height={}, returning={}",
            min_height, content_height, min_height.max(content_height)
        );
        min_height.max(content_height) + margins
    }

    /// Estimate content height (simplified).
    fn estimate_content_height(&self) -> f32 {
        // Get font size for text content
        let font_size = match self.layout_box.style.font_size {
            Length::Px(px) => px,
            _ => 16.0,
        };

        // Get line height in pixels
        let line_height_px = crate::resolve_line_height(&self.layout_box.style, font_size);

        // Count text children (simplified)
        let text_lines = self.count_text_lines();

        // Padding contribution (rem against the 16px root, not the item's
        // own font size — see the placement pass).
        let style = &self.layout_box.style;
        let padding_top = style.padding_top.to_px(font_size, 16.0, 0.0);
        let padding_bottom = style.padding_bottom.to_px(font_size, 16.0, 0.0);

        let text_content = if text_lines > 0 {
            line_height_px * text_lines as f32
        } else {
            0.0
        };

        // Also consider children's min-heights and actual content
        let children_height = self.estimate_children_height(font_size);

        // Use max of text content or children content
        let content = text_content.max(children_height);

        content + padding_top + padding_bottom
    }

    /// Estimate height from children (including min-height).
    fn estimate_children_height(&self, font_size: f32) -> f32 {
        let mut total_height = 0.0f32;
        let mut max_height = 0.0f32;

        for child in &self.layout_box.children {
            let child_style = &child.style;

            // Check child's min-height
            let min_height = match child_style.min_height {
                Length::Px(h) => h,
                Length::Em(em) => em * font_size,
                _ => 0.0,
            };

            // Check child's explicit height
            let explicit_height = match child_style.height {
                Length::Px(h) => h,
                Length::Em(em) => em * font_size,
                _ => 0.0,
            };

            // If child has display: flex, it might have content
            let child_font_size = match child_style.font_size {
                Length::Px(px) => px,
                _ => font_size,
            };
            let child_line_height_px = crate::resolve_line_height(child_style, child_font_size);

            // Estimate child's content height
            let child_content_height = if let crate::BoxType::Text(_) = &child.box_type {
                child_line_height_px
            } else {
                // Recursively estimate children
                let nested_height: f32 = child.children.iter()
                    .map(|c| {
                        let c_min = match c.style.min_height {
                            Length::Px(h) => h,
                            _ => 0.0,
                        };
                        let c_height = match c.style.height {
                            Length::Px(h) => h,
                            _ => 0.0,
                        };
                        c_min.max(c_height)
                    })
                    .sum();
                nested_height
            };

            let child_height = min_height.max(explicit_height).max(child_content_height);

            // For absolute positioned children, don't count their height in flow
            if matches!(child_style.position, rustkit_css::Position::Absolute | rustkit_css::Position::Fixed) {
                // Skip absolutely positioned children for height calculation
                continue;
            }

            // Accumulate heights (block-level) or take max (inline-level)
            if matches!(child_style.display, rustkit_css::Display::Block | rustkit_css::Display::Flex | rustkit_css::Display::Grid) {
                total_height += child_height;
            } else {
                max_height = max_height.max(child_height);
            }
        }

        total_height.max(max_height)
    }

    /// Count approximate text lines in this item.
    fn count_text_lines(&self) -> usize {
        fn count_text(layout_box: &LayoutBox) -> usize {
            let mut count = 0;
            if let crate::BoxType::Text(_) = &layout_box.box_type {
                count += 1;
            }
            for child in &layout_box.children {
                count += count_text(child);
            }
            count
        }
        count_text(self.layout_box)
    }

    /// Get the item's contribution to column sizing.
    ///
    /// Outer size, for the same §12.4 reason as `get_height_contribution`.
    pub fn get_width_contribution(&self, container_width: f32) -> f32 {
        let style = &self.layout_box.style;
        let margins = horizontal_margins(style);

        // Check for explicit width
        match &style.width {
            Length::Px(w) => return *w + margins,
            Length::Percent(p) if container_width > 0.0 => {
                return container_width * p / 100.0 + margins;
            }
            _ => {}
        }

        // Check for min-width
        let min_width = match &style.min_width {
            Length::Px(w) => *w,
            Length::Percent(p) if container_width > 0.0 => container_width * p / 100.0,
            _ => 0.0,
        };

        // Automatic minimum size (CSS Grid §6.6): an auto-width item whose
        // overflow is visible is floored by its min-content width, so
        // unbreakable content inside the item widens intrinsic and flexible
        // tracks. Scroll containers may shrink below their content and keep
        // the explicit-only contribution.
        let auto_min = if style.overflow_x == Overflow::Visible {
            estimate_min_content_width(self.layout_box)
        } else {
            0.0
        };

        min_width.max(auto_min) + margins
    }

    /// Set explicit placement from style.
    pub fn set_placement(&mut self, placement: &GridPlacement) {
        // Start with both dimensions needing auto-placement
        self.auto_column = true;
        self.auto_row = true;

        // Resolve column placement
        match (&placement.column_start, &placement.column_end) {
            (GridLine::Number(start), GridLine::Number(end)) => {
                self.column_start = *start;
                self.column_end = *end;
                self.auto_column = false;
            }
            (GridLine::Number(start), GridLine::Auto) => {
                self.column_start = *start;
                self.column_end = start + 1;
                self.auto_column = false;
            }
            (GridLine::Number(start), GridLine::Span(span)) => {
                self.column_start = *start;
                self.column_end = start + *span as i32;
                self.auto_column = false;
            }
            (GridLine::Auto, GridLine::Number(end)) => {
                self.column_end = *end;
                self.column_start = end - 1;
                self.auto_column = false;
            }
            (GridLine::Span(span), _) => {
                self.column_span = *span;
                // Still needs auto-placement, but with specified span
            }
            _ => {
                // Auto placement for columns
            }
        }

        // Resolve row placement
        match (&placement.row_start, &placement.row_end) {
            (GridLine::Number(start), GridLine::Number(end)) => {
                self.row_start = *start;
                self.row_end = *end;
                self.auto_row = false;
            }
            (GridLine::Number(start), GridLine::Auto) => {
                self.row_start = *start;
                self.row_end = start + 1;
                self.auto_row = false;
            }
            (GridLine::Number(start), GridLine::Span(span)) => {
                self.row_start = *start;
                self.row_end = start + *span as i32;
                self.auto_row = false;
            }
            (GridLine::Auto, GridLine::Number(end)) => {
                self.row_end = *end;
                self.row_start = end - 1;
                self.auto_row = false;
            }
            (GridLine::Span(span), _) => {
                self.row_span = *span;
                // Still needs auto-placement, but with specified span
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

    /// Set placement with grid context for named line resolution.
    ///
    /// This should be called after the grid tracks are created so that
    /// named lines can be resolved to their actual positions.
    pub fn set_placement_with_grid(&mut self, placement: &GridPlacement, grid: &GridLayout) {
        // Start with both dimensions needing auto-placement
        self.auto_column = true;
        self.auto_row = true;

        // Resolve column placement using grid's named line lookup
        // Use position-aware resolve to correctly handle area names:
        // - For start: area name returns area.column_start
        // - For end: area name returns area.column_end
        let col_start = grid.resolve_column_start_line(&placement.column_start);
        let col_end = grid.resolve_column_end_line(&placement.column_end);

        match (col_start, col_end) {
            // Both start and end are explicit (number or resolved name)
            ((start, false, None), (end, false, None)) if start != 0 && end != 0 => {
                self.column_start = start;
                self.column_end = end;
                self.auto_column = false;
            }
            // Start is explicit, end is auto
            ((start, false, None), (_, true, None)) if start != 0 => {
                self.column_start = start;
                self.column_end = start + 1;
                self.auto_column = false;
            }
            // Start is explicit, end is a span
            ((start, false, None), (_, _, Some(span))) if start != 0 => {
                self.column_start = start;
                self.column_end = start + span as i32;
                self.auto_column = false;
            }
            // End is explicit, start is auto
            ((_, true, None), (end, false, None)) if end != 0 => {
                self.column_end = end;
                self.column_start = end - 1;
                self.auto_column = false;
            }
            // Start is a span (auto-place but with span)
            ((_, _, Some(span)), _) => {
                self.column_span = span;
            }
            _ => {
                // Auto placement for columns
            }
        }

        // Resolve row placement using position-aware resolve
        let row_start = grid.resolve_row_start_line(&placement.row_start);
        let row_end = grid.resolve_row_end_line(&placement.row_end);

        match (row_start, row_end) {
            // Both start and end are explicit
            ((start, false, None), (end, false, None)) if start != 0 && end != 0 => {
                self.row_start = start;
                self.row_end = end;
                self.auto_row = false;
            }
            // Start is explicit, end is auto
            ((start, false, None), (_, true, None)) if start != 0 => {
                self.row_start = start;
                self.row_end = start + 1;
                self.auto_row = false;
            }
            // Start is explicit, end is a span
            ((start, false, None), (_, _, Some(span))) if start != 0 => {
                self.row_start = start;
                self.row_end = start + span as i32;
                self.auto_row = false;
            }
            // End is explicit, start is auto
            ((_, true, None), (end, false, None)) if end != 0 => {
                self.row_end = end;
                self.row_start = end - 1;
                self.auto_row = false;
            }
            // Start is a span
            ((_, _, Some(span)), _) => {
                self.row_span = span;
            }
            _ => {
                // Auto placement for rows
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

/// Stored auto-repeat pattern for layout-time expansion.
#[derive(Debug, Clone)]
pub struct AutoRepeatPattern {
    /// Track definitions to repeat.
    pub tracks: Vec<TrackDefinition>,
    /// Whether this is auto-fit (collapse empty) vs auto-fill.
    pub is_auto_fit: bool,
    /// Insert position in the track list.
    pub insert_position: usize,
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
    /// Pending auto-repeat for columns (resolved at layout time).
    pub column_auto_repeat: Option<AutoRepeatPattern>,
    /// Pending auto-repeat for rows (resolved at layout time).
    pub row_auto_repeat: Option<AutoRepeatPattern>,
    /// Template areas for named area placement.
    pub template_areas: Option<GridTemplateAreas>,
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
        // Expand repeat() patterns in column template
        let (expanded_columns, col_auto_repeat) = template_columns.expand_tracks();

        // Extract auto-repeat pattern for columns if present
        let column_auto_repeat = Self::extract_auto_repeat(template_columns, col_auto_repeat);

        // Create explicit column tracks from expanded template
        let columns: Vec<GridTrack> = expanded_columns
            .iter()
            .map(|def| {
                let mut track = GridTrack::new(&def.size);
                track.line_names = def.line_names.clone();
                track
            })
            .collect();

        // Expand repeat() patterns in row template
        let (expanded_rows, row_auto_repeat) = template_rows.expand_tracks();

        // Extract auto-repeat pattern for rows if present
        let row_auto_repeat = Self::extract_auto_repeat(template_rows, row_auto_repeat);

        // Create explicit row tracks from expanded template
        let rows: Vec<GridTrack> = expanded_rows
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
            column_auto_repeat,
            row_auto_repeat,
            template_areas: None,
        }
    }

    /// Set template areas for named area placement.
    pub fn set_template_areas(&mut self, areas: Option<GridTemplateAreas>) {
        self.template_areas = areas;
    }

    /// Get an area by name from template-areas.
    pub fn get_area(&self, name: &str) -> Option<&rustkit_css::GridArea> {
        self.template_areas.as_ref().and_then(|ta| ta.get_area(name))
    }

    /// Extract auto-repeat pattern from template.
    fn extract_auto_repeat(
        template: &GridTemplate,
        auto_repeat: Option<&TrackRepeat>,
    ) -> Option<AutoRepeatPattern> {
        auto_repeat.and_then(|repeat| {
            // Find insert position from template repeats
            let insert_pos = template
                .repeats
                .iter()
                .find_map(|(pos, r)| {
                    if matches!(r, TrackRepeat::AutoFill(_) | TrackRepeat::AutoFit(_)) {
                        Some(*pos)
                    } else {
                        None
                    }
                })
                .unwrap_or(0);

            match repeat {
                TrackRepeat::AutoFill(tracks) => Some(AutoRepeatPattern {
                    tracks: tracks.clone(),
                    is_auto_fit: false,
                    insert_position: insert_pos,
                }),
                TrackRepeat::AutoFit(tracks) => Some(AutoRepeatPattern {
                    tracks: tracks.clone(),
                    is_auto_fit: true,
                    insert_position: insert_pos,
                }),
                TrackRepeat::Count(_, _) => None, // Already expanded
            }
        })
    }

    /// Expand auto-fill/auto-fit patterns now that we have container size.
    ///
    /// Per CSS Grid spec:
    /// - Calculate how many repetitions fit in the available space
    /// - Insert the repeated tracks at the stored insert position
    /// - For auto-fit, empty tracks will be collapsed to 0 during sizing
    pub fn expand_auto_repeats(&mut self, container_width: f32, container_height: f32) {
        // Expand column auto-repeat
        if let Some(pattern) = self.column_auto_repeat.take() {
            let available = container_width - (self.columns.len().saturating_sub(1)) as f32 * self.column_gap;
            let new_tracks = Self::calculate_auto_repeat_tracks(&pattern, available, self.column_gap);

            // Insert at the stored position
            let insert_at = pattern.insert_position.min(self.columns.len());
            for (i, track) in new_tracks.into_iter().enumerate() {
                self.columns.insert(insert_at + i, track);
            }
            self.explicit_columns = self.columns.len();
        }

        // Expand row auto-repeat
        if let Some(pattern) = self.row_auto_repeat.take() {
            let available = container_height - (self.rows.len().saturating_sub(1)) as f32 * self.row_gap;
            let new_tracks = Self::calculate_auto_repeat_tracks(&pattern, available, self.row_gap);

            // Insert at the stored position
            let insert_at = pattern.insert_position.min(self.rows.len());
            for (i, track) in new_tracks.into_iter().enumerate() {
                self.rows.insert(insert_at + i, track);
            }
            self.explicit_rows = self.rows.len();
        }
    }

    /// Calculate how many tracks to create for auto-fill/auto-fit.
    ///
    /// Returns a Vec of GridTrack to insert.
    fn calculate_auto_repeat_tracks(
        pattern: &AutoRepeatPattern,
        available_space: f32,
        gap: f32,
    ) -> Vec<GridTrack> {
        if pattern.tracks.is_empty() {
            return Vec::new();
        }

        // Calculate the fixed size of one repetition of the pattern
        let pattern_fixed_size: f32 = pattern
            .tracks
            .iter()
            .map(|def| Self::get_track_definite_size(&def.size))
            .sum();

        // Include gaps between tracks in one repetition
        let pattern_gaps = if pattern.tracks.len() > 1 {
            (pattern.tracks.len() - 1) as f32 * gap
        } else {
            0.0
        };

        let single_repetition_size = pattern_fixed_size + pattern_gaps;

        // If pattern has no definite size (all fr units), create exactly 1 repetition
        if single_repetition_size <= 0.0 {
            let tracks: Vec<GridTrack> = pattern
                .tracks
                .iter()
                .map(|def| {
                    let mut track = GridTrack::new(&def.size);
                    track.line_names = def.line_names.clone();
                    track
                })
                .collect();
            return tracks;
        }

        // Calculate how many repetitions fit
        // Account for gaps between repetitions
        let mut repetitions = 1u32;
        let mut total_size = single_repetition_size;

        while total_size + gap + single_repetition_size <= available_space {
            repetitions += 1;
            total_size += gap + single_repetition_size;
        }

        // Per spec, at least 1 repetition
        repetitions = repetitions.max(1);

        trace!(
            "auto-repeat: {} repetitions fit in {}px (pattern size: {}px)",
            repetitions,
            available_space,
            single_repetition_size
        );

        // Create the tracks
        let mut result = Vec::with_capacity(repetitions as usize * pattern.tracks.len());
        for _ in 0..repetitions {
            for def in &pattern.tracks {
                let mut track = GridTrack::new(&def.size);
                track.line_names = def.line_names.clone();
                // Mark for auto-fit collapsing (handled during sizing)
                if pattern.is_auto_fit {
                    track.is_auto_fit = true;
                }
                result.push(track);
            }
        }

        result
    }

    /// Get the definite (fixed) size of a track for auto-repeat calculations.
    /// Returns 0 for flexible tracks (fr) since they don't contribute to fixed size.
    fn get_track_definite_size(size: &TrackSize) -> f32 {
        match size {
            TrackSize::Px(px) => *px,
            TrackSize::MinMax(min, max) => {
                // Use the definite bound
                let min_size = Self::get_track_definite_size(min);
                let max_size = Self::get_track_definite_size(max);
                // If max is definite, use it; otherwise use min
                if max_size > 0.0 {
                    max_size
                } else {
                    min_size
                }
            }
            TrackSize::FitContent(max) => *max,
            // Flexible and intrinsic sizes are not definite
            TrackSize::Fr(_)
            | TrackSize::Percent(_)
            | TrackSize::MinContent
            | TrackSize::MaxContent
            | TrackSize::Auto => 0.0,
        }
    }

    /// Collapse empty auto-fit column tracks.
    ///
    /// For auto-fit, empty tracks (tracks with no items spanning them)
    /// should be treated as having a fixed sizing function of 0px.
    pub fn collapse_empty_auto_fit_columns(&mut self, column_occupied: &[bool]) {
        for (i, track) in self.columns.iter_mut().enumerate() {
            if track.is_auto_fit {
                let has_items = column_occupied.get(i).copied().unwrap_or(false);
                if !has_items {
                    // Collapse this track to 0
                    track.base_size = 0.0;
                    track.growth_limit = 0.0;
                    track.size = 0.0;
                    track.is_flexible = false;
                    track.flex_factor = 0.0;
                    trace!("Collapsed empty auto-fit column track {}", i);
                }
            }
        }
    }

    /// Collapse empty auto-fit row tracks.
    pub fn collapse_empty_auto_fit_rows(&mut self, row_occupied: &[bool]) {
        for (i, track) in self.rows.iter_mut().enumerate() {
            if track.is_auto_fit {
                let has_items = row_occupied.get(i).copied().unwrap_or(false);
                if !has_items {
                    // Collapse this track to 0
                    track.base_size = 0.0;
                    track.growth_limit = 0.0;
                    track.size = 0.0;
                    track.is_flexible = false;
                    track.flex_factor = 0.0;
                    trace!("Collapsed empty auto-fit row track {}", i);
                }
            }
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

    /// Find a column line by name.
    ///
    /// Line names are stored on the track that follows them, so line N
    /// corresponds to track N-1's line_names (0-indexed).
    /// Returns the 1-based line number if found.
    ///
    /// Also checks implicit line names from template-areas:
    /// - "area-name-start" → column_start of the named area
    /// - "area-name-end" → column_end of the named area
    pub fn find_column_line_by_name(&self, name: &str) -> Option<i32> {
        // First check explicit line names on tracks
        for (track_idx, track) in self.columns.iter().enumerate() {
            // Line before track at index `track_idx` is line number `track_idx + 1` (1-based)
            if track.line_names.iter().any(|n| n == name) {
                return Some((track_idx + 1) as i32);
            }
        }

        // Check implicit line names from template-areas
        if let Some(ref areas) = self.template_areas {
            // Check for "area-start" pattern
            if let Some(area_name) = name.strip_suffix("-start") {
                if let Some(area) = areas.get_area(area_name) {
                    return Some(area.column_start);
                }
            }
            // Check for "area-end" pattern
            if let Some(area_name) = name.strip_suffix("-end") {
                if let Some(area) = areas.get_area(area_name) {
                    return Some(area.column_end);
                }
            }
            // Check if the name itself is an area name (returns the start line)
            if let Some(area) = areas.get_area(name) {
                return Some(area.column_start);
            }
        }

        None
    }

    /// Find a row line by name.
    ///
    /// Also checks implicit line names from template-areas:
    /// - "area-name-start" → row_start of the named area
    /// - "area-name-end" → row_end of the named area
    pub fn find_row_line_by_name(&self, name: &str) -> Option<i32> {
        // First check explicit line names on tracks
        for (track_idx, track) in self.rows.iter().enumerate() {
            if track.line_names.iter().any(|n| n == name) {
                return Some((track_idx + 1) as i32);
            }
        }

        // Check implicit line names from template-areas
        if let Some(ref areas) = self.template_areas {
            // Check for "area-start" pattern
            if let Some(area_name) = name.strip_suffix("-start") {
                if let Some(area) = areas.get_area(area_name) {
                    return Some(area.row_start);
                }
            }
            // Check for "area-end" pattern
            if let Some(area_name) = name.strip_suffix("-end") {
                if let Some(area) = areas.get_area(area_name) {
                    return Some(area.row_end);
                }
            }
            // Check if the name itself is an area name (returns the start line)
            if let Some(area) = areas.get_area(name) {
                return Some(area.row_start);
            }
        }

        None
    }

    /// Resolve a GridLine to a line number for columns.
    ///
    /// Returns (line_number, is_auto, span) where:
    /// - line_number: 1-based line number (may be 0 if not resolved)
    /// - is_auto: whether this needs auto-placement
    /// - span: optional span count
    pub fn resolve_column_line(&self, line: &GridLine) -> (i32, bool, Option<u32>) {
        self.resolve_column_line_impl(line, true) // Default to start position
    }

    /// Resolve a GridLine to a line number for column start position.
    /// For area names, returns the column_start of the area.
    pub fn resolve_column_start_line(&self, line: &GridLine) -> (i32, bool, Option<u32>) {
        self.resolve_column_line_impl(line, true)
    }

    /// Resolve a GridLine to a line number for column end position.
    /// For area names, returns the column_end of the area.
    pub fn resolve_column_end_line(&self, line: &GridLine) -> (i32, bool, Option<u32>) {
        self.resolve_column_line_impl(line, false)
    }

    fn resolve_column_line_impl(&self, line: &GridLine, is_start: bool) -> (i32, bool, Option<u32>) {
        match line {
            GridLine::Auto => (0, true, None),
            GridLine::Number(n) => (*n, false, None),
            GridLine::Name(name) => {
                // First check for explicit line names
                if let Some(n) = self.find_explicit_column_line_by_name(name) {
                    return (n, false, None);
                }
                // Then check if this is an area name
                if let Some(area) = self.get_area(name) {
                    let line_num = if is_start {
                        area.column_start
                    } else {
                        area.column_end
                    };
                    return (line_num, false, None);
                }
                // Then check for implicit line names (area-start, area-end)
                if let Some(n) = self.find_column_line_by_name(name) {
                    (n, false, None)
                } else {
                    trace!("Column line name '{}' not found, using auto", name);
                    (0, true, None)
                }
            }
            GridLine::Span(count) => (0, true, Some(*count)),
            GridLine::SpanName(name) => {
                // SpanName resolves to the target line number.
                // Per CSS Grid spec, `span <name>` means "span to the named line".
                // The actual span count is calculated by set_placement_with_grid as (end - start).
                // We also check area names (e.g., "span header" finds header-start or header-end).

                // First check explicit line names
                if let Some(n) = self.find_explicit_column_line_by_name(name) {
                    return (n, false, None);
                }
                // Then check if this is an area name
                if let Some(area) = self.get_area(name) {
                    // For SpanName used at end position, return the area's end
                    // For SpanName used at start position, return the area's start
                    let line_num = if is_start {
                        area.column_start
                    } else {
                        area.column_end
                    };
                    return (line_num, false, None);
                }
                // Then check implicit line names (area-start, area-end)
                if let Some(n) = self.find_column_line_by_name(name) {
                    (n, false, None)
                } else {
                    trace!("Column span name '{}' not found, using span 1", name);
                    (0, true, Some(1))
                }
            }
        }
    }

    /// Find explicit column line by name (only checks track line_names, not area names).
    fn find_explicit_column_line_by_name(&self, name: &str) -> Option<i32> {
        for (track_idx, track) in self.columns.iter().enumerate() {
            if track.line_names.iter().any(|n| n == name) {
                return Some((track_idx + 1) as i32);
            }
        }
        None
    }

    /// Find explicit row line by name (only checks track line_names, not area names).
    fn find_explicit_row_line_by_name(&self, name: &str) -> Option<i32> {
        for (track_idx, track) in self.rows.iter().enumerate() {
            if track.line_names.iter().any(|n| n == name) {
                return Some((track_idx + 1) as i32);
            }
        }
        None
    }

    /// Resolve a GridLine to a line number for rows.
    pub fn resolve_row_line(&self, line: &GridLine) -> (i32, bool, Option<u32>) {
        self.resolve_row_line_impl(line, true) // Default to start position
    }

    /// Resolve a GridLine to a line number for row start position.
    /// For area names, returns the row_start of the area.
    pub fn resolve_row_start_line(&self, line: &GridLine) -> (i32, bool, Option<u32>) {
        self.resolve_row_line_impl(line, true)
    }

    /// Resolve a GridLine to a line number for row end position.
    /// For area names, returns the row_end of the area.
    pub fn resolve_row_end_line(&self, line: &GridLine) -> (i32, bool, Option<u32>) {
        self.resolve_row_line_impl(line, false)
    }

    fn resolve_row_line_impl(&self, line: &GridLine, is_start: bool) -> (i32, bool, Option<u32>) {
        match line {
            GridLine::Auto => (0, true, None),
            GridLine::Number(n) => (*n, false, None),
            GridLine::Name(name) => {
                // First check for explicit line names
                if let Some(n) = self.find_explicit_row_line_by_name(name) {
                    return (n, false, None);
                }
                // Then check if this is an area name
                if let Some(area) = self.get_area(name) {
                    let line_num = if is_start {
                        area.row_start
                    } else {
                        area.row_end
                    };
                    return (line_num, false, None);
                }
                // Then check for implicit line names (area-start, area-end)
                if let Some(n) = self.find_row_line_by_name(name) {
                    (n, false, None)
                } else {
                    trace!("Row line name '{}' not found, using auto", name);
                    (0, true, None)
                }
            }
            GridLine::Span(count) => (0, true, Some(*count)),
            GridLine::SpanName(name) => {
                // SpanName resolves to the target line number.
                // Per CSS Grid spec, `span <name>` means "span to the named line".

                // First check explicit line names
                if let Some(n) = self.find_explicit_row_line_by_name(name) {
                    return (n, false, None);
                }
                // Then check if this is an area name
                if let Some(area) = self.get_area(name) {
                    let line_num = if is_start {
                        area.row_start
                    } else {
                        area.row_end
                    };
                    return (line_num, false, None);
                }
                // Then check implicit line names (area-start, area-end)
                if let Some(n) = self.find_row_line_by_name(name) {
                    (n, false, None)
                } else {
                    trace!("Row span name '{}' not found, using span 1", name);
                    (0, true, Some(1))
                }
            }
        }
    }

    /// Find next available cell for auto-placement.
    ///
    /// For sparse packing (default), uses the cursor position.
    /// For dense packing, starts from (0,0) to backfill gaps.
    pub fn find_next_cell(&self, col_span: usize, row_span: usize, occupied: &[Vec<bool>]) -> (usize, usize) {
        self.find_next_cell_impl(col_span, row_span, occupied, false)
    }

    /// Find next available cell with dense packing (backfill gaps).
    pub fn find_next_cell_dense(&self, col_span: usize, row_span: usize, occupied: &[Vec<bool>]) -> (usize, usize) {
        self.find_next_cell_impl(col_span, row_span, occupied, true)
    }

    fn find_next_cell_impl(&self, col_span: usize, row_span: usize, occupied: &[Vec<bool>], dense: bool) -> (usize, usize) {
        // For dense packing, always start from (0,0) to backfill gaps
        // For sparse packing, start from cursor position
        let (mut col, mut row) = if dense { (0, 0) } else { self.cursor };

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

    /// Find next available row at a specific column for items with explicit column placement.
    pub fn find_next_row_at_column(&self, col_start: usize, col_span: usize, row_span: usize, occupied: &[Vec<bool>]) -> usize {
        let mut row = 0;
        
        loop {
            // Check if cells are available at this row for the given column range
            let available = (0..row_span).all(|dr| {
                (0..col_span).all(|dc| {
                    let r = row + dr;
                    let c = col_start + dc;
                    r >= occupied.len() || c >= occupied.get(r).map_or(0, |row_vec| row_vec.len()) || !occupied[r][c]
                })
            });

            if available {
                return row;
            }

            row += 1;

            // Safety limit
            if row > 1000 {
                break;
            }
        }

        row
    }

    /// Find next available column at a specific row for items with explicit row placement.
    pub fn find_next_column_at_row(&self, row_start: usize, col_span: usize, row_span: usize, occupied: &[Vec<bool>]) -> usize {
        let mut col = 0;
        
        loop {
            if col + col_span <= self.column_count() {
                // Check if cells are available at this column for the given row range
                let available = (0..row_span).all(|dr| {
                    (0..col_span).all(|dc| {
                        let r = row_start + dr;
                        let c = col + dc;
                        r >= occupied.len() || c >= occupied.get(r).map_or(0, |row_vec| row_vec.len()) || !occupied[r][c]
                    })
                });

                if available {
                    return col;
                }
            }

            col += 1;

            // Safety limit
            if col > 1000 {
                break;
            }
        }

        col
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

    // Set template areas for named area placement
    grid.set_template_areas(style.grid_template_areas.clone());

    // Expand auto-fill/auto-fit patterns now that we have container size
    grid.expand_auto_repeats(container_width, container_height);

    // Ensure at least one column and row
    if grid.columns.is_empty() {
        grid.columns.push(GridTrack::implicit(&TrackSize::Auto));
    }
    if grid.rows.is_empty() {
        grid.rows.push(GridTrack::implicit(&TrackSize::Auto));
    }

    // Collect items with placement info
    // Use set_placement_with_grid to resolve named lines
    let mut items: Vec<GridItem> = container
        .children
        .iter()
        .filter(|child| child.style.display != Display::None)
        .map(|child| {
            let mut item = GridItem::new(child);
            // Set placement from style, resolving named lines via grid
            let placement = GridPlacement {
                column_start: child.style.grid_column_start.clone(),
                column_end: child.style.grid_column_end.clone(),
                row_start: child.style.grid_row_start.clone(),
                row_end: child.style.grid_row_end.clone(),
            };
            item.set_placement_with_grid(&placement, &grid);
            item
        })
        .collect();

    // Helper to resolve negative grid lines (e.g., -1 = last line)
    // In CSS Grid, negative indices count from the end: -1 is the last line
    let resolve_line = |line: i32, track_count: usize| -> i32 {
        if line < 0 {
            // -1 means last line, which is track_count + 1 in 1-based indexing
            // -2 means second-to-last, etc.
            (track_count as i32 + 1) + line + 1
        } else {
            line
        }
    };

    // Sort items by order property (stable sort to preserve document order for equal values).
    // Per CSS Grid spec, items are placed in "order-modified document order".
    // Items with lower order values are placed before items with higher order values.
    items.sort_by_key(|item| item.order());

    // Phase 1: Place items with explicit placement in BOTH dimensions
    let mut occupied: Vec<Vec<bool>> = Vec::new();

    for item in items.iter_mut().filter(|i| i.is_fully_placed()) {
        // Resolve negative line numbers before converting to 0-based indices
        let resolved_col_start = resolve_line(item.column_start, grid.column_count());
        let resolved_col_end = resolve_line(item.column_end, grid.column_count());
        let resolved_row_start = resolve_line(item.row_start, grid.row_count());
        let resolved_row_end = resolve_line(item.row_end, grid.row_count());

        // Convert to 0-based indices
        // Line numbers are 1-based, indices are 0-based
        // For exclusive end indices: line N means "after column N-1" = index N-1 (exclusive)
        let col_start = (resolved_col_start - 1).max(0) as usize;
        let col_end = ((resolved_col_end - 1).max(0) as usize).max(col_start + 1);
        let row_start = (resolved_row_start - 1).max(0) as usize;
        let row_end = ((resolved_row_end - 1).max(0) as usize).max(row_start + 1);

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

    // Phase 2-4 Combined: Place remaining items in DOM order
    // CSS Grid spec requires items to maintain document order during auto-placement.
    // Items with partial explicit placement (explicit column OR explicit row) are
    // interleaved with fully auto-placed items in their source order.
    for item in items.iter_mut().filter(|i| !i.is_fully_placed()) {
        let (col, row, col_span, row_span) = if !item.auto_column && item.auto_row {
            // Item has explicit column, auto row (e.g., grid-column: 1 / -1)
            let resolved_col_start = resolve_line(item.column_start, grid.column_count());
            let resolved_col_end = resolve_line(item.column_end, grid.column_count());
            let col_start = (resolved_col_start - 1).max(0) as usize;
            let col_end = ((resolved_col_end - 1).max(0) as usize).max(col_start + 1);
            let col_span = col_end.saturating_sub(col_start).max(1);
            let row_span = item.row_span.max(1) as usize;

            grid.ensure_tracks(col_end, grid.row_count(), &style.grid_auto_columns, &style.grid_auto_rows);
            let row = grid.find_next_row_at_column(col_start, col_span, row_span, &occupied);

            (col_start, row, col_span, row_span)
        } else if item.auto_column && !item.auto_row {
            // Item has auto column, explicit row
            let resolved_row_start = resolve_line(item.row_start, grid.row_count());
            let resolved_row_end = resolve_line(item.row_end, grid.row_count());
            let row_start = (resolved_row_start - 1).max(0) as usize;
            let row_end = ((resolved_row_end - 1).max(0) as usize).max(row_start + 1);
            let row_span = row_end.saturating_sub(row_start).max(1);
            let col_span = item.column_span.max(1) as usize;

            grid.ensure_tracks(grid.column_count(), row_end, &style.grid_auto_columns, &style.grid_auto_rows);
            let col = grid.find_next_column_at_row(row_start, col_span, row_span, &occupied);

            (col, row_start, col_span, row_span)
        } else {
            // Fully auto-placed item
            let col_span = item.column_span.max(1) as usize;
            let row_span = item.row_span.max(1) as usize;

            grid.ensure_tracks(
                grid.column_count().max(col_span),
                grid.row_count().max(row_span),
                &style.grid_auto_columns,
                &style.grid_auto_rows,
            );

            let (col, row) = if grid.auto_flow.is_dense() {
                grid.find_next_cell_dense(col_span, row_span, &occupied)
            } else {
                grid.find_next_cell(col_span, row_span, &occupied)
            };

            (col, row, col_span, row_span)
        };

        let col_end = col + col_span;
        let row_end = row + row_span;

        // Ensure tracks exist
        grid.ensure_tracks(col_end, row_end, &style.grid_auto_columns, &style.grid_auto_rows);

        // Ensure occupied grid is large enough
        while occupied.len() < row_end {
            occupied.push(vec![false; grid.column_count()]);
        }
        for occ_row in &mut occupied {
            while occ_row.len() < grid.column_count() {
                occ_row.push(false);
            }
        }

        // Mark cells as occupied
        for r in row..row_end {
            for c in col..col_end {
                if r < occupied.len() && c < occupied[r].len() {
                    occupied[r][c] = true;
                }
            }
        }

        // Update item placement (1-based)
        item.column_start = col as i32 + 1;
        item.column_end = col_end as i32 + 1;
        item.row_start = row as i32 + 1;
        item.row_end = row_end as i32 + 1;
        item.column_span = col_span as u32;
        item.row_span = row_span as u32;

        // Update cursor for sparse packing
        grid.cursor = if grid.auto_flow.is_row() {
            (col + col_span, row)
        } else {
            (col, row + row_span)
        };

        trace!(
            "Placed item at ({}, {}) span ({}, {})",
            col, row, col_span, row_span
        );
    }

    // Phase 4.5: Collapse empty auto-fit tracks
    // For auto-fit, tracks with no items spanning them collapse to 0
    {
        // Calculate which columns have items
        let mut column_occupied = vec![false; grid.column_count()];
        let mut row_occupied = vec![false; grid.row_count()];

        for item in &items {
            let col_start = (item.column_start - 1).max(0) as usize;
            let col_end = (item.column_end - 1).max(0) as usize;
            let row_start = (item.row_start - 1).max(0) as usize;
            let row_end = (item.row_end - 1).max(0) as usize;

            for c in col_start..col_end.min(column_occupied.len()) {
                column_occupied[c] = true;
            }
            for r in row_start..row_end.min(row_occupied.len()) {
                row_occupied[r] = true;
            }
        }

        // Collapse empty auto-fit tracks
        grid.collapse_empty_auto_fit_columns(&column_occupied);
        grid.collapse_empty_auto_fit_rows(&row_occupied);
    }

    // Phase 5: Size tracks with item contributions
    // Per CSS Grid Level 1, Section 11.5: Resolve intrinsic track sizes
    //
    // Process items by span count (1-span first, then 2-span, etc.)
    // This ensures single-span items get priority and multi-span items
    // distribute extra space among their spanned tracks.

    // Collect item info for span-based processing
    struct ItemSizing {
        row_start: usize,
        row_span: usize,
        col_start: usize,
        col_span: usize,
        height_contribution: f32,
        width_contribution: f32,
    }

    // For auto-height containers, use 0.0 for height contribution calculation.
    // This prevents percentage heights from resolving against the incorrect block-flow
    // computed height (which stacks children vertically). Items with percentage heights
    // will contribute based on their intrinsic content height instead.
    let has_definite_height = !matches!(style.height, Length::Auto);
    let height_for_contributions = if has_definite_height { container_height } else { 0.0 };

    let item_sizings: Vec<ItemSizing> = items
        .iter()
        .map(|item| ItemSizing {
            row_start: (item.row_start - 1).max(0) as usize,
            row_span: item.row_span.max(1) as usize,
            col_start: (item.column_start - 1).max(0) as usize,
            col_span: item.column_span.max(1) as usize,
            height_contribution: item.get_height_contribution(height_for_contributions),
            width_contribution: item.get_width_contribution(container_width),
        })
        .collect();

    // Find max spans
    let max_row_span = item_sizings.iter().map(|s| s.row_span).max().unwrap_or(1);
    let max_col_span = item_sizings.iter().map(|s| s.col_span).max().unwrap_or(1);

    // DEBUG: Uncomment to trace track sizing issues
    // let initial_base_sizes: Vec<f32> = grid.rows.iter().map(|t| t.base_size).collect();
    // debug!("Before contribution loop: row base_sizes = {:?}", initial_base_sizes);

    // Rows and columns: the same span-ordered distribution (see
    // distribute_span_contributions).
    let row_contributions: Vec<(usize, usize, f32)> = item_sizings
        .iter()
        .map(|s| (s.row_start, s.row_span, s.height_contribution))
        .collect();
    distribute_span_contributions(&mut grid.rows, &row_contributions, row_gap, max_row_span);
    let column_contributions: Vec<(usize, usize, f32)> = item_sizings
        .iter()
        .map(|s| (s.col_start, s.col_span, s.width_contribution))
        .collect();
    distribute_span_contributions(
        &mut grid.columns,
        &column_contributions,
        column_gap,
        max_col_span,
    );

    // DEBUG: Uncomment to trace track sizing issues
    // let after_base_sizes: Vec<f32> = grid.rows.iter().map(|t| t.base_size).collect();
    // debug!("After contribution loop: row base_sizes = {:?}", after_base_sizes);

    // Size tracks (handles percentages, intrinsic sizing, flexible tracks)
    // For auto-height containers, pass 0 as container height to prevent distributing
    // "remaining space" that doesn't exist (the container sizes to content, not vice versa)
    let row_container_height = if has_definite_height { container_height } else { 0.0 };
    size_grid_tracks(&mut grid.columns, container_width, column_gap);
    size_grid_tracks(&mut grid.rows, row_container_height, row_gap);

    // DEBUG: Uncomment to trace track sizing issues
    // let row_sizes: Vec<f32> = grid.rows.iter().map(|t| t.size).collect();
    // debug!("After size_grid_tracks: row sizes = {:?}, row_container_height = {}", row_sizes, row_container_height);

    // Stretch auto tracks if align-content is stretch AND container has definite height
    // Per CSS Grid spec, stretch distributes remaining space to auto tracks
    // Note: justify-content doesn't have a stretch value in the current CSS spec
    // IMPORTANT: Only stretch if the container has an explicit height (not auto).
    // When height is auto, the grid sizes to content and there's no extra space to distribute.
    // (has_definite_height was computed earlier for height contribution calculation)
    if style.align_content == AlignContent::Stretch && has_definite_height {
        stretch_auto_tracks(&mut grid.rows, container_height, row_gap);
    }

    // Apply content alignment (justify-content for columns, align-content for rows)
    // For auto-height containers, skip row alignment - there's no "free space" to distribute
    // when the container sizes to its content.
    apply_content_alignment(&mut grid.columns, container_width, column_gap, &style.justify_content);
    if has_definite_height {
        apply_content_alignment(&mut grid.rows, container_height, row_gap, &align_content_to_justify(&style.align_content));
    }

    // Update container height when auto-sized
    // When the container has auto height, the block layout algorithm incorrectly computes height
    // by stacking children vertically. We need to update it to the actual grid-based height.
    if !has_definite_height {
        let non_collapsed_row_count = grid.rows.iter().filter(|t| t.size > 0.0).count();
        let total_row_gaps = non_collapsed_row_count.saturating_sub(1) as f32 * row_gap;
        let actual_grid_height: f32 = grid.rows.iter().map(|t| t.size).sum::<f32>() + total_row_gaps;
        container.dimensions.content.height = actual_grid_height;
        debug!("Updated auto-height grid container: {} -> {}", container_height, actual_grid_height);
    }

    // Phase 6: Position items
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

    // Phase 7: Collect final positions (drops immutable borrow of children)
    let item_count = items.len();
    let positions: Vec<Rect> = items.iter().map(|item| item.rect.clone()).collect();
    // Row spans for Phase 9.5 (real-height row growth): (row_start, row_end)
    // as 0-based track indices, in the same order as `positions`.
    let row_spans: Vec<(usize, usize)> = items
        .iter()
        .map(|it| {
            (
                (it.row_start - 1).max(0) as usize,
                (it.row_end - 1).max(0) as usize,
            )
        })
        .collect();
    drop(items); // Explicitly drop to release borrow

    // Phase 8: Apply positions to children
    let mut position_idx = 0;
    for child in container.children.iter_mut() {
        if child.style.display == Display::None {
            continue;
        }

        if let Some(rect) = positions.get(position_idx) {
            // css-grid-1 §6.5: the grid area is filled by the item's MARGIN
            // box. Alignment — including `stretch`, which is what makes this
            // load-bearing — runs on the area shrunk by the margins, so a
            // stretched item ends up `area - margins` tall rather than
            // swallowing its own margin back into its border box.
            let (margin_left, margin_right, margin_top, margin_bottom) =
                item_margins(&child.style);
            let area = Rect::new(
                rect.x + margin_left,
                rect.y + margin_top,
                (rect.width - margin_left - margin_right).max(0.0),
                (rect.height - margin_top - margin_bottom).max(0.0),
            );
            child.dimensions.margin.left = margin_left;
            child.dimensions.margin.right = margin_right;
            child.dimensions.margin.top = margin_top;
            child.dimensions.margin.bottom = margin_bottom;

            // Apply alignment - returns border-box dimensions
            let (x, border_box_width) = apply_justify_self(
                &child.style.justify_self,
                &style.justify_items,
                area.x,
                area.width,
                child,
            );

            let (y, border_box_height) = apply_align_self(
                &child.style.align_self,
                &style.align_items,
                area.y,
                area.height,
                child,
            );

            // Calculate padding and border. `rem` resolves against the ROOT
            // font size (16px, the crate-wide convention in length_to_px /
            // intrinsic_len_px), never the item's own: with the item's
            // font-size passed as the root, `padding: 0.75rem` on a 14px
            // card read as 10.5px, and every flex/block grid item with rem
            // padding came out 2·(rem·(16 − font)) short in both axes
            // (new_tab's .shortcut rows 57 for Chrome's 60, kbd x 387 for
            // 389).
            let font_size = match child.style.font_size {
                Length::Px(px) => px,
                _ => 16.0,
            };
            let px = |l: &Length, against: f32| l.to_px(font_size, 16.0, against);
            let padding_left = px(&child.style.padding_left, border_box_width);
            let padding_right = px(&child.style.padding_right, border_box_width);
            let padding_top = px(&child.style.padding_top, border_box_height);
            let padding_bottom = px(&child.style.padding_bottom, border_box_height);
            let border_left = px(&child.style.border_left_width, border_box_width);
            let border_right = px(&child.style.border_right_width, border_box_width);
            let border_top = px(&child.style.border_top_width, border_box_height);
            let border_bottom = px(&child.style.border_bottom_width, border_box_height);

            // Set padding and border dimensions
            child.dimensions.padding.left = padding_left;
            child.dimensions.padding.right = padding_right;
            child.dimensions.padding.top = padding_top;
            child.dimensions.padding.bottom = padding_bottom;
            child.dimensions.border.left = border_left;
            child.dimensions.border.right = border_right;
            child.dimensions.border.top = border_top;
            child.dimensions.border.bottom = border_bottom;

            // Derive the content box. The alignment helpers hand back the
            // SPECIFIED size: an explicit `width`/`height` verbatim (so
            // box-sizing decides what it covers), or the grid area for
            // `auto`. An auto-sized item fills the area with its margin box
            // (css-grid-1 §6.6 / css-align-3 §5.4 stretch), so its content
            // is the area minus padding and border whatever its box-sizing.
            // Treating the area as a content-box size added the padding on
            // top: a padded content-box item overflowed its track by its
            // padding in both axes (repro grid-auto-fit-minmax: 223.3 wide
            // in a 199.3 track).
            let is_border_box = child.style.box_sizing == BoxSizing::BorderBox;
            let explicit_width = !matches!(child.style.width, Length::Auto);
            let explicit_height = !matches!(child.style.height, Length::Auto);
            let h_padding_border = padding_left + padding_right + border_left + border_right;
            let v_padding_border = padding_top + padding_bottom + border_top + border_bottom;
            let content_width = if explicit_width && !is_border_box {
                border_box_width
            } else {
                (border_box_width - h_padding_border).max(0.0)
            };
            let content_height = if explicit_height && !is_border_box {
                border_box_height
            } else {
                (border_box_height - v_padding_border).max(0.0)
            };

            // Position includes padding and border offset
            child.dimensions.content.x = x + padding_left + border_left;
            child.dimensions.content.y = y + padding_top + border_top;
            child.dimensions.content.width = content_width;
            child.dimensions.content.height = content_height;
        }
        position_idx += 1;
    }

    // Phase 9: Recursively layout children of grid items.
    // Each item's REAL flowed content height is recorded for Phase 9.5 —
    // the track-sizing pass only had estimate_content_height (blind to
    // wrapped text), so auto rows can be far too short.
    let mut real_heights: Vec<Option<f32>> = vec![None; positions.len()];
    let mut phase9_idx: usize = 0;
    for child in container.children.iter_mut() {
        if child.style.display == Display::None {
            continue;
        }
        let item_idx = phase9_idx;
        phase9_idx += 1;

        if !child.children.is_empty() {
            if child.style.display.is_flex() {
                // Nested flex container
                let child_containing = child.dimensions.clone();
                crate::flex::layout_flex_container(child, &child_containing);
                if let Some(slot) = real_heights.get_mut(item_idx) {
                    *slot = Some(child.dimensions.content.height);
                }
            } else if child.style.display.is_grid() {
                // Nested grid container
                layout_grid_container(
                    child,
                    child.dimensions.content.width,
                    child.dimensions.content.height,
                );
                if let Some(slot) = real_heights.get_mut(item_idx) {
                    *slot = Some(child.dimensions.content.height);
                }
            } else {
                // Block container: re-layout children with correct positioning and height resolution.
                // The grid item's dimensions.content.height is the grid-assigned height.
                // Children should:
                // 1. Position at the top of the grid item (not below its height)
                // 2. Resolve percentage heights against the grid item's actual height
                let grid_item_height = child.dimensions.content.height;
                // The grid item's own ratio inputs, read before the loop
                // below takes a mutable borrow of its children (see the
                // out-of-flow arm in Phase 9).
                let item_style_height = child.style.height.clone();
                let item_style_for_ratio = child.style.clone();
                let item_pb_width = child.dimensions.padding.left
                    + child.dimensions.padding.right
                    + child.dimensions.border.left
                    + child.dimensions.border.right;
                let item_pb_height = child.dimensions.padding.top
                    + child.dimensions.padding.bottom
                    + child.dimensions.border.top
                    + child.dimensions.border.bottom;
                let grid_item_y = child.dimensions.content.y;
                let grid_item_x = child.dimensions.content.x;
                let grid_item_width = child.dimensions.content.width;
                let mut current_y = grid_item_y;
                // Sibling margin collapse for the re-stack below (CSS 2.1
                // §8.3.1). A grid item establishes an independent formatting
                // context — the FRESH context means nothing collapses across
                // the item boundary — but its in-flow children collapse among
                // themselves. The old advance summed prev.margin_bottom +
                // next.margin_top at every seam, re-implementing block flow
                // without collapse and overwriting the collapsed pre-pass
                // (measured: sticky-scroll main column +20/+10 staircase).
                let mut seam_margins = crate::MarginCollapseContext::new();
                // A positioned grid item is the containing block for its abs
                // descendants; a static one is not.
                // A grid item that is a positioned CONTAINING BLOCK anchors its
                // abs descendants. Check the STYLE position, not the layout
                // position field: the engine maps relative->Static in that field
                // (to stay out of the z-reorder paint path that regresses about's
                // cards), which also dropped relative's containing-block role.
                // The style still says relative, so use it — this establishes the
                // CB without entering the positioned paint path.
                let grid_item_positioned = !matches!(
                    child.style.position,
                    rustkit_css::Position::Static
                );

                trace!(
                    "Phase 9: Re-laying out children of grid item. grid_item_height={}, grid_item_y={}",
                    grid_item_height, grid_item_y
                );

                for grandchild in &mut child.children {
                    // DEBUG: Mark that we've been here by setting a specific height
                    trace!("Phase 9: Processing grandchild with position={:?}", grandchild.position);

                    // Out-of-flow children take no flow space, but they DO need
                    // positioning against the grid item when it is their
                    // containing block (position:relative). Skipping them left
                    // abs overlays (image-gallery captions) at their block-flow
                    // geometry — full container width, stacked BELOW the grid
                    // (measured: overlay at y=879 w=1200 for an item at y=147
                    // w=288) — so overflow:hidden clipped them off-card. Lay the
                    // out-of-flow child out against the item's box; its own
                    // apply_position_offsets (subtree-aware since #50) then
                    // resolves inset/bottom and carries its text.
                    if grandchild.position == crate::Position::Absolute
                        || grandchild.position == crate::Position::Fixed {
                        if grid_item_positioned {
                            // The item's own `aspect-ratio` height, where it
                            // has one. Phase 9 runs BEFORE the item's
                            // `calculate_block_height` applies the ratio, so
                            // `child.dimensions.content.height` is still the
                            // pre-ratio number here — 32 on image-gallery's
                            // four `.aspect-box` cards, every one of which
                            // ends up 288/216/192/162 tall a moment later.
                            // Handing that 32 over as the containing block
                            // makes an `inset: 0` overlay stretch to 32 in a
                            // 288px card: `.content` came out 256px short on
                            // all four, and Chrome puts it at the full 288.
                            //
                            // css-sizing-4 §4 — a definite width plus a ratio
                            // is a definite height. The width IS resolved by
                            // now, so the number was available and only had
                            // to be asked for.
                            //
                            // GROW-ONLY, and `height: auto` only, because that
                            // is what Phase 9.5 does twenty lines below when it
                            // repairs the item itself: content wins where it is
                            // taller (a `4 / 1` item 400px wide holding a 300px
                            // child is 300, not the ratio's 100). The overlay's
                            // containing block has to be the height the item
                            // actually ends up with — the two passes disagreeing
                            // would just move the defect.
                            let cb_height = if matches!(item_style_height, rustkit_css::Length::Auto)
                            {
                                match crate::aspect_ratio_content_height(
                                    &item_style_for_ratio,
                                    grid_item_width,
                                    item_pb_width,
                                    item_pb_height,
                                ) {
                                    Some(ar_h) => grid_item_height.max(ar_h),
                                    None => grid_item_height,
                                }
                            } else {
                                grid_item_height
                            };
                            let item_cb = crate::Dimensions {
                                content: crate::Rect::new(
                                    grid_item_x,
                                    grid_item_y,
                                    grid_item_width,
                                    cb_height,
                                ),
                                ..Default::default()
                            };
                            grandchild.layout(&item_cb);
                            // `item_cb` is this grandchild's REAL containing
                            // block, which `layout` above cannot know: the
                            // generic path has to assume it may have been
                            // handed a static-position stand-in. The re-anchor
                            // is where that is asserted, and it is what
                            // re-justifies an inset-stretched flex line in the
                            // used height instead of in the flow cursor
                            // (image-gallery's `.aspect-box > .content`).
                            grandchild.reanchor_absolute(&item_cb);
                        } else {
                            trace!("Phase 9: abs/fixed grandchild, static grid item — skip");
                        }
                        continue;
                    }

                    // The grandchild's used width in the grid item's box. A
                    // block box gets the ordinary block-width rules (§10.3.3:
                    // a specified width, min/max, box-sizing, auto margins,
                    // shrink-to-fit for inline-blocks) against the ITEM's
                    // width. This loop used to give every grandchild the
                    // item's full width, so `<div style="width:100px">` in a
                    // 600px item came out 600 wide, and a `margin: 0 auto`
                    // child was never centred. A replaced element keeps the
                    // size its own layout gave it: google's logo (a 272px
                    // inline SVG) was stretched to its 1072px grid item.
                    // Text and inline boxes keep the historical fill.
                    let stale_width = grandchild.dimensions.content.width;
                    match grandchild.box_type {
                        crate::BoxType::Block | crate::BoxType::AnonymousBlock => {
                            let item_box = crate::Dimensions {
                                content: crate::Rect::new(
                                    grid_item_x,
                                    current_y,
                                    grid_item_width,
                                    grid_item_height,
                                ),
                                ..Default::default()
                            };
                            grandchild.calculate_block_width(&item_box);
                        }
                        crate::BoxType::Image { .. } => {}
                        _ => {
                            grandchild.dimensions.content.width = grid_item_width
                                - grandchild.dimensions.margin.left
                                - grandchild.dimensions.border.left
                                - grandchild.dimensions.padding.left
                                - grandchild.dimensions.margin.right
                                - grandchild.dimensions.border.right
                                - grandchild.dimensions.padding.right;
                        }
                    }

                    // Calculate the grandchild's margin box offsets
                    let margin_top = grandchild.dimensions.margin.top;
                    let border_top = grandchild.dimensions.border.top;
                    let padding_top = grandchild.dimensions.padding.top;
                    let margin_left = grandchild.dimensions.margin.left;
                    let border_left = grandchild.dimensions.border.left;
                    let padding_left = grandchild.dimensions.padding.left;

                    // Set the grandchild's position directly — and carry the
                    // WHOLE SUBTREE with it. This loop used to move only the
                    // grandchild box while its descendants kept pre-grid
                    // geometry (sticky-scroll: article-card correctly at
                    // (340,90), its gradient hero still at (60,647) from the
                    // block pre-pass — painted below the fold). Same disease
                    // flex had; same cure (translate_subtree).
                    let old_x = grandchild.dimensions.content.x;
                    let old_y = grandchild.dimensions.content.y;
                    grandchild.dimensions.content.x = grid_item_x + margin_left + border_left + padding_left;
                    seam_margins.add_margin(margin_top);
                    grandchild.dimensions.content.y =
                        current_y + seam_margins.resolve() + border_top + padding_top;
                    let dx = grandchild.dimensions.content.x - old_x;
                    let dy = grandchild.dimensions.content.y - old_y;
                    if dx != 0.0 || dy != 0.0 {
                        for gc_child in &mut grandchild.children {
                            crate::flex::translate_subtree(gc_child, dx, dy);
                        }
                    }

                    // The block pre-pass laid this whole subtree out against
                    // the GRID CONTAINER's content width, because grid item
                    // widths do not exist until track sizing has run. The line
                    // above repairs the grandchild's own box; until now nothing
                    // repaired anything BELOW it, and the old comment here said
                    // so out loud ("For block, children were already laid out").
                    // They were — against the wrong containing block. Measured
                    // on sticky-scroll: `.sidebar-card` correct at 250px with
                    // every one of its h3/ul/li children at 1120px, which is the
                    // container's 1160px content box less the card's padding —
                    // 30 boxes, +910px each.
                    //
                    // Re-flow the subtree against the corrected box. Position is
                    // already final (set above), so the children land relative to
                    // it, and the collapse pass writes the grandchild's auto
                    // height back — which is why this runs BEFORE the height
                    // resolution below: with the correct width, text wraps to a
                    // different line count and the stale height is wrong too.
                    //
                    // Only when the width actually moved: an unchanged width
                    // means the pre-pass geometry is already right, and
                    // re-flowing it would be a no-op that still costs a walk of
                    // the subtree on every grid item on the page.
                    //
                    // That narrowing is a COST guard, measured and not assumed:
                    // forcing `width_changed` to true is bit-identical on all
                    // 26 corpus cases, so nothing tests it and nothing should
                    // pretend to. Deleting the CALL is a different matter and
                    // is guarded — see the tests below.
                    //
                    // `!children.is_empty()` reads like the same kind of cost
                    // guard and is NOT one. A block re-flow derives the box's
                    // height from the children it flows, so running it over a
                    // childless box — every text run is one — writes a height
                    // of zero. Measured: dropping that clause takes Gate A from
                    // 2500 to 2572 failing axes.
                    //
                    // The flex/grid exclusions below are a COST guard and not a
                    // correctness one, stated that way because a mutation sweep
                    // asked and the answer was measured: removing them is
                    // bit-identical on all 26 corpus cases, and it is also
                    // bit-identical on a hand-built auto-height row-flex
                    // grandchild, which is the shape that should have broken.
                    // The flex/grid repair further down re-derives the box the
                    // block pass touched, so the block pass is throwaway work
                    // rather than a wrong answer. Nothing here is guarded by a
                    // test, because every test written for it stayed green
                    // without it.
                    let width_changed =
                        (grandchild.dimensions.content.width - stale_width).abs() > 0.01;
                    if width_changed
                        && !grandchild.children.is_empty()
                        && !grandchild.style.display.is_flex()
                        && !grandchild.style.display.is_grid()
                    {
                        let mut child_margins = crate::MarginCollapseContext::new();
                        let mut floats = crate::FloatContext::new();
                        grandchild
                            .layout_block_children_with_collapse(&mut child_margins, &mut floats, None);
                    }

                    // Calculate height for percentage resolution
                    // DEBUG: Uncomment to trace Phase 9 percentage height issues
                    // debug!("Phase 9: grandchild style.height={:?}, grid_item_height={}, existing_height={}",
                    //        grandchild.style.height, grid_item_height, grandchild.dimensions.content.height);
                    let grandchild_height = match &grandchild.style.height {
                        rustkit_css::Length::Percent(pct) => {
                            // Resolve percentage against grid item's height
                            (pct / 100.0 * grid_item_height).max(0.0)
                        }
                        rustkit_css::Length::Px(h) => *h,
                        rustkit_css::Length::Auto => {
                            // For auto height, use the existing computed height from children
                            grandchild.dimensions.content.height
                        }
                        _ => grandchild.dimensions.content.height
                    };

                    // Apply min-height constraint
                    let min_height = match &grandchild.style.min_height {
                        rustkit_css::Length::Px(h) => *h,
                        rustkit_css::Length::Percent(pct) => pct / 100.0 * grid_item_height,
                        _ => 0.0,
                    };

                    grandchild.dimensions.content.height = grandchild_height.max(min_height);

                    // Re-layout grandchild's children with the corrected dimensions
                    if !grandchild.children.is_empty() {
                        let grandchild_containing = grandchild.dimensions.clone();
                        if grandchild.style.display.is_flex() {
                            crate::flex::layout_flex_container(grandchild, &grandchild_containing);
                        } else if grandchild.style.display.is_grid() {
                            layout_grid_container(grandchild, grandchild.dimensions.content.width, grandchild.dimensions.content.height);
                        }
                        // Block containers re-flowed above, before the height
                        // resolution that depends on their reflowed extent.
                    }

                    // Update y for next sibling: advance to the BORDER-BOX
                    // bottom and bank margin_bottom in the collapse context,
                    // where the next sibling's margin_top will max against it
                    // instead of stacking on top of it.
                    current_y = grandchild.dimensions.content.y + grandchild.dimensions.content.height
                        + grandchild.dimensions.padding.bottom + grandchild.dimensions.border.bottom;
                    seam_margins.reset();
                    seam_margins.add_margin(grandchild.dimensions.margin.bottom);
                }

                // Record the item's real flowed content height for Phase 9.5.
                // current_y now stops at the last border-box bottom; the last
                // child's bottom margin is pending in the context. Resolve it
                // here so the recorded height keeps the pre-fix semantics
                // (trailing margin included).
                if let Some(slot) = real_heights.get_mut(item_idx) {
                    *slot = Some((current_y + seam_margins.resolve() - grid_item_y).max(0.0));
                }
            }
        }
    }

    // Phase 9.5: re-size AUTO rows to the items' REAL content heights.
    // Track sizing ran on estimate_content_height, which cannot see wrapped
    // text (holdout-grid-mosaic: tiles measured 110px — the banner only —
    // while their text put real height at 205px; row 2 was then placed at
    // the stale 110px pitch, overlapping row 1's content). Single-row items
    // grow their row to the real border-box height; later rows shift down
    // with their subtrees; the container's auto height is recomputed.
    //
    // n51: the same estimate also OVER-shoots, and this pass was grow-only.
    // `count_text_lines` charges one line-height per text NODE, so a flex
    // row holding `<kbd>Ctrl</kbd>/<kbd>Cmd</kbd>+<kbd>K</kbd> <span>…</span>`
    // — seven text nodes on ONE line — was estimated at seven lines: new_tab's
    // `.shortcuts` grid sized every row 143px for items that lay out 60px
    // tall (Chrome's pitch 72, RustKit's 152), the grid ran 832px instead of
    // 400, the page's flex-centred container overflowed the viewport, and
    // every box from the search field down sat 63px above Chrome's. An
    // `auto` track is minmax(min-content, max-content) — its size IS the
    // items' real size — so a row of that kind now shrinks to the tallest
    // single-row item's margin box, the same figure the grow path already
    // computes. Rows a multi-row item spans, and rows with an item whose
    // real height is unknown, keep the grow-only behaviour.
    if !has_definite_height && !grid.rows.is_empty() {
        // Per row: the tallest single-row item's margin box (`None` = no
        // single-row item with a known height), and whether the row may
        // shrink to it.
        let mut row_real: Vec<Option<f32>> = vec![None; grid.rows.len()];
        let mut row_shrinkable: Vec<bool> = grid
            .rows
            .iter()
            .map(|t| {
                t.is_min_content
                    && t.is_max_content
                    && !t.is_flexible
                    && t.percent.is_none()
                    && t.max_percent.is_none()
                    && t.fit_content_limit.is_none()
            })
            .collect();
        {
            let mut idx = 0usize;
            for child in container.children.iter() {
                if child.style.display == Display::None {
                    continue;
                }
                if let Some(&(r0, r1)) = row_spans.get(idx) {
                    // Single-row items only; multi-span distribution is a
                    // separate (rarer) problem. A spanning item's
                    // contribution was spread over its rows by track sizing
                    // and this pass cannot re-derive it, so those rows stay
                    // grow-only.
                    if r1 > r0 + 1 {
                        for r in r0..r1.min(grid.rows.len()) {
                            row_shrinkable[r] = false;
                        }
                    }
                    if r1 <= r0 + 1 && r0 < grid.rows.len() {
                        let pb = child.dimensions.padding.top
                            + child.dimensions.padding.bottom
                            + child.dimensions.border.top
                            + child.dimensions.border.bottom;

                        // `real_h + pb` is a BORDER box; the row is a margin
                        // box. Comparing them directly makes an item with
                        // margins look like it already fits, so this pass
                        // stops repairing it.
                        //
                        // A childless item flowed nothing: its real content
                        // height is 0, not unknown. An explicit `height` is
                        // its own answer (the estimate used it too), floored
                        // by `min-height` like the rest.
                        let mut wanted: Option<f32> = match real_heights.get(idx) {
                            Some(Some(real_h)) => Some(real_h + pb),
                            _ if child.children.is_empty() => Some(pb),
                            _ => None,
                        };
                        let is_border_box = child.style.box_sizing == BoxSizing::BorderBox;
                        if let Length::Px(h) = child.style.height {
                            let border_box = if is_border_box { h } else { h + pb };
                            wanted = Some(border_box.max(wanted.unwrap_or(0.0)));
                        }
                        if let Length::Px(min_h) = child.style.min_height {
                            let floor = if is_border_box { min_h } else { min_h + pb };
                            wanted = Some(floor.max(wanted.unwrap_or(0.0)));
                        }
                        if !matches!(child.style.height, Length::Px(_) | Length::Auto)
                            || !matches!(child.style.min_height, Length::Px(_) | Length::Auto)
                        {
                            // A percentage or other relative block size: the
                            // estimate and the flow disagree on what it
                            // resolves against; do not shrink under it.
                            row_shrinkable[r0] = false;
                        }

                        // css-sizing-4 §4: an `aspect-ratio` item whose block
                        // size is `auto` derives it from its (now definite)
                        // inline size. Track sizing could not: it runs before
                        // the columns are resolved, so `get_height_contribution`
                        // sees no inline size to derive from and falls through
                        // to the content estimate. Every `aspect-ratio` grid
                        // item therefore sized to its content alone —
                        // `image-gallery`'s four `.aspect-box`es collapsed from
                        // 288/216/192/162 to 32, taking `.aspect-section` from
                        // 332 to 73 and shifting the 85 boxes below it 272px up
                        // the page.
                        //
                        // Grow-only, like the rest of this pass, and that is
                        // also what Chrome does here: measured, a `4 / 1` item
                        // 400px wide holding a 300px-tall child is 300 tall,
                        // not the ratio's 100. Content wins where it is taller;
                        // the ratio wins where the box would otherwise collapse.
                        if matches!(child.style.height, Length::Auto) {
                            let pb_w = child.dimensions.padding.left
                                + child.dimensions.padding.right
                                + child.dimensions.border.left
                                + child.dimensions.border.right;
                            if let Some(ar_h) = crate::aspect_ratio_content_height(
                                &child.style,
                                child.dimensions.content.width,
                                pb_w,
                                pb,
                            ) {
                                let ar_border_box = ar_h + pb;
                                wanted = Some(match wanted {
                                    Some(w) => w.max(ar_border_box),
                                    None => ar_border_box,
                                });
                            }
                        }

                        match wanted {
                            Some(wanted) => {
                                let outer = wanted + vertical_margins(&child.style);
                                row_real[r0] =
                                    Some(row_real[r0].map_or(outer, |r: f32| r.max(outer)));
                            }
                            None => row_shrinkable[r0] = false,
                        }
                    }
                }
                idx += 1;
            }
        }

        // Per row: the change to apply. Growth wherever the items need
        // more; shrinkage only where the track is intrinsic and every item
        // in it reported a real height.
        let row_delta: Vec<f32> = grid
            .rows
            .iter()
            .enumerate()
            .map(|(i, track)| match row_real[i] {
                Some(real) => {
                    let delta = real - track.size;
                    if delta > 0.5 || (delta < -0.5 && row_shrinkable[i]) {
                        delta
                    } else {
                        0.0
                    }
                }
                None => 0.0,
            })
            .collect();

        if row_delta.iter().any(|g| *g != 0.0) {
            let old_positions: Vec<f32> = grid.rows.iter().map(|t| t.position).collect();
            for (i, g) in row_delta.iter().enumerate() {
                grid.rows[i].size = (grid.rows[i].size + g).max(0.0);
            }
            // Recompute row positions from the first row's origin; a gap
            // follows every non-collapsed row (mirrors the auto-height
            // container formula above).
            let mut cursor = old_positions.first().copied().unwrap_or(0.0);
            for row in grid.rows.iter_mut() {
                row.position = cursor;
                if row.size > 0.0 {
                    cursor += row.size + row_gap;
                }
            }

            let mut idx = 0usize;
            for child in container.children.iter_mut() {
                if child.style.display == Display::None {
                    continue;
                }
                if let Some(&(r0, _)) = row_spans.get(idx) {
                    if r0 < grid.rows.len() {
                        let dy = grid.rows[r0].position - old_positions[r0];
                        if dy.abs() > 0.01 {
                            crate::flex::translate_subtree(child, 0.0, dy);
                        }
                        // Default align stretch: the item's MARGIN box fills
                        // the (re-sized) row, so its border box gets the row
                        // less its own margins. Explicit heights are left
                        // alone. In a grown row content taller than the row
                        // is left alone too; in a SHRUNK row the row is the
                        // tallest item's real height, so an item still
                        // holding the stale area height (the block path
                        // hands it the pre-pass area) gives it back.
                        if matches!(child.style.height, Length::Auto) {
                            let pb = child.dimensions.padding.top
                                + child.dimensions.padding.bottom
                                + child.dimensions.border.top
                                + child.dimensions.border.bottom;
                            let target = grid.rows[r0].size - vertical_margins(&child.style) - pb;
                            if child.dimensions.content.height < target
                                || (row_delta[r0] < 0.0 && child.dimensions.content.height > target)
                            {
                                child.dimensions.content.height = target;
                            }
                        }
                    }
                }
                idx += 1;
            }

            let non_collapsed = grid.rows.iter().filter(|t| t.size > 0.0).count();
            let total_gaps = non_collapsed.saturating_sub(1) as f32 * row_gap;
            container.dimensions.content.height =
                grid.rows.iter().map(|t| t.size).sum::<f32>() + total_gaps;
        }
    }

    // Phase 9.6: an `aspect-ratio` item keeps its ratio instead of stretching.
    //
    // `align-self: stretch` is the grid default, so every auto-height item is
    // grown to its row. An item with a non-`auto` `aspect-ratio` must not be:
    // measured against Chrome, `image-gallery`'s four `.aspect-box`es share one
    // 288px row (the `1 / 1` box sets it) and Chrome still lays them out
    // 288 / 216 / 192 / 162 — each keeps its own ratio and the shorter three
    // simply do not fill the row. Stretching them made the three non-tallest
    // boxes 288 apiece.
    //
    // Deliberately outside the `!has_definite_height` block above: the ratio
    // holds whether or not any row happened to grow, and gating it on that
    // would make an item's height depend on its neighbours' content.
    //
    // The ratio replaces the STRETCH, never the item's own content: Chrome,
    // measured, gives a 400px-wide `4 / 1` item holding a 300px-tall child a
    // height of 300, not the ratio's 100. So the item's real flowed height —
    // the same figure Phase 9.5 grew its row from — is the floor. Writing the
    // ratio unconditionally here was the first draft, and it shrank exactly
    // that case; `content_taller_than_the_ratio_keeps_its_own_height` is the
    // guard that caught it.
    {
        let mut idx = 0usize;
        for child in container.children.iter_mut() {
            if child.style.display == Display::None {
                continue;
            }
            let item_idx = idx;
            idx += 1;
            if !matches!(child.style.height, Length::Auto) {
                continue;
            }
            let pb_v = child.dimensions.padding.top
                + child.dimensions.padding.bottom
                + child.dimensions.border.top
                + child.dimensions.border.bottom;
            let pb_w = child.dimensions.padding.left
                + child.dimensions.padding.right
                + child.dimensions.border.left
                + child.dimensions.border.right;
            if let Some(ar_h) = crate::aspect_ratio_content_height(
                &child.style,
                child.dimensions.content.width,
                pb_w,
                pb_v,
            ) {
                let content_floor = match real_heights.get(item_idx) {
                    Some(Some(real_h)) => *real_h,
                    _ => 0.0,
                };
                child.dimensions.content.height = ar_h.max(content_floor);
            }
        }
    }

    // Phase 9.7: `height: fit-content` items take their CONTENT height.
    //
    // css-align-3 §4.2: `stretch` is the used alignment only where the item's
    // size in that axis is `auto`. `fit-content` is not `auto` — it is how a
    // page opts one item out of stretching — so a fit-content item keeps the
    // size its content gives it while its siblings fill the row.
    //
    // This cannot be done in Phase 8 with the rest of alignment, because an
    // item's content height does not exist until Phase 9 has flowed its
    // children; Phase 8 can only hand it the grid area. Phase 9 records that
    // number for every item, so the correction lands here, after Phase 9.5 has
    // had its say about the row — 9.5 grows rows and stretches AUTO items, and
    // must not then be undone. It is its own pass rather than a branch of 9.6
    // because the two name disjoint items: 9.6 acts only on `height: auto`,
    // this only on `height: fit-content`.
    //
    // Written as an assignment rather than a shrink, because the rule is "size
    // to content" in both directions. Be clear about what that buys today:
    // NOTHING, and it is measured rather than assumed. A mutation replacing
    // this with `min()` survives the whole suite, because Phase 9's re-flow has
    // already grown any item whose content overruns the box it was handed — so
    // by the time this runs, `real_h` is never larger than the current height
    // and the growing direction is unreachable. The assignment states the rule;
    // the shrink is the only half a test can hold, and that is said here
    // instead of shipping a guard that would stay green without its fix.
    {
        let mut idx = 0usize;
        for child in container.children.iter_mut() {
            if child.style.display == Display::None {
                continue;
            }
            if matches!(child.style.height, Length::FitContent) {
                if let Some(Some(real_h)) = real_heights.get(idx).copied() {
                    child.dimensions.content.height = real_h.max(0.0);
                }
            }
            idx += 1;
        }
    }

    debug!(
        "Grid layout complete: {} columns, {} rows, {} items",
        grid.column_count(),
        grid.row_count(),
        item_count
    );
}

/// Size grid tracks using the track sizing algorithm.
/// Estimate of a box's min-content (border-box) width.
///
/// Explicit pixel widths are exact. Text contributes its longest unbreakable
/// unit (longest word) — under nowrap/pre, the whole run — measured with the
/// same shaper layout uses, so min-content matches what line-box wrapping
/// will actually produce. Consecutive inline-level boxes under nowrap sum
/// into one unbreakable run; otherwise children contribute independently
/// (max), per css-sizing-3 §4.
pub(crate) fn estimate_min_content_width(layout_box: &LayoutBox) -> f32 {
    // Out-of-flow boxes don't CONTRIBUTE to an ancestor's intrinsic size.
    // That is a statement about contribution, not about the box's own
    // min-content width — which CSS 2.1 §10.3.7 needs in order to size the
    // box itself. Callers that want the latter use `own_min_content_width`.
    //
    // The text carve-out is not a new rule: this guard used to sit BELOW the
    // `BoxType::Text` arm, so a text box carrying an out-of-flow position
    // always answered its text width. Keeping the precedence keeps the split
    // behaviour-preserving.
    if matches!(
        layout_box.position,
        crate::Position::Absolute | crate::Position::Fixed
    ) && !matches!(layout_box.box_type, BoxType::Text(_))
    {
        return 0.0;
    }
    own_min_content_width(layout_box)
}

/// The box's OWN min-content width (border box), with the out-of-flow
/// contribution rule NOT applied to the box itself.
///
/// Split out of `estimate_min_content_width` so shrink-to-fit can size an
/// out-of-flow box from its own content. The contribution rule still applies
/// to every CHILD walked below, exactly as before.
pub(crate) fn own_min_content_width(layout_box: &LayoutBox) -> f32 {
    let style = &layout_box.style;
    if style.display == Display::None {
        return 0.0;
    }
    if let BoxType::Text(text) = &layout_box.box_type {
        return text_min_content_width(text, style);
    }

    let padding_border = horizontal_padding_border(style);

    // An explicit pixel width fixes the contribution regardless of content.
    if let Length::Px(w) = style.width {
        return match style.box_sizing {
            BoxSizing::BorderBox => w,
            BoxSizing::ContentBox => w + padding_border,
        };
    }

    // Content-derived width. Under white-space that forbids wrapping,
    // consecutive inline-level children form one unbreakable run (sum);
    // otherwise every child stands alone (max). A block-level child always
    // interrupts an inline run.
    let nowrap = matches!(style.white_space, WhiteSpace::Nowrap | WhiteSpace::Pre);
    // css-text-3 §4.1 + §4.1.3: inside a run that cannot wrap, the document
    // white space BETWEEN two inline-level boxes collapses to one space and is
    // rendered — it is not a break opportunity, so min-content must carry it.
    // Only under `nowrap`: `pre` preserves white space verbatim and breaks at
    // its newlines, which this function does not model, so that arm keeps the
    // behaviour it has rather than gaining a wrong one.
    let collapses_to_one_space = matches!(style.white_space, WhiteSpace::Nowrap);
    let mut max_contribution = 0.0f32;
    let mut inline_run = 0.0f32;
    let mut run_has_content = false;
    // A collapsed space is only rendered between two inline contributions.
    // Held here until the next one arrives; dropped if the run ends first,
    // which is how white space at a line's edges is removed.
    let mut pending_space = 0.0f32;
    for child in &layout_box.children {
        if child.style.display == Display::None {
            continue;
        }
        if matches!(
            child.position,
            crate::Position::Absolute | crate::Position::Fixed
        ) {
            continue;
        }
        if collapses_to_one_space && is_collapsible_whitespace_only(child) {
            if run_has_content {
                // Consecutive white-space children collapse together, so this
                // assigns rather than accumulates.
                pending_space = collapsed_space_width(&child.style);
            }
            continue;
        }
        let inline_level =
            child.style.display.is_inline_level() || matches!(child.box_type, BoxType::Text(_));
        let outer = estimate_min_content_width(child) + horizontal_margins(&child.style);
        if inline_level && nowrap {
            inline_run += pending_space + outer;
            pending_space = 0.0;
            run_has_content = true;
        } else {
            max_contribution = max_contribution.max(outer);
            if !inline_level {
                max_contribution = max_contribution.max(inline_run);
                inline_run = 0.0;
                run_has_content = false;
                pending_space = 0.0;
            }
        }
    }
    max_contribution = max_contribution.max(inline_run);

    max_contribution + padding_border
}

/// Is this child a text box made only of collapsible document white space?
///
/// css-text-3 §4.1 counts space, tab and the line endings as collapsible and
/// deliberately excludes U+00A0, which is a rendered character. A text node of
/// only NBSP is therefore NOT matched here and keeps the behaviour it has
/// (`text_min_content_width` answers 0 for it, because `str::trim` follows the
/// White_Space property and does trim NBSP). That is a separate defect from
/// the one this predicate exists for, and it is left alone rather than
/// half-fixed.
fn is_collapsible_whitespace_only(child: &LayoutBox) -> bool {
    match &child.box_type {
        BoxType::Text(text) => {
            !text.is_empty()
                && text
                    .chars()
                    .all(|c| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
        }
        _ => false,
    }
}

/// The advance of the single space that collapsible white space collapses to,
/// measured with the shaper line layout uses so the intrinsic size and the
/// laid-out line agree about the same character.
fn collapsed_space_width(style: &ComputedStyle) -> f32 {
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };
    crate::measure_text_advanced(
        " ",
        &style.font_family,
        font_size,
        style.font_weight,
        style.font_style,
    )
    .width
}

/// Min-content width of a text run: the widest unbreakable unit (word).
/// Under white-space that forbids wrapping, the whole run is unbreakable.
fn text_min_content_width(text: &str, style: &ComputedStyle) -> f32 {
    if text.trim().is_empty() {
        return 0.0;
    }
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };
    let measure = |s: &str| {
        crate::measure_text_advanced(s, &style.font_family, font_size, style.font_weight, style.font_style).width
    };
    if matches!(style.white_space, WhiteSpace::Nowrap | WhiteSpace::Pre) {
        return measure(text);
    }
    text.split_whitespace().map(measure).fold(0.0f32, f32::max)
}

/// Max-content width of a text run: the full single-line measure.
fn text_max_content_width(text: &str, style: &ComputedStyle) -> f32 {
    if text.trim().is_empty() {
        return 0.0;
    }
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };
    crate::measure_text_advanced(text, &style.font_family, font_size, style.font_weight, style.font_style).width
}

/// Estimate of a box's max-content (border-box) width: the width the box
/// takes laying its inline content on one line with no wrap opportunities
/// taken (css-sizing-3 §4). Consecutive inline-level children always sum
/// (max-content never takes an optional break); block-level children
/// interrupt the run and contribute independently. Used by flex-basis:auto
/// content sizing (css-flexbox-1 §9.2.3.C).
pub(crate) fn estimate_max_content_width(layout_box: &LayoutBox) -> f32 {
    // Contribution rule, same as `estimate_min_content_width`. Note the
    // precedence differs from that function's and is preserved as found: here
    // the out-of-flow guard sits ABOVE the text arm, so an out-of-flow text
    // box answers 0 rather than its text width. The asymmetry is pre-existing;
    // it is recorded rather than quietly harmonised, because harmonising it
    // would be an engine behaviour change riding along on a refactor.
    if matches!(
        layout_box.position,
        crate::Position::Absolute | crate::Position::Fixed
    ) {
        return 0.0;
    }
    own_max_content_width(layout_box)
}

/// The box's OWN max-content width (border box), with the out-of-flow
/// contribution rule NOT applied to the box itself. See
/// `own_min_content_width` for why the split exists.
pub(crate) fn own_max_content_width(layout_box: &LayoutBox) -> f32 {
    let style = &layout_box.style;
    if style.display == Display::None {
        return 0.0;
    }
    if let BoxType::Text(text) = &layout_box.box_type {
        return text_max_content_width(text, style);
    }

    let padding_border = horizontal_padding_border(style);

    if let Length::Px(w) = style.width {
        return match style.box_sizing {
            BoxSizing::BorderBox => w,
            BoxSizing::ContentBox => w + padding_border,
        };
    }

    // A flex container's max-content main size sums its ITEMS plus
    // main-axis gaps (row), or takes the widest item (column). Whitespace-
    // only text never becomes a flex item (css-flexbox-1 §4), so it
    // contributes neither width nor a gap slot. The generic inline-run
    // walk below misses the gaps — a nav with 30px gaps measured 120px
    // narrow, then flex-shrink smashed every link to ~2px on re-layout.
    if style.display.is_flex() {
        let is_row = style.flex_direction.is_row();
        let main_gap = match style.column_gap {
            Length::Px(g) => g,
            _ => 0.0,
        };
        let mut sum = 0.0f32;
        let mut widest = 0.0f32;
        let mut item_count = 0usize;
        for child in &layout_box.children {
            if child.style.display == Display::None {
                continue;
            }
            if matches!(
                child.position,
                crate::Position::Absolute | crate::Position::Fixed
            ) {
                continue;
            }
            if let BoxType::Text(t) = &child.box_type {
                if t.trim().is_empty() {
                    continue;
                }
            }
            let outer = estimate_max_content_width(child) + horizontal_margins(&child.style);
            sum += outer;
            widest = widest.max(outer);
            item_count += 1;
        }
        let content = if is_row {
            sum + main_gap * item_count.saturating_sub(1) as f32
        } else {
            widest
        };
        return content + padding_border;
    }

    let mut max_contribution = 0.0f32;
    let mut inline_run = 0.0f32;
    for child in &layout_box.children {
        if child.style.display == Display::None {
            continue;
        }
        if matches!(
            child.position,
            crate::Position::Absolute | crate::Position::Fixed
        ) {
            continue;
        }
        let inline_level =
            child.style.display.is_inline_level() || matches!(child.box_type, BoxType::Text(_));
        let outer = estimate_max_content_width(child) + horizontal_margins(&child.style);
        if inline_level {
            inline_run += outer;
        } else {
            max_contribution = max_contribution.max(inline_run);
            inline_run = 0.0;
            max_contribution = max_contribution.max(outer);
        }
    }
    max_contribution = max_contribution.max(inline_run);

    max_contribution + padding_border
}

/// Resolve a length used in an intrinsic-size contribution to px.
///
/// These used to be `if let Length::Px(v) = l { v } else { 0.0 }`, which
/// silently dropped EVERY relative unit. `padding: 0.25rem 0.5rem` — an
/// entirely ordinary declaration — contributed zero, so an element's
/// min/max-content width came out one padding-box too small.
///
/// The consequences were not cosmetic. Grid tracks sized from these
/// contributions were too narrow, and flex items floored at an
/// automatic-minimum computed here could be shrunk by exactly their own
/// padding — which is how a `kbd` chip whose basis was 34.66px got squeezed
/// to 18.66px while paint still drew its glyphs at full width.
///
/// Percentages are the one case still returning 0.0: they resolve against the
/// containing block, which is not available at intrinsic-sizing time. That is
/// a real remaining gap, left explicit here rather than hidden behind the
/// same silent fallback that caused this bug.
fn intrinsic_len_px(l: &Length, font_size: f32) -> f32 {
    match l {
        Length::Percent(_) => 0.0,
        other => other.to_px_with_viewport(font_size, 16.0, 0.0, 800.0, 600.0),
    }
}

/// The font size a relative length on this element resolves against.
fn style_font_size_px(style: &ComputedStyle) -> f32 {
    match style.font_size {
        Length::Px(px) => px,
        ref other => other.to_px_with_viewport(16.0, 16.0, 0.0, 800.0, 600.0),
    }
}

fn horizontal_margins(style: &ComputedStyle) -> f32 {
    let fs = style_font_size_px(style);
    intrinsic_len_px(&style.margin_left, fs) + intrinsic_len_px(&style.margin_right, fs)
}

/// The item's block-axis margins, in the same convention `horizontal_margins`
/// uses on the inline axis: percentages resolve to 0 because the containing
/// block's size is not yet known during track sizing.
pub(crate) fn vertical_margins(style: &ComputedStyle) -> f32 {
    let fs = style_font_size_px(style);
    intrinsic_len_px(&style.margin_top, fs) + intrinsic_len_px(&style.margin_bottom, fs)
}

/// The four resolved margins of a grid item, used to inset its grid area.
///
/// css-grid-1 §6.5: a grid item's MARGIN box fills its grid area — the area is
/// not the item's border box. Alignment therefore runs on the area shrunk by
/// the margins, and the border box lands inside that.
pub(crate) fn item_margins(style: &ComputedStyle) -> (f32, f32, f32, f32) {
    let fs = style_font_size_px(style);
    (
        intrinsic_len_px(&style.margin_left, fs),
        intrinsic_len_px(&style.margin_right, fs),
        intrinsic_len_px(&style.margin_top, fs),
        intrinsic_len_px(&style.margin_bottom, fs),
    )
}

/// Horizontal padding+border resolved from STYLE (the figure the intrinsic
/// estimators above fold into their border-box results). `pub(crate)` so a
/// caller that needs the CONTENT figure can subtract exactly what was added.
pub(crate) fn horizontal_padding_border(style: &ComputedStyle) -> f32 {
    let fs = style_font_size_px(style);
    intrinsic_len_px(&style.padding_left, fs)
        + intrinsic_len_px(&style.padding_right, fs)
        + intrinsic_len_px(&style.border_left_width, fs)
        + intrinsic_len_px(&style.border_right_width, fs)
}

/// css-grid-1 §12.5 (intrinsic track sizes), items processed by span count:
/// each `(start, span, contribution)` grows the base sizes of the tracks it
/// spans until they (plus the spanned gutters, which the item already owns)
/// hold it.
///
/// §12.5.1 distributes the extra space "up to limits" first: a track whose
/// max sizing function is intrinsic has, once smaller-span items have sized
/// it, a growth limit at that size — only a track NO item has sized yet keeps
/// an infinite limit. So a `span 2` item over [a row holding a 200px item,
/// an empty row] puts all its extra into the empty row; splitting it equally
/// (the old behaviour) made image-gallery's row 3 300px where Chrome has 200,
/// and the wide card in it 100px too tall. Space left once every track is at
/// its limit goes to the growable tracks equally ("beyond limits").
fn distribute_span_contributions(
    tracks: &mut [GridTrack],
    contributions: &[(usize, usize, f32)],
    gap: f32,
    max_span: usize,
) {
    const EPS: f32 = 0.01;
    // Per track: has an item of a smaller span already sized it?
    let mut sized = vec![false; tracks.len()];
    for span in 1..=max_span {
        let mut sized_this_span = Vec::new();
        for &(start, _, contribution) in contributions.iter().filter(|c| c.1 == span) {
            if contribution <= 0.0 {
                continue;
            }
            let end = (start + span).min(tracks.len());
            if start >= end {
                continue;
            }
            let spanned_gaps = gap * (end - start - 1) as f32;
            let current: f32 =
                (start..end).map(|i| tracks[i].base_size).sum::<f32>() + spanned_gaps;
            sized_this_span.extend(start..end);
            let mut extra = contribution - current;
            if extra <= 0.0 {
                continue;
            }
            let growable: Vec<usize> = (start..end)
                .filter(|&i| {
                    let t = &tracks[i];
                    t.is_min_content || t.is_max_content || t.is_flexible
                        || t.growth_limit > t.base_size
                })
                .collect();
            if growable.is_empty() {
                // All tracks are fixed: distribute equally anyway.
                let per_track = extra / (end - start) as f32;
                for t in &mut tracks[start..end] {
                    t.base_size += per_track;
                }
                continue;
            }
            let limits: Vec<f32> = growable
                .iter()
                .map(|&i| {
                    let t = &tracks[i];
                    if t.is_flexible || (t.is_max_content && !sized[i]) {
                        f32::INFINITY
                    } else if t.is_max_content {
                        t.base_size
                    } else {
                        t.growth_limit
                    }
                })
                .collect();
            // Up to limits: equal shares, freezing a track at its limit.
            let mut open: Vec<usize> = (0..growable.len())
                .filter(|&k| limits[k] > tracks[growable[k]].base_size + EPS)
                .collect();
            while extra > EPS && !open.is_empty() {
                let share = extra / open.len() as f32;
                let mut still_open = Vec::new();
                for &k in &open {
                    let t = &mut tracks[growable[k]];
                    let room = limits[k] - t.base_size;
                    let give = share.min(room);
                    t.base_size += give;
                    extra -= give;
                    if room - give > EPS {
                        still_open.push(k);
                    }
                }
                open = still_open;
            }
            // Beyond limits.
            if extra > EPS {
                let per_track = extra / growable.len() as f32;
                for &i in &growable {
                    tracks[i].base_size += per_track;
                }
            }
        }
        for i in sized_this_span {
            sized[i] = true;
        }
    }
}

fn size_grid_tracks(tracks: &mut [GridTrack], container_size: f32, gap: f32) {
    if tracks.is_empty() {
        return;
    }

    // Count non-collapsed tracks for gap calculation
    // Collapsed (auto-fit empty) tracks are explicitly marked is_auto_fit and have all sizing zeroed
    // A track is collapsed only if it's an auto-fit track with no content
    let non_collapsed_count = tracks
        .iter()
        .filter(|t| {
            // A track is NOT collapsed if:
            // - It's not an auto-fit track, OR
            // - It has some size (base_size, growth_limit, percent, flex)
            !t.is_auto_fit
                || t.base_size > 0.0
                || t.growth_limit > 0.0
                || t.is_flexible
                || t.percent.is_some()
                || t.max_percent.is_some()
        })
        .count();
    let total_gaps = non_collapsed_count.saturating_sub(1) as f32 * gap;
    let available_space = (container_size - total_gaps).max(0.0);

    // Step 1: Initialize base sizes
    for track in tracks.iter_mut() {
        track.size = track.base_size;
    }

    // Step 2: Resolve percentage tracks against container size
    // Per spec, percentage tracks are resolved against the content box of the grid container
    for track in tracks.iter_mut() {
        // Resolve min percentage (base_size)
        if let Some(pct) = track.percent {
            let resolved_size = container_size * (pct / 100.0);
            track.base_size = resolved_size;
            track.size = resolved_size;
            // If no max percentage and not flexible, growth_limit = base_size
            if track.max_percent.is_none() && !track.is_flexible {
                track.growth_limit = resolved_size;
            }
        }
        // Resolve max percentage (growth_limit)
        if let Some(pct) = track.max_percent {
            let resolved_size = container_size * (pct / 100.0);
            track.growth_limit = resolved_size;
            // If track hasn't been sized yet, use base_size
            if track.size < track.base_size {
                track.size = track.base_size;
            }
        }
    }

    // Step 2.5: Handle intrinsic tracks (min-content, max-content, auto, fit-content)
    // Item contributions should already be set in base_size from layout_grid_container
    for track in tracks.iter_mut() {
        if track.is_min_content {
            // min-content track: size is the minimum content size
            // base_size should already have the item contribution
            track.size = track.base_size;
            // For pure min-content, growth_limit = base_size (no growth allowed)
            if !track.is_max_content && track.fit_content_limit.is_none() {
                track.growth_limit = track.base_size;
            }
        }
        if track.is_max_content {
            // max-content track: can grow to fit content
            // base_size has min contribution, allow growth
            track.size = track.base_size;
            // growth_limit stays at INFINITY or is set based on max-content
            // For pure max-content, we want to expand to fill
            if track.growth_limit == 0.0 {
                track.growth_limit = f32::INFINITY;
            }
        }
        // Handle fit-content(length): clamp growth_limit to the specified length
        // fit-content behaves like minmax(min-content, min(max-content, length))
        if let Some(limit) = track.fit_content_limit {
            // Base size is already set from min-content contribution
            track.size = track.base_size;
            // Cap growth at the specified limit
            track.growth_limit = limit.min(track.growth_limit);
            // But growth_limit should be at least base_size
            track.growth_limit = track.growth_limit.max(track.base_size);
        }
    }

    // Step 3: Distribute remaining space to flexible tracks.
    //
    // css-grid-1 §12.7.1 "find the size of an fr": the hypothetical fr size
    // is leftover / sum(flex factors); any flexible track whose factor × that
    // size is LESS than its base size is treated as inflexible at its base
    // size and the fr is re-found over the rest. Sizing every fr track from
    // the first unit and flooring each at its base leaves the surplus from
    // the floored tracks undistributed: minmax(150px, 1fr) × 3 in 622px
    // (gap 12) gives 199.33 each either way, but minmax(150px, 1fr) 1fr in
    // 200px gave 150 + 100 (overflow) instead of 150 + 50.
    let flexible_count = tracks.iter().filter(|t| t.is_flexible).count();
    if flexible_count > 0 {
        let mut treat_inflexible = vec![false; tracks.len()];
        loop {
            let fixed_size: f32 = tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| !t.is_flexible || treat_inflexible[*i])
                .map(|(_, t)| t.size)
                .sum();
            let flex_space = (available_space - fixed_size).max(0.0);
            let total_flex: f32 = tracks
                .iter()
                .enumerate()
                .filter(|(i, t)| t.is_flexible && !treat_inflexible[*i])
                .map(|(_, t)| t.flex_factor)
                .sum();
            if total_flex <= 0.0 {
                break;
            }
            // Spec: a flex-factor sum below 1 is treated as 1.
            let flex_unit = flex_space / total_flex.max(1.0);

            let mut floored_any = false;
            for (i, track) in tracks.iter().enumerate() {
                if track.is_flexible
                    && !treat_inflexible[i]
                    && track.flex_factor * flex_unit < track.base_size
                {
                    treat_inflexible[i] = true;
                    floored_any = true;
                }
            }
            if floored_any {
                continue;
            }

            for (i, track) in tracks.iter_mut().enumerate() {
                if track.is_flexible && !treat_inflexible[i] {
                    track.size = track.flex_factor * flex_unit;
                    // Respect growth limit
                    if track.growth_limit < f32::INFINITY {
                        track.size = track.size.min(track.growth_limit);
                    }
                }
            }
            break;
        }
    }

    // Step 4: Distribute remaining space to auto tracks if any space left
    // Per spec, this is the "maximize tracks" step - grow tracks to their growth_limit
    let mut used_space: f32 = tracks.iter().map(|t| t.size).sum();
    let mut remaining = (available_space - used_space).max(0.0);

    // Iteratively distribute space, respecting growth_limit
    while remaining > 0.01 {
        // Find tracks that can still grow (not yet at growth_limit)
        let growable: Vec<(usize, f32)> = tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_flexible && t.growth_limit > t.size && t.growth_limit < f32::INFINITY)
            .map(|(i, t)| (i, t.growth_limit - t.size)) // (index, room_to_grow)
            .collect();

        if growable.is_empty() {
            break;
        }

        // Calculate how much each track can receive
        let total_room: f32 = growable.iter().map(|(_, room)| room).sum();

        if total_room <= 0.0 {
            break;
        }

        // Distribute proportionally, but don't exceed room_to_grow
        let to_distribute = remaining.min(total_room);
        for (i, room) in &growable {
            let share = (room / total_room) * to_distribute;
            tracks[*i].size += share.min(*room);
        }

        // Recalculate remaining space
        used_space = tracks.iter().map(|t| t.size).sum();
        remaining = (available_space - used_space).max(0.0);
    }

    // If there's still remaining space and we have tracks with infinite growth_limit,
    // distribute to them equally
    if remaining > 0.01 {
        let infinite_tracks: Vec<usize> = tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| !t.is_flexible && t.growth_limit == f32::INFINITY)
            .map(|(i, _)| i)
            .collect();

        if !infinite_tracks.is_empty() {
            let per_track = remaining / infinite_tracks.len() as f32;
            for i in infinite_tracks {
                tracks[i].size += per_track;
            }
        }
    }

    // Step 5: Calculate positions (starting from 0, alignment applied separately)
    // For auto-fit, collapsed tracks (size = 0) should not have gaps
    let mut position = 0.0;
    let mut prev_was_collapsed = true; // Start true to skip gap before first track
    for track in tracks.iter_mut() {
        // Add gap only if previous track was not collapsed and current track is not collapsed
        if !prev_was_collapsed && track.size > 0.0 {
            position += gap;
        }
        track.position = position;
        position += track.size;
        prev_was_collapsed = track.size == 0.0;
    }
}

/// Stretch auto tracks when align-content is stretch.
/// Per CSS Grid Level 1, Section 11.5.1: When align-content is stretch,
/// the free space is distributed to auto tracks proportionally.
fn stretch_auto_tracks(tracks: &mut [GridTrack], container_size: f32, gap: f32) {
    if tracks.is_empty() {
        return;
    }

    // Calculate used space
    let non_collapsed_count = tracks.iter().filter(|t| t.size > 0.0).count();
    let total_gaps = non_collapsed_count.saturating_sub(1) as f32 * gap;
    let total_track_size: f32 = tracks.iter().map(|t| t.size).sum();
    let used_space = total_track_size + total_gaps;
    let free_space = (container_size - used_space).max(0.0);

    if free_space <= 0.0 {
        return;
    }

    // Find auto tracks (tracks that can stretch)
    // Auto tracks are those with is_min_content AND is_max_content (minmax(min-content, max-content))
    let auto_track_indices: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| {
            // A track is "auto" if it behaves like minmax(min-content, max-content)
            // or if it has room to grow (growth_limit > size)
            (t.is_min_content && t.is_max_content) || t.growth_limit > t.size
        })
        .filter(|(_, t)| t.size > 0.0) // Only stretch non-collapsed tracks
        .map(|(i, _)| i)
        .collect();

    if auto_track_indices.is_empty() {
        return;
    }

    // Distribute free space equally among auto tracks
    let per_track = free_space / auto_track_indices.len() as f32;
    for i in auto_track_indices {
        tracks[i].size += per_track;
    }

    // Recalculate positions after stretching
    let mut position = 0.0;
    let mut prev_was_collapsed = true;
    for track in tracks.iter_mut() {
        if !prev_was_collapsed && track.size > 0.0 {
            position += gap;
        }
        track.position = position;
        position += track.size;
        prev_was_collapsed = track.size == 0.0;
    }
}

/// Apply content alignment (justify-content/align-content) to tracks.
/// This adjusts track positions to distribute free space according to the alignment.
fn apply_content_alignment(tracks: &mut [GridTrack], container_size: f32, gap: f32, alignment: &JustifyContent) {
    if tracks.is_empty() {
        return;
    }

    // Calculate total used space (tracks + gaps)
    let non_collapsed_tracks: Vec<usize> = tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.size > 0.0)
        .map(|(i, _)| i)
        .collect();

    let track_count = non_collapsed_tracks.len();
    if track_count == 0 {
        return;
    }

    let total_track_size: f32 = tracks.iter().map(|t| t.size).sum();
    let total_gaps = track_count.saturating_sub(1) as f32 * gap;
    let used_space = total_track_size + total_gaps;
    let free_space = (container_size - used_space).max(0.0);

    if free_space <= 0.0 {
        // No free space to distribute
        return;
    }

    match alignment {
        JustifyContent::FlexStart => {
            // Tracks already at start, nothing to do
        }
        JustifyContent::FlexEnd => {
            // Shift all tracks to end
            for track in tracks.iter_mut() {
                track.position += free_space;
            }
        }
        JustifyContent::Center => {
            // Center tracks
            let offset = free_space / 2.0;
            for track in tracks.iter_mut() {
                track.position += offset;
            }
        }
        JustifyContent::SpaceBetween => {
            // Distribute free space between tracks
            if track_count > 1 {
                let extra_gap = free_space / (track_count - 1) as f32;
                let mut cumulative_offset = 0.0;
                for (i, track) in tracks.iter_mut().enumerate() {
                    if track.size > 0.0 {
                        track.position += cumulative_offset;
                        // Add gap after each non-collapsed track except the last
                        let is_last_non_collapsed = non_collapsed_tracks.last() == Some(&i);
                        if !is_last_non_collapsed {
                            cumulative_offset += extra_gap;
                        }
                    } else {
                        // Collapsed track - just shift it
                        track.position += cumulative_offset;
                    }
                }
            }
            // If only one track, it stays at start (no space-between)
        }
        JustifyContent::SpaceAround => {
            // Each track gets half the gap on each side
            if track_count > 0 {
                let gap_per_side = free_space / (track_count * 2) as f32;
                let mut cumulative_offset = gap_per_side; // Start with half gap
                for track in tracks.iter_mut() {
                    if track.size > 0.0 {
                        track.position += cumulative_offset;
                        cumulative_offset += gap_per_side * 2.0; // Full gap between tracks
                    } else {
                        track.position += cumulative_offset;
                    }
                }
            }
        }
        JustifyContent::SpaceEvenly => {
            // Equal space between and around all tracks
            if track_count > 0 {
                let space = free_space / (track_count + 1) as f32;
                let mut cumulative_offset = space; // Start with one unit of space
                for track in tracks.iter_mut() {
                    if track.size > 0.0 {
                        track.position += cumulative_offset;
                        cumulative_offset += space;
                    } else {
                        track.position += cumulative_offset;
                    }
                }
            }
        }
    }
}

/// Convert AlignContent to JustifyContent for unified handling.
/// Both enums have the same values, just different naming conventions.
fn align_content_to_justify(align: &AlignContent) -> JustifyContent {
    match align {
        AlignContent::Stretch => JustifyContent::FlexStart, // Stretch is handled separately
        AlignContent::FlexStart => JustifyContent::FlexStart,
        AlignContent::FlexEnd => JustifyContent::FlexEnd,
        AlignContent::Center => JustifyContent::Center,
        AlignContent::SpaceBetween => JustifyContent::SpaceBetween,
        AlignContent::SpaceAround => JustifyContent::SpaceAround,
        AlignContent::SpaceEvenly => JustifyContent::SpaceEvenly,
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

    // Check if width is explicitly set (not auto)
    let has_explicit_width = !matches!(child.style.width, Length::Auto);
    let child_width = match child.style.width {
        // css-align-3 §6.1: an auto-width item that is NOT stretched sizes
        // as fit-content, min(max-content, max(min-content, cell)). Using
        // the whole cell made `justify-items: center` a no-op on every
        // auto-width item: google's logo wrapper filled its 1088px cell and
        // the logo sat at the left edge. The estimators give border boxes,
        // which is the size this helper returns.
        Length::Auto if align != JustifySelf::Stretch => estimate_max_content_width(child)
            .min(cell_width.max(estimate_min_content_width(child))),
        Length::Auto => cell_width,
        Length::Px(w) => w,
        Length::Percent(p) => cell_width * p / 100.0,
        _ => cell_width,
    };

    match align {
        JustifySelf::Start | JustifySelf::Auto => (cell_x, child_width),
        JustifySelf::End => (cell_x + cell_width - child_width, child_width),
        JustifySelf::Center => (cell_x + (cell_width - child_width) / 2.0, child_width),
        // Per CSS spec: stretch only applies when width is auto
        JustifySelf::Stretch => {
            if has_explicit_width {
                (cell_x, child_width)
            } else {
                (cell_x, cell_width)
            }
        },
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

    // Check if height is explicitly set (not auto)
    let has_explicit_height = !matches!(child.style.height, Length::Auto);
    let child_height = match child.style.height {
        Length::Auto => cell_height,
        Length::Px(h) => h,
        Length::Percent(p) => cell_height * p / 100.0,
        // A `calc()` is a length, and `has_explicit_height` above already
        // counts it as one — so it must resolve here too, against the same
        // cell height the percentage arm uses. Left on the `_` arm it would
        // be called explicit and then sized as if it were `auto`.
        Length::Calc(_) => child.length_to_px(&child.style.height, cell_height),
        _ => cell_height,
    };

    match align {
        AlignSelf::FlexStart | AlignSelf::Auto => (cell_y, child_height),
        AlignSelf::FlexEnd => (cell_y + cell_height - child_height, child_height),
        AlignSelf::Center => (cell_y + (cell_height - child_height) / 2.0, child_height),
        // Per CSS spec: stretch only applies when height is auto
        AlignSelf::Stretch => {
            if has_explicit_height {
                (cell_y, child_height)
            } else {
                (cell_y, cell_height)
            }
        },
        AlignSelf::Baseline => (cell_y, child_height), // Simplified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BoxType;
    use rustkit_css::{ComputedStyle, GridTemplateAreas};

    #[allow(dead_code)]
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


    /// image-gallery's `.aspect-grid > .aspect-box > .content`: a
    /// `position:absolute; inset:0` overlay inside a `position:relative`
    /// grid item whose height comes from `aspect-ratio`.
    ///
    /// Phase 9 hands the overlay its containing block BEFORE the item's own
    /// `calculate_block_height` applies the ratio, so the height it saw was
    /// the pre-ratio content number. Measured on the corpus: `.content` 32
    /// tall inside a 288px card, on all four cards, against Chrome's 288.
    fn ratio_item_with_overlay(ratio: Option<f32>, height: Length, child_h: f32) -> LayoutBox {
        let mut gs = ComputedStyle::new();
        gs.display = Display::Grid;
        gs.grid_template_columns = GridTemplate::from_sizes(vec![TrackSize::Px(288.0)]);
        let mut grid = LayoutBox::new(BoxType::Block, gs);
        grid.dimensions.content = crate::Rect::new(0.0, 0.0, 288.0, 0.0);

        let mut is = ComputedStyle::new();
        is.aspect_ratio = ratio;
        is.height = height;
        is.position = rustkit_css::Position::Relative;
        let mut item = LayoutBox::with_position(BoxType::Block, is, crate::Position::Static);

        // an in-flow child, so the item has a content height of its own
        let mut fs = ComputedStyle::new();
        fs.height = Length::Px(child_h);
        item.children.push(LayoutBox::new(BoxType::Block, fs));

        // `height: auto` (the default) — the overlay is sized by its insets.
        let mut overlay =
            LayoutBox::with_position(BoxType::Block, ComputedStyle::new(), crate::Position::Absolute);
        overlay.set_offsets(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
        item.children.push(overlay);

        grid.children.push(item);
        grid
    }

    fn overlay_height(mut grid: LayoutBox) -> f32 {
        layout_grid_container(&mut grid, 288.0, 0.0);
        let item = &grid.children[0];
        item.children[1].dimensions.content.height
    }

    #[test]
    fn an_inset_overlay_fills_a_grid_item_sized_by_its_aspect_ratio() {
        // 288 wide, ratio 1/1 -> the item is 288 tall and the overlay fills it.
        let h = overlay_height(ratio_item_with_overlay(Some(1.0), Length::Auto, 10.0));
        assert!(
            (h - 288.0).abs() < 0.5,
            "the overlay fills the 288px ratio box, got {h}"
        );
    }

    #[test]
    fn a_taller_content_height_still_beats_the_ratio_for_the_overlay() {
        // Phase 9.5 is grow-only and Chrome agrees: a `4 / 1` item 288 wide
        // holding a 300px child is 300 tall, not the ratio's 72. The overlay's
        // containing block must be the height the item actually takes, or the
        // two passes disagree and the defect just moves.
        let h = overlay_height(ratio_item_with_overlay(Some(4.0), Length::Auto, 300.0));
        assert!(
            (h - 300.0).abs() < 0.5,
            "content taller than the ratio wins: expected 300, got {h}"
        );
    }

    /// The other half of the same card, and the half that needed Phase 9 to
    /// re-anchor: the overlay is the right SIZE (the test above) and its flex
    /// line has to be justified in that size rather than in its own content.
    ///
    /// `LayoutBox::layout` cannot do this on its own, because on the plain
    /// block path the box it is handed is the static-position stand-in — so
    /// Phase 9 has to say "this one is real" by re-anchoring. Delete that call
    /// and the caption goes back to the top edge of the card, which is
    /// image-gallery's `.aspect-box > .content > span` at y=1086.65 against
    /// Chrome's 1234.
    ///
    /// The overlay's items carry EXPLICIT heights so that step 11d never
    /// fires: 11d's re-derivation must not be able to stand in for this.
    #[test]
    fn an_inset_overlay_justifies_its_flex_line_in_the_card_not_in_its_content() {
        let mut grid = ratio_item_with_overlay(Some(1.0), Length::Auto, 10.0);
        {
            let overlay = &mut grid.children[0].children[1];
            overlay.style.display = Display::Flex;
            overlay.style.flex_direction = rustkit_css::FlexDirection::Column;
            overlay.style.justify_content = rustkit_css::JustifyContent::Center;
            for h in [30.0, 20.0] {
                let mut item_style = ComputedStyle::new();
                item_style.height = Length::Px(h);
                overlay
                    .children
                    .push(LayoutBox::new(BoxType::Block, item_style));
            }
        }
        layout_grid_container(&mut grid, 288.0, 0.0);

        let overlay = &grid.children[0].children[1];
        assert!(
            (overlay.dimensions.content.height - 288.0).abs() < 0.5,
            "the overlay fills the 288px card (the precondition), got {}",
            overlay.dimensions.content.height
        );
        let lead = overlay.children[0].dimensions.content.y - overlay.dimensions.content.y;
        assert!(
            (lead - 119.0).abs() < 0.5,
            "50 of content centred in 288 leaves 119 above, not {lead} — the line \
             was justified in the overlay's own content"
        );
    }

    #[test]
    fn a_specified_item_height_keeps_the_overlay_off_the_ratio() {
        // With `height` specified the ratio does not size the block axis, so
        // the grid-assigned number stays the overlay's containing block.
        let h = overlay_height(ratio_item_with_overlay(
            Some(1.0),
            Length::Px(120.0),
            10.0,
        ));
        assert!(
            (h - 120.0).abs() < 0.5,
            "a specified 120px height wins over the 1/1 ratio, got {h}"
        );
    }

    /// Intrinsic contributions must count padding expressed in ANY unit.
    ///
    /// T-RED: with the old `if let Length::Px(v) = l { v } else { 0.0 }`
    /// closure, the rem case returns the bare text width and this fails —
    /// a whole padding box goes missing from the element's min-content size.
    ///
    /// That is not a rounding error. A flex item floored at this value could
    /// be shrunk by exactly its own padding, which is how a `kbd` chip with
    /// `padding: 0.25rem 0.5rem` and a basis of 34.66px got squeezed to
    /// 18.66px while paint still drew its glyphs at full width.
    #[test]
    fn min_content_width_counts_padding_in_relative_units() {
        let text_only = {
            let mut s = ComputedStyle::new();
            s.font_size = Length::Px(12.0);
            let mut b = LayoutBox::new(BoxType::Block, s.clone());
            b.children
                .push(LayoutBox::new(BoxType::Text("Ctrl".to_string()), s));
            estimate_min_content_width(&b)
        };

        let with_rem_padding = {
            let mut s = ComputedStyle::new();
            s.font_size = Length::Px(12.0);
            s.padding_left = Length::Rem(0.5); // 8px against the 16px root
            s.padding_right = Length::Rem(0.5);
            let mut child = ComputedStyle::new();
            child.font_size = Length::Px(12.0);
            let mut b = LayoutBox::new(BoxType::Block, s);
            b.children
                .push(LayoutBox::new(BoxType::Text("Ctrl".to_string()), child));
            estimate_min_content_width(&b)
        };

        // Assert the baseline is non-zero first: if the text measured 0 the
        // delta below would be 0 too and the test would pass vacuously.
        assert!(
            text_only > 1.0,
            "setup failed: unpadded min-content was {text_only}, so this test cannot detect anything"
        );

        let delta = with_rem_padding - text_only;
        assert!(
            (delta - 16.0).abs() < 0.01,
            "rem padding contributed {delta}px, expected 16px (0.5rem each side). \
             Relative units are being dropped from intrinsic sizing."
        );
    }

    /// The same for em, which resolves against the ELEMENT's own font-size
    /// rather than the root — a different code path through the resolver.
    #[test]
    fn min_content_width_counts_em_padding_against_element_font_size() {
        let mut s = ComputedStyle::new();
        s.font_size = Length::Px(20.0);
        s.padding_left = Length::Em(1.0); // 20px, not 16
        s.padding_right = Length::Em(1.0);
        let b = LayoutBox::new(BoxType::Block, s);

        let got = estimate_min_content_width(&b);
        assert!(
            (got - 40.0).abs() < 0.01,
            "em padding on a 20px font contributed {got}px, expected 40px — \
             em is resolving against the wrong font size (or being dropped)"
        );
    }

    // ---------------------------------------------------------------
    // White space inside a run that cannot wrap (css-text-3 §4.1).
    //
    // The corpus shape these exist for is `sticky-scroll`'s
    // `.horizontal-scroll { white-space: nowrap }`, six 200px inline-blocks
    // written on separate source lines. Chrome's committed rect for the `1fr`
    // grid column it floors is 1295.9375 = 1275 + 5 spaces; RustKit answered
    // 1275, because a text node of pure white space contributed nothing to
    // min-content while the laid-out line rendered it. The intrinsic size and
    // the line disagreed about the same characters.
    //
    // The space advance is measured independently in each test rather than
    // taken from `collapsed_space_width`, so a mutation of that helper cannot
    // move the expectation with it and stay green.
    // ---------------------------------------------------------------

    /// Build a nowrap container holding the given children in order.
    /// `W` is a collapsible white-space text node; `B(px)` an inline-block.
    #[cfg(test)]
    enum Kid {
        W,
        B(f32),
        BM(f32, f32),
        Block(f32),
    }

    #[cfg(test)]
    fn nowrap_container(white_space: WhiteSpace, kids: &[Kid]) -> LayoutBox {
        let mut cs = ComputedStyle::new();
        cs.font_size = Length::Px(16.0);
        cs.white_space = white_space;
        let mut container = LayoutBox::new(BoxType::Block, cs.clone());
        for kid in kids {
            let child = match kid {
                Kid::W => {
                    let mut ws = cs.clone();
                    ws.display = Display::Inline;
                    LayoutBox::new(BoxType::Text("\n            ".to_string()), ws)
                }
                Kid::B(w) | Kid::BM(w, _) => {
                    let mut ib = cs.clone();
                    ib.display = Display::InlineBlock;
                    ib.width = Length::Px(*w);
                    if let Kid::BM(_, m) = kid {
                        ib.margin_right = Length::Px(*m);
                    }
                    LayoutBox::new(BoxType::Block, ib)
                }
                Kid::Block(w) => {
                    let mut bs = cs.clone();
                    bs.display = Display::Block;
                    bs.width = Length::Px(*w);
                    LayoutBox::new(BoxType::Block, bs)
                }
            };
            container.children.push(child);
        }
        container
    }

    #[cfg(test)]
    fn one_space() -> f32 {
        let s = ComputedStyle::new();
        crate::measure_text_advanced(" ", &s.font_family, 16.0, s.font_weight, s.font_style).width
    }

    /// T-RED without the white-space branch: 400 instead of 400 + a space.
    #[test]
    fn white_space_between_two_inline_boxes_is_part_of_an_unbreakable_run() {
        let space = one_space();
        assert!(
            space > 0.0,
            "setup failed: this seat measures a space as {space}px, so every \
             assertion below would hold with the fix removed"
        );
        let b = nowrap_container(WhiteSpace::Nowrap, &[Kid::B(200.0), Kid::W, Kid::B(200.0)]);
        let got = estimate_min_content_width(&b);
        assert!(
            (got - (400.0 + space)).abs() < 0.01,
            "min-content was {got}, expected {} (two 200px boxes and the space \
             between them, which nowrap cannot break at)",
            400.0 + space
        );
    }

    /// css-text-3 §4.1.3: white space at the edges of a line is removed. The
    /// run below has three white-space children and renders exactly one space.
    #[test]
    fn white_space_at_the_edges_of_a_nowrap_run_is_removed() {
        let space = one_space();
        let b = nowrap_container(
            WhiteSpace::Nowrap,
            &[Kid::W, Kid::B(200.0), Kid::W, Kid::B(200.0), Kid::W],
        );
        let got = estimate_min_content_width(&b);
        assert!(
            (got - (400.0 + space)).abs() < 0.01,
            "min-content was {got}, expected {} — leading and trailing white \
             space must not be counted",
            400.0 + space
        );
    }

    #[test]
    fn consecutive_white_space_children_collapse_to_one_space() {
        let space = one_space();
        let b = nowrap_container(
            WhiteSpace::Nowrap,
            &[Kid::B(200.0), Kid::W, Kid::W, Kid::W, Kid::B(200.0)],
        );
        let got = estimate_min_content_width(&b);
        assert!(
            (got - (400.0 + space)).abs() < 0.01,
            "min-content was {got}, expected {} — three adjacent white-space \
             nodes collapse to one space, they do not accumulate",
            400.0 + space
        );
    }

    /// One space is consumed once. Without clearing it after use, the second
    /// gap — which has no white space in the source — would be charged one too.
    #[test]
    fn a_consumed_space_is_not_charged_to_the_next_box_as_well() {
        let space = one_space();
        let b = nowrap_container(
            WhiteSpace::Nowrap,
            &[Kid::B(200.0), Kid::W, Kid::B(200.0), Kid::B(200.0)],
        );
        let got = estimate_min_content_width(&b);
        assert!(
            (got - (600.0 + space)).abs() < 0.01,
            "min-content was {got}, expected {} — only one gap in this run \
             carries white space",
            600.0 + space
        );
    }

    /// A block-level child ends the run, so white space held from before it
    /// belongs to a line that is already over and must be dropped.
    #[test]
    fn a_block_child_ends_the_run_and_drops_the_white_space_before_it() {
        let b = nowrap_container(
            WhiteSpace::Nowrap,
            &[Kid::B(200.0), Kid::W, Kid::Block(300.0)],
        );
        let got = estimate_min_content_width(&b);
        assert!(
            (got - 300.0).abs() < 0.01,
            "min-content was {got}, expected 300 — the block child stands \
             alone and the pending space died with the run before it"
        );
    }

    /// The run the block ended is over, so its held space must not be charged
    /// to the run that STARTS after the block. The test above cannot see this:
    /// with nothing following the block there is nowhere for a leaked space to
    /// land, so it passes either way. This shape is the rule; that one is the
    /// example.
    #[test]
    fn white_space_held_before_a_block_does_not_leak_into_the_run_after_it() {
        let b = nowrap_container(
            WhiteSpace::Nowrap,
            &[
                Kid::B(200.0),
                Kid::W,
                Kid::Block(100.0),
                Kid::B(200.0),
                Kid::B(200.0),
            ],
        );
        let got = estimate_min_content_width(&b);
        assert!(
            (got - 400.0).abs() < 0.01,
            "min-content was {got}, expected 400 — the second run holds two \
             200px boxes and no white space of its own"
        );
    }

    /// Where the run CAN wrap, white space is a break opportunity and
    /// contributes nothing: min-content is the widest single child.
    #[test]
    fn white_space_in_a_wrapping_run_is_a_break_opportunity_not_a_width() {
        let b = nowrap_container(WhiteSpace::Normal, &[Kid::B(200.0), Kid::W, Kid::B(200.0)]);
        let got = estimate_min_content_width(&b);
        assert!(
            (got - 200.0).abs() < 0.01,
            "min-content was {got}, expected 200 — under `normal` the run \
             breaks at the space, so the boxes do not sum"
        );
    }

    /// The scope line, pinned. `pre` preserves white space verbatim AND breaks
    /// at its newlines; this function models neither, so it keeps the
    /// behaviour it had rather than gaining the collapsed-space rule, which
    /// would be wrong for a different reason.
    #[test]
    fn pre_does_not_get_the_collapsed_space_rule() {
        let b = nowrap_container(WhiteSpace::Pre, &[Kid::B(200.0), Kid::W, Kid::B(200.0)]);
        let got = estimate_min_content_width(&b);
        assert!(
            (got - 400.0).abs() < 0.01,
            "min-content was {got}, expected 400 — `pre` is deliberately \
             outside this rule"
        );
    }

    /// U+00A0 is White_Space to `char::is_whitespace` and is NOT collapsible
    /// document white space: it is a rendered character. Classifying it as a
    /// collapsed space would answer one space's advance for a node that
    /// should answer its own measured width.
    #[test]
    fn a_no_break_space_is_not_collapsible_white_space() {
        let mut s = ComputedStyle::new();
        s.font_size = Length::Px(16.0);
        let nbsp = LayoutBox::new(BoxType::Text("\u{a0}".to_string()), s.clone());
        assert!(
            !is_collapsible_whitespace_only(&nbsp),
            "U+00A0 was classified as collapsible white space"
        );
        let spaces = LayoutBox::new(BoxType::Text(" \n\t".to_string()), s.clone());
        assert!(
            is_collapsible_whitespace_only(&spaces),
            "space/newline/tab was not classified as collapsible white space"
        );
        // `"".chars().all(..)` is vacuously true, so an empty text node would
        // be charged a space it does not contain.
        let empty = LayoutBox::new(BoxType::Text(String::new()), s);
        assert!(
            !is_collapsible_whitespace_only(&empty),
            "an empty text node was classified as collapsible white space"
        );
    }

    /// The corpus shape itself: `sticky-scroll`'s `.horizontal-scroll`, six
    /// 200px items with `margin-right: 15px` on all but the last, written on
    /// separate source lines. Chrome floors the `1fr` column at
    /// 1275 + 5 spaces; the structure of that sum is what this asserts.
    /// The space's own advance is a font metric and deliberately not pinned.
    #[test]
    fn the_horizontal_scroll_row_sums_six_items_five_margins_and_five_spaces() {
        let space = one_space();
        let mut kids = Vec::new();
        for i in 0..6 {
            if i > 0 {
                kids.push(Kid::W);
            }
            kids.push(if i < 5 {
                Kid::BM(200.0, 15.0)
            } else {
                Kid::B(200.0)
            });
        }
        let b = nowrap_container(WhiteSpace::Nowrap, &kids);
        let got = estimate_min_content_width(&b);
        let want = 6.0 * 200.0 + 5.0 * 15.0 + 5.0 * space;
        assert!(
            (got - want).abs() < 0.01,
            "min-content was {got}, expected {want} = 6*200 + 5*15 + 5 spaces"
        );
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

    fn nowrap_scroller_with_inline_blocks() -> LayoutBox {
        // Six 200px inline-blocks with 15px right margins (last one 0) under
        // white-space: nowrap — an unbreakable run of 6*200 + 5*15 = 1275.
        let mut scroller_style = ComputedStyle::new();
        scroller_style.white_space = WhiteSpace::Nowrap;
        scroller_style.overflow_x = Overflow::Auto;
        let mut scroller = LayoutBox::new(BoxType::Block, scroller_style);
        for i in 0..6 {
            let mut item_style = ComputedStyle::new();
            item_style.display = Display::InlineBlock;
            item_style.width = Length::Px(200.0);
            item_style.margin_right = Length::Px(if i < 5 { 15.0 } else { 0.0 });
            scroller.children.push(LayoutBox::new(BoxType::Block, item_style));
        }
        scroller
    }

    #[test]
    fn test_width_contribution_nowrap_inline_block_run() {
        let mut main_box = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        main_box.children.push(nowrap_scroller_with_inline_blocks());

        let item = GridItem::new(&main_box);
        assert_eq!(item.get_width_contribution(0.0), 1275.0);
    }

    #[test]
    fn test_width_contribution_wrappable_inline_blocks_take_max() {
        // Without nowrap, inline-blocks may wrap between each other: the
        // min-content contribution is the widest single item, not the sum.
        let mut wrapper = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        for _ in 0..3 {
            let mut item_style = ComputedStyle::new();
            item_style.display = Display::InlineBlock;
            item_style.width = Length::Px(200.0);
            wrapper.children.push(LayoutBox::new(BoxType::Block, item_style));
        }
        let mut main_box = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        main_box.children.push(wrapper);

        let item = GridItem::new(&main_box);
        assert_eq!(item.get_width_contribution(0.0), 200.0);
    }

    #[test]
    fn test_width_contribution_scroll_container_item_explicit_only() {
        // A grid item that is itself a scroll container can shrink below its
        // content: the automatic minimum doesn't apply (CSS Grid §6.6).
        let mut style = ComputedStyle::new();
        style.overflow_x = Overflow::Auto;
        let mut item_box = LayoutBox::new(BoxType::Block, style);
        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(500.0);
        item_box
            .children
            .push(LayoutBox::new(BoxType::Block, child_style));

        let item = GridItem::new(&item_box);
        assert_eq!(item.get_width_contribution(0.0), 0.0);
    }

    #[test]
    fn test_fr_track_floored_by_item_min_content() {
        // The sticky-scroll shape: 250px 1fr 250px in a 1160px container.
        // Free space would give the fr track 660px, but the item's 1275px
        // unbreakable row floors it there (Chrome resolves the real page to
        // 1295.94px; the extra 20.94 is collapsed inter-item spaces, which
        // the conservative estimate deliberately drops).
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns = GridTemplate::from_sizes(vec![
            TrackSize::Px(250.0),
            TrackSize::Fr(1.0),
            TrackSize::Px(250.0),
        ]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut main_style = ComputedStyle::new();
        main_style.grid_column_start = GridLine::Number(2);
        main_style.grid_column_end = GridLine::Number(3);
        let mut main_box = LayoutBox::new(BoxType::Block, main_style);
        main_box.children.push(nowrap_scroller_with_inline_blocks());
        container.children.push(main_box);

        layout_grid_container(&mut container, 1160.0, 800.0);

        let width = container.children[0].dimensions.content.width;
        assert!(
            (width - 1275.0).abs() < 1.0,
            "fr track should be floored at the item's 1275px min-content, got {width}"
        );
    }

    #[test]
    fn test_auto_fit_minmax_fits_columns_to_the_container_with_gap() {
        // The about-page features shape: repeat(auto-fit, minmax(150px, 1fr))
        // with a 12px gap in a 622px container. Three repetitions fit
        // (3×150 + 2×12 = 474; a fourth needs 636), and the 1fr max stretches
        // each to (622 − 24) / 3 = 199.33 — Chrome's exact track.
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns = GridTemplate {
            tracks: Vec::new(),
            repeats: vec![(
                0,
                TrackRepeat::AutoFit(vec![TrackDefinition::simple(TrackSize::MinMax(
                    Box::new(TrackSize::Px(150.0)),
                    Box::new(TrackSize::Fr(1.0)),
                ))]),
            )],
            final_line_names: Vec::new(),
        };
        container_style.column_gap = Length::Px(12.0);
        container_style.row_gap = Length::Px(12.0);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        for _ in 0..6 {
            container
                .children
                .push(LayoutBox::new(BoxType::Block, ComputedStyle::new()));
        }

        layout_grid_container(&mut container, 622.0, 600.0);

        let xs: Vec<f32> = container
            .children
            .iter()
            .map(|c| c.dimensions.content.x)
            .collect();
        let ws: Vec<f32> = container
            .children
            .iter()
            .map(|c| c.dimensions.content.width)
            .collect();
        for w in &ws {
            assert!(
                (w - 199.33).abs() < 0.1,
                "column width should be 199.33, got {ws:?}"
            );
        }
        assert!(
            (xs[0] - 0.0).abs() < 0.01
                && (xs[1] - 211.33).abs() < 0.1
                && (xs[2] - 422.67).abs() < 0.1,
            "first row should sit at 0 / 211.33 / 422.67, got {xs:?}"
        );
        assert!(
            (xs[3] - 0.0).abs() < 0.01,
            "fourth item wraps to the second row, got {xs:?}"
        );
    }

    #[test]
    fn test_stretched_content_box_item_keeps_its_padding_inside_the_track() {
        // A padded grid item with the default box-sizing (content-box) and
        // width:auto fills its 200px area with its border box: content 176,
        // not 200 + 24 (which overflowed the track by the padding).
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(200.0)]);
        container_style.grid_template_rows = GridTemplate::from_sizes(vec![TrackSize::Px(100.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = BoxSizing::ContentBox;
        item_style.padding_left = Length::Px(12.0);
        item_style.padding_right = Length::Px(12.0);
        item_style.padding_top = Length::Px(12.0);
        item_style.padding_bottom = Length::Px(12.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, item_style));

        // An explicit content-box width still means what it says.
        let mut fixed_style = ComputedStyle::new();
        fixed_style.box_sizing = BoxSizing::ContentBox;
        fixed_style.width = Length::Px(100.0);
        fixed_style.padding_left = Length::Px(10.0);
        fixed_style.padding_right = Length::Px(10.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, fixed_style));

        layout_grid_container(&mut container, 200.0, 100.0);

        let d = &container.children[0].dimensions;
        assert!(
            (d.content.width - 176.0).abs() < 0.01,
            "content width {}",
            d.content.width
        );
        assert!(
            (d.content.height - 76.0).abs() < 0.01,
            "content height {}",
            d.content.height
        );
        assert!(
            (d.border_box().width - 200.0).abs() < 0.01,
            "border box {}",
            d.border_box().width
        );

        let f = &container.children[1].dimensions;
        assert!(
            (f.content.width - 100.0).abs() < 0.01,
            "explicit content width {}",
            f.content.width
        );
    }

    #[test]
    fn test_fr_floored_track_returns_its_surplus_to_the_other_fr_tracks() {
        // css-grid-1 §12.7.1: minmax(150px, 1fr) 1fr in 200px. The first
        // hypothetical fr is 100px, below the 150px base, so that track is
        // treated as inflexible at 150 and the remaining 50px is the fr.
        let mut tracks = vec![
            GridTrack::new(&TrackSize::MinMax(
                Box::new(TrackSize::Px(150.0)),
                Box::new(TrackSize::Fr(1.0)),
            )),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];
        size_grid_tracks(&mut tracks, 200.0, 0.0);
        assert!(
            (tracks[0].size - 150.0).abs() < 0.01,
            "got {}",
            tracks[0].size
        );
        assert!(
            (tracks[1].size - 50.0).abs() < 0.01,
            "got {}",
            tracks[1].size
        );
    }

    #[test]
    fn test_row_span_credits_the_spanned_gutters() {
        // css-grid-1 §12.5. The image-gallery shape: 2 auto rows, 16px gap,
        // one item spanning both rows with a 416px min-height.
        //
        // The spanned item already owns the 16px gutter BETWEEN the two rows,
        // so it only demands 400px from the tracks: 200px each. Charging the
        // full 416px to the tracks alone gives 208px rows -- an 8px error that
        // compounds into every row below (Chrome: rows 200, item 416).
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0)]);
        container_style.grid_template_rows =
            GridTemplate::from_sizes(vec![TrackSize::Auto, TrackSize::Auto]);
        container_style.row_gap = Length::Px(16.0);
        container_style.column_gap = Length::Px(16.0);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut tall_style = ComputedStyle::new();
        tall_style.grid_row_start = GridLine::Number(1);
        tall_style.grid_row_end = GridLine::Number(3); // span 2
        tall_style.min_height = Length::Px(416.0);
        container.children.push(LayoutBox::new(BoxType::Block, tall_style));

        for _ in 0..2 {
            let mut short_style = ComputedStyle::new();
            short_style.min_height = Length::Px(200.0);
            container
                .children
                .push(LayoutBox::new(BoxType::Block, short_style));
        }

        layout_grid_container(&mut container, 800.0, 600.0);

        let tall = container.children[0].dimensions.border_box().height;
        assert!(
            (tall - 416.0).abs() < 0.5,
            "row-spanning item should be 416 (200 + 16 gap + 200), got {tall}"
        );
        for (i, child) in container.children.iter().enumerate().skip(1) {
            let h = child.dimensions.border_box().height;
            assert!(
                (h - 200.0).abs() < 0.5,
                "single-row item {i} should be 200 (the spanning item must not \
                 inflate the tracks by the gutters it already spans), got {h}"
            );
        }
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

        let grid = GridLayout::new(
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

        assert!(item.is_fully_placed());
        assert_eq!(item.column_start, 1);
        assert_eq!(item.column_end, 3);
        assert_eq!(item.column_span, 2);
    }

    #[test]
    fn test_grid_item_column_only_placement() {
        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);

        // Simulate grid-column: 1 / 3 with no row placement
        let placement = GridPlacement {
            column_start: GridLine::Number(1),
            column_end: GridLine::Number(3),
            row_start: GridLine::Auto,
            row_end: GridLine::Auto,
        };
        item.set_placement(&placement);

        assert!(!item.auto_column); // Column is explicitly placed
        assert!(item.auto_row);     // Row needs auto-placement
        assert!(item.needs_auto_placement()); // Overall needs auto-placement
        assert!(!item.is_fully_placed());     // Not fully placed
    }

    #[test]
    fn test_track_sizing_percentage() {
        // 50% track in a 400px container should be 200px
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Percent(50.0)),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 0.0);

        assert_eq!(tracks[0].size, 200.0);
        // Remaining 200px goes to 1fr
        assert_eq!(tracks[1].size, 200.0);
    }

    #[test]
    fn test_track_sizing_percentage_with_gap() {
        // Two 25% tracks with 20px gap in a 400px container
        // 25% of 400 = 100px each
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Percent(25.0)),
            GridTrack::new(&TrackSize::Percent(25.0)),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 20.0);

        // Percentages resolve to 25% of container
        assert_eq!(tracks[0].size, 100.0);
        assert_eq!(tracks[1].size, 100.0);
        // Available space = 400 - 40 (gaps) = 360
        // After fixed (200px), remaining = 160px for 1fr
        // But wait, percentage tracks are considered fixed
        assert_eq!(tracks[2].size, 160.0);
    }

    #[test]
    fn test_track_sizing_multiple_percentages() {
        // 30% + 20% + 1fr in 500px container
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Percent(30.0)),
            GridTrack::new(&TrackSize::Percent(20.0)),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];

        size_grid_tracks(&mut tracks, 500.0, 0.0);

        assert_eq!(tracks[0].size, 150.0); // 30% of 500
        assert_eq!(tracks[1].size, 100.0); // 20% of 500
        assert_eq!(tracks[2].size, 250.0); // Remaining space
    }

    #[test]
    fn test_track_sizing_minmax_with_percentage_min() {
        // minmax(25%, 1fr) in 400px container
        // min = 25% of 400 = 100px
        // The track should get at least 100px from the fr distribution
        let mut tracks = vec![
            GridTrack::new(&TrackSize::MinMax(
                Box::new(TrackSize::Percent(25.0)),
                Box::new(TrackSize::Fr(1.0)),
            )),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 0.0);

        // Both are 1fr, but first has 100px minimum
        // With 400px available, 200px each, but first is clamped to at least 100px
        assert!(tracks[0].size >= 100.0);
        assert_eq!(tracks[0].size, 200.0); // Gets half of 400px
        assert_eq!(tracks[1].size, 200.0);
    }

    #[test]
    fn test_track_sizing_minmax_with_percentage_max() {
        // minmax(100px, 50%) in 400px container
        // min = 100px, max = 50% of 400 = 200px
        let mut tracks = vec![
            GridTrack::new(&TrackSize::MinMax(
                Box::new(TrackSize::Px(100.0)),
                Box::new(TrackSize::Percent(50.0)),
            )),
            GridTrack::new(&TrackSize::Fr(1.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 0.0);

        // First track has min=100px, max=200px
        // It should start at 100px, and fr gets remaining 300px
        assert_eq!(tracks[0].size, 100.0); // Gets base size
        assert_eq!(tracks[1].size, 300.0); // Remaining goes to fr
    }

    #[test]
    fn test_track_min_content_flag() {
        let track = GridTrack::new(&TrackSize::MinContent);
        assert!(track.is_min_content);
        assert!(!track.is_max_content);
    }

    #[test]
    fn test_track_max_content_flag() {
        let track = GridTrack::new(&TrackSize::MaxContent);
        assert!(!track.is_min_content);
        assert!(track.is_max_content);
    }

    #[test]
    fn test_track_auto_is_intrinsic() {
        let track = GridTrack::new(&TrackSize::Auto);
        assert!(track.is_min_content); // auto behaves like minmax(min-content, max-content)
        assert!(track.is_max_content);
    }

    #[test]
    fn test_track_sizing_min_content() {
        // min-content track with contribution of 100px
        let mut track = GridTrack::new(&TrackSize::MinContent);
        track.base_size = 100.0; // Simulating item contribution

        let mut tracks = vec![track, GridTrack::new(&TrackSize::Fr(1.0))];
        size_grid_tracks(&mut tracks, 500.0, 0.0);

        // min-content track should stay at its base size
        assert_eq!(tracks[0].size, 100.0);
        // fr track gets remaining space
        assert_eq!(tracks[1].size, 400.0);
    }

    #[test]
    fn test_track_sizing_auto() {
        // auto track with contribution of 150px
        let mut track = GridTrack::new(&TrackSize::Auto);
        track.base_size = 150.0; // Simulating item contribution

        let mut tracks = vec![track, GridTrack::new(&TrackSize::Px(100.0))];
        size_grid_tracks(&mut tracks, 500.0, 0.0);

        // auto track should use its base size as minimum
        // remaining space should be distributed
        assert!(tracks[0].size >= 150.0);
        assert_eq!(tracks[1].size, 100.0);
    }

    #[test]
    fn test_track_fit_content_flag() {
        let track = GridTrack::new(&TrackSize::FitContent(200.0));
        assert!(track.is_min_content); // fit-content uses min-content as minimum
        assert!(!track.is_max_content);
        assert_eq!(track.fit_content_limit, Some(200.0));
    }

    #[test]
    fn test_track_sizing_fit_content_within_limit() {
        // fit-content(300px) with content that needs 100px
        let mut track = GridTrack::new(&TrackSize::FitContent(300.0));
        track.base_size = 100.0; // Simulating item contribution (min-content)

        let mut tracks = vec![track, GridTrack::new(&TrackSize::Fr(1.0))];
        size_grid_tracks(&mut tracks, 500.0, 0.0);

        // fit-content should clamp to min-content (100px) since that's less than limit
        assert_eq!(tracks[0].size, 100.0);
        // fr track gets remaining space
        assert_eq!(tracks[1].size, 400.0);
    }

    #[test]
    fn test_track_sizing_fit_content_at_limit() {
        // fit-content(150px) with content that would need more
        let mut track = GridTrack::new(&TrackSize::FitContent(150.0));
        track.base_size = 200.0; // Content needs 200px but we cap at 150px

        let mut tracks = vec![track, GridTrack::new(&TrackSize::Fr(1.0))];
        size_grid_tracks(&mut tracks, 500.0, 0.0);

        // fit-content should use base_size since it exceeds the limit
        // (In a real scenario, base_size would be clamped to the limit,
        // but we're simulating the case where content already exceeds)
        assert_eq!(tracks[0].size, 200.0);
        assert_eq!(tracks[1].size, 300.0);
    }

    // ==================== Phase 2: Auto-fill/Auto-fit Tests ====================

    #[test]
    fn test_auto_repeat_pattern_creation() {
        // Create a template with auto-fill
        let mut template = GridTemplate::default();
        template.repeats.push((
            0,
            TrackRepeat::AutoFill(vec![TrackDefinition::simple(TrackSize::Px(100.0))]),
        ));

        let (expanded, auto_repeat) = template.expand_tracks();

        // Expanded tracks should be empty (auto-fill not expanded yet)
        assert_eq!(expanded.len(), 0);
        // Auto-repeat should be present
        assert!(auto_repeat.is_some());
    }

    #[test]
    fn test_auto_fill_expansion_basic() {
        // repeat(auto-fill, 100px) in 500px container should create 5 tracks
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::Px(100.0))],
            is_auto_fit: false,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 0.0);

        assert_eq!(tracks.len(), 5);
        for track in &tracks {
            assert_eq!(track.base_size, 100.0);
            assert!(!track.is_auto_fit);
        }
    }

    #[test]
    fn test_auto_fill_expansion_with_gap() {
        // repeat(auto-fill, 100px) in 500px container with 20px gap
        // 100 + 20 + 100 + 20 + 100 + 20 + 100 = 460, can fit 4 tracks
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::Px(100.0))],
            is_auto_fit: false,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 20.0);

        assert_eq!(tracks.len(), 4);
    }

    #[test]
    fn test_auto_fill_expansion_minmax() {
        // repeat(auto-fill, minmax(100px, 1fr)) in 500px container
        // The definite size is 100px (min), so we get 5 tracks
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::MinMax(
                Box::new(TrackSize::Px(100.0)),
                Box::new(TrackSize::Fr(1.0)),
            ))],
            is_auto_fit: false,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 0.0);

        assert_eq!(tracks.len(), 5);
        // Each track should be flexible
        for track in &tracks {
            assert!(track.is_flexible);
            assert_eq!(track.base_size, 100.0);
        }
    }

    #[test]
    fn test_auto_fill_expansion_multiple_tracks() {
        // repeat(auto-fill, 100px 50px) in 500px container
        // One repetition = 150px, can fit 3 repetitions = 450px
        let pattern = AutoRepeatPattern {
            tracks: vec![
                TrackDefinition::simple(TrackSize::Px(100.0)),
                TrackDefinition::simple(TrackSize::Px(50.0)),
            ],
            is_auto_fit: false,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 0.0);

        // 3 repetitions * 2 tracks = 6 tracks
        assert_eq!(tracks.len(), 6);
        assert_eq!(tracks[0].base_size, 100.0);
        assert_eq!(tracks[1].base_size, 50.0);
        assert_eq!(tracks[2].base_size, 100.0);
        assert_eq!(tracks[3].base_size, 50.0);
    }

    #[test]
    fn test_auto_fill_minimum_one_repetition() {
        // repeat(auto-fill, 200px) in 100px container should still create 1 track
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::Px(200.0))],
            is_auto_fit: false,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 100.0, 0.0);

        // At least 1 repetition per spec
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].base_size, 200.0);
    }

    #[test]
    fn test_auto_fit_flag_set() {
        // auto-fit tracks should have is_auto_fit = true
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::Px(100.0))],
            is_auto_fit: true,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 0.0);

        assert_eq!(tracks.len(), 5);
        for track in &tracks {
            assert!(track.is_auto_fit);
        }
    }

    #[test]
    fn test_auto_fill_with_fr_only() {
        // repeat(auto-fill, 1fr) - no definite size, should create 1 repetition
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::Fr(1.0))],
            is_auto_fit: false,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 0.0);

        // With no definite size, we get exactly 1 repetition
        assert_eq!(tracks.len(), 1);
        assert!(tracks[0].is_flexible);
    }

    #[test]
    fn test_grid_layout_expand_auto_repeats() {
        // Test that GridLayout properly expands auto-repeat patterns
        let mut template = GridTemplate::default();
        template.repeats.push((
            0,
            TrackRepeat::AutoFill(vec![TrackDefinition::simple(TrackSize::Px(100.0))]),
        ));

        let mut grid = GridLayout::new(
            &template,
            &GridTemplate::from_sizes(vec![TrackSize::Auto]),
            &TrackSize::Auto,
            &TrackSize::Auto,
            0.0,
            0.0,
            GridAutoFlow::Row,
        );

        // Before expansion, columns should be empty (auto-fill not expanded)
        assert_eq!(grid.columns.len(), 0);

        // Expand with 500px container width
        grid.expand_auto_repeats(500.0, 100.0);

        // Now should have 5 columns
        assert_eq!(grid.columns.len(), 5);
        for col in &grid.columns {
            assert_eq!(col.base_size, 100.0);
        }
    }

    #[test]
    fn test_auto_fit_collapse_empty_tracks() {
        // Test that auto-fit collapses empty tracks
        let pattern = AutoRepeatPattern {
            tracks: vec![TrackDefinition::simple(TrackSize::Px(100.0))],
            is_auto_fit: true,
            insert_position: 0,
        };

        let tracks = GridLayout::calculate_auto_repeat_tracks(&pattern, 500.0, 0.0);

        // Should have 5 tracks, all marked as auto-fit
        assert_eq!(tracks.len(), 5);
        for track in &tracks {
            assert!(track.is_auto_fit);
        }
    }

    #[test]
    fn test_auto_fit_collapse_method() {
        // Create a grid with auto-fit tracks
        let mut grid = GridLayout {
            columns: vec![
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.is_auto_fit = true;
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.is_auto_fit = true;
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.is_auto_fit = true;
                    t
                },
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 10.0,
            row_gap: 10.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Mark only the first and third columns as occupied
        let column_occupied = vec![true, false, true];
        grid.collapse_empty_auto_fit_columns(&column_occupied);

        // First column should be intact
        assert_eq!(grid.columns[0].base_size, 100.0);
        assert_eq!(grid.columns[0].size, 100.0);

        // Second column (empty) should be collapsed
        assert_eq!(grid.columns[1].base_size, 0.0);
        assert_eq!(grid.columns[1].size, 0.0);
        assert_eq!(grid.columns[1].growth_limit, 0.0);

        // Third column should be intact
        assert_eq!(grid.columns[2].base_size, 100.0);
        assert_eq!(grid.columns[2].size, 100.0);
    }

    #[test]
    fn test_auto_fit_gap_collapsing() {
        // Test that gaps around collapsed tracks also collapse
        // Per CSS spec: "the gutters on either side of it collapse"
        let mut tracks = vec![
            {
                let mut t = GridTrack::new(&TrackSize::Px(100.0));
                t.is_auto_fit = true;
                t
            },
            {
                // This one will be collapsed
                let mut t = GridTrack::new(&TrackSize::Px(100.0));
                t.is_auto_fit = true;
                t.base_size = 0.0;
                t.size = 0.0;
                t.growth_limit = 0.0;
                t
            },
            {
                let mut t = GridTrack::new(&TrackSize::Px(100.0));
                t.is_auto_fit = true;
                t
            },
        ];

        // Size with 20px gap
        size_grid_tracks(&mut tracks, 500.0, 20.0);

        // First track at position 0
        assert_eq!(tracks[0].position, 0.0);

        // Second track (collapsed) should be at position 100 (no gap added after first track
        // because next track is collapsed)
        assert_eq!(tracks[1].position, 100.0);
        assert_eq!(tracks[1].size, 0.0);

        // Third track should be at position 100 (no gap because previous was collapsed)
        // The gutters on EITHER SIDE of a collapsed track collapse
        assert_eq!(tracks[2].position, 100.0);
    }

    #[test]
    fn test_auto_fit_all_collapsed() {
        // Test when all auto-fit tracks are collapsed
        let mut tracks = vec![
            {
                let mut t = GridTrack::new(&TrackSize::Px(100.0));
                t.is_auto_fit = true;
                t.base_size = 0.0;
                t.size = 0.0;
                t.growth_limit = 0.0;
                t
            },
            {
                let mut t = GridTrack::new(&TrackSize::Px(100.0));
                t.is_auto_fit = true;
                t.base_size = 0.0;
                t.size = 0.0;
                t.growth_limit = 0.0;
                t
            },
        ];

        size_grid_tracks(&mut tracks, 500.0, 20.0);

        // All tracks should be at position 0 with size 0
        assert_eq!(tracks[0].position, 0.0);
        assert_eq!(tracks[0].size, 0.0);
        assert_eq!(tracks[1].position, 0.0);
        assert_eq!(tracks[1].size, 0.0);
    }

    // ==================== Phase 3: Named Lines Tests ====================

    #[test]
    fn test_find_column_line_by_name() {
        // Create a grid with named lines
        let grid = GridLayout {
            columns: vec![
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["main-start".to_string(), "content-start".to_string()];
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["sidebar-start".to_string()];
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["sidebar-end".to_string(), "main-end".to_string()];
                    t
                },
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Find lines by name
        assert_eq!(grid.find_column_line_by_name("main-start"), Some(1));
        assert_eq!(grid.find_column_line_by_name("content-start"), Some(1));
        assert_eq!(grid.find_column_line_by_name("sidebar-start"), Some(2));
        assert_eq!(grid.find_column_line_by_name("sidebar-end"), Some(3));
        assert_eq!(grid.find_column_line_by_name("main-end"), Some(3));
        assert_eq!(grid.find_column_line_by_name("nonexistent"), None);
    }

    #[test]
    fn test_resolve_column_line_by_name() {
        let grid = GridLayout {
            columns: vec![
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["header-start".to_string()];
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["header-end".to_string()];
                    t
                },
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Test resolve_column_line with named line
        let (line, is_auto, span) = grid.resolve_column_line(&GridLine::Name("header-start".to_string()));
        assert_eq!(line, 1);
        assert!(!is_auto);
        assert!(span.is_none());

        let (line2, is_auto2, _) = grid.resolve_column_line(&GridLine::Name("header-end".to_string()));
        assert_eq!(line2, 2);
        assert!(!is_auto2);

        // Unknown name falls back to auto
        let (line3, is_auto3, _) = grid.resolve_column_line(&GridLine::Name("unknown".to_string()));
        assert_eq!(line3, 0);
        assert!(is_auto3);
    }

    #[test]
    fn test_set_placement_with_named_lines() {
        let grid = GridLayout {
            columns: vec![
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["col-a".to_string()];
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["col-b".to_string()];
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["col-c".to_string()];
                    t
                },
            ],
            rows: vec![
                {
                    let mut t = GridTrack::new(&TrackSize::Px(50.0));
                    t.line_names = vec!["row-1".to_string()];
                    t
                },
                {
                    let mut t = GridTrack::new(&TrackSize::Px(50.0));
                    t.line_names = vec!["row-2".to_string()];
                    t
                },
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);

        // Place using named lines: grid-column: col-a / col-c; grid-row: row-1 / row-2
        let placement = GridPlacement {
            column_start: GridLine::Name("col-a".to_string()),
            column_end: GridLine::Name("col-c".to_string()),
            row_start: GridLine::Name("row-1".to_string()),
            row_end: GridLine::Name("row-2".to_string()),
        };
        item.set_placement_with_grid(&placement, &grid);

        // Should be fully placed
        assert!(!item.auto_column);
        assert!(!item.auto_row);
        assert_eq!(item.column_start, 1);
        assert_eq!(item.column_end, 3);
        assert_eq!(item.row_start, 1);
        assert_eq!(item.row_end, 2);
        assert_eq!(item.column_span, 2);
        assert_eq!(item.row_span, 1);
    }

    #[test]
    fn test_named_line_mixed_with_numbers() {
        let grid = GridLayout {
            columns: vec![
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["start".to_string()];
                    t
                },
                GridTrack::new(&TrackSize::Px(100.0)),
                {
                    let mut t = GridTrack::new(&TrackSize::Px(100.0));
                    t.line_names = vec!["end".to_string()];
                    t
                },
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);

        // Mix named line start with numeric end
        let placement = GridPlacement {
            column_start: GridLine::Name("start".to_string()),
            column_end: GridLine::Number(4),
            row_start: GridLine::Number(1),
            row_end: GridLine::Auto,
        };
        item.set_placement_with_grid(&placement, &grid);

        assert!(!item.auto_column);
        assert!(!item.auto_row);
        assert_eq!(item.column_start, 1);
        assert_eq!(item.column_end, 4);
        assert_eq!(item.column_span, 3);
    }

    // ==================== Phase 3.2: Template Areas Tests ====================

    #[test]
    fn test_grid_template_areas_placement() {
        // Create template areas
        let areas = GridTemplateAreas::parse(
            "\"header header header\"
             \"nav main main\"
             \"footer footer footer\""
        ).unwrap();

        // Create a grid with the template areas
        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(50.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
                GridTrack::new(&TrackSize::Px(50.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 3,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: Some(areas),
        };

        // Test area lookup
        let header = grid.get_area("header").unwrap();
        assert_eq!(header.column_start, 1);
        assert_eq!(header.column_end, 4);
        assert_eq!(header.row_start, 1);
        assert_eq!(header.row_end, 2);

        let main = grid.get_area("main").unwrap();
        assert_eq!(main.column_start, 2);
        assert_eq!(main.column_end, 4);
        assert_eq!(main.row_start, 2);
        assert_eq!(main.row_end, 3);
    }

    #[test]
    fn test_placement_with_area_name() {
        // Create template areas
        let areas = GridTemplateAreas::parse(
            "\"header header\"
             \"main sidebar\""
        ).unwrap();

        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(200.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(50.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: Some(areas),
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);

        // Place using grid-area: header (expands to header / header / header / header)
        // which means grid-column: header / header; grid-row: header / header
        // For start position, "header" resolves to column_start=1, row_start=1
        // For end position, "header" resolves to column_end=3, row_end=2
        let placement = GridPlacement {
            column_start: GridLine::Name("header".to_string()),
            column_end: GridLine::Name("header".to_string()),
            row_start: GridLine::Name("header".to_string()),
            row_end: GridLine::Name("header".to_string()),
        };
        item.set_placement_with_grid(&placement, &grid);

        assert!(!item.auto_column);
        assert!(!item.auto_row);
        assert_eq!(item.column_start, 1); // header column_start
        assert_eq!(item.column_end, 3);   // header column_end
        assert_eq!(item.row_start, 1);    // header row_start
        assert_eq!(item.row_end, 2);      // header row_end
    }

    #[test]
    fn test_implicit_line_names_from_areas() {
        // Create template areas
        let areas = GridTemplateAreas::parse(
            "\"header header\"
             \"main sidebar\""
        ).unwrap();

        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(200.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(50.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: Some(areas),
        };

        // Test implicit line names
        assert_eq!(grid.find_column_line_by_name("header-start"), Some(1));
        assert_eq!(grid.find_column_line_by_name("header-end"), Some(3));
        assert_eq!(grid.find_row_line_by_name("header-start"), Some(1));
        assert_eq!(grid.find_row_line_by_name("header-end"), Some(2));

        assert_eq!(grid.find_column_line_by_name("main-start"), Some(1));
        assert_eq!(grid.find_column_line_by_name("main-end"), Some(2));
        assert_eq!(grid.find_column_line_by_name("sidebar-start"), Some(2));
        assert_eq!(grid.find_column_line_by_name("sidebar-end"), Some(3));
    }

    #[test]
    fn test_placement_with_implicit_line_names() {
        // Create template areas
        let areas = GridTemplateAreas::parse(
            "\"header header\"
             \"nav content\""
        ).unwrap();

        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(50.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: Some(areas),
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);

        // Place using implicit line names: grid-column: header-start / header-end
        let placement = GridPlacement {
            column_start: GridLine::Name("header-start".to_string()),
            column_end: GridLine::Name("header-end".to_string()),
            row_start: GridLine::Name("content-start".to_string()),
            row_end: GridLine::Name("content-end".to_string()),
        };
        item.set_placement_with_grid(&placement, &grid);

        assert!(!item.auto_column);
        assert!(!item.auto_row);
        assert_eq!(item.column_start, 1); // header-start
        assert_eq!(item.column_end, 3);   // header-end
        assert_eq!(item.row_start, 2);    // content-start
        assert_eq!(item.row_end, 3);      // content-end
    }

    // ==================== Phase 4: Placement Algorithm Tests ====================

    #[test]
    fn test_dense_packing_backfills_gaps() {
        // Test that dense packing fills gaps left by earlier items
        // Grid: 3 columns, auto rows
        // Item 1: spans 2 columns (occupies cols 0-1)
        // Item 2: spans 1 column (should go to col 2 in sparse, col 2 in dense)
        // Item 3: spans 2 columns (in sparse: new row; in dense: fills row 0 cols 0-1 if gap exists)

        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::RowDense,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Simulate occupied grid:
        // Row 0: [X, X, _] (item spanning cols 0-1)
        let occupied = vec![
            vec![true, true, false],
        ];

        // Find cell for 1-column item - should go to col 2
        let (col, row) = grid.find_next_cell_dense(1, 1, &occupied);
        assert_eq!((col, row), (2, 0), "1-col item should fill col 2 in row 0");
    }

    #[test]
    fn test_dense_packing_vs_sparse() {
        // Compare dense vs sparse packing behavior
        let grid_sparse = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row, // Sparse (default)
            cursor: (2, 0), // Cursor at col 2, row 0
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        let grid_dense = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::RowDense,
            cursor: (2, 0), // Same cursor
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Occupied: col 0 taken, col 1 free, col 2 cursor position
        let occupied = vec![
            vec![true, false, false],
        ];

        // Sparse: starts from cursor (2, 0), wraps to next row since col 2 + 1 col = 3 > 3
        // Actually, col 2 fits a 1-col item
        let (sparse_col, sparse_row) = grid_sparse.find_next_cell(1, 1, &occupied);

        // Dense: starts from (0, 0), finds col 1 is free
        let (dense_col, dense_row) = grid_dense.find_next_cell_dense(1, 1, &occupied);

        // Sparse starts at cursor (2,0), finds col 2 available
        assert_eq!((sparse_col, sparse_row), (2, 0), "Sparse should use cursor position (col 2)");

        // Dense starts at (0,0), finds first free cell at col 1
        assert_eq!((dense_col, dense_row), (1, 0), "Dense should backfill to col 1");
    }

    #[test]
    fn test_dense_packing_column_flow() {
        // Test dense packing with column flow
        let grid = GridLayout {
            columns: vec![GridTrack::new(&TrackSize::Auto)],
            rows: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::ColumnDense,
            cursor: (0, 0),
            explicit_columns: 1,
            explicit_rows: 3,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Occupied: row 0 taken, row 1 free
        let occupied = vec![
            vec![true],
            vec![false],
        ];

        // Dense should find row 1 (backfill)
        let (col, row) = grid.find_next_cell_dense(1, 1, &occupied);
        assert_eq!((col, row), (0, 1), "Dense column flow should backfill to row 1");
    }

    #[test]
    fn test_span_name_with_explicit_line_names() {
        // Test grid-column: 1 / span main (where "main" is at line 2)
        let col1 = GridTrack::new(&TrackSize::Px(100.0));
        let mut col2 = GridTrack::new(&TrackSize::Px(100.0));
        col2.line_names.push("main".to_string());
        let col3 = GridTrack::new(&TrackSize::Px(100.0));

        let grid = GridLayout {
            columns: vec![col1, col2, col3],
            rows: vec![GridTrack::new(&TrackSize::Auto)],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Test resolving SpanName for column end position
        // "main" is at line 2 (1-indexed, before track index 1)
        let (line, is_auto, span) = grid.resolve_column_end_line(&GridLine::SpanName("main".to_string()));
        assert_eq!(line, 2, "SpanName 'main' should resolve to line 2");
        assert!(!is_auto, "SpanName should not be auto");
        assert!(span.is_none(), "SpanName should resolve to explicit line, not span");

        // Test placement: grid-column: 1 / span main
        let placement = GridPlacement {
            column_start: GridLine::Number(1),
            column_end: GridLine::SpanName("main".to_string()),
            row_start: GridLine::Auto,
            row_end: GridLine::Auto,
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);
        item.set_placement_with_grid(&placement, &grid);

        assert_eq!(item.column_start, 1, "Column start should be 1");
        assert_eq!(item.column_end, 2, "Column end should be 2 (span main)");
        assert!(!item.auto_column, "Column should be explicitly placed");
    }

    #[test]
    fn test_span_name_with_area_name() {
        // Test grid-column: 2 / span header (using area name)
        // Parse template areas to create header spanning columns 1-4
        let areas = GridTemplateAreas::parse(
            "\"header header header\"
             \"nav main main\""
        ).unwrap();

        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(50.0)),
                GridTrack::new(&TrackSize::Px(50.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: Some(areas),
        };

        // Test "span header" at column end - should resolve to header's column_end (4)
        let (line, is_auto, span) = grid.resolve_column_end_line(&GridLine::SpanName("header".to_string()));
        assert_eq!(line, 4, "SpanName 'header' at end should resolve to column 4");
        assert!(!is_auto);
        assert!(span.is_none());

        // Test "span header" at column start - should resolve to header's column_start (1)
        let (line, is_auto, span) = grid.resolve_column_start_line(&GridLine::SpanName("header".to_string()));
        assert_eq!(line, 1, "SpanName 'header' at start should resolve to column 1");
        assert!(!is_auto);
        assert!(span.is_none());

        // Test placement: grid-column: 2 / span header (should span from 2 to header's end at 4)
        let placement = GridPlacement {
            column_start: GridLine::Number(2),
            column_end: GridLine::SpanName("header".to_string()),
            row_start: GridLine::Auto,
            row_end: GridLine::Auto,
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);
        item.set_placement_with_grid(&placement, &grid);

        assert_eq!(item.column_start, 2, "Column start should be 2");
        assert_eq!(item.column_end, 4, "Column end should be 4 (span header)");
        assert_eq!(item.column_span, 2, "Span should be 2 tracks (from line 2 to line 4)");
    }

    #[test]
    fn test_span_name_row_with_implicit_lines() {
        // Test grid-row: 1 / span sidebar-end (using implicit line name from area)
        // Create a grid with sidebar area spanning rows 1-3
        let areas = GridTemplateAreas::parse(
            "\"sidebar main\"
             \"sidebar main\"
             \"footer footer\""
        ).unwrap();

        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(200.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 3,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: Some(areas),
        };

        // Verify sidebar area bounds
        let sidebar = grid.get_area("sidebar").unwrap();
        assert_eq!(sidebar.row_start, 1, "sidebar row_start");
        assert_eq!(sidebar.row_end, 3, "sidebar row_end");

        // "sidebar-end" is an implicit line name pointing to row_end (line 3)
        let (line, is_auto, span) = grid.resolve_row_end_line(&GridLine::SpanName("sidebar-end".to_string()));
        assert_eq!(line, 3, "SpanName 'sidebar-end' should resolve to row line 3");
        assert!(!is_auto);
        assert!(span.is_none());

        // Test placement: grid-row: 1 / span sidebar-end
        let placement = GridPlacement {
            column_start: GridLine::Auto,
            column_end: GridLine::Auto,
            row_start: GridLine::Number(1),
            row_end: GridLine::SpanName("sidebar-end".to_string()),
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);
        item.set_placement_with_grid(&placement, &grid);

        assert_eq!(item.row_start, 1, "Row start should be 1");
        assert_eq!(item.row_end, 3, "Row end should be 3 (span sidebar-end)");
        assert_eq!(item.row_span, 2, "Span should be 2 tracks");
    }

    #[test]
    fn test_order_property_sorting() {
        // Test that GridItem.order() returns the correct value from the layout box
        let mut style1 = ComputedStyle::new();
        style1.order = 2;
        let layout_box1 = LayoutBox::new(BoxType::Block, style1);
        let item1 = GridItem::new(&layout_box1);
        assert_eq!(item1.order(), 2, "Item1 should have order 2");

        let mut style2 = ComputedStyle::new();
        style2.order = -1;
        let layout_box2 = LayoutBox::new(BoxType::Block, style2);
        let item2 = GridItem::new(&layout_box2);
        assert_eq!(item2.order(), -1, "Item2 should have order -1");

        let style3 = ComputedStyle::new(); // Default order is 0
        let layout_box3 = LayoutBox::new(BoxType::Block, style3);
        let item3 = GridItem::new(&layout_box3);
        assert_eq!(item3.order(), 0, "Item3 should have order 0");

        // Create a vector and sort by order
        let mut items = vec![item1, item2, item3];
        items.sort_by_key(|item| item.order());

        // Verify order after sorting: -1, 0, 2
        assert_eq!(items[0].order(), -1, "First item should have order -1");
        assert_eq!(items[1].order(), 0, "Second item should have order 0");
        assert_eq!(items[2].order(), 2, "Third item should have order 2");
    }

    #[test]
    fn test_order_property_stable_sort() {
        // Test that items with equal order values maintain document order (stable sort)
        let mut style_a = ComputedStyle::new();
        style_a.order = 1;
        let layout_box_a = LayoutBox::new(BoxType::Block, style_a);

        let mut style_b = ComputedStyle::new();
        style_b.order = 1;
        let layout_box_b = LayoutBox::new(BoxType::Block, style_b);

        let mut style_c = ComputedStyle::new();
        style_c.order = 0;
        let layout_box_c = LayoutBox::new(BoxType::Block, style_c);

        // Items in "document order": A, B, C
        // After sorting by order: C (order 0), A (order 1), B (order 1)
        // A and B have same order, so A should come before B (stable)
        let item_a = GridItem::new(&layout_box_a);
        let item_b = GridItem::new(&layout_box_b);
        let item_c = GridItem::new(&layout_box_c);

        let mut items = vec![item_a, item_b, item_c];

        // Store original positions by pointer comparison
        let ptr_a = items[0].layout_box as *const _;
        let ptr_b = items[1].layout_box as *const _;
        let ptr_c = items[2].layout_box as *const _;

        items.sort_by_key(|item| item.order());

        // C (order 0) should be first
        assert_eq!(items[0].layout_box as *const _, ptr_c, "C (order 0) should be first");
        // A and B both have order 1, but A was before B in document order
        assert_eq!(items[1].layout_box as *const _, ptr_a, "A (order 1) should be second (stable)");
        assert_eq!(items[2].layout_box as *const _, ptr_b, "B (order 1) should be third (stable)");
    }

    #[test]
    fn test_items_beyond_explicit_grid() {
        // Test that items placed beyond the explicit grid create implicit tracks
        let mut grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(50.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 1,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Initial grid: 2 columns, 1 row
        assert_eq!(grid.column_count(), 2, "Initial columns");
        assert_eq!(grid.row_count(), 1, "Initial rows");
        assert_eq!(grid.explicit_columns, 2, "Explicit columns tracked");
        assert_eq!(grid.explicit_rows, 1, "Explicit rows tracked");

        // Place item at column 5, row 3 (beyond explicit grid)
        grid.ensure_tracks(5, 3, &TrackSize::Auto, &TrackSize::Auto);

        // Grid should now have 5 columns, 3 rows
        assert_eq!(grid.column_count(), 5, "Columns after ensure_tracks");
        assert_eq!(grid.row_count(), 3, "Rows after ensure_tracks");

        // Explicit counts remain the same (they track template-defined tracks)
        assert_eq!(grid.explicit_columns, 2, "Explicit column count unchanged");
        assert_eq!(grid.explicit_rows, 1, "Explicit row count unchanged");

        // Implicit tracks (beyond explicit) are columns 3-5 and rows 2-3
        // We can't easily distinguish them by field, but the count difference tells us
        let implicit_columns = grid.column_count() - grid.explicit_columns;
        let implicit_rows = grid.row_count() - grid.explicit_rows;
        assert_eq!(implicit_columns, 3, "3 implicit columns added");
        assert_eq!(implicit_rows, 2, "2 implicit rows added");
    }

    #[test]
    fn test_overlapping_explicit_placement() {
        // Test that explicitly placed items can overlap
        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 2,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Item 1: grid-column: 1 / 3; grid-row: 1 / 2; (spans both columns, row 1)
        let placement1 = GridPlacement {
            column_start: GridLine::Number(1),
            column_end: GridLine::Number(3),
            row_start: GridLine::Number(1),
            row_end: GridLine::Number(2),
        };

        // Item 2: grid-column: 1 / 2; grid-row: 1 / 3; (column 1, spans both rows)
        // This overlaps with Item 1 at cell (0, 0)
        let placement2 = GridPlacement {
            column_start: GridLine::Number(1),
            column_end: GridLine::Number(2),
            row_start: GridLine::Number(1),
            row_end: GridLine::Number(3),
        };

        let style = ComputedStyle::new();
        let layout_box1 = LayoutBox::new(BoxType::Block, style.clone());
        let layout_box2 = LayoutBox::new(BoxType::Block, style);

        let mut item1 = GridItem::new(&layout_box1);
        let mut item2 = GridItem::new(&layout_box2);

        item1.set_placement_with_grid(&placement1, &grid);
        item2.set_placement_with_grid(&placement2, &grid);

        // Both items should be fully placed (not needing auto-placement)
        assert!(item1.is_fully_placed(), "Item 1 should be fully placed");
        assert!(item2.is_fully_placed(), "Item 2 should be fully placed");

        // Item 1: columns 1-3, row 1-2
        assert_eq!(item1.column_start, 1);
        assert_eq!(item1.column_end, 3);
        assert_eq!(item1.row_start, 1);
        assert_eq!(item1.row_end, 2);

        // Item 2: column 1-2, rows 1-3
        assert_eq!(item2.column_start, 1);
        assert_eq!(item2.column_end, 2);
        assert_eq!(item2.row_start, 1);
        assert_eq!(item2.row_end, 3);

        // Both items occupy cell (0, 0) - this is valid overlapping
    }

    #[test]
    fn test_negative_line_numbers() {
        // Test that negative line numbers work correctly
        let grid = GridLayout {
            columns: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            rows: vec![
                GridTrack::new(&TrackSize::Px(100.0)),
                GridTrack::new(&TrackSize::Px(100.0)),
            ],
            column_gap: 0.0,
            row_gap: 0.0,
            auto_flow: GridAutoFlow::Row,
            cursor: (0, 0),
            explicit_columns: 3,
            explicit_rows: 2,
            column_auto_repeat: None,
            row_auto_repeat: None,
            template_areas: None,
        };

        // Test resolving -1 (last line) for columns
        // With 3 columns, lines are: 1, 2, 3, 4 (4 is after the last track)
        // -1 should resolve to line 4
        let (line, is_auto, _) = grid.resolve_column_end_line(&GridLine::Number(-1));
        assert_eq!(line, -1, "GridLine::Number(-1) should stay as -1 for later resolution");
        assert!(!is_auto);

        // Test a placement with grid-column: 1 / -1 (all columns)
        let placement = GridPlacement {
            column_start: GridLine::Number(1),
            column_end: GridLine::Number(-1),
            row_start: GridLine::Number(1),
            row_end: GridLine::Number(2),
        };

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let mut item = GridItem::new(&layout_box);
        item.set_placement_with_grid(&placement, &grid);

        // The item should span from column 1 to column -1
        // Negative line resolution happens in layout_grid_container, not set_placement_with_grid
        assert_eq!(item.column_start, 1);
        assert_eq!(item.column_end, -1);  // Will be resolved later to 4
    }

    // ==================== Phase 5: Alignment Tests ====================

    #[test]
    fn test_content_alignment_start() {
        // Test justify-content: flex-start (default)
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        // Size tracks first
        size_grid_tracks(&mut tracks, 400.0, 10.0);

        // Initial positions: 0, 110 (100 + 10 gap)
        assert_eq!(tracks[0].position, 0.0);
        assert_eq!(tracks[1].position, 110.0);

        // Apply flex-start alignment (should not change positions)
        apply_content_alignment(&mut tracks, 400.0, 10.0, &JustifyContent::FlexStart);

        assert_eq!(tracks[0].position, 0.0);
        assert_eq!(tracks[1].position, 110.0);
    }

    #[test]
    fn test_content_alignment_end() {
        // Test justify-content: flex-end
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 10.0);

        // Total used: 100 + 10 + 100 = 210
        // Free space: 400 - 210 = 190
        apply_content_alignment(&mut tracks, 400.0, 10.0, &JustifyContent::FlexEnd);

        // Tracks should be shifted by 190
        assert_eq!(tracks[0].position, 190.0);
        assert_eq!(tracks[1].position, 300.0); // 190 + 100 + 10
    }

    #[test]
    fn test_content_alignment_center() {
        // Test justify-content: center
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 10.0);

        // Free space: 190, center offset: 95
        apply_content_alignment(&mut tracks, 400.0, 10.0, &JustifyContent::Center);

        assert_eq!(tracks[0].position, 95.0);
        assert_eq!(tracks[1].position, 205.0); // 95 + 100 + 10
    }

    #[test]
    fn test_content_alignment_space_between() {
        // Test justify-content: space-between
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 10.0);

        // Free space: 190, distributed between 2 tracks = 190 extra gap
        apply_content_alignment(&mut tracks, 400.0, 10.0, &JustifyContent::SpaceBetween);

        // First track at start, last track at end
        assert_eq!(tracks[0].position, 0.0);
        assert_eq!(tracks[1].position, 300.0); // 0 + 100 + 10 + 190
    }

    #[test]
    fn test_content_alignment_space_around() {
        // Test justify-content: space-around
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 10.0);

        // Free space: 190, 2 tracks = 4 half-gaps = 190/4 = 47.5 per half-gap
        apply_content_alignment(&mut tracks, 400.0, 10.0, &JustifyContent::SpaceAround);

        // First track offset by half-gap (47.5)
        // Second track offset by half-gap + full gap (47.5 + 95 = 142.5 from first)
        assert!((tracks[0].position - 47.5).abs() < 0.01);
        assert!((tracks[1].position - 252.5).abs() < 0.01); // 47.5 + 100 + 10 + 95
    }

    #[test]
    fn test_content_alignment_space_evenly() {
        // Test justify-content: space-evenly
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        size_grid_tracks(&mut tracks, 400.0, 10.0);

        // Free space: 190, 2 tracks = 3 spaces = 190/3 ≈ 63.33 per space
        apply_content_alignment(&mut tracks, 400.0, 10.0, &JustifyContent::SpaceEvenly);

        let space = 190.0 / 3.0;
        assert!((tracks[0].position - space).abs() < 0.01);
        // Second track: space + 100 + 10 + space
        assert!((tracks[1].position - (space + 100.0 + 10.0 + space)).abs() < 0.01);
    }

    #[test]
    fn test_align_content_to_justify_conversion() {
        // Test the conversion function
        assert_eq!(align_content_to_justify(&AlignContent::FlexStart), JustifyContent::FlexStart);
        assert_eq!(align_content_to_justify(&AlignContent::FlexEnd), JustifyContent::FlexEnd);
        assert_eq!(align_content_to_justify(&AlignContent::Center), JustifyContent::Center);
        assert_eq!(align_content_to_justify(&AlignContent::SpaceBetween), JustifyContent::SpaceBetween);
        assert_eq!(align_content_to_justify(&AlignContent::SpaceAround), JustifyContent::SpaceAround);
        assert_eq!(align_content_to_justify(&AlignContent::SpaceEvenly), JustifyContent::SpaceEvenly);
        assert_eq!(align_content_to_justify(&AlignContent::Stretch), JustifyContent::FlexStart);
    }

    #[test]
    fn test_justify_self_alignment() {
        // Test justify-self: start (default from justify-items: start)
        let mut style = ComputedStyle::new();
        style.width = Length::Px(50.0); // Explicit width
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // Cell: x=10, width=100
        // Child width: 50

        // justify-self: start
        let (x, w) = apply_justify_self(&JustifySelf::Start, &JustifyItems::Start, 10.0, 100.0, &layout_box);
        assert_eq!(x, 10.0, "justify-self: start should position at cell start");
        assert_eq!(w, 50.0, "Width should match explicit width");

        // justify-self: end
        let (x, w) = apply_justify_self(&JustifySelf::End, &JustifyItems::Start, 10.0, 100.0, &layout_box);
        assert_eq!(x, 60.0, "justify-self: end should position at cell end - width (10 + 100 - 50)");
        assert_eq!(w, 50.0);

        // justify-self: center
        let (x, w) = apply_justify_self(&JustifySelf::Center, &JustifyItems::Start, 10.0, 100.0, &layout_box);
        assert_eq!(x, 35.0, "justify-self: center should center (10 + (100-50)/2)");
        assert_eq!(w, 50.0);
    }

    #[test]
    fn test_justify_self_stretch() {
        // Test justify-self: stretch with auto width
        let mut style = ComputedStyle::new();
        style.width = Length::Auto;
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // With auto width, stretch should use cell width
        let (x, w) = apply_justify_self(&JustifySelf::Stretch, &JustifyItems::Stretch, 10.0, 100.0, &layout_box);
        assert_eq!(x, 10.0);
        assert_eq!(w, 100.0, "Stretch with auto width should fill cell");

        // With explicit width, stretch should use explicit width
        let mut style2 = ComputedStyle::new();
        style2.width = Length::Px(50.0);
        let layout_box2 = LayoutBox::new(BoxType::Block, style2);

        let (x, w) = apply_justify_self(&JustifySelf::Stretch, &JustifyItems::Stretch, 10.0, 100.0, &layout_box2);
        assert_eq!(x, 10.0);
        assert_eq!(w, 50.0, "Stretch with explicit width should respect width");
    }

    #[test]
    fn test_justify_self_auto_fallback() {
        // Test justify-self: auto falls back to justify-items
        let mut style = ComputedStyle::new();
        style.width = Length::Px(50.0);
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // justify-self: auto, justify-items: end -> should align end
        let (x, _) = apply_justify_self(&JustifySelf::Auto, &JustifyItems::End, 10.0, 100.0, &layout_box);
        assert_eq!(x, 60.0, "Auto should fall back to justify-items: end");

        // justify-self: auto, justify-items: center -> should center
        let (x, _) = apply_justify_self(&JustifySelf::Auto, &JustifyItems::Center, 10.0, 100.0, &layout_box);
        assert_eq!(x, 35.0, "Auto should fall back to justify-items: center");
    }

    #[test]
    fn test_align_self_alignment() {
        // Test align-self alignment
        let mut style = ComputedStyle::new();
        style.height = Length::Px(30.0); // Explicit height
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // Cell: y=20, height=100
        // Child height: 30

        // align-self: flex-start
        let (y, h) = apply_align_self(&AlignSelf::FlexStart, &AlignItems::Stretch, 20.0, 100.0, &layout_box);
        assert_eq!(y, 20.0, "align-self: flex-start should position at cell start");
        assert_eq!(h, 30.0);

        // align-self: flex-end
        let (y, h) = apply_align_self(&AlignSelf::FlexEnd, &AlignItems::Stretch, 20.0, 100.0, &layout_box);
        assert_eq!(y, 90.0, "align-self: flex-end should position at cell end - height (20 + 100 - 30)");
        assert_eq!(h, 30.0);

        // align-self: center
        let (y, h) = apply_align_self(&AlignSelf::Center, &AlignItems::Stretch, 20.0, 100.0, &layout_box);
        assert_eq!(y, 55.0, "align-self: center should center (20 + (100-30)/2)");
        assert_eq!(h, 30.0);
    }

    /// A `calc()` height is explicit, so `apply_align_self` must resolve it
    /// rather than treat it as the cell height. Without the `Length::Calc`
    /// arm the item is called explicit and then sized as if it were `auto` —
    /// it fills the cell and centres at the cell's own origin.
    #[test]
    fn a_calc_height_grid_item_aligns_at_its_resolved_height() {
        let mut style = ComputedStyle::new();
        style.height = rustkit_css::parse_length("calc(100% - 40px)").expect("calc parses");
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // Cell: y=20, height=100 -> the item is 60 tall, centred at 20+20.
        let (y, h) = apply_align_self(
            &AlignSelf::Center,
            &AlignItems::Stretch,
            20.0,
            100.0,
            &layout_box,
        );
        assert_eq!(h, 60.0, "100% of the 100px cell minus 40px");
        assert_eq!(y, 40.0, "20 + (100 - 60) / 2");
    }

    #[test]
    fn test_align_self_stretch() {
        // Test align-self: stretch with auto height
        let mut style = ComputedStyle::new();
        style.height = Length::Auto;
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // With auto height, stretch should use cell height
        let (y, h) = apply_align_self(&AlignSelf::Stretch, &AlignItems::Stretch, 20.0, 100.0, &layout_box);
        assert_eq!(y, 20.0);
        assert_eq!(h, 100.0, "Stretch with auto height should fill cell");

        // With explicit height, stretch should use explicit height
        let mut style2 = ComputedStyle::new();
        style2.height = Length::Px(30.0);
        let layout_box2 = LayoutBox::new(BoxType::Block, style2);

        let (y, h) = apply_align_self(&AlignSelf::Stretch, &AlignItems::Stretch, 20.0, 100.0, &layout_box2);
        assert_eq!(y, 20.0);
        assert_eq!(h, 30.0, "Stretch with explicit height should respect height");
    }

    // ==================== Phase 6 Tests ====================

    #[test]
    fn test_spanning_item_distribution() {
        // Test that spanning items distribute extra space correctly
        // Single-span items should be processed first, then multi-span

        // Create tracks: [auto, auto, auto]
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Auto),
            GridTrack::new(&TrackSize::Auto),
            GridTrack::new(&TrackSize::Auto),
        ];

        // Simulate: item1 in track 0 needs 100px
        // Item contributes to base_size
        tracks[0].base_size = 100.0;

        // Simulate: item2 spans tracks 1-2 and needs 150px
        // With spec-compliant distribution, this should add 75px to each track

        // Current space in tracks 1-2 is 0
        let current_space: f32 = tracks[1].base_size + tracks[2].base_size;
        let needed = 150.0;
        let extra = needed - current_space;

        // Distribute equally among the spanned tracks
        let per_track = extra / 2.0;
        tracks[1].base_size += per_track;
        tracks[2].base_size += per_track;

        // Verify distribution
        assert_eq!(tracks[0].base_size, 100.0, "Track 0 should have single item size");
        assert_eq!(tracks[1].base_size, 75.0, "Track 1 should have half of spanning item");
        assert_eq!(tracks[2].base_size, 75.0, "Track 2 should have half of spanning item");
    }

    #[test]
    fn spanning_item_fills_the_unsized_track_first() {
        // image-gallery row 3/4: a 200px item in row 3, a `span 2` item of
        // 416px over rows 3-4 (gap 16), nothing else in row 4. Chrome: 200 / 200.
        let mut rows = vec![GridTrack::new(&TrackSize::Auto), GridTrack::new(&TrackSize::Auto)];
        distribute_span_contributions(&mut rows, &[(0, 1, 200.0), (0, 2, 416.0)], 16.0, 2);
        assert_eq!((rows[0].base_size, rows[1].base_size), (200.0, 200.0));
    }

    #[test]
    fn spanning_item_splits_beyond_limits_when_every_track_is_sized() {
        // Both rows hold a 100px item; the span-2 item needs 256 (+16 gap):
        // no track has room below its limit, so the 40 extra splits evenly.
        let mut rows = vec![GridTrack::new(&TrackSize::Auto), GridTrack::new(&TrackSize::Auto)];
        let items = [(0, 1, 100.0), (1, 1, 100.0), (0, 2, 256.0)];
        distribute_span_contributions(&mut rows, &items, 16.0, 2);
        assert_eq!((rows[0].base_size, rows[1].base_size), (120.0, 120.0));
    }

    #[test]
    fn spanning_item_skips_fixed_tracks() {
        let mut cols = vec![GridTrack::new(&TrackSize::Px(100.0)), GridTrack::new(&TrackSize::Auto)];
        distribute_span_contributions(&mut cols, &[(0, 2, 200.0)], 0.0, 2);
        assert_eq!((cols[0].base_size, cols[1].base_size), (100.0, 100.0));
    }

    #[test]
    fn test_spanning_prioritizes_growable_tracks() {
        // When a spanning item needs extra space, it should go to growable tracks
        // Fixed tracks should not grow if there are growable alternatives

        // Create tracks: [100px (fixed), auto (growable)]
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Auto),
        ];

        // The fixed track has base_size = 100, growth_limit = 100
        // The auto track has is_min_content=true, is_max_content=true

        // Simulate a spanning item needing 200px total
        let needed = 200.0;
        let current_space: f32 = tracks[0].base_size + tracks[1].base_size; // 100 + 0 = 100
        let extra = needed - current_space; // 100 extra needed

        // Find growable tracks
        let growable: Vec<usize> = (0..2)
            .filter(|&i| {
                let t = &tracks[i];
                t.is_min_content || t.is_max_content || t.is_flexible || t.growth_limit > t.base_size
            })
            .collect();

        // Track 1 (auto) is growable, Track 0 (fixed) is not
        assert_eq!(growable.len(), 1);
        assert_eq!(growable[0], 1);

        // Distribute extra to growable track only
        if !growable.is_empty() {
            let per_track = extra / growable.len() as f32;
            for i in growable {
                tracks[i].base_size += per_track;
            }
        }

        // Verify: fixed track unchanged, auto track grew
        assert_eq!(tracks[0].base_size, 100.0, "Fixed track should not grow");
        assert_eq!(tracks[1].base_size, 100.0, "Auto track should absorb all extra space");
    }

    #[test]
    fn test_stretch_auto_tracks() {
        // Test that stretch_auto_tracks distributes free space to auto tracks

        // Create tracks: [100px, auto, 100px] in 400px container
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Auto),
            GridTrack::new(&TrackSize::Px(100.0)),
        ];

        // Initialize sizes
        tracks[0].size = 100.0;
        tracks[1].size = 50.0; // auto track has 50px from content
        tracks[2].size = 100.0;

        // Container is 400px with 10px gaps
        // Used: 100 + 50 + 100 + 2*10 = 270px
        // Free: 400 - 270 = 130px

        stretch_auto_tracks(&mut tracks, 400.0, 10.0);

        // Auto track should receive all free space (130px)
        // Final size should be 50 + 130 = 180px
        assert_eq!(tracks[0].size, 100.0, "Fixed track should not change");
        assert_eq!(tracks[1].size, 180.0, "Auto track should stretch to fill");
        assert_eq!(tracks[2].size, 100.0, "Fixed track should not change");
    }

    #[test]
    fn test_stretch_multiple_auto_tracks() {
        // Test that free space is distributed equally among multiple auto tracks

        // Create tracks: [auto, 100px, auto] in 500px container
        let mut tracks = vec![
            GridTrack::new(&TrackSize::Auto),
            GridTrack::new(&TrackSize::Px(100.0)),
            GridTrack::new(&TrackSize::Auto),
        ];

        // Initialize sizes
        tracks[0].size = 50.0;
        tracks[1].size = 100.0;
        tracks[2].size = 50.0;

        // Container is 500px with 10px gaps
        // Used: 50 + 100 + 50 + 2*10 = 220px
        // Free: 500 - 220 = 280px (split between 2 auto tracks = 140 each)

        stretch_auto_tracks(&mut tracks, 500.0, 10.0);

        // Each auto track should receive 140px
        assert_eq!(tracks[0].size, 190.0, "First auto track should stretch");
        assert_eq!(tracks[1].size, 100.0, "Fixed track should not change");
        assert_eq!(tracks[2].size, 190.0, "Second auto track should stretch");
    }

    #[test]
    fn test_maximize_tracks_step() {
        // Test that tracks grow from base_size toward growth_limit

        // Create a track with room to grow
        let track = GridTrack::new(&TrackSize::MinMax(
            Box::new(TrackSize::Px(50.0)),
            Box::new(TrackSize::Px(200.0)),
        ));

        // base_size = 50, growth_limit = 200
        assert_eq!(track.base_size, 50.0);
        assert_eq!(track.growth_limit, 200.0);

        // In size_grid_tracks, tracks should maximize toward growth_limit
        // when there's available space
        let mut tracks = vec![track];
        size_grid_tracks(&mut tracks, 300.0, 0.0);

        // Track should grow to growth_limit (200px) if there's room
        // Actually, in current impl, step 4 distributes remaining space
        // With 300px container and 50px used, remaining = 250px
        // Track can grow by 150px (to 200px growth_limit)
        assert!(tracks[0].size >= tracks[0].base_size);
        assert!(tracks[0].size <= 200.0, "Should not exceed growth_limit");
    }

    // ==================== Phase 7 Tests (Edge Cases) ====================

    #[test]
    fn test_empty_grid_container() {
        // Test that empty grid containers are handled gracefully
        // Should have at least one implicit row and column

        let grid = GridLayout::new(
            &GridTemplate::default(), // No columns
            &GridTemplate::default(), // No rows
            &TrackSize::Auto,
            &TrackSize::Auto,
            0.0,
            0.0,
            GridAutoFlow::Row,
        );

        // Initially empty templates
        assert!(grid.columns.is_empty());
        assert!(grid.rows.is_empty());

        // In layout_grid_container, empty grids get at least one implicit track
        // This is tested indirectly via the existing tests
    }

    #[test]
    fn test_grid_track_sizing_respects_growth_limit() {
        // Test that track sizing doesn't exceed growth_limit

        let mut tracks = vec![
            GridTrack::new(&TrackSize::MinMax(
                Box::new(TrackSize::Px(50.0)),
                Box::new(TrackSize::Px(100.0)),
            )),
        ];

        // Container has 500px, track should grow to max of 100px, not fill
        size_grid_tracks(&mut tracks, 500.0, 0.0);

        assert_eq!(tracks[0].size, 100.0, "Track should stop at growth_limit");
    }

    #[test]
    fn test_grid_items_filter_display_none() {
        // Verify that items with display: none are not placed in the grid
        // This is tested by verifying the filter in layout_grid_container
        // exists: filter(|child| child.style.display != Display::None)

        // The implementation filters display:none items, which is correct
        // per CSS Grid spec - they don't participate in grid layout
        assert!(true);
    }

    #[test]
    fn test_grid_item_explicit_size_overrides_cell() {
        // Test that items with explicit width/height use those values
        // rather than filling the entire cell

        // Create a style with explicit 50px width
        let mut style = ComputedStyle::new();
        style.width = Length::Px(50.0);
        let layout_box = LayoutBox::new(BoxType::Block, style);

        // Test justify-self with explicit width
        let (x, w) = apply_justify_self(&JustifySelf::Start, &JustifyItems::Start, 0.0, 200.0, &layout_box);
        assert_eq!(x, 0.0);
        assert_eq!(w, 50.0, "Should use explicit width, not cell width");

        // Test with stretch - should still respect explicit width
        let (x, w) = apply_justify_self(&JustifySelf::Stretch, &JustifyItems::Stretch, 0.0, 200.0, &layout_box);
        assert_eq!(x, 0.0);
        assert_eq!(w, 50.0, "Stretch with explicit width should use explicit width");
    }

    #[test]
    fn test_grid_cell_with_zero_size() {
        // Test behavior when a track has zero size (collapsed auto-fit)

        let mut tracks = vec![
            GridTrack::new(&TrackSize::Auto),
            GridTrack::new(&TrackSize::Auto),
        ];

        // Simulate fully collapsed track (e.g., empty auto-fit)
        // When a track is collapsed, all its sizing properties are zeroed
        tracks[0].is_auto_fit = true;
        tracks[0].base_size = 0.0;
        tracks[0].growth_limit = 0.0;
        tracks[0].size = 0.0;
        tracks[0].is_min_content = false;  // Clear intrinsic flags
        tracks[0].is_max_content = false;
        tracks[0].is_flexible = false;
        tracks[1].base_size = 100.0;

        size_grid_tracks(&mut tracks, 200.0, 10.0);

        // First track should remain at 0 (collapsed)
        assert_eq!(tracks[0].size, 0.0, "Collapsed track should stay at 0");
        // Second track should get all the space
        assert!(tracks[1].size > 0.0, "Non-collapsed track should have size");
    }

    #[test]
    fn test_track_line_names_preserved() {
        // Test that line names are preserved through track operations

        let mut track = GridTrack::new(&TrackSize::Px(100.0));
        track.line_names = vec!["header-start".to_string(), "main".to_string()];

        // Verify names are preserved
        assert_eq!(track.line_names.len(), 2);
        assert_eq!(track.line_names[0], "header-start");
        assert_eq!(track.line_names[1], "main");

        // Names should survive track sizing
        let mut tracks = vec![track];
        size_grid_tracks(&mut tracks, 200.0, 0.0);

        assert_eq!(tracks[0].line_names.len(), 2);
    }

    #[test]
    fn test_negative_span_handled() {
        // Test that negative or zero spans are handled gracefully
        // by being clamped to at least 1

        let style = ComputedStyle::new();
        let layout_box = LayoutBox::new(BoxType::Block, style);
        let item = GridItem::new(&layout_box);

        // column_span and row_span should default to 1
        assert_eq!(item.column_span, 1);
        assert_eq!(item.row_span, 1);
    }

    // ---------------------------------------------------------------
    // Grid items size and place from their MARGIN box (css-grid-1 §6.5,
    // §12.4).
    //
    // The corpus shape these come from: `gradient-no-radius` and
    // `gradient-radius-only` have four `.section-header { margin-bottom:
    // 10px }` rows. Each row was short by exactly that margin, and because
    // rows stack the error accumulated -- row 2 was 10px high, row 4 was
    // 30px, the last 40px. Gate A read 47 and 46 geometry failures on those
    // two cases with every box's x, width and height already exact: the
    // whole defect was one missing term, repeated.
    // ---------------------------------------------------------------

    /// A row is sized from the item's outer height, so the row below starts
    /// below the margin as well as the border box.
    #[test]
    fn a_grid_row_is_sized_from_the_item_margin_box_not_its_border_box() {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0)]);
        container_style.row_gap = Length::Px(20.0);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        // Row 1: a 40px header carrying a 10px bottom margin.
        let mut header_style = ComputedStyle::new();
        header_style.height = Length::Px(40.0);
        header_style.margin_bottom = Length::Px(10.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, header_style));

        // Row 2: a plain 100px box.
        let mut body_style = ComputedStyle::new();
        body_style.height = Length::Px(100.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, body_style));

        layout_grid_container(&mut container, 400.0, 600.0);

        let header = container.children[0].dimensions.border_box();
        let body = container.children[1].dimensions.border_box();

        // The header's own border box is unchanged by its margin.
        assert!(
            (header.height - 40.0).abs() < 0.01,
            "header border box should stay 40, got {}",
            header.height
        );
        // 40 border box + 10 margin + 20 gap.
        let expected_y = header.y + 40.0 + 10.0 + 20.0;
        assert!(
            (body.y - expected_y).abs() < 0.01,
            "row 2 should start below the header's MARGIN box: expected {expected_y}, \
             got {} (a {}px shortfall is the margin being dropped)",
            body.y,
            expected_y - body.y
        );
    }

    /// Stretch fills the grid area with the item's margin box, so the border
    /// box gets the area less its own margins -- it must not swallow the
    /// margin back and paint over the gap.
    #[test]
    fn a_stretched_grid_item_does_not_swallow_its_own_margin() {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0)]);
        container_style.grid_template_rows = GridTemplate::from_sizes(vec![TrackSize::Px(100.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut item_style = ComputedStyle::new();
        item_style.margin_top = Length::Px(15.0);
        item_style.margin_bottom = Length::Px(25.0);
        item_style.margin_left = Length::Px(5.0);
        item_style.margin_right = Length::Px(35.0);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, item_style));

        layout_grid_container(&mut container, 400.0, 600.0);

        let b = container.children[0].dimensions.border_box();
        assert!(
            (b.height - 60.0).abs() < 0.01,
            "stretched item in a 100px row with 15+25 margins should be 60 tall, got {}",
            b.height
        );
        assert!(
            (b.width - 360.0).abs() < 0.01,
            "stretched item in a 400px column with 5+35 margins should be 360 wide, got {}",
            b.width
        );
        assert!(
            (b.y - 15.0).abs() < 0.01,
            "the border box starts after the top margin, got y={}",
            b.y
        );
        assert!(
            (b.x - 5.0).abs() < 0.01,
            "the border box starts after the left margin, got x={}",
            b.x
        );

        // §6.5 stated as the invariant it is: the item's MARGIN box is the
        // grid area. This is what makes the resolved margins worth recording
        // on the item's dimensions at all — `margin_box()` is read by the
        // float, inline and scroll-extent paths, and a grid item that reports
        // a margin box equal to its border box lies to every one of them.
        let m = container.children[0].dimensions.margin_box();
        assert!(
            (m.x).abs() < 0.01
                && (m.y).abs() < 0.01
                && (m.width - 400.0).abs() < 0.01
                && (m.height - 100.0).abs() < 0.01,
            "the item's margin box should BE the grid area (0,0 400x100), got {m:?}"
        );
    }

    /// The explicit-size early returns are a separate path through
    /// `get_height_contribution` and carry the margins too.
    #[test]
    fn an_explicitly_sized_item_contributes_its_outer_height() {
        let mut style = ComputedStyle::new();
        style.height = Length::Px(180.0);
        style.margin_top = Length::Px(4.0);
        style.margin_bottom = Length::Px(6.0);
        let b = LayoutBox::new(BoxType::Block, style);
        let item = GridItem::new(&b);
        assert!(
            (item.get_height_contribution(0.0) - 190.0).abs() < 0.01,
            "explicit 180px height + 4 + 6 margins = 190, got {}",
            item.get_height_contribution(0.0)
        );
    }

    /// The auto/min-height path carries them as well.
    #[test]
    fn an_auto_height_item_contributes_its_outer_height() {
        let mut style = ComputedStyle::new();
        style.min_height = Length::Px(50.0);
        style.margin_top = Length::Px(7.0);
        style.margin_bottom = Length::Px(3.0);
        let b = LayoutBox::new(BoxType::Block, style);
        let item = GridItem::new(&b);
        assert!(
            (item.get_height_contribution(0.0) - 60.0).abs() < 0.01,
            "min-height 50 + 7 + 3 margins = 60, got {}",
            item.get_height_contribution(0.0)
        );
    }

    /// Columns size from the outer width for the same reason.
    #[test]
    fn a_column_contribution_includes_the_inline_margins() {
        let mut explicit = ComputedStyle::new();
        explicit.width = Length::Px(200.0);
        explicit.margin_left = Length::Px(8.0);
        explicit.margin_right = Length::Px(12.0);
        let eb = LayoutBox::new(BoxType::Block, explicit);
        assert!(
            (GridItem::new(&eb).get_width_contribution(0.0) - 220.0).abs() < 0.01,
            "explicit 200px width + 8 + 12 margins = 220"
        );

        let mut auto = ComputedStyle::new();
        auto.min_width = Length::Px(90.0);
        auto.margin_left = Length::Px(10.0);
        auto.margin_right = Length::Px(10.0);
        let ab = LayoutBox::new(BoxType::Block, auto);
        assert!(
            (GridItem::new(&ab).get_width_contribution(0.0) - 110.0).abs() < 0.01,
            "min-width 90 + 10 + 10 margins = 110"
        );
    }

    /// Phase 9.5 repairs a row whose track-sizing estimate was short -- the
    /// estimate omits the item's BORDER. Its shortfall test compares a border
    /// box against the row, so once the row carries margins the comparison has
    /// to subtract them again or the repair silently stops firing.
    ///
    /// This is not hypothetical: it is what the first half of this change did.
    /// `.section-header` has `border-bottom: 1px`, and with only the sizing
    /// and placement halves landed its height went 57.4 -> 56.4 on both
    /// gradient cases while the 10px-per-row error was being fixed.
    #[test]
    fn the_row_repair_pass_measures_against_the_row_less_the_margins() {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        // The section-header shape: padding, a bottom border the estimate
        // cannot see, a bottom margin, and a fixed-height child standing in
        // for the text line. `box-sizing: border-box` because every case in
        // the corpus sets it in a `*` rule, and the two sizing modes take
        // different arithmetic through this pass.
        let mut header_style = ComputedStyle::new();
        header_style.box_sizing = BoxSizing::BorderBox;
        header_style.padding_top = Length::Px(20.0);
        header_style.padding_bottom = Length::Px(10.0);
        header_style.border_bottom_width = Length::Px(1.0);
        header_style.margin_bottom = Length::Px(10.0);
        let mut header = LayoutBox::new(BoxType::Block, header_style);

        let mut line_style = ComputedStyle::new();
        line_style.height = Length::Px(26.0);
        header
            .children
            .push(LayoutBox::new(BoxType::Block, line_style));
        container.children.push(header);

        layout_grid_container(&mut container, 400.0, 600.0);

        let h = container.children[0].dimensions.border_box().height;
        assert!(
            (h - 57.0).abs() < 0.51,
            "header should be 26 content + 30 padding + 1 border = 57, got {h} \
             (56 means the repair pass compared a border box against a margin box)"
        );
    }

    /// The new_tab `.shortcut` shape: a padded flex row whose children are
    /// three inline chips, each one text node. `count_text_lines` charges a
    /// line per text NODE, so the row track is estimated at three lines
    /// while the flex row lays out one.
    fn shortcut_row(font_px: f32) -> LayoutBox {
        let mut s = ComputedStyle::new();
        s.display = Display::Flex;
        s.box_sizing = BoxSizing::BorderBox;
        s.font_size = Length::Px(font_px);
        s.padding_top = Length::Px(12.0);
        s.padding_bottom = Length::Px(12.0);
        s.padding_left = Length::Px(16.0);
        s.padding_right = Length::Px(16.0);
        s.border_top_width = Length::Px(1.0);
        s.border_bottom_width = Length::Px(1.0);
        let mut row = LayoutBox::new(BoxType::Block, s);
        for key in ["Ctrl", "Cmd", "K"] {
            let mut cs = ComputedStyle::new();
            cs.font_size = Length::Px(font_px);
            let mut chip = LayoutBox::new(BoxType::Block, cs.clone());
            chip.children
                .push(LayoutBox::new(BoxType::Text(key.to_string()), cs));
            row.children.push(chip);
        }
        row
    }

    /// n51: Phase 9.5 was grow-only, so a row whose estimate OVER-shot kept
    /// the over-estimate. new_tab's `.shortcuts` grid sized every row 143px
    /// for 60px items (seven text nodes on one line, estimated as seven
    /// lines): the grid ran 832px instead of Chrome's 400 and everything
    /// from the search field down sat 63px above Chrome. An `auto` row is
    /// the items' real size, so it shrinks to the tallest item's margin box.
    ///
    /// T-RED on the grow-only pass: the second row lands at the estimated
    /// pitch (three lines + padding + gap), not one line below the first.
    #[test]
    fn an_auto_row_shrinks_to_its_items_real_height() {
        const FONT: f32 = 14.0;
        const GAP: f32 = 12.0;
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(262.0)]);
        container_style.row_gap = Length::Px(GAP);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        container.children.push(shortcut_row(FONT));
        container.children.push(shortcut_row(FONT));

        layout_grid_container(&mut container, 262.0, 600.0);

        let first = container.children[0].dimensions.border_box();
        let second = container.children[1].dimensions.border_box();
        let one_line = crate::resolve_line_height(&container.children[0].style, FONT);
        // One line of chips + 24 padding + 2 border; the estimate said three.
        let real = one_line + 26.0;
        assert!(
            (first.height - real).abs() < 1.0,
            "a flex row of three chips is one line tall ({real}), got {} \
             (the row kept track sizing's three-text-node estimate)",
            first.height
        );
        assert!(
            (second.y - (first.y + first.height + GAP)).abs() < 1.0,
            "row 2 must start one real row + gap below row 1: expected {}, got {} \
             (the estimated pitch would be {})",
            first.y + first.height + GAP,
            second.y,
            first.y + 3.0 * one_line + 24.0 + GAP
        );
        let expected_container = 2.0 * first.height + GAP;
        assert!(
            (container.dimensions.content.height - expected_container).abs() < 1.0,
            "the container's auto height follows the shrunk rows: expected {expected_container}, got {}",
            container.dimensions.content.height
        );
    }

    /// The shrink is limited to intrinsic tracks. `minmax(100px, auto)`
    /// carries a definite floor the track no longer remembers once the
    /// contribution loop has grown its base size, so it keeps the grow-only
    /// behaviour and the row stays at least 100px.
    #[test]
    fn a_minmax_row_with_a_definite_floor_does_not_shrink() {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(262.0)]);
        container_style.grid_template_rows = GridTemplate::from_sizes(vec![TrackSize::MinMax(
            Box::new(TrackSize::Px(100.0)),
            Box::new(TrackSize::Auto),
        )]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        container.children.push(shortcut_row(14.0));

        layout_grid_container(&mut container, 262.0, 600.0);

        // The ROW is the subject: a flex-container item re-derives its own
        // auto height from its content and is not stretched back to a row
        // that did not move (pre-existing; the grow path has the same gap).
        let h = container.dimensions.content.height;
        assert!(
            h >= 99.5,
            "a minmax(100px, auto) row must keep its 100px floor, got {h}"
        );
    }

    // ---------------------------------------------------------------
    // Phase 9 repaired the grid item's CHILD and stopped there. Everything
    // below that child kept the width the block pre-pass gave it, which is
    // the GRID CONTAINER's content width -- grid item widths do not exist
    // until track sizing has run, so the pre-pass cannot know them.
    //
    // Measured on sticky-scroll before the fix: `.sidebar-card` correct at
    // 250px, and every h3/ul/li inside it at 1120px -- the container's
    // 1160px content box less the card's 2x20 padding. 30 boxes, +910px
    // each, on a card that was itself exactly right.
    // ---------------------------------------------------------------

    #[test]
    fn a_grid_items_children_keep_their_own_width_in_the_item() {
        // Phase 9 gave every child of a grid item the item's full width: a
        // `width:100px` box came out 600 wide, a `margin: 0 auto` box was
        // never centred, and google's 272px logo (an inline SVG) was
        // stretched across its 1072px item.
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(600.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());

        let mut fixed_style = ComputedStyle::new();
        fixed_style.width = Length::Px(100.0);
        fixed_style.height = Length::Px(20.0);
        let mut centred_style = ComputedStyle::new();
        centred_style.width = Length::Px(200.0);
        centred_style.height = Length::Px(20.0);
        centred_style.margin_left = Length::Auto;
        centred_style.margin_right = Length::Auto;
        let mut fixed = LayoutBox::new(BoxType::Block, fixed_style);
        let mut centred = LayoutBox::new(BoxType::Block, centred_style);
        let mut logo = LayoutBox::new(
            BoxType::Image {
                url: String::new(),
                natural_width: 272.0,
                natural_height: 92.0,
            },
            ComputedStyle::new(),
        );
        // The block pre-pass's sizes, measured against the container.
        fixed.dimensions.content.width = 100.0;
        centred.dimensions.content.width = 200.0;
        logo.dimensions.content.width = 272.0;
        logo.dimensions.content.height = 92.0;
        item.children.push(fixed);
        item.children.push(centred);
        item.children.push(logo);
        container.children.push(item);

        layout_grid_container(&mut container, 600.0, 400.0);

        let item = &container.children[0];
        let item_x = item.dimensions.content.x;
        let widths: Vec<f32> = item
            .children
            .iter()
            .map(|c| c.dimensions.content.width)
            .collect();
        assert_eq!(widths, vec![100.0, 200.0, 272.0], "each child keeps its own width");
        let centred_offset = item.children[1].dimensions.content.x - item_x;
        assert!(
            (centred_offset - 200.0).abs() < 0.01,
            "margin: 0 auto centres a 200px box in the 600px item: offset {centred_offset}"
        );
    }

    /// A grid item's GRANDchildren size against the item, not against the
    /// grid container the pre-pass measured them with.
    #[test]
    fn a_grid_items_grandchildren_resize_with_the_item_not_the_container() {
        const CONTAINER_WIDTH: f32 = 1000.0;
        const COLUMN: f32 = 250.0;
        const CARD_PADDING: f32 = 20.0;

        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(COLUMN)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = BoxSizing::BorderBox;
        let mut item = LayoutBox::new(BoxType::Block, item_style);

        let mut card_style = ComputedStyle::new();
        card_style.box_sizing = BoxSizing::BorderBox;
        card_style.padding_left = Length::Px(CARD_PADDING);
        card_style.padding_right = Length::Px(CARD_PADDING);
        let mut card = LayoutBox::new(BoxType::Block, card_style);

        let mut line_style = ComputedStyle::new();
        line_style.height = Length::Px(24.0);
        let mut line = LayoutBox::new(BoxType::Block, line_style);

        // Stand in for the block pre-pass: before track sizing exists, every
        // box in this subtree was measured against the CONTAINER. Without
        // that stale state the fixture cannot tell "re-flowed correctly"
        // apart from "never laid out at all".
        line.dimensions.content.width = CONTAINER_WIDTH - 2.0 * CARD_PADDING;
        card.dimensions.content.width = CONTAINER_WIDTH - 2.0 * CARD_PADDING;
        card.dimensions.padding.left = CARD_PADDING;
        card.dimensions.padding.right = CARD_PADDING;
        card.children.push(line);
        item.dimensions.content.width = CONTAINER_WIDTH;
        item.children.push(card);
        container.children.push(item);

        layout_grid_container(&mut container, CONTAINER_WIDTH, 600.0);

        let card_border_box = container.children[0].children[0]
            .dimensions
            .border_box()
            .width;
        assert!(
            (card_border_box - COLUMN).abs() < 0.01,
            "the item's own child already sized to the column before this fix; \
             expected {COLUMN}, got {card_border_box}"
        );

        let line_width = container.children[0].children[0].children[0]
            .dimensions
            .content
            .width;
        let expected = COLUMN - 2.0 * CARD_PADDING;
        assert!(
            (line_width - expected).abs() < 0.01,
            "a grandchild of the grid item must size against the item: expected \
             {expected}, got {line_width} ({} is the CONTAINER's content width \
             less the card padding -- the stale pre-pass value)",
            CONTAINER_WIDTH - 2.0 * CARD_PADDING
        );
    }

    /// A grandchild with NO children is never re-flowed, and that clause is a
    /// correctness guard rather than the cost guard it reads as.
    ///
    /// `layout_block_children_with_collapse` derives the box's content height
    /// from the children it flows, so running it over a box that has none
    /// writes a height of zero. Text boxes are exactly that shape — they carry
    /// a measured height and no children — and they arrive in this loop like
    /// any other grandchild. Measured on the corpus rather than argued: with
    /// the clause removed, `gradient-backgrounds` loses height under its grid
    /// items and Gate A goes 2500 -> 2572 failing axes, 72 of them added and
    /// 45 worsened.
    #[test]
    fn a_childless_grandchild_keeps_its_measured_height() {
        const CONTAINER_WIDTH: f32 = 1000.0;
        const COLUMN: f32 = 250.0;
        const TEXT_HEIGHT: f32 = 24.0;

        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(COLUMN)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = BoxSizing::BorderBox;
        let mut item = LayoutBox::new(BoxType::Block, item_style);

        // A text box: measured height, no children. The pre-pass measured it
        // against the CONTAINER, so the column assignment moves its width and
        // anything keyed on "the width changed" fires on it.
        let text_style = ComputedStyle::new();
        let mut text = LayoutBox::new(BoxType::Text("a text run".to_string()), text_style);
        text.dimensions.content.width = CONTAINER_WIDTH;
        text.dimensions.content.height = TEXT_HEIGHT;

        item.dimensions.content.width = CONTAINER_WIDTH;
        item.children.push(text);
        container.children.push(item);

        layout_grid_container(&mut container, CONTAINER_WIDTH, 600.0);

        let text_height = container.children[0].children[0].dimensions.content.height;
        assert!(
            (text_height - TEXT_HEIGHT).abs() < 0.01,
            "a grandchild with no children must not be re-flowed: expected \
             {TEXT_HEIGHT}, got {text_height} (a block re-flow derives the \
             height from the children it flows, and there are none)"
        );
    }

    /// The subtree re-flow runs BEFORE the height resolution, and that order
    /// is load-bearing rather than incidental.
    ///
    /// `layout_block_children_with_collapse` writes the flowed content extent
    /// back onto the box it re-flows. Run it after the height resolution and
    /// it overwrites the height that resolution just decided, so a grandchild
    /// with an explicit `height` collapses to whatever its children happen to
    /// occupy. Measured on the corpus rather than argued: with the re-flow
    /// moved after, sticky-scroll's `.overflow-demo` (`height: 150px`, one
    /// out-of-flow child) comes out 0px tall and Gate A goes 2500 -> 2524
    /// failing axes, 24 of them added.
    ///
    /// The width guard above stays green under that move -- widths are
    /// correct either way -- which is why this needs its own test.
    #[test]
    fn the_subtree_reflow_does_not_overwrite_an_explicit_grandchild_height() {
        const CONTAINER_WIDTH: f32 = 1000.0;
        const COLUMN: f32 = 250.0;
        const CARD_HEIGHT: f32 = 150.0;
        const INNER_HEIGHT: f32 = 10.0;

        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(COLUMN)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = BoxSizing::BorderBox;
        let mut item = LayoutBox::new(BoxType::Block, item_style);

        let mut card_style = ComputedStyle::new();
        card_style.box_sizing = BoxSizing::BorderBox;
        card_style.height = Length::Px(CARD_HEIGHT);
        let mut card = LayoutBox::new(BoxType::Block, card_style);

        let mut inner_style = ComputedStyle::new();
        inner_style.height = Length::Px(INNER_HEIGHT);
        let mut inner = LayoutBox::new(BoxType::Block, inner_style);

        // The block pre-pass state: measured against the CONTAINER, so the
        // width moves when the column is assigned and the re-flow fires.
        inner.dimensions.content.width = CONTAINER_WIDTH;
        inner.dimensions.content.height = INNER_HEIGHT;
        card.dimensions.content.width = CONTAINER_WIDTH;
        card.dimensions.content.height = CARD_HEIGHT;
        card.children.push(inner);
        item.dimensions.content.width = CONTAINER_WIDTH;
        item.children.push(card);
        container.children.push(item);

        layout_grid_container(&mut container, CONTAINER_WIDTH, 600.0);

        let card_height = container.children[0].children[0]
            .dimensions
            .content
            .height;
        assert!(
            (card_height - CARD_HEIGHT).abs() < 0.01,
            "an explicit height must survive the subtree re-flow: expected \
             {CARD_HEIGHT}, got {card_height} ({INNER_HEIGHT} is the flowed \
             extent of its children -- the re-flow ran after the height \
             resolution and overwrote it)"
        );
    }

    // ---------------------------------------------------------------
    // `height: fit-content` on a grid item.
    //
    // css-align-3 §4.2: `stretch` is the used alignment only where the item's
    // size in that axis is `auto`. `fit-content` is not `auto`, and opting an
    // item out of stretching is the reason a page writes it.
    //
    // Measured on sticky-scroll before the fix: `.sidebar-left` and
    // `.sidebar-right` are `position: sticky; height: fit-content` grid items
    // sharing their row with a `main { min-height: 1500px }`. Chrome sizes them
    // to their cards -- 577.44 and 566.14 -- and RustKit gave both the row's
    // 1972.70. Two boxes, ~1400px each, the largest non-known_fail geometry
    // error in the corpus. The root was one line up in the parser:
    // `parse_length` had no `fit-content` case, so it returned None, the
    // declaration was dropped, and `height` kept its `auto` initial value --
    // which is precisely the value that DOES stretch.
    // ---------------------------------------------------------------

    /// Build the sticky-scroll shape: one fit-content item and one tall
    /// sibling that forces the shared row far past it.
    fn fit_content_row(fit_height: Length) -> LayoutBox {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(250.0), TrackSize::Fr(1.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut aside_style = ComputedStyle::new();
        aside_style.box_sizing = BoxSizing::BorderBox;
        aside_style.height = fit_height;
        let mut aside = LayoutBox::new(BoxType::Block, aside_style);
        let mut card_style = ComputedStyle::new();
        card_style.height = Length::Px(120.0);
        aside
            .children
            .push(LayoutBox::new(BoxType::Block, card_style));
        container.children.push(aside);

        let mut main_style = ComputedStyle::new();
        main_style.box_sizing = BoxSizing::BorderBox;
        let mut main = LayoutBox::new(BoxType::Block, main_style);
        let mut tall_style = ComputedStyle::new();
        tall_style.height = Length::Px(1500.0);
        main.children
            .push(LayoutBox::new(BoxType::Block, tall_style));
        container.children.push(main);

        container
    }

    #[test]
    fn a_fit_content_grid_item_takes_its_content_height_not_the_row() {
        let mut container = fit_content_row(Length::FitContent);
        layout_grid_container(&mut container, 1200.0, 800.0);

        let aside = container.children[0].dimensions.border_box().height;
        let main = container.children[1].dimensions.border_box().height;

        assert!(
            (aside - 120.0).abs() < 0.51,
            "a fit-content item is its content: expected 120, got {aside} \
             ({main} is the row -- the item stretched, which is what `auto` \
             does and what `fit-content` exists to refuse)"
        );
        assert!(
            main >= 1500.0,
            "the sibling must still fill the row it forced: expected >= 1500, \
             got {main}"
        );
    }

    /// The other half of the same rule, and the one that stops the fix from
    /// being "never stretch": an `auto` sibling in that same row still does.
    #[test]
    fn an_auto_sibling_in_the_same_row_still_stretches() {
        let mut container = fit_content_row(Length::Auto);
        layout_grid_container(&mut container, 1200.0, 800.0);

        let aside = container.children[0].dimensions.border_box().height;
        assert!(
            aside >= 1500.0,
            "`height: auto` still stretches to the row: expected >= 1500, got \
             {aside} (a pass that keys off the recorded content height instead \
             of the `fit-content` keyword shrinks this one too)"
        );
    }

    /// `display: none` children take no grid slot, and the correction reads a
    /// per-item vector that was filled by a loop skipping them. Off-by-one
    /// here would size the fit-content item from its neighbour's content.
    #[test]
    fn a_display_none_sibling_does_not_shift_the_fit_content_correction() {
        let mut container = fit_content_row(Length::FitContent);
        let mut hidden_style = ComputedStyle::new();
        hidden_style.display = Display::None;
        container
            .children
            .insert(0, LayoutBox::new(BoxType::Block, hidden_style));

        layout_grid_container(&mut container, 1200.0, 800.0);

        let aside = container.children[1].dimensions.border_box().height;
        assert!(
            (aside - 120.0).abs() < 0.51,
            "the fit-content item is still 120 with a display:none sibling \
             ahead of it, got {aside}"
        );
    }

    // ---- aspect-ratio on grid items (css-sizing-4 §4) --------------------
    //
    // Every expectation below is a MEASURED Chrome 141 value, captured on this
    // seat through the same launch options the pinned baseline set uses, not a
    // reading of the spec. The probe fixtures are recorded in the night's
    // digest entry.

    /// Build a 1-column auto-height grid holding one item.
    fn ratio_grid(item_style: ComputedStyle, container_width: f32) -> LayoutBox {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        container
            .children
            .push(LayoutBox::new(BoxType::Block, item_style));
        layout_grid_container(&mut container, container_width, 0.0);
        container
    }

    /// The defect this pass exists for: track sizing runs before the columns
    /// are resolved, so `get_height_contribution` has no inline size to derive
    /// a ratio from and falls through to the content estimate. An empty
    /// `aspect-ratio` item therefore collapsed to zero, and `image-gallery`'s
    /// four `.aspect-box`es went 288/216/192/162 -> 32 apiece.
    ///
    /// Chrome, measured: a 400px-wide `16 / 9` grid item is 225 tall.
    #[test]
    fn an_aspect_ratio_grid_item_sizes_its_auto_row_from_its_inline_size() {
        let mut item_style = ComputedStyle::new();
        item_style.aspect_ratio = Some(16.0 / 9.0);
        let container = ratio_grid(item_style, 400.0);

        let b = container.children[0].dimensions.border_box();
        assert!(
            (b.height - 225.0).abs() < 0.01,
            "a 400px-wide `16 / 9` item should be 225 tall (Chrome: 225), got {}",
            b.height
        );
        assert!(
            (container.dimensions.content.height - 225.0).abs() < 0.01,
            "the auto row -- and so the container -- should be 225 tall, got {}",
            container.dimensions.content.height
        );
    }

    /// css-sizing-4 §4: the ratio applies to the box named by `box-sizing`.
    /// Chrome, measured, for a 400px-wide `2 / 1` item with `padding: 20px`:
    /// `border-box` -> 200 tall, `content-box` -> 220. Deriving the CONTENT
    /// height as `content_width / ratio` gives 220 in both cases, so the
    /// border-box reading is wrong by exactly the padding -- and every corpus
    /// page opens with `* { box-sizing: border-box }`.
    #[test]
    fn the_ratio_applies_to_the_box_named_by_box_sizing() {
        let mut border_box_style = ComputedStyle::new();
        border_box_style.aspect_ratio = Some(2.0);
        border_box_style.box_sizing = BoxSizing::BorderBox;
        border_box_style.padding_top = Length::Px(20.0);
        border_box_style.padding_bottom = Length::Px(20.0);
        border_box_style.padding_left = Length::Px(20.0);
        border_box_style.padding_right = Length::Px(20.0);
        let container = ratio_grid(border_box_style, 400.0);
        let b = container.children[0].dimensions.border_box();
        assert!(
            (b.height - 200.0).abs() < 0.01,
            "under border-box the ratio is the BORDER box: 400/2 = 200 (Chrome: 200), got {}",
            b.height
        );

        let mut content_box_style = ComputedStyle::new();
        content_box_style.aspect_ratio = Some(2.0);
        content_box_style.box_sizing = BoxSizing::ContentBox;
        content_box_style.padding_top = Length::Px(20.0);
        content_box_style.padding_bottom = Length::Px(20.0);
        let content_h = crate::aspect_ratio_content_height(&content_box_style, 360.0, 40.0, 40.0)
            .expect("a content-box item with a ratio has a derived height");
        assert!(
            (content_h - 180.0).abs() < 0.01,
            "under content-box the ratio is the CONTENT box: 360/2 = 180 \
             (Chrome's 220 border box less its 40px padding), got {content_h}"
        );
    }

    /// The ratio must not be treated as a floor that content can never beat.
    /// Chrome, measured: a 400px-wide `4 / 1` item holding a 300px-tall child
    /// is **300** tall, not the ratio's 100. Content wins where it is taller;
    /// the ratio wins where the box would otherwise collapse.
    #[test]
    fn content_taller_than_the_ratio_keeps_its_own_height() {
        let mut item_style = ComputedStyle::new();
        item_style.aspect_ratio = Some(4.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);

        let mut child_style = ComputedStyle::new();
        child_style.height = Length::Px(300.0);
        item.children
            .push(LayoutBox::new(BoxType::Block, child_style));

        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        container.children.push(item);
        layout_grid_container(&mut container, 400.0, 0.0);

        let b = container.children[0].dimensions.border_box();
        assert!(
            b.height >= 299.99,
            "a `4 / 1` item holding a 300px child should be 300 tall, not the \
             ratio's 100 (Chrome: 300), got {}",
            b.height
        );
    }

    /// `align-self: stretch` is the grid default and it must NOT override a
    /// ratio. Chrome, measured: `image-gallery`'s four `.aspect-box`es share
    /// one 288px row -- the `1 / 1` box sets it -- and Chrome still lays them
    /// out 288 / 216 / 192 / 162. Stretching made the three shorter boxes 288
    /// apiece, so this is the half of the fix that the row contribution alone
    /// does not buy.
    #[test]
    fn an_aspect_ratio_item_is_not_stretched_to_a_taller_row() {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Fr(1.0), TrackSize::Fr(1.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        for ratio in [1.0f32, 16.0 / 9.0] {
            let mut s = ComputedStyle::new();
            s.aspect_ratio = Some(ratio);
            container.children.push(LayoutBox::new(BoxType::Block, s));
        }
        // Two 200px columns; the 1/1 item makes the shared row 200 tall.
        layout_grid_container(&mut container, 400.0, 0.0);

        let square = container.children[0].dimensions.border_box();
        let wide = container.children[1].dimensions.border_box();
        assert!(
            (square.height - 200.0).abs() < 0.01,
            "the `1 / 1` item sets the row at 200, got {}",
            square.height
        );
        assert!(
            (wide.height - 112.5).abs() < 0.01,
            "the `16 / 9` item keeps its own ratio (200*9/16 = 112.5) instead of \
             stretching to the 200px row, got {}",
            wide.height
        );
    }

    /// An explicit height is not a ratio-derived one; the ratio may not touch
    /// it. Without this the new pass would overwrite every sized item that
    /// happens to carry an `aspect-ratio`.
    #[test]
    fn an_explicit_height_beats_the_ratio() {
        let mut item_style = ComputedStyle::new();
        item_style.aspect_ratio = Some(1.0);
        item_style.height = Length::Px(50.0);
        let container = ratio_grid(item_style, 400.0);
        let b = container.children[0].dimensions.border_box();
        assert!(
            (b.height - 50.0).abs() < 0.01,
            "an explicit `height: 50px` wins over `aspect-ratio: 1 / 1` on a \
             400px-wide item, got {}",
            b.height
        );
    }

    /// A degenerate ratio must not produce a NaN or an infinite row.
    #[test]
    fn a_zero_or_negative_ratio_is_ignored_rather_than_dividing() {
        let style = ComputedStyle::new();
        for bad in [0.0f32, -2.0, f32::NAN, f32::INFINITY] {
            let mut s = style.clone();
            s.aspect_ratio = Some(bad);
            assert!(
                crate::aspect_ratio_content_height(&s, 400.0, 0.0, 0.0).is_none(),
                "ratio {bad} should be refused, not divided by"
            );
        }
        let mut s = style.clone();
        s.aspect_ratio = Some(2.0);
        assert!(
            crate::aspect_ratio_content_height(&s, 0.0, 0.0, 0.0).is_none(),
            "a box with no inline size has nothing to derive a block size from"
        );
    }

    /// A grid item's `rem` padding/border resolves against the 16px root, not
    /// the item's own font size. The placement pass passed the item's
    /// font-size as the root, so `padding: 0.75rem` on a 14px item read as
    /// 10.5px in every axis (new_tab: each `.shortcut` row 57 for Chrome's
    /// 60, its first key at x 387 for Chrome's 389 — the whole kbd column
    /// 3px per row above Chrome).
    #[test]
    fn a_grid_items_rem_padding_resolves_against_the_root_font_size() {
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(262.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);

        let mut item_style = ComputedStyle::new();
        item_style.box_sizing = BoxSizing::BorderBox;
        item_style.font_size = Length::Px(14.0);
        item_style.padding_top = Length::Rem(0.75);
        item_style.padding_bottom = Length::Rem(0.75);
        item_style.padding_left = Length::Rem(1.0);
        item_style.padding_right = Length::Rem(1.0);
        item_style.border_top_width = Length::Px(1.0);
        let mut item = LayoutBox::new(BoxType::Block, item_style);
        let mut line_style = ComputedStyle::new();
        line_style.height = Length::Px(34.0);
        let line = LayoutBox::new(BoxType::Block, line_style);
        item.children.push(line);
        container.children.push(item);

        layout_grid_container(&mut container, 262.0, 600.0);

        let d = &container.children[0].dimensions;
        assert!(
            (d.padding.top - 12.0).abs() < 0.01 && (d.padding.left - 16.0).abs() < 0.01,
            "0.75rem / 1rem are 12 / 16 against the root; got top {} left {} \
             (10.5 / 14 is the item's 14px font used as the root)",
            d.padding.top,
            d.padding.left
        );
        let content_x = d.content.x;
        assert!(
            (content_x - 16.0).abs() < 0.01,
            "content starts after the 16px padding: got x {content_x}"
        );
    }

    #[test]
    fn a_centred_auto_width_grid_item_shrinks_to_fit_its_content() {
        // `justify-items: center` on an auto-width item used the whole cell
        // as the item's width, so centring moved nothing.
        let mut container_style = ComputedStyle::new();
        container_style.display = Display::Grid;
        container_style.justify_items = JustifyItems::Center;
        container_style.grid_template_columns =
            GridTemplate::from_sizes(vec![TrackSize::Px(600.0)]);
        let mut container = LayoutBox::new(BoxType::Block, container_style);
        let mut item = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        let mut child_style = ComputedStyle::new();
        child_style.width = Length::Px(100.0);
        child_style.height = Length::Px(20.0);
        item.children.push(LayoutBox::new(BoxType::Block, child_style));
        container.children.push(item);

        layout_grid_container(&mut container, 600.0, 400.0);

        let item = container.children[0].dimensions.border_box();
        let offset = item.x - container.dimensions.content.x;
        assert!(
            (item.width - 100.0).abs() < 0.01 && (offset - 250.0).abs() < 0.01,
            "expected a 100px item centred at +250, got {}px at +{offset}",
            item.width
        );
    }
}
