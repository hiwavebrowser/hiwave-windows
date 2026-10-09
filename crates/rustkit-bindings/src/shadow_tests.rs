//! Shadow DOM, the script-visible half (web_shadow.js): attachShadow, the
//! ShadowRoot, slot assignment, and events through a shadow boundary.

use super::*;

const PAGE: &str = "<html><body><div id=host><span id=a slot=x>A</span><b id=b>B</b>tail</div><p id=plain></p><x-widget id=w></x-widget></body></html>";

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.evaluate("var log = []; var host = document.getElementById('host');").unwrap();
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

#[test]
fn attachshadow_makes_a_shadow_root_and_validates() {
    let b = bound();
    assert_eq!(
        ev(&b, "var root = host.attachShadow({ mode: 'open' }); [root instanceof ShadowRoot, root instanceof DocumentFragment, root.nodeType, root.host === host, root.mode, host.shadowRoot === root, root.delegatesFocus, root.slotAssignment].join()"),
        "true,true,11,true,open,true,false,named"
    );
    assert_eq!(thrown(&b, "host.attachShadow({ mode: 'open' });"), "NotSupportedError", "one shadow root per host");
    assert_eq!(thrown(&b, "document.createElement('a').attachShadow({ mode: 'open' });"), "NotSupportedError", "not a valid host");
    assert_eq!(thrown(&b, "document.getElementById('plain').attachShadow({ mode: 'bogus' });"), "TypeError");
    assert_eq!(thrown(&b, "document.getElementById('plain').attachShadow();"), "TypeError");
    // Valid hosts: the listed built-ins, and autonomous custom elements.
    assert_eq!(thrown(&b, "document.getElementById('plain').attachShadow({ mode: 'open' });"), "no error");
    assert_eq!(thrown(&b, "document.getElementById('w').attachShadow({ mode: 'closed' });"), "no error");
    assert_eq!(thrown(&b, "document.createElement('font-face').attachShadow({ mode: 'open' });"), "NotSupportedError");
}

#[test]
fn a_closed_root_is_not_reachable_through_shadowroot() {
    let b = bound();
    assert_eq!(
        ev(&b, "var closed = host.attachShadow({ mode: 'closed' }); [String(host.shadowRoot), closed.mode, closed.host === host].join()"),
        "null,closed,true"
    );
}

#[test]
fn the_shadow_tree_is_separate_from_the_light_tree() {
    let b = bound();
    b.evaluate("var root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<p id=in>inside</p><slot></slot>';").unwrap();
    // The light children are untouched.
    assert_eq!(ev(&b, "host.childNodes.length + ':' + host.children.length + ':' + host.innerHTML"), "3:2:<span id=\"a\" slot=\"x\">A</span><b id=\"b\">B</b>tail");
    // Queries from the document do not pierce; queries on the root do.
    assert_eq!(ev(&b, "[String(document.getElementById('in')), String(document.querySelector('#in')), root.getElementById('in').textContent, root.firstElementChild.id, root.children.length].join()"), "null,null,inside,in,2");
    assert_eq!(ev(&b, "root.innerHTML"), "<p id=\"in\">inside</p><slot></slot>");
    assert_eq!(ev(&b, "root.firstChild.getRootNode() === root"), "true");
    assert_eq!(ev(&b, "root.firstChild.getRootNode({ composed: true }) === document"), "true");
    assert_eq!(ev(&b, "[root.isConnected, root.firstChild.isConnected, host.contains(root.firstChild), document.contains(root.firstChild)].join()"), "true,true,false,false");
}

#[test]
fn a_shadow_node_of_a_detached_host_is_not_connected() {
    let b = bound();
    assert_eq!(
        ev(&b, "var d = document.createElement('div'); var r = d.attachShadow({ mode: 'open' }); r.innerHTML = '<i></i>'; var c1 = r.firstChild.isConnected; document.body.appendChild(d); [c1, r.firstChild.isConnected].join()"),
        "false,true"
    );
}

#[test]
fn slots_get_the_light_children_by_name() {
    let b = bound();
    b.evaluate("var root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<slot name=x id=sx></slot><slot id=sd><i id=fb>fallback</i></slot>';").unwrap();
    assert_eq!(
        ev(&b, "var sx = root.getElementById('sx'), sd = root.getElementById('sd'); [sx.name, sd.name === '', sx.assignedNodes().map(function (n) { return n.id; }).join('+'), sd.assignedNodes().map(function (n) { return n.nodeName; }).join('+'), sd.assignedElements().map(function (n) { return n.id; }).join('+')].join()"),
        "x,true,a,B+#text,b"
    );
    // assignedSlot, through an open root.
    assert_eq!(ev(&b, "[document.getElementById('a').assignedSlot === sx, document.getElementById('b').assignedSlot === sd, host.lastChild.assignedSlot === sd, String(host.assignedSlot)].join()"), "true,true,true,null");
    // No light child for a slot: assignedNodes is empty; flatten gives the fallback.
    b.evaluate("root.innerHTML = '<slot name=none id=sn><i id=fb>fallback</i></slot>';").unwrap();
    assert_eq!(ev(&b, "var sn = root.getElementById('sn'); [sn.assignedNodes().length, sn.assignedNodes({ flatten: true }).map(function (n) { return n.id; }).join()].join()"), "0,fb");
}

#[test]
fn a_nested_slot_flattens_through() {
    let b = bound();
    // host's shadow has <slot id=outer>; an inner widget (in that shadow) slots it again.
    b.evaluate(
        "var outerRoot = host.attachShadow({ mode: 'open' }); outerRoot.innerHTML = '<div id=inner><slot id=so></slot></div>';\
         var inner = outerRoot.getElementById('inner'); var innerRoot = inner.attachShadow({ mode: 'open' }); innerRoot.innerHTML = '<slot id=si></slot>';",
    )
    .unwrap();
    // inner's light child is <slot id=so>, assigned to si; flattening reaches host's light children, except
    // the span: its slot=x names no slot in this shadow tree, so it is assigned to none.
    assert_eq!(ev(&b, "var si = innerRoot.getElementById('si'); si.assignedElements().map(function (n) { return n.id; }).join()"), "so");
    assert_eq!(ev(&b, "si.assignedNodes({ flatten: true }).map(function (n) { return n.nodeName; }).join('+')"), "B+#text");
}

#[test]
fn a_composed_event_leaves_the_shadow_root_and_is_retargeted() {
    let b = bound();
    b.evaluate(
        "var root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button id=btn></button>'; var btn = root.getElementById('btn');\
         host.addEventListener('ping', function (e) { log.push('host:' + e.target.id + ':' + (e.currentTarget === host)); });\
         root.addEventListener('ping', function (e) { log.push('root:' + e.target.id); });\
         btn.addEventListener('ping', function (e) { log.push('btn:' + e.target.id); });\
         document.addEventListener('ping', function (e) { log.push('doc:' + e.target.id); });\
         btn.dispatchEvent(new Event('ping', { bubbles: true, composed: true }));",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "btn:btn,root:btn,host:host:true,doc:host");
    // After dispatch the target is the outermost host.
    assert_eq!(ev(&b, "var e2 = new Event('pong', { bubbles: true, composed: true }); btn.dispatchEvent(e2); e2.target === host"), "true");
}

#[test]
fn a_non_composed_event_stops_at_the_shadow_root() {
    let b = bound();
    b.evaluate(
        "var root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button id=btn></button>'; var btn = root.getElementById('btn');\
         host.addEventListener('ping', function () { log.push('host'); }); root.addEventListener('ping', function () { log.push('root'); });\
         document.addEventListener('ping', function () { log.push('doc'); });\
         btn.dispatchEvent(new Event('ping', { bubbles: true }));",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "root");
    // An event dispatched on the host itself is seen by the host and the document, not the shadow tree.
    b.evaluate("log.length = 0; host.dispatchEvent(new Event('ping', { bubbles: true }));").unwrap();
    assert_eq!(ev(&b, "log.join()"), "host,doc");
}

#[test]
fn composedpath_lists_the_path_through_the_shadow() {
    let b = bound();
    b.evaluate(
        "var root = host.attachShadow({ mode: 'open' }); root.innerHTML = '<button id=btn></button>'; var btn = root.getElementById('btn'); var path;\
         document.addEventListener('ping', function (e) { path = e.composedPath().map(function (n) { return n === window ? 'window' : n === document ? 'document' : n === root ? 'root' : n.nodeName.toLowerCase(); }); });\
         btn.dispatchEvent(new Event('ping', { bubbles: true, composed: true }));",
    )
    .unwrap();
    assert_eq!(ev(&b, "path.join()"), "button,root,div,body,html,document,window");
    assert_eq!(ev(&b, "new Event('x').composedPath().length"), "0", "empty outside dispatch");
}

#[test]
fn nothing_changes_for_a_page_with_no_shadow_root() {
    let b = bound();
    b.evaluate("host.addEventListener('ping', function (e) { log.push(e.target.id); }); document.getElementById('a').dispatchEvent(new Event('ping', { bubbles: true }));").unwrap();
    assert_eq!(ev(&b, "log.join()"), "a");
    assert_eq!(ev(&b, "[host.isConnected, document.getRootNode() === document, host.getRootNode() === document].join()"), "true,true,true");
}

#[test]
fn adopted_style_sheets_and_the_small_properties() {
    let b = bound();
    b.evaluate("var root = host.attachShadow({ mode: 'open', delegatesFocus: true });").unwrap();
    assert_eq!(ev(&b, "[root.delegatesFocus, Array.isArray(root.adoptedStyleSheets), root.adoptedStyleSheets.length, String(root.activeElement), root.clonable, Object.prototype.toString.call(root)].join()"), "true,true,0,null,false,[object ShadowRoot]");
    assert_eq!(thrown(&b, "Object.getOwnPropertyDescriptor(ShadowRoot.prototype, 'mode').get.call({});"), "TypeError");
}

