//! `aspect-ratio` on replaced and non-replaced boxes inside flex and grid
//! (Z row C10 (e)), with block-flow controls.
//!
//! The rows are what Chromium 143.0.7499.4 (the parity oracle) reports for each
//! hand-made body: `<width>x<height> @<left>,<top>` of #t's border box from
//! `getBoundingClientRect`, rounded to 0.01. Written by
//! `trench/z/tools/c10/aspect_ratio_gen.py` on the Atlas hub. No text, so no
//! font metrics; the image is a 40x20 PNG data URL (intrinsic ratio 2:1).
//!
//! Left out (wrong on develop and at this change, or LayoutUnit rounding):
//! Chromium snaps to 1/64 px LayoutUnits; not this item:
//! - grid: ratio 1, three columns, gap
//! - grid: ratio 16/9, auto-fill minmax
//! KNOWN: block flow, not flex/grid (a float's auto width from height through the ratio):
//! - block: height, ratio 2/1, auto width
//! KNOWN: an item with in-flow content keeps its content height (R1 on #869,
//! finding 3; why the rows above have no text or children):
//! - row: flex-basis 50, ratio 2/1, wide child (300x5, Chromium 300x150)
//! - row: flex-basis 50, ratio 2/1, max-height 40, wide child (80x5, Chromium 80x40)
//! KNOWN: min-height over max-height on a row item's height (max wins here,
//! min wins in Chromium; generic, not aspect-ratio):
//! - row: flex-basis 50, ratio 2/1, min-height 40, max-height 30 (80x30, Chromium 80x40)
//! KNOWN: replaced boxes (natural ratio) in flex and grid; next slice:
//! - img nosrc block: width 100, auto 2 (100x100, Chromium 100x50)
//! - img flex row: flex 1, align start
//! - img flex column: width
//! - img flex column: stretch
//! - img grid: one column
//! - img grid: two columns width 100%
//! - img grid: justify start
//! - img grid: ratio 1, width 100%
//! - canvas flex row: width
//! - canvas grid: width 100%
//! - card: grid column, img 100% ratio 16/9 cover


use super::*;

const CASES: &[(&str, &str, &str)] = &[
    ("block: width, ratio 2/1", r##"<div id=t style="width:200px;aspect-ratio:2/1"></div>"##, "200x100 @0,0"),
    ("block: auto width, ratio 4/1", r##"<div id=t style="aspect-ratio:4/1"></div>"##, "800x200 @0,0"),
    ("flex row: width, ratio 2/1", r##"<div id=c style="display:flex;width:400px;"><div id=t style="width:200px;aspect-ratio:2/1"></div></div>"##, "200x100 @0,0"),
    ("flex row: width, ratio 2/1, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="width:200px;aspect-ratio:2/1"></div></div>"##, "200x100 @0,0"),
    ("flex row: width, ratio 2/1, definite container height", r##"<div id=c style="display:flex;width:400px;height:300px"><div id=t style="width:200px;aspect-ratio:2/1"></div></div>"##, "200x300 @0,0"),
    ("flex row: width, ratio 2/1, container height, align start", r##"<div id=c style="display:flex;width:400px;height:300px;align-items:flex-start"><div id=t style="width:200px;aspect-ratio:2/1"></div></div>"##, "200x100 @0,0"),
    ("flex row: flex-basis, ratio 2/1", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 120px;aspect-ratio:2/1"></div></div>"##, "120x60 @0,0"),
    ("flex row: flex 1, ratio 4/1", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:1;aspect-ratio:4/1"></div></div>"##, "400x100 @0,0"),
    ("flex row: two flex 1, ratio 1", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:1;aspect-ratio:1"></div><div style="flex:1;height:10px"></div></div>"##, "200x200 @0,0"),
    ("flex row: height, ratio 2/1, auto width", r##"<div id=c style="display:flex;width:400px;"><div id=t style="height:50px;aspect-ratio:2/1"></div></div>"##, "100x50 @0,0"),
    ("flex row: height, ratio 2/1, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="height:50px;aspect-ratio:2/1"></div></div>"##, "100x50 @0,0"),
    ("flex row: width %, ratio 16/9", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="width:50%;aspect-ratio:16/9"></div></div>"##, "200x112.5 @0,0"),
    ("flex row: ratio with sibling", r##"<div id=c style="display:flex;width:400px;"><div id=t style="width:100px;aspect-ratio:1"></div><div style="width:50px;height:20px"></div></div>"##, "100x100 @0,0"),
    ("flex row: ratio 1, align center", r##"<div id=c style="display:flex;width:400px;height:200px;align-items:center"><div id=t style="width:80px;aspect-ratio:1"></div></div>"##, "80x80 @0,60"),
    ("flex row: auto ratio 3/1 no size, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><div id=t style="aspect-ratio:3/1;min-width:0"></div></div>"##, "0x0 @0,0"),
    ("flex column: width, ratio 2/1", r##"<div id=c style="display:flex;width:400px;flex-direction:column"><div id=t style="width:200px;aspect-ratio:2/1"></div></div>"##, "200x100 @0,0"),
    ("flex column: stretch, ratio 4/1", r##"<div id=c style="display:flex;width:400px;flex-direction:column"><div id=t style="aspect-ratio:4/1"></div></div>"##, "400x100 @0,0"),
    ("flex column: align start, width, ratio 4/1", r##"<div id=c style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><div id=t style="width:100px;aspect-ratio:4/1"></div></div>"##, "100x25 @0,0"),
    ("flex column: height, ratio 2/1, align start", r##"<div id=c style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><div id=t style="height:50px;aspect-ratio:2/1"></div></div>"##, "100x50 @0,0"),
    ("flex column: ratio 1, sibling", r##"<div id=c style="display:flex;width:400px;flex-direction:column"><div id=t style="aspect-ratio:2/1"></div><div style="height:20px"></div></div>"##, "400x200 @0,0"),
    ("grid: ratio 2/1, one column", r##"<div id=c style="display:grid;width:400px;"><div id=t style="aspect-ratio:2/1"></div></div>"##, "400x200 @0,0"),
    ("grid: ratio 2/1, two columns", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><div id=t style="aspect-ratio:2/1"></div><div></div></div>"##, "200x100 @0,0"),
    ("grid: ratio 2/1, justify start, width", r##"<div id=c style="display:grid;width:400px;justify-items:start"><div id=t style="width:100px;aspect-ratio:2/1"></div></div>"##, "100x50 @0,0"),
    ("grid: ratio 2/1, align start", r##"<div id=c style="display:grid;width:400px;align-items:start"><div id=t style="aspect-ratio:2/1"></div></div>"##, "400x200 @0,0"),
    ("grid: ratio 1, fixed rows", r##"<div id=c style="display:grid;width:400px;grid-template-rows:100px;grid-template-columns:300px"><div id=t style="aspect-ratio:1"></div></div>"##, "300x300 @0,0"),
    ("grid: ratio 1, fixed rows, align start", r##"<div id=c style="display:grid;width:400px;grid-template-rows:100px;grid-template-columns:300px;align-items:start"><div id=t style="aspect-ratio:1"></div></div>"##, "300x300 @0,0"),
    ("img block: width only", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px">"##, "100x50 @0,0"),
    ("img block: aspect-ratio 1 overrides", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px;aspect-ratio:1">"##, "100x100 @0,0"),
    ("img block: aspect-ratio auto 1", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px;aspect-ratio:auto 1">"##, "100x50 @0,0"),
    ("img flex row: width", r##"<div id=c style="display:flex;width:400px;"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px"></div>"##, "100x50 @0,0"),
    ("img flex row: width, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px"></div>"##, "100x50 @0,0"),
    ("img flex row: ratio 1, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px;aspect-ratio:1"></div>"##, "100x100 @0,0"),
    ("img flex row: height, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;height:60px"></div>"##, "120x60 @0,0"),
    ("img flex column: align start", r##"<div id=c style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("canvas flex row: ratio 1", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><canvas id=t width=300 height=150 style="display:block;width:100px;aspect-ratio:1"></canvas></div>"##, "100x100 @0,0"),
    ("card: flex item, img 100% ratio 16/9", r##"<div id=c style="display:flex;width:400px;"><div style="flex:1"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%;aspect-ratio:16/9"></div><div style="flex:1"></div></div>"##, "200x112.5 @0,0"),
    ("card: div ratio wrapper in flex 1", r##"<div id=c style="display:flex;width:400px;"><div style="flex:1"><div id=t style="aspect-ratio:16/9"></div></div><div style="flex:1"></div></div>"##, "200x112.5 @0,0"),
    // R1 on #869: wrapping columns, min/max through the ratio, `auto <ratio>`.
    ("col wrap height 400: ratio 2/1", r##"<div style="display:flex;width:400px;flex-direction:column;flex-wrap:wrap;height:400px"><div id=t style="aspect-ratio:2/1"></div></div>"##, "400x0 @0,0"),
    ("col wrap-reverse height 400: ratio 2/1", r##"<div style="display:flex;width:400px;flex-direction:column;flex-wrap:wrap-reverse;height:400px"><div id=t style="aspect-ratio:2/1"></div></div>"##, "400x0 @0,0"),
    ("col wrap no height: ratio 2/1", r##"<div style="display:flex;width:400px;flex-direction:column;flex-wrap:wrap"><div id=t style="aspect-ratio:2/1"></div></div>"##, "400x0 @0,0"),
    ("col nowrap height 400: ratio 2/1", r##"<div style="display:flex;width:400px;flex-direction:column;height:400px"><div id=t style="aspect-ratio:2/1"></div></div>"##, "400x200 @0,0"),
    ("col stretch: ratio 2/1, max-width 200", r##"<div style="display:flex;width:400px;flex-direction:column;"><div id=t style="aspect-ratio:2/1;max-width:200px"></div></div>"##, "200x100 @0,0"),
    ("col stretch: ratio 2/1, min-width 600", r##"<div style="display:flex;width:400px;flex-direction:column;"><div id=t style="aspect-ratio:2/1;min-width:600px"></div></div>"##, "600x300 @0,0"),
    ("col stretch: ratio 2/1, max-width 200, padding 10 content-box", r##"<div style="display:flex;width:400px;flex-direction:column;"><div id=t style="aspect-ratio:2/1;max-width:200px;padding:10px"></div></div>"##, "220x120 @0,0"),
    ("col stretch: ratio 2/1, max-width 200, padding 10 border-box", r##"<div style="display:flex;width:400px;flex-direction:column;"><div id=t style="aspect-ratio:2/1;max-width:200px;padding:10px;box-sizing:border-box"></div></div>"##, "200x100 @0,0"),
    ("col stretch: ratio 2/1, max-height 50", r##"<div style="display:flex;width:400px;flex-direction:column;"><div id=t style="aspect-ratio:2/1;max-height:50px"></div></div>"##, "400x50 @0,0"),
    ("col start: width 300, ratio 2/1, max-width 100", r##"<div style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><div id=t style="width:300px;aspect-ratio:2/1;max-width:100px"></div></div>"##, "100x50 @0,0"),
    ("row: flex-basis 200, ratio 2/1, min-height 150", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;min-height:150px"></div></div>"##, "300x150 @0,0"),
    ("row: flex-basis 200, ratio 2/1, max-height 50", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;max-height:50px"></div></div>"##, "200x50 @0,0"),
    ("row: ratio 2/1, min-height 150", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="aspect-ratio:2/1;min-height:150px"></div></div>"##, "300x150 @0,0"),
    ("row: flex-basis 200, ratio 2/1, min-height 150, overflow hidden", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;min-height:150px;overflow:hidden"></div></div>"##, "200x150 @0,0"),
    ("row: flex-basis 200, ratio 2/1, min-height 150, min-width 0", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;min-height:150px;min-width:0"></div></div>"##, "200x150 @0,0"),
    ("row: width 200, ratio 2/1, min-height 150", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="width:200px;aspect-ratio:2/1;min-height:150px"></div></div>"##, "200x150 @0,0"),
    ("row: flex-basis 200, ratio 2/1, min-height 150, border-box pad 10", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;min-height:150px;padding:10px;box-sizing:border-box"></div></div>"##, "300x150 @0,0"),
    ("row: flex-basis 200, ratio 2/1, min-height 150, content-box pad 10", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;min-height:150px;padding:10px"></div></div>"##, "320x170 @0,0"),
    ("row: height 50, ratio 2/1, max-height 30", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="height:50px;aspect-ratio:2/1;max-height:30px"></div></div>"##, "60x30 @0,0"),
    ("row: height 50, ratio 2/1, min-height 80", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="height:50px;aspect-ratio:2/1;min-height:80px"></div></div>"##, "160x80 @0,0"),
    ("row: height 50, ratio 2/1, max-height 30, pad 5 content-box", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="height:50px;aspect-ratio:2/1;max-height:30px;padding:5px"></div></div>"##, "70x40 @0,0"),
    ("row: width 160, auto 16/9, align start", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="width:160px;aspect-ratio:auto 16/9"></div></div>"##, "160x90 @0,0"),
    ("row: width 160, 16/9 auto, align start", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="width:160px;aspect-ratio:16/9 auto"></div></div>"##, "160x90 @0,0"),
    ("block: width 160, auto 16/9", r##"<div id=t style="width:160px;aspect-ratio:auto 16/9"></div>"##, "160x90 @0,0"),
    ("block: width 160, 16/9 auto", r##"<div id=t style="width:160px;aspect-ratio:16/9 auto"></div>"##, "160x90 @0,0"),
    ("block: width 160, auto 2", r##"<div id=t style="width:160px;aspect-ratio:auto 2"></div>"##, "160x80 @0,0"),
    ("block: width 160, auto auto (invalid)", r##"<div id=t style="width:160px;aspect-ratio:2;aspect-ratio:auto auto"></div>"##, "160x80 @0,0"),
    ("block: width 160, 2/1 then auto 16/9 auto (invalid)", r##"<div id=t style="width:160px;aspect-ratio:2/1;aspect-ratio:auto 16/9 auto"></div>"##, "160x80 @0,0"),
    ("col stretch: auto 16/9", r##"<div style="display:flex;width:400px;flex-direction:column;"><div id=t style="aspect-ratio:auto 16/9"></div></div>"##, "400x225 @0,0"),
    ("img block: width 100, auto 1", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px;aspect-ratio:auto 1">"##, "100x50 @0,0"),
    ("img block: width 100, 1 auto", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px;aspect-ratio:1 auto">"##, "100x50 @0,0"),
    ("img block: width 100, 1", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px;aspect-ratio:1">"##, "100x100 @0,0"),
    ("row: flex 1, ratio 2/1, min-height 300", r##"<div style="display:flex;width:400px;align-items:flex-start"><div id=t style="flex:1;aspect-ratio:2/1;min-height:300px"></div></div>"##, "600x300 @0,0"),
    ("row stretch: flex-basis 200, ratio 2/1, min-height 150", r##"<div style="display:flex;width:400px;"><div id=t style="flex:0 0 200px;aspect-ratio:2/1;min-height:150px"></div></div>"##, "300x150 @0,0"),
    ("col: flex-basis 50, ratio 2/1, min-width 300, align start", r##"<div style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><div id=t style="flex:0 0 50px;aspect-ratio:2/1;min-width:300px"></div></div>"##, "300x150 @0,0"),
    ("col: height 50, ratio 2/1, min-width 300, align start", r##"<div style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><div id=t style="height:50px;aspect-ratio:2/1;min-width:300px"></div></div>"##, "300x50 @0,0"),
];

const READ: &str = "(function () { var r = document.getElementById('t').getBoundingClientRect(); \
    var f = function (v) { return String(Math.round(v * 100) / 100); }; \
    return f(r.width) + 'x' + f(r.height) + ' @' + f(r.left) + ',' + f(r.top); })()";

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn aspect_ratio_in_flex_and_grid_matches_chromium() {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds::new(0, 0, 800, 600))
        .expect("headless view");
    let mut wrong = Vec::new();
    for (name, body, want) in CASES {
        let html = format!("<!doctype html><style>body{{margin:0}}</style><body>{body}");
        engine.load_html(view, &html).expect("load_html");
        let value = engine.execute_script(view, READ).expect("script");
        let got = value
            .strip_prefix("String(\"")
            .and_then(|v| v.strip_suffix("\")"))
            .unwrap_or(&value);
        if got != *want {
            wrong.push(format!("{name}: {got}, Chromium {want}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "{} of {} wrong:\n{}",
        wrong.len(),
        CASES.len(),
        wrong.join("\n")
    );
}
