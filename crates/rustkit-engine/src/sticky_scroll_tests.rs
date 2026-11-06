//! Plan row C2 (d): `position: sticky` sticks to its nearest scroller, or to
//! the window when it has none. Until this the engine laid sticky out as
//! static and nothing ever moved it. Expected strings are Chromium
//! 143.0.7499.4 on the same pages (`trench/z/tools/c2/c2d_probe.mjs`,
//! `c2d_probe2.mjs` and `c2d_probe3.mjs` on the hub, where the 400px spacers
//! are a `margin-top` on the next box; here that margin would collapse
//! through `body` and move the absolute boxes, which an engine bug places
//! from their static parent (PR #850's body), and nothing else changes).

use super::*;
use crate::script_net_tests::serve_routes;

/// One scroller per rule, side by side, and two boxes in the window's flow.
const PAGE: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}",
    ".sc{overflow:auto;width:200px;height:100px;position:absolute;top:0}",
    ".tall{height:600px}.gap{height:50px}.st{position:sticky;height:20px;width:50px}",
    "#a-s{left:0}#a{top:0}",
    "#b-s{left:210px}#b{top:10px;margin-bottom:5px}",
    "#c-s{left:420px}#c{bottom:0}",
    "#d-s{left:630px}#d{top:10%}",
    "#e-s{left:840px}#e-cb{height:100px;margin-top:50px}#e{top:0;margin-bottom:6px}",
    "#f-s{left:1050px}#f{top:5px;bottom:5px;height:200px}",
    "#g-s{left:1260px;height:100px;width:100px}#g-wide{width:800px;height:20px}#g{left:7px;top:auto}",
    "#h-s{left:1470px}#h{top:0}#h-abs{position:absolute;left:3px;top:4px;width:5px;height:5px}#h-kid{height:5px}",
    "#i-out{left:1680px}#i-in{height:50px;overflow:hidden;margin-top:40px}#i{top:0}",
    "#j-s{left:1890px;overflow:clip}#j{top:0}",
    "#sp{height:400px}#k{position:sticky;top:30px;height:20px;width:40px;margin-left:2100px}",
    "#k-kid{height:5px;width:5px}",
    "#l-cb{margin-left:2200px;height:300px;margin-top:100px}",
    "#l{position:sticky;top:15px;height:10px;width:10px}",
    "#m-s{left:2400px}#m{top:0;right:0;width:10px}",
    "#o-s{left:2600px}#o1{top:0;height:60px;width:60px}#o2{top:5px;height:5px;width:5px}#o-gap{height:30px}",
    "</style><body>",
    "<div class=sc id=a-s><div class=gap></div><div class=st id=a></div><div class=tall></div></div>",
    "<div class=sc id=b-s><div class=gap></div><div class=st id=b></div><div class=tall></div></div>",
    "<div class=sc id=c-s><div class=tall></div><div class=st id=c></div><div class=gap></div></div>",
    "<div class=sc id=d-s><div class=gap></div><div class=st id=d></div><div class=tall></div></div>",
    "<div class=sc id=e-s><div id=e-cb><div class=st id=e></div></div><div class=tall></div></div>",
    "<div class=sc id=f-s><div class=gap></div><div class=st id=f></div><div class=tall></div></div>",
    "<div class=sc id=g-s><div id=g-wide><div class=st id=g></div></div></div>",
    "<div class=sc id=h-s><div class=gap></div><div class=st id=h><div id=h-kid></div><div id=h-abs></div></div><div class=tall></div></div>",
    "<div class=sc id=i-out><div id=i-in><div class=st id=i></div><div class=tall></div></div><div class=tall></div></div>",
    "<div class=sc id=j-s><div class=gap></div><div class=st id=j></div><div class=tall></div></div>",
    "<div class=sc id=m-s><div class=gap></div><div class=st id=m></div><div class=tall></div></div>",
    "<div class=sc id=o-s><div class=gap></div><div class=st id=o1><div id=o-gap></div><div class=st id=o2></div></div><div class=tall></div></div>",
    "<div id=sp></div><div id=k><div id=k-kid></div></div>",
    "<div id=l-cb><div id=l></div></div>",
    "</body>",
);

/// Padding on the scroller and on the containing block, a sticky box under a
/// fixed box, in a transform, and holding a fixed box.
const PAGE2: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}",
    ".sc{overflow:auto;width:200px;height:100px;position:absolute;top:0}",
    ".tall{height:600px}.gap{height:50px}.st{position:sticky;top:0;height:20px;width:50px}",
    "#a-s{left:0;padding:11px;border:3px solid #000}",
    "#b-s{left:300px}#b-cb{padding:9px 0 13px;height:100px;margin-top:50px}",
    "#fx{position:fixed;left:600px;top:40px;width:100px;height:100px}#c{top:60px}",
    "#sp{height:400px}#e-tr{transform:translateY(100px);margin-left:1100px;height:400px}#e{top:10px}",
    "</style><body>",
    "<div class=sc id=a-s><div class=gap></div><div class=st id=a></div><div class=tall></div></div>",
    "<div class=sc id=b-s><div id=b-cb><div class=st id=b></div></div><div class=tall></div></div>",
    "<div id=fx><div class=st id=c></div></div>",
    "<div id=sp></div><div id=e-tr><div class=st id=e></div></div>",
    "</body>",
);

/// Everything in a window's reach, for the hit tests.
const PAGE3: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}",
    "#s{overflow:auto;width:200px;height:100px;position:absolute;left:0;top:0}",
    "#gap{height:50px}#st{position:sticky;top:0;height:20px;width:50px}#fill{height:600px}",
    "#sp{height:400px}#w{position:sticky;top:30px;height:20px;width:40px;margin-left:300px}",
    "#wk{height:5px;width:5px}",
    "#ws{position:sticky;top:60px;height:20px;width:40px;margin-left:400px}",
    "#wf{position:fixed;left:500px;top:7px;width:20px;height:20px}",
    "#ds{position:absolute;left:600px;top:0;width:100px;height:100px}",
    "#dst{overflow:auto;width:100px;height:100px}",
    "#dab{position:absolute;left:0;top:500px;width:60px;height:60px}",
    "#d{position:sticky;top:10px;height:5px;width:5px}",
    "#dtall{height:600px}",
    "</style><body>",
    "<div id=s><div id=gap></div><div id=st></div><div id=fill></div></div>",
    "<div id=ds><div id=dst><div id=dab><div id=d></div></div><div id=dtall></div></div></div>",
    "<div id=sp></div><div id=w><div id=wk></div></div>",
    "<div id=ws><div id=wf></div></div>",
    "</body>",
);

const AT: &str = "var $ = function (i) { return document.getElementById(i); }; \
     var at = function (i) { var r = $(i).getBoundingClientRect(); return r.x + ',' + r.y; }; \
     var top = function (i) { return at(i) + ' ' + $(i).offsetTop; }; \
     var hit = function (x, y) { var e = document.elementFromPoint(x, y); return e && e.id || '-'; }; ";

#[track_caller]
fn check(page: &str, script: &str, want: &str) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine.execute_script(view, &format!("{AT}{script}")).expect("script");
    assert_eq!(got, format!("String(\"{want}\")"));
}

/// `scroller.scrollTop` at each offset, then `read` there.
fn walk(scroller: &str, offsets: &[u32], read: &str) -> String {
    let steps: Vec<String> = offsets
        .iter()
        .map(|o| format!("(($('{scroller}').scrollTop = {o}), {read})"))
        .collect();
    format!("[{}].join(' | ')", steps.join(", "))
}

#[test]
fn it_follows_its_scroller_then_sticks_at_the_edge() {
    check(PAGE, &walk("a-s", &[0, 30, 50, 70, 200], "top('a')"), "0,50 50 | 0,20 50 | 0,0 50 | 0,0 70 | 0,0 200");
}

#[test]
fn the_inset_is_kept_from_the_edge() {
    check(PAGE, &walk("b-s", &[0, 30, 45, 70], "top('b')"), "210,50 50 | 210,20 50 | 210,10 55 | 210,10 80");
}

#[test]
fn a_bottom_inset_holds_it_up_until_its_place_comes() {
    check(PAGE, &walk("c-s", &[0, 400, 545, 570], "top('c')"), "420,80 80 | 420,80 480 | 420,55 600 | 420,30 600");
}

#[test]
fn a_percent_inset_is_of_the_scrollport() {
    check(PAGE, &walk("d-s", &[0, 45, 100], "top('d')"), "630,50 50 | 630,10 55 | 630,10 110");
}

#[test]
fn it_stays_in_its_containing_block_and_keeps_its_margin() {
    check(
        PAGE,
        &walk("e-s", &[0, 50, 100, 124, 130, 160, 200], "top('e')"),
        "840,50 50 | 840,0 50 | 840,0 100 | 840,0 124 | 840,-6 124 | 840,-36 124 | 840,-76 124",
    );
}

#[test]
fn a_box_taller_than_the_scrollport_keeps_its_top() {
    check(PAGE, &walk("f-s", &[0, 50, 200, 400], "top('f')"), "1050,50 50 | 1050,5 55 | 1050,5 205 | 1050,5 405");
}

/// With both insets, the far one holds a box that fits and is dropped for one
/// that does not (`c2d_probe4.mjs`).
#[test]
fn both_insets_apply_only_to_a_box_that_fits_between_them() {
    let page = |height: u32, before: u32, after: u32| {
        format!(
            "<!doctype html><style>body{{margin:0}}#s{{overflow:auto;width:200px;height:100px}}\
             #t{{position:sticky;width:50px;top:5px;bottom:5px;height:{height}px}}</style><body>\
             <div id=s><div style='height:{before}px'></div><div id=t></div><div style='height:{after}px'></div></div></body>"
        )
    };
    let read = walk("s", &[0, 20, 50, 100, 280, 400], "at('t')");
    check(&page(20, 300, 300), &read, "0,75 | 0,75 | 0,75 | 0,75 | 0,20 | 0,5");
    check(&page(95, 50, 600), &read, "0,50 | 0,30 | 0,5 | 0,5 | 0,5 | 0,5");
    check(&page(90, 50, 600), &read, "0,5 | 0,5 | 0,5 | 0,5 | 0,5 | 0,5");
    check(&page(95, 300, 300), &read, "0,300 | 0,280 | 0,250 | 0,200 | 0,20 | 0,5");
}

#[test]
fn it_sticks_across_too() {
    let steps: Vec<String> = [0, 30, 100, 700]
        .iter()
        .map(|o| format!("(($('g-s').scrollLeft = {o}), at('g'))"))
        .collect();
    check(PAGE, &format!("[{}].join(' ')", steps.join(", ")), "1267,0 1267,0 1267,0 1267,0");
}

#[test]
fn what_is_inside_goes_with_it() {
    check(
        PAGE,
        &walk("h-s", &[0, 100], "[top('h'), top('h-kid'), top('h-abs')].join(' ')"),
        "1470,50 50 1470,50 0 1473,54 4 | 1470,0 100 1470,0 0 1473,4 4",
    );
}

#[test]
fn the_nearest_scroller_is_the_one_that_counts_even_when_hidden() {
    check(
        PAGE,
        &walk("i-out", &[0, 40, 60, 80], "at('i')"),
        "1680,40 | 1680,0 | 1680,-20 | 1680,-40",
    );
    check(PAGE, "$('i-in').scrollTop = 30; top('i')", "1680,40 70");
}

#[test]
fn clip_is_not_a_scroller() {
    check(PAGE, "$('j-s').scrollTop = 100; top('j')", "1890,50 50");
}

#[test]
fn a_right_inset_with_no_room_changes_nothing() {
    check(PAGE, &walk("m-s", &[0, 100], "top('m')"), "2400,50 50 | 2400,0 100");
}

#[test]
fn a_sticky_box_in_a_sticky_box() {
    check(
        PAGE,
        &walk("o-s", &[0, 50, 100], "[at('o1'), top('o2')].join(' ')"),
        "2600,50 2600,80 30 | 2600,0 2600,30 30 | 2600,0 2600,30 30",
    );
}

#[test]
fn it_sticks_to_the_window_with_no_scroller() {
    let steps: Vec<String> = [0, 100, 380, 500, 1000]
        .iter()
        .map(|y| format!("(window.scrollTo(0, {y}), [top('k'), at('k-kid'), top('l')].join(' '))"))
        .collect();
    check(
        PAGE,
        &format!("[{}].join(' | ')", steps.join(", ")),
        "2100,400 400 2100,400 2200,520 520 | 2100,300 400 2100,300 2200,420 520 | \
         2100,30 410 2100,30 2200,140 520 | 2100,30 530 2100,30 2200,20 520 | \
         2100,30 1030 2100,30 2200,-190 810",
    );
}

#[test]
fn the_scrollers_padding_narrows_the_scrollport() {
    check(PAGE2, &walk("a-s", &[0, 30, 45, 100], "at('a')"), "14,64 | 14,34 | 14,19 | 14,14");
}

#[test]
fn the_containing_block_is_its_content_box() {
    check(PAGE2, &walk("b-s", &[0, 50, 59, 100, 200], "at('b')"), "300,59 | 300,9 | 300,0 | 300,0 | 300,-61");
}

#[test]
fn under_a_fixed_box_the_window_does_not_move_it_but_the_inset_holds() {
    check(
        PAGE2,
        "var a = at('c'); window.scrollTo(0, 700); a + ' ' + at('c')",
        "600,60 600,60",
    );
}

#[test]
fn a_transform_above_moves_it_after_the_rule() {
    let steps: Vec<String> = [0, 300, 450, 700, 1200]
        .iter()
        .map(|y| format!("(window.scrollTo(0, {y}), at('e'))"))
        .collect();
    check(PAGE2, &format!("[{}].join(' ')", steps.join(", ")), "1100,500 1100,200 1100,110 1100,110 1100,-320");
}

#[test]
fn an_absolute_box_takes_its_sticky_child_past_a_static_scroller() {
    check(
        PAGE3,
        "$('dst').scrollTop = 300; var a = at('d'); window.scrollTo(0, 500); var b = at('d'); \
         window.scrollTo(0, 2000); [a, b, at('d')].join(' ')",
        "600,500 600,10 600,-1445",
    );
}

#[test]
fn a_fixed_box_in_a_stuck_box_stays_put() {
    check(PAGE3, "window.scrollTo(0, 500); at('ws') + ' ' + at('wf')", "400,60 500,7");
}

#[test]
fn the_hit_test_finds_it_where_it_is() {
    check(
        PAGE3,
        "var pts = function () { return [hit(5, 5), hit(5, 30), hit(5, 55), hit(320, 40), hit(320, 405), hit(420, 70), hit(505, 10)].join(' '); }; \
         var one = pts(); $('s').scrollTop = 100; var two = pts(); window.scrollTo(0, 500); [one, two, pts()].join(' | ')",
        // `sp` is the spacer, where Chromium's page has `body`.
        "gap gap st sp w sp wf | st fill fill sp w sp wf | - - - w - ws wf",
    );
}

const W: usize = 800;
const H: usize = 600;

fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = ppm.len() - W * H * 3 + (y * W + x) * 3;
    [ppm[at], ppm[at + 1], ppm[at + 2]]
}

const WHITE: [u8; 3] = [255, 255, 255];
const LIME: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];

const PAINT: &str = concat!(
    "<html><head><style>body{margin:0;height:3000px}",
    "#s{overflow:auto;width:200px;height:100px;position:absolute;left:0;top:0}",
    "#gap{height:50px}#st{position:sticky;top:10px;height:20px;width:50px;background:#0f0}#fill{height:600px}",
    "#sp{height:400px}#w{position:sticky;top:30px;height:20px;width:40px;margin-left:300px;background:#00f}",
    "#x{position:sticky;top:0;height:20px;width:40px;margin-left:400px;background:#00f}",
    "#xf{position:fixed;left:500px;top:7px;width:20px;height:20px;background:#0f0}",
    "</style></head><body>",
    "<div id=s><div id=gap></div><div id=st></div><div id=fill></div></div>",
    "<div id=sp></div><div id=w></div><div id=x><div id=xf></div></div>",
    "</body></html>",
);

fn painted() -> (Engine, EngineViewId) {
    let server = serve_routes(vec![("/", "text/html", PAINT.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: W as u32, height: H as u32 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view)
}

/// The view's list as the window draws it at `offset`, checked against the
/// list copied with the same scroll (`scrolled_commands`).
fn at_offset(engine: &mut Engine, view: EngineViewId, offset: (f32, f32), tag: &str) -> Vec<u8> {
    let viewhost_id = engine.views[&view].viewhost_id;
    let list = engine.views[&view].display_list.as_ref().expect("list").commands.clone();
    let copied = std::env::temp_dir().join(format!("rustkit-c2d-{tag}-c-{}.ppm", std::process::id()));
    engine
        .presenter
        .as_mut()
        .expect("presenter")
        .capture_commands(viewhost_id, (W as u32, H as u32), copied.to_str().unwrap(), &scrolled_commands(&list, offset))
        .expect("capture");
    let path = std::env::temp_dir().join(format!("rustkit-c2d-{tag}-{}.ppm", std::process::id()));
    let display_list = engine.views[&view].display_list.as_ref().expect("list");
    engine
        .presenter
        .as_mut()
        .expect("presenter")
        .draw_scrolled(viewhost_id, display_list, offset, Some(path.to_str().unwrap()))
        .expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let reference = std::fs::read(&copied).expect("frame");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(&copied);
    assert!(ppm == reference, "{tag}: the offset draws what the copied list drew");
    ppm
}

#[test]
fn a_box_sticks_in_its_scrollers_paint() {
    let (mut engine, view) = painted();
    engine.execute_script(view, "document.getElementById('s').scrollTop = 200").expect("script");
    engine.render_view(view).expect("render");
    let ppm = at_offset(&mut engine, view, (0.0, 0.0), "box");
    assert_eq!(pixel(&ppm, 20, 15), LIME, "stuck 10 from the box's top");
    assert_eq!(pixel(&ppm, 20, 5), WHITE, "and not above that");
    assert_eq!(pixel(&ppm, 60, 15), WHITE, "only as wide as it is");
}

#[test]
fn a_box_sticks_in_the_windows_paint() {
    let (mut engine, view) = painted();
    let top = at_offset(&mut engine, view, (0.0, 0.0), "top");
    assert_eq!(pixel(&top, 320, 410), BLUE, "where it is laid out");
    assert_eq!(pixel(&top, 320, 40), WHITE);

    let down = at_offset(&mut engine, view, (0.0, 500.0), "down");
    assert_eq!(pixel(&down, 320, 40), BLUE, "held 30 from the window's top");
    assert_eq!(pixel(&down, 320, 60), WHITE);
    assert_eq!(pixel(&down, 420, 10), BLUE, "the next one, held at the top");
    assert_eq!(pixel(&down, 510, 20), LIME, "the fixed box inside it stays put");
}

/// The list is not built again for a window scroll: the rule is in the list.
#[test]
fn a_window_scroll_does_not_build_the_list_again() {
    let (mut engine, view) = painted();
    let list = engine.views[&view].display_list.clone().expect("list");
    engine.set_scroll_offset(view, 0.0, 500.0).expect("scroll");
    engine.render_view(view).expect("render");
    let after = engine.views[&view].display_list.clone().expect("list");
    assert!(Arc::ptr_eq(&list, &after));
}

/// A pointer in the window reaches the same rule as script does.
#[test]
fn after_a_window_scroll_a_click_finds_the_stuck_box() {
    let (mut engine, view) = painted();
    engine.set_scroll_offset(view, 0.0, 500.0).expect("scroll");
    let hit = Engine::hit_at(&engine.views[&view], 320.0, 40.0).expect("hit");
    let rect = (hit.border_box.x, hit.border_box.y, hit.border_box.width);
    assert_eq!(rect, (300.0, 400.0, 40.0), "#w, as laid out");
}

/// Sticky boxes under transforms (`trench/z/tools/c2/c2d_r1_probe.mjs`).
const TRANSFORMS: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}.tall{height:600px}.gap{height:50px}",
    ".st{position:sticky;top:0;height:20px;width:50px}",
    "#a-s{position:absolute;left:0;top:0;width:200px;height:100px;overflow:auto;transform:translateX(10px)}",
    "#b-s{position:absolute;left:300px;top:0;width:200px;height:100px;overflow:auto}#b{transform:scale(2)}",
    "#d-s{position:absolute;left:0;top:200px;width:200px;height:100px;overflow:auto;transform:scale(2);transform-origin:0 0}",
    "#sp{height:400px}#c-tr{transform:translateY(50px);margin-left:600px;height:1000px}",
    "</style><body>",
    "<div id=a-s><div class=gap></div><div class=st id=a></div><div class=tall></div></div>",
    "<div id=b-s><div class=gap></div><div class=st id=b></div><div class=tall></div></div>",
    "<div id=d-s><div class=gap></div><div class=st id=d></div><div class=tall></div></div>",
    "<div id=sp></div><div id=c-tr><div class=st id=c></div></div></body>",
);

#[test]
fn it_sticks_in_a_transformed_scroller() {
    check(TRANSFORMS, "$('a-s').scrollTop = 100; at('a')", "10,0");
}

#[test]
fn its_own_transform_goes_with_it() {
    check(TRANSFORMS, "$('b-s').scrollTop = 100; at('b')", "275,-10");
}

#[test]
fn it_sticks_in_a_scaled_scroller() {
    check(TRANSFORMS, "$('d-s').scrollTop = 100; at('d')", "0,200");
}

#[test]
fn it_sticks_to_the_window_under_a_transform() {
    check(
        TRANSFORMS,
        "[0, 300, 450, 700, 1200].map(function (y) { window.scrollTo(0, y); return at('c'); }).join(' ')",
        "600,450 600,150 600,50 600,50 600,50",
    );
}

/// A sticky box under a fixed box takes nothing from a sticky box above that.
const IN_FIXED_IN_STICKY: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}",
    "#sp{height:400px}#ws{position:sticky;top:50px;width:300px;height:200px;margin-left:100px}",
    "#wf{position:fixed;left:400px;top:0;width:200px;height:200px}",
    "#wfs{position:sticky;top:10px;width:100px;height:50px}",
    "</style><body><div id=sp></div><div id=ws><div id=wf><div id=wfs></div></div></div></body>",
);

#[test]
fn a_fixed_box_cuts_off_the_sticky_boxes_above_it() {
    check(
        IN_FIXED_IN_STICKY,
        "[0, 500, 1000].map(function (y) { window.scrollTo(0, y); return at('wfs') + ':' + hit(410, 55) + ':' + hit(410, 5); }).join(' ')",
        "400,10:wfs:wf 400,10:wfs:wf 400,10:wfs:wf",
    );
}

const BAR_STYLE: &str = "<!doctype html><style>body{margin:0;height:3000px}\
    #bar{position:fixed;left:0;top:0;width:400px;height:40px}\
    #sp{height:400px}#st{position:sticky;top:0;width:300px;height:30px}";

#[track_caller]
fn over_a_bar(extra_style: &str, body: &str, script: &str, want: &str) {
    check(&format!("{BAR_STYLE}{extra_style}</style><body>{body}</body>"), script, want);
}

const BAR_FIRST: &str = "<div id=bar></div><div id=sp></div><div id=st></div>";
const BAR_LAST: &str = "<div id=sp></div><div id=st></div><div id=bar></div>";

#[test]
fn a_stuck_box_later_in_the_tree_is_over_a_fixed_bar() {
    over_a_bar("", BAR_FIRST, "window.scrollTo(0, 500); hit(10, 10) + ' ' + hit(350, 10)", "st bar");
}

#[test]
fn a_higher_z_index_on_the_bar_keeps_it_on_top() {
    over_a_bar("#bar{z-index:5}", BAR_FIRST, "window.scrollTo(0, 500); hit(10, 10)", "bar");
}

#[test]
fn a_higher_z_index_on_the_stuck_box_puts_it_on_top_of_a_later_bar() {
    over_a_bar("#st{z-index:5}", BAR_LAST, "window.scrollTo(0, 500); hit(10, 10)", "st");
}

#[test]
fn a_bar_later_in_the_tree_is_over_a_stuck_box() {
    over_a_bar("", BAR_LAST, "window.scrollTo(0, 500); hit(10, 10)", "bar");
}

#[test]
fn a_child_of_the_stuck_box_is_hit_over_the_bar() {
    over_a_bar(
        "",
        "<div id=bar></div><div id=sp></div><div id=st><div id=kid style=\"width:50px;height:10px\"></div></div>",
        "window.scrollTo(0, 500); hit(10, 5) + ' ' + hit(10, 20)",
        "kid st",
    );
}

#[test]
fn a_sticky_box_not_yet_at_the_bar_changes_nothing() {
    over_a_bar("", BAR_FIRST, "hit(10, 10)", "bar");
}

/// A stuck header (`h`, 300 wide) under a fixed box (`m`, 400 wide): the
/// stacking contexts on each path decide, from where the paths part
/// (`trench/z/tools/c2/sticky_ctx_probe.mjs`, Chromium 143 at scroll 500).
fn ctx_header(z: i32) -> String {
    format!("<div style=\"height:400px\"></div><div id=h style=\"position:sticky;top:0;width:300px;height:30px;z-index:{z}\"></div>")
}

fn ctx_wrap(style: &str, inner: &str) -> String {
    format!("<div style=\"height:2000px;{style}\">{inner}</div>")
}

fn ctx_modal(z: i32) -> String {
    format!("<div id=m style=\"position:fixed;left:0;top:0;width:400px;height:40px;z-index:{z}\"></div>")
}

fn ctx_page(body: &str) -> String {
    format!("<!doctype html><style>body{{margin:0;height:3000px}}</style>{body}<div style=\"height:3000px\"></div>")
}

fn ctx_hit(body: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, &ctx_page(body)).expect("load");
    let got = engine
        .execute_script(view, &format!("{AT} window.scrollTo(0, 500); hit(10, 10)"))
        .expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

#[track_caller]
fn check_ctx(cases: &[(&str, String, &str)]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(name, body, want)| {
            let got = ctx_hit(body);
            (got != *want).then(|| format!("{name}: {got}, Chromium {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

#[test]
fn a_z_index_wrapper_places_the_stuck_box_against_a_fixed_box() {
    let rel = |z: i32| format!("position:relative;z-index:{z}");
    check_ctx(&[
        ("no wrapper", ctx_wrap("", &ctx_header(10)) + &ctx_modal(5), "h"),
        ("z0, modal after", ctx_wrap(&rel(0), &ctx_header(10)) + &ctx_modal(5), "m"),
        ("z0, modal before", ctx_modal(5) + &ctx_wrap(&rel(0), &ctx_header(10)), "m"),
        ("z1, 100 over 50", ctx_wrap(&rel(1), &ctx_header(100)) + &ctx_modal(50), "m"),
        ("z60, 1 under 50", ctx_wrap(&rel(60), &ctx_header(1)) + &ctx_modal(50), "h"),
        ("equal z, modal after", ctx_wrap(&rel(5), &ctx_header(10)) + &ctx_modal(5), "m"),
        ("equal z, modal before", ctx_modal(5) + &ctx_wrap(&rel(5), &ctx_header(10)), "h"),
        ("relative, z auto", ctx_wrap("position:relative", &ctx_header(10)) + &ctx_modal(5), "h"),
        ("static with a z-index", ctx_wrap("z-index:0", &ctx_header(10)) + &ctx_modal(5), "h"),
        (
            "absolute z0",
            ctx_wrap("position:absolute;left:0;top:0;width:100%;z-index:0", &ctx_header(10)) + &ctx_modal(5),
            "m",
        ),
        ("negative wrapper", ctx_wrap(&rel(-1), &ctx_header(10)) + &ctx_modal(0), "m"),
    ]);
}

#[test]
fn the_comparison_starts_where_the_paths_part() {
    let rel = |z: i32| format!("position:relative;z-index:{z}");
    let held = |z: i32, m: i32| format!("<div style=\"{}\">{}</div>", rel(z), ctx_modal(m));
    check_ctx(&[
        ("outer 20, inner 1", ctx_wrap(&rel(20), &ctx_wrap(&rel(1), &ctx_header(1))) + &ctx_modal(5), "h"),
        ("outer 1, inner 20", ctx_wrap(&rel(1), &ctx_wrap(&rel(20), &ctx_header(10))) + &ctx_modal(5), "m"),
        ("shared, 10 over 5", ctx_wrap(&rel(0), &(ctx_header(10) + &ctx_modal(5))), "h"),
        ("shared, 1 under 5", ctx_wrap(&rel(0), &(ctx_header(1) + &ctx_modal(5))), "m"),
        ("modal in 20, app 10", ctx_wrap(&rel(10), &ctx_header(1)) + &held(20, 1), "m"),
        ("modal in 1, app 10", ctx_wrap(&rel(10), &ctx_header(1)) + &held(1, 50), "h"),
    ]);
}

#[test]
fn a_flex_item_with_a_z_index_is_a_stacking_context() {
    let flex = |item: &str| format!("<div style=\"display:flex\">{}</div>", ctx_wrap(item, &ctx_header(10)));
    check_ctx(&[
        ("z0", flex("flex:1;z-index:0") + &ctx_modal(5), "m"),
        ("z auto", flex("flex:1") + &ctx_modal(5), "h"),
    ]);
}

#[test]
fn what_makes_a_stacking_context_without_a_z_index() {
    let cases: Vec<(&str, String, &str)> = [
        ("transform:translateX(0)", "m"),
        ("translate:0px", "m"),
        ("scale:1", "m"),
        ("rotate:0deg", "m"),
        ("opacity:.9", "m"),
        ("filter:blur(0px)", "m"),
        ("backdrop-filter:blur(0px)", "m"),
        ("clip-path:inset(0)", "m"),
        ("mask-image:linear-gradient(#000,#000)", "m"),
        ("isolation:isolate", "m"),
        ("mix-blend-mode:multiply", "m"),
        ("perspective:100px", "m"),
        ("transform-style:preserve-3d", "m"),
        ("contain:paint", "m"),
        ("contain:layout", "m"),
        ("will-change:transform", "m"),
        ("will-change:opacity", "m"),
        ("will-change:clip-path", "m"),
        ("will-change:mask", "m"),
        ("will-change:isolation", "m"),
        ("will-change:mix-blend-mode", "m"),
        ("will-change:z-index", "m"),
        ("will-change:position", "m"),
        ("will-change:top", "h"),
        ("container-type:size", "h"),
        ("opacity:1", "h"),
        ("transform:none", "h"),
        ("isolation:auto", "h"),
        ("mix-blend-mode:normal", "h"),
        ("contain:style", "h"),
        ("contain:size", "h"),
    ]
    .into_iter()
    .map(|(style, want)| (style, ctx_wrap(style, &ctx_header(10)) + &ctx_modal(5), want))
    .collect();
    check_ctx(&cases);
}

/// A pointer in the window reaches the same rule as script does: the fixed
/// modal is 400 wide, the header 300.
#[test]
fn a_click_over_a_stuck_header_in_a_wrapper_goes_to_the_modal() {
    let width_at = |body: String| {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
            .expect("view");
        engine.load_html(view, &ctx_page(&body)).expect("load");
        engine.set_scroll_offset(view, 0.0, 500.0).expect("scroll");
        Engine::hit_at(&engine.views[&view], 10.0, 10.0).expect("hit").border_box.width
    };
    let wrapper = "position:relative;z-index:0";
    assert_eq!(width_at(ctx_wrap(wrapper, &ctx_header(10)) + &ctx_modal(5)), 400.0, "the modal");
    assert_eq!(width_at(ctx_wrap("", &ctx_header(10)) + &ctx_modal(5)), 300.0, "no wrapper: the header");
}

/// `trench/z/tools/c2/ctx_z_probe.mjs`, Chromium 143 at scroll 500.
#[track_caller]
fn check_ctx_styles(styles: &[(&'static str, &'static str)]) {
    let cases: Vec<(&str, String, &str)> = styles
        .iter()
        .map(|&(style, want)| (style, ctx_wrap(style, &ctx_header(10)) + &ctx_modal(5), want))
        .collect();
    check_ctx(&cases);
}

#[test]
fn a_z_index_counts_only_where_it_applies() {
    check_ctx_styles(&[
        ("opacity:.9;z-index:50", "m"),
        ("isolation:isolate;z-index:50", "m"),
        ("transform:translateX(0);z-index:50", "m"),
        ("filter:blur(0px);z-index:50", "m"),
        ("contain:paint;z-index:50", "m"),
        ("will-change:opacity;z-index:50", "m"),
        ("mix-blend-mode:multiply;z-index:50", "m"),
        ("opacity:.9;z-index:-1", "m"),
        ("opacity:.9;z-index:0", "m"),
        ("position:relative;opacity:.9;z-index:50", "h"),
        ("position:relative;isolation:isolate;z-index:50", "h"),
        ("position:relative;opacity:.9;z-index:1", "m"),
        ("position:relative;opacity:.9;z-index:-1", "m"),
    ]);
}

#[test]
fn a_z_index_applies_on_an_item_and_on_the_modals_side() {
    let in_parent = |display: &str, item: &str| {
        format!("<div style=\"display:{display}\">{}</div>", ctx_wrap(item, &ctx_header(10))) + &ctx_modal(5)
    };
    let app = ctx_wrap("position:relative;z-index:10", &ctx_header(1));
    let held = |style: &str| format!("<div style=\"{style}\">{}</div>", ctx_modal(1));
    check_ctx(&[
        ("flex item opacity z50", in_parent("flex", "flex:1;opacity:.9;z-index:50"), "h"),
        ("flex item opacity z1", in_parent("flex", "opacity:.9;z-index:1"), "m"),
        ("grid item isolate z50", in_parent("grid", "isolation:isolate;z-index:50"), "h"),
        ("modal in static z50", app.clone() + &held("opacity:.9;z-index:50"), "h"),
        ("modal in relative z50", app + &held("position:relative;opacity:.9;z-index:50"), "m"),
    ]);
}

#[test]
fn only_the_unprefixed_backdrop_filter_makes_a_context() {
    check_ctx_styles(&[
        ("-webkit-backdrop-filter:blur(2px)", "h"),
        ("backdrop-filter:blur(2px)", "m"),
        ("backdrop-filter:none", "h"),
        ("backdrop-filter:blur(2px);-webkit-backdrop-filter:none", "m"),
        ("-webkit-backdrop-filter:blur(2px);backdrop-filter:none", "h"),
    ]);
}

#[test]
fn content_visibility_makes_a_context_unless_visible() {
    check_ctx_styles(&[
        ("content-visibility:auto", "m"),
        ("content-visibility:hidden", "m"),
        ("CONTENT-VISIBILITY:AUTO", "m"),
        ("content-visibility:visible", "h"),
        ("content-visibility:foo", "h"),
        ("content-visibility:auto;content-visibility:visible", "h"),
        ("content-visibility:auto;content-visibility:foo", "m"),
    ]);
}

#[test]
fn a_view_transition_name_makes_a_context() {
    check_ctx_styles(&[
        ("view-transition-name:vt", "m"),
        ("view-transition-name:VT", "m"),
        ("view-transition-name:match-element", "m"),
        ("view-transition-name:none", "h"),
        ("view-transition-name:auto", "h"),
        ("view-transition-name:a b", "h"),
        ("view-transition-name:1a", "h"),
        ("view-transition-name:vt;view-transition-name:none", "h"),
        ("view-transition-name:vt;view-transition-name:1a", "m"),
    ]);
}

#[test]
fn will_change_of_a_mask_image_or_transition_name_makes_a_context() {
    check_ctx_styles(&[
        ("will-change:mask-image", "m"),
        ("will-change:-webkit-mask-image", "m"),
        ("will-change:MASK-IMAGE", "m"),
        ("will-change:top,mask-image", "m"),
        ("will-change:view-transition-name", "m"),
        ("will-change:mask-size", "h"),
        ("will-change:content-visibility", "h"),
        ("will-change:contain-intrinsic-size", "h"),
    ]);
}

/// Tailwind's `isolate` on the root, with the cookie bar outside it.
#[test]
fn a_cookie_bar_is_over_a_stuck_header_in_an_isolate_root() {
    let bar = "<div id=m style=\"position:fixed;left:0;top:0;width:400px;height:40px\"></div>";
    check_ctx(&[
        ("bar z40", ctx_wrap("isolation:isolate", &ctx_header(50)) + &ctx_modal(40), "m"),
        ("bar z auto", ctx_wrap("isolation:isolate", &ctx_header(50)) + bar, "m"),
    ]);
}

/// #861's owed pin: percent and `calc()` `opacity` on the wrapper make the
/// context `sticky_is_over` reads. Chromium 143 at scroll 500, on `ctx_z_probe.mjs`'s
/// page.
#[test]
fn a_percent_opacity_wrapper_keeps_the_stuck_header_under_a_later_fixed_box() {
    check_ctx_styles(&[
        ("opacity:50%", "m"),
        ("opacity:99%", "m"),
        ("opacity:calc(50%)", "m"),
        ("opacity:calc(99%)", "m"),
        ("opacity:0%", "m"),
        ("opacity:.99", "m"),
        ("opacity:var(--nope, 50%)", "m"),
        ("opacity:100%", "h"),
        ("opacity:calc(100%)", "h"),
        ("opacity:150%", "h"),
        ("opacity:1", "h"),
        ("opacity:calc(50% + 50%)", "h"),
        ("opacity:50 %", "h"),
    ]);
}

/// The context places the whole wrapper: order and the fixed box's `z-index`
/// outside it, and only `z-index` inside it.
#[test]
fn a_percent_opacity_wrapper_orders_as_one_box() {
    let half = "opacity:50%";
    check_ctx(&[
        ("fixed box first", ctx_modal(5) + &ctx_wrap(half, &ctx_header(10)), "m"),
        ("fixed box z 0 after", ctx_wrap(half, &ctx_header(10)) + &ctx_modal(0), "m"),
        ("fixed box z -1 after", ctx_wrap(half, &ctx_header(10)) + &ctx_modal(-1), "h"),
        ("both inside, header higher", ctx_wrap(half, &(ctx_header(10) + &ctx_modal(5))), "h"),
        ("both inside, fixed box higher", ctx_wrap(half, &(ctx_header(5) + &ctx_modal(10))), "m"),
        ("100%, fixed box first", ctx_modal(5) + &ctx_wrap("opacity:100%", &ctx_header(10)), "h"),
    ]);
}

/// The pointer's path through `Engine::hit_at`, not only script's.
#[test]
fn a_click_over_a_stuck_header_in_a_percent_opacity_wrapper_goes_to_the_fixed_box() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let body = ctx_wrap("opacity:50%", &ctx_header(10)) + &ctx_modal(5);
    engine.load_html(view, &ctx_page(&body)).expect("load");
    engine.set_scroll_offset(view, 0.0, 500.0).expect("scroll");
    assert_eq!(Engine::hit_at(&engine.views[&view], 10.0, 10.0).expect("hit").border_box.width, 400.0);
}

/// Whitespace in `calc()` and numbers past the float range reach the context too.
#[test]
fn opacity_with_a_tab_a_newline_or_a_huge_number_decides_the_context() {
    check_ctx_styles(&[
        ("opacity:calc(25%\n+\n25%)", "m"),
        ("opacity:calc(25%\t+\t25%)", "m"),
        ("opacity:-1e40", "m"),
        ("opacity:calc(-1e40)", "m"),
        ("opacity:calc(0\t+\t50%)", "h"),
        ("opacity:calc(0\n+\n50%)", "h"),
        ("opacity:1e40", "h"),
        ("opacity:1e40%", "h"),
        ("opacity:calc(1e40)", "h"),
    ]);
}

/// R1 delta on #861: an overflowing division, a spaced `%` and `1.e40`.
#[test]
fn an_overflowing_division_or_a_dropped_calc_opacity_makes_no_wrong_context() {
    check_ctx_styles(&[
        ("opacity:calc(1 / 1e-40)", "h"),
        ("opacity:calc(50% / 1e-45)", "h"),
        ("opacity:calc(-1 / 1e-40)", "m"),
        ("opacity:calc(50\t%)", "h"),
        ("opacity:calc(50 %)", "h"),
        ("opacity:calc(1.e40)", "h"),
    ]);
}
