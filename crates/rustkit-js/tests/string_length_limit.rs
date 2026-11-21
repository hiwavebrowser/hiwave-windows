//! A string longer than Chrome's maximum (2^29 - 24 = 536,870,888 code units on 64-bit) is never
//! built: the operation throws `RangeError: Invalid string length`, as Chrome for Testing
//! 148.0.7778.216 does for every case below (docs/diagnostics/2026-11-16/string_limit_chrome148.json),
//! and the script can catch it and go on.
//!
//! The over-long results are refused before they are built, so these tests stay small: the inputs
//! are at most one 2^28-unit string (for `+`, which cannot pass the limit with smaller inputs).

use rustkit_js::{JsRuntime, JsValue};

fn run(src: &str) -> String {
    let mut rt = JsRuntime::new().unwrap();
    let wrapped = format!(
        "(function () {{ try {{ var v = (function () {{ {src} }})(); return 'value ' + v; }} \
         catch (e) {{ return e.name + ': ' + e.message; }} }})()"
    );
    match rt.evaluate_script(&wrapped).unwrap() {
        JsValue::String(s) => s,
        other => format!("{other:?}"),
    }
}

const REFUSED: &str = "RangeError: Invalid string length";

#[test]
fn the_limit_is_chromes() {
    // The largest string Chrome builds has 2^29 - 24 code units; one more is refused.
    assert_eq!(run("return 'x'.repeat(2 ** 29 - 23).length;"), REFUSED);
    assert_eq!(run("return 'x'.padEnd(2 ** 29 - 23).length;"), REFUSED);
    assert_eq!(run("return 'x'.padStart(2 ** 29).length;"), REFUSED);
    // The largest string a test can afford to build here: a quarter of the limit, by doubling.
    assert_eq!(run("return 'x'.repeat(2 ** 28).length;"), "value 268435456");
}

#[test]
fn a_replacement_that_would_pass_the_limit_is_refused() {
    // A capture substituted 2^15 times into a 2^20-unit match: 2^35 units.
    let puff = "function puff(x, n) { while (x.length < n) x += x; return x.substring(0, n); }
        var x = puff('1', 1 << 20), rep = puff('$1', 1 << 16);";
    assert_eq!(run(&format!("{puff} return x.replace(/(.+)/g, rep).length;")), REFUSED);
    assert_eq!(run(&format!("{puff} return x.replace(/(.+)/, rep).length;")), REFUSED);
    // Many matches, each replaced by a long string: 2^10 x 2^20 units.
    let many = "var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20);";
    assert_eq!(run(&format!("{many} return x.replaceAll('x', y).length;")), REFUSED);
    assert_eq!(run(&format!("{many} return x.replace(/x/g, y).length;")), REFUSED);
    assert_eq!(run(&format!("{many} return x.replaceAll(/x/g, y).length;")), REFUSED);
    assert_eq!(run(&format!("{many} return x.replace(/x/g, () => y).length;")), REFUSED);
}

#[test]
fn a_join_that_would_pass_the_limit_is_refused() {
    let parts = "var a = new Array(2 ** 15 + 1).fill('x'.repeat(2 ** 14));";
    assert_eq!(run(&format!("{parts} return a.join('').length;")), REFUSED);
    assert_eq!(run(&format!("{parts} return String(a).length;")), REFUSED);
    assert_eq!(run(&format!("{parts} return a.toLocaleString().length;")), REFUSED);
    // The separator counts.
    let seps = "var b = new Array(2 ** 15).fill('x'.repeat(2 ** 14 - 1));";
    assert_eq!(run(&format!("{seps} return b.join('--').length;")), REFUSED);
}

#[test]
fn a_concatenation_that_would_pass_the_limit_is_refused() {
    let s = "var s = 'x'.repeat(2 ** 28);";
    assert_eq!(run(&format!("{s} return (s + s).length;")), REFUSED);
    assert_eq!(run(&format!("{s} return `${{s}}${{s}}`.length;")), REFUSED);
    assert_eq!(run(&format!("{s} return s.concat(s).length;")), REFUSED);
    assert_eq!(run(&format!("{s} var t = s; t += s; return t.length;")), REFUSED);
}

#[test]
fn strings_under_the_limit_are_unchanged_and_the_page_goes_on() {
    assert_eq!(run("return 'ab'.repeat(3) + 'c'.padEnd(3, '.') + ['x', 'y'].join('-');"), "value abababc..x-y");
    assert_eq!(run("return 'aXbXc'.replace(/X/g, '$&$&').replaceAll('b', '$`');"), "value aXXaXXXXc");
    assert_eq!(
        run("var e; try { 'x'.repeat(2 ** 30); } catch (err) { e = err; } return (e instanceof RangeError) + ' ' + 'ok'.length;"),
        "value true 2"
    );
}

#[test]
fn the_refusal_comes_where_chromes_does() {
    // Chrome 148 (string_limit_chrome148.json): `RegExp.prototype[@@replace]` calls every replacer
    // and named-group getter and then refuses; `replaceAll` with a string pattern and `join` refuse
    // as soon as the result passes the limit.
    let calls = "var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20), n = 0;";
    assert_eq!(
        run(&format!("{calls} try {{ x.replace(/x/g, () => (n++, y)); }} catch (e) {{ return e.name + ' after ' + n; }}")),
        "value RangeError after 1024"
    );
    assert_eq!(
        run(&format!("{calls} try {{ x.replaceAll('x', () => (n++, y)); }} catch (e) {{ return e.name + ' after ' + n; }}")),
        "value RangeError after 512"
    );
    assert_eq!(
        run("var x = 'ab'.repeat(2 ** 10), rep = '$<g>'.repeat(2 ** 4), z = 'z'.repeat(2 ** 16), calls = 0;
            var re = /(?<g>ab)/g, orig = RegExp.prototype.exec;
            re.exec = function (s) { var r = orig.call(this, s); if (r) r.groups = { get g() { calls++; return z; } }; return r; };
            try { re[Symbol.replace](x, rep); } catch (e) { return e.name + ' after ' + calls; }"),
        "value RangeError after 16384"
    );
    assert_eq!(
        run("var n = 0, big = 'x'.repeat(2 ** 14), o = { length: 2 ** 16 };
            for (let i = 0; i < 2 ** 16; i++) Object.defineProperty(o, i, { get() { n++; return big; } });
            try { Array.prototype.join.call(o, ''); } catch (e) { return e.name + ' after ' + n; }"),
        "value RangeError after 32768"
    );
}
