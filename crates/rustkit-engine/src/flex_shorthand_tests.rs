//! The `flex` shorthand (css-flexbox-1 §7.1.1), Castor A1 item 2. The keywords
//! `none`, `auto` and `initial` and a lone `<length>` were no-ops, and a basis
//! written first (`10px 2 3`) read the third number as the basis or was dropped.
//! Expected values are Chromium 143.0.7499.4 (`trench/z/tools/di/flex_probe.mjs`
//! on the hub), each set over `flex: 7 8 9px` so a dropped declaration shows.

use super::*;
use rustkit_css::FlexBasis::{self, Auto, Content, Length, Percent};

fn applied(value: &str) -> (f32, f32, FlexBasis) {
    let engine = Engine::new(EngineConfig::default()).expect("engine");
    let mut s = ComputedStyle::default();
    engine.lctx.apply_style_property(&mut s, "flex", "7 8 9px");
    engine.lctx.apply_style_property(&mut s, "flex", value);
    (s.flex_grow, s.flex_shrink, s.flex_basis)
}

#[track_caller]
fn check(cases: &[(&str, (f32, f32, FlexBasis))]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(value, want)| {
            let got = applied(value);
            (got != *want).then(|| format!("flex: {value}: got {got:?}, want {want:?}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn the_keywords_set_all_three_longhands() {
    check(&[
        ("none", (0.0, 0.0, Auto)),
        ("NONE", (0.0, 0.0, Auto)),
        ("auto", (1.0, 1.0, Auto)),
        ("Auto", (1.0, 1.0, Auto)),
        ("initial", (0.0, 1.0, Auto)),
        ("unset", (0.0, 1.0, Auto)),
        ("content", (1.0, 1.0, Content)),
    ]);
}

#[test]
fn a_lone_basis_grows_and_shrinks_by_one() {
    check(&[("10px", (1.0, 1.0, Length(10.0))), ("50%", (1.0, 1.0, Percent(50.0)))]);
}

#[test]
fn a_basis_may_come_first() {
    check(&[
        ("10px 2", (2.0, 1.0, Length(10.0))),
        ("10px 2 3", (2.0, 3.0, Length(10.0))),
        ("10px 1 0", (1.0, 0.0, Length(10.0))),
        ("0px 1 1", (1.0, 1.0, Length(0.0))),
        ("auto 2", (2.0, 1.0, Auto)),
        ("auto 2 3", (2.0, 3.0, Auto)),
        ("content 2 3", (2.0, 3.0, Content)),
    ]);
}

/// These already worked; the rewrite keeps them.
#[test]
fn the_factor_first_forms_stay_as_they_were() {
    check(&[
        ("0", (0.0, 1.0, Percent(0.0))),
        ("1", (1.0, 1.0, Percent(0.0))),
        ("2 3", (2.0, 3.0, Percent(0.0))),
        ("1 0", (1.0, 0.0, Percent(0.0))),
        ("2 10px", (2.0, 1.0, Length(10.0))),
        ("2 3 10px", (2.0, 3.0, Length(10.0))),
        ("0 0 0", (0.0, 0.0, Length(0.0))),
        ("1 1 0", (1.0, 1.0, Length(0.0))),
        ("2 auto", (2.0, 1.0, Auto)),
        ("2 3 auto", (2.0, 3.0, Auto)),
        ("1e1", (10.0, 1.0, Percent(0.0))),
        ("+1", (1.0, 1.0, Percent(0.0))),
        (".5 10px", (0.5, 1.0, Length(10.0))),
    ]);
}

/// An invalid value drops the whole declaration.
#[test]
fn an_invalid_value_leaves_the_earlier_declaration() {
    let earlier = (7.0, 8.0, Length(9.0));
    let cases: Vec<(&str, (f32, f32, FlexBasis))> = [
        "0 2 3", "1 2 3", "2 10px 3", "10px 20px", "none 1", "1 none", "none auto", "bogus",
        "-1", "1 -1", "10px -1", "1 1 1 1", "1 -5px", "nan", "inf",
    ]
    .iter()
    .map(|v| (*v, earlier))
    .collect();
    check(&cases);
}

/// What it does to a row: `flex: auto` takes the free space, `flex: none`
/// refuses to shrink, a lone length is a basis that still grows. The expected
/// string is Chromium 143 on this page (`flex_row_probe.mjs`).
#[test]
fn the_keywords_change_the_row() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine
        .load_html(
            view,
            concat!(
                "<!doctype html><style>body{margin:0} .r{display:flex;width:400px}",
                " .r div{height:10px} #a{width:100px;flex:none} #b{flex:auto}",
                " #d,#e{width:500px;flex:none} #f,#g{flex:10px}</style><body>",
                "<div class=r><div id=a>a</div><div id=b>b</div></div>",
                "<div class=r><div id=d>d</div><div id=e>e</div></div>",
                "<div class=r><div id=f>f</div><div id=g>g</div></div></body>",
            ),
        )
        .expect("load");
    let places = "[].map.call(document.querySelectorAll('[id]'), function (e) { \
         var r = e.getBoundingClientRect(); return e.id + ':' + r.x + ',' + r.width; }).join(' ')";
    assert_eq!(
        engine.execute_script(view, places).expect("script"),
        "String(\"a:0,100 b:100,300 d:0,500 e:500,500 f:0,200 g:200,200\")"
    );
}
