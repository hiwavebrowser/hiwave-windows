//! The width of a replaced box whose `width` is `auto` and whose `height` is
//! given, where no block container works it out: a flex base size and a
//! min/max-content contribution. The height crosses the ratio (css-sizing-4
//! "transferred size"); the natural width is only for a box with neither.
//!
//! reddit's wordmark (H26c) is `<a style="display: flex"><svg viewBox="0 0
//! 514 149" style="height: 22px">` in the header's flex row. Chrome 148 has
//! it 75.89 x 22 in every container below; RustKit had 514 as a row item, 16
//! inside the nested `<a>` and 0 inside an inline-flex one, and 75.89 only
//! as a block child.

use super::*;
use rustkit_css::Display;

const WORDMARK_WIDTH: f32 = 22.0 * 514.0 / 149.0;

fn wordmark() -> LayoutBox {
    let mut style = ComputedStyle::new();
    style.display = Display::InlineBlock;
    style.height = Length::Px(22.0);
    LayoutBox::new(
        BoxType::Image {
            url: "inline-svg:wordmark".to_string(),
            natural_width: 514.0,
            natural_height: 149.0,
        },
        style,
    )
}

fn flex_box(children: Vec<LayoutBox>) -> LayoutBox {
    let mut style = ComputedStyle::new();
    style.display = Display::Flex;
    let mut b = LayoutBox::new(BoxType::Block, style);
    b.children = children;
    b
}

fn laid_out(mut root: LayoutBox) -> LayoutBox {
    root.set_viewport(1280.0, 800.0);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, 600.0, 0.0),
        ..Default::default()
    };
    let mut mc = MarginCollapseContext::new();
    let mut fc = FloatContext::new();
    root.layout_with_collapse(&cb, &mut mc, &mut fc);
    root
}

fn close(got: f32, want: f32) -> bool {
    (got - want).abs() < 0.1
}

#[test]
fn a_row_item_with_a_height_and_a_ratio_takes_its_width_from_them() {
    let row = laid_out(flex_box(vec![wordmark()]));
    let item = &row.children[0].dimensions.content;
    assert!(
        close(item.width, WORDMARK_WIDTH) && close(item.height, 22.0),
        "514x149 at height 22 in a flex row: {} x {}, Chromium has 75.89 x 22",
        item.width,
        item.height
    );
}

#[test]
fn a_row_item_with_an_em_height_and_a_ratio_takes_its_width_from_them() {
    // `svg { height: 1.5em }` at a 20px font: 30 x 30 for a square viewBox.
    let mut icon = wordmark();
    icon.box_type = BoxType::Image {
        url: "inline-svg:icon".to_string(),
        natural_width: 24.0,
        natural_height: 24.0,
    };
    icon.style.font_size = Length::Px(20.0);
    icon.style.height = Length::Em(1.5);
    let row = laid_out(flex_box(vec![icon]));
    let item = &row.children[0].dimensions.content;
    assert!(close(item.width, 30.0), "width {}, Chromium has 30", item.width);
}

#[test]
fn a_flex_link_around_it_is_as_wide_as_it_is() {
    // The page's nesting: the row's item is a flex container holding the svg.
    let row = laid_out(flex_box(vec![flex_box(vec![wordmark()])]));
    let link = &row.children[0];
    assert!(
        close(link.dimensions.content.width, WORDMARK_WIDTH),
        "the link is {} wide, Chromium has 75.89",
        link.dimensions.content.width
    );
    let svg = &link.children[0].dimensions.content;
    assert!(close(svg.width, WORDMARK_WIDTH), "the svg is {} wide, Chromium has 75.89", svg.width);
}

#[test]
fn its_content_contributions_are_that_width() {
    let svg = wordmark();
    let max = crate::grid::estimate_max_content_width(&svg);
    let min = crate::grid::estimate_min_content_width(&svg);
    assert!(close(max, WORDMARK_WIDTH), "max-content {max}");
    assert!(close(min, WORDMARK_WIDTH), "min-content {min}");
}

#[test]
fn a_natural_size_with_no_height_given_contributes_its_natural_width() {
    let mut img = wordmark();
    img.style.height = Length::Auto;
    assert!(close(crate::grid::estimate_max_content_width(&img), 514.0));
    assert!(close(crate::grid::estimate_min_content_width(&img), 514.0));
}

#[test]
fn padding_and_border_are_added_to_the_contribution() {
    let mut svg = wordmark();
    svg.style.padding_left = Length::Px(4.0);
    svg.style.padding_right = Length::Px(6.0);
    let max = crate::grid::estimate_max_content_width(&svg);
    assert!(close(max, WORDMARK_WIDTH + 10.0), "max-content {max}");
}

#[test]
fn a_border_box_height_crosses_the_ratio_as_its_content_height() {
    // 30px border-box height with 4px padding above and below: 22px of
    // content, so the same 75.89 of content width.
    let mut svg = wordmark();
    svg.style.box_sizing = rustkit_css::BoxSizing::BorderBox;
    svg.style.height = Length::Px(30.0);
    svg.style.padding_top = Length::Px(4.0);
    svg.style.padding_bottom = Length::Px(4.0);
    let max = crate::grid::estimate_max_content_width(&svg);
    assert!(close(max, WORDMARK_WIDTH), "max-content {max}");
}

/// `img { max-width: 100% }`: the minimum contribution is 0 (css-sizing-3
/// §5.2.2), so the flex item around it can shrink and the image with it.
/// Chrome 148: a 100x50 image in a `<div>` item of a 60px row is 60 x 30.
#[test]
fn a_percentage_max_width_makes_the_minimum_contribution_zero() {
    let mut img = wordmark();
    img.style.height = Length::Auto;
    img.style.max_width = Length::Percent(100.0);
    assert!(close(crate::grid::estimate_min_content_width(&img), 0.0));
    assert!(close(crate::grid::estimate_max_content_width(&img), 514.0));
}

#[test]
fn a_pixel_max_width_caps_both_contributions() {
    let mut img = wordmark();
    img.style.height = Length::Auto;
    img.style.max_width = Length::Px(200.0);
    assert!(close(crate::grid::estimate_min_content_width(&img), 200.0));
    assert!(close(crate::grid::estimate_max_content_width(&img), 200.0));
}

/// Pins: a width the page gave is still the answer, and a box with no
/// natural size contributes what it did.
#[test]
fn a_given_width_and_an_unloaded_image_are_as_before() {
    let mut sized = wordmark();
    sized.style.width = Length::Px(100.0);
    assert!(close(crate::grid::estimate_max_content_width(&sized), 100.0));
    let row = laid_out(flex_box(vec![sized]));
    assert!(close(row.children[0].dimensions.content.width, 100.0));

    let mut unloaded = wordmark();
    unloaded.box_type = BoxType::Image {
        url: "x.png".to_string(),
        natural_width: 0.0,
        natural_height: 0.0,
    };
    assert!(close(crate::grid::estimate_max_content_width(&unloaded), 0.0));
}

// ---- Review of #659: the height that crosses the ratio is the USED height.
// `min-height` and `max-height` bound it (CSS 2.1 §10.4, §10.7), and a
// percentage resolves against the flex container's definite cross size. The
// first version read `height` alone, so `.logo img { max-height: 22px }`
// answered its natural 514 as a minimum and could no longer shrink. Every
// number is Chromium 143's on the same shape (natural 514 x 149).

/// The 514 x 149 picture with only the style `set` gives it.
fn picture(set: impl FnOnce(&mut ComputedStyle)) -> LayoutBox {
    let mut b = wordmark();
    b.style.height = Length::Auto;
    set(&mut b.style);
    b
}

fn row(width: f32, height: Option<f32>, children: Vec<LayoutBox>) -> LayoutBox {
    let mut b = flex_box(children);
    b.style.width = Length::Px(width);
    if let Some(h) = height {
        b.style.height = Length::Px(h);
    }
    b
}

fn size(b: &LayoutBox) -> (f32, f32) {
    (b.dimensions.content.width, b.dimensions.content.height)
}

#[test]
fn a_max_height_crosses_the_ratio() {
    let img = picture(|s| s.max_height = Length::Px(22.0));
    let row = laid_out(row(300.0, None, vec![img]));
    let (w, h) = size(&row.children[0]);
    assert!(
        close(w, WORDMARK_WIDTH) && close(h, 22.0),
        "max-height 22 in a 300 row: {w} x {h}, Chromium has 75.9 x 22"
    );
}

#[test]
fn a_max_height_picture_leaves_its_sibling_in_the_row() {
    let img = picture(|s| s.max_height = Length::Px(22.0));
    let mut sibling = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    sibling.style.width = Length::Px(250.0);
    sibling.style.height = Length::Px(10.0);
    let row = laid_out(row(300.0, None, vec![img, sibling]));
    let (w, _) = size(&row.children[0]);
    let s = &row.children[1].dimensions.content;
    assert!(close(w, WORDMARK_WIDTH), "the picture is {w} wide, Chromium has 75.9");
    // Chromium also shrinks the sibling to 224.1. Here it stays 250: a box
    // with a pixel width has that width as its automatic minimum, with or
    // without this change, so only its position is asserted.
    assert!(close(s.x, WORDMARK_WIDTH), "the sibling is at x {}, Chromium has 75.9", s.x);
}

#[test]
fn a_max_height_bounds_both_contributions() {
    let img = picture(|s| s.max_height = Length::Px(22.0));
    let max = crate::grid::estimate_max_content_width(&img);
    let min = crate::grid::estimate_min_content_width(&img);
    assert!(close(max, WORDMARK_WIDTH), "max-content {max}");
    assert!(close(min, WORDMARK_WIDTH), "min-content {min}");
}

#[test]
fn a_percentage_height_resolves_against_the_rows_definite_height() {
    let img = picture(|s| s.height = Length::Percent(100.0));
    let row = laid_out(row(300.0, Some(60.0), vec![img]));
    let (w, h) = size(&row.children[0]);
    assert!(
        close(w, 60.0 * 514.0 / 149.0) && close(h, 60.0),
        "height 100% in a 300 x 60 row: {w} x {h}, Chromium has 207 x 60"
    );
}

#[test]
fn a_percentage_max_height_resolves_against_the_rows_definite_height() {
    let img = picture(|s| s.max_height = Length::Percent(100.0));
    let row = laid_out(row(300.0, Some(60.0), vec![img]));
    let (w, h) = size(&row.children[0]);
    assert!(
        close(w, 60.0 * 514.0 / 149.0) && close(h, 60.0),
        "max-height 100% in a 300 x 60 row: {w} x {h}, Chromium has 207 x 60"
    );
}

/// With no base to resolve it, a percentage height does not make the natural
/// width a minimum: the estimators answer what they did before this PR.
#[test]
fn an_unresolved_percentage_height_does_not_raise_the_minimum() {
    let img = picture(|s| s.height = Length::Percent(100.0));
    let min = crate::grid::estimate_min_content_width(&img);
    assert!(min < 1.0, "min-content {min}");
    let img = picture(|s| s.max_height = Length::Percent(100.0));
    let min = crate::grid::estimate_min_content_width(&img);
    assert!(min < 1.0, "min-content {min}");
}

#[test]
fn a_min_height_above_the_height_crosses_the_ratio() {
    let img = picture(|s| {
        s.height = Length::Px(22.0);
        s.min_height = Length::Px(40.0);
    });
    let row = laid_out(row(600.0, None, vec![img]));
    let (w, h) = size(&row.children[0]);
    assert!(
        close(w, 40.0 * 514.0 / 149.0) && close(h, 40.0),
        "height 22, min-height 40: {w} x {h}, Chromium has 138 x 40"
    );
}

#[test]
fn a_min_height_above_the_natural_height_crosses_the_ratio() {
    let img = picture(|s| s.min_height = Length::Px(200.0));
    let row = laid_out(row(900.0, None, vec![img]));
    let (w, h) = size(&row.children[0]);
    assert!(
        close(w, 200.0 * 514.0 / 149.0) && close(h, 200.0),
        "min-height 200: {w} x {h}, Chromium has 689.9 x 200"
    );
}

/// Pins, both from Chromium: a height with a `max-width` or a `min-width`
/// keeps the height (§10.4, the height is specified).
#[test]
fn a_height_with_a_max_or_min_width_keeps_the_height() {
    let img = picture(|s| {
        s.height = Length::Px(22.0);
        s.max_width = Length::Px(50.0);
    });
    let row_a = laid_out(row(300.0, None, vec![img]));
    let (w, h) = size(&row_a.children[0]);
    assert!(close(w, 50.0) && close(h, 22.0), "max-width 50: {w} x {h}");
    let img = picture(|s| {
        s.height = Length::Px(22.0);
        s.min_width = Length::Px(100.0);
    });
    let row_b = laid_out(row(300.0, None, vec![img]));
    let (w, h) = size(&row_b.children[0]);
    assert!(close(w, 100.0) && close(h, 22.0), "min-width 100: {w} x {h}");
}

/// The change the first version made without saying so, pinned: a picture
/// with no size given is its natural width as a row item and does not
/// shrink. Chromium has 1000 x 500 for a 1000 x 500 picture in a 300 row.
#[test]
fn a_natural_sized_picture_does_not_shrink_in_a_row() {
    let mut img = picture(|_| {});
    img.box_type = BoxType::Image {
        url: "picture".to_string(),
        natural_width: 1000.0,
        natural_height: 500.0,
    };
    let row = laid_out(row(300.0, None, vec![img]));
    let (w, h) = size(&row.children[0]);
    assert!(close(w, 1000.0) && close(h, 500.0), "{w} x {h}, Chromium has 1000 x 500");
}
