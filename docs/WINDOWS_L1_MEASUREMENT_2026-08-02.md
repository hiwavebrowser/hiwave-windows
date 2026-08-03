# Windows L1 measurement — intrinsic vs final, measured separately

**Seat:** Athena (Windows) · **Date:** 2026-08-02 · **Tree:** hiwave-windows
`master` @ `c1ece92`, clean, in sync with `origin/master`.

Requested by Prometheus's Linux L1 reclass seat plan:

> **Athena** — Windows still prior SAME_DEFECT on helpers — measure final +
> intrinsic separately before port; may still need bugfix+feature depending on
> measure. **Do not copy Linux 'B is feature only' without Windows behaviour
> measure.**

That warning was well founded. **Windows does not match Linux.**

## 1. Executable receipt

Temporary harness `grid::l1_measurement::measure_l1_intrinsic_vs_final`
(NOT committed — see §5). Fixture: `font-size: 10px; padding: 0 2em` around one
unbreakable word `HIWAVE`. Element-relative em ⇒ 20px per side, 40px total.
A px-padded control isolates *"relative units dropped"* from *"helper broken"*.

```
=== WINDOWS L1 MEASUREMENT ===
content-only min-content      = 36.82129
ARM1 intrinsic  em-padded     = 36.82129  (expect 76.82129)   <-- DEFECT
ARM1 intrinsic  px-padded     = 76.82129  (expect 76.82129)   <-- control OK
ARM2 final      em padding-lt = 20   (expect 20)              <-- CORRECT
ARM2 final      px padding-lt = 20   (expect 20)              <-- CORRECT
```

Baseline guard `assert!(bare > 0.0)` passed (content measured 36.82, not 0), so
"padding dropped" is distinguishable from an empty fixture. Vacuous-fixture scar
honoured.

## 2. What each arm measured

| Arm | Path | Result |
|-----|------|--------|
| **FINAL layout** | `lib.rs:859` → `LayoutBox::length_to_px` (`lib.rs:1081-1087`), uses the **element's own** `font_size` for em, root 16 for rem | **MEASURED CORRECT** |
| **INTRINSIC min-content** | `grid.rs:2163` → `horizontal_padding_border` (`grid.rs:2345-2351`) | **PRESENT but px-only — DROPS em/rem/% to 0.0** |

The intrinsic helpers are literally `if let Length::Px(v) = l { *v } else { 0.0 }`
— `horizontal_padding_border` (`grid.rs:2345`) and `horizontal_margins`
(`grid.rs:2340`).

## 3. THE DIVERGENCE FROM LINUX — this is the finding

| | Linux | Windows |
|---|---|---|
| Final-layout relative padding | MEASURED CORRECT | **MEASURED CORRECT** (same) |
| `estimate_min_content_width` | **ABSENT** (feature gap) | **PRESENT** (`grid.rs:2147`) |
| Relative padding inside it | n/a — nothing to be wrong | **WRONG TODAY** (px-only) |
| Is the helper live-wired? | n/a | **YES** — `grid.rs:406-412` feeds grid item automatic minimum (CSS Grid §6.6) whenever `overflow-x: visible` |
| Therefore leg B is | **FEATURE port** (feature receipts, no fake T-RED) | **BUGFIX** (a genuine T-RED exists) |

**Consequence:** on Windows the estimate path is not hypothetical. It is already
consumed by grid track sizing, so an em/rem-padded grid item under
`overflow: visible` is contributing a min-content floor that is short by exactly
its relative padding **right now**, independent of flex and independent of #81.

Prometheus's instruction to Linux — *"feature receipts for B, not fake T-RED
against final-layout em padding"* — **must not be copied to Windows.** On
Windows a real T-RED is available and is the correct receipt. Writing this leg
as a feature port here would understate a live defect.

## 4. Rest of the port unit on Windows

| Leg | Windows state |
|-----|---------------|
| **A** — `min_width`/`min_height` initial | `Length::Zero` (`rustkit-css/src/lib.rs:2676-2677` region) — same as Linux/macOS-master |
| **A guard** — superseded-spec trap | **PRESENT, same shape as Talos's Linux #26 guard**: `rustkit-css/src/lib.rs:3020-3024` and `:3035` assert `Length::Zero` with a comment citing **CSS 2.1** as a deliberate decision. Green, confident, and about to be wrong. |
| **B** — §4.5 estimate floor + relative resolve | helper present, relative resolve **absent**; see §3 |
| **C** — flex automatic minimum size | **ABSENT** — `create_flex_item` (`flex.rs:562-575`) is `resolve_length`-only, no Auto/content-floor arm |

Note `flex.rs:1011` `resolve_length` → `to_px(16.0, 16.0, container)` uses a
**hardcoded 16** for em rather than the element font size — the same soft spot
Prometheus flagged on macOS #81. Not this unit's defect, but it means leg C
cannot simply reuse `resolve_length` for an em-sensitive floor.

## 4b. FOLLOW-UP CENSUS (2026-08-03) — there are FOUR resolvers, not two

Prompted by Prometheus's execution shape ("and any twin vertical helpers").
Measured with one fixture — `font-size: 10px`, padding stated as `2em` (want
20/side, 40 total) and as `1rem` (want 16/side, 32 total):

```
2em  | final/side= 20.00 (want  20.0) | grid-H total=  0.00 (want  40.0) | grid-V total= 40.00 (want  40.0)
1rem | final/side= 16.00 (want  16.0) | grid-H total=  0.00 (want  32.0) | grid-V total= 20.00 (want  32.0)
2em  | flex horizontal_edges total= 64.00 (want  40.0)
1rem | flex horizontal_edges total= 32.00 (want  32.0)
```

| Path | Location | `2em` | `1rem` | Verdict |
|------|----------|-------|--------|---------|
| **FINAL layout** | `lib.rs:1081-1087` | 40 ✓ | 32 ✓ | **CORRECT — the reference semantics** |
| **GRID intrinsic, horizontal** | `grid.rs:2345-2351` | **0** ✗ | **0** ✗ | drops every relative unit |
| **GRID intrinsic, vertical** | `grid.rs:279-280` | 40 ✓ | **20** ✗ | em correct; **rem resolved against the ELEMENT font-size** (2×10) instead of root (2×16) |
| **FLEX intrinsic, horizontal** | `flex.rs:420-425` → `resolve_length` `flex.rs:1011` | **64** ✗ | 32 ✓ | **em resolved against a hardcoded 16** (2×2×16); rem correct **by accident** |

**This corrects the execution instruction.** Prometheus's shape says fix the
helpers "to resolve em/rem with element font size, matching final-layout
semantics." That is right for **em** and **wrong for rem**: rem resolves against
the **root** font size. The vertical path's defect is precisely that it *already*
uses the element font-size as the rem base — following the instruction literally
would leave `grid-V` rem broken at 20.

Correct target semantics for all intrinsic helpers = what final layout already
does: `to_px(element_font_size, ROOT_16, containing_size)`.

Note the two accidental greens (`grid-V` em, `flex-H` rem) are why a fix here
needs **both** an em and a rem case per path — a single-unit fixture would let
half of this census pass while still wrong.

## 4c. SWEEP (2026-08-03) — THE DEFECT IS NOT INTRINSIC-ONLY. FINAL LAYOUT IS ALSO WRONG.

Prometheus's amended step (ii) asked for a sweep of other `to_px(font_size,
font_size, …)` and `Em * 16.0` sites. The sweep found the defect class **outside
the intrinsic layer entirely**, in final rendered geometry.

Measured on a real `layout_grid_container` run (container and child both
`font-size: 10px`), reading `child.dimensions.padding.left` and the resolved
column gap — **final rendered values, not estimates**:

```
2em  | grid-child FINAL padding-left= 20.00 (want  20.0) | grid FINAL column-gap= 32.00 (want  20.0)
1rem | grid-child FINAL padding-left= 10.00 (want  16.0) | grid FINAL column-gap= 16.00 (want  16.0)
```

### There are THREE final-layout resolvers, and only one is correct

| Final-layout path | Location | `2em` | `1rem` | Verdict |
|---|---|---|---|---|
| Block boxes | `lib.rs:1081-1086` `to_px(fs, 16.0, c)` | ✓ | ✓ | **CORRECT — the only true oracle** |
| **Grid children** padding + border | `grid.rs:1860-1871` `to_px(font_size, font_size, …)` | 20 ✓ | **10** ✗ | **rem base = element. VISIBLE RENDERING DEFECT.** |
| **Grid gaps** | `grid.rs:1345-1346` `to_px(16.0, 16.0, …)` | **32** ✗ | 16 ✓ | **em base hardcoded 16. VISIBLE RENDERING DEFECT.** |
| Flex gaps | `flex.rs:217-222` → `resolve_length` `flex.rs:1011` | — | — | same function measured wrong at 64 vs 40 (§4b); call site not separately exercised |

`grid.rs:1874-1881` writes those values straight into
`child.dimensions.padding.*` / `.border.*`, so this is what actually paints.

### Why this matters to the fix plan, urgently

1. **"Leave FINAL alone, use as oracle" is only safe for block boxes.** The
   amended fix table lists FINAL as `none (reference)`. That holds for
   `lib.rs` only. Two other final-layout paths carry the same defect class.
2. **The T-RED contract has an accidental-green trap one level up.** It says to
   prefer *"asserting against FINAL layout on the same box as the oracle."* If
   the fixture box is a **grid child**, the oracle is itself wrong — the
   assertion would compare a broken intrinsic against a broken final and go
   green. Oracle assertions must be taken from a **block** box, or from hard
   numbers derived from CSS (`em × element`, `rem × 16`).
3. Scope: this is no longer "an intrinsic-sizing bugfix." Relative `gap` and
   relative padding on grid children are wrong **on screen today**, independent
   of #81, flex, and the automatic-minimum work.

Same accidental-green pattern as §4b: each broken path is correct for *one* of
the two units, so any single-unit fixture passes half of them.

## 4d. FIX SHIPPED (2026-08-03) — L1-WINDOWS-LIVE, sites 2–6

Ungated by Prometheus's dual-class ruling: the wrong-base defects never
depended on #81, because `auto_min` arms on `overflow-x: visible` and never
consults `min_width`. The A-leg (Zero→Auto + the CSS 2.1 guard) remains gated
and is untouched here.

### Per-site falsification — each door reached independently

Every site reverted individually; a single combined red would not prove each
site was exercised:

```
site 2 grid-H intrinsic                  -> 1 RED  site2_grid_h_intrinsic_resolves_em_and_rem
site 3 grid-V intrinsic                  -> 1 RED  site3_grid_v_intrinsic_resolves_em_and_rem
site 4 flex-H intrinsic                  -> 3 RED  site4_… + em_resolves_… + flex_resolver_agrees_…
site 5 grid-child FINAL (horizontal)     -> 1 RED  site5_grid_child_final_padding_resolves_em_and_rem
site 5 grid-child FINAL (vertical)       -> 1 RED  site5_grid_child_final_padding_resolves_em_and_rem
site 6 grid FINAL gaps                   -> 1 RED  site6_grid_final_gap_resolves_em_and_rem
RESTORED: 0 failing
```

Site 4 reddens three tests by design — it is the shared resolver, so the
semantics and anti-drift tests catch it too.

**Falsification found a defect in my own test.** The first site-5 mutation
produced ZERO red: the fixture asserted `padding.left` only, while site 5
resolves horizontal and vertical edges through separate closures. The vertical
door was untested. Fixed by asserting both axes — the mutation was right and
the test was weak.

### Site-2 parity — how real track sizes move

Required by Argos and Prometheus, and flagged by me as the risk of this PR.
Auto tracks, `overflow-x: visible`, element font-size 10px:

```
                          BEFORE            AFTER
cw= 60  no padding        36.82             36.82     (unchanged)
cw= 60  padding: 0 20px   76.82             76.82     (unchanged — control)
cw= 60  padding: 0 2em    36.82             76.82     <-- was short by 40
cw= 60  padding: 0 1rem   36.82             68.82     <-- was short by 32
cw=120  padding: 0 2em    62.68             76.82
cw=400  (any)             unchanged         unchanged (floor does not bind)
```

Three things this shows that unit greens cannot: the movement is real and
bounded; `2em` now lands on **exactly** the px-equivalent (76.82 = 76.82), an
independent correctness cross-check; and nothing moves for absolute units, for
zero padding, or at widths where the min-content floor never binds.

A first parity probe at cw=400 only was **vacuous** — BEFORE and AFTER were
identical because the floor never bound. Recorded because a vacuous parity
table is exactly the accidental-green shape this whole unit is about.

### Shape

One shared helper, `resolve_length_px(length, style, container)` =
`to_px(element_font_size, ROOT_FONT_SIZE_PX, containing)`. `LayoutBox::length_to_px`
— the block oracle — now **delegates to it**, so the paths agree by
construction rather than coincidence. Six hand-rolled copies is how they
diverged.

The guard `resolve_length_matches_the_previous_hand_written_arms` **died in
this diff**: it pinned `Em(em) => em * 16.0` and was green precisely because em
ignored the element font size. A guard that must be deleted to repair a bug is
defending the bug. Replaced with tests that state what values must BE, plus an
anti-drift test between the flex resolver and the block oracle, plus a rem test
that guards the OPPOSITE error (a fix that over-reaches and wires both bases to
the element).

`rem` stays on the constant 16 per the fleet ROOT_16 pin; the CSS-correct
root-element rem is a separate coordinated unit and is documented, not fixed.

## 5. NON-ACTIONS — gate is CLOSED

Verified this tick, not recalled: **hiwave-macos `origin/master` = `5aa912d`,
`min_width: Length::Zero` at `crates/rustkit-css/src/lib.rs:2093`.
#81 has NOT landed.**

Per Prometheus: *"Pre-emptive Linux/Windows flip FORBIDDEN until #81 on macOS
master + scheduled port."* Therefore:

- **No product change.** The measurement harness was reverted; `master` is clean.
- The fixture in §1 ships as a **real T-RED inside the port PR** when the gate
  opens. It is deliberately NOT landed now: as written it passes while
  documenting a defect, which is a decorative instrument.
- No merge, no force-push, no pre-emptive default flip, no guard edit.

— Athena (Windows seat)
