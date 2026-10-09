//! Script utility globals: crypto, queueMicrotask, and the corners of
//! TextEncoder/TextDecoder, URL/URLSearchParams and structuredClone that the
//! family tests in `lib.rs` and `web_blob_tests.rs` do not pin.

use super::*;

fn bindings() -> DomBindings {
    DomBindings::new(JsRuntime::new().unwrap()).unwrap()
}

fn ev(bindings: &DomBindings, script: &str) -> String {
    match bindings.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(b) => b.to_string(),
        JsValue::Number(n) => {
            if n.fract() == 0.0 {
                format!("{}", n as i64)
            } else {
                n.to_string()
            }
        }
        JsValue::Null => "null".to_string(),
        JsValue::Undefined => "undefined".to_string(),
        other => panic!("{script} evaluated to {other:?}"),
    }
}

/// `crypto` was `ReferenceError: crypto is not defined`; uuid and nanoid
/// bundles call it on load.
#[test]
fn crypto_fills_integer_arrays_and_makes_v4_uuids() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "typeof crypto + ',' + typeof crypto.getRandomValues + ',' + typeof crypto.randomUUID"
        ),
        "object,function,function"
    );
    // getRandomValues fills in place and returns the same array.
    assert_eq!(
        ev(
            &b,
            "var a = new Uint8Array(64); var r = crypto.getRandomValues(a); r === a"
        ),
        "true"
    );
    assert_eq!(
        ev(&b, "Array.from(a).some(function (x) { return x !== 0; })"),
        "true"
    );
    assert_eq!(ev(&b, "var w = new Uint32Array(8); crypto.getRandomValues(w); w.some(function (x) { return x > 255; })"), "true");
    // A sub-view is filled only within its window.
    assert_eq!(ev(&b, "var whole = new Uint8Array(32); crypto.getRandomValues(whole.subarray(8, 24)); whole.slice(0, 8).every(function (x) { return x === 0; }) && whole.slice(24).every(function (x) { return x === 0; })"), "true");
    assert_eq!(
        ev(&b, "crypto.getRandomValues(new BigUint64Array(2)).length"),
        "2"
    );
    // Float arrays are TypeMismatchError; over 65536 bytes is QuotaExceededError.
    assert_eq!(ev(&b, "var t; try { crypto.getRandomValues(new Float32Array(2)); t = 'no throw'; } catch (e) { t = e.name; } t"), "TypeMismatchError");
    assert_eq!(ev(&b, "var q; try { crypto.getRandomValues(new Uint8Array(65537)); q = 'no throw'; } catch (e) { q = e.name; } q"), "QuotaExceededError");
    assert_eq!(
        ev(&b, "crypto.getRandomValues(new Uint8Array(65536)).length"),
        "65536"
    );
    // randomUUID is a lower-case RFC 9562 version 4 UUID, fresh each call.
    assert_eq!(ev(&b, "/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID())"), "true");
    assert_eq!(ev(&b, "var seen = new Set(); for (var i = 0; i < 50; i++) seen.add(crypto.randomUUID()); seen.size"), "50");
    assert_eq!(
        ev(&b, "Object.prototype.toString.call(crypto)"),
        "[object Crypto]"
    );
}

#[test]
fn queue_microtask_runs_after_the_script_and_rejects_non_callables() {
    let b = bindings();
    assert_eq!(
        ev(
            &b,
            "var q = []; queueMicrotask(function () { q.push('m'); }); q.push('s'); q.join()"
        ),
        "s"
    );
    assert_eq!(ev(&b, "q.join()"), "s,m");
    assert_eq!(
        ev(
            &b,
            "var e1; try { queueMicrotask(1); e1 = 'no throw'; } catch (e) { e1 = e.name; } e1"
        ),
        "TypeError"
    );
    assert_eq!(
        ev(
            &b,
            "var e2; try { queueMicrotask(); e2 = 'no throw'; } catch (e) { e2 = e.name; } e2"
        ),
        "TypeError"
    );
}

#[test]
fn url_href_setter_throws_and_statics_check_their_arguments() {
    let b = bindings();
    // The href setter is the one setter that throws on a bad value.
    assert_eq!(ev(&b, "var u = new URL('https://a.test/'); var h; try { u.href = 'not a url'; h = 'no throw'; } catch (e) { h = e.name; } h + ',' + u.href"), "TypeError,https://a.test/");
    assert_eq!(
        ev(
            &b,
            "var c; try { URL.canParse(); c = 'no throw'; } catch (e) { c = e.name; } c"
        ),
        "TypeError"
    );
    assert_eq!(
        ev(
            &b,
            "var p; try { URL.parse(); p = 'no throw'; } catch (e) { p = e.name; } p"
        ),
        "TypeError"
    );
    assert_eq!(ev(&b, "[URL.canParse('/x', 'https://a.test'), URL.canParse('/x'), URL.parse('nope'), URL.parse('/p', 'https://a.test/').href].join()"), "true,false,,https://a.test/p");
}

#[test]
fn url_search_params_stay_linked_to_their_url() {
    let b = bindings();
    assert_eq!(ev(&b, "var u = new URL('https://a.test/?x=1#h'); u.searchParams.append('y', '2 3'); u.searchParams.set('x', '0'); u.href"), "https://a.test/?x=0&y=2+3#h");
    assert_eq!(
        ev(
            &b,
            "u.searchParams.delete('x'); u.searchParams.delete('y'); u.href"
        ),
        "https://a.test/#h"
    );
    assert_eq!(ev(&b, "var sp = u.searchParams; u.search = '?q=9'; sp.get('q') + ',' + (sp === u.searchParams)"), "9,true");
    assert_eq!(
        ev(
            &b,
            "u.href = 'https://b.test/?k=v'; sp.get('k') + ',' + sp.size"
        ),
        "v,1"
    );
    // The two-argument delete and has, and sort by code units.
    assert_eq!(ev(&b, "var p = new URLSearchParams('a=1&a=2&b=3'); p.delete('a', '2'); [p.toString(), p.has('a', '1'), p.has('a', '2')].join()"), "a=1&b=3,true,false");
    assert_eq!(
        ev(&b, "new URLSearchParams('a=b=c&d&%zz=%41').toString()"),
        "a=b%3Dc&d=&%25zz=A"
    );
}

#[test]
fn the_utility_interfaces_have_their_to_string_tags() {
    let b = bindings();
    let tag = |expr: &str| ev(&b, &format!("Object.prototype.toString.call({expr})"));
    assert_eq!(tag("new TextEncoder()"), "[object TextEncoder]");
    assert_eq!(tag("new TextDecoder()"), "[object TextDecoder]");
    assert_eq!(tag("new URL('https://a.test/')"), "[object URL]");
    assert_eq!(tag("new URLSearchParams()"), "[object URLSearchParams]");
    assert_eq!(
        tag("new URLSearchParams('a=1').entries()"),
        "[object URLSearchParams Iterator]"
    );
}

#[test]
fn structured_clone_shares_buffers_and_rejects_platform_objects() {
    let b = bindings();
    // Views over one buffer share one cloned buffer.
    assert_eq!(ev(&b, "var buf = new ArrayBuffer(8); var c = structuredClone({ a: new Uint8Array(buf, 0, 2), b: new Uint16Array(buf, 2, 3) }); [c.a.buffer === c.b.buffer, c.b.length, c.b.byteOffset, c.a.buffer !== buf].join()"), "true,3,2,true");
    assert_eq!(ev(&b, "var dv = structuredClone(new DataView(new ArrayBuffer(6), 2, 3)); dv.byteOffset + ',' + dv.byteLength"), "2,3");
    assert_eq!(
        ev(
            &b,
            "var m = new Map(); m.set(m, m); var mc = structuredClone(m); mc.get(mc) === mc"
        ),
        "true"
    );
    // Only own enumerable string keys; the prototype becomes Object.prototype.
    assert_eq!(ev(&b, "class K { constructor() { this.a = 1; } } var k = structuredClone(new K()); (Object.getPrototypeOf(k) === Object.prototype) + ',' + k.a"), "true,1");
    // Primitive wrappers stay wrappers (Boolean/Number/String and BigInt).
    // Object(10n) used to clone as a plain object before the BigInt arm.
    assert_eq!(
        ev(
            &b,
            "var big = structuredClone(Object(10n)); typeof big + ',' + String(big.valueOf())"
        ),
        "object,10"
    );
    assert_eq!(
        ev(
            &b,
            "var w = structuredClone({ b: Object(true), n: Object(4), s: Object('z') }); \
             [w.b instanceof Boolean, w.b.valueOf(), w.n instanceof Number, w.n.valueOf(), \
              w.s instanceof String, String(w.s.valueOf())].join()"
        ),
        "true,true,true,4,true,z"
    );
    // Nodes, WeakMaps and promises are not serializable.
    b.set_document(Rc::new(
        Document::parse_html("<html><body><p>x</p></body></html>").unwrap(),
    ))
    .unwrap();
    assert_eq!(ev(&b, "var n; try { structuredClone({ el: document.body }); n = 'no throw'; } catch (e) { n = e.name; } n"), "DataCloneError");
    assert_eq!(ev(&b, "var w; try { structuredClone(new WeakMap()); w = 'no throw'; } catch (e) { w = e.name; } w"), "DataCloneError");
    assert_eq!(ev(&b, "var pr; try { structuredClone(Promise.resolve()); pr = 'no throw'; } catch (e) { pr = e.name; } pr"), "DataCloneError");
}
