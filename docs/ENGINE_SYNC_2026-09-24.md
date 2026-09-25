# Engine sync — hiwave-windows adopts the hiwave-macos engine crates

**Date:** 2026-09-24 · **Seat:** Athena (Windows) · **Mandate:** Pete, in-session:
"get on par with the macOS version … as much goodness as you can without
breaking hiwave-windows from mac differences."

## The measurement that decided the shape

Against a worktree of `hiwavebrowser/hiwave-macos` develop `5207ea1`
(2026-09-24), with hiwave-windows develop `36c3b75` (2026-08-08):

| crate | Windows lines | macOS lines | identical files | Windows-only files |
|---|---:|---:|---:|---|
| rustkit-layout | 14,100 | 32,411 | 1 | 0 |
| rustkit-engine | 7,198 | 15,415 | 0 | 0 |
| rustkit-renderer | 3,183 | 9,371 | 2 | 0 |
| rustkit-text | 2,290 | 3,890 | 3 | `linux.rs` |
| rustkit-css | 3,197 | 3,928 | 0 | 0 |
| rustkit-compositor | 870 | 1,317 | 0 | 0 |
| rustkit-viewhost | 2,358 | 2,576 | 0 | `screenshot.rs`, `linux.rs` |

92 macOS PRs merged to develop since the Windows tree last moved. Every
Windows "W-lane" PR in July–August was itself a port *from* macOS, so the
shared crates on Windows are a stale **subset** of the macOS ones: the only
Windows-only content in them is 65 inline test functions (engine 38, layout
16, dom 5, renderer 3, css 2, text 1).

The macOS crates are still cross-platform by construction: `cfg(windows)`
dependency blocks, a byte-identical `rustkit-text/src/win.rs`, a
`#[cfg(windows)]` branch in the renderer's glyph cache, 40 Windows cfg sites
in viewhost. Nobody had built them on Windows for weeks.

**Experiment:** Windows develop with all 30 `rustkit-*` crates + `parity-capture`
replaced by the macOS ones.

| build | result |
|---|---|
| `cargo build -p parity-capture` | ✅ (after two `cfg(windows)`-only bit-rot hunks, upstreamed as hiwave-macos#233) |
| `cargo build -p hiwave-app` (WebView2 shell) | ✅ |
| `cargo build -p hiwave-app --features native-win32` | 8 errors — all Engine methods that exist only in the Windows engine |
| staged sync (leaf crates only, Windows layout kept) | ❌ 11 errors in rustkit-layout (css `Gradient`/`LineHeight` types restructured) — the rendering core must move together |

So the port is one mechanical crate sync plus a **finite, enumerated seam**,
not ~60 behaviour ports into a fork half the size. The review unit for this
PR is the seam — the diff of this tree against hiwave-macos develop — not
the 52k-line diff against Windows develop.

## The seam (Windows additions carried forward)

| id | what | where | why it exists |
|---|---|---|---|
| S0 | `#[cfg(windows)] use tracing::error;` · engine `handle_key_event` key table | viewhost, engine | Windows bit-rot in the macOS tree; upstream PR hiwave-macos#233 |
| S1 | `rustkit-viewhost/src/screenshot.rs` + `pub mod screenshot` + `png` dep | viewhost | Windows GPU readback for the shell's screenshot harness |
| S2 | `Renderer::execute_and_capture` (PNG + JSON sidecar), `RenderStats`, `get_render_stats`; `Engine::{get_render_stats, capture_view_screenshot, get_view_hwnd}` | renderer, engine | native-win32 shell + hiwave-smoke call these; macOS parity-capture uses the PPM path |
| S3 | `SessionHistory`-canonical `NavigationStateMachine`; `Engine::{go_back, go_forward, reload}` via replace-disposition loads (Windows #81); generation-gated `Engine::stop` with four await gates (Windows #80) | core, engine | back/forward/reload/stop are "OURS" on Windows — the macOS engine only exposed `can_go_back/forward`. Ported by applying the #80/#81 diffs (never snapshot copies). Adds one gate the Windows body did not have: after subresource loading, so a stop during a stylesheet/image fetch cannot finish the navigation. |
| S4 | DirectWrite glyph rasterization in the macOS glyph cache's `#[cfg(windows)]` branch, with the ClearType-3x1 fallback (hiwave-windows #7) | renderer `glyph.rs` | the macOS branch was a bordered-box placeholder — Windows text would be tofu |
| S5 | `Compositor::is_headless` | compositor | **not carried** — no caller anywhere in the Windows tree |
| S6 | the 65 Windows-only tests, re-homed into the new crates | css, dom, engine, layout, renderer, text | the Windows contract; a red one means a Windows fix to re-port |

Everything in S1–S4 is `#[cfg(windows)]`-gated, so the crates stay
behaviour-identical on macOS and can be re-synced by diff.

**2026-09-25:** the seam is gone. S0–S4 and the fixes S6 found were upstreamed
to hiwave-macos as #233, #234, #235, #236, #238, #239, #240 (all merged, all in
release V1.1.0) and #242 (open, next release). The crates in this tree are now
a verbatim copy of hiwave-macos tag `V1.1.0` (`2c17931`, develop `6e508fd`),
plus only what V1.1.0 does not yet carry:

- the #242 test modules (`windows_engine_pins`, `windows_a_leg_pins`,
  `windows_flex_pins`, `border_radius_emit_tests`, `windows_parser_pins`,
  `windows_shadow_pins`) and its two parser fixes (elliptical `border-radius`
  takes the horizontal radii; a single-value gradient position keeps y centred);
- `windows_capture_metadata_pins` in `rustkit-renderer/src/screenshot.rs`
  (the capture-sidecar pin; `cfg(windows)`).
- the `cfg(target_os = "macos")` gate on the four strut pixel tests in
  `rustkit-layout/src/lib.rs` (hiwave-macos #264, open).

When #242 lands, the next re-sync drops the first bullet and this tree becomes
`hiwave-macos develop` byte-for-byte except the sidecar pin.

## Gates for this PR

- Windows develop baseline (36c3b75, rustc 1.90): 73 test binaries,
  **1012 passed / 0 failed / 5 ignored**. Must hold, or every moved/replaced
  test is named.
- Both shells build; `hiwave-smoke` and `tools/render-test` build.
- The native shell renders `https://example.com` and the about page with
  real glyphs (render-test smoke; Pete eyeball).
- Numbers go in the PR body, never as committed run outputs (macOS #220).

## Gate results (2026-09-25, this tree, rustc 1.90)

| gate | result |
|---|---|
| full suite (`cargo test --workspace --no-fail-fast`, 43 test binaries + 34 doc-test targets) | **1387 passed / 4 failed / 5 ignored** (baseline 1012 / 0 / 5) |
| the 4 reds | `rustkit-layout` `tests::{a_line_sums_whole_pixel_ascents_like_blink, baseline_aligned_atomic_still_extends_strut, textarea_alone_on_a_line_hangs_the_strut_descent_below_it, wrapped_inline_block_hangs_the_line_off_its_last_line}` — pixel expectations calibrated on the macOS system font; identical reds on hiwave-macos develop when run on Windows. **Decided (Prometheus, 2026-09-25): gated `cfg(target_os = "macos")`** — upstream hiwave-macos #264; the same four-line gate is carried here as a Windows-side extra until #264 lands. After the gate: `rustkit-layout` 488 / 0 |
| full suite after the gate | **1387 passed / 0 failed / 5 ignored** (the four are compiled out on Windows) |
| `cargo build -p hiwave-app` (WebView2) | ✅ |
| `cargo build -p hiwave-app --no-default-features --features native-win32` | ✅ |
| `cargo build --release -p parity-capture` | ✅ |
| native-win32 render-test `https://example.com` | real glyphs (DirectWrite); the GPU content PNG is pixel-identical to the 2026-09-24 seam-tree capture (0 differing pixels of 1060×800) |
| native-win32 render-test, dark page (`#1a1a2e` body, `#5a5a76` / `#ff0000` boxes) | reads back (26,26,46), (90,90,118), (255,0,0) — exact; the Windows sRGB re-encode fix is not needed on the linear render targets |

Receipts: `P:
epos\hiwave-renders\engine-sync-2026-09-25\` (PNGs + sidecars),
attached to the PR. Not committed (macOS #220).

## What comes after

- Port `scripts/parity_test.py` + `wpt_tier1.py` from macOS so Windows has a
  pixel board (this box has a GPU; CI runners do not — the README's "not
  measured on Windows" stays true for CI until a GPU runner exists).
- Re-sync cadence: macOS merges ~1 engine PR/day. Re-sync = copy crates,
  re-apply the seam by diff, run the gates. The seam is the maintenance cost.
- The macOS refactor plan (REFACTOR_PLAN.md, approved 2026-09-23) will split
  `engine/lib.rs` and `layout/lib.rs`; the next re-sync after that lands is
  the expensive one. Long-term the honest fix is one shared engine repo —
  Pete's call, not this PR's.
