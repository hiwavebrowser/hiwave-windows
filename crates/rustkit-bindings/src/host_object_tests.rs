//! The engine reads only own properties of its own objects: a
//! page-defined accessor on a built-in prototype is never invoked by
//! engine code, whatever the engine does next (load, timers, input,
//! focus, the network bridge, canvas, Intl), and the page holds no object
//! it did not create or was not given.

use super::*;

const PAGE: &str = "<!DOCTYPE html><html><head><title>t</title></head><body>\
    <p id=\"p\">text</p><form id=\"f\"><input id=\"i\" value=\"v\"><textarea id=\"t\"></textarea>\
    <button id=\"b\" type=\"button\">b</button></form><a id=\"a\" href=\"#frag\">a</a><img id=\"img\">\
    <canvas id=\"c\"></canvas><div id=\"frag\" draggable=\"true\">d</div></body></html>";

/// Member names of the engine's registries that its scripts fill in later.
const REGISTRY_NAMES: &str = "hasListener loadSnapshot loadReplay hostOf mutation retarget scripts shadowParent windowNamed \
    request take deliver shelve unshelve fail_all waiting queued begin prepare settle";

/// The names this crate's scripts use on the engine's own object, read
/// from the sources, and the registries' member names.
fn engine_names() -> String {
    let mut names = std::collections::BTreeSet::new();
    let mut stack = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                stack.push(path);
            } else if !name.ends_with("_tests.rs") && (name.ends_with(".rs") || name.ends_with(".js")) {
                let source = std::fs::read_to_string(&path).unwrap();
                for part in source.split("__rk.").skip(1) {
                    let member: String =
                        part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect();
                    if !member.is_empty() {
                        names.insert(member);
                    }
                }
            }
        }
    }
    names.extend(REGISTRY_NAMES.split_whitespace().map(str::to_string));
    names.extend(shared_names());
    names.into_iter().collect::<Vec<_>>().join(" ")
}

/// The names this crate's scripts use on the frame tree's shared state
/// (one object for every document of the tree), read from the sources.
fn shared_names() -> std::collections::BTreeSet<String> {
    let mut names: std::collections::BTreeSet<String> =
        "handoff timerQueue".split_whitespace().map(str::to_string).collect();
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for file in ["lib.rs", "dom.rs"] {
        let source = std::fs::read_to_string(dir.join(file)).unwrap();
        for marker in ["SHARED.", "__rk.shared."] {
            for part in source.split(marker).skip(1) {
                let member: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect();
                if !member.is_empty() {
                    names.insert(member);
                }
            }
        }
    }
    names
}

/// Member names of what the engine keeps for a frame tree: the timer
/// queue's, a frame record's, the registries' frame members. (Not
/// `loaded`: it is also a ProgressEvent's, set on the page's own event.)
const FRAME_NAMES: &str = "now nextId timers count pumps pumpCount up top el win tree changed scan attach size nodeId byTag \
    det reads seenNow skew lastPerf startedAt frameTs frameTsAt prevFrameTs prevFrameAt";

/// The names the engine describes the events of the user's input, of
/// transitions and of animations with (the fields it writes on what it
/// hands `make`).
const INIT_NAMES: &str = "bubbles cancelable composed view detail relatedTarget related dataTransfer pointerId \
    pointerType isPrimary pressure deltaX deltaY deltaZ deltaMode wheelDeltaX wheelDeltaY key code repeat keyCode \
    charCode clientX clientY screenX screenY offsetX offsetY movementX movementY pageX pageY which button buttons \
    ctrlKey altKey shiftKey metaKey propertyName animationName elapsedTime pseudoElement data inputType";

/// The page: an accessor on `Object.prototype` for every name, counting
/// its calls. `mine` holds what the page made or was given; a receiver
/// that is not in it, not a node, an event or a function, is counted as
/// not the page's. A set goes on to define the own property, as a plain
/// assignment would have.
const TRAP: &str = r#"
var calls = 0, notMine = 0, which = {}, inTrap = false;
var mine = new WeakSet([window, document, Object.prototype]);
var others = [];
// Another window of the frame tree, which the page was given.
function adopt(w) { mine.add(w); mine.add(w.document); mine.add(w.Object.prototype); others.push(w); }
function given(o) {
    for (var i = 0; i < others.length; i++) if (o instanceof others[i].Node || o instanceof others[i].Event) return true;
    return false;
}
var defineProperty = Object.defineProperty;
function note(name, kind, o) {
    if (inTrap) return;
    inTrap = true;
    calls++;
    var page = o === null || (typeof o !== 'object') || mine.has(o) || o instanceof Node || o instanceof Event || given(o);
    if (!page) notMine++;
    which[name + ':' + kind + (page ? '' : ':not-the-page\'s')] = true;
    inTrap = false;
}
function trap(name) {
    defineProperty(Object.prototype, name, {
        configurable: true,
        get: function () { note(name, 'get', this); return undefined; },
        set: function (v) {
            note(name, 'set', this);
            defineProperty(this, name, { value: v, writable: true, enumerable: true, configurable: true });
        }
    });
}
NAMES.split(/\s+/).forEach(trap);
"#;

/// What the page does itself, through the public surface.
const PAGE_USES: &str = r#"
(function () {
    var $ = function (id) { return document.getElementById(id); };
    function attempt(f) { try { f(); } catch (e) {} }
    attempt(function () { document.hasFocus(); });
    attempt(function () { $('i').focus(); $('i').blur(); $('b').click(); $('a').click(); });
    attempt(function () { $('img').src = 'x.png'; $('img').decode && $('img').decode()['catch'](function () {}); });
    attempt(function () { document.adoptNode(document.createElement('p')); document.importNode($('p'), true); });
    attempt(function () { setTimeout(function () {}, 0); requestAnimationFrame(function () {}); });
    attempt(function () { new MutationObserver(function () {}).observe(document.body, { childList: true }); document.body.appendChild(document.createElement('i')); });
    attempt(function () { var s = document.createElement('script'); s.text = 'void 0'; document.body.appendChild(s); });
    attempt(function () { $('p').attachShadow({ mode: 'open' }).innerHTML = '<b>x</b>'; });
    attempt(function () { window.postMessage('m', '*'); var c = new MessageChannel(); c.port1.onmessage = function () {}; c.port2.postMessage(1); });
    attempt(function () { history.pushState({}, '', '#x'); location.hash = '#frag'; });
    attempt(function () { window.scrollTo(0, 10); $('frag').scrollIntoView(); });
    attempt(function () { window.matchMedia('(min-width: 1px)').addListener(function () {}); });
    attempt(function () { var x = $('c').getContext('2d'); x.fillRect(0, 0, 1, 1); x.measureText('a'); });
    attempt(function () { new Intl.DateTimeFormat('en').format(new Date(0)); new Intl.NumberFormat('en').format(1); });
    attempt(function () { console.log('x', {}); });
    attempt(function () { var x = new XMLHttpRequest(); x.open('GET', 'https://example.test/a'); x.send(); });
    attempt(function () { fetch('https://example.test/b')['catch'](function () {}); });
    attempt(function () { btoa('a'); atob('YQ=='); new TextEncoder().encode('a'); new TextDecoder().decode(new Uint8Array(1)); });
    attempt(function () { URL.revokeObjectURL(URL.createObjectURL(new Blob(['a']))); });
    attempt(function () { new CSSStyleSheet().replaceSync('p{}'); document.adoptedStyleSheets = [new CSSStyleSheet()]; });
    attempt(function () { $('i').dispatchEvent(new Event('change')); $('f').requestSubmit && $('f').requestSubmit(); });
    attempt(function () { window.getSelection().selectAllChildren($('p')); document.execCommand('selectAll'); });
    attempt(function () { $('p').animate([{ opacity: 0 }, { opacity: 1 }], 10); });
    attempt(function () { new IntersectionObserver(function () {}).observe($('p')); new ResizeObserver(function () {}).observe($('p')); });
    attempt(function () { customElements.define('x-a', class extends HTMLElement {}); document.createElement('x-a'); });
})();
"#;

/// How many accessor calls there were, how many with a receiver that is
/// not the page's, and which.
const VERDICT: &str = "calls + ' ' + notMine + ' ' + Object.keys(which).sort().join(',')";

fn text(v: JsValue) -> String {
    match v {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

/// Every engine path this crate has, with the traps in place.
fn exercise(b: &DomBindings, doc: &Document, net: bool) {
    let id = |name: &str| doc.get_element_by_id(name).unwrap().id.raw();
    let (input, button, link, para, drag) = (id("i"), id("b"), id("a"), id("p"), id("frag"));
    let mouse = MouseEventBindingData { client_x: 5.0, client_y: 5.0, ..Default::default() };
    let key = KeyboardEventBindingData { key: "a".into(), code: "KeyA".into(), ..Default::default() };
    let _ = b.set_ready_state("interactive");
    let _ = b.fire_lifecycle_event(LifecycleTarget::Document, "DOMContentLoaded");
    let _ = b.set_ready_state("complete");
    let _ = b.fire_lifecycle_event(LifecycleTarget::Window, "load");
    let _ = b.evaluate(PAGE_USES);
    let _ = b.run_jobs();
    let _ = b.run_timers(100, 100);
    let _ = b.advance_timers(50, 100);
    let _ = b.run_timers_for_frame(200, 100, 16);
    let _ = b.advance_timers_for_frame(50, 100, 16);
    let _ = b.clock_now_ms();
    let _ = b.next_timer_delay();
    for kind in ["mousemove", "mousedown", "mouseup", "click"] {
        let _ = b.fire_mouse_event(button, kind, &mouse);
    }
    let _ = b.fire_mouse_event(link, "click", &mouse);
    for kind in ["dragstart", "dragover", "drop", "dragend"] {
        let _ = b.fire_drag_event(drag, kind, &mouse, "move");
    }
    let _ = b.set_view_focused(false);
    let _ = b.set_view_focused(true);
    let _ = b.set_focus(Some(input));
    let _ = b.take_focus_move();
    for kind in ["keydown", "keypress", "keyup"] {
        let _ = b.fire_key_event(Some(input), kind, &key);
    }
    let _ = b.fire_input_event(input, Some("a"));
    let _ = b.user_edit(input, &rustkit_dom::forms::EditCommand::SelectAll);
    let _ = b.edit_key(input, "insert", "x");
    let _ = b.implicit_submit(input);
    let _ = b.set_selection(Some((para, 0, para, 1)));
    let _ = b.take_selection_change();
    let _ = b.set_focus(None);
    let _ = b.wheel_blocks(para);
    let _ = b.fire_wheel_event(para, &mouse, 0.0, 10.0, true);
    b.notify_scrolled();
    b.notify_element_scrolled(para);
    let _ = b.tick_observers();
    let _ = b.fire_media_query_changes();
    let _ = b.fire_script_event(id("img"), "load");
    let _ = b.fire_image_load(id("img"), 1, 1, "https://example.test/x.png");
    let _ = b.fire_image_error(id("img"));
    let _ = b.fire_image_gave_up(id("img"));
    let _ = b.fire_transition_event(para, None, "transitionend", "opacity", 0.1);
    let _ = b.fire_animation_event(para, None, "animationend", "n", 0.1);
    let _ = b.set_current_script(Some(para));
    let _ = b.set_current_script(None);
    let _ = b.set_location(&Url::parse("https://example.test/page#frag").unwrap());
    let _ = b.net_bridge_enabled();
    let requests = b.take_net_requests();
    if net {
        for request in &requests {
            let _ = b.shelve_net_request(request.id);
        }
        let _ = b.unshelve_net_requests();
        if let Some(first) = requests.first() {
            let _ = b.deliver_net_response(first.id, NetDelivery::Error("blocked".into()));
        }
    }
    let _ = b.pending_net_requests();
    let _ = b.fail_pending_net_requests("done");
    let _ = b.run_jobs();
    let _ = b.run_timers(1000, 100);
}

fn accessor_calls(net: bool) -> String {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    b.set_location(&Url::parse("https://example.test/page").unwrap()).unwrap();
    if net {
        b.enable_net_bridge().unwrap();
    }
    b.evaluate(&format!("var NAMES = {:?};", engine_names())).unwrap();
    b.evaluate(TRAP).unwrap();
    exercise(&b, &doc, net);
    text(b.evaluate(VERDICT).unwrap())
}

#[test]
fn engine_code_invokes_no_page_defined_accessor() {
    assert_eq!(accessor_calls(true), "0 0 ", "accessor calls, receivers not the page's, which");
}

#[test]
fn engine_code_invokes_no_page_defined_accessor_without_the_net_bridge() {
    assert_eq!(accessor_calls(false), "0 0 ", "accessor calls, receivers not the page's, which");
}

/// The engine's bookkeeping reads (the clock, the network queue) invoke
/// no page-defined accessor either.
#[test]
fn bookkeeping_reads_invoke_no_page_defined_accessor() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.evaluate(&format!("var NAMES = {:?};", engine_names())).unwrap();
    b.evaluate(TRAP).unwrap();
    let _ = b.take_net_requests();
    let _ = b.unshelve_net_requests();
    let _ = b.pending_net_requests();
    let _ = b.shelve_net_request(1);
    let _ = b.clock_now_ms();
    let _ = b.next_timer_delay();
    assert_eq!(text(b.evaluate(VERDICT).unwrap()), "0 0 ", "accessor calls, receivers not the page's, which");
}

/// Built-ins replaced by the page, each counting its calls.
const REPLACED: &str = r#"
var x = new XMLHttpRequest(); x.open('GET', 'https://example.test/a'); x.send();
var ran = 0;
function count() { ran++; }
JSON.stringify = function () { ran++; return '[]'; };
Object.keys = function () { ran++; return []; };
Array.prototype.splice = function () { ran++; return []; };
Array.prototype.push = function () { ran++; return 0; };
Array.prototype.toJSON = function () { ran++; return []; };
Object.prototype.toJSON = function () { ran++; return {}; };
Object.defineProperty(Object.prototype, 'stray', { get: count, set: count, enumerable: true, configurable: true });
ran = 0;
"#;

/// The network bridge's bookkeeping (taking the queue, shelving, counting)
/// uses the built-ins of install time, and reads the same answers whatever
/// a page has put in their place.
#[test]
fn net_bookkeeping_calls_nothing_a_page_replaced() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.set_location(&Url::parse("https://example.test/page").unwrap()).unwrap();
    b.enable_net_bridge().unwrap();
    b.evaluate(REPLACED).unwrap();
    let requests = b.take_net_requests();
    let waiting = b.pending_net_requests();
    let shelved = requests.first().map(|r| b.shelve_net_request(r.id));
    let back = b.unshelve_net_requests();
    let _ = b.clock_now_ms();
    let _ = b.next_timer_delay();
    assert_eq!(text(b.evaluate("String(ran)").unwrap()), "0", "calls of replaced built-ins");
    // The queue takes the next request after it has been emptied.
    b.evaluate("var y = new XMLHttpRequest(); y.open('GET', 'https://example.test/b'); y.send(); ran = 0;").unwrap();
    let again: Vec<String> = b.take_net_requests().into_iter().map(|r| r.url).collect();
    assert_eq!(again, vec!["https://example.test/b".to_string()], "the second take");
    assert_eq!(text(b.evaluate("String(ran)").unwrap()), "0", "calls of replaced built-ins");
    assert_eq!(
        (requests.len(), requests.first().map(|r| r.url.clone()), waiting, shelved, back),
        (1, Some("https://example.test/a".to_string()), 1, Some(true), 1),
        "what the bookkeeping read"
    );
}

/// The natives the runtime puts on the host object itself (rustkit-js).
const HOST_NATIVES: &[&str] = &[
    "compileDocumentFunction", "runDocumentScript",
    "reportError", "unsupported", "base64Decode", "base64Encode", "binaryDecode", "binaryEncode", "blobGet",
    "blobRevoke", "blobUrl", "takeBytes", "utf8Decode", "utf8Encode", "pixelsIn", "pixelsOut",
];

/// Every name this crate's sources read or write on the host object is a
/// reserved slot or a native: a new one is added to `HOST_SLOTS` with it.
#[test]
fn every_host_name_is_reserved() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut missing = std::collections::BTreeSet::new();
    let mut stack = vec![dir];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if name.ends_with("_tests.rs") || !(name.ends_with(".rs") || name.ends_with(".js")) {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for part in source.split("__rk.").skip(1) {
                let member: String = part.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$').collect();
                if !member.is_empty() && !HOST_SLOTS.contains(&member.as_str()) && !HOST_NATIVES.contains(&member.as_str()) {
                    missing.insert(format!("{member} ({name})"));
                }
            }
        }
    }
    assert!(missing.is_empty(), "host names that are not reserved slots: {missing:?}");
}

/// The host object and what hangs off it inherit from nothing, and every
/// reserved slot is an own property from the start.
#[test]
fn the_host_object_inherits_nothing() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.enable_net_bridge().unwrap();
    let slots = HOST_SLOTS.iter().chain(HOST_NATIVES).map(|s| format!("{s:?}")).collect::<Vec<_>>().join(",");
    let got = b
        .host_turn(
            &format!(
                "var proto = Object.getPrototypeOf, has = Object.prototype.hasOwnProperty, out = [];
                 if (proto(__rk) !== null) out.push('host');
                 [{slots}].forEach(function (n) {{ if (!has.call(__rk, n)) out.push(n); }});
                 ['domHooks', 'net', 'dragStore'].forEach(function (n) {{ if (proto(__rk[n]) !== null) out.push(n); }});
                 ['shadowParent', 'retarget', 'hostOf', 'mutation', 'scripts', 'windowNamed', 'hasListener', 'loadSnapshot', 'loadReplay', 'attrs', 'watched'].forEach(function (n) {{
                     if (!has.call(__rk.domHooks, n)) out.push('domHooks.' + n);
                 }});
                 return out.join(' ');"
            ),
            &[],
        )
        .unwrap();
    assert_eq!(text(got), "");
}

/// A response handler that asks again is heard at each round.
#[test]
fn the_queue_takes_a_request_made_by_a_response_handler() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    b.set_location(&Url::parse("https://example.test/page").unwrap()).unwrap();
    b.enable_net_bridge().unwrap();
    b.evaluate(
        "var path = []; function next(n) { var x = new XMLHttpRequest(); x.open('GET', 'https://example.test/s' + n); \
         x.onloadend = function () { path.push(n + ':' + x.status); if (n < 3) next(n + 1); }; x.send(); } next(1);",
    )
    .unwrap();
    let mut rounds = Vec::new();
    for _ in 0..4 {
        let requests = b.take_net_requests();
        rounds.push(requests.len());
        for request in requests {
            b.deliver_net_response(request.id, NetDelivery::Error("blocked".into())).unwrap();
        }
        b.run_jobs().unwrap();
    }
    assert_eq!((rounds, text(b.evaluate("path.join()").unwrap())), (vec![1, 1, 1, 0], "1:0,2:0,3:0".to_string()));
}

// ---------------------------------------------------------------------
// The same, for a frame tree: a page and a frame of its origin, each a
// document with its own window, and what the engine keeps once for both.
// ---------------------------------------------------------------------

fn frame_page() -> String {
    PAGE.replace("</body>", "<iframe id=\"fr\" src=\"frame.html\" width=\"120\" height=\"40\"></iframe></body>")
}

struct Tree {
    parent: DomBindings,
    doc: Rc<Document>,
    child: DomBindings,
    child_doc: Rc<Document>,
    node: usize,
}

/// A page with frames on, its frame asked for, and the frame's document
/// made, not yet attached. `before_child` runs in the page first, as its
/// script does before a frame of it exists.
fn tree(net: bool, before_child: &str) -> Tree {
    let parent = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(&frame_page()).unwrap());
    parent.set_document(doc.clone()).unwrap();
    parent.set_location(&Url::parse("https://example.test/page").unwrap()).unwrap();
    if net {
        parent.enable_net_bridge().unwrap();
    }
    parent.enable_frames().unwrap();
    parent.evaluate(before_child).unwrap();
    let _ = parent.scan_frames();
    let _ = parent.take_frame_requests();
    let child = parent.new_child().unwrap();
    let child_doc = Rc::new(Document::parse_html(PAGE).unwrap());
    child.set_document(child_doc.clone()).unwrap();
    child.set_location(&Url::parse("https://example.test/frame.html").unwrap()).unwrap();
    if net {
        child.enable_net_bridge().unwrap();
    }
    child.enable_frames().unwrap();
    let node = doc.get_element_by_id("fr").unwrap().id.raw();
    Tree { parent, doc, child, child_doc, node }
}

/// The accessor calls in the page and in its frame, with the accessors
/// on both windows' `Object.prototype`, through every engine path of both
/// documents and the frame's whole life.
fn accessor_calls_in_a_frame_tree(net: bool) -> (String, String) {
    let names = format!("var NAMES = {:?};", format!("{} {FRAME_NAMES}", engine_names()));
    let t = tree(net, &format!("{names}{TRAP}"));
    t.child.evaluate(&names).unwrap();
    t.child.evaluate(TRAP).unwrap();
    let _ = t.parent.frame_size(t.node);
    let attached = t.parent.attach_frame(t.node, &t.child);
    assert!(matches!(attached, Ok(true)), "the frame is attached: {attached:?}");
    t.parent.evaluate("adopt(document.getElementById('fr').contentWindow);").unwrap();
    t.child.evaluate("adopt(parent);").unwrap();
    let _ = t.parent.frame_loaded(t.node, false);
    exercise(&t.child, &t.child_doc, net);
    exercise(&t.parent, &t.doc, net);
    // Each window's script in the other's document.
    let _ = t.parent.evaluate(
        "(function () { try { var w = document.getElementById('fr').contentWindow; w.setTimeout(function () {}, 0); \
         w.requestAnimationFrame(function () {}); w.document.getElementById('p').dispatchEvent(new Event('x')); \
         w.document.body.appendChild(document.createElement('i')); w.document.getElementById('b').click(); } catch (e) {} })();",
    );
    let _ = t.child.evaluate(
        "(function () { try { parent.setTimeout(function () {}, 0); parent.document.getElementById('b').click(); \
         parent.document.body.appendChild(document.createElement('i')); frameElement.setAttribute('src', 'other.html'); } catch (e) {} })();",
    );
    let _ = t.parent.advance_timers(50, 100);
    let _ = t.child.advance_timers(50, 100);
    let _ = t.parent.run_animation_round(100);
    let _ = t.child.run_animation_round(100);
    let _ = t.child.next_timer_delay();
    let _ = t.parent.run_timers(5000, 100);
    let _ = t.parent.take_frame_requests();
    let in_frame = text(t.child.evaluate(VERDICT).unwrap());
    let Tree { parent, child, .. } = t;
    parent.drop_child(child);
    let _ = parent.next_timer_delay();
    let _ = parent.run_timers(9000, 100);
    (text(parent.evaluate(VERDICT).unwrap()), in_frame)
}

#[test]
fn engine_code_invokes_no_page_defined_accessor_in_a_frame_tree() {
    let (page, frame) = accessor_calls_in_a_frame_tree(true);
    assert_eq!((page.as_str(), frame.as_str()), ("0 0 ", "0 0 "), "accessor calls, receivers not the page's, which: in the page, in its frame");
}

#[test]
fn engine_code_invokes_no_page_defined_accessor_in_a_frame_tree_without_the_net_bridge() {
    let (page, frame) = accessor_calls_in_a_frame_tree(false);
    assert_eq!((page.as_str(), frame.as_str()), ("0 0 ", "0 0 "), "accessor calls, receivers not the page's, which: in the page, in its frame");
}

/// A frame's host object is as the page's: it inherits nothing and owns
/// every reserved slot. What the frame tree keeps once inherits nothing
/// either, and owns every name the scripts use on it from the start.
#[test]
fn a_frames_host_object_and_the_trees_shared_state_inherit_nothing() {
    let t = tree(true, "");
    let slots = HOST_SLOTS.iter().chain(HOST_NATIVES).map(|s| format!("{s:?}")).collect::<Vec<_>>().join(",");
    let shared = shared_names().iter().map(|s| format!("{s:?}")).collect::<Vec<_>>().join(",");
    let body = format!(
        "var proto = Object.getPrototypeOf, has = Object.prototype.hasOwnProperty, out = [];
         if (proto(__rk) !== null) out.push('host');
         [{slots}].forEach(function (n) {{ if (!has.call(__rk, n)) out.push(n); }});
         ['domHooks', 'net', 'dragStore', 'frames', 'shared'].forEach(function (n) {{ if (!__rk[n] || proto(__rk[n]) !== null) out.push(n); }});
         ['shadowParent', 'retarget', 'hostOf', 'mutation', 'scripts', 'windowNamed', 'hasListener', 'loadSnapshot', 'loadReplay', 'nodeId', 'byTag', 'frames', 'attrs', 'watched'].forEach(function (n) {{
             if (!has.call(__rk.domHooks, n)) out.push('domHooks.' + n);
         }});
         if (__rk.domHooks.frames && proto(__rk.domHooks.frames) !== null) out.push('domHooks.frames');
         [{shared}].forEach(function (n) {{ if (!__rk.shared || !has.call(__rk.shared, n)) out.push('shared.' + n); }});
         var queue = __rk.shared && __rk.shared.timerQueue;
         if (!queue || proto(queue) !== null) out.push('shared.timerQueue');
         // The tree's clock is kept on the queue's record, every slot there from the start.
         'now det reads seenNow skew lastPerf startedAt frameTs frameTsAt prevFrameTs prevFrameAt'.split(' ').forEach(function (n) {{
             if (!queue || !has.call(queue, n)) out.push('timerQueue.' + n);
         }});
         return out.join(' ');"
    );
    let page = text(t.parent.host_turn(&body, &[]).unwrap());
    let frame = text(t.child.host_turn(&body, &[]).unwrap());
    assert_eq!((page.as_str(), frame.as_str()), ("", ""), "what is not so: in the page, in its frame");
    let same = t.parent.host_turn("__rk.shared.probe = 1; return '';", &[]).and_then(|_| t.child.host_turn("var one = __rk.shared.probe === 1; delete __rk.shared.probe; return String(one);", &[]));
    assert_eq!(text(same.unwrap()), "true", "the two documents hold one shared state");
}

/// A timer set in a frame joins the tree's queue through nothing a page
/// has replaced or defined, and runs.
#[test]
fn a_frames_timer_joins_the_queue_through_nothing_a_page_replaced() {
    let replaced = "var ran = 0; Array.prototype.push = function () { ran++; return 0; }; \
        ['0', '1', '2'].forEach(function (i) { \
            Object.defineProperty(Array.prototype, i, { get: function () { ran++; }, set: function () { ran++; }, configurable: true }); \
        }); ran = 0;";
    let t = tree(false, replaced);
    t.parent.evaluate("var hits = 0; setTimeout(function () { hits++; }, 5);").unwrap();
    t.child.evaluate("var hits = 0; setTimeout(function () { hits++; }, 5); setTimeout(function () { hits++; }, 6);").unwrap();
    let _ = t.child.next_timer_delay();
    t.parent.run_timers(100, 100).unwrap();
    let got = (
        text(t.parent.evaluate("String(ran)").unwrap()),
        text(t.parent.evaluate("String(hits)").unwrap()),
        text(t.child.evaluate("String(hits)").unwrap()),
    );
    assert_eq!(got, ("0".to_string(), "1".to_string(), "2".to_string()), "calls of what the page replaced, the page's timers run, the frame's");
}

/// The frame bindings' host function is taken off the window by their
/// install, also by an install that finds them already there.
#[test]
fn the_frame_install_leaves_no_function_on_the_window() {
    let page = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    page.set_document(Rc::new(Document::parse_html(PAGE).unwrap())).unwrap();
    let left = "['__rustkit_frame_request'].filter(function (n) { return n in globalThis; }).join()";
    page.enable_frames().unwrap();
    assert_eq!(text(page.evaluate(left).unwrap()), "");
    page.enable_frames().unwrap();
    assert_eq!(text(page.evaluate(left).unwrap()), "", "after a second install");
}

/// Attaching a frame hands two windows over. Whatever the outcome, nothing
/// is left where they changed hands, and a frame that was not attached
/// holds no window.
#[test]
fn an_attach_leaves_no_window_behind() {
    let frame_window = "document.getElementById('fr').contentWindow === null ? 'none' : 'the frame'";
    let left = "return String(__rk.shared.handoff === null || __rk.shared.handoff === undefined);";
    // One that goes through.
    let t = tree(false, "");
    let attached = t.parent.attach_frame(t.node, &t.child);
    assert!(matches!(attached, Ok(true)), "{attached:?}");
    assert_eq!(text(t.parent.host_turn(left, &[]).unwrap()), "true", "nothing left after an attach");
    assert_eq!(text(t.parent.evaluate(frame_window).unwrap()), "the frame");
    // One whose frame's window refuses its new properties.
    let t = tree(false, "");
    t.child.evaluate("Object.defineProperty = function () { throw new Error('refused'); };").unwrap();
    let attached = matches!(t.parent.attach_frame(t.node, &t.child), Ok(true));
    assert_eq!(text(t.parent.host_turn(left, &[]).unwrap()), "true", "nothing left after an attach that was refused");
    let expected = if attached { "the frame" } else { "none" };
    assert_eq!(text(t.parent.evaluate(frame_window).unwrap()), expected, "attached: {attached}");
    // One whose frame's window cannot take its place in the tree: the
    // frame holds no window, and nothing is left with either document.
    let t = tree(false, "");
    t.child.evaluate("Object.defineProperty(window, 'top', { value: 1, writable: false, configurable: false });").unwrap();
    let attached = t.parent.attach_frame(t.node, &t.child);
    assert!(!matches!(attached, Ok(true)), "an attach that cannot be completed: {attached:?}");
    assert_eq!(text(t.parent.host_turn(left, &[]).unwrap()), "true", "nothing left with the page after a failed attach");
    assert_eq!(text(t.child.host_turn(left, &[]).unwrap()), "true", "nothing left with the frame after a failed attach");
    assert_eq!(text(t.parent.evaluate(frame_window).unwrap()), "none", "a frame that was not attached holds no window");
    // One for a node that is not a frame.
    let t = tree(false, "");
    let para = t.doc.get_element_by_id("p").unwrap().id.raw();
    assert!(matches!(t.parent.attach_frame(para, &t.child), Ok(false)));
    assert_eq!(text(t.child.host_turn(left, &[]).unwrap()), "true", "nothing left after an attach to no frame");
}

// ---------------------------------------------------------------------
// One event loop for the frame tree: the timers, the animation frame
// round, the workers' messages and an event's dispatch state are the
// tree's, whichever document set, asked for or made them.
// ---------------------------------------------------------------------

/// A frame's document goes away: its timers go with it, the page's stay.
#[test]
fn a_dropped_frames_timers_are_gone() {
    let t = tree(false, "");
    t.parent.evaluate("var hits = 0; setTimeout(function () { hits++; }, 50);").unwrap();
    t.child
        .evaluate(
            "setTimeout(function () {}, 5); setInterval(function () {}, 7); requestAnimationFrame(function () {}); \
             setTimeout(function () {}, 900);",
        )
        .unwrap();
    assert_eq!(t.parent.next_timer_delay(), Some(5), "the frame's timer is the tree's next");
    let Tree { parent, child, .. } = t;
    parent.drop_child(child);
    let next = parent.next_timer_delay();
    let ran = parent.run_timers(5000, 100).unwrap();
    let after = parent.next_timer_delay();
    assert_eq!(
        (next, ran, after, text(parent.evaluate("String(hits)").unwrap())),
        (Some(50), 1, None, "1".to_string()),
        "the next timer once the frame is gone, callbacks then run, what is left, the page's own timer"
    );
}

/// A turn that stopped for its time still has its frame: the animation
/// frame callbacks that are due run once, the page's and its frame's, and
/// one asked for during the round waits for the next.
#[test]
fn a_stopped_turns_animation_round_runs_the_frame_callbacks() {
    let t = tree(false, "");
    t.parent
        .evaluate("var frames = 0; requestAnimationFrame(function () { frames++; }); setTimeout(function () { frames += 100; }, 500);")
        .unwrap();
    t.child
        .evaluate(
            "var frames = 0; requestAnimationFrame(function () { frames++; requestAnimationFrame(function () { frames++; }); });",
        )
        .unwrap();
    // The clock moves on and no callback runs, as when a turn stops.
    assert_eq!(t.parent.advance_timers(20, 0).unwrap(), 0);
    let ran = t.parent.run_animation_round(100).unwrap();
    let got = (
        ran,
        text(t.parent.evaluate("String(frames)").unwrap()),
        text(t.child.evaluate("String(frames)").unwrap()),
        t.parent.next_timer_delay(),
    );
    // The frame asked for in the round is the next on the 60 Hz grid: 34, from the clock at 20.
    assert_eq!(
        got,
        (2, "1".to_string(), "1".to_string(), Some(14)),
        "callbacks run, frames seen by the page, by its frame, and when the next is due"
    );
}

/// A worker started by a frame's document is heard: the frame's turn
/// takes no timers (the tree's are run from the page's), and the worker's
/// messages arrive all the same.
#[test]
fn a_frames_worker_is_heard_in_the_trees_turn() {
    let t = tree(true, "");
    let attached = t.parent.attach_frame(t.node, &t.child);
    assert!(matches!(attached, Ok(true)), "{attached:?}");
    t.child
        .evaluate(
            "var got = []; var w = new Worker('data:text/javascript,postMessage(7)'); \
             w.onmessage = function (e) { got.push(e.data); };",
        )
        .unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut heard = String::new();
    while std::time::Instant::now() < end {
        // A turn of the tree: the frame's with no timers, then the page's.
        let _ = t.child.advance_timers_for_frame(0, 0, 16);
        let _ = t.parent.advance_timers_for_frame(5, 1, 16);
        heard = text(t.child.evaluate("got.join()").unwrap());
        if !heard.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(heard, "7", "what the frame's worker sent");
}

/// An event made in one window and dispatched in another has one
/// dispatch state: stopping it through either window's prototype stops
/// the dispatch, and its path reads the same.
#[test]
fn an_event_of_one_window_keeps_its_dispatch_state_in_another() {
    let t = tree(false, "");
    let attached = t.parent.attach_frame(t.node, &t.child);
    assert!(matches!(attached, Ok(true)), "{attached:?}");
    // `doc` is the document dispatched in, `W` the window whose `Event` makes the event.
    let run = "function run(doc, W, n) {
        var x = 'x' + n, y = 'y' + n;
        var seen = [], p = doc.getElementById('p'), b = doc.getElementById('b');
        p.addEventListener(x, function (e) {
            seen.push(e.composedPath()[0] === p ? 'path' : 'no-path');
            e.stopPropagation();
            seen.push('stopped:' + e.cancelBubble);
        });
        doc.body.addEventListener(x, function () { seen.push('body'); });
        p.dispatchEvent(new W.Event(x, { bubbles: true }));
        b.addEventListener(y, function (e) { seen.push('first'); e.stopImmediatePropagation(); });
        b.addEventListener(y, function () { seen.push('second'); });
        doc.body.addEventListener(y, function () { seen.push('body'); });
        b.dispatchEvent(new W.Event(y, { bubbles: true }));
        return seen.join(' ');
    }";
    let got = text(
        t.parent
            .evaluate(&format!(
                "{run} var w = document.getElementById('fr').contentWindow; \
                 [run(document, w, 1), run(w.document, window, 2), run(document, window, 3), run(w.document, w, 4)].join(' | ')"
            ))
            .unwrap(),
    );
    let one = "path stopped:true first";
    assert_eq!(
        got,
        format!("{one} | {one} | {one} | {one}"),
        "the frame's event in the page, the page's in the frame, each window's own"
    );
}

/// The user's input and the engine's transition and animation events in
/// a frame tree, with an accessor on both windows' `Object.prototype` for
/// every name such an event is described with: none is invoked, in the
/// page or in its frame, and each event arrives as the engine made it.
#[test]
fn input_events_in_a_frame_tree_are_described_by_objects_that_inherit_nothing() {
    let names = format!("var NAMES = {INIT_NAMES:?};");
    let listen = "var seen = []; ['mousedown', 'click', 'keydown', 'wheel', 'dragstart', 'transitionend', 'animationend'].forEach(function (type) { \
        document.addEventListener(type, function (e) { seen.push(type + ':' + e.isTrusted + ':' + e.bubbles + ':' + e.cancelable); }); });";
    let t = tree(false, &format!("{names}{TRAP}{listen}"));
    t.child.evaluate(&format!("{names}{TRAP}{listen}")).unwrap();
    let attached = t.parent.attach_frame(t.node, &t.child);
    assert!(matches!(attached, Ok(true)), "{attached:?}");
    t.parent.evaluate("adopt(document.getElementById('fr').contentWindow);").unwrap();
    t.child.evaluate("adopt(parent);").unwrap();
    let mouse = MouseEventBindingData { client_x: 5.0, client_y: 5.0, ..Default::default() };
    let key = KeyboardEventBindingData { key: "a".into(), code: "KeyA".into(), ..Default::default() };
    let fire = |b: &DomBindings, doc: &Document| {
        let id = |name: &str| doc.get_element_by_id(name).unwrap().id.raw();
        let (para, input, drag) = (id("p"), id("i"), id("frag"));
        for kind in ["mousedown", "click"] {
            let _ = b.fire_mouse_event(para, kind, &mouse);
        }
        let _ = b.fire_key_event(Some(input), "keydown", &key);
        let _ = b.fire_wheel_event(para, &mouse, 0.0, 10.0, true);
        let _ = b.fire_drag_event(drag, "dragstart", &mouse, "move");
        let _ = b.fire_transition_event(para, None, "transitionend", "opacity", 0.1);
        let _ = b.fire_animation_event(para, None, "animationend", "n", 0.1);
        (text(b.evaluate(VERDICT).unwrap()), text(b.evaluate("seen.join(' ')").unwrap()))
    };
    let in_frame = fire(&t.child, &t.child_doc);
    let in_page = fire(&t.parent, &t.doc);
    let seen = "mousedown:true:true:true click:true:true:true keydown:true:true:true wheel:true:true:true \
        dragstart:true:true:true transitionend:true:true:true animationend:true:true:false";
    let want = ("0 0 ".to_string(), seen.to_string());
    assert_eq!((in_page, in_frame), (want.clone(), want), "accessor calls and what the listeners saw: in the page, in its frame");
}
