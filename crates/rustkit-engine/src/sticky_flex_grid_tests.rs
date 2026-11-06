//! Plan row C7: `position: sticky` in flex and grid, and on table parts.
//! Expected strings are Chromium 143.0.7499.4 on the same bytes (page and
//! script), one fresh 800x600 page per case; each script samples five scroll
//! offsets. `top(i)` reads the rect and `offsetTop`.

use super::*;

/// Run inside a function: in Chromium a global `var top` cannot replace `window.top`.
const AT: &str = concat!(
    "var $ = function (i) { return document.getElementById(i); }; ",
    "var at = function (i) { var r = $(i).getBoundingClientRect(); return r.x + ',' + r.y; }; ",
    "var top = function (i) { return at(i) + ' ' + $(i).offsetTop; }; ",
    "var hit = function (x, y) { var e = document.elementFromPoint(x, y); return e && e.id || '-'; }; ",
    "var walk = function (s, offs, f) { return offs.map(function (o) { $(s).scrollTop = o; return f(); }).join(' | '); }; ",
    "var win = function (offs, f) { return offs.map(function (o) { window.scrollTo(0, o); return f(); }).join(' | '); }; ",
    "var H = function (i) { var e = $(i); return e.offsetHeight + '/' + e.clientHeight + '/' + e.scrollHeight; }; ",
    "var D = function () { return document.documentElement.scrollHeight; }; ",
    "var Y = function (i) { return Math.round($(i).getBoundingClientRect().y); }; ",
);

#[track_caller]
fn check(page: &str, script: &str, want: &str) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, page).expect("load");
    let got = engine
        .execute_script(view, &format!("(function () {{ {AT}return {script}; }})()"))
        .expect("script");
    assert_eq!(got, format!("String(\"{want}\")"));
}

/// A sticky header inside a flex column, in a scroller.
#[test]
fn a_header_in_a_flex_column_sticks_to_its_scroller() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:200px;height:100px}#f{display:flex;flex-direction:column}#gap{height:50px}#h{position:sticky;top:0;height:20px}#t{height:600px}",
            "</style><body><div id=s><div id=f><div id=gap></div><div id=h></div><div id=t></div></div></div></body>",
        ),
        "walk('s', [0, 30, 50, 200, 600], function () { return top('h'); })",
        "0,50 50 | 0,20 50 | 0,0 50 | 0,0 200 | 0,0 570",
    );
}

/// A sticky header in a flex column in the window leaves with the column.
#[test]
fn a_header_in_a_flex_column_leaves_with_its_container() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:100px}#f{display:flex;flex-direction:column;width:300px}#h{position:sticky;top:0;height:40px}#t{height:1000px}#after{height:2000px}",
            "</style><body><div id=sp></div><div id=f><div id=h></div><div id=t></div></div><div id=after></div></body>",
        ),
        "win([0, 50, 100, 600, 1200], function () { return top('h'); })",
        "0,100 100 | 0,50 100 | 0,0 100 | 0,0 600 | 0,-100 1100",
    );
}

/// A sticky sidebar in a grid column, `align-self: start`.
#[test]
fn a_sidebar_in_a_grid_column_sticks_until_the_grid_ends() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:100px}#g{display:grid;grid-template-columns:100px 200px}#side{position:sticky;top:10px;height:50px;align-self:start}#main{height:1000px}",
            "</style><body><div id=sp></div><div id=g><div id=side></div><div id=main></div></div></body>",
        ),
        "win([0, 50, 100, 500, 1100], function () { return top('side'); })",
        "0,100 100 | 0,50 100 | 0,10 110 | 0,10 510 | 0,-50 1050",
    );
}

/// The same sidebar left to stretch: as tall as its row, so it has no room.
#[test]
fn a_stretched_grid_sidebar_never_moves() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:100px}#g{display:grid;grid-template-columns:100px 200px}#side{position:sticky;top:10px}#main{height:1000px}",
            "</style><body><div id=sp></div><div id=g><div id=side></div><div id=main></div></div></body>",
        ),
        "win([0, 50, 100, 500, 1100], function () { return top('side') + ' ' + $('side').offsetHeight; })",
        "0,100 100 1000 | 0,50 100 1000 | 0,0 100 1000 | 0,-400 100 1000 | 0,-1000 100 1000",
    );
}

/// A grid sidebar in a scroller.
#[test]
fn a_grid_sidebar_sticks_to_its_scroller() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:300px;height:200px}#g{display:grid;grid-template-columns:100px 200px;margin-top:40px}#side{position:sticky;top:0;height:50px;align-self:start}#main{height:600px}",
            "</style><body><div id=s><div id=g><div id=side></div><div id=main></div></div></div></body>",
        ),
        "walk('s', [0, 20, 40, 300, 600], function () { return top('side'); })",
        "0,40 40 | 0,20 40 | 0,0 40 | 0,0 300 | 0,0 440",
    );
}

/// A sticky `th` in a table in a scroller: the table, not the row, holds it.
#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_th_sticks_in_a_scrolling_table() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:300px;height:100px}table{border-collapse:collapse;border-spacing:0}td,th{padding:0;height:20px;width:100px}th{position:sticky;top:0}",
            "</style><body><div id=s><table id=tb><thead><tr><th id=th1>a</th><th id=th2>b</th></tr></thead><tbody><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr></tbody></table></div></body>",
        ),
        "walk('s', [0, 30, 100, 300, 1000], function () { return top('th1') + ' ' + at('th2'); })",
        "0,0 0 100,0 | 0,0 30 100,0 | 0,0 100 100,0 | 0,0 300 100,0 | 0,0 320 100,0",
    );
}

/// A sticky `th` in the window, until the table's end.
#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_th_sticks_to_the_window_until_the_table_ends() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:100px}table{border-collapse:collapse;border-spacing:0}td,th{padding:0;height:20px;width:100px}th{position:sticky;top:0}",
            "</style><body><div id=sp></div><table id=tb><thead><tr><th id=th1>a</th><th id=th2>b</th></tr></thead><tbody><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr></tbody></table></body>",
        ),
        "win([0, 50, 100, 300, 600], function () { return top('th1') + ' ' + at('th2'); })",
        "0,100 100 100,100 | 0,50 100 100,50 | 0,0 100 100,0 | 0,0 300 100,0 | 0,-100 500 100,-100",
    );
}

/// A sticky `thead`: its cells go with it.
#[test]
fn a_sticky_thead_carries_its_cells() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:300px;height:100px}table{border-collapse:collapse;border-spacing:0}td,th{padding:0;height:20px;width:100px}thead{position:sticky;top:0}",
            "</style><body><div id=s><table id=tb><thead id=hd><tr><th id=th1>a</th><th>b</th></tr></thead><tbody><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr></tbody></table></div></body>",
        ),
        "walk('s', [0, 30, 100, 300, 1000], function () { return top('hd') + ' ' + at('th1'); })",
        "0,0 0 0,0 | 0,0 30 0,0 | 0,0 100 0,0 | 0,0 300 0,0 | 0,0 320 0,0",
    );
}

/// A sticky `tr`, held by the table.
#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_sticky_tr_sticks_in_a_scrolling_table() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:300px;height:100px}table{border-collapse:collapse;border-spacing:0}td,th{padding:0;height:20px;width:100px}#tr{position:sticky;top:0}",
            "</style><body><div id=s><table id=tb><thead><tr id=tr><th id=th1>a</th><th>b</th></tr></thead><tbody><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr></tbody></table></div></body>",
        ),
        "walk('s', [0, 30, 100, 300, 1000], function () { return top('tr') + ' ' + at('th1'); })",
        "0,0 0 0,0 | 0,0 30 0,0 | 0,0 100 0,0 | 0,0 300 0,0 | 0,0 320 0,0",
    );
}

/// A sticky `th` stops at the table's content box: under its border and padding, past its border spacing, with a caption above.
#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_th_stops_at_the_tables_content_box() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:100px}table{border-spacing:4px;padding:6px;border:2px solid}caption{height:30px}td,th{padding:0;height:20px;width:100px}th{position:sticky;top:0}",
            "</style><body><div id=sp></div><table id=tb><caption>c</caption><thead><tr><th id=th1>a</th><th>b</th></tr></thead><tbody><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr></tbody></table></body>",
        ),
        "win([0, 100, 140, 400, 1000], function () { return top('th1'); })",
        "12,142 142 | 12,42 142 | 12,2 142 | 12,0 400 | 12,-374 626",
    );
}

/// A `bottom: 0` footer cell rides up to the scrollport's bottom edge.
#[test]
#[ignore = "sticky table parts need stacking-context hoisting: PLAN-z follow-up row after C7 (#868 R1 delta 3); expected values are Chromium 143"]
fn a_bottom_sticky_footer_cell_holds_up_from_the_tables_end() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:300px;height:100px}table{border-collapse:collapse;border-spacing:0}td,th{padding:0;height:20px;width:100px}#ft{position:sticky;bottom:0}",
            "</style><body><div id=s><table id=tb><tbody><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr><tr><td>r</td><td>r</td></tr></tbody><tfoot><tr><td id=ft>f</td><td>g</td></tr></tfoot></table></div></body>",
        ),
        "walk('s', [0, 30, 100, 300, 1000], function () { return top('ft'); })",
        "0,80 80 | 0,80 110 | 0,80 180 | 0,80 380 | 0,80 400",
    );
}

/// Sticky inside an `overflow: auto` flex item of a row.
#[test]
fn it_sticks_inside_a_flex_item_that_scrolls() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#r{display:flex;height:200px;width:400px}#nav{width:100px}#p{flex:1;overflow:auto}#gap{height:50px}#h{position:sticky;top:0;height:20px}#t{height:600px}",
            "</style><body><div id=r><div id=nav></div><div id=p><div id=gap></div><div id=h></div><div id=t></div></div></div></body>",
        ),
        "walk('p', [0, 30, 50, 200, 600], function () { return top('h'); })",
        "100,50 50 | 100,20 50 | 100,0 50 | 100,0 200 | 100,0 470",
    );
}

/// Sticky top and bottom inside a `flex: 1` scroller of a column: the scrollport is the item's flexed height.
#[test]
fn it_sticks_inside_a_flexed_scroller_in_a_column() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#c{display:flex;flex-direction:column;height:200px;width:300px}#hd{height:40px;flex:none}#p{flex:1;overflow:auto;min-height:0}#gap{height:50px}#h{position:sticky;top:0;height:20px}#t{height:600px}#ft{position:sticky;bottom:0;height:10px}",
            "</style><body><div id=c><div id=hd></div><div id=p><div id=gap></div><div id=h></div><div id=t></div><div id=ft></div></div></div></body>",
        ),
        "walk('p', [0, 30, 50, 200, 600], function () { return top('h') + ' ' + at('ft'); })",
        "0,90 90 0,190 | 0,60 90 0,190 | 0,40 90 0,190 | 0,40 240 0,190 | 0,40 560 0,190",
    );
}

/// `top` and `bottom` both set, on a flex column item.
#[test]
fn top_and_bottom_both_hold_a_flex_column_item() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:200px;height:100px}#f{display:flex;flex-direction:column}#gap{height:300px}#b{position:sticky;top:10px;bottom:10px;height:20px}#t{height:600px}",
            "</style><body><div id=s><div id=f><div id=gap></div><div id=b></div><div id=t></div></div></div></body>",
        ),
        "walk('s', [0, 150, 220, 300, 900], function () { return top('b'); })",
        "0,70 70 | 0,70 220 | 0,70 290 | 0,10 310 | 0,10 830",
    );
}

/// `top` and `bottom` both set, on a centred grid item in the window.
#[test]
fn top_and_bottom_both_hold_a_centred_grid_item() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:800px}#g{display:grid;grid-template-columns:100px 100px}#b{position:sticky;top:10px;bottom:10px;height:30px;align-self:center}#t{height:1000px}",
            "</style><body><div id=sp></div><div id=g><div id=b></div><div id=t></div></div></body>",
        ),
        "win([0, 300, 700, 1000, 1700], function () { return top('b'); })",
        "0,800 800 | 0,560 860 | 0,560 1260 | 0,285 1285 | 0,10 1710",
    );
}

/// `align-self: stretch` makes the item as tall as the line, so it never moves; `start` and `end` stick.
#[test]
fn a_stretched_flex_item_never_moves_but_start_and_end_do() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#sp{height:100px}#r{display:flex}#a{position:sticky;top:0;width:50px}#b{position:sticky;top:0;width:50px;height:30px;align-self:start}#e{position:sticky;top:0;width:50px;height:30px;align-self:end}#c{height:1000px;width:50px}",
            "</style><body><div id=sp></div><div id=r><div id=a></div><div id=b></div><div id=e></div><div id=c></div></div></body>",
        ),
        "win([0, 100, 500, 1070, 1300], function () { return [top('a'), top('b'), top('e'), $('a').offsetHeight].join(' '); })",
        "0,100 100 50,100 100 100,1070 1070 1000 | 0,0 100 50,0 100 100,970 1070 1000 | 0,-400 100 50,0 500 100,570 1070 1000 | 0,-970 100 50,0 1070 100,0 1070 1000 | 0,-1200 100 50,-230 1070 100,-230 1070 1000",
    );
}

/// The app shell: a 600px column, a 50px header, a `flex: 1` row holding an empty 200px rail and a `flex: 1; overflow: auto` pane with a sticky header. The rail is the row's 550, the page does not scroll.
#[test]
fn an_app_shell_pane_scrolls_and_its_empty_rail_does_not_grow_the_page() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}#app{display:flex;flex-direction:column;height:600px}#hd{height:50px;flex:none}#r{flex:1;display:flex;min-height:0}#side{width:200px}#p{flex:1;overflow:auto}",
            "</style><body><div id=app><div id=hd></div><div id=r><div id=side></div><div id=p><div id=gap style='height:50px'></div><div id=h style='position:sticky;top:0;height:20px'></div><div id=t style='height:600px'></div></div></div></div></body>",
        ),
        "walk('p', [0, 30, 50, 300, 1000], function () { return Y('h'); }) + ' # ' + [H('side'), H('p'), H('r'), D(), (function () { window.scrollTo(0, 500); return window.scrollY; })()].join(' ')",
        "100 | 70 | 50 | 50 | 50 # 550/550/550 550/550/670 550/550/550 600 0",
    );
}

/// The same shell with no sticky box.
#[test]
fn an_app_shell_without_sticky_keeps_its_rail_to_the_row() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}#app{display:flex;flex-direction:column;height:600px}#hd{height:50px;flex:none}#r{flex:1;display:flex;min-height:0}#side{width:200px}#p{flex:1;overflow:auto}",
            "</style><body><div id=app><div id=hd></div><div id=r><div id=side></div><div id=p><div style='height:670px'></div></div></div></div></body>",
        ),
        "[H('side'), H('p'), H('r'), D(), (function () { window.scrollTo(0, 500); return window.scrollY; })()].join(' ')",
        "550/550/550 550/550/670 550/550/550 600 0",
    );
}

/// In a 200px row every stretched item is 200: an empty rail, a box whose 670px content overflows (it does not grow the row), a line of text.
#[test]
fn every_stretched_item_of_a_fixed_row_takes_the_rows_height() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}#r{display:flex;height:200px}#rail{width:1px}#v{width:50px}#txt{width:50px;font:16px/20px sans-serif}#after{height:10px}",
            "</style><body><div id=r><div id=rail></div><div id=v><div style='height:670px'></div></div><div id=txt>a</div></div><div id=after></div></body>",
        ),
        "[H('rail'), H('v'), H('txt'), H('r'), Y('after'), D()].join(' ')",
        "200/200/200 200/200/670 200/200/200 200/200/670 200 670",
    );
}

/// A table's height is a minimum: in a 200px row a table of ten 30px rows stays 300 and overflows.
#[test]
fn a_table_in_a_fixed_row_keeps_its_rows() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}td{height:30px;padding:0;width:20px}#r{display:flex;height:200px}#b{width:20px}#nx{height:5px}",
            "</style><body><div id=r><table id=tb cellspacing=0><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr id=last><td></td></tr></table><div id=b></div></div><div id=nx></div></body>",
        ),
        "[H('tb'), Y('last'), H('b'), H('r'), Y('nx'), D()].join(' ')",
        "300/300/300 270 200/200/200 200/200/300 200 600",
    );
}

/// The same with a collapsed 2px border.
#[test]
fn a_collapsed_table_in_a_fixed_row_keeps_its_rows() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}td{height:30px;padding:0;width:20px}#r{display:flex;height:200px}#b{width:20px}#nx{height:5px}#tb{border:2px solid;border-collapse:collapse}",
            "</style><body><div id=r><table id=tb><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr id=last><td></td></tr></table><div id=b></div></div><div id=nx></div></body>",
        ),
        "[$('tb').offsetHeight, Y('last'), H('b'), H('r'), Y('nx'), D()].join(' ')",
        "304 272 200/200/200 200/200/304 200 600",
    );
}

/// The same as `display: inline-table`, with its border spacing.
#[test]
fn an_inline_table_in_a_fixed_row_keeps_its_rows() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}td{height:30px;padding:0;width:20px}#r{display:flex;height:200px}#b{width:20px}#nx{height:5px}#tb{display:inline-table}",
            "</style><body><div id=r><table id=tb><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr><td></td></tr><tr id=last><td></td></tr></table><div id=b></div></div><div id=nx></div></body>",
        ),
        "[H('tb'), Y('last'), H('b'), H('r'), Y('nx'), D()].join(' ')",
        "322/322/322 290 200/200/200 200/200/322 200 600",
    );
}

/// A `display: table` div holding 670px stays 670.
#[test]
fn a_display_table_div_in_a_fixed_row_keeps_its_content_height() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}#r{display:flex;height:200px}#tb{display:table}#b{width:20px}#nx{height:5px}",
            "</style><body><div id=r><div id=tb><div id=last style='height:670px;width:20px'></div></div><div id=b></div></div><div id=nx></div></body>",
        ),
        "[H('tb'), Y('last'), H('b'), H('r'), Y('nx'), D()].join(' ')",
        "670/670/670 0 200/200/200 200/200/670 200 670",
    );
}

/// A table shorter than the row still stretches to it.
#[test]
fn a_short_table_in_a_fixed_row_still_stretches() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}td{height:30px;padding:0;width:20px}#r{display:flex;height:200px}#b{width:20px}#nx{height:5px}",
            "</style><body><div id=r><table id=tb cellspacing=0><tr id=last><td></td></tr></table><div id=b></div></div><div id=nx></div></body>",
        ),
        "[H('tb'), Y('last'), H('b'), H('r'), Y('nx'), D()].join(' ')",
        "200/200/200 0 200/200/200 200/200/200 200 600",
    );
}

/// A stretched item is the row's height, and what it holds keeps its own: a 300px `display: table` box and a `flex: none` 400px child overflow.
#[test]
fn content_taller_than_a_fixed_row_keeps_its_size() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0}#r{display:flex;height:200px}#a{width:50px}#t{display:table;height:300px}#c{display:flex;flex-direction:column;width:50px}#k{height:400px;flex:none}#nx{height:5px}",
            "</style><body><div id=r><div id=a><div id=t style='width:10px'></div></div><div id=c><div id=k></div></div></div><div id=nx></div></body>",
        ),
        "[H('a'), H('t'), H('c'), H('k'), H('r'), Y('nx'), D()].join(' ')",
        "200/200/300 300/300/300 200/200/400 400/400/400 200/200/400 200 600",
    );
}

/// `align-items: flex-start` on the container, in a scroller.
#[test]
fn align_items_start_lets_a_flex_item_stick() {
    check(
        concat!(
            "<!doctype html><style>body{margin:0;height:3000px}#s{overflow:auto;width:300px;height:100px}#r{display:flex;align-items:flex-start;margin-top:20px}#a{position:sticky;top:5px;width:50px;height:30px}#c{height:500px;width:50px}",
            "</style><body><div id=s><div id=r><div id=a></div><div id=c></div></div></div></body>",
        ),
        "walk('s', [0, 15, 100, 475, 600], function () { return top('a'); })",
        "0,20 20 | 0,5 20 | 0,5 105 | 0,5 425 | 0,5 425",
    );
}
