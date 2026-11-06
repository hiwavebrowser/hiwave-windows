//! Plan row C10 (a), ledger #474: a float's margins do not collapse with the
//! block before it. Expected `y` of `#f` and `#n` are Chromium 143.0.7499.4 on
//! the same pages (`trench/z/tools/c10/float_margin_probe.mjs` on the hub).

use super::*;

const CASES: &[(&str, &str, &str)] = &[
    ("block mb, float mt", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div>", "60 -"),
    ("block mb smaller than float mt", "<div id=a style=\"height:20px;margin-bottom:10px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:30px\"></div>", "60 -"),
    ("block mb, float none", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;\"></div>", "50 -"),
    ("block none, float mt", "<div id=a style=\"height:20px;\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div>", "30 -"),
    ("negative block mb, float mt", "<div id=a style=\"height:20px;margin-bottom:-5px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div>", "25 -"),
    ("block mb, negative float mt", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:-10px\"></div>", "40 -"),
    ("then a block with mt", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div><div id=n style=\"height:10px;margin-top:40px\"></div>", "60 60"),
    ("then a block with smaller mt", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div><div id=n style=\"height:10px;margin-top:5px\"></div>", "60 50"),
    ("then a block with no mt", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;\"></div><div id=n style=\"height:10px;\"></div>", "50 50"),
    ("float mb then block", "<div id=a style=\"height:20px;\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-bottom:30px\"></div><div id=n style=\"height:10px;margin-top:5px\"></div>", "20 25"),
    ("right float", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:right;width:50px;height:50px;margin-top:10px\"></div>", "60 -"),
    ("two floats", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div><div id=n style=\"float:left;width:50px;height:50px;margin-top:20px\"></div>", "60 70"),
    ("clear after float", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div><div id=n style=\"height:10px;clear:both;margin-top:5px\"></div>", "60 110"),
    ("in a parent", "<div style=\"margin-top:7px\"><div id=a style=\"height:20px;margin-bottom:30px\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div></div>", "67 -"),
    ("float first in a parent with mt", "<div style=\"margin-top:7px\"><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div><div id=n style=\"height:10px;\"></div></div>", "17 7"),
    ("float first in body", "<div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div><div id=n style=\"height:10px;margin-top:15px\"></div>", "25 15"),
    ("empty block between", "<div id=a style=\"height:20px;margin-bottom:30px\"></div><div style=\"margin:12px 0\"></div><div id=f style=\"float:left;width:50px;height:50px;margin-top:10px\"></div>", "60 -"),
];

#[test]
fn a_float_keeps_its_margin_and_the_blocks_as_chromium_places_them() {
    let wrong: Vec<String> = CASES
        .iter()
        .filter_map(|(name, body, want)| {
            let mut engine = Engine::new(EngineConfig::default()).expect("engine");
            let view = engine
                .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
                .expect("view");
            let page = format!("<!doctype html><style>body{{margin:0}}</style>{body}");
            engine.load_html(view, &page).expect("load");
            let got = engine
                .execute_script(
                    view,
                    "['f', 'n'].map(function (i) { var e = document.getElementById(i); \
                     return e ? e.getBoundingClientRect().y : '-'; }).join(' ')",
                )
                .expect("script");
            let got = got.trim_start_matches("String(\"").trim_end_matches("\")").to_string();
            (got != *want).then(|| format!("{name}: {got}, Chromium {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), CASES.len(), wrong.join("\n"));
}

/// An empty block's margins collapse through it, with what is pending before
/// it and what comes after. It sat at its top margin with only its bottom one
/// pending, so `20 + 30`, then 12 more. Expected are the `y` of `#e` and `#n` and
/// the height of `#p`, Chromium 143 (`trench/z/tools/c10/empty_through_probe.mjs`).
const THROUGH: &[(&str, &str, &str)] = &[
    ("then a block", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div>", "50 50 -"),
    ("then a block with a larger mt", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;margin-top:40px\"></div>", "50 60 -"),
    ("larger bottom than the block before", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0 45px\"></div><div id=n style=\"height:10px;\"></div>", "50 65 -"),
    ("larger top than the block before", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:45px 0 12px\"></div><div id=n style=\"height:10px;\"></div>", "65 65 -"),
    ("two empties", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div style=\"margin:35px 0\"></div><div id=n style=\"height:10px;\"></div>", "50 55 -"),
    ("negative bottom", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:0 0 -5px\"></div><div id=n style=\"height:10px;\"></div>", "50 45 -"),
    ("nothing before", "<div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div>", "12 12 -"),
    ("height 0", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;height:0\"></div><div id=n style=\"height:10px;\"></div>", "50 50 -"),
    ("min-height", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;min-height:1px\"></div><div id=n style=\"height:10px;\"></div>", "50 63 -"),
    ("border stops it", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;border-top:1px solid\"></div><div id=n style=\"height:10px;\"></div>", "50 63 -"),
    ("padding stops it", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;padding-bottom:1px\"></div><div id=n style=\"height:10px;\"></div>", "50 63 -"),
    ("overflow hidden stops it", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;overflow:hidden\"></div><div id=n style=\"height:10px;\"></div>", "50 62 -"),
    ("display flex stops it", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;display:flex\"></div><div id=n style=\"height:10px;\"></div>", "50 62 -"),
    ("height 1 stops it", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;height:1px\"></div><div id=n style=\"height:10px;\"></div>", "50 63 -"),
    ("clear none after no float", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;clear:both\"></div><div id=n style=\"height:10px;\"></div>", "50 50 -"),
    ("holds an abspos box", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"><div style=\"position:absolute;width:5px;height:5px\"></div></div><div id=n style=\"height:10px;\"></div>", "50 50 -"),
    ("holds an empty block", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"><div style=\"margin:50px 0\"></div></div><div id=n style=\"height:10px;\"></div>", "70 70 -"),
    ("last in a parent", "<div id=p><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div></div><div id=n style=\"height:10px;\"></div>", "50 50 20"),
    ("last in a parent with a border", "<div id=p style=\"border-bottom:1px solid\"><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div></div><div id=n style=\"height:10px;\"></div>", "50 51 51"),
    ("last in a fixed-height parent", "<div id=p style=\"height:60px\"><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div></div><div id=n style=\"height:10px;\"></div>", "50 60 60"),
    ("first in a parent", "<div id=p style=\"margin-top:7px\"><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div></div>", "12 12 10"),
    ("first in a parent with a border", "<div id=p style=\"border-top:1px solid;margin-top:7px\"><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div></div>", "20 20 23"),
    ("only child of a parent", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=p style=\"margin:5px 0\"><div id=e style=\"margin:12px 0\"></div></div><div id=n style=\"height:10px;\"></div>", "50 50 0"),
    ("then a float", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"float:left;width:50px;height:50px;margin-top:10px\"></div>", "50 60 -"),
    ("then text", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n>x</div>", "50 50 -"),
    ("first in a parent, next has a larger mt", "<div id=p style=\"margin-top:7px\"><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;margin-top:40px\"></div></div>", "40 40 10"),
    ("first in a parent, then text then a block", "<div id=p style=\"margin-top:7px\"><div id=e style=\"margin:12px 0\"></div>x<div id=n style=\"height:10px;margin-top:40px\"></div></div>", "12 70 68"),
    ("first in a parent, then a float then a block", "<div id=p style=\"margin-top:7px\"><div id=e style=\"margin:12px 0\"></div><div style=\"float:left;width:5px;height:5px\"></div><div id=n style=\"height:10px;margin-top:40px\"></div></div>", "40 40 10"),
    ("clearance past a float", "<div style=\"float:left;width:50px;height:100px\"></div><div style=\"height:20px;margin-bottom:0\"></div><div id=e style=\"margin:12px 0;clear:both\"></div><div id=n style=\"height:10px;\"></div>", "100 100 -"),
    ("clearance past a float, block before, next has mt", "<div style=\"float:left;width:50px;height:100px\"></div><div style=\"height:20px;margin-bottom:0\"></div><div id=e style=\"margin:12px 0;clear:left\"></div><div id=n style=\"height:10px;margin-top:30px\"></div>", "100 118 -"),
    ("clear none beside a float", "<div style=\"float:left;width:50px;height:100px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div>", "12 12 -"),
    ("holds a float", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"><div style=\"float:left;width:5px;height:5px\"></div></div><div id=n style=\"height:10px;\"></div>", "50 50 -"),
    ("percent margin", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:1% 0\"></div><div id=n style=\"height:10px;\"></div>", "50 50 -"),
    ("relative with an offset", "<div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0;position:relative;top:5px\"></div><div id=n style=\"height:10px;\"></div>", "55 50 -"),
    ("in an inline-block parent", "<div id=p style=\"display:inline-block;width:100px\"><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div></div><div id=n style=\"height:10px;\"></div>", "50 54 50"),
    ("in a flex item", "<div style=\"display:flex\"><div id=p style=\"width:100px\"><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div></div></div>", "50 50 60"),
    ("a flex container child", "<div id=p style=\"display:flex;flex-direction:column\"><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div></div>", "62 74 84"),
    ("in a table cell", "<table style=\"border-spacing:0\"><tr><td id=p style=\"padding:0\"><div style=\"height:20px;margin-bottom:30px\"></div><div id=e style=\"margin:12px 0\"></div><div id=n style=\"height:10px;\"></div></td></tr></table>", "50 50 60"),
    ("a list item", "<ul style=\"margin:0;padding:0;list-style:none\"><li id=e style=\"margin:12px 0\"></li><div id=n style=\"height:10px;\"></div></ul>", "12 12 -"),
    ("an empty paragraph", "<div style=\"height:20px;margin-bottom:30px\"></div><p id=e></p><div id=n style=\"height:10px;\"></div>", "50 50 -"),
];

#[test]
fn margins_collapse_through_an_empty_block() {
    let wrong: Vec<String> = THROUGH
        .iter()
        .filter_map(|(name, body, want)| {
            let mut engine = Engine::new(EngineConfig::default()).expect("engine");
            let view = engine
                .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
                .expect("view");
            let page = format!("<!doctype html><style>body{{margin:0}}</style>{body}");
            engine.load_html(view, &page).expect("load");
            let got = engine
                .execute_script(
                    view,
                    "var y = function (i) { var e = document.getElementById(i); \
                     return e ? e.getBoundingClientRect().y : '-'; }; \
                     var p = document.getElementById('p'); \
                     [y('e'), y('n'), p ? p.getBoundingClientRect().height : '-'].join(' ')",
                )
                .expect("script");
            let got = got.trim_start_matches("String(\"").trim_end_matches("\")").to_string();
            (got != *want).then(|| format!("{name}: {got}, Chromium {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), THROUGH.len(), wrong.join("\n"));
}


/// R1 on #863: pages develop got right and this row broke, and their
/// neighbours. A first box holding floats and a `clear` (direct, nested or a
/// clearfix `::after`) has height and does not collapse through; neither do
/// an `aspect-ratio` box, a box that is its own formatting context
/// (`flow-root`, `contain: layout|paint|content|strict`,
/// `content-visibility: auto|hidden`, `container-type`, vertical
/// `writing-mode`), `hr`, `fieldset` or a replaced element. A `clear` that
/// follows only floats on the other side gets no clearance. An empty
/// box moved by clearance: the next box sits at
/// `e.y - margin-top + collapse(margin-top, margin-bottom, next margin-top)`.
/// Expected: `y/height` of every `[id]`, Chromium 143.0.7499.4, same page.
const REVIEW: &[(&str, &str, &str)] = &[
    ("clear moved, mb larger", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="margin:0 0 20px;clear:both"></div><div id=n style="height:10px;"></div>"#, "f=0/100 e=100/0 n=120/10"),
    ("clear moved, neg mb", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="margin:0 0 -8px;clear:both"></div><div id=n style="height:10px;"></div>"#, "f=0/100 e=100/0 n=92/10"),
    ("clear moved, next neg mt", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="margin:0 0 20px;clear:both"></div><div id=n style="height:10px;margin-top:-5px"></div>"#, "f=0/100 e=100/0 n=115/10"),
    ("clear last in auto parent, no margins (spacer)", r#"<div id=p><div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both"></div></div><div id=n style="height:10px;margin-top:15px"></div>"#, "p=0/100 f=0/100 e=100/0 n=115/10"),
    ("clear last in parent mb on parent", r#"<div id=p style="margin-bottom:25px"><div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both"></div></div><div id=n style="height:10px;margin-top:15px"></div>"#, "p=0/100 f=0/100 e=100/0 n=125/10"),
    ("clear last in padded parent", r#"<div id=p style="padding:5px"><div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin-bottom:20px"></div></div><div id=n style="height:10px;"></div>"#, "p=0/130 f=5/100 e=105/0 n=130/10"),
    ("clear last in BFC parent", r#"<div id=p style="overflow:hidden"><div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin-bottom:20px"></div></div><div id=n style="height:10px;"></div>"#, "p=0/120 f=0/100 e=100/0 n=120/10"),
    ("clear in nested parent offset", r#"<div id=p style="margin-top:50px;padding-top:10px"><div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin:0 0 20px"></div><div id=n style="height:10px;"></div></div>"#, "p=50/140 f=60/100 e=160/0 n=180/10"),
    ("clearfix block", r#"<style>.cf::after{content:"";display:block;clear:both}.cf2::after{content:"";display:table;clear:both}.cf3::after{content:".";display:block;height:0;clear:both;visibility:hidden}</style><div id=p class=cf><div id=f style="float:left;width:50px;height:100px;"></div></div><div id=n style="height:10px;margin-top:20px"></div>"#, "p=0/100 f=0/100 n=120/10"),
    ("clearfix with mb on parent", r#"<style>.cf::after{content:"";display:block;clear:both}.cf2::after{content:"";display:table;clear:both}.cf3::after{content:".";display:block;height:0;clear:both;visibility:hidden}</style><div id=p class=cf style="margin-bottom:25px"><div id=f style="float:left;width:50px;height:100px;"></div></div><div id=n style="height:10px;margin-top:20px"></div>"#, "p=0/100 f=0/100 n=125/10"),
    ("first aspect-ratio", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;aspect-ratio:4/1;width:80px"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/70 e=12/20 n=72/10"),
    ("bfc flow-root", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;display:flow-root"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first flow-root", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;display:flow-root"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("bfc contain layout", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;contain:layout"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first contain layout", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;contain:layout"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("bfc contain paint", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;contain:paint"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first contain paint", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;contain:paint"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("bfc contain content", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;contain:content"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first contain content", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;contain:content"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("bfc container", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;container-type:size"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first container", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;container-type:size"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("bfc container inline", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;container-type:inline-size"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first container inline", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;container-type:inline-size"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("bfc writing-mode", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;writing-mode:vertical-rl"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("bfc-first writing-mode", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;writing-mode:vertical-rl"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("el iframe0", r#"<div id=a style="height:20px;margin-bottom:30px"></div><iframe id=e style="display:block;margin:12px 0;height:0;border:0"></iframe><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("el-first iframe0", r#"<div id=p style="margin-top:7px"><iframe id=e style="display:block;margin:12px 0;height:0;border:0"></iframe><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("el hr0", r#"<div id=a style="height:20px;margin-bottom:30px"></div><hr id=e style="margin:12px 0;border:0"><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("el-first hr0", r#"<div id=p style="margin-top:7px"><hr id=e style="margin:12px 0;border:0"><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("el fieldset", r#"<div id=a style="height:20px;margin-bottom:30px"></div><fieldset id=e style="margin:12px 0;border:0;padding:0"></fieldset><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("el-first fieldset", r#"<div id=p style="margin-top:7px"><fieldset id=e style="margin:12px 0;border:0;padding:0"></fieldset><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("page: nav clearfix then h1", r#"<style>.cf::after{content:"";display:block;clear:both}li{list-style:none}</style><nav id=p class=cf><div id=f style="float:left;width:50px;height:100px;"></div></nav><h1 id=n style="height:30px"></h1>"#, "p=0/100 f=0/100 n=121.4/30"),
    ("page: wrap, header clearfix, main mt", r#"<style>.cf::after{content:"";display:block;clear:both}li{list-style:none}</style><div id=w><header id=p class=cf><div id=f style="float:left;width:50px;height:100px;"></div></header><main id=n style="height:30px;margin-top:20px"></main></div>"#, "w=0/150 p=0/100 f=0/100 n=120/30"),
    ("page: floats then clear div, then section mt", r#"<div id=p><div id=f style="float:left;width:50px;height:100px;"></div><div style="clear:both"></div></div><section id=n style="height:30px;margin-top:40px"></section>"#, "p=0/100 f=0/100 n=140/30"),
    ("aspect-ratio hero first", r#"<div id=p style="aspect-ratio:4/1;margin-bottom:10px"></div><div id=n style="height:10px;margin-top:40px"></div>"#, "p=0/200 n=240/10"),
    ("clear spacer mb, then footer", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin-bottom:24px"></div><footer id=n style="height:30px"></footer>"#, "f=0/100 e=100/0 n=124/30"),
    ("clear spacer mb, in padded card", r#"<div id=p style="padding:10px"><div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin-bottom:24px"></div></div>"#, "p=0/144 f=10/100 e=110/0"),
    ("clear grandchild", r#"<div id=p><div id=f style="float:left;width:50px;height:100px;"></div><div><div style="clear:both"></div></div></div><div id=n style="height:10px;margin-top:20px"></div>"#, "p=0/100 f=0/100 n=120/10"),
    ("clear spacer mt10 mb30", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin:10px 0 30px"></div><div id=n style="height:10px;"></div>"#, "f=0/100 e=100/0 n=120/10"),
    ("clear spacer mt30 mb10", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin:30px 0 10px"></div><div id=n style="height:10px;margin-top:5px"></div>"#, "f=0/100 e=100/0 n=100/10"),
    ("clear spacer mt-10 mb20", r#"<div id=f style="float:left;width:50px;height:100px;"></div><div id=e style="clear:both;margin:-10px 0 20px"></div><div id=n style="height:10px;"></div>"#, "f=0/100 e=100/0 n=120/10"),
    ("cf table first", r#"<style>.cf::after{content:"";display:table;clear:both}</style><div id=p class=cf><div id=f style="float:left;width:50px;height:100px;"></div></div><div id=n style="height:10px;margin-top:20px"></div>"#, "p=0/100 f=0/100 n=120/10"),
    ("cf nested two deep first", r#"<style>.cf::after{content:"";display:block;clear:both}</style><div id=w><div id=p class=cf><div id=f style="float:left;width:50px;height:100px;"></div></div></div><div id=n style="height:10px;margin-top:20px"></div>"#, "w=0/100 p=0/100 f=0/100 n=120/10"),
    ("aspect-ratio sib", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;aspect-ratio:4/1"></div><div id=n style="height:10px"></div>"#, "a=0/20 e=50/200 n=262/10"),
    ("only floats first then mt", r#"<div id=p><div id=f style="float:left;width:50px;height:100px;"></div></div><div id=n style="height:10px;margin-top:20px"></div>"#, "p=20/0 f=20/100 n=20/10"),
    ("video block h0", r#"<div id=a style="height:20px;margin-bottom:30px"></div><video id=e style="display:block;margin:12px 0;height:0"></video><div id=n style="height:10px"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("vertical first", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;writing-mode:vertical-rl"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("contain strict", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;contain:strict"></div><div id=n style="height:10px"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("contain size only", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;contain:size"></div><div id=n style="height:10px"></div>"#, "a=0/20 e=50/0 n=50/10"),
    ("contain style only", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;contain:style"></div><div id=n style="height:10px"></div>"#, "a=0/20 e=50/0 n=50/10"),
    ("container normal", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;container-name:x"></div><div id=n style="height:10px"></div>"#, "a=0/20 e=50/0 n=50/10"),
    ("hr0 first", r#"<div id=p style="margin-top:7px"><hr id=e style="margin:12px 0;border:0"><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("object block", r#"<div id=a style="height:20px;margin-bottom:30px"></div><object id=e style="display:block;margin:12px 0;height:0"></object><div id=n style="height:10px"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("right float, clear:left empty", r#"<div id=f style="float:left;width:50px;height:100px;float:right"></div><div id=e style="margin:12px 0;clear:left"></div><div id=n style="height:10px;"></div>"#, "f=12/100 e=12/0 n=12/10"),
    ("right float, clear:left empty, first in wrapper", r#"<div id=w style="margin-top:7px"><div id=f style="float:left;width:50px;height:100px;float:right"></div><div id=e style="margin:12px 0;clear:left"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "w=40/10 f=40/100 e=40/0 n=40/10"),
    ("right float, clearfix left only", r#"<style>.cl::after{content:"";display:block;clear:left}</style><div id=w><div id=p class=cl><div id=f style="float:left;width:50px;height:100px;float:right"></div></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "w=40/10 p=40/0 f=40/100 n=40/10"),
    ("ctx content-visibility auto", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;content-visibility:auto"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("ctx-first content-visibility auto", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;content-visibility:auto"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
    ("ctx content-visibility hidden", r#"<div id=a style="height:20px;margin-bottom:30px"></div><div id=e style="margin:12px 0;content-visibility:hidden"></div><div id=n style="height:10px;"></div>"#, "a=0/20 e=50/0 n=62/10"),
    ("ctx-first content-visibility hidden", r#"<div id=p style="margin-top:7px"><div id=e style="margin:12px 0;content-visibility:hidden"></div><div id=n style="height:10px;margin-top:40px"></div></div>"#, "p=12/50 e=12/0 n=52/10"),
];

fn rects(v: &str) -> Vec<(String, f32, f32)> {
    v.split_whitespace()
        .filter_map(|t| {
            let (k, r) = t.split_once('=')?;
            let (y, h) = r.split_once('/')?;
            Some((k.to_string(), y.parse().ok()?, h.parse().ok()?))
        })
        .collect()
}

#[test]
fn floats_clearance_and_formatting_roots_stop_an_empty_block_as_chromium_does() {
    let wrong: Vec<String> = REVIEW
        .iter()
        .filter_map(|(name, body, want)| {
            let mut engine = Engine::new(EngineConfig::default()).expect("engine");
            let view = engine
                .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
                .expect("view");
            let page = format!("<!doctype html><style>body{{margin:0}}</style>{body}");
            engine.load_html(view, &page).expect("load");
            let got = engine
                .execute_script(
                    view,
                    "Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) { \
                     var r = e.getBoundingClientRect(); return e.id + '=' + r.y + '/' + r.height; \
                     }).join(' ')",
                )
                .expect("script");
            let got = got.trim_start_matches("String(\"").trim_end_matches("\")").to_string();
            let (g, w) = (rects(&got), rects(want));
            let same = g.len() == w.len()
                && g.iter().zip(&w).all(|(a, b)| {
                    a.0 == b.0 && (a.1 - b.1).abs() <= 0.5 && (a.2 - b.2).abs() <= 0.5
                });
            (!same).then(|| format!("{name}: {got}, Chromium {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{} of {} wrong:\n{}", wrong.len(), REVIEW.len(), wrong.join("\n"));
}
