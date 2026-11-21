//! Window layout for the native Win32 shell, as plain numbers.
//!
//! Same split as `tabs.rs` and `shortcuts.rs`: the arithmetic lives here, away
//! from HWNDs and the engine, so it can be tested without a window.
//!
//! The model is the WebView2 shell's `apply_layout` (main.rs), because
//! `chrome.html` is written for it and is the same file in both shells:
//!
//! * The CHROME view covers the WHOLE window. The tab strip and toolbar draw
//!   along its top, the left sidebar down its left edge, the right panel down
//!   its right edge.
//! * The CONTENT view sits on top of it (it is created after the chrome view,
//!   so it is later in z-order), inset by the sidebar, the chrome bar height
//!   and the right panel. Whatever it does not cover is the chrome showing
//!   through.
//! * The SHELF view sits on top of the content, along its bottom edge.
//!
//! This replaces a layout that made the chrome view a fixed 220px-wide,
//! full-height column. `chrome.html` draws its tab strip and toolbar across
//! the full width, so in a 220px view they were cut off at the right edge,
//! and the sidebar (which starts closed in the HTML) had no room to open.

/// Width of the right-hand panel when it is open, in pixels. The WebView2
/// shell uses the same figure for both side panels.
pub const RIGHT_PANEL_WIDTH: u32 = 220;

/// A rectangle in window client coordinates, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self { x, y, width, height }
    }
}

/// Everything the layout depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutInput {
    pub window_width: u32,
    pub window_height: u32,
    /// Height of the tab strip and toolbar (grows while an overlay is open).
    pub chrome_height: u32,
    /// Height of the shelf (command palette); 0 when closed.
    pub shelf_height: u32,
    /// Width of the left sidebar; 0 when closed.
    pub sidebar_width: u32,
    pub right_panel_open: bool,
    /// Focus mode: `Some(bar)` hides the sidebar, right panel and shelf and leaves only
    /// a strip of `bar` pixels of chrome at the top (a thin reveal zone, or a taller
    /// one while the peek is showing). `None` is the normal layout.
    pub focus_bar: Option<u32>,
}

/// Height of the chrome strip left in focus mode, so the mouse can reach the top
/// edge to peek. The WebView2 shell uses the same figures.
pub const FOCUS_REVEAL_HEIGHT: u32 = 10;
/// Height of the chrome while the focus-mode peek is showing.
pub const FOCUS_PEEK_HEIGHT: u32 = 60;

/// The three view rectangles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub chrome: Rect,
    pub content: Rect,
    pub shelf: Rect,
}

/// Work out where each view goes. Never underflows: a window smaller than its
/// own chrome gets an empty content area rather than a panic or a huge one.
pub fn compute(input: LayoutInput) -> Layout {
    let LayoutInput {
        window_width: width,
        window_height: height,
        chrome_height,
        shelf_height,
        sidebar_width,
        right_panel_open,
        focus_bar,
    } = input;

    // Focus mode drops everything but the strip along the top.
    let (chrome_height, shelf_height, sidebar_width, right_panel_open) = match focus_bar {
        Some(bar) => (bar, 0, 0, false),
        None => (chrome_height, shelf_height, sidebar_width, right_panel_open),
    };

    let sidebar = sidebar_width.min(width);
    let right = if right_panel_open {
        RIGHT_PANEL_WIDTH.min(width - sidebar)
    } else {
        0
    };
    let content_width = width - sidebar - right;
    let chrome_bar = chrome_height.min(height);
    let shelf = shelf_height.min(height - chrome_bar);
    let content_height = height - chrome_bar - shelf;

    Layout {
        chrome: Rect::new(0, 0, width, height),
        content: Rect::new(sidebar as i32, chrome_bar as i32, content_width, content_height),
        shelf: Rect::new(sidebar as i32, (height - shelf) as i32, content_width, shelf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> LayoutInput {
        LayoutInput {
            window_width: 1280,
            window_height: 800,
            chrome_height: 104,
            shelf_height: 0,
            sidebar_width: 0,
            right_panel_open: false,
            focus_bar: None,
        }
    }

    /// The bug this module exists for: the chrome view must span the window,
    /// not a 220px column, or the tab strip is cut off.
    #[test]
    fn focus_mode_leaves_only_the_reveal_strip() {
        let busy = LayoutInput {
            sidebar_width: 220,
            right_panel_open: true,
            shelf_height: 280,
            chrome_height: 460,
            focus_bar: Some(FOCUS_REVEAL_HEIGHT),
            ..input()
        };
        let l = compute(busy);
        assert_eq!(l.content, Rect::new(0, 10, 1280, 790));
        assert_eq!(l.shelf.height, 0);
        // The peek makes the strip taller, still without a sidebar.
        let peek = compute(LayoutInput { focus_bar: Some(FOCUS_PEEK_HEIGHT), ..busy });
        assert_eq!(peek.content, Rect::new(0, 60, 1280, 740));
        // And the chrome still spans the window.
        assert_eq!(l.chrome, Rect::new(0, 0, 1280, 800));
    }

    #[test]
    fn the_chrome_view_spans_the_whole_window() {
        let l = compute(input());
        assert_eq!(l.chrome, Rect::new(0, 0, 1280, 800));
    }

    #[test]
    fn with_everything_closed_content_fills_the_window_below_the_chrome_bar() {
        let l = compute(input());
        assert_eq!(l.content, Rect::new(0, 104, 1280, 696));
    }

    #[test]
    fn an_open_sidebar_insets_content_and_leaves_the_chrome_view_whole() {
        let l = compute(LayoutInput { sidebar_width: 220, ..input() });
        assert_eq!(l.content, Rect::new(220, 104, 1060, 696));
        assert_eq!(l.chrome, Rect::new(0, 0, 1280, 800), "the sidebar draws inside the chrome view");
    }

    #[test]
    fn the_right_panel_narrows_content_from_the_right() {
        let l = compute(LayoutInput { right_panel_open: true, ..input() });
        assert_eq!(l.content, Rect::new(0, 104, 1060, 696));
        let both = compute(LayoutInput { sidebar_width: 300, right_panel_open: true, ..input() });
        assert_eq!(both.content, Rect::new(300, 104, 760, 696));
    }

    #[test]
    fn the_shelf_sits_on_the_bottom_of_the_content_column() {
        let l = compute(LayoutInput { shelf_height: 280, sidebar_width: 220, ..input() });
        assert_eq!(l.shelf, Rect::new(220, 520, 1060, 280));
        assert_eq!(l.content, Rect::new(220, 104, 1060, 416), "content gives up the shelf's rows");
    }

    #[test]
    fn an_expanded_chrome_bar_pushes_content_down() {
        let l = compute(LayoutInput { chrome_height: 460, ..input() });
        assert_eq!(l.content, Rect::new(0, 460, 1280, 340));
    }

    #[test]
    fn a_sidebar_wider_than_the_window_is_clamped() {
        let l = compute(LayoutInput { sidebar_width: 5000, ..input() });
        assert_eq!(l.content.x, 1280);
        assert_eq!(l.content.width, 0);
    }

    #[test]
    fn panels_never_overlap_each_other() {
        // 300px window, 250px sidebar: only 50px is left for the right panel.
        let l = compute(LayoutInput {
            window_width: 300,
            sidebar_width: 250,
            right_panel_open: true,
            ..input()
        });
        assert_eq!(l.content.width, 0);
        assert!(l.content.x as u32 + l.content.width <= 300);
    }

    #[test]
    fn a_window_shorter_than_its_chrome_bar_gets_no_content_and_no_underflow() {
        let l = compute(LayoutInput { window_height: 60, shelf_height: 280, ..input() });
        assert_eq!(l.content.height, 0);
        assert_eq!(l.shelf.height, 0);
        assert_eq!(l.content.y, 60);
    }

    #[test]
    fn a_minimised_zero_size_window_is_all_zeros_not_a_panic() {
        let l = compute(LayoutInput { window_width: 0, window_height: 0, ..input() });
        assert_eq!(l.chrome, Rect::new(0, 0, 0, 0));
        assert_eq!(l.content, Rect::new(0, 0, 0, 0));
    }
}
