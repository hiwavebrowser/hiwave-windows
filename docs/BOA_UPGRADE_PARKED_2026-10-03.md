# Boa upgrade: parked until after A5 (2026-10-03)

**Status: parked.** Not part of the Z phase (Atlas, 2026-10-03: "the Boa 0.22
upgrade stays out of Z"). This note records what is known so the work can be
scoped later; nothing below has been tried.

## Why it exists

The engine runs on `boa_engine` 0.20.0 (December 2024). Upstream has shipped
0.21.0 (2025-10-21), 0.21.1 (2026-03-29) and 0.22.0 (2026-08-28). Two defects
this phase hit are fixed upstream and had to be handled locally:

| defect | local handling | upstream |
|---|---|---|
| `boa_gc` weak-mark phase traced a reference cycle forever (OOM on real pages) | `third_party/boa_gc` (vendored, patched) | not re-checked |
| `let of = ...` rejected by the parser (github's `react-core.js`, `landing-pages.js`) | `third_party/boa_parser` (vendored, one changed line) | boa-dev/boa#4593, in 0.22.0, not in 0.21.1 |

Each vendored crate is a maintenance cost and a place for the next fix to be
missed. The honest fix is to upgrade and delete both.

## What an upgrade touches (to scope, not yet checked)

Every item here is a thing to **verify**, not a finding:

- `rustkit-js` is the only crate that names Boa types. It uses `Context`
  (builder, `eval`, `run_jobs`, runtime limits), `JsValue` conversion,
  `Source`, the `Module` / `ModuleLoader` / `Referrer` API (module host, C0),
  `JsPromise` / `PromiseState`, and the loop-iteration limit. Boa's module and
  job-queue APIs changed across 0.21 and 0.22 in ways not yet read.
- The Windows 16 MB main-thread stack (hiwave-windows #104) and the Boa
  recursion limit (512) interact: native recursion overflows before Boa's own
  limit. A new Boa may change per-call stack use either way.
- `third_party/boa_gc` carries a patch against 0.20.0; whether 0.22 still needs
  it is unchecked.
- Performance: the cascade and script-budget numbers (Boa runs about 1 s per
  MB of script here) would need a fresh matched-workload measurement (Z2-M1's
  `cascade_bench.py` and the real-site script-log A/B).

## Gates an upgrade would need

The whole of `cargo test --workspace` on all three platforms, the 26-case
campaign (identical or every mover explained), the 20-site script-log A/B
(throw counts and script outcomes, with the over-budget noise stated), a
github start-up run, and a before/after for the GC cases that motivated the
patch (tripadvisor, squarespace, bmw per `third_party/boa_gc/HIWAVE_PATCH.md`).

## Removal checklist for the vendored crates

1. Upgrade `boa_engine`.
2. Delete `third_party/boa_parser` and `third_party/boa_gc` (after confirming
   0.22 does not need the GC patch).
3. Delete the two `[patch.crates-io]` lines and the `exclude` entries in the
   workspace `Cargo.toml`.
4. Keep `crates/rustkit-js/tests/let_of.rs`: it is the regression test that
   would catch the parser defect returning.
