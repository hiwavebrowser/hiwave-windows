# Ledger: last-child bottom-margin over-inclusion (Windows)

**Status:** KNOWN DIVERGENCE — deliberately not fixed.
**Ruling:** Prometheus, 2026-07-30 — *LEAVE IT (ledger divergence) · fix only via
Option 3 (real parity board) · REJECT fixing on unit receipts alone.*

## What Windows does

At the end of `layout_block_children_with_collapse`, the last in-flow child's
bottom margin is materialized into the parent's content height **unconditionally**.

Measured (parent + one child 20px tall with `margin-bottom: 10px`, viewport 800):

| Parent | Windows content height | Spec / macOS |
|---|---|---|
| `padding-bottom: 5px` (collapse **blocked**) | 30 — margin included | included ✓ |
| no padding/border/height (collapse-**through**) | 30 — margin included | **excluded** |

## Why that is wrong

CSS 2.1 §8.3.1: when a parent establishes no bottom boundary — no bottom
padding, no bottom border, `height: auto` — the last in-flow child's bottom
margin **collapses with the parent's own bottom margin**. It therefore belongs
*outside* the parent's content height, propagating upward. Keeping it inside is
over-inclusion.

macOS reaches the same conclusion from the opposite direction: it had the margin
*missing* when collapse was blocked, fixed that, and deliberately left the
collapse-through case dropping the margin (their comment ledgers it as "a smaller
residual").

So the two trees **agree** on the blocked case and **diverge** on collapse-through.
Windows is not behind here — it is wrong the other way.

## Why it is not being fixed now

1. **Blast radius.** It shortens every unpadded, unbordered, auto-height container
   whose last child carries a bottom margin — one of the commonest shapes on the
   web. Not a narrow correction.
2. **No trustworthy receipt available on this seat.** The change needs a parity
   number. `scripts/parity_test.py` defaults a case to `100.0` when capture yields
   nothing, and this seat has no usable GPU adapter headless — so any local board
   figure measures the runner, not the renderer.
3. **Suspected coupling to form-control metrics.** The macOS source comments that
   "the form-control bare-height blobs had calibrated themselves against the
   deficit." Windows form-control heights (PR #27) were calibrated against
   *Windows* behaviour. If Windows has been over-including this margin all along,
   some of those constants may be absorbing the surplus — so changing the collapse
   rule could move form-control metrics as a side effect. That must be measured,
   not assumed absent.

## Unblocking condition

A trustworthy parity board on the Windows axis — i.e. a GPU-capable runner, or a
capture harness that reports **no-result** instead of defaulting to `100.0`.
Until then this stays ledgered.

## Guard

`c1_last_child_margin_tests` in `crates/rustkit-layout/src/lib.rs` are
**characterization tests**: they assert current behaviour, including the arm that
is spec-wrong, so that an accidental change is caught and has to be deliberate.
They are not an endorsement. If the divergence is fixed, the
`unpadded_parent_*` expectation is the one that must flip.

## Provenance

Found while attempting Rail C1 (wire `should_collapse_with_last_child`). The
T-RED written against the macOS pre-fix defect **passed on unfixed Windows
source** — which meant the premise was wrong, not that the work was done.
