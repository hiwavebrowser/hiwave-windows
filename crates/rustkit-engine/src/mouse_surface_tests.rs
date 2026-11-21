//! CH2.E4: the mouse surface past the primary button's single click: a
//! right press (`contextmenu`), the click count (`detail`, `dblclick`, word
//! and paragraph selection), the `cursor` property and the `title` under
//! the pointer, and a move with the button held.
//!
//! The expected logs are Chromium 143.0.7499.4's, from
//! `tools/parity_oracle/mouse_surface_log.mjs` on
//! `tools/parity_oracle/mouse_surface_page.html` (the page below is that
//! page). Each line is `type:target:button:buttons:detail:isPointerEvent`.

use super::*;

const PAGE: &str = r##"<html><body style="margin:0;font:16px/20px sans-serif">
<p id="p" style="margin:0;height:40px">alpha bravo charlie</p>
<a id="link" href="#x" title="go somewhere" style="display:block;height:30px">link</a>
<div id="text" style="height:30px;cursor:text">t</div>
<div id="ew" style="height:30px;cursor:ew-resize" title="outer"><span id="inner" style="cursor:ns-resize">n</span></div>
<div id="na" style="height:30px;cursor:not-allowed">n</div>
<div id="grab" style="height:30px;cursor:grab">g</div>
<div id="mv" style="height:30px;cursor:move">m</div>
<div id="def" style="height:30px">d</div>
</body></html>"##;

/// Where "bravo" is (the second word of #p), and points in the other rows:
/// on their one-letter text, and to the right of it.
const BRAVO: (f32, f32) = (60.0, 10.0);
const LINK: (f32, f32) = (10.0, 55.0);
const TEXT_ROW_BLANK: (f32, f32) = (300.0, 85.0);
const INNER: (f32, f32) = (4.0, 115.0);
const EW_BLANK: (f32, f32) = (300.0, 115.0);
const NA_BLANK: (f32, f32) = (300.0, 145.0);
const GRAB_BLANK: (f32, f32) = (300.0, 175.0);
const MOVE_BLANK: (f32, f32) = (300.0, 205.0);
const DEF_BLANK: (f32, f32) = (300.0, 235.0);
const DEF_TEXT: (f32, f32) = (3.0, 235.0);

const LISTEN: &str = "window.log = []; window.prevent = false; \
    ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'auxclick', 'dblclick', \
     'contextmenu', 'pointermove', 'mousemove'].forEach(function (t) { \
      document.addEventListener(t, function (e) { \
        if (t === 'contextmenu' && window.prevent) e.preventDefault(); \
        log.push([e.type, e.target.id, e.button, e.buttons, e.detail, e instanceof PointerEvent].join(':')); \
      }); }); ''";

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 800, 600))
        .expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    js(&mut engine, id, LISTEN);
    (engine, id)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, script).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|v| v.strip_suffix("\")"))
        .unwrap_or(&out)
        .to_string()
}

/// The log since the last call, one event a line.
fn take_log(engine: &mut Engine, id: EngineViewId) -> Vec<String> {
    let joined = js(engine, id, "String(window.log.splice(0).join('|'))");
    joined.split('|').filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn selection(engine: &mut Engine, id: EngineViewId) -> String {
    js(engine, id, "String(getSelection())")
}

fn lines(expected: &[&str]) -> Vec<String> {
    expected.iter().map(|s| s.to_string()).collect()
}

#[test]
fn a_right_press_fires_contextmenu_after_mousedown_as_chromium_does() {
    let (mut engine, id) = page();
    let shown = engine.context_menu_at_point(id, BRAVO.0, BRAVO.1);
    assert!(shown, "nothing cancelled the contextmenu: the browser's menu shows");
    engine.context_menu_release(id, BRAVO.0, BRAVO.1);
    assert_eq!(
        take_log(&mut engine, id),
        lines(&[
            "pointerdown:p:2:2:0:true",
            "mousedown:p:2:2:1:false",
            "contextmenu:p:2:2:0:true",
            "pointerup:p:2:0:0:true",
            "mouseup:p:2:0:1:false",
            "auxclick:p:2:0:1:true",
        ])
    );
    // macOS Chrome: a right press the page let through selects the word.
    assert_eq!(selection(&mut engine, id), "bravo");
}

#[test]
fn a_prevented_contextmenu_shows_no_menu_and_selects_nothing() {
    let (mut engine, id) = page();
    js(&mut engine, id, "window.prevent = true; ''");
    let shown = engine.context_menu_at_point(id, BRAVO.0, BRAVO.1);
    assert!(!shown, "the page called preventDefault()");
    assert_eq!(
        js(&mut engine, id, "String(window.log.filter(function (l) { return l.indexOf('contextmenu') === 0; }).length)"),
        "1"
    );
    assert_eq!(selection(&mut engine, id), "");
}

#[test]
fn a_right_press_does_not_leave_the_primary_button_held() {
    let (mut engine, id) = page();
    engine.context_menu_at_point(id, BRAVO.0, BRAVO.1);
    // The menu took the release: the page never hears it.
    take_log(&mut engine, id);
    engine.mouse_move_at_point(id, LINK.0, LINK.1);
    let log = take_log(&mut engine, id);
    assert!(log.contains(&"pointermove:link:-1:0:0:true".to_string()), "{log:?}");
    assert!(log.contains(&"mousemove:link:0:0:0:false".to_string()), "{log:?}");
}

#[test]
fn a_double_click_counts_its_clicks_fires_dblclick_and_selects_the_word() {
    let (mut engine, id) = page();
    engine.mouse_down_with(id, BRAVO.0, BRAVO.1, 1, false);
    engine.click_at_point(id, BRAVO.0, BRAVO.1);
    engine.mouse_down_with(id, BRAVO.0, BRAVO.1, 2, false);
    engine.click_at_point(id, BRAVO.0, BRAVO.1);
    assert_eq!(
        take_log(&mut engine, id),
        lines(&[
            "pointerdown:p:0:1:0:true",
            "mousedown:p:0:1:1:false",
            "pointerup:p:0:0:0:true",
            "mouseup:p:0:0:1:false",
            "click:p:0:0:1:true",
            "pointerdown:p:0:1:0:true",
            "mousedown:p:0:1:2:false",
            "pointerup:p:0:0:0:true",
            "mouseup:p:0:0:2:false",
            "click:p:0:0:2:true",
            "dblclick:p:0:0:2:false",
        ])
    );
    assert_eq!(selection(&mut engine, id), "bravo");
}

#[test]
fn a_triple_click_counts_three_and_fires_one_dblclick() {
    let (mut engine, id) = page();
    for clicks in 1..=3 {
        engine.mouse_down_with(id, BRAVO.0, BRAVO.1, clicks, false);
        engine.click_at_point(id, BRAVO.0, BRAVO.1);
    }
    let log = take_log(&mut engine, id);
    assert_eq!(
        log[log.len() - 5..].to_vec(),
        lines(&[
            "pointerdown:p:0:1:0:true",
            "mousedown:p:0:1:3:false",
            "pointerup:p:0:0:0:true",
            "mouseup:p:0:0:3:false",
            "click:p:0:0:3:true",
        ])
    );
    assert_eq!(log.iter().filter(|l| l.starts_with("dblclick")).count(), 1, "{log:?}");
    // Chromium's string ends "\n\n" (the paragraph's trailing break): not
    // asserted here.
    assert!(selection(&mut engine, id).starts_with("alpha bravo charlie"));
}

#[test]
fn a_move_with_the_button_held_is_a_drag_as_chromium_reports_it() {
    let (mut engine, id) = page();
    engine.mouse_down_at_point(id, BRAVO.0, BRAVO.1);
    engine.mouse_move_at_point(id, BRAVO.0 + 60.0, BRAVO.1);
    engine.click_at_point(id, BRAVO.0 + 60.0, BRAVO.1);
    assert_eq!(
        take_log(&mut engine, id),
        lines(&[
            "pointerdown:p:0:1:0:true",
            "mousedown:p:0:1:1:false",
            "pointermove:p:-1:1:0:true",
            "mousemove:p:0:1:0:false",
            "pointerup:p:0:0:0:true",
            "mouseup:p:0:0:1:false",
            "click:p:0:0:1:true",
        ])
    );
    assert!(!selection(&mut engine, id).is_empty(), "the drag selected text");
}

#[test]
fn computed_cursor_matches_chromium() {
    let (mut engine, id) = page();
    let got = js(
        &mut engine,
        id,
        "String(['p', 'link', 'text', 'ew', 'inner', 'na', 'grab', 'mv', 'def'].map(function (id) { \
           return id + '=' + getComputedStyle(document.getElementById(id)).cursor; }).join(' '))",
    );
    assert_eq!(
        got,
        "p=auto link=pointer text=text ew=ew-resize inner=ns-resize na=not-allowed grab=grab mv=move def=auto"
    );
}

/// The cursor the window shows at a point: `auto` resolved (text over
/// text, the arrow elsewhere).
fn cursor_at(engine: &mut Engine, id: EngineViewId, at: (f32, f32)) -> String {
    engine.mouse_move_at_point(id, at.0, at.1);
    engine.hover_feedback(id).map(|f| f.cursor.to_string()).unwrap_or_default()
}

#[test]
fn the_cursor_under_the_pointer_is_the_hit_boxs() {
    let (mut engine, id) = page();
    assert_eq!(cursor_at(&mut engine, id, BRAVO), "text", "auto over text");
    assert_eq!(cursor_at(&mut engine, id, LINK), "pointer", "a link");
    assert_eq!(cursor_at(&mut engine, id, TEXT_ROW_BLANK), "text");
    assert_eq!(cursor_at(&mut engine, id, INNER), "ns-resize", "the inner span's own");
    assert_eq!(cursor_at(&mut engine, id, EW_BLANK), "ew-resize");
    assert_eq!(cursor_at(&mut engine, id, NA_BLANK), "not-allowed");
    assert_eq!(cursor_at(&mut engine, id, GRAB_BLANK), "grab");
    assert_eq!(cursor_at(&mut engine, id, MOVE_BLANK), "move");
    assert_eq!(cursor_at(&mut engine, id, DEF_BLANK), "default", "auto off text");
    assert_eq!(cursor_at(&mut engine, id, DEF_TEXT), "text", "auto over text");
    engine.mouse_leave(id);
    assert!(engine.hover_feedback(id).is_none(), "nothing under a pointer that left");
}

#[test]
fn the_tooltip_is_the_nearest_title() {
    let (mut engine, id) = page();
    let title_at = |engine: &mut Engine, at: (f32, f32)| {
        engine.mouse_move_at_point(id, at.0, at.1);
        engine.hover_feedback(id).and_then(|f| f.title)
    };
    assert_eq!(title_at(&mut engine, LINK).as_deref(), Some("go somewhere"));
    assert_eq!(title_at(&mut engine, INNER).as_deref(), Some("outer"), "an ancestor's title");
    assert_eq!(title_at(&mut engine, EW_BLANK).as_deref(), Some("outer"));
    assert_eq!(title_at(&mut engine, BRAVO), None);
    // An empty title stops the search (HTML §3.2.6.1).
    js(&mut engine, id, "document.getElementById('inner').title = ''; ''");
    assert_eq!(title_at(&mut engine, INNER), None);
}
