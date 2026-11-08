//! Plan row C10 (b2): the 3D transform functions were not parsed, and a value
//! the parser did not know was skipped, so `translate3d(-50%,0,0)` did nothing
//! and `translateX(5px) nonsense(1)` kept its first half. With no perspective a
//! 3D transform flattens onto the page. Expected rects are Chromium 143.0.7499.4
//! for a 100x50 block at (20,10) (`trench/z/tools/c10/c10b2_probe.mjs` on the hub).

use super::*;

#[track_caller]
fn rect(transform: &str, want: &str) {
    let page = format!(
        "<!doctype html><style>html{{font-size:10px}}body{{margin:0}}\
         #b{{position:absolute;left:20px;top:10px;width:100px;height:50px;font-size:10px;\
         transform:{transform}}}</style><div id=b></div>"
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds {
            x: 0,
            y: 0,
            width: 800,
            height: 600,
        })
        .expect("view");
    engine.load_html(view, &page).expect("load");
    let got = engine
        .execute_script(
            view,
            "var r = document.getElementById('b').getBoundingClientRect(); \
             [r.x, r.y, r.width, r.height].map(function (v) { return +v.toFixed(3); }).join(' ')",
        )
        .expect("script");
    assert_eq!(got, format!("String(\"{want}\")"), "transform: {transform}");
}

/// What `transform` parsed to, as a matrix at this box's size.
#[track_caller]
fn matrix(value: &str) -> [f32; 6] {
    parse_transform(value)
        .unwrap_or_else(|| panic!("refused: {value}"))
        .to_matrix(100.0, 50.0)
}

#[track_caller]
fn same(value: &str, want: [f32; 6]) {
    let got = matrix(value);
    for (g, w) in got.iter().zip(want) {
        assert!((g - w).abs() < 1e-3, "{value}: {got:?}, want {want:?}");
    }
}

const ONE: [f32; 6] = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

#[test]
fn translate3d_flattens() {
    same("translate3d(5px,6px,0)", [1.0, 0.0, 0.0, 1.0, 5.0, 6.0]);
    same("translate3d(5px,6px,100px)", [1.0, 0.0, 0.0, 1.0, 5.0, 6.0]);
    same(
        "translate3d(-50%,-50%,0)",
        [1.0, 0.0, 0.0, 1.0, -50.0, -25.0],
    );
}

#[test]
fn translate_z_is_a_transform_that_moves_nothing() {
    same("translateZ(10px)", ONE);
    same("translateZ(0)", ONE);
}

#[test]
fn scale3d_drops_its_depth() {
    same("scale3d(2,3,1)", [2.0, 0.0, 0.0, 3.0, 0.0, 0.0]);
    same("scale3d(2,3,5)", [2.0, 0.0, 0.0, 3.0, 0.0, 0.0]);
    same("scaleZ(2)", ONE);
}

#[test]
fn a_rotation_about_z_is_a_rotation() {
    same("rotateZ(90deg)", [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    same("rotate3d(0,0,1,90deg)", [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    same("rotate3d(0,0,2,90deg)", [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
}

#[test]
fn a_rotation_about_x_or_y_foreshortens() {
    same("rotateX(60deg)", [1.0, 0.0, 0.0, 0.5, 0.0, 0.0]);
    same("rotateY(60deg)", [0.5, 0.0, 0.0, 1.0, 0.0, 0.0]);
    same("rotate3d(1,0,0,60deg)", [1.0, 0.0, 0.0, 0.5, 0.0, 0.0]);
    same("rotate3d(0,0,0,90deg)", ONE);
}

#[test]
fn matrix3d_flattens() {
    same(
        "matrix3d(1,0,0,0, 0,1,0,0, 0,0,1,0, 7,9,0,1)",
        [1.0, 0.0, 0.0, 1.0, 7.0, 9.0],
    );
    same(
        "matrix3d(2,0,0,0, 0,3,0,0, 0,0,1,0, 7,9,5,1)",
        [2.0, 0.0, 0.0, 3.0, 7.0, 9.0],
    );
}

#[test]
fn perspective_alone_moves_nothing() {
    same("perspective(500px)", ONE);
    same("perspective(none)", ONE);
    same(
        "perspective(500px) translateX(10px)",
        [1.0, 0.0, 0.0, 1.0, 10.0, 0.0],
    );
}

#[test]
fn a_percentage_scales() {
    same("scale(50%)", [0.5, 0.0, 0.0, 0.5, 0.0, 0.0]);
    same("scale(50%,200%)", [0.5, 0.0, 0.0, 2.0, 0.0, 0.0]);
    same("scaleX(50%)", [0.5, 0.0, 0.0, 1.0, 0.0, 0.0]);
    same("scale3d(50%,50%,1)", [0.5, 0.0, 0.0, 0.5, 0.0, 0.0]);
}

#[test]
fn function_names_ignore_case() {
    same("TRANSLATE(5px,6px)", [1.0, 0.0, 0.0, 1.0, 5.0, 6.0]);
}

#[test]
fn a_bad_value_refuses_the_whole_declaration() {
    for value in [
        "translate3d(5px,6px)",
        "translateZ(10%)",
        "scale3d(2,3)",
        "matrix3d(1,0,0,1)",
        "translate3d(5px 6px 0)",
        "translate(5px 6px)",
        "scale(2 3)",
        "rotate(90)",
        "translate(5,6)",
        "translateX(5px) nonsense(1)",
        "nonsense(1) translateX(5px)",
        "translateX(5px) nonsense",
        "translateX(5px",
        "translate (5px)",
        "translate()",
    ] {
        assert!(
            parse_transform(value).is_none(),
            "{value} should be refused"
        );
    }
}

#[test]
fn zero_needs_no_unit() {
    same("rotate(0)", ONE);
    same("translate(0,0)", ONE);
}

#[test]
fn the_box_reads_the_flattened_rect() {
    rect("translate3d(-50%,-50%,0)", "-30 -15 100 50");
    rect("rotateY(60deg)", "45 10 50 50");
    rect("rotateX(60deg)", "20 22.5 100 25");
    rect("scale3d(2,3,5)", "-30 -40 200 150");
    rect(
        "matrix3d(2,0,0,0, 0,3,0,0, 0,0,1,0, 7,9,5,1)",
        "-23 -31 200 150",
    );
}

#[test]
fn a_refused_value_leaves_the_box_alone() {
    rect("translateX(5px) nonsense(1)", "20 10 100 50");
}

#[test]
fn calc_in_a_translate() {
    rect("translate(calc(10px + 5%),0)", "35 10 100 50");
    rect("translateX(calc(100% - 10px))", "110 10 100 50");
}

/// Known gap: `to_matrix` resolves against 16px and has no viewport. It reads
/// `36 26` and `20 10` here.
#[test]
#[ignore = "transform lengths in em, rem, vw and vh do not see the element or the viewport"]
fn font_and_viewport_units_in_a_translate() {
    rect("translate(1em,1rem)", "30 20 100 50");
    rect("translate(1vw,1vh)", "28 16 100 50");
}

/// Known gap: depth is not kept, so what perspective does to a z is lost.
#[test]
#[ignore = "perspective with depth is not an affine the 2D painter can hold"]
fn perspective_with_depth() {
    rect("perspective(500px) translateZ(100px)", "7.5 3.75 125 62.5");
    rect(
        "perspective(500px) rotateY(60deg)",
        "42.63 7.63 50.378 54.741",
    );
}

/// The rect of a 100x50 box at (20,10) when this value follows
/// `transform: translateX(7px)` in a parent with `translateY(3px)`: refused
/// reads `27 13 100 50`, as the earlier declaration stays. Chromium 143
/// (`trench/z/tools/c10/c10b2_r1_probe.mjs`, `c10b2_var_probe.mjs`).
#[track_caller]
fn after_an_earlier_one(value: &str, want: &str) {
    let page = format!(
        "<!doctype html><style>body{{margin:0}}#w{{transform:translateY(3px)}}\
         #b{{--i:2;--a:90deg;position:absolute;left:20px;top:10px;width:100px;height:50px;\
         transform:translateX(7px);transform:{value}}}</style><div id=w><div id=b></div></div>"
    );
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, &page).expect("load");
    let got = engine
        .execute_script(
            view,
            "var r = document.getElementById('b').getBoundingClientRect(); \
             [r.x, r.y, r.width, r.height].map(function (v) { return +v.toFixed(3); }).join(' ')",
        )
        .expect("script");
    assert_eq!(got, format!("String(\"{want}\")"), "transform: {value}");
}

const KEPT: &str = "27 13 100 50";
const ROTATED_45: &str = "16.967 -15.033 106.066 106.066";
const ROTATED_90: &str = "45 -12 50 100";
const NONE: &str = "20 13 100 50";

#[test]
fn perspective_takes_no_negative_length_and_no_bare_number() {
    for value in ["perspective(-10px)", "perspective(10)", "perspective(10%)"] {
        after_an_earlier_one(value, KEPT);
    }
    for value in [
        "perspective(0)",
        "perspective(10px)",
        "perspective(+10px)",
        "perspective(-0px)",
        "perspective(calc(-10px))",
        "perspective(calc(10px))",
        "perspective(none)",
        "perspective(NONE)",
    ] {
        after_an_earlier_one(value, NONE);
    }
}

#[test]
fn calc_in_an_angle() {
    for value in [
        "rotate(calc(-1 * 45deg))",
        "rotate(calc(45deg))",
        "rotate(calc(0.25turn + 45deg))",
        "rotate(calc(90deg / 2))",
    ] {
        after_an_earlier_one(value, ROTATED_45);
    }
    for value in [
        "rotate(calc(2 * (10deg + 35deg)))",
        "rotate(min(90deg, 180deg))",
        "rotate(max(90deg, 45deg))",
        "rotate(clamp(10deg, 90deg, 180deg))",
        "rotate(CALC(90DEG))",
        "rotate(calc( 90deg ))",
    ] {
        after_an_earlier_one(value, ROTATED_90);
    }
    after_an_earlier_one("rotate(calc(1rad - 1rad))", NONE);
    after_an_earlier_one("skew(calc(45deg))", "-5 13 150 50");
    after_an_earlier_one("rotateX(calc(60deg))", "20 25.5 100 25");
}

/// An infinite angle is no rotation.
#[test]
fn an_angle_divided_by_zero_turns_nothing() {
    after_an_earlier_one("rotate(calc(90deg / 0))", NONE);
}

#[test]
fn calc_that_is_not_an_angle_is_refused() {
    for value in [
        "rotate(calc(90))",
        "rotate(calc(0))",
        "rotate(calc(45deg * 2deg))",
        "rotate(calc(45deg + 1))",
        "rotate(calc(90deg+45deg))",
    ] {
        after_an_earlier_one(value, KEPT);
    }
}

#[test]
fn calc_in_a_number() {
    after_an_earlier_one("scale(calc(1.5))", "-5 0.5 150 75");
    after_an_earlier_one("scale(calc(1 + 0.5))", "-5 0.5 150 75");
    after_an_earlier_one("scale(calc(2 * 1.5), calc(4 / 2))", "-80 -12 300 100");
    after_an_earlier_one("scale(calc(50%))", "45 25.5 50 25");
    after_an_earlier_one("scale(min(2, 3))", "-30 -12 200 100");
    after_an_earlier_one("scaleX(calc(2))", "-30 13 200 50");
    after_an_earlier_one("scale3d(calc(2),1,1)", "-30 13 200 50");
    after_an_earlier_one("matrix(calc(2),0,0,1,0,0)", "-30 13 200 50");
    after_an_earlier_one("rotate3d(0,0,calc(1),90deg)", ROTATED_90);
    after_an_earlier_one("scale(calc(1.5deg))", KEPT);
}

#[test]
fn a_list_keeps_all_of_itself_with_calc_in_it() {
    after_an_earlier_one("translate(-50%,-50%) rotate(calc(2 * 45deg))", "-5 -37 50 100");
    after_an_earlier_one(
        "translate(-50%, -50%) rotate(calc(-1 * 45deg)) scale(calc(1 + 1))",
        "-86.066 -93.066 212.132 212.132",
    );
    after_an_earlier_one(
        "translate(-50%,-50%) rotate(calc(var(--i) * 30deg))",
        "-26.651 -42.801 93.301 111.603",
    );
    after_an_earlier_one("rotate(var(--a))", ROTATED_90);
    after_an_earlier_one("scale(var(--i))", "-30 -12 200 100");
}

#[test]
fn units_ignore_case() {
    after_an_earlier_one("translate(5PX,6Px)", "25 19 100 50");
    after_an_earlier_one("translateX(5PX)", "25 13 100 50");
    after_an_earlier_one("rotate(45DEG)", ROTATED_45);
    after_an_earlier_one("rotate(0.25TURN)", ROTATED_90);
    after_an_earlier_one("rotate(100GRAD)", ROTATED_90);
    after_an_earlier_one("rotate(1RAD)", "21.948 -17.581 96.104 111.162");
    after_an_earlier_one("scale(1E1)", "-430 -212 1000 500");
    after_an_earlier_one("translateX(1E1px)", "30 13 100 50");
}

#[test]
fn keywords_ignore_case_and_take_the_earlier_one_away() {
    for value in ["NONE", "None", "initial", "INITIAL", "unset", "revert", "revert-layer"] {
        after_an_earlier_one(value, NONE);
    }
    after_an_earlier_one("inherit", "20 16 100 50");
}

#[test]
fn infinity_and_nan_are_not_numbers() {
    for value in [
        "scale(inf)",
        "scale(infinity)",
        "scale(NaN)",
        "scale(nan)",
        "scale(-inf)",
        "matrix(inf,0,0,1,0,0)",
        "rotate(infdeg)",
        "rotate(nandeg)",
        "translateX(infpx)",
        "rotate3d(inf,0,1,90deg)",
        "scale(5.)",
    ] {
        after_an_earlier_one(value, KEPT);
    }
    after_an_earlier_one("scale(.5)", "45 25.5 50 25");
    after_an_earlier_one("scale(+2)", "-30 -12 200 100");
}

/// A depth has no box side to be a part of, so a percentage anywhere in its
/// `calc()` refuses it, even one that sums to nothing. Chromium 143
/// (`trench/z/tools/c10/depth_persp_probe.mjs` on the hub).
#[test]
fn a_depth_refuses_a_percentage_inside_calc() {
    for value in [
        "translateZ(10%)",
        "translateZ(calc(10%))",
        "translateZ(calc(10% + 1px))",
        "translateZ(calc(0% + 0px))",
        "translateZ(calc(5% - 5%))",
        "translateZ(calc(1))",
        "translateZ(1)",
        "translate3d(1px,2px,3%)",
        "translate3d(1px,2px,calc(3%))",
        "translate3d(1px,2px,calc(3% + 1px))",
        "translate3d(0,0,calc(0%))",
        "perspective(10%)",
        "perspective(calc(10%))",
    ] {
        assert!(parse_transform(value).is_none(), "{value} is refused in Chromium");
    }
}

#[test]
fn a_depth_takes_calc_with_no_percentage() {
    for value in ["translateZ(calc(10px + 1px))", "translateZ(calc(1vw))", "translateZ(calc(1px * 2))", "translate3d(0,0,calc(1em))"] {
        same(value, ONE);
    }
    same("translate3d(1px,2px,calc(3px + 1px))", [1.0, 0.0, 0.0, 1.0, 1.0, 2.0]);
    same("translate3d(10%,20%,0)", [1.0, 0.0, 0.0, 1.0, 10.0, 10.0]);
    same("translate3d(calc(10% + 1px),0,0)", [1.0, 0.0, 0.0, 1.0, 11.0, 0.0]);
}

/// A plain negative length is refused by its sign, whatever its unit; the
/// viewport is not needed to know `-1vw` is below zero.
#[test]
fn a_negative_perspective_is_refused_in_any_unit() {
    for value in ["perspective(-1px)", "perspective(-1vw)", "perspective(-1vh)", "perspective(-1vmin)", "perspective(-1vmax)", "perspective(-1em)", "perspective(-1rem)"] {
        assert!(parse_transform(value).is_none(), "{value} is refused in Chromium");
    }
}

/// A `calc()` is clamped to zero when used, not refused, whatever it comes to.
#[test]
fn a_perspective_in_calc_is_taken_whatever_it_comes_to() {
    for value in [
        "perspective(calc(100px))",
        "perspective(calc(1vw - 100px))",
        "perspective(calc(100px - 1vw))",
        "perspective(calc(-1px))",
        "perspective(calc(1em - 100px))",
        "perspective(1vw)",
        "perspective(0)",
        "perspective(none)",
    ] {
        same(value, ONE);
    }
}

