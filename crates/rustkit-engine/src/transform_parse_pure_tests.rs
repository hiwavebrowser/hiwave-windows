//! #853 C10b2: portable pins for `parse_transform` / 3D flatten / refuse-whole.
//!
//! The companion suite `transform_parse_tests` already covers these cases plus
//! getBoundingClientRect oracles, but the whole module is gated behind
//! `feature = "headless"`. Default Linux `cargo test -p rustkit-engine --lib`
//! (and develop-compile / non-headless CI) therefore never runs even the pure
//! matrix/refuse pins. A regression that accepts `translateX(5px) nonsense(1)`
//! half-way or drops `translate3d` would ship silently on every non-headless
//! run.

use super::*;

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

/// Pure parse pins for the #853 R1 calc/refuse path that the headless suite
/// only exercises via getBoundingClientRect after an earlier declaration.
#[test]
fn calc_angle_in_rotate_is_accepted() {
    same("rotate(calc(90deg))", [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    same("rotate(calc(45deg * 2))", [0.0, 1.0, -1.0, 0.0, 0.0, 0.0]);
    same("rotate(calc(90deg / 2))", {
        let a = (45.0_f32).to_radians();
        [a.cos(), a.sin(), -a.sin(), a.cos(), 0.0, 0.0]
    });
}

#[test]
fn non_angle_calc_in_rotate_is_refused() {
    for value in [
        "rotate(calc(90))",
        "rotate(calc(0))",
        "rotate(calc(45deg * 2deg))",
        "rotate(calc(45deg + 1))",
    ] {
        assert!(
            parse_transform(value).is_none(),
            "{value} should be refused"
        );
    }
}

#[test]
fn inf_and_nan_scale_are_refused() {
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
        assert!(
            parse_transform(value).is_none(),
            "{value} should be refused"
        );
    }
    same("scale(.5)", [0.5, 0.0, 0.0, 0.5, 0.0, 0.0]);
    same("scale(+2)", [2.0, 0.0, 0.0, 2.0, 0.0, 0.0]);
}
