//! `obj[n]` where `n` is a whole-number double must act as the array index `n`, and every other
//! double must keep its canonical string key (ES2024 ToPropertyKey = ToString for a Number).
//! JS-PERF1 maps the whole-number case straight to an index instead of number -> string -> parsed
//! back; these are the cases that mapping must not break.

use rustkit_js::{JsRuntime, JsValue};

fn run(src: &str) -> String {
    let mut rt = JsRuntime::new().unwrap();
    match rt.evaluate_script(src) {
        Ok(JsValue::String(s)) => s,
        other => format!("{other:?}"),
    }
}

/// `f(x)` makes sure the key is a double: `(x + 0.5) - 0.5` is a Float64 whole number.
const FLOAT: &str = "function f(x) { return (x + 0.5) - 0.5; }";

#[test]
fn whole_number_doubles_index_arrays_and_objects() {
    assert_eq!(run(&format!("{FLOAT} var a = [10, 20, 30]; String(a[f(0)]) + a[f(1)] + a[f(2)] + a[f(3)]")), "102030undefined");
    assert_eq!(run(&format!("{FLOAT} var a = [10, 20, 30]; a[f(1)] = 99; a.join()")), "10,99,30");
    assert_eq!(run(&format!("{FLOAT} var o = {{}}; o[f(7)] = 'x'; o[7] + o['7'] + Object.keys(o).join()")), "xx7");
    assert_eq!(run(&format!("{FLOAT} var s = 'abc'; s[f(1)]")), "b");
    assert_eq!(run(&format!("{FLOAT} var o = {{}}; o[f(3)] = 1; o[f(1)] = 1; o[f(2)] = 1; Object.keys(o).join()")), "1,2,3");
    assert_eq!(run(&format!("{FLOAT} var a = []; a[f(2)] = 'z'; String(a.length) + a.hasOwnProperty(2)")), "3true");
    assert_eq!(run(&format!("{FLOAT} var a = [1,2,3]; delete a[f(1)]; String(1 in a) + a.length")), "false3");
    assert_eq!(run(&format!("{FLOAT} var a = [1,2,3]; String(f(2) in a) + String(f(5) in a)")), "truefalse");
}

#[test]
fn negative_zero_is_index_zero() {
    assert_eq!(run("var a = ['zero']; a[-0]"), "zero");
    assert_eq!(run("var a = ['zero']; var z = -0; a[z * 1]"), "zero");
    assert_eq!(run("var o = {}; o[-0] = 'z'; Object.keys(o).join() + o['0']"), "0z");
}

#[test]
fn doubles_that_are_not_array_indices_keep_their_string_keys() {
    // 2^32 - 1 is not an array index (the largest is 2^32 - 2) but is still the string "4294967295".
    assert_eq!(run("var o = {}; o[4294967295] = 1; Object.keys(o).join() + o['4294967295']"), "42949672951");
    assert_eq!(run("var o = {}; o[4294967296] = 1; Object.keys(o).join() + o['4294967296']"), "42949672961");
    assert_eq!(run("var a = []; a[4294967294] = 1; String(a.length)"), "4294967295");
    assert_eq!(run("var a = []; a[4294967295] = 1; String(a.length)"), "0");
    assert_eq!(run("var o = {}; o[NaN] = 5; o['NaN'] + Object.keys(o).join()"), "5NaN");
    assert_eq!(run("var o = {}; o[1.5] = 7; o['1.5'] + Object.keys(o).join()"), "71.5");
    assert_eq!(run("var a = [1, 2, 3]; String(a[1.5])"), "undefined");
    assert_eq!(run("var o = {}; o[-1] = 'n'; o['-1'] + Object.keys(o).join()"), "n-1");
    assert_eq!(run("var o = {}; o[-2.5] = 'n'; o['-2.5']"), "n");
    assert_eq!(run("var o = {}; o[1e21] = 'big'; o['1e+21'] + Object.keys(o).join()"), "big1e+21");
    assert_eq!(run("var o = {}; o[9007199254740992] = 'p'; o['9007199254740992']"), "p");
    assert_eq!(run("var o = {}; o[Infinity] = 'i'; o['Infinity'] + Object.keys(o).join()"), "iInfinity");
    assert_eq!(run("var o = {}; o[-Infinity] = 'i'; o['-Infinity']"), "i");
    assert_eq!(run("var o = {}; o[0.1 + 0.2] = 'f'; o['0.30000000000000004']"), "f");
    assert_eq!(run("var o = {}; o[1e-7] = 'e'; o['1e-7']"), "e");
}

#[test]
fn a_double_key_and_its_string_are_the_same_property() {
    assert_eq!(
        run("var o = {}; for (var i = 0; i < 5; i++) { o[(i + 0.5) - 0.5] = i; } Object.keys(o).join() + o['3'] + o[3]"),
        "0,1,2,3,433"
    );
    assert_eq!(run("var m = {}; m['12'] = 'a'; m[12.0] = 'b'; String(Object.keys(m).length) + m[12]"), "1b");
}

/// Typed arrays and Proxies see the CANONICAL key of a double (`ToPropertyKey` = `ToString`), not
/// the index the fast path may have used for it. Expected values: Chrome for Testing
/// 148.0.7778.216, `tools/parity_oracle`'s Playwright, the same expressions.
#[test]
fn typed_arrays_and_proxies_see_the_canonical_key_of_a_double() {
    let cases: [(&str, &str); 9] = [
        ("var ta = new Int8Array([5,6,7]); String(ta[-0]) + ',' + ta[f(0)] + ',' + ta[0 * -1]", "5,5,5"),
        ("var ta = new Int8Array([5,6,7]); ta[1.5] = 9; String(ta[1.5]) + ',' + ta[f(1)] + ',' + Object.keys(ta).join('|') + ',' + ta.hasOwnProperty(1.5)", "undefined,6,0|1|2,false"),
        ("var ta = new Int8Array([5,6,7]); ta[2**53] = 9; String(ta[2**53]) + ',' + String(ta[f(3)]) + ',' + String(ta[4294967295]) + ',' + String(ta[2**32]) + ',' + Object.keys(ta).join('|')", "undefined,undefined,undefined,undefined,0|1|2"),
        ("var ta = new Int8Array([5,6,7]); ta[f(5)] = 1; String(ta[f(5)]) + ',' + ta.length + ',' + Object.keys(ta).join('|')", "undefined,3,0|1|2"),
        ("var ta = new Int8Array(3); ta[f(1)] = 42; ta[f(2)] = 7.9; Array.from(ta).join()", "0,42,7"),
        ("var ta = new Int8Array([5,6,7]); ta[NaN] = 1; String(ta[NaN]) + ',' + ta.hasOwnProperty('NaN') + ',' + Object.keys(ta).join('|')", "undefined,false,0|1|2"),
        ("var ta = new Int8Array([5,6,7]); ta[-1] = 1; String(ta[-1]) + ',' + ta.hasOwnProperty('-1')", "undefined,false"),
        (
            "var log = []; var p = new Proxy({}, { get(t, k) { log.push(typeof k + ':' + k); return 1; }, has(t, k) { log.push('has ' + typeof k + ':' + k); return true; } }); p[f(0)]; p[f(7)]; p[-0]; p[1.5]; p[2**32]; p[NaN]; p[1e21]; (f(3) in p); log.join(' ')",
            "string:0 string:7 string:0 string:1.5 string:4294967296 string:NaN string:1e+21 has string:3",
        ),
        (
            "var log = []; var p = new Proxy({}, { set(t, k, v) { log.push(typeof k + ':' + k); return true; }, deleteProperty(t, k) { log.push('del ' + k); return true; } }); p[f(0)] = 1; p[-0] = 2; p[f(12)] = 3; delete p[f(4)]; p[0.25] = 4; log.join(' ')",
            "string:0 string:0 string:12 del 4 string:0.25",
        ),
    ];
    for (body, expected) in cases {
        assert_eq!(run(&format!("{FLOAT} {body}")), expected, "{body}");
    }
}
