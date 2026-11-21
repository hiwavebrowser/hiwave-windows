# Engine sync, refresh #22, 2026-11-20

Source: hiwave-macos develop `64733ae3` (batches 112-116, batch-tested; first synced at `b60da7f5`,
batches 114-115, then retargeted to `64733ae3` when batch 116 landed). Windows base: develop `30780464`
(refresh #21 at `2f70f02f`, plus #164-#166).

## What was done

1. Replaced every `crates/rustkit-*` (29 crates) and `third_party/` whole from the `64733ae3` tree,
   extracted with fresh file times (`tar -m`), so cargo does not reuse artifacts older than the sources.
2. Re-ran `scripts/apply_windows_patches.py` and brought each patch to the new upstream (below).
3. `scripts/sync_engine_fixtures.py` (6 added) and `scripts/sync_parity_tooling.py` against a
   `64733ae3` checkout.
4. `parity-capture` (Windows keeps its own copy): upstream's `2f70f02f..64733ae3` delta (the HAR dump
   and the post-action settle) applied cleanly. `hiwave-app/src/ui/chrome.html`: no upstream change in
   the range. `Cargo.toml`: none. `Cargo.lock` re-resolved: upstream's two added dependencies
   (`objc2-core-video`, macOS only, and `serde_json`).

## Windows patches

- **sans_serif leads with Arial (text.rs):** retired. Upstream #1351 gives Windows its own
  `sans_serif()` leading with Arial.
- **The two strut-descent test tolerances (lib.rs):** retired. Upstream tests within a pixel on
  Windows (`let tolerance = if cfg!(windows) { 1.0 } else { 0.5 }`), which is what these edits did.
- **BlinkMacSystemFont is unknown on Windows (text.rs) and its test edits:** unchanged. Upstreamed as
  hiwave-macos #1394 (draft); retire it when that lands.
- **663 (new):** all 8 rustkit-engine `media_host_tests` (MEDIA-AVF step 3) drive a scripted
  `FakeHost` but read the metadata through AVFoundation (macOS only), so here no metadata is read (a
  video's `videoWidth` stays 0, the fixture is 64). Ignored on Windows with that reason, as 661
  ignores the media fetch and load tests (the Windows media backend row).
- 641, 645, 646, 648, 649, 650, 654, 656, 658, 661, 662 apply unchanged; 642-643, 644 and 660 are
  already upstream. The nested-layout and flex expectation edits apply unchanged.

## Gates (Windows 11, RTX 4090)

- **Workspace without the engine**, crate by crate (`cargo test -p <crate> -j 4`, 38 crates): 3,429
  passed, 0 failed, 38 ignored; `rustkit-renderer` and `parity-capture` with `--test-threads=1`.
  - `parity-capture`: with all 97 tests in parallel, the run stalled for an hour with 20 settle,
    live-loop and inserted-script tests unfinished. Each of those 20 passes alone in 1-8 s, and the
    crate passes 97/97 serially (70 s). This is contention between parallel tests on Windows, not a
    failing test; a follow-up issue tracks whether macOS shows it too.
- **rustkit-engine** (`--features headless`, `--test-threads=4`), run before patch 663: 2,614 passed,
  9 failed, 78 ignored. 8 of the 9 are the `media_host_tests` that 663 now ignores (a targeted rerun
  after 663: 8 ignored). The ninth:
  - `replaced_float_avoid_tests::boxes_that_avoid_floats_are_placed_as_chrome_places_them` ("inline
    img with clear beside a float": the image at 130.23, Chrome 128). A real Windows bug, not this
    sync's: the initial `font-family` is `sans-serif` (Arial) off macOS, where Chrome for Testing 148
    win64 lays an unstyled page out in Times New Roman. Fixed upstream by hiwave-macos #1397 (Talos,
    with the Windows half from here); it passes with #1397 applied. Left failing here until that lands,
    rather than carried as a patch.
- **Parity campaign** (`scripts/parity_test.py`, 26 cases, against `baselines/chrome-148`, captured on
  Windows with Chrome for Testing 148.0.7778.216): 26/26 pass, mean 1.052% (refresh #21: 1.073%).
  Worst: article-typography 3.83%, about 3.44%, form-controls 2.93% (as in #20 and #21).
- **Chrome board** (42 rows, macOS pages at `64733ae3`): 17/42 green, mean geometry 65.33, pixel
  0.636%. hiwave-macos #1392, #1393 (Windows fallback faces, 24/42 on the `2f70f02f` board) and
  #1391 are not in this base yet.

## Not run

The real window and the live-site boards; `collect-metrics` on the PR is the CI run (engine lib tests
skipped on its software adapter, 658, as in refresh #21).
