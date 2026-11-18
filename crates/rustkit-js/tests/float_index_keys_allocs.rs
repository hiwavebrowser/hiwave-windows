//! JS-PERF1: a whole-number double used as `obj[n]` must not allocate a string for its key.
//! Profiled on CodeMirror's first parse (3,404 engine samples): number keys turned into strings
//! and parsed back were ~7%. Counting allocations is deterministic where a timing is not; this is
//! the only test in its binary, so nothing else allocates while it counts.

use rustkit_js::JsRuntime;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        unsafe { System.realloc(p, l, n) }
    }
}
#[global_allocator]
static A: Counting = Counting;

#[test]
fn float_keys_cost_about_what_integer_keys_cost() {
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script("var a = []; for (var i = 0; i < 1000; i++) a.push(i);").unwrap();
    let mut per_iter = |body: &str| -> f64 {
        let src = format!("(function(){{ var s = 0; for (var i = 0; i < 1000; i++) {{ {body} }} return s; }})()");
        rt.evaluate_script(&src).unwrap(); // warm up
        let before = ALLOCS.load(Ordering::Relaxed);
        for _ in 0..20 {
            rt.evaluate_script(&src).unwrap();
        }
        (ALLOCS.load(Ordering::Relaxed) - before) as f64 / 20_000.0
    };
    let int = per_iter("s += a[i];");
    let float_whole = per_iter("s += a[(i + 0.5) - 0.5];");
    // The native call allocates by itself (its argument list); what the key adds is the difference.
    let floor_call = per_iter("s += Math.floor(i / 2);");
    let floor_key = per_iter("s += a[Math.floor(i / 2)];");
    println!("allocs per iteration: int {int:.2}, float {float_whole:.2}, Math.floor call {floor_call:.2}, as a key {floor_key:.2}");
    // Integer keys allocate ~0.1 per iteration here (the loop itself). A float key used to add one
    // more (the string) on top of its source; allow the baseline plus a little noise.
    assert!(float_whole <= int + 0.3, "float key: {float_whole:.2} allocations per read, integer key {int:.2}");
    assert!(floor_key <= floor_call + 0.3, "Math.floor key: {floor_key:.2} per read, the call alone {floor_call:.2}");
}
