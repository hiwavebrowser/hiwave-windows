//! Replaced boxes (`<img>`, `<canvas>`) with a natural ratio inside flex
//! and grid, with and without `aspect-ratio` (Z row C10 (e), second slice).
//!
//! The rows are what Chromium 143.0.7499.4 (the parity oracle) reports for each
//! hand-made body: `<width>x<height> @<left>,<top>` of #t's border box from
//! `getBoundingClientRect`, rounded to 0.01. Written by
//! `trench/z/tools/c10/replaced_ratio_gen.py` on the Atlas hub. No text, so no
//! font metrics; the image is a 40x20 PNG data URL (natural ratio 2:1), the
//! canvas 300x150 unless the row says otherwise.
//!
//! Left out (wrong on develop and at this change, or LayoutUnit rounding):
//! KNOWN: <canvas> has no natural size anywhere (built as an empty box, 0x0 in block flow too); a box-builder item, not flex/grid:
//! - canvas block: none
//! - canvas grid: one column
//! - canvas grid: width 100%
//! - canvas grid: justify start
//! - canvas flex row: width
//! - canvas flex column: width
//! - canvas flex column: stretch
//! KNOWN: an author `height: auto` loses to the height= attribute (the builder cannot tell it from the initial auto); block flow too:
//! - img block: width 100%, height auto attrs
//! - img grid: width 100%, height auto attrs
//! - card: grid column, img attrs 800x400 max-width 100% height auto
//! - card: three columns gap, img attrs 800x400 max-width 100% height auto
//! KNOWN: a percentage max-width scales a height= attribute down with the width (layout_image_in); it stands in for the author `height: auto` above, so the two are fixed together:
//! - img grid: two columns, attrs 800x400 max-width 100%
//! - table: cell img attrs max-width 100%, narrow
//! KNOWN: a table caption is placed on top whatever caption-side says and is left out of the table's box (C10 (g)/(j)); the width (258) is right, the height and y are not:
//! - table figure: float, link, img attrs max-width 100%, caption
//! KNOWN: replaced flex items keep their natural height when the flexed or stretched width changes (flex.rs, after #869):
//! - img flex row: flex 1, align start
//! - img flex column: width
//! - img flex column: stretch
//! - img flex column: width 100%


use super::*;

const CASES: &[(&str, &str, &str)] = &[
    ("img block: none", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;">"##, "40x20 @0,0"),
    ("img block: width 50%", r##"<img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:50%">"##, "400x200 @0,0"),
    ("img grid: one column", r##"<div id=c style="display:grid;width:400px;"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img grid: one column, width", r##"<div id=c style="display:grid;width:400px;"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px"></div>"##, "100x50 @0,0"),
    ("img grid: one column, height", r##"<div id=c style="display:grid;width:400px;"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;height:60px"></div>"##, "120x60 @0,0"),
    ("img grid: two columns", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"><div></div></div>"##, "40x20 @0,0"),
    ("img grid: two columns width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"><div></div></div>"##, "200x100 @0,0"),
    ("img grid: two columns width 50%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:50%"><div></div></div>"##, "100x50 @0,0"),
    ("img grid: two columns max-width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;max-width:100%"><div></div></div>"##, "40x20 @0,0"),
    ("img grid: fixed column 300px", r##"<div id=c style="display:grid;width:400px;grid-template-columns:300px"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img grid: justify start", r##"<div id=c style="display:grid;width:400px;justify-items:start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img grid: align start", r##"<div id=c style="display:grid;width:400px;align-items:start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img grid: justify start, width", r##"<div id=c style="display:grid;width:400px;justify-items:start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100px"></div>"##, "100x50 @0,0"),
    ("img grid: ratio 1, width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%;aspect-ratio:1"><div></div></div>"##, "200x200 @0,0"),
    ("img grid: fixed row 100px", r##"<div id=c style="display:grid;width:400px;grid-template-rows:100px;grid-template-columns:300px"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img grid: three columns gap", r##"<div id=c style="display:grid;width:400px;grid-template-columns:repeat(3,1fr);gap:20px"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"><div></div><div></div></div>"##, "120x60 @0,0"),
    ("img flex row: none", r##"<div id=c style="display:flex;width:400px;"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img flex row: align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("img flex row: height, align start", r##"<div id=c style="display:flex;width:400px;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;height:60px"></div>"##, "120x60 @0,0"),
    ("img flex column: align start", r##"<div id=c style="display:flex;width:400px;flex-direction:column;align-items:flex-start"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;"></div>"##, "40x20 @0,0"),
    ("card: grid column, img 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><div><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"></div><div></div></div>"##, "200x100 @0,0"),
    ("card: grid column, img 100% ratio 16/9 cover", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><div><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%;aspect-ratio:16/9;object-fit:cover"></div><div></div></div>"##, "200x112.5 @0,0"),
    ("card: flex 1, img 100%", r##"<div id=c style="display:flex;width:400px;"><div style="flex:1"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"></div><div style="flex:1"></div></div>"##, "200x100 @0,0"),
    ("card: flex 1, img 100% ratio 16/9", r##"<div id=c style="display:flex;width:400px;"><div style="flex:1"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%;aspect-ratio:16/9"></div><div style="flex:1"></div></div>"##, "200x112.5 @0,0"),
    ("card: grid column, img attrs 800x400 max-width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><div><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" width=800 height=400 style="display:block;max-width:100%"></div><div></div></div>"##, "200x400 @0,0"),
    ("card: three columns, img attrs 800x400 max-width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:repeat(3,1fr)"><div><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" width=800 height=400 style="display:block;max-width:100%"></div><div></div><div></div></div>"##, "133.33x400 @0,0"),
    ("img grid: two columns, attrs 800x400 width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:1fr 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" width=800 height=400 style="display:block;width:100%"><div></div></div>"##, "200x400 @0,0"),
    ("img grid: self stretch both, row 100px", r##"<div id=c style="display:grid;width:400px;grid-template-rows:100px"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;justify-self:stretch;align-self:stretch"></div>"##, "400x100 @0,0"),
    ("img grid: justify-self stretch", r##"<div id=c style="display:grid;width:400px;"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;justify-self:stretch"></div>"##, "400x200 @0,0"),
    ("img grid: align-self stretch, row 100px", r##"<div id=c style="display:grid;width:400px;grid-template-rows:100px"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;align-self:stretch"></div>"##, "200x100 @0,0"),
    ("img grid: auto column, width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:auto 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"><div></div></div>"##, "40x20 @0,0"),
    ("img grid: max-content column, width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:max-content 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"><div></div></div>"##, "40x20 @0,0"),
    ("img grid: auto column, max-width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:auto 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;max-width:100%"><div></div></div>"##, "40x20 @0,0"),
    ("img grid: min-content column, width 100%", r##"<div id=c style="display:grid;width:400px;grid-template-columns:min-content 1fr"><img id=t src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" style="display:block;width:100%"><div></div></div>"##, "0x0 @0,0"),
    ("table figure: float, link, img attrs max-width 100%", r##"<figure id=t style="float:right;display:table;margin:0 0 8px 16px;"><a style="display:block;border:1px solid #888;padding:3px"><img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" width=250 height=125 style="display:block;max-width:100%;height:auto"></a></figure>"##, "258x133 @542,0"),
    ("table: cell img attrs max-width 100%", r##"<div style="width:400px"><table id=t style="border-spacing:0"><tr><td style="padding:0"><img src="data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACgAAAAUCAIAAABwJOjsAAAAI0lEQVR42u3NsQ0AAAjAoP7/tJ7hIAk7TZ1ILBaLxWKx+E+8ffodDqIehz0AAAAASUVORK5CYII=" width=250 height=125 style="display:block;max-width:100%"></td></tr></table></div>"##, "250x125 @0,0"),
];

const READ: &str = "(function () { var r = document.getElementById('t').getBoundingClientRect(); \
    var f = function (v) { return String(Math.round(v * 100) / 100); }; \
    return f(r.width) + 'x' + f(r.height) + ' @' + f(r.left) + ',' + f(r.top); })()";

#[test]
#[cfg(all(target_os = "macos", feature = "headless"))]
fn replaced_ratio_in_flex_and_grid_matches_chromium() {
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
