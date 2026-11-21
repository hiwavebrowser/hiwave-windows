# Engine sync, refresh #18, 2026-11-09

Source: hiwave-macos develop `6ac1b8b3` (batch 50: #1009 M1b close-tab test fix, #1010 M1c tab
order, #1011 CH2.E50 querySelector/matches/closest honour ancestor attribute filters). Windows
base: develop `3a1fd86` (refresh #17, #140 and #141 merged).

## What was done
The refresh #14-#17 procedure: reverse the Windows patches (650, 649, 648, 646, 645, 644, 641;
642-643 is not reversible and is detected as applied; 651 touches `load_turns_tests.rs`, which
did not change upstream, so it stays and is detected as applied), copy the upstream-changed
engine files from the `6ac1b8b3` blobs (`rustkit-engine`: `lib.rs`, and the new
`ancestor_attribute_query_tests.rs`), run `scripts/sync_parity_tooling.py` and
`scripts/sync_engine_fixtures.py` against a `6ac1b8b3` checkout (no content change), re-run
`scripts/apply_windows_patches.py`. Every patch re-applied; none dropped. Staged diff: two files,
+184 / -3, all upstream's.

## Not synced, on purpose
- `crates/hiwave-shell/src/lib.rs` (#1010, explicit tab order in `BrowserShell`): Windows'
  `hiwave-shell` is an older, reformatted fork of the crate and upstream's diff does not apply to
  it (`git apply` fails at the first hunk). The native shell already orders what it shows
  itself (workspaces sorted by id in `chrome_data`, tabs in workspace order), so nothing on
  Windows needs it today. A manual port of #1010 is a separate, reviewable change; offered as a
  follow-up.
- `crates/hiwave-app/src/close_tab_tests.rs` and `close_tab_tests/tab_order_tests.rs`: macOS-only
  (Cmd+W / tab-strip tests against the macOS shell); the Windows copies do not contain that
  machinery. Same as refresh #16.

## Receipt
Working tree vs the `6ac1b8b3` blobs differs only by the patch set (641-651) and the script's
inline `rustkit-layout` edits, as in refresh #17; `git status` after the copy showed exactly the
two engine files.

## Gates (this machine, final tree, own target directory and log)
- `cargo test --workspace --no-fail-fast -- --test-threads=1`: exit 0, **4678 passed / 0 failed /
  57 ignored** (refresh #17: 4675 / 0 / 56).
- WebView2 `hiwave-app`, native-win32 and release `parity-capture` build; native-win32 app tests
  109 passed.
- Real window, native-win32 with `HIWAVE_CONTENT_THREAD=1`: two tabs loaded, quit with WM_CLOSE:
  clean exit, code 0, each of the 4 views torn down once.

## Not run
No parity swarm or live-site board; `collect-metrics` on the PR is the authoritative run.
