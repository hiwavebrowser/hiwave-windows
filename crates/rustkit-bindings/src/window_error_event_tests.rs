//! The window's `error` event for an uncaught exception (HTML "report an exception").
//!
//! The expected answers are the pinned Chrome's (CfT 148.0.7778.216, headless, recorded
//! 2026-11-16) on `docs/diagnostics/2026-11-16/window_error{.html,_page.js}`, in
//! `window_error_cft148.json`; the engine's end-to-end test runs that page
//! (`rustkit-engine` `window_error_event_tests`). Here, each rule alone:
//!   - an uncaught exception fires one trusted, cancelable, non-bubbling `ErrorEvent` at the
//!     Window, at once, with `message` ("Uncaught " and the error), `filename`, `lineno`,
//!     `colno` and `error`;
//!   - `window.onerror` is called with those five and `this` the Window, and `true` back
//!     cancels the event; a listener's `preventDefault()` cancels it too;
//!   - a canceled error is not reported any further (the engine's log), any other once;
//!   - an exception thrown by an `error` handler is reported and fires no second event;
//!   - a promise job's exception is an `unhandledrejection`, never an `error`;
//!   - `reportError(e)` does the same for `e`, and with no argument is a `TypeError`;
//!   - script errors from another origin are reported without detail, as the HTML standard's
//!     muted errors: "Script error.", no file, no position, no value (Chrome rows
//!     `other origin ...`), and their rejections fire no event.
//!
//! The engine says what each script it evaluates is (`register_script`: its URL, and whether its
//! errors are reported without detail); a script it never registered is reported without detail.
//! A script's code carries that, and so does code it makes at run time. What an event says is
//! decided by the code that was running where the exception was thrown, as in Chrome (rows
//! `other origin made function`, `page-made function called by the other origin`, `page ... after`).

use super::*;

const WATCH: &str = "var seen = [], calls = [], rejected = 0; \
    window.addEventListener('error', function (ev) { \
        seen.push([Object.prototype.toString.call(ev), ev.message, ev.filename, ev.lineno, ev.colno, \
                   ev.error instanceof Error ? ev.error.name : String(ev.error), ev.isTrusted, ev.cancelable, ev.bubbles, ev.target === window].join('|')); }); \
    window.addEventListener('unhandledrejection', function () { rejected++; });";

fn bare_page() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(Document::parse_html("<html><head></head><body><button id=b>x</button></body></html>").unwrap()))
        .unwrap();
    b.set_location(&Url::parse("https://example.test/page.html").unwrap()).unwrap();
    b
}

const DOC: &str = "https://example.test/page.html";
/// An event reported without detail, as `WATCH` records one.
const MUTED: &str = "[object ErrorEvent]|Script error.||0|0|null|true|true|false|true";

/// The page's own inline scripts and one external script of its origin, registered as the
/// engine registers them.
fn register_own(b: &DomBindings) {
    for name in ["inline#1", "inline#2", "inline#3"] {
        b.register_script(name, DOC, false);
    }
    b.register_script("https://example.test/a.js", "https://example.test/a.js", false);
}

fn page() -> DomBindings {
    let b = bare_page();
    register_own(&b);
    b.evaluate_named(WATCH, "inline#1").unwrap();
    b
}

/// `page()`, and `theirs`: a script of another origin, evaluated under the engine's own name
/// for it, whose errors are reported without detail.
fn page_with_theirs(source: &str) -> (DomBindings, bool) {
    let b = page();
    b.register_script("script#1", "https://other.test/lib.js", true);
    let threw = b.evaluate_named(source, "script#1").is_err();
    (b, threw)
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

/// Chrome rows `top level`, `listener` (two events by the time `dispatchEvent` returns:
/// `syncAfterDispatch`), `timer`, `frame callback`, `inline handler attribute`.
#[test]
fn an_uncaught_exception_fires_one_error_event_at_the_window() {
    let b = page();
    assert!(b.evaluate_named("throw new Error('from a script');", "https://example.test/a.js").is_err());
    assert_eq!(ev(&b, "seen.join(' ; ')"), "[object ErrorEvent]|Uncaught Error: from a script|https://example.test/a.js|1|7|Error|true|true|false|true");
    b.evaluate_named(
        "seen.length = 0; var btn = document.getElementById('b'); \
         btn.addEventListener('click', function () { throw new TypeError('from a listener'); }); \
         btn.dispatchEvent(new Event('click')); var atOnce = seen.length; \
         setTimeout(function () { throw new RangeError('from a timer'); }, 0); \
         requestAnimationFrame(function () { throw 'a string'; }); \
         btn.setAttribute('onkeydown', 'throw null'); btn.dispatchEvent(new Event('keydown'));",
        "inline#2",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "String(atOnce)"), "1");
    let seen = ev(&b, "seen.map(function (s) { var f = s.split('|'); return [f[1], f[2], f[5]].join('|'); }).join(' ; ')");
    assert_eq!(
        seen,
        "Uncaught TypeError: from a listener|https://example.test/page.html|TypeError ; \
         Uncaught null|https://example.test/page.html|null ; \
         Uncaught RangeError: from a timer|https://example.test/page.html|RangeError ; \
         Uncaught a string|https://example.test/page.html|a string"
    );
    // Each is still reported once.
    assert_eq!(b.take_reported_errors().len(), 4);
}

/// Chrome rows `onerror returns true` (`cancelSeen`), `preventDefault` (`preventSeen`) and the
/// `onerror` list (five arguments, `this` the Window; the handler's place among the listeners is a known difference, below).
#[test]
fn onerror_gets_five_arguments_and_true_back_cancels() {
    let b = page();
    b.evaluate_named(
        "var order = []; \
         window.onerror = function (message, source, lineno, colno, error) { \
             order.push('onerror'); \
             calls.push([arguments.length, message, source, typeof lineno, typeof colno, error && error.name, this === window].join('|')); \
             return message.indexOf('cancel me') >= 0; }; \
         var later = []; \
         window.addEventListener('error', function (ev) { order.push('listener'); later.push(ev.defaultPrevented + '/' + ev.returnValue); \
             if (ev.message.indexOf('prevent me') >= 0) ev.preventDefault(); }); \
         setTimeout(function () { throw new Error('cancel me'); }, 0); \
         setTimeout(function () { throw new Error('prevent me'); }, 1); \
         setTimeout(function () { throw new Error('report me'); }, 2);",
        "inline#2",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(
        ev(&b, "calls.join(' ; ')"),
        "5|Uncaught Error: cancel me|https://example.test/page.html|number|number|Error|true ; \
         5|Uncaught Error: prevent me|https://example.test/page.html|number|number|Error|true ; \
         5|Uncaught Error: report me|https://example.test/page.html|number|number|Error|true"
    );
    // Known difference, not this event's: an `on<type>` handler runs after the target's listeners
    // here, wherever it was set among them (Chrome runs it in its place, so there a listener added
    // after `onerror` sees the cancel: `cancelSeen` is `prevented: true`). The event ends canceled
    // either way, which the report below shows.
    assert_eq!(ev(&b, "later.join() + ' ' + order.slice(0, 2).join()"), "false/true,false/true,false/true listener,onerror");
    assert_eq!(b.take_reported_errors(), vec!["Error: report me".to_string()], "a canceled error is not reported further");
}

/// Chrome `inHandler`: one call. The handler's own exception is reported and fires nothing.
#[test]
fn an_exception_in_an_error_handler_fires_no_second_event() {
    let b = page();
    b.evaluate_named(
        "var handled = 0; window.addEventListener('error', function () { handled++; throw new Error('from the handler'); }); \
         window.onerror = function () { handled += 10; throw new Error('from onerror'); }; \
         setTimeout(function () { throw new Error('first'); }, 0);",
        "inline#2",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "handled + ' ' + seen.length"), "11 1");
    let mut reported = b.take_reported_errors();
    reported.sort();
    assert_eq!(reported, vec!["Error: first", "Error: from onerror", "Error: from the handler"]);
    // The next uncaught exception fires as usual.
    b.evaluate_named("setTimeout(function () { throw new Error('second'); }, 0);", "inline#3").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "handled + ' ' + seen.length"), "22 2");
}

/// Chrome `rejections` and no `promise job` row among the events.
#[test]
fn a_promise_jobs_exception_is_a_rejection_not_an_error() {
    let b = page();
    b.evaluate_named("Promise.resolve().then(function () { throw new Error('from a promise job'); });", "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.length + ' ' + rejected"), "0 1");
}

/// Chrome `reportError`, `reportErrorSync`, `reportErrorSame`, `reportErrorNoArg`.
#[test]
fn report_error_reports_its_argument() {
    let b = page();
    b.evaluate_named(
        "var e = new RangeError('reported'), last; window.addEventListener('error', function (ev) { last = ev.error; }); \
         var kind = typeof self.reportError; reportError(e); var atOnce = seen.length, same = last === e, bare; \
         try { reportError(); bare = 'returned'; } catch (x) { bare = x.name; }",
        "inline#2",
    )
    .unwrap();
    assert_eq!(ev(&b, "[kind, atOnce, same, bare, seen[0].split('|')[1]].join()"), "function,1,true,TypeError,Uncaught RangeError: reported");
}

/// Chrome `pageDispatchCallsOnerror`: an `ErrorEvent` the page dispatches at the Window calls
/// `onerror` with the event's five fields too; any other event calls it with the event.
#[test]
fn onerror_takes_a_page_dispatched_error_events_fields() {
    let b = page();
    b.evaluate_named(
        "window.onerror = function () { calls.push(arguments.length + ':' + Array.prototype.join.call(arguments, '|')); }; \
         window.dispatchEvent(new ErrorEvent('error', { message: 'made by the page', filename: 'f.js', lineno: 3, colno: 4, error: null })); \
         window.dispatchEvent(new Event('error'));",
        "inline#2",
    )
    .unwrap();
    assert_eq!(ev(&b, "calls.join(' ; ')"), "5:made by the page|f.js|3|4| ; 1:[object Event]");
}

/// The event is the engine's own whatever the page has replaced: the constructors, the
/// property helpers and the call helpers of the page are not asked.
#[test]
fn what_the_page_replaced_does_not_change_the_event() {
    let b = bare_page();
    register_own(&b);
    b.evaluate_named(
        "var real = { toString: Object.prototype.toString, proto: ErrorEvent.prototype, apply: Reflect.apply, protoOf: Object.getPrototypeOf }, ran = 0, got = []; \
         window.addEventListener('error', function (ev) { \
             got[0] = real.apply(real.toString, ev, []) + '|' + (real.protoOf(ev) === real.proto) + '|' + ev.message + '|' + ev.isTrusted; }); \
         function no() { ran++; throw new Error('the page was asked'); } \
         window.ErrorEvent = no; window.Event = no; \
         Object.defineProperty = no; Object.create = no; Object.setPrototypeOf = no; \
         Function.prototype.call = no; Function.prototype.apply = no; Function.prototype.bind = no; \
         String.prototype.indexOf = no; String.prototype.slice = no; RegExp.prototype.exec = no; \
         ran = 0; setTimeout(function () { throw new Error('still an event'); }, 0);",
        "inline#2",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    let b2 = &b;
    assert!(matches!(b2.evaluate("ran"), Ok(JsValue::Number(n)) if n == 0.0), "page functions ran");
    assert!(matches!(b2.evaluate("got[0]"), Ok(JsValue::String(s)) if s == "[object ErrorEvent]|true|Uncaught Error: still an event|true"));
}

/// Chrome row `other origin top level`: at the script's top level, an `Error`, an object, a
/// string, and code the script makes with `eval` and `Function`.
#[test]
fn another_origins_top_level_exception_is_reported_without_detail() {
    for source in [
        "throw new Error('theirs');",
        "throw { secret: 1 };",
        "throw 'their string';",
        "eval(\"throw new Error('in eval')\");",
        "new Function(\"throw new Error('in a made function')\")();",
        "eval(\"throw 'a string in eval'\");",
    ] {
        let (b, threw) = page_with_theirs(source);
        assert!(threw, "{source}");
        assert_eq!(ev(&b, "seen.join(' ; ')"), MUTED, "{source}");
    }
}

/// Chrome rows `other origin function`, `other origin eval`, `other origin string`: in a timer
/// and in an event listener, the script's own and the page's that call into it.
#[test]
fn another_origins_exception_in_a_callback_is_reported_without_detail() {
    let theirs = "function theirError() { throw new Error('theirs'); } \
        function theirString() { throw 'their string'; } function theirObject() { throw { secret: 1 }; } \
        function theirEval() { eval(\"throw new Error('in eval')\"); } \
        function theirMade() { new Function(\"throw new Error('in a made function')\")(); } \
        var theirButton = document.getElementById('b');";
    for call in ["theirError", "theirString", "theirObject", "theirEval", "theirMade"] {
        // Their timer, their listener.
        let (b, _) = page_with_theirs(&format!(
            "{theirs} setTimeout({call}, 0); theirButton.addEventListener('click', {call}); theirButton.dispatchEvent(new Event('click'));"
        ));
        b.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&b, "seen.join(' ; ')"), format!("{MUTED} ; {MUTED}"), "their callbacks: {call}");
        // The page's timer and listener, calling their function.
        let (b, _) = page_with_theirs(theirs);
        b.evaluate_named(
            &format!("setTimeout(function () {{ {call}(); }}, 0); document.getElementById('b').addEventListener('click', function () {{ {call}(); }}); \
                      document.getElementById('b').dispatchEvent(new Event('click'));"),
            "inline#2",
        )
        .unwrap();
        b.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&b, "seen.join(' ; ')"), format!("{MUTED} ; {MUTED}"), "the page's callbacks: {call}");
    }
}

/// Chrome `rejections` (the other origin's rejection is not in it): a rejection made by such a
/// script, at its top level and in a reaction that throws again, fires no `unhandledrejection`
/// and no `error`; the engine's own report still has it.
#[test]
fn another_origins_rejection_fires_no_event() {
    let (b, threw) = page_with_theirs(
        "Promise.reject(new Error('their rejection')); \
         Promise.resolve().then(function () { throw new Error('thrown in their reaction'); }); \
         Promise.reject(new Error('caught, then thrown again')).catch(function (e) { throw e; });",
    );
    assert!(!threw);
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.length + ' ' + rejected"), "0 0");
    // The page's own rejection still fires.
    b.evaluate_named("Promise.reject(new Error('ours'));", "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.length + ' ' + rejected"), "0 1");
}

/// A resource of another origin that is not a script at all (markup served for a `src`): its
/// syntax error is reported without detail, with nothing of its text.
#[test]
fn another_origins_syntax_error_is_reported_without_detail() {
    let (b, threw) = page_with_theirs("<!doctype html><title>account 12345</title>");
    assert!(threw);
    assert_eq!(ev(&b, "seen.join(' ; ')"), MUTED);
}

/// A script the engine did not register is one nothing is known of: without detail. So is every
/// script past the table's room (4096), its callbacks included; the page's own code, registered
/// before, is still reported in full.
#[test]
fn an_unregistered_script_is_reported_without_detail() {
    let b = page();
    assert!(b.evaluate_named("throw new Error('unknown');", "https://example.test/never-registered.js").is_err());
    assert_eq!(ev(&b, "seen.join(' ; ')"), MUTED);

    let b = page();
    for n in 0..5000 {
        b.register_script(&format!("script#{n}"), &format!("https://example.test/{n}.js"), false);
    }
    // In the table: reported in full. Past it: not.
    assert!(b.evaluate_named("throw new Error('early');", "script#7").is_err());
    assert!(b.evaluate_named("function late() { throw new Error('late, later'); } function lateString() { throw 'late string'; } throw new Error('late');", "script#4999").is_err());
    b.evaluate_named("setTimeout(function () { late(); }, 0); setTimeout(function () { lateString(); }, 1); setTimeout(function () { throw 'the page'; }, 2);", "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(
        ev(&b, "seen.join(' ; ')"),
        format!(
            "[object ErrorEvent]|Uncaught Error: early|https://example.test/7.js|1|7|Error|true|true|false|true ; {MUTED} ; {MUTED} ; {MUTED} ; \
             [object ErrorEvent]|Uncaught the page|{DOC}|0|0|the page|true|true|false|true"
        )
    );
}

/// Two documents of one tree evaluate different scripts under one name (`script#1` counts from
/// each document's start). Each document's code carries its own script's answer: the parent's
/// `script#1` is another origin's, the frame's `script#1` is the frame's own.
#[test]
fn a_script_name_shared_by_two_documents_of_a_tree_is_each_documents_own() {
    let parent = page();
    let child = parent.new_child().unwrap();
    child
        .set_document(Rc::new(Document::parse_html("<html><head></head><body></body></html>").unwrap()))
        .unwrap();
    child.set_location(&Url::parse("https://example.test/frame.html").unwrap()).unwrap();
    child.register_script("frame-inline#1", "https://example.test/frame.html", false);
    child.evaluate_named(WATCH, "frame-inline#1").unwrap();
    parent.register_script("script#1", "https://other.test/lib.js", true);
    child.register_script("script#1", "https://example.test/frame-own.js", false);
    assert!(child.evaluate_named("throw new Error('the frame own script');", "script#1").is_err());
    assert!(parent.evaluate_named("function theirError() { throw new Error('theirs'); } throw new Error('theirs');", "script#1").is_err());
    assert_eq!(ev(&parent, "seen.join(' ; ')"), MUTED);
    assert_eq!(
        ev(&child, "seen.join(' ; ')"),
        "[object ErrorEvent]|Uncaught Error: the frame own script|https://example.test/frame-own.js|1|7|Error|true|true|false|true"
    );
    // The other origin's function, called by the frame's code across the tree: reported to the
    // frame, without detail.
    parent.expose_window("child", &child).unwrap();
    child.expose_window("up", &parent).unwrap();
    child.evaluate_named("seen.length = 0; setTimeout(function () { up.theirError(); }, 0);", "frame-inline#1").unwrap();
    child.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&child, "seen.join(' ; ')"), MUTED);
}

/// A document with no origin of its own to compare (about:blank here; the engine registers an
/// external script of such a document as reported without detail: `rustkit-engine`
/// `script_error_detail`): the event has no detail and no file.
#[test]
fn a_document_without_an_origin_reports_an_external_script_without_detail() {
    let b = bare_page();
    b.set_location(&Url::parse("about:blank").unwrap()).unwrap();
    b.register_script("inline#1", "about:blank", false);
    b.evaluate_named(WATCH, "inline#1").unwrap();
    b.register_script("script#1", "https://other.test/lib.js", true);
    assert!(b.evaluate_named("throw new Error('theirs');", "script#1").is_err());
    assert_eq!(ev(&b, "seen.join(' ; ')"), MUTED);
}

/// Another origin's script, as the families below use it: each name is code of that script
/// that throws when the page's own callback runs it.
const THEIR_FAMILIES: &str = "
function theirError() { throw new Error('theirs'); }
var theirMade = new Function(\"throw new Error('in a function they made')\");
var theirEvalMade = eval(\"(function () { throw new Error('in a function they made with eval'); })\");
var theirAccessor = { get x() { throw new Error('their getter'); }, set y(v) { throw new Error('their setter'); } };
var theirProxy = new Proxy({}, { get: function () { throw new Error('their trap'); } });
var theirValue = { valueOf: function () { throw new Error('their valueOf'); }, toString: function () { throw new Error('their toString'); } };
var theirBound = theirError.bind(null);
class TheirClass { constructor() { throw new Error('their constructor'); } }
function* theirGenerator() { yield 1; throw new Error('their generator, resumed'); }
function theirNull() { return null.x; }
function theirJson() { return JSON.parse('{'); }
function theirMicrotask() { queueMicrotask(function () { throw new Error('their microtask'); }); }
async function theirAsync() { await null; throw new Error('their async, resumed'); }
function theirReaction() { Promise.resolve().then(function () { throw new Error('their reaction'); }); }
function theirReject() { Promise.reject({ theirs: 1 }); }
function theirCalls(f) { return f(); }
function theirCallsAsync(f) { return Promise.resolve().then(f); }
";

/// The code that was running where the exception was thrown decides, whatever called it. Each
/// of another origin's functions here is run by the page's own timer: its exception is reported
/// without detail (Chrome rows `other origin function`, `... made function`, `... eval-made
/// function`, `... string`, `... object`), by family: plain and bound functions, code made with
/// `Function` and `eval`, accessors, a proxy trap, `valueOf` / `toString`, a class constructor,
/// a resumed generator, the built-ins' own errors raised in their code, a microtask.
#[test]
fn the_code_that_threw_decides_family_by_family() {
    for call in [
        "theirError()",
        "theirMade()",
        "theirEvalMade()",
        "theirAccessor.x",
        "theirAccessor.y = 1",
        "theirProxy.anything",
        "+theirValue",
        "'' + { toString: theirValue.toString }",
        "theirBound()",
        "new TheirClass()",
        "Reflect.construct(TheirClass, [])",
        "var g = theirGenerator(); g.next(); g.next()",
        "theirNull()",
        "theirJson()",
        "[1].forEach(theirError)",
        "theirMicrotask()",
    ] {
        let (b, threw) = page_with_theirs(THEIR_FAMILIES);
        assert!(!threw);
        b.evaluate_named(&format!("setTimeout(function () {{ {call}; }}, 0);"), "inline#2").unwrap();
        b.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&b, "seen.join(' ; ') + ' / ' + rejected"), format!("{MUTED} / 0"), "{call}");
    }
}

/// The same for rejections (Chrome `rejections`: none of the other origin's): their async
/// function resumed after an `await`, their reaction that throws, their own `Promise.reject`,
/// each started by the page's timer, fire no `unhandledrejection`; nor does `rejectionhandled`
/// fire later for one that was never announced.
#[test]
fn a_rejection_is_decided_by_the_code_that_rejected() {
    for call in ["theirAsync()", "theirReaction()", "theirReject()"] {
        let (b, _) = page_with_theirs(THEIR_FAMILIES);
        b.evaluate_named(
            &format!("var handled = 0; window.addEventListener('rejectionhandled', function () {{ handled++; }}); \
                      var kept; setTimeout(function () {{ kept = {call}; }}, 0);"),
            "inline#2",
        )
        .unwrap();
        b.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&b, "seen.length + ' ' + rejected"), "0 0", "{call}");
        b.evaluate_named("if (kept && kept.catch) kept.catch(function () {});", "inline#3").unwrap();
        b.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&b, "seen.length + ' ' + rejected + ' ' + handled"), "0 0 0", "{call}, handled later");
    }
}

/// The controls, with another origin's script on the page (Chrome rows `page function called by
/// the other origin`, `page-made function called by the other origin`, `page string after`,
/// `page object after`, and its `rejections`): the page's own code is reported in full when the
/// other origin's code calls it, when it throws a value that is not an error, and when it
/// rejects with an object, a string, `undefined`, or through a chain with no handler.
#[test]
fn the_pages_own_code_stays_reported_in_full_beside_another_origins() {
    let full = |message: &str, file: &str, line: u32, column: u32, error: &str| {
        format!("[object ErrorEvent]|Uncaught {message}|{file}|{line}|{column}|{error}|true|true|false|true")
    };
    let (b, _) = page_with_theirs(THEIR_FAMILIES);
    b.register_script("script#2", "https://example.test/own.js", false);
    assert!(b.evaluate_named("function ours() { throw new TypeError('ours, later'); } throw new Error('ours');", "script#2").is_err());
    assert!(b.evaluate_named("throw new RangeError('inline');", "inline#2").is_err());
    b.evaluate_named(
        "var made = new Function(\"throw new Error('in a function the page made')\"); \
         setTimeout(ours, 0); \
         setTimeout(function () { theirCalls(ours); }, 1); \
         setTimeout(function () { theirCalls(made); }, 2); \
         setTimeout(function () { throw 'a string of ours'; }, 3); \
         setTimeout(function () { throw { ours: 1 }; }, 4);",
        "inline#3",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(
        ev(&b, "seen.join(' ; ')"),
        [
            full("Error: ours", "https://example.test/own.js", 1, 63, "Error"),
            full("RangeError: inline", DOC, 1, 7, "RangeError"),
            full("TypeError: ours, later", "https://example.test/own.js", 1, 25, "TypeError"),
            full("TypeError: ours, later", "https://example.test/own.js", 1, 25, "TypeError"),
            full("Error: in a function the page made", DOC, 0, 0, "Error"),
            full("a string of ours", DOC, 0, 0, "a string of ours"),
            full("[object Object]", DOC, 0, 0, "[object Object]"),
        ]
        .join(" ; ")
    );
    // Rejections: each fires, with its reason.
    b.evaluate_named(
        "var reasons = []; window.addEventListener('unhandledrejection', function (ev) { \
             var r = ev.reason; reasons.push(r instanceof Error ? r.message : typeof r === 'object' && r !== null ? JSON.stringify(r) : String(r)); }); \
         setTimeout(function () { Promise.reject({ status: 404 }); Promise.reject('a string reason'); Promise.reject(undefined); \
             Promise.reject(new Error('through a chain')).then(function () {}); \
             theirCallsAsync(function () { throw new Error('ours, in a reaction they queued'); }); \
             (async function () { await null; throw new Error('ours, resumed'); })(); }, 0);",
        "inline#3",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(
        ev(&b, "reasons.join(' | ')"),
        "{\"status\":404} | a string reason | undefined | through a chain | ours, in a reaction they queued | ours, resumed"
    );
}

/// Another origin's script whose calls make the browser itself raise an error.
const THEIR_DOM: &str = "
function theirName() { document.createElement('their ' + 'name'); }
function theirHierarchy() { document.body.appendChild(document.documentElement); }
function theirArgument() { document.body.appendChild(null); }
function theirTimer() { setTimeout(theirName, 0); }
function theirListens(el) { el.addEventListener('pong', theirName); }
async function theirAsyncDom() { await null; document.createElement('their name'); }
function theirExecutorDom() { new Promise(function () { document.createElement('their name'); }); }
function theirReactionDom() { Promise.resolve().then(function () { document.createElement('their name'); }); }
function theirCombinators() { Promise.all([Promise.reject(new Error('their all'))]); Promise.reject(new Error('their finally')).finally(function () {}); }
function theirDispatches(el) { el.dispatchEvent(new Event('ping')); }
function theirStringTimer() { setTimeout(\"throw new Error('in a string timer they set')\", 0); }
function theirAttribute() { var d = document.createElement('div'); d.setAttribute('onclick', \"throw new Error('in a handler attribute they set')\"); document.body.appendChild(d); d.dispatchEvent(new Event('click')); }
function theirInline() { var s = document.createElement('script'); s.textContent = \"throw new Error('in an inline script they inserted')\"; document.body.appendChild(s); }
";

/// An error the engine raises for a call is the caller's: for another origin's script it is
/// reported without detail, wherever the call is made from (an invalid element name here, an
/// invalid selector in the probe; Chrome rows `other origin dom top
/// level`, `... dom function`, `... dom hierarchy`, `... dom timer`, `... dom listener`; and none of
/// its `rejections`): its top level, a function the page's timer calls, its own timer, its own
/// listener, its async function after an `await`, its promise executor, its reaction, and the
/// promise combinators it calls.
#[test]
fn an_error_the_engine_raises_for_another_origins_call_is_reported_without_detail() {
    let (b, threw) = page_with_theirs("document.createElement('their ' + 'top name');");
    assert!(threw);
    assert_eq!(ev(&b, "seen.join(' ; ')"), MUTED, "top level");
    for call in ["theirName()", "theirHierarchy()", "theirArgument()"] {
        let (b, _) = page_with_theirs(THEIR_DOM);
        b.evaluate_named(&format!("setTimeout(function () {{ {call}; }}, 0);"), "inline#2").unwrap();
        b.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&b, "seen.join(' ; ') + ' / ' + rejected"), format!("{MUTED} / 0"), "{call}");
    }
    let (b, _) = page_with_theirs(&format!("{THEIR_DOM} theirTimer(); theirListens(document.getElementById('b'));"));
    b.evaluate_named("document.getElementById('b').dispatchEvent(new Event('pong'));", "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.join(' ; ')"), format!("{MUTED} ; {MUTED}"), "their listener, their timer");
    let (b, _) = page_with_theirs(THEIR_DOM);
    b.evaluate_named("setTimeout(function () { theirAsyncDom(); theirExecutorDom(); theirReactionDom(); theirCombinators(); }, 0);", "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.length + ' ' + rejected"), "0 0", "their rejections");
}

/// The same calls by the page's own code on that page are reported in full (Chrome rows `page
/// dom after`, `page dom hierarchy after`), and so is the page's listener when the other origin's
/// code dispatches the event (`page listener run for the other origin`).
#[test]
fn an_error_the_engine_raises_for_the_pages_call_is_reported_in_full() {
    let (b, _) = page_with_theirs(THEIR_DOM);
    b.evaluate_named(
        "setTimeout(function () { document.createElement('page ' + 'name'); }, 0); \
         setTimeout(function () { document.body.appendChild(document.documentElement); }, 1); \
         document.getElementById('b').addEventListener('ping', function () { throw new Error('the page listener'); }); \
         setTimeout(function () { theirDispatches(document.getElementById('b')); }, 2);",
        "inline#2",
    )
    .unwrap();
    b.advance_timers(20, 10).unwrap();
    let got = ev(&b, "seen.map(function (s) { var f = s.split('|'); return [f[1].split(':')[0], f[2], f[5]].join('|'); }).join(' ; ')");
    assert_eq!(
        got,
        format!("Uncaught InvalidCharacterError|{DOC}|InvalidCharacterError ; Uncaught HierarchyRequestError|{DOC}|HierarchyRequestError ; Uncaught Error|{DOC}|Error")
    );
}

/// Code the standard gives to the document, whoever made it (Chrome rows `other origin string
/// timer`, `other origin attribute`, `other origin inline script`: each in full): a handler
/// attribute, an inline script and a string given to `setTimeout`, set by another origin's code.
#[test]
fn a_handler_attribute_an_inline_script_and_a_string_timer_are_the_documents() {
    let (b, _) = page_with_theirs(THEIR_DOM);
    b.evaluate_named("setTimeout(function () { theirAttribute(); theirInline(); theirStringTimer(); }, 0);", "inline#2").unwrap();
    b.advance_timers(40, 10).unwrap();
    let got = ev(&b, "seen.map(function (s) { var f = s.split('|'); return [f[1], f[2], f[5]].join('|'); }).join(' ; ')");
    assert_eq!(
        got,
        format!("Uncaught Error: in a handler attribute they set|{DOC}|Error ; Uncaught Error: in an inline script they inserted|{DOC}|Error ; \
                 Uncaught Error: in a string timer they set|{DOC}|Error")
    );
}

/// Document text that fails to compile leaves later engine code unchanged.
#[test]
fn document_text_that_fails_leaves_the_engines_later_code_its_own() {
    const THEIR_STREAM: &str = "function theirLocked() { var s = new ReadableStream(); s.getReader(); s.getReader(); }";
    for before in [
        "setTimeout('((', 0);",
        "setTimeout({ toString: function () { window.asked = true; return ''; } }, 0);",
        "var d = document.createElement('div'); d.setAttribute('onclick', '(('); document.body.appendChild(d); d.dispatchEvent(new Event('click'));",
        "var s = document.createElement('script'); s.textContent = '(('; document.body.appendChild(s);",
    ] {
        let parent = page();
        let child = parent.new_child().unwrap();
        child
            .set_document(Rc::new(Document::parse_html("<html><head></head><body></body></html>").unwrap()))
            .unwrap();
        child.set_location(&Url::parse("https://example.test/frame.html").unwrap()).unwrap();
        child.register_script("frame-inline#1", "https://example.test/frame.html", false);
        child.evaluate_named(WATCH, "frame-inline#1").unwrap();
        child.register_script("script#1", "https://other.test/lib.js", true);
        child.evaluate_named(THEIR_STREAM, "script#1").unwrap();
        parent.evaluate_named(before, "inline#2").unwrap();
        parent.advance_timers(20, 10).unwrap();
        child
            .evaluate_named("var first = new ReadableStream(); setTimeout(function () { theirLocked(); }, 0);", "frame-inline#1")
            .unwrap();
        parent.advance_timers(20, 10).unwrap();
        assert_eq!(ev(&child, "seen.join(' ; ')"), MUTED, "after: {before}");
    }
}

/// Errors from host-run script (`evaluate`: the app's own scripts, a console's input) are reported
/// to the host only: no `error` or `unhandledrejection` event at the page, and the host's own
/// report whatever the page's handlers do.
#[test]
fn errors_from_host_run_script_are_reported_to_the_host_only() {
    let b = page();
    b.evaluate_named("window.onerror = function () { return true; }; \
                      window.addEventListener('unhandledrejection', function (ev) { ev.preventDefault(); });", "inline#2").unwrap();
    let _ = b.take_reported_errors();
    b.evaluate("Promise.reject(new Error('the host rejects')); setTimeout(function () { throw new Error('the host callback'); }, 0);").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.length + ' / ' + rejected"), "0 / 0");
    assert_eq!(b.take_reported_errors(), vec!["Error: the host callback".to_string()]);
    // The page's own code beside it is unchanged (and its handler does cancel its own).
    b.evaluate_named("setTimeout(function () { throw new Error('the page'); }, 0);", "inline#3").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "seen.length + ' ' + seen[0].split('|')[1]"), "1 Uncaught Error: the page");
    assert!(b.take_reported_errors().is_empty());
}

/// The engine's compile of document text takes a string and nothing else, and keeps no state:
/// handed anything else it throws, and what the engine compiles next is still its own.
#[test]
fn the_document_text_natives_take_a_string_and_leave_nothing() {
    let b = page();
    let got = b
        .host_turn(
            "var out = []; \
             [function () { __rk.runDocumentScript(5); }, function () { __rk.runDocumentScript({ toString: function () { return '1'; } }); }, \
              function () { __rk.compileDocumentFunction('a', 5); }, function () { __rk.compileDocumentFunction(5, 'return 1'); }, \
              function () { __rk.runDocumentScript('(('); }, function () { __rk.compileDocumentFunction('a', '(('); }] \
                 .forEach(function (f) { try { f(); out.push('returned'); } catch (e) { out.push(e.name); } }); \
             out.push(typeof __rk.compileDocumentFunction('a,b', 'return a + b'), __rk.compileDocumentFunction('a,b', 'return a + b')(2, 3)); \
             __rk.runDocumentScript('var ranAsScript = 7;'); out.push(globalThis.ranAsScript); \
             return out.join();",
            &[],
        )
        .unwrap();
    assert_eq!(format!("{got:?}"), r#"String("TypeError,TypeError,TypeError,TypeError,SyntaxError,SyntaxError,function,5,7")"#);
}

/// The page's own rejections that reach the page through the engine's promise machinery
/// (Chrome `rejections`, the `one origin` and `mixed` rows: every one fires): `Promise.all`,
/// `race`, `any`, a `finally` that passes the reason on, a thenable that rejects, an `await` of
/// a rejected promise; with and without another origin's script on the page.
#[test]
fn rejections_passed_on_by_the_promise_machinery_are_announced() {
    // `Promise.any`'s own error is named, not quoted: its message is the engine's wording.
    const COMBINATORS: &str = "var reasons = []; window.addEventListener('unhandledrejection', function (ev) { \
            reasons.push(ev.reason instanceof AggregateError ? 'AggregateError' : ev.reason.message); }); \
        setTimeout(function () { \
            Promise.all([Promise.reject(new Error('all'))]); \
            Promise.race([Promise.reject(new Error('race'))]); \
            Promise.any([Promise.reject(new Error('any'))]); \
            Promise.allSettled([Promise.reject(new Error('allSettled, handled'))]); \
            Promise.reject(new Error('finally')).finally(function () {}); \
            Promise.resolve({ then: function (resolve, reject) { reject(new Error('thenable')); } }); \
            (async function () { await Promise.reject(new Error('awaited')); })(); }, 0);";
    let want = "AggregateError,all,awaited,finally,race,thenable";
    let b = page();
    b.evaluate_named(COMBINATORS, "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "reasons.sort().join()"), want, "one origin");
    let (b, _) = page_with_theirs(THEIR_DOM);
    b.evaluate_named(COMBINATORS, "inline#2").unwrap();
    b.advance_timers(20, 10).unwrap();
    assert_eq!(ev(&b, "reasons.sort().join()"), want, "beside another origin's script");
}

/// A dynamic `import()` that fails rejects with the engine's own error about a URL the importing
/// code asked for (Chrome `rejections`: "TypeError: a failed import", on both pages): announced
/// to the page when the page's code imported, not when another origin's code did.
#[test]
fn a_failed_import_is_announced_to_the_code_that_imported() {
    let run = |theirs: bool| {
        let b = page();
        if theirs {
            b.register_script("script#1", "https://other.test/lib.js", true);
        }
        b.evaluate_named("var failed = []; window.addEventListener('unhandledrejection', function (ev) { failed.push(ev.reason && ev.reason.name); });", "inline#2").unwrap();
        b.evaluate_named("import('https://example.test/missing.js');", if theirs { "script#1" } else { "inline#3" }).unwrap();
        for requested in b.take_module_requests() {
            b.supply_module(&requested, Err("status 404".to_string()));
        }
        b.advance_timers(20, 10).unwrap();
        ev(&b, "failed.join() + '/' + seen.length")
    };
    assert_eq!(run(false), "TypeError/0");
    assert_eq!(run(true), "/0");
}

/// What an `error` listener rejects after a script failed is announced in the same turn.
#[test]
fn a_rejection_made_by_an_error_listener_is_announced_in_the_same_turn() {
    let b = page();
    b.evaluate_named("window.addEventListener('error', function () { Promise.reject(new Error('from the listener')); });", "inline#2").unwrap();
    assert!(b.evaluate_named("throw new Error('first');", "inline#3").is_err());
    assert_eq!(ev(&b, "seen.length + ' ' + rejected"), "1 1");
}

/// What a script's code says of its errors and the mark of a comparator
/// that is `a - b` (the engine's sort shortcut) are separate: host code,
/// a page's script and another origin's script all sort with `a - b`,
/// `b - a` and a comparator that is neither exactly as written, and a
/// page's own `a - b` function that throws is reported in full.
#[test]
fn a_scripts_error_detail_and_the_sort_shortcut_do_not_mix() {
    const SORTS: &str = "(function () { var calls = 0; return [[3, 1, 2, 10].sort(function (a, b) { return a - b; }).join(), \
        [3, 1, 2, 10].sort(function (a, b) { return b - a; }).join(), [3, 1, 2, 10].sort(function () { calls++; return 0; }).join(), \
        [3, 1, 2, 10].sort(function (a, b) { calls++; return (b - a) + 0; }).join(), calls > 3].join('|'); })()";
    const SORTED: &str = "1,2,3,10|10,3,2,1|3,1,2,10|10,3,2,1|true";
    let b = page();
    assert_eq!(ev(&b, SORTS), SORTED, "host code");
    b.evaluate_named(&format!("window.ours = {SORTS};"), "inline#2").unwrap();
    assert_eq!(ev(&b, "window.ours"), SORTED, "the page's script");
    b.register_script("script#1", "https://other.test/lib.js", true);
    b.evaluate_named(&format!("window.theirs = {SORTS};"), "script#1").unwrap();
    assert_eq!(ev(&b, "window.theirs"), SORTED, "another origin's script");
    b.evaluate_named("setTimeout(function () { var difference = function (a, b) { return a - b; }; difference(Symbol(), 1); }, 0);", "inline#3").unwrap();
    b.advance_timers(20, 10).unwrap();
    let message = ev(&b, "seen.length + ' ' + seen[0].split('|')[1]");
    assert!(message.starts_with("1 Uncaught TypeError"), "{message}");
}

/// An error the page cancels is among the embedder's error reports as it
/// would be had the page left it alone, with one mark more: nothing else
/// is added to what a report says, for the page's own script and for
/// another origin's (whose report holds what it held before: shapes and
/// the engine's own name for the script, no text).
#[test]
fn a_cancelled_errors_report_differs_from_an_uncancelled_one_only_by_its_mark() {
    for (name, url, muted) in [("inline#2", DOC, false), ("script#1", "https://other.test/lib.js", true)] {
        let reports = |cancel: bool| {
            let b = page();
            b.set_error_details(true, false).unwrap();
            if cancel {
                b.evaluate_named("window.onerror = function () { return true; };", "inline#9").unwrap();
            }
            b.register_script(name, url, muted);
            let _ = b.take_error_reports();
            let _ = b.evaluate_named("setTimeout(function () { throw new RangeError('in a timer'); }, 0); throw new TypeError('at the top');", name);
            b.advance_timers(20, 10).unwrap();
            (b.take_error_reports(), b.take_reported_errors())
        };
        let (plain, plain_texts) = reports(false);
        let (cancelled, cancelled_texts) = reports(true);
        assert_eq!(plain.len(), 2, "{name}: {plain:?}");
        assert!(plain.iter().all(|report| !report.canceled) && cancelled.iter().all(|report| report.canceled), "{name}");
        let unmarked: Vec<_> = cancelled.iter().cloned().map(|report| rustkit_js::ErrorReport { canceled: false, ..report }).collect();
        assert_eq!(unmarked, plain, "{name}");
        assert!(plain.iter().all(|report| report.message.is_none()), "{name}: no text is kept by default");
        // The embedder's own text log is as before: a cancelled error is not in it.
        assert_eq!((plain_texts.len(), cancelled_texts.len()), (1, 0), "{name}");
    }
}

