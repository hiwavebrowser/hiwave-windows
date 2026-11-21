//! #855: portable pins for the individual `translate` property parser.
//!
//! `translate: 0px calc(-100% - 1px)` (Tailwind / Shopify) must split at spaces
//! outside functions, and a depth that is not a bare `<length>` must refuse the
//! whole value (Chromium 143). The only companions today live in headless
//! `transform_rect_tests::translate_*`, so default Linux `--lib` never sees
//! `split_outside_functions` or `parse_individual_translate`.

use super::*;
use rustkit_css::{Length, TransformOp};

#[track_caller]
fn op(value: &str) -> TransformOp {
    match parse_individual_translate(value) {
        Some(Some(op)) => op,
        Some(None) => panic!("got none for {value}"),
        None => panic!("refused: {value}"),
    }
}

#[track_caller]
fn matrix(value: &str) -> [f32; 6] {
    let list = rustkit_css::TransformList {
        ops: vec![op(value)],
    };
    list.to_matrix(100.0, 50.0)
}

#[test]
fn split_outside_functions_keeps_calc_spaces() {
    assert_eq!(
        split_outside_functions("0px calc(-100% - 1px)"),
        vec!["0px", "calc(-100% - 1px)"]
    );
    assert_eq!(
        split_outside_functions("calc(10px + 5%) 0"),
        vec!["calc(10px + 5%)", "0"]
    );
    assert_eq!(
        split_outside_functions("calc( 1px + 1px ) 3px 0px"),
        vec!["calc( 1px + 1px )", "3px", "0px"]
    );
    assert_eq!(
        split_outside_functions("min(1px, 2px) max(3px, 4px)"),
        vec!["min(1px, 2px)", "max(3px, 4px)"]
    );
    assert_eq!(split_outside_functions("1px  2px   3px"), vec!["1px", "2px", "3px"]);
}

#[test]
fn translate_takes_a_function_with_spaces_in_it() {
    let got = matrix("0px calc(-100% - 1px)");
    assert!(
        (got[4] - 0.0).abs() < 1e-3 && (got[5] - (-51.0)).abs() < 1e-3,
        "got {got:?}, want tx=0 ty=-51"
    );
    let got = matrix("calc(10px + 5%) 0");
    assert!(
        (got[4] - 15.0).abs() < 1e-3 && (got[5] - 0.0).abs() < 1e-3,
        "got {got:?}, want tx=15 ty=0"
    );
    let got = matrix("calc(1px + 1px) calc(2px * 2) 0");
    assert!(
        (got[4] - 2.0).abs() < 1e-3 && (got[5] - 4.0).abs() < 1e-3,
        "got {got:?}, want tx=2 ty=4"
    );
}

#[test]
fn translate_flattens_a_length_depth() {
    assert_eq!(
        op("1px 2px 3px"),
        TransformOp::Translate(Length::Px(1.0), Length::Px(2.0))
    );
    assert_eq!(
        op("1px 2px calc(1px + 1px)"),
        TransformOp::Translate(Length::Px(1.0), Length::Px(2.0))
    );
    assert_eq!(
        op("10% 10% 3px"),
        TransformOp::Translate(Length::Percent(10.0), Length::Percent(10.0))
    );
    // One value sets x; y is zero.
    assert_eq!(op("5px"), TransformOp::Translate(Length::Px(5.0), Length::Zero));
}

#[test]
fn translate_refuses_a_depth_that_is_not_a_length() {
    for z in [
        "10%",
        "auto",
        "fit-content",
        "1",
        "2.5",
        "calc(10%)",
        "calc(1px + 10%)",
        "min(1px, 10%)",
    ] {
        let value = format!("1px 2px {z}");
        assert!(
            parse_individual_translate(&value).is_none(),
            "{value} should be refused"
        );
    }
    // A length depth (including calc that is a length, and zero) is accepted
    // and flattened away.
    for z in ["0", "0px", "3px", "calc(1px + 1px)", "+0"] {
        let value = format!("1px 2px {z}");
        assert!(
            parse_individual_translate(&value).is_some(),
            "{value} should be accepted"
        );
    }
}

#[test]
fn translate_takes_three_values_at_most() {
    assert!(parse_individual_translate("1px 2px 3px 4px").is_none());
    assert!(parse_individual_translate("calc(1px + 1px) calc(2px * 2) 0 0").is_none());
    assert_eq!(parse_individual_translate("none"), Some(None));
    assert_eq!(parse_individual_translate("NONE"), Some(None));
    assert!(parse_individual_translate("").is_none());
}
