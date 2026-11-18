//! An array-like's `length` is set by the page, and can be far larger than what it holds. Work that
//! follows that `length` must not reserve memory for it up front, and the operations Chrome refuses
//! at once must refuse at once here too, with Chrome's error (Chrome for Testing 148.0.7778.216,
//! docs/diagnostics/2026-11-16/length_alloc_chrome148.json):
//!
//! (`JSON.stringify` of such an array is refused in `string_length_limit_more.rs`.)
//!
//! - `Function.prototype.apply`, `Reflect.apply` and `Reflect.construct` with more than 2^27
//!   (134,217,728) arguments: `RangeError: Invalid array length`, before any element is read.
//! - A call with more arguments than fit Chrome's stack (about 120,000; 131,073 is refused, after
//!   every element is read; a spread call is refused at once): `RangeError: Maximum call stack size
//!   exceeded`. The bound here is 2^17.
//! - `Array.prototype.sort` over a sparse array: Chrome walks the whole length (still running after
//!   60 s at 2^32 - 1), so the check here is the memory it takes, not an error.
//!
//! The largest single allocation is counted per thread, so tests running beside it do not count.

use rustkit_js::{JsRuntime, JsValue};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    // Const-initialised: reading it never allocates, so the allocator can use it.
    static LARGEST: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    let _ = LARGEST.try_with(|largest| largest.set(largest.get().max(size)));
}

struct Largest;
unsafe impl GlobalAlloc for Largest {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        note(l.size());
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        note(n);
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Largest = Largest;

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

#[test]
fn a_call_with_more_arguments_than_chrome_takes_is_refused_at_once() {
    let refused = "RangeError: Invalid array length";
    let f = "var n = 0, o = { get 0() { n++; return 1; } };";
    for len in ["2 ** 32 - 1", "2 ** 27 + 1"] {
        assert_eq!(run(&format!("{f} o.length = {len}; try {{ (function () {{}}).apply(null, o); }} catch (e) {{ return e.name + ': ' + e.message + ' ' + n; }}")), format!("value {refused} 0"));
        assert_eq!(run(&format!("{f} o.length = {len}; try {{ Reflect.apply(function () {{}}, null, o); }} catch (e) {{ return e.name + ': ' + e.message + ' ' + n; }}")), format!("value {refused} 0"));
        assert_eq!(run(&format!("{f} o.length = {len}; try {{ Reflect.construct(function () {{}}, o); }} catch (e) {{ return e.name + ': ' + e.message + ' ' + n; }}")), format!("value {refused} 0"));
    }
    // Ordinary calls through an array-like are unchanged.
    assert_eq!(run("return (function () { return [].join.call(arguments, '-'); }).apply(null, { length: 3, 0: 'a', 1: 'b', 2: 'c' });"), "value a-b-c");
}

#[test]
fn sorting_a_sparse_array_reserves_nothing_for_its_length() {
    // 2^24 slots, 3 elements. A reservation for every slot would be 128 MiB (8 bytes each).
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script("var a = [3, 1, 2]; a.length = 2 ** 24; var o = { length: 2 ** 24, 0: 3, 1: 1, 2: 2 };").unwrap();
    LARGEST.with(|largest| largest.set(0));
    let got = rt
        .evaluate_script("a.sort(); Array.prototype.sort.call(o); [a[0], a[1], a[2], a[3], a.length, o[0], o[1], o[2], o.length].join()")
        .unwrap();
    let largest = LARGEST.with(Cell::get);
    assert!(matches!(&got, JsValue::String(s) if s == "1,2,3,,16777216,1,2,3,16777216"), "{got:?}");
    assert!(largest < 1 << 20, "largest single allocation while sorting: {largest} bytes");
}

#[test]
fn a_call_with_more_arguments_than_fit_is_refused() {
    let stack = "RangeError: Maximum call stack size exceeded";
    // apply reads every element first (Chrome: 131,073 getter calls), then refuses.
    assert_eq!(
        run("var n = 0, o = { length: 2 ** 17 + 1 }; for (let i = 0; i < 2 ** 17 + 1; i++) Object.defineProperty(o, i, { get() { n++; return i; } });
            try { (function () {}).apply(null, o); return 'ok'; } catch (e) { return e.name + ': ' + e.message + ' ' + n; }"),
        format!("value {stack} 131073")
    );
    // Chrome takes 2^16 too; this engine's VM stack (64K values) refuses calls that large today: a
    // separate row. 2^15 is taken by both.
    assert_eq!(run("return (function () { return arguments.length; }).apply(null, { length: 2 ** 15 });"), "value 32768");
    assert_eq!(run("return Math.max(...new Array(2 ** 17 + 1).fill(1));"), stack);
    assert_eq!(run("return new Array(...new Array(2 ** 17 + 1).fill(1)).length;"), stack);
    assert_eq!(run("return Math.max(...new Array(2 ** 15).fill(1));"), "value 1");
    assert_eq!(run("return Array.of.apply(null, { length: 2 ** 27 }).length;"), stack);
}

#[test]
fn a_proxy_own_keys_list_past_the_bound_is_refused_at_once() {
    // The trap's result goes through the same list builder: Chrome 148 refuses more than 2^27 keys
    // before reading one.
    assert_eq!(
        run("var n = 0, keys = { length: 2 ** 27 + 1, get 0() { n++; return 'a'; } };
            try { Reflect.ownKeys(new Proxy({}, { ownKeys() { return keys; } })); return 'ok'; }
            catch (e) { return e.name + ': ' + e.message + ' ' + n; }"),
        "value RangeError: Invalid array length 0"
    );
    assert_eq!(run("return Reflect.ownKeys(new Proxy({}, { ownKeys() { return ['b', 'a']; } })).join();"), "value b,a");
}

#[test]
fn a_typed_array_join_past_the_string_limit_is_refused() {
    let refused = "RangeError: Invalid string length";
    assert_eq!(run("return new Uint8Array(2 ** 20).join('x'.repeat(2 ** 12)).length;"), refused);
    assert_eq!(run("return new Uint8Array(2 ** 28).join('xx').length;"), refused);
    assert_eq!(run("return new Uint8Array([1, 2, 3]).join('-');"), "value 1-2-3");
    assert_eq!(run("return new Uint8Array(2 ** 20).join('').length;"), "value 1048576");
    // Chrome 148 joins with "," in toLocaleString too.
    assert_eq!(run("return new Uint8Array([1, 2, 3]).toLocaleString();"), "value 1,2,3");
}
