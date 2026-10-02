//! Blob, File, FormData, AbortController/AbortSignal, structuredClone
//! (web_blob.js). Its own test module so it does not collide with the other
//! families' tests in `lib.rs`.

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

/// Blob was `ReferenceError: Blob is not defined` on two live sites.
#[test]
fn blob_holds_bytes_slices_and_reads_back_asynchronously() {
    let b = bindings();
    assert_eq!(ev(&b, "var bl = new Blob(['hello, ', new Uint8Array([119, 111]), 'rld'], { type: 'Text/Plain' }); bl.size + ':' + bl.type"), "12:text/plain");
    assert_eq!(ev(&b, "new Blob().size + ',' + new Blob([]).type + ',' + new Blob(['x'], { type: 'bad\\u0001' }).type.length"), "0,,0");
    // slice: negative indexes count from the end, the type is the argument.
    assert_eq!(ev(&b, "var sl = bl.slice(-5, -2, 'a/b'); sl.size + ':' + sl.type"), "3:a/b");
    assert_eq!(ev(&b, "bl.slice(3).size + ',' + bl.slice(5, 2).size + ',' + bl.slice(0, 99).size"), "9,0,12");
    // Blobs nest.
    assert_eq!(ev(&b, "new Blob([bl, bl]).size"), "24");
    // text() and arrayBuffer() resolve on the microtask queue.
    b.evaluate("var got = []; bl.text().then(function (t) { got.push(t); }); bl.slice(0, 5).arrayBuffer().then(function (a) { got.push(a.byteLength + ':' + new Uint8Array(a)[0]); });").unwrap();
    assert_eq!(ev(&b, "got.join('|')"), "hello, world|5:104");
    // Not iterable is a TypeError, and `new` is required.
    assert_eq!(ev(&b, "var e1; try { new Blob(5); e1 = 'no throw'; } catch (e) { e1 = e.name; } e1"), "TypeError");
    assert_eq!(ev(&b, "var e2; try { Blob([]); e2 = 'no throw'; } catch (e) { e2 = e.name; } e2"), "TypeError");
    assert_eq!(ev(&b, "Object.prototype.toString.call(bl)"), "[object Blob]");
}

#[test]
fn file_is_a_named_blob() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var f = new File(['abc'], 'a/b.txt', { type: 'text/plain', lastModified: 42 }); [f.name, f.size, f.type, f.lastModified, f instanceof Blob, f instanceof File].join('|')"),
        "a:b.txt|3|text/plain|42|true|true"
    );
    assert_eq!(ev(&b, "var e; try { new File(['x']); e = 'no throw'; } catch (x) { e = x.name; } e"), "TypeError");
}

#[test]
fn form_data_keeps_ordered_entries_and_wraps_blobs_as_files() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var fd = new FormData(); fd.append('a', 1); fd.append('b', 'x'); fd.append('a', 2); \
                [fd.get('a'), fd.getAll('a').join('/'), fd.has('b'), String(fd.get('nope')), Array.from(fd.keys()).join()].join('|')"),
        "1|1/2|true|null|a,b,a"
    );
    assert_eq!(ev(&b, "fd.set('a', 'only'); fd.delete('b'); Array.from(fd.entries()).map(function (e) { return e.join('='); }).join('&')"), "a=only");
    // A Blob appended becomes a File named 'blob' (or the filename given).
    assert_eq!(ev(&b, "var fd2 = new FormData(); fd2.append('f', new Blob(['zz'])); fd2.append('g', new Blob(['y']), 'n.bin'); [fd2.get('f').name, fd2.get('f') instanceof File, fd2.get('g').name, fd2.get('g').size].join('|')"), "blob|true|n.bin|1");
    assert_eq!(ev(&b, "var seen = []; fd2.forEach(function (v, k) { seen.push(k); }); seen.join()"), "f,g");
    assert_eq!(ev(&b, "var n = 0; for (var kv of new FormData()) n++; n"), "0");
}

#[test]
fn abort_controller_aborts_its_signal_once_and_notifies_listeners() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var c = new AbortController(); var log = []; \
                c.signal.addEventListener('abort', function () { log.push('listener'); }); \
                c.signal.onabort = function () { log.push('onabort'); }; \
                var before = c.signal.aborted; c.abort(); c.abort(); \
                [before, c.signal.aborted, log.join(), c.signal.reason.name].join('|')"),
        "false|true|onabort,listener|AbortError"
    );
    assert_eq!(ev(&b, "var c2 = new AbortController(); c2.abort('why'); c2.signal.reason"), "why");
    assert_eq!(ev(&b, "var t; try { c2.signal.throwIfAborted(); t = 'no throw'; } catch (e) { t = String(e); } t"), "why");
    assert_eq!(ev(&b, "var s = AbortSignal.abort(); [s.aborted, s.reason.name].join()"), "true,AbortError");
    // once listeners fire once; removeEventListener works; the constructor is not callable.
    assert_eq!(ev(&b, "var c3 = new AbortController(), n = 0; c3.signal.addEventListener('abort', function () { n++; }, { once: true }); c3.abort(); c3.abort(); n"), "1");
    assert_eq!(ev(&b, "var c4 = new AbortController(), m = 0, h = function () { m++; }; c4.signal.addEventListener('abort', h); c4.signal.removeEventListener('abort', h); c4.abort(); m"), "0");
    assert_eq!(ev(&b, "var i; try { new AbortSignal(); i = 'no throw'; } catch (e) { i = e.name; } i"), "TypeError");
    // AbortSignal.any follows the first to abort.
    assert_eq!(ev(&b, "var a1 = new AbortController(), a2 = new AbortController(); var any = AbortSignal.any([a1.signal, a2.signal]); a2.abort('two'); [any.aborted, any.reason].join()"), "true,two");
}

#[test]
fn abort_signal_timeout_fires_on_the_timer_clock() {
    let b = bindings();
    b.evaluate("var ts = AbortSignal.timeout(50); var fired = []; ts.addEventListener('abort', function () { fired.push(ts.reason.name); });").unwrap();
    assert_eq!(ev(&b, "String(ts.aborted)"), "false");
    b.run_timers(1_000, 100).unwrap();
    assert_eq!(ev(&b, "ts.aborted + ':' + fired.join()"), "true:TimeoutError");
}

#[test]
fn structured_clone_copies_deeply_keeps_cycles_and_rejects_the_uncloneable() {
    let b = bindings();
    assert_eq!(
        ev(&b, "var src = { n: 1, s: 'x', d: new Date(5), r: /a+/gi, arr: [1, [2, 3]], m: new Map([[1, { k: 2 }]]), st: new Set([1, 2]), u8: new Uint8Array([7, 8]) }; \
                var cp = structuredClone(src); \
                [cp !== src, cp.arr !== src.arr, cp.arr[1][1], cp.d instanceof Date && cp.d.getTime(), cp.r.source + cp.r.flags, cp.m.get(1).k, cp.st.has(2), cp.u8[1], cp.u8.buffer !== src.u8.buffer].join('|')"),
        "true|true|3|5|a+gi|2|true|8|true"
    );
    // The copy is independent.
    assert_eq!(ev(&b, "cp.arr[1][0] = 99; src.arr[1][0]"), "2");
    // Cycles and shared references survive.
    assert_eq!(ev(&b, "var cyc = { name: 'c' }; cyc.self = cyc; cyc.list = [cyc]; var cc = structuredClone(cyc); [cc.self === cc, cc.list[0] === cc, cc !== cyc].join()"), "true,true,true");
    // Errors keep their name and message.
    assert_eq!(ev(&b, "var er = structuredClone(new RangeError('boom')); [er instanceof RangeError, er.message].join()"), "true,boom");
    // Functions and symbols throw DataCloneError.
    assert_eq!(ev(&b, "var f1; try { structuredClone({ f: function () {} }); f1 = 'no throw'; } catch (e) { f1 = e.name; } f1"), "DataCloneError");
    assert_eq!(ev(&b, "var f2; try { structuredClone(Symbol('s')); f2 = 'no throw'; } catch (e) { f2 = e.name; } f2"), "DataCloneError");
    assert_eq!(ev(&b, "var f3; try { structuredClone(); f3 = 'no throw'; } catch (e) { f3 = e.name; } f3"), "TypeError");
    // Primitives pass through.
    assert_eq!(ev(&b, "[structuredClone(5), structuredClone('s'), structuredClone(null), String(structuredClone(undefined))].join()"), "5,s,,undefined");
}
