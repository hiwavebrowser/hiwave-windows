//! Plan row C2 (a): an `overflow: auto|scroll|hidden` box scrolls. The offsets
//! are stored and clamped, but nothing downstream reads them: a child's
//! geometry and the hit test stay at offset zero, and so does the paint.
//! Expected strings are Chromium 143.0.7499.4 on the same page
//! (`trench/z/tools/c2/c2_probe.mjs` and `c2_probe2.mjs` on the hub).

use super::*;

const PAGE: &str = concat!(
    "<!doctype html><style>body{margin:0}",
    "#s{overflow:auto;width:200px;height:100px;position:absolute;left:10px;top:20px}",
    "#w{width:500px;height:400px}",
    "#a{height:50px;background:red}#b{height:50px;background:lime}",
    "#c{height:300px;background:blue}",
    "#h{overflow:hidden;width:100px;height:50px;position:absolute;left:300px;top:20px}",
    "#hc{height:200px;background:orange}",
    "#v{overflow:visible;width:100px;height:50px;position:absolute;left:450px;top:20px}",
    "#vc{height:200px;background:teal}",
    "</style><body><div id=s><div id=w><div id=a>a</div><div id=b>b</div><div id=c>c</div>",
    "</div></div><div id=h><div id=hc>h</div></div><div id=v><div id=vc>v</div></div></body>",
);

fn loaded() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, PAGE).expect("load");
    (engine, view)
}

const AT: &str = "var $ = function (i) { return document.getElementById(i); }; \
     var at = function (i) { var r = $(i).getBoundingClientRect(); return r.x + ',' + r.y; }; ";

#[track_caller]
fn check(script: &str, want: &str) {
    let (mut engine, view) = loaded();
    let got = engine.execute_script(view, &format!("{AT}{script}")).expect("script");
    assert_eq!(got, format!("String(\"{want}\")"));
}

#[test]
fn the_extents_before_any_scroll() {
    check(
        "var s = $('s'); [s.scrollTop, s.scrollLeft, s.scrollHeight, s.scrollWidth, \
         s.clientHeight, s.clientWidth, at('b')].join(' ')",
        "0 0 400 500 100 200 10,70",
    );
}

#[test]
fn scroll_top_moves_the_children_and_not_the_box() {
    check(
        "var s = $('s'); s.scrollTop = 30; [s.scrollTop, at('s'), at('a'), at('b')].join(' ')",
        "30 10,20 10,-10 10,40",
    );
}

#[test]
fn scroll_left_moves_the_children() {
    check(
        "var s = $('s'); s.scrollTop = 30; s.scrollLeft = 40; [s.scrollLeft, at('b')].join(' ')",
        "40 -30,40",
    );
}

#[test]
fn the_offsets_clamp_at_both_ends() {
    check(
        "var s = $('s'); s.scrollTop = 9999; s.scrollLeft = 9999; \
         var far = [s.scrollTop, s.scrollLeft, at('c')].join(' '); \
         s.scrollTop = -5; s.scrollLeft = -5; far + ' ' + s.scrollTop + ' ' + s.scrollLeft",
        "300 300 -290,-180 0 0",
    );
}

#[test]
fn scroll_to_and_scroll_by_on_an_element() {
    check(
        "var s = $('s'); s.scrollTo(0, 60); var one = s.scrollTop + ' ' + at('b'); \
         s.scrollBy(0, 10); one + ' ' + s.scrollTop + ' ' + at('b')",
        "60 10,10 70 10,0",
    );
}

#[test]
fn the_hit_test_follows_the_scroll() {
    check(
        "var s = $('s'); s.scrollTop = 70; document.elementFromPoint(20, 30).id",
        "b",
    );
}

/// `overflow: hidden` scrolls from script too; `visible` does not scroll at all.
#[test]
fn hidden_scrolls_from_script_and_visible_does_not() {
    check(
        "var h = $('h'), v = $('v'); h.scrollTop = 25; v.scrollTop = 25; \
         [h.scrollTop, at('hc'), v.scrollTop, at('vc')].join(' ')",
        "25 300,-5 0 450,20",
    );
}

/// The second probe page: out-of-flow boxes, nesting, an `overflow: clip` box,
/// a box that goes away and comes back, and one that fires events.
const OUT_OF_FLOW: &str = concat!(
    "<!doctype html><style>body{margin:0}",
    "#st{overflow:auto;width:100px;height:100px}",
    "#st-in{height:300px;background:#00f}",
    "#st-abs{position:absolute;left:300px;top:0;width:20px;height:20px;background:#0f0}",
    "#st-fix{position:fixed;left:330px;top:0;width:20px;height:20px;background:#ff0}",
    "#rel{position:relative;overflow:auto;width:100px;height:100px;margin-top:10px}",
    "#rel-in{height:300px;background:#0ff}",
    "#rel-abs{position:absolute;left:10px;top:40px;width:20px;height:20px;background:#f00}",
    "#rel-fix{position:fixed;left:360px;top:0;width:20px;height:20px;background:#f0f}",
    "#out{overflow:auto;width:100px;height:100px;position:absolute;left:400px;top:0}",
    "#inner{overflow:auto;width:80px;height:200px}",
    "#deep{height:500px;background:#888}",
    "#clip{overflow:clip;width:50px;height:50px;position:absolute;left:520px;top:0}",
    "#clip-in{height:200px;background:#333}",
    "#gone{overflow:auto;width:50px;height:50px;position:absolute;left:580px;top:0}",
    "#gone-in{height:200px;background:#666}",
    "#ev{overflow:auto;width:50px;height:50px;position:absolute;left:640px;top:0}",
    "#ev-in{height:200px}",
    "</style><body><div id=st><div id=st-in><div id=st-abs></div><div id=st-fix></div></div></div>",
    "<div id=rel><div id=rel-in></div><div id=rel-abs></div><div id=rel-fix></div></div>",
    "<div id=out><div id=inner><div id=deep></div></div></div>",
    "<div id=clip><div id=clip-in></div></div>",
    "<div id=gone><div id=gone-in></div></div>",
    "<div id=ev><div id=ev-in></div></div></body>",
);

fn loaded_page(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    (engine, view)
}

#[track_caller]
fn check_on(page: &str, script: &str, want: &str) {
    let (mut engine, view) = loaded_page(page);
    let got = engine.execute_script(view, &format!("{AT}{script}")).expect("script");
    assert_eq!(got, format!("String(\"{want}\")"));
}

/// An absolute or fixed box whose containing block is above a static scroller
/// is not in its scrolling box: only the in-flow child moves.
#[test]
fn out_of_flow_children_of_a_static_scroller_stay() {
    check_on(
        OUT_OF_FLOW,
        "$('st').scrollTop = 50; [at('st-in'), at('st-abs'), at('st-fix')].join(' ')",
        "0,-50 300,0 330,0",
    );
}

/// A positioned scroller is the containing block of its absolute children, so
/// they scroll; a fixed child still does not.
#[test]
fn a_positioned_scroller_carries_its_absolute_child_and_not_a_fixed_one() {
    check_on(
        OUT_OF_FLOW,
        "$('rel').scrollTop = 50; [at('rel-in'), at('rel-abs'), at('rel-fix')].join(' ')",
        "0,60 10,100 360,0",
    );
}

/// `offsetTop` is from the layout: scrolling does not change it.
#[test]
fn offset_top_does_not_follow_the_scroll() {
    check_on(
        OUT_OF_FLOW,
        "$('rel').scrollTop = 50; $('st').scrollTop = 50; \
         [$('rel-abs').offsetTop, $('rel-abs').offsetParent.id, $('rel-in').offsetTop, $('st-in').offsetTop].join(' ')",
        "40 rel 0 0",
    );
}

#[test]
fn nested_scrollers_add_up_in_rects_and_the_hit_test() {
    check_on(
        OUT_OF_FLOW,
        "$('out').scrollTop = 30; $('inner').scrollTop = 70; \
         [at('inner'), at('deep'), document.elementFromPoint(410, 10).id].join(' ')",
        "400,-30 400,-100 deep",
    );
}

#[test]
fn overflow_clip_and_the_body_do_not_scroll() {
    check_on(
        OUT_OF_FLOW,
        "$('clip').scrollTop = 25; document.body.style.overflow = 'hidden'; document.body.scrollTop = 20; \
         [$('clip').scrollTop, at('clip-in'), document.body.scrollTop, at('st'), scrollY].join(' ')",
        "0 520,0 0 0,0 0",
    );
}

/// A box that goes away reads 0, and has its offset back when it returns. A
/// layout that shrinks the content clamps the offset.
#[test]
fn the_offset_survives_display_none_and_clamps_to_a_new_extent() {
    check_on(
        OUT_OF_FLOW,
        "var g = $('gone'); g.scrollTop = 40; g.style.display = 'none'; var hidden = g.scrollTop; \
         g.style.display = ''; var back = g.scrollTop + ' ' + at('gone-in'); \
         $('gone-in').style.height = '60px'; \
         [hidden, back, g.scrollTop, at('gone-in')].join(' ')",
        "0 40 580,-40 10 580,-10",
    );
}

/// The scroll event goes to the element, does not bubble, comes later, and is
/// one for several writes.
#[test]
fn a_scroll_fires_one_event_at_the_element_later() {
    let (mut engine, view) = loaded_page(OUT_OF_FLOW);
    let got = engine
        .execute_script(
            view,
            "window.seen = []; var e = document.getElementById('ev'); \
             e.addEventListener('scroll', function (v) { seen.push('el:' + v.bubbles + ':' + v.target.id); }); \
             document.addEventListener('scroll', function () { seen.push('doc'); }); \
             e.scrollTop = 10; e.scrollTop = 20; e.scrollTop = 20; String(seen.length)",
        )
        .expect("script");
    assert_eq!(got, "String(\"0\")");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for _ in 0..5 {
        rt.block_on(engine.pump_live(view, 16));
    }
    let got = engine.execute_script(view, "seen.join(' ')").expect("script");
    assert_eq!(got, "String(\"el:false:ev\")");
}

// ---- pixels: the frame after a scroll, checked at the colours Chromium paints.

const W: usize = 800;
const H: usize = 600;

fn pixel(ppm: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = ppm.len() - W * H * 3 + (y * W + x) * 3;
    [ppm[at], ppm[at + 1], ppm[at + 2]]
}

fn frame(engine: &mut Engine, view: EngineViewId, tag: &str) -> Vec<u8> {
    engine.render_view(view).expect("render");
    let path = std::env::temp_dir().join(format!("rustkit-c2-{tag}-{}.ppm", std::process::id()));
    engine.capture_frame(view, path.to_str().unwrap()).expect("capture");
    let ppm = std::fs::read(&path).expect("frame");
    let _ = std::fs::remove_file(&path);
    ppm
}

const WHITE: [u8; 3] = [255, 255, 255];
const LIME: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];

/// The paint moves inside the box's clip: with 70 scrolled, `a` (0..50) is gone
/// and the box's first row is `b` (50..100), then `c`, cut at the box's bottom.
#[test]
fn scroll_top_moves_the_paint_inside_the_clip() {
    let (mut engine, view) = loaded_page(PAGE);
    let before = frame(&mut engine, view, "before");
    assert_eq!(pixel(&before, 150, 80), LIME, "b paints at y 70..120 before the scroll");
    engine.execute_script(view, "document.getElementById('s').scrollTop = 70").expect("script");
    let after = frame(&mut engine, view, "after");
    assert_eq!(pixel(&after, 150, 22), LIME, "b is at the top of the box");
    assert_eq!(pixel(&after, 150, 80), BLUE, "c follows b");
    assert_eq!(pixel(&after, 150, 119), BLUE, "the box's last row");
    assert_eq!(pixel(&after, 150, 121), WHITE, "cut at the box's bottom");
    assert_eq!(pixel(&after, 150, 19), WHITE, "and at its top");
}

/// Out-of-flow boxes paint where the rects say, and the scroller's clip does
/// not hold the ones that escape it.
#[test]
fn out_of_flow_paint_follows_the_same_rules() {
    let (mut engine, view) = loaded_page(OUT_OF_FLOW);
    engine
        .execute_script(view, "document.getElementById('st').scrollTop = 50; document.getElementById('rel').scrollTop = 50")
        .expect("script");
    let ppm = frame(&mut engine, view, "oof");
    assert_eq!(pixel(&ppm, 310, 10), LIME, "st-abs stays");
    assert_eq!(pixel(&ppm, 340, 10), [255, 255, 0], "st-fix stays");
    assert_eq!(pixel(&ppm, 20, 110), [255, 0, 0], "rel-abs paints at 10,100, moved with its scroller");
    assert_eq!(pixel(&ppm, 20, 150), [0, 255, 255], "where it was laid out is rel-in now");
}

/// A later layout paints the offset too.
#[test]
fn a_relayout_keeps_the_offset() {
    let (mut engine, view) = loaded_page(PAGE);
    engine
        .execute_script(
            view,
            "document.getElementById('s').scrollTop = 70; document.getElementById('v').style.left = '460px'",
        )
        .expect("script");
    let ppm = frame(&mut engine, view, "relayout");
    assert_eq!(pixel(&ppm, 150, 22), LIME);
}

/// `c2_probe3.mjs`: a box's own border is not scrollable overflow, so a child
/// that fits leaves nothing to scroll. A write rounds to a whole pixel.
#[test]
fn a_border_is_not_overflow_and_a_write_rounds() {
    check_on(
        concat!(
            "<!doctype html><style>body{margin:0}",
            "#b{overflow:auto;width:100px;height:100px;border:5px solid #000;padding:7px}",
            "#b-in{height:100px}",
            "#f{overflow:auto;width:100px;height:100px}#f-in{height:300px}",
            "</style><body><div id=b><div id=b-in></div></div><div id=f><div id=f-in></div></div></body>",
        ),
        "var b = $('b'), f = $('f'); b.scrollTop = 500; f.scrollTop = 30.5; var up = f.scrollTop + ' ' + at('f-in'); \
         f.scrollTop = 30.4; [b.scrollHeight, b.clientHeight, b.scrollTop, up, f.scrollTop, at('f-in')].join(' ')",
        "114 114 0 31 0,93 30 0,94",
    );
}

/// A group under a scroll fades where it was moved to.
#[test]
fn an_opacity_group_moves_with_the_scroll() {
    let (mut engine, view) = loaded_page(concat!(
        "<!doctype html><style>body{margin:0}",
        "#o{overflow:auto;width:100px;height:100px}",
        "#o-in{height:120px;background:#f00;opacity:.5}#rest{height:200px}",
        "</style><body><div id=o><div id=o-in></div><div id=rest></div></div></body>",
    ));
    engine.execute_script(view, "document.getElementById('o').scrollTop = 50").expect("script");
    let ppm = frame(&mut engine, view, "group");
    let faded = pixel(&ppm, 50, 30);
    assert_eq!(faded[0], 255, "{faded:?}");
    assert!((120..=136).contains(&faded[1]), "half red: {faded:?}");
    assert_eq!(pixel(&ppm, 50, 80), WHITE, "the group ends at y 70 now");
}

/// A page's own script scrolls a box while it loads (a chat log, a carousel),
/// and the first frame shows it.
#[test]
fn a_script_in_the_load_scrolls_a_box() {
    let page = concat!(
        "<!doctype html><style>body{margin:0}#s{overflow:auto;width:200px;height:100px}",
        "#a{height:50px;background:#f00}#b{height:300px;background:#0f0}</style>",
        "<body><div id=s><div id=a></div><div id=b></div></div><script>",
        "var s = document.getElementById('s'); s.scrollTop = 70;",
        "window.r = s.scrollTop + ' ' + document.getElementById('b').getBoundingClientRect().y;",
        "</script></body>",
    );
    let server = crate::script_net_tests::serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let got = engine.execute_script(view, "window.r").expect("script");
    assert_eq!(got, "String(\"70 -20\")");
    let ppm = frame(&mut engine, view, "load");
    assert_eq!(pixel(&ppm, 100, 2), LIME, "the first frame is scrolled");
}
