# Vendored boa_gc 0.20.0, with one backported fix

Source: crates.io `boa_gc` 0.20.0, unmodified except the lines marked
`HIWAVE PATCH` in `src/lib.rs` and `src/trace.rs`. Wired in through
`[patch.crates-io]` in the workspace `Cargo.toml`. License as upstream
(see `Cargo.toml`).

**Bug.** The collector's weak-mark phase (ephemerons and weak maps: steps 1, 2
and 3 of `Collector::mark_heap`) traced every queued node without checking or
setting its mark bit. A reference cycle reachable from a live WeakMap/WeakSet
entry's value was therefore traced forever, and the tracer queue grew until the
process ran out of memory. Real pages hit it: tripadvisor, squarespace, bmw and
toyota hung inside `Collector::collect` with RSS reaching many GB.

**Fix.** It's backported from upstream boa_gc 0.22. `Tracer::trace_until_empty`
skips marked nodes and marks each node before tracing it, and the three
weak-phase loops call it. This is the same shape as the strong-root loop above
them.

**Test.** `crates/rustkit-js/tests/gc_weak_cycle.rs` doesn't terminate on stock
0.20 and passes with this patch.

**Remove** this directory and the `[patch.crates-io]` entry when Boa is
upgraded to 0.22 or later, which contains the fix upstream.
