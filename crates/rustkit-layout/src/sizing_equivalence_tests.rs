//! Sizing equivalence: one length, many spellings.
//!
//! With the root font at 16px, the element's own font at 16px and a 1000px
//! wide viewport, `20px`, `1.25rem`, `1.25em`, `calc(16px + 4px)`, `2vw` and
//! the mixed-unit `calc(1em + 4px)` are the same length, so every box in the
//! laid-out tree must come out with identical geometry whichever spelling a
//! property uses. The same defect kept coming back one code path at a time
//! (replaced elements, flex containers, media queries, intrinsic widths): one
//! part of layout resolved the length while another read only `px` and took
//! the rest for `auto`. These tests check the whole family at once, one test
//! per property x context, with `px` as the reference. They need no oracle:
//! whatever `20px` does, the other spellings must do the same. Each case runs
//! through both entry points, `layout` and `layout_with_collapse`.
//!
//! NOT covered: `flex-basis`, `grid-template-columns` track sizes and
//! `border-spacing`. `FlexBasis`, `TrackSize` and `border_spacing` carry no
//! unit, so layout only ever sees px; `rustkit-engine` converts them, and
//! (read, not tested) its converters look like the same defect: `em`, `vw`
//! and mixed `calc()` become `auto`, or 0 for `border-spacing`. They need
//! engine-side equivalence tests.
//!
//! `calc(16px + 4px)` folds to `Length::Px(20.0)` at parse time, so that
//! spelling only guards the parser's folding; `calc(1em + 4px)` is the one
//! that exercises `Length::Calc`.

use super::*;
use crate::table::fixup_table_boxes;
use rustkit_css::{parse_length, Display, FlexDirection};

const VIEWPORT: (f32, f32) = (1000.0, 800.0);

/// Every spelling of 20px under test; the first is the reference.
fn spellings() -> Vec<(&'static str, Length)> {
    [
        "20px",
        "1.25rem",
        "1.25em",
        "calc(16px + 4px)",
        "2vw",
        "calc(1em + 4px)",
    ]
    .iter()
    .map(|css| {
        let l = parse_length(css).unwrap_or_else(|| panic!("{css} does not parse"));
        (*css, l)
    })
    .collect()
}

/// Lays `root` out as the only child of a 400px wide body; returns every
/// box's margin, border, padding and content edges in tree order.
fn geometry(root: LayoutBox, collapse: bool) -> Vec<(String, [f32; 16])> {
    let mut body = LayoutBox::new(BoxType::Block, ComputedStyle::new());
    body.children.push(root);
    fixup_table_boxes(&mut body);
    body.set_viewport(VIEWPORT.0, VIEWPORT.1);
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
    let mut out = Vec::new();
    fn walk(b: &LayoutBox, path: String, out: &mut Vec<(String, [f32; 16])>) {
        let d = &b.dimensions;
        let e = |s: &EdgeSizes| [s.top, s.right, s.bottom, s.left];
        let mut g = [0.0; 16];
        g[..4].copy_from_slice(&[d.content.x, d.content.y, d.content.width, d.content.height]);
        g[4..8].copy_from_slice(&e(&d.padding));
        g[8..12].copy_from_slice(&e(&d.border));
        g[12..].copy_from_slice(&e(&d.margin));
        out.push((path.clone(), g));
        for (i, c) in b.children.iter().enumerate() {
            walk(c, format!("{path}/{i}"), out);
        }
    }
    walk(&body, "body".into(), &mut out);
    out
}

fn check(cell: &str, build: impl Fn(Length) -> LayoutBox) {
    let all = spellings();
    let (ref_css, ref_len) = &all[0];
    let mut wrong = Vec::new();
    for collapse in [false, true] {
        let want = geometry(build(ref_len.clone()), collapse);
        for (css, len) in &all[1..] {
            let got = geometry(build(len.clone()), collapse);
            if got.len() != want.len() {
                wrong.push(format!(
                    "{css} (collapse = {collapse}): {} boxes, {ref_css} has {}",
                    got.len(),
                    want.len()
                ));
                continue;
            }
            let first = got
                .iter()
                .zip(&want)
                .find(|((_, g), (_, w))| g.iter().zip(w).any(|(a, b)| (a - b).abs() >= 0.01));
            if let Some(((path, g), (_, w))) = first {
                wrong.push(format!(
                    "{css} (collapse = {collapse}): box {path}\n    got  {g:?}\n    {ref_css} {w:?}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{cell}: not the same as {ref_css}; [x y w h | padding trbl | border trbl | margin trbl]\n  {}",
        wrong.join("\n  ")
    );
}

// ---------------------------------------------------------------- properties

type Prop = fn(&mut ComputedStyle, Length);

fn width(s: &mut ComputedStyle, l: Length) {
    s.width = l;
}
fn height(s: &mut ComputedStyle, l: Length) {
    s.height = l;
}
/// Under a zero width, so the minimum is what sizes the box.
fn min_width(s: &mut ComputedStyle, l: Length) {
    s.width = Length::Px(0.0);
    s.min_width = l;
}
/// Over a 100px width, so the maximum is what sizes the box.
fn max_width(s: &mut ComputedStyle, l: Length) {
    s.width = Length::Px(100.0);
    s.max_width = l;
}
fn min_height(s: &mut ComputedStyle, l: Length) {
    s.height = Length::Px(0.0);
    s.min_height = l;
}
fn max_height(s: &mut ComputedStyle, l: Length) {
    s.height = Length::Px(100.0);
    s.max_height = l;
}
fn padding(s: &mut ComputedStyle, l: Length) {
    s.padding_top = l.clone();
    s.padding_right = l.clone();
    s.padding_bottom = l.clone();
    s.padding_left = l;
}
fn margin(s: &mut ComputedStyle, l: Length) {
    s.margin_top = l.clone();
    s.margin_right = l.clone();
    s.margin_bottom = l.clone();
    s.margin_left = l;
}
fn gap(s: &mut ComputedStyle, l: Length) {
    s.row_gap = l.clone();
    s.column_gap = l;
}
fn top_left(s: &mut ComputedStyle, l: Length) {
    s.top = Some(l.clone());
    s.left = Some(l);
}

// ------------------------------------------------------------------ builders

fn styled(display: Display) -> ComputedStyle {
    let mut s = ComputedStyle::new();
    s.display = display;
    s
}

fn with_children(display: Display, children: Vec<LayoutBox>) -> LayoutBox {
    let mut b = LayoutBox::new(BoxType::Block, styled(display));
    b.children = children;
    b
}

/// A fixed `w`x`h` block, sized in px only.
fn fixed(w: f32, h: f32) -> LayoutBox {
    let mut s = styled(Display::Block);
    s.width = Length::Px(w);
    s.height = Length::Px(h);
    LayoutBox::new(BoxType::Block, s)
}

/// The box under test: `display`, holding a 6x6 block so it has content,
/// with `prop` set to `l`.
fn target(display: Display, prop: Prop, l: Length) -> LayoutBox {
    let mut b = with_children(display, vec![fixed(6.0, 6.0)]);
    prop(&mut b.style, l);
    b
}

/// A sibling after the target, so a change in the target's outer size moves
/// something.
fn after() -> LayoutBox {
    fixed(10.0, 10.0)
}

// ------------------------------------------------------------------ contexts

fn block_child(prop: Prop, l: Length) -> LayoutBox {
    with_children(
        Display::Block,
        vec![target(Display::Block, prop, l), after()],
    )
}

/// `top`/`left` on a box positioned against a 300x200 relative parent.
fn positioned(position: Position, l: Length) -> LayoutBox {
    let mut cb =
        LayoutBox::with_position(BoxType::Block, styled(Display::Block), Position::Relative);
    cb.style.position = rustkit_css::Position::Relative;
    cb.style.width = Length::Px(300.0);
    cb.style.height = Length::Px(200.0);
    let mut b = target(Display::Block, top_left, l);
    b.position = position;
    b.style.position = match position {
        Position::Absolute => rustkit_css::Position::Absolute,
        _ => rustkit_css::Position::Relative,
    };
    b.style.width = Length::Px(30.0);
    cb.children = vec![b, after()];
    cb
}

fn flex(direction: FlexDirection, children: Vec<LayoutBox>) -> LayoutBox {
    let mut c = with_children(Display::Flex, children);
    c.style.flex_direction = direction;
    c.style.width = Length::Px(300.0);
    c.style.height = Length::Px(200.0);
    c
}

fn flex_item(direction: FlexDirection, prop: Prop, l: Length) -> LayoutBox {
    flex(direction, vec![target(Display::Block, prop, l), after()])
}

fn flex_gap(direction: FlexDirection, l: Length) -> LayoutBox {
    let mut c = flex(direction, (0..3).map(|_| fixed(30.0, 30.0)).collect());
    gap(&mut c.style, l);
    c
}

fn grid(children: Vec<LayoutBox>) -> LayoutBox {
    use rustkit_css::{GridTemplate, TrackDefinition, TrackSize};
    let mut g = with_children(Display::Grid, children);
    g.style.width = Length::Px(300.0);
    g.style.grid_template_columns = GridTemplate {
        tracks: vec![
            TrackDefinition::simple(TrackSize::Px(100.0)),
            TrackDefinition::simple(TrackSize::Px(100.0)),
        ],
        ..Default::default()
    };
    g
}

fn grid_item(prop: Prop, l: Length) -> LayoutBox {
    grid(vec![target(Display::Block, prop, l), after(), after()])
}

fn grid_gap(l: Length) -> LayoutBox {
    let mut g = grid((0..4).map(|_| fixed(30.0, 30.0)).collect());
    gap(&mut g.style, l);
    g
}

/// An `<img>` with a `natural`-square image and only `prop` set.
fn image(natural: f32, prop: Prop, l: Length) -> LayoutBox {
    let mut img = LayoutBox::new(
        BoxType::Image {
            url: String::new(),
            natural_width: natural,
            natural_height: natural,
        },
        styled(Display::Inline),
    );
    prop(&mut img.style, l);
    with_children(Display::Block, vec![img])
}

fn inline_block(prop: Prop, l: Length) -> LayoutBox {
    with_children(
        Display::Block,
        vec![
            target(Display::InlineBlock, prop, l),
            with_children(Display::InlineBlock, vec![after()]),
        ],
    )
}

/// The intrinsic-width wrappers: each is as wide as its content, and its
/// content is the target (and a 10px sibling).
fn shrink_to_fit(prop: Prop, l: Length) -> LayoutBox {
    let mut stf = with_children(
        Display::Block,
        vec![target(Display::Block, prop, l), after()],
    );
    stf.position = Position::Absolute;
    stf.style.position = rustkit_css::Position::Absolute;
    let mut cb =
        LayoutBox::with_position(BoxType::Block, styled(Display::Block), Position::Relative);
    cb.style.position = rustkit_css::Position::Relative;
    cb.children = vec![stf];
    cb
}

fn float(prop: Prop, l: Length) -> LayoutBox {
    let mut f = with_children(
        Display::Block,
        vec![target(Display::Block, prop, l), after()],
    );
    f.float = Float::Left;
    f.style.float = Float::Left;
    with_children(Display::Block, vec![f])
}

fn content_sized_flex(prop: Prop, l: Length) -> LayoutBox {
    let mut c = with_children(
        Display::Flex,
        vec![target(Display::Block, prop, l), after()],
    );
    c.float = Float::Left;
    c.style.float = Float::Left;
    with_children(Display::Block, vec![c])
}

fn table_cell(prop: Prop, l: Length) -> LayoutBox {
    let cell = target(Display::TableCell, prop, l);
    let other = with_children(Display::TableCell, vec![after()]);
    let row = with_children(Display::TableRow, vec![cell, other]);
    let mut t = with_children(Display::Table, vec![row]);
    t.style.border_spacing = (0.0, 0.0);
    with_children(Display::Block, vec![t, after()])
}

// --------------------------------------------------------------------- cells

macro_rules! cells {
    ($( $(#[$m:meta])* $name:ident: $build:expr; )*) => {$(
        $(#[$m])*
        #[test]
        fn $name() {
            check(stringify!($name), $build);
        }
    )*};
}

use FlexDirection::{Column, Row};

cells! {
    block_child_width: |l| block_child(width, l);
    block_child_height: |l| block_child(height, l);
    block_child_min_width: |l| block_child(min_width, l);
    block_child_max_width: |l| block_child(max_width, l);
    block_child_min_height: |l| block_child(min_height, l);
    block_child_max_height: |l| block_child(max_height, l);
    block_child_padding: |l| block_child(padding, l);
    block_child_margin: |l| block_child(margin, l);
    absolute_box_top_left: |l| positioned(Position::Absolute, l);
    relative_box_top_left: |l| positioned(Position::Relative, l);

    row_flex_item_width: |l| flex_item(Row, width, l);
    row_flex_item_height: |l| flex_item(Row, height, l);
    row_flex_item_min_width: |l| flex_item(Row, min_width, l);
    row_flex_item_max_width: |l| flex_item(Row, max_width, l);
    row_flex_item_min_height: |l| flex_item(Row, min_height, l);
    row_flex_item_max_height: |l| flex_item(Row, max_height, l);
    row_flex_item_padding: |l| flex_item(Row, padding, l);
    row_flex_item_margin: |l| flex_item(Row, margin, l);
    row_flex_container_gap: |l| flex_gap(Row, l);

    column_flex_item_width: |l| flex_item(Column, width, l);
    column_flex_item_height: |l| flex_item(Column, height, l);
    column_flex_item_min_width: |l| flex_item(Column, min_width, l);
    column_flex_item_max_width: |l| flex_item(Column, max_width, l);
    column_flex_item_min_height: |l| flex_item(Column, min_height, l);
    column_flex_item_max_height: |l| flex_item(Column, max_height, l);
    column_flex_item_padding: |l| flex_item(Column, padding, l);
    column_flex_item_margin: |l| flex_item(Column, margin, l);
    column_flex_container_gap: |l| flex_gap(Column, l);

    grid_item_width: |l| grid_item(width, l);
    grid_item_height: |l| grid_item(height, l);
    grid_item_min_width: |l| grid_item(min_width, l);
    grid_item_max_width: |l| grid_item(max_width, l);
    grid_item_min_height: |l| grid_item(min_height, l);
    grid_item_max_height: |l| grid_item(max_height, l);
    grid_item_padding: |l| grid_item(padding, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    grid_item_margin: |l| grid_item(margin, l);
    grid_container_gap: grid_gap;

    // One dimension given; the other follows the 1:1 ratio. The minima start
    // from a 10px image and the maxima from a 40px one so they bind.
    img_width_only: |l| image(40.0, width, l);
    img_height_only: |l| image(40.0, height, l);
    img_min_width: |l| image(10.0, |s, l| s.min_width = l, l);
    img_max_width: |l| image(40.0, |s, l| s.max_width = l, l);
    img_min_height: |l| image(10.0, |s, l| s.min_height = l, l);
    img_max_height: |l| image(40.0, |s, l| s.max_height = l, l);
    img_padding: |l| image(40.0, padding, l);
    img_margin: |l| image(40.0, margin, l);

    inline_block_width: |l| inline_block(width, l);
    inline_block_height: |l| inline_block(height, l);
    inline_block_min_width: |l| inline_block(min_width, l);
    inline_block_max_width: |l| inline_block(max_width, l);
    inline_block_min_height: |l| inline_block(min_height, l);
    inline_block_max_height: |l| inline_block(max_height, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    inline_block_padding: |l| inline_block(padding, l);
    inline_block_margin: |l| inline_block(margin, l);

    #[ignore = "W7-A: own_max_content_width takes a px width only, grid.rs:3451 (draft PR #622 fixes it)"]
    shrink_to_fit_child_width: |l| shrink_to_fit(width, l);
    shrink_to_fit_child_height: |l| shrink_to_fit(height, l);
    shrink_to_fit_child_min_width: |l| shrink_to_fit(min_width, l);
    shrink_to_fit_child_max_width: |l| shrink_to_fit(max_width, l);
    shrink_to_fit_child_min_height: |l| shrink_to_fit(min_height, l);
    shrink_to_fit_child_max_height: |l| shrink_to_fit(max_height, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    shrink_to_fit_child_padding: |l| shrink_to_fit(padding, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    shrink_to_fit_child_margin: |l| shrink_to_fit(margin, l);

    #[ignore = "W7-A: own_max_content_width takes a px width only, grid.rs:3451 (draft PR #622 fixes it)"]
    float_child_width: |l| float(width, l);
    float_child_height: |l| float(height, l);
    float_child_min_width: |l| float(min_width, l);
    float_child_max_width: |l| float(max_width, l);
    float_child_min_height: |l| float(min_height, l);
    float_child_max_height: |l| float(max_height, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    float_child_padding: |l| float(padding, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    float_child_margin: |l| float(margin, l);

    #[ignore = "W7-A: own_max_content_width takes a px width only, grid.rs:3451 (draft PR #622 fixes it)"]
    content_sized_flex_item_width: |l| content_sized_flex(width, l);
    content_sized_flex_item_height: |l| content_sized_flex(height, l);
    content_sized_flex_item_min_width: |l| content_sized_flex(min_width, l);
    content_sized_flex_item_max_width: |l| content_sized_flex(max_width, l);
    content_sized_flex_item_min_height: |l| content_sized_flex(min_height, l);
    content_sized_flex_item_max_height: |l| content_sized_flex(max_height, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    content_sized_flex_item_padding: |l| content_sized_flex(padding, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    content_sized_flex_item_margin: |l| content_sized_flex(margin, l);

    table_cell_width: |l| table_cell(width, l);
    table_cell_height: |l| table_cell(height, l);
    table_cell_min_width: |l| table_cell(min_width, l);
    table_cell_max_width: |l| table_cell(max_width, l);
    table_cell_min_height: |l| table_cell(min_height, l);
    table_cell_max_height: |l| table_cell(max_height, l);
    #[ignore = "W7-A: intrinsic_len_px resolves vw against a hard-coded 800x600 viewport, grid.rs:3611"]
    table_cell_padding: |l| table_cell(padding, l);
}
