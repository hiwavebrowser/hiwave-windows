//! DOM utility completeness: the everyday Element/Node helpers pages call
//! (closest/matches, classList, dataset, insertAdjacent*, ChildNode and
//! ParentNode, contains/compareDocumentPosition/isConnected,
//! template.content, DOMParser, CustomEvent, attribute helpers). Its own
//! test module so it does not collide with the other families' tests in
//! `lib.rs`.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title></head>
<body><div id="main" class="box wide" data-user-id="42" data-x="1"><p id="p1" class="x">Hello, <b id="w">world</b>!</p><p id="p2" class="x">Two</p></div>
<template id="tpl"><li class="row">A</li><li class="row">B</li></template>
<p id="outside">Out</p></body></html>"#;

fn bound() -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(Document::parse_html(PAGE).unwrap()))
        .unwrap();
    // The stand-in matcher: a tag name, `.class` or `#id`; "!" is invalid.
    bindings.set_selector_matcher(Rc::new(|node, sel, _| {
        if sel == "!" {
            return None;
        }
        Some(sel.split(',').map(str::trim).any(|s| {
            if let Some(c) = s.strip_prefix('.') {
                node.get_attribute("class")
                    .is_some_and(|v| v.split_whitespace().any(|t| t == c))
            } else if let Some(i) = s.strip_prefix('#') {
                node.get_attribute("id") == Some(i)
            } else {
                node.tag_name() == Some(s)
            }
        }))
    }));
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
fn closest_and_matches_walk_ancestors_and_reject_bad_selectors() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var w = document.getElementById('w'), r = []; \
             r.push(w.closest('.x').id, w.closest('#main, table').id, w.closest('b') === w, \
                    String(w.closest('table')), w.matches('.x, b'), w.matches('p')); \
             var d = document.createElement('div'); d.innerHTML = '<span><i>x</i></span>'; \
             r.push(d.querySelector('i').closest('div') === d); \
             try { w.closest('!'); } catch (e) { r.push(e.name, e instanceof DOMException); } \
             try { w.matches('!'); } catch (e) { r.push(e.name); } \
             r.join(',')"
        ),
        "p1,main,true,null,true,false,true,SyntaxError,true,SyntaxError"
    );
}

#[test]
fn class_list_takes_many_tokens_and_iterates() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), c = m.classList, r = []; \
             c.add('a', 'b'); c.remove('box', 'a', 'nope'); r.push(m.className); \
             r.push(c.toggle('wide', false), c.toggle('z', false), c.toggle('b', true), m.className); \
             r.push(c.replace('nope', 'q'), c.replace('b', 'q'), m.className, c.contains('q'), c.contains('')); \
             r.push(c[0], c.length, Array.from(c).join('+'), [].slice.call(c).length); \
             var o = document.getElementById('outside'); o.classList.remove('x'); \
             r.push(o.hasAttribute('class')); \
             r.join('|')"
        ),
        "wide b|false|false|true|b|false|true|q|true|false|q|1|q|1|false"
    );
}

#[test]
fn dataset_enumerates_its_data_attributes() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), d = m.dataset, r = []; \
             r.push(d.userId, Object.keys(d).join('+'), JSON.stringify(d)); \
             var seen = []; for (var k in d) seen.push(k); r.push(seen.join('+')); \
             d.newThing = 'y'; r.push(m.getAttribute('data-new-thing'), Object.keys(d).length); \
             r.join('|')"
        ),
        "42|userId+x|{\"userId\":\"42\",\"x\":\"1\"}|userId+x|y|3"
    );
}

#[test]
fn insert_adjacent_variants_place_and_validate() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var p = document.getElementById('p2'), r = []; \
             p.insertAdjacentHTML('afterend', '<em id=e>e</em>'); \
             p.insertAdjacentText('beforebegin', 'T'); \
             var s = document.createElement('s'); \
             r.push(p.insertAdjacentElement('afterbegin', s) === s, p.firstChild === s, \
                    p.nextSibling.id, p.previousSibling.data); \
             var lone = document.createElement('div'); \
             r.push(String(lone.insertAdjacentElement('beforebegin', s))); \
             try { lone.insertAdjacentHTML('afterend', '<i></i>'); } catch (e) { r.push(e.name); } \
             try { p.insertAdjacentHTML('middle', 'x'); } catch (e) { r.push(e.name); } \
             try { p.insertAdjacentElement('afterend', 'x'); } catch (e) { r.push(e.name); } \
             lone.insertAdjacentHTML('beforeend', '<b>1</b>2'); r.push(lone.innerHTML); \
             r.join('|')"
        ),
        "true|true|e|T|null|NoModificationAllowedError|SyntaxError|TypeError|<b>1</b>2"
    );
}

#[test]
fn child_node_and_parent_node_helpers_take_nodes_and_strings() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), p1 = document.getElementById('p1'), \
                 p2 = document.getElementById('p2'), r = []; \
             p1.before('<b>', document.createElement('hr')); \
             p2.after('end'); m.prepend('start'); m.append('tail', document.createElement('br')); \
             r.push(m.firstChild.data, m.childNodes[1].data, m.childNodes[2].nodeName, \
                    m.lastChild.nodeName, m.lastChild.previousSibling.data); \
             p2.replaceWith('two'); r.push(String(p2.parentNode), p2.isConnected); \
             p1.remove(); p1.remove(); r.push(String(p1.parentNode), m.textContent); \
             r.join('|')"
        ),
        "start|<b>|HR|BR|tail|null|false|null|start<b>twoendtail"
    );
}

#[test]
fn contains_compare_document_position_and_is_connected() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), w = document.getElementById('w'), \
                 o = document.getElementById('outside'), x = document.createElement('x'), r = []; \
             r.push(m.contains(w), w.contains(m), m.contains(m), m.contains(null), \
                    document.contains(w), document.contains(x), \
                    m.compareDocumentPosition(w), w.compareDocumentPosition(m), \
                    m.compareDocumentPosition(o), o.compareDocumentPosition(m), \
                    m.compareDocumentPosition(m), \
                    (m.compareDocumentPosition(x) & 1) === 1, \
                    (m.compareDocumentPosition(x) & 32) === 32, \
                    Node.DOCUMENT_POSITION_CONTAINED_BY, w.isConnected, x.isConnected); \
             m.appendChild(x); r.push(x.isConnected, m.compareDocumentPosition(x)); \
             var f = document.createDocumentFragment(); f.appendChild(x); r.push(x.isConnected); \
             r.join(',')"
        ),
        "true,false,true,false,true,false,20,10,4,2,0,true,true,16,true,false,true,20,false"
    );
}

#[test]
fn template_content_is_a_fragment_holding_the_parsed_children() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var t = document.getElementById('tpl'), c = t.content, r = []; \
             r.push(c instanceof DocumentFragment, c === t.content, t.childNodes.length, \
                    c.childNodes.length, c.firstChild.textContent, \
                    document.querySelectorAll('.row').length, c.querySelectorAll('.row').length, \
                    t instanceof HTMLTemplateElement); \
             var copy = c.cloneNode(true); var ul = document.createElement('ul'); \
             ul.appendChild(copy); r.push(ul.children.length, c.childNodes.length); \
             var u2 = document.createElement('ul'); u2.appendChild(document.importNode(c, true)); \
             r.push(u2.children.length, c.childNodes.length); \
             r.push(t.innerHTML); \
             var n = document.createElement('template'); n.innerHTML = '<td>x</td><span>y</span>'; \
             r.push(n.childNodes.length, n.content.childNodes.length, n.content.lastChild.tagName); \
             r.join('|')"
        ),
        "true|true|0|2|A|0|2|true|2|2|2|2|<li class=\"row\">A</li><li class=\"row\">B</li>|0|2|SPAN"
    );
    // Clones carry their content; nested templates get content of their
    // own; serialization reads the content; script-appended children stay
    // real children.
    assert_eq!(
        ev(
            &b,
            "var t = document.getElementById('tpl'), r = []; \
             var deep = t.cloneNode(true), shallow = t.cloneNode(false); \
             r.push(deep.content !== t.content, deep.content.childNodes.length, \
                    shallow.content.childNodes.length, deep.outerHTML === t.outerHTML); \
             var d = document.createElement('div'); \
             d.innerHTML = '<template id=o><p>x</p><template><i>in</i></template></template>'; \
             var o = d.firstChild, inner = o.content.lastChild; \
             r.push(o.childNodes.length, o.content.childNodes.length, inner.childNodes.length, \
                    inner.content.firstChild.tagName, d.innerHTML); \
             var e = document.createElement('template'); e.appendChild(document.createElement('b')); \
             r.push(e.childNodes.length, e.content.childNodes.length); \
             r.join('|')"
        ),
        "true|2|0|true|0|2|0|I|<template id=\"o\"><p>x</p><template><i>in</i></template></template>|1|0"
    );
}

#[test]
fn dom_parser_parses_html_into_its_own_document() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var d = new DOMParser().parseFromString( \
                 '<!DOCTYPE html><title>Hi</title><p id=q class=k>a &amp; b</p>', 'text/html'), r = []; \
             r.push(d instanceof Document, d !== document, d.nodeType, d.title, \
                    d.body.firstChild.textContent, d.getElementById('q').className, \
                    d.querySelector('.k').id, String(document.getElementById('q')), \
                    d.documentElement.tagName, d.head.firstChild.tagName, d.body.isConnected); \
             r.push(new DOMParser().parseFromString('<p>&lt;b&gt; &#39;</p>', 'text/html').body.textContent, \
                    new DOMParser().parseFromString('', 'text/html').body.tagName); \
             var moved = document.importNode(d.getElementById('q'), true); \
             document.body.appendChild(moved); r.push(document.getElementById('q') === moved); \
             try { new DOMParser().parseFromString('x', 'text/plain'); } catch (e) { r.push(e.name); } \
             r.join('|')"
        ),
        "true|true|9|Hi|a & b|k|q|null|HTML|TITLE|true|<b> '|BODY|true|TypeError"
    );
}

#[test]
fn custom_event_carries_detail_to_listeners() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), r = []; \
             document.body.addEventListener('ping', function (e) { \
                 r.push(e.detail.n, e.target === m, e instanceof CustomEvent, e instanceof Event); \
             }); \
             r.push(m.dispatchEvent(new CustomEvent('ping', { detail: { n: 7 }, bubbles: true }))); \
             var e = new CustomEvent('x'); r.push(String(e.detail), e.type, e.bubbles); \
             r.push(typeof e.initCustomEvent); \
             r.join(',')"
        ),
        "7,true,true,true,true,null,x,false,function"
    );
}

#[test]
fn attribute_names_toggle_and_has_attributes() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var m = document.getElementById('main'), x = document.createElement('x'), r = []; \
             r.push(m.getAttributeNames().sort().join(' '), x.getAttributeNames().length, \
                    Array.isArray(x.getAttributeNames()), x.hasAttributes(), m.hasAttributes()); \
             r.push(x.toggleAttribute('hidden'), x.hasAttribute('hidden'), x.getAttribute('hidden'), \
                    x.toggleAttribute('hidden'), x.hasAttribute('hidden'), \
                    x.toggleAttribute('open', true), x.toggleAttribute('open', true), \
                    x.toggleAttribute('closed', false), x.hasAttributes()); \
             try { x.toggleAttribute('a b'); } catch (e) { r.push(e.name); } \
             r.join(',')"
        ),
        "class data-user-id data-x id,0,true,false,true,true,true,,false,false,true,true,false,true,InvalidCharacterError"
    );
}
