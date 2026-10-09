//! Custom elements (web_components.js). The page is a real parsed document;
//! the selector matcher stands in for the engine's (tag-name lists only).

use super::*;

fn bound(html: &str) -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(html).unwrap())).unwrap();
    b.set_selector_matcher(Rc::new(|node, selector, _| {
        Some(selector.split(',').any(|s| node.tag_name() == Some(s.trim())))
    }));
    b.evaluate("var log = [];").unwrap();
    b
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

fn thrown(b: &DomBindings, script: &str) -> String {
    ev(b, &format!("(function () {{ try {{ {script} }} catch (e) {{ return e.name; }} return 'no error'; }})()"))
}

const PAGE: &str = "<html><body><div id=host><x-a id=a1 color=red></x-a><p>text</p><x-a id=a2></x-a></div></body></html>";

#[test]
fn the_registry_validates_names_and_constructors() {
    let b = bound(PAGE);
    b.evaluate("class X extends HTMLElement {}").unwrap();
    for bad in ["foo", "Foo-bar", "font-face", "a", "1-x", "x-A", ""] {
        assert_eq!(thrown(&b, &format!("customElements.define('{bad}', class extends HTMLElement {{}});")), "SyntaxError", "{bad}");
    }
    assert_eq!(thrown(&b, "customElements.define('x-ok', {});"), "TypeError");
    assert_eq!(thrown(&b, "customElements.define('x-ok', X); customElements.define('x-ok', class extends HTMLElement {});"), "NotSupportedError");
    assert_eq!(thrown(&b, "customElements.define('x-two', X);"), "NotSupportedError", "one constructor, one name");
    assert_eq!(thrown(&b, "customElements.define('x-ext', class extends HTMLElement {}, { extends: 'div' });"), "NotSupportedError");
    assert_eq!(
        ev(&b, "[customElements.get('x-ok') === X, String(customElements.get('nope')), customElements.getName(X), String(customElements.getName(Object)), Object.prototype.toString.call(customElements)].join()"),
        "true,undefined,x-ok,null,[object CustomElementRegistry]"
    );
    assert_eq!(thrown(&b, "new CustomElementRegistry();"), "TypeError");
}

#[test]
fn whenDefined_resolves_when_the_name_is_defined() {
    let b = bound(PAGE);
    b.evaluate(
        "class W extends HTMLElement {} \
         customElements.whenDefined('x-late').then(function (c) { log.push('late:' + (c === W)); }); \
         customElements.whenDefined('bad').catch(function (e) { log.push('bad:' + e.name); }); \
         log.push('before');",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "before,bad:SyntaxError");
    b.evaluate("customElements.define('x-late', W);").unwrap();
    assert_eq!(ev(&b, "log.join()"), "before,bad:SyntaxError,late:true");
    b.evaluate("var done = 'no'; customElements.whenDefined('x-late').then(function () { done = 'again'; });").unwrap();
    assert_eq!(ev(&b, "done"), "again");
}

#[test]
fn defining_upgrades_the_elements_already_in_the_document() {
    let b = bound(PAGE);
    b.evaluate(
        "class XA extends HTMLElement { \
             static get observedAttributes() { return ['color']; } \
             constructor() { super(); log.push('ctor:' + this.id + ':' + (this === document.getElementById(this.id)) + ':' + this.isConnected); } \
             connectedCallback() { log.push('connected:' + this.id); } \
             attributeChangedCallback(n, o, v) { log.push('attr:' + this.id + ':' + n + ':' + o + ':' + v); } \
             hello() { return 'hi ' + this.id; } \
         } \
         var before = document.getElementById('a1'); \
         log.push('pre:' + (typeof before.hello)); \
         customElements.define('x-a', XA);",
    )
    .unwrap();
    assert_eq!(
        ev(&b, "log.join()"),
        "pre:undefined,ctor:a1:true:true,attr:a1:color:null:red,connected:a1,ctor:a2:true:true,connected:a2"
    );
    assert_eq!(
        ev(&b, "var a1 = document.getElementById('a1'); [a1 === before, a1 instanceof XA, a1 instanceof HTMLElement, a1.hello(), a1.constructor === XA].join()"),
        "true,true,true,hi a1,true"
    );
    assert_eq!(
        ev(&b, "var para = document.querySelector('p'); (para instanceof HTMLElement) + ':' + (para instanceof XA)"),
        "true:false"
    );
}

#[test]
fn new_and_createElement_make_a_defined_element_that_connects_on_insertion() {
    let b = bound(PAGE);
    b.evaluate(
        "class XB extends HTMLElement { constructor() { super(); log.push('ctor'); } \
             connectedCallback() { log.push('connected'); } disconnectedCallback() { log.push('disconnected'); } } \
         customElements.define('x-b', XB); log.length = 0; \
         var n = new XB(); var c = document.createElement('x-b'); \
         log.push([n.localName, n instanceof XB, c instanceof XB, n.isConnected, n.tagName].join('/'));",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "ctor,ctor,x-b/true/true/false/X-B");
    b.evaluate("log.length = 0; var host = document.getElementById('host'); host.appendChild(n); log.push('inserted');").unwrap();
    assert_eq!(ev(&b, "log.join()"), "connected,inserted");
    // Moving a connected element: disconnected, then connected at the new place.
    b.evaluate("log.length = 0; document.body.appendChild(n);").unwrap();
    assert_eq!(ev(&b, "log.join()"), "disconnected,connected");
    // An element made before its definition upgrades when it is inserted.
    b.evaluate(
        "log.length = 0; var early = document.createElement('x-c'); log.push(typeof early.later); \
         class XC extends HTMLElement { connectedCallback() { log.push('xc connected'); } later() {} } \
         customElements.define('x-c', XC); log.push(typeof early.later); document.body.appendChild(early);",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "undefined,undefined,xc connected");
    assert_eq!(ev(&b, "typeof early.later + ':' + (early instanceof XC)"), "function:true");
}

#[test]
fn disconnected_fires_on_every_way_out_of_the_document() {
    let b = bound("<html><body><div id=host></div></body></html>");
    b.evaluate(
        "class XD extends HTMLElement { constructor() { super(); } \
             connectedCallback() { log.push('+' + this.id); } disconnectedCallback() { log.push('-' + this.id); } } \
         customElements.define('x-d', XD); \
         var host = document.getElementById('host'); \
         function make(id) { var e = new XD(); e.id = id; return e; } \
         host.appendChild(make('a')); host.appendChild(make('b')); host.appendChild(make('c')); host.appendChild(make('d')); host.appendChild(make('e')); \
         var detached = document.createElement('div'); detached.appendChild(make('never'));",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "+a,+b,+c,+d,+e");
    b.evaluate("log.length = 0; host.removeChild(document.getElementById('a'));").unwrap();
    b.evaluate("document.getElementById('b').remove();").unwrap();
    b.evaluate("document.getElementById('c').replaceWith(document.createElement('span'));").unwrap();
    assert_eq!(ev(&b, "log.join()"), "-a,-b,-c");
    b.evaluate("log.length = 0; host.innerHTML = '';").unwrap();
    assert_eq!(ev(&b, "log.join()"), "-d,-e");
    // A subtree that contains one leaves with it.
    b.evaluate("log.length = 0; var wrap = document.createElement('section'); host.appendChild(wrap); wrap.appendChild(make('deep')); host.removeChild(wrap);").unwrap();
    assert_eq!(ev(&b, "log.join()"), "+deep,-deep");
    b.evaluate("log.length = 0; host.appendChild(make('t')); host.textContent = 'gone';").unwrap();
    assert_eq!(ev(&b, "log.join()"), "+t,-t");
    b.evaluate("log.length = 0; host.appendChild(make('r')); host.replaceChildren();").unwrap();
    assert_eq!(ev(&b, "log.join()"), "+r,-r");
}

#[test]
fn html_insertion_creates_and_connects_defined_elements() {
    let b = bound("<html><body><div id=host></div></body></html>");
    b.evaluate(
        "class XE extends HTMLElement { connectedCallback() { log.push('+' + this.id + ':' + (this instanceof XE)); } } \
         customElements.define('x-e', XE); var host = document.getElementById('host'); \
         host.innerHTML = '<x-e id=one></x-e><b>b</b><x-e id=two></x-e>'; \
         host.insertAdjacentHTML('beforeend', '<x-e id=three></x-e>'); \
         host.insertAdjacentHTML('afterend', '<x-e id=four></x-e>'); \
         var frag = document.createDocumentFragment(); var f = document.createElement('x-e'); f.id = 'five'; frag.appendChild(f); host.append(frag);",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "+one:true,+two:true,+three:true,+four:true,+five:true");
}

#[test]
fn attributes_report_only_the_observed_ones() {
    let b = bound(PAGE);
    b.evaluate(
        "class XF extends HTMLElement { static get observedAttributes() { return ['a', 'b']; } \
             attributeChangedCallback(n, o, v) { log.push(n + ':' + o + '>' + v); } } \
         customElements.define('x-f', XF); var e = new XF(); \
         e.setAttribute('a', '1'); e.setAttribute('a', '2'); e.setAttribute('c', 'ignored'); e.setAttribute('b', 'x'); \
         e.removeAttribute('a'); e.removeAttribute('a'); e.removeAttribute('c'); e.toggleAttribute('b'); e.toggleAttribute('b');",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "a:null>1,a:1>2,b:null>x,a:2>null,b:x>null,b:null>");
}

#[test]
fn a_throwing_or_wrong_constructor_is_reported_and_isolated() {
    let b = bound("<html><body><x-g id=bad></x-g><x-g id=good></x-g></body></html>");
    b.evaluate(
        "var n = 0; class XG extends HTMLElement { constructor() { super(); if (n++ === 0) throw new Error('ctor boom'); log.push('ok:' + this.id); } connectedCallback() { throw new Error('cb boom'); } } \
         customElements.define('x-g', XG);",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "ok:good");
    let errors = b.take_reported_errors();
    assert!(errors.iter().any(|e| e.contains("ctor boom")), "{errors:?}");
    assert!(errors.iter().any(|e| e.contains("cb boom")), "a throwing callback is reported and does not stop others: {errors:?}");
    // A constructor that returns something else cannot take over the element.
    let b = bound("<html><body><x-h id=h></x-h></body></html>");
    b.evaluate("class XH extends HTMLElement { constructor() { super(); return document.createElement('div'); } } customElements.define('x-h', XH);").unwrap();
    assert!(b.take_reported_errors().iter().any(|e| e.contains("InvalidStateError") || e.contains("did not produce")));
}

#[test]
fn html_element_is_only_constructible_through_a_definition() {
    let b = bound(PAGE);
    assert_eq!(thrown(&b, "new HTMLElement();"), "TypeError");
    assert_eq!(thrown(&b, "class Loose extends HTMLElement {} new Loose();"), "TypeError");
    assert_eq!(thrown(&b, "HTMLElement();"), "TypeError");
    assert_eq!(
        ev(&b, "[typeof HTMLElement, HTMLElement.name, document.createElement('div') instanceof HTMLElement, HTMLDivElement.prototype instanceof HTMLElement, document.createElement('div').constructor === HTMLDivElement].join()"),
        "function,HTMLElement,true,true,true"
    );
}

#[test]
fn nothing_changes_until_the_first_define() {
    let b = bound(PAGE);
    assert_eq!(
        ev(&b, "var h = document.getElementById('host'); var d = document.createElement('i'); h.appendChild(d); h.removeChild(d); d.setAttribute('k', 'v'); [h.children.length, d.getAttribute('k'), d.isConnected].join()"),
        "3,v,false"
    );
}

#[test]
fn upgrade_converts_a_detached_subtree_on_demand() {
    let b = bound(PAGE);
    b.evaluate(
        "class XU extends HTMLElement { constructor() { super(); log.push('ctor'); } } \
         var holder = document.createElement('div'); holder.innerHTML = '<x-u></x-u>'; \
         customElements.define('x-u', XU); log.push('defined'); \
         customElements.upgrade(holder); log.push((holder.firstChild instanceof XU) + ''); customElements.upgrade(holder);",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "defined,ctor,true");
}
