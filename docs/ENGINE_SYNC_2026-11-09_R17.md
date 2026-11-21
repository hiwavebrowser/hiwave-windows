# Engine sync, refresh #17, 2026-11-09

Source: hiwave-macos develop `b80f8bfb`. The engine delta since refresh #16 (`0a13dfb4`) is
batch 48, `4b49aaf2` (#1006 LAT15: a page load serves the window's commands between its
phases). `b80f8bfb` (batch 49, #1008 T36) adds only `tools/real_window/*` (the macOS
real-window driver, its README and tests), which Windows does not carry, so the tree is the
`4b49aaf2` tree. Windows base: develop `0b0ab70` (refresh #16 and the #126-#133 stack merged).

## What was done
The refresh #14-#16 procedure: reverse the Windows patches newest-first (650, 649, 648, 646,
645, 644, 641; 642-643 is not reversible and is detected as applied), copy the four
upstream-changed files from the `4b49aaf2` blobs (`rustkit-engine`: `lib.rs`, `page_protocol.rs`,
`wgate.rs`, and the new `load_turns_tests.rs`), run `scripts/sync_parity_tooling.py` and
`scripts/sync_engine_fixtures.py` against a `4b49aaf2` checkout (no content change: git sees only
the four engine files), re-run `scripts/apply_windows_patches.py`. Every existing patch
re-applied; none dropped. Staged diff: exactly upstream's delta, 4 files, +1568 / -232 before
patch 651.

## Windows seam changes
- **New patch 651** (`651-find-script-premise-test.patch`). The first full run failed one test
  new in LAT15: `load_turns_tests::find_in_page_mid_load_then_a_hydrating_script` pins that the
  app's find script ERRORS in RustKit content, because macOS's `find_in_page.js` wraps matches
  with `Range.surroundContents`, which RustKit lacks. The Windows copy of `find_in_page.js`
  (native find, e26544d) wraps with `createTextNode` / `insertBefore` instead, so the find works
  here and answers `String("1")`. Not an engine bug: the two copies of the script differ. The
  Windows arm (`cfg!(windows)`) expects that answer; the rest of the test (answered at once, the
  page's own script and the load untouched) is unchanged and passes. Remove if the two copies of
  the script converge.

## Receipt
Tree vs the `4b49aaf2` blobs differs only by the patch set (641-651) and the script's inline
`rustkit-layout` edits, as in refresh #16; `git status` after the copy showed exactly the four
engine files.

## Gates (this machine, final tree, own target directory and log)
- `cargo test --workspace --no-fail-fast -- --test-threads=1`: exit 0, **4675 passed / 0 failed /
  56 ignored** (refresh #16: 4660 / 0 / 55). The first run stopped at the one failure above
  (cargo does not run later crates after a failing one), so this is the rerun after patch 651.
- WebView2 `hiwave-app`, native-win32 and release `parity-capture` build; native-win32 app tests
  92 passed.
- Real window, native-win32 with `HIWAVE_CONTENT_THREAD=1`: two tabs loaded, quit with WM_CLOSE:
  clean exit, code 0, each view torn down once. This exercises LAT15's load path in the shell.

## Not run
No parity swarm, live-site board or launch smoke beyond the above; `collect-metrics` on the PR is
the authoritative run. Nothing measured for LAT15's latency claim (macOS measures it).
