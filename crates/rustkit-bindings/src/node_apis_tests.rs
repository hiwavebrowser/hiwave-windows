//! Node API gaps from the 2026-10-03 web API census (rows 5, 11, 13 and
//! 16): `Element.attributes` and `getAttributeNode`, `document.activeElement`
//! with `focus()`/`blur()`, `createElementNS` and the namespaced attribute
//! methods, and `document.createEvent`. Its own test module so it does not
//! collide with the other families' tests in `lib.rs`.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title></head>
<body><div id="main" class="box" data-x="1"><input id="q"><input id="r"><input id="off" disabled><div id="t" tabindex="0"></div></div></body></html>"#;

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
fn attributes_is_a_live_named_node_map_of_attrs() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), a = m.attributes, r = []; \
             r.push(a.length, a instanceof NamedNodeMap, a === m.attributes); \
             var id = a.getNamedItem('ID'); \
             r.push(id instanceof Attr, id.name, id.value, id.localName, id.ownerElement === m, \
                    String(id.namespaceURI), id.nodeType); \
             r.push(String(a.getNamedItem('nope')), String(a.item(99)), String(a[99])); \
             m.setAttribute('title', 't'); r.push(a.length, a.getNamedItem('title').value); \
             var names = []; for (var x of a) names.push(x.name); r.push(names.sort().join('+')); \
             r.push(Array.from(a).length, a.item(0) === a[0], a[0].name === m.getAttributeNames()[0]); \
             id.value = 'main2'; r.push(m.id); m.id = 'main'; \
             m.removeAttribute('title'); r.push(a.length, String(a.getNamedItem('title'))); \
             r.push(m.getAttributeNode('class').value, String(m.getAttributeNode('nope')), \
                    m.getAttributeNode('id') === a.getNamedItem('id')); \
             r.push(document.createElement('i').attributes.length); \
             r.join(',')"
        ),
        "3,true,true,true,id,main,id,true,null,2,null,null,undefined,4,t,\
         class+data-x+id+title,4,true,true,main2,3,null,box,null,true,0"
    );
}

#[test]
fn active_element_follows_focus_and_blur() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = [], log = [], d = document, q = d.getElementById('q'), s = d.getElementById('r'); \
             r.push(d.activeElement === d.body); \
             ['focus', 'blur', 'focusin', 'focusout'].forEach(function (t) { \
                 d.addEventListener(t, function (e) { log.push(t + ':' + e.target.id); }, true); }); \
             q.focus(); r.push(d.activeElement === q); \
             s.focus(); r.push(d.activeElement === s); \
             s.focus(); s.blur(); r.push(d.activeElement === d.body); \
             q.blur(); \
             d.getElementById('main').focus(); r.push(d.activeElement === d.body); \
             d.getElementById('off').focus(); r.push(d.activeElement === d.body); \
             var t = d.getElementById('t'); t.focus(); r.push(d.activeElement === t); \
             r.push(log.join(' ')); \
             q.focus(); q.remove(); r.push(d.activeElement === d.body); \
             r.join('|')"
        ),
        "true|true|true|true|true|true|true|\
         focus:q focusin:q blur:q focusout:q focus:r focusin:r blur:r focusout:r focus:t focusin:t|true"
    );
}

#[test]
fn create_element_ns_builds_svg_and_html_nodes() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var SVG = 'http://www.w3.org/2000/svg', XL = 'http://www.w3.org/1999/xlink', r = []; \
             var s = document.createElementNS(SVG, 'svg'), c = document.createElementNS(SVG, 'circle'); \
             r.push(s.namespaceURI === SVG, s instanceof SVGSVGElement, c instanceof SVGElement, \
                    c instanceof HTMLElement, c.tagName, c.localName); \
             s.setAttribute('viewBox', '0 0 1 1'); r.push(s.getAttribute('viewBox'), s.getAttributeNames().join()); \
             c.setAttributeNS(null, 'cx', '5'); r.push(c.getAttribute('cx'), c.getAttributeNS(null, 'cx'), \
                    String(c.getAttributeNS(null, 'cy'))); \
             var u = document.createElementNS(SVG, 'use'); u.setAttributeNS(XL, 'xlink:href', '#a'); \
             r.push(u.getAttributeNS(XL, 'href'), u.getAttribute('xlink:href'), u.attributes[0].localName); \
             s.appendChild(c); document.body.appendChild(s); \
             r.push(document.body.lastChild === s, s.firstChild === c); \
             var h = document.createElementNS('http://www.w3.org/1999/xhtml', 'div'); \
             r.push(h.namespaceURI, h instanceof HTMLDivElement, h.tagName); \
             r.push(document.createElement('p').namespaceURI, document.getElementById('main').namespaceURI); \
             try { document.createElementNS(SVG, 'a b'); } catch (e) { r.push(e.name); } \
             r.push(String(document.createElementNS(null, 'x').namespaceURI)); \
             r.join('|')"
        ),
        "true|true|true|false|circle|circle|0 0 1 1|viewBox|5|5|null|#a|#a|href|true|true|\
         http://www.w3.org/1999/xhtml|true|DIV|http://www.w3.org/1999/xhtml|\
         http://www.w3.org/1999/xhtml|InvalidCharacterError|null"
    );
}

#[test]
fn create_event_makes_uninitialised_events_for_init_and_dispatch() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = [], got = [], d = document.getElementById('main'); \
             d.addEventListener('ping', function (e) { got.push(e.type, e.bubbles, e.cancelable, e.detail); }); \
             var e = document.createEvent('Event'); \
             r.push(JSON.stringify(e.type), e instanceof Event, e.bubbles, e.isTrusted); \
             try { d.dispatchEvent(e); } catch (x) { r.push(x.name); } \
             e.initEvent('ping', true, false); r.push(d.dispatchEvent(e)); \
             var c = document.createEvent('CustomEvent'); c.initCustomEvent('ping', false, true, 7); \
             d.dispatchEvent(c); \
             r.push(c instanceof CustomEvent, document.createEvent('MouseEvents') instanceof MouseEvent, \
                    document.createEvent('mouseevent') instanceof MouseEvent, \
                    document.createEvent('UIEvent') instanceof UIEvent, \
                    document.createEvent('KeyboardEvent') instanceof KeyboardEvent, \
                    document.createEvent('HTMLEvents').constructor === Event); \
             try { document.createEvent('Nope'); } catch (x) { r.push(x.name); } \
             r.push(got.join(':')); \
             r.join(',')"
        ),
        "\"\",true,false,false,InvalidStateError,true,true,true,true,true,true,true,\
         NotSupportedError,ping:true:false::ping:false:true:7"
    );
}

/// `initEvent` / `initCustomEvent` must not rewrite a live event. Before the
/// `eventPhase !== 0` guard, a listener that re-inited its event changed the
/// type mid-dispatch and broke later listeners on the same path.
#[test]
fn init_event_during_dispatch_does_nothing() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = [], d = document.getElementById('main'); \
             d.addEventListener('keep', function (e) { \
               e.initEvent('mutated', false, true); \
               r.push(e.type, e.bubbles, e.cancelable, e.eventPhase !== 0); \
             }); \
             var e = document.createEvent('Event'); e.initEvent('keep', true, false); \
             d.dispatchEvent(e); \
             d.addEventListener('cust', function (e) { \
               e.initCustomEvent('nope', false, true, 99); \
               r.push(e.type, e.bubbles, e.cancelable, e.detail); \
             }); \
             var c = document.createEvent('CustomEvent'); \
             c.initCustomEvent('cust', true, false, 3); \
             d.dispatchEvent(c); \
             c.initCustomEvent('after', false, true); \
             r.push(c.type, c.cancelable, String(c.detail)); \
             r.join('|')"
        ),
        // During dispatch the inits are ignored; after dispatch they apply,
        // and a missing detail argument becomes null.
        "keep|true|false|true|cust|true|false|3|after|true|null"
    );
}
