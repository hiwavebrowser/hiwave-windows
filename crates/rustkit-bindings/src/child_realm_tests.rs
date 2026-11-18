//! Stage 1 of the nested-browsing-contexts plan
//! (docs/diagnostics/2026-11-11/nested-browsing-contexts-plan.md): two
//! documents in one JavaScript runtime, each in its own realm, the way a
//! page and its same-origin `<iframe>` must be for script to reach across.
//! No engine, no layout, no loading: this is the proof that the runtime and
//! the bindings can be shared at all, or the place the plan stops.
//!
//! The expected answers are a browser's for a page and a same-origin frame
//! (`parent.child` standing for `iframe.contentWindow`). Where a comment
//! says "Chrome 148", the value was recorded from Chrome for Testing
//! 148.0.7778.216 (headless, 2026-11-11) on a page with a same-origin
//! `<iframe>` running the same expressions against `iframe.contentWindow`:
//!   node:     true,false,true,true,false,child text
//!   dispatch: true, and the child heard `ping:true:true:false,bubbled`
//!   error:    child broke:true:false

use super::*;

const PARENT: &str = "<html><head><title>parent-title</title></head><body><p id=p>parent text</p></body></html>";
const CHILD: &str = "<html><head><title>child-title</title></head><body><div id=c>child text</div><input id=i></body></html>";

struct Tree {
    parent: DomBindings,
    child: DomBindings,
}

fn tree() -> Tree {
    let parent = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    parent.set_document(Rc::new(Document::parse_html(PARENT).unwrap())).unwrap();
    let child = parent.new_child().unwrap();
    child.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
    parent.expose_window("child", &child).unwrap();
    child.expose_window("up", &parent).unwrap();
    Tree { parent, child }
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Boolean(x)) => x.to_string(),
        Ok(JsValue::Number(n)) => format!("{n}"),
        Ok(JsValue::Undefined) => "undefined".into(),
        Ok(JsValue::Null) => "null".into(),
        Ok(other) => format!("{other:?}"),
        Err(e) => format!("ERROR {e}"),
    }
}

/// The engine's entry points are each document's own: making a child
/// realm does not replace the parent's. (They are kept on the host object;
/// with one host object for the runtime, the child's install replaced the
/// parent's, and the engine's key event for a node of the parent's document
/// was looked up in the child's.)
#[test]
fn the_engines_entry_points_stay_each_documents_own() {
    let parent = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PARENT).unwrap());
    parent.set_document(doc.clone()).unwrap();
    let child = parent.new_child().unwrap();
    child.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
    ev(&parent, "window.heard = ''; document.getElementById('p').addEventListener('keydown', function (e) { heard = 'parent p ' + e.key; }); ''");
    let p = doc.get_element_by_id("p").expect("the parent's p").id.raw();
    let key = KeyboardEventBindingData { key: "a".to_string(), code: "KeyA".to_string(), ..Default::default() };
    parent.fire_key_event(Some(p), "keydown", &key).unwrap();
    assert_eq!(ev(&parent, "heard"), "parent p a");
}

#[test]
fn each_page_has_its_own_globals_and_its_own_document() {
    let t = tree();
    ev(&t.parent, "var mark = 'parent mark';");
    ev(&t.child, "var mark = 'child mark';");
    assert_eq!(ev(&t.parent, "mark + '|' + document.title + '|' + document.getElementById('p').textContent"), "parent mark|parent-title|parent text");
    assert_eq!(ev(&t.child, "mark + '|' + document.title + '|' + document.getElementById('c').textContent"), "child mark|child-title|child text");
    assert_eq!(ev(&t.parent, "String(document.getElementById('c'))"), "null", "the parent's document is not the child's");
    assert_eq!(ev(&t.child, "typeof p_only + '|' + (window === up)"), "undefined|false");
}

#[test]
fn the_parent_reaches_the_childs_window_document_and_functions() {
    let t = tree();
    ev(&t.child, "function childFn(a) { return 'child-fn:' + a + ':' + document.title; } var counter = 0;");
    // Speedometer's runner: frame.contentWindow[name](), contentDocument.querySelector(...)
    assert_eq!(ev(&t.parent, "child.childFn('x')"), "child-fn:x:child-title");
    assert_eq!(ev(&t.parent, "child.document.title + '|' + child.document.querySelector('#c').textContent"), "child-title|child text");
    assert_eq!(ev(&t.parent, "child.counter = 5; child.counter"), "5");
    assert_eq!(ev(&t.child, "counter"), "5", "a write from the parent is the child's global");
    assert_eq!(ev(&t.child, "up.document.title + '|' + (up.child === window)"), "parent-title|true");
}

#[test]
fn a_child_node_held_by_the_parent_is_the_childs_node() {
    let t = tree();
    // Chrome 148: true,false,true,true,false,child text
    assert_eq!(
        ev(
            &t.parent,
            "var n = child.document.getElementById('c'); \
             [n instanceof child.HTMLElement, n instanceof HTMLElement, n.ownerDocument === child.document, \
              n.ownerDocument.defaultView === child, child.Array === Array, n.textContent].join()"
        ),
        "true,false,true,true,false,child text"
    );
    // Written from the parent, read in the child.
    ev(&t.parent, "n.textContent = 'written by parent'; n.setAttribute('data-from', 'parent'); child.document.getElementById('i').value = 'typed';");
    assert_eq!(
        ev(&t.child, "var c = document.getElementById('c'); c.textContent + '|' + c.dataset.from + '|' + document.getElementById('i').value"),
        "written by parent|parent|typed"
    );
    assert_ne!(t.child.take_dirty(), DomDirty::Clean, "the write marked the child's document for layout");
    assert_eq!(t.parent.take_dirty(), DomDirty::Clean, "and not the parent's");
}

#[test]
fn an_event_made_in_the_parent_reaches_a_listener_in_the_child() {
    let t = tree();
    ev(&t.child, "var heard = []; document.getElementById('c').addEventListener('ping', function (e) { heard.push(e.type + ':' + e.bubbles + ':' + (e.target === this) + ':' + (e instanceof Event)); }); document.addEventListener('ping', function () { heard.push('bubbled'); });");
    // PageElement.dispatchEvent: `new Event(...)` is the parent's Event.
    // Chrome 148: the listener runs; `e instanceof Event` in the child is false.
    assert_eq!(ev(&t.parent, "child.document.getElementById('c').dispatchEvent(new Event('ping', { bubbles: true }))"), "true");
    assert_eq!(ev(&t.child, "heard.join()"), "ping:true:true:false,bubbled");
    assert_eq!(ev(&t.parent, "child.document.getElementById('c').click(); 'clicked'"), "clicked");
}

/// The documents of a frame tree are one event loop (HTML 8.1.7): their
/// timers are one queue on one clock, run in due order (ties in the order
/// set), each in the document that set it. (Stage 1 gave each page its own
/// clock; Speedometer's React-Stockcharts-SVG then ran a frame's due timers
/// after the page's later ones. The order is Chrome 148's, pinned in
/// rustkit-engine `frames_stage2_tests`.)
#[test]
fn a_frame_trees_timers_are_one_queue() {
    let t = tree();
    ev(&t.parent, "var ticks = []; setTimeout(function () { ticks.push('parent'); }, 10);");
    ev(&t.child, "var ticks = []; setTimeout(function () { ticks.push('child'); }, 10); requestAnimationFrame(function () { ticks.push('child frame'); });");
    ev(&t.parent, "window.seen = []; setTimeout(function () { seen.push(child.ticks.length); }, 10);");
    t.parent.run_timers(100, 100).unwrap();
    assert_eq!(ev(&t.parent, "ticks.join()"), "parent");
    assert_eq!(ev(&t.child, "ticks.join()"), "child,child frame", "the tree's clock runs the child's timers");
    assert_eq!(ev(&t.parent, "seen.join()"), "1", "in the order set: the child's 10 ms ran before the parent's later 10 ms");
    // A timer the parent sets through the child's window is the child's,
    // and clearing it from the parent's own clearTimeout does nothing.
    ev(&t.parent, "var id = child.setTimeout(function () { child.ticks.push('set by parent'); }, 10); clearTimeout(id);");
    t.parent.run_timers(200, 100).unwrap();
    assert_eq!(ev(&t.child, "ticks[2]"), "set by parent");
}

#[test]
fn promises_cross_the_boundary_on_the_one_job_queue() {
    let t = tree();
    ev(&t.child, "function later(v) { return Promise.resolve(v).then(function (x) { return x + ' via child'; }); }");
    ev(&t.parent, "var got = 'pending'; child.later('value').then(function (v) { got = v; });");
    assert_eq!(ev(&t.parent, "got"), "value via child");
}

#[test]
fn a_runaway_loop_in_the_child_is_stopped_whoever_called_it() {
    let t = tree();
    ev(&t.child, "function spin() { var n = 0; while (true) { n++; } }");
    t.parent.set_loop_iteration_limit(10_000);
    let stopped = ev(&t.parent, "var caught = 'not caught'; try { child.spin(); } catch (e) { caught = 'caught'; } caught");
    assert!(stopped.starts_with("ERROR"), "the stop is not catchable: {stopped}");
    assert_eq!(ev(&t.parent, "1 + 1"), "2", "the parent runs again");
    assert_eq!(ev(&t.child, "document.title"), "child-title", "and so does the child");
}

#[test]
fn an_uncaught_error_in_the_child_is_the_childs_and_leaves_both_pages_running() {
    let t = tree();
    ev(&t.child, "function boom() { throw new RangeError('child broke'); }");
    // Chrome 148: child broke:true:false
    assert_eq!(ev(&t.parent, "var seen; try { child.boom(); } catch (e) { seen = e.message + ':' + (e instanceof child.RangeError) + ':' + (e instanceof RangeError); } seen"), "child broke:true:false");
    assert_eq!(ev(&t.child, "typeof boom"), "function");
}

#[test]
fn dropping_the_child_leaves_the_parent_working() {
    let Tree { parent, child } = tree();
    ev(&child, "var kept = { from: 'child' };");
    ev(&parent, "var held = child.kept; var heldNode = child.document.getElementById('c');");
    assert!(parent.drop_child(child));
    assert_eq!(ev(&parent, "held.from + '|' + document.title"), "child|parent-title", "an object it held outlives the child's page");
    // A second child takes the first one's place.
    let again = parent.new_child().unwrap();
    again.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
    parent.expose_window("child", &again).unwrap();
    assert_eq!(ev(&parent, "child.document.title + '|' + typeof child.kept"), "child-title|undefined");
}

#[test]
fn ten_children_in_one_runtime() {
    let parent = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    parent.set_document(Rc::new(Document::parse_html(PARENT).unwrap())).unwrap();
    let started = std::time::Instant::now();
    let children: Vec<DomBindings> = (0..10)
        .map(|i| {
            let child = parent.new_child().unwrap();
            child.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
            ev(&child, &format!("var me = {i};"));
            parent.expose_window(&format!("frame{i}"), &child).unwrap();
            child
        })
        .collect();
    eprintln!("ten child pages: {} ms", started.elapsed().as_millis());
    assert_eq!(ev(&parent, "[frame0.me, frame4.me, frame9.me, frame9.document.title].join()"), "0,4,9,child-title");
    assert_eq!(ev(&children[3], "me"), "3");
}

/// A child realm installs Intl, Streams, Web Animations and the canvas 2D
/// context on first use (web_lazy.js); script sees the same answers as in
/// a page that installed them eagerly.
#[test]
fn a_childs_lazily_installed_interfaces_answer_as_the_parents_do() {
    let t = tree();
    let probe = "[typeof Intl, typeof Intl.NumberFormat, (1234.5).toLocaleString(), new Date(0).toLocaleDateString('en-US', { timeZone: 'UTC' }), \
                 'a'.localeCompare('b'), typeof ReadableStream, typeof new ReadableStream().getReader, typeof TransformStream, \
                 typeof Animation, typeof document.timeline, typeof document.body.animate, typeof document.getAnimations, \
                 typeof CanvasRenderingContext2D, document.createElement('canvas').width, \
                 typeof document.createElement('canvas').getContext('2d').fillRect].join()";
    let parent = ev(&t.parent, probe);
    assert!(!parent.starts_with("ERROR"), "{parent}");
    assert_eq!(ev(&t.child, probe), parent);
    // Touching one installs that one only: the others are still stand-ins
    // until asked (a fresh child).
    let fresh = t.parent.new_child().unwrap();
    fresh.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
    assert_eq!(ev(&fresh, "(1234.5).toLocaleString()"), "1,234.5");
    assert_eq!(ev(&fresh, "typeof Object.getOwnPropertyDescriptor(globalThis, 'ReadableStream').get"), "function");
}

/// A blob URL belongs to the origin of the document that made it. The
/// documents of a frame tree share one runtime, and so one store: a frame
/// of the maker's origin reads and revokes the URL (File API §8.3.2: the
/// blob URL store is shared by same-origin documents), a document of
/// another origin does neither, and the name is not one it could count its
/// way to (Chrome 148: `blob:<origin>/<uuid>`).
#[test]
fn a_blob_url_belongs_to_the_origin_that_made_it() {
    let page = |parent: Option<&DomBindings>, url: &str| {
        let b = match parent {
            Some(parent) => parent.new_child().unwrap(),
            None => DomBindings::new(JsRuntime::new().unwrap()).unwrap(),
        };
        b.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
        b.set_location(&Url::parse(url).unwrap()).unwrap();
        b.enable_net_bridge().unwrap();
        b
    };
    let parent = page(None, "https://a.test/");
    let same = page(Some(&parent), "https://a.test/frame");
    let other = page(Some(&parent), "https://b.test/frame");
    let url = ev(&parent, "var u = URL.createObjectURL(new Blob(['secret'], { type: 'text/plain' })); u");
    let read = |b: &DomBindings| {
        ev(b, &format!("var out = 'pending'; fetch({url:?}).then(function (r) {{ return r.text(); }}).then(function (t) {{ out = t; }}, function (e) {{ out = e.name; }}); 0"));
        ev(b, "out")
    };

    assert_eq!(other.blob_url_text(&url), None, "another origin's document read the blob through the engine");
    assert_eq!(read(&other), "TypeError", "another origin's document fetched the blob");
    assert_eq!(same.blob_url_text(&url).as_deref(), Some("secret"));
    assert_eq!(read(&same), "secret");

    ev(&other, &format!("URL.revokeObjectURL({url:?}); 0"));
    assert_eq!(parent.blob_url_text(&url).as_deref(), Some("secret"), "another origin's document revoked the blob");
    ev(&same, &format!("URL.revokeObjectURL({url:?}); 0"));
    assert_eq!(parent.blob_url_text(&url), None, "a same-origin frame revokes it");

    // The name: the maker's origin and a version 4 UUID.
    let shape = "/^blob:https:\\/\\/a\\.test\\/[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/";
    assert_eq!(ev(&parent, &format!("{shape}.test(u) + '|' + (URL.createObjectURL(new Blob([])) !== URL.createObjectURL(new Blob([])))")), "true|true", "{url}");
    // What the page says its origin is does not move it.
    ev(&other, "try { window.location.origin = 'https://a.test'; } catch (e) {} 0");
    let second = ev(&parent, "URL.createObjectURL(new Blob(['again']))");
    assert_eq!(other.blob_url_text(&second), None);
    assert_eq!(ev(&other, &format!("{shape}.test(URL.createObjectURL(new Blob([])))")), "false", "the name carries the origin the engine knows");
}

/// Past the store's bound a blob URL has nothing behind it, and the page's
/// developer is told: the miss is reported as an unsupported feature.
#[test]
fn a_full_blob_store_is_reported() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html(CHILD).unwrap())).unwrap();
    b.runtime.borrow_mut().set_blob_store_limit(8);
    let kept = ev(&b, "URL.createObjectURL(new Blob(['12345678']))");
    assert_eq!(b.blob_url_text(&kept).as_deref(), Some("12345678"));
    assert!(b.take_unsupported_apis().is_empty());
    let dropped = ev(&b, "URL.createObjectURL(new Blob(['x']))");
    assert_eq!(b.blob_url_text(&dropped), None);
    let apis = b.take_unsupported_apis();
    assert_eq!(apis.len(), 1, "{apis:?}");
    assert_eq!((apis[0].0.as_str(), apis[0].1.as_str()), ("URL", "createObjectURL (blob store full)"));
    ev(&b, &format!("URL.revokeObjectURL({kept:?}); 0"));
    let after = ev(&b, "URL.createObjectURL(new Blob(['x']))");
    assert_eq!(b.blob_url_text(&after).as_deref(), Some("x"), "revoking makes room");
}

// ---------------------------------------------------------------------
// TRUST-SHARED: what the engine keeps about an event (which it made,
// its type, its kind, its dispatch state) is the frame tree's, not each
// window's: an event is one object whichever window made it, dispatches
// it or reads it. The host object (`__rk`) is reached with `host`, as the
// embedder's scripts reach it; page script (`ev`) cannot.
// ---------------------------------------------------------------------

fn host(b: &DomBindings, body: &str) -> String {
    match b.host_turn(body, &[]) {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Boolean(x)) => x.to_string(),
        Ok(other) => format!("{other:?}"),
        Err(e) => format!("ERROR {e}"),
    }
}

/// (a) An event the engine made and trusted in the parent, dispatched at a
/// node of the child's document: the child's listener reads it trusted,
/// with its type, target, currentTarget and phase, and it is still the
/// mouse click it was made as (a checkbox of the child's is checked by it).
#[test]
fn a_parent_made_trusted_event_keeps_its_trust_and_type_in_the_child() {
    let t = tree();
    ev(
        &t.child,
        "var heard = []; var c = document.getElementById('c'); \
         c.addEventListener('click', function (e) { heard.push([e.isTrusted, e.type, e.target === this, e.currentTarget === this, e.eventPhase, e instanceof up.MouseEvent].join(':')); }); \
         document.addEventListener('click', function (e) { heard.push(['doc', e.isTrusted, e.type, e.target === c, e.currentTarget === document, e.eventPhase].join(':')); }); 0",
    );
    assert_eq!(
        host(
            &t.parent,
            "var c = child.document.getElementById('c'); \
             var e = __rk.trust(__rk.make('MouseEvent', 'click', { bubbles: true, cancelable: true })); \
             var r = c.dispatchEvent(e); \
             return [r, e.isTrusted, e.type, e.target === c, e.currentTarget === null, e.eventPhase].join(':');"
        ),
        "true:true:click:true:true:0"
    );
    assert_eq!(ev(&t.child, "heard.join()"), "true:click:true:true:2:true,doc:true:click:true:true:3");
    // Its one dispatch is spent whichever window dispatches it next.
    assert_eq!(
        host(
            &t.parent,
            "var seen = ''; var p = document.getElementById('p'); \
             var e = __rk.trust(__rk.make('Event', 'ping')); \
             child.document.getElementById('c').dispatchEvent(e); \
             p.addEventListener('ping', function (x) { seen = String(x.isTrusted); }); p.dispatchEvent(e); return seen;"
        ),
        "false"
    );
    // The kind it was made as: the child's activation behaviour runs.
    ev(&t.child, "var box = document.getElementById('i'); box.type = 'checkbox'; 0");
    assert_eq!(
        host(
            &t.parent,
            "var box = child.document.getElementById('i'); \
             box.dispatchEvent(__rk.trust(__rk.make('MouseEvent', 'click', { bubbles: true, cancelable: true }))); \
             return String(box.checked);"
        ),
        "true"
    );
}

/// (b) A page's own event never becomes trusted, in either window: it was
/// not made by the engine, whichever window's entry point is asked and
/// whichever window's `Event` made it, and an engine event's one trust is
/// spent for the whole tree.
#[test]
fn a_child_made_page_event_gains_no_trust_through_either_window() {
    let t = tree();
    ev(
        &t.child,
        "var pe = new Event('click', { bubbles: true }); var pm = new MouseEvent('click', { bubbles: true }); \
         var fake = Object.create(up.Event.prototype); var seen = []; \
         document.getElementById('c').addEventListener('click', function (e) { seen.push('child:' + e.isTrusted); }); 0",
    );
    ev(&t.parent, "var seen = []; document.getElementById('p').addEventListener('click', function (e) { seen.push('parent:' + e.isTrusted); }); 0");
    let refuse = |b: &DomBindings, what: &str| {
        host(b, &format!("try {{ __rk.trust({what}); return 'trusted'; }} catch (e) {{ return 'refused'; }}"))
    };
    for what in ["pe", "pm", "fake"] {
        assert_eq!(refuse(&t.child, what), "refused", "the child's entry point, {what}");
        assert_eq!(refuse(&t.parent, &format!("child.{what}")), "refused", "the parent's entry point, {what}");
    }
    assert_eq!(ev(&t.child, "[pe.isTrusted, pm.isTrusted].join()"), "false,false");
    // Dispatched in each window, by each window's dispatch.
    ev(&t.child, "document.getElementById('c').dispatchEvent(pe); up.document.getElementById('p').dispatchEvent(pe); up.document.getElementById('p').dispatchEvent(pm); 0");
    assert_eq!(ev(&t.child, "seen.join()"), "child:false");
    assert_eq!(ev(&t.parent, "seen.join()"), "parent:false,parent:false");
    // An engine event is trusted once for the tree: made and trusted in
    // the parent, the child's entry point cannot trust it again (not
    // before its dispatch, not after).
    host(&t.parent, "__rk.shared.testEvent = __rk.trust(__rk.make('Event', 'ping')); return '';");
    assert_eq!(refuse(&t.child, "__rk.shared.testEvent"), "refused");
    host(&t.parent, "document.getElementById('p').dispatchEvent(__rk.shared.testEvent); document.getElementById('p').dispatchEvent(__rk.shared.testEvent); return '';");
    assert_eq!(refuse(&t.child, "__rk.shared.testEvent"), "refused");
    assert_eq!(refuse(&t.parent, "__rk.shared.testEvent"), "refused");
    assert_eq!(host(&t.parent, "return String(__rk.shared.testEvent.isTrusted);"), "false");
    // An event one window made and not yet trusted is the tree's to trust.
    host(&t.child, "__rk.shared.testEvent = __rk.make('Event', 'ping'); return '';");
    assert_eq!(refuse(&t.parent, "__rk.shared.testEvent"), "trusted");
    assert_eq!(host(&t.child, "return String(__rk.shared.testEvent.isTrusted);"), "true");
}

/// (c) An event being dispatched cannot be re-initialised or dispatched
/// again through the other window's prototypes: `initEvent` during
/// dispatch does nothing and a nested `dispatchEvent` is an
/// InvalidStateError, whichever realm's method is called (Chrome: the
/// dispatch flag is the event's).
#[test]
fn an_in_flight_event_is_not_retyped_through_the_other_windows_prototype() {
    let t = tree();
    ev(
        &t.child,
        "function retype(e) { try { Event.prototype.initEvent.call(e, 'keydown', false, true); return 'ran'; } catch (x) { return 'threw ' + x.name; } } \
         function retypeCustom(e) { try { CustomEvent.prototype.initCustomEvent.call(e, 'keydown', false, true, 7); return 'ran'; } catch (x) { return 'threw ' + x.name; } } \
         function again(e) { try { document.getElementById('c').dispatchEvent(e); return 'dispatched'; } catch (x) { return x.name; } } 0",
    );
    ev(
        &t.parent,
        "var log = []; var p = document.getElementById('p'); \
         p.addEventListener('click', function (e) { log.push(child.retype(e), child.retypeCustom(e), child.again(e), e.type, e.bubbles, e.cancelable, e.isTrusted); }); \
         document.addEventListener('click', function (e) { log.push('doc ' + e.type + ' ' + e.isTrusted + ' ' + (e.target === p)); }); \
         document.addEventListener('keydown', function (e) { log.push('RETYPED'); }); 0",
    );
    // The engine's trusted click.
    assert_eq!(
        host(
            &t.parent,
            "var e = __rk.trust(__rk.make('MouseEvent', 'click', { bubbles: true, cancelable: false })); \
             __rk.dispatchTo(document.getElementById('p'), e); return e.type + ' ' + e.cancelable;"
        ),
        "click false"
    );
    assert_eq!(ev(&t.parent, "log.join()"), "ran,ran,InvalidStateError,click,true,false,true,doc click true true");
    // A page's own event, the same.
    ev(&t.parent, "log = []; p.dispatchEvent(new MouseEvent('click', { bubbles: true })); 0");
    assert_eq!(ev(&t.parent, "log.join()"), "ran,ran,InvalidStateError,click,true,false,false,doc click false true");
    // And the other way: in flight in the child, the parent's prototype.
    ev(
        &t.child,
        "var log = []; var c = document.getElementById('c'); \
         c.addEventListener('click', function (e) { var r; try { up.Event.prototype.initEvent.call(e, 'keydown', false, true); r = 'ran'; } catch (x) { r = 'threw ' + x.name; } log.push(r, e.type, e.bubbles, e.cancelable); }); \
         document.addEventListener('click', function (e) { log.push('doc ' + e.type); }); \
         c.dispatchEvent(new Event('click', { bubbles: true })); 0",
    );
    assert_eq!(ev(&t.child, "log.join()"), "ran,click,true,false,doc click");
}

/// The embedder's bookkeeping reads (`evaluate_host_bookkeeping`) are
/// given the host object of the document asked, as its turns are: each
/// document's entry points are its own.
#[test]
fn a_bookkeeping_read_holds_the_host_object_of_its_own_document() {
    let t = tree();
    host(&t.parent, "__rk.testMark = 'parent'; return '';");
    host(&t.child, "__rk.testMark = 'child'; return '';");
    let read = |b: &DomBindings| match b.runtime.borrow_mut().evaluate_host_bookkeeping("return __rk.testMark;") {
        Ok(JsValue::String(s)) => s,
        other => format!("{other:?}"),
    };
    assert_eq!(read(&t.child), "child");
    assert_eq!(read(&t.parent), "parent");
}

/// What the lazily installed scripts define is there in a child from the
/// first read, whichever is touched first: `ImageData` (web_canvas.js) and
/// `Intl.Locale`'s likely subtags (the lookup web_intl.js takes at load).
#[test]
fn a_childs_first_read_of_a_lazy_global_installs_it() {
    let t = tree();
    assert_eq!(ev(&t.child, "typeof ImageData + '|' + new ImageData(2, 1).data.length"), ev(&t.parent, "typeof ImageData + '|' + new ImageData(2, 1).data.length"));
    assert_eq!(ev(&t.child, "typeof ImageData"), "function");
    assert_eq!(ev(&t.child, "new Intl.Locale('zh').maximize().toString()"), "zh-Hans-CN");
}
