//! No page function runs inside engine bookkeeping. The engine's reads of
//! its own state (the network queue, the next timer, the clock) use the
//! built-ins of install time and tables only the engine holds, whatever a
//! page has replaced or defined since; and the events the engine makes for
//! the user's input are built from objects that inherit nothing.
//!
//! Every scenario runs on its own thread with a time bound, and every page
//! function here only counts its calls.

use super::*;
use std::time::Duration;

const PAGE: &str = "<!DOCTYPE html><html><head><title>t</title></head><body>\
    <p id=\"p\">text</p><input id=\"i\" value=\"v\"><div id=\"d\" draggable=\"true\">d</div></body></html>";

/// How long a scenario may take before it counts as not having returned.
const BOUND: Duration = Duration::from_secs(60);

/// Run `scenario` on its own thread and fail if it has not returned
/// within [`BOUND`].
fn bounded<T: Send + 'static>(scenario: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(scenario());
    });
    match rx.recv_timeout(BOUND) {
        Ok(value) => value,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => panic!("the scenario did not return within {BOUND:?}"),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => panic!("the scenario panicked"),
    }
}

fn text(v: JsValue) -> String {
    match v {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

fn bindings(net: bool) -> (DomBindings, Rc<Document>) {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    let doc = Rc::new(Document::parse_html(PAGE).unwrap());
    b.set_document(doc.clone()).unwrap();
    b.set_location(&Url::parse("https://example.test/page").unwrap()).unwrap();
    if net {
        b.enable_net_bridge().unwrap();
    }
    (b, doc)
}

/// The page's side of every scenario. `ran` counts calls of the page's
/// functions; `handed` counts the objects a replaced built-in was called
/// with that the page did not make. Whatever the page is handed, it leaves
/// counting accessors on and a counting object in.
const KIT: &str = r#"
var ran = 0, handed = 0, made = new WeakSet([window, document]);
var real = { push: Array.prototype.push, splice: Array.prototype.splice, indexOf: Array.prototype.indexOf,
             slice: Array.prototype.slice, define: Object.defineProperty, max: Math.max, String: String, Number: Number };
var FIELDS = ['id', 'seq', 'due', 'cb', 'args', 'every', 'frame', 'method', 'url', 'headers', 'body_b64', 'mode',
              'credentials', 'redirect', 'destination'];
function counted() { ran++; return 0; }
function counting() {
    var o = {};
    for (var i = 0; i < FIELDS.length; i++) {
        real.define(o, FIELDS[i], { get: counted, set: counted, enumerable: true, configurable: true });
    }
    o.toJSON = counted; o.valueOf = counted; o.toString = counted;
    made.add(o);
    return o;
}
function receive(o) {
    if (o === null || typeof o !== 'object' || made.has(o) || o instanceof Node || o instanceof Event) return;
    handed++;
    for (var i = 0; i < FIELDS.length; i++) {
        try { real.define(o, FIELDS[i], { get: counted, set: counted, enumerable: true, configurable: true }); } catch (e) {}
    }
    try {
        real.define(o, o.length >>> 0, { value: counting(), writable: true, enumerable: true, configurable: true });
    } catch (e) {}
}
function receiveAll(self, args) {
    receive(self);
    for (var i = 0; i < args.length; i++) receive(args[i]);
}
function replaceArrayMethods() {
    Array.prototype.push = function () { receiveAll(this, arguments); return real.push.apply(this, arguments); };
    Array.prototype.splice = function () { receiveAll(this, arguments); return real.splice.apply(this, arguments); };
    Array.prototype.indexOf = function () { receiveAll(this, arguments); return real.indexOf.apply(this, arguments); };
}
function restoreArrayMethods() {
    Array.prototype.push = real.push; Array.prototype.splice = real.splice; Array.prototype.indexOf = real.indexOf;
}
function get(path) { var x = new XMLHttpRequest(); x.open('GET', 'https://example.test/' + path); x.send(); return x; }
"#;

/// `ran` and `handed`, as the page counted them.
fn counts(b: &DomBindings) -> String {
    text(b.evaluate("'ran=' + ran + ' handed=' + handed").unwrap())
}

fn urls(requests: Vec<NetRequest>) -> Vec<String> {
    requests.into_iter().map(|r| r.url).collect()
}

/// A queued request is withdrawn while the page has its own array methods
/// in place: the queue never passes through them, and taking it runs
/// nothing of the page's.
#[test]
fn a_withdrawn_request_leaves_the_queue_private() {
    let (taken, waiting, counts) = bounded(|| {
        let (b, _) = bindings(true);
        b.evaluate(KIT).unwrap();
        b.evaluate("var a = get('a'), keep = get('b'), c = get('c');").unwrap();
        b.evaluate("replaceArrayMethods(); a.abort(); c.abort(); restoreArrayMethods(); ran = 0;").unwrap();
        let taken = urls(b.take_net_requests());
        let waiting = b.pending_net_requests();
        (taken, waiting, counts(&b))
    });
    assert_eq!(
        (taken, waiting, counts.as_str()),
        (vec!["https://example.test/b".to_string()], 1, "ran=0 handed=0"),
        "the requests taken, how many wait, and the page's counts"
    );
}

/// The page defines accessors for the first indices on `Array.prototype`
/// and makes requests: taking the queue runs nothing of the page's, and
/// every request is in it.
#[test]
fn taking_the_queue_runs_no_index_accessor() {
    let (taken, ran) = bounded(|| {
        let (b, _) = bindings(true);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "[0, 1, 2, 3, 4, 5, 6, 7].forEach(function (k) {
                 real.define(Array.prototype, k, {
                     configurable: true,
                     get: function () { ran++; return undefined; },
                     set: function (v) {
                         ran++;
                         real.define(this, k, { value: v, writable: true, enumerable: true, configurable: true });
                         receive(this);
                     }
                 });
             });
             get('a'); get('b'); get('c');",
        )
        .unwrap();
        b.evaluate("ran = 0;").unwrap();
        let taken = urls(b.take_net_requests());
        let _ = b.pending_net_requests();
        (taken, text(b.evaluate("String(ran)").unwrap()))
    });
    let expected: Vec<String> = ["a", "b", "c"].iter().map(|p| format!("https://example.test/{p}")).collect();
    assert_eq!((taken, ran.as_str()), (expected, "0"), "the requests taken, and calls of the page's accessors");
}

/// The next timer's delay is read with the arithmetic of install time.
#[test]
fn the_next_timer_is_read_without_the_pages_math() {
    let (delay, clock, ran) = bounded(|| {
        let (b, _) = bindings(false);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "setTimeout(function () {}, 1000);
             Math.max = counted; Math.min = counted; Math.round = counted; Math.floor = counted; Math.abs = counted;
             ran = 0;",
        )
        .unwrap();
        let delay = b.next_timer_delay();
        let clock = b.clock_now_ms();
        (delay, clock, text(b.evaluate("String(ran)").unwrap()))
    });
    assert_eq!((delay, clock, ran.as_str()), (Some(1000), 0.0, "0"), "the delay, the clock, and calls of the page's functions");
}

/// An interval held back on the real clock keeps its grid, and a turn's
/// frame budget is timed, with the arithmetic and clock of install time: a
/// page whose `Math.floor` returns `NaN` still has its interval run, once,
/// and on its grid, and its `Date.now` is not what ends a turn.
#[test]
fn an_interval_keeps_its_grid_without_the_pages_math_or_clock() {
    let got = bounded(|| {
        let (b, _) = bindings(false);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "var n = 0, realFloor = Math.floor, realNow = Date.now;
             setInterval(function () { n++; }, 10);
             Math.floor = function () { ran++; return NaN; }; Date.now = counted;",
        )
        .unwrap();
        b.advance_real_timers_for_frame(55, 1000, 16).unwrap();
        let next = b.next_timer_delay();
        b.evaluate("Math.floor = realFloor; Date.now = realNow;").unwrap();
        (next.map(|d| d % 10), text(b.evaluate("'n=' + n + ' ran=' + ran").unwrap()))
    });
    assert_eq!(got, (Some(5), "n=1 ran=0".to_string()), "the next run's place on the grid, and the page's counts");
}

/// Animation frames are put on their grid, and a turn's frame budget is
/// timed, with the arithmetic and clock of install time: a page whose
/// `Math.floor` returns 0, or whose `Math` is a plain object, still gets its
/// frames on the grid, and its `Date.now` is not what ends a turn.
#[test]
fn frames_and_the_frame_budget_run_without_the_pages_math_or_clock() {
    let got = bounded(|| {
        let (b, _) = bindings(false);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "var frames = 0, threw = '', realMath = Math, realNow = Date.now;
             Math.floor = counted; Date.now = counted;
             (function f() { requestAnimationFrame(function () { frames++; f(); }); })();",
        )
        .unwrap();
        let first = b.next_timer_delay();
        b.advance_timers_for_frame(17, 1000, 16).unwrap();
        let second = b.next_timer_delay();
        b.evaluate(
            "Math = {};
             try { requestAnimationFrame(function () { frames++; }); } catch (e) { threw = String(e); }",
        )
        .unwrap();
        let ran_round = b.advance_timers_for_frame(17, 1000, 16).unwrap();
        b.evaluate("Math = realMath; Date.now = realNow;").unwrap();
        (first, second, ran_round, text(b.evaluate("'frames=' + frames + ' ran=' + ran + ' threw=' + threw").unwrap()))
    });
    assert_eq!(
        got,
        (Some(17), Some(17), (2, false), "frames=3 ran=0 threw=".to_string()),
        "the first and next frame's delay, the second turn's callbacks, and the page's counts"
    );
}

/// Timers are set and cleared while the page has its own array methods in
/// place: the timer table never passes through them, and reading the next
/// timer runs nothing of the page's.
#[test]
fn setting_and_clearing_timers_leaves_the_table_private() {
    let (delay, counts) = bounded(|| {
        let (b, _) = bindings(false);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "replaceArrayMethods();
             var first = setTimeout(function () {}, 500), second = setTimeout(function () {}, 800, 'x');
             var every = setInterval(function () {}, 900);
             clearTimeout(first); clearInterval(every);
             restoreArrayMethods(); ran = 0;",
        )
        .unwrap();
        let delay = b.next_timer_delay();
        let _ = b.clock_now_ms();
        (delay, counts(&b))
    });
    assert_eq!((delay, counts.as_str()), (Some(800), "ran=0 handed=0"), "the delay, and the page's counts");
}

/// The same while the engine runs a frame's callbacks: a timer that stays
/// set is never handed to the page.
#[test]
fn a_frame_round_leaves_the_table_private() {
    let (ran_callbacks, delay, counts, frames) = bounded(|| {
        let (b, _) = bindings(false);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "var frames = 0;
             requestAnimationFrame(function () { frames++; }); requestAnimationFrame(function () { frames++; });
             setTimeout(function () {}, 700);
             replaceArrayMethods();",
        )
        .unwrap();
        // One callback of the two that are due: the other stays set.
        let ran_callbacks = b.advance_timers(20, 1).unwrap();
        b.evaluate("restoreArrayMethods(); ran = 0;").unwrap();
        let delay = b.next_timer_delay();
        let counts = counts(&b);
        (ran_callbacks, delay, counts, text(b.evaluate("String(frames)").unwrap()))
    });
    assert_eq!(
        (ran_callbacks, delay, counts.as_str(), frames.as_str()),
        (1, Some(0), "ran=0 handed=0", "1"),
        "callbacks run, the delay, the page's counts, frames seen"
    );
}

/// Requests and timers made while the page has its own `String`, `Number`
/// and `Math` in place: what the engine keeps is plain data, and reading
/// it runs nothing of the page's.
#[test]
fn bookkeeping_holds_plain_data_whatever_the_page_replaced() {
    let (taken, delay, ran) = bounded(|| {
        let (b, _) = bindings(true);
        b.evaluate(KIT).unwrap();
        b.evaluate(
            "String = function () { return counting(); }; Number = function () { return counting(); };
             Math.max = function () { return counting(); };
             setTimeout(function () {}, 300); setInterval(function () {}, 400);",
        )
        .unwrap();
        // The bridge's own entry point, as the engine's surfaces call it.
        b.host_turn(
            "return __rk.net.request({ method: 'POST', url: 'https://example.test/a', headers: [['a', 'b']], \
             body_b64: 'eA==' }, function () {});",
            &[],
        )
        .unwrap();
        b.evaluate("String = real.String; Number = real.Number; Math.max = real.max; ran = 0;").unwrap();
        let taken: Vec<(String, String, Option<String>)> =
            b.take_net_requests().into_iter().map(|r| (r.method, r.url, r.body_b64)).collect();
        let delay = b.next_timer_delay();
        (taken, delay, text(b.evaluate("String(ran)").unwrap()))
    });
    assert_eq!(
        (taken, delay, ran.as_str()),
        (vec![("POST".to_string(), "https://example.test/a".to_string(), Some("eA==".to_string()))], Some(300), "0"),
        "the request taken, the delay, and calls of the page's functions"
    );
}

/// The names the engine's input events are described with.
const INIT_NAMES: &str = "bubbles cancelable composed view detail relatedTarget related pointerId pointerType \
    isPrimary pressure deltaX deltaY deltaZ deltaMode wheelDeltaX wheelDeltaY dataTransfer key code repeat keyCode \
    charCode clientX clientY screenX screenY offsetX offsetY movementX movementY pageX pageY which button buttons \
    ctrlKey altKey shiftKey metaKey propertyName animationName elapsedTime pseudoElement";

/// An accessor on `Object.prototype` for each name, counting its calls; a
/// set goes on to define the own property, as a plain assignment would.
const INIT_TRAP: &str = r#"
var calls = 0, which = {}, seen = [];
NAMES.split(/\s+/).forEach(function (name) {
    Object.defineProperty(Object.prototype, name, {
        configurable: true,
        get: function () { calls++; which[name + ':get'] = true; return undefined; },
        set: function (v) {
            calls++; which[name + ':set'] = true;
            Object.defineProperty(this, name, { value: v, writable: true, enumerable: true, configurable: true });
        }
    });
});
['mousedown', 'click', 'keydown', 'wheel', 'dragstart', 'transitionend', 'animationend'].forEach(function (type) {
    document.addEventListener(type, function (e) {
        seen.push(type + ':' + e.isTrusted + ':' + e.bubbles + ':' + e.cancelable + ':' + e.composed);
    });
});
calls = 0; which = {};
"#;

/// The user's mouse, key, wheel and drag input, and the engine's
/// transition and animation events, with an accessor on `Object.prototype`
/// for every name an event is described with: none is invoked, and each
/// event arrives as the engine described it.
#[test]
fn input_events_are_described_by_objects_that_inherit_nothing() {
    let (calls, seen) = bounded(|| {
        let (b, doc) = bindings(false);
        let id = |name: &str| doc.get_element_by_id(name).unwrap().id.raw();
        let (para, input, drag) = (id("p"), id("i"), id("d"));
        b.evaluate(&format!("var NAMES = {INIT_NAMES:?};")).unwrap();
        b.evaluate(INIT_TRAP).unwrap();
        let mouse = MouseEventBindingData { client_x: 5.0, client_y: 5.0, ..Default::default() };
        let key = KeyboardEventBindingData { key: "a".into(), code: "KeyA".into(), ..Default::default() };
        for kind in ["mousedown", "click"] {
            let _ = b.fire_mouse_event(para, kind, &mouse);
        }
        let _ = b.fire_key_event(Some(input), "keydown", &key);
        let _ = b.fire_wheel_event(para, &mouse, 0.0, 10.0, true);
        let _ = b.fire_drag_event(drag, "dragstart", &mouse, "move");
        let _ = b.fire_transition_event(para, None, "transitionend", "opacity", 0.1);
        let _ = b.fire_animation_event(para, None, "animationend", "n", 0.1);
        (
            text(b.evaluate("calls + ' ' + Object.keys(which).sort().join(',')").unwrap()),
            text(b.evaluate("seen.join(' ')").unwrap()),
        )
    });
    assert_eq!(
        (calls.as_str(), seen.as_str()),
        (
            "0 ",
            "mousedown:true:true:true:true click:true:true:true:true keydown:true:true:true:true \
             wheel:true:true:true:true dragstart:true:true:true:true transitionend:true:true:true:false \
             animationend:true:true:false:false"
        ),
        "accessor calls and which, and what the page's listeners saw"
    );
}

/// An engine event's dispatch state is its own from the moment it is
/// made: an accessor the page defines under any key an earlier event
/// carried is not invoked for a later one.
#[test]
fn an_engine_events_dispatch_state_is_its_own() {
    let calls = bounded(|| {
        let (b, doc) = bindings(false);
        let input = doc.get_element_by_id("i").unwrap().id.raw();
        b.evaluate(
            "var calls = 0, known = [];
             document.addEventListener('keydown', function (e) {
                 Object.getOwnPropertySymbols(e).forEach(function (s) {
                     if (known.indexOf(s) >= 0) return;
                     known.push(s);
                     Object.defineProperty(Object.prototype, s, {
                         configurable: true,
                         get: function () { calls++; return undefined; },
                         set: function (v) {
                             calls++;
                             Object.defineProperty(this, s, { value: v, writable: true, enumerable: true, configurable: true });
                         }
                     });
                 });
             });",
        )
        .unwrap();
        let key = KeyboardEventBindingData { key: "a".into(), code: "KeyA".into(), ..Default::default() };
        for _ in 0..3 {
            let _ = b.fire_key_event(Some(input), "keydown", &key);
        }
        text(b.evaluate("String(calls)").unwrap())
    });
    assert_eq!(calls, "0", "accessor calls");
}

/// The fields of an event the engine makes are read from its init record with the own-property
/// helper of install time: they are the same whatever `Object.prototype.hasOwnProperty` and
/// `Function.prototype.call` are when the event is made.
#[test]
fn an_engine_events_fields_are_read_with_the_install_time_own_property_helper() {
    let got = bounded(|| {
        let (b, _doc) = bindings(false);
        b.evaluate_as_page_script(
            "var calls = 0, seen = [], p;
             window.addEventListener('unhandledrejection', function (e) {
                 seen.push([e.type, e.cancelable, e.promise === p, e.reason, e.isTrusted].join(':'));
             });
             Object.prototype.hasOwnProperty = function () { calls++; return false; };
             Function.prototype.call = function () { calls++; return false; };
             p = Promise.reject('r');",
        )
        .unwrap();
        text(b.evaluate("seen.join() + ' calls=' + calls").unwrap())
    });
    assert_eq!(got, "unhandledrejection:true:true:r:true calls=0");
}

/// The load-listener check made when a frame goes into the document reads
/// the tree and the handler attribute from the engine's own records: an
/// ancestor lookup or attribute read the page has replaced is not asked,
/// nor is a set constructor the page put in the built-in's place;
/// the check ends, and its answers are the same. (The check is called
/// here as web_frames.js calls it; an insertion made with these accessors
/// replaced has other readers of the tree.)
#[test]
fn the_load_listener_check_asks_nothing_of_the_page() {
    let (ran, heard) = bounded(|| {
        let (b, _) = bindings(false);
        b.enable_frames().unwrap();
        b.evaluate(
            "var ran = 0, none = function () {};
             var box = document.createElement('div'); box.addEventListener('load', none, true); document.body.appendChild(box);
             var marked = document.createElement('iframe'); marked.setAttribute('onload', '0');
             var plain = document.createElement('iframe'), inner = document.createElement('iframe');
             document.body.appendChild(plain); document.body.appendChild(marked); box.appendChild(inner);
             Object.defineProperty(Node.prototype, 'parentNode', { get: function () { ran++; return this; }, configurable: true });
             Object.defineProperty(Node.prototype, 'parentElement', { get: function () { ran++; return this; }, configurable: true });
             Element.prototype.getAttribute = function () { ran++; return null; };
             Element.prototype.hasAttribute = function () { ran++; return false; };
             window.WeakSet = function () { ran++; this.add = function () { ran++; }; this.has = function () { ran++; return false; }; };",
        )
        .unwrap();
        let heard = b
            .host_turn(
                "var H = __rk.domHooks, d = document; \
                 return [!!H.loadSnapshot(d.querySelectorAll('iframe')[0]), !!H.loadSnapshot(d.querySelectorAll('iframe')[1]), \
                         !!H.loadSnapshot(d.querySelectorAll('iframe')[2])].join();",
                &[],
            )
            .unwrap();
        (text(b.evaluate("String(ran)").unwrap()), text(heard))
    });
    assert_eq!((ran.as_str(), heard.as_str()), ("0", "true,false,true"), "page functions run; the check for the frame under a capturing listener, a plain frame, a frame with the attribute");
}
