# Engine sync, refresh #19, 2026-11-09

Source: hiwave-macos develop `e6a4ac5b` (batch 51: #1013 LAT17, page scripts run as they arrive,
in document order; with its R1 nit fixes). Engine delta since refresh #18 (`6ac1b8b3`): seven
`rustkit-engine` files, +1028 / -135. Base: the refresh #18 commit `31c11dd` (merged into
develop as #143); develop has moved since (#144, others), none of it overlapping these files.

## What was done
The refresh #14-#18 procedure: reverse the Windows patches newest-first (651, 650, 649, 648, 646,
645, 644, 641; 642-643 is not reversible and is detected as applied), copy the seven
upstream-changed files from the `e6a4ac5b` blobs (`lib.rs`, `load_turns_tests.rs`,
`page_protocol.rs`, `script_lifecycle_tests.rs`, `script_net.rs`, `script_net_t7_tests.rs`, and the
new `script_stream_tests.rs`), run `scripts/sync_parity_tooling.py` and
`scripts/sync_engine_fixtures.py` against an `e6a4ac5b` checkout (no content change), re-run
`scripts/apply_windows_patches.py`. Every patch re-applied; none dropped. Patch 651 had to be
reversed first this time because upstream changed `load_turns_tests.rs`, the file it edits; it
re-applied cleanly. Staged diff: exactly upstream's delta (7 files, +1028 / -135).

## Windows seam changes
None. No new patch.

## Receipt
Working tree vs the `e6a4ac5b` blobs differs only by the patch set (641-651) and the script's
inline `rustkit-layout` edits, as in refresh #18; `git status` after the copy showed exactly the
seven engine files.

## Gates (this machine, final tree, own target directory and log)
- `cargo test --workspace --no-fail-fast -- --test-threads=1`: exit 0, **4696 passed / 0 failed /
  57 ignored** (refresh #18: 4678 / 0 / 57; the extra 18 are LAT17's `script_stream_tests` and
  the new pins).
- WebView2 `hiwave-app`, native-win32 and release `parity-capture` build; native-win32 app tests
  109 passed (this branch is on the refresh #18 base, so it does not include the later shell PRs).
- Real window, native-win32 with `HIWAVE_CONTENT_THREAD=1`: two tabs loaded, quit with WM_CLOSE:
  clean exit, code 0, each of the 4 views torn down once. LAT17 changes how a content-thread load
  runs scripts and leaves a page, so this is the shell path it touches.

## Not run
No parity swarm or live-site board; `collect-metrics` on the PR is the authoritative run. LAT17's
latency claim (the i0a_nav chain) is macOS's to measure; nothing of it is claimed here.
