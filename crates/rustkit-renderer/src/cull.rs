//! Viewport culling: a command whose paint lands wholly outside the target
//! is skipped before it is turned into quads.
//!
//! Until this, every command of the page was shaped, rasterized, batched
//! and uploaded on every frame, and the GPU clipped all but a viewport's
//! worth away: 99.8% of the quads of a 20,000-command text page, which
//! is what a scrolled frame cost (see `scroll_cost_tests`). On an
//! 80,000-command page the glyph batch outgrew wgpu's buffer limit and
//! the frame failed.
//!
//! Only commands whose paint is bounded by their own fields are culled,
//! with a margin for antialiasing and glyph overhang; everything else
//! (state pushes and pops, shadows, filters, gradients, form controls,
//! ...) is always processed, so the clip, transform, opacity and stacking
//! stacks are exactly what they were. Fixed content needs nothing special:
//! `PushViewportFixed` undoes the scroll in the transform stack, and the
//! cull tests the command where the current transform puts it. A clip only
//! ever removes pixels, so a command outside the viewport stays outside
//! it whatever clip is open.
//!
//! Nothing samples the paint of a culled command either: the only filter
//! command is `BackdropFilter`, which reads the frame, and the frame
//! holds only the viewport. A group filter (`filter: blur()` or
//! `drop-shadow()` over a subtree) would have to inflate the cull by its
//! reach, or turn it off inside the group.
//!
//! The fast path's mid-list flushes are the one thing culling changes: a
//! flush whose overlap was off-screen no longer happens. That cannot change
//! a pixel, because the fast path flushes whenever a command would be drawn
//! under something batched before it that it overlaps (`order_probe`), in
//! every batch: the frame is then the list painted in order, wherever the
//! other flushes fall. Text decorations are the one exception, drawn under
//! the text they follow, and no flush ever falls between the two; they are
//! ordered against everything batched before that text, at the text, so
//! culling the text does not drop the check. The GPU-gradient and blur
//! paths run the same check, since their size-cap flushes move with
//! culling too.
//!
//! The rule both halves keep: a bound holds every pixel the drawing code
//! can touch (a negative size is drawn normalised, a border side wider
//! than its box reaches past it), and culling never changes paint order.

use crate::glyph_grid::Batch;
use rustkit_layout::{DisplayCommand, Rect};

/// Antialiasing fringe added to every bound, in CSS px.
const FRINGE: f32 = 2.0;

/// The rect, before the current transform, that holds every pixel `cmd`
/// paints; `None` for a command that is not bounded this way (it is
/// never culled).
pub(crate) fn paint_bounds(cmd: &DisplayCommand) -> Option<Rect> {
    // How far past its rect a command paints, besides the fringe.
    let mut reach = 0.0f32;
    let rect = match cmd {
        DisplayCommand::SolidColor(_, rect) | DisplayCommand::RoundedRect { rect, .. } => *rect,
        // A side wider than the box is drawn past the box's opposite edge
        // (`draw_border`, `draw_rounded_border`).
        DisplayCommand::Border {
            rect,
            top,
            right,
            bottom,
            left,
            ..
        } => {
            reach = top.max(*right).max(*bottom).max(*left).max(0.0);
            *rect
        }
        DisplayCommand::RoundedBorder { rect, widths, .. } => {
            reach = widths.iter().fold(0.0f32, |m, w| m.max(*w));
            *rect
        }
        DisplayCommand::Text {
            x,
            y,
            font_size,
            advances: Some(advances),
            ascent,
            run,
            ..
        } => {
            // A glyph's ink can start left of its pen position and reach
            // past its advance (italics, overhang) and below the
            // baseline: one font size of slack on every side covers it.
            // The pen walks the advances from `x`, and a negative advance
            // (letter- or word-spacing) walks it left. It never gets
            // further left than all the negative advances together, nor
            // further right than all the positive ones: a superset of the
            // walk that sums without a branch. A shaped run's offsets add
            // their extremes the same way.
            let mut size = font_size.abs().max(1.0);
            let mut top = ascent.map_or(0.0, f32::abs);
            let mut lo: f32 = advances.iter().map(|a| a.min(0.0)).sum();
            let mut hi: f32 = advances.iter().map(|a| a.max(0.0)).sum();
            let (mut rise, mut drop) = (0.0f32, 0.0f32);
            if let Some(run) = run {
                size = size.max(run.font_size.abs());
                top = top.max(run.ascent.abs());
                let (mut run_lo, mut run_hi, mut off_lo, mut off_hi) =
                    (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for g in &run.glyphs {
                    run_lo += g.advance.min(0.0);
                    run_hi += g.advance.max(0.0);
                    off_lo = off_lo.min(g.x_offset);
                    off_hi = off_hi.max(g.x_offset);
                    rise = rise.min(g.y_offset);
                    drop = drop.max(g.y_offset);
                }
                lo = lo.min(run_lo + off_lo);
                hi = hi.max(run_hi + off_hi);
            }
            let top = top.max(size);
            Rect::new(
                x + lo - size,
                y + rise - size,
                hi - lo + 2.0 * size,
                top + 3.0 * size + drop - rise,
            )
        }
        DisplayCommand::TextDecoration {
            x,
            y,
            width,
            thickness,
            ..
        } => {
            // A wavy or double line swings a few thicknesses each way.
            let swing = 3.0 * thickness.abs() + 4.0;
            Rect::new(*x, y - swing, *width, thickness.abs() + 2.0 * swing)
        }
        // `ObjectFit::paint_quad` clips the image to its content box.
        DisplayCommand::Image { content_box, .. } => *content_box,
        _ => return None,
    };
    let finite = rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite()
        && reach.is_finite();
    // A negative size is drawn too: once a transform is set (any scroll),
    // `clip_quad_under` maps the corners to screen space and the quad covers
    // them whichever way round they are. So the bound is the normalised rect.
    let pad = reach + FRINGE;
    finite.then(|| {
        Rect::new(
            rect.x.min(rect.x + rect.width) - pad,
            rect.y.min(rect.y + rect.height) - pad,
            rect.width.abs() + 2.0 * pad,
            rect.height.abs() + 2.0 * pad,
        )
    })
}

/// What the fast path's paint-order check needs to know about `cmd`
/// (`Renderer::occludes_batched`). A flush draws the color batch, then
/// images, then glyphs, then color glyphs, so a command is drawn under
/// anything batched before it in a later batch than its own. The probe
/// gives the first such batch and the rect, before the current transform,
/// that holds the command's pixels; `None` for the rect means unbounded
/// (whatever is batched there counts). `None` overall: the command draws
/// nothing.
///
/// A command drawing into several batches (form controls: a box, then
/// text) is probed at its lowest, which checks the most.
pub(crate) fn order_probe(cmd: &DisplayCommand) -> Option<(Batch, Option<Rect>)> {
    use DisplayCommand as C;
    let grown =
        |r: &Rect, d: f32| Rect::new(r.x - d, r.y - d, r.width + 2.0 * d, r.height + 2.0 * d);
    Some(match cmd {
        C::PushClip(_)
        | C::PushClipRounded { .. }
        | C::PopClip
        | C::PushStackingContext { .. }
        | C::PopStackingContext
        | C::PushTransform { .. }
        | C::PopTransform
        | C::PushOpacity(_)
        | C::PopOpacity
        | C::PushFilter(_)
        | C::PopFilter
        | C::PushViewportFixed
        | C::PopViewportFixed
        | C::PushWindowSticky(_)
        | C::PopWindowSticky => return None,
        // Fills keep the exact rect the check has always used.
        C::SolidColor(color, rect) | C::RoundedRect { color, rect, .. } => {
            if color.a <= 0.0 {
                return None;
            }
            (Batch::Image, Some(*rect))
        }
        // A decoration that follows its text is probed when the text comes
        // up, not here (`Renderer::order_flush_needed`).
        C::TextDecoration { .. } => (Batch::Image, paint_bounds(cmd)),
        C::Text { .. } => (Batch::ColorGlyph, paint_bounds(cmd)),
        C::Image { content_box, .. } => (Batch::Glyph, Some(*content_box)),
        C::BackgroundImage { rect, .. } => (Batch::Glyph, Some(grown(rect, FRINGE))),
        C::BoxShadow {
            rect,
            offset_x,
            offset_y,
            blur_radius,
            spread_radius,
            ..
        } => {
            // Outer layers reach the spread and the blur past the moved box.
            let reach = offset_x.abs().max(offset_y.abs())
                + spread_radius.abs()
                + 2.0 * blur_radius.abs()
                + FRINGE;
            (Batch::Image, Some(grown(rect, reach)))
        }
        C::FocusRing {
            rect,
            width,
            offset,
            ..
        } => (
            Batch::Image,
            Some(grown(rect, width.abs() + offset.abs() + FRINGE)),
        ),
        C::Caret { x, y, height, .. } => (
            Batch::Image,
            Some(grown(
                &Rect::new(*x, *y, 0.0, height.max(0.0)),
                2.0 + FRINGE,
            )),
        ),
        C::MaskedColor { rect, .. }
        | C::BackdropFilter { rect, .. }
        | C::LinearGradient { rect, .. }
        | C::RadialGradient { rect, .. }
        | C::ConicGradient { rect, .. } => (Batch::Image, Some(grown(rect, FRINGE))),
        // Borders are bounded; form controls, canvas shapes and gradient
        // text are not.
        _ => (Batch::Image, paint_bounds(cmd)),
    })
}

/// True when `bounds`, mapped through the affine `m`, misses the
/// `width` x `height` target.
pub(crate) fn outside(m: [f32; 6], bounds: Rect, width: f32, height: f32) -> bool {
    let map = |x: f32, y: f32| (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]);
    let corners = [
        map(bounds.x, bounds.y),
        map(bounds.x + bounds.width, bounds.y),
        map(bounds.x, bounds.y + bounds.height),
        map(bounds.x + bounds.width, bounds.y + bounds.height),
    ];
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in corners {
        if !(x.is_finite() && y.is_finite()) {
            return false;
        }
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    x1 <= 0.0 || y1 <= 0.0 || x0 >= width || y0 >= height
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustkit_css::Color;

    const ID: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

    #[test]
    fn a_fill_below_the_viewport_is_outside_until_scrolled_to() {
        let fill = DisplayCommand::SolidColor(
            Color::new(0, 0, 0, 1.0),
            Rect::new(0.0, 2000.0, 100.0, 20.0),
        );
        let b = paint_bounds(&fill).unwrap();
        assert!(outside(ID, b, 800.0, 600.0));
        // Scrolled by 1500: the page translate brings it to y = 500.
        assert!(!outside(
            [1.0, 0.0, 0.0, 1.0, 0.0, -1500.0],
            b,
            800.0,
            600.0
        ));
    }

    #[test]
    fn the_fringe_keeps_an_edge_touching_fill() {
        let fill = DisplayCommand::SolidColor(
            Color::new(0, 0, 0, 1.0),
            Rect::new(0.0, 600.5, 100.0, 20.0),
        );
        assert!(!outside(ID, paint_bounds(&fill).unwrap(), 800.0, 600.0));
    }

    #[test]
    fn text_without_advances_and_state_commands_are_never_culled() {
        let text = DisplayCommand::Text {
            text: "x".into(),
            x: 0.0,
            y: 1e6,
            color: Color::new(0, 0, 0, 1.0),
            font_size: 14.0,
            font_family: String::new(),
            font_weight: 400,
            font_style: 0,
            advances: None,
            ascent: None,
            run: None,
        };
        assert!(paint_bounds(&text).is_none());
        assert!(paint_bounds(&DisplayCommand::PushClip(Rect::new(0.0, 1e6, 1.0, 1.0))).is_none());
        assert!(paint_bounds(&DisplayCommand::PopClip).is_none());
    }

    #[test]
    fn a_rotation_is_tested_on_the_mapped_corners() {
        // 90° about the origin: x' = -y, y' = x. A box at x 100..120,
        // y -50..-30 lands at x' 30..50, y' 100..120: inside.
        let fill = DisplayCommand::SolidColor(
            Color::new(0, 0, 0, 1.0),
            Rect::new(100.0, -50.0, 20.0, 20.0),
        );
        let b = paint_bounds(&fill).unwrap();
        assert!(outside(ID, b, 800.0, 600.0));
        assert!(!outside([0.0, 1.0, -1.0, 0.0, 0.0, 0.0], b, 800.0, 600.0));
    }

    fn spaced_text(x: f32, advances: Vec<f32>) -> DisplayCommand {
        DisplayCommand::Text {
            text: "x".repeat(advances.len()),
            x,
            y: 100.0,
            color: Color::new(0, 0, 0, 1.0),
            font_size: 14.0,
            font_family: String::new(),
            font_weight: 400,
            font_style: 0,
            advances: Some(advances),
            ascent: Some(13.0),
            run: None,
        }
    }

    #[test]
    fn negative_letter_spacing_keeps_text_whose_pen_walks_back_on_screen() {
        // Starts 300px right of an 800px viewport; letter-spacing of -40px
        // walks the pen 600px left, so its last glyphs are on screen.
        let b = paint_bounds(&spaced_text(1100.0, vec![-30.0; 20])).unwrap();
        assert!(b.x <= 1100.0 - 600.0 - 14.0, "{b:?}");
        assert!(!outside(ID, b, 800.0, 600.0));
        // The same text walking right stays outside.
        let b = paint_bounds(&spaced_text(1100.0, vec![30.0; 20])).unwrap();
        assert!(outside(ID, b, 800.0, 600.0));
    }

    #[test]
    fn a_negative_size_is_bounded_where_it_is_drawn_and_a_wide_border_past_its_box() {
        let red = Color::new(255, 0, 0, 1.0);
        // Drawn over x 800..1300 once a transform is set.
        let b = paint_bounds(&DisplayCommand::SolidColor(
            red,
            Rect::new(1300.0, 500.0, -500.0, 40.0),
        ))
        .unwrap();
        assert!(b.x <= 800.0 && b.x + b.width >= 1300.0, "{b:?}");
        assert!(!outside(ID, b, 1024.0, 768.0));
        // The top side reaches 380px below a 20px box above the viewport.
        let border = DisplayCommand::Border {
            color: red,
            rect: Rect::new(100.0, -200.0, 300.0, 20.0),
            top: 400.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        };
        let b = paint_bounds(&border).unwrap();
        assert!(b.y + b.height >= 200.0, "{b:?}");
        assert!(!outside(ID, b, 1024.0, 768.0));
    }

    #[test]
    fn the_order_probe_names_the_batches_drawn_above_each_command() {
        let red = Color::new(255, 0, 0, 1.0);
        let fill = DisplayCommand::SolidColor(red, Rect::new(0.0, 0.0, 10.0, 10.0));
        let probe = order_probe(&fill).map(|(b, r)| (b, r.map(|r| (r.x, r.y, r.width, r.height))));
        assert_eq!(probe, Some((Batch::Image, Some((0.0, 0.0, 10.0, 10.0)))));
        let clear =
            DisplayCommand::SolidColor(Color::new(0, 0, 0, 0.0), Rect::new(0.0, 0.0, 10.0, 10.0));
        assert!(order_probe(&clear).is_none());
        assert!(order_probe(&DisplayCommand::PopClip).is_none());
        assert_eq!(
            order_probe(&spaced_text(0.0, vec![7.0])).map(|p| p.0),
            Some(Batch::ColorGlyph)
        );
        // Unbounded commands count everything batched above them.
        let line = DisplayCommand::Line {
            x1: 0.0,
            y1: 0.0,
            x2: 10.0,
            y2: 10.0,
            color: red,
            width: 1.0,
        };
        assert!(matches!(order_probe(&line), Some((Batch::Image, None))));
    }

    #[test]
    fn a_negative_width_or_height_spans_back_from_its_origin() {
        let fill = DisplayCommand::SolidColor(
            Color::new(0, 0, 0, 1.0),
            Rect::new(1100.0, 400.0, -300.0, -20.0),
        );
        let b = paint_bounds(&fill).unwrap();
        assert_eq!((b.x, b.y, b.width, b.height), (798.0, 378.0, 304.0, 24.0));
        assert!(!outside(ID, b, 1024.0, 768.0));
    }
}
