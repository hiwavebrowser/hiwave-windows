//! CH2.E6: a RustKit-drawn `<select>` drop-down, through the engine's
//! user-input entry points (press, release, move, keys) on a headless view.
//!
//! Each test needs a GPU device to make an engine; where there is none
//! (a Linux container) it prints why and passes, as `consent_click_tests`
//! does. The popup's model is covered everywhere by `select_popup_tests`,
//! and the pick through script by rustkit-bindings' `form_controls_tests`.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><body style="margin:0;height:600px">
<select id="s" style="position:absolute;left:10px;top:10px;width:150px;height:24px;font-size:13px">
  <option>Zero</option>
  <option disabled>One (off)</option>
  <optgroup label="Group"><option>Two</option><option>Three</option></optgroup>
  <option>Four</option>
</select>
<select id="low" style="position:absolute;left:10px;top:570px;width:150px;height:24px">
  <option>a</option><option>b</option><option>c</option>
</select>
<select id="box" size="3" style="position:absolute;left:300px;top:10px;width:100px">
  <option>x</option><option>y</option><option>z</option>
</select>
<div id="other" style="position:absolute;left:400px;top:300px;width:100px;height:100px"></div>
<script>
  window.log = [];
  var s = document.getElementById('s');
  ['input', 'change'].forEach(function (t) {
    document.addEventListener(t, function (e) { log.push(t + ':' + e.target.id + ':' + e.target.selectedIndex); });
  });
  document.addEventListener('keydown', function (e) { log.push('keydown:' + e.key); });
</script>
</body></html>"#;

fn page() -> Option<(Engine, EngineViewId)> {
    let mut engine = match Engine::new(EngineConfig::default()) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("Skipping test: no engine ({e:?})");
            return None;
        }
    };
    let id = engine.create_headless_view(Bounds::new(0, 0, 800, 600)).expect("headless view");
    engine.load_html(id, PAGE).expect("load_html");
    Some((engine, id))
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, script).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map_or(out.clone(), str::to_string)
}

fn click(engine: &mut Engine, id: EngineViewId, x: f32, y: f32) {
    engine.mouse_down_at_point(id, x, y);
    engine.click_at_point(id, x, y);
}

/// Click the middle of the select `s`.
fn open(engine: &mut Engine, id: EngineViewId) {
    click(engine, id, 85.0, 22.0);
    assert!(engine.select_popup_open(id), "a click on the select opens its popup");
}

fn texts(engine: &Engine, id: EngineViewId) -> Vec<String> {
    let list = engine.views[&id].display_list.clone().expect("display list");
    list.commands
        .iter()
        .filter_map(|c| match c {
            rustkit_layout::DisplayCommand::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// The middle of option `index`'s row in the open popup, in viewport
/// coordinates (the page is not scrolled).
fn row_point(engine: &Engine, id: EngineViewId, index: usize) -> (f32, f32) {
    let popup = engine.views[&id].lay.select_popup.borrow().clone().expect("open popup");
    let row = popup
        .rows
        .iter()
        .position(|(r, _)| *r == select_popup::Row::Option(index))
        .expect("a row");
    let y = popup.rect.y + 1.0 + popup.row_height * ((row - popup.first_row) as f32 + 0.5);
    (popup.rect.x + 20.0, y)
}

fn log(engine: &mut Engine, id: EngineViewId) -> String {
    js(engine, id, "log.join(' ')")
}

#[test]
fn a_click_opens_the_popup_and_the_display_list_holds_its_rows() {
    let Some((mut engine, id)) = page() else { return };
    let before = texts(&engine, id);
    open(&mut engine, id);
    let after = texts(&engine, id);
    let added: Vec<&String> = after.iter().skip(before.len()).collect();
    assert_eq!(added, ["Zero", "One (off)", "Group", "Two", "Three", "Four"], "the rows follow the page");
    let popup = engine.views[&id].lay.select_popup.borrow().clone().unwrap();
    assert_eq!(popup.rect.y, 34.0, "the popup hangs under the control");
    assert_eq!(popup.highlighted, Some(0), "the selected option is highlighted");
}

#[test]
fn a_click_on_the_third_option_selects_it_and_fires_change_once() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    let (x, y) = row_point(&engine, id, 2);
    click(&mut engine, id, x, y);
    assert!(!engine.select_popup_open(id), "a pick closes the popup");
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex) + s.value"), "2Two");
    assert_eq!(log(&mut engine, id), "input:s:2 change:s:2");
    assert_eq!(js(&mut engine, id, "String(s.options[2].matches(':checked'))"), "true");
    // The closed control shows it.
    assert!(texts(&engine, id).iter().all(|t| t != "Zero"), "the label moved off Zero");
}

#[test]
fn escape_closes_without_a_change() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    assert!(engine.handle_text_key(id, 0x28, "", false, false, false), "ArrowDown moves the highlight");
    assert!(engine.handle_text_key(id, 0x1B, "", false, false, false));
    assert!(!engine.select_popup_open(id));
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "0");
    assert_eq!(log(&mut engine, id), "", "no change, and the page heard no key while the popup was open");
    let before = texts(&engine, id);
    assert!(!before.contains(&"Group".to_string()), "the rows are gone from the display list");
}

#[test]
fn arrow_down_and_enter_select_the_next_enabled_option() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    engine.handle_text_key(id, 0x28, "", false, false, false);
    assert_eq!(engine.submit_focused_form(id), None, "Enter in the popup submits nothing");
    engine.handle_text_key(id, 0x0D, "", false, false, false);
    assert!(!engine.select_popup_open(id));
    // Option 1 is disabled: the next is 2.
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "2");
    assert_eq!(log(&mut engine, id), "input:s:2 change:s:2");
}

#[test]
fn a_disabled_option_cannot_be_selected() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    let (x, y) = row_point(&engine, id, 1);
    engine.mouse_move_at_point(id, x, y);
    click(&mut engine, id, x, y);
    assert!(engine.select_popup_open(id), "a click on a disabled option leaves the popup open");
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "0");
    assert_eq!(log(&mut engine, id), "");
}

#[test]
fn a_click_elsewhere_closes_and_reaches_nothing() {
    let Some((mut engine, id)) = page() else { return };
    js(&mut engine, id, "document.getElementById('other').onclick = function () { log.push('other'); }; ''");
    open(&mut engine, id);
    click(&mut engine, id, 450.0, 350.0);
    assert!(!engine.select_popup_open(id));
    assert_eq!(log(&mut engine, id), "", "no change, and the click that closed the popup went nowhere");
    // A click on the select while open closes it without reopening.
    open(&mut engine, id);
    click(&mut engine, id, 85.0, 22.0);
    assert!(!engine.select_popup_open(id));
}

#[test]
fn hover_moves_the_highlight() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    let (x, y) = row_point(&engine, id, 3);
    engine.mouse_move_at_point(id, x, y);
    let popup = engine.views[&id].lay.select_popup.borrow().clone().unwrap();
    assert_eq!(popup.highlighted, Some(3));
}

#[test]
fn closed_arrow_keys_and_letters_change_the_value_directly() {
    let Some((mut engine, id)) = page() else { return };
    js(&mut engine, id, "s.focus(); ''");
    assert!(engine.handle_text_key(id, 0x28, "", false, false, false));
    assert!(!engine.select_popup_open(id), "an arrow key does not open a closed select");
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "2");
    assert!(engine.handle_text_key(id, 0, "f", false, false, false));
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "4");
    assert_eq!(log(&mut engine, id), "keydown:ArrowDown input:s:2 change:s:2 keydown:f input:s:4 change:s:4");
    // Space opens it.
    assert!(engine.handle_text_key(id, 0, " ", false, false, false));
    assert!(engine.select_popup_open(id));
}

#[test]
fn the_popup_opens_above_a_control_near_the_bottom() {
    let Some((mut engine, id)) = page() else { return };
    click(&mut engine, id, 85.0, 582.0);
    let popup = engine.views[&id].lay.select_popup.borrow().clone().expect("open");
    assert_eq!(popup.rect.y + popup.rect.height, 570.0);
}

#[test]
fn a_list_box_does_not_pop_up() {
    let Some((mut engine, id)) = page() else { return };
    click(&mut engine, id, 350.0, 20.0);
    assert!(!engine.select_popup_open(id));
}


// R1 on #899, nit 1: the popup opens where the window shows the control,
// inside a scrolled element and under a fixed toolbar on a scrolled page,
// and its rows take the pointer there.

const SCROLLED_PAGE: &str = r#"<!DOCTYPE html><html><body style="margin:0;height:2000px">
<div id="bar" style="position:fixed;left:0;top:0;width:800px;height:40px;background:#eee">
  <select id="fx" style="margin:8px 0 0 10px;width:150px;height:24px">
    <option>f0</option><option>f1</option><option>f2</option>
  </select>
</div>
<div id="panel" style="position:absolute;left:300px;top:100px;width:200px;height:100px;overflow:auto">
  <div style="height:200px"></div>
  <select id="inner" style="display:block;width:150px;height:24px">
    <option>i0</option><option>i1</option><option>i2</option>
  </select>
  <div style="height:200px"></div>
</div>
<script>
  window.log = [];
  document.addEventListener('change', function (e) { log.push(e.target.id + ':' + e.target.selectedIndex); });
</script>
</body></html>"#;

fn scrolled_page() -> Option<(Engine, EngineViewId)> {
    let (mut engine, id) = page()?;
    engine.load_html(id, SCROLLED_PAGE).expect("load_html");
    Some((engine, id))
}

#[test]
fn a_select_in_a_scrolled_element_opens_its_popup_where_it_is_shown() {
    let Some((mut engine, id)) = scrolled_page() else { return };
    js(&mut engine, id, "document.getElementById('panel').scrollTop = 150; ''");
    // In the document the select is at 100 + 200 = 300; the panel's
    // scroll shows it at 150.
    click(&mut engine, id, 375.0, 162.0);
    assert!(engine.select_popup_open(id), "a click where the select is shown opens it");
    let popup = engine.views[&id].lay.select_popup.borrow().clone().unwrap();
    assert_eq!(popup.rect.y, 174.0, "the popup hangs under the select as it is shown");
    assert_eq!(popup.rect.x, 300.0);
    let (x, y) = row_point(&engine, id, 2);
    click(&mut engine, id, x, y);
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "inner:2");
}

#[test]
fn a_select_in_a_fixed_bar_on_a_scrolled_page_opens_its_popup_under_it() {
    let Some((mut engine, id)) = scrolled_page() else { return };
    engine.set_scroll_offset(id, 0.0, 500.0).expect("scroll");
    click(&mut engine, id, 85.0, 20.0);
    assert!(engine.select_popup_open(id), "a click on the fixed select opens it");
    let popup = engine.views[&id].lay.select_popup.borrow().clone().unwrap();
    assert_eq!(popup.rect.y, 500.0 + 32.0, "under the control where the window shows it, in document coordinates");
    // The rows take the pointer at viewport coordinates.
    let (x, y) = row_point(&engine, id, 1);
    click(&mut engine, id, x, y - 500.0);
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "fx:1");
}

// Nit 2: a Ctrl (or Cmd) chord is no type-to-select.
#[test]
fn a_ctrl_chord_does_not_type_to_select() {
    let Some((mut engine, id)) = page() else { return };
    js(&mut engine, id, "s.focus(); ''");
    assert!(!engine.handle_text_key(id, 0, "f", true, false, false));
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "0");
    open(&mut engine, id);
    engine.handle_text_key(id, 0, "f", true, false, false);
    let popup = engine.views[&id].lay.select_popup.borrow().clone().unwrap();
    assert_eq!(popup.highlighted, Some(0), "the highlight stays");
}

// Nit 5: a closed select builds a prefix as the popup does.
#[test]
fn a_closed_select_types_a_prefix() {
    let Some((mut engine, id)) = page() else { return };
    js(
        &mut engine,
        id,
        "s.innerHTML = '<option>Apple</option><option>Banana</option><option>Blueberry</option><option>Bean</option>'; \
         s.focus(); ''",
    );
    engine.handle_text_key(id, 0, "b", false, false, false);
    engine.handle_text_key(id, 0, "l", false, false, false);
    assert_eq!(js(&mut engine, id, "s.value"), "Blueberry");
}

// CH2.E6 fix round (R1 nits 1, 3 and 4).

// Nit 1, the brief's shape: a select in an `overflow: auto` panel
// scrolled 100px, the panel itself in a scrolled panel, on a scrolled
// page. Every scroller above the select moves the popup with it.
const NESTED_SCROLL_PAGE: &str = r#"<!DOCTYPE html><html><body style="margin:0;height:2000px">
<div style="height:300px"></div>
<div id="outer" style="margin-left:100px;width:400px;height:200px;overflow:auto">
  <div style="height:50px"></div>
  <div id="panel" style="width:300px;height:150px;overflow:auto">
    <div style="height:200px"></div>
    <select id="inner" style="display:block;width:150px;height:24px">
      <option>i0</option><option>i1</option><option>i2</option>
    </select>
    <div style="height:300px"></div>
  </div>
  <div style="height:400px"></div>
</div>
<script>
  window.log = [];
  document.addEventListener('change', function (e) { log.push(e.target.id + ':' + e.target.selectedIndex); });
</script>
</body></html>"#;

#[test]
fn a_select_in_a_panel_scrolled_100px_opens_its_popup_where_it_is_shown() {
    let Some((mut engine, id)) = page() else { return };
    engine.load_html(id, NESTED_SCROLL_PAGE).expect("load_html");
    js(&mut engine, id, "document.getElementById('panel').scrollTop = 100; document.getElementById('outer').scrollTop = 30; ''");
    engine.set_scroll_offset(id, 0.0, 200.0).expect("scroll");
    // Document y: 300 (spacer) + 50 + 200 = 550; shown at
    // 550 - 100 (panel) - 30 (outer) - 200 (page) = 220 in the viewport.
    click(&mut engine, id, 175.0, 232.0);
    assert!(engine.select_popup_open(id), "a press where the select is shown opens it");
    let popup = engine.views[&id].lay.select_popup.borrow().clone().unwrap();
    assert_eq!(popup.rect.x, 100.0);
    assert_eq!(popup.rect.y, 200.0 + 244.0, "under the select as shown, in document coordinates");
    let (x, y) = row_point(&engine, id, 2);
    click(&mut engine, id, x, y - 200.0);
    assert_eq!(js(&mut engine, id, "log.join(' ')"), "inner:2");
}

// Nit 3: the popup opens at the press, as Chrome's does.
#[test]
fn a_press_on_the_select_opens_the_popup_before_the_release() {
    let Some((mut engine, id)) = page() else { return };
    engine.mouse_down_at_point(id, 85.0, 22.0);
    assert!(engine.select_popup_open(id), "the press opens it");
    engine.click_at_point(id, 85.0, 22.0);
    assert!(engine.select_popup_open(id), "the release on the select leaves it open");
    assert_eq!(log(&mut engine, id), "", "nothing picked");
}

#[test]
fn press_drag_release_onto_an_option_picks_it() {
    let Some((mut engine, id)) = page() else { return };
    engine.mouse_down_at_point(id, 85.0, 22.0);
    assert!(engine.select_popup_open(id), "the press opens it");
    let (x, y) = row_point(&engine, id, 3);
    engine.mouse_move_at_point(id, x, y);
    engine.click_at_point(id, x, y);
    assert!(!engine.select_popup_open(id), "the release on an option picks it and closes");
    assert_eq!(js(&mut engine, id, "String(s.selectedIndex)"), "3");
    assert_eq!(log(&mut engine, id), "input:s:3 change:s:3");
}

#[test]
fn a_cancelled_mousedown_does_not_open_the_popup() {
    let Some((mut engine, id)) = page() else { return };
    js(&mut engine, id, "s.addEventListener('mousedown', function (e) { e.preventDefault(); }); ''");
    click(&mut engine, id, 85.0, 22.0);
    assert!(!engine.select_popup_open(id), "preventDefault on mousedown keeps it shut");
}

// Nit 4: changes from outside close the popup.
#[test]
fn a_blur_closes_the_popup() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    js(&mut engine, id, "s.blur(); ''");
    assert!(!engine.select_popup_open(id), "losing the focus closes it");
    assert_eq!(texts(&engine, id).iter().filter(|t| *t == "Four").count(), 0, "its rows are gone");
}

#[test]
fn removing_the_select_closes_the_popup() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    js(&mut engine, id, "s.remove(); ''");
    assert!(!engine.select_popup_open(id), "a select taken out of the document closes it");
}

#[test]
fn disabling_the_select_closes_the_popup() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    js(&mut engine, id, "s.disabled = true; ''");
    assert!(!engine.select_popup_open(id), "a disabled select closes it");
}

#[test]
fn a_viewport_resize_closes_the_popup() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    engine.resize_view(id, Bounds::new(0, 0, 700, 500)).expect("resize");
    assert!(!engine.select_popup_open(id), "a resize closes it");
}

#[test]
fn an_unrelated_relayout_keeps_the_popup_open() {
    let Some((mut engine, id)) = page() else { return };
    open(&mut engine, id);
    js(&mut engine, id, "document.getElementById('other').style.background = 'red'; ''");
    assert!(engine.select_popup_open(id), "a style change elsewhere leaves it open");
    assert!(texts(&engine, id).iter().any(|t| t == "Four"), "and painted");
}
