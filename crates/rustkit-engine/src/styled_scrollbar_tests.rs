//! Hub row CH2.E7: styled scrollbars. A scroll container with
//! `::-webkit-scrollbar { width: 6px }` (or `scrollbar-width` /
//! `scrollbar-color`) paints a classic, space-taking scrollbar inside its
//! border, with Chrome's thumb geometry, and the thumb drags and the track
//! pages. Expected numbers are Chrome for Testing 153.0.8010.12 (Linux,
//! classic scrollbars) on the same pages:
//! `tools/parity_oracle/scrollbar_geometry_log.mjs`.
//!
//! These tests need styled bars shown: `RUSTKIT_HIDE_SCROLLBARS=1` (or a
//! `rustkit_layout::scroll::set_scrollbars_hidden(true)` anywhere in the
//! test binary) hides them process-wide, as parity-capture does, and every
//! test here then fails on a missing bar.

use super::*;
use rustkit_css::Color;
use rustkit_layout::{DisplayCommand, Rect};

const THUMB: Color = Color { r: 0x88, g: 0x88, b: 0x88, a: 1.0 };
const THUMB_HOVER: Color = Color { r: 0x55, g: 0x55, b: 0x55, a: 1.0 };
const TRACK: Color = Color { r: 0, g: 0, b: 255, a: 1.0 };

/// A 300×200 `overflow: auto` box at (10, 20) holding 600px of content.
fn page(extra: &str) -> String {
    format!(
        "<!doctype html><style>body{{margin:0}}\
         #s{{position:absolute;left:10px;top:20px;width:300px;height:200px;overflow:auto}}\
         #s::-webkit-scrollbar{{width:6px}}\
         #s::-webkit-scrollbar-track{{background:rgb(0,0,255)}}\
         #s::-webkit-scrollbar-thumb{{background:#888;border-radius:3px}}\
         #s::-webkit-scrollbar-thumb:hover{{background:#555}}\
         #c{{height:600px}}{extra}</style><div id=s><div id=c></div></div>"
    )
}

fn loaded(html: &str) -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let view = engine
        .create_headless_view(Bounds { x: 0, y: 0, width: 800, height: 600 })
        .expect("view");
    engine.load_html(view, html).expect("load");
    (engine, view)
}

fn read(engine: &mut Engine, view: EngineViewId, expr: &str) -> String {
    let got = engine.execute_script(view, &format!("String({expr})")).expect("script");
    got.trim_start_matches("String(\"").trim_end_matches("\")").to_string()
}

/// The rect of every fill of `color` in the view's display list.
fn fills(engine: &Engine, view: EngineViewId, color: Color) -> Vec<Rect> {
    let list = engine.views[&view].display_list.as_ref().expect("display list");
    list.commands
        .iter()
        .filter_map(|c| match c {
            DisplayCommand::SolidColor(c, rect) if *c == color => Some(*rect),
            DisplayCommand::RoundedRect { color: c, rect, .. } if *c == color => Some(*rect),
            _ => None,
        })
        .collect()
}

fn one(engine: &Engine, view: EngineViewId, color: Color) -> Rect {
    let got = fills(engine, view, color);
    assert_eq!(got.len(), 1, "one fill of {color:?}, got {got:?}");
    got[0]
}

#[track_caller]
fn assert_rect(got: Rect, want: (f32, f32, f32, f32)) {
    let near = |a: f32, b: f32| (a - b).abs() < 0.01;
    assert!(
        near(got.x, want.0) && near(got.y, want.1) && near(got.width, want.2) && near(got.height, want.3),
        "got {got:?}, want {want:?}"
    );
}

#[test]
fn a_six_pixel_track_paints_at_the_right_edge() {
    let (engine, view) = loaded(&page(""));
    assert_rect(one(&engine, view, TRACK), (304.0, 20.0, 6.0, 200.0));
}

#[test]
fn the_thumb_is_viewport_over_content_of_the_track_at_the_top() {
    // Chrome: 200 * 200 / 600 = 66.7, rounded to 67, at the top.
    let (engine, view) = loaded(&page(""));
    let thumb = one(&engine, view, THUMB);
    assert_rect(thumb, (304.0, 20.0, 6.0, 67.0));
    let rounded = engine.views[&view].display_list.as_ref().unwrap().commands.iter().any(|c| {
        matches!(c, DisplayCommand::RoundedRect { color, radius, .. }
            if *color == THUMB && (radius.top_left.h - 3.0).abs() < 0.01)
    });
    assert!(rounded, "the thumb takes its 3px border-radius");
}

#[test]
fn after_scrolling_a_third_the_thumb_moves_a_third() {
    // Chrome: scrollTop 200 of 400 puts the thumb at (200 - 67) / 2 = 66.
    let (mut engine, view) = loaded(&page(""));
    read(&mut engine, view, "document.getElementById('s').scrollTop = 200");
    assert_rect(one(&engine, view, THUMB), (304.0, 86.0, 6.0, 67.0));
}

#[test]
fn the_scrollbar_takes_its_width_from_the_content_box() {
    // Chrome: clientWidth 294, offsetWidth 300, the child 294 wide.
    let (mut engine, view) = loaded(&page(""));
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), $s.clientWidth, $s.offsetWidth, \
         document.getElementById('c').getBoundingClientRect().width, $s.scrollHeight].slice(1).join(' ')",
    );
    assert_eq!(got, "294 300 294 600");
}

#[test]
fn scrollbar_width_none_paints_nothing_and_takes_no_space() {
    let (mut engine, view) = loaded(&page("#s{scrollbar-width:none}"));
    assert!(fills(&engine, view, TRACK).is_empty());
    assert!(fills(&engine, view, THUMB).is_empty());
    assert_eq!(read(&mut engine, view, "document.getElementById('c').getBoundingClientRect().width"), "300");
}

#[test]
fn display_none_on_the_scrollbar_paints_nothing_and_takes_no_space() {
    let (mut engine, view) = loaded(&page("#s::-webkit-scrollbar{display:none}"));
    assert!(fills(&engine, view, TRACK).is_empty());
    assert!(fills(&engine, view, THUMB).is_empty());
    assert_eq!(read(&mut engine, view, "document.getElementById('s').clientWidth"), "300");
}

#[test]
fn auto_without_overflow_takes_no_space_and_scroll_always_does() {
    let (mut engine, view) = loaded(&page("#c{height:50px}"));
    assert!(fills(&engine, view, TRACK).is_empty());
    assert_eq!(read(&mut engine, view, "document.getElementById('s').clientWidth"), "300");
    // `overflow: scroll` shows both bars (6px wide, 15px tall: no
    // `height`), and no thumb when there is nothing to scroll.
    let (mut engine, view) = loaded(&page("#c{height:50px}#s{overflow:scroll}"));
    assert_eq!(read(&mut engine, view, "document.getElementById('s').clientWidth"), "294");
    assert!(fills(&engine, view, THUMB).is_empty());
    assert_eq!(fills(&engine, view, TRACK).len(), 2, "a vertical and a horizontal track");
}

#[test]
fn the_page_does_not_see_the_pseudo_elements_declarations() {
    // `width: 6px` and the thumb's background are the scrollbar's, not
    // the element's.
    let (mut engine, view) = loaded(&page(""));
    let got = read(
        &mut engine,
        view,
        "[getComputedStyle(document.getElementById('s')).backgroundColor, \
         getComputedStyle(document.getElementById('s')).borderTopLeftRadius].join(' ')",
    );
    assert_eq!(got, "rgba(0, 0, 0, 0) 0px");
}

#[test]
fn the_standard_properties_win_over_the_pseudo_elements() {
    // Chrome 121+: `scrollbar-color` turns the `::-webkit-scrollbar` rules
    // off; `thin` lays out 10px.
    let (mut engine, view) = loaded(&page("#s{scrollbar-width:thin;scrollbar-color:rgb(255,0,0) rgb(0,255,0)}"));
    assert!(fills(&engine, view, THUMB).is_empty());
    assert_eq!(read(&mut engine, view, "document.getElementById('s').clientWidth"), "290");
    let track = one(&engine, view, Color { r: 0, g: 255, b: 0, a: 1.0 });
    assert_rect(track, (300.0, 20.0, 10.0, 200.0));
    assert_eq!(fills(&engine, view, Color { r: 255, g: 0, b: 0, a: 1.0 }).len(), 1);
}

#[test]
fn dragging_the_thumb_fifty_pixels_scrolls_in_proportion() {
    // Chrome: 50 * (600 - 200) / (200 - 67) = 150.
    let (mut engine, view) = loaded(&page(""));
    assert!(engine.mouse_down_at_point(view, 307.0, 30.0) || true);
    engine.mouse_move_at_point(view, 307.0, 55.0);
    engine.mouse_move_at_point(view, 307.0, 80.0);
    engine.click_at_point(view, 307.0, 80.0);
    assert_eq!(read(&mut engine, view, "document.getElementById('s').scrollTop"), "150");
    // The release ends the drag.
    engine.mouse_move_at_point(view, 307.0, 150.0);
    assert_eq!(read(&mut engine, view, "document.getElementById('s').scrollTop"), "150");
}

#[test]
fn a_press_on_the_track_pages_by_seven_eighths() {
    // Chrome: a press below the thumb scrolls 0.875 * 200 = 175.
    let (mut engine, view) = loaded(&page(""));
    engine.mouse_down_at_point(view, 307.0, 170.0);
    engine.click_at_point(view, 307.0, 170.0);
    assert_eq!(read(&mut engine, view, "document.getElementById('s').scrollTop"), "175");
    // And above it, back.
    engine.mouse_down_at_point(view, 307.0, 25.0);
    engine.click_at_point(view, 307.0, 25.0);
    assert_eq!(read(&mut engine, view, "document.getElementById('s').scrollTop"), "0");
}

#[test]
fn the_thumb_takes_its_hover_background_under_the_pointer() {
    let (mut engine, view) = loaded(&page(""));
    engine.mouse_move_at_point(view, 307.0, 40.0);
    assert_rect(one(&engine, view, THUMB_HOVER), (304.0, 20.0, 6.0, 67.0));
    assert!(fills(&engine, view, THUMB).is_empty());
    engine.mouse_move_at_point(view, 100.0, 100.0);
    assert_eq!(fills(&engine, view, THUMB).len(), 1);
}

#[test]
fn the_root_scrollbar_paints_at_the_viewport_edge() {
    // Chrome: the viewport's bar is 6px at x 794; its thumb 600 * 600 /
    // 2400 = 150 tall, and at 150 once the window scrolls 600.
    let html = "<!doctype html><style>body{margin:0}::-webkit-scrollbar{width:6px}\
                ::-webkit-scrollbar-thumb{background:#888}#c{height:2400px}</style><div id=c></div>";
    let (mut engine, view) = loaded(html);
    assert_eq!(read(&mut engine, view, "document.getElementById('c').getBoundingClientRect().width"), "794");
    engine.render(view).expect("render");
    let thumb_in = |engine: &Engine| -> Vec<Rect> {
        let commit = engine.latest_commit(view).expect("commit");
        let list = commit.list.as_ref().expect("list");
        let mut fixed = false;
        let mut found = Vec::new();
        for command in &list.commands {
            match command {
                DisplayCommand::PushViewportFixed => fixed = true,
                DisplayCommand::PopViewportFixed => fixed = false,
                DisplayCommand::SolidColor(c, rect) if fixed && *c == THUMB => found.push(*rect),
                _ => {}
            }
        }
        found
    };
    let got = thumb_in(&engine);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_rect(got[0], (794.0, 0.0, 6.0, 150.0));
    engine.scroll_view(view, 0.0, -600.0).expect("scroll");
    let got = thumb_in(&engine);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_rect(got[0], (794.0, 150.0, 6.0, 150.0));
}

#[test]
fn the_box_geometry_script_reads_counts_the_bar_where_chrome_does() {
    // Chrome, with `border: 2px; padding: 4px`: the bar is neither border
    // nor padding to the page. `width` is the used content width.
    let (mut engine, view) = loaded(&page("#s{border:2px solid black;padding:4px}"));
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), getComputedStyle($s).borderRightWidth, \
         getComputedStyle($s).width, $s.clientLeft, $s.clientTop, $s.clientWidth, $s.clientHeight, \
         $s.offsetWidth, $s.offsetHeight, $s.getBoundingClientRect().x, $s.getBoundingClientRect().y, \
         $s.getBoundingClientRect().width, $s.getBoundingClientRect().height, $s.scrollWidth, \
         document.getElementById('c').getBoundingClientRect().width].slice(1).join(' ')",
    );
    assert_eq!(got, "2px 294px 2 2 302 208 312 212 10 20 312 212 302 294");
}

#[test]
fn a_flex_item_scroller_takes_its_bar_from_its_content_box() {
    // Chrome: a `flex: 1` scroller beside a 100px item in a 500px row.
    let html = "<!doctype html><style>body{margin:0}#f{display:flex;height:200px;width:500px}\
                #a{width:100px}#s{flex:1;overflow:auto}#s::-webkit-scrollbar{width:6px}\
                #s::-webkit-scrollbar-thumb{background:#888}#c{height:600px}</style>\
                <div id=f><div id=a></div><div id=s><div id=c></div></div></div>";
    let (mut engine, view) = loaded(html);
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), $s.clientWidth, $s.offsetWidth, \
         document.getElementById('c').getBoundingClientRect().width].slice(1).join(' ')",
    );
    assert_eq!(got, "394 400 394");
    assert_rect(one(&engine, view, THUMB), (494.0, 0.0, 6.0, 67.0));
}

#[test]
fn a_column_flex_item_scroller_takes_its_bar_too() {
    // Chrome: the chrome's sidebar shape, a header over a `flex: 1;
    // min-height: 0` list in a 300px column.
    let html = "<!doctype html><style>body{margin:0}#f{display:flex;flex-direction:column;\
                height:300px;width:400px}#h{height:50px}#s{flex:1;min-height:0;overflow:auto}\
                #s::-webkit-scrollbar{width:6px}#s::-webkit-scrollbar-thumb{background:#888}\
                #c{height:600px}</style><div id=f><div id=h></div><div id=s><div id=c></div></div></div>";
    let (mut engine, view) = loaded(html);
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), $s.clientWidth, $s.clientHeight, $s.offsetWidth, \
         document.getElementById('c').getBoundingClientRect().width].slice(1).join(' ')",
    );
    assert_eq!(got, "394 250 400 394");
}

const EVENT_LOG: &str = "<script>window.log=[];['pointerdown','mousedown','pointerup','mouseup','click']\
     .forEach(function(t){document.addEventListener(t,function(e){log.push(t+':'+(e.target.id||e.target.tagName))},true)})</script>";

#[test]
fn a_press_on_a_bar_is_the_scrollers_and_fires_no_click() {
    // Chrome: down and up at the scroller, no `click`, for a thumb drag
    // and for a track press alike.
    for (x, y) in [(307.0, 30.0), (307.0, 170.0)] {
        let (mut engine, view) = loaded(&format!("{}{EVENT_LOG}", page("")));
        engine.mouse_down_at_point(view, x, y);
        engine.mouse_move_at_point(view, x, y + 20.0);
        engine.click_at_point(view, x, y + 20.0);
        assert_eq!(
            read(&mut engine, view, "log.join(' ')"),
            "pointerdown:s mousedown:s pointerup:s mouseup:s",
            "press at {x},{y}"
        );
    }
}

#[test]
fn a_press_on_the_viewports_bar_is_the_root_elements() {
    // Chrome: a fixed `100vw` box under the bar does not hear it.
    let html = format!(
        "<!doctype html><style>body{{margin:0}}::-webkit-scrollbar{{width:6px}}\
         ::-webkit-scrollbar-thumb{{background:#888}}#big{{height:2400px}}\
         #fx{{position:fixed;top:0;left:0;width:100vw;height:600px}}</style>\
         <div id=big></div><div id=fx></div>{EVENT_LOG}"
    );
    let (mut engine, view) = loaded(&html);
    engine.mouse_down_at_point(view, 797.0, 30.0);
    engine.mouse_move_at_point(view, 797.0, 50.0);
    engine.click_at_point(view, 797.0, 50.0);
    assert_eq!(
        read(&mut engine, view, "log.join(' ')"),
        "pointerdown:HTML mousedown:HTML pointerup:HTML mouseup:HTML"
    );
    // The thumb (150 tall) went 20px: 20 * 1800 / 450 = 80.
    assert_eq!(read(&mut engine, view, "window.scrollY"), "80");
}

#[test]
fn a_relayout_keeps_the_bar_while_it_overflows_and_drops_it_after() {
    let (mut engine, view) = loaded(&page(""));
    let width = "document.getElementById('s').clientWidth";
    read(&mut engine, view, "document.getElementById('c').style.height = '700px'");
    assert_eq!(read(&mut engine, view, width), "294");
    read(&mut engine, view, "document.getElementById('c').style.height = '50px'");
    assert_eq!(read(&mut engine, view, width), "300");
    assert!(fills(&engine, view, TRACK).is_empty());
    read(&mut engine, view, "document.getElementById('c').style.height = '600px'");
    assert_eq!(read(&mut engine, view, width), "294");
}

// ---------------------------------------------------------------------------
// R1 on #900. Chrome numbers: Chrome for Testing 153.0.8010.12 (Linux,
// launched without --hide-scrollbars), tools/parity_oracle/scrollbar_events_log.mjs.
// ---------------------------------------------------------------------------

#[test]
fn the_scrollbar_is_its_own_gutter_not_border() {
    // Chrome: borderRightWidth 3px, borderBottomWidth 2px, clientLeft 2,
    // clientTop 2, clientWidth 294, clientHeight 200, offsetWidth 305,
    // the rect 305 wide, the child 294.
    let (mut engine, view) = loaded(&page("#s{border:2px solid black;border-right-width:3px}"));
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), getComputedStyle($s).borderRightWidth, \
         getComputedStyle($s).borderBottomWidth, $s.clientLeft, $s.clientTop, $s.clientWidth, \
         $s.clientHeight, $s.offsetWidth, $s.getBoundingClientRect().width, \
         document.getElementById('c').getBoundingClientRect().width].slice(1).join(' ')",
    );
    assert_eq!(got, "3px 2px 2 2 294 200 305 305 294");
    // The bar sits inside the 3px right border, and the border strip is
    // 3px, not 9.
    assert_rect(one(&engine, view, TRACK), (306.0, 22.0, 6.0, 200.0));
    let black = Color { r: 0, g: 0, b: 0, a: 1.0 };
    let right = fills(&engine, view, black).into_iter().filter(|r| r.height > 200.0 && r.x > 300.0).collect::<Vec<_>>();
    assert_eq!(right.len(), 1, "one right border strip: {right:?}");
    assert_rect(right[0], (312.0, 20.0, 3.0, 204.0));
    // No border at all: Chrome 0px, 0, 294, 300.
    let (mut engine, view) = loaded(&page(""));
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), getComputedStyle($s).borderRightWidth, $s.clientLeft, \
         $s.clientWidth, $s.offsetWidth].slice(1).join(' ')",
    );
    assert_eq!(got, "0px 0 294 300");
}

/// (layout passes, builds) the view has run so far.
fn layout_counts(engine: &Engine, view: EngineViewId) -> (u32, u32) {
    let lay = &engine.views[&view].lay;
    (lay.layout_passes.get(), lay.layout_builds.get())
}

/// Run `script` (a write that changes layout, then a geometry read) and
/// return what it read, with the (passes, builds) it cost.
fn counted(engine: &mut Engine, view: EngineViewId, script: &str) -> (String, (u32, u32)) {
    let before = layout_counts(engine, view);
    let got = read(engine, view, script);
    engine.relayout(view).expect("relayout");
    let after = layout_counts(engine, view);
    (got, (after.0 - before.0, after.1 - before.1))
}

#[test]
fn a_relayout_of_an_overflowing_scroller_takes_one_pass() {
    let (mut engine, view) = loaded(&page(""));
    assert_eq!(read(&mut engine, view, "document.getElementById('s').clientWidth"), "294");
    // The first layout finds the overflow and lays out again for the bar;
    // every relayout after it starts with the bar reserved: as many
    // passes as builds.
    for h in [610, 620, 630] {
        let (got, (passes, builds)) = counted(
            &mut engine,
            view,
            &format!("(document.getElementById('c').style.height = '{h}px', document.getElementById('s').clientWidth)"),
        );
        assert_eq!(got, "294");
        assert!(builds >= 1, "the write was laid out");
        assert_eq!(passes, builds, "one layout pass per build");
    }
    // Content that stops overflowing drops the bar (Chrome: 300), and
    // gets it back when it overflows again.
    assert_eq!(read(&mut engine, view, "(document.getElementById('c').style.height = '50px', document.getElementById('s').clientWidth)"), "300");
    assert!(fills(&engine, view, TRACK).is_empty());
    assert_eq!(read(&mut engine, view, "(document.getElementById('c').style.height = '600px', document.getElementById('s').clientWidth)"), "294");
    let (_, (passes, builds)) = counted(&mut engine, view, "(document.getElementById('c').style.height = '640px', document.getElementById('s').clientWidth)");
    assert!(builds >= 1);
    assert_eq!(passes, builds);
}

#[test]
fn a_relayout_with_the_root_bar_takes_one_pass() {
    let html = "<!doctype html><style>body{margin:0}::-webkit-scrollbar{width:6px}\
                ::-webkit-scrollbar-thumb{background:#888}#c{height:2400px}</style><div id=c></div>";
    let (mut engine, view) = loaded(html);
    let (got, (passes, builds)) = counted(
        &mut engine,
        view,
        "(document.getElementById('c').style.height = '2500px', document.getElementById('c').getBoundingClientRect().width)",
    );
    assert_eq!(got, "794");
    assert!(builds >= 1);
    assert_eq!(passes, builds);
}

#[test]
fn a_flex_item_scroller_matches_chrome_in_a_row_and_a_column() {
    // A row: 100px, the scroller (flex: 1), 120px in a 500px container.
    // Chrome: the scroller at x 100, 280 x 200, clientWidth 274,
    // clientHeight 200, its child 274 wide; the last item at x 380.
    let html = "<!doctype html><style>body{margin:0}#f{display:flex;width:500px;height:200px}\
                #a{width:100px;flex:none}#s{flex:1;overflow:auto;min-width:0}\
                #s::-webkit-scrollbar{width:6px}#s::-webkit-scrollbar-track{background:rgb(0,0,255)}\
                #c{height:600px}#d{width:120px;flex:none}</style>\
                <div id=f><div id=a></div><div id=s><div id=c></div></div><div id=d></div></div>";
    let (mut engine, view) = loaded(html);
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), $r = $s.getBoundingClientRect(), $r.x, $r.width, $r.height, \
         $s.clientWidth, $s.clientHeight, document.getElementById('c').getBoundingClientRect().width, \
         document.getElementById('d').getBoundingClientRect().x].slice(2).join(' ')",
    );
    assert_eq!(got, "100 280 200 274 200 274 380");
    assert_rect(one(&engine, view, TRACK), (374.0, 0.0, 6.0, 200.0));
    // A column: a 50px header over the scroller (flex: 1) in 300 x 400.
    // Chrome: y 50, 300 x 350, clientWidth 294, clientHeight 350, child 294.
    let html = "<!doctype html><style>body{margin:0}#f{display:flex;flex-direction:column;width:300px;height:400px}\
                #h{height:50px;flex:none}#s{flex:1;overflow:auto;min-height:0}\
                #s::-webkit-scrollbar{width:6px}#c{height:600px}</style>\
                <div id=f><div id=h></div><div id=s><div id=c></div></div></div>";
    let (mut engine, view) = loaded(html);
    let got = read(
        &mut engine,
        view,
        "[$s = document.getElementById('s'), $r = $s.getBoundingClientRect(), $r.y, $r.width, $r.height, \
         $s.clientWidth, $s.clientHeight, document.getElementById('c').getBoundingClientRect().width].slice(2).join(' ')",
    );
    assert_eq!(got, "50 300 350 294 350 294");
}

/// Logs the pointer and mouse events the page hears, by target id (or tag).
const EVENT_LOG_AT: &str = "<script>window.log=[];for (const t of ['pointerdown','mousedown','pointerup','mouseup','click'])\
    document.addEventListener(t, e => log.push(t + ':' + (e.target.id || e.target.nodeName) \
    + (t == 'mousedown' ? '@' + e.offsetX : '')), true);</script>";

fn take_log(engine: &mut Engine, view: EngineViewId) -> String {
    read(engine, view, "[log.join(','), log.length = 0][0]")
}

#[test]
fn a_bar_press_goes_to_the_scroller_and_clicks_nothing() {
    // Chrome 153: pointerdown, mousedown (offsetX 297, past the padding
    // box), pointerup and mouseup at the scroller, never at the content
    // under the point, and no click; the focus stays; a press on the
    // content still clicks it.
    let html = format!("{}<input id=i style='position:absolute;top:300px'>{EVENT_LOG_AT}", page(""));
    let (mut engine, view) = loaded(&html);
    read(&mut engine, view, "document.getElementById('i').focus()");
    take_log(&mut engine, view);
    let bar_press = "pointerdown:s,mousedown:s@297,pointerup:s,mouseup:s";
    // The thumb.
    engine.mouse_down_at_point(view, 307.0, 30.0);
    engine.click_at_point(view, 307.0, 30.0);
    assert_eq!(take_log(&mut engine, view), bar_press);
    // The track.
    engine.mouse_down_at_point(view, 307.0, 170.0);
    engine.click_at_point(view, 307.0, 170.0);
    assert_eq!(take_log(&mut engine, view), bar_press);
    // A drag that ends off the bar: the release is still the scroller's.
    engine.mouse_down_at_point(view, 307.0, 30.0);
    engine.mouse_move_at_point(view, 450.0, 80.0);
    engine.click_at_point(view, 450.0, 80.0);
    assert_eq!(take_log(&mut engine, view), bar_press);
    assert_eq!(read(&mut engine, view, "document.activeElement.id"), "i");
    // The content.
    engine.mouse_down_at_point(view, 100.0, 100.0);
    engine.click_at_point(view, 100.0, 100.0);
    assert_eq!(take_log(&mut engine, view), "pointerdown:c,mousedown:c@90,pointerup:c,mouseup:c,click:c");
}

#[test]
fn a_cancelled_mousedown_does_not_stop_the_bar() {
    // Chrome 153: the bar has the press before the page does.
    let html = format!("{}{EVENT_LOG_AT}", page(""));
    let (mut engine, view) = loaded(&html);
    read(&mut engine, view, "document.getElementById('s').addEventListener('mousedown', e => e.preventDefault())");
    engine.mouse_down_at_point(view, 307.0, 170.0);
    engine.click_at_point(view, 307.0, 170.0);
    assert_eq!(read(&mut engine, view, "document.getElementById('s').scrollTop"), "175");
}

#[test]
fn a_press_on_the_viewport_bar_goes_to_the_root_element() {
    // Chrome 153: a fixed 100vw box under the viewport's bar hears
    // nothing; html hears the four events, and there is no click.
    let html = format!(
        "<!doctype html><style>body{{margin:0}}::-webkit-scrollbar{{width:6px}}\
         ::-webkit-scrollbar-thumb{{background:#888}}#c{{height:2400px}}\
         #fx{{position:fixed;left:0;top:0;width:100vw;height:100vh}}</style>\
         <div id=c></div><div id=fx></div>{EVENT_LOG_AT}"
    );
    let (mut engine, view) = loaded(&html);
    take_log(&mut engine, view);
    engine.mouse_down_at_point(view, 797.0, 300.0);
    engine.click_at_point(view, 797.0, 300.0);
    let got = take_log(&mut engine, view);
    let without_offsets = got.split(',').map(|e| e.split('@').next().unwrap()).collect::<Vec<_>>().join(",");
    assert_eq!(without_offsets, "pointerdown:HTML,mousedown:HTML,pointerup:HTML,mouseup:HTML");
}
