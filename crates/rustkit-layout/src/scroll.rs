//! Scrolling and overflow handling for RustKit.
//!
//! This module implements:
//! - Scroll container detection
//! - Scroll position management
//! - Scrollbar rendering
//! - Scroll APIs (scrollTo, scrollBy, scrollIntoView)
//! - Smooth scrolling animation
//! - Position: sticky handling

use crate::{DisplayCommand, Rect};
use rustkit_css::{Color, Overflow, ScrollbarGutter, ScrollbarWidth};
use std::time::{Duration, Instant};

/// Scroll state for a scroll container.
#[derive(Debug, Clone)]
pub struct ScrollState {
    /// Current scroll position (horizontal).
    pub scroll_x: f32,

    /// Current scroll position (vertical).
    pub scroll_y: f32,

    /// Total scrollable width (content width - viewport width).
    pub scroll_width: f32,

    /// Total scrollable height (content height - viewport height).
    pub scroll_height: f32,

    /// Viewport width.
    pub viewport_width: f32,

    /// Viewport height.
    pub viewport_height: f32,

    /// Content width.
    pub content_width: f32,

    /// Content height.
    pub content_height: f32,

    /// Whether currently animating.
    pub animating: bool,

    /// Animation target position.
    pub target_x: f32,
    pub target_y: f32,

    /// Animation start position.
    pub start_x: f32,
    pub start_y: f32,

    /// Animation start time.
    pub animation_start: Option<Instant>,

    /// Animation duration.
    pub animation_duration: Duration,

    /// Momentum scrolling state.
    pub momentum: Option<ScrollMomentum>,
}

impl ScrollState {
    /// Create a new scroll state.
    pub fn new(viewport_width: f32, viewport_height: f32) -> Self {
        Self {
            scroll_x: 0.0,
            scroll_y: 0.0,
            scroll_width: 0.0,
            scroll_height: 0.0,
            viewport_width,
            viewport_height,
            content_width: viewport_width,
            content_height: viewport_height,
            animating: false,
            target_x: 0.0,
            target_y: 0.0,
            start_x: 0.0,
            start_y: 0.0,
            animation_start: None,
            animation_duration: Duration::from_millis(300),
            momentum: None,
        }
    }

    /// Update content size.
    pub fn set_content_size(&mut self, width: f32, height: f32) {
        self.content_width = width;
        self.content_height = height;
        self.scroll_width = (width - self.viewport_width).max(0.0);
        self.scroll_height = (height - self.viewport_height).max(0.0);

        // Clamp current scroll position
        self.scroll_x = self.scroll_x.clamp(0.0, self.scroll_width);
        self.scroll_y = self.scroll_y.clamp(0.0, self.scroll_height);
    }

    /// Update viewport size.
    pub fn set_viewport_size(&mut self, width: f32, height: f32) {
        self.viewport_width = width;
        self.viewport_height = height;
        self.scroll_width = (self.content_width - width).max(0.0);
        self.scroll_height = (self.content_height - height).max(0.0);

        // Clamp current scroll position
        self.scroll_x = self.scroll_x.clamp(0.0, self.scroll_width);
        self.scroll_y = self.scroll_y.clamp(0.0, self.scroll_height);
    }

    /// Scroll to a position (instant).
    pub fn scroll_to(&mut self, x: f32, y: f32) {
        self.scroll_x = x.clamp(0.0, self.scroll_width);
        self.scroll_y = y.clamp(0.0, self.scroll_height);
        self.animating = false;
        self.momentum = None;
    }

    /// Scroll to a position with smooth animation.
    pub fn scroll_to_smooth(&mut self, x: f32, y: f32, duration: Duration) {
        self.target_x = x.clamp(0.0, self.scroll_width);
        self.target_y = y.clamp(0.0, self.scroll_height);
        self.start_x = self.scroll_x;
        self.start_y = self.scroll_y;
        self.animation_start = Some(Instant::now());
        self.animation_duration = duration;
        self.animating = true;
        self.momentum = None;
    }

    /// Scroll by a delta amount (instant).
    pub fn scroll_by(&mut self, dx: f32, dy: f32) {
        self.scroll_to(self.scroll_x + dx, self.scroll_y + dy);
    }

    /// Scroll by a delta with smooth animation.
    pub fn scroll_by_smooth(&mut self, dx: f32, dy: f32, duration: Duration) {
        self.scroll_to_smooth(self.scroll_x + dx, self.scroll_y + dy, duration);
    }

    /// Update animation state (call each frame).
    pub fn update(&mut self) -> bool {
        // Handle smooth scrolling animation
        if self.animating {
            if let Some(start) = self.animation_start {
                let elapsed = start.elapsed();
                let progress = (elapsed.as_secs_f32() / self.animation_duration.as_secs_f32()).min(1.0);

                // Ease out cubic
                let eased = 1.0 - (1.0 - progress).powi(3);

                self.scroll_x = self.start_x + (self.target_x - self.start_x) * eased;
                self.scroll_y = self.start_y + (self.target_y - self.start_y) * eased;

                if progress >= 1.0 {
                    self.scroll_x = self.target_x;
                    self.scroll_y = self.target_y;
                    self.animating = false;
                    self.animation_start = None;
                }

                return true; // Needs repaint
            }
        }

        // Handle momentum scrolling
        if let Some(ref mut momentum) = self.momentum {
            let decay = 0.95; // Velocity decay per frame
            let threshold = 0.5; // Stop threshold

            self.scroll_x = (self.scroll_x + momentum.velocity_x).clamp(0.0, self.scroll_width);
            self.scroll_y = (self.scroll_y + momentum.velocity_y).clamp(0.0, self.scroll_height);

            momentum.velocity_x *= decay;
            momentum.velocity_y *= decay;

            // Stop if velocity is low or hit bounds
            let at_bounds_x = self.scroll_x <= 0.0 || self.scroll_x >= self.scroll_width;
            let at_bounds_y = self.scroll_y <= 0.0 || self.scroll_y >= self.scroll_height;

            if (momentum.velocity_x.abs() < threshold && momentum.velocity_y.abs() < threshold)
                || (at_bounds_x && at_bounds_y)
            {
                self.momentum = None;
            }

            return true; // Needs repaint
        }

        false
    }

    /// Start momentum scrolling.
    pub fn start_momentum(&mut self, velocity_x: f32, velocity_y: f32) {
        self.animating = false;
        self.momentum = Some(ScrollMomentum {
            velocity_x,
            velocity_y,
        });
    }

    /// Stop all scrolling animation.
    pub fn stop(&mut self) {
        self.animating = false;
        self.momentum = None;
    }

    /// Check if scrollable horizontally.
    pub fn can_scroll_x(&self) -> bool {
        self.scroll_width > 0.0
    }

    /// Check if scrollable vertically.
    pub fn can_scroll_y(&self) -> bool {
        self.scroll_height > 0.0
    }

    /// Get scroll progress (0.0 - 1.0) for horizontal.
    pub fn progress_x(&self) -> f32 {
        if self.scroll_width > 0.0 {
            self.scroll_x / self.scroll_width
        } else {
            0.0
        }
    }

    /// Get scroll progress (0.0 - 1.0) for vertical.
    pub fn progress_y(&self) -> f32 {
        if self.scroll_height > 0.0 {
            self.scroll_y / self.scroll_height
        } else {
            0.0
        }
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new(0.0, 0.0)
    }
}

/// Momentum scrolling state.
#[derive(Debug, Clone)]
pub struct ScrollMomentum {
    /// Horizontal velocity (pixels per frame).
    pub velocity_x: f32,

    /// Vertical velocity (pixels per frame).
    pub velocity_y: f32,
}

/// Scrollbar orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollbarOrientation {
    Horizontal,
    Vertical,
}

/// Scrollbar state for rendering.
#[derive(Debug, Clone)]
pub struct Scrollbar {
    /// Orientation.
    pub orientation: ScrollbarOrientation,

    /// Track rectangle.
    pub track_rect: Rect,

    /// Thumb rectangle.
    pub thumb_rect: Rect,

    /// Whether the scrollbar is hovered.
    pub hovered: bool,

    /// Whether the thumb is being dragged.
    pub dragging: bool,

    /// Drag start position.
    pub drag_start: Option<f32>,

    /// Scroll position at drag start.
    pub drag_start_scroll: f32,
}

impl Scrollbar {
    /// Create a new scrollbar.
    pub fn new(orientation: ScrollbarOrientation, track_rect: Rect) -> Self {
        Self {
            orientation,
            track_rect,
            thumb_rect: Rect::default(),
            hovered: false,
            dragging: false,
            drag_start: None,
            drag_start_scroll: 0.0,
        }
    }

    /// Update thumb position based on scroll state.
    pub fn update_thumb(&mut self, scroll_state: &ScrollState, scrollbar_width: f32) {
        let (content_size, viewport_size, scroll_pos, track_size) = match self.orientation {
            ScrollbarOrientation::Horizontal => (
                scroll_state.content_width,
                scroll_state.viewport_width,
                scroll_state.scroll_x,
                self.track_rect.width,
            ),
            ScrollbarOrientation::Vertical => (
                scroll_state.content_height,
                scroll_state.viewport_height,
                scroll_state.scroll_y,
                self.track_rect.height,
            ),
        };

        if content_size <= viewport_size {
            // No scrolling needed, hide thumb
            self.thumb_rect = Rect::default();
            return;
        }

        // Calculate thumb size (proportional to viewport/content ratio)
        let thumb_size = (viewport_size / content_size * track_size).max(20.0).min(track_size);

        // Calculate thumb position
        let scroll_range = content_size - viewport_size;
        let track_range = track_size - thumb_size;
        let thumb_pos = if scroll_range > 0.0 {
            scroll_pos / scroll_range * track_range
        } else {
            0.0
        };

        self.thumb_rect = match self.orientation {
            ScrollbarOrientation::Horizontal => Rect {
                x: self.track_rect.x + thumb_pos,
                y: self.track_rect.y,
                width: thumb_size,
                height: scrollbar_width,
            },
            ScrollbarOrientation::Vertical => Rect {
                x: self.track_rect.x,
                y: self.track_rect.y + thumb_pos,
                width: scrollbar_width,
                height: thumb_size,
            },
        };
    }

    /// Check if a point is over the thumb.
    pub fn hit_test_thumb(&self, x: f32, y: f32) -> bool {
        x >= self.thumb_rect.x
            && x < self.thumb_rect.x + self.thumb_rect.width
            && y >= self.thumb_rect.y
            && y < self.thumb_rect.y + self.thumb_rect.height
    }

    /// Check if a point is over the track.
    pub fn hit_test_track(&self, x: f32, y: f32) -> bool {
        x >= self.track_rect.x
            && x < self.track_rect.x + self.track_rect.width
            && y >= self.track_rect.y
            && y < self.track_rect.y + self.track_rect.height
    }

    /// Start dragging the thumb.
    pub fn start_drag(&mut self, pos: f32, current_scroll: f32) {
        self.dragging = true;
        self.drag_start = Some(pos);
        self.drag_start_scroll = current_scroll;
    }

    /// Update drag position and return new scroll position.
    pub fn update_drag(&self, pos: f32, scroll_state: &ScrollState) -> Option<f32> {
        if !self.dragging {
            return None;
        }

        let drag_start = self.drag_start?;
        let delta = pos - drag_start;

        let (content_size, viewport_size, track_size) = match self.orientation {
            ScrollbarOrientation::Horizontal => (
                scroll_state.content_width,
                scroll_state.viewport_width,
                self.track_rect.width,
            ),
            ScrollbarOrientation::Vertical => (
                scroll_state.content_height,
                scroll_state.viewport_height,
                self.track_rect.height,
            ),
        };

        let thumb_size = (viewport_size / content_size * track_size).max(20.0).min(track_size);
        let track_range = track_size - thumb_size;
        let scroll_range = content_size - viewport_size;

        if track_range > 0.0 {
            let scroll_delta = delta / track_range * scroll_range;
            Some((self.drag_start_scroll + scroll_delta).clamp(0.0, scroll_range))
        } else {
            None
        }
    }

    /// End dragging.
    pub fn end_drag(&mut self) {
        self.dragging = false;
        self.drag_start = None;
    }
}

/// Render scrollbars for a scroll container.
pub fn render_scrollbars(
    scroll_state: &ScrollState,
    container_rect: Rect,
    scrollbar_width_setting: ScrollbarWidth,
    _scrollbar_gutter: ScrollbarGutter,
    scrollbar_color: Option<(Color, Color)>,
    overflow_x: Overflow,
    overflow_y: Overflow,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    let scrollbar_width = match scrollbar_width_setting {
        ScrollbarWidth::Auto => 12.0,
        ScrollbarWidth::Thin => 8.0,
        ScrollbarWidth::None => return commands,
    };

    let (thumb_color, track_color) = scrollbar_color.unwrap_or((
        Color::new(128, 128, 128, 0.6),
        Color::new(200, 200, 200, 0.3),
    ));

    let show_vertical = overflow_y.is_scrollable() && scroll_state.can_scroll_y();
    let show_horizontal = overflow_x.is_scrollable() && scroll_state.can_scroll_x();

    // Vertical scrollbar
    if show_vertical {
        let track_rect = Rect {
            x: container_rect.x + container_rect.width - scrollbar_width,
            y: container_rect.y,
            width: scrollbar_width,
            height: container_rect.height - if show_horizontal { scrollbar_width } else { 0.0 },
        };

        // Track background
        commands.push(DisplayCommand::SolidColor(track_color, track_rect));

        // Thumb
        let mut scrollbar = Scrollbar::new(ScrollbarOrientation::Vertical, track_rect);
        scrollbar.update_thumb(scroll_state, scrollbar_width);

        if scrollbar.thumb_rect.height > 0.0 {
            commands.push(DisplayCommand::SolidColor(thumb_color, scrollbar.thumb_rect));
        }
    }

    // Horizontal scrollbar
    if show_horizontal {
        let track_rect = Rect {
            x: container_rect.x,
            y: container_rect.y + container_rect.height - scrollbar_width,
            width: container_rect.width - if show_vertical { scrollbar_width } else { 0.0 },
            height: scrollbar_width,
        };

        // Track background
        commands.push(DisplayCommand::SolidColor(track_color, track_rect));

        // Thumb
        let mut scrollbar = Scrollbar::new(ScrollbarOrientation::Horizontal, track_rect);
        scrollbar.update_thumb(scroll_state, scrollbar_width);

        if scrollbar.thumb_rect.width > 0.0 {
            commands.push(DisplayCommand::SolidColor(thumb_color, scrollbar.thumb_rect));
        }
    }

    // Corner piece if both scrollbars are shown
    if show_vertical && show_horizontal {
        let corner_rect = Rect {
            x: container_rect.x + container_rect.width - scrollbar_width,
            y: container_rect.y + container_rect.height - scrollbar_width,
            width: scrollbar_width,
            height: scrollbar_width,
        };
        commands.push(DisplayCommand::SolidColor(track_color, corner_rect));
    }

    commands
}

/// Check if an element should create a scroll container.
pub fn is_scroll_container(overflow_x: Overflow, overflow_y: Overflow) -> bool {
    overflow_x.is_scrollable() || overflow_y.is_scrollable()
}

/// Calculate scroll into view position.
pub fn calculate_scroll_into_view(
    element_rect: Rect,
    viewport_rect: Rect,
    scroll_state: &ScrollState,
    align_x: ScrollAlignment,
    align_y: ScrollAlignment,
) -> (f32, f32) {
    let mut new_scroll_x = scroll_state.scroll_x;
    let mut new_scroll_y = scroll_state.scroll_y;

    // Horizontal alignment
    let elem_left = element_rect.x - viewport_rect.x + scroll_state.scroll_x;
    let elem_right = elem_left + element_rect.width;
    let view_right = viewport_rect.width;

    match align_x {
        ScrollAlignment::Start => {
            new_scroll_x = elem_left;
        }
        ScrollAlignment::Center => {
            new_scroll_x = elem_left - (viewport_rect.width - element_rect.width) / 2.0;
        }
        ScrollAlignment::End => {
            new_scroll_x = elem_right - viewport_rect.width;
        }
        ScrollAlignment::Nearest => {
            if elem_left < scroll_state.scroll_x {
                new_scroll_x = elem_left;
            } else if elem_right > scroll_state.scroll_x + view_right {
                new_scroll_x = elem_right - view_right;
            }
        }
    }

    // Vertical alignment
    let elem_top = element_rect.y - viewport_rect.y + scroll_state.scroll_y;
    let elem_bottom = elem_top + element_rect.height;
    let view_bottom = viewport_rect.height;

    match align_y {
        ScrollAlignment::Start => {
            new_scroll_y = elem_top;
        }
        ScrollAlignment::Center => {
            new_scroll_y = elem_top - (viewport_rect.height - element_rect.height) / 2.0;
        }
        ScrollAlignment::End => {
            new_scroll_y = elem_bottom - viewport_rect.height;
        }
        ScrollAlignment::Nearest => {
            if elem_top < scroll_state.scroll_y {
                new_scroll_y = elem_top;
            } else if elem_bottom > scroll_state.scroll_y + view_bottom {
                new_scroll_y = elem_bottom - view_bottom;
            }
        }
    }

    (
        new_scroll_x.clamp(0.0, scroll_state.scroll_width),
        new_scroll_y.clamp(0.0, scroll_state.scroll_height),
    )
}

/// Scroll alignment for scrollIntoView.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollAlignment {
    Start,
    Center,
    End,
    #[default]
    Nearest,
}

/// Handle wheel event and return scroll delta.
pub fn handle_wheel_event(
    delta_x: f32,
    delta_y: f32,
    delta_mode: WheelDeltaMode,
    line_height: f32,
    page_height: f32,
) -> (f32, f32) {
    let multiplier = match delta_mode {
        WheelDeltaMode::Pixel => 1.0,
        WheelDeltaMode::Line => line_height,
        WheelDeltaMode::Page => page_height,
    };

    (delta_x * multiplier, delta_y * multiplier)
}

/// Wheel event delta mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WheelDeltaMode {
    #[default]
    Pixel,
    Line,
    Page,
}

/// Sticky position state.
#[derive(Debug, Clone)]
pub struct StickyState {
    /// Original position before sticking.
    pub original_rect: Rect,

    /// Computed stuck position.
    pub stuck_rect: Option<Rect>,

    /// Sticky offsets (top, right, bottom, left).
    pub offsets: StickyOffsets,

    /// Whether currently stuck.
    pub is_stuck: bool,
}

/// Sticky offsets.
#[derive(Debug, Clone, Copy, Default)]
pub struct StickyOffsets {
    pub top: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
    pub left: Option<f32>,
}

impl StickyState {
    /// Create a new sticky state.
    pub fn new(original_rect: Rect, offsets: StickyOffsets) -> Self {
        Self {
            original_rect,
            stuck_rect: None,
            offsets,
            is_stuck: false,
        }
    }

    /// Update sticky position based on scroll state.
    pub fn update(&mut self, scroll_y: f32, container_rect: Rect) {
        // Simplified sticky: only handle top sticky
        if let Some(top_offset) = self.offsets.top {
            let threshold = self.original_rect.y - top_offset;

            if scroll_y > threshold {
                // Element should stick
                let max_y = container_rect.y + container_rect.height - self.original_rect.height;
                let sticky_y = (container_rect.y + top_offset).min(max_y);

                self.stuck_rect = Some(Rect {
                    x: self.original_rect.x,
                    y: sticky_y,
                    width: self.original_rect.width,
                    height: self.original_rect.height,
                });
                self.is_stuck = true;
            } else {
                self.stuck_rect = None;
                self.is_stuck = false;
            }
        }
    }

    /// Get the effective rect (stuck or original).
    pub fn effective_rect(&self) -> Rect {
        self.stuck_rect.unwrap_or(self.original_rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_state_creation() {
        let state = ScrollState::new(800.0, 600.0);
        assert_eq!(state.viewport_width, 800.0);
        assert_eq!(state.viewport_height, 600.0);
        assert_eq!(state.scroll_x, 0.0);
        assert_eq!(state.scroll_y, 0.0);
    }

    #[test]
    fn test_scroll_to() {
        let mut state = ScrollState::new(800.0, 600.0);
        state.set_content_size(1600.0, 1200.0);

        state.scroll_to(100.0, 200.0);
        assert_eq!(state.scroll_x, 100.0);
        assert_eq!(state.scroll_y, 200.0);

        // Test clamping
        state.scroll_to(2000.0, 2000.0);
        assert_eq!(state.scroll_x, 800.0); // 1600 - 800
        assert_eq!(state.scroll_y, 600.0); // 1200 - 600
    }

    #[test]
    fn test_scroll_by() {
        let mut state = ScrollState::new(800.0, 600.0);
        state.set_content_size(1600.0, 1200.0);

        state.scroll_by(50.0, 100.0);
        assert_eq!(state.scroll_x, 50.0);
        assert_eq!(state.scroll_y, 100.0);

        state.scroll_by(50.0, 100.0);
        assert_eq!(state.scroll_x, 100.0);
        assert_eq!(state.scroll_y, 200.0);
    }

    #[test]
    fn test_scroll_progress() {
        let mut state = ScrollState::new(100.0, 100.0);
        state.set_content_size(200.0, 200.0);

        state.scroll_to(50.0, 75.0);
        assert!((state.progress_x() - 0.5).abs() < 0.01);
        assert!((state.progress_y() - 0.75).abs() < 0.01);
    }

    #[test]
    fn test_scrollbar_thumb_position() {
        let mut state = ScrollState::new(800.0, 600.0);
        state.set_content_size(800.0, 1200.0);

        let track_rect = Rect {
            x: 788.0,
            y: 0.0,
            width: 12.0,
            height: 600.0,
        };

        let mut scrollbar = Scrollbar::new(ScrollbarOrientation::Vertical, track_rect);
        scrollbar.update_thumb(&state, 12.0);

        // Thumb should be at top, size should be proportional
        assert_eq!(scrollbar.thumb_rect.y, 0.0);
        assert!(scrollbar.thumb_rect.height > 0.0);
        assert!(scrollbar.thumb_rect.height < 600.0);
    }

    #[test]
    fn test_overflow_is_scrollable() {
        assert!(Overflow::Scroll.is_scrollable());
        assert!(Overflow::Auto.is_scrollable());
        assert!(!Overflow::Hidden.is_scrollable());
        assert!(!Overflow::Visible.is_scrollable());
    }

    #[test]
    fn test_wheel_delta_modes() {
        let line_height = 20.0;
        let page_height = 600.0;

        let (_dx, dy) = handle_wheel_event(0.0, 3.0, WheelDeltaMode::Line, line_height, page_height);
        assert_eq!(dy, 60.0);

        let (_dx, dy) = handle_wheel_event(0.0, 1.0, WheelDeltaMode::Page, line_height, page_height);
        assert_eq!(dy, 600.0);
    }

    #[test]
    fn test_sticky_state() {
        let original = Rect { x: 0.0, y: 100.0, width: 200.0, height: 50.0 };
        let offsets = StickyOffsets { top: Some(0.0), ..Default::default() };
        let mut sticky = StickyState::new(original, offsets);

        let container = Rect { x: 0.0, y: 0.0, width: 800.0, height: 600.0 };

        // Not scrolled - should not be stuck
        sticky.update(0.0, container);
        assert!(!sticky.is_stuck);

        // Scrolled past threshold - should stick
        sticky.update(150.0, container);
        assert!(sticky.is_stuck);
        assert_eq!(sticky.effective_rect().y, 0.0);
    }
}


// ---------------------------------------------------------------------------
// Styled, classic scrollbars (hub row CH2.E7)
// ---------------------------------------------------------------------------

use crate::{BorderRadius, CornerRadius, LayoutBox};
use rustkit_css::{ComputedStyle, ScrollbarPart, WebkitScrollbar};

/// How a box's scrollbars are styled, when they are drawn at all. Without
/// `::-webkit-scrollbar` rules or the standard properties a scrollbar is
/// the platform's overlay one (macOS), which takes no space and is not in
/// the display list.
#[derive(Debug, Clone, Copy)]
pub enum ScrollbarLook<'a> {
    /// `::-webkit-scrollbar*` parts.
    Webkit(&'a WebkitScrollbar),
    /// `scrollbar-width: auto | thin` with `scrollbar-color` (or the
    /// default colours): Chrome's classic bar, without arrow buttons.
    Standard { thickness: f32, thumb: Color, track: Color },
}

/// Chrome's default light-scheme classic colours.
const STANDARD_THUMB: Color = Color { r: 0xc1, g: 0xc1, b: 0xc1, a: 1.0 };
const STANDARD_TRACK: Color = Color { r: 0xf1, g: 0xf1, b: 0xf1, a: 1.0 };

static SCROLLBARS_HIDDEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hide every styled scrollbar in this process, as Chrome's
/// `--hide-scrollbars` does (bars take no space and paint nothing). The
/// Chrome baselines are captured with it (Playwright passes it by
/// default), so `parity-capture` turns it on; `RUSTKIT_HIDE_SCROLLBARS=1`
/// does the same for any other binary.
pub fn set_scrollbars_hidden(hidden: bool) {
    SCROLLBARS_HIDDEN.store(hidden, std::sync::atomic::Ordering::Relaxed);
}

/// Whether styled scrollbars are hidden in this process
/// (`set_scrollbars_hidden`, or `RUSTKIT_HIDE_SCROLLBARS=1`).
pub fn scrollbars_hidden() -> bool {
    static FROM_ENV: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    SCROLLBARS_HIDDEN.load(std::sync::atomic::Ordering::Relaxed)
        || *FROM_ENV.get_or_init(|| std::env::var("RUSTKIT_HIDE_SCROLLBARS").is_ok_and(|v| v == "1"))
}

impl<'a> ScrollbarLook<'a> {
    /// The look a style gives, Chrome's precedence: `scrollbar-width: none`
    /// draws nothing; a non-`auto` `scrollbar-width` or `scrollbar-color`
    /// turns the `::-webkit-scrollbar` rules off (Chrome 121+).
    pub fn of(style: &'a ComputedStyle) -> Option<Self> {
        if scrollbars_hidden() {
            return None;
        }
        let standard = |thickness| ScrollbarLook::Standard {
            thickness,
            thumb: style.scrollbar_color.map_or(STANDARD_THUMB, |c| c.0),
            track: style.scrollbar_color.map_or(STANDARD_TRACK, |c| c.1),
        };
        match style.scrollbar_width {
            ScrollbarWidth::None => None,
            ScrollbarWidth::Thin => Some(standard(rustkit_css::scrollbar::THIN_SCROLLBAR_PX)),
            ScrollbarWidth::Auto if style.scrollbar_color.is_some() => {
                Some(standard(rustkit_css::scrollbar::CLASSIC_SCROLLBAR_PX))
            }
            ScrollbarWidth::Auto => style.webkit_scrollbar.as_deref().map(ScrollbarLook::Webkit),
        }
    }

    /// (vertical bar width, horizontal bar height).
    pub fn thickness(&self) -> (f32, f32) {
        match self {
            ScrollbarLook::Webkit(w) => (w.vertical_thickness(), w.horizontal_thickness()),
            ScrollbarLook::Standard { thickness, .. } => (*thickness, *thickness),
        }
    }
}

/// How an axis's `overflow` shows a scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarRule {
    Never,
    /// `auto`: when the content overflows on the axis.
    WhenOverflowing,
    /// `scroll`: always.
    Always,
}

/// (rule for the vertical bar, rule for the horizontal bar). `visible`
/// beside a non-`visible` axis computes to `auto`.
pub fn bar_rules(style: &ComputedStyle) -> (BarRule, BarRule) {
    let rule = |own: Overflow, other: Overflow| match own {
        Overflow::Scroll => BarRule::Always,
        Overflow::Auto => BarRule::WhenOverflowing,
        Overflow::Visible if !matches!(other, Overflow::Visible | Overflow::Clip) => BarRule::WhenOverflowing,
        _ => BarRule::Never,
    };
    (rule(style.overflow_y, style.overflow_x), rule(style.overflow_x, style.overflow_y))
}

/// Where one bar of a box is, in the box's own (unscrolled) coordinates,
/// and what the thumb does on it.
#[derive(Debug, Clone, Copy)]
pub struct BarGeometry {
    pub orientation: ScrollbarOrientation,
    /// The whole bar (`::-webkit-scrollbar`'s box): the track's rect.
    pub track: Rect,
    /// The thumb, `None` when there is nothing to scroll.
    pub thumb: Option<Rect>,
    /// What one pixel of thumb travel scrolls.
    pub scroll_per_px: f32,
    /// The visible length along the axis (a page step is 7/8 of it).
    pub visible: f32,
}

/// Chrome's `ScrollbarTheme::ThumbLength`: the visible fraction of the
/// track, rounded, never under `min`.
fn thumb_length(visible: f32, total: f32, track: f32, min: f32) -> f32 {
    if total <= 0.0 {
        return track;
    }
    (visible / total * track).round().max(min).min(track)
}

/// Chrome's `ScrollbarTheme::ThumbPosition`: truncated, and a whole pixel
/// once the box is scrolled at all.
fn thumb_position(scroll: f32, max: f32, track: f32, thumb: f32) -> f32 {
    if max <= 0.0 {
        return 0.0;
    }
    let pos = scroll.max(0.0) * (track - thumb) / max;
    if pos > 0.0 && pos < 1.0 {
        1.0
    } else {
        pos.trunc()
    }
}

/// The shortest thumb: Chrome 153 draws a 100000px document's custom thumb
/// 17px long whatever the bar's width or the track's length
/// (`tools/parity_oracle/scrollbar_geometry_log.mjs`, `long_doc`).
pub const MIN_THUMB_PX: f32 = 17.0;

/// Chrome's page step for a track press: 7/8 of what is visible.
pub fn page_step(visible: f32) -> f32 {
    (visible * 0.875).max(1.0)
}

/// The bars of one scroller, from its padding box, how far it can scroll,
/// where it is scrolled to and which bars show. Shared by the painter and
/// the engine's press handling, so the two cannot disagree.
pub fn bar_geometry(
    padding_box: Rect,
    (vertical, horizontal): (f32, f32),
    max_scroll: (f32, f32),
    scroll: (f32, f32),
) -> Vec<BarGeometry> {
    let mut bars = Vec::new();
    let pb = padding_box;
    if vertical > 0.0 {
        let track = Rect { x: pb.x + pb.width, y: pb.y, width: vertical, height: pb.height };
        bars.push(axis_bar(ScrollbarOrientation::Vertical, track, pb.height, max_scroll.1, scroll.1));
    }
    if horizontal > 0.0 {
        let track = Rect { x: pb.x, y: pb.y + pb.height, width: pb.width, height: horizontal };
        bars.push(axis_bar(ScrollbarOrientation::Horizontal, track, pb.width, max_scroll.0, scroll.0));
    }
    bars
}

fn axis_bar(
    orientation: ScrollbarOrientation,
    track: Rect,
    visible: f32,
    max: f32,
    scroll: f32,
) -> BarGeometry {
    let vertical = orientation == ScrollbarOrientation::Vertical;
    let track_len = if vertical { track.height } else { track.width };
    let length = thumb_length(visible, visible + max, track_len, MIN_THUMB_PX.min(track_len));
    let pos = thumb_position(scroll, max, track_len, length);
    let thumb = (max > 0.0 && length > 0.0).then(|| {
        if vertical {
            Rect { x: track.x, y: track.y + pos, width: track.width, height: length }
        } else {
            Rect { x: track.x + pos, y: track.y, width: length, height: track.height }
        }
    });
    let travel = track_len - length;
    BarGeometry {
        orientation,
        track,
        thumb,
        scroll_per_px: if travel > 0.0 { max / travel } else { 0.0 },
        visible,
    }
}

impl LayoutBox {
    /// This box's classic scrollbar look, if it is a scroller that has one.
    pub fn scrollbar_look(&self) -> Option<ScrollbarLook<'_>> {
        if !self.is_element_scroller() {
            return None;
        }
        ScrollbarLook::of(&self.style)
    }

    /// The space this box's classic scrollbars take, (vertical bar width,
    /// horizontal bar height): an `overflow: scroll` axis always, an
    /// `auto` one once the overflow pass found it overflowing
    /// (`scrollbar_overflows`). Layout keeps it as a gutter inside the border (`Dimensions::scrollbar`), out of
    /// the content box, as Chrome does.
    pub fn scrollbar_reserve(&self) -> (f32, f32) {
        let Some(look) = self.scrollbar_look() else {
            return (0.0, 0.0);
        };
        let (v, h) = look.thickness();
        let (rule_v, rule_h) = bar_rules(&self.style);
        let (over_v, over_h) = self.scrollbar_overflows.get();
        let shows = |rule, over| match rule {
            BarRule::Always => true,
            BarRule::WhenOverflowing => over,
            BarRule::Never => false,
        };
        (
            if shows(rule_v, over_v) { v } else { 0.0 },
            if shows(rule_h, over_h) { h } else { 0.0 },
        )
    }

    /// The overflow pass, after a layout: mark each `auto` scroller with a
    /// styled scrollbar that overflows on an axis whose bar it has not
    /// reserved yet. With `unmark`, also clear a bar (one a previous layout
    /// left, `apply_scrollbar_marks`) whose box no longer overflows even
    /// with it. True when a mark changed and the layout must run again.
    /// Only the first pass unmarks, so the passes end.
    pub fn mark_overflowing_scrollbars(&self, unmark: bool) -> bool {
        let mut changed = false;
        if self.scrollbar_look().is_some() {
            let (rule_v, rule_h) = bar_rules(&self.style);
            let (max_x, max_y) = self.element_scroll_max();
            let (mut over_v, mut over_h) = self.scrollbar_overflows.get();
            let mut settle = |rule: BarRule, over: &mut bool, overflows: bool| {
                if rule != BarRule::WhenOverflowing {
                    return;
                }
                if !*over && overflows {
                    *over = true;
                    changed = true;
                } else if *over && !overflows && unmark {
                    *over = false;
                    changed = true;
                }
            };
            settle(rule_v, &mut over_v, max_y > 0.5);
            settle(rule_h, &mut over_h, max_x > 0.5);
            self.scrollbar_overflows.set((over_v, over_h));
        }
        for child in &self.children {
            changed |= child.mark_overflowing_scrollbars(unmark);
        }
        changed
    }

    /// Whether any box in this tree may grow an `auto` styled scrollbar,
    /// so the overflow pass has anything to find.
    pub fn may_mark_scrollbars(&self) -> bool {
        let here = self.scrollbar_look().is_some() && {
            let (v, h) = bar_rules(&self.style);
            v == BarRule::WhenOverflowing || h == BarRule::WhenOverflowing
        };
        here || self.children.iter().any(LayoutBox::may_mark_scrollbars)
    }

    /// The overflow pass's marks, by node, to carry onto a fresh copy of
    /// the unlaid tree (`apply_scrollbar_marks`).
    pub fn collect_scrollbar_marks(&self, marks: &mut std::collections::HashMap<usize, (bool, bool)>) {
        let mine = self.scrollbar_overflows.get();
        if let (Some(node), true) = (self.node_id, mine != (false, false)) {
            marks.insert(node, mine);
        }
        for child in &self.children {
            child.collect_scrollbar_marks(marks);
        }
    }

    /// Set the marks `collect_scrollbar_marks` gathered on this tree; a box
    /// missing from them is unmarked.
    pub fn apply_scrollbar_marks(&self, marks: &std::collections::HashMap<usize, (bool, bool)>) {
        let mark = self.node_id.and_then(|node| marks.get(&node)).copied();
        self.scrollbar_overflows.set(mark.unwrap_or((false, false)));
        for child in &self.children {
            child.apply_scrollbar_marks(marks);
        }
    }

    /// This scroller's bars as laid out and scrolled now, in its own
    /// coordinates.
    pub fn scrollbar_bars(&self) -> Vec<BarGeometry> {
        let reserve = self.scrollbar_reserve();
        if reserve == (0.0, 0.0) {
            return Vec::new();
        }
        bar_geometry(
            self.dimensions.padding_box(),
            reserve,
            self.element_scroll_max(),
            self.element_scroll.get(),
        )
    }
}

/// Resolve a part's `border-radius` against its own box.
fn part_radius(part: &ScrollbarPart, rect: Rect, font_size: f32) -> BorderRadius {
    let corner = |c: &rustkit_css::CornerRadius| CornerRadius {
        h: c.horizontal.to_px(font_size, 16.0, rect.width),
        v: c.vertical.to_px(font_size, 16.0, rect.height),
    };
    BorderRadius {
        top_left: corner(&part.radius[0]),
        top_right: corner(&part.radius[1]),
        bottom_right: corner(&part.radius[2]),
        bottom_left: corner(&part.radius[3]),
    }
    .fitted(rect.width, rect.height)
}

fn fill(commands: &mut Vec<DisplayCommand>, color: Color, rect: Rect, radius: BorderRadius) {
    if color.a <= 0.0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    if radius.is_zero() {
        commands.push(DisplayCommand::SolidColor(color, rect));
    } else {
        commands.push(DisplayCommand::RoundedRect { color, rect, radius });
    }
}

fn paint_part(commands: &mut Vec<DisplayCommand>, part: &ScrollbarPart, rect: Rect, font_size: f32) {
    if part.hidden {
        return;
    }
    if let Some(color) = part.background {
        fill(commands, color, rect, part_radius(part, rect, font_size));
    }
}

/// Paint a set of bars in a look. `thumb_hovered` picks the
/// `::-webkit-scrollbar-thumb:hover` part.
pub fn paint_bars(
    look: ScrollbarLook<'_>,
    bars: &[BarGeometry],
    thumb_hovered: bool,
    font_size: f32,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();
    for bar in bars {
        match look {
            ScrollbarLook::Webkit(w) => {
                paint_part(&mut commands, &w.bar, bar.track, font_size);
                paint_part(&mut commands, &w.track, bar.track, font_size);
                if let Some(thumb) = bar.thumb {
                    let part = match (&w.thumb_hover, thumb_hovered) {
                        (Some(hover), true) => hover,
                        _ => &w.thumb,
                    };
                    paint_part(&mut commands, part, thumb, font_size);
                }
            }
            ScrollbarLook::Standard { thumb, track, thickness } => {
                fill(&mut commands, track, bar.track, BorderRadius::default());
                if let Some(rect) = bar.thumb {
                    // Chrome insets the classic thumb across the bar and
                    // rounds it fully.
                    let inset = if thickness <= rustkit_css::scrollbar::THIN_SCROLLBAR_PX { 2.0 } else { 3.0 };
                    let rect = match bar.orientation {
                        ScrollbarOrientation::Vertical => {
                            Rect { x: rect.x + inset, width: (rect.width - 2.0 * inset).max(0.0), ..rect }
                        }
                        ScrollbarOrientation::Horizontal => {
                            Rect { y: rect.y + inset, height: (rect.height - 2.0 * inset).max(0.0), ..rect }
                        }
                    };
                    let r = rect.width.min(rect.height) / 2.0;
                    fill(&mut commands, thumb, rect, BorderRadius::uniform(r));
                }
            }
        }
    }
    // The corner, where both bars meet.
    if let (Some(v), Some(h)) = (
        bars.iter().find(|b| b.orientation == ScrollbarOrientation::Vertical),
        bars.iter().find(|b| b.orientation == ScrollbarOrientation::Horizontal),
    ) {
        let corner = Rect { x: v.track.x, y: h.track.y, width: v.track.width, height: h.track.height };
        match look {
            ScrollbarLook::Webkit(w) => paint_part(&mut commands, &w.corner, corner, font_size),
            ScrollbarLook::Standard { track, .. } => fill(&mut commands, track, corner, BorderRadius::default()),
        }
    }
    commands
}

/// A scroller's scrollbars, as the painter lists them after its content.
pub fn scrollbar_commands(layout_box: &LayoutBox) -> Vec<DisplayCommand> {
    let Some(look) = layout_box.scrollbar_look() else {
        return Vec::new();
    };
    let bars = layout_box.scrollbar_bars();
    if bars.is_empty() {
        return Vec::new();
    }
    let font_size = match layout_box.style.font_size {
        rustkit_css::Length::Px(px) => px,
        _ => 16.0,
    };
    paint_bars(look, &bars, layout_box.scrollbar_thumb_hovered.get(), font_size)
}

#[cfg(test)]
mod styled_scrollbar_tests {
    use super::*;

    #[test]
    fn chrome_thumb_geometry_for_a_six_pixel_bar() {
        // 200 visible of 600: a 67px thumb at the top, at 66 after 200.
        let pb = Rect { x: 0.0, y: 0.0, width: 294.0, height: 200.0 };
        let bars = bar_geometry(pb, (6.0, 0.0), (0.0, 400.0), (0.0, 0.0));
        assert_eq!(bars.len(), 1);
        let r = |r: Rect| (r.x, r.y, r.width, r.height);
        assert_eq!(r(bars[0].track), (294.0, 0.0, 6.0, 200.0));
        assert_eq!(r(bars[0].thumb.unwrap()), (294.0, 0.0, 6.0, 67.0));
        let bars = bar_geometry(pb, (6.0, 0.0), (0.0, 400.0), (0.0, 200.0));
        assert_eq!(bars[0].thumb.unwrap().y, 66.0);
        // A drag of 50px scrolls 50 * 400 / 133.
        assert!((bars[0].scroll_per_px * 50.0 - 150.375).abs() < 0.01);
    }

    #[test]
    fn a_long_documents_thumb_is_chromes_seventeen_pixels() {
        let pb = Rect { x: 0.0, y: 0.0, width: 294.0, height: 200.0 };
        let bars = bar_geometry(pb, (6.0, 0.0), (0.0, 99_800.0), (0.0, 0.0));
        assert_eq!(bars[0].thumb.unwrap().height, MIN_THUMB_PX);
    }

    #[test]
    fn nothing_to_scroll_has_no_thumb_and_a_page_is_seven_eighths() {
        let pb = Rect { x: 0.0, y: 0.0, width: 294.0, height: 200.0 };
        let bars = bar_geometry(pb, (6.0, 6.0), (0.0, 0.0), (0.0, 0.0));
        assert_eq!(bars.len(), 2);
        assert!(bars.iter().all(|b| b.thumb.is_none()));
        assert_eq!(page_step(200.0), 175.0);
    }

    #[test]
    fn the_gutter_is_in_the_border_box_but_not_the_border() {
        // R1 on #900: a 6px bar inside a 3px right border on a 294px
        // content box: border box 2 + 294 + 6 + 3 = 305, padding box 294,
        // and `border` keeps the border alone.
        let d = crate::Dimensions {
            content: Rect { x: 12.0, y: 22.0, width: 294.0, height: 200.0 },
            border: crate::EdgeSizes { top: 2.0, right: 3.0, bottom: 2.0, left: 2.0 },
            scrollbar: crate::EdgeSizes { right: 6.0, ..Default::default() },
            ..Default::default()
        };
        assert_eq!(d.border.right, 3.0);
        assert_eq!(d.border_with_gutter().right, 9.0);
        assert_eq!(d.padding_box().width, 294.0);
        let bb = d.border_box();
        assert_eq!((bb.x, bb.width, bb.height), (10.0, 305.0, 204.0));
    }

    #[test]
    fn the_standard_properties_turn_the_webkit_parts_off() {
        let mut style = ComputedStyle::new();
        style.webkit_scrollbar = Some(Box::new(WebkitScrollbar { width: Some(6.0), ..Default::default() }));
        assert!(matches!(ScrollbarLook::of(&style), Some(ScrollbarLook::Webkit(_))));
        style.scrollbar_width = ScrollbarWidth::Thin;
        assert_eq!(ScrollbarLook::of(&style).unwrap().thickness(), (10.0, 10.0));
        style.scrollbar_width = ScrollbarWidth::None;
        assert!(ScrollbarLook::of(&style).is_none());
        style.scrollbar_width = ScrollbarWidth::Auto;
        style.scrollbar_color = Some((Color::BLACK, Color::WHITE));
        assert_eq!(ScrollbarLook::of(&style).unwrap().thickness(), (15.0, 15.0));
    }
}
