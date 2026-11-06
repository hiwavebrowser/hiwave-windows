//! Form-control IDL state (HTML §4.10): checkedness, select/option
//! selectedness, button/label/fieldset reflection, form.elements/length
//! and requestSubmit, and the `Image`/`Option` named constructors. Its own
//! test module so it does not collide with the other families' tests in
//! `lib.rs`.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><body>
<form id="f" name="signup"><fieldset id="fs"><legend>L</legend>
<label id="lab" for="q">Q</label><input id="q" name="q" value="ab">
<label id="wrap">W <input id="inner" type="checkbox" checked></label>
<input id="r1" type="radio" name="g" checked><input id="r2" type="radio" name="g">
<select id="sel" name="s"><option value="a">A</option><optgroup><option selected>  B
 b </option></optgroup></select>
<select id="multi" multiple><option id="m1" selected>1</option><option id="m2" selected>2</option></select>
<select id="none"><option disabled>x</option><option id="y">y</option></select>
<textarea id="t">hi</textarea><input type="image" id="img">
<button id="b">Go</button><button id="rb" type="reset">R</button></fieldset></form>
</body></html>"#;

fn bound() -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html(PAGE).unwrap()))
        .unwrap();
    bindings
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => n.to_string(),
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

#[test]
fn checkedness_follows_the_attribute_until_set_and_radios_exclude() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var c = document.getElementById('inner'), r = []; \
             r.push(c.checked, c.defaultChecked); c.removeAttribute('checked'); r.push(c.checked); \
             c.checked = true; c.defaultChecked = false; r.push(c.checked, c.hasAttribute('checked')); \
             var r1 = document.getElementById('r1'), r2 = document.getElementById('r2'); \
             r.push(r1.checked, r2.checked); r2.checked = true; r.push(r1.checked, r2.checked); \
             var n = document.createElement('input'); n.type = 'checkbox'; n.checked = true; \
             r.push(n.checked, n.defaultChecked); \
             document.getElementById('f').reset(); r.push(c.checked, r1.checked, r2.checked); \
             r.join(',')"
        ),
        "true,true,false,true,false,true,false,false,true,true,false,false,true,false"
    );
}

#[test]
fn input_files_is_an_empty_list_only_for_file_inputs() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var i = document.createElement('input'), r = [String(i.files)]; i.type = 'file'; \
             var f = i.files; r.push(f.length, String(f.item(0)), f instanceof FileList, \
                                     Array.from(f).length); r.join(',')"
        ),
        "null,0,null,true,0"
    );
}

#[test]
fn select_options_selected_index_value_and_selected_options() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('sel'), r = []; \
             r.push(s.options.length, s.length, s.selectedIndex, s.value, s.type, \
                    s.options[1].text, s.options[1].value, s.options[1].index, s.selectedOptions.length); \
             s.selectedIndex = 0; r.push(s.value, s.options[0].selected, s.options[1].selected); \
             s.value = 'B b'; r.push(s.selectedIndex); s.value = 'zzz'; r.push(s.selectedIndex, s.value); \
             var m = document.getElementById('multi'); \
             r.push(m.type, m.selectedIndex, m.selectedOptions.length, m.value); \
             document.getElementById('m1').selected = false; r.push(m.selectedIndex, m.value); \
             var n = document.getElementById('none'); r.push(n.selectedIndex, n.value); \
             r.join(',')"
        ),
        "2,2,1,B b,select-one,B b,B b,1,1,a,true,false,1,-1,,select-multiple,0,2,1,1,2,1,y"
    );
}

#[test]
fn single_select_keeps_one_selected_option() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('sel'), o = s.options, r = []; \
             o[0].selected = true; r.push(o[0].selected, o[1].selected, o[1].defaultSelected); \
             o[0].selected = false; r.push(s.selectedIndex); \
             var opt = document.createElement('option'); opt.textContent = 'C'; opt.value = 'c'; \
             s.add(opt); r.push(s.length, opt.value, opt.getAttribute('value')); \
             document.getElementById('f').reset(); r.push(s.value); r.join(',')"
        ),
        "true,false,true,0,3,c,c,B b"
    );
}

#[test]
fn button_label_and_fieldset_reflect() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var bt = document.getElementById('b'), r = []; \
             r.push(bt.type, document.getElementById('rb').type, bt.disabled, bt.form.id); \
             bt.type = 'BUTTON'; bt.disabled = true; r.push(bt.type, bt.hasAttribute('disabled')); \
             bt.type = 'nope'; r.push(bt.type); \
             var l = document.getElementById('lab'), w = document.getElementById('wrap'); \
             r.push(l.htmlFor, l.control.id, w.htmlFor, w.control.id, l.form.id); \
             l.htmlFor = 'missing'; r.push(l.getAttribute('for'), String(l.control)); \
             var fs = document.getElementById('fs'); r.push(fs.disabled, fs.type); fs.disabled = true; \
             r.push(fs.hasAttribute('disabled'), fs.elements.length); r.join(',')"
        ),
        "submit,reset,false,f,button,true,submit,q,q,,inner,f,missing,null,false,fieldset,true,11"
    );
}

#[test]
fn form_elements_length_and_named_access() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var f = document.getElementById('f'), e = f.elements, r = []; \
             r.push(f.length, e.length, e[0].id, e.namedItem('q').id, e.q.id, \
                    e[e.length - 1].id, Array.prototype.indexOf.call(e, document.getElementById('img'))); \
             r.join(',')"
        ),
        "11,11,fs,q,q,rb,-1"
    );
}

#[test]
fn request_submit_fires_a_cancelable_submit_event_after_validation() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var f = document.getElementById('f'), bt = document.getElementById('b'), r = []; \
             f.addEventListener('submit', function (e) { \
                 r.push(e.type, e.cancelable, e.bubbles, e.submitter === null ? 'none' : e.submitter.id); \
                 e.preventDefault(); }); \
             f.requestSubmit(); f.requestSubmit(bt); \
             try { f.requestSubmit(document.getElementById('q')); } catch (e) { r.push(e.name); } \
             var q = document.getElementById('q'); q.value = ''; q.required = true; \
             q.addEventListener('invalid', function () { r.push('invalid'); }); \
             f.requestSubmit(); r.push(f.checkValidity()); \
             f.noValidate = true; f.requestSubmit(); r.join(',')"
        ),
        "submit,true,true,none,submit,true,true,b,TypeError,invalid,invalid,false,submit,true,true,none"
    );
}

#[test]
fn image_constructor_makes_an_img_with_reflected_size_and_src() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var i = new Image(4, 5), r = []; \
             r.push(i.tagName, i instanceof HTMLImageElement, i.width, i.height, \
                    i.getAttribute('width'), new Image().hasAttribute('width')); \
             i.src = 'pic.png'; r.push(i.getAttribute('src'), i.parentNode === null); \
             i.width = 9; r.push(i.getAttribute('width'), Image.prototype === HTMLImageElement.prototype); \
             r.join(',')"
        ),
        "IMG,true,4,5,4,false,pic.png,true,9,true"
    );
}

#[test]
fn option_constructor_sets_text_value_and_selectedness() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var o = new Option('t', 'v'), r = []; \
             r.push(o.tagName, o instanceof HTMLOptionElement, o.text, o.value, o.selected, o.defaultSelected); \
             var p = new Option('x', undefined, true, false); \
             r.push(p.value, p.defaultSelected, p.selected, new Option().childNodes.length); \
             var s = document.getElementById('sel'); s.add(new Option('Z', 'z', false, true)); \
             r.push(s.value, s.selectedIndex); r.join(',')"
        ),
        "OPTION,true,t,v,false,false,x,true,false,0,z,2"
    );
}

// Activation behaviour: a click (here `el.click()`; the engine's tests
// cover the user's) checks the control, and the engine is told.
#[test]
fn click_activates_checkboxes_radios_and_labels() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var $ = function (i) { return document.getElementById(i); }, r = [], seen = []; \
             var c = $('inner'); \
             c.addEventListener('change', function () { seen.push('change:' + c.checked); }); \
             c.click(); r.push(c.checked); \
             $('wrap').click(); r.push(c.checked); \
             c.dispatchEvent(new Event('click', { bubbles: true })); r.push(c.checked); \
             c.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true })); r.push(c.checked); \
             c.setAttribute('disabled', ''); c.click(); $('wrap').click(); r.push(c.checked); \
             $('r2').click(); r.push($('r1').checked, $('r2').checked); \
             var q = 0; $('q').addEventListener('click', function () { q++; }); $('lab').click(); r.push(q); \
             r.join(',') + ' ' + seen.join(',')"
        ),
        "false,true,true,false,false,false,true,1 change:false,change:true,change:false"
    );
    let inner = match b.evaluate("document.getElementById('inner').checked = true; 0") {
        Ok(_) => b.take_checked_writes().last().copied().expect("a write"),
        Err(e) => panic!("{e}"),
    };
    assert_eq!(inner.1, Some(true));
    // A reset hands the control back to its attribute.
    b.evaluate("document.getElementById('f').reset()").unwrap();
    assert!(b.take_checked_writes().contains(&(inner.0, None)));
    assert!(b.take_checked_writes().is_empty());
}

// A click on a submit button fires `submit` with the button as submitter
// and, uncancelled, asks the engine to submit; a reset button resets.
#[test]
fn click_on_a_submit_or_reset_button_acts_on_its_form() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var $ = function (i) { return document.getElementById(i); }, seen = []; \
             $('f').addEventListener('submit', function (e) { \
                 seen.push('submit:' + e.submitter.id); if (window.block) e.preventDefault(); }); \
             $('f').addEventListener('reset', function () { seen.push('reset'); }); \
             $('b').click(); seen.join(',')"
        ),
        "submit:b"
    );
    let requests = b.take_submit_requests();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].1.is_some(), "the submitter goes with the request");

    // Cancelled: the event runs, the engine is not asked.
    assert_eq!(
        ev(&b, "seen.length = 0; window.block = true; $('b').click(); window.block = false; seen.join(',')"),
        "submit:b"
    );
    assert!(b.take_submit_requests().is_empty());

    // A reset button puts the form's controls back and submits nothing.
    assert_eq!(
        ev(
            &b,
            "seen.length = 0; $('inner').checked = false; $('rb').click(); \
             seen.join(',') + ' ' + $('inner').checked"
        ),
        "reset true"
    );
    assert!(b.take_submit_requests().is_empty());
}

/// A protocol-relative URL (`//host/path`) reads back with the document's
/// scheme and its own host and path (WHATWG URL): the Wikipedia portal's
/// search form, and the other URL-reflecting attributes.
#[test]
fn protocol_relative_urls_read_back_absolute() {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(
            Document::parse_html(
                r#"<html><body><form id="f" action="//www.wikipedia.org/search-redirect.php"></form>
<a id="a" href="//example.test/p?q#f">a</a><img id="i" src="//example.test"></body></html>"#,
            )
            .unwrap(),
        ))
        .unwrap();
    bindings
        .set_location(&url::Url::parse("https://www.wikipedia.org/").unwrap())
        .unwrap();
    assert_eq!(
        ev(
            &bindings,
            "var $ = function (id) { return document.getElementById(id); }; \
             [$('f').action, $('a').href, $('i').src, new URL('//example.test/s', location.href).href].join(' ')"
        ),
        "https://www.wikipedia.org/search-redirect.php https://example.test/p?q#f https://example.test/ https://example.test/s"
    );
}

// CH2.E6: the engine's `<select>` drop-down picks through
// `DomBindings::pick_option`: selectedness moves, `input` then `change`
// fire once each (trusted, bubbling), and the engine is told each
// option's selectedness, which `:checked` and the painted label follow.
const PICK_PAGE: &str = r#"<!DOCTYPE html><html><body>
<select id="s"><option>Zero</option><option disabled>One</option><optgroup label="G"><option>Two</option></optgroup>
<optgroup label="Off" disabled><option>Three</option></optgroup><option>Four</option></select>
</body></html>"#;

fn bound_pick() -> (DomBindings, usize) {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PICK_PAGE).unwrap());
    bindings.set_document(doc.clone()).unwrap();
    let s = doc.get_element_by_id("s").expect("select").id.raw();
    ev(
        &bindings,
        "window.log = []; var s = document.getElementById('s'); \
         ['input', 'change'].forEach(function (t) { document.addEventListener(t, function (e) { \
         log.push(t + ':' + e.target.id + ':' + e.isTrusted + ':' + e.bubbles + ':' + s.selectedIndex); }); }); ''",
    );
    (bindings, s)
}

#[test]
fn picking_an_option_selects_it_and_fires_input_then_change_once() {
    let (b, s) = bound_pick();
    b.take_checked_writes();
    assert!(b.pick_option(s, 2).unwrap());
    assert_eq!(
        ev(&b, "[s.selectedIndex, s.value, s.options[2].selected, s.options[0].selected].join(',')"),
        "2,Two,true,false"
    );
    assert_eq!(ev(&b, "log.join(' ')"), "input:s:true:true:2 change:s:true:true:2");
    // The engine is told: option 2 selected, the others not.
    let writes = b.take_checked_writes();
    assert!(writes.iter().any(|(_, v)| *v == Some(true)), "a selected write: {writes:?}");
    assert!(writes.iter().any(|(_, v)| *v == Some(false)), "a deselected write: {writes:?}");
    // The same option again changes nothing and fires nothing.
    assert!(!b.pick_option(s, 2).unwrap());
    assert_eq!(ev(&b, "String(log.length)"), "2");
}

#[test]
fn a_disabled_option_or_one_in_a_disabled_group_cannot_be_picked() {
    let (b, s) = bound_pick();
    assert!(!b.pick_option(s, 1).unwrap(), "disabled option");
    assert!(!b.pick_option(s, 3).unwrap(), "option in a disabled optgroup");
    assert!(!b.pick_option(s, 9).unwrap(), "no such option");
    assert_eq!(ev(&b, "s.selectedIndex + ':' + log.length"), "0:0");
}

#[test]
fn script_selection_reaches_the_engine_as_option_checkedness() {
    let (b, _) = bound_pick();
    b.take_checked_writes();
    ev(&b, "s.selectedIndex = 4; ''");
    let writes = b.take_checked_writes();
    assert_eq!(writes.iter().filter(|(_, v)| *v == Some(true)).count(), 1, "{writes:?}");
    assert_eq!(writes.iter().filter(|(_, v)| *v == Some(false)).count(), 4, "{writes:?}");
    // No events for a script change.
    assert_eq!(ev(&b, "String(log.length)"), "0");
}
