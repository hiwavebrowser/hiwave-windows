//! HTML drag and drop (HTML §6.11.5, "drag-and-drop processing model")
//! through the engine's pointer entry points: a press on a draggable
//! element and a move past the 3px threshold start a drag; the moves fire
//! `drag` at the source and `dragenter`/`dragover`/`dragleave` where the
//! pointer is; the release drops on a target whose `dragover` was
//! cancelled and fires `dragend` at the source. The page hears no mouse
//! move, up or click while the drag runs, and `:hover` follows the pointer.

use super::*;

/// Rows 40px tall from the top: #src (draggable) 0-40, #plain 40-80, #zone
/// (accepts) 80-140, #deaf (refuses) 140-200, #link 200-240.
const PAGE: &str = concat!(
    r#"<html><head><style>body{margin:0} ul{margin:0;padding:0;list-style:none} "#,
    r#"li,div,a{display:block;height:40px} #zone,#deaf{height:60px} #zone:hover{background:red}</style></head><body>"#,
    r#"<ul><li id="src" draggable="true">drag me</li><li id="plain">not draggable</li></ul>"#,
    r#"<div id="zone">accepting zone</div><div id="deaf">refusing zone</div>"#,
    r#"<a id="link" href="https://example.test/l">a link</a>"#,
    r#"</body></html>"#,
);

const SRC: (f32, f32) = (10.0, 20.0);
const PLAIN: (f32, f32) = (10.0, 60.0);
const ZONE: (f32, f32) = (10.0, 100.0);
const DEAF: (f32, f32) = (10.0, 170.0);
const LINK: (f32, f32) = (10.0, 220.0);

/// Every drag event, and the mouse events a drag must hold back, as
/// `type:target[:dropEffect]` from a capturing listener on the document.
const LISTEN: &str = "window.log = []; \
    ['dragstart', 'drag', 'dragenter', 'dragover', 'dragleave', 'drop', 'dragend', \
     'mousemove', 'mouseup', 'click', 'pointercancel'].forEach(function (t) { \
      document.addEventListener(t, function (e) { \
        var s = t + ':' + (e.target.id || e.target.nodeName); \
        if (t === 'dragend' || t === 'drop') s += ':' + e.dataTransfer.dropEffect; \
        log.push(s); }, true); }); \
    var src = document.getElementById('src'), zone = document.getElementById('zone'); \
    src.addEventListener('dragstart', function (e) { \
      e.dataTransfer.setData('text/plain', 'payload'); e.dataTransfer.effectAllowed = 'move'; }); \
    zone.addEventListener('dragenter', function (e) { e.preventDefault(); }); \
    zone.addEventListener('dragover', function (e) { \
      window.overData = e.dataTransfer.getData('text/plain'); window.overTypes = e.dataTransfer.types.join(','); \
      e.dataTransfer.dropEffect = 'move'; e.preventDefault(); }); \
    zone.addEventListener('drop', function (e) { \
      window.dropped = e.dataTransfer.getData('text/plain'); window.isDrag = e instanceof DragEvent && e instanceof MouseEvent; \
      e.preventDefault(); }); ''";

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new(EngineConfig::default()).expect("engine");
    let id = engine
        .create_headless_view(Bounds::new(0, 0, 400, 400))
        .expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    js(&mut engine, id, LISTEN);
    (engine, id)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, script).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map_or(out.clone(), str::to_string)
}

fn taken(engine: &mut Engine, id: EngineViewId) -> String {
    js(engine, id, "var out = log.join(' '); log.length = 0; out")
}

fn down(engine: &mut Engine, id: EngineViewId, at: (f32, f32)) {
    engine.mouse_down_at_point(id, at.0, at.1);
}

fn move_to(engine: &mut Engine, id: EngineViewId, at: (f32, f32)) {
    engine.mouse_move_at_point(id, at.0, at.1);
}

fn up(engine: &mut Engine, id: EngineViewId, at: (f32, f32)) -> ClickOutcome {
    engine.click_at_point(id, at.0, at.1)
}

/// The innermost hovered element (raw NodeId), or "none".
fn hovered(engine: &mut Engine, id: EngineViewId) -> String {
    let first = engine
        .views
        .get(&id)
        .and_then(|v| v.lay.hover_chain.borrow().first().copied());
    first.map_or("none".into(), |raw| raw.to_string())
}

fn raw_of(engine: &Engine, id: EngineViewId, element: &str) -> String {
    let doc = engine
        .views
        .get(&id)
        .and_then(|v| v.document.clone())
        .expect("document");
    doc.get_element_by_id(element)
        .expect("element")
        .id
        .raw()
        .to_string()
}

#[test]
fn a_draggable_dropped_on_an_accepting_zone_fires_the_whole_sequence() {
    let (mut engine, id) = page();
    down(&mut engine, id, SRC);
    // 2px: under the threshold, still a press.
    move_to(&mut engine, id, (10.0, 22.0));
    assert_eq!(taken(&mut engine, id), "mousemove:src");
    // Past it: the drag starts, and its first iteration runs at once.
    move_to(&mut engine, id, (10.0, 30.0));
    assert_eq!(
        taken(&mut engine, id),
        "dragstart:src pointercancel:src drag:src dragenter:src dragover:src"
    );
    move_to(&mut engine, id, ZONE);
    assert_eq!(
        taken(&mut engine, id),
        "drag:src dragenter:zone dragleave:src dragover:zone"
    );
    assert_eq!(
        hovered(&mut engine, id),
        raw_of(&engine, id, "zone"),
        ":hover follows the drag"
    );
    move_to(&mut engine, id, (12.0, 110.0));
    assert_eq!(taken(&mut engine, id), "drag:src dragover:zone");
    // The data is protected outside dragstart and drop: types, no data.
    assert_eq!(
        js(&mut engine, id, "overData + '|' + overTypes"),
        "|text/plain"
    );
    let outcome = up(&mut engine, id, (12.0, 110.0));
    assert_eq!(taken(&mut engine, id), "drop:zone:move dragend:src:move");
    assert_eq!(
        js(&mut engine, id, "dropped + '|' + isDrag"),
        "payload|true"
    );
    assert_eq!(outcome.navigate, None);
}

#[test]
fn a_zone_that_does_not_cancel_dragover_gets_no_drop() {
    let (mut engine, id) = page();
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (10.0, 30.0));
    move_to(&mut engine, id, DEAF);
    taken(&mut engine, id);
    up(&mut engine, id, DEAF);
    assert_eq!(taken(&mut engine, id), "dragleave:deaf dragend:src:none");
    assert_eq!(js(&mut engine, id, "String(window.dropped)"), "undefined");
}

#[test]
fn a_drag_back_onto_its_source_ends_without_a_drop() {
    let (mut engine, id) = page();
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (10.0, 30.0));
    move_to(&mut engine, id, ZONE);
    move_to(&mut engine, id, SRC);
    assert!(taken(&mut engine, id).ends_with("drag:src dragenter:src dragleave:zone dragover:src"));
    up(&mut engine, id, SRC);
    assert_eq!(taken(&mut engine, id), "dragleave:src dragend:src:none");
}

#[test]
fn a_press_on_a_plain_div_never_starts_a_drag() {
    let (mut engine, id) = page();
    down(&mut engine, id, PLAIN);
    move_to(&mut engine, id, (10.0, 70.0));
    move_to(&mut engine, id, ZONE);
    up(&mut engine, id, ZONE);
    assert_eq!(
        taken(&mut engine, id),
        "mousemove:plain mousemove:zone mouseup:zone click:BODY"
    );
}

#[test]
fn a_cancelled_mousedown_or_dragstart_starts_no_drag() {
    let (mut engine, id) = page();
    js(&mut engine, id, "window.noDown = function (e) { e.preventDefault(); }; src.addEventListener('mousedown', noDown); ''");
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (10.0, 30.0));
    up(&mut engine, id, (10.0, 30.0));
    assert_eq!(
        taken(&mut engine, id),
        "mousemove:src mouseup:src click:src"
    );

    js(
        &mut engine,
        id,
        "src.removeEventListener('mousedown', noDown); \
         src.addEventListener('dragstart', function (e) { e.preventDefault(); }); ''",
    );
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (10.0, 30.0));
    move_to(&mut engine, id, (10.0, 35.0));
    up(&mut engine, id, (10.0, 35.0));
    assert_eq!(
        taken(&mut engine, id),
        "dragstart:src mousemove:src mousemove:src mouseup:src click:src"
    );
}

#[test]
fn the_release_ends_the_drag_and_clicks_resume() {
    let (mut engine, id) = page();
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (10.0, 30.0));
    move_to(&mut engine, id, ZONE);
    up(&mut engine, id, ZONE);
    taken(&mut engine, id);
    // The pointer moves on with no button: plain moves again.
    move_to(&mut engine, id, (10.0, 105.0));
    assert_eq!(taken(&mut engine, id), "mousemove:zone");
    down(&mut engine, id, ZONE);
    up(&mut engine, id, ZONE);
    assert_eq!(taken(&mut engine, id), "mouseup:zone click:zone");
    // A short press on the draggable is a click, not a drag.
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (11.0, 21.0));
    up(&mut engine, id, (11.0, 21.0));
    assert_eq!(
        taken(&mut engine, id),
        "mousemove:src mouseup:src click:src"
    );
}

#[test]
fn a_link_is_draggable_by_default_and_its_drag_follows_nothing() {
    let (mut engine, id) = page();
    js(
        &mut engine,
        id,
        "document.getElementById('link').addEventListener('dragstart', function (e) { \
           window.linkData = e.dataTransfer.getData('text/uri-list'); }); ''",
    );
    down(&mut engine, id, LINK);
    move_to(&mut engine, id, (20.0, 225.0));
    let outcome = up(&mut engine, id, (20.0, 225.0));
    assert_eq!(
        taken(&mut engine, id),
        "dragstart:link pointercancel:link drag:link dragenter:link dragover:link dragleave:link dragend:link:none"
    );
    assert_eq!(js(&mut engine, id, "linkData"), "https://example.test/l");
    assert_eq!(outcome.navigate, None, "a dragged link is not followed");
}

/// HiWave's chrome makes its tabs draggable from script
/// (`tabEl.draggable = true`, crates/hiwave-app/src/ui/chrome.html).
#[test]
fn an_element_made_draggable_by_script_drags() {
    let (mut engine, id) = page();
    js(
        &mut engine,
        id,
        "document.getElementById('plain').draggable = true; ''",
    );
    down(&mut engine, id, PLAIN);
    move_to(&mut engine, id, (10.0, 70.0));
    move_to(&mut engine, id, ZONE);
    up(&mut engine, id, ZONE);
    assert_eq!(
        taken(&mut engine, id),
        "dragstart:plain pointercancel:plain drag:plain dragenter:plain dragover:plain \
         drag:plain dragenter:zone dragleave:plain dragover:zone drop:zone:move dragend:plain:move"
    );
    // #plain put no data in: the drop reads an empty string.
    assert_eq!(js(&mut engine, id, "dropped"), "");
}

/// A drag whose release the engine never sees (the view lost focus) is
/// ended without a drop, and the page hears plain mouse events again.
#[test]
fn a_cancelled_drag_ends_without_a_drop_and_moves_resume() {
    let (mut engine, id) = page();
    down(&mut engine, id, SRC);
    move_to(&mut engine, id, (10.0, 30.0));
    move_to(&mut engine, id, ZONE);
    taken(&mut engine, id);
    engine.cancel_drag(id);
    assert_eq!(taken(&mut engine, id), "dragleave:zone dragend:src:none");
    assert_eq!(js(&mut engine, id, "String(window.dropped)"), "undefined");
    move_to(&mut engine, id, (10.0, 105.0));
    assert_eq!(taken(&mut engine, id), "mousemove:zone");
    // Nothing to cancel: no event.
    engine.cancel_drag(id);
    assert_eq!(taken(&mut engine, id), "");
}

/// A press in a text field inside a draggable row selects text there; it
/// does not drag the row (Chrome).
#[test]
fn a_press_in_a_text_field_inside_a_draggable_row_does_not_drag_it() {
    let (mut engine, id) = page();
    js(
        &mut engine,
        id,
        "var f = document.createElement('input'); f.id = 'field'; \
         f.style.cssText = 'display:block;width:200px;height:30px;margin:0;padding:0;border:0'; \
         var e = document.createElement('div'); e.id = 'edit'; e.setAttribute('contenteditable', ''); \
         e.textContent = 'editable'; e.style.height = '30px'; \
         var row = document.getElementById('src'); row.textContent = ''; \
         row.style.height = '60px'; row.appendChild(f); row.appendChild(e); ''",
    );
    for (at, target) in [((10.0, 15.0), "field"), ((10.0, 45.0), "edit")] {
        down(&mut engine, id, at);
        move_to(&mut engine, id, (at.0 + 20.0, at.1));
        up(&mut engine, id, (at.0 + 20.0, at.1));
        assert_eq!(
            taken(&mut engine, id),
            format!("mousemove:{target} mouseup:{target} click:{target}")
        );
    }
}
