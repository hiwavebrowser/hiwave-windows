//! CH2.E3: macOS text input through the engine. The content view's
//! `NSTextInputClient` hands the page marked text (`setMarkedText:`, a dead
//! key or an IME composing), commits (`insertText:`) and keeps what is
//! marked (`unmarkText`); the Edit menu's responders run edit commands; a
//! Cmd chord goes to the page's keydown before the menu.
//!
//! Every event sequence here is Chrome 143.0.7499.4's, logged with
//! `tools/parity_oracle/composition_log.mjs` (CDP `Input.imeSetComposition`
//! and `Input.insertText` drive the renderer's ImeSetComposition and
//! ImeCommitText, the calls Chromium's NSTextInputClient makes).
//! A line is `type data inputType isComposing value Constructor cancelable`.

use super::*;

const PAGE: &str = concat!(
    r#"<html><body style="margin:0">"#,
    r#"<input id="i" value="ab" style="font-size:20px"><textarea id="t">ab</textarea>"#,
    r#"<input id="q" value="hello">"#,
    r#"</body></html>"#,
);

fn page() -> (Engine, EngineViewId) {
    let mut engine = Engine::new_page(EngineConfig::default(), None, Arc::new(AtomicU64::new(0)))
        .expect("engine");
    let id = engine
        .create_page_view(Bounds::new(0, 0, 800, 600))
        .expect("page view");
    engine.load_html(id, PAGE).expect("load_html");
    js(
        &mut engine,
        id,
        "window.log = []; var $ = function (i) { return document.getElementById(i); }; \
         ['i', 't', 'q'].forEach(function (id) { var el = $(id); \
         ['compositionstart', 'compositionupdate', 'compositionend', 'beforeinput', 'input'].forEach(function (t) { \
         el.addEventListener(t, function (e) { log.push([t, e.data === undefined ? '-' : JSON.stringify(e.data), \
         e.inputType === undefined ? '-' : e.inputType, e.isComposing === undefined ? '-' : e.isComposing, \
         JSON.stringify(el.value), e.constructor.name, e.cancelable].join(' ')); }); }); }); 0",
    );
    (engine, id)
}

fn js(engine: &mut Engine, id: EngineViewId, script: &str) -> String {
    let out = engine
        .execute_script(id, &format!("String(eval({script:?}))"))
        .expect("script");
    out.strip_prefix("String(\"")
        .and_then(|s| s.strip_suffix("\")"))
        .map(|s| s.replace("\\\"", "\"").replace("\\\\", "\\"))
        .unwrap_or(out)
}

/// Focus `el` with the caret at the end of its value.
fn focus_end(engine: &mut Engine, id: EngineViewId, el: &str) {
    js(engine, id, &format!("$('{el}').focus(); $('{el}').setSelectionRange(99, 99)"));
    assert!(engine.focused_node(id).is_some(), "{el} focused");
}

/// The log since the last call, one event a line.
fn take_log(engine: &mut Engine, id: EngineViewId) -> Vec<String> {
    let joined = js(engine, id, "var l = log.join(' | '); log = []; l");
    joined.split(" | ").filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn value(engine: &mut Engine, id: EngineViewId, el: &str) -> String {
    js(engine, id, &format!("$('{el}').value"))
}

#[test]
fn a_dead_key_composes_then_commits_as_chrome_does() {
    for el in ["i", "t"] {
        let (mut engine, id) = page();
        focus_end(&mut engine, id, el);
        // Option+e: the input source marks "´".
        assert!(engine.set_marked_text(id, "\u{b4}"));
        assert_eq!(
            take_log(&mut engine, id),
            [
                "compositionstart \"\" - - \"ab\" CompositionEvent true",
                "compositionupdate \"´\" - - \"ab\" CompositionEvent true",
                "beforeinput \"´\" insertCompositionText true \"ab\" InputEvent false",
                "input \"´\" insertCompositionText true \"ab´\" InputEvent false",
            ],
            "{el}: setMarkedText"
        );
        // e: the input source commits "é" in its place.
        assert!(engine.insert_text(id, "\u{e9}"));
        assert_eq!(
            take_log(&mut engine, id),
            [
                "compositionupdate \"é\" - - \"ab´\" CompositionEvent true",
                "beforeinput \"é\" insertCompositionText true \"ab´\" InputEvent false",
                "input \"é\" insertCompositionText true \"abé\" InputEvent false",
                "compositionend \"é\" - - \"abé\" CompositionEvent true",
            ],
            "{el}: insertText"
        );
        assert_eq!(value(&mut engine, id, el), "abé");
        assert_eq!(js(&mut engine, id, &format!("$('{el}').selectionStart + ',' + $('{el}').selectionEnd")), "3,3");
    }
}

#[test]
fn an_ime_updates_twice_then_commits_other_text() {
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "i");
    engine.set_marked_text(id, "n");
    take_log(&mut engine, id);
    engine.set_marked_text(id, "ni");
    assert_eq!(
        take_log(&mut engine, id),
        [
            "compositionupdate \"ni\" - - \"abn\" CompositionEvent true",
            "beforeinput \"ni\" insertCompositionText true \"abn\" InputEvent false",
            "input \"ni\" insertCompositionText true \"abni\" InputEvent false",
        ]
    );
    engine.insert_text(id, "\u{4f60}");
    assert_eq!(
        take_log(&mut engine, id),
        [
            "compositionupdate \"你\" - - \"abni\" CompositionEvent true",
            "beforeinput \"你\" insertCompositionText true \"abni\" InputEvent false",
            "input \"你\" insertCompositionText true \"ab你\" InputEvent false",
            "compositionend \"你\" - - \"ab你\" CompositionEvent true",
        ]
    );
    assert_eq!(js(&mut engine, id, "$('i').selectionStart + ',' + $('i').selectionEnd"), "3,3");
}

#[test]
fn empty_marked_text_cancels_the_composition() {
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "i");
    engine.set_marked_text(id, "x");
    take_log(&mut engine, id);
    engine.set_marked_text(id, "");
    assert_eq!(
        take_log(&mut engine, id),
        [
            "compositionupdate \"\" - - \"abx\" CompositionEvent true",
            "beforeinput \"\" insertCompositionText true \"abx\" InputEvent false",
            "input null insertCompositionText true \"ab\" InputEvent false",
            "compositionend \"\" - - \"ab\" CompositionEvent true",
        ]
    );
    // Nothing is marked now: an empty mark does nothing.
    assert!(!engine.set_marked_text(id, ""));
    assert!(take_log(&mut engine, id).is_empty());
}

#[test]
fn unmark_keeps_the_marked_text_and_ends_the_composition() {
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "t");
    engine.set_marked_text(id, "k");
    take_log(&mut engine, id);
    assert!(engine.unmark_text(id));
    assert_eq!(
        take_log(&mut engine, id),
        ["compositionend \"k\" - - \"abk\" CompositionEvent true"]
    );
    assert_eq!(value(&mut engine, id, "t"), "abk");
    assert!(!engine.unmark_text(id), "nothing marked: nothing to do");
}

#[test]
fn insert_text_with_nothing_marked_is_a_cancelable_insert() {
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "i");
    assert!(engine.insert_text(id, "z"));
    assert_eq!(
        take_log(&mut engine, id),
        [
            "beforeinput \"z\" insertText false \"ab\" InputEvent true",
            "input \"z\" insertText false \"abz\" InputEvent false",
        ]
    );
}

#[test]
fn an_accent_picked_from_the_press_and_hold_popover_replaces_the_letter() {
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "i");
    engine.insert_text(id, "e");
    take_log(&mut engine, id);
    // The popover's pick: insertText:"è" replacementRange:{2, 1}. Not
    // logged from Chrome (CDP's insertText takes no replacement range):
    // the events are the plain insertText ones, over the replaced letter.
    assert!(engine.insert_text_replacing(id, "\u{e8}", Some((2, 1))));
    assert_eq!(value(&mut engine, id, "i"), "abè");
    assert_eq!(
        take_log(&mut engine, id),
        [
            "beforeinput \"è\" insertText false \"abe\" InputEvent true",
            "input \"è\" insertText false \"abè\" InputEvent false",
        ]
    );
}

#[test]
fn the_text_input_state_reports_the_marked_range_and_a_caret_rect() {
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "i");
    let before = engine.text_input_state(id).expect("a focused text control");
    assert_eq!(before.marked, None);
    assert_eq!(before.selected, (2, 2));
    engine.set_marked_text(id, "\u{4f60}\u{597d}");
    // The caret is read from the last frame built, as the window draws it.
    engine.render_view(id).expect("frame");
    let state = engine.text_input_state(id).expect("state");
    assert_eq!(state.marked, Some((2, 4)), "UTF-16 start and end of what is marked");
    assert_eq!(state.selected, (4, 4));
    let caret = state.caret.expect("the caret is painted in the focused input");
    assert!(caret.height > 0.0 && caret.x > 0.0 && caret.y >= 0.0 && caret.y < 60.0, "{caret:?}");
    engine.insert_text(id, "\u{4f60}\u{597d}");
    assert_eq!(engine.text_input_state(id).unwrap().marked, None);
    js(&mut engine, id, "$('i').blur()");
    assert!(engine.text_input_state(id).is_none(), "nothing focused: no text input");
}

// ---- Cmd chords: the page's keydown first, then the menu ----

#[test]
fn a_cmd_chord_the_page_cancels_is_claimed_and_carries_meta_key() {
    let (mut engine, id) = page();
    js(
        &mut engine,
        id,
        "window.keys = []; document.addEventListener('keydown', function (e) { \
         keys.push(e.key + ':' + e.metaKey + ':' + e.ctrlKey); if (e.key === 't') e.preventDefault(); }); 0",
    );
    assert!(engine.handle_command_key(id, 0, "t", false, false), "cancelled: the page's");
    assert!(!engine.handle_command_key(id, 0, "w", false, false), "not cancelled: the menu's");
    assert_eq!(js(&mut engine, id, "keys.join(' ')"), "t:true:false w:true:false");
}

#[test]
fn a_cmd_chord_in_a_text_field_edits_and_others_go_to_the_menu() {
    let (mut engine, id) = page();
    engine.clipboard().write_text("XY");
    focus_end(&mut engine, id, "q");
    assert!(engine.handle_command_key(id, 0, "v", false, false), "Cmd+V pastes");
    assert_eq!(value(&mut engine, id, "q"), "helloXY");
    assert!(!engine.handle_command_key(id, 0, "t", false, false), "Cmd+T is the menu's");
    assert!(!engine.handle_command_key(id, 0, "l", false, false), "Cmd+L is the menu's");
}

// ---- the Edit menu's responders ----

#[test]
fn edit_menu_commands_act_on_the_focused_field() {
    use rustkit_dom::forms::EditCommand;
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "q");
    assert!(engine.run_edit_command(id, &EditCommand::SelectAll));
    assert!(engine.run_edit_command(id, &EditCommand::Copy));
    assert_eq!(engine.clipboard().read_text().as_deref(), Some("hello"));
    assert!(engine.run_edit_command(id, &EditCommand::Cut));
    assert_eq!(value(&mut engine, id, "q"), "");
    assert!(engine.run_edit_command(id, &EditCommand::Paste));
    assert!(engine.run_edit_command(id, &EditCommand::Paste));
    assert_eq!(value(&mut engine, id, "q"), "hellohello");
    assert!(engine.run_edit_command(id, &EditCommand::Undo));
    assert_eq!(value(&mut engine, id, "q"), "hello");
    assert!(engine.run_edit_command(id, &EditCommand::Redo));
    assert_eq!(value(&mut engine, id, "q"), "hellohello");
    let log = take_log(&mut engine, id);
    assert!(log.iter().any(|l| l.starts_with("beforeinput \"hello\" insertFromPaste")), "{log:?}");
    js(&mut engine, id, "$('q').blur()");
    assert!(!engine.run_edit_command(id, &EditCommand::Paste), "nothing focused: nothing to paste into");
}

// #918 R1 HOLD, nit 3: focus leaving a field mid-composition finishes it.
// Chrome 143 (tools/parity_oracle/ime_gate_log.mjs, CDP half): the marked
// text stays, compositionend fires before blur, and the next composition
// starts fresh in the newly focused field.
#[test]
fn focus_leaving_a_composing_field_ends_the_composition_first() {
    let (mut engine, id) = page();
    js(
        &mut engine,
        id,
        "['i', 'q'].forEach(function (id) { $(id).addEventListener('blur', function () { log.push('blur ' + id); }); }); 0",
    );
    focus_end(&mut engine, id, "i");
    engine.set_marked_text(id, "n");
    take_log(&mut engine, id);
    js(&mut engine, id, "$('q').focus(); 0");
    assert_eq!(
        take_log(&mut engine, id),
        ["compositionend \"n\" - - \"abn\" CompositionEvent true", "blur i"]
    );
    assert_eq!(value(&mut engine, id, "i"), "abn", "Chrome keeps the marked text");
    js(&mut engine, id, "$('q').setSelectionRange(5, 5); 0");
    engine.set_marked_text(id, "ni");
    let log = take_log(&mut engine, id);
    assert_eq!(log.first().map(String::as_str), Some("compositionstart \"\" - - \"hello\" CompositionEvent true"), "{log:?}");
    assert_eq!(value(&mut engine, id, "i"), "abn");
    assert_eq!(value(&mut engine, id, "q"), "helloni");

    // blur() with nothing taking the focus: the same.
    let (mut engine, id) = page();
    focus_end(&mut engine, id, "t");
    engine.set_marked_text(id, "k");
    take_log(&mut engine, id);
    js(&mut engine, id, "$('t').blur(); 0");
    assert_eq!(take_log(&mut engine, id), ["compositionend \"k\" - - \"abk\" CompositionEvent true"]);
    assert!(engine.text_input_state(id).is_none(), "nothing editable focused");
    assert!(!engine.unmark_text(id), "the composition is over");
}

// #918 R1 HOLD, nit 4, pinned: Chrome 143 does not end a composition when
// script moves the selection (select(), setSelectionRange()); the next
// update still replaces the marked text, and the commit lands there.
#[test]
fn script_selection_mid_composition_leaves_the_composition_open() {
    for act in ["$('i').select()", "$('i').setSelectionRange(0, 0)"] {
        let (mut engine, id) = page();
        focus_end(&mut engine, id, "i");
        engine.set_marked_text(id, "n");
        take_log(&mut engine, id);
        js(&mut engine, id, &format!("{act}; 0"));
        assert_eq!(take_log(&mut engine, id), Vec::<String>::new(), "{act}: no events");
        assert!(engine.set_marked_text(id, "ni"));
        assert_eq!(
            take_log(&mut engine, id),
            [
                "compositionupdate \"ni\" - - \"abn\" CompositionEvent true",
                "beforeinput \"ni\" insertCompositionText true \"abn\" InputEvent false",
                "input \"ni\" insertCompositionText true \"abni\" InputEvent false",
            ],
            "{act}"
        );
        assert!(engine.insert_text(id, "X"));
        assert_eq!(value(&mut engine, id, "i"), "abX", "{act}");
    }
}
