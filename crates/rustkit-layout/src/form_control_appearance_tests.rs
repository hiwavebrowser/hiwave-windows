//! Portable pins for CB1b / #1027: `appearance: none` drops a select's
//! arrow well and MenuList chrome; an author frame leaves the control
//! command's background and border transparent so the box decorations win.
//!
//! The pixel oracle in engine `form_control_decoration_tests` is
//! headless-only. These call `form_control_intrinsic_size` and
//! `DisplayList::build` on default `--lib`.

use super::*;
use rustkit_css::{Appearance, CornerRadius};

fn select(options: &[&str]) -> FormControlType {
    FormControlType::Select {
        options: options.iter().map(|o| (*o).to_string()).collect(),
        selected_index: Some(0),
        size: 0,
        selected: Vec::new(),
    }
}

fn bare_select_style() -> ComputedStyle {
    let mut style = ComputedStyle::new();
    // Same control font the menulist width pins in lib.rs use.
    style.font_size = Length::Px(40.0 / 3.0);
    style.font_family = "Arial".to_string();
    style
}

fn dropdown_box(appearance: Appearance, author_frame: bool) -> LayoutBox {
    let mut style = bare_select_style();
    style.appearance = appearance;
    if author_frame {
        let solid = BorderStyle::Solid;
        let w = Length::Px(1.0);
        style.border_top_style = solid;
        style.border_right_style = solid;
        style.border_bottom_style = solid;
        style.border_left_style = solid;
        style.border_top_width = w.clone();
        style.border_right_width = w.clone();
        style.border_bottom_width = w.clone();
        style.border_left_width = w;
        style.border_top_color = Color::new(6, 182, 212, 1.0);
        style.border_right_color = Color::new(6, 182, 212, 1.0);
        style.border_bottom_color = Color::new(6, 182, 212, 1.0);
        style.border_left_color = Color::new(6, 182, 212, 1.0);
        let r = CornerRadius::circular(Length::Px(8.0));
        style.border_top_left_radius = r.clone();
        style.border_top_right_radius = r.clone();
        style.border_bottom_right_radius = r.clone();
        style.border_bottom_left_radius = r;
    }
    let mut box_ = LayoutBox::new(BoxType::FormControl(select(&["Select"])), style);
    box_.dimensions.content = Rect::new(0.0, 0.0, 80.0, 24.0);
    box_
}

#[test]
fn an_appearance_none_select_drops_the_twenty_px_arrow_well() {
    // Absolute widths follow the host font (Linux Arial ≠ CfT macOS), so
    // pin the well itself: Auto is always 20px wider than None for the
    // same options (Chrome 148 form_control_decoration_tests).
    for options in [
        &["Select"][..],
        &["Dark", "Light", "System"][..],
        &["A longer option text"][..],
    ] {
        let auto = bare_select_style();
        let mut none = bare_select_style();
        none.appearance = Appearance::None;
        let auto_w = form_control_intrinsic_size(&auto, &select(options)).0;
        let none_w = form_control_intrinsic_size(&none, &select(options)).0;
        assert_eq!(auto_w - none_w, 20.0, "{options:?}: {auto_w} vs {none_w}");
        assert!(none_w > 0.0, "{options:?} still sizes to its label");
    }
}

#[test]
fn an_appearance_none_dropdown_is_text_not_menulist() {
    let list = DisplayList::build(&dropdown_box(Appearance::None, false));
    let kind = list
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::TextInput { kind, value, .. } if value == "Select" => Some(*kind),
            _ => None,
        })
        .expect("TextInput for the selected option");
    assert_eq!(kind, TextControlKind::Text);

    let list = DisplayList::build(&dropdown_box(Appearance::Auto, false));
    let kind = list
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::TextInput { kind, value, .. } if value == "Select" => Some(*kind),
            _ => None,
        })
        .expect("TextInput for the selected option");
    assert_eq!(kind, TextControlKind::MenuList);
}

#[test]
fn an_author_frame_leaves_the_control_command_background_transparent() {
    let list = DisplayList::build(&dropdown_box(Appearance::None, true));
    let (bg, border) = list
        .commands
        .iter()
        .find_map(|c| match c {
            DisplayCommand::TextInput {
                background_color,
                border_color,
                ..
            } => Some((*background_color, *border_color)),
            _ => None,
        })
        .expect("TextInput command");
    assert_eq!(bg.a, 0.0, "control command must not re-fill the box");
    assert_eq!(
        border.a, 0.0,
        "author frame owns the ring; the command's solid frame stays off"
    );
    assert!(
        list.commands.iter().any(|c| matches!(
            c,
            DisplayCommand::RoundedBorder { .. } | DisplayCommand::Border { .. }
        )),
        "author frame paints via paint_border_ring"
    );
}
