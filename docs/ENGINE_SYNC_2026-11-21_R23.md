# Engine sync, refresh #23, 2026-11-21

Source: hiwave-macos develop `b94cd112` (batches 117-118, batch-tested; first synced at `bdfda8b2`,
batch 117, then retargeted to `b94cd112` when batch 118 landed). Windows base: master/develop
`fe8f8bf5` (refresh #22, released as V1.6.0).

Upstream in this range that is Windows-specific:
- hiwave-macos #1392: the Windows shaper unites each fallback face's line metrics, as Chrome does.
- hiwave-macos #1393: the Windows general text fallback (Chrome's per-Unicode-block family first, then
  DirectWrite's `MapCharacters`; whole-pixel advances at a face's bitmap-strike sizes).
- hiwave-macos #1397: the initial `font-family` is Times New Roman off macOS, as in Chrome.

The rest of the range: #1396 (a 16 MiB per-script cap), #1399 (a smaller boa statement size), #1400
(font-family serialization) and a boa parser fix.

## What was done

1. Replaced every `crates/rustkit-*` and `third_party/` whole from the `b94cd112` tree, extracted
   with fresh file times (`tar -m`), so cargo does not reuse artifacts older than the sources.
2. Re-ran `scripts/apply_windows_patches.py`: every patch applies unchanged (642-643, 644 and 660
   are already upstream).
3. `scripts/sync_engine_fixtures.py` (1 added) and `scripts/sync_parity_tooling.py` (nothing
   changed) against a `b94cd112` checkout.
4. `parity-capture` (Windows keeps its own copy): upstream's per-script cap test change from #1396
   applied. `hiwave-app/src/ui/chrome.html`, `Cargo.toml`: no upstream change in the range.

## Windows patches

- **BlinkMacSystemFont is unknown on Windows (text.rs) and its test edits:** unchanged. Upstreamed as
  hiwave-macos #1394 (draft); retire it when that lands.
- All numbered patches (641-663) apply unchanged.

## Gates (Windows 11, RTX 4090, cold target dir)

- **Workspace without the engine**, crate by crate (`cargo test -p <crate> -j 4`, 38 crates): 3,537
  passed, 0 failed, 38 ignored; `rustkit-renderer` and `parity-capture` with `--test-threads=1`
  (`parity-capture` stalls when all its tests run in parallel on Windows; see refresh #22).
- **rustkit-engine** (`--features headless`, `--test-threads=4`): 2,617 passed, 0 failed, 86
  ignored. Refresh #22's one failure,
  `replaced_float_avoid_tests::boxes_that_avoid_floats_are_placed_as_chrome_places_them`, passes: it
  was the initial font, fixed by #1397.
- **Parity campaign** (`scripts/parity_test.py`, 26 cases, against `baselines/chrome-148`, captured on
  Windows with Chrome for Testing 148.0.7778.216): 26/26 pass, mean 1.052% (refresh #22: 1.052%).
  Worst: article-typography 3.83%, about 3.44%, form-controls 2.93% (as in #20 to #22).
- **Chrome board** (42 rows, macOS pages at `b94cd112`, Chrome for Testing 148.0.7778.216 win64):
  22/42 green (refresh #22: 17/42), mean geometry diffs 18.79 (65.33), mean pixel 0.537% (0.636%).
  - Five chrome rows turn green (`chrome` default 1600x1000, downloads_open 1280x800, find_open
    1280x800 and 1600x1000, hover_tab2 1600x1000), each from 92-132 geometry diffs to 22.
  - No row turns red. The `report` rows keep refresh #22's pixel diffs (default 1600x1000 0.678%,
    weekly 1600x1000 0.900%) with fewer geometry diffs (12-13 to 4-5). On the older `2f70f02f`
    pages those two rows were green at about 0.41%; the report page changed upstream since, and
    refresh #22's board already shows the higher figure.

## Not in this base

hiwave-macos #1391 (an emoji-property character honours its variation selector) and #1394
(BlinkMacSystemFont, carried here as a patch) are open upstream.

## Not run

The real window and the live-site boards; `collect-metrics` on the PR is the CI run (engine lib tests
skipped on its software adapter, 658, as in refresh #22).
