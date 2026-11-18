//! GPU canvas A0 (docs/design/GPU_CANVAS_2D.md, "Plan of record: option A"):
//! the context records every raster operation and replays the recording on
//! the CPU when something reads the bitmap. Each test draws the same thing on
//! a context that rasterizes at the call (`set_immediate_raster(true)`, the
//! path before A0) and on a recording one, checks that the recording one
//! drew nothing yet (the operations are pending), then that every read gives
//! the same bytes.

use crate::*;
use std::sync::Arc;

/// These tests pin the CPU recording's own bookkeeping (a read empties the
/// pending store), so they keep the GPU canvas off on this thread whatever the
/// environment says: with it on, a read keeps draws until the GPU has taken them.
fn cpu_ctx(w: u32, h: u32) -> CanvasRenderingContext2D {
    crate::gpu::set_gpu_canvas(Some(false));
    CanvasRenderingContext2D::new(w, h)
}

/// Draws `draw` on an immediate and a recording `w` x `h` context; returns
/// the recording one, already checked against the other.
fn both(w: u32, h: u32, draw: impl Fn(&mut CanvasRenderingContext2D)) -> CanvasRenderingContext2D {
    let mut direct = cpu_ctx(w, h);
    direct.set_immediate_raster(true);
    let mut recorded = cpu_ctx(w, h);
    draw(&mut direct);
    draw(&mut recorded);
    assert_eq!(direct.pending_ops(), 0, "an immediate context records nothing");
    assert!(recorded.pending_ops() > 0, "the draw was recorded, not rasterized");
    assert!(recorded.pending_bytes() > 0);
    same(&direct, &recorded);
    assert_eq!(recorded.pending_ops(), 0, "a read replays the recording");
    assert_eq!(recorded.pending_bytes(), 0);
    recorded
}

/// Every read of the two contexts gives the same bytes.
fn same(direct: &CanvasRenderingContext2D, recorded: &CanvasRenderingContext2D) {
    let (w, h) = (direct.width, direct.height);
    assert_eq!(direct.bitmap().is_some(), recorded.bitmap().is_some(), "a bitmap exists on both or neither");
    assert_eq!(direct.bitmap_bytes(), recorded.bitmap_bytes());
    let (a, b) = (direct.premultiplied_pixels(), recorded.premultiplied_pixels());
    assert!(a == b, "premultiplied pixels differ at {} of {} bytes", diff(&a, &b), a.len());
    let (a, b) = (direct.get_image_data(-3, -2, w + 6, h + 4), recorded.get_image_data(-3, -2, w + 6, h + 4));
    assert!(a.data == b.data, "getImageData differs at {} bytes", diff(&a.data, &b.data));
    if let (Some(a), Some(b)) = (direct.bitmap(), recorded.bitmap()) {
        assert!(a.0.data() == b.0.data(), "the bitmap as a drawImage source differs");
    }
}

fn diff(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count() + a.len().abs_diff(b.len())
}

fn color(r: u8, g: u8, b: u8, a: f32) -> CanvasStyle {
    CanvasStyle::Color(Color::new(r, g, b, a))
}

/// A 7x5 bitmap of distinct colours, some translucent.
fn image() -> CanvasBitmap {
    let mut rgba = Vec::new();
    for i in 0..35u32 {
        rgba.extend_from_slice(&[(i * 37 % 256) as u8, (i * 91 % 256) as u8, (i * 53 % 256) as u8, if i % 3 == 0 { 128 } else { 255 }]);
    }
    CanvasBitmap::from_rgba(7, 5, &rgba).unwrap()
}

/// A path with every segment kind.
fn shape(c: &mut CanvasRenderingContext2D, dx: f32) {
    c.begin_path();
    c.move_to(10.0 + dx, 10.0);
    c.line_to(60.0 + dx, 15.0);
    c.quadratic_curve_to(70.0 + dx, 40.0, 50.0 + dx, 55.0);
    c.bezier_curve_to(40.0 + dx, 70.0, 20.0 + dx, 30.0, 12.0 + dx, 60.0);
    c.arc_to(5.0 + dx, 40.0, 30.0 + dx, 30.0, 8.0);
    c.arc(30.0 + dx, 30.0, 12.0, 0.3, 4.0, true);
    c.ellipse(45.0 + dx, 35.0, 14.0, 6.0, 0.4, 0.0, 5.0, false);
    c.close_path();
    c.rect(15.0 + dx, 45.0, 20.0, 10.0);
}

#[test]
fn fill_rect_records_and_replays_the_same_pixels() {
    both(64, 48, |c| {
        c.set_fill_style(color(200, 30, 40, 1.0));
        c.fill_rect(3.5, 4.25, 30.0, 20.0);
        c.set_fill_style(color(10, 200, 40, 0.4));
        c.rotate(0.3);
        c.fill_rect(20.0, -5.0, 25.0, 30.0);
        c.set_global_alpha(0.5);
        c.fill_rect(0.0, 30.0, 64.0, 5.0);
    });
}

#[test]
fn fill_paths_under_both_rules_record_and_replay_the_same_pixels() {
    both(160, 80, |c| {
        c.set_fill_style(color(30, 60, 200, 0.8));
        shape(c, 0.0);
        c.fill_with_rule(false);
        c.translate(2.5, 0.0);
        c.scale(1.1, 0.9);
        shape(c, 70.0);
        c.fill_with_rule(true);
    });
}

#[test]
fn strokes_with_every_cap_join_and_dash_record_and_replay_the_same_pixels() {
    both(200, 160, |c| {
        let caps = [LineCap::Butt, LineCap::Round, LineCap::Square];
        let joins = [LineJoin::Miter, LineJoin::Round, LineJoin::Bevel];
        for (i, cap) in caps.iter().enumerate() {
            for (j, join) in joins.iter().enumerate() {
                c.save();
                c.translate(i as f32 * 65.0, j as f32 * 50.0);
                c.set_line_cap(*cap);
                c.set_line_join(*join);
                c.set_line_width(1.0 + (i * 3 + j) as f32 * 1.5);
                c.set_miter_limit(2.0 + j as f32);
                if (i + j) % 2 == 1 {
                    c.set_line_dash(vec![4.0, 2.0, 1.0]);
                    c.set_line_dash_offset(1.5 * i as f32);
                }
                c.set_stroke_style(color(20 * i as u8, 40 * j as u8, 120, 0.9));
                c.begin_path();
                c.move_to(5.0, 5.0);
                c.line_to(40.0, 30.0);
                c.line_to(10.0, 40.0);
                c.arc(30.0, 20.0, 9.0, 0.0, 2.5, false);
                c.stroke();
                c.restore();
            }
        }
        // A thin stroke under a scale (the stroker's resolution scale).
        c.scale(3.0, 0.5);
        c.set_line_width(0.3);
        c.stroke_rect(2.0, 250.0, 50.0, 40.0);
    });
}

#[test]
fn clears_under_a_transform_and_a_clip_record_and_replay_the_same_pixels() {
    // A clear before anything is drawn leaves no bitmap, in both.
    let mut direct = cpu_ctx(40, 30);
    direct.set_immediate_raster(true);
    let mut recorded = cpu_ctx(40, 30);
    for c in [&mut direct, &mut recorded] {
        c.clear_rect(0.0, 0.0, 10.0, 10.0);
    }
    same(&direct, &recorded);
    both(80, 60, |c| {
        c.set_fill_style(color(250, 120, 0, 1.0));
        c.fill_rect(0.0, 0.0, 80.0, 60.0);
        c.save();
        c.rotate(0.2);
        c.clear_rect(10.3, 5.7, 20.0, 15.0);
        c.restore();
        c.begin_path();
        c.arc(50.0, 30.0, 15.0, 0.0, 6.3, false);
        c.clip();
        c.clear_rect(40.0, 0.0, 30.0, 60.0);
    });
}

#[test]
fn gradients_and_patterns_record_and_replay_the_same_pixels() {
    both(120, 90, |c| {
        let mut g = c.create_linear_gradient(0.0, 0.0, 120.0, 30.0);
        g.add_color_stop(0.0, Color::new(255, 0, 0, 1.0));
        g.add_color_stop(0.6, Color::new(0, 0, 255, 0.5));
        g.add_color_stop(1.0, Color::new(0, 255, 0, 1.0));
        c.set_fill_style(CanvasStyle::LinearGradient(g));
        c.fill_rect(0.0, 0.0, 120.0, 30.0);
        let mut r = c.create_radial_gradient(60.0, 50.0, 5.0, 60.0, 50.0, 25.0);
        r.add_color_stop(0.0, Color::new(255, 255, 0, 1.0));
        r.add_color_stop(1.0, Color::new(0, 128, 128, 0.7));
        c.set_stroke_style(CanvasStyle::RadialGradient(r));
        c.set_line_width(6.0);
        c.begin_path();
        c.arc(60.0, 50.0, 18.0, 0.0, 6.3, false);
        c.stroke();
        for (k, rep) in [PatternRepetition::Repeat, PatternRepetition::RepeatX, PatternRepetition::RepeatY, PatternRepetition::NoRepeat].into_iter().enumerate() {
            c.save();
            c.set_global_alpha(0.3 + 0.2 * k as f32);
            c.translate(k as f32 * 30.0, 60.0);
            c.set_fill_style(CanvasStyle::Pattern(CanvasPattern { bitmap: Arc::new(image()), repetition: rep }));
            c.fill_rect(0.0, 0.0, 28.0, 28.0);
            c.restore();
        }
    });
}

#[test]
fn every_composite_operation_records_and_replays_the_same_pixels() {
    use CompositeOperation::*;
    let ops = [SourceOver, SourceIn, SourceOut, SourceAtop, DestinationOver, DestinationIn, DestinationOut, DestinationAtop, Lighter, Copy, Xor, Multiply, Screen, Overlay, Darken, Lighten];
    both(16 * 20, 30, |c| {
        for (i, op) in ops.iter().enumerate() {
            let x = i as f32 * 20.0;
            c.set_global_composite_operation(SourceOver);
            c.set_fill_style(color(0, 90, 200, 0.7));
            c.fill_rect(x + 2.0, 2.0, 12.0, 20.0);
            c.set_global_composite_operation(*op);
            c.set_fill_style(color(220, 40, 30, 0.6));
            c.begin_path();
            c.arc(x + 12.0, 16.0, 7.5, 0.0, 6.3, false);
            c.fill();
        }
    });
}

#[test]
fn nested_clips_with_save_and_restore_record_and_replay_the_same_pixels() {
    both(100, 80, |c| {
        c.set_fill_style(color(0, 0, 0, 0.3));
        c.fill_rect(0.0, 0.0, 100.0, 80.0);
        c.save();
        c.begin_path();
        c.arc(40.0, 40.0, 30.0, 0.0, 6.3, false);
        c.clip();
        c.set_fill_style(color(200, 0, 0, 1.0));
        c.fill_rect(0.0, 0.0, 100.0, 80.0);
        c.save();
        c.rotate(0.25);
        shape(c, 20.0);
        c.clip_with_rule(true);
        c.set_fill_style(color(0, 200, 0, 0.8));
        c.fill_rect(0.0, 0.0, 100.0, 80.0);
        c.restore();
        c.set_fill_style(color(0, 0, 200, 0.5));
        c.fill_rect(30.0, 0.0, 15.0, 80.0);
        c.restore();
        // A clip to an empty path clips everything away.
        c.save();
        c.begin_path();
        c.clip();
        c.fill_rect(0.0, 0.0, 100.0, 80.0);
        c.restore();
        c.fill_rect(80.0, 60.0, 10.0, 10.0);
    });
}

#[test]
fn draw_image_of_a_bitmap_and_of_the_canvas_itself_records_and_replays_the_same_pixels() {
    both(90, 70, |c| {
        c.draw_bitmap(image(), [0.0, 0.0, 7.0, 5.0], [2.0, 2.0, 7.0, 5.0]);
        c.draw_bitmap(image(), [1.5, 0.5, 4.0, 3.0], [10.0, 5.0, 30.0, 21.0]);
        c.save();
        c.rotate(0.4);
        c.set_global_alpha(0.6);
        c.draw_bitmap(image(), [0.0, 0.0, 7.0, 5.0], [40.0, 0.0, 21.0, 15.0]);
        c.restore();
        // The canvas drawn onto itself copies what it held at the call.
        let me = c.bitmap().expect("drawn");
        c.set_fill_style(color(0, 0, 0, 1.0));
        c.fill_rect(0.0, 0.0, 20.0, 20.0);
        c.draw_bitmap(me, [0.0, 0.0, 45.0, 35.0], [45.0, 35.0, 45.0, 35.0]);
    });
}

#[test]
fn put_image_data_records_and_replays_the_same_pixels() {
    let mut data = ImageData::new(6, 4);
    for (i, b) in data.data.iter_mut().enumerate() {
        *b = (i * 29 % 256) as u8;
    }
    both(30, 20, |c| {
        c.set_fill_style(color(10, 20, 30, 1.0));
        c.fill_rect(0.0, 0.0, 30.0, 20.0);
        c.put_image_data_dirty(&data, 2, 3, [0, 0, 6, 4]);
        c.put_image_data_dirty(&data, 27, 18, [1, 1, 5, 3]);
        c.put_image_data_dirty(&data, -2, -1, [0, 0, 6, 4]);
        c.rotate(0.5);
        c.fill_rect(5.0, 0.0, 10.0, 10.0);
    });
    // A put wholly off the bitmap, before anything is drawn, still makes it.
    let mut direct = cpu_ctx(30, 20);
    direct.set_immediate_raster(true);
    let mut recorded = cpu_ctx(30, 20);
    for c in [&mut direct, &mut recorded] {
        c.put_image_data_dirty(&data, 100, 100, [0, 0, 6, 4]);
    }
    same(&direct, &recorded);
}

#[test]
fn reset_and_resize_drop_what_is_pending() {
    both(40, 30, |c| {
        c.fill_rect(0.0, 0.0, 40.0, 30.0);
        c.reset();
        c.set_fill_style(color(0, 100, 0, 1.0));
        c.fill_rect(5.0, 5.0, 10.0, 10.0);
        c.reset_bitmap(50, 20);
        c.fill_rect(30.0, 5.0, 10.0, 10.0);
    });
    let mut c = cpu_ctx(40, 30);
    c.fill_rect(0.0, 0.0, 40.0, 30.0);
    c.reset_bitmap(40, 30);
    assert_eq!(c.pending_ops(), 0, "a resize clears the bitmap: nothing before it is left to replay");
    assert!(c.bitmap().is_none());
}

#[test]
fn reads_between_draws_see_exactly_what_was_drawn_before_them() {
    let mut direct = cpu_ctx(50, 40);
    direct.set_immediate_raster(true);
    let mut recorded = cpu_ctx(50, 40);
    for step in 0..6 {
        for c in [&mut direct, &mut recorded] {
            c.set_fill_style(color(40 * step as u8, 255 - 40 * step as u8, 90, 0.7));
            c.begin_path();
            c.arc(8.0 + step as f32 * 7.0, 20.0, 9.0, 0.0, 6.3, false);
            c.fill();
            c.clear_rect(step as f32 * 3.0, 0.0, 2.0, 40.0);
        }
        assert!(recorded.pending_ops() > 0, "step {step}");
        same(&direct, &recorded);
    }
}

#[test]
fn a_page_drawing_forever_without_reading_holds_at_most_the_budget_pending() {
    let mut direct = cpu_ctx(300, 200);
    direct.set_immediate_raster(true);
    let mut recorded = cpu_ctx(300, 200);
    let mut most = 0;
    let mut flushed = false;
    for i in 0..30_000u32 {
        for c in [&mut direct, &mut recorded] {
            c.set_stroke_style(color((i % 251) as u8, (i % 113) as u8, 200, 0.5));
            c.begin_path();
            c.arc((i % 300) as f32, (i % 200) as f32, 3.0 + (i % 17) as f32, 0.0, 4.0, false);
            c.line_to((i * 7 % 300) as f32, (i * 3 % 200) as f32);
            c.stroke();
        }
        let pending = recorded.pending_bytes();
        flushed |= pending < most;
        most = most.max(pending);
        assert!(pending <= PENDING_BYTE_BUDGET, "draw {i}: {pending} bytes pending");
    }
    assert!(flushed, "the cap replayed the recording into the bitmap ({most} bytes at most)");
    assert!(recorded.pending_ops() > 0);
    same(&direct, &recorded);
}

#[test]
fn a_long_chain_of_clips_replays_without_deep_recursion_or_unbounded_masks() {
    both(64, 64, |c| {
        for i in 0..5_000 {
            c.begin_path();
            c.rect(-1.0 + (i % 7) as f32 * 0.01, -1.0, 70.0, 70.0);
            c.clip();
        }
        c.set_fill_style(color(1, 2, 3, 0.9));
        c.fill_rect(10.0, 10.0, 30.0, 30.0);
    });
}

// #1282 R1: what pending draws keep alive counts toward the cap.

/// A page that sets a large pattern as its fill before every fill (each set
/// holds its own copy of the source's pixels, as the bindings make one per
/// `fillStyle = pattern`) on a canvas it never reads: the patterns the
/// pending draws keep alive stay within the cap (plus the one that
/// crossed it). Counted in bytes, no clock.
#[test]
fn pattern_bitmaps_held_by_pending_draws_count_toward_the_cap() {
    let side = 512u32;
    let pattern_bytes = (side * side * 4) as usize;
    let source = CanvasBitmap::from_rgba(side, side, &vec![200u8; pattern_bytes]).unwrap();
    let mut c = cpu_ctx(64, 64);
    for i in 0..64 {
        c.set_fill_style(CanvasStyle::Pattern(CanvasPattern { bitmap: Arc::new(source.clone()), repetition: PatternRepetition::Repeat }));
        c.fill_rect((i % 8) as f32, 0.0, 8.0, 8.0);
        let held = c.pending_ops() * pattern_bytes;
        assert!(held <= PENDING_BYTE_BUDGET + pattern_bytes, "fill {i}: {} pending draws hold {held} bytes of patterns", c.pending_ops());
        assert!(c.pending_bytes() >= c.pending_ops() * pattern_bytes, "fill {i}: the count misses the patterns");
    }
    // One pattern shared by many draws counts once.
    let mut c = cpu_ctx(64, 64);
    c.set_fill_style(CanvasStyle::Pattern(CanvasPattern { bitmap: Arc::new(source), repetition: PatternRepetition::Repeat }));
    for i in 0..64 {
        c.fill_rect((i % 8) as f32, 0.0, 8.0, 8.0);
    }
    assert_eq!(c.pending_ops(), 64, "a shared pattern does not fill the cap");
    assert!(c.pending_bytes() < 2 * pattern_bytes);
}

/// Clips with long paths, a new one before every draw, on a canvas never
/// read: the clip paths (and masks) the pending draws keep alive count
/// toward the cap.
#[test]
fn clip_paths_held_by_pending_draws_count_toward_the_cap() {
    let points = 20_000usize;
    let path_bytes = points * std::mem::size_of::<PathCommand>();
    let mut c = cpu_ctx(32, 32);
    for i in 0..200 {
        c.save();
        c.begin_path();
        c.move_to(0.0, 0.0);
        for k in 0..points {
            c.line_to((k % 32) as f32, (k / 32 % 32) as f32 + (i % 3) as f32);
        }
        c.clip();
        c.fill_rect(0.0, 0.0, 32.0, 32.0);
        c.restore();
        let held = c.pending_ops() * path_bytes;
        assert!(held <= PENDING_BYTE_BUDGET + path_bytes, "draw {i}: {} pending draws hold {held} bytes of clip paths", c.pending_ops());
    }
}
