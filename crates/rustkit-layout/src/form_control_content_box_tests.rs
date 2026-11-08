//! CH2.B17: a content-box text control's author width/height sizes the
//! content box; the border box adds the UA (or author) padding and border.
//!
//! The engine oracle (`form_control_author_size_tests`) is macOS+headless.
//! `percent_height_definite_tests` zeroes padding and sets `border: none`,
//! so it never exercises the UA extras. These pins call
//! `content_box_control_extra` directly on default `--lib`.

use super::*;

fn text_input(input_type: &str) -> FormControlType {
    FormControlType::TextInput {
        value: String::new(),
        placeholder: String::new(),
        input_type: input_type.to_string(),
    }
}

fn textarea() -> FormControlType {
    FormControlType::TextArea {
        value: String::new(),
        placeholder: String::new(),
        rows: 2,
        cols: 20,
    }
}

fn control_box(style: ComputedStyle) -> LayoutBox {
    LayoutBox::new(BoxType::FormControl(text_input("text")), style)
}

#[test]
fn a_content_box_text_input_adds_the_ua_padding_and_border() {
    // Chrome 143 UA: padding 1px 2px, border 2px inset → (8, 6).
    let box_ = control_box(ComputedStyle::new());
    assert_eq!(box_.content_box_control_extra(&text_input("text"), 400.0), (8.0, 6.0));
    assert_eq!(box_.content_box_control_extra(&text_input("password"), 400.0), (8.0, 6.0));
    assert_eq!(box_.content_box_control_extra(&text_input(""), 400.0), (8.0, 6.0));
}

#[test]
fn a_content_box_textarea_adds_its_ua_padding_and_border() {
    // Chrome 143 UA: padding 2px, border 1px solid → (6, 6).
    let box_ = LayoutBox::new(BoxType::FormControl(textarea()), ComputedStyle::new());
    assert_eq!(box_.content_box_control_extra(&textarea(), 400.0), (6.0, 6.0));
}

#[test]
fn border_box_and_non_text_controls_add_nothing() {
    let mut border_box = ComputedStyle::new();
    border_box.box_sizing = BoxSizing::BorderBox;
    let box_ = control_box(border_box);
    assert_eq!(box_.content_box_control_extra(&text_input("text"), 400.0), (0.0, 0.0));
    assert_eq!(box_.content_box_control_extra(&text_input("search"), 400.0), (0.0, 0.0));
    assert_eq!(
        box_.content_box_control_extra(
            &FormControlType::Button {
                label: String::new(),
                button_type: String::from("submit"),
            },
            400.0
        ),
        (0.0, 0.0)
    );
}

#[test]
fn border_none_keeps_the_ua_padding_only() {
    let mut style = ComputedStyle::new();
    style.border_top_style = BorderStyle::None;
    style.border_right_style = BorderStyle::None;
    style.border_bottom_style = BorderStyle::None;
    style.border_left_style = BorderStyle::None;
    let box_ = control_box(style);
    // padding 1px 2px, no border → (4, 2).
    assert_eq!(box_.content_box_control_extra(&text_input("text"), 400.0), (4.0, 2.0));
}

#[test]
fn an_author_padding_side_replaces_the_ua_value() {
    let mut style = ComputedStyle::new();
    style.padding_left = Length::Px(0.0);
    style.padding_right = Length::Px(0.0);
    style.padding_top = Length::Px(0.0);
    style.padding_bottom = Length::Px(0.0);
    let box_ = control_box(style);
    // Author 0 padding (not unset Zero) + UA 2px borders → (4, 4).
    assert_eq!(box_.content_box_control_extra(&text_input("text"), 400.0), (4.0, 4.0));
}
