//! A sibling combinator left of a descendant or child step (Z row S7, the
//! eBay checkbox from Pete's hand test). eBay's skin draws a checkbox as the
//! `<input>` followed by an icon span holding two svgs, and hides one with
//! `.ctrl:checked + .icon .off` / `.ctrl:not(:checked) + .icon .on`. The
//! `+` there relates the icon span (an ANCESTOR of the subject) to its
//! earlier sibling; the matcher tested the subject's own earlier siblings
//! instead, so neither rule matched and both icons showed on every box.
//! The same walk also tested every chained sibling step against the same
//! sibling (`.t + .u + .v`, `.a ~ .bb ~ .c`), and left `:not(:checked)` on
//! an earlier sibling permissive.
//!
//! The rows are `getComputedStyle(el).display` from Chrome 143.0.7499.4 (the
//! parity oracle's chromium-1200) for this page, at load and after a script
//! sets `checked` on five controls (probe and page:
//! `trench/z/tools/s7/checked_sibling_probe.mjs` on the hub).

use super::*;

const PAGE: &str = r#"<!doctype html>
<html><head><style>
body { margin: 0; font: 16px/20px monospace }
i, b { display: block; width: 10px; height: 10px }
.cb__ctrl:checked + .cb__icon .cb__off,
.cb__ctrl:not(:checked) + .cb__icon .cb__on { display: none }
.r:checked + .lab > .mark { display: none }
.x:checked ~ .w .deep { display: none }
.k + .m .n { display: none }
.t + .u + .v { display: none }
.a ~ .bb ~ .c { display: none }
.y:not(:checked) + .z { display: none }
</style></head><body>
<span class="cb"><input class="cb__ctrl" type="checkbox" id="c1"><span class="cb__icon"><i class="cb__off" id="off1"></i><i class="cb__on" id="on1"></i></span></span>
<span class="cb"><input class="cb__ctrl" type="checkbox" id="c2" checked><span class="cb__icon"><i class="cb__off" id="off2"></i><i class="cb__on" id="on2"></i></span></span>
<div><input class="r" type="radio" id="r1" checked><span class="lab"><i class="mark" id="mark1"></i></span></div>
<div><input class="r" type="radio" id="r2"><span class="lab"><i class="mark" id="mark2"></i></span></div>
<div><input class="x" type="checkbox" id="x1" checked><p></p><div class="w"><p><i class="deep" id="deep1"></i></p></div></div>
<div><input class="x" type="checkbox" id="x2"><p></p><div class="w"><p><i class="deep" id="deep2"></i></p></div></div>
<div><div class="m"><i class="k"></i><i class="n" id="n1"></i></div></div>
<div><i class="k"></i><div class="m"><i class="n" id="n2"></i></div></div>
<div><b class="t"></b><b class="u"></b><b class="v" id="v1"></b></div>
<div><b class="t"></b><b class="q"></b><b class="u"></b><b class="v" id="v2"></b></div>
<div><b class="a"></b><b class="bb"></b><b class="c" id="g1"></b></div>
<div><b class="bb"></b><b class="a"></b><b class="c" id="g2"></b></div>
<div><input class="y" type="checkbox" id="y1"><b class="z" id="z1"></b></div>
<div><input class="y" type="checkbox" id="y2" checked><b class="z" id="z2"></b></div>
</body></html>"#;

const IDS: &[&str] = &[
    "off1", "on1", "off2", "on2", "mark1", "mark2", "deep1", "deep2", "n1", "n2", "v1", "v2", "g1", "g2", "z1", "z2",
];

/// Chrome 143 at load.
const CHROME_LOAD: &str = "off1:block on1:none off2:none on2:block mark1:none mark2:block deep1:none deep2:block \
    n1:block n2:none v1:none v2:block g1:none g2:block z1:none z2:block";

/// Chrome 143 after `c1`, `r2`, `x2` are checked and `c2`, `y2` unchecked by script.
const CHROME_TOGGLED: &str = "off1:none on1:block off2:block on2:none mark1:none mark2:none deep1:none deep2:none \
    n1:block n2:none v1:none v2:block g1:none g2:block z1:none z2:none";

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 400, 600))
        .expect("headless view");
    engine.load_html(view, PAGE).expect("load_html");
    (engine, view)
}

fn eval(engine: &mut Engine, view: EngineViewId, js: &str) -> String {
    let value = engine.execute_script(view, js).expect("script");
    value
        .strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&value)
        .to_string()
}

fn displays(engine: &mut Engine, view: EngineViewId) -> String {
    let ids = IDS.iter().map(|id| format!("'{id}'")).collect::<Vec<_>>().join(",");
    eval(
        engine,
        view,
        &format!(
            "[{ids}].map(function (id) {{ return id + ':' + getComputedStyle(document.getElementById(id)).display; }}).join(' ')"
        ),
    )
}

/// The rows that differ from Chrome's, `id: got (want)`.
fn wrong(got: &str, want: &str) -> Vec<String> {
    let want: Vec<&str> = want.split_whitespace().collect();
    got.split_whitespace()
        .zip(want.iter())
        .filter(|(g, w)| g != *w)
        .map(|(g, w)| format!("{g} (Chrome {w})"))
        .collect()
}

#[test]
fn sibling_steps_above_the_subject_match_as_chrome_at_load() {
    let (mut engine, view) = page();
    let got = displays(&mut engine, view);
    let bad = wrong(&got, CHROME_LOAD);
    assert!(bad.is_empty(), "{} of {} rows differ from Chrome 143: {bad:?}", bad.len(), IDS.len());
}

#[test]
fn sibling_steps_above_the_subject_follow_script_checkedness() {
    let (mut engine, view) = page();
    eval(
        &mut engine,
        view,
        "['c1','r2','x2'].forEach(function (id) { document.getElementById(id).checked = true; });\
         ['c2','y2'].forEach(function (id) { document.getElementById(id).checked = false; }); ''",
    );
    let got = displays(&mut engine, view);
    let bad = wrong(&got, CHROME_TOGGLED);
    assert!(bad.is_empty(), "{} of {} rows differ from Chrome 143: {bad:?}", bad.len(), IDS.len());
}

/// Milliseconds for the fastest of two loads of `n` list items, each
/// `<li><span>`, styled by `rule`.
fn list_load_ms(n: usize, rule: &str) -> u128 {
    let items = "<li><span>x</span></li>".repeat(n);
    let html = format!("<!doctype html><html><head><style>{rule}</style></head><body><ul>{items}</ul></body></html>");
    (0..2)
        .map(|_| {
            let mut engine = Engine::new(EngineConfig::default()).expect("engine");
            let view = engine
                .create_headless_view(Bounds::new(0, 0, 400, 600))
                .expect("headless view");
            let start = std::time::Instant::now();
            engine.load_html(view, &html).expect("load_html");
            start.elapsed().as_millis()
        })
        .min()
        .unwrap()
}

#[test]
fn sibling_steps_above_the_subject_stay_linear_in_the_sibling_count() {
    // #878 batch 13 hold: a sibling step above the subject read EVERY
    // earlier sibling of the ancestor, cloned, for each subject, so 3000
    // `li` under `li ~ li span` built in 1160 ms against 32 ms on develop
    // (6000: 4669 vs 62). Each shape must stay within 2x of the same page
    // with a rule that has no sibling step: `+` reads the one element
    // before, a `~` that matches stops at the nearest, one that never
    // matches is not rescanned per subject.
    const N: usize = 3000;
    let control = list_load_ms(N, "ul li span { color: red }");
    for rule in [
        "li ~ li span { color: red }",
        "li + li span { color: red }",
        ".absent ~ li span { color: red }",
        "li ~ li > span { color: red }",
    ] {
        let ms = list_load_ms(N, rule);
        eprintln!("{N} siblings: `{rule}` {ms} ms, control {control} ms");
        assert!(
            ms <= control * 2 + 50,
            "`{rule}` on {N} siblings: {ms} ms, control (no sibling step) {control} ms"
        );
    }
}

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn a_click_on_the_box_swaps_its_icon() {
    // What Pete does on the page: click an unchecked box. Its unchecked
    // icon goes and its checked icon shows. The control is a block here:
    // an inline checkbox inside an inline span (PAGE's shape) is not found
    // by the hit test at its centre (`elementFromPoint` reads BODY), a
    // separate gap this change does not touch.
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 400, 600))
        .expect("headless view");
    let click_page = PAGE.replace(
        r#"<input class="cb__ctrl" type="checkbox" id="c1">"#,
        r#"<input class="cb__ctrl" type="checkbox" id="c1" style="display:block;width:30px;height:30px;margin:0">"#,
    );
    assert_ne!(click_page, PAGE);
    engine.load_html(view, &click_page).expect("load_html");
    let at = eval(
        &mut engine,
        view,
        "(function () { var r = document.getElementById('c1').getBoundingClientRect(); \
         return (r.left + r.width / 2) + ',' + (r.top + r.height / 2); })()",
    );
    let (x, y) = at.split_once(',').expect("point");
    let (x, y): (f32, f32) = (x.parse().unwrap(), y.parse().unwrap());
    let hit = eval(
        &mut engine,
        view,
        &format!(
            "(function () {{ var e = document.elementFromPoint({x}, {y}); \
             return e ? e.tagName + '.' + e.className + '#' + e.id : 'none'; }})()"
        ),
    );
    assert_eq!(hit, "INPUT.cb__ctrl#c1", "elementFromPoint at the box's centre {at}");
    engine.click_at_point(view, x, y);
    let got = eval(
        &mut engine,
        view,
        "document.getElementById('c1').checked + ' ' + getComputedStyle(document.getElementById('off1')).display + ' ' + \
         getComputedStyle(document.getElementById('on1')).display",
    );
    assert_eq!(got, "true none block", "after a click on #c1: checked, off1, on1");
}
