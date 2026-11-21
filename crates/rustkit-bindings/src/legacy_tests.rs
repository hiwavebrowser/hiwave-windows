//! ECMAScript members Boa lacks that pages call unguarded (web_legacy.js).

use super::*;

fn ev(script: &str) -> String {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        JsValue::Boolean(x) => x.to_string(),
        JsValue::Number(n) => format!("{n}"),
        JsValue::Undefined => "undefined".into(),
        JsValue::Null => "null".into(),
        other => format!("{other:?}"),
    }
}

#[test]
fn substr_follows_annex_b() {
    // (start, length) over a 10 character string.
    assert_eq!(
        ev("var s = 'abcdefghij'; [s.substr(2), s.substr(2, 3), s.substr(-3), s.substr(-3, 2), s.substr(0, 0), s.substr(8, 99), s.substr(99), s.substr(-99, 2), s.substr(2, -1), s.substr(), s.substr(NaN, 2), s.substr(1, undefined)].join('|')"),
        "cdefghij|cde|hij|hi||ij||ab||abcdefghij|ab|bcdefghij"
    );
    // github's favicon code: strip the extension.
    assert_eq!(ev("var h = '/fav.svg'; h.substr(0, h.lastIndexOf('.'))"), "/fav");
    assert_eq!(ev("var e; try { String.prototype.substr.call(null, 1); } catch (x) { e = x.name; } e"), "TypeError");
}

#[test]
fn trimleft_trimright_and_the_html_methods() {
    assert_eq!(ev("['  x '.trimLeft() + '|', '  x '.trimRight() + '|'].join()"), "x |,  x|");
    assert_eq!(ev("'t'.anchor('a\"b')"), "<a name=\"a&quot;b\">t</a>");
    assert_eq!(ev("['x'.bold(), 'x'.italics(), 'x'.link('/u'), 'x'.fontsize(3), 'x'.sub(), 'x'.sup()].join('')"), "<b>x</b><i>x</i><a href=\"/u\">x</a><font size=\"3\">x</font><sub>x</sub><sup>x</sup>");
}

#[test]
fn set_methods() {
    assert_eq!(ev("var a = new Set([1, 2, 3]), b = new Set([3, 4]); [[...a.union(b)], [...a.intersection(b)], [...a.difference(b)], [...a.symmetricDifference(b)]].map(function (x) { return x.join(''); }).join('|')"), "1234|3|12|124");
    assert_eq!(ev("var a = new Set([1, 2]); [a.isSubsetOf(new Set([1, 2, 3])), a.isSupersetOf(new Set([1])), a.isDisjointFrom(new Set([3])), a.isDisjointFrom(new Set([2]))].join()"), "true,true,true,false");
    assert_eq!(ev("var e; try { new Set().union(5); } catch (x) { e = x.name; } e"), "TypeError");
    assert_eq!(ev("var s = new Set([1]); s.union(new Map([[2, 'x']])).size"), "2", "a Map is set-like");
}

#[test]
fn array_fromasync_and_finalizationregistry() {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.evaluate("var log = []; Array.fromAsync([Promise.resolve(1), 2, Promise.resolve(3)], function (x) { return x * 2; }).then(function (a) { log.push(a.join()); });").unwrap();
    b.evaluate("(async function () { async function* g() { yield 'a'; yield 'b'; } log.push((await Array.fromAsync(g())).join()); })();").unwrap();
    b.run_timers(100, 10).unwrap();
    match b.evaluate("log.join('|')").unwrap() {
        JsValue::String(s) => assert_eq!(s, "2,4,6|a,b"),
        other => panic!("{other:?}"),
    }
    assert_eq!(ev("var r = new FinalizationRegistry(function () {}); var o = {}; r.register(o, 1, o); [typeof FinalizationRegistry, r.unregister(o), r.unregister(o)].join()"), "function,true,false");
    assert_eq!(ev("var e; try { new FinalizationRegistry(1); } catch (x) { e = x.name; } e"), "TypeError");
}

#[test]
fn nothing_the_engine_already_has_is_replaced() {
    assert_eq!(ev("[String.prototype.trimStart.name, Array.prototype.flat.name, typeof String.prototype.replaceAll].join()"), "trimStart,flat,function");
}
