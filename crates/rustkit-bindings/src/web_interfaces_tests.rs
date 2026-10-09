//! Standard interface objects (dom.rs element interfaces and
//! web_interfaces.js). Its own test module so it does not collide with the
//! other families' tests in `lib.rs`.

use super::*;

const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title></head>
<body><div id="d"><a id="a" href="/x">l</a><img id="im"><button id="b">b</button><select id="s"><option id="o">1</option></select>
<h2 id="h">h</h2><table id="t"><thead id="th"><tr id="tr"><td id="td">c</td></tr></thead></table>
<audio id="au"></audio><video id="vi"></video><input id="in"><x-foo id="xf"></x-foo><span id="sp"></span><svg id="sv"></svg></div></body></html>"#;

fn bound() -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    bindings
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => {
            if n.fract() == 0.0 { format!("{}", n as i64) } else { n.to_string() }
        }
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

/// `el instanceof HTMLAnchorElement` and friends work on real nodes.
#[test]
fn elements_are_instances_of_their_tags_interface() {
    let b = bound();
    let is = |id: &str, iface: &str| ev(&b, &format!("String(document.getElementById('{id}') instanceof {iface})"));
    for (id, iface) in [
        ("a", "HTMLAnchorElement"), ("im", "HTMLImageElement"), ("b", "HTMLButtonElement"), ("s", "HTMLSelectElement"),
        ("o", "HTMLOptionElement"), ("h", "HTMLHeadingElement"), ("t", "HTMLTableElement"), ("th", "HTMLTableSectionElement"),
        ("tr", "HTMLTableRowElement"), ("td", "HTMLTableCellElement"), ("d", "HTMLDivElement"), ("sp", "HTMLSpanElement"),
        ("au", "HTMLAudioElement"), ("vi", "HTMLVideoElement"), ("au", "HTMLMediaElement"), ("vi", "HTMLMediaElement"),
        ("in", "HTMLInputElement"), ("sv", "SVGSVGElement"), ("sv", "SVGElement"), ("sv", "Element"),
    ] {
        assert_eq!(is(id, iface), "true", "#{id} instanceof {iface}");
    }
    // Every HTML element is also an HTMLElement, Element and Node.
    assert_eq!(ev(&b, "var a = document.getElementById('a'); String([a instanceof HTMLElement, a instanceof Element, a instanceof Node, a instanceof EventTarget].join())"), "true,true,true,true");
    // And not of the wrong one.
    assert_eq!(ev(&b, "String([a instanceof HTMLImageElement, document.getElementById('d') instanceof HTMLAnchorElement, document.getElementById('au') instanceof HTMLVideoElement].join())"), "false,false,false");
    // Custom elements and unmapped tags stay plain HTMLElement.
    assert_eq!(ev(&b, "var xf = document.getElementById('xf'); String([xf instanceof HTMLElement, xf instanceof HTMLUnknownElement, xf instanceof HTMLAnchorElement].join())"), "true,false,false");
    assert_eq!(ev(&b, "Object.prototype.toString.call(a) + Object.prototype.toString.call(document.getElementById('h'))"), "[object HTMLAnchorElement][object HTMLHeadingElement]");
    // The interface objects are not constructible.
    assert_eq!(ev(&b, "var t1; try { new HTMLAnchorElement(); t1 = 'no throw'; } catch (e) { t1 = e.name + ':' + e.message; } t1"), "TypeError:Illegal constructor");
}

#[test]
fn event_subclasses_extend_event_with_their_init_members() {
    let b = bound();
    assert_eq!(
        ev(&b, "var k = new KeyboardEvent('keydown', { key: 'a', code: 'KeyA', ctrlKey: true, bubbles: true, keyCode: 65 }); \
                [k.type, k.key, k.code, k.ctrlKey, k.shiftKey, k.repeat, k.keyCode, k.bubbles, k instanceof UIEvent, k instanceof Event, k.getModifierState('Control')].join('|')"),
        "keydown|a|KeyA|true|false|false|65|true|true|true|true"
    );
    assert_eq!(
        ev(&b, "var m = new MouseEvent('click', { clientX: 10, clientY: 20, button: 2, relatedTarget: document.body }); \
                [m.clientX, m.clientY, m.x, m.pageY, m.offsetX, m.button, m.buttons, m.detail, m.relatedTarget === document.body, m instanceof UIEvent].join('|')"),
        "10|20|10|20|10|2|0|0|true|true"
    );
    // Defaults, with a fresh array per instance.
    assert_eq!(ev(&b, "var m1 = new MessageEvent('message', { data: { a: 1 }, origin: 'https://x.test' }); var m2 = new MessageEvent('message'); m1.ports.push(1); [m1.data.a, m1.origin, String(m2.data), m2.ports.length, m1.lastEventId === ''].join('|')"), "1|https://x.test|null|0|true");
    assert_eq!(ev(&b, "var e = new ErrorEvent('error', { message: 'boom', lineno: 7 }); [e.message, e.lineno, e.colno, String(e.error)].join('|')"), "boom|7|0|null");
    assert_eq!(ev(&b, "var p = new ProgressEvent('progress', { lengthComputable: true, loaded: 5, total: 10 }); [p.lengthComputable, p.loaded, p.total].join()"), "true,5,10");
    assert_eq!(ev(&b, "var w = new WheelEvent('wheel', { deltaY: 100 }); [w.deltaY, w.deltaX, w instanceof MouseEvent].join()"), "100,0,true");
    assert_eq!(ev(&b, "[new FocusEvent('focus') instanceof UIEvent, new PopStateEvent('popstate', { state: 3 }).state, new HashChangeEvent('hashchange', { newURL: 'u' }).newURL, new PageTransitionEvent('pageshow', { persisted: true }).persisted].join()"), "true,3,u,true");
    // They dispatch through the real event system.
    assert_eq!(
        ev(&b, "var got = []; document.getElementById('b').addEventListener('click', function (e) { got.push(e.constructor === MouseEvent, e.clientX, e instanceof Event, e.isTrusted); }); \
                document.getElementById('b').dispatchEvent(new MouseEvent('click', { clientX: 5, bubbles: true })); got.join()"),
        "true,5,true,false"
    );
    // `new` and an argument are required.
    assert_eq!(ev(&b, "var n1; try { MouseEvent('x'); n1 = 'no throw'; } catch (e) { n1 = e.name; } n1"), "TypeError");
    assert_eq!(ev(&b, "var n2; try { new KeyboardEvent(); n2 = 'no throw'; } catch (e) { n2 = e.name; } n2"), "TypeError");
    assert_eq!(ev(&b, "Object.prototype.toString.call(k)"), "[object KeyboardEvent]");
}

#[test]
fn geometry_types_compute_edges_and_serialise() {
    let b = bound();
    assert_eq!(ev(&b, "var r = new DOMRect(10, 20, 30, 40); [r.x, r.y, r.width, r.height, r.top, r.right, r.bottom, r.left].join()"), "10,20,30,40,20,40,60,10");
    // Negative sizes: the edges are the min and max.
    assert_eq!(ev(&b, "var n = new DOMRect(10, 20, -4, -6); [n.top, n.bottom, n.left, n.right].join()"), "14,20,6,10");
    assert_eq!(ev(&b, "JSON.stringify(DOMRect.fromRect({ x: 1, y: 2, width: 3, height: 4 }))"), r#"{"x":1,"y":2,"width":3,"height":4,"top":2,"right":4,"bottom":6,"left":1}"#);
    assert_eq!(ev(&b, "[r instanceof DOMRectReadOnly, new DOMRectReadOnly() instanceof DOMRect, new DOMRect().width].join()"), "true,false,0");
    assert_eq!(ev(&b, "var pt = new DOMPoint(1, 2); [pt.x, pt.y, pt.z, pt.w, pt instanceof DOMPointReadOnly, JSON.stringify(DOMPoint.fromPoint({ x: 5 }))].join('|')"), r#"1|2|0|1|true|{"x":5,"y":0,"z":0,"w":1}"#);
}

#[test]
fn existing_singletons_get_their_interfaces_and_the_rest_are_interface_only() {
    let b = bound();
    assert_eq!(
        ev(&b, "[navigator instanceof Navigator, history instanceof History, location instanceof Location, screen instanceof Screen, \
                performance instanceof Performance, localStorage instanceof Storage, sessionStorage instanceof Storage, \
                window instanceof Window, ({}) instanceof Window, matchMedia('(min-width: 1px)') instanceof MediaQueryList].join()"),
        "true,true,true,true,true,true,true,true,false,true"
    );
    // matchMedia results still work, and gain addListener.
    assert_eq!(ev(&b, "var q = matchMedia('(min-width: 1px)'); [typeof q.addListener, typeof q.removeListener, q.media].join()"), "function,function,(min-width: 1px)");
    // Interface-only objects exist, are not constructible, and chain correctly.
    assert_eq!(ev(&b, "[typeof ShadowRoot, typeof Attr, typeof NamedNodeMap, typeof CSSStyleSheet, typeof Range, typeof Selection, typeof FileList].join()"), "function,function,function,function,function,function,function");
    assert_eq!(ev(&b, "[ShadowRoot.prototype instanceof DocumentFragment, CSSStyleSheet.prototype instanceof StyleSheet, Attr.prototype instanceof Node].join()"), "true,true,true");
    assert_eq!(ev(&b, "var t; try { new ShadowRoot(); t = 'no throw'; } catch (e) { t = e.name; } t"), "TypeError");
    assert_eq!(ev(&b, "var w; try { new Window(); w = 'no throw'; } catch (e) { w = e.name; } w"), "TypeError");
    // Constructors for features the engine does not have stay UNDEFINED, so
    // `typeof Worker` style feature detection keeps working.
    assert_eq!(
        ev(&b, "[typeof Worker, typeof WebAssembly, typeof Notification, typeof AudioContext, typeof OffscreenCanvas, typeof BroadcastChannel].join()"),
        "undefined,undefined,undefined,undefined,undefined,undefined"
    );
}

#[test]
fn cdatasection_and_processinginstruction_interfaces_exist() {
    let b = bound();
    assert_eq!(
        ev(&b, "[typeof CDATASection, typeof ProcessingInstruction].join()"),
        "function,function"
    );
    assert_eq!(
        ev(&b, "[CDATASection.prototype instanceof Text, CDATASection.prototype instanceof CharacterData, CDATASection.prototype instanceof Node].join()"),
        "true,true,true"
    );
    assert_eq!(
        ev(&b, "[ProcessingInstruction.prototype instanceof CharacterData, ProcessingInstruction.prototype instanceof Node].join()"),
        "true,true"
    );
    assert_eq!(
        ev(&b, "var t1; try { new CDATASection(); t1 = 'no throw'; } catch (e) { t1 = e.name; } t1"),
        "TypeError"
    );
    assert_eq!(
        ev(&b, "var t2; try { new ProcessingInstruction(); t2 = 'no throw'; } catch (e) { t2 = e.name; } t2"),
        "TypeError"
    );
    // YouTube webcomponents-sd Tag 878 iteration
    assert_eq!(
        ev(&b, "['Document','DocumentFragment','Element','Text','Comment','CDATASection','ProcessingInstruction'].every(function(a){ return typeof Object.create(window[a].prototype) === 'object'; })"),
        "true"
    );
}

#[test]
fn window_inherits_from_window_prototype_and_event_target_prototype() {
    let b = bound();
    // 1. Object.getPrototypeOf(window) === Window.prototype
    assert_eq!(
        ev(&b, "String(Object.getPrototypeOf(window) === Window.prototype)"),
        "true"
    );
    // 2. window instanceof Window
    assert_eq!(
        ev(&b, "String(window instanceof Window)"),
        "true"
    );
    // 3. window instanceof EventTarget
    assert_eq!(
        ev(&b, "String(window instanceof EventTarget)"),
        "true"
    );
    // 4. a property defined on EventTarget.prototype is visible on window
    assert_eq!(
        ev(&b, "EventTarget.prototype.__custom_test_prop = 'from_event_target'; window.__custom_test_prop"),
        "from_event_target"
    );
    // 5. webcomponents-sd __shady_native_addEventListener pattern works on window
    assert_eq!(
        ev(&b, "var called = false; EventTarget.prototype.__shady_native_addEventListener = function() { called = true; }; window.__shady_native_addEventListener('test', function(){}, true); String(called)"),
        "true"
    );
}

