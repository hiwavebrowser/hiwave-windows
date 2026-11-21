//! `width: fit-content` on a block: `min(max-content, max(min-content,
//! available))` (css-sizing-3 §3.2), the shrink-to-fit width.
//!
//! `calculate_block_width` had an arm for `auto` only and sent every other
//! value through `length_to_px`, which is 0 for `fit-content`, so the box
//! came out 0 wide (nested layout matrix, `fit-content-width-is-zero`, 30
//! cases). Every expected number is Chromium 141's for the same boxes in a
//! 400px wide block (`getBoundingClientRect`); each case runs through both
//! entry points, `layout` and `layout_with_collapse`.

use super::*;

fn sized(w: f32, h: f32) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::Px(w);
    s.height = Length::Px(h);
    LayoutBox::new(BoxType::Block, s)
}

/// A `width: fit-content` block holding `children`, edited by `edit`.
fn fit_content(children: Vec<LayoutBox>, edit: impl Fn(&mut LayoutBox)) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::FitContent;
    let mut b = LayoutBox::new(BoxType::Block, s);
    b.children = children;
    edit(&mut b);
    b
}

/// The box's border-box x and width once laid out in a 400px wide block.
fn laid_out(root: LayoutBox, collapse: bool) -> (f32, f32) {
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(root);
    body.set_viewport(1280.0, 800.0);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, 400.0, 0.0),
        ..Default::default()
    };
    if collapse {
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        body.layout_with_collapse(&cb, &mut mc, &mut fc);
    } else {
        body.layout(&cb);
    }
    let b = body.children[0].dimensions.border_box();
    (b.x, b.width)
}

fn assert_x_width(name: &str, build: impl Fn() -> LayoutBox, want: (f32, f32)) {
    for collapse in [false, true] {
        let got = laid_out(build(), collapse);
        assert!(
            (got.0 - want.0).abs() < 0.5 && (got.1 - want.1).abs() < 0.5,
            "{name} (collapse = {collapse}): x {} width {}, Chromium has x {} width {}",
            got.0,
            got.1,
            want.0,
            want.1
        );
    }
}

#[test]
fn a_fit_content_block_is_as_wide_as_its_content() {
    assert_x_width(
        "one 70px child",
        || fit_content(vec![sized(70.0, 30.0)], |_| {}),
        (0.0, 70.0),
    );
    assert_x_width(
        "a 50px and a 70px child",
        || fit_content(vec![sized(50.0, 20.0), sized(70.0, 30.0)], |_| {}),
        (0.0, 70.0),
    );
    // min-content wider than the available 400: the content wins.
    assert_x_width(
        "one 500px child",
        || fit_content(vec![sized(500.0, 30.0)], |_| {}),
        (0.0, 500.0),
    );
}

#[test]
fn a_fit_content_block_is_centred_by_auto_margins() {
    assert_x_width(
        "margin: 0 auto",
        || {
            fit_content(vec![sized(70.0, 30.0)], |b| {
                b.style.margin_left = Length::Auto;
                b.style.margin_right = Length::Auto;
            })
        },
        (165.0, 70.0),
    );
}

#[test]
fn a_fit_content_block_keeps_its_padding_border_and_max_width() {
    assert_x_width(
        "border-box, padding 0 10px, border-left 5px",
        || {
            fit_content(vec![sized(70.0, 30.0)], |b| {
                b.style.box_sizing = BoxSizing::BorderBox;
                b.style.padding_left = Length::Px(10.0);
                b.style.padding_right = Length::Px(10.0);
                b.style.border_left_width = Length::Px(5.0);
            })
        },
        (0.0, 95.0),
    );
    assert_x_width(
        "max-width: 50px",
        || {
            fit_content(vec![sized(70.0, 30.0)], |b| {
                b.style.max_width = Length::Px(50.0)
            })
        },
        (0.0, 50.0),
    );
    assert_x_width(
        "min-width: 100px",
        || {
            fit_content(vec![sized(70.0, 30.0)], |b| {
                b.style.min_width = Length::Px(100.0)
            })
        },
        (0.0, 100.0),
    );
}

#[test]
fn a_fit_content_float_is_as_wide_as_its_content() {
    assert_x_width(
        "float: left",
        || fit_content(vec![sized(70.0, 30.0)], |b| b.float = Float::Left),
        (0.0, 70.0),
    );
}
