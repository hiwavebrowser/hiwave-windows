//! A parenthesized expression is not copied when it is parsed (`paren-no-clone`, see
//! third_party/boa_engine/PATCHES.md).
//!
//! Boa parsed `( E )` into a list, then built `Parenthesized` from a deep clone of `E` and dropped
//! the list, so for a moment the whole of `E` existed twice and every node of it was allocated
//! twice. A bundle wrapped in one `(function(){ ... })` (YouTube's 10.9 MB kevlar_base.js) peaked
//! at 808 MB of heap where the finished AST is 434 MB.
//!
//! The pin compares `x = E;` with `x = (E);` for one large `E` under a counting allocator: the
//! parentheses may cost a few allocations, not a second copy of `E`.
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

use boa_engine::ast::scope::Scope;
use boa_engine::interner::Interner;
use boa_engine::parser::{Parser, Source};

static CUR: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);

struct Counting;
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            grow(layout.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        unsafe { System.dealloc(p, layout) };
        CUR.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, p: *mut u8, layout: Layout, new: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, layout, new) };
        if !q.is_null() {
            CUR.fetch_sub(layout.size(), Relaxed);
            grow(new);
        }
        q
    }
}
fn grow(n: usize) {
    let now = CUR.fetch_add(n, Relaxed) + n;
    PEAK.fetch_max(now, Relaxed);
    COUNT.fetch_add(1, Relaxed);
}
#[global_allocator]
static ALLOC: Counting = Counting;

/// (allocations, peak heap above the start) of parsing `src` as a script.
fn parse_cost(src: &str) -> (usize, usize) {
    let mut interner = Interner::default();
    let scope = Scope::new_global();
    let start = CUR.load(Relaxed);
    PEAK.store(start, Relaxed);
    let count = COUNT.load(Relaxed);
    let script = Parser::new(Source::from_bytes(src))
        .parse_script(&scope, &mut interner)
        .expect("parses");
    let cost = (COUNT.load(Relaxed) - count, PEAK.load(Relaxed) - start);
    drop(script);
    cost
}

fn big_function() -> String {
    let mut body = String::new();
    for i in 0..2000 {
        body.push_str(&format!(
            "a.b{i} = c(d, {i}, function () {{ return e[{i}] + f.g; }});\n"
        ));
    }
    format!("function (a, c, d, e, f) {{\n{body}}}")
}

#[test]
fn parenthesized_expression_is_not_copied() {
    let e = big_function();
    let plain = format!("x = {e};");
    let wrapped = format!("x = ({e});");
    // Warm the allocator and any lazy statics once.
    parse_cost(&plain);
    let (plain_allocs, plain_peak) = parse_cost(&plain);
    let (wrapped_allocs, wrapped_peak) = parse_cost(&wrapped);
    eprintln!(
        "plain: {plain_allocs} allocations, peak {plain_peak} B; wrapped: {wrapped_allocs} allocations, peak {wrapped_peak} B"
    );
    // A copy of `E` doubles both; the parentheses themselves cost a handful of allocations.
    assert!(
        wrapped_allocs <= plain_allocs + 64,
        "`(E)` allocated {wrapped_allocs} times, `E` {plain_allocs}: the parenthesized expression was copied"
    );
    assert!(
        wrapped_peak <= plain_peak + plain_peak / 10,
        "`(E)` peaked at {wrapped_peak} B, `E` at {plain_peak} B: the parenthesized expression was copied"
    );

    // The node built from the moved expression is the one the clone made. (One test function: the
    // counters are process-wide, so a second test running alongside would skew them.)
    let mut interner = Interner::default();
    let scope = Scope::new_global();
    let script = Parser::new(Source::from_bytes("x = (a + (b * (c, d)));"))
        .parse_script(&scope, &mut interner)
        .expect("parses");
    use boa_engine::interner::ToInternedString;
    // (Boa prints a comma expression as `c , d`.)
    assert_eq!(
        script.to_interned_string(&interner).trim(),
        "x = (a + (b * (c , d)));"
    );
}
