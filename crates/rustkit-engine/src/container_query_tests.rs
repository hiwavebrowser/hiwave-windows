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
    // 300, so Chromium gives 15, but since HOLD 3 the walk knows widths of
    // plain blocks only and a flex item is unknown: the query is false.
    assert_eq!(heights, ["0", "0", "0", "0", "0"]);
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
const R2_PAGE: &str = include_str!("../../../docs/diagnostics/2026-10-26/css_container_query_r2.html");

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
