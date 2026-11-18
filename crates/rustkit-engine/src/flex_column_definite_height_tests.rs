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

/// Flex parity (found by INC5's 6a review, Prometheus #1064): two
/// `height: 300px` items in a 500px column body overflow it in RustKit
/// (300 + 300). The items shrink (flex-shrink 1), each no further than its
/// automatic minimum, min(its content height, its `height`): the first,
/// holding 320px of content, stays at 300 and the second takes the shrink.
/// When the first's content drops to 150px its minimum drops, and the two
/// share the shrink. Expected: Chrome for Testing 148.0.7778.216, headless,
/// 400x600 (scratch probe, `getBoundingClientRect().height` of each item):
/// 300/200 before, 250/250 after; padded 1px top and bottom, 302/198 then
/// 250/250.
#[test]
fn column_items_with_a_length_height_shrink_to_their_automatic_minimum() {
    let page = |pad: &str| {
        format!(
            "<style>body{{margin:0;height:500px;display:flex;flex-direction:column}} .i{{height:300px;{pad}}}</style>\
             <div class=i><div id=t style='height:320px'></div></div><div class=i><div style='height:100px'></div></div>"
        )
    };
    const HEIGHTS: &str = "Array.prototype.map.call(document.querySelectorAll('.i'), function(e){return Math.round(e.getBoundingClientRect().height)}).join('/')";
    let mut wrong = Vec::new();
    for (name, pad, before, after) in [("plain", "", "300/200", "250/250"), ("padded", "padding:1px 0", "302/198", "250/250")] {
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let id = engine.create_headless_view(Bounds::new(0, 0, 400, 600)).expect("view");
        engine.load_html(id, &page(pad)).expect("load");
        let unquote = |v: String| {
            v.strip_prefix("String(\"").and_then(|v| v.strip_suffix("\")")).unwrap_or(&v).to_string()
        };
        let got_before = unquote(engine.execute_script(id, HEIGHTS).expect("read"));
        engine
            .execute_script(id, "document.getElementById('t').style.height = '150px';")
            .expect("edit");
        let got_after = unquote(engine.execute_script(id, HEIGHTS).expect("read"));
        if got_before != before || got_after != after {
            wrong.push(format!("{name}: {got_before} then {got_after} (Chrome 148: {before} then {after})"));
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

