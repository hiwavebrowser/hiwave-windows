//! Portable #862 depth/%-in-calc and negative-perspective refuse pins.
//!
//! Open #859 covers flatten/refuse for the general 3D parse suite but not
//! these Chromium edges: a percentage anywhere in a depth `calc()`, and a
//! plain negative perspective in any unit. Headless `transform_parse_tests`
//! owns the oracles; default Linux `--lib` never compiles them.

use super::*;

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
        assert!(
            parse_transform(value).is_none(),
            "{value} is refused in Chromium"
        );
    }
}

#[test]
fn a_negative_perspective_is_refused_in_any_unit() {
    for value in [
        "perspective(-1px)",
        "perspective(-1vw)",
        "perspective(-1vh)",
        "perspective(-1vmin)",
        "perspective(-1vmax)",
        "perspective(-1em)",
        "perspective(-1rem)",
    ] {
        assert!(
            parse_transform(value).is_none(),
            "{value} is refused in Chromium"
        );
    }
}

#[test]
fn a_depth_takes_calc_with_no_percentage() {
    for value in [
        "translateZ(calc(10px + 1px))",
        "translateZ(calc(1vw))",
        "translateZ(calc(1px * 2))",
        "translate3d(0,0,calc(1em))",
    ] {
        assert!(
            parse_transform(value).is_some(),
            "{value} must parse"
        );
    }
}
