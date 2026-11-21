//! CH2.E12: the selection of a filled `<input>`, from the user (a click,
//! Cmd+A) and from script (`select()`, `setSelectionRange`,
//! `selectionStart`/`selectionEnd`), is one selection: what script sets is
//! what the next typed text replaces, and what the user does is what
//! script reads.
//!
//! Expected values are the oracle Chrome's (143.0.7499.4), from
//! `tools/parity_oracle/text_control_select_log.mjs` on the same page:
//! four inputs, `value="old-value"`, 20px Menlo, no border or padding,
//! content box at x=10.

use super::*;

const STYLE: &str = "position:absolute;left:10px;width:300px;height:30px;margin:0;padding:0;border:0;font-family:Menlo,Consolas,monospace;font-size:20px";

fn page() -> (Engine, EngineViewId) {
    let html = format!(
        "<html><body style=\"margin:0\">\
         <input id=\"a\" value=\"old-value\" style=\"{STYLE};top:10px\">\
         <input id=\"b\" value=\"old-value\" style=\"{STYLE};top:60px\">\
         <input id=\"c\" value=\"old-value\" style=\"{STYLE};top:110px\">\
         <input id=\"d\" value=\"old-value\" style=\"{STYLE};top:160px\">\
         <script>document.getElementById('b').addEventListener('focus', function () {{ this.select(); }});</script>\
         </body></html>"
    );
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0))).expect("engine");
    let id = engine.create_page_view(Bounds::new(0, 0, 800, 600)).expect("page view");
    engine.load_html(id, &html).expect("load_html");
    js(&mut engine, id, "window.$ = function (i) { return document.getElementById(i); }; 0");
    (engine, id)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine.execute_script(id, &format!("String(eval({script:?}))")).expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\\"", "\""))
        .unwrap_or(out)
}

/// Script's `selectionStart,selectionEnd`.
fn sel(engine: &mut Engine, id: EngineViewId, el: &str) -> String {
    js(engine, id, &format!("$('{el}').selectionStart + ',' + $('{el}').selectionEnd"))
}

/// The value as script reads it, checked against what the engine paints.
fn value(engine: &mut Engine, id: EngineViewId, el: &str) -> String {
    let script = js(engine, id, &format!("$('{el}').value"));
    let raw = engine.views[&id].document.as_ref().unwrap().get_element_by_id(el).unwrap().id.raw();
    if let Some((painted, _)) = engine.edit_value_in(id, raw) {
        assert_eq!(painted, script, "script and paint agree on {el}");
    }
    script
}

fn click(engine: &mut Engine, id: EngineViewId, x: f32, y: f32) {
    engine.mouse_move_at_point(id, x, y);
    engine.mouse_down_at_point(id, x, y);
    engine.click_at_point(id, x, y);
}

fn type_text(engine: &mut Engine, id: EngineViewId, text: &str) {
    for ch in text.chars() {
        assert!(engine.handle_text_key(id, 0, &ch.to_string(), false, false, false), "typed {ch:?}");
    }
}

/// Menlo's advance at 20px (the oracle's canvas measure: 12.041015625).
fn advance() -> f32 {
    rustkit_layout::measure_text_advanced("o", "Menlo,Consolas,monospace", 20.0, Default::default(), Default::default()).width
}

// (a) Click a filled input, Cmd+A, type: the typed text replaces the value.
// Chrome: click sel=9,9; Cmd+A sel=0,9; typed value="new".
#[test]
fn click_then_cmd_a_then_typing_replaces_the_value() {
    let (mut engine, id) = page();
    click(&mut engine, id, 250.0, 25.0);
    assert_eq!(sel(&mut engine, id, "a"), "9,9", "click past the text: caret at the end");
    assert!(engine.handle_text_key(id, 0, "a", true, false, false), "Cmd+A handled");
    assert_eq!(sel(&mut engine, id, "a"), "0,9", "Cmd+A selects the value");
    type_text(&mut engine, id, "new");
    assert_eq!(value(&mut engine, id, "a"), "new");
}

// (b) A focus handler calling select(), focus by script.
// Chrome: sel=0,9; typed value="new".
#[test]
fn focus_handler_select_by_script_focus_then_typing_replaces() {
    let (mut engine, id) = page();
    js(&mut engine, id, "$('b').focus()");
    assert_eq!(sel(&mut engine, id, "b"), "0,9");
    type_text(&mut engine, id, "new");
    assert_eq!(value(&mut engine, id, "b"), "new");
}

// (b) The same handler, focus by a click: Chrome keeps the handler's
// selection through the press and release. Chrome: sel=0,9; "new".
#[test]
fn focus_handler_select_by_click_then_typing_replaces() {
    let (mut engine, id) = page();
    click(&mut engine, id, 40.0, 75.0);
    assert_eq!(sel(&mut engine, id, "b"), "0,9");
    type_text(&mut engine, id, "new");
    assert_eq!(value(&mut engine, id, "b"), "new");
}

// (c) setSelectionRange(2, 5) from script, then type.
// Chrome: sel=2,5; typed "X" value="olXalue" sel=3,3.
#[test]
fn set_selection_range_then_typing_replaces_the_middle() {
    let (mut engine, id) = page();
    js(&mut engine, id, "$('c').focus(); $('c').setSelectionRange(2, 5)");
    assert_eq!(sel(&mut engine, id, "c"), "2,5");
    type_text(&mut engine, id, "X");
    assert_eq!(value(&mut engine, id, "c"), "olXalue");
    assert_eq!(sel(&mut engine, id, "c"), "3,3");
}

// select() on an unfocused input: script reads it. Chrome: sel=0,9.
#[test]
fn select_on_an_unfocused_input_reads_back() {
    let (mut engine, id) = page();
    js(&mut engine, id, "$('c').select()");
    assert_eq!(sel(&mut engine, id, "c"), "0,9");
}

// (c) selectionStart/End follow the user's caret.
// Chrome: End, Left, Left sel=7,7; Shift+Left sel=6,7; typed "ab"
// value="old-vaabue" sel=8,8.
#[test]
fn selection_start_and_end_follow_the_users_caret() {
    const LEFT: u32 = 0x25;
    const END: u32 = 0x23;
    let (mut engine, id) = page();
    js(&mut engine, id, "$('c').focus()");
    engine.handle_text_key(id, END, "", false, false, false);
    engine.handle_text_key(id, LEFT, "", false, false, false);
    engine.handle_text_key(id, LEFT, "", false, false, false);
    assert_eq!(sel(&mut engine, id, "c"), "7,7");
    engine.handle_text_key(id, LEFT, "", false, true, false);
    assert_eq!(sel(&mut engine, id, "c"), "6,7");
    type_text(&mut engine, id, "ab");
    assert_eq!(value(&mut engine, id, "c"), "old-vaabue");
    assert_eq!(sel(&mut engine, id, "c"), "8,8");
}

// (d) A click places the caret at the click point, not at the end.
// Chrome, x = 10 + f * advance: 0.2 -> 0, 1.4 -> 1, 3.3 -> 3, 3.7 -> 4,
// 8.6 -> 9, 12 -> 9; after the 3.3 click, "Z" gives "oldZ-value".
#[test]
fn a_click_places_the_caret_at_the_click_point() {
    let adv = advance();
    // Menlo is a macOS font: the click maths below scale with `adv`, only this pin does not.
    #[cfg(target_os = "macos")]
    assert!((adv - 12.041).abs() < 0.05, "Menlo 20px advance {adv}");
    for (f, want) in [(0.2, "0,0"), (1.4, "1,1"), (3.3, "3,3"), (3.7, "4,4"), (8.6, "9,9"), (12.0, "9,9")] {
        let (mut engine, id) = page();
        click(&mut engine, id, 10.0 + f * adv, 175.0);
        assert_eq!(sel(&mut engine, id, "d"), want, "click at 10 + {f} * advance");
    }
    let (mut engine, id) = page();
    click(&mut engine, id, 10.0 + 3.3 * adv, 175.0);
    type_text(&mut engine, id, "Z");
    assert_eq!(value(&mut engine, id, "d"), "oldZ-value");
}
