# Windows PR design gate — #38 + #39 APPROVE (2026-07-29)

**Seat:** Prometheus (design / outside-eye)  
**Scope:** hiwave-windows open PRs that need design rulings this tick  
**Not from this seat:** merge, force-push, spend, delete

---

## 0. One-screen scoreboard

| PR | Title | Ruling | Why |
|----|--------|--------|-----|
| **#38** | rem before em (silent drop) | **APPROVE merge** | Live fidelity bug; macOS recipe; dual T-RED; no blast radius |
| **#39** | Vw/Vh/Vmin/Vmax + `to_px_with_viewport` | **APPROVE merge** | Length PROMOTE slice; Copy preserved; parse inert; pin-compliant |
| **#34** | first metrics workflow | **APPROVE merge** | First CI signal; parity pixel intentionally omitted (honest); green on PR |
| **#33** | MemoryCache module port | **HOLD** | Pre-§1 shape; unpartitioned `CacheKey`; wait pathfinder post-#67 + §2.1 |
| **#31** | intrinsic_cache module + TEST_LOCK | **APPROVE module-only** | No production consume; C3 wire still DEFER |

**Merge order (Atlas):** `#38` → `#39` → `#34` → `#31`. Do **not** merge `#33` until rebased onto post-#67 macOS module + §2.1 product line.

**macOS #65:** prior Prometheus CLEAR (`4d8ec2c1429e`) stands — Atlas merge lane, not re-reviewed this tick.

---

## 1. #38 — rem silent drop — APPROVE

### Independent verify
- Mechanism matches macOS comment at `parse_length` (rem before em because `"2rem".ends_with("em")`).
- Diff is +30 / 0: rem arm + comment + two tests only.
- Tests cover both failure modes of a bad fix:
  - `2rem` / `0.5rem` / `-1rem` → `Length::Rem`
  - `2em` still → `Length::Em` (rem-first must not swallow em)

### Design notes
- Not a wrong-value bug — declaration dropped entirely (`?` bails). Highest-value small fix in the batch.
- Independent of #39 (parallel branches; 38 is not an ancestor of 39). Land either order; prefer **#38 first** so rem works before viewport type work stacks.

### Non-goals / not required
- No parser expansion beyond rem order.
- No force-push to fix the commit-message backtick loss (PR body is source of truth; already accepted).

---

## 2. #39 — viewport units (Length PROMOTE) — APPROVE

### Prior pin
`450f05db136a` confirmed the **Copy-boundary split**: ship Vw/Vh/Vmin/Vmax only; Min/Max/Clamp = separate Copy-removal PR. This gate is the merge verification of that pin on the shipped tip.

### Independent verify (origin/athena/w-length-viewport-units)

| Check | Result |
|-------|--------|
| `Length` still `Copy` | **YES** — `#[derive(Debug, Clone, Copy, PartialEq, Default)]` |
| Math matches macOS | Vw/Vh = % of axis; Vmin/Vmax = min/max of axes — same as macos `to_px_with_viewport` |
| `to_px` zero-viewport default | Delegates `(0.0, 0.0)` — macOS-identical |
| Parser untouched | `parse_length("50vw")` / `"10vmin"` pinned `None` |
| flex `resolve_length` | Explicit `Vw\|Vh\|Vmin\|Vmax => 0.0` (not `_`); DEFER comment for viewport-threaded flex |
| Min/Max/Clamp | **Omitted** — correct split |
| `todo!` / `unreachable!` / catch-all `_` | **None** on new arms |

### Behaviour claim (load-bearing)
This PR is **type-complete, parse-inert**: stylesheets do not parse `vw` yet. No layout number moves on real pages from parser input. Flex arms returning 0.0 match zero-viewport `to_px`. **Pete eyeball not required for merge** on behaviour-parity grounds (no live layout move from author CSS).

When a later PR wires `parse_length` for vw/vh/vmin/vmax, that PR **must** update the inert canary test and is the first behaviour-change surface — treat as non-inert.

### Copy-removal follow-on (still separate)
Title must say **Remove Copy from Length; add Min/Max/Clamp**. Required receipt unchanged from `450f05db136a`: ~216 `Length::` refs / 12 files; FlexBasis + VerticalAlign cascade; Box shape one-liner; explicit arms; no stubs.

---

## 3. #34 — metrics collection CI — APPROVE

- First workflow this repo has ever had; unblocks signal for every later PR.
- **Correct omission:** parity pixel-diff not collected on hosted runners (would publish instrument-as-score — the same class #65 fixed on macOS). Omission is explicit in summary/JSON, not silent.
- Local collector bug (stderr/stdout interleave) fixed via `STDOUT` merge — real; keep that discipline.
- Checks: `collect-metrics` green on the PR.

Residual (non-blocking): when a self-hosted GPU Windows runner exists, parity metric can join under the three-state instrument contract (#65), never as decorative 100.

---

## 4. #33 — MemoryCache port — HOLD

Module-only surface (`cache.rs` + `pub use`) — **no ResourceLoader wire** observed. That matches the old "module-only stands" arm, but:

1. Tip still carries pre-§1 decorative config shape (`default_ttl` / `respect_cache_control` as fields without the post-#67 accessor discipline).
2. `CacheKey { url, method }` remains **unpartitioned** — the §2.1 headline defect.
3. C2 wire remains **HARD HOLD** until §2.1 Pete YES + §2.2–2.4 on pathfinder.

**Ruling:** do **not** merge #33 as-is. Rebase/re-port from macOS **after** #67 module shape (and preferably after §2.1 lands on pathfinder), still module-only until wire is authorized. Shipping the pre-fix module to Windows multiplies the unpartitioned key across trees for no gain.

---

## 5. #31 — intrinsic_cache — APPROVE module-only

- New module + `pub use IntrinsicSizingMode` + TEST_LOCK tests.
- Production layout does **not** call lookup/store (C3 consume still DEFER / HARD NO invent).
- TEST_LOCK is the correct Windows addition for test races (Talos should mirror when porting).

**Wire of C3 remains DEFER** — this PR does not authorize consume. Pollux: execute-count on module tests only.

---

## 6. §2.1 critical path (standing — not closed this tick)

Pete elevated §2.1 to critical path (Atlas `a5f44c51f002`). Prometheus design still **YES**: double-key `(top-level eTLD+1, url, method)`.

| Item | State |
|------|--------|
| Design recommendation | YES (stands) |
| Pete product line | **Still required before code** |
| §2.2–2.4 | Landed on macOS #67 |
| C2 wire any platform | HARD HOLD until §2.1 code on pathfinder |
| This tick | Does not re-litigate; does not start §2.1 implementation |

---

## 7. Seat asks

### Atlas
1. Merge **#38 → #39 → #34 → #31** on green when ready (this seat does not merge).
2. macOS **#65** still CLEAR — merge when CI shard complete.
3. **#33 HOLD** until rebased post-#67 (+ §2.1 ideally).
4. §2.1 remains Pete-gated for product YES; do not open implement PR without it.

### Athena
1. After #39: **Length Copy removal** is the next non-inert Windows Length unit (fresh session; no half-apply).
2. Do **not** wire C2 or C3.
3. BackgroundImage/Layer still HELD with Gradient co-port (prior pin).

### Pollux
1. Spot-check #39: Copy still derives; `parse_length("50vw")==None`; flex arms exhaustive non-wildcard.
2. #38: both rem and em tests execute.

### Talos
1. **rem-before-em** check on hiwave-linux `parse_length` still priority if not shipped.
2. intrinsic_cache mirror may use Athena TEST_LOCK pattern after #31 lands.

### Pete
1. Optional: one-line YES/NO on §2.1 double-key (critical path). No other design ask on this gate.
2. No merge/scrub/force-push authorized by this note.

---

## 8. What Prometheus is not doing

- Not merging any PR.
- Not implementing §2.1 or Copy removal.
- Not re-pinning Gradient DEFER / Background carve / C2 wire HOLD.
- Not re-reviewing macOS #65 CLEAR.
