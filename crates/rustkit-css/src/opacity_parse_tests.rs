//! Portable `parse_opacity` pins for #861.
//!
//! The headless `opacity_value_tests` suite covers cascade + paint but is
//! gated behind `feature = "headless"`, so default Linux `--lib` never sees
//! percentage / calc / float-range edges. A regression that only breaks the
//! pure parser (Tailwind's `opacity: 50%`, mixed `calc(0 + 50%)`, `1.e40`)
//! would ship green on every non-headless run.

use crate::parse_opacity;

fn check(cases: &[(&str, Option<f32>)]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|&(input, want)| {
            let got = parse_opacity(input);
            let same = match (got, want) {
                (None, None) => true,
                (Some(a), Some(b)) => (a - b).abs() < 1e-4,
                _ => false,
            };
            (!same).then(|| format!("{input}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "{} wrong:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}

#[test]
fn a_percentage_is_a_hundredth_clamped_when_used() {
    check(&[
        ("50%", Some(0.5)),
        ("99%", Some(0.99)),
        ("100%", Some(1.0)),
        ("0%", Some(0.0)),
        ("150%", Some(1.0)),
        ("-10%", Some(0.0)),
        ("12.5%", Some(0.125)),
        ("5e1%", Some(0.5)),
        ("+50%", Some(0.5)),
        (".5%", Some(0.005)),
    ]);
}

#[test]
fn a_number_still_reads_as_a_number() {
    check(&[
        (".5", Some(0.5)),
        ("0", Some(0.0)),
        ("1", Some(1.0)),
        ("2", Some(1.0)),
        ("-1", Some(0.0)),
        ("+.5", Some(0.5)),
        ("1e-1", Some(0.1)),
    ]);
}

#[test]
fn what_is_not_a_css_number_or_percentage_is_dropped() {
    check(&[
        ("50 %", None),
        ("50%%", None),
        ("%", None),
        ("1.", None),
        (".", None),
        ("1e", None),
        ("1px", None),
        ("inf", None),
        ("NaN", None),
    ]);
}

#[test]
fn calc_of_a_number_or_of_a_percentage() {
    check(&[
        ("calc(50%)", Some(0.5)),
        ("CALC(50%)", Some(0.5)),
        ("calc(0.5)", Some(0.5)),
        ("calc(25% + 25%)", Some(0.5)),
        ("calc(0.25 + 0.25)", Some(0.5)),
        ("calc(50% * 2)", Some(1.0)),
        ("calc(1 / 2)", Some(0.5)),
        ("calc(150%)", Some(1.0)),
        ("calc(-1)", Some(0.0)),
    ]);
}

#[test]
fn calc_that_mixes_or_has_a_unit_is_dropped() {
    check(&[
        ("calc(0.5 + 50%)", None),
        ("calc(0 + 50%)", None),
        ("calc(50% + 0)", None),
        ("calc((0 + 50%) * 1)", None),
        ("calc(25%+25%)", None),
        ("calc(1px)", None),
        ("calc(50%) foo", None),
        ("calc()", None),
        ("calc(50% * 2%)", None),
    ]);
}

#[test]
fn tabs_and_newlines_in_calc_are_whitespace() {
    check(&[
        ("calc(25%\t+\t25%)", Some(0.5)),
        ("calc(25%\n+\n25%)", Some(0.5)),
        ("calc(\n  25% + 25%\n)", Some(0.5)),
        ("calc(.25\t+\t.25)", Some(0.5)),
    ]);
}

#[test]
fn tabs_and_newlines_do_not_excuse_what_calc_refuses() {
    check(&[
        ("calc(0\t+\t50%)", None),
        ("calc(0\n+\n50%)", None),
        ("calc(50%\t+\t0)", None),
        ("calc(25%\t+25%)", None),
        ("50\t%", None),
        ("50\n%", None),
    ]);
}

#[test]
fn a_number_past_the_float_range_clamps() {
    check(&[
        ("1e40", Some(1.0)),
        ("1e40%", Some(1.0)),
        ("-1e40", Some(0.0)),
        ("1e-400", Some(0.0)),
        ("calc(1e40)", Some(1.0)),
        ("calc(1e40 * 1e-40)", Some(0.0340282)),
        ("calc((1e40 + 1e40) * 0)", Some(0.0)),
        ("calc(1 / 1e-40)", Some(1.0)),
    ]);
}

#[test]
fn whitespace_before_a_percent_in_calc_is_dropped() {
    check(&[
        ("calc(50 %)", None),
        ("calc(50\t%)", None),
        ("calc(25% + 25 %)", None),
    ]);
}

#[test]
fn a_number_ending_in_a_dot_before_its_exponent_is_dropped_in_calc() {
    check(&[
        ("calc(1.e40)", None),
        ("calc(-1.e40)", None),
        ("calc(1.e40%)", None),
        ("calc(1.e40 * 1e-40)", None),
    ]);
}
