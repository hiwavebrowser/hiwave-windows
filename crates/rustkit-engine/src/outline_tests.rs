//! Z-plan C5: `outline`. Until this row the family was dropped at the
//! cascade: no field, no paint, no computed value, so `outline: 2px solid
//! red` drew 0 red pixels and all of WPT's css-ui-outline matched itself.
//! Every expected number below is Chromium 143.0.7499.4's answer on the page
//! in the test (400x300, a hand-made page; ink counted over the whole frame).

use super::*;

const WIDTH: u32 = 400;
const HEIGHT: u32 = 300;
const RED: [u8; 3] = [255, 0, 0];
const BLUE: [u8; 3] = [0, 0, 255];

fn loaded(body: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT,
        })
        .expect("view");
    let html = format!(
        "<!doctype html><style>html,body{{margin:0;background:#fff}}div{{box-sizing:border-box}}</style>{body}"
    );
    engine.load_html(view, &html).expect("load_html");
    (engine, view)
}

fn frame(engine: &mut Engine, view: EngineViewId) -> Vec<u8> {
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!(
        "rustkit-outline-{}-{:?}.ppm",
        std::process::id(),
        std::thread::current().id()
    ));
    engine
        .capture_frame(view, path.to_str().unwrap())
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

fn pixels(ppm: &[u8]) -> &[u8] {
    &ppm[ppm.len() - WIDTH as usize * HEIGHT as usize * 3..]
}

fn count(ppm: &[u8], color: [u8; 3]) -> usize {
    pixels(ppm).chunks_exact(3).filter(|p| **p == color).count()
}

fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = (y * WIDTH as usize + x) * 3;
    let p = pixels(ppm);
    [p[at], p[at + 1], p[at + 2]]
}

/// A ring's area: `w` thick, `outer_w` by `outer_h` on the outside.
const fn ring(w: usize, outer_w: usize, outer_h: usize) -> usize {
    outer_w * outer_h - (outer_w - 2 * w) * (outer_h - 2 * w)
}

/// The ink count of each colour, as Chromium's frame of the same page has it.
fn assert_ink(body: &str, red: usize, blue: usize) {
    let (mut engine, view) = loaded(body);
    let ppm = frame(&mut engine, view);
    assert_eq!(
        (count(&ppm, RED), count(&ppm, BLUE)),
        (red, blue),
        "red, blue: {body}"
    );
}

fn eval(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let out = engine
        .execute_script(view, &format!("String(eval({script:?}))"))
        .expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .unwrap_or_else(|| panic!("{script} evaluated to {out}"))
        .replace("\\\"", "\"")
}

#[test]
fn a_plain_outline_paints_around_the_border_box() {
    let body = r#"<div style="position:absolute;left:50px;top:40px;width:100px;height:50px;outline:2px solid red"></div>"#;
    let (mut engine, view) = loaded(body);
    let ppm = frame(&mut engine, view);
    assert_eq!(count(&ppm, RED), 616);
    assert_eq!(pixel(&ppm, 48, 38), RED, "the outer corner");
    assert_eq!(pixel(&ppm, 151, 91), RED, "the far corner");
    assert_eq!(
        pixel(&ppm, 50, 40),
        [255, 255, 255],
        "the box itself is not inked"
    );
}

#[test]
fn an_outline_takes_no_space() {
    let (mut with, view) = loaded(
        r#"<div id=a style="margin:30px;width:100px;height:40px;outline:6px solid red;outline-offset:9px;background:#ccc"></div><div id=b style="height:20px"></div>"#,
    );
    // Chromium: the box is 30,30 100x40, its sibling follows at 100, and the
    // scrollable overflow of a scroller ignores what its child's outline reaches.
    assert_eq!(
        eval(&mut with, view, "var a=document.getElementById('a'),r=a.getBoundingClientRect();[r.x,r.y,r.width,r.height,a.offsetWidth,a.clientWidth,document.getElementById('b').getBoundingClientRect().y].join()"),
        "30,30,100,40,100,100,100"
    );
    let (mut scroller, view) = loaded(
        r#"<div id=s style="margin:30px;width:100px;height:60px;overflow:auto;background:#eee"><div style="width:50px;height:40px;margin:5px;outline:50px solid rgba(255,0,0,.2)"></div></div>"#,
    );
    assert_eq!(
        eval(
            &mut scroller,
            view,
            "var s=document.getElementById('s');[s.scrollWidth,s.scrollHeight].join()"
        ),
        "100,60"
    );
}

#[test]
fn the_offset_moves_the_ring_either_way() {
    assert_ink(
        r#"<div style="position:absolute;left:30px;top:30px;width:80px;height:50px;outline:3px solid red;outline-offset:5px"></div>"#,
        ring(3, 96, 66),
        0,
    );
    assert_ink(
        r#"<div style="position:absolute;left:30px;top:30px;width:80px;height:50px;outline:3px solid red;outline-offset:-10px"></div>"#,
        ring(3, 66, 36),
        0,
    );
}

#[test]
fn the_outline_sits_above_the_boxs_content() {
    assert_ink(
        r#"<div style="margin:20px;width:100px;height:60px;outline:4px solid red;outline-offset:-10px"><div style="width:100px;height:60px;background:#00f"></div></div>"#,
        1024,
        4976,
    );
}

#[test]
fn a_later_block_in_flow_paints_under_the_outline() {
    assert_ink(
        r#"<div style="margin:20px;width:100px;height:40px;outline:8px solid red"></div><div style="margin:-20px 40px 0;width:100px;height:40px;background:#00f"></div>"#,
        2496,
        3296,
    );
}

#[test]
fn a_later_positioned_box_paints_over_the_outline() {
    assert_ink(
        r#"<div style="margin:20px;width:100px;height:40px;outline:8px solid red"></div><div style="position:relative;margin:-20px 40px 0;width:100px;height:40px;background:#00f"></div>"#,
        1792,
        4000,
    );
}

#[test]
fn a_positioned_child_paints_over_its_parents_outline() {
    assert_ink(
        r#"<div style="margin:20px;width:100px;height:60px;outline:8px solid red;outline-offset:-20px"><div style="position:relative;width:60px;height:60px;background:#00f"></div></div>"#,
        608,
        3600,
    );
}

#[test]
fn a_later_float_paints_under_an_outline_in_flow() {
    assert_ink(
        // No negative margin on the float: the engine places a float with
        // one 20px higher than Chromium does (a layout gap, not this row's).
        r#"<div style="margin:20px;width:100px;height:40px;outline:8px solid red;outline-offset:30px"></div><div style="float:left;margin:0 40px;width:100px;height:40px;background:#00f"></div>"#,
        1984,
        3200,
    );
}

#[test]
fn a_floats_outline_paints_with_the_float() {
    assert_ink(
        r#"<div style="float:left;margin:20px;width:100px;height:40px;outline:8px solid red"></div><div style="margin:30px 0 0 60px;width:100px;height:60px;background:#00f"></div>"#,
        2496,
        5136,
    );
}

#[test]
fn a_later_inline_block_paints_over_an_inline_blocks_outline() {
    assert_ink(
        r#"<div style="margin:20px"><span style="display:inline-block;width:60px;height:40px;outline:8px solid red"></span><span style="display:inline-block;margin-left:-10px;width:60px;height:40px;background:#00f"></span></div>"#,
        1536,
        2400,
    );
}

#[test]
fn its_own_overflow_does_not_clip_a_boxs_outline() {
    let body = r#"<div style="margin:30px;width:100px;height:60px;overflow:hidden;outline:4px solid red;background:#ddd"><div style="width:200px;height:100px;background:#00f"></div></div>"#;
    assert_ink(body, ring(4, 108, 68), 6000);
}

#[test]
fn a_clipping_ancestor_clips_a_childs_outline() {
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:100px;height:60px;overflow:hidden;background:#ddd"><div style="margin:20px;width:100px;height:100px;outline:6px solid red"></div></div>"#,
    );
    let ppm = frame(&mut engine, view);
    assert_eq!(
        pixel(&ppm, 44, 44),
        RED,
        "the child's corner, inside the clip"
    );
    assert_eq!(
        pixel(&ppm, 130, 44),
        [255, 255, 255],
        "cut at the clip's right edge"
    );
    assert_eq!(
        pixel(&ppm, 44, 90),
        [255, 255, 255],
        "cut at the clip's bottom edge"
    );
}

#[test]
fn a_positioned_box_paints_its_own_outline_outside_its_clip_and_its_child_over_it() {
    // A positioned scroller, a clipped child outline, a positioned child.
    let (mut engine, view) = loaded(
        r#"<div style="position:relative;margin:30px;width:100px;height:60px;overflow:hidden;outline:4px solid red"><div style="position:absolute;left:-10px;top:40px;width:60px;height:40px;background:#00f"></div></div>"#,
    );
    let ppm = frame(&mut engine, view);
    assert_eq!(pixel(&ppm, 26, 26), RED, "own outline, outside own clip");
    assert_eq!(
        pixel(&ppm, 28, 88),
        RED,
        "and not cut by the positioned child"
    );
    assert_eq!(pixel(&ppm, 35, 80), BLUE, "the child is inside the clip");
    assert_eq!(
        pixel(&ppm, 35, 92),
        RED,
        "the child stops at the clip, the ring goes on"
    );
}

#[test]
fn nested_outlines_paint_the_child_first_and_siblings_in_order() {
    // Chromium: the parent's ring crosses over its child's.
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:120px;height:80px;outline:6px solid red;outline-offset:-20px"><div style="width:80px;height:40px;margin:10px;outline:6px solid #00f;outline-offset:6px"></div></div>"#,
    );
    let ppm = frame(&mut engine, view);
    assert_eq!(pixel(&ppm, 128, 46), RED);
    // The later sibling's ring crosses over the earlier's.
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:60px;height:40px;outline:6px solid red"></div><div style="margin:-50px 50px;width:60px;height:40px;outline:6px solid #00f"></div>"#,
    );
    let ppm = frame(&mut engine, view);
    assert_eq!(pixel(&ppm, 92, 46), BLUE);
    assert_eq!(
        pixel(&ppm, 46 + 34, 72),
        RED,
        "the earlier ring where nothing crosses it"
    );
}

#[test]
fn no_style_no_width_or_no_visibility_no_outline() {
    for body in [
        r#"<div style="margin:30px;width:80px;height:30px;outline:none"></div>"#,
        r#"<div style="margin:30px;width:80px;height:30px;outline:0 solid red"></div>"#,
        r#"<div style="margin:30px;width:80px;height:30px;outline-width:5px;outline-color:red"></div>"#,
        r#"<div style="margin:30px;width:80px;height:30px;visibility:hidden;outline:6px solid red"></div>"#,
        r#"<div style="margin:30px;width:80px;height:30px;display:none;outline:6px solid red"></div>"#,
        r#"<div style="margin:30px;width:80px;height:30px;outline:2px solid red;outline:none"></div>"#,
        r#"<div style="margin:30px;width:80px;height:30px;outline:2px solid red;outline-style:none"></div>"#,
    ] {
        assert_ink(body, 0, 0);
    }
    // Only the element paints a ring: no text run or anonymous box repeats it.
    let (mut engine, view) = loaded(
        r#"<div style="position:absolute;left:30px;top:30px;width:80px;height:50px;font:16px sans-serif;outline:2px solid red">a<div>b</div>c</div>"#,
    );
    assert_eq!(count(&frame(&mut engine, view), RED), ring(2, 84, 54));
}

#[test]
fn the_color_defaults_to_the_elements_color() {
    let (mut engine, view) = loaded(
        r#"<div style="position:absolute;left:30px;top:30px;width:80px;height:50px;color:#00f;outline:3px solid"></div>"#,
    );
    assert_eq!(count(&frame(&mut engine, view), BLUE), ring(3, 86, 56));
}

#[test]
fn a_translucent_ring_blends_once_at_the_corners() {
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:80px;height:50px;background:#00f;outline:8px solid rgba(255,0,0,.5);outline-offset:-20px"></div>"#,
    );
    let ppm = frame(&mut engine, view);
    let edge = pixel(&ppm, 45, 55);
    assert_ne!(edge, BLUE, "the ring is drawn");
    assert_eq!(
        pixel(&ppm, 45, 45),
        edge,
        "its corner is the colour of its edge"
    );
    assert_eq!(
        count(&ppm, edge),
        1056,
        "every pixel of the ring, Chromium's 80007f"
    );
}

#[test]
fn an_inline_box_is_outlined_across_its_own_box() {
    // Chromium's ring around the span runs from x 56 to 92.
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:300px;font:20px/30px sans-serif">aa <span style="outline:2px solid red">bbb</span> cc</div>"#,
    );
    let ppm = frame(&mut engine, view);
    let columns: Vec<usize> = (0..WIDTH as usize)
        .filter(|&x| (0..HEIGHT as usize).any(|y| pixel(&ppm, x, y) == RED))
        .collect();
    assert_eq!((columns.first(), columns.last()), (Some(&56), Some(&92)));
}

/// Pins a gap: an inline box is as tall as its line here (30px), and
/// Chromium rings its content area (23px): 240 red pixels, not 268. An empty
/// inline gets no box at all here, where Chromium rings its zero-wide box
/// (108). Inline box geometry, not the outline's.
#[test]
#[ignore = "inline boxes are line-tall, and an empty inline has no box"]
fn an_inline_ring_is_as_tall_as_chromiums() {
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:300px;font:20px/30px sans-serif">aa <span style="outline:2px solid red">bbb</span> cc</div>"#,
    );
    assert_eq!(count(&frame(&mut engine, view), RED), 240);
    let (mut engine, view) = loaded(
        r#"<div style="margin:30px;width:300px;font:20px/30px sans-serif">aa <span style="outline:2px solid red"></span> cc</div>"#,
    );
    assert_eq!(count(&frame(&mut engine, view), RED), 108);
}

/// The colours met walking down from above the ring at x 90, on a box at 30,30
/// 120x70: Chromium's cross-section of each style, with its width.
fn cross(style: &str, width: u32) -> Vec<([u8; 3], usize)> {
    let (mut engine, view) = loaded(&format!(
        r#"<div style="position:absolute;left:30px;top:30px;width:120px;height:70px;outline:{width}px {style} rgb(200,20,20)"></div>"#
    ));
    let ppm = frame(&mut engine, view);
    let mut runs: Vec<([u8; 3], usize)> = Vec::new();
    for y in 10..34 {
        let p = pixel(&ppm, 90, y);
        match runs.last_mut() {
            Some((c, n)) if *c == p => *n += 1,
            _ => runs.push((p, 1)),
        }
    }
    runs.retain(|(c, _)| *c != [255, 255, 255]);
    runs
}

const INK: [u8; 3] = [200, 20, 20];
const DARK: [u8; 3] = [116, 11, 11];

#[test]
fn the_styles_cross_the_top_edge_as_chromium_paints_them() {
    assert_eq!(cross("solid", 4), [(INK, 4)]);
    assert_eq!(cross("double", 2), [(INK, 2)]);
    assert_eq!(cross("double", 7), [(INK, 2), (INK, 2)], "2, a gap of 3, 2");
    assert_eq!(cross("inset", 4), [(DARK, 4)]);
    assert_eq!(cross("outset", 4), [(INK, 4)]);
    assert_eq!(cross("groove", 4), [(DARK, 2), (INK, 2)]);
    assert_eq!(cross("ridge", 4), [(INK, 2), (DARK, 2)]);
    assert_eq!(cross("groove", 1), [(INK, 1)]);
    // `auto` ignores the width: 2px, the outer one outside the box.
    assert_eq!(cross("auto", 1), [(INK, 2)]);
    assert_eq!(cross("auto", 7), [(INK, 2)]);
}

#[test]
fn the_far_sides_of_inset_and_outset_turn_the_colours_over() {
    let far = |style: &str| {
        let (mut engine, view) = loaded(&format!(
            r#"<div style="position:absolute;left:30px;top:30px;width:120px;height:70px;outline:4px {style} rgb(200,20,20)"></div>"#
        ));
        let ppm = frame(&mut engine, view);
        (
            pixel(&ppm, 90, 102),
            pixel(&ppm, 152, 60),
            pixel(&ppm, 28, 60),
        )
    };
    assert_eq!(far("inset"), (INK, INK, DARK), "bottom, right, left");
    assert_eq!(far("outset"), (DARK, DARK, INK), "bottom, right, left");
}

#[test]
fn a_hover_that_changes_an_outline_is_paint_only() {
    let (mut engine, view) = loaded(
        r#"<style>#t{outline:0 solid red;width:100px;height:50px;margin:20px}#t:hover{outline:3px solid red}</style><div id=t></div>"#,
    );
    let before = count(&frame(&mut engine, view), RED);
    assert_eq!(before, 0);
    engine.mouse_move_at_point(view, 50.0, 40.0);
    assert_eq!(
        engine.relayout_stats(view).expect("view").layouts_skipped,
        1,
        "an outline takes no space: no layout"
    );
    assert_eq!(count(&frame(&mut engine, view), RED), ring(3, 106, 56));
}

#[test]
fn computed_values_read_as_chromium_serializes_them() {
    // (declarations, outline, outline-offset), Chromium 143 under a parent
    // with color rgb(0,128,0), `outline: 3px dashed rgb(9,9,9)`,
    // `outline-offset: 7px` and font-size 20px.
    let cases: &[(&str, &str, &str)] = &[
        ("", "rgb(0, 128, 0) none 0px", "0px"),
        ("outline:2px solid red", "rgb(255, 0, 0) solid 2px", "0px"),
        ("outline:none", "rgb(0, 128, 0) none 0px", "0px"),
        ("outline:0", "rgb(0, 128, 0) none 0px", "0px"),
        ("outline:solid", "rgb(0, 128, 0) solid 3px", "0px"),
        ("outline:red", "rgb(255, 0, 0) none 0px", "0px"),
        ("outline:thin dotted", "rgb(0, 128, 0) dotted 1px", "0px"),
        (
            "outline:medium dashed blue",
            "rgb(0, 0, 255) dashed 3px",
            "0px",
        ),
        (
            "outline:thick double #0f0",
            "rgb(0, 255, 0) double 5px",
            "0px",
        ),
        ("outline:auto", "rgb(0, 128, 0) auto 3px", "0px"),
        ("outline:auto 5px", "rgb(0, 128, 0) auto 5px", "0px"),
        ("outline-style:auto", "rgb(0, 128, 0) auto 3px", "0px"),
        ("outline-width:3px", "rgb(0, 128, 0) none 0px", "0px"),
        (
            "outline-style:solid;outline-width:thin",
            "rgb(0, 128, 0) solid 1px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:thick",
            "rgb(0, 128, 0) solid 5px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:-1px",
            "rgb(0, 128, 0) solid 3px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:1.5px",
            "rgb(0, 128, 0) solid 1px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:0.5px",
            "rgb(0, 128, 0) solid 1px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:0.2em",
            "rgb(0, 128, 0) solid 4px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:calc(1px + 2px)",
            "rgb(0, 128, 0) solid 3px",
            "0px",
        ),
        (
            "outline-style:solid;outline-width:10%",
            "rgb(0, 128, 0) solid 3px",
            "0px",
        ),
        (
            "outline-color:currentcolor",
            "rgb(0, 128, 0) none 0px",
            "0px",
        ),
        (
            "outline-color:transparent",
            "rgba(0, 0, 0, 0) none 0px",
            "0px",
        ),
        ("outline-color:invert", "rgb(0, 128, 0) none 0px", "0px"),
        (
            "outline-color:rgba(1,2,3,.5)",
            "rgba(1, 2, 3, 0.5) none 0px",
            "0px",
        ),
        ("outline-offset:4px", "rgb(0, 128, 0) none 0px", "4px"),
        ("outline-offset:-3px", "rgb(0, 128, 0) none 0px", "-3px"),
        ("outline-offset:1em", "rgb(0, 128, 0) none 0px", "20px"),
        ("outline-offset:1.5px", "rgb(0, 128, 0) none 0px", "1.5px"),
        (
            "outline-offset:calc(1px + 2px)",
            "rgb(0, 128, 0) none 0px",
            "3px",
        ),
        (
            "outline-offset:0.2px",
            "rgb(0, 128, 0) none 0px",
            "0.1875px",
        ),
        ("outline-offset:10%", "rgb(0, 128, 0) none 0px", "0px"),
        ("outline-style:hidden", "rgb(0, 128, 0) none 0px", "0px"),
        ("outline:hidden", "rgb(0, 128, 0) none 0px", "0px"),
        (
            "outline:2px solid red 3px",
            "rgb(0, 128, 0) none 0px",
            "0px",
        ),
        (
            "outline:2px solid red;outline:none",
            "rgb(0, 128, 0) none 0px",
            "0px",
        ),
        (
            "outline:2px solid red;outline-style:none",
            "rgb(255, 0, 0) none 0px",
            "0px",
        ),
        (
            "outline:2px solid red;outline:initial",
            "rgb(0, 128, 0) none 0px",
            "0px",
        ),
        (
            "outline:2px solid red;outline:unset",
            "rgb(0, 128, 0) none 0px",
            "0px",
        ),
        (
            "outline:2px solid red;outline-width:initial",
            "rgb(255, 0, 0) solid 3px",
            "0px",
        ),
        (
            "outline:2px solid red;outline-color:initial",
            "rgb(0, 128, 0) solid 2px",
            "0px",
        ),
        (
            "outline:2px solid red;outline-style:initial",
            "rgb(255, 0, 0) none 0px",
            "0px",
        ),
        (
            "outline-offset:4px;outline-offset:initial",
            "rgb(0, 128, 0) none 0px",
            "0px",
        ),
        ("outline:inherit", "rgb(9, 9, 9) dashed 3px", "0px"),
        ("outline-color:inherit", "rgb(9, 9, 9) none 0px", "0px"),
        ("outline-style:inherit", "rgb(0, 128, 0) dashed 3px", "0px"),
        ("outline-width:inherit", "rgb(0, 128, 0) none 0px", "0px"),
        ("outline-offset:inherit", "rgb(0, 128, 0) none 0px", "7px"),
        // Values are case-insensitive. (An upper-case property NAME in a sheet
        // is dropped for every property, not only these: not this row's.)
        ("outline:2PX SOLID RED", "rgb(255, 0, 0) solid 2px", "0px"),
        ("outline:2px groove red", "rgb(255, 0, 0) groove 2px", "0px"),
        ("outline:2px ridge red", "rgb(255, 0, 0) ridge 2px", "0px"),
        ("outline:2px inset red", "rgb(255, 0, 0) inset 2px", "0px"),
        ("outline:2px outset red", "rgb(255, 0, 0) outset 2px", "0px"),
        (
            "outline:2px solid currentcolor",
            "rgb(0, 128, 0) solid 2px",
            "0px",
        ),
        ("outline:0 solid red", "rgb(255, 0, 0) solid 0px", "0px"),
        ("outline:1em solid", "rgb(0, 128, 0) solid 20px", "0px"),
        ("outline:red 1px dotted", "rgb(255, 0, 0) dotted 1px", "0px"),
        (
            "outline: solid 2px rgb(1, 2, 3)",
            "rgb(1, 2, 3) solid 2px",
            "0px",
        ),
        ("outline:none 2px red", "rgb(255, 0, 0) none 0px", "0px"),
        (
            "display:none;outline:2px solid red",
            "rgb(255, 0, 0) solid 2px",
            "0px",
        ),
    ];
    let rules: String = cases
        .iter()
        .enumerate()
        .map(|(i, (decl, _, _))| format!("<style>#d{i}{{{decl}}}</style><div id=d{i}>x</div>"))
        .collect();
    let (mut engine, view) = loaded(&format!(
        "<style>#par{{color:rgb(0,128,0);outline:3px dashed rgb(9,9,9);outline-offset:7px;font-size:20px}}</style><div id=par>{rules}</div>"
    ));
    let mut wrong = Vec::new();
    for (i, (decl, outline, offset)) in cases.iter().enumerate() {
        let got = eval(
            &mut engine,
            view,
            &format!("var c=getComputedStyle(document.getElementById('d{i}'));[c.outline,c.outlineOffset,c.getPropertyValue('outline')].join('|')"),
        );
        if got != format!("{outline}|{offset}|{outline}") {
            wrong.push(format!("{decl:?}: {got}, Chromium {outline}|{offset}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        cases.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_longhands_read_as_chromium_serializes_them() {
    let (mut engine, view) = loaded(
        "<style>#d{color:rgb(0,128,0);outline:2px dashed rgb(1,2,3);outline-offset:-3px}#e{color:rgb(0,128,0)}</style><div id=d>x</div><div id=e>y</div>",
    );
    assert_eq!(
        eval(&mut engine, view, "var c=getComputedStyle(document.getElementById('d'));[c.outlineColor,c.outlineStyle,c.outlineWidth,c.outlineOffset].join('|')"),
        "rgb(1, 2, 3)|dashed|2px|-3px"
    );
    assert_eq!(
        eval(&mut engine, view, "var c=getComputedStyle(document.getElementById('e'));[c.outlineColor,c.outlineStyle,c.outlineWidth,c.outlineOffset].join('|')"),
        "rgb(0, 128, 0)|none|0px|0px"
    );
}

/// Pins a gap (#867 R1 finding 1). Chromium keeps A's ring over C: the
/// outlines of a paint layer's normal flow paint after all of it. Here the
/// positioned box inside B flushes the waiting rings early (the engine paints
/// a positioned box where it meets it, not after the normal flow), so C's
/// background covers A's ring where they overlap.
#[test]
#[ignore = "a positioned descendant flushes earlier siblings' rings before later in-flow backgrounds"]
fn a_positioned_box_in_a_middle_sibling_keeps_an_earlier_ring_on_top() {
    assert_ink(
        r#"<div style="margin:20px;width:100px;height:40px;outline:10px solid red"></div><div style="margin:0 20px;height:5px"><div style="position:relative;width:5px;height:5px"></div></div><div style="margin:-25px 20px 0;width:100px;height:40px;background:#00f"></div>"#,
        3200,
        3000,
    );
}

#[test]
fn a_large_negative_offset_keeps_twice_the_width_about_the_centre() {
    // Chromium: 80x50 at 30,30 with a 3px ring at -40 is a 6x6 block at
    // 67,52; at -30 it is 26 wide and 6 tall.
    let (mut engine, view) = loaded(
        r#"<div style="position:absolute;left:30px;top:30px;width:80px;height:50px;outline:3px solid red;outline-offset:-40px"></div>"#,
    );
    let ppm = frame(&mut engine, view);
    assert_eq!(count(&ppm, RED), 36);
    assert_eq!((pixel(&ppm, 67, 52), pixel(&ppm, 72, 57)), (RED, RED));
    assert_ink(
        r#"<div style="position:absolute;left:30px;top:30px;width:80px;height:50px;outline:3px solid red;outline-offset:-30px"></div>"#,
        156,
        0,
    );
}

#[test]
fn a_tables_ring_goes_around_its_caption_too() {
    // Chromium rings the table wrapper: grid and the caption's margin box.
    let table = |side: &str, caption: &str| {
        format!(
            r#"<table style="margin:30px;outline:3px solid red;border-spacing:0;font:16px/20px sans-serif;caption-side:{side}"><caption style="{caption}">cap</caption><tr><td style="width:100px;height:40px;padding:0"></td></tr></table>"#
        )
    };
    let (mut engine, view) = loaded(&table("top", ""));
    let ppm = frame(&mut engine, view);
    assert_eq!(count(&ppm, RED), 996);
    assert_eq!(pixel(&ppm, 27, 27), RED, "the ring's corner, above the caption");
    let (mut engine, view) = loaded(&table("bottom", "margin:4px"));
    let ppm = frame(&mut engine, view);
    assert_eq!(count(&ppm, RED), 1044);
    assert_eq!(pixel(&ppm, 132, 100), RED, "the ring's corner, below the caption");
}

#[test]
fn dashes_a_clip_hides_are_not_drawn() {
    // #867 R1 finding 4: 3000 dashed rows in a 280px clipper drew every dash
    // of every row, about 150 commands a row.
    let rows: String = (0..3000)
        .map(|_| r#"<div style="height:2px;margin:2px;outline:1px dashed red"></div>"#)
        .collect();
    let (mut engine, view) =
        loaded(&format!(r#"<div style="height:280px;overflow:hidden">{rows}</div>"#));
    let ppm = frame(&mut engine, view);
    let commands = engine.views[&view].display_list.as_ref().expect("list").commands.len();
    assert!(commands < 30_000, "{commands} commands for about 70 visible rows");
    assert_eq!(pixel(&ppm, 1, 1), RED, "the first row's dash is still drawn");
    assert!(count(&ppm, RED) > 0 && (281..300).all(|y| pixel(&ppm, 100, y) != RED));
}
