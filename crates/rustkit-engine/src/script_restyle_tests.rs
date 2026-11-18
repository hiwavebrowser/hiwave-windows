//! LPF1: a relayout asked for by script style writes that change only what is painted builds only
//! the written elements and patches their paint onto the laid-out boxes, as the pointer restyle
//! does, instead of building the whole document (LAYOUT-PER-FRAME: on MotionMark Multiply's shape
//! the build was ~78% of a 112-122 ms frame and scaled with the document,
//! `docs/diagnostics/2026-11-16/layout_per_frame.md`). Chrome keeps 60 frames a second on that page
//! (`layout_per_frame_probe_cft148.json`).
//!
//! The short path must paint exactly what the full rebuild paints: `the_short_path_paints_what_the_full_rebuild_paints`
//! runs the same writes with the path on and off and compares the display lists after every frame.

use super::*;
use crate::script_net_tests::serve_routes;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
}

/// `tiles` absolutely positioned tiles in a positioned, clipped stage, and an animation-frame loop
/// that runs `write` (with `el` a tile and `n` the frame) on 3 tiles a frame.
fn page(tiles: usize, write: &str) -> String {
    let mut p = String::from(
        "<!doctype html><style>body{margin:0}#stage{position:relative;width:800px;height:600px;overflow:hidden}\
         .t{position:absolute;width:12px;height:12px;background:#08f}</style><div id=stage>",
    );
    for i in 0..tiles {
        let (x, y) = ((i % 60) * 13, (i / 60) * 13);
        p.push_str(&format!("<div class=t style=\"left:{x}px;top:{y}px\"></div>"));
    }
    p.push_str(&format!(
        "</div><script>var tiles = document.querySelectorAll('.t'), n = 0; \
         requestAnimationFrame(function f() {{ n++; for (var k = 0; k < 3; k++) {{ var el = tiles[(n * 7 + k * 331) % tiles.length]; {write} }} requestAnimationFrame(f); }});</script>"
    ));
    p
}

fn load(page: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    rt().block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn frame(engine: &mut Engine, view: EngineViewId) {
    rt().block_on(engine.pump_live(view, 17));
}

const BACKGROUND: &str = "el.style.backgroundColor = 'hsl(' + (n * 37 % 360) + ',60%,50%)';";

/// After the first write (which builds the page in full and keeps the tree it built), every frame
/// whose writes change only a written tile's paint takes the short path: no full build, and the
/// build goes into a handful of elements (the written tiles and the path down to them), not the
/// page's 2,000.
#[test]
fn a_script_paint_write_builds_only_the_written_elements() {
    let (mut engine, view, _server) = load(&page(2_000, BACKGROUND));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    let before = engine.relayout_stats(view).expect("view").clone();
    let mut walked = Vec::new();
    for _ in 0..5 {
        frame(&mut engine, view);
        walked.push(engine.relayout_stats(view).expect("view").last_restyle_walked);
    }
    let after = engine.relayout_stats(view).expect("view");
    assert_eq!(
        after.script_restyles - before.script_restyles,
        5,
        "frames that built only the written tiles (relayouts {}, layouts skipped {}, last miss {:?})",
        after.relayouts - before.relayouts,
        after.layouts_skipped - before.layouts_skipped,
        last_restyle_miss_for_test()
    );
    assert!(walked.iter().all(|&w| w > 0 && w <= 40), "elements the build went into per frame: {walked:?}");
}

/// The short path paints exactly what the full rebuild paints: the same page and writes, the path
/// on in one engine and off in the other, and the two display lists equal after every frame.
#[test]
fn the_short_path_paints_what_the_full_rebuild_paints() {
    let lists = |on: bool| {
        set_script_restyle_for_test(on);
        let (mut engine, view, _server) = load(&page(300, BACKGROUND));
        let mut out = Vec::new();
        for _ in 0..8 {
            frame(&mut engine, view);
            out.push(format!("{:?}", engine.views[&view].display_list.as_ref().expect("display list").commands));
        }
        set_script_restyle_for_test(true);
        (out, engine.relayout_stats(view).expect("view").script_restyles)
    };
    let ((short, took), (full, none)) = (lists(true), lists(false));
    assert!(took >= 6, "the short path ran {took} times of 8 frames (the first builds in full)");
    assert_eq!(none, 0, "the path was off in the other arm");
    for (i, (a, b)) in short.iter().zip(&full).enumerate() {
        assert!(a == b, "frame {i}: the short path's display list differs from the full rebuild's");
    }
}

/// LPF2's starting point: what the engine reports today (a full rebuild) for the transform and
/// opacity probe Chrome is pinned on (`transform_paint_probe_cft148.json`). Printed, not asserted.
#[test]
#[ignore]
fn transform_paint_probe_on_the_engine() {
    let html = include_str!("../../../docs/diagnostics/2026-11-16/transform_paint_probe.html");
    let (mut engine, view, _server) = load(html);
    let read = |engine: &mut Engine, expr: &str| engine.execute_script(view, &format!("JSON.stringify({expr})")).unwrap();
    for part in ["overflow", "fixed", "rect", "clip", "opacity"] {
        eprintln!("{part}: {}", read(&mut engine, &format!("window.rec && window.rec.{part}")));
    }
}

/// `tiles` tiles that already have a transform (as Multiply's do once animated), in the same stage.
fn transformed_page(tiles: usize, write: &str) -> String {
    page(tiles, write).replace(".t{position:absolute;", ".t{transform:rotate(0deg);opacity:0.9;position:absolute;")
}

const TRANSFORM: &str = "el.style.transform = 'rotate(' + (n * 13 % 360) + 'deg)'; \
                         el.style.backgroundColor = 'hsl(' + (n * 37 % 360) + ',60%,50%)'; \
                         el.style.opacity = String(0.5 + (n % 4) / 10);";

/// LPF2: a transform that stays a transform and an opacity that stays below 1 change only what is
/// painted: the box keeps its stacking context and its containing block for fixed and absolute
/// descendants (Chrome, `transform_paint_probe_cft148.json`: between two transforms a fixed child
/// moves with its parent; opacity moves no box). MotionMark Multiply's writes, on tiles that already
/// have a transform, take the short path every frame. They took the full build.
#[test]
fn a_transform_and_opacity_write_that_keeps_what_the_box_establishes_is_paint() {
    let (mut engine, view, _server) = load(&transformed_page(2_000, TRANSFORM));
    for _ in 0..3 {
        frame(&mut engine, view);
    }
    let before = engine.relayout_stats(view).expect("view").script_restyles;
    for _ in 0..5 {
        frame(&mut engine, view);
    }
    let after = engine.relayout_stats(view).expect("view").script_restyles;
    assert_eq!(after - before, 5, "frames on the short path (last miss {:?})", last_restyle_miss_for_test());
}

/// The cross-check for LPF2: transform and opacity writes give the full rebuild's display list and
/// the full rebuild's element rects (`getBoundingClientRect` follows the rotation, as Chrome's
/// does) after every frame.
#[test]
fn transform_writes_on_the_short_path_paint_and_measure_what_the_full_rebuild_does() {
    let run = |on: bool| {
        set_script_restyle_for_test(on);
        let (mut engine, view, _server) = load(&transformed_page(300, TRANSFORM));
        let mut out = Vec::new();
        for _ in 0..8 {
            frame(&mut engine, view);
            let rects = engine
                .execute_script(view, "JSON.stringify([0, 7, 201].map(function (i) { var b = tiles[i].getBoundingClientRect(); return [b.left, b.top, b.width]; }))")
                .unwrap();
            out.push((format!("{:?}", engine.views[&view].display_list.as_ref().expect("display list").commands), rects));
        }
        set_script_restyle_for_test(true);
        (out, engine.relayout_stats(view).expect("view").script_restyles)
    };
    let ((short, took), (full, _)) = (run(true), run(false));
    assert!(took >= 6, "the short path ran {took} times of 8 frames");
    for (i, (a, b)) in short.iter().zip(&full).enumerate() {
        assert!(a.0 == b.0, "frame {i}: the display list differs from the full rebuild's");
        assert_eq!(a.1, b.1, "frame {i}: the rects differ from the full rebuild's");
    }
}

/// A transform that comes or goes changes what the box establishes: its fixed child is placed
/// against it (Chrome: `none` -> `translateX(0px)` moves a fixed child from (0, 0) to its parent's
/// corner). That write is laid out; the short path is not taken.
#[test]
fn a_transform_that_comes_is_laid_out() {
    let html = "<!doctype html><style>body{margin:0}#out{position:absolute;left:100px;top:300px;width:60px;height:60px}\
                #fx{position:fixed;left:0;top:0;width:10px;height:10px}</style><div id=out><div id=fx></div></div>\
                <script>var out = document.getElementById('out'), fx = document.getElementById('fx'), n = 0; \
                requestAnimationFrame(function f() { n++; out.style.transform = n < 3 ? 'none' : 'translateX(0px)'; requestAnimationFrame(f); });</script>";
    let (mut engine, view, _server) = load(html);
    for _ in 0..5 {
        frame(&mut engine, view);
    }
    let rect = engine
        .execute_script(view, "JSON.stringify((function (b) { return [b.left, b.top]; })(fx.getBoundingClientRect()))")
        .unwrap();
    assert!(rect.contains("[100,300]"), "the fixed child against its transformed parent: {rect}");
}

/// KNOWN DIVERGENCE, row TRANSFORM-OVERFLOW: Chrome counts a transformed box in its scroller's
/// scrollable overflow (`transform_paint_probe_cft148.json`, "overflow": `translateX(400px)` on a
/// 50 px box in a 200 px scroller makes `scrollWidth` 450, also under `overflow: hidden`); the
/// engine's layout does not (200), with or without the short path. When that is fixed, a transform
/// write in a scroll container must update the scroller's extent or leave the short path.
#[test]
fn known_divergence_a_transform_is_not_in_scrollable_overflow() {
    let html = "<!doctype html><style>body{margin:0}#sc{width:200px;height:100px;overflow:auto}#sc i{display:block;width:50px;height:50px}</style>\
                <div id=sc><i id=sci></i></div>";
    let (mut engine, view, _server) = load(html);
    let got = engine
        .execute_script(view, "String((sci.style.transform = 'translateX(400px)', sc.scrollWidth))")
        .unwrap();
    assert!(got.contains("200"), "Chrome: 450; here {got}");
}

/// The cross-check on any page: the same page and script run with the short path on and off;
/// the display lists (and `probe`'s value, if any) must be equal after every frame, and differ
/// between some two frames (the page did change). Returns how many frames took the short path.
fn cross_check(html: &str, frames: usize, probe: &str) -> u32 {
    cross_check_go(html, frames, probe, 0)
}

/// [`cross_check`], and before frame `go_at` (a frame after the load, whose own frames run ahead)
/// script sets `window.go = 1`: the page does its one-off write in that frame's task.
fn cross_check_go(html: &str, frames: usize, probe: &str, go_at: usize) -> u32 {
    let run = |on: bool| {
        set_script_restyle_for_test(on);
        let (mut engine, view, _server) = load(html);
        let mut out = Vec::new();
        for i in 0..frames {
            if go_at > 0 && i == go_at {
                engine.execute_script(view, "window.go = 1").unwrap();
            }
            frame(&mut engine, view);
            let read = if probe.is_empty() { String::new() } else { engine.execute_script(view, probe).unwrap() };
            out.push((format!("{:?}", engine.views[&view].display_list.as_ref().expect("display list").commands), read));
        }
        set_script_restyle_for_test(true);
        (out, engine.relayout_stats(view).expect("view").script_restyles)
    };
    let ((short, took), (full, none)) = (run(true), run(false));
    assert_eq!(none, 0, "the path was off in the other arm");
    for (i, (a, b)) in short.iter().zip(&full).enumerate() {
        assert!(a.0 == b.0, "frame {i}: the display list differs from the full rebuild's (short path ran {took} times)");
        assert_eq!(a.1, b.1, "frame {i}: the probe differs from the full rebuild's");
    }
    assert!(full.windows(2).any(|w| w[0].0 != w[1].0), "the page did not change between frames");
    eprintln!("cross_check: short path took {took} of {frames} frames (last miss {:?})", last_restyle_miss_for_test());
    took
}

/// R1 hold 1: a write the mutation log does not record (here a sheet enabled through
/// `sheet.disabled`) in the same task as a paint write: the whole page restyles, not only the
/// written element. The short path took the paint write alone and the page kept its old theme.
#[test]
fn an_unlogged_sheet_write_in_the_task_restyles_the_page() {
    let html = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f}</style>\
                <style id=dk>.t{background:#222}</style>\
                <div class=t></div><div class=t></div><div class=t id=w></div>\
                <script>var dk = document.getElementById('dk').sheet, w = document.getElementById('w'), n = 0; dk.disabled = true; \
                requestAnimationFrame(function f() { n++; if (window.go === 1) { window.go = 2; dk.disabled = false; } \
                w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    assert!(cross_check_go(html, 6, "", 3) >= 3, "the short path ran around the toggle");
}

/// R1 hold 2: an element scroll in the same task as a paint write is laid out: the scroller's
/// offset moves with the frame. The short path recoloured and did not scroll.
#[test]
fn an_element_scroll_in_the_task_is_not_dropped() {
    let html = "<!doctype html><style>body{margin:0}#sc{width:200px;height:100px;overflow:auto}#in{width:2000px;height:50px;background:linear-gradient(90deg,#000,#fff)}\
                #dot{width:20px;height:20px;background:#08f}</style><div id=sc><div id=in></div></div><div id=dot></div>\
                <script>var sc = document.getElementById('sc'), dot = document.getElementById('dot'), n = 0; \
                requestAnimationFrame(function f() { n++; sc.scrollLeft += 7; dot.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check(html, 6, "String(sc.scrollLeft)");
}

/// R1 hold 2, the window: `scrollTo` in the same task as a paint write, with a sticky header.
#[test]
fn a_window_scroll_in_the_task_is_not_dropped() {
    let html = "<!doctype html><style>body{margin:0;height:3000px}#h{position:sticky;top:0;height:30px;background:#ccc}\
                #dot{width:20px;height:20px;background:#08f}</style><div id=h></div><div style=\"height:200px\"></div><div id=dot></div>\
                <script>var dot = document.getElementById('dot'), n = 0; \
                requestAnimationFrame(function f() { n++; window.scrollTo(0, n * 11); dot.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check(html, 6, "String(window.scrollY)");
}

/// R1 hold 1, `adoptedStyleSheets` (also unlogged): adopted in the same task as a paint write.
#[test]
fn an_adopted_sheet_in_the_task_restyles_the_page() {
    let html = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f}</style>\
                <div class=t></div><div class=t></div><div class=t id=w></div>\
                <script>var dk = new CSSStyleSheet(); dk.replaceSync('.t{background:#222}'); var w = document.getElementById('w'), n = 0; \
                requestAnimationFrame(function f() { n++; if (window.go === 1) { window.go = 2; document.adoptedStyleSheets = [dk]; } \
                w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    assert!(cross_check_go(html, 6, "", 3) >= 3, "the short path ran around the adoption");
}

/// R1's wider cross-check: what a written element's subtree inherits or derives from the written
/// value repaints as the full rebuild does: inherited `color` on text, a `currentColor` border, a
/// custom property consumed below (`--x`).
#[test]
fn inherited_and_derived_paint_follow_the_write() {
    let html = "<!doctype html><style>body{margin:0}#w{width:200px;padding:4px;border:3px solid currentColor;--x:#0a0}\
                #w b{color:inherit}#w i{display:block;width:20px;height:20px;background:var(--x)}</style>\
                <div id=w><b>text that inherits</b><i></i></div>\
                <script>var w = document.getElementById('w'), n = 0; requestAnimationFrame(function f() { n++; \
                w.style.color = 'hsl(' + (n * 40 % 360) + ',60%,40%)'; w.style.setProperty('--x', 'hsl(' + (n * 70 % 360) + ',60%,50%)'); \
                requestAnimationFrame(f); });</script>";
    assert!(cross_check(html, 6, "getComputedStyle(document.querySelector('#w i')).backgroundColor") >= 3);
}

/// A `background-color` transition starts from a script write on the short path as on the full one.
#[test]
fn a_transition_starts_from_a_short_path_write() {
    let html = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f;transition:background-color 50ms linear}</style>\
                <div class=t id=w></div><script>var w = document.getElementById('w'), n = 0; requestAnimationFrame(function f() { n++; \
                if (n % 3 === 0) w.style.backgroundColor = n % 2 ? '#f00' : '#00f'; else w.style.outlineColor = 'hsl(' + n * 30 + ',50%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check(html, 7, "String(w.getAnimations().length)");
}

/// A write that changes more than paint (a miss), then paint writes again: the path sits out its
/// cooldown and comes back, and every frame paints as the full rebuild does.
#[test]
fn a_miss_then_paint_writes_again_paint_as_the_full_rebuild() {
    let html = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f}</style>\
                <div class=t id=w></div><div class=t></div><script>var w = document.getElementById('w'), n = 0; \
                requestAnimationFrame(function f() { n++; if (window.go === 1) { window.go = 2; w.style.width = '40px'; } \
                w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check_go(html, 80, "", 5);
    // Counted once the path has settled after the load: every frame writes a colour, and one frame
    // a width as well. The width write misses; its full build keeps the tree, and the next frame's
    // paint write takes the short path again (LPF3: a one-off miss sits out no relayouts). So that
    // frame is the one full build, and every other relayout takes the short path.
    let (mut engine, view, _server) = load(html);
    settle_path(&mut engine, view);
    engine.execute_script(view, "window.go = 1").unwrap();
    let counted = count_frames(&mut engine, view, 40);
    assert!(counted.relaid >= 35, "only {} relayouts in 40 frames", counted.relaid);
    assert_eq!(
        counted.relaid - counted.hits,
        1,
        "{} of {} relayouts took the full build after one width write",
        counted.relaid - counted.hits,
        counted.relaid
    );
}

/// A geometry read between two paint writes in one task (a forced layout) leaves the frame
/// painting as the full rebuild does.
#[test]
fn a_forced_read_mid_task_paints_as_the_full_rebuild() {
    let html = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f}</style>\
                <div class=t id=a></div><div class=t id=b></div><script>var a = document.getElementById('a'), b = document.getElementById('b'), n = 0, h = 0; \
                requestAnimationFrame(function f() { n++; a.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; \
                h = b.getBoundingClientRect().top; b.style.backgroundColor = 'hsl(' + (n * 70 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check(html, 6, "String(h)");
}

/// A pointer restyle (`:hover`) between script paint writes (R1: a tile kept under `:hover` red, the
/// pointer leaves and the pointer path paints it blue, script then sets it red): the short path
/// copies every written box's paint from its fresh build, not only what differs from the kept tree
/// (which still says red), so the tile is red as in the full rebuild. Copying only the differences,
/// it stayed blue.
#[test]
fn a_hover_between_script_writes_paints_as_the_full_rebuild() {
    let html = "<!doctype html><style>body{margin:0}.t{width:40px;height:40px;background:#08f}.t:hover{background:#f00}</style>\
                <div class=t id=w></div><div class=t id=v></div><script>var w = document.getElementById('w'), n = 0; \
                requestAnimationFrame(function f() { n++; w.style.outlineColor = 'hsl(' + n * 30 + ',50%,50%)'; \
                if (window.go === 1) { window.go = 2; w.style.backgroundColor = '#f00'; } requestAnimationFrame(f); });</script>";
    let run = |on: bool| {
        set_script_restyle_for_test(on);
        let (mut engine, view, _server) = load(html);
        let mut out = Vec::new();
        for i in 0..8 {
            if i == 1 {
                engine.mouse_move_at_point(view, 10.0, 10.0);
            }
            if i == 4 {
                engine.mouse_move_at_point(view, 300.0, 300.0);
            }
            if i == 6 {
                engine.execute_script(view, "window.go = 1").unwrap();
            }
            frame(&mut engine, view);
            out.push(format!("{:?}", engine.views[&view].display_list.as_ref().expect("display list").commands));
        }
        set_script_restyle_for_test(true);
        out
    };
    let (short, full) = (run(true), run(false));
    for (i, (a, b)) in short.iter().zip(&full).enumerate() {
        assert!(a == b, "frame {i}: the display list differs from the full rebuild's");
    }
}

/// `RUSTKIT_SCRIPT_RESTYLE` reads as on unless it is `0`; the batch turns the path off with it.
#[test]
fn the_production_switch_is_read_from_the_environment() {
    assert!(script_restyle_enabled() || std::env::var_os("RUSTKIT_SCRIPT_RESTYLE").is_some_and(|v| v == "0"));
}

/// R1 delta hold: a control's selection moved by script (`setSelectionRange` on the focused field)
/// in the same task as a paint write. The selection move is in no `MutationLog` record, so the
/// task is not all in the set the short path trusts: the highlight is painted as the full rebuild
/// paints it. Trusted as logged (a `form` mark inferred as logged from its kind), only the
/// colour was repainted and the highlight was not.
#[test]
fn a_selection_move_in_the_task_is_painted() {
    let html = "<!doctype html><style>body{margin:0}#st{width:20px;height:20px;background:#08f}</style>\
                <input id=f value=\"hello world\"><div id=st></div>\
                <script>var f = document.getElementById('f'), st = document.getElementById('st'), n = 0; f.focus(); \
                requestAnimationFrame(function g() { n++; if (window.go === 1) { window.go = 2; f.setSelectionRange(0, 3); } \
                st.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(g); });</script>";
    assert!(cross_check_go(html, 6, "String(f.selectionStart) + ',' + f.selectionEnd", 3) >= 2);
}

/// R1 follow-up: a paint-only CSS animation (a spinner) beside script paint writes on every other
/// frame. The spinner-only frames take the paint-only animation frame (`animation_frames_painted`),
/// and the script frames must still take the short path: the animation frame must not let the kept
/// tree go. When it did, every script frame missed ("no kept tree") and took a full build and a
/// copy of the tree.
#[test]
fn a_spinner_beside_script_paint_writes_keeps_the_short_path() {
    let html = "<!doctype html><style>body{margin:0}@keyframes spin{to{transform:rotate(360deg)}}\
                #sp{width:20px;height:20px;background:#333;animation:spin 1s linear infinite}\
                .t{width:20px;height:20px;background:#08f}</style><div id=sp></div><div class=t id=w></div>\
                <script>var w = document.getElementById('w'), n = 0; requestAnimationFrame(function f() { n++; \
                if (n % 2 === 0) w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    let (mut engine, view, _server) = load(html);
    for _ in 0..4 {
        frame(&mut engine, view);
    }
    let before = engine.relayout_stats(view).expect("view").clone();
    for _ in 0..10 {
        frame(&mut engine, view);
    }
    let after = engine.relayout_stats(view).expect("view");
    let hits = after.script_restyles - before.script_restyles;
    let painted = after.animation_frames_painted - before.animation_frames_painted;
    assert!(painted >= 4, "the spinner-only frames took the animation frame {painted} times of 5");
    assert!(hits >= 4, "the script frames took the short path {hits} times of 5 (last miss {:?})", last_restyle_miss_for_test());
}

/// R1 behaviour checks for LPF2, each through the cross-check with the path on and off: a written
/// `translate(-50%, -50%) rotate()` (the element's rect follows it).
#[test]
fn a_centred_rotation_write_paints_and_measures_as_the_full_rebuild() {
    let html = "<!doctype html><style>body{margin:0}#w{position:absolute;left:200px;top:200px;width:80px;height:40px;background:#08f;\
                transform:translate(-50%,-50%) rotate(0deg)}</style><div id=w></div>\
                <script>var w = document.getElementById('w'), n = 0; requestAnimationFrame(function f() { n++; \
                w.style.transform = 'translate(-50%, -50%) rotate(' + (n * 17 % 360) + 'deg)'; requestAnimationFrame(f); });</script>";
    assert!(cross_check(html, 6, "JSON.stringify((function (b) { return [b.left, b.top, b.width, b.height]; })(w.getBoundingClientRect()))") >= 4);
}

/// A fixed child under a box whose transform script writes (transform to transform): the child is
/// placed against the transformed box, as in the full rebuild.
#[test]
fn a_fixed_child_under_a_written_transform_is_placed_as_the_full_rebuild() {
    let html = "<!doctype html><style>body{margin:0}#p{position:absolute;left:100px;top:100px;width:60px;height:60px;background:#ccc;transform:translateX(0px)}\
                #fx{position:fixed;left:5px;top:5px;width:10px;height:10px;background:#f00}</style><div id=p><div id=fx></div></div>\
                <script>var p = document.getElementById('p'), fx = document.getElementById('fx'), n = 0; requestAnimationFrame(function f() { n++; \
                p.style.transform = 'translateX(' + (n * 7 % 200) + 'px)'; requestAnimationFrame(f); });</script>";
    assert!(cross_check(html, 6, "JSON.stringify((function (b) { return [b.left, b.top]; })(fx.getBoundingClientRect()))") >= 4);
}

/// Hit testing after a transform write: `elementFromPoint` finds the moved box where the full
/// rebuild's hit test does.
#[test]
fn hit_testing_after_a_transform_write_is_the_full_rebuilds() {
    let html = "<!doctype html><style>body{margin:0}#w{position:absolute;left:0;top:0;width:50px;height:50px;background:#08f;transform:translateX(0px)}</style>\
                <div id=w></div><script>var w = document.getElementById('w'), n = 0; requestAnimationFrame(function f() { n++; \
                w.style.transform = 'translateX(' + (n * 30 % 300) + 'px)'; requestAnimationFrame(f); });</script>";
    assert!(cross_check(html, 6, "[30, 90, 150, 210].map(function (x) { var e = document.elementFromPoint(x, 25); return e ? e.id || e.tagName : 'none'; }).join()") >= 4);
}

/// Opacity reaching 0 (and back) on a box that already had an opacity below 1: still paint (the box
/// keeps its stacking context); computed style and hit testing match the full rebuild's.
#[test]
fn opacity_reaching_zero_paints_and_hits_as_the_full_rebuild() {
    let html = "<!doctype html><style>body{margin:0}#w{width:50px;height:50px;background:#08f;opacity:0.5}</style>\
                <div id=w></div><script>var w = document.getElementById('w'), n = 0; requestAnimationFrame(function f() { n++; \
                w.style.opacity = String([0.5, 0.2, 0, 0, 0.3][n % 5]); requestAnimationFrame(f); });</script>";
    assert!(cross_check(html, 7, "getComputedStyle(w).opacity + ',' + (document.elementFromPoint(25, 25) || {}).id") >= 4);
}

/// LPF3 (MotionMark Multiply on the Mac: the short path took about half the frames; each
/// `a box came or went` miss was followed by a run of full-build `cooling` relayouts and a
/// `no kept tree` miss): `tiles` tiles with colour and transform writes on 3 tiles a frame, and
/// every `every` frames `change` runs (a tile hidden or shown, appended or removed). Returns the
/// relayouts of the `count` frames after the path settles and how many of them took the short
/// path (relayouts, not frames: a slow load runs the page's frames ahead, and the first pumped
/// frames then write nothing), the path's misses by name, and checks every frame paints as the
/// full rebuild does.
fn tiles_that_come_and_go(tiles: usize, every: usize, change: &str, count: usize) -> (u32, u32, Vec<String>) {
    let write = "el.style.backgroundColor = 'hsl(' + (n * 37 % 360) + ',60%,50%)'; \
                 el.style.transform = 'rotate(' + (n * 7 % 360) + 'deg)';";
    let mut html = page(tiles, write);
    html = html.replace(
        "requestAnimationFrame(f); });</script>",
        &format!("if (n % {every} === 0) {{ {change} }} requestAnimationFrame(f); }});</script>"),
    );
    // The path-on arm warms up until the path has settled after the load (a short-path frame with
    // no cooldown left: the load runs the page's frames ahead with few relayouts between, and its
    // misses count as in a row); the path-off arm runs as many frames, so the arms stay aligned.
    let settle = std::cell::Cell::new(0usize);
    let run = |on: bool| {
        set_script_restyle_for_test(on);
        let (mut engine, view, _server) = load(&html);
        if on {
            settle.set(settle_path(&mut engine, view));
        } else {
            for _ in 0..settle.get() {
                frame(&mut engine, view);
            }
        }
        let stats = engine.relayout_stats(view).expect("view").clone();
        let (before, relaid_before) = (stats.script_restyles, stats.relayouts);
        let (mut lists, mut misses) = (Vec::new(), Vec::new());
        for _ in 0..count {
            frame(&mut engine, view);
            lists.push(format!("{:?}", engine.views[&view].display_list.as_ref().expect("display list").commands));
            let miss = last_restyle_miss_for_test();
            if misses.last() != Some(&miss) {
                misses.push(miss);
            }
        }
        set_script_restyle_for_test(true);
        let stats = engine.relayout_stats(view).expect("view");
        (lists, stats.script_restyles - before, stats.relayouts - relaid_before, misses)
    };
    let ((short, took, relaid, misses), (full, none, _, _)) = (run(true), run(false));
    assert_eq!(none, 0, "the path was off in the other arm");
    for (i, (a, b)) in short.iter().zip(&full).enumerate() {
        assert!(a == b, "frame {i}: the display list differs from the full rebuild's (short path ran {took} times)");
    }
    eprintln!(
        "LPF3 {change}: settled after {} frames; short path {took} of {relaid} relayouts in {count} frames, misses {misses:?}",
        settle.get()
    );
    (took, relaid, misses)
}

/// The relayouts that took the full build, of `relaid`, when one in `every` frames changes the
/// tree: no more than those frames (and the first, which may come before a kept tree).
fn assert_one_full_build_per_change(took: u32, relaid: u32, every: u32, what: &str) {
    assert!(relaid >= 15, "only {relaid} relayouts ran: the page wrote too little to judge");
    let full = relaid - took;
    assert!(
        full <= relaid / every + 2,
        "{full} of {relaid} relayouts took the full build with {what} one frame in {every}"
    );
}

/// A written tile hidden and shown again every 10 frames (`display`): the frame that hides or shows
/// it is a full build (a box came or went), and the next frame's paint-only writes take the short
/// path again: the full build re-establishes the kept tree, and nothing else needs sitting out.
#[test]
fn a_written_box_that_comes_or_goes_costs_one_full_build() {
    let change = "var h = tiles[(n * 7) % tiles.length]; h.style.display = h.style.display === 'none' ? '' : 'none';";
    let (took, relaid, _) = tiles_that_come_and_go(300, 10, change, 60);
    assert_one_full_build_per_change(took, relaid, 10, "a box coming or going");
}

/// A tile appended, then removed, every 10 frames (the tile list does not change, so the writes
/// stay on the old tiles): only the frame that changes the tree is a full build.
#[test]
fn a_tile_appended_or_removed_costs_one_full_build() {
    let change = "var s = document.getElementById('stage'); if (window.extra) { extra.remove(); window.extra = null; } \
                  else { window.extra = document.createElement('div'); extra.className = 't'; s.appendChild(extra); }";
    let (took, relaid, _) = tiles_that_come_and_go(300, 10, change, 60);
    assert_one_full_build_per_change(took, relaid, 10, "a tile appended or removed");
}

/// Pumps frames until the path has settled after the load: a short-path frame with no cooldown left
/// (the load runs the page's frames ahead, with few relayouts between, and its misses count as in
/// a row). Returns the frames pumped.
fn settle_path(engine: &mut Engine, view: EngineViewId) -> usize {
    for i in 0..100 {
        let hits = engine.relayout_stats(view).expect("view").script_restyles;
        frame(engine, view);
        if engine.relayout_stats(view).expect("view").script_restyles > hits
            && engine.views[&view].lay.script_cooldown.get() == 0
        {
            return i + 1;
        }
    }
    panic!("the short path did not settle in 100 frames");
}

/// What the script restyle path did over some frames.
struct Counted {
    /// Relayouts in the frames.
    relaid: u32,
    /// Relayouts that took the short path.
    hits: u32,
    /// The frames (from 0) in which the path was attempted: a hit or a miss.
    attempts: Vec<usize>,
}

fn count_frames(engine: &mut Engine, view: EngineViewId, frames: usize) -> Counted {
    let start = engine.relayout_stats(view).expect("view").clone();
    let mut attempts = Vec::new();
    for i in 0..frames {
        let hits = engine.relayout_stats(view).expect("view").script_restyles;
        crate::LAST_RESTYLE_MISS.with(|m| m.borrow_mut().clear());
        frame(engine, view);
        if engine.relayout_stats(view).expect("view").script_restyles > hits || !last_restyle_miss_for_test().is_empty() {
            attempts.push(i);
        }
    }
    let end = engine.relayout_stats(view).expect("view");
    Counted {
        relaid: end.relayouts - start.relayouts,
        hits: end.script_restyles - start.script_restyles,
        attempts,
    }
}

/// A colour write every frame, and after `go` a width write every frame (a miss each time the path
/// is attempted).
const WIDTH_EVERY_FRAME: &str = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f}</style>\
                <div class=t id=w></div><div class=t></div><script>var w = document.getElementById('w'), n = 0; \
                requestAnimationFrame(function f() { n++; if (window.go === 1) w.style.width = (20 + n % 30) + 'px'; \
                w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";

/// Asserts misses in a row taper to the cap over `counted`'s 200 frames of one relayout each.
fn assert_attempts_taper(counted: &Counted, what: &str) {
    assert!(counted.relaid >= 190, "only {} relayouts in 200 frames", counted.relaid);
    let late = counted.attempts.iter().filter(|&&i| i >= 100).count();
    // Misses in a row sit out 0, 1, 2, 4, 8, 16, 32 relayouts, then the cap of 60 each time: with
    // one relayout a frame, attempts at frames 0, 1, 3, 6, 11, 20, 37, 70, 131 and 192 (ten). From
    // frame 100 the cap holds: at most 2 attempts in 100 frames (one per 61). A refused relayout
    // may push an attempt one frame later, which changes neither count.
    assert!(
        late <= 2 && counted.attempts.len() <= 11,
        "{what}: {} attempts in 200 frames, {late} of them after frame 100: {:?}",
        counted.attempts.len(),
        counted.attempts
    );
}

/// LPF3: a page whose writes change layout every frame still sits out relayouts: misses in a row
/// back off (1, 2, 4, ... up to the cap, and stay at it), so it does not pay a short-path attempt
/// and a kept tree on top of a full build each frame. Painted as the full rebuild throughout.
#[test]
fn misses_in_a_row_back_off() {
    cross_check_go(WIDTH_EVERY_FRAME, 70, "", 5);
    let (mut engine, view, _server) = load(WIDTH_EVERY_FRAME);
    settle_path(&mut engine, view);
    engine.execute_script(view, "window.go = 1").unwrap();
    let counted = count_frames(&mut engine, view, 200);
    assert_attempts_taper(&counted, "a width write every frame");
}

/// LPF3 R1: a relayout the path refuses (here an element scroll in the task) between two misses
/// does not end the run of misses: a scroll handler alternating with a width write still backs
/// off, instead of paying an attempt and a kept tree every other frame.
#[test]
fn a_refused_relayout_between_misses_keeps_the_backoff() {
    let html = "<!doctype html><style>body{margin:0}#sc{width:200px;height:100px;overflow:auto}\
                #in{width:4000px;height:50px;background:linear-gradient(90deg,#000,#fff)}\
                .t{width:20px;height:20px;background:#08f}</style><div id=sc><div id=in></div></div>\
                <div class=t id=w></div><div class=t></div>\
                <script>var sc = document.getElementById('sc'), w = document.getElementById('w'), n = 0; \
                requestAnimationFrame(function f() { n++; if (window.go === 1) { if (n % 2) sc.scrollLeft += 7; \
                else w.style.width = (20 + n % 30) + 'px'; } \
                w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check_go(html, 70, "String(sc.scrollLeft)", 5);
    let (mut engine, view, _server) = load(html);
    settle_path(&mut engine, view);
    engine.execute_script(view, "window.go = 1").unwrap();
    let counted = count_frames(&mut engine, view, 200);
    assert_attempts_taper(&counted, "a scroll and a width write in turn");
}

/// LPF3 R1: a hit and a miss in turn, every frame: each miss is a one-off (a hit came between), so
/// none is sat out, and every frame with only a colour write takes the short path. One full build
/// per width write, no more.
#[test]
fn a_hit_and_a_miss_in_turn_pay_only_the_misses() {
    let html = "<!doctype html><style>body{margin:0}.t{width:20px;height:20px;background:#08f}</style>\
                <div class=t id=w></div><div class=t></div><script>var w = document.getElementById('w'), n = 0; \
                requestAnimationFrame(function f() { n++; \
                if (window.go === 1 && n % 2 === 0) w.style.width = w.style.width === '30px' ? '20px' : '30px'; \
                w.style.backgroundColor = 'hsl(' + (n * 40 % 360) + ',60%,50%)'; requestAnimationFrame(f); });</script>";
    cross_check_go(html, 40, "", 5);
    let (mut engine, view, _server) = load(html);
    settle_path(&mut engine, view);
    engine.execute_script(view, "window.go = 1").unwrap();
    let counted = count_frames(&mut engine, view, 100);
    assert!(counted.relaid >= 95, "only {} relayouts in 100 frames", counted.relaid);
    let full = counted.relaid - counted.hits;
    assert!(
        full <= counted.relaid.div_ceil(2) + 1,
        "{full} of {} relayouts took the full build with a width write every other frame",
        counted.relaid
    );
}

/// LPF3 R1: a navigation ends the old page's run of misses: the new page does not inherit its
/// cooldown.
#[test]
fn a_navigation_ends_the_backoff() {
    let (mut engine, view, _server) = load(WIDTH_EVERY_FRAME);
    settle_path(&mut engine, view);
    engine.execute_script(view, "window.go = 1").unwrap();
    count_frames(&mut engine, view, 40);
    let left = engine.views[&view].lay.script_cooldown.get();
    assert!(left > 0, "the old page's misses in a row left no cooldown to carry over");
    engine.load_html(view, "<!doctype html><p>next</p>").unwrap();
    assert_eq!(engine.views[&view].lay.script_cooldown.get(), 0, "the new page inherited {left} cooling relayouts");
}
