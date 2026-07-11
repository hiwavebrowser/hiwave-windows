//! Image layout and rendering for the RustKit layout engine.
//!
//! This module handles:
//! - Computing image layout boxes
//! - Generating display commands for images
//! - Background image positioning and tiling

use crate::{BackgroundRepeat, BackgroundSize, DisplayCommand, ObjectFit, Rect};
use rustkit_css::Color;

/// Information needed to render an image
#[derive(Debug, Clone)]
pub struct ImageLayoutInfo {
    /// URL or cache key
    pub url: String,

    /// Natural dimensions of the image
    pub natural_width: f32,
    pub natural_height: f32,

    /// Whether the image has loaded
    pub is_loaded: bool,

    /// Alt text for broken image display
    pub alt_text: Option<String>,
}

/// Generate display command for an img element
pub fn render_image(
    url: &str,
    container: Rect,
    natural_width: f32,
    natural_height: f32,
    object_fit: ObjectFit,
    object_position: (f32, f32),
    opacity: f32,
) -> DisplayCommand {
    let draw_rect = object_fit.compute_rect(container, natural_width, natural_height, object_position);

    DisplayCommand::Image {
        url: url.to_string(),
        src_rect: draw_rect.src,
        dest_rect: draw_rect.dest,
        object_fit,
        opacity,
    }
}

/// Generate display commands for a background image
pub fn render_background_image(
    url: &str,
    container: Rect,
    image_width: f32,
    image_height: f32,
    size: &BackgroundSize,
    position: (f32, f32),
    repeat: BackgroundRepeat,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    // Calculate the size of the background image
    let (bg_width, bg_height) = size.compute_size(container, image_width, image_height);

    if bg_width == 0.0 || bg_height == 0.0 {
        return commands;
    }

    // Calculate the starting position
    let start_x = container.x + (container.width - bg_width) * position.0;
    let start_y = container.y + (container.height - bg_height) * position.1;

    // Determine tiling
    let (tile_x, tile_y) = match repeat {
        BackgroundRepeat::Repeat => (true, true),
        BackgroundRepeat::RepeatX => (true, false),
        BackgroundRepeat::RepeatY => (false, true),
        BackgroundRepeat::NoRepeat => (false, false),
        BackgroundRepeat::Space => (true, true), // TODO: proper spacing
        BackgroundRepeat::Round => (true, true), // TODO: proper rounding
    };

    // Generate tile positions
    if !tile_x && !tile_y {
        // Single image
        commands.push(DisplayCommand::BackgroundImage {
            url: url.to_string(),
            rect: Rect {
                x: start_x,
                y: start_y,
                width: bg_width,
                height: bg_height,
            },
            size: size.clone(),
            position,
            repeat,
        });
    } else {
        // Tiled images
        let x_start = if tile_x {
            // Find the leftmost position that's visible
            let tiles_left = ((start_x - container.x) / bg_width).ceil() as i32;
            start_x - (tiles_left as f32 * bg_width)
        } else {
            start_x
        };

        let y_start = if tile_y {
            let tiles_up = ((start_y - container.y) / bg_height).ceil() as i32;
            start_y - (tiles_up as f32 * bg_height)
        } else {
            start_y
        };

        let mut y = y_start;
        while y < container.y + container.height {
            let mut x = x_start;
            while x < container.x + container.width {
                // Clip to container bounds
                let tile_rect = Rect {
                    x,
                    y,
                    width: bg_width,
                    height: bg_height,
                };

                // Only emit if visible
                if tile_rect.x + tile_rect.width > container.x
                    && tile_rect.y + tile_rect.height > container.y
                    && tile_rect.x < container.x + container.width
                    && tile_rect.y < container.y + container.height
                {
                    commands.push(DisplayCommand::BackgroundImage {
                        url: url.to_string(),
                        rect: tile_rect,
                        size: size.clone(),
                        position,
                        repeat,
                    });
                }

                if tile_x {
                    x += bg_width;
                } else {
                    break;
                }
            }

            if tile_y {
                y += bg_height;
            } else {
                break;
            }
        }
    }

    commands
}

/// Generate display command for a broken image placeholder
pub fn render_broken_image(
    container: Rect,
    alt_text: Option<&str>,
    text_color: Color,
    bg_color: Color,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    // Background
    commands.push(DisplayCommand::SolidColor(bg_color, container));

    // Border to indicate broken image
    let border_color = Color::new(192, 192, 192, 1.0);
    commands.push(DisplayCommand::Border {
        color: border_color,
        rect: container,
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    });

    // Alt text if available
    if let Some(alt) = alt_text {
        if !alt.is_empty() {
            let padding = 4.0;
            let font_size = 12.0;
            commands.push(DisplayCommand::Text {
                text: alt.to_string(),
                x: container.x + padding,
                y: container.y + padding + font_size,
                color: text_color,
                font_size,
                font_family: "sans-serif".to_string(),
                font_weight: 400,
                font_style: 0,
                gradient: None,
                gradient_rect: Rect::default(),
            });
        }
    }

    // Broken image icon (simple X)
    let icon_size = 16.0_f32.min(container.width - 8.0).min(container.height - 8.0);
    if icon_size > 4.0 {
        let icon_x = container.x + (container.width - icon_size) / 2.0;
        let icon_y = container.y + (container.height - icon_size) / 2.0;

        // Draw X lines using small rectangles
        let line_color = Color::new(128, 128, 128, 1.0);
        let line_thickness = 2.0;

        // Diagonal line 1 (top-left to bottom-right) - simplified as horizontal line
        commands.push(DisplayCommand::SolidColor(
            line_color,
            Rect {
                x: icon_x,
                y: icon_y + icon_size / 2.0 - line_thickness / 2.0,
                width: icon_size,
                height: line_thickness,
            },
        ));

        // Diagonal line 2 (vertical line)
        commands.push(DisplayCommand::SolidColor(
            line_color,
            Rect {
                x: icon_x + icon_size / 2.0 - line_thickness / 2.0,
                y: icon_y,
                width: line_thickness,
                height: icon_size,
            },
        ));
    }

    commands
}

/// Calculate the intrinsic size for an image element
pub fn calculate_intrinsic_size(
    natural_width: Option<f32>,
    natural_height: Option<f32>,
    explicit_width: Option<f32>,
    explicit_height: Option<f32>,
    container_width: f32,
) -> (f32, f32) {
    match (natural_width, natural_height, explicit_width, explicit_height) {
        // Both explicit dimensions
        (_, _, Some(w), Some(h)) => (w, h),

        // Explicit width, calculate height from aspect ratio
        (Some(nw), Some(nh), Some(w), None) if nw > 0.0 => {
            (w, w * nh / nw)
        }

        // Explicit height, calculate width from aspect ratio
        (Some(nw), Some(nh), None, Some(h)) if nh > 0.0 => {
            (h * nw / nh, h)
        }

        // Natural dimensions available
        (Some(nw), Some(nh), None, None) => (nw, nh),

        // Only natural width
        (Some(nw), None, None, None) => (nw, nw),

        // Only natural height
        (None, Some(nh), None, None) => (nh, nh),

        // No dimensions known - use container width and assume square
        _ => {
            let size = container_width.min(300.0); // Default max size
            (size, size)
        }
    }
}

/// Calculate placeholder dimensions while image is loading
pub fn calculate_placeholder_size(
    explicit_width: Option<f32>,
    explicit_height: Option<f32>,
    aspect_ratio: Option<f32>,
    container_width: f32,
) -> (f32, f32) {
    match (explicit_width, explicit_height, aspect_ratio) {
        (Some(w), Some(h), _) => (w, h),
        (Some(w), None, Some(ar)) if ar > 0.0 => (w, w / ar),
        (None, Some(h), Some(ar)) if ar > 0.0 => (h * ar, h),
        (Some(w), None, None) => (w, w), // Assume square
        (None, Some(h), None) => (h, h), // Assume square
        _ => {
            // No hints - use a default placeholder size
            (container_width.min(150.0), 100.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_image() {
        let container = Rect { x: 0.0, y: 0.0, width: 200.0, height: 200.0 };
        let cmd = render_image(
            "http://example.com/image.png",
            container,
            400.0,
            200.0,
            ObjectFit::Contain,
            (0.5, 0.5),
            1.0,
        );

        if let DisplayCommand::Image { dest_rect, .. } = cmd {
            // Should be centered and scaled to fit
            assert!((dest_rect.width - 200.0).abs() < 0.001);
            assert!((dest_rect.height - 100.0).abs() < 0.001);
            assert!((dest_rect.y - 50.0).abs() < 0.001); // Centered vertically
        } else {
            panic!("Expected Image command");
        }
    }

    #[test]
    fn test_background_no_repeat() {
        let container = Rect { x: 0.0, y: 0.0, width: 200.0, height: 200.0 };
        let commands = render_background_image(
            "bg.png",
            container,
            100.0,
            100.0,
            &BackgroundSize::Auto,
            (0.5, 0.5),
            BackgroundRepeat::NoRepeat,
        );

        assert_eq!(commands.len(), 1);
    }

    #[test]
    fn test_background_repeat() {
        let container = Rect { x: 0.0, y: 0.0, width: 200.0, height: 200.0 };
        let commands = render_background_image(
            "bg.png",
            container,
            50.0,
            50.0,
            &BackgroundSize::Auto,
            (0.0, 0.0),
            BackgroundRepeat::Repeat,
        );

        // Should tile 4x4 = 16 times
        assert!(commands.len() >= 16);
    }

    #[test]
    fn test_calculate_intrinsic_size() {
        // Both explicit
        assert_eq!(
            calculate_intrinsic_size(Some(100.0), Some(50.0), Some(200.0), Some(100.0), 400.0),
            (200.0, 100.0)
        );

        // Explicit width, aspect from natural
        let (w, h) = calculate_intrinsic_size(Some(100.0), Some(50.0), Some(200.0), None, 400.0);
        assert!((w - 200.0).abs() < 0.001);
        assert!((h - 100.0).abs() < 0.001);

        // Natural only
        assert_eq!(
            calculate_intrinsic_size(Some(100.0), Some(50.0), None, None, 400.0),
            (100.0, 50.0)
        );
    }

    #[test]
    fn test_broken_image_has_content() {
        let container = Rect { x: 0.0, y: 0.0, width: 100.0, height: 100.0 };
        let commands = render_broken_image(
            container,
            Some("Alt text"),
            Color::new(0, 0, 0, 1.0),
            Color::new(240, 240, 240, 1.0),
        );

        assert!(commands.len() >= 2); // At least background and border
    }
}

