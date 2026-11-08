//! Plan row C10 (b): `getBoundingClientRect` and `getClientRects` of a
//! transformed box answer where it paints; `offset*` stay where layout put it.
//! Expected strings are Chromium 143.0.7499.4 on the same pages
//! (`trench/z/tools/c10/c10b_probe.mjs` on the hub).

use super::*;

const READ: &str = "var $ = function (i) { return document.getElementById(i); }; \
     var nums = function (a) { return a.map(function (v) { return +v.toFixed(3); }).join(' '); }; \
     var box = function (i) { var r = $(i).getBoundingClientRect(); \
       return nums([r.x, r.y, r.width, r.height]); }; \
     var off = function (i) { var e = $(i); \
       return [e.offsetLeft, e.offsetTop, e.offsetWidth, e.offsetHeight].join(' '); }; ";

fn loaded(page: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, page).expect("load");
    (engine, view)
}

#[track_caller]
fn check(page: &str, script: &str, want: &str) {
    let (mut engine, view) = loaded(page);
    let got = engine
        .execute_script(view, &format!("{READ}{script}"))
        .expect("script");
    assert_eq!(got, format!("String(\"{want}\")"));
}

/// A 100x50 block at (20,10) with this style.
#[track_caller]
fn one(style: &str, want: &str) {
    let page = format!(
        "<!doctype html><style>body{{margin:0}}</style>\
         <div id=b style=\"position:absolute;left:20px;top:10px;width:100px;height:50px;{style}\"></div>"
    );
    check(&page, "box('b')", want);
}

#[test]
fn translate_moves_the_rect() {
    one("transform:translate(30px,40px)", "50 50 100 50");
}

#[test]
fn a_percentage_is_of_the_box() {
    one("transform:translate(-50%,-50%)", "-30 -15 100 50");
}

#[test]
fn scale_is_about_the_centre() {
    one("transform:scale(2)", "-30 -15 200 100");
}

#[test]
fn the_origin_moves_a_scale() {
    one("transform:scale(2);transform-origin:0 0", "20 10 200 100");
    one(
        "transform:scale(2);transform-origin:10px 20px",
        "10 -10 200 100",
    );
}

#[test]
fn a_rotation_reads_as_its_bounding_box() {
    one("transform:rotate(90deg)", "45 -15 50 100");
    one("transform:rotate(45deg)", "16.967 -18.033 106.066 106.066");
}

#[test]
fn matrix_and_skew() {
    one("transform:matrix(1,0,0,1,7,9)", "27 19 100 50");
    one("transform:skewX(45deg)", "-5 10 150 50");
}

#[test]
fn the_individual_properties() {
    one("translate:30px 40px", "50 50 100 50");
    one("scale:2", "-30 -15 200 100");
    one("rotate:90deg", "45 -15 50 100");
    one("translate:10px 0;transform:scale(2)", "-20 -15 200 100");
}

#[test]
fn the_rect_is_of_the_border_box() {
    one(
        "border:5px solid red;transform:translate(1px,2px)",
        "21 12 110 60",
    );
}

#[test]
fn no_transform_is_no_change() {
    one("transform:none", "20 10 100 50");
}

#[test]
fn get_client_rects_answers_the_same() {
    let page = "<!doctype html><style>body{margin:0}</style>\
        <div id=b style=\"position:absolute;left:20px;top:10px;width:100px;height:50px;transform:rotate(90deg)\"></div>";
    check(
        page,
        "var c = $('b').getClientRects(); c.length + ' ' + nums([c[0].x, c[0].y, c[0].width, c[0].height])",
        "1 45 -15 50 100",
    );
}

const METRICS: &str = "<!doctype html><style>body{margin:0}</style>\
    <div id=wrap style=\"position:relative;left:50px;top:60px;width:100px;height:50px;transform:translate(10px,20px) scale(2)\">\
      <div id=kid style=\"position:absolute;left:5px;top:6px;width:10px;height:10px\"></div>\
      <div id=flow style=\"width:20px;height:10px\"></div>\
    </div>\
    <div id=next style=\"height:10px;margin-top:80px\"></div>";

#[test]
fn offsets_and_sizes_stay_where_layout_put_the_box() {
    check(
        METRICS,
        "[off('wrap'), off('kid'), off('flow'), off('next')].join(' | ')",
        "50 60 100 50 | 5 6 10 10 | 0 0 20 10 | 0 130 800 10",
    );
}

#[test]
fn what_is_inside_moves_with_it_and_what_follows_does_not() {
    check(
        METRICS,
        "[box('wrap'), box('kid'), box('flow'), box('next')].join(' | ')",
        "10 55 200 100 | 20 67 20 20 | 10 55 40 20 | 0 130 800 10",
    );
}

#[test]
fn transforms_multiply_down_the_tree() {
    let page = "<!doctype html><style>body{margin:0}</style>\
        <div id=o style=\"position:absolute;left:0;top:0;width:200px;height:200px;transform:translate(100px,0) rotate(90deg);transform-origin:0 0\">\
          <div id=i style=\"width:50px;height:20px;margin-left:10px;transform:translate(5px,0)\">\
            <div id=leaf style=\"width:10px;height:10px\"></div>\
          </div>\
        </div>";
    check(
        page,
        "[box('o'), box('i'), box('leaf')].join(' | ')",
        "-100 0 200 200 | 80 15 20 50 | 90 15 10 10",
    );
}

const SCROLLERS: &str = "<!doctype html><style>body{margin:0}</style>\
    <div id=tin style=\"position:absolute;left:0;top:0;width:100px;height:100px;overflow:auto;transform:scale(2);transform-origin:0 0\">\
      <div id=tinkid style=\"height:400px;width:50px\"></div></div>\
    <div id=sout style=\"position:absolute;left:300px;top:0;width:100px;height:100px;overflow:auto\">\
      <div style=\"height:30px\"></div><div id=soutkid style=\"height:20px;width:40px;transform:scale(2);transform-origin:0 0\"></div>\
      <div style=\"height:400px\"></div></div>";

#[test]
fn a_scroll_inside_a_transform_is_scaled_by_it() {
    check(
        SCROLLERS,
        "var a = box('tinkid'); $('tin').scrollTop = 10; [a, box('tin'), box('tinkid')].join(' | ')",
        "0 0 100 800 | 0 0 200 200 | 0 -20 100 800",
    );
}

#[test]
fn a_transform_inside_a_scroll_is_moved_by_it() {
    check(
        SCROLLERS,
        "var a = box('soutkid'); $('sout').scrollTop = 7; [a, box('sout'), box('soutkid')].join(' | ')",
        "300 30 80 40 | 300 0 100 100 | 300 23 80 40",
    );
}

const HOLDS: &str = "<!doctype html><style>body{margin:0;height:3000px}</style>\
    <div style=\"height:100px\"></div>\
    <div id=t style=\"width:200px;height:50px;transform:translate(10px,20px)\">\
      <div id=abs style=\"position:absolute;left:1px;top:2px;width:10px;height:10px\"></div>\
      <div id=fix style=\"position:fixed;left:3px;top:4px;width:10px;height:10px\"></div>\
    </div>\
    <div id=sc style=\"margin-top:100px;width:100px;height:100px;overflow:auto;transform:translate(1px,1px)\">\
      <div id=sabs style=\"position:absolute;left:1px;top:2px;width:10px;height:10px\"></div>\
      <div style=\"height:400px\"></div></div>";

#[test]
fn a_transform_holds_absolute_boxes() {
    check(
        HOLDS,
        "[box('t'), box('abs'), box('sc'), box('sabs')].join(' | ')",
        "10 120 200 50 | 11 122 10 10 | 1 251 100 100 | 2 253 10 10",
    );
}

/// Known gap, not rects: layout places a fixed box from the viewport even
/// under a transform, its containing block. It reads 13 24 here. The window's
/// scroll and the transform are right; the box's place in layout is not.
#[test]
#[ignore = "layout: a fixed box under a transform is placed from the viewport"]
fn a_transform_holds_fixed_boxes() {
    check(HOLDS, "box('fix')", "13 124 10 10");
}

#[test]
fn the_window_scroll_comes_off_after_the_transform() {
    check(
        HOLDS,
        "window.scrollTo(0, 40); [box('t'), box('abs'), box('sc'), box('sabs')].join(' | ')",
        "10 80 200 50 | 11 82 10 10 | 1 211 100 100 | 2 213 10 10",
    );
}

/// An absolute box in a transformed scroller that is not positioned is
/// moved by the scroller's scroll: the transform makes it the containing block.
#[test]
fn an_absolute_box_held_by_a_transformed_scroller_scrolls_with_it() {
    check(HOLDS, "$('sc').scrollTop = 30; box('sabs')", "2 223 10 10");
}

/// How far a window scroll of 500 moves a fixed box under a wrapper with this
/// style. Only the move: where layout places a fixed box under its containing
/// block is row C10 (m).
#[track_caller]
fn fixed_moves_by(wrapper: &str, want: &str) {
    let page = format!(
        "<!doctype html><style>body{{margin:0;height:3000px}}</style><div style=\"height:100px\"></div>\
         <div id=w style=\"width:200px;height:50px;{wrapper}\">\
         <div id=f style=\"position:fixed;left:3px;top:4px;width:10px;height:10px\"></div></div>"
    );
    check(
        &page,
        "var a = $('f').getBoundingClientRect().y; window.scrollTo(0, 500); String($('f').getBoundingClientRect().y - a)",
        want,
    );
}

/// A filter is a containing block for fixed boxes, so the page takes them along.
#[test]
fn a_filter_holds_a_fixed_box_to_the_page() {
    fixed_moves_by("filter:blur(1px)", "-500");
}

#[test]
fn what_is_no_containing_block_leaves_a_fixed_box_alone() {
    for wrapper in ["", "filter:none", "opacity:0.5", "isolation:isolate"] {
        fixed_moves_by(wrapper, "0");
    }
}

/// `will-change` holds a fixed box when it names a property that would.
/// Chromium 143, `held2_probe.mjs`.
#[test]
fn will_change_holds_a_fixed_box_to_the_page() {
    for wrapper in [
        "will-change:transform",
        "will-change:TRANSFORM",
        "will-change:top, transform",
        "will-change:translate",
        "will-change:scale",
        "will-change:rotate",
        "will-change:perspective",
        "will-change:filter",
        "will-change:backdrop-filter",
        "will-change:contain",
        "will-change:transform-style",
        "will-change:-webkit-transform",
        "will-change:transform;will-change:bogus,",
    ] {
        fixed_moves_by(wrapper, "-500");
    }
    for wrapper in [
        "will-change:auto",
        "will-change:position",
        "will-change:scroll-position",
        "will-change:contents",
        "will-change:transform;will-change:auto",
    ] {
        fixed_moves_by(wrapper, "0");
    }
}

/// `contain` holds a fixed box for paint and layout, and so for the two that imply them.
#[test]
fn contain_holds_a_fixed_box_to_the_page() {
    for wrapper in [
        "contain:paint",
        "contain:PAINT",
        "contain:layout",
        "contain:strict",
        "contain:content",
        "contain:layout paint",
        "contain:paint;contain:bogus",
    ] {
        fixed_moves_by(wrapper, "-500");
    }
    for wrapper in [
        "contain:none",
        "contain:size",
        "contain:style",
        "contain:inline-size",
        "contain:size style",
        "contain:paint;contain:none",
        "contain:paint paint",
    ] {
        fixed_moves_by(wrapper, "0");
    }
}

/// A transform that moves nothing still holds it.
#[test]
fn an_identity_transform_holds_a_fixed_box_to_the_page() {
    fixed_moves_by("transform:translate(0)", "-500");
    fixed_moves_by("transform:scale(1)", "-500");
    fixed_moves_by("transform:none", "0");
}

/// What holds a fixed box in a scroller goes with the scroller, and takes
/// its fixed and absolute children along. With nothing holding them, they stay.
#[test]
fn a_box_that_holds_a_fixed_box_scrolls_it_with_its_scroller() {
    for (wrapper, moved) in [
        ("", "0 0"),
        ("filter:blur(1px)", "-40 -40"),
        ("will-change:transform", "-40 -40"),
        ("contain:paint", "-40 -40"),
        ("transform:translateX(0)", "-40 -40"),
    ] {
        let page = format!(
            "<!doctype html><style>body{{margin:0}}</style>\
             <div id=s style=\"width:200px;height:100px;overflow:auto\">\
             <div id=w style=\"width:100px;height:20px;{wrapper}\">\
             <div id=f style=\"position:fixed;left:3px;top:4px;width:10px;height:10px\"></div>\
             <div id=a style=\"position:absolute;left:5px;top:6px;width:10px;height:10px\"></div></div>\
             <div style=\"height:800px\"></div></div>"
        );
        check(
            &page,
            "var y = (i) => $(i).getBoundingClientRect().y; var f = y('f'), a = y('a'); $('s').scrollTop = 40; (y('f') - f) + ' ' + (y('a') - a)",
            moved,
        );
    }
}

/// Neither the window's scroll nor the scroller's moves it.
#[test]
fn a_fixed_box_in_an_element_scroller_stays_put() {
    check(
        "<!doctype html><style>body{margin:0;height:3000px}</style>\
         <div id=s style=\"width:200px;height:100px;overflow:auto\">\
         <div id=f style=\"position:fixed;left:3px;top:4px;width:10px;height:10px\"><div id=k style=\"height:5px\"></div></div>\
         <div style=\"height:800px\"></div></div>",
        "window.scrollTo(0, 500); $('s').scrollTop = 40; \
         var r = function (i) { var b = $(i).getBoundingClientRect(); return b.x + ' ' + b.y; }; r('f') + ' | ' + r('k')",
        "3 4 | 3 4",
    );
}

/// The rect of a 100x50 box at (20,10) when this value follows `translate:7px`:
/// refused reads `27 10`, as the earlier declaration stays.
#[track_caller]
fn translate_after_an_earlier_one(value: &str, want: &str) {
    let page = format!(
        "<!doctype html><style>body{{margin:0}}\
         #b{{position:absolute;left:20px;top:10px;width:100px;height:50px;translate:7px;translate:{value}}}</style>\
         <div id=b></div>"
    );
    check(&page, "box('b')", want);
}

/// Tailwind's slide-out panel: `-translate-y-[calc(100%+1px)]`.
#[test]
fn translate_takes_a_function_with_spaces_in_it() {
    translate_after_an_earlier_one("0px calc(-100% - 1px)", "20 -41 100 50");
    translate_after_an_earlier_one("calc(10px + 5%) 0", "35 10 100 50");
    translate_after_an_earlier_one("calc(10px + 5%)", "35 10 100 50");
    translate_after_an_earlier_one("calc(1px + 1px) calc(2px * 2) 0", "22 14 100 50");
    translate_after_an_earlier_one("calc( 1px + 1px ) 3px 0px", "22 13 100 50");
    translate_after_an_earlier_one("min(1px, 2px) max(3px, 4px)", "21 14 100 50");
}

/// With no perspective a depth flattens, as in `translate3d`.
#[test]
fn translate_flattens_a_depth() {
    translate_after_an_earlier_one("1px 2px 3px", "21 12 100 50");
    translate_after_an_earlier_one("1px 2px calc(1px + 1px)", "21 12 100 50");
}

/// The depth is a `<length>`: no percentage, not even inside a function, and no keyword.
/// Chromium 143, `translate_z_probe.mjs`.
#[test]
fn translate_refuses_a_depth_that_is_not_a_length() {
    for z in [
        "3%",
        "0%",
        "auto",
        "fit-content",
        "calc(1px + 1%)",
        "calc(0% + 1px)",
        "min(1%, 2px)",
        "max(1px, calc(1px + 1%))",
        "clamp(1px, 1%, 2px)",
        "3",
    ] {
        translate_after_an_earlier_one(&format!("1px 2px {z}"), "27 10 100 50");
    }
    for z in ["0", "-3px", "3em", "3vw", "calc(1px + 1em)", "min(1px, 2px)"] {
        translate_after_an_earlier_one(&format!("1px 2px {z}"), "21 12 100 50");
    }
    // A percentage is fine in the other two.
    translate_after_an_earlier_one("10% 10% 3px", "30 15 100 50");
}

#[test]
fn translate_takes_three_values_at_most() {
    translate_after_an_earlier_one("1px 2px 3px 4px", "27 10 100 50");
    translate_after_an_earlier_one("calc(1px + 1px) calc(2px * 2) 0 0", "27 10 100 50");
    translate_after_an_earlier_one("none", "20 10 100 50");
}

/// The page's shape: an opaque panel slid up out of a clipping wrapper, which
/// is what keeps it from showing. Where layout puts the wrapper is row C15; the
/// panel's move from it is this one.
#[test]
fn a_panel_slides_out_of_its_clipping_wrapper() {
    let page = "<!doctype html><style>body{margin:0;height:2000px}\
        #h{position:sticky;top:0;height:72px;width:800px;display:flex}\
        ul{display:flex;margin:0;padding:0;list-style:none}li{display:flex;z-index:30;margin-left:100px;width:90px}\
        #w{position:absolute;top:72px;left:0;width:100%;overflow:hidden auto;max-height:500px}\
        #p{position:relative;background:rgb(2,9,10);height:200px;translate:0px calc(-100% - 1px)}\
        </style><body><div id=h><ul><li><div id=w><div id=p></div></div></li></ul></div></body>";
    check(
        page,
        "$('p').getBoundingClientRect().y - $('w').getBoundingClientRect().y + ' ' + $('p').getBoundingClientRect().height",
        "-201 200",
    );
}

const FIXED: &str = "<!doctype html><style>body{margin:0;height:3000px}</style>\
    <div id=m style=\"position:fixed;left:50%;top:10px;width:200px;height:50px;transform:translateX(-50%)\"></div>\
    <div id=plain style=\"position:fixed;left:7px;top:9px;width:20px;height:20px\"></div>\
    <div id=wrap style=\"position:fixed;left:100px;top:100px;width:200px;height:100px;transform:scale(2);transform-origin:0 0\">\
      <div id=kid style=\"position:absolute;left:5px;top:6px;width:10px;height:10px\"></div>\
      <div id=fkid style=\"width:20px;height:10px\"></div>\
    </div>\
    <div id=undertr style=\"position:fixed;left:300px;top:300px;width:100px;height:100px;overflow:auto;transform:translate(1px,2px)\">\
      <div id=skid style=\"height:30px;width:30px\"></div><div style=\"height:400px\"></div>\
    </div>\
    <div id=fs style=\"position:fixed;left:500px;top:100px;width:100px;height:100px;overflow:auto\">\
      <div id=fsk style=\"height:20px;width:40px;transform:translate(3px,4px)\"></div><div style=\"height:400px\"></div>\
    </div>\
    <div id=flow style=\"width:50px;height:50px;transform:translate(1px,2px)\"></div>\
    <div style=\"height:200px\"></div>\
    <div id=inner style=\"width:100px;height:100px;transform:translate(10px,20px)\">\
      <div id=abs style=\"position:absolute;left:1px;top:2px;width:10px;height:10px\"></div>\
    </div>";

const HELD_READ: &str = "['m', 'plain', 'wrap', 'kid', 'fkid', 'undertr', 'skid', 'fs', 'fsk']\
     .map(box).join(' | ')";

const HELD_AT_ANY_SCROLL: &str = "300 10 200 50 | 7 9 20 20 | 100 100 400 200 | 110 112 20 20 | \
     100 100 40 20 | 301 302 100 100 | 301 302 30 30 | 500 100 100 100 | 503 104 40 20";

#[test]
fn a_fixed_box_does_not_read_the_window_scroll_at_the_top() {
    check(FIXED, HELD_READ, HELD_AT_ANY_SCROLL);
}

/// The centred modal: `left:50%` + `translateX(-50%)`.
#[test]
fn a_transformed_fixed_box_stays_put_when_the_window_scrolls() {
    check(FIXED, "window.scrollTo(0, 500); box('m')", "300 10 200 50");
}

#[test]
fn a_plain_fixed_box_stays_put_when_the_window_scrolls() {
    check(FIXED, "window.scrollTo(0, 500); box('plain')", "7 9 20 20");
}

#[test]
fn what_is_under_a_fixed_box_stays_put_when_the_window_scrolls() {
    check(
        FIXED,
        &format!("window.scrollTo(0, 500); {HELD_READ}"),
        HELD_AT_ANY_SCROLL,
    );
}

/// Under a fixed box an element's own scroll still moves what it holds.
#[test]
fn an_element_scroll_under_a_fixed_box_still_moves_its_content() {
    check(
        FIXED,
        "window.scrollTo(0, 500); $('undertr').scrollTop = 7; $('fs').scrollTop = 7; \
         [box('undertr'), box('skid'), box('fs'), box('fsk')].join(' | ')",
        "301 302 100 100 | 301 295 30 30 | 500 100 100 100 | 503 97 40 20",
    );
}

/// `getClientRects` is the same read.
#[test]
fn get_client_rects_of_a_fixed_box_stays_put_too() {
    check(
        FIXED,
        "window.scrollTo(0, 500); var c = $('m').getClientRects()[0]; nums([c.x, c.y, c.width, c.height])",
        "300 10 200 50",
    );
}

/// A transform in the page's flow still goes with the page.
#[test]
fn a_transformed_box_in_the_flow_moves_with_the_window() {
    check(
        FIXED,
        "window.scrollTo(0, 500); [box('flow'), box('inner'), box('abs')].join(' | ')",
        "1 -498 50 50 | 10 -230 100 100 | 11 -228 10 10",
    );
}

const INLINES: &str = "<!doctype html><style>body{margin:0}</style>\
    <div style=\"width:300px\"><span id=sp style=\"transform:translate(30px,40px)\">text</span>\
    <span id=ib style=\"display:inline-block;width:20px;height:20px;transform:translate(30px,40px)\"></span>\
    </div>\
    <div id=fl style=\"display:flex;transform:translate(1px,2px)\"><span id=fi style=\"transform:translate(3px,4px)\">x</span></div>";

#[test]
fn a_transform_does_not_apply_to_a_plain_inline() {
    check(
        INLINES,
        "var r = $('sp').getBoundingClientRect(); [r.x - $('sp').offsetLeft, r.y - $('sp').offsetTop].map(Math.round).join(' ')",
        "0 0",
    );
}

#[test]
fn it_applies_to_an_inline_block_and_a_flex_item() {
    check(
        INLINES,
        "var r = $('ib').getBoundingClientRect(), f = $('fi').getBoundingClientRect(), l = $('fl').getBoundingClientRect(); \
         [r.x - $('ib').offsetLeft, r.y - $('ib').offsetTop, l.x - $('fl').offsetLeft, l.y - $('fl').offsetTop, f.x - l.x, f.y - l.y].map(Math.round).join(' ')",
        "30 40 1 2 3 4",
    );
}
