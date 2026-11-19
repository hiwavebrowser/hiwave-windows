//! What follows an attribute write, pinned to Chrome for Testing 148.0.7778.216 (`/tmp/js1/aw/scen.mjs`, the same
//! script): `on<type>` handlers, custom-element reactions (also after a page patched the built-ins it could reach) and
//! attribute names in an XML document. The write path asks its follow-up steps only about the names they act on
//! (dom.rs `setData`, `setAttribute`, `removeAttribute`) and the reaction hook reads only what was captured at load;
//! this holds every name's answer where it was.
use super::*;

fn ev(script: &str) -> String {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><head></head><body></body></html>").unwrap())).unwrap();
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

const SCRIPT: &str = r#"(function () {
var r = [];
// 2. on<type> handlers
var b = document.createElement('button'), f = function () {};
b.onclick = f; b.removeAttribute('onclick'); var h1 = b.onclick === f;
b.removeAttribute('OnMouseDown'); var h2 = b.onclick === f;
b.setAttribute('ONCLICK', 'void 0'); var h3 = typeof b.onclick + ',' + (b.onclick === f);
var b2 = document.createElement('button'); b2.onclick = f; b2.setAttribute('data-onclick', '1'); b2.setAttribute('xonclick', '1'); var h4 = b2.onclick === f;
b2.setAttribute('onclick', 'void 0'); var h5 = b2.onclick === f;
r.push('handlers:' + [h1, h2, h3, h4, h5].join(','));
// 3. custom element reactions
var log = [];
class Q extends HTMLElement {
  static get observedAttributes() { return ['class', 'id', 'data-a', 'title', 'TITLE', '__proto__', 'srcset', 'onclick', 'async']; }
  attributeChangedCallback(n, o, v) { log.push(n + ':' + o + '>' + v); }
}
customElements.define('x-q', Q);
var q = document.createElement('x-q'); document.body.appendChild(q);
q.setAttribute('title', 'a'); q.setAttribute('TITLE', 'b'); q.setAttribute('Data-A', '1'); q.removeAttribute('DATA-A'); q.removeAttribute('data-a');
q.setAttribute('__proto__', 'p'); q.setAttribute('srcset', 's'); q.setAttribute('onclick', 'void 0'); q.setAttribute('OnClick', 'void 1'); q.setAttribute('async', ''); q.setAttribute('ASYNC', '2');
q.setAttribute('unwatched', 'u'); q.id = 'i'; q.className = 'c'; q.classList.add('d'); q.toggleAttribute('title'); q.toggleAttribute('title');
q.removeAttribute('nope'); q.removeAttribute('onclick');
r.push('ce:' + log.join(' '));
// 4. a page that patched the built-ins it could reach, after its elements exist
log.length = 0;
var q2 = document.createElement('x-q'); document.body.appendChild(q2);
var save = [String.prototype.toLowerCase, String.prototype.charCodeAt, Array.prototype.indexOf, WeakMap.prototype.get];
String.prototype.toLowerCase = function () { return 'zzz'; };
String.prototype.charCodeAt = function () { return 0; };
Array.prototype.indexOf = function () { return -1; };
WeakMap.prototype.get = function () { return undefined; };
try {
  q2.setAttribute('title', 'a'); q2.setAttribute('TITLE', 'b'); q2.id = 'i2'; q2.className = 'c2'; q2.removeAttribute('title');
  q2.setAttribute('srcset', 's'); q2.setAttribute('onclick', 'void 0'); q2.setAttribute('Data-A', '3');
} finally {
  String.prototype.toLowerCase = save[0]; String.prototype.charCodeAt = save[1]; Array.prototype.indexOf = save[2]; WeakMap.prototype.get = save[3];
}
r.push('patched:' + log.join(' '));
return r.join('\n');
})()"#;

#[test]
fn what_follows_an_attribute_write_answers_as_chrome_148_does() {
    assert_eq!(ev(SCRIPT), "handlers:true,true,function,false,true,false\nce:title:null>a title:a>b data-a:null>1 data-a:1>null __proto__:null>p srcset:null>s onclick:null>void 0 onclick:void 0>void 1 async:null> async:>2 id:null>i class:null>c class:c>c d title:b>null title:null> onclick:void 1>null\npatched:title:null>a title:a>b id:null>i2 class:null>c2 title:b>null srcset:null>s onclick:null>void 0 data-a:null>3");
}

const XML: &str = r#"(function () {
var XHTML = 'http://www.w3.org/1999/xhtml';
var xd = document.implementation.createDocument(XHTML, 'html', null);
var xe = xd.createElementNS(XHTML, 'div');
xe.setAttribute('TITLE', 'x'); xe.setAttribute('onClick', '1'); xe.setAttribute('class', 'k'); xe.id = 'zz';
return 'xml:' + xe.getAttributeNames().join(',') + '|' + xe.getAttribute('TITLE') + '|' + xe.getAttribute('title') + '|' + (xe.onclick === undefined ? 'undef' : typeof xe.onclick);
})()"#;

/// PINS A GAP. In an XML document an element keeps the case of the names written to it (Chrome: `xml:TITLE,onClick,class,id|x|null|object`); the engine's
/// native attribute write lowercases the name of an HTML-namespace element whatever the document is (rustkit-dom), so
/// `TITLE` is stored as `title` and `onClick` compiles an `onclick` handler. The reaction hook and the steps after a
/// write lowercase the same way the native does, so they agree with it; when the native is fixed this test fails and
/// both follow it (new row XML-ATTR-CASE).
#[test]
fn xml_document_attribute_names_keep_their_case_known_divergence() {
    assert_eq!(ev(XML), "xml:title,onclick,class,id|x|x|function");
}
