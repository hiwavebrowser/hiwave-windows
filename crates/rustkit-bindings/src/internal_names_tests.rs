//! The engine's own names. The embedder's scripts keep what the engine
//! calls on the host object (`__rk`, which only they hold), not on
//! `window`, `document` or a prototype. This test walks what page script
//! can reach after install and lists every property named `__rk...` or
//! `__rustkit...` (and `__net`, the name the bridge's own tests give it):
//! the list is the inventory, and a new name cannot be added to a
//! reachable object without failing it.

use super::*;

/// The names page script can reach, sorted. Each is to move host-side.
const REACHABLE: &[&str] = &[];

pub(super) const PAGE: &str = "<!DOCTYPE html><html><head><title>t</title><style>p{color:red}</style></head><body>\
    <p id=\"p\">text</p><form id=\"f\"><input id=\"i\"><select id=\"s\"><option>a</option></select>\
    <textarea id=\"t\"></textarea><button id=\"b\">b</button></form><img id=\"img\"><canvas id=\"c\"></canvas>\
    <div id=\"host\"></div></body></html>";

/// Walk own properties and prototype chains from the roots a page has
/// (data properties only: no getter is called), three levels of values
/// deep, and collect the internal-looking names with where each was seen.
pub(super) const WALK: &str = r#"(function () {
    var seen = new Set(), found = {};
    function visit(o, depth) {
        if (o === null || (typeof o !== 'object' && typeof o !== 'function') || seen.has(o) || depth > 3) return;
        seen.add(o);
        var names; try { names = Object.getOwnPropertyNames(o); } catch (e) { names = []; }
        names.forEach(function (k) {
            if (/^__(rk|rustkit)/i.test(k) || k === '__net') found[k] = true;
            var d; try { d = Object.getOwnPropertyDescriptor(o, k); } catch (e) { return; }
            if (d && 'value' in d) visit(d.value, depth + 1);
        });
        var proto; try { proto = Object.getPrototypeOf(o); } catch (e) { proto = null; }
        visit(proto, depth);
    }
    var $ = function (id) { return document.getElementById(id); };
    var roots = [window, document, document.documentElement, $('p'), $('p').firstChild, $('f'), $('i'), $('s'),
        $('s').options[0], $('t'), $('b'), $('img'), $('c'), new Event('x'), document.createDocumentFragment(),
        document.createComment('c'), document.createRange && document.createRange(), window.getSelection && window.getSelection(),
        window.getComputedStyle && getComputedStyle($('p')), document.styleSheets, $('p').style, $('p').classList,
        $('p').attributes, $('p').childNodes, window.history, window.location, window.navigator, window.performance,
        window.localStorage, window.console];
    ['XMLHttpRequest', 'Blob', 'FormData', 'Headers', 'URL', 'URLSearchParams', 'AbortController', 'MutationObserver',
     'DOMParser', 'TextEncoder', 'TextDecoder', 'MessageChannel', 'CustomEvent', 'MouseEvent', 'KeyboardEvent',
     'InputEvent', 'FocusEvent', 'DataTransfer', 'Image'].forEach(function (name) {
        try {
            var C = window[name];
            roots.push(name === 'MutationObserver' ? new C(function () {}) : name === 'URL' ? new C('https://a.test/')
                : /Event$/.test(name) ? new C('x') : new C());
        } catch (e) {}
    });
    try { roots.push($('host').attachShadow({ mode: 'open' })); } catch (e) {}
    try { roots.push($('c').getContext('2d')); } catch (e) {}
    try { roots.push(window.matchMedia('(min-width: 1px)')); } catch (e) {}
    try { roots.push(new DOMParser().parseFromString('<p>x</p>', 'text/html')); } catch (e) {}
    // What the engine's own steps leave on a node: a click, a focus, typing.
    try { $('i').focus(); $('b').click(); } catch (e) {}
    roots.forEach(function (r) { visit(r, 0); });
    return Object.keys(found).sort().join(' ');
})()"#;

fn reachable() -> Vec<String> {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.set_location(&Url::parse("https://example.test/page").unwrap()).unwrap();
    b.enable_net_bridge().unwrap();
    match b.evaluate(WALK).unwrap() {
        JsValue::String(names) => names.split(' ').filter(|n| !n.is_empty()).map(str::to_string).collect(),
        other => panic!("the walk answered {other:?}"),
    }
}

#[test]
fn page_script_reaches_none_of_the_engines_names() {
    assert_eq!(reachable(), REACHABLE.iter().map(|n| n.to_string()).collect::<Vec<_>>());
}
