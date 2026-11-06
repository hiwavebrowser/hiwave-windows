//! Z plan CH2.B7 (disclosed on #903): `display:flex; flex-direction:column;
//! height:0` was sized by its content (84px for two 40/44px children,
//! Chromium 0), while `height:10px` was honoured. A column flex container
//! with a definite height of zero is zero tall; its items shrink to their
//! min-content (0 for empty boxes).
//!
//! Expected values: Chromium 143.0.7499.4 on these exact pages at 1280x800
//! (tools/ch2_b7/oracle.mjs, variants.mjs). Each reads `top+height` of #c,
//! its children, and #n when present.

#![cfg(all(target_os = "macos", feature = "headless"))]

use super::*;

const KIDS: &str = "<div style=\"height:40px\"></div><div style=\"height:44px\"></div>";
const B: &str = "<style>body{margin:0}</style>";
const COL0: &str = "display:flex;flex-direction:column;height:0";
const READ: &str = "(function(){var c=document.getElementById('c');function r(e){var b=e.getBoundingClientRect();return Math.round(b.top)+'+'+Math.round(b.height)}\
    var n=document.getElementById('n');return 'c='+r(c)+' kids='+Array.prototype.map.call(c.children,r).join(',')+(n?' n='+r(n):'');})()";

fn read(html: &str) -> String {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 1280, 800))
        .expect("view");
    engine.load_html(id, html).expect("load");
    let v = engine.execute_script(id, READ).expect("script");
    v.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&v)
        .to_string()
}

fn one(style: &str) -> String {
    format!("{B}<div id=c style=\"{style}\">{KIDS}</div>")
}

fn check(cases: &[(&str, String, &str)]) {
    let wrong: Vec<String> = cases
        .iter()
        .filter_map(|(name, html, chrome)| {
            let got = read(html);
            (got != *chrome).then(|| format!("{name}: {got} (Chromium 143: {chrome})"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn column_flex_with_zero_height_is_zero_tall() {
    check(&[
        ("col_h0", one(COL0), "c=0+0 kids=0+0,0+0"),
        ("col_h0_ovh", one(&format!("{COL0};overflow:hidden")), "c=0+0 kids=0+0,0+0"),
        ("col_h0_pad", one(&format!("{COL0};padding:5px")), "c=0+10 kids=5+0,5+0"),
        (
            "col_h0_bb_pad",
            one(&format!("{COL0};padding:5px;box-sizing:border-box")),
            "c=0+10 kids=5+0,5+0",
        ),
        (
            "col_h0_inflow",
            format!("{B}<div id=p><div id=c style=\"{COL0}\">{KIDS}</div><div id=n style=\"height:5px\"></div></div>"),
            "c=0+0 kids=0+0,0+0 n=0+5",
        ),
        (
            "col_h0_pct",
            format!("{B}<div id=p style=\"height:0\"><div id=c style=\"display:flex;flex-direction:column;height:100%\">{KIDS}</div></div>"),
            "c=0+0 kids=0+0,0+0",
        ),
    ]);
}

#[test]
fn column_flex_with_zero_height_inside_flex() {
    check(&[
        (
            "col_h0_in_colflex_noshrink",
            format!("{B}<div id=p style=\"display:flex;flex-direction:column;height:300px\"><div id=c style=\"{COL0};flex-shrink:0\">{KIDS}</div><div id=n style=\"flex:1\"></div></div>"),
            "c=0+0 kids=0+0,0+0 n=0+300",
        ),
        (
            "col_h0_in_colflex",
            format!("{B}<div id=p style=\"display:flex;flex-direction:column;height:300px\"><div id=c style=\"{COL0}\">{KIDS}</div><div id=n style=\"flex:1\"></div></div>"),
            "c=0+0 kids=0+0,0+0 n=0+300",
        ),
        (
            "col_h0_in_rowflex",
            format!("{B}<div id=p style=\"display:flex;height:300px;align-items:flex-start\"><div id=c style=\"{COL0}\">{KIDS}</div></div>"),
            "c=0+0 kids=0+0,0+0",
        ),
    ]);
}

/// The cause is wider than column flex: a unitless `0` (Length::Zero) in a
/// sizing property was read as `auto` by layout; `0px` was always right.
#[test]
fn unitless_zero_sizes_are_zero() {
    check(&[
        ("block_h0", one("height:0"), "c=0+0 kids=0+40,40+44"),
        ("row_h0", one("display:flex;height:0"), "c=0+0 kids=0+40,0+44"),
        ("col_maxh0", one("display:flex;flex-direction:column;max-height:0"), "c=0+0 kids=0+0,0+0"),
    ]);
}

/// A column whose height is indefinite flexes its items into its
/// `max-height` (css-flexbox-1 §9.3), not into their stack. Shrinking is
/// weighted by basis (14/16), and text items stop at their min-content.
#[test]
fn column_flex_items_shrink_into_max_height() {
    const C: &str = "display:flex;flex-direction:column;max-height:20px";
    check(&[
        ("col_maxh20", one(C), "c=0+20 kids=0+10,10+10"),
        ("col_maxh20_minh30", one(&format!("{C};min-height:30px")), "c=0+30 kids=0+14,14+16"),
        (
            "col_maxh20_bb_pad",
            one(&format!("{C};padding:5px;box-sizing:border-box")),
            "c=0+20 kids=5+5,10+5",
        ),
        ("col_maxh20_ovauto", one(&format!("{C};overflow:auto")), "c=0+20 kids=0+10,10+10"),
        (
            "col_maxh20_textkids",
            format!("{B}<div id=c style=\"{C}\"><div>a</div><div>b</div></div>"),
            "c=0+20 kids=0+18,18+18",
        ),
    ]);
}

/// CSS 2.1 §10.7: min-height wins over a smaller max-height. max won.
#[test]
fn min_height_wins_over_max_height() {
    check(&[
        ("block_maxh20_minh30", one("max-height:20px;min-height:30px"), "c=0+30 kids=0+40,40+44"),
        (
            "block_h50_maxh20_minh30",
            one("height:50px;max-height:20px;min-height:30px"),
            "c=0+30 kids=0+40,40+44",
        ),
    ]);
}

/// Controls that were right before the fix.
#[test]
fn definite_height_controls() {
    check(&[
        ("col_h10", one("display:flex;flex-direction:column;height:10px"), "c=0+10 kids=0+5,5+5"),
        ("block_h0px", one("height:0px"), "c=0+0 kids=0+40,40+44"),
    ]);
}
