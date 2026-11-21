//! CH2.E1: `document.execCommand`, `navigator.clipboard`, and the
//! `beforeinput`/`input` a user's edit command fires. The commands run on
//! rustkit-dom's TextEditState, the model the engine's keys drive.

use super::*;
use rustkit_dom::clipboard::{Clipboard, MemoryClipboard};
use rustkit_dom::forms::EditCommand;

const PAGE: &str = r#"<!DOCTYPE html><html><body>
<input id="q" value="hello"><input id="pw" type="password" value="secret">
<input id="short" maxlength="4"><textarea id="t">a</textarea>
<input id="box" type="checkbox"><div id="plain">text</div>
</body></html>"#;

struct Page {
    b: DomBindings,
    doc: Rc<Document>,
    cb: Rc<MemoryClipboard>,
}

impl Page {
    fn id(&self, el: &str) -> usize {
        self.doc.get_element_by_id(el).unwrap().id.raw()
    }
}

/// A page that may read the clipboard, as HiWave's own chrome may.
fn page() -> Page {
    let p = web_page();
    p.b.set_clipboard_read_allowed(true);
    p
}

/// A web page: script clipboard reads are denied (the default).
fn web_page() -> Page {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    let cb = Rc::new(MemoryClipboard::new());
    b.set_clipboard(cb.clone());
    b.evaluate(
        "var $ = function (i) { return document.getElementById(i); }; window.log = []; \
         ['beforeinput', 'input'].forEach(function (t) { document.addEventListener(t, function (e) { \
         log.push(t + ':' + e.target.id + ':' + e.inputType + ':' + e.data + ':' + e.target.value); }); });",
    )
    .unwrap();
    Page { b, doc, cb }
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(v) => v.to_string(),
        JsValue::Number(n) => n.to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

#[test]
fn exec_command_copy_with_a_selection_fills_the_clipboard() {
    let p = page();
    assert_eq!(
        ev(
            &p.b,
            "$('q').focus(); $('q').setSelectionRange(1, 4); document.execCommand('copy')"
        ),
        "true"
    );
    assert_eq!(p.cb.read_text().as_deref(), Some("ell"));
    // Copy changes nothing, so no input event.
    assert_eq!(ev(&p.b, "$('q').value + '|' + log.length"), "hello|0");
}

#[test]
fn navigator_clipboard_write_then_read_resolves_the_text() {
    let p = page();
    // writeText needs a user activation (a copy button's click).
    p.b.notify_user_activation();
    p.b.evaluate(
        "window.got = []; navigator.clipboard.writeText('x').then(function (v) { got.push('w:' + v); \
         return navigator.clipboard.readText(); }).then(function (t) { got.push('r:' + t); });",
    )
    .unwrap();
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "w:undefined,r:x");
    assert_eq!(p.cb.read_text().as_deref(), Some("x"));
    assert_eq!(
        ev(
            &p.b,
            "navigator.clipboard instanceof Clipboard && typeof navigator.clipboard.readText"
        ),
        "function"
    );
}

#[test]
fn a_web_page_is_denied_clipboard_reads_by_default() {
    let p = web_page();
    p.cb.write_text("private");
    p.b.evaluate(
        "window.got = []; navigator.clipboard.readText().then(function (t) { got.push('r:' + t); }, \
         function (e) { got.push(e.name); });",
    )
    .unwrap();
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "NotAllowedError");
    assert_eq!(
        ev(&p.b, "$('q').focus(); document.execCommand('paste')"),
        "false"
    );
    assert_eq!(
        ev(
            &p.b,
            "$('q').value + '|' + document.queryCommandEnabled('paste')"
        ),
        "hello|false"
    );
    // The user's own paste still reads it.
    p.b.user_edit(p.id("q"), &EditCommand::Paste).unwrap();
    assert_eq!(ev(&p.b, "$('q').value"), "helloprivate");
}

#[test]
fn exec_command_edits_the_focused_control_and_fires_input() {
    let p = page();
    assert_eq!(
        ev(&p.b, "$('q').focus(); document.execCommand('selectAll')"),
        "true"
    );
    assert_eq!(
        ev(&p.b, "$('q').selectionStart + ',' + $('q').selectionEnd"),
        "0,5"
    );
    assert_eq!(ev(&p.b, "document.execCommand('cut')"), "true");
    assert_eq!(p.cb.read_text().as_deref(), Some("hello"));
    assert_eq!(ev(&p.b, "$('q').value"), "");
    assert_eq!(
        ev(&p.b, "document.execCommand('insertText', false, 'ab')"),
        "true"
    );
    assert_eq!(ev(&p.b, "document.execCommand('paste')"), "true");
    assert_eq!(
        ev(&p.b, "$('q').value + '|' + $('q').selectionStart"),
        "abhello|7"
    );
    assert_eq!(ev(&p.b, "document.execCommand('undo')"), "true");
    assert_eq!(ev(&p.b, "$('q').value"), "ab");
    assert_eq!(ev(&p.b, "document.execCommand('redo')"), "true");
    assert_eq!(ev(&p.b, "$('q').value"), "abhello");
    // execCommand fires `input` (not `beforeinput`, Input Events §5.1).
    assert_eq!(
        ev(&p.b, "log.join(' / ')"),
        "input:q:deleteByCut:null: / input:q:insertText:ab:ab / input:q:insertFromPaste:hello:abhello / \
         input:q:historyUndo:null:ab / input:q:historyRedo:null:abhello"
    );
}

#[test]
fn exec_command_answers_false_when_it_cannot_run() {
    let p = page();
    // Nothing focused.
    assert_eq!(ev(&p.b, "document.execCommand('selectAll')"), "false");
    // Unsupported commands.
    assert_eq!(
        ev(&p.b, "$('q').focus(); document.execCommand('bold')"),
        "false"
    );
    // Nothing to undo yet.
    assert_eq!(ev(&p.b, "document.execCommand('undo')"), "false");
    // A password field refuses copy.
    p.cb.write_text("keep");
    assert_eq!(
        ev(
            &p.b,
            "$('pw').focus(); $('pw').select(); document.execCommand('copy')"
        ),
        "false"
    );
    assert_eq!(p.cb.read_text().as_deref(), Some("keep"));
    // A checkbox is not a text control.
    assert_eq!(
        ev(&p.b, "$('box').focus(); document.execCommand('paste')"),
        "false"
    );
    assert_eq!(
        ev(&p.b, "['copy','cut','paste','selectAll','undo','redo','insertText','bold'].map(function (c) { \
                  return document.queryCommandSupported(c); }).join()"),
        "true,true,true,true,true,true,true,false"
    );
}

#[test]
fn exec_command_paste_respects_maxlength() {
    let p = page();
    p.cb.write_text("123456");
    assert_eq!(
        ev(&p.b, "$('short').focus(); document.execCommand('paste')"),
        "true"
    );
    assert_eq!(ev(&p.b, "$('short').value"), "1234");
}

#[test]
fn a_user_paste_fires_beforeinput_with_insert_from_paste_then_input() {
    let p = page();
    p.cb.write_text("XY");
    let q = p.id("q");
    let out = p.b.user_edit(q, &EditCommand::Paste).unwrap();
    assert!(out.changed);
    assert_eq!(out.input_type, Some("insertFromPaste"));
    assert_eq!(
        ev(&p.b, "log.join(' / ')"),
        "beforeinput:q:insertFromPaste:XY:hello / input:q:insertFromPaste:XY:helloXY"
    );
    assert_eq!(
        ev(&p.b, "$('q').value + '|' + $('q').selectionStart"),
        "helloXY|7"
    );
}

#[test]
fn a_cancelled_beforeinput_stops_the_edit() {
    let p = page();
    p.cb.write_text("XY");
    p.b.evaluate("$('q').addEventListener('beforeinput', function (e) { e.preventDefault(); });")
        .unwrap();
    let out = p.b.user_edit(p.id("q"), &EditCommand::Paste).unwrap();
    assert!(!out.changed);
    assert_eq!(ev(&p.b, "$('q').value + '|' + log.length"), "hello|1");
}

#[test]
fn user_cut_and_undo_fire_their_input_types() {
    let p = page();
    let q = p.id("q");
    p.b.user_edit(q, &EditCommand::SelectAll).unwrap();
    p.b.user_edit(q, &EditCommand::Cut).unwrap();
    p.b.user_edit(q, &EditCommand::Undo).unwrap();
    assert_eq!(
        ev(&p.b, "log.join(' / ')"),
        "beforeinput:q:deleteByCut:null:hello / input:q:deleteByCut:null: / \
         beforeinput:q:historyUndo:null: / input:q:historyUndo:null:hello"
    );
    assert_eq!(p.cb.read_text().as_deref(), Some("hello"));
}

#[test]
fn edits_run_on_the_shared_edit_state_the_engine_paints() {
    let p = page();
    let states: SharedEditStates = Rc::default();
    let t = p.id("t");
    let typed = rustkit_dom::forms::TextEditState::with_value("a");
    typed.insert_text("b");
    states.borrow_mut().insert(t, typed);
    p.b.share_edit_states(states.clone());
    p.b.sync_control_value(t, "ab".to_string());
    // Script's undo undoes the user's typing, in the engine's state.
    assert_eq!(
        ev(&p.b, "$('t').focus(); document.execCommand('undo')"),
        "true"
    );
    assert_eq!(states.borrow()[&t].value(), "a");
    assert_eq!(ev(&p.b, "$('t').value"), "a");
    // A textarea's paste keeps line breaks.
    p.cb.write_text("1\n2");
    assert_eq!(ev(&p.b, "document.execCommand('paste')"), "true");
    assert_eq!(states.borrow()[&t].value(), "a1\n2");
}

// ---- CH2.E1 R1 HOLD: clipboard permissions ----

/// A key press the user made (trusted `keydown` at the active element).
fn press_key(b: &DomBindings) {
    let data = KeyboardEventBindingData {
        key: "k".to_string(),
        ..Default::default()
    };
    b.fire_key_event(None, "keydown", &data).unwrap();
}

#[test]
fn read_text_rejects_by_default() {
    let p = web_page();
    p.cb.write_text("copied in another tab");
    p.b.evaluate(
        "window.got = []; navigator.clipboard.readText().then(function (t) { got.push('r:' + t); }, \
         function (e) { got.push(e.name + ':' + (e instanceof DOMException)); });",
    )
    .unwrap();
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "NotAllowedError:true");
}

#[test]
fn read_text_resolves_when_the_embedder_grants_clipboard_read() {
    let p = web_page();
    p.cb.write_text("chrome page text");
    p.b.set_clipboard_read_allowed(true);
    p.b.evaluate(
        "window.got = []; navigator.clipboard.readText().then(function (t) { got.push('r:' + t); }, \
         function (e) { got.push(e.name); });",
    )
    .unwrap();
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "r:chrome page text");
    // Revoking the grant denies the next read.
    p.b.set_clipboard_read_allowed(false);
    p.b.evaluate(
        "window.got = []; navigator.clipboard.readText().then(function (t) { got.push('r:' + t); }, \
         function (e) { got.push(e.name); });",
    )
    .unwrap();
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "NotAllowedError");
}

#[test]
fn exec_command_paste_from_script_is_false_by_default_and_true_with_the_grant() {
    let p = web_page();
    p.cb.write_text("X");
    assert_eq!(
        ev(&p.b, "$('q').focus(); $('q').setSelectionRange(5, 5); document.execCommand('paste')"),
        "false"
    );
    assert_eq!(ev(&p.b, "$('q').value + '|' + log.length"), "hello|0");
    p.b.set_clipboard_read_allowed(true);
    assert_eq!(ev(&p.b, "document.execCommand('paste')"), "true");
    assert_eq!(ev(&p.b, "$('q').value"), "helloX");
}

#[test]
fn write_text_without_user_activation_rejects() {
    let p = web_page();
    p.cb.write_text("keep");
    p.b.evaluate(
        "window.got = []; navigator.clipboard.writeText('overwrite').then(function () { got.push('ok'); }, \
         function (e) { got.push(e.name + ':' + (e instanceof DOMException)); });",
    )
    .unwrap();
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "NotAllowedError:true");
    assert_eq!(p.cb.read_text().as_deref(), Some("keep"));
}

#[test]
fn write_text_with_user_activation_resolves() {
    let p = web_page();
    p.cb.write_text("keep");
    // A copy button: the page writes from its key handler.
    p.b.evaluate(
        "window.got = []; document.addEventListener('keydown', function () { \
         navigator.clipboard.writeText('from a key').then(function () { got.push('ok'); }, \
         function (e) { got.push(e.name); }); });",
    )
    .unwrap();
    press_key(&p.b);
    p.b.run_jobs().unwrap();
    assert_eq!(ev(&p.b, "got.join()"), "ok");
    assert_eq!(p.cb.read_text().as_deref(), Some("from a key"));
}

#[test]
fn script_writes_to_a_control_value_need_no_activation() {
    let p = web_page();
    assert_eq!(ev(&p.b, "$('q').value = 'set by script'; $('q').value"), "set by script");
    assert_eq!(
        ev(&p.b, "$('q').focus(); document.execCommand('insertText', false, '!'); $('q').value"),
        "set by script!"
    );
}
