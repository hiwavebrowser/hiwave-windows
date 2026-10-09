//! CSS 2.1 §9.5 / §9.5.1 and CSS Inline 3: a float is out of flow, and the
//! LINE BOXES next to it are shortened to make room for its margin box. A
//! block in normal flow that does not establish a formatting context keeps
//! its full width (its border box runs behind the float) while its lines
//! are shortened; a box that establishes one sits beside the float. Floats
//! of an ancestor's formatting context reach the lines of descendant blocks
//! sharing it. Reduced from hand test H19: Wikipedia's right-floated
//! thumbnail (`figure.mw-halign-right`) and infobox (`table.infobox`), whose
//! neighbouring paragraphs ran underneath them at full width.
//!
//! Text metrics differ between platforms, so these tests assert geometry
//! relations (no line box overlaps a float's margin box; lines beside a
//! float end at its edge; lines below it use the full width) rather than
//! oracle numbers. Each case runs through both entry points, `layout` and
//! `layout_with_collapse`.

use super::*;
use rustkit_css::{Display, Overflow, TextAlign};

const EPS: f32 = 0.5;

const PARA: &str = "A web browser is an application for accessing websites. When a user \
                    requests a web page from a particular website, the browser retrieves \
                    its files from a web server and then displays the page on the user's \
                    screen. Browsers run on a range of devices, including desktops, \
                    laptops, tablets, and smartphones, and the most used one is Chrome.";

fn text(s: &str) -> LayoutBox {
    LayoutBox::new(BoxType::Text(s.to_string()), ComputedStyle::new())
}

fn block(children: Vec<LayoutBox>) -> LayoutBox {
    block_with(|_| {}, children)
}

fn block_with(edit: impl Fn(&mut ComputedStyle), children: Vec<LayoutBox>) -> LayoutBox {
    let mut s = ComputedStyle::new();
    edit(&mut s);
    let mut b = LayoutBox::new(BoxType::Block, s);
    b.children = children;
    b
}

/// `<p>` with the given inline content and no margins.
fn para(children: Vec<LayoutBox>) -> LayoutBox {
    block(children)
}

/// A floated block `width` x `height` (a coloured box standing in for an
/// image), with `edit` for margins.
fn float_box(side: Float, width: f32, height: f32, edit: impl Fn(&mut ComputedStyle)) -> LayoutBox {
    let mut s = ComputedStyle::new();
    s.width = Length::Px(width);
    s.height = Length::Px(height);
    s.float = side;
    edit(&mut s);
    LayoutBox::with_float(BoxType::Block, s, side)
}

fn cleared(mut b: LayoutBox, clear: Clear) -> LayoutBox {
    b.style.clear = clear;
    b.clear = clear;
    b
}

/// Lays `children` out in a `<body>` 400px wide at the origin through the
/// named entry point.
fn laid_out(children: Vec<LayoutBox>, collapse: bool) -> LayoutBox {
    laid_out_in(400.0, |_| {}, children, collapse)
}

fn laid_out_in(
    width: f32,
    edit: impl Fn(&mut ComputedStyle),
    children: Vec<LayoutBox>,
    collapse: bool,
) -> LayoutBox {
    let mut body = block_with(edit, children);
    let cb = Dimensions {
        content: Rect::new(0.0, 0.0, width, 0.0),
        ..Default::default()
    };
    if collapse {
        let mut mc = MarginCollapseContext::new();
        let mut fc = FloatContext::new();
        body.layout_with_collapse(&cb, &mut mc, &mut fc);
    } else {
        body.layout(&cb);
    }
    body
}

/// Every text line box under `b` (in tree order), as page rects.
fn line_rects(b: &LayoutBox) -> Vec<Rect> {
    fn walk(b: &LayoutBox, out: &mut Vec<Rect>) {
        if let BoxType::Text(t) = &b.box_type {
            if t.trim().is_empty() {
                return;
            }
            let c = b.dimensions.content;
            match &b.text_lines {
                Some(lines) => {
                    // Where paint seats each line (`text_line_fragments`).
                    let rects = b.text_line_fragments().unwrap_or_default();
                    for (l, r) in lines.iter().zip(rects) {
                        if !l.text.trim().is_empty() {
                            out.push(r);
                        }
                    }
                }
                None => out.push(c),
            }
            return;
        }
        for child in &b.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(b, &mut out);
    out
}

/// The margin boxes of every float under `b`.
fn float_rects(b: &LayoutBox) -> Vec<Rect> {
    fn walk(b: &LayoutBox, out: &mut Vec<Rect>) {
        if b.float != Float::None {
            out.push(b.dimensions.margin_box());
        }
        for child in &b.children {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(b, &mut out);
    out
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x < b.right() - EPS
        && b.x < a.right() - EPS
        && a.y < b.bottom() - EPS
        && b.y < a.bottom() - EPS
}

fn beside(line: &Rect, f: &Rect) -> bool {
    line.y < f.bottom() - EPS && line.bottom() > f.y + EPS
}

/// No line box under `scope` overlaps a float margin box under `root`.
fn assert_no_line_under_a_float(name: &str, root: &LayoutBox, scope: &LayoutBox) {
    let floats = float_rects(root);
    assert!(!floats.is_empty(), "{name}: no float in the tree");
    let lines = line_rects(scope);
    assert!(!lines.is_empty(), "{name}: no line boxes");
    for f in &floats {
        for l in &lines {
            assert!(
                !overlaps(l, f),
                "{name}: line box {l:?} runs under the float's margin box {f:?}"
            );
        }
    }
}

fn for_both(name: &str, check: impl Fn(&str, bool)) {
    for collapse in [false, true] {
        check(&format!("{name} (collapse = {collapse})"), collapse);
    }
}

/// (a) `<div><figure style="float:right"> <p> <p>`: lines of both
/// paragraphs beside the float end at its left edge; lines below it use the
/// full width; the paragraphs' own boxes keep the full width.
#[test]
fn a_right_float_before_two_paragraphs_shortens_their_lines() {
    for_both("float:right + p + p", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Right, 150.0, 100.0, |_| {}),
                para(vec![text(PARA)]),
                para(vec![text(PARA)]),
            ],
            collapse,
        );
        let f = body.children[0].dimensions.margin_box();
        assert!(
            (f.x - 250.0).abs() < EPS && f.y.abs() < EPS,
            "{name}: float at {f:?}"
        );
        assert_no_line_under_a_float(name, &body, &body);
        for p in &body.children[1..] {
            assert!(
                (p.dimensions.border_box().width - 400.0).abs() < EPS,
                "{name}: a paragraph that is not a formatting root keeps its full width"
            );
        }
        let lines = line_rects(&body);
        assert!(
            lines.iter().any(|l| beside(l, &f)),
            "{name}: no line beside the float"
        );
        for l in lines.iter().filter(|l| beside(l, &f)) {
            assert!(
                l.x.abs() < EPS,
                "{name}: a line beside a right float starts at 0: {l:?}"
            );
        }
        assert!(
            lines
                .iter()
                .any(|l| l.y >= f.bottom() - EPS && l.right() > 250.0 + EPS),
            "{name}: lines below the float use the full width: {lines:?}"
        );
    });
}

/// (b) the same with `float: left`: the lines beside it start at its right
/// edge, the lines below it at 0.
#[test]
fn a_left_float_before_two_paragraphs_moves_their_lines_right() {
    for_both("float:left + p + p", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Left, 150.0, 100.0, |_| {}),
                para(vec![text(PARA)]),
                para(vec![text(PARA)]),
            ],
            collapse,
        );
        let f = body.children[0].dimensions.margin_box();
        assert_no_line_under_a_float(name, &body, &body);
        let lines = line_rects(&body);
        for l in lines.iter().filter(|l| beside(l, &f)) {
            assert!(
                l.x >= 150.0 - EPS,
                "{name}: {l:?} starts inside the left float"
            );
        }
        assert!(
            lines
                .iter()
                .any(|l| l.y >= f.bottom() - EPS && l.x.abs() < EPS),
            "{name}: lines below the float start at the content edge: {lines:?}"
        );
    });
}

/// (c) a float declared in the middle of a paragraph's inline content
/// shortens the line it is anchored on and the lines after it.
#[test]
fn a_float_in_the_middle_of_a_paragraph_shortens_the_following_lines() {
    for_both("text + float:right + text", |name, collapse| {
        let body = laid_out(
            vec![para(vec![
                text("Browsers fetch pages."),
                float_box(Float::Right, 120.0, 80.0, |_| {}),
                text(PARA),
            ])],
            collapse,
        );
        let f = float_rects(&body)[0];
        assert!(
            (f.x - 280.0).abs() < EPS,
            "{name}: the float goes to the right edge: {f:?}"
        );
        assert_no_line_under_a_float(name, &body, &body);
        let lines = line_rects(&body);
        assert!(
            lines.iter().filter(|l| beside(l, &f)).count() >= 2,
            "{name}: several lines run beside the float: {lines:?}"
        );
    });
}

/// (d) a float inside one paragraph intrudes into the next paragraph.
#[test]
fn a_float_in_one_paragraph_shortens_the_next_paragraph() {
    for_both("p(float) + p", |name, collapse| {
        let body = laid_out(
            vec![
                para(vec![
                    text("Short."),
                    float_box(Float::Right, 150.0, 120.0, |_| {}),
                ]),
                para(vec![text(PARA)]),
            ],
            collapse,
        );
        let f = float_rects(&body)[0];
        assert_no_line_under_a_float(name, &body, &body.children[1]);
        assert!(
            line_rects(&body.children[1]).iter().any(|l| beside(l, &f)),
            "{name}: the second paragraph starts beside the float"
        );
    });
}

/// (e) `clear: right` / `clear: both` put a following block below the
/// floats it clears, and its lines then use the full width.
#[test]
fn a_cleared_block_goes_below_the_float_with_full_width_lines() {
    for clear in [Clear::Right, Clear::Both] {
        for_both(
            &format!("float:right + p(clear:{clear:?})"),
            |name, collapse| {
                let body = laid_out(
                    vec![
                        float_box(Float::Right, 150.0, 100.0, |_| {}),
                        cleared(para(vec![text(PARA)]), clear),
                    ],
                    collapse,
                );
                let f = body.children[0].dimensions.margin_box();
                let p = &body.children[1];
                assert!(
                    p.dimensions.border_box().y >= f.bottom() - EPS,
                    "{name}: the cleared block starts below the float: {:?}",
                    p.dimensions.border_box()
                );
                let lines = line_rects(p);
                assert!(
                    lines.iter().any(|l| l.right() > 250.0 + EPS),
                    "{name}: its lines use the full width: {lines:?}"
                );
            },
        );
    }
    for_both(
        "float:left + float:right + p(clear:both)",
        |name, collapse| {
            let body = laid_out(
                vec![
                    float_box(Float::Left, 100.0, 60.0, |_| {}),
                    float_box(Float::Right, 100.0, 100.0, |_| {}),
                    cleared(para(vec![text(PARA)]), Clear::Both),
                ],
                collapse,
            );
            let p = &body.children[2];
            assert!(
                p.dimensions.border_box().y >= 100.0 - EPS,
                "{name}: below both floats"
            );
            assert!(line_rects(p)
                .iter()
                .any(|l| l.x.abs() < EPS && l.right() > 300.0 + EPS));
        },
    );
}

/// (f) a block with `overflow: hidden` establishes a formatting context: it
/// sits beside the float, not under it.
#[test]
fn a_formatting_root_sits_beside_the_float() {
    for_both("float:right + div(overflow:hidden)", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Right, 150.0, 100.0, |_| {}),
                block_with(|s| s.overflow_x = Overflow::Hidden, vec![text(PARA)]),
            ],
            collapse,
        );
        let f = body.children[0].dimensions.margin_box();
        let d = body.children[1].dimensions.border_box();
        assert!(
            d.y < f.bottom(),
            "{name}: beside the float, not below it: {d:?}"
        );
        assert!(
            d.right() <= f.x + EPS,
            "{name}: does not overlap the float: {d:?}"
        );
        assert_no_line_under_a_float(name, &body, &body);
    });
}

/// (g) a left and a right float together narrow lines from both sides.
#[test]
fn left_and_right_floats_narrow_lines_from_both_sides() {
    for_both("float:left + float:right + p", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Left, 100.0, 80.0, |_| {}),
                float_box(Float::Right, 100.0, 80.0, |_| {}),
                para(vec![text(PARA)]),
            ],
            collapse,
        );
        assert_no_line_under_a_float(name, &body, &body);
        let lines = line_rects(&body.children[2]);
        for l in lines.iter().filter(|l| l.y < 80.0 - EPS) {
            assert!(
                l.x >= 100.0 - EPS && l.right() <= 300.0 + EPS,
                "{name}: {l:?}"
            );
        }
        assert!(
            lines.iter().any(|l| l.y < 80.0 - EPS),
            "{name}: no line beside the floats"
        );
    });
}

/// (h) when the space beside a float is too narrow for any content, the
/// line moves down until it fits.
#[test]
fn a_line_too_narrow_beside_a_float_moves_below_it() {
    for_both("float:right(380) + p", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Right, 380.0, 50.0, |_| {}),
                para(vec![text(
                    "Incomprehensibilities are everywhere on the web today.",
                )]),
            ],
            collapse,
        );
        assert_no_line_under_a_float(name, &body, &body);
        let first = line_rects(&body.children[1])[0];
        assert!(
            first.y >= 50.0 - EPS,
            "{name}: the first line moves below: {first:?}"
        );
    });
}

/// (i) the Wikipedia shape: `div.mw-parser-output > figure[float:right] + p
/// + p` inside the content block, with links in the paragraphs.
#[test]
fn the_wikipedia_thumbnail_shape_wraps_text_beside_the_figure() {
    let link = |s: &str| {
        let mut a = LayoutBox::new(BoxType::Inline, {
            let mut st = ComputedStyle::new();
            st.display = Display::Inline;
            st
        });
        a.children.push(text(s));
        a
    };
    for_both("div > div > figure + p + p", |name, collapse| {
        let figure = float_box(Float::Right, 222.0, 180.0, |s| {
            s.margin_left = Length::Px(14.0);
            s.margin_bottom = Length::Px(14.0);
        });
        let p1 = para(vec![
            text("A "),
            link("web browser"),
            text(" is an application for accessing "),
            link("websites"),
            text(". "),
            text(PARA),
        ]);
        let p2 = para(vec![text(PARA)]);
        let content = block(vec![block(vec![figure, p1, p2])]);
        let body = laid_out(vec![content], collapse);
        let f = float_rects(&body)[0];
        assert!(
            (f.right() - 400.0).abs() < EPS,
            "{name}: figure at the right edge: {f:?}"
        );
        assert_no_line_under_a_float(name, &body, &body);
        assert!(
            line_rects(&body).iter().any(|l| beside(l, &f)),
            "{name}: no line beside"
        );
    });
}

/// (j) a floated table of fixed width (`table.infobox`) with paragraphs
/// beside it.
#[test]
fn paragraphs_sit_beside_a_floated_infobox_table() {
    for_both("table.infobox[float:right] + p + p", |name, collapse| {
        let mut table = float_box(Float::Right, 160.0, 0.0, |s| {
            s.height = Length::Auto;
            s.margin_left = Length::Px(16.0);
        });
        for row in ["Developer", "Initial release", "Written in"] {
            table.children.push(block(vec![text(row)]));
        }
        let body = laid_out(
            vec![table, para(vec![text(PARA)]), para(vec![text(PARA)])],
            collapse,
        );
        let f = body.children[0].dimensions.margin_box();
        assert!(f.height > 0.0, "{name}: the table has its rows' height");
        assert_no_line_under_a_float(name, &body, &body.children[1]);
        assert_no_line_under_a_float(name, &body, &body.children[2]);
    });
}

/// (k) a containing block with padding: the float sits at the content edge
/// and the shortening is measured from it.
#[test]
fn shortening_is_measured_from_the_content_edge() {
    for_both("padded div > float:right + p", |name, collapse| {
        let body = laid_out(
            vec![block_with(
                |s| {
                    s.padding_left = Length::Px(20.0);
                    s.padding_right = Length::Px(30.0);
                    s.padding_top = Length::Px(10.0);
                },
                vec![
                    float_box(Float::Right, 100.0, 80.0, |_| {}),
                    para(vec![text(PARA)]),
                ],
            )],
            collapse,
        );
        let f = float_rects(&body)[0];
        assert!(
            (f.right() - 370.0).abs() < EPS,
            "{name}: float at the content edge: {f:?}"
        );
        assert!(
            (f.y - 10.0).abs() < EPS,
            "{name}: float under the padding: {f:?}"
        );
        assert_no_line_under_a_float(name, &body, &body);
        let lines = line_rects(&body);
        for l in &lines {
            assert!(
                l.x >= 20.0 - EPS,
                "{name}: line left of the content edge: {l:?}"
            );
        }
        assert!(lines.iter().any(|l| beside(l, &f)));
    });
}

/// (l) the float's margins are part of the excluded area.
#[test]
fn a_floats_margins_are_excluded_too() {
    for_both("float:right(margin) + p", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Right, 100.0, 60.0, |s| {
                    s.margin_left = Length::Px(40.0);
                    s.margin_bottom = Length::Px(20.0);
                }),
                para(vec![text(PARA)]),
            ],
            collapse,
        );
        let f = body.children[0].dimensions.margin_box();
        assert!(
            (f.x - 260.0).abs() < EPS && (f.height - 80.0).abs() < EPS,
            "{name}: {f:?}"
        );
        assert_no_line_under_a_float(name, &body, &body);
        let lines = line_rects(&body);
        assert!(
            lines
                .iter()
                .any(|l| l.y >= 60.0 - EPS && l.y < 80.0 - EPS && l.right() <= 260.0 + EPS)
                || !lines.iter().any(|l| l.y >= 60.0 - EPS && l.y < 80.0 - EPS),
            "{name}: lines beside the bottom margin are shortened too: {lines:?}"
        );
    });
}

// ---- risks ----------------------------------------------------------------

/// Intrinsic widths are not computed from shortened lines: the min-content
/// and max-content widths of a block holding a float and text are at least
/// those of its unbroken content. (Then a shrink-to-fit float holding both
/// lays out at that width.)
#[test]
fn intrinsic_width_does_not_shrink_with_shortened_lines() {
    let make = || {
        let mut s = ComputedStyle::new();
        s.float = Float::Left;
        let mut outer = LayoutBox::with_float(BoxType::Block, s, Float::Left);
        outer.children = vec![
            float_box(Float::Right, 50.0, 40.0, |_| {}),
            text("Browsers fetch pages."),
        ];
        outer
    };
    let mut alone = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    alone.children = vec![text("Browsers fetch pages.")];
    let base = crate::grid::estimate_max_content_width(&alone);
    let w = crate::grid::estimate_max_content_width(&make());
    assert!(
        w >= base - EPS,
        "max-content {w} shrank below the text's own {base}"
    );
    let mn = crate::grid::estimate_min_content_width(&make());
    assert!(
        mn >= 50.0 - EPS,
        "min-content {mn} smaller than the inner float"
    );
}

/// A float does not shorten lines inside another formatting context: the
/// inline-block / overflow:hidden box is placed beside the float, and its
/// own lines use its whole width.
#[test]
fn a_float_does_not_reach_into_another_formatting_context() {
    for_both(
        "float:right + div(overflow:hidden) > p",
        |name, collapse| {
            let body = laid_out(
                vec![
                    float_box(Float::Right, 150.0, 100.0, |_| {}),
                    block_with(
                        |s| s.overflow_x = Overflow::Hidden,
                        vec![para(vec![text(PARA)])],
                    ),
                ],
                collapse,
            );
            let d = body.children[1].dimensions.content;
            let lines = line_rects(&body.children[1]);
            assert!(
                lines
                    .iter()
                    .any(|l| l.y < 100.0 && (l.right() - d.right()).abs() < 40.0),
                "{name}: lines fill the formatting root's own width {d:?}: {lines:?}"
            );
            for l in &lines {
                assert!(l.x >= d.x - EPS, "{name}: {l:?} left of the root {d:?}");
            }
        },
    );
    for_both("float:right + inline-block", |name, collapse| {
        let mut ib = block_with(
            |s| {
                s.display = Display::InlineBlock;
                s.width = Length::Px(200.0);
            },
            vec![text(PARA)],
        );
        ib.box_type = BoxType::Block;
        let body = laid_out(
            vec![para(vec![
                float_box(Float::Right, 150.0, 100.0, |_| {}),
                ib,
            ])],
            collapse,
        );
        let ib = &body.children[0].children[1];
        let d = ib.dimensions.content;
        for l in line_rects(ib) {
            assert!(
                l.x >= d.x - EPS && l.right() <= d.right() + EPS,
                "{name}: inline-block lines stay in its box {d:?}: {l:?}"
            );
        }
        assert!(
            line_rects(ib).iter().any(|l| l.right() > d.right() - 40.0),
            "{name}: inline-block lines are not shortened by the outer float"
        );
    });
}

/// `text-align: center` / `right` aligns within the shortened line, not
/// under the float.
#[test]
fn text_align_works_within_the_shortened_line() {
    for align in [TextAlign::Center, TextAlign::Right] {
        for_both(
            &format!("float:right + p(text-align:{align:?})"),
            |name, collapse| {
                let body = laid_out(
                    vec![
                        float_box(Float::Right, 150.0, 100.0, |_| {}),
                        block_with(|s| s.text_align = align, vec![text(PARA)]),
                    ],
                    collapse,
                );
                let f = body.children[0].dimensions.margin_box();
                assert_no_line_under_a_float(name, &body, &body);
                let lines = line_rects(&body.children[1]);
                let beside_lines: Vec<_> = lines.iter().filter(|l| beside(l, &f)).collect();
                assert!(!beside_lines.is_empty(), "{name}: no line beside the float");
                for l in beside_lines {
                    let slack = 250.0 - l.right();
                    let lead = l.x;
                    match align {
                        TextAlign::Right => {
                            assert!(slack < EPS, "{name}: right-aligned to the float: {l:?}")
                        }
                        _ => assert!(
                            (slack - lead).abs() < 1.0,
                            "{name}: centred in 0..250: {l:?}"
                        ),
                    }
                }
            },
        );
    }
}

/// A flex container establishes a formatting context: it is narrowed beside
/// the float, and its item's lines use the item's whole width.
#[test]
fn a_float_does_not_reach_into_a_flex_item() {
    for_both("float:right + div(flex) > div > text", |name, collapse| {
        let body = laid_out(
            vec![
                float_box(Float::Right, 150.0, 100.0, |_| {}),
                block_with(|s| s.display = Display::Flex, vec![block(vec![text(PARA)])]),
            ],
            collapse,
        );
        assert_no_line_under_a_float(name, &body, &body);
        let item = &body.children[1].children[0];
        let d = item.dimensions.content;
        let lines = line_rects(item);
        for l in &lines {
            assert!(
                l.x >= d.x - EPS && l.right() <= d.right() + EPS,
                "{name}: {l:?} outside the item {d:?}"
            );
        }
        assert!(
            lines.iter().any(|l| l.right() > d.right() - 40.0),
            "{name}: the item's lines are not shortened again inside it: {lines:?}"
        );
    });
}

/// A non-atomic inline (a link) too wide for the band at the start of a
/// line is not pushed below the float as a whole: it starts beside it.
#[test]
fn a_wide_link_at_the_start_of_a_line_stays_beside_the_float() {
    for_both("float:right + p > a(long)", |name, collapse| {
        let mut a = LayoutBox::new(BoxType::Inline, {
            let mut st = ComputedStyle::new();
            st.display = Display::Inline;
            st
        });
        a.children
            .push(text("the list of web browsers and their engines"));
        let body = laid_out(
            vec![
                float_box(Float::Right, 250.0, 100.0, |_| {}),
                para(vec![a, text(" is long.")]),
            ],
            collapse,
        );
        let link = &body.children[1].children[0];
        assert!(
            link.dimensions.content.y < 100.0,
            "{name}: the link starts beside the float, not below it: {:?}",
            link.dimensions.content
        );
    });
}
