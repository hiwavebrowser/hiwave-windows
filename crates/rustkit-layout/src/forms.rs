//! # Form Element Layout
//!
//! Layout and rendering support for form elements (input, textarea, button, select).

use crate::{ComputedStyle, DisplayCommand, Rect};
use rustkit_css::Color;

/// Input element visual state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputState {
    #[default]
    Normal,
    Hover,
    Focused,
    Disabled,
}

/// Caret rendering information.
#[derive(Debug, Clone)]
pub struct CaretInfo {
    /// X position of the caret.
    pub x: f32,
    /// Y position of the caret (top).
    pub y: f32,
    /// Height of the caret.
    pub height: f32,
    /// Width of the caret (typically 1-2px).
    pub width: f32,
    /// Caret color.
    pub color: Color,
    /// Whether the caret should be visible (for blinking).
    pub visible: bool,
}

impl Default for CaretInfo {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            height: 16.0,
            width: 1.0,
            color: Color::from_rgb(0, 0, 0),
            visible: true,
        }
    }
}

/// Selection rendering information.
#[derive(Debug, Clone)]
pub struct SelectionInfo {
    /// Selection rectangles (may span multiple lines in textarea).
    pub rects: Vec<Rect>,
    /// Selection background color.
    pub color: Color,
}

impl Default for SelectionInfo {
    fn default() -> Self {
        Self {
            rects: Vec::new(),
            color: Color::new(51, 144, 255, 0.5), // Semi-transparent blue
        }
    }
}

/// Layout information for an input element.
#[derive(Debug, Clone)]
pub struct InputLayout {
    /// Border box (outer bounds).
    pub border_box: Rect,
    /// Content box (where text goes).
    pub content_box: Rect,
    /// Padding for text inside the input.
    pub text_padding: f32,
    /// Visual state.
    pub state: InputState,
    /// Caret info (if focused).
    pub caret: Option<CaretInfo>,
    /// Selection info.
    pub selection: Option<SelectionInfo>,
    /// Placeholder visible.
    pub show_placeholder: bool,
}

impl Default for InputLayout {
    fn default() -> Self {
        Self {
            border_box: Rect::zero(),
            content_box: Rect::zero(),
            text_padding: 4.0,
            state: InputState::Normal,
            caret: None,
            selection: None,
            show_placeholder: false,
        }
    }
}

/// Calculate the position of a caret given text.
pub fn calculate_caret_position(
    text: &str,
    caret_index: usize,
    content_box: &Rect,
    font_size: f32,
) -> CaretInfo {
    let text_before_caret = if caret_index <= text.len() {
        &text[..caret_index]
    } else {
        text
    };

    let text_width = estimate_text_width(text_before_caret, font_size);

    CaretInfo {
        x: content_box.x + text_width,
        y: content_box.y,
        height: font_size * 1.2,
        width: 1.0,
        color: Color::from_rgb(0, 0, 0),
        visible: true,
    }
}

/// Calculate selection rectangles.
pub fn calculate_selection_rects(
    text: &str,
    start: usize,
    end: usize,
    content_box: &Rect,
    font_size: f32,
) -> Vec<Rect> {
    if start >= end || start >= text.len() {
        return Vec::new();
    }

    let (start, end) = (start.min(text.len()), end.min(text.len()));
    let text_before = &text[..start];
    let selected_text = &text[start..end];

    let start_x = estimate_text_width(text_before, font_size);
    let selection_width = estimate_text_width(selected_text, font_size);

    vec![Rect::new(
        content_box.x + start_x,
        content_box.y,
        selection_width,
        font_size * 1.2,
    )]
}

/// Estimate text width without proper shaping.
fn estimate_text_width(text: &str, font_size: f32) -> f32 {
    // Rough approximation: average character width is about 0.5-0.6 of font size
    text.chars().count() as f32 * font_size * 0.5
}

/// Generate display commands for an input element.
pub fn render_input(
    layout: &InputLayout,
    value: &str,
    placeholder: &str,
    style: &ComputedStyle,
    is_password: bool,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    // 1. Background
    let bg_color = match layout.state {
        InputState::Disabled => Color::from_rgb(240, 240, 240), // Grayed out
        _ => style.background_color,
    };
    commands.push(DisplayCommand::SolidColor(bg_color, layout.border_box));

    // 2. Border
    let border_color = match layout.state {
        InputState::Focused => Color::from_rgb(51, 144, 255), // Focus blue
        InputState::Hover => Color::from_rgb(100, 100, 100),
        InputState::Disabled => Color::from_rgb(200, 200, 200),
        _ => style.border_top_color,
    };

    commands.push(DisplayCommand::Border {
        color: border_color,
        rect: layout.border_box,
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    });

    // 3. Selection background (behind text)
    if let Some(ref selection) = layout.selection {
        for rect in &selection.rects {
            commands.push(DisplayCommand::SolidColor(selection.color, *rect));
        }
    }

    // 4. Text content
    let font_size = match style.font_size {
        rustkit_css::Length::Px(px) => px,
        _ => 14.0,
    };

    let (text_to_render, text_color) = if layout.show_placeholder && value.is_empty() {
        (
            placeholder.to_string(),
            Color::from_rgb(150, 150, 150), // Placeholder gray
        )
    } else if is_password && !value.is_empty() {
        let dots = "●".repeat(value.chars().count());
        (
            dots,
            if layout.state == InputState::Disabled {
                Color::from_rgb(128, 128, 128)
            } else {
                style.color
            },
        )
    } else {
        (
            value.to_string(),
            if layout.state == InputState::Disabled {
                Color::from_rgb(128, 128, 128)
            } else {
                style.color
            },
        )
    };

    if !text_to_render.is_empty() {
        commands.push(DisplayCommand::Text {
            text: text_to_render,
            x: layout.content_box.x,
            y: layout.content_box.y + font_size, // Baseline
            color: text_color,
            font_size,
            font_family: style.font_family.clone(),
            font_weight: style.font_weight.0,
            font_style: match style.font_style {
                rustkit_css::FontStyle::Normal => 0,
                rustkit_css::FontStyle::Italic => 1,
                rustkit_css::FontStyle::Oblique => 2,
            },
            gradient: None,
            gradient_rect: Rect::default(),
        });
    }

    // 5. Caret (on top of text)
    if let Some(ref caret) = layout.caret {
        if caret.visible {
            commands.push(DisplayCommand::SolidColor(
                caret.color,
                Rect::new(caret.x, caret.y, caret.width, caret.height),
            ));
        }
    }

    commands
}

/// Generate display commands for a button element.
pub fn render_button(
    border_box: Rect,
    label: &str,
    style: &ComputedStyle,
    state: InputState,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    // Background with state-based coloring
    let bg_color = match state {
        InputState::Hover => lighten_color(&style.background_color, 0.1),
        InputState::Focused => style.background_color,
        InputState::Disabled => Color::from_rgb(200, 200, 200),
        _ => style.background_color,
    };
    commands.push(DisplayCommand::SolidColor(bg_color, border_box));

    // Border
    commands.push(DisplayCommand::Border {
        color: style.border_top_color,
        rect: border_box,
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    });

    // Centered text
    let font_size = match style.font_size {
        rustkit_css::Length::Px(px) => px,
        _ => 14.0,
    };

    let text_width = estimate_text_width(label, font_size);
    let text_x = border_box.x + (border_box.width - text_width) / 2.0;
    let text_y = border_box.y + (border_box.height + font_size) / 2.0;

    let text_color = if state == InputState::Disabled {
        Color::from_rgb(128, 128, 128)
    } else {
        style.color
    };

    commands.push(DisplayCommand::Text {
        text: label.to_string(),
        x: text_x,
        y: text_y,
        color: text_color,
        font_size,
        font_family: style.font_family.clone(),
        font_weight: style.font_weight.0,
        font_style: match style.font_style {
            rustkit_css::FontStyle::Normal => 0,
            rustkit_css::FontStyle::Italic => 1,
            rustkit_css::FontStyle::Oblique => 2,
        },
        gradient: None,
        gradient_rect: Rect::default(),
    });

    commands
}

/// Generate display commands for a checkbox.
pub fn render_checkbox(
    border_box: Rect,
    checked: bool,
    indeterminate: bool,
    state: InputState,
) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    // Checkbox box
    let box_size = border_box.height.min(16.0);
    let checkbox_rect = Rect::new(border_box.x, border_box.y, box_size, box_size);

    let bg_color = if checked && state != InputState::Disabled {
        Color::from_rgb(51, 144, 255) // Blue when checked
    } else {
        Color::from_rgb(255, 255, 255)
    };
    commands.push(DisplayCommand::SolidColor(bg_color, checkbox_rect));

    let border_color = match state {
        InputState::Focused => Color::from_rgb(51, 144, 255),
        InputState::Disabled => Color::from_rgb(200, 200, 200),
        _ => Color::from_rgb(150, 150, 150),
    };
    commands.push(DisplayCommand::Border {
        color: border_color,
        rect: checkbox_rect,
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    });

    // Checkmark or indeterminate line
    if checked || indeterminate {
        let check_color = Color::from_rgb(255, 255, 255);
        if indeterminate {
            // Horizontal line
            let line_rect = Rect::new(
                checkbox_rect.x + 3.0,
                checkbox_rect.y + box_size / 2.0 - 1.0,
                box_size - 6.0,
                2.0,
            );
            commands.push(DisplayCommand::SolidColor(check_color, line_rect));
        } else {
            // Simple checkmark (would be better with paths)
            // Left leg
            commands.push(DisplayCommand::SolidColor(
                check_color,
                Rect::new(
                    checkbox_rect.x + 3.0,
                    checkbox_rect.y + box_size / 2.0,
                    2.0,
                    box_size / 3.0,
                ),
            ));
            // Right leg
            commands.push(DisplayCommand::SolidColor(
                check_color,
                Rect::new(
                    checkbox_rect.x + box_size / 3.0,
                    checkbox_rect.y + 3.0,
                    2.0,
                    box_size - 6.0,
                ),
            ));
        }
    }

    commands
}

/// Generate display commands for a radio button.
pub fn render_radio(border_box: Rect, checked: bool, state: InputState) -> Vec<DisplayCommand> {
    let mut commands = Vec::new();

    // For now, render as a circle approximation (square with note about needing circle support)
    let box_size = border_box.height.min(16.0);
    let radio_rect = Rect::new(border_box.x, border_box.y, box_size, box_size);

    // Outer circle (rendered as square for now)
    let bg_color = Color::from_rgb(255, 255, 255);
    commands.push(DisplayCommand::SolidColor(bg_color, radio_rect));

    let border_color = match state {
        InputState::Focused => Color::from_rgb(51, 144, 255),
        InputState::Disabled => Color::from_rgb(200, 200, 200),
        _ => Color::from_rgb(150, 150, 150),
    };
    commands.push(DisplayCommand::Border {
        color: border_color,
        rect: radio_rect,
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    });

    // Inner dot when checked
    if checked {
        let dot_color = if state == InputState::Disabled {
            Color::from_rgb(150, 150, 150)
        } else {
            Color::from_rgb(51, 144, 255)
        };
        let dot_size = box_size * 0.5;
        let dot_rect = Rect::new(
            radio_rect.x + (box_size - dot_size) / 2.0,
            radio_rect.y + (box_size - dot_size) / 2.0,
            dot_size,
            dot_size,
        );
        commands.push(DisplayCommand::SolidColor(dot_color, dot_rect));
    }

    commands
}

/// Lighten a color by a factor (0.0 - 1.0).
fn lighten_color(color: &Color, factor: f32) -> Color {
    let factor = factor.clamp(0.0, 1.0);
    Color::new(
        (color.r as f32 + (255.0 - color.r as f32) * factor) as u8,
        (color.g as f32 + (255.0 - color.g as f32) * factor) as u8,
        (color.b as f32 + (255.0 - color.b as f32) * factor) as u8,
        color.a,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_caret_position() {
        let content_box = Rect::new(10.0, 10.0, 200.0, 20.0);
        let caret = calculate_caret_position("Hello", 5, &content_box, 14.0);

        assert!(caret.x > content_box.x);
        assert_eq!(caret.y, content_box.y);
        assert!(caret.height > 0.0);
    }

    #[test]
    fn test_selection_rects() {
        let content_box = Rect::new(10.0, 10.0, 200.0, 20.0);
        let rects = calculate_selection_rects("Hello World", 0, 5, &content_box, 14.0);

        assert_eq!(rects.len(), 1);
        assert!(rects[0].width > 0.0);
    }

    #[test]
    fn test_render_input() {
        let layout = InputLayout {
            border_box: Rect::new(0.0, 0.0, 200.0, 30.0),
            content_box: Rect::new(4.0, 4.0, 192.0, 22.0),
            state: InputState::Focused,
            caret: Some(CaretInfo::default()),
            ..Default::default()
        };

        let style = ComputedStyle::new();
        let commands = render_input(&layout, "Hello", "placeholder", &style, false);

        assert!(!commands.is_empty());
    }

    #[test]
    fn test_render_button() {
        let rect = Rect::new(0.0, 0.0, 100.0, 30.0);
        let style = ComputedStyle::new();
        let commands = render_button(rect, "Click me", &style, InputState::Normal);

        assert!(!commands.is_empty());
    }

    #[test]
    fn test_render_checkbox() {
        let rect = Rect::new(0.0, 0.0, 16.0, 16.0);

        let unchecked = render_checkbox(rect, false, false, InputState::Normal);
        let checked = render_checkbox(rect, true, false, InputState::Normal);
        let indeterminate = render_checkbox(rect, false, true, InputState::Normal);

        // Checked should have more commands (for checkmark)
        assert!(checked.len() > unchecked.len());
        assert!(indeterminate.len() > unchecked.len());
    }

    #[test]
    fn test_lighten_color() {
        let color = Color::from_rgb(100, 100, 100);
        let lightened = lighten_color(&color, 0.5);

        assert!(lightened.r > color.r);
        assert!(lightened.g > color.g);
        assert!(lightened.b > color.b);
    }
}
