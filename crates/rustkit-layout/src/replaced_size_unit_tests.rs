//! `width`, `height` and their minima and maxima on a replaced element (an
//! inline `<svg>`, an `<img>`) in font-relative and viewport units.
//! `layout_image_in` read `px` and percentages only: every other unit was
//! `auto`, so `svg { width: 1em; height: 1em }` came out at the viewBox size
//! (24x24 for 14x14) and `max-width: 1em` let an unsized icon fill its
//! container. `min-width` and `min-height` were not read at all. Every
//! expected number was measured on the oracle Chromium 143 at a 1280x800
//! viewport with `font: 14px Arial` (`getBoundingClientRect`); each case runs
//! through both entry points, `layout` and `layout_with_collapse`.

use super::*;

/// A replaced box with a 14px font, as an icon in the measured pages.
fn replaced(natural: f32, edit: impl Fn(&mut ComputedStyle)) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.font_size = Length::Px(14.0);
    edit(&mut s);
    LayoutBox::new(
        BoxType::Image {
            url: String::new(),
            natural_width: natural,
            natural_height: natural,
        },
        s,
    )
}

fn laid_out_size(root: LayoutBox, collapse: bool) -> (f32, f32) {
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    let mut root = root;
    root.set_viewport(1280.0, 800.0);
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
    let c = body.children[0].dimensions.content;
    (c.width, c.height)
}

fn assert_size(name: &str, build: impl Fn() -> LayoutBox, want: (f32, f32)) {
    for collapse in [false, true] {
        let got = laid_out_size(build(), collapse);
        assert!(
            (got.0 - want.0).abs() < 0.5 && (got.1 - want.1).abs() < 0.5,
            "{name} (collapse = {collapse}): {} x {}, Chromium has {} x {}",
            got.0,
            got.1,
            want.0,
            want.1
        );
    }
}

/// What the engine's tree build gives an inline `<svg viewBox>` with neither
/// axis sized: the containing block's width, and the ratio for the height.
fn fill(s: &mut ComputedStyle) {
    s.width = Length::Percent(100.0);
}

#[test]
fn an_icon_sized_in_rem_takes_that_size() {
    assert_size(
        "svg { width: 2rem; height: 2rem }",
        || {
            replaced(24.0, |s| {
                s.width = Length::Rem(2.0);
                s.height = Length::Rem(2.0);
            })
        },
        (32.0, 32.0),
    );
}

#[test]
fn an_icon_sized_in_em_takes_its_own_font_size() {
    assert_size(
        "svg { width: 1em; height: 1em }",
        || {
            replaced(24.0, |s| {
                s.width = Length::Em(1.0);
                s.height = Length::Em(1.0);
            })
        },
        (14.0, 14.0),
    );
}

#[test]
fn one_axis_in_a_font_or_viewport_unit_carries_the_other_across_the_ratio() {
    assert_size(
        "svg { height: 1em }",
        || replaced(24.0, |s| s.height = Length::Em(1.0)),
        (14.0, 14.0),
    );
    assert_size(
        "svg { width: 2vw }",
        || replaced(24.0, |s| s.width = Length::Vw(2.0)),
        (25.6, 25.6),
    );
    assert_size(
        "img { width: 2em }",
        || replaced(20.0, |s| s.width = Length::Em(2.0)),
        (28.0, 28.0),
    );
    assert_size(
        "img { height: 3rem }",
        || replaced(20.0, |s| s.height = Length::Rem(3.0)),
        (48.0, 48.0),
    );
    assert_size(
        "img { height: 5vh }",
        || replaced(20.0, |s| s.height = Length::Vh(5.0)),
        (40.0, 40.0),
    );
}

#[test]
fn a_comparison_function_is_a_length() {
    assert_size(
        "svg { width: min(2em, 100px) }",
        || {
            replaced(24.0, |s| {
                s.width = Length::Min(Box::new((Length::Em(2.0), Length::Px(100.0))))
            })
        },
        (28.0, 28.0),
    );
}

#[test]
fn a_maximum_in_a_font_unit_bounds_the_box() {
    assert_size(
        "svg { max-width: 1em } with a viewBox and no size",
        || {
            replaced(24.0, |s| {
                fill(s);
                s.max_width = Length::Em(1.0);
            })
        },
        (14.0, 14.0),
    );
    assert_size(
        "svg { max-height: 2rem } with a viewBox and no size",
        || {
            replaced(24.0, |s| {
                fill(s);
                s.max_height = Length::Rem(2.0);
            })
        },
        (32.0, 32.0),
    );
    assert_size(
        "img { max-width: 1em }",
        || replaced(20.0, |s| s.max_width = Length::Em(1.0)),
        (14.0, 14.0),
    );
}

#[test]
fn a_minimum_grows_the_box() {
    assert_size(
        "svg { width: 16px; min-width: 2em }: the auto height follows",
        || {
            replaced(24.0, |s| {
                s.width = Length::Px(16.0);
                s.min_width = Length::Em(2.0);
            })
        },
        (28.0, 28.0),
    );
    assert_size(
        "svg { width: 16px; height: 16px; min-height: 32px }: the set width stays",
        || {
            replaced(24.0, |s| {
                s.width = Length::Px(16.0);
                s.height = Length::Px(16.0);
                s.min_height = Length::Px(32.0);
            })
        },
        (16.0, 32.0),
    );
}

/// Passes before the fix: pixel and percentage sizes, and no minimum.
#[test]
fn pixel_and_percentage_sizes_are_unchanged() {
    assert_size(
        "svg { width: 16px }",
        || replaced(24.0, |s| s.width = Length::Px(16.0)),
        (16.0, 16.0),
    );
    assert_size(
        "svg with a viewBox and no size in a 400px block",
        || replaced(24.0, fill),
        (400.0, 400.0),
    );
    assert_size(
        "img at its natural size",
        || replaced(20.0, |_| {}),
        (20.0, 20.0),
    );
    assert_size(
        "img { max-width: 10px }",
        || replaced(20.0, |s| s.max_width = Length::Px(10.0)),
        (10.0, 10.0),
    );
}
