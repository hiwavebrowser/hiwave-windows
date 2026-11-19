//! JS-PERF2 item 2, the deterministic pin: `a[i]` and `a[i] = v` with a whole-DOUBLE key on a dense
//! array or a typed array must not allocate. On the base this was written against, the general path
//! turned a double key into a string (one allocation per access, measured 1.1 per read against 0.12
//! for an integer key) before parsing it back to an index; the shortcut takes the number directly.
//! (An integer key allocates nothing on either path, so integer keys are here as a guard, not as
//! the red; the saving there is clone and refcount work, which only the timing in the PR body shows.)
//! Counting allocations is deterministic where a timing is not; this is the only test in its
//! binary, so nothing else allocates while it counts.

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
fn indexed_reads_and_writes_do_not_allocate_for_their_key() {
    let mut rt = JsRuntime::new().unwrap();
    rt.evaluate_script(
        "var a = []; for (var i = 0; i < 1000; i++) a.push(i); var u = new Uint16Array(1000); var f = new Float64Array(1000);",
    )
    .unwrap();
    let mut per_iter = |body: &str| -> f64 {
        let src = format!("(function(){{ var s = 0; for (var i = 0; i < 1000; i++) {{ {body} }} return s; }})()");
        rt.evaluate_script(&src).unwrap(); // warm up
        let before = ALLOCS.load(Ordering::Relaxed);
        for _ in 0..20 {
            rt.evaluate_script(&src).unwrap();
        }
        (ALLOCS.load(Ordering::Relaxed) - before) as f64 / 20_000.0
    };
    let base = per_iter("s += i;");
    // `(i + 0.5) - 0.5` is a whole double, not an int32.
    let cases = [
        ("array read, int key", "s += a[i];"),
        ("array read, double key", "s += a[(i + 0.5) - 0.5];"),
        ("array write, int key", "a[i] = i;"),
        ("array write, double key", "a[(i + 0.5) - 0.5] = i;"),
        ("Uint16Array read, int key", "s += u[i];"),
        ("Uint16Array read, double key", "s += u[(i + 0.5) - 0.5];"),
        ("Uint16Array write, int key", "u[i] = i;"),
        ("Uint16Array write, double key", "u[(i + 0.5) - 0.5] = i;"),
        ("Float64Array read, double key", "s += f[(i + 0.5) - 0.5];"),
        ("Float64Array write, double key", "f[(i + 0.5) - 0.5] = i;"),
    ];
    let mut report = Vec::new();
    let mut over = Vec::new();
    for (name, body) in cases {
        let v = per_iter(body);
        report.push(format!("{name}: {v:.2}"));
        if v > base + 0.3 {
            over.push(format!("{name}: {v:.2} allocations per access, the empty loop {base:.2}"));
        }
    }
    println!("allocations per iteration (empty loop {base:.2}): {}", report.join(", "));
    assert!(over.is_empty(), "{}", over.join("; "));
}
