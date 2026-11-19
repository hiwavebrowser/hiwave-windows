# Engine sync, refresh #21, 2026-11-19

Source: hiwave-macos develop `47b3097f` (batch 110, batch-tested). Windows base: develop `f5a5039b`
(refresh #20 at `295ce9f4`, plus #159-#162). 170 upstream commits; under `crates/` and `third_party/`
66 files, +7.1k / -0.8k lines, mostly rustkit-engine and rustkit-html.

## What was done

1. Replaced every `crates/rustkit-*` (29 crates; the manifests now equal upstream's) and `third_party/`
   whole from the `47b3097f` tree, with fresh file times (an extracted file keeps its commit's time,
   older than an existing build, and cargo would reuse stale artifacts).
2. Re-ran `scripts/apply_windows_patches.py` and brought each patch to the new upstream (below).
3. `scripts/sync_engine_fixtures.py` (6 files) and `scripts/sync_parity_tooling.py` (287 files, all
   but one unchanged apart from line endings) against a `47b3097f` checkout.
4. `parity-capture` and `hiwave-app/src/ui/chrome.html` (Windows keeps its own copies): upstream's
   `295ce9f4..47b3097f` delta applied per file, cleanly. `Cargo.toml`: upstream's one change
   (tiny-skia `overflow-checks = false` in dev) was already here from refresh #20. `Cargo.lock` re-resolved.

## Windows patches

- **652:** dropped. Upstream #1319 takes a value with a backslash as the console file's path on every OS.
- **653:** dropped. Upstream #1321 polls the shared test device.
- **655:** down to one hunk. Upstream #1343 (Windows web fonts) is in this base, so the six web-font
  ignores are gone and those tests run. The dom1 1 px ignore stays until the sync brings #1345
  (Windows FontSubstitutes, batch 111). The hunk is now against upstream's file.
- **657:** dropped. Upstream #1339 (with #1304) fixes the retained-style tiles flake.
- **659:** stays until the sync brings #1347 (batch 111), which takes the same one-line fix upstream.
- **Inline edits** (`text.rs`: `sans-serif` leads with Arial, `BlinkMacSystemFont` unknown, the strut
  tolerances; the nested-layout and flex expectation edits): unchanged; upstream #1351 takes the generic
  families as these edits have them.
- 641, 645, 646, 648, 649, 650, 654, 656, 658, 660 apply unchanged; 642-643 and 644 are already
  upstream.

## Gates (Windows 11, RTX 4090)

- **Workspace without the engine**, crate by crate (`cargo test -p <crate> -j 4`, 40 crates): 3,497
  passed, 0 failed, 38 ignored.
- **rustkit-engine** (`--features headless`, `--test-threads=8`): 2,568 passed, 0 failed, 65 ignored.
  The six web-font tests run and pass (upstream #1343).
- **Parity campaign** (`scripts/parity_test.py`, 26 cases, against `baselines/chrome-148`, captured on
  Windows with Chrome for Testing 148.0.7778.216): 26/26 pass, mean 1.072% (refresh #20: 1.07%).
  Worst: article-typography 3.83%, about 3.44%, form-controls 2.91% (as in #20).

## Not run

The real window, the chrome board and the live-site boards; `collect-metrics` on the PR is the CI run
(engine lib tests skipped on its software adapter, 658, as in refresh #20).
