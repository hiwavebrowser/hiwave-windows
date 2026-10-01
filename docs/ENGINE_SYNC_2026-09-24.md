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

**2026-09-26 (refresh #2, after #89 and #90 merged):** #242, #264 and #265
landed upstream, so all of the above dropped out. Every `crates/rustkit-*`
crate and `parity-capture` is now a verbatim copy of hiwave-macos develop
`bf806c5` (Merge #278). The residual against that tip is **one file**:
`rustkit-renderer/src/screenshot.rs`, the 29-line `cfg(all(test, windows))`
`windows_capture_metadata_pins` module. This refresh brings hiwave-macos
#245–#278 to Windows: page scripts on the load path with a fetch-inclusive
budget, per-phase subresource deadlines, CSS escapes in selectors and
cssparser, @media evaluation, `object-fit`, `visibility` (+ the `:defined`
stopgap), logical margin/padding/inset, `display` keywords, justify-items /
justify-self, image percentage heights, and the selector / cascade /
text-measure / shape memoisation work.

Gate results (Windows, rustc 1.90, this tree):

| gate | result |
|---|---|
| `cargo test --workspace --no-fail-fast --exclude rustkit-engine` | **1298 passed / 0 failed / 5 ignored** |
| `cargo test -p rustkit-engine` (parallel, and again with `--test-threads=1`) | **125 passed / 0 failed** both ways |
| total | **1423 passed / 0 failed / 5 ignored** (refresh #1 was 1387 / 0 / 5) |
| both shells, release `parity-capture` | build |
| native-win32 render-test `https://example.com` | GPU content PNG pixel-identical to the 2026-09-25 capture (0 of 1060×800 differ); all three sidecars now carry the real date (#265 + #90) |
| native-win32 dark-page smoke | (26,26,46), (90,90,118), (255,0,0) — exact |

Why the engine crate is listed separately: on this box the `rustkit-engine`
test binary has hung intermittently (100 % CPU, no progress) in three
workspace runs across two days, once with nothing else running; the same
binary passes alone in 8–17 s, serial or parallel, and Windows CI (no GPU)
has never seen it. It is a local GPU/device-init interaction between
parallel headless-view tests, not a tree problem; the split keeps the
receipt honest and points at the flake if it recurs.

## Gates for this PR

- Windows develop baseline (36c3b75, rustc 1.90): 73 test binaries,
  **1012 passed / 0 failed / 5 ignored**. Must hold, or every moved/replaced
  test is named.
- Both shells build; `hiwave-smoke` and `tools/render-test` build.
- The native shell renders `https://example.com` and the about page with
  real glyphs (render-test smoke; Pete eyeball).
- Numbers go in the PR body, never as committed run outputs (macOS #220).

**2026-09-30, later (parity to 1 point, on top of refresh #8):** Pete lowered
the bar to "within 1% of macOS measurements", read as every case within 1.0
point of Mac's number. Two cases missed on the refresh #8 board:
`gradient-no-radius` +1.34 and `image-gallery` +1.07. Both were engine bugs,
both cross-platform in origin, both sent upstream, and both carried here until
they land, as patch files under `scripts/windows-patches/` that
`apply_windows_patches.py` applies after the string-edit patches:

- **`0001-line-fit-epsilon.patch` (hiwave-macos #388, landed; file deleted in the next PR).** A shrink-to-fit box
  is sized from its text's max-content width, which comes back to the line
  breaker a few f32 ulps smaller after `(width + padding) - padding` and the
  flex sizing arithmetic. The exact `<=` then wrapped text measured to fit its
  own box: "to right Pink-Blue" broke at the hyphen. The three fit comparisons
  now allow 1/64 px, Chrome's LayoutUnit. gradient-no-radius 1.86 to 1.27,
  gradient-backgrounds 1.29 to 0.74.
- **`0002-windows-color-emoji.patch` (hiwave-macos #390, landed; file deleted in the next PR).** RustKit on Windows
  painted no emoji at all. The colour path existed for macOS; Windows returned
  `None`. The rasterizer now draws the character from Segoe UI Emoji through
  Direct2D's `DrawTextLayout` with `ENABLE_COLOR_FONT`, which renders the
  COLRv1 artwork Chrome shows (DirectWrite's `TranslateColorGlyphRun` gives
  only the flat COLRv0 layers and scored worse than the blank). The Windows
  shaper also gives emoji Segoe UI Emoji's advance and variation selectors
  none, which put every emoji 8 px right of Chrome's. image-gallery 1.59 to
  0.51, about 3.98 to 3.45, card-grid 1.00 to 0.76, chrome_rustkit 1.23 to
  0.74, sticky-scroll 0.79 to 0.45.

A patch file whose first added test is already in the tree counts as applied
(or landed upstream: delete it); one that no longer applies is an error.
`git apply` runs with `--ignore-whitespace` because this repo checks out CRLF
and the patches come from an LF tree.

Board after both: **26 / 26, mean 1.27% (Mac 1.18%), every case within 0.87
points** (worst `css-selectors`), nine cases better than Mac.

**2026-09-30 (refresh #8, after #102 merged):** crates now verbatim from
hiwave-macos develop `22092e6` (Merge #384), 36 upstream PRs past refresh #7
(#348-#384). Highlights: `light-dark()` resolves to its light argument (#384,
one of the two example.com gaps noted under refresh #7), cascade layers and
`id` compound selectors, cascade phases and restyle-by-default in the engine,
pseudo-element block display and inline pseudo line boxes, five flex fixes
(collapse-through, indefinite column, empty cross, max-content clamp), the
individual transform properties, hidden UA styles, `innerText`, `cloneNode`,
fragments and form values in the Rust DOM, HTTP/2 in rustkit-http (#355, the
`h2` crate), fetch destinations on `rustkit-net::Request` (#364), and the MDN
JS ladder (`scripts/js_ladder.py`, `websuite/js-ladder/01-mdn`).

Outside `crates/rustkit-*`:

- **`scripts/apply_windows_patches.py` (new).** The Windows-local engine
  patches from #101 (Arial-first `sans-serif`, the macOS-only font aliases
  skipped, and the two strut-test tolerances) are now re-applied by a script
  instead of by hand. It is idempotent, looks for its own markers, and stops
  with an error if upstream moved the code it edits. `--check` verifies a tree
  without writing. Refresh procedure: copy crates, re-append the sidecar pin,
  run this script, run `sync_parity_tooling.py`.
- **`crates/hiwave-app/src/shield_adapter.rs`.** One test line: the test
  helper's `Request` literal gains `destination: RequestDestination::Other`,
  the new field from #364. Same shape as the `referrer_policy` line from #94.
  The Windows shell does not carry #364's EasyList interceptor yet; the Windows
  file is otherwise the pre-#364 upstream file, so that is a separate port.
- **`Cargo.lock`.** `h2` and its dependencies for HTTP/2.
- **`crates/rustkit-engine/src/lib.rs`, a TEMPORARY test patch, cross-platform,
  reported upstream.** Four test modules wrap `Engine::new` in a module mutex,
  `ENGINE_INIT`. `Engine::new` itself takes the GPU test guard, which the thread
  then holds until it exits. #380 adds `the_layer_pins_selectors_match_the_box`,
  which builds four engines on one thread: after the first it holds the guard
  and waits for the mutex, while another test holds the mutex inside
  `Engine::new` and waits 120 s for the guard, then panics and poisons the
  mutex. Parallel run: 16 of 220 fail, all with the guard's own message naming
  that test; serial run: clean. The mutex predates the guard and is redundant
  (the guard already serialises creation), so `apply_windows_patches.py`
  removes it from the four helpers. Parallel run after: 220 / 0 in 40 s. The
  script reports the patch MISSING once upstream removes the mutex, which is
  the cue to delete it.
- The parity tooling sync against the same commit brought the JS ladder
  scripts, `tools/parity_oracle/capture_url.mjs`, and no changes to the
  scripts already here.
- **`baselines/common/` is now in the sync set, and three baselines are
  recaptured.** Chasing `about` (7.58 on Windows against 3.75 on Mac) led to
  the Chrome baseline, not the engine: card 3 sat at a fractional y of 376.83
  and painted at 60% opacity, mid `fadeInUp`. `baselines/common/parity-freeze.js`
  is the init script that disables animations before a capture. Its
  `document.documentElement.appendChild(style)` ran before a `file://`
  document had a root element, threw, and took the `matchMedia` shim down with
  it, so animated fixtures were captured wherever the clock happened to be.
  macOS fixed that on 2026-07-09 (8443b8b), but `sync_parity_tooling.py`
  excluded all of `baselines/`, so Windows kept the broken script for eleven
  weeks, through yesterday's recapture in #102. The sync now copies
  `baselines/common/*` (shared tooling: the freeze script, the reset
  stylesheet, the Noto fonts), and leaves `baselines/chrome-148/` alone as
  before. A full recapture into a scratch set with the fixed script changed
  exactly three of 32 baselines, the three animated built-ins: `about`,
  `new_tab`, `settings`. The other 29 were byte-identical. Two consecutive
  captures of `about` with the fix are byte-identical too; without it they
  differ in 43% of pixels.

Gates:

| Gate | Result |
|---|---|
| workspace minus `rustkit-engine` | **1439 / 0 / 5** |
| `rustkit-engine`, `--test-threads=1` | **220 / 0** |
| `rustkit-engine`, parallel | **220 / 0** in 40 s (16 / 220 failed in 518 s before the ENGINE_INIT patch, see below) |
| total | **1659 / 0 / 5** (refresh #7: 1593 / 0 / 5) |
| both shells, release `parity-capture` | build |
| parity board (`parity_swarm --scope all`, 3 iterations) | **26 / 26, mean 1.42%** (Mac 1.18%); every case within 1.34 points of Mac, worst `gradient-no-radius`; `about` 3.98 against Mac's 3.75 |
| `apply_windows_patches.py --check` | clean; `pytest scripts/tests` 239 / 5 skipped / 2 xfailed; `audit_baselines.py` 32 clean |

**2026-09-29 (refresh #7, after #98 merged):** crates now verbatim from
hiwave-macos develop `675156a` (Merge #347), 25 upstream PRs past refresh #6
(#323-#347). Highlights: grid-template-areas and fixed tracks, blockified flex
and grid items (#330, the fix for the span-height bug found in refresh #6),
em/rem and viewport units in flex and grid sizing, the Rust-backed DOM for
scripts (new `rustkit-bindings/src/dom.rs`, innerHTML, tree moves, one
relayout per script write), cascade performance work, and rustkit-http on
rustls.

Outside `crates/rustkit-*`:

- **`Cargo.lock`.** `rustkit-http` now depends on `rustls`, `tokio-rustls` and
  `rustls-native-certs` (native-tls is an optional feature). Cargo resolved the
  Windows lock by adding those packages and bumping a few; it stays a minimal
  update, not a copy of the macOS lock. `aws-lc-sys` builds here because cmake
  and NASM are installed.
- **Nothing else.** The residual inside the crates is unchanged (the sidecar
  pin), as are the vendored `boa_gc` and the one shell test line. The parity
  tooling sync against the same commit found no changes.

Gates:

| Gate | Result |
|---|---|
| workspace minus `rustkit-engine` | **1409 / 0 / 5** |
| `rustkit-engine`, `--test-threads=1` | **184 / 0** |
| `rustkit-engine`, parallel | **184 / 0** |
| total | **1593 / 0 / 5** (refresh #6: 1502 / 0 / 5) |
| both shells, release `parity-capture` | build |
| parity board (`parity_swarm --scope all`, 3 iterations) | **23 / 26, mean 5.97%, no case moved** against the nightly of the same morning |
| dark smoke | exact |

`https://example.com` is no longer a usable regression fixture. The live page
was redesigned (a `light-dark()` background, a centred grid body, no heading),
so a pixel comparison against the 2026-09-27 capture is meaningless. Its render
shows two engine gaps: the background paints white where `light-dark(#eee,#222)`
should give `#eee`, and the anchor's underline spans the whole grid item
instead of the text. Both are parity work, recorded here so they are not lost.

**2026-09-28 (refresh #6, after #97 merged):** crates now verbatim from
hiwave-macos develop `2062352` (Merge #322). That brings #320-#322, cascade
performance work: compiled selectors, per-rule preparation, incremental
restyle. The Windows diff is `rustkit-engine/src/lib.rs` only. The residual
and the parity tooling are unchanged.

This refresh stops one merge short of develop's tip on purpose. #323
(`grid-template-areas`) adds a test that fails on Windows,
`a_min_content_row_is_its_items_real_height_not_the_estimate`: the row is
10.640625, not 10. The cause is a cross-platform engine bug that the macOS
font hides. A `<span>` flex item is not blockified, so it keeps the inline-box
height path and its rect is the font content area, not the line box. It
reproduces with Arial on any OS (8px font, 8px line-height: flex span 8.9375,
block span 8). It was reported upstream. #323 and its fix come in the next
refresh.

Gates:

| Gate | Result |
|---|---|
| workspace minus `rustkit-engine` | **1351 / 0 / 5** |
| `rustkit-engine`, `--test-threads=1` | **151 / 0** |
| `rustkit-engine`, parallel | **151 / 0** |
| total | **1502 / 0 / 5** |
| both shells, release `parity-capture` | build |
| example.com capture | pixel-identical to refresh #4 |
| dark smoke | exact |

**2026-09-28 (refresh #5, after #94-#96 merged):** crates now verbatim from
hiwave-macos develop `19a1319` (Merge #319), 12 upstream PRs past refresh #4
(#308-#319), including the whole-test GPU guard (#312). The residual is
unchanged: the one sidecar-pin file, the vendored `boa_gc`, and the one shell
test line. There were no root `Cargo.toml` or `third_party/` changes upstream.
`scripts/sync_parity_tooling.py` run against the same commit found no tooling
changes.

This refresh first landed at `f83865c` (#317). There, upstream's new
`seam_kern_tests::text_kerns_across_an_inline_seam_like_one_run` (#313)
failed on Windows. Its non-vacuity assertion assumed the macOS system font
kerns `c|x` and `z|d`. Segoe UI does not: per-node, one-run and `def`'s
position are all 94.515625, and the engine assertions passed. The test-only
fix went upstream as hiwave-macos #319, and this refresh was bumped to include
it.

Gates:

| Gate | Result |
|---|---|
| workspace minus `rustkit-engine` | **1351 / 0 / 5** |
| `rustkit-engine`, `--test-threads=1` | **146 / 0** |
| `rustkit-engine`, parallel | **146 / 0** |
| total | **1497 / 0 / 5** (refresh #4: 1481 / 0 / 5) |
| both shells, release `parity-capture` | build |
| example.com capture | pixel-identical to refresh #4 (0 of 848000) |
| dark smoke | exact |

**2026-09-27 (refresh #4, after #92 merged):** crates now verbatim from
hiwave-macos develop `04dd1d1` (Merge #307), 27 upstream PRs past refresh #3
(#281–#307). Two things outside `crates/rustkit-*` came with it:

- **Vendored `boa_gc`.** hiwave-macos #287 backports boa_gc 0.22's weak-phase
  mark fix into 0.20 via `third_party/boa_gc` and a workspace
  `[patch.crates-io]` (plus `exclude`). Without it this workspace would build
  stock boa_gc 0.20: pages that hold a cycle behind a WeakMap entry hang the
  collector (tripadvisor, squarespace, bmw, toyota), and the new
  `rustkit-js/tests/gc_weak_cycle.rs` never terminates. `third_party/boa_gc`
  is byte-identical to upstream; the two `Cargo.toml` lines match upstream.
- **One shell test line.** #296 added `referrer_policy` to
  `rustkit_net::Request`; `hiwave-app/src/shield_adapter.rs`'s test helper
  builds a `Request` by hand and gets the same `Default::default()` line
  macOS added.

The residual inside `crates/rustkit-*` is still the one sidecar-pin file.
Gates: workspace minus `rustkit-engine` **1336 / 0 / 5**; `rustkit-engine`
**145 / 0** with `--test-threads=1` **and** in parallel (19.7 s) — the first
parallel engine pass on this box, courtesy of the test-only GPU-init lock in
hiwave-macos #306; total **1481 / 0 / 5**. Both shells and release
`parity-capture` build (`CARGO_BUILD_JOBS=4`); example.com capture
pixel-identical to the 2026-09-26 capture; dark smoke exact; sidecars dated
correctly.

**2026-09-26 (refresh #3, after #91 merged):** crates now verbatim from
hiwave-macos develop `8f8b53d` (Merge #280); adds #263, #276 (float/clear
placement), #279 (grid item child width, revived) and #280 (`:root` custom
property lists). Residual against the tip: still the one sidecar-pin file.
Gates: workspace minus `rustkit-engine` **1299 / 0 / 5**; `rustkit-engine`
**131 / 0** with `--test-threads=1`; the parallel engine run stalled once
(10-min cap) and then, re-run alone twice with `--nocapture`, passed in
8.5 s and stalled again after 37 tests — a ~50 % local reproducer. When it
stalls, every unfinished test is one that constructs an `Engine` (headless
view → compositor → wgpu device), and libtest prints no 60-second warnings,
so the runner thread is blocked too: concurrent device creation on this
box, not a test. Serial has never failed. Proposed upstream mitigation: a
test-only mutex around `Engine::new` in the engine tests. Both shells and
release `parity-capture` build; example.com capture pixel-identical to the
refresh-2 capture; dark smoke exact; sidecars dated correctly.

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
