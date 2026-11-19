# Engine sync, refresh #21, 2026-11-19

Source: hiwave-macos develop `2f70f02f` (batch 111, batch-tested; first synced at `47b3097f`, batch 110,
then batch 111 folded in before review). Windows base: develop `f5a5039b`
(refresh #20 at `295ce9f4`, plus #159-#162). 192 upstream commits; under `crates/` and `third_party/`
66 files, +7.1k / -0.8k lines, mostly rustkit-engine and rustkit-html.

## What was done

1. Replaced every `crates/rustkit-*` (29 crates; the manifests now equal upstream's) and `third_party/`
   whole from the `47b3097f` tree, and again from `2f70f02f` for batch 111, with fresh file times (an extracted file keeps its commit's time,
   older than an existing build, and cargo would reuse stale artifacts).
2. Re-ran `scripts/apply_windows_patches.py` and brought each patch to the new upstream (below).
3. `scripts/sync_engine_fixtures.py` and `scripts/sync_parity_tooling.py` against `47b3097f`, then
   `2f70f02f` checkouts. The fixture script only copies a MISSING file, so a fixture upstream changed stays
   stale here: `docs/diagnostics/2026-11-18/media_box_cft148.json` (two cases added in #1332 R1) was
   copied by hand, and `media_box_tests` pass with it. Two other included files are older Windows copies
   whose tests pass (`fixtures/pseudo-elements.html`, `tools/parity_oracle/nested_layout_matrix_expected_failures.txt`);
   left as they are (follow-up: the script should refresh a changed fixture, not only a missing one).
4. `parity-capture` and `hiwave-app/src/ui/chrome.html` (Windows keeps its own copies): upstream's
   `295ce9f4..47b3097f` delta applied per file, cleanly. `Cargo.toml`: upstream's one change
   (tiny-skia `overflow-checks = false` in dev) was already here from refresh #20. `Cargo.lock` re-resolved.

## Windows patches

- **652:** dropped. Upstream #1319 takes a value with a backslash as the console file's path on every OS.
- **653:** dropped. Upstream #1321 polls the shared test device.
- **655:** dropped. Upstream #1343 (Windows web fonts) and #1345 (FontSubstitutes) are in this base, so
  the six web-font tests and the dom1 1 px test run and pass.
- **657:** dropped. Upstream #1339 (with #1304) fixes the retained-style tiles flake.
- **659:** dropped. Upstream #1347 takes the same one-line fix.
- **661 (new):** 14 media tests (`media_fetch_tests`, `media_load_tests`) need a media backend; MEDIA-AVF
  plays through AVFoundation (macOS only), so here `canPlayType` answers "" and no metadata fetch starts.
  Ignored on Windows with that reason (the Windows media backend row); `media_box_tests` run and pass.
- **Inline edits** (`text.rs`: `sans-serif` leads with Arial, `BlinkMacSystemFont` unknown, the strut
  tolerances; the nested-layout and flex expectation edits): unchanged; upstream #1351 takes the generic
  families as these edits have them.
- 641, 645, 646, 648, 649, 650, 654, 656, 658, 660 apply unchanged; 642-643 and 644 are already
  upstream.

## Gates (Windows 11, RTX 4090)

- **Workspace without the engine**, crate by crate (`cargo test -p <crate> -j 4`, 40 crates): 3,497
  passed, 0 failed, 38 ignored.
- **rustkit-engine** (`--features headless`): at `47b3097f` 2,568 passed, 0 failed, 65 ignored
  (`--test-threads=8`); with batch 111 folded in, 2,575 passed, 0 failed, 78 ignored (`--test-threads=4`:
  at 8 threads one networked test ran the machine out of socket buffers, os error 10055, and passed
  alone). The six web-font tests and the dom1 test run and pass (upstream #1343, #1345); the 14 media
  tests of 661 are the new ignores.
- **Parity campaign** (`scripts/parity_test.py`, 26 cases, against `baselines/chrome-148`, captured on
  Windows with Chrome for Testing 148.0.7778.216): 26/26 pass, mean 1.073% with batch 111 (1.072% at `47b3097f`; refresh #20: 1.07%).
  Worst: article-typography 3.83%, about 3.44%, form-controls 2.91% (as in #20).

## Not run

The real window, the chrome board and the live-site boards; `collect-metrics` on the PR is the CI run
(engine lib tests skipped on its software adapter, 658, as in refresh #20).
