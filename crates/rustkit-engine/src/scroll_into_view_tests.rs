//! Plan row C2 (c): `scrollIntoView` and fragment navigation scroll every
//! enclosing scroller, innermost first, then the window. Expected states are
//! Chromium 143.0.7499.4 on the same pages (`trench/z/tools/c2/c2c_probe.mjs`
//! and `c2c_probe2.mjs` on the hub).

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px;width:3000px}",
    "#out{overflow:auto;width:300px;height:200px;position:absolute;left:0;top:1000px}",
    "#gap{height:400px}",
    "#in{overflow:auto;width:200px;height:100px;border:5px solid #000;padding:7px}",
    "#in-gap{height:300px;width:600px}",
    "#t{height:20px;width:30px;margin-left:400px}",
    "#after{height:300px}",
    "#hid{overflow:hidden;width:100px;height:100px;position:absolute;left:500px;top:0}",
    "#hid-t{margin-top:400px;height:10px}",
    "#clip{overflow:clip;width:100px;height:100px;position:absolute;left:650px;top:0}",
    "#clip-t{margin-top:400px;height:10px}",
    "#rel{overflow:auto;width:100px;height:100px;position:absolute;left:950px;top:0}",
    "#rel-gap{height:300px}",
    "#ab2{position:absolute;left:0;top:250px;width:20px;height:20px}",
    "#fx{position:fixed;left:0;top:500px;width:20px;height:20px}",
    "#nd{display:none}",
    "#big-s{overflow:auto;width:100px;height:100px;position:absolute;left:1100px;top:0}",
    "#big{height:500px;margin-top:50px}",
    "</style><body>",
    "<div id=out><div id=gap></div><div id=in><div id=in-gap></div><div id=t></div><div id=after></div></div></div>",
    "<div id=hid><div id=hid-t></div></div>",
    "<div id=clip><div id=clip-t></div></div>",
    "<div id=rel><div id=rel-gap></div><div id=ab2></div></div>",
    "<div id=fx></div><div id=nd></div>",
    "<div id=big-s><div id=big></div></div>",
    r##"<a id=link href="#t" style="position:fixed;left:700px;top:500px">go</a>"##,
    "</body>",
);

const PAGE2: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}",
    "#sp0{height:1000px}#s{overflow:auto;width:200px;height:100px}",
    "#s-gap{height:300px}#s-t{height:20px}",
    "#s-ab{position:absolute;left:10px;top:1800px;width:20px;height:20px}",
    "#s-fx{position:fixed;left:400px;top:500px;width:20px;height:20px}",
    "#f{position:fixed;left:400px;top:0;width:100px;height:100px;overflow:auto}",
    "#f-gap{height:300px}#f-t{height:20px}",
    "#nm{display:block;height:20px}#under{height:600px}",
    "</style><body>",
    "<div id=sp0></div><div id=s><div id=s-gap></div><div id=s-t></div><div id=s-ab></div><div id=s-fx></div>",
    r#"<div style="height:100px"></div></div>"#,
    "<div id=f><div id=f-gap></div><div id=f-t></div></div>",
    "<div id=under></div>",
    "<a name=named id=nm>named</a>",
    r#"<div id="a b" style="height:20px">spaced</div>"#,
    r##"<a id=lk href="#s-t" style="position:fixed;left:600px;top:0">go</a>"##,
    "</body>",
);

const STATE: &str = "function state() { \
    return ['in', 'out', 'hid', 'clip', 'rel', 'big-s', 's', 'f', 'tw', 'tb'].map(function (i) { \
        var e = document.getElementById(i); \
        return e && (e.scrollLeft || e.scrollTop) ? i + '=' + e.scrollLeft + ',' + e.scrollTop : ''; \
    }).filter(Boolean).concat('win=' + scrollX + ',' + scrollY).join(' '); } \
    var $ = function (i) { return document.getElementById(i); };";

fn serve(page: &str, path: &str) -> (Engine, EngineViewId, crate::script_net_tests::Server) {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}{path}", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (engine, view, server)
}

fn read(engine: &mut Engine, view: EngineViewId, script: &str) -> String {
    let got = engine.execute_script(view, &format!("{STATE} {script}")).expect("script");
    got.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&got).to_string()
}

/// `script` on a fresh load of `page`, then the scroll state.
#[track_caller]
fn check(page: &str, script: &str, want: &str) {
    let (mut engine, view, _server) = serve(page, "/");
    read(&mut engine, view, script);
    assert_eq!(read(&mut engine, view, "state()"), want, "{script}");
    let (x, y) = engine.get_scroll_offset(view).unwrap();
    assert_eq!(format!("win={x},{y}"), want.rsplit(' ').next().unwrap(), "the view's own offset: {script}");
}

// The window has no horizontal extent (`max_scroll_offset.0` is always 0), so
// where Chromium also moves the window across, these expect its x to stay 0:
// start/start 12, `ab2` 170, `big` 400. Everything else is Chromium's.

#[test]
fn the_default_scrolls_every_scroller_and_the_window() {
    let want = "in=223,307 out=0,324 win=0,1081";
    check(PAGE, "$('t').scrollIntoView()", want);
    check(PAGE, "$('t').scrollIntoView(true)", want);
    check(PAGE, "$('t').scrollIntoView({behavior: 'instant'})", want);
}

#[test]
fn block_and_inline_apply_at_every_level() {
    let end = "in=223,213 out=0,319 win=0,600";
    check(PAGE, "$('t').scrollIntoView(false)", end);
    check(PAGE, "$('t').scrollIntoView({block: 'end', inline: 'end'})", end);
    check(PAGE, "$('t').scrollIntoView({block: 'nearest', inline: 'nearest'})", end);
    check(PAGE, "$('t').scrollIntoView({block: 'center', inline: 'center'})", "in=315,260 out=0,324 win=0,838");
    check(PAGE, "$('t').scrollIntoView({block: 'start', inline: 'start'})", "in=400,307 out=0,324 win=0,1081");
}

#[test]
fn nearest_leaves_what_already_shows() {
    check(
        PAGE,
        "$('in').scrollTop = 280; $('out').scrollTop = 9999; scrollTo(0, 2000); $('t').scrollIntoView({block: 'nearest'})",
        "in=223,280 out=0,324 win=0,1108",
    );
    check(PAGE, "scrollTo(0, 1000); $('gap').scrollIntoView({block: 'nearest'})", "win=0,1000");
}

#[test]
fn a_target_taller_than_the_scroller() {
    check(PAGE, "$('big').scrollIntoView()", "big-s=0,50 win=0,0");
    check(PAGE, "$('big').scrollIntoView({block: 'nearest'})", "big-s=0,50 win=0,0");
    check(PAGE, "$('big-s').scrollTop = 100; $('big').scrollIntoView({block: 'nearest'})", "big-s=0,100 win=0,0");
    check(PAGE, "$('big').scrollIntoView({block: 'center'})", "big-s=0,250 win=0,0");
}

#[test]
fn hidden_scrolls_and_clip_does_not() {
    check(PAGE, "$('hid-t').scrollIntoView()", "hid=0,310 win=0,90");
    check(PAGE, "$('clip-t').scrollIntoView()", "win=0,400");
}

#[test]
fn an_absolute_box_goes_by_its_containing_block() {
    check(PAGE, "$('ab2').scrollIntoView()", "rel=0,200 win=0,50");
    check(PAGE2, "$('s-t').scrollIntoView()", "s=0,300 win=0,1000");
}

#[test]
fn a_fixed_box_leaves_the_window_alone() {
    check(PAGE, "scrollTo(0, 200); $('fx').scrollIntoView()", "win=0,200");
    check(PAGE2, "scrollTo(0, 200); $('s-fx').scrollIntoView()", "win=0,200");
    check(PAGE2, "scrollTo(0, 200); $('f').scrollIntoView()", "win=0,200");
    check(PAGE2, "scrollTo(0, 200); $('f-t').scrollIntoView()", "f=0,220 win=0,200");
}

#[test]
fn a_box_that_is_not_rendered_scrolls_nothing() {
    for arg in ["", "{block: 'center'}", "false"] {
        check(PAGE, &format!("scrollTo(0, 200); $('nd').scrollIntoView({arg})"), "win=0,200");
    }
}

#[test]
fn each_scroller_hears_one_scroll_and_the_document_one() {
    let (mut engine, view, _server) = serve(PAGE, "/");
    let got = read(
        &mut engine,
        view,
        "window.ev = []; ['in', 'out'].forEach(function (i) { $(i).onscroll = function (e) { ev.push(i + ':' + e.bubbles); }; }); \
         document.onscroll = function () { ev.push('doc'); }; $('t').scrollIntoView(); String(ev.length)",
    );
    assert_eq!(got, "0", "not from inside the call");
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    for _ in 0..5 {
        rt.block_on(engine.pump_live(view, 16));
    }
    assert_eq!(read(&mut engine, view, "ev.join(' ')"), "in:false out:false doc");
}

#[test]
fn a_script_fragment_navigation_scrolls_as_scroll_into_view_does() {
    check(PAGE, "location.hash = '#t'", "in=223,307 out=0,324 win=0,1081");
    check(PAGE, "location.href = '#in'", "out=0,324 win=0,1076");
    check(PAGE, "location.hash = '#hid-t'", "hid=0,310 win=0,90");
    check(PAGE2, "location.replace('#s-t')", "s=0,300 win=0,1000");
    check(PAGE2, "location.assign('#s-t')", "s=0,300 win=0,1000");
}

#[test]
fn the_scroll_comes_before_hashchange() {
    let (mut engine, view, _server) = serve(PAGE, "/");
    let got = read(
        &mut engine,
        view,
        "window.seen = ''; window.onhashchange = function () { seen = scrollY + ':' + $('in').scrollTop; }; location.hash = '#t'; seen",
    );
    assert_eq!(got, "1081:307");
}

#[test]
fn a_fragment_finds_a_name_and_a_percent_encoded_id() {
    check(PAGE2, "location.hash = 'named'", "win=0,1700");
    check(PAGE2, "location.hash = 'a%20b'", "win=0,1720");
}

#[test]
fn a_fragment_that_matches_nothing_stays_and_top_goes_to_the_top() {
    check(PAGE, "scrollTo(0, 50); location.hash = '#nope'", "win=0,50");
    check(PAGE2, "scrollTo(0, 50); location.hash = '#S-T'", "win=0,50");
    check(PAGE, "scrollTo(0, 50); $('in').scrollTop = 40; location.hash = '#top'", "in=0,40 win=0,0");
    check(PAGE, "scrollTo(0, 50); location.hash = '#TOP'", "win=0,0");
}

#[test]
fn pushstate_does_not_scroll() {
    check(PAGE, "history.pushState(null, '', '#t')", "win=0,0");
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_click_on_a_fragment_link_scrolls_every_scroller() {
    let (mut engine, view, _server) = serve(PAGE2, "/");
    assert_eq!(engine.follow_link_at_point(view, 605.0, 5.0), None);
    assert_eq!(read(&mut engine, view, "state()"), "s=0,300 win=0,1000");
    assert_eq!(engine.get_scroll_offset(view).unwrap(), (0.0, 1000.0));
    // The same link again scrolls again, with no new history entry.
    read(&mut engine, view, "$('s').scrollTop = 0; scrollTo(0, 0); window.len = history.length");
    assert_eq!(engine.follow_link_at_point(view, 605.0, 5.0), None);
    assert_eq!(read(&mut engine, view, "state() + ' ' + (history.length - len)"), "s=0,300 win=0,1000 0");
}

#[test]
fn a_page_loaded_with_a_fragment_opens_there() {
    let (mut engine, view, _server) = serve(PAGE2, "/#s-t");
    assert_eq!(read(&mut engine, view, "state()"), "s=0,300 win=0,1000");
    assert_eq!(engine.get_scroll_offset(view).unwrap(), (0.0, 1000.0));
    let (mut engine, view, _server) = serve(PAGE2, "/#nope");
    assert_eq!(read(&mut engine, view, "state()"), "win=0,0");
    // Loading it is not a navigation script hears about.
    let page = format!("{PAGE2}<script>window.heard = 0; window.onhashchange = function () {{ heard++; }};</script>");
    let (mut engine, view, _server) = serve(&page, "/#s-t");
    assert_eq!(read(&mut engine, view, "String(heard) + ' ' + history.length"), "0 1");
}

// The extents these clamp to, Chromium's numbers (`c2c_extent_probe.mjs`).
#[test]
fn a_scrollers_clipped_content_is_not_overflow_of_the_scroller_around_it() {
    check(PAGE, "$('out').scrollTop = 99999", "out=0,324 win=0,0");
    let (mut engine, view, _server) = serve(PAGE, "/");
    assert_eq!(read(&mut engine, view, "[$('out').scrollHeight, $('in').scrollHeight, $('in').scrollWidth].join(' ')"), "524 634 614");
}

const EXTENTS: &str = concat!(
    "<!doctype html><style>body{margin:0}",
    "#vis{width:100px;height:50px;padding:7px}#vis-c{height:100px;width:200px}",
    "#hid{overflow:hidden;width:100px;height:50px;padding:7px}#hid-c{height:100px;width:200px}",
    "#clip{overflow:clip;width:100px;height:50px;padding:7px}#clip-c{height:100px;width:200px}",
    "#o2{overflow:auto;width:100px;height:50px}#vis2{height:20px}#vis2-c{height:100px}",
    "#o3{overflow:auto;width:100px;height:50px}#hid3{overflow:hidden;height:20px}#hid3-c{height:100px}",
    "#o4{overflow:auto;width:100px;height:50px}#clip4{overflow:clip;height:20px}#clip4-c{height:100px}",
    "#o5{overflow:auto;width:100px;height:50px;position:relative}#st5{overflow:auto;height:20px}",
    "#ab5{position:absolute;top:200px;height:10px;width:10px}",
    "#short{overflow:auto;width:100px;height:100px;padding:7px}#short-c{height:20px}",
    "</style><body>",
    "<div id=vis><div id=vis-c></div></div><div id=hid><div id=hid-c></div></div>",
    "<div id=clip><div id=clip-c></div></div><div id=o2><div id=vis2><div id=vis2-c></div></div></div>",
    "<div id=o3><div id=hid3><div id=hid3-c></div></div></div>",
    "<div id=o4><div id=clip4><div id=clip4-c></div></div></div>",
    "<div id=o5><div id=st5><div id=ab5></div></div></div>",
    "<div id=short><div id=short-c></div></div></body>",
);

#[test]
fn padding_end_counts_on_a_scroller_and_clipped_content_stops_at_its_box() {
    let (mut engine, view, _server) = serve(EXTENTS, "/");
    let got = read(
        &mut engine,
        view,
        "['vis', 'hid', 'clip', 'o2', 'o3', 'o4', 'o5', 'short'].map(function (i) { \
         return i + ' ' + $(i).scrollWidth + ' ' + $(i).scrollHeight; }).join(', ')",
    );
    assert_eq!(got, "vis 207 107, hid 214 114, clip 207 107, o2 100 100, o3 100 50, o4 100 50, o5 100 210, short 114 114");
}

#[test]
fn the_paint_clamps_to_the_same_extent_script_reads() {
    let (mut engine, view, _server) = serve(EXTENTS, "/");
    read(&mut engine, view, "$('hid').scrollTop = 99999; $('hid').scrollLeft = 99999");
    assert_eq!(read(&mut engine, view, "$('hid').scrollLeft + ',' + $('hid').scrollTop"), "100,50");
    let layout = engine.views[&view].layout.clone().expect("layout");
    fn find(b: &LayoutBox, id: usize) -> Option<(f32, f32)> {
        if b.node_id == Some(id) {
            return Some(b.element_scroll_max());
        }
        b.children.iter().find_map(|c| find(c, id))
    }
    let node = engine.views[&view].document.as_ref().unwrap().get_element_by_id("hid").unwrap().id.raw();
    assert_eq!(find(&layout, node), Some((100.0, 50.0)));
}

// Past a static scroller. At y=0, because an absolute box with no positioned
// ancestor is placed from its static parent (2800 where Chromium has 1800 with
// the parent at y=1000): `c2c_abs_probe.mjs`, on the plan's C12.
#[test]
fn an_absolute_box_skips_a_static_scroller() {
    let page = concat!(
        "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:200px;height:100px}",
        "#a{position:absolute;left:10px;top:1800px;width:20px;height:20px}</style>",
        "<body><div id=s><div style=height:300px></div><div id=a></div></div></body>",
    );
    check(page, "$('a').scrollIntoView()", "win=0,1800");
}

// `location.hash = v` when the URL's fragment (none reads as empty) is
// already `v` without its `#` does nothing (HTML §7.2.4, the hash setter).
// Chromium 143.0.7499.4 on PAGE (`c2c_hash_probe.mjs` on the hub): the
// window and `#in` stay, no `hashchange`, no entry, the URL unchanged.
// Clearing a fragment that is there goes to the top and is heard.

/// From `path`, with the window at 50 and `#in` at 40, run `script`, then
/// the scroll state, `hashchange`s heard, entries added, and the URL.
fn hash_case(path: &str, script: &str) -> String {
    let (mut engine, view, _server) = serve(PAGE, path);
    read(
        &mut engine,
        view,
        "window.hc = 0; window.onhashchange = function () { hc++; }; scrollTo(0, 50); $('in').scrollTop = 40; window.len0 = history.length",
    );
    read(&mut engine, view, script);
    let got = read(
        &mut engine,
        view,
        "state() + ' ' + hc + ' ' + (history.length - len0) + ' ' + location.href.slice(location.origin.length)",
    );
    let (x, y) = engine.get_scroll_offset(view).unwrap();
    format!("{got} view={x},{y}")
}

#[test]
fn clearing_the_hash_of_a_url_without_one_does_nothing() {
    assert_eq!(hash_case("/", "location.hash = ''"), "in=0,40 win=0,50 0 0 / view=0,50");
    assert_eq!(hash_case("/", "location.hash = '#'"), "in=0,40 win=0,50 0 0 / view=0,50");
}

#[test]
fn clearing_a_fragment_goes_to_the_top_and_is_heard() {
    assert_eq!(hash_case("/#nope", "location.hash = ''"), "in=0,40 win=0,0 1 1 /# view=0,0");
    assert_eq!(hash_case("/#nope", "location.hash = '#'"), "in=0,40 win=0,0 1 1 /# view=0,0");
}

#[test]
fn the_same_hash_again_does_nothing() {
    assert_eq!(hash_case("/#nope", "location.hash = 'nope'"), "in=0,40 win=0,50 0 0 /#nope view=0,50");
    assert_eq!(hash_case("/#nope", "location.hash = '#nope'"), "in=0,40 win=0,50 0 0 /#nope view=0,50");
    // A bare `#` is an empty fragment, the same as the one asked for.
    assert_eq!(hash_case("/#", "location.hash = ''"), "in=0,40 win=0,50 0 0 /# view=0,50");
    assert_eq!(hash_case("/#", "location.hash = '#'"), "in=0,40 win=0,50 0 0 /# view=0,50");
}

#[test]
fn an_href_with_a_bare_hash_still_navigates() {
    // Only the hash setter compares fragments; `href` to the URL plus `#`
    // is a fragment navigation in Chromium too.
    assert_eq!(hash_case("/", "location.href = location.href + '#'"), "in=0,40 win=0,0 1 1 /# view=0,0");
}

// A transformed box is on the layout's `scroller` chain since #852 (it moves
// the boxes inside it), but it does not scroll. Chromium 143.0.7499.4 on
// this page (`c2c_transform_chain_probe.mjs` on the hub): only `#s` and the
// window move. For `#tb-t` Chromium also moves the window 5 across; x stays
// 0 here, as above.
const PAGE3: &str = concat!(
    "<!doctype html><style>body{margin:0;height:3000px}#sp0{height:1000px}",
    "#s{overflow:auto;width:200px;height:100px}#s-gap{height:300px}",
    "#tw{transform:translateY(50px);height:200px}#tw-gap{height:100px}#tw-t{height:20px}#s-end{height:100px}",
    "#tb{transform:translateX(5px)}#tb-gap{height:500px}#tb-t{height:20px}",
    "</style><body><div id=sp0></div><div id=s><div id=s-gap></div>",
    "<div id=tw><div id=tw-gap></div><div id=tw-t></div></div><div id=s-end></div></div>",
    "<div id=tb><div id=tb-gap></div><div id=tb-t></div></div></body>",
);

#[test]
fn a_transformed_box_that_does_not_scroll_is_not_a_scroller() {
    check(PAGE3, "$('tw-t').scrollIntoView()", "s=0,450 win=0,1000");
    check(PAGE3, "$('tw-t').scrollIntoView({ block: 'nearest' })", "s=0,370 win=0,500");
    check(PAGE3, "$('tb-t').scrollIntoView()", "win=0,1600");
}
