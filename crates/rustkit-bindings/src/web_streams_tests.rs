//! Streams (web_streams.js). Its own test module so it does not collide with
//! the other families' tests in `lib.rs`. Promise reactions run when the
//! enclosing evaluation returns, so each test starts the work in one
//! evaluation and reads the outcome in the next.

use super::*;

fn bindings() -> DomBindings {
    DomBindings::new(JsRuntime::new().unwrap()).unwrap()
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

/// shopify and x died on `ReferenceError: ReadableStream is not defined`.
#[test]
fn a_stream_fed_in_start_reads_in_order_then_ends() {
    let b = bindings();
    b.evaluate(
        "var log = [], closedLog = []; \
         var rs = new ReadableStream({ start: function (c) { c.enqueue('a'); c.enqueue('b'); c.close(); } }); \
         var rd = rs.getReader(); \
         rd.read().then(function (r) { log.push(r.value + ':' + r.done); return rd.read(); }) \
           .then(function (r) { log.push(r.value + ':' + r.done); return rd.read(); }) \
           .then(function (r) { log.push(String(r.value) + ':' + r.done); }); \
         rd.closed.then(function () { closedLog.push('closed'); });",
    )
    .unwrap();
    // The reads arrive in order; `closed` is a separate chain and settles once.
    assert_eq!(ev(&b, "log.join('|') + '#' + closedLog.join()"), "a:false|b:false|undefined:true#closed");
    assert_eq!(ev(&b, "String(rs.locked) + ',' + Object.prototype.toString.call(rs)"), "true,[object ReadableStream]");
}

#[test]
fn pull_runs_until_the_high_water_mark_and_again_as_chunks_are_read() {
    let b = bindings();
    b.evaluate(
        "var pulls = 0, n = 0; \
         var rs = new ReadableStream({ pull: function (c) { pulls++; c.enqueue(++n); if (n === 4) c.close(); } }, { highWaterMark: 2 });",
    )
    .unwrap();
    // With nothing reading, pull fills the queue to the mark and stops.
    assert_eq!(ev(&b, "pulls + ':' + n"), "2:2");
    b.evaluate("var rd = rs.getReader(), got = []; (function loop() { return rd.read().then(function (r) { if (r.done) return; got.push(r.value); return loop(); }); })();").unwrap();
    assert_eq!(ev(&b, "got.join() + ':' + pulls"), "1,2,3,4:4");
}

#[test]
fn errors_cancel_and_locking_behave() {
    let b = bindings();
    // controller.error rejects reads and `closed`; enqueue after close throws.
    b.evaluate(
        "var res = []; var rc; var rs = new ReadableStream({ start: function (c) { rc = c; } }); var rd = rs.getReader(); \
         var p = rd.read().then(function () { res.push('resolved'); }, function (e) { res.push('read rejected:' + e); }); \
         rd.closed.catch(function (e) { res.push('closed rejected:' + e); }); rc.error('boom');",
    )
    .unwrap();
    assert_eq!(ev(&b, "res.sort().join('|')"), "closed rejected:boom|read rejected:boom");
    assert_eq!(ev(&b, "var t; try { new ReadableStream({ start: function (c) { c.close(); try { c.enqueue(1); } catch (e) { t = e.name; } } }); } catch (e) { t = 'outer'; } String(t)"), "TypeError");
    // cancel reaches the source with the reason, and later reads end.
    b.evaluate("var seen = []; var cs = new ReadableStream({ cancel: function (r) { seen.push('cancel:' + r); } }); var cr = cs.getReader(); cr.cancel('why').then(function () { seen.push('done'); }); cr.read().then(function (r) { seen.push('read:' + r.done); });").unwrap();
    assert_eq!(ev(&b, "seen.slice().sort().join('|')"), "cancel:why|done|read:true");
    // Locking: a second reader throws, releaseLock frees the stream and rejects a pending read.
    assert_eq!(ev(&b, "var ls = new ReadableStream(); var r1 = ls.getReader(); var e1; try { ls.getReader(); e1 = 'no throw'; } catch (e) { e1 = e.name; } e1"), "TypeError");
    b.evaluate("var pend = []; r1.read().then(function () { pend.push('ok'); }, function (e) { pend.push('rejected'); }); r1.releaseLock();").unwrap();
    assert_eq!(ev(&b, "pend.join() + ':' + ls.locked"), "rejected:false");
    assert_eq!(ev(&b, "var e2; try { ls.getReader({ mode: 'byob' }); e2 = 'no throw'; } catch (e) { e2 = e.name; } e2"), "NotSupportedError");
}

#[test]
fn tee_async_iteration_and_from_work() {
    let b = bindings();
    b.evaluate(
        "var out = {}; var src = new ReadableStream({ start: function (c) { c.enqueue(1); c.enqueue(2); c.close(); } }); \
         var pair = src.tee(); \
         function drain(s, key) { var r = s.getReader(), got = []; return (function loop() { return r.read().then(function (x) { if (x.done) { out[key] = got.join(); return; } got.push(x.value); return loop(); }); })(); } \
         drain(pair[0], 'a'); drain(pair[1], 'b');",
    )
    .unwrap();
    assert_eq!(ev(&b, "out.a + '|' + out.b"), "1,2|1,2");
    b.evaluate(
        "var iter = []; (async function () { for await (var v of new ReadableStream({ start: function (c) { c.enqueue('x'); c.enqueue('y'); c.close(); } })) iter.push(v); iter.push('end'); })(); \
         var from = []; (async function () { for await (var v of ReadableStream.from([7, 8, 9])) from.push(v); })();",
    )
    .unwrap();
    assert_eq!(ev(&b, "iter.join() + '|' + from.join()"), "x,y,end|7,8,9");
}

#[test]
fn a_writable_stream_runs_the_sink_in_order_and_reports_state() {
    let b = bindings();
    b.evaluate(
        "var sunk = [], ev2 = []; \
         var ws = new WritableStream({ start: function () { sunk.push('start'); }, write: function (c) { sunk.push('w:' + c); return new Promise(function (r) { r(); }); }, close: function () { sunk.push('close'); } }, { highWaterMark: 2 }); \
         var w = ws.getWriter(); var ds0 = w.desiredSize; \
         w.write('a'); w.write('b'); var ds1 = w.desiredSize; \
         w.write('c').then(function () { ev2.push('c written'); }); \
         w.close().then(function () { ev2.push('closed'); }); w.closed.then(function () { ev2.push('closed promise'); });",
    )
    .unwrap();
    assert_eq!(ev(&b, "sunk.join()"), "start,w:a,w:b,w:c,close");
    assert_eq!(ev(&b, "ev2.slice().sort().join('|') + ':' + ds0 + ',' + ds1"), "c written|closed|closed promise:2,0");
    assert_eq!(ev(&b, "String(ws.locked) + ',' + Object.prototype.toString.call(ws)"), "true,[object WritableStream]");
    // A throwing sink errors the stream and rejects the write and later writes.
    b.evaluate("var errs = []; var ws2 = new WritableStream({ write: function () { throw 'bad'; } }); var w2 = ws2.getWriter(); w2.write(1).catch(function (e) { errs.push('write:' + e); }); w2.closed.catch(function (e) { errs.push('closed:' + e); });").unwrap();
    assert_eq!(ev(&b, "errs.sort().join('|')"), "closed:bad|write:bad");
    // abort reaches the sink; writing after close rejects with TypeError.
    b.evaluate("var ab = []; var ws3 = new WritableStream({ abort: function (r) { ab.push('abort:' + r); } }); ws3.getWriter().abort('stop').then(function () { ab.push('done'); }); var ws4 = new WritableStream(); var w4 = ws4.getWriter(); w4.close(); w4.write(1).catch(function (e) { ab.push(e.name); });").unwrap();
    assert_eq!(ev(&b, "ab.slice().sort().join('|')"), "TypeError|abort:stop|done");
}

#[test]
fn pipe_through_a_transform_stream_with_flush() {
    let b = bindings();
    b.evaluate(
        "var piped = []; \
         var upper = new TransformStream({ transform: function (chunk, c) { c.enqueue(String(chunk).toUpperCase()); }, flush: function (c) { c.enqueue('!'); } }); \
         var source = new ReadableStream({ start: function (c) { c.enqueue('a'); c.enqueue('b'); c.close(); } }); \
         var reader = source.pipeThrough(upper).getReader(); \
         (function loop() { return reader.read().then(function (r) { if (r.done) { piped.push('end'); return; } piped.push(r.value); return loop(); }); })();",
    )
    .unwrap();
    assert_eq!(ev(&b, "piped.join()"), "A,B,!,end");
    // pipeTo a WritableStream, and an identity TransformStream.
    b.evaluate(
        "var sink = []; var id = new TransformStream(); \
         var ws = new WritableStream({ write: function (c) { sink.push(c); }, close: function () { sink.push('closed'); } }); \
         new ReadableStream({ start: function (c) { c.enqueue(1); c.enqueue(2); c.close(); } }).pipeThrough(id).pipeTo(ws).then(function () { sink.push('pipe done'); });",
    )
    .unwrap();
    assert_eq!(ev(&b, "sink.join()"), "1,2,closed,pipe done");
}

#[test]
fn queuing_strategies_and_argument_checks() {
    let b = bindings();
    assert_eq!(ev(&b, "var rc; new ReadableStream({ start: function (c) { rc = c; } }, new CountQueuingStrategy({ highWaterMark: 3 })); String(rc.desiredSize)"), "3");
    assert_eq!(ev(&b, "var rc2; new ReadableStream({ start: function (c) { rc2 = c; c.enqueue(new Uint8Array(4)); } }, new ByteLengthQueuingStrategy({ highWaterMark: 10 })); String(rc2.desiredSize)"), "6");
    assert_eq!(ev(&b, "var e1; try { ReadableStream(); e1 = 'no throw'; } catch (e) { e1 = e.name; } e1"), "TypeError");
    assert_eq!(ev(&b, "var e2; try { new ReadableStream({}, { highWaterMark: -1 }); e2 = 'no throw'; } catch (e) { e2 = e.name; } e2"), "RangeError");
    assert_eq!(ev(&b, "var e3; try { new ReadableStreamDefaultController(); e3 = 'no throw'; } catch (e) { e3 = e.name; } e3"), "TypeError");
    assert_eq!(ev(&b, "[typeof ReadableStream, typeof WritableStream, typeof TransformStream, typeof CountQueuingStrategy].join()"), "function,function,function,function");
}

/// shopify's last throw after ReadableStream: `TextEncoderStream is not defined`.
#[test]
fn text_encoder_and_decoder_streams_round_trip_across_chunk_boundaries() {
    let b = bindings();
    b.evaluate(
        "var enc = []; \
         var src = new ReadableStream({ start: function (c) { c.enqueue('h\\u00e9'); c.enqueue('\\u20ac!'); c.close(); } }); \
         var r = src.pipeThrough(new TextEncoderStream()).getReader(); \
         (function loop() { return r.read().then(function (x) { if (x.done) return; enc.push(Array.from(x.value).join('.')); return loop(); }); })();",
    )
    .unwrap();
    assert_eq!(ev(&b, "enc.join('|')"), "104.195.169|226.130.172.33");
    // A euro sign (E2 82 AC) split 2 + 1 across chunks decodes once whole.
    b.evaluate(
        "var dec = []; \
         var bytes = new ReadableStream({ start: function (c) { c.enqueue(new Uint8Array([0x61, 0xE2, 0x82])); c.enqueue(new Uint8Array([0xAC, 0x62])); c.close(); } }); \
         var dr = bytes.pipeThrough(new TextDecoderStream()).getReader(); \
         (function loop() { return dr.read().then(function (x) { if (x.done) return; dec.push(x.value); return loop(); }); })();",
    )
    .unwrap();
    assert_eq!(ev(&b, "String(dec.join('|') === 'a|\\u20acb')"), "true");
    assert_eq!(
        ev(&b, "var t = new TextDecoderStream('utf-8', { fatal: true }); [new TextEncoderStream().encoding, t.encoding, t.fatal, t.ignoreBOM, typeof t.readable.getReader, typeof t.writable.getWriter].join()"),
        "utf-8,utf-8,true,false,function,function"
    );
}
