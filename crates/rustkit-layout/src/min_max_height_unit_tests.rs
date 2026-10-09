//! `min-height` and `max-height` in font-relative and viewport-width units.
//! Only `px`, `vh`, percentages and `calc()` were resolved; every other unit
//! fell through to "no minimum" and "no maximum". GitHub's small buttons are
//! `min-height: var(--base-size-32)`, which is `2rem`, and were 22px tall for
//! 32. Every expected number was measured on the oracle Chromium 143 at a
//! 1280x800 viewport with `font: 14px Arial` (`getBoundingClientRect`); each case
//! runs through both entry points, `layout` and `layout_with_collapse`.

use super::*;
use rustkit_css::Display;

/// `<div style="width: 42px; font-size: 14px"><div style="height: <inner>px">`.
fn holder(display: Display, inner: f32) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.display = display;
    s.width = Length::Px(42.0);
    s.font_size = Length::Px(14.0);
    let mut k = ComputedStyle::new();
    k.width = Length::Px(16.0);
    k.height = Length::Px(inner);
    let mut b = LayoutBox::new(BoxType::Block, s);
    b.children.push(LayoutBox::new(BoxType::Block, k));
    b
}

fn laid_out_height(root: LayoutBox, collapse: bool) -> f32 {
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.set_viewport(1280.0, 800.0);
    let mut root = root;
    root.set_viewport(1280.0, 800.0);
    body.children.push(root);
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
    body.children[0].dimensions.content.height
}

fn assert_height(name: &str, build: impl Fn() -> LayoutBox, want: f32) {
    assert_height_through(name, build, want, &[false, true]);
}

/// A flex or grid container gets its `min-height` from the block pass that
/// follows the flex or grid pass, which only the collapse entry point (the
/// one pages take) runs: through plain `layout` a pixel minimum is lost on
/// them as well. That is not a unit question and is not pinned here.
fn assert_height_through(name: &str, build: impl Fn() -> LayoutBox, want: f32, entries: &[bool]) {
    for &collapse in entries {
        let got = laid_out_height(build(), collapse);
        assert!(
            (got - want).abs() < 0.5,
            "{name} (collapse = {collapse}): height {got}, Chromium has {want}"
        );
    }
}

#[test]
fn a_min_height_in_rem_or_em_is_a_minimum() {
    for display in [Display::Block, Display::Flex, Display::Grid] {
        let entries: &[bool] = if display == Display::Block {
            &[false, true]
        } else {
            &[true]
        };
        assert_height_through(
            &format!("{display:?}, min-height: 2rem"),
            || {
                let mut b = holder(display, 16.0);
                b.style.min_height = Length::Rem(2.0);
                b
            },
            32.0,
            entries,
        );
        assert_height_through(
            &format!("{display:?}, min-height: 2em at 14px"),
            || {
                let mut b = holder(display, 16.0);
                b.style.min_height = Length::Em(2.0);
                b
            },
            28.0,
            entries,
        );
    }
}

#[test]
fn a_min_height_in_vw_is_a_minimum() {
    // 10vw of a 1280px viewport.
    assert_height(
        "min-height: 10vw",
        || {
            let mut b = holder(Display::Block, 16.0);
            b.style.min_height = Length::Vw(10.0);
            b
        },
        128.0,
    );
    // 5vmin of 1280x800.
    assert_height(
        "min-height: 5vmin",
        || {
            let mut b = holder(Display::Block, 16.0);
            b.style.min_height = Length::Vmin(5.0);
            b
        },
        40.0,
    );
}

#[test]
fn a_max_height_in_rem_or_em_is_a_maximum() {
    assert_height(
        "max-height: 2rem over 60px of content",
        || {
            let mut b = holder(Display::Block, 60.0);
            b.style.max_height = Length::Rem(2.0);
            b
        },
        32.0,
    );
    assert_height(
        "max-height: 2em at 14px over 60px of content",
        || {
            let mut b = holder(Display::Block, 60.0);
            b.style.max_height = Length::Em(2.0);
            b
        },
        28.0,
    );
}

#[test]
fn a_pixel_minimum_and_a_content_taller_than_the_minimum_are_unchanged() {
    assert_height(
        "min-height: 32px",
        || {
            let mut b = holder(Display::Block, 16.0);
            b.style.min_height = Length::Px(32.0);
            b
        },
        32.0,
    );
    assert_height(
        "min-height: 2rem under 60px of content",
        || {
            let mut b = holder(Display::Block, 60.0);
            b.style.min_height = Length::Rem(2.0);
            b
        },
        60.0,
    );
}
