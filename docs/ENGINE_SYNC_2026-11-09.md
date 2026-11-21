# Engine sync, refresh #16, 2026-11-09

Source: hiwave-macos develop `0a13dfb4` (batch 47: #1002 H19, #1003 E35, #1004 M1, #1005 M2 on
top of batches 44-46). 55 commits past `f712df45` (refresh #15) to `35034d6f`, then 19 files
past that to `0a13dfb4`. Windows base: develop `eac3613` (refresh #15 and the timeout change
merged). The target moved twice while this was being built (35034d6f, then 0a13dfb4); the
numbers below are for the final tree only.

## What was done
The refresh #14/#15 procedure: reverse the Windows patches newest-first (650, 649, 648, 646,
645, 644, 641; 642-643 is not reversible and is detected as applied), copy the upstream-changed
files under `crates/` from the `0a13dfb4` blobs, delete the patch-created
`windows_live_input_tests.rs`, run `scripts/sync_parity_tooling.py` and
`scripts/sync_engine_fixtures.py` against a `0a13dfb4` checkout (nothing missing), re-run
`scripts/apply_windows_patches.py`. Every existing patch re-applied; none dropped.
`Cargo.lock`: upstream's one-line change (`boa_gc` added to `rustkit-engine`'s dev
dependencies) applied to the Windows lock.

## Windows seam changes
- **New patch 650** (`650-layout-font-premise-tests.patch`). Two `rustkit-layout` tests added
  upstream pin macOS font behaviour and failed here (907 passed, 2 failed, found by this
  refresh; cargo then stops, so later crates do not run):
  - `test_an_emoji_label_lifts_a_styled_buttons_line`: skipped where the platform does not unite
    an emoji's fallback face into the line, with the same guard the existing n40 test uses. The
    skip fires on Windows, so this is a gap in Windows text measurement, not merely a different
    number. I did not measure Chrome on Windows.
  - `test_negative_half_leading_inline_keeps_the_line_baseline`: ignored on Windows with the
    reason. It pins Chrome-on-macOS metrics of the macOS system font (a 15 px content area at
    12 px, strut seat 28); Arial gives 14 and 27. No Chrome-on-Windows numbers exist for it, and
    rewriting the expectation from our own output would be circular.
  Upstream row: hiwave-macos #1007.
- **`parity-capture`** is an older, Windows-adapted fork (`run_capture`, not `capture`). Upstream
  added three LAT14 options (`--animations`, `--prelude-url`, `--prelude-live-ms`); they were
  ported by hand (Args fields, the prelude load and `run_live` before the `--url` load, and
  `disable_animations = false` under `--animations`), and upstream's two new tests for them pass
  here (`parity-capture`: 16 passed). The port was done by a second Claude session working in
  parallel; I reviewed the diff, and the tests are the check.
- **Not synced, on purpose:** `crates/hiwave-app/src/main.rs`, `webview_rustkit.rs` and the new
  `close_tab_tests.rs`. The delta is macOS-only wiring (LAT1c input-to-present:
  `note_input(&event)`, `NoteInput { kind }`, `absorb_undrawn`, and Cmd+W tests); Windows' copies
  do not contain that machinery at all, so every hunk rejects. Also not carried:
  `tools/real_window/*` and `tools/latency/tab_cycle_views.py` (macOS real-window lanes).
- **`ContentPage::close` (its own commit, a shell change).** #1003 (CH2.E35) adds
  `impl Drop for WindowSurface`, with a Windows arm: it releases the GPU surface and then calls
  `Presenter::destroy_view` -> `ViewHost::destroy_view`, whose Windows arm unregisters the view
  and calls `DestroyWindow`. `ContentPage::close` used to `drop(self)` and then call
  `DestroyWindow` itself (because `WindowSurface` had no such Drop), which would now destroy the
  same window twice. It now only drops. Checked in a real window with
  `HIWAVE_CONTENT_THREAD=1`: two tabs, a real Ctrl+W on the second, child windows **4 -> 3**,
  and the log shows the teardown once, in order (compositor surface destroyed, engine view
  destroyed, viewhost view destroyed), with no error. #1003 says its Windows arm was never run
  on Windows; this is that run, for the content-thread path only.

## Receipt
Working tree vs the `0a13dfb4` blobs (line endings normalised), `crates/rustkit-*`: the files
that differ are the patch-touched set (`rustkit-core/input.rs` 648; `rustkit-engine/lib.rs` 641
and 649, `inspector_page_tests.rs`, `text_control_select_tests.rs`, `transform_rect_tests.rs`;
`rustkit-layout/lib.rs` and `text.rs` from the script's inline edits plus 650;
`rustkit-renderer/screenshot.rs` 646 and `layer_cost_tests.rs` 649; `rustkit-viewhost/lib.rs`
644) plus `windows_live_input_tests.rs`, which 641 creates. Nothing missing, nothing stray.

## Gates (this machine, final tree)
- `cargo test --workspace -- --test-threads=1`: exit 0, **4660 passed / 0 failed / 55 ignored**
  (refresh #15: 4635 / 0 / 54). Own target directory and log.
- WebView2 `hiwave-app`, native-win32 and release `parity-capture` build; native app tests 80
  passed.
- **A GPU host ran the frame-budget test that patch 649 skips on a software adapter**
  (`layer_cost_tests::fifty_opacity_layers_fit_a_frame_budget`): it ran, not skipped, and
  passed: 50 opacity layers 0.485 ms/frame, none 0.189 ms/frame (budget: less than 4 ms more).
  Hosted CI never runs that test, by design.

## Not run
No parity swarm, live-site board or launch smoke; `collect-metrics` on the PR is the
authoritative run (its limit is now 210 minutes). The close check above is the content-thread
path only.

## Process note
Two Claude sessions worked on this refresh at once (this branch and an unpushed
`athena/engine-sync-refresh-16b`), and both wrote to the same scratch log names, which corrupted
one test run's counts until it was noticed (the log showed crates compiled from the other
worktree). The numbers above are from a run in its own target directory with its own log.
