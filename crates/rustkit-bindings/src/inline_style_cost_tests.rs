//! What reading and writing inline style properties costs (MotionMark's Multiply writes three inline
//! properties per tile per frame, thousands a frame; one frame ran past the 5 s script budget on a
//! Windows release build: 15000 reads 5116 ms, 15000 writes 3626 ms, against Chrome 148's 3.8 ms and 2.6 ms
//! on the same page). The work is counted, not timed: how many times an element's `style` attribute text
//! is parsed (`__rk.inlineStyleParses`). Reading the same unchanged text again, and reading after the
//! engine's own write, need no new parse.

use super::*;

fn bound() -> DomBindings {
    let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    b.set_document(Rc::new(
        Document::parse_html("<html><body><div id=t style='left: 1px; top: 2px; width: 3px'></div></body></html>").unwrap(),
    ))
    .unwrap();
    b
}

fn parses(b: &DomBindings) -> f64 {
    match b.runtime.borrow_mut().evaluate_host_bookkeeping("return __rk.inlineStyleParses()").unwrap() {
        JsValue::Number(n) => n,
        other => panic!("{other:?}"),
    }
}

fn ev(b: &DomBindings, script: &str) -> String {
    match b.evaluate(script).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

#[test]
fn reading_and_writing_inline_properties_parses_the_style_text_once_per_change() {
    let b = bound();
    let before = parses(&b);
    // 1000 reads of unchanged text, then 300 writes (each read back), as an animation frame does.
    assert_eq!(
        ev(
            &b,
            "var s = document.getElementById('t').style, out = ''; \
             for (var i = 0; i < 1000; i++) out = s.left + s.top; \
             for (var j = 0; j < 100; j++) { s.transform = 'rotate(' + j + 'deg)'; s.backgroundColor = 'hsla(' + j + ',100%,50%,0.5)'; s.opacity = '1'; } \
             out + '|' + s.transform + '|' + s.backgroundColor + '|' + s.getPropertyValue('width') + '|' + s.length",
        ),
        "1px2px|rotate(99deg)|hsla(99,100%,50%,0.5)|3px|6"
    );
    let n = parses(&b) - before;
    assert!(n <= 5.0, "{n} parses of the style text for 1000 reads and 300 writes");
}

#[test]
fn a_style_attribute_changed_by_script_is_read_afresh() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style, a = s.left; \
             e.setAttribute('style', 'left: 9px; color: red'); \
             a + '|' + s.left + '|' + s.color + '|' + s.top + '|' + s.length",
        ),
        "1px|9px|red||2"
    );
}

/// What a read gives after a write is what the attribute now says, whatever the name or value written: a
/// write the parser would read differently (a `;` or `:` in the name, a comment, mismatched brackets) is not
/// taken from the kept list. (The attribute is set again with a space in front: different text, the same
/// declarations, so the second read parses it.)
#[test]
fn a_read_after_an_odd_write_matches_the_attribute() {
    let b = bound();
    for (name, value) in [("a;b", "1px"), ("x:y", "2px"), ("transform", "rotate(3deg]"), ("color", "red /* c */"), ("left", "4px\u{1}")] {
        let script = format!(
            "var e = document.getElementById('t'); e.setAttribute('style', 'left: 1px; top: 2px'); var s = e.style; \
             s.setProperty({name:?}, {value:?}); \
             var kept = s.cssText + '|' + s.length + '|' + s.getPropertyValue({name:?}); \
             e.setAttribute('style', ' ' + e.getAttribute('style')); \
             var fresh = s.cssText + '|' + s.length + '|' + s.getPropertyValue({name:?}); \
             kept === fresh ? 'same' : kept + ' != ' + fresh"
        );
        assert_eq!(ev(&b, &script), "same", "{name}: {value}");
    }
}

/// What the kept declarations and the property-name table remember is made without the page's built-ins:
/// a page that replaces them for a while (and puts them back) leaves nothing behind. Reads and writes made
/// while they are replaced may fail; every one after must be right.
#[test]
fn replaced_built_ins_leave_no_trace_in_inline_style() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style; \
             var rp = String.prototype.replace, tl = String.prototype.toLowerCase; \
             String.prototype.replace = function () { return 'width'; }; String.prototype.toLowerCase = function () { return 'width'; }; \
             try { s.outlineColor = 'green'; } catch (x) {} \
             String.prototype.replace = rp; String.prototype.toLowerCase = tl; \
             var saved = [[Object.prototype, 'hasOwnProperty'], [Array.prototype, 'every'], [Array.prototype, 'map'], \
               [Array.prototype, 'filter'], [Array.prototype, 'push'], [String.prototype, 'replace'], \
               [String.prototype, 'toLowerCase'], [String.prototype, 'indexOf'], [String.prototype, 'charCodeAt'], \
               [RegExp.prototype, 'exec'], [RegExp.prototype, 'test'], [WeakMap.prototype, 'get'], [WeakMap.prototype, 'set']] \
               .map(function (p) { return [p[0], p[1], p[0][p[1]]]; }); \
             Object.prototype.hasOwnProperty = function () { return true; }; \
             Array.prototype.every = function () { return true; }; Array.prototype.map = function () { return []; }; \
             Array.prototype.filter = function () { return []; }; Array.prototype.push = function () { return 0; }; \
             String.prototype.replace = function () { return 'width'; }; String.prototype.toLowerCase = function () { return 'width'; }; \
             String.prototype.indexOf = function () { return -1; }; String.prototype.charCodeAt = function () { return 97; }; \
             RegExp.prototype.exec = function () { return null; }; RegExp.prototype.test = function () { return true; }; \
             WeakMap.prototype.get = function () { return undefined; }; WeakMap.prototype.set = function () { return this; }; \
             try { s.paddingLeft = '7px'; } catch (x) {} try { s.marginTop = '8px'; } catch (x) {} \
             try { s.setProperty('a;b', '1px'); } catch (x) {} try { var r1 = s.left + s.top + s.borderTopWidth; } catch (x) {} \
             saved.forEach(function (p) { p[0][p[1]] = p[2]; }); \
             e.setAttribute('style', 'left: 1px; top: 2px'); \
             s.paddingLeft = '4px'; s.marginTop = '5px'; s.borderTopWidth = '6px'; s.outlineColor = 'green'; \
             var kept = s.cssText + '|' + s.length + '|' + s.paddingLeft + '|' + s.marginTop + '|' + s.left + '|' + s.outlineColor; \
             e.setAttribute('style', ' ' + e.getAttribute('style')); \
             var fresh = s.cssText + '|' + s.length + '|' + s.paddingLeft + '|' + s.marginTop + '|' + s.left + '|' + s.outlineColor; \
             kept === fresh ? kept : kept + ' != ' + fresh",
        ),
        "left: 1px; top: 2px; padding-left: 4px; margin-top: 5px; border-top-width: 6px; outline-color: green;|6|4px|5px|1px|green"
    );
}

/// A write the element answers by changing its own style again (a custom element's
/// attributeChangedCallback) is read back as the attribute now stands, not as the write left it.
#[test]
fn a_style_changed_again_inside_the_write_is_read_as_it_stands() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var seen = 0; \
             customElements.define('x-restyle', class extends HTMLElement { \
               static get observedAttributes() { return ['style']; } \
               attributeChangedCallback() { if (seen++ === 0) this.setAttribute('style', 'color: blue; width: 9px'); } }); \
             var e = document.createElement('x-restyle'); document.body.appendChild(e); \
             e.style.color = 'red'; \
             [e.getAttribute('style'), e.style.color, e.style.width, e.style.length].join('|')",
        ),
        "color: blue; width: 9px|blue|9px|2"
    );
}

/// Reads `s` (the element's style) as kept, then sets the attribute again with a space in front (different
/// text, the same declarations, so the next read parses it), and says whether the two reads agree.
const KEPT_VS_FRESH: &str = "function keptVsFresh(e, s, more) { \
       var read = function () { return s.cssText + '|' + s.length + '|' + more.map(function (n) { return s.getPropertyValue(n); }).join(','); }; \
       var kept = read(); e.setAttribute('style', ' ' + e.getAttribute('style')); var fresh = read(); \
       return kept === fresh ? kept : kept + ' != ' + fresh; } ";

/// The check that the parser's built-ins are the originals looks at an own data property, so an accessor
/// in a built-in's place is not taken for the original by way of a `value` on `Object.prototype`. A parse
/// made through it is not kept.
#[test]
fn an_accessor_in_place_of_a_parser_built_in_is_seen() {
    let b = bound();
    // The accessor stays in place and answers the original once the read is done (the engine's own
    // property caches are not what this checks).
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style, f = Array.prototype.filter, bad = true; \
             e.setAttribute('style', 'left: 1px; top: 2px; width: 3px'); \
             Object.defineProperty(Array.prototype, 'filter', { configurable: true, \
               get: function () { return bad ? function () { return []; } : f; } }); \
             Object.prototype.value = f; \
             try { var during = s.length; } catch (x) {} \
             delete Object.prototype.value; bad = false; \
             s.cssText + '|' + s.length",
        ),
        "left: 1px; top: 2px; width: 3px;|3"
    );
}

/// What a write keeps is plain strings, whatever the page's global `String` returns.
#[test]
fn a_write_keeps_plain_strings_when_string_is_replaced() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style, S = String; \
             function P(t) { this.t = t; } P.prototype = Object.create(S.prototype); \
             P.prototype.valueOf = function () { return this.t; }; P.prototype.toString = function () { return this.t; }; \
             P.prototype.trim = function () { return this; }; P.prototype.toLowerCase = function () { return this; }; \
             var r = s.left; \
             String = function (v) { var t = S(v); return t === 'left' || t === '5px' ? new P(t) : t; }; \
             var err = ''; try { s.setProperty('left', '5px'); } catch (x) { err = 'ERR ' + x.message + ' '; } \
             String = S; \
             err + typeof s.getPropertyValue('left') + '|' + s.getPropertyValue('left') + '|' + s.cssText",
        ),
        "string|5px|left: 5px; top: 2px; width: 3px;"
    );
}

/// The kept declarations are not reachable through an index accessor on `Array.prototype`: changing what
/// it saw, and removing it, changes nothing that is read after.
#[test]
fn an_index_accessor_on_array_prototype_does_not_reach_the_kept_declarations() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style, seen = Object.create(null), n = 0, last; \
             e.setAttribute('style', 'left: 1px; top: 2px; width: 3px; height: 4px'); \
             Object.defineProperty(Array.prototype, '3', { configurable: true, \
               get: function () { return last; }, set: function (v) { last = v; seen[n++] = v; } }); \
             try { var during = s.length; } catch (x) {} \
             delete Array.prototype[3]; \
             for (var i = 0; i < n; i++) { var v = seen[i]; if (v && typeof v === 'object' && 'value' in v) v.value = 'HACKED'; } \
             var out; try { out = s.cssText + '|' + s.length + '|' + s.height; } catch (x) { out = 'threw ' + x; } \
             out",
        ),
        "left: 1px; top: 2px; width: 3px; height: 4px;|4|4px"
    );
}

/// A write keeps its declarations only when every one of them reads back as itself, whatever was kept
/// before: a declaration from a parse that was not kept (here under a replaced `push` that does nothing
/// different) is checked like the one written.
#[test]
fn a_write_after_a_parse_that_was_not_kept_is_checked_whole() {
    let b = bound();
    let got = ev(
        &b,
        &format!(
            "{KEPT_VS_FRESH} var e = document.getElementById('t'), s = e.style, r = s.left, push = Array.prototype.push; \
             Array.prototype.push = function () {{ return push.apply(this, arguments); }}; \
             e.setAttribute('style', '--x: ('); s.top = '2px'; \
             Array.prototype.push = push; \
             keptVsFresh(e, s, ['top', '--x'])"
        ),
    );
    assert!(!got.contains(" != "), "{got}");
}

/// A parse made while a parser built-in is replaced is not kept: once it is put back, the same text reads
/// right.
#[test]
fn a_parse_while_a_built_in_is_replaced_is_not_kept() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style, r = s.left, trim = String.prototype.trim; \
             String.prototype.trim = function () { return 'color'; }; \
             e.setAttribute('style', 'left: 6px; top: 7px'); try { var during = s.left; } catch (x) {} \
             String.prototype.trim = trim; \
             s.cssText + '|' + s.length + '|' + s.left",
        ),
        "left: 6px; top: 7px;|2|6px"
    );
}

/// A write the element answers by writing its own inline style (`this.style.top` inside
/// attributeChangedCallback) leaves both writes, read as the attribute stands.
#[test]
fn a_style_write_inside_the_write_is_read_as_it_stands() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var seen = 0; \
             customElements.define('x-retop', class extends HTMLElement { \
               static get observedAttributes() { return ['style']; } \
               attributeChangedCallback() { if (seen++ === 0) this.style.top = '7px'; } }); \
             var e = document.createElement('x-retop'); document.body.appendChild(e); \
             e.style.color = 'red'; \
             [e.getAttribute('style'), e.style.color, e.style.top, e.style.length].join('|')",
        ),
        "color: red; top: 7px;|red|7px|2"
    );
}

/// Writes of every shape read back as the attribute says: priority, quotes, escapes, braces, `url()`, a
/// custom property, a removal, and a write on a base the kept list could not hold exactly.
#[test]
fn writes_of_every_shape_read_back_as_the_attribute_says() {
    let b = bound();
    for (base, op, names) in [
        ("left: 1px", "s.setProperty('color', 'red', 'important')", "color"),
        ("color: red !important; left: 1px", "s.left = '2px'", "color,left"),
        ("left: 1px", "s.setProperty('font-family', '\"A;B\", serif')", "font-family"),
        ("left: 1px", "s.setProperty('content', '\"a\\\\\"b\"')", "content"),
        ("left: 1px", "s.setProperty('--a', '{ b: c; }')", "--a,left"),
        ("left: 1px", "s.setProperty('background-image', 'url(\"a;b.png\")')", "background-image"),
        ("left: 1px", "s.setProperty('--Custom', '3px')", "--custom,--Custom"),
        ("left: 1px; top: 2px", "s.removeProperty('top')", "top,left"),
        ("color: red /* c */; left: 1px", "s.top = '2px'", "color,top"),
        ("content: \"x;y\"; left: 1px", "s.top = '2px'", "content,top"),
    ] {
        let names: Vec<String> = names.split(',').map(|n| format!("'{n}'")).collect();
        let script = format!(
            "{KEPT_VS_FRESH} var e = document.getElementById('t'), s = e.style; e.setAttribute('style', {base:?}); \
             var r = s.length; {op}; keptVsFresh(e, s, [{}])",
            names.join(",")
        );
        let got = ev(&b, &script);
        assert!(!got.contains(" != "), "{base} / {op}: {got}");
    }
}

/// Each field of a parsed declaration is read once, and what is checked is what is kept: an index
/// accessor on `Array.prototype` that hands the parse an object whose `name` answers a string once and
/// an object after leaves a string kept.
#[test]
fn a_declaration_field_is_read_once_and_kept_as_checked() {
    let b = bound();
    assert_eq!(
        ev(
            &b,
            "var e = document.getElementById('t'), s = e.style, r = s.left, calls = 0, names = 0; \
             var x = { value: '1px', important: false }; \
             Object.defineProperty(x, 'name', { get: function () { return names++ ? { toString: function () { return 'zzz'; } } : 'left'; } }); \
             Object.defineProperty(Array.prototype, '0', { configurable: true, \
               get: function () { return calls++ ? x : 'left: 1px'; }, set: function () {} }); \
             e.setAttribute('style', 'left: 1px'); \
             try { var during = s.length; } catch (err) {} \
             delete Array.prototype[0]; \
             [typeof s.item(0), s.cssText].join(',')",
        ),
        "string,left: 1px;"
    );
}
