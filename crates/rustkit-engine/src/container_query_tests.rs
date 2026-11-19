//! `@container` size queries (css-contain-3 §6). Their rules used to be
//! dropped by the parser, so a page's container-specific styles never
//! applied. Heights below are Chromium 148's on this page at 400 x 200.

use super::*;
use crate::script_net_tests::serve_routes;

const PAGE: &str = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 }
.c { container-type: inline-size; width: 300px }
#c1 { container-name: card }
#auto { container: wide / inline-size; padding: 0 50px }
#inner { container-type: inline-size; width: 100px }
@container card (width >= 200px) { #k1 { height: 11px } }
@container (min-width: 250px) { #k2 { height: 12px } }
@container (min-width: 400px) { #k3 { height: 13px } }
@container wide (width = 300px) { #k4 { height: 14px } }
@container (min-width: 1px) { #k5 { height: 15px } }
@container (max-width: 150px) { #k6 { height: 16px } }
@container nope (min-width: 1px) { #k1 { height: 99px } }
@container card (max-width: 150px) { #k6 { height: 99px } }
@container (height > 0px) { #k2 { height: 99px } }
</style></head><body>
<div class="c" id="c1"><div id="k1"></div><div id="inner"><div id="k6"></div></div></div>
<div class="c"><div id="k2"></div></div>
<div class="c"><div id="k3"></div></div>
<div id="auto"><div id="k4"></div></div>
<div id="k5"></div>
</body></html>"#;

/// Loads `page` in a headless view of `width` x `height` and returns the
/// `offsetHeight` of `#k1`..`#k{count}`.
fn offset_heights(page: &str, width: u32, height: u32, count: usize) -> Vec<String> {
    let server = serve_routes(vec![("/", "text/html", page.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width,
            height,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    (1..=count)
        .map(|k| {
            let expr = format!("String(document.getElementById('k{k}').offsetHeight)");
            let value = engine.execute_script(view, &expr).unwrap();
            value
                .strip_prefix("String(\"")
                .and_then(|v| v.strip_suffix("\")"))
                .unwrap_or(&value)
                .to_string()
        })
        .collect()
}

#[test]
fn container_rules_apply_where_their_query_container_matches() {
    let heights = offset_heights(PAGE, 400, 200, 6);
    // k1: named, 300 >= 200. k2: the nearest container; an inline-size one
    // has no height to ask. k3: 300 < 400. k4: an auto width less padding.
    // k5: no container. k6: the nearest container is the 100px one.
    assert_eq!(heights, ["11", "12", "0", "14", "0", "16"]);
}

/// #782 HOLD (Atlas #837): a container whose laid-out width is not its own
/// `width` answered with the style width, so wide-layout rules fired on a
/// narrow box. Same page as docs/diagnostics/2026-10-23/
/// css_container_query_flex.html; Chromium 148 lays `.s` at 300px, `.main`
/// at 400px, `.a` and `.gi` at 200px, `.n` at 300px, and applies only #k5's
/// rule (which the walk now declines, see below).
const LAYOUT_WIDTH_PAGE: &str = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 }
.f { display: flex; width: 300px } .s { width: 500px; min-width: 0; container-type: inline-size }
.f2 { display: flex; width: 600px } .main { width: 100%; container-type: inline-size } .side { width: 200px; flex-shrink: 0 }
.p { position: relative; width: 200px } .w { width: 600px }
.a { position: absolute; top: 0; left: 0; width: 100%; container-type: inline-size }
.g { display: grid; grid-template-columns: 200px; width: 600px } .gi { width: 100%; container-type: inline-size }
.n { flex-shrink: 0; width: 300px; container-type: inline-size }
@container (min-width: 400px) { #k1, #k3, #k4 { height: 11px } }
@container (min-width: 500px) { #k2 { height: 12px } }
@container (min-width: 250px) { #k5 { height: 15px } }
</style></head><body>
<div class="f"><div class="s"><div id="k1"></div></div></div>
<div class="f2"><div class="main"><div id="k2"></div></div><div class="side"></div></div>
<div class="p"><div class="w"><div class="a"><div id="k3"></div></div></div></div>
<div class="g"><div class="gi"><div id="k4"></div></div></div>
<div class="f"><div class="n"><div id="k5"></div></div></div>
</body></html>"#;

#[test]
fn container_query_answers_with_the_laid_out_width_not_the_style_width() {
    let heights = offset_heights(LAYOUT_WIDTH_PAGE, 800, 600, 5);
    // k1: flex shrinks 500 -> 300. k2: 100% of 600 shrinks to 400 beside a
    // 200px sidebar. k3: 100% of the positioned 200px box, not of the 600px
    // parent. k4: 100% of the 200px grid column. k5: flex-shrink:0 keeps
    // 300, so Chromium gives 15. Since CQ1 a flex item answers with its
    // laid-out width (it was a fail-safe false after HOLD 3).
    assert_eq!(heights, ["0", "0", "0", "0", "15"]);
}

/// #782 HOLD 2 (Atlas #843) and HOLD 3 (Atlas #847, review 5634168311):
/// where only layout knows a container's width, the walk guessed one wider
/// than the laid-out box, so wide-layout rules fired on a narrow box. Every
/// width is now unknown (the query false) unless the container and its
/// ancestors are plain blocks. Chromium 148 on this page at 800 x 600 gives
/// 0 to all fifteen probes:
/// k1 an inline container, k2 an auto width beside a float, k3 a child of
/// a two-column box, k4 a `display: contents` container, k7 `fit-content`,
/// k8 `max-content`; k9 a float left by an earlier wrapper div, k10 one
/// left by an earlier span, k11 `max-width: min-content`, k12
/// `-webkit-max-content`, k13 a `display: block` button, k14 a vertical-lr
/// container, k15 a child of a table with `border-spacing` (580px, asked
/// for 590px), k16 a child of a `-webkit-box`, k17 `MAX-CONTENT`.
/// Controls get 10: k5 and k6 (plain and list-item containers 600px wide)
/// and k18 (a 600px scroll container). They come first: an earlier float
/// in the same formatting context makes every later container that starts
/// a context unknown, however far down the page (float heights and `clear`
/// are not modelled).
const UNKNOWN_WIDTH_PAGE: &str = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 } .w { width: 600px }
@container (min-width: 400px) { .a, #k1,#k2,#k3,#k4,#k5,#k6,#k7,#k8 { height: 10px } }
@container (min-width: 590px) { .b { height: 10px } }
</style></head><body>
<div class="w"><div style="container-type:inline-size"><div id="k5"></div></div></div>
<div class="w"><ul style="margin:0;padding:0"><li style="container-type:inline-size"><div id="k6"></div></li></ul></div>
<div class="w"><div style="overflow-y:scroll;container-type:inline-size"><div id="k18" class="a"></div></div></div>
<div class="w"><span style="container-type:inline-size"><b id="k1" style="display:inline-block;width:5px"></b></span></div>
<div class="w"><div style="float:left;width:300px;height:1px"></div><div style="container-type:inline-size"><div id="k2"></div></div></div>
<div class="w" style="columns:2;column-gap:0"><div style="container-type:inline-size"><div id="k3"></div></div></div>
<div class="w"><div style="display:contents;container-type:inline-size"><div id="k4"></div></div></div>
<div class="w"><div style="width:fit-content;container-type:inline-size"><div id="k7"></div></div></div>
<div class="w"><div style="width:max-content;container-type:inline-size"><div id="k8"></div></div></div>
<div class="w"><div><div style="float:left;width:300px;height:50px"></div></div><div style="container-type:inline-size"><div id="k9" class="a"></div></div></div>
<div class="w"><span><b style="float:left;width:300px;height:50px"></b></span><div style="container-type:inline-size"><div id="k10" class="a"></div></div></div>
<div class="w"><div style="max-width:min-content;container-type:inline-size"><div id="k11" class="a"></div></div></div>
<div class="w"><div style="width:-webkit-max-content;container-type:inline-size"><div id="k12" class="a"></div></div></div>
<div class="w"><button style="display:block;padding:0;border:0;container-type:inline-size"><div id="k13" class="a"></div></button></div>
<div class="w"><div style="writing-mode:vertical-lr;width:600px;height:100px;container-type:inline-size"><div id="k14" class="a" style="writing-mode:horizontal-tb"></div></div></div>
<div class="w"><div style="display:table;width:600px;border-spacing:10px"><div style="container-type:inline-size"><div id="k15" class="b"></div></div></div></div>
<div class="w"><div style="display:-webkit-box"><div style="container-type:inline-size"><div id="k16" class="a"></div></div></div></div>
<div class="w"><div style="width:MAX-CONTENT;container-type:inline-size"><div id="k17" class="a"></div></div></div>
</body></html>"#;

#[test]
fn container_query_is_false_where_only_layout_knows_the_width() {
    let heights = offset_heights(UNKNOWN_WIDTH_PAGE, 800, 600, 18);
    let fired: Vec<String> = heights
        .iter()
        .enumerate()
        .filter(|(_, h)| *h != "0")
        .map(|(i, h)| format!("k{}={h}", i + 1))
        .collect();
    assert_eq!(fired, ["k5=10", "k6=10", "k18=10"]);
}

/// What the allowlist still knows (#782 HOLD 3 regressions seen on the
/// 2026-10-20 and HOLD 2 pages): an absolutely positioned or floated
/// container with a length width (k2, k3). Unknown: a container after a
/// float, even one that cannot leave its fixed-height row (k1; Chromium
/// gives 10, but since HOLD 4 only layout decides where a float ends, so
/// this is a fail-safe false), a percentage width on an absolute box (k4),
/// and anything after a float taller than its row (k5).
const OUT_OF_FLOW_PAGE: &str = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 } .w { position: relative; width: 600px; height: 30px }
@container (min-width: 250px) { .a { height: 10px } }
</style></head><body>
<div class="w"><div style="float:left;width:300px;height:1px"></div></div>
<div class="w"><div style="container-type:inline-size"><div id="k1" class="a"></div></div></div>
<div class="w"><div style="position:absolute;width:300px;container-type:inline-size"><div id="k2" class="a"></div></div></div>
<div class="w"><div style="float:left;width:300px;height:10px;container-type:inline-size"><div id="k3" class="a"></div></div></div>
<div class="w"><div style="position:absolute;width:30%;container-type:inline-size"><div id="k4" class="a"></div></div></div>
<div class="w"><div style="float:left;width:10px;height:50px"></div></div>
<div class="w"><div style="container-type:inline-size"><div id="k5" class="a"></div></div></div>
</body></html>"#;

#[test]
fn container_query_knows_out_of_flow_lengths_and_contained_floats() {
    let heights = offset_heights(OUT_OF_FLOW_PAGE, 800, 600, 5);
    assert_eq!(heights, ["0", "10", "10", "0", "0"]);
}

/// #782 HOLD 4 (Atlas #848; reviews 5635034145 and 5635107389 at
/// 1d17810c): inputs where the walk still guessed wider than the laid-out
/// box. Each row is its own block formatting context (`.r`), so a float in
/// one row cannot reach the next. Chromium gives 0 to every probe but the
/// control k1 (a plain 600px container). k2 a block with a definite height
/// and `aspect-ratio` (100px wide); k3-k15 a container beside a float that
/// overhangs its definite-height ancestor, or sits in a box starting a
/// context the walk cannot see: k3 `max-height`, k4 text before the float,
/// k5-k7 a `width: 100%` container in a `flow-root` / `contain: paint` /
/// `overflow-y: hidden` box, k8 `max-height` again, k9 `padding-top` on a
/// wrapper, k10 text in a narrow box, k11 a `::before` block, k12 a
/// negative `margin-bottom`, k13 a float `min-height`, k14 a border-box
/// float with padding, k15 `height` on an inline span; k16 and k17 an open
/// `dialog` (absolute and `fit-content` in Chromium's UA sheet).
const FLOAT_AND_DIALOG_PAGE: &str = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 } .r { overflow: hidden; width: 600px } .w { width: 600px }
.fl { float: left; width: 300px; height: 50px }
.d::before { content: ""; display: block; height: 30px }
@container (min-width: 400px) { .a { height: 10px } }
</style></head><body>
<div class="r"><div class="w"><div style="container-type:inline-size"><div id="k1" class="a"></div></div></div></div>
<div class="r"><div class="w"><div style="height:100px;aspect-ratio:1"><div style="container-type:inline-size"><div id="k2" class="a"></div></div></div></div></div>
<div class="r"><div class="w" style="height:120px;max-height:10px"><div class="fl"></div></div><div class="w"><div style="container-type:inline-size"><div id="k3" class="a"></div></div></div></div>
<div class="r"><div class="w"><div style="height:60px;font:10px/20px monospace;white-space:pre">a
b
<span style="float:left;width:300px;height:55px"></span></div><div style="container-type:inline-size"><div id="k4" class="a"></div></div></div></div>
<div class="r"><div class="w"><div class="fl"></div><div style="display:flow-root"><div style="container-type:inline-size;width:100%"><div id="k5" class="a"></div></div></div></div></div>
<div class="r"><div class="w"><div class="fl"></div><div style="contain:paint"><div style="container-type:inline-size;width:100%"><div id="k6" class="a"></div></div></div></div></div>
<div class="r"><div class="w"><div class="fl"></div><div style="overflow-y:hidden"><div style="container-type:inline-size;width:100%"><div id="k7" class="a"></div></div></div></div></div>
<div class="r"><div style="height:100px;max-height:20px"><div class="fl"></div></div><div style="container-type:inline-size"><div id="k8" class="a"></div></div></div>
<div class="r"><div style="height:60px"><div style="padding-top:30px"><div class="fl"></div></div></div><div style="container-type:inline-size"><div id="k9" class="a"></div></div></div>
<div class="r"><div style="height:60px;width:300px">text<div class="fl"></div></div><div style="container-type:inline-size"><div id="k10" class="a"></div></div></div>
<div class="r"><div class="d" style="height:60px"><div class="fl"></div></div><div style="container-type:inline-size"><div id="k11" class="a"></div></div></div>
<div class="r"><div style="height:60px;margin-bottom:-60px"><div class="fl"></div></div><div style="container-type:inline-size"><div id="k12" class="a"></div></div></div>
<div class="r"><div style="height:60px"><div class="fl" style="min-height:100px"></div></div><div style="container-type:inline-size"><div id="k13" class="a"></div></div></div>
<div class="r"><div style="height:60px"><div class="fl" style="box-sizing:border-box;padding:40px 0"></div></div><div style="container-type:inline-size"><div id="k14" class="a"></div></div></div>
<div class="r"><span style="height:100px"><b class="fl"></b></span><div style="container-type:inline-size"><div id="k15" class="a"></div></div></div>
<div class="r"><dialog open style="container-type:inline-size;padding:0;margin:0"><div id="k16" class="a"></div></dialog></div>
<div class="r"><dialog open style="padding:0;margin:0"><div style="container-type:inline-size"><div id="k17" class="a"></div></div></dialog></div>
</body></html>"#;

#[test]
fn container_query_is_false_after_any_float_and_for_aspect_ratio_or_dialog() {
    let heights = offset_heights(FLOAT_AND_DIALOG_PAGE, 800, 600, 17);
    let fired: Vec<String> = heights
        .iter()
        .enumerate()
        .filter(|(_, h)| *h != "0")
        .map(|(i, h)| format!("k{}={h}", i + 1))
        .collect();
    assert_eq!(fired, ["k1=10"]);
}

/// #782 R1 HOLD @ 73558160 (review 5651507176): three more ways the walk
/// thought a container wider than Chromium lays it out. Each row is its own
/// block formatting context (`.r`). Controls first: k1 and k2 are plain
/// 600px containers asked for 400px and 590px (10 in Chromium). Then 0 in
/// Chromium (its container width in brackets):
/// k3 (q1) a float inside an `overflow: hidden` inline span (300), k4 (q2)
/// inside a `container-type` inline span (300), k5 (q3) inside a
/// `display: contents; overflow: hidden` div (300): none of these start a
/// formatting context, so the float narrows the later container. k6 (q4) a
/// floated `::before` of the container's parent (300), k7 (q5) a floated
/// `::after` of an earlier sibling (300), k8 (q6) a floated `::first-letter`
/// of an earlier sibling (556.7, asked for 590px). k9 (q7) an absolute
/// border-box container, 500px with `padding-left: 10%` of the 800px
/// viewport, not of the 600px row (420, asked for 430px).
const HOLD5_PAGE: &str = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 } .r { overflow: hidden; width: 600px }
.fl { float: left; width: 300px; height: 50px }
.pb::before { content: ""; float: left; width: 300px; height: 50px }
.pa::after { content: ""; float: left; width: 300px; height: 50px }
.fx { height: 20px; font: 16px/20px sans-serif }
.fx::first-letter { float: left; font-size: 60px; line-height: 60px }
@container (min-width: 400px) { .a { height: 10px } }
@container (min-width: 590px) { .b { height: 10px } }
@container (min-width: 430px) { .c { height: 10px } }
</style></head><body>
<div class="r"><div style="container-type:inline-size"><div id="k1" class="a"></div></div></div>
<div class="r"><div style="container-type:inline-size"><div id="k2" class="b"></div></div></div>
<div class="r"><span style="overflow:hidden"><b class="fl"></b></span><div style="container-type:inline-size"><div id="k3" class="a"></div></div></div>
<div class="r"><span style="container-type:inline-size"><b class="fl"></b></span><div style="container-type:inline-size"><div id="k4" class="a"></div></div></div>
<div class="r"><div style="display:contents;overflow:hidden"><div class="fl"></div></div><div style="container-type:inline-size"><div id="k5" class="a"></div></div></div>
<div class="r"><div class="pb"><div style="container-type:inline-size"><div id="k6" class="a"></div></div></div></div>
<div class="r"><div class="pa"></div><div style="container-type:inline-size"><div id="k7" class="a"></div></div></div>
<div class="r"><div class="fx">Hello</div><div style="container-type:inline-size"><div id="k8" class="b"></div></div></div>
<div class="r"><div style="position:absolute;box-sizing:border-box;width:500px;padding-left:10%;container-type:inline-size"><div id="k9" class="c"></div></div></div>
</body></html>"#;

#[test]
fn container_query_sees_floats_in_inline_boxes_and_pseudos_and_abs_percent_padding() {
    let heights = offset_heights(HOLD5_PAGE, 800, 600, 9);
    let fired: Vec<String> = heights
        .iter()
        .enumerate()
        .filter(|(_, h)| *h != "0")
        .map(|(i, h)| format!("k{}={h}", i + 1))
        .collect();
    assert_eq!(fired, ["k1=10", "k2=10"]);
}

/// #782 R2 (review on 5c3b1ae7), fixes 1, 2 and 4 and the `(width)` nit,
/// on docs/diagnostics/2026-10-26/css_container_query_r2.html. Chromium
/// 148 applies k1, k9, k11, k17, k18, k21, k22 and k24:
/// - k1-k7, k13-k15, k23 (fix 1): `not` over a part the evaluator does not
///   know (`style()`, `30ch`, `hover`) was true, and media-only features
///   (`hover`, `color`, `prefers-color-scheme`, `min-device-width`)
///   answered for a container; an unknown part anywhere makes the whole
///   condition false. k1 `not style(--x)` holds in Chromium, which answers
///   style queries; here it stays false.
/// - k8/k9: `(width)` is false on a 0px container, true on a 300px one.
/// - k10-k12: em is the container's font size (40px, 20px); vw the
///   viewport's, not the container's.
/// - k16-k18 (fix 2): a size container's height is clamped by max-height
///   (200 -> 30) and min-height (10 -> 30; 200 -> 40, min wins).
/// - k19-k21 (fix 4): `max-width: none` / `max-height: none` after a
///   500px / 30px limit removes it (800px wide, 200px tall).
const R2_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-10-26/css_container_query_r2.html");

#[test]
fn container_query_r2_unknown_parts_min_max_height_and_none_limits() {
    let heights = offset_heights(R2_PAGE, 800, 600, 24);
    let fired: Vec<String> = heights
        .iter()
        .enumerate()
        .filter(|(_, h)| *h != "0")
        .map(|(i, h)| format!("k{}={h}", i + 1))
        .collect();
    assert_eq!(
        fired,
        ["k9=10", "k11=10", "k17=10", "k18=10", "k21=10", "k22=10", "k24=10"]
    );
}

const GRAMMAR_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-10-26/css_container_query_grammar.html");

/// #782 R1 (review 5654936187): invalid not/and/or grammar never applies,
/// and min()/max() read every argument. Expected rows are Chromium 148's.
#[test]
fn container_query_grammar_drops_invalid_not_and_or() {
    let heights = offset_heights(GRAMMAR_PAGE, 800, 600, 26);
    let fired: Vec<usize> = heights
        .iter()
        .enumerate()
        .filter(|(_, h)| *h != "0")
        .map(|(i, _)| i + 1)
        .collect();
    assert_eq!(
        fired,
        [7, 8, 9, 10, 11, 12, 13, 14, 15, 18, 19, 20, 22, 23, 24, 25, 26]
    );
}

const NITS_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-10-27/css_container_query_nits.html");

/// #782 follow-up nits (Atlas #868, Prometheus on #782). k1: `foo(` opens a
/// function, so `@container foo(width > 1px)` has no container name and its
/// general-enclosed condition is unknown; the name was split off at `(` and
/// the rule fired. k3: Chromium answers the style query (13); here it stays
/// unknown, so false. k4-k9 pin the boxes that keep a float from narrowing
/// a later container (`starts_context`): ruby, ruby-text, a table row, a
/// block and a cell with `overflow` do in Chromium, an inline span does not.
/// Every other row is Chromium 148's.
#[test]
fn container_query_nits_name_split_and_context_roots() {
    let heights = offset_heights(NITS_PAGE, 800, 600, 9);
    assert_eq!(heights, ["0", "12", "0", "14", "14", "14", "0", "14", "14"]);
}

const CQ1_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-11/css_container_query_cq1.html");

/// CQ1, reduced from a real-site page whose card row is a named
/// `@container gridrow (width >= 42rem)`: the container sits in a column
/// flex item of a grid item of a flexed `<main>`, so the style-time
/// estimate had no width and every query under it was false; the row
/// stayed one column. Chromium answers with the laid-out content box
/// (Chrome for Testing 148.0.7778.216 at 1280 x 800: `#c1` 692 wide, `#c2`
/// a `flex: 1` item 700 wide):
/// k1 named, 692 >= 672; k2 named, 692 < 896; k3 unnamed, the nearest
/// container (`#c1`); k4 named, past a nearer container of another name;
/// k5 that nearer 100px container; k6 past an element with the name but no
/// `container-type`; k7 a named container that is itself a flex item.
#[test]
fn named_container_query_answers_with_the_laid_out_size_of_flex_and_grid_items() {
    let heights = offset_heights(CQ1_PAGE, 1280, 800, 7);
    assert_eq!(heights, ["11", "0", "13", "14", "0", "16", "17"]);
}

/// CQ1: a container query is answered again when its container resizes.
/// The same page at 1280 x 800, then the view resized to 900: Chromium has
/// `#c1` 312 wide, so only k7 (the 700px flex item) still holds.
#[test]
fn container_query_is_answered_again_when_the_container_resizes() {
    let server = serve_routes(vec![("/", "text/html", CQ1_PAGE.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let read = |engine: &mut Engine| -> Vec<String> {
        (1..=7)
            .map(|k| {
                let expr = format!("String(document.getElementById('k{k}').offsetHeight)");
                let value = engine.execute_script(view, &expr).unwrap();
                value
                    .strip_prefix("String(\"")
                    .and_then(|v| v.strip_suffix("\")"))
                    .unwrap_or(&value)
                    .to_string()
            })
            .collect()
    };
    assert_eq!(read(&mut engine), ["11", "0", "13", "14", "0", "16", "17"]);
    engine
        .resize_view(
            view,
            Bounds {
                x: 0,
                y: 0,
                width: 900,
                height: 800,
            },
        )
        .expect("resize");
    assert_eq!(read(&mut engine), ["0", "0", "0", "0", "0", "0", "17"]);
}

const CONTENT_SIZED_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-11/css_container_query_content_sized.html");

/// #1070 R1 MED 2: `container-type: inline-size` applies inline-size
/// containment, so a container whose width comes from its contents is
/// sized as if empty. Chrome for Testing 148.0.7778.216 gives each
/// `padding: 0 5px` container around a 300px child a width of 10, so
/// `(min-width: 200px)` is false in all four: a `flex: 0 0 auto` item, an
/// `inline-block`, `width: fit-content`, and a block in a float.
/// rustkit-layout lays them out 310 wide, so its measurement must not
/// answer the query.
#[test]
fn container_query_on_a_content_sized_container_is_false() {
    let heights = offset_heights(CONTENT_SIZED_PAGE, 1280, 800, 4);
    assert_eq!(heights, ["0", "0", "0", "0"]);
}

/// #1070 R1 MED 1: an `inline-size` container's height answers no query,
/// so a write that only grows it costs the builds the same write costs on
/// the page without its `@container` rule, and no measured pass. It cost
/// one more each time (the height counted as a moved size, and the
/// measured pass ran again).
#[test]
fn a_height_change_inside_an_inline_size_container_takes_no_extra_pass() {
    let page = |rule: &str| {
        format!(
            r#"<!DOCTYPE html><html><head><style>
body {{ margin: 0 }} .c {{ container-type: inline-size; width: 400px }}
{rule}
</style></head><body><div class="c"><div id="k"></div><div id="grow" style="height: 10px"></div></div></body></html>"#
        )
    };
    // The builds three height writes cost, and the measured passes.
    let cost = |html: String| -> (u32, u32) {
        let server = serve_routes(vec![("/", "text/html", html)]);
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds {
                x: 0,
                y: 0,
                width: 800,
                height: 600,
            })
            .expect("view");
        let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(engine.load_url(view, url)).expect("load_url");
        let counts = |engine: &Engine| {
            let lay = &engine.views[&view].lay;
            (lay.layout_builds.get(), lay.container_passes.get())
        };
        let before = counts(&engine);
        for h in [20, 30, 40] {
            engine
                .execute_script(
                    view,
                    &format!(
                        "document.getElementById('grow').style.height = '{h}px'; \
                         String(document.getElementById('k').offsetHeight)"
                    ),
                )
                .unwrap();
        }
        let after = counts(&engine);
        (after.0 - before.0, after.1 - before.1)
    };
    let with_rule = cost(page(
        "@container (min-width: 300px) { #k { height: 11px } }",
    ));
    let without = cost(page(""));
    assert_eq!(
        with_rule,
        (without.0, 0),
        "builds and measured passes, against {without:?}"
    );
}

const AUTO_MINIMUM_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-11/css_container_query_auto_minimum.html");

/// #1070 R1 MED (review 5861798552): inline-size containment contains a
/// container's automatic minimum too. Two `flex: 1` containers in a 200px
/// row, one holding a 300px box, and the same in a `1fr 1fr` grid: Chrome
/// for Testing 148.0.7778.216 makes all four 100 wide, so `(min-width:
/// 150px)` holds in none. Without containment the engine floors the item
/// holding the 300px box at its content (300, and 0 for its sibling).
#[test]
fn a_containers_automatic_minimum_is_contained() {
    let heights = offset_heights(AUTO_MINIMUM_PAGE, 1280, 800, 4);
    assert_eq!(heights, ["0", "0", "0", "0"]);
}

const CONTAINED_INTRINSIC_PAGE: &str = include_str!(
    "../../../docs/diagnostics/2026-11-11/css_container_query_contained_intrinsic.html"
);

/// CQ2: inline-size containment in intrinsic sizing. A `container-type:
/// inline-size; padding: 0 5px` container around a 300px child contributes
/// only its padding and border to its min- and max-content width, so the
/// four content-sized shapes (a `flex: 0 0 auto` item, an `inline-block`,
/// `width: fit-content`, a block in a float) are 10 wide (Chrome for Testing
/// 148.0.7778.216), not 310. Their content box is 0, so `(max-width: 1px)`
/// holds: Chrome gives z1..z4 7. The float's container (z4) is trusted
/// since CQ3.
#[test]
fn a_size_containers_intrinsic_width_ignores_its_contents() {
    let server = serve_routes(vec![(
        "/",
        "text/html",
        CONTAINED_INTRINSIC_PAGE.to_string(),
    )]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let got = engine
        .execute_script(
            view,
            "[1,2,3,4].map(function (i) { return document.getElementById('c' + i).offsetWidth + '/' \
             + document.getElementById('k' + i).offsetHeight + '/' \
             + document.getElementById('z' + i).offsetHeight; }).join(' ')",
        )
        .unwrap();
    assert!(
        got.contains("10/0/7 10/0/7 10/0/7 10/0/7"),
        "width/k/z per container, Chrome 10/0/7 each: {got}"
    );
}

/// Loads `html` at 1280 x 800; returns the boxes layout visited for the
/// load, the measured re-passes it took, and `#k`'s height.
fn load_cost(html: String) -> (u64, u32, String) {
    let server = serve_routes(vec![("/", "text/html", html)]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let before = rustkit_layout::layout_visits();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let visits = rustkit_layout::layout_visits() - before;
    let passes = engine.views[&view].lay.container_passes.get();
    let k = engine
        .execute_script(view, "String(document.getElementById('k').offsetHeight)")
        .unwrap();
    (visits, passes, k)
}

/// CQ3 (batch 62: first-layout time rose with #1070's re-passes): a
/// measured size that changes no query's answer costs no second build and
/// layout. The container is a `flex: 1` item (700 wide, which the
/// style-time estimate cannot know, so the first build answers false).
/// `(min-width: 2000px)` is false at 700 too: the load visits exactly the
/// boxes the same page without the rule visits. `(min-width: 150px)` turns
/// true at 700, so that page takes its one re-pass and `#k` gets its 10px.
#[test]
fn a_measured_size_that_changes_no_answer_costs_no_re_pass() {
    let page = |rule: &str| {
        format!(
            r#"<!DOCTYPE html><html><head><style>
body {{ margin: 0 }} .row {{ display: flex; width: 800px }} .pad {{ width: 100px; flex: none }}
.c {{ flex: 1; container-type: inline-size }}
{rule}
</style></head><body><div class="row"><div class="pad"></div><div class="c"><div id="k"></div><p>text</p></div></div></body></html>"#
        )
    };
    let (plain_visits, plain_passes, _) = load_cost(page(""));
    let (same_visits, same_passes, same_k) = load_cost(page(
        "@container (min-width: 2000px) { #k { height: 10px } }",
    ));
    let (_, flip_passes, flip_k) = load_cost(page(
        "@container (min-width: 150px) { #k { height: 10px } }",
    ));
    assert_eq!(plain_passes, 0);
    assert_eq!(
        (same_passes, same_visits, same_k.contains("\"0\")")),
        (0, plain_visits, true),
        "an answer that stays false: no re-pass, the visits of the page without the rule"
    );
    assert_eq!(
        (flip_passes, flip_k.contains("10")),
        (1, true),
        "an answer that flips: one re-pass"
    );
}

const INLINE_AND_TABLE_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-11/css_container_query_inline_and_table.html");

/// #1081 R1 HIGH and MED: size containment does not apply to a non-atomic
/// inline box or to a table box (css-contain-2 §3.1), so such a container
/// is sized by its contents and answers no size query. Chrome for Testing
/// 148.0.7778.216:
/// - a `<span style="container-type: inline-size">` around a 100px box
///   leaves its inline-block, float, flex-item and grid-item wrapper 100
///   wide (w1..w4); containing the span collapsed each wrapper to 0;
/// - the span as a direct flex item (w5) or grid item (w6) is blockified,
///   so it IS contained: 0 wide around the same 100px box;
/// - `(min-width: 1px)` is false in a span (k1), a `display: table` box
///   with or without a width (k2, k3), an `inline-table` (k6) and a
///   table cell (k7), and true in a 300px inline-block (k4) and a block
///   (k5).
#[test]
fn an_inline_or_table_container_is_not_contained_and_answers_no_query() {
    let server = serve_routes(vec![("/", "text/html", INLINE_AND_TABLE_PAGE.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let got = engine
        .execute_script(
            view,
            "['w1','w2','w3','w4','w5','w6','k1','k2','k3','k4','k5','k6','k7'].map(function (id) { \
             return id + '=' + Math.round(document.getElementById(id).getBoundingClientRect().width); }).join(' ')",
        )
        .unwrap();
    // The second line of the recorded Chrome output
    // (`css_container_query_inline_and_table_probe.cjs`).
    let chrome = include_str!(
        "../../../docs/diagnostics/2026-11-11/css_container_query_inline_and_table.chrome148.txt"
    )
    .lines()
    .nth(1)
    .expect("the recorded widths");
    assert_eq!(
        got.strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&got),
        chrome
    );
}

/// #1081 delta R1: the wiring that tells the walk a build replays memoized
/// styles (`container_query::note_replaying`, called from the build with
/// `StyleMemoBuild::replays`). With the style memo armed, the first
/// relayout records and the second, with the same key, replays: the view
/// counts the replaying build (`ViewLayout::container_replays`), and the
/// query's answer is unchanged by it.
#[test]
fn a_build_that_replays_memoized_styles_is_flagged_to_the_walk() {
    let page = r#"<!DOCTYPE html><html><head><style>
body { margin: 0 } .c { container-type: inline-size; width: 400px }
@container (min-width: 300px) { #k1 { height: 11px } }
</style></head><body><div class="c"><div id="k1"></div></div></body></html>"#;
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, page).expect("load_html");
    let before = engine.views[&view].lay.container_replays.get();
    {
        let _memo = StyleMemoScope::arm_with(RestyleMode::Reuse).expect("armed");
        engine.relayout(view).expect("recording relayout");
        assert_eq!(
            engine.views[&view].lay.container_replays.get(),
            before,
            "the recording build is not a replay"
        );
        engine.relayout(view).expect("replaying relayout");
    }
    assert_eq!(
        engine.views[&view].lay.container_replays.get(),
        before + 1,
        "the replaying build was flagged to the walk"
    );
    let k = engine
        .execute_script(view, "String(document.getElementById('k1').offsetHeight)")
        .unwrap();
    assert!(k.contains("11"), "the query still holds: {k}");
}

const IN_FLOAT_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-13/css_container_query_in_float.html");

/// CQ3 leftover (#1081 "Not done"): a size container in or beside a float
/// answers with its laid-out width. Twelve shapes: a container under an
/// auto-width float (contained, so 10 / 8 wide), under a length, percentage
/// or `max-width` float, beside a float in a new formatting context, and a
/// float that is itself the container. Each `.q` is as tall as the number of
/// eight `min-width` thresholds (1..500px) its container meets. Chrome for
/// Testing 148.0.7778.216 gives the expected string (width:count), apart
/// from the two containers beside a float (see the assertion).
#[test]
fn a_container_in_or_beside_a_float_answers_with_its_width() {
    let server = serve_routes(vec![("/", "text/html", IN_FLOAT_PAGE.to_string())]);
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 1280,
            height: 800,
        })
        .expect("view");
    let url = Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(engine.load_url(view, url)).expect("load_url");
    let got = engine
        .execute_script(
            view,
            "(function(){var o=[];for(var i=1;i<=12;i++){var c=document.getElementById('c'+i);\
             o.push('c'+i+':'+c.offsetWidth+':'+c.querySelector('.q').offsetHeight);}\
             return o.join(' ');})()",
        )
        .unwrap();
    // Chrome: c10 1080:8 and c11 550:8. Both sit beside a float, which stays
    // on the fail-safe (count 0): c11 is laid out 640 wide here, not 550.
    assert!(
        got.contains(
            "c1:10:0 c2:250:5 c3:220:5 c4:384:7 c5:140:3 c6:238:5 c7:280:6 c8:300:6 \
             c9:8:0 c10:1080:0 c11:640:0 c12:180:4"
        ),
        "width:thresholds met per container (Chrome 148, c10/c11 fail-safe): {got}"
    );
}
