# Engine sync, refresh #24, 2026-11-23

Source: hiwave-macos develop `92d783a6` (batches 119-122, batch-tested). Windows base: develop
`afd9d434` (refresh #23 merged).

Upstream in this range that is Windows-specific:
- hiwave-macos #1411: Windows runs are kerned as Chrome kerns them (GPOS pairs, no ligatures).
- hiwave-macos #1394: `BlinkMacSystemFont` is an unknown family on Windows, as in Chrome. Until
  now this was a Windows patch here; it is retired (below).

The rest of the range: #1404 (a class constructor's `toString` is its source text), #1406 (the
`Audio` named constructor), #1407 (a wrapped inline alone on its lines aligns each line), #1408
(`rem` is the root element's computed font-size), #1409 (real-site board tooling), #1413 (TLS set
up on the first https connection), #1414 (a button hangs its bottom padding and border below its
baseline), #1415 (an svg shape's exported rect follows the svg's transform), #1416 (an empty element
block keeps its box; an aspect-ratio auto width follows a definite height), #1421 (one module map
per realm, and the 11-21 stress truth).

## What was done

1. Replaced every `crates/rustkit-*` and `third_party/` whole from the `92d783a6` tree, extracted
   with fresh file times (`tar -m`), so cargo does not reuse artifacts older than the sources.
2. Re-ran `scripts/apply_windows_patches.py`: every patch applies.
3. `scripts/sync_engine_fixtures.py` (7 added, 2 refreshed) and `scripts/sync_parity_tooling.py`
   (5 files with real changes: the real-site board tooling from #1409, and the nested layout
   matrix's expected-failure list) against a `92d783a6` checkout.
4. `parity-capture`, `hiwave-app/src/ui/chrome.html` and `Cargo.toml`: no upstream change in the
   range.

## Windows patches

- **Retired: the text.rs patch** (BlinkMacSystemFont skipped on Windows, its test expectations
  and the CB7b walk change). hiwave-macos #1394 landed it upstream; at `92d783a6` text.rs is the
  upstream file unchanged, and all five edits reported "already applied". `patch_text` in
  `scripts/apply_windows_patches.py` is now an empty stub, like `patch_lib`.
- The nested-layout-matrix and flex-aspect-ratio test edits apply unchanged.
- The numbered patch files (641-663) are all already in the tree ("already applied (or landed
  upstream)").

## Gates (Windows 11, RTX 4090, cold target dir)

- **Workspace without the engine**, crate by crate (`cargo test -p <crate> -j 4`, 38 crates): 3,551
  passed, 0 failed, 38 ignored; `rustkit-renderer` and `parity-capture` with `--test-threads=1`.
- **rustkit-engine** (`--features headless`, `--test-threads=4`): 2,633 passed, 0 failed, 86
  ignored (refresh #23: 2,617 / 0 / 86).
- **Parity campaign** (`scripts/parity_test.py`, 26 cases, against `baselines/chrome-148`, captured on
  Windows with Chrome for Testing 148.0.7778.216): 26/26 pass, mean 0.823% (refresh #23: 1.052%).
  Worst: article-typography 3.83%, form-controls 2.69%, about 2.06%.
- **Chrome board** (42 rows, macOS pages at `92d783a6`, Chrome for Testing 148.0.7778.216 win64):
  "board: 38/42 rows green" (refresh #23: 22/42), mean geometry diffs 0.36 (18.79), mean pixel
  0.454% (0.537%).
  - 16 rows turn green: `chrome` address_focused at all three sizes, default and hover_tab2 at
    1024x700 and 1280x800, downloads_open and find_open at 1024x700; `report` default and weekly at
    all three sizes; `settings` default at 1280x800. With refresh #23's five, every `chrome` row is
    green.
  - No row turns red.
  - Still red: `inspector` default at all three sizes (3 geometry diffs; 0.55-1.23% pixels) and
    `settings` default at 1024x700 (2 geometry diffs; 0.59%).

## Not in this base

Nothing Windows-specific is open upstream.

## Not run

The real window and the live-site boards; `collect-metrics` on the PR is the CI run (engine lib tests
skipped on its software adapter, 658, as in refresh #23).
