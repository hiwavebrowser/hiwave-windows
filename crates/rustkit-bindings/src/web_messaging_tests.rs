//! Task-queue messaging in the page-script environment: window.postMessage
//! to the same window, MessageChannel/MessagePort, the MessageEvent
//! constructor, and requestIdleCallback. What matters most is order and
//! "never synchronous": a message is a task on the same queue timers use,
//! so it runs after the posting script and its microtasks, in post order,
//! interleaved with `setTimeout(0)` by when each was queued. React's
//! scheduler drives its work loop through MessageChannel when present.
//! Its own test module so it does not collide with the other families'
//! tests in `lib.rs`.

use super::*;

fn bound() -> DomBindings {
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings
        .set_document(Rc::new(
            Document::parse_html("<!DOCTYPE html><html><body><p>x</p></body></html>").unwrap(),
        ))
        .unwrap();
    bindings
        .set_location(&Url::parse("https://app.test/dir/page.html?q=1").unwrap())
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

/// Run every task due now on the virtual clock.
fn run_tasks(bindings: &DomBindings) {
    bindings.run_timers(0, 1_000).unwrap();
}

#[test]
fn messages_run_after_the_script_and_its_microtasks_in_post_order_with_timers() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var log = []; var ch = new MessageChannel(); \
             addEventListener('message', function (e) { log.push('w:' + e.data); }); \
             ch.port2.onmessage = function (e) { log.push('p:' + e.data); }; \
             setTimeout(function () { log.push('t0'); }, 0); \
             postMessage('a', '*'); \
             ch.port1.postMessage('1'); \
             setTimeout(function () { log.push('t1'); }, 0); \
             postMessage('b', '*'); \
             ch.port1.postMessage('2'); \
             Promise.resolve().then(function () { log.push('micro'); }); \
             queueMicrotask(function () { log.push('qm'); }); \
             log.push('sync'); log.join()"
        ),
        "sync",
        "nothing is delivered synchronously"
    );
    assert_eq!(
        ev(&b, "log.join()"),
        "sync,micro,qm",
        "microtasks run when the script ends; messages wait for a task"
    );
    run_tasks(&b);
    assert_eq!(ev(&b, "log.join()"), "sync,micro,qm,t0,w:a,p:1,t1,w:b,p:2");
}

#[test]
fn a_message_posted_from_a_task_runs_after_tasks_already_queued() {
    let b = bound();
    b.evaluate(
        "var log = []; var ch = new MessageChannel(); \
         ch.port2.onmessage = function (e) { log.push('m' + e.data); \
             if (e.data < 3) ch.port1.postMessage(e.data + 1); }; \
         setTimeout(function () { log.push('t'); ch.port1.postMessage(1); \
             setTimeout(function () { log.push('t2'); }, 0); log.push('t-end'); }, 0); \
         setTimeout(function () { log.push('u'); }, 0);",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(ev(&b, "log.join()"), "t,t-end,u,m1,t2,m2,m3");
}

#[test]
fn window_post_message_clones_and_fills_the_message_event() {
    let b = bound();
    b.evaluate(
        "var got = null, viaHandler = 0; var payload = { n: 1, list: [1, 2], when: new Date(5) }; \
         addEventListener('message', function (e) { got = e; }); \
         onmessage = function () { viaHandler++; }; \
         postMessage(payload, '*'); payload.n = 2; payload.list.push(3);",
    )
    .unwrap();
    assert_eq!(ev(&b, "String(got)"), "null");
    run_tasks(&b);
    assert_eq!(
        ev(
            &b,
            "[got instanceof MessageEvent, got instanceof Event, got.type, got.data !== payload, \
              got.data.n, got.data.list.join('-'), got.data.when instanceof Date, got.data.when.getTime(), \
              got.origin, got.source === window, got.lastEventId, Array.isArray(got.ports), got.ports.length, \
              got.target === window, got.bubbles, got.cancelable, viaHandler].join()"
        ),
        "true,true,message,true,1,1-2,true,5,https://app.test,true,,true,0,true,false,false,1"
    );
}

#[test]
fn target_origin_is_checked_and_a_mismatch_is_silently_dropped() {
    let b = bound();
    b.evaluate(
        "var got = []; addEventListener('message', function (e) { got.push(e.data); }); \
         postMessage('star', '*'); \
         postMessage('slash', '/'); \
         postMessage('same', 'https://app.test'); \
         postMessage('same-path', 'https://app.test/other/x.html?y#z'); \
         postMessage('other-host', 'https://evil.test'); \
         postMessage('other-scheme', 'http://app.test'); \
         postMessage('other-port', 'https://app.test:8443'); \
         postMessage('opts', { targetOrigin: 'https://app.test' }); \
         postMessage('opts-default', {}); \
         postMessage('opts-bad', { targetOrigin: 'https://evil.test' }); \
         postMessage('no-origin');",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(
        ev(&b, "got.join()"),
        "star,slash,same,same-path,opts,opts-default,no-origin"
    );
}

#[test]
fn post_message_throws_for_bad_origins_uncloneable_data_and_no_arguments() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var r = [], got = 0; addEventListener('message', function () { got++; }); \
             try { postMessage('x', 'not a url'); } catch (e) { r.push(e.name, e instanceof DOMException); } \
             try { postMessage('x', { targetOrigin: 'app.test' }); } catch (e) { r.push(e.name); } \
             try { postMessage(function () {}, '*'); } catch (e) { r.push(e.name); } \
             try { postMessage({ s: Symbol('s') }, '*'); } catch (e) { r.push(e.name); } \
             try { postMessage(); } catch (e) { r.push(e.name); } \
             r.join()"
        ),
        "SyntaxError,true,SyntaxError,DataCloneError,DataCloneError,TypeError"
    );
    run_tasks(&b);
    assert_eq!(ev(&b, "String(got)"), "0", "a throwing post queues nothing");
}

#[test]
fn an_opaque_origin_document_only_accepts_star_and_slash() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.evaluate(
        "var got = []; addEventListener('message', function (e) { got.push(e.data + ':' + e.origin); }); \
         postMessage('star', '*'); postMessage('slash', '/'); postMessage('named', 'https://app.test');",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(ev(&b, "got.join()"), "star:null,slash:null");
}

#[test]
fn message_channel_ports_are_entangled_and_deliver_in_order_both_ways() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var ch = new MessageChannel(), log = [], ev2 = null; \
             var r = [ch.port1 instanceof MessagePort, ch.port2 instanceof MessagePort, \
                      ch.port1 instanceof EventTarget, ch.port1 !== ch.port2, \
                      ch.port1 === ch.port1, String(ch)]; \
             ch.port2.onmessage = function (e) { log.push('2<' + e.data.v); ev2 = e; }; \
             ch.port1.onmessage = function (e) { log.push('1<' + e.data); }; \
             var obj = { v: 'a' }; ch.port1.postMessage(obj); obj.v = 'changed'; \
             ch.port1.postMessage({ v: 'b' }); ch.port2.postMessage('x'); ch.port1.postMessage({ v: 'c' }); \
             r.push(log.length); r.join()"
        ),
        "true,true,true,true,true,[object MessageChannel],0"
    );
    run_tasks(&b);
    assert_eq!(ev(&b, "log.join()"), "2<a,2<b,1<x,2<c");
    assert_eq!(
        ev(
            &b,
            "[ev2 instanceof MessageEvent, ev2.type, ev2.target === ch.port2, ev2.origin, \
              String(ev2.source), ev2.lastEventId, ev2.ports.length].join()"
        ),
        "true,message,true,,null,,0"
    );
    assert_eq!(
        ev(
            &b,
            "var r = []; try { MessagePort(); } catch (e) { r.push(e.name); } \
             try { new MessagePort(); } catch (e) { r.push(e.name); } \
             try { MessageChannel(); } catch (e) { r.push(e.name); } \
             try { ch.port1.postMessage(function () {}); } catch (e) { r.push(e.name); } \
             r.join()"
        ),
        "TypeError,TypeError,TypeError,DataCloneError"
    );
}

#[test]
fn add_event_listener_waits_for_start_and_onmessage_starts_the_port() {
    let b = bound();
    b.evaluate(
        "var log = [], ch = new MessageChannel(); \
         ch.port2.addEventListener('message', function (e) { log.push(e.data); }); \
         ch.port1.postMessage(1); ch.port1.postMessage(2);",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(
        ev(&b, "log.join()"),
        "",
        "an unstarted port holds its messages"
    );
    b.evaluate("ch.port1.postMessage(3); ch.port2.start(); ch.port2.start(); ch.port1.postMessage(4); log.push('sync');")
        .unwrap();
    run_tasks(&b);
    assert_eq!(ev(&b, "log.join()"), "sync,1,2,3,4");

    b.evaluate(
        "var log2 = [], ch2 = new MessageChannel(); \
         ch2.port1.postMessage('early'); \
         ch2.port2.onmessage = function (e) { log2.push(e.data); }; \
         ch2.port1.postMessage('late'); \
         var h = ch2.port2.onmessage; ch2.port2.onmessage = null; ch2.port2.onmessage = h;",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(ev(&b, "log2.join()"), "early,late");
}

#[test]
fn close_disentangles_and_drops_pending_and_later_messages() {
    let b = bound();
    b.evaluate(
        "var log = [], ch = new MessageChannel(); \
         ch.port1.onmessage = function (e) { log.push('1<' + e.data); }; \
         ch.port2.onmessage = function (e) { log.push('2<' + e.data); }; \
         ch.port1.postMessage('pending'); ch.port2.close(); \
         ch.port1.postMessage('after'); ch.port2.postMessage('from-closed'); \
         var ch3 = new MessageChannel(); ch3.port2.onmessage = function (e) { log.push('3<' + e.data); }; \
         ch3.port1.close(); ch3.port1.close(); ch3.port1.postMessage('x');",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(ev(&b, "log.join()"), "");
}

#[test]
fn ports_transfer_through_post_message_and_arrive_in_event_ports() {
    let b = bound();
    b.evaluate(
        "var log = [], ch = new MessageChannel(), carrier = new MessageChannel(), wev = null; \
         addEventListener('message', function (e) { wev = e; \
             e.ports[0].onmessage = function (m) { log.push('w-port<' + m.data); }; }); \
         postMessage('hi', '*', [ch.port2]); \
         carrier.port2.onmessage = function (e) { log.push('carrier:' + e.data + ':' + e.ports.length); \
             e.ports[0].postMessage('back'); }; \
         var extra = new MessageChannel(); \
         extra.port1.onmessage = function (m) { log.push('extra<' + m.data); }; \
         carrier.port1.postMessage('take', { transfer: [extra.port2] }); \
         ch.port1.postMessage('ping');",
    )
    .unwrap();
    run_tasks(&b);
    assert_eq!(
        ev(
            &b,
            "[wev.data, wev.ports.length, wev.ports[0] === ch.port2].join()"
        ),
        "hi,1,true"
    );
    assert_eq!(
        ev(&b, "log.join()"),
        "carrier:take:1,w-port<ping,extra<back"
    );
    assert_eq!(
        ev(
            &b,
            "var r = [], c = new MessageChannel(); \
             try { c.port1.postMessage('x', [c.port1]); } catch (e) { r.push(e.name); } \
             try { postMessage('x', '*', [c.port1, c.port1]); } catch (e) { r.push(e.name); } \
             try { postMessage(c.port1, '*'); } catch (e) { r.push(e.name); } \
             r.join()"
        ),
        "DataCloneError,DataCloneError,DataCloneError"
    );
}

#[test]
fn message_event_constructor_takes_every_init_member() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var p = new MessageChannel().port1, data = { a: 1 }; \
             var e = new MessageEvent('message', { data: data, origin: 'https://o.test', lastEventId: '7', \
                 source: window, ports: [p], bubbles: true }); \
             var d = new MessageEvent('custom'); \
             [e instanceof Event, e.type, e.data === data, e.origin, e.lastEventId, e.source === window, \
              e.ports.length, e.ports[0] === p, e.bubbles, \
              d.type, String(d.data), d.origin, d.lastEventId, String(d.source), d.ports.length, \
              typeof e.initMessageEvent, String(MessageEvent.prototype)].join()"
        ),
        "true,message,true,https://o.test,7,true,1,true,true,custom,null,,,null,0,function,[object MessageEvent]"
    );
    assert_eq!(
        ev(
            &b,
            "var r = []; try { MessageEvent('message'); } catch (e) { r.push(e.name); } \
             try { new MessageEvent(); } catch (e) { r.push(e.name); } \
             try { new MessageEvent('message', { ports: [{}] }); } catch (e) { r.push(e.name); } \
             r.join()"
        ),
        "TypeError,TypeError,TypeError"
    );
}

#[test]
fn idle_callbacks_run_after_the_tasks_due_now_with_a_deadline() {
    let b = bound();
    b.evaluate(
        "var log = [], dl = null, t = -1, ch = new MessageChannel(); \
         ch.port2.onmessage = function (e) { log.push('m' + e.data); \
             if (e.data < 2) ch.port1.postMessage(e.data + 1); }; \
         requestIdleCallback(function (d) { dl = d; t = d.timeRemaining(); log.push('idle-a'); }); \
         setTimeout(function () { log.push('t0'); }, 0); \
         ch.port1.postMessage(1); \
         requestIdleCallback(function () { log.push('idle-b'); \
             requestIdleCallback(function () { log.push('idle-next'); }); }); \
         setTimeout(function () { log.push('t1'); }, 0); log.push('sync');",
    )
    .unwrap();
    assert_eq!(ev(&b, "log.join()"), "sync");
    run_tasks(&b);
    assert_eq!(
        ev(&b, "log.join()"),
        "sync,t0,m1,t1,m2,idle-a,idle-b,idle-next"
    );
    assert_eq!(
        ev(
            &b,
            "[dl instanceof IdleDeadline, dl.didTimeout, t > 0, t <= 50].join()"
        ),
        "true,false,true,true"
    );
}

#[test]
fn the_idle_timeout_fires_when_the_loop_stays_busy() {
    let b = bound();
    b.evaluate(
        "var log = [], n = 0; \
         function busy() { var end = Date.now() + 3; while (Date.now() < end) {} \
             log.push('busy' + n); if (++n < 10) setTimeout(busy, 0); } \
         setTimeout(busy, 0); \
         requestIdleCallback(function (d) { log.push('idle:' + d.didTimeout + ':' + d.timeRemaining()); }, { timeout: 1 }); \
         requestIdleCallback(function (d) { log.push('patient:' + d.didTimeout); });",
    )
    .unwrap();
    run_tasks(&b);
    let log = ev(&b, "log.join()");
    let idle = log.find("idle:true:0").expect(&log);
    let last_busy = log.find("busy9").expect(&log);
    assert!(
        idle < last_busy,
        "the timed-out callback runs while the loop is busy: {log}"
    );
    assert!(
        log.ends_with("busy9,patient:false"),
        "the callback without a timeout waits for idle: {log}"
    );
}

#[test]
fn idle_callbacks_with_a_timeout_run_on_the_virtual_clock_and_cancel() {
    let b = bound();
    b.evaluate(
        "var log = []; \
         var a = requestIdleCallback(function (d) { log.push('a:' + d.didTimeout); }, { timeout: 500 }); \
         var c = requestIdleCallback(function () { log.push('cancelled'); }, { timeout: 100 }); \
         cancelIdleCallback(c); cancelIdleCallback(12345); cancelIdleCallback('x'); \
         var r = [typeof a, a > 0, a !== c]; \
         try { requestIdleCallback(null); } catch (e) { r.push(e.name); }",
    )
    .unwrap();
    assert_eq!(ev(&b, "r.join()"), "number,true,true,TypeError");
    b.run_timers(1_000, 1_000).unwrap();
    assert_eq!(
        ev(&b, "log.join()"),
        "a:false",
        "runs once, idle, not again at its timeout"
    );
}
