//! The SVG transform DOM, vs Chrome for Testing 148.0.7778.216
//! (`docs/diagnostics/2026-11-14/svg_transform_dom_cases.mjs`, output `svg_transform_dom_cft148.json`):
//! `SVGSVGElement.createSVGTransform` / `createSVGTransformFromMatrix`, `SVGTransform` and its setters, and the
//! `SVGAnimatedTransformList` / `SVGTransformList` behind `transform` and `gradientTransform`. MotionMark
//! 1.3.2's Suits test calls `createSVGTransform`, `setRotate` and `gradientTransform.baseVal.initialize` on
//! every particle reset, and stopped there with "not a callable function".

use super::*;
use std::collections::BTreeMap;

const CASES: &str = include_str!("../../../docs/diagnostics/2026-11-14/svg_transform_dom_cases.json");
const CHROME: &str = include_str!("../../../docs/diagnostics/2026-11-14/svg_transform_dom_cft148.json");

/// The probe's helper: an SVGTransform's matrix as six numbers rounded to three places.
const M: &str = "function m(t) { var x = t.matrix; return [x.a, x.b, x.c, x.d, x.e, x.f].map(function (v) { return Math.round(v * 1000) / 1000; }).join(','); }";

/// Runs one case script. The scripts are this repository's own fixed strings (the Chrome probe's), evaluated
/// in a throwaway test runtime: `eval` is how the probe runs them too.
fn run(body: &str) -> String {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html(
            "<!doctype html><svg id=s width=100 height=100><defs><linearGradient id=g></linearGradient></defs><rect id=r width=10 height=10></rect></svg>",
        )
        .unwrap(),
    ))
    .unwrap();
    let script = format!(
        "(function () {{ var svg = document.getElementById('s'), grad = document.getElementById('g'), rect = document.getElementById('r'); {M}; \
         try {{ return String(eval({body:?})); }} catch (e) {{ return 'threw:' + e.name + ':' + e.message; }} }})()"
    );
    match b.evaluate(&script) {
        Ok(JsValue::String(s)) => s,
        Ok(other) => format!("{other:?}"),
        Err(e) => format!("error: {e}"),
    }
}

#[test]
fn the_svg_transform_dom_answers_as_in_chrome() {
    let cases: BTreeMap<String, String> = serde_json::from_str(CASES).unwrap();
    let chrome: BTreeMap<String, String> = serde_json::from_str(CHROME).unwrap();
    let mut wrong = Vec::new();
    for (name, body) in &cases {
        let got = run(body);
        if got != chrome[name] {
            wrong.push(format!("{name}: chrome {:?}, here {got:?}", chrome[name]));
        }
    }
    assert!(wrong.is_empty(), "{} of {} differ:\n{}", wrong.len(), cases.len(), wrong.join("\n"));
}

/// The list operations of the pin below, written against four helpers so the same text runs with and
/// without the page's built-ins replaced: `setT` / `getT` / `getG` read and write the attributes, `put`
/// records one value.
const SEQUENCE: &str = "setT('scale(3) translate(1, 1) rotate(10 2 3) skewX(5) matrix(1 0 0 1 4 5)'); \
    var l = rect.transform.baseVal; \
    var t = svg.createSVGTransform(); t.setRotate(30, 0, 0); l.appendItem(t); \
    l.insertItemBefore(l.getItem(0), 0); l.replaceItem(svg.createSVGTransform(), 1); l.removeItem(2); \
    put(getT()); put(l.numberOfItems); put(l.getItem(1).type); \
    t.setScale(2, 4); put(getT()); \
    var c = l.consolidate(); put(c.type); put(l.numberOfItems); \
    var u = svg.createSVGTransform(); u.setTranslate(7, 8); \
    var s = grad.gradientTransform.baseVal; s.initialize(u); s.initialize(u); u.setSkewY(3); \
    put(getG()); put(s.numberOfItems); \
    try { rect.transform.animVal.clear(); } catch (e) { put(e.name); } \
    try { l.getItem(9); } catch (e) { put(e.name); }";

/// The list paths use only what was taken at install: with the page's built-ins replaced by functions that
/// throw, and setters on `Object.prototype` and `Array.prototype` that count whatever is assigned through
/// them, the same list operations give the same attribute text and lists, and nothing the lists keep
/// passes through the page's setters.
#[test]
fn the_list_paths_answer_the_same_with_the_page_built_ins_replaced() {
    let plain = run(&format!(
        "(function () {{ var acc = ''; function put(v) {{ acc += '|' + v; }} \
           function setT(v) {{ rect.setAttribute('transform', v); }} \
           function getT() {{ return rect.getAttribute('transform'); }} \
           function getG() {{ return grad.getAttribute('gradientTransform'); }} \
           {SEQUENCE} return acc; }})()"
    ));
    let poisoned = run(&format!(
        "(function () {{ \
           var acc = '', leaks = 0, saved = [], boom = function () {{ throw new Error('a page built-in was used'); }}; \
           var gpd = Object.getOwnPropertyDescriptor, dp = Object.defineProperty; \
           var ga = Element.prototype.getAttribute, sa = Element.prototype.setAttribute, call = Function.prototype.call; \
           function put(v) {{ acc += '|' + v; }} \
           function setT(v) {{ call.call(sa, rect, 'transform', v); }} \
           function getT() {{ return call.call(ga, rect, 'transform'); }} \
           function getG() {{ return call.call(ga, grad, 'gradientTransform'); }} \
           function swap(o, k) {{ saved[saved.length] = [o, k, gpd(o, k)]; dp(o, k, {{ value: boom, writable: true, configurable: true }}); }} \
           function trap(o, k) {{ saved[saved.length] = [o, k, gpd(o, k)]; \
             dp(o, k, {{ configurable: true, set: function () {{ leaks++; }}, get: function () {{ return undefined; }} }}); }} \
           var arr = ['forEach', 'map', 'join', 'splice', 'push', 'indexOf', 'slice', 'reduce', 'filter', 'some']; \
           var math = ['cos', 'sin', 'tan', 'min']; \
           var keys = ['list', 'm', 'type', 'angle', 'n', 'el', 'attr', 'text', 'items', 'wrapper', 'cx', 'cy', 'enumerable', \
             '0', '1', '2', '3', '4', '5']; \
           var i; \
           for (i = 0; i < math.length; i++) swap(Math, math[i]); \
           swap(String.prototype, 'split'); swap(RegExp.prototype, 'exec'); swap(window, 'Number'); swap(window, 'isFinite'); \
           swap(WeakMap.prototype, 'get'); swap(WeakMap.prototype, 'set'); \
           swap(Element.prototype, 'getAttribute'); swap(Element.prototype, 'setAttribute'); \
           for (i = 0; i < keys.length; i++) trap(Object.prototype, keys[i]); \
           trap(Array.prototype, '0'); trap(Array.prototype, '1'); trap(Array.prototype, '2'); trap(Array.prototype, '3'); \
           for (i = 0; i < arr.length; i++) swap(Array.prototype, arr[i]); \
           trap(Object.prototype, 'value'); \
           var err = ''; \
           try {{ {SEQUENCE} }} catch (e) {{ err = 'threw ' + e.message; }} \
           for (i = saved.length - 1; i >= 0; i--) {{ var x = saved[i]; if (x[2]) dp(x[0], x[1], x[2]); else delete x[0][x[1]]; }} \
           return err || acc + '|leaks ' + leaks; \
         }})()"
    ));
    assert_eq!(poisoned, format!("{plain}|leaks 0"), "plain: {plain}");
}
