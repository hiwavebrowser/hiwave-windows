//! The engine's own key, wheel and input events are built with the
//! constructors of install time, whatever a page has put on the window
//! since, and only an event the engine made is marked trusted.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><body><input id="t" value="v"></body></html>"#;

const PAGE_SCRIPT: &str = r#"
var pageMade = [], seen = [];
function Fake(type) { var e = new Event(type); pageMade.push(e); return e; }
window.KeyboardEvent = Fake;
window.WheelEvent = Fake;
window.InputEvent = Fake;
var t = document.getElementById('t');
['keydown', 'wheel', 'input'].forEach(function (type) {
    t.addEventListener(type, function (e) {
        seen.push(type + ':' + Object.prototype.toString.call(e) + ':' + e.isTrusted + ':' + (pageMade.indexOf(e) >= 0));
    });
});
"#;

fn text(v: JsValue) -> String {
    match v {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

#[test]
fn engine_events_come_from_install_time_constructors() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    let id = doc.get_element_by_id("t").unwrap().id.raw();
    b.set_document(doc).unwrap();
    b.evaluate(PAGE_SCRIPT).unwrap();
    b.host_turn(&format!("return __rk.fireKey({id}, 'keydown', {{ key: 'a', code: 'KeyA' }});"), &[]).unwrap();
    b.host_turn(&format!("return __rk.fireWheel({id}, {{ clientX: 1, clientY: 1, deltaX: 0, deltaY: 10 }}, true);"), &[]).unwrap();
    b.host_turn(&format!("return __rk.fireInput({id}, 'x');"), &[]).unwrap();
    let seen = text(b.evaluate("seen.join('\\n')").unwrap());
    assert_eq!(
        seen,
        "keydown:[object KeyboardEvent]:true:false\nwheel:[object WheelEvent]:true:false\ninput:[object InputEvent]:true:false",
        "what the page's listeners saw"
    );
    let page_trusted = text(b.evaluate("String(pageMade.filter(function (e) { return e.isTrusted; }).length)").unwrap());
    assert_eq!(page_trusted, "0", "page-made events read trusted");
}

const PAGE2: &str = r#"<!DOCTYPE html><html><body><input id="c" type="checkbox"></body></html>"#;

const PAGE2_SCRIPT: &str = r#"
var clicks = [];
var c = document.getElementById('c');
c.addEventListener('click', function (e) { clicks.push(String(e.isTrusted)); });
Object.defineProperty(Event.prototype, 'type', { get: function () { return 'click'; }, set: function () {}, configurable: true });
window.MouseEvent = window.KeyboardEvent;
"#;

#[test]
fn engine_events_keep_their_own_type() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE2).unwrap());
    let id = doc.get_element_by_id("c").unwrap().id.raw();
    b.set_document(doc).unwrap();
    b.evaluate(PAGE2_SCRIPT).unwrap();
    b.host_turn(&format!("return __rk.fireKey({id}, 'keydown', {{ key: 'a', code: 'KeyA' }});"), &[]).unwrap();
    b.host_turn(&format!("return __rk.fireKey({id}, 'keyup', {{ key: 'a', code: 'KeyA' }});"), &[]).unwrap();
    let got = text(b.evaluate("'clicks=' + clicks.join(',') + ' checked=' + c.checked").unwrap());
    assert_eq!(got, "clicks= checked=false");
}

const PAGE3: &str = r#"<!DOCTYPE html><html><body><a id="l" href="next.html">l</a></body></html>"#;

const PAGE3_SCRIPT: &str = r#"
var moves = [], clicks = [];
var l = document.getElementById('l');
l.addEventListener('mousemove', function (e) { moves.push(String(e.isTrusted)); });
l.addEventListener('click', function (e) { clicks.push(String(e.isTrusted)); e.preventDefault(); });
var realCall = Function.prototype.call, realConstruct = Reflect.construct, hasOwn = Object.prototype.hasOwnProperty;
function bend(o) {
    if (o && typeof o === 'object' && Event.prototype.isPrototypeOf(o) && !hasOwn.apply(o, ['type'])) {
        Object.defineProperty(o, 'type', { value: 'click', writable: true, configurable: true });
        return true;
    }
    return false;
}
Function.prototype.call = function (self) {
    if (bend(self)) return undefined;
    return realCall.apply(this, arguments);
};
Reflect.construct = function (T, args, nt) {
    var o = realConstruct(T, args, nt);
    bend(o);
    return o;
};
function unhook() { Function.prototype.call = realCall; Reflect.construct = realConstruct; }
"#;

#[test]
fn engine_events_are_built_without_page_reachable_code() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE3).unwrap());
    let id = doc.get_element_by_id("l").unwrap().id.raw();
    b.set_document(doc).unwrap();
    b.evaluate(PAGE3_SCRIPT).unwrap();
    let _ = b.host_turn(&format!("return __rk.fireMouse({id}, 'mousemove', {{ clientX: 1, clientY: 1, button: 0, buttons: 0 }});"), &[]);
    let _ = b.host_turn(&format!("return __rk.fireWheel({id}, {{ clientX: 1, clientY: 1, deltaX: 0, deltaY: 10 }}, true);"), &[]);
    b.evaluate("unhook()").unwrap();
    let got = text(b.evaluate("'moves=' + moves.join(',') + ' clicks=' + clicks.join(',')").unwrap());
    assert_eq!(got, "moves=true clicks=");
}

const PAGE4_SCRIPT: &str = r#"
var replaced = 0, got = [];
var realDispatch = EventTarget.prototype.dispatchEvent;
EventTarget.prototype.dispatchEvent = function () { replaced++; return realDispatch.apply(this, arguments); };
window.addEventListener('message', function (e) { got.push('window:' + e.data + ':' + e.isTrusted); });
window.addEventListener('ping', function (e) { got.push('ping:' + e.isTrusted); });
var ch = new MessageChannel();
ch.port1.onmessage = function (e) { got.push('port:' + e.data + ':' + e.isTrusted); };
window.postMessage('w', '*');
ch.port2.postMessage('p');
"#;

#[test]
fn engine_dispatches_do_not_go_through_the_pages_dispatch_event() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.evaluate(PAGE4_SCRIPT).unwrap();
    b.host_turn("return __rk.fire('window', 'ping');", &[]).unwrap();
    b.run_timers(0, 1_000).unwrap();
    let got = text(b.evaluate("'replaced=' + replaced + ' ' + got.slice().sort().join(' ')").unwrap());
    assert_eq!(got, "replaced=0 ping:true port:p:true window:w:true");
}

const PAGE5: &str = r#"<!DOCTYPE html><html><body><input id="q" value=""></body></html>"#;

const PAGE5_SCRIPT: &str = r#"
var hooked = 0, got = [];
var q = document.getElementById('q');
['beforeinput', 'input'].forEach(function (t) { q.addEventListener(t, function (e) { got.push(t + ':' + e.isTrusted); }); });
var realCall = Function.prototype.call;
Function.prototype.call = function (self, ev) {
    if (ev && typeof ev === 'object' && ev.isTrusted === true) hooked++;
    return realCall.apply(this, arguments);
};
function unhook() { Function.prototype.call = realCall; }
"#;

#[test]
fn the_users_edit_events_are_dispatched_by_the_engine_alone() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE5).unwrap());
    let id = doc.get_element_by_id("q").unwrap().id.raw();
    b.set_document(doc).unwrap();
    b.evaluate(PAGE5_SCRIPT).unwrap();
    b.evaluate("q.focus()").unwrap();
    let _ = b.user_edit(id, &rustkit_dom::forms::EditCommand::InsertText("a".to_string()));
    let _ = b.user_compose(id, "mark", "b");
    let _ = b.user_compose(id, "commit", "b");
    b.evaluate("q.blur()").unwrap();
    b.evaluate("unhook()").unwrap();
    let got = text(b.evaluate("'hooked=' + hooked + ' ' + got.join(',')").unwrap());
    assert!(got.starts_with("hooked=0 beforeinput:true,input:true"), "{got}");
}

const PAGE6: &str = r#"<!DOCTYPE html><html><body><div id="d"><span id="s">s</span></div></body></html>"#;

const PAGE6_SCRIPT: &str = r#"
var log = [];
var s = document.getElementById('s'), d = document.getElementById('d');
s.addEventListener('wheel', function (e) {
    e.preventDefault();
    try { e.eventPhase = 0; } catch (_) {}
    try { e.initEvent('click', true, true); } catch (_) {}
});
d.addEventListener('wheel', function (e) { log.push('wheel:' + e.type + ':' + e.isTrusted + ':' + e.defaultPrevented); });
d.addEventListener('click', function (e) { log.push('click:' + e.isTrusted); });
"#;

#[test]
fn an_engine_event_is_not_reinitialised_during_its_dispatch() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE6).unwrap());
    let id = doc.get_element_by_id("s").unwrap().id.raw();
    b.set_document(doc).unwrap();
    b.evaluate(PAGE6_SCRIPT).unwrap();
    let _ = b.host_turn(&format!("return __rk.fireWheel({id}, {{ clientX: 1, clientY: 1, deltaX: 0, deltaY: 10 }}, true);"), &[]);
    let got = text(b.evaluate("log.join(',')").unwrap());
    assert_eq!(got, "wheel:wheel:true:true");
}
