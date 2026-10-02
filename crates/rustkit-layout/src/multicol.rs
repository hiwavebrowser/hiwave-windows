//! Multi-column layout (css-multicol-1), block-level fragmentation only.
//!
//! A box with `column-count: N` lays its children out at the column width
//! and then distributes them over N columns of one balanced height. Breaks
//! are taken only BETWEEN in-flow children (class A break points); a child
//! is never split across columns. Blink breaks between lines as well, so a
//! lone tall paragraph balances in Chrome and stays in one column here — the
//! column height then grows to fit it (css-multicol-1 §8.2 lets content
//! overflow; we prefer the taller column to losing ink).
//!
//! Balancing follows Blink's column balancer: start at the flowed height
//! divided by the column count, pack greedily, and while the content does
//! not fit, grow the height by the smallest shortage any break reported.
//! A margin adjoining a break is truncated (css-break-3 §5.2): a child that
//! opens a column starts at the column top, and the flowed coordinate of
//! its border-box top becomes that column's origin.

use rustkit_css::{ComputedStyle, Length};

use crate::{LayoutBox, Position};

/// Used column geometry of a multi-column container.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnGeometry {
    pub count: usize,
    pub width: f32,
    pub gap: f32,
}

/// Column geometry for `style` in a content box `content_width` wide, or
/// `None` when the box is not a multi-column container (css-multicol-1 §3.4:
/// `column-width: auto` and `column-count: N` give N columns of
/// `(W − (N − 1)·gap) / N`).
pub fn column_geometry(style: &ComputedStyle, content_width: f32) -> Option<ColumnGeometry> {
    let count = style.column_count? as usize;
    if count < 2 || content_width <= 0.0 {
        return None;
    }
    let font_size = match style.font_size {
        Length::Px(px) => px,
        _ => 16.0,
    };
    // `column-gap: normal` is 1em in a multi-column container. The initial
    // value is stored as Length::Zero (flex/grid read normal as 0), so an
    // unauthored gap is told apart from an authored `0px` by its variant.
    let gap = match style.column_gap {
        Length::Zero => font_size,
        ref g => g.to_px(font_size, 16.0, content_width).max(0.0),
    };
    let width = (content_width - (count as f32 - 1.0) * gap) / count as f32;
    if width <= 0.0 {
        return None;
    }
    Some(ColumnGeometry { count, width, gap })
}

/// One unbreakable piece of the flow: border-box top and bottom, relative to
/// the container's content top.
#[derive(Debug, Clone, Copy)]
struct Piece {
    index: usize,
    top: f32,
    bottom: f32,
}

const EPS: f32 = 0.01;

/// Pack `pieces` into at most `count` columns of height `h`.
/// Ok: per piece (column, column origin). Err: the smallest height increase
/// that changes the packing.
fn pack(pieces: &[Piece], total: f32, count: usize, h: f32) -> Result<Vec<(usize, f32)>, f32> {
    let mut placed = Vec::with_capacity(pieces.len());
    let mut column = 0;
    let mut origin = 0.0_f32;
    // Deepest bottom placed in this column: a piece starting above it shares
    // a line with an earlier piece and cannot open a column.
    let mut column_bottom = 0.0_f32;
    let mut shortage = f32::INFINITY;
    let mut fits = true;
    for p in pieces {
        let need = p.bottom - origin;
        if need > h + EPS {
            shortage = shortage.min(need - h);
            let breakable =
                placed.iter().any(|&(c, _)| c == column) && p.top >= column_bottom - EPS;
            if breakable && column + 1 < count {
                column += 1;
                origin = p.top;
                column_bottom = p.top;
                if p.bottom - origin > h + EPS {
                    shortage = shortage.min(p.bottom - origin - h);
                    fits = false;
                }
            } else {
                fits = false;
            }
        }
        column_bottom = column_bottom.max(p.bottom);
        placed.push((column, origin));
    }
    // The last column also holds whatever follows its last piece (the
    // trailing margin a formatting root keeps inside).
    if total - origin > h + EPS {
        shortage = shortage.min(total - origin - h);
        fits = false;
    }
    if fits {
        Ok(placed)
    } else {
        Err(shortage.max(EPS))
    }
}

/// Balance `container`'s children (already laid out at `cols.width`) over
/// its columns: move each child into its column and set the container's
/// content height to the balanced column height.
pub fn balance_columns(container: &mut LayoutBox, cols: &ColumnGeometry) {
    let content_y = container.dimensions.content.y;
    let total = container.dimensions.content.height;
    let pieces: Vec<Piece> = container
        .children
        .iter()
        .enumerate()
        .filter(|(_, c)| !matches!(c.position, Position::Absolute | Position::Fixed))
        .map(|(index, c)| {
            let bb = c.dimensions.border_box();
            Piece {
                index,
                top: bb.y - content_y,
                bottom: bb.y + bb.height - content_y,
            }
        })
        .collect();
    if pieces.is_empty() || total <= 0.0 {
        return;
    }

    let mut h = total / cols.count as f32;
    let placed = loop {
        match pack(&pieces, total, cols.count, h) {
            Ok(placed) => break placed,
            Err(grow) => {
                h += grow;
                // One column always fits: never loop past the flowed height.
                if h >= total - EPS {
                    h = total;
                    break vec![(0, 0.0); pieces.len()];
                }
            }
        }
    };

    for (p, (column, origin)) in pieces.iter().zip(placed) {
        if column == 0 {
            continue;
        }
        let dx = column as f32 * (cols.width + cols.gap);
        crate::flex::translate_subtree(&mut container.children[p.index], dx, -origin);
    }
    container.dimensions.content.height = h;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BoxType, Dimensions, FloatContext, MarginCollapseContext, Rect};

    fn block(height: f32, margin_bottom: f32) -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.height = Length::Px(height);
        style.margin_bottom = Length::Px(margin_bottom);
        LayoutBox::new(BoxType::Block, style)
    }

    fn lay_out(count: u32, gap: Length, children: Vec<LayoutBox>) -> LayoutBox {
        let mut style = ComputedStyle::new();
        style.column_count = Some(count);
        style.column_gap = gap;
        let mut multicol = LayoutBox::new(BoxType::Block, style);
        multicol.children = children;
        let mut root = LayoutBox::new(BoxType::Block, ComputedStyle::new());
        root.children = vec![multicol];
        let mut cb = Dimensions::default();
        cb.content = Rect::new(0.0, 0.0, 760.0, 0.0);
        root.layout_with_collapse(&cb, &mut MarginCollapseContext::new(), &mut FloatContext::new());
        root.children.remove(0)
    }

    #[test]
    fn two_paragraphs_balance_one_per_column() {
        // article-typography's .columns: Chrome puts p1 (168.84) in column 1,
        // p2 (140.70) atop column 2 with its margin truncated at the break,
        // and the container is half the flow including the trailing margin
        // (174.78 = 349.55 / 2).
        let mc = lay_out(2, Length::Px(40.0), vec![block(168.0, 20.0), block(140.0, 20.0)]);
        let (p1, p2) = (&mc.children[0].dimensions, &mc.children[1].dimensions);
        assert_eq!((p1.content.x, p1.content.y, p1.content.width), (0.0, 0.0, 360.0));
        assert_eq!((p2.content.x, p2.content.y, p2.content.width), (400.0, 0.0, 360.0));
        assert_eq!(mc.dimensions.content.width, 760.0);
        assert_eq!(mc.dimensions.content.height, 174.0);
    }

    #[test]
    fn column_height_grows_by_the_smallest_shortage() {
        // Flow 100 + 10 + 30 = 140 over 2 columns: 70 cannot hold the 100px
        // piece, so the height grows to it and the two short ones share
        // column 2.
        let mc = lay_out(
            2,
            Length::Px(40.0),
            vec![block(100.0, 0.0), block(10.0, 0.0), block(30.0, 0.0)],
        );
        assert_eq!(mc.dimensions.content.height, 100.0);
        let ys: Vec<(f32, f32)> = mc
            .children
            .iter()
            .map(|c| (c.dimensions.content.x, c.dimensions.content.y))
            .collect();
        assert_eq!(ys, vec![(0.0, 0.0), (400.0, 0.0), (400.0, 10.0)]);
    }

    #[test]
    fn unauthored_gap_is_one_em() {
        let mut style = ComputedStyle::new();
        style.column_count = Some(2);
        let cols = column_geometry(&style, 760.0).unwrap();
        assert_eq!((cols.gap, cols.width), (16.0, 372.0));
        style.column_count = Some(1);
        assert!(column_geometry(&style, 760.0).is_none());
    }
}
