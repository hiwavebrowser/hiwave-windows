//! Portable S5 (#874) legacy `clip: rect()` geometry pins.
//!
//! `clip_parse_tests` covers parse. The paint oracle
//! `visually_hidden_tests` is macos+headless. These pin the position and
//! box-type gates that actually hide sr-only ink (including form controls).

use super::*;
use rustkit_css::{ClipRect, Length};

fn clipped_box(
    position: Position,
    box_type: BoxType,
    clip: Option<ClipRect>,
    border: Rect,
) -> LayoutBox {
    let mut style = ComputedStyle::new();
    style.clip = clip;
    style.font_size = Length::Px(16.0);
    let mut b = LayoutBox::new(box_type, style);
    b.position = position;
    b.dimensions.content = Rect::new(
        border.x,
        border.y,
        border.width,
        border.height,
    );
    b
}

fn list() -> DisplayList {
    DisplayList::new()
}

#[test]
fn absolute_and_fixed_blocks_take_the_clip_rect() {
    let clip = ClipRect {
        top: Some(Length::Px(0.0)),
        right: Some(Length::Px(0.0)),
        bottom: Some(Length::Px(0.0)),
        left: Some(Length::Px(0.0)),
    };
    let border = Rect::new(10.0, 20.0, 100.0, 50.0);
    let abs = clipped_box(Position::Absolute, BoxType::Block, Some(clip.clone()), border);
    let got = list().clip_rect_clip(&abs).expect("absolute clip");
    assert_eq!((got.x, got.y, got.width, got.height), (10.0, 20.0, 0.0, 0.0));

    let fixed = clipped_box(Position::Fixed, BoxType::Block, Some(clip), border);
    assert!(list().clip_rect_clip(&fixed).is_some());
}

#[test]
fn static_and_relative_ignore_clip() {
    let clip = ClipRect {
        top: Some(Length::Px(0.0)),
        right: Some(Length::Px(0.0)),
        bottom: Some(Length::Px(0.0)),
        left: Some(Length::Px(0.0)),
    };
    let border = Rect::new(0.0, 0.0, 40.0, 40.0);
    for position in [Position::Static, Position::Relative] {
        let b = clipped_box(position, BoxType::Block, Some(clip.clone()), border);
        assert!(
            list().clip_rect_clip(&b).is_none(),
            "{position:?} must ignore clip"
        );
    }
}

#[test]
fn form_controls_clip_like_blocks_and_auto_sides_use_the_border() {
    let clip = ClipRect {
        top: None,
        right: None,
        bottom: None,
        left: None,
    };
    let border = Rect::new(5.0, 7.0, 80.0, 24.0);
    let button = clipped_box(
        Position::Absolute,
        BoxType::FormControl(FormControlType::Button {
            label: String::new(),
            button_type: "button".into(),
        }),
        Some(clip),
        border,
    );
    let got = list().clip_rect_clip(&button).expect("button clip");
    assert_eq!(
        (got.x, got.y, got.width, got.height),
        (5.0, 7.0, 80.0, 24.0),
        "auto sides are the border edges"
    );
}

#[test]
fn inverted_sides_collapse_to_a_zero_size_rect() {
    // Chromium's visually-hidden pattern: right < left or bottom < top.
    let clip = ClipRect {
        top: Some(Length::Px(1.0)),
        right: Some(Length::Px(1.0)),
        bottom: Some(Length::Px(1.0)),
        left: Some(Length::Px(1.0)),
    };
    let border = Rect::new(0.0, 0.0, 200.0, 100.0);
    let b = clipped_box(Position::Absolute, BoxType::Block, Some(clip), border);
    let got = list().clip_rect_clip(&b).expect("inverted clip");
    assert_eq!((got.x, got.y, got.width, got.height), (1.0, 1.0, 0.0, 0.0));
}
