//! The boxes a frame paints over part of its view (CH3.A3b).
//!
//! The RustKit chrome is the whole window, stacked over the page, and is
//! shown (and hit) only inside its *region*: the window less the page
//! column (the hole), plus whatever the chrome paints over the hole — an
//! open menu, panel or tooltip. This module reads that "whatever" from the
//! frame's own display list, so no list of popup selectors is needed: one
//! pass over the commands, bounds only, no pixels and no GPU read-back.
//!
//! What counts as painted over the hole:
//!
//! * the box of every command that puts colour on the surface (fills,
//!   borders, text, images, gradients, form controls, SVG shapes), where
//!   the transforms in force put it, cut by the clips in force;
//! * not a command inside a group faded to opacity 0, nor a transparent
//!   fill;
//! * not a box shadow: the chrome's surface is opaque, so outside a box its
//!   frame holds the body's colour, and revealing a shadow's reach would
//!   show a block of that colour, not a shadow over the page;
//! * not a *backdrop*: a box that covers at least [`BACKDROP_COVER`] of the
//!   hole is the window's background (the root and body, which chrome.html
//!   paints over the whole window, and the main column), never a popup.
//!   The fraction, not exact containment, so a column that disagrees with
//!   the app's hole by a few pixels is still a backdrop.
//!
//! The boxes are clipped to the hole, and a box inside another one (a
//! menu's rows and text inside the menu) is dropped, so an open menu is one
//! rect.

use rustkit_layout::{DisplayCommand, DisplayList, Rect};

/// A box covering this much of the hole's area is a backdrop.
pub const BACKDROP_COVER: f32 = 0.9;

type Affine = [f32; 6];

const IDENTITY: Affine = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn translate(x: f32, y: f32) -> Affine {
    [1.0, 0.0, 0.0, 1.0, x, y]
}

/// `a · b` (column vectors), as the renderer composes its stack.
fn multiply(a: Affine, b: Affine) -> Affine {
    [
        a[0] * b[0] + a[2] * b[1],
        a[1] * b[0] + a[3] * b[1],
        a[0] * b[2] + a[2] * b[3],
        a[1] * b[2] + a[3] * b[3],
        a[0] * b[4] + a[2] * b[5] + a[4],
        a[1] * b[4] + a[3] * b[5] + a[5],
    ]
}

fn about_origin(m: Affine, origin: (f32, f32)) -> Affine {
    multiply(translate(origin.0, origin.1), multiply(m, translate(-origin.0, -origin.1)))
}

/// The bounds of `r` mapped through `m`; `None` when not finite.
fn map_rect(m: Affine, r: Rect) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in [(r.x, r.y), (r.x + r.width, r.y), (r.x, r.y + r.height), (r.x + r.width, r.y + r.height)] {
        let px = m[0] * x + m[2] * y + m[4];
        let py = m[1] * x + m[3] * y + m[5];
        if !(px.is_finite() && py.is_finite()) {
            return None;
        }
        x0 = x0.min(px);
        y0 = y0.min(py);
        x1 = x1.max(px);
        y1 = y1.max(py);
    }
    Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = (a.x + a.width).min(b.x + b.width);
    let y1 = (a.y + a.height).min(b.y + b.height);
    Rect::new(x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0))
}

fn area(r: &Rect) -> f32 {
    r.width.max(0.0) * r.height.max(0.0)
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    const SLACK: f32 = 0.5;
    inner.x >= outer.x - SLACK
        && inner.y >= outer.y - SLACK
        && inner.x + inner.width <= outer.x + outer.width + SLACK
        && inner.y + inner.height <= outer.y + outer.height + SLACK
}

fn grown(r: Rect, d: f32) -> Rect {
    Rect::new(r.x - d, r.y - d, r.width + 2.0 * d, r.height + 2.0 * d)
}

fn normalised(r: Rect) -> Rect {
    Rect::new(r.x.min(r.x + r.width), r.y.min(r.y + r.height), r.width.abs(), r.height.abs())
}

fn points_box(points: &[(f32, f32)], reach: f32) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    (x0 <= x1).then(|| grown(Rect::new(x0, y0, x1 - x0, y1 - y0), reach))
}

/// The box, before the transform in force, that a command puts colour in;
/// `None` for a command that paints no surface (state commands, shadows,
/// transparent fills).
fn surface_box(cmd: &DisplayCommand) -> Option<Rect> {
    use DisplayCommand as C;
    let visible = |a: f32| a > 0.0;
    let r = match cmd {
        C::SolidColor(color, rect) | C::RoundedRect { color, rect, .. } | C::FillRect { rect, color } => {
            visible(color.a).then_some(*rect)?
        }
        C::FillEllipse { rect, color } => visible(color.a).then_some(*rect)?,
        C::Border { color, rect, top, right, bottom, left } => {
            (visible(color.a) && top.max(*right).max(*bottom).max(*left) > 0.0).then_some(*rect)?
        }
        C::RoundedBorder { rect, widths, colors, .. } => {
            (widths.iter().zip(colors).any(|(w, c)| *w > 0.0 && visible(c.a))).then_some(*rect)?
        }
        C::Text { x, y, color, font_size, advances, text, .. } => {
            if !visible(color.a) {
                return None;
            }
            let size = font_size.abs().max(1.0);
            let width = match advances {
                Some(a) => a.iter().map(|v| v.max(0.0)).sum(),
                None => text.chars().count() as f32 * size * 0.6,
            };
            Rect::new(*x, *y, width, size * 1.4)
        }
        C::GradientText { rect, .. } => *rect,
        C::TextDecoration { x, y, width, thickness, color, .. } => {
            visible(color.a).then(|| Rect::new(*x, *y, *width, thickness.abs().max(1.0)))?
        }
        C::Image { content_box, opacity, .. } => visible(*opacity).then_some(*content_box)?,
        C::BackgroundImage { rect, .. }
        | C::MaskedColor { rect, .. }
        | C::BackdropFilter { rect, .. }
        | C::LinearGradient { rect, .. }
        | C::RadialGradient { rect, .. }
        | C::ConicGradient { rect, .. }
        | C::TextInput { rect, .. }
        | C::ListBox { rect, .. }
        | C::Button { rect, .. } => *rect,
        C::FocusRing { rect, color, width, offset } => {
            visible(color.a).then(|| grown(*rect, width.abs() + offset.abs()))?
        }
        C::Caret { x, y, height, color } => visible(color.a).then(|| Rect::new(*x, *y, 1.0, height.max(0.0)))?,
        C::StrokeRect { rect, color, width } => visible(color.a).then(|| grown(*rect, width.abs() / 2.0))?,
        C::FillCircle { cx, cy, radius, color } => {
            visible(color.a).then(|| Rect::new(cx - radius, cy - radius, 2.0 * radius, 2.0 * radius))?
        }
        C::StrokeCircle { cx, cy, radius, color, width } => visible(color.a).then(|| {
            let r = radius + width.abs() / 2.0;
            Rect::new(cx - r, cy - r, 2.0 * r, 2.0 * r)
        })?,
        C::Line { x1, y1, x2, y2, color, width } => {
            visible(color.a).then(|| points_box(&[(*x1, *y1), (*x2, *y2)], width.abs() / 2.0))??
        }
        C::Polyline { points, color, width } | C::StrokePolygon { points, color, width } => {
            visible(color.a).then(|| points_box(points, width.abs() / 2.0))??
        }
        C::FillPolygon { points, color } => visible(color.a).then(|| points_box(points, 0.0))??,
        C::BoxShadow { .. }
        | C::PushClip(_)
        | C::PushClipRounded { .. }
        | C::PushClipPath { .. }
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
    };
    Some(normalised(r))
}

/// The boxes `list`, drawn at `scroll` (CSS px, as the renderer draws a
/// `FrameCommit`), paints over `within`, in view coordinates: clipped to
/// `within`, backdrops left out, a box inside another dropped. Empty when
/// nothing but the window's background is there. See the module doc.
pub fn painted_boxes(list: &DisplayList, scroll: (f32, f32), within: Rect) -> Vec<Rect> {
    if area(&within) <= 0.0 {
        return Vec::new();
    }
    let backdrop = area(&within) * BACKDROP_COVER;
    let base = translate(-scroll.0, -scroll.1);
    let mut transforms: Vec<Affine> = vec![base];
    let mut clips: Vec<Option<Rect>> = vec![None];
    let mut opacity: Vec<f32> = vec![1.0];
    let mut found: Vec<Rect> = Vec::new();
    for cmd in &list.commands {
        let m = *transforms.last().unwrap_or(&IDENTITY);
        let clip = *clips.last().unwrap_or(&None);
        match cmd {
            DisplayCommand::PushTransform { matrix, origin } => {
                transforms.push(multiply(m, about_origin(*matrix, *origin)));
                continue;
            }
            // A fixed run undoes the scroll; a sticky shift is left out (the
            // views this serves are not scrolled).
            DisplayCommand::PushViewportFixed => {
                transforms.push(multiply(m, translate(scroll.0, scroll.1)));
                continue;
            }
            DisplayCommand::PushWindowSticky(_) => {
                transforms.push(m);
                continue;
            }
            DisplayCommand::PopTransform | DisplayCommand::PopViewportFixed | DisplayCommand::PopWindowSticky => {
                if transforms.len() > 1 {
                    transforms.pop();
                }
                continue;
            }
            DisplayCommand::PushClip(rect)
            | DisplayCommand::PushClipRounded { rect, .. }
            | DisplayCommand::PushClipPath { rect, .. } => {
                let screen = map_rect(m, normalised(*rect)).unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
                clips.push(Some(match clip {
                    Some(outer) => intersect(outer, screen),
                    None => screen,
                }));
                continue;
            }
            DisplayCommand::PopClip => {
                if clips.len() > 1 {
                    clips.pop();
                }
                continue;
            }
            DisplayCommand::PushOpacity(alpha) => {
                let outer = *opacity.last().unwrap_or(&1.0);
                opacity.push(outer * alpha.clamp(0.0, 1.0));
                continue;
            }
            DisplayCommand::PopOpacity => {
                if opacity.len() > 1 {
                    opacity.pop();
                }
                continue;
            }
            _ => {}
        }
        if *opacity.last().unwrap_or(&1.0) <= 0.0 {
            continue;
        }
        let Some(local) = surface_box(cmd) else {
            continue;
        };
        let Some(mut screen) = map_rect(m, local) else {
            continue;
        };
        if let Some(clip) = clip {
            screen = intersect(screen, clip);
        }
        let over = intersect(screen, within);
        if area(&over) <= 0.0 || area(&over) >= backdrop {
            continue;
        }
        found.push(over);
    }
    // Largest first; a box inside one already kept adds nothing.
    found.sort_by(|a, b| area(b).total_cmp(&area(a)));
    let mut kept: Vec<Rect> = Vec::new();
    for r in found {
        if !kept.iter().any(|k| contains(k, &r)) {
            kept.push(r);
        }
    }
    kept
}
