# Engine sync, refresh #14, 2026-11-08

Source: hiwave-macos develop `8ebc1b79` (batch 39), 167 commits past `95691f85`
(refresh #13). Windows base: develop `8edabfd`. 101 files changed upstream, no
deletes or renames.

## What was done

1. Reversed the Windows patches (`scripts/windows-patches`, newest first), copied
   the 69 upstream-changed files under `crates/` verbatim from the `8ebc1b79`
   blobs, deleted the patch-created `windows_live_input_tests.rs`, and re-ran
   `scripts/apply_windows_patches.py` (the refresh #13 lessons: a kept
   patch-created file makes the marker check skip the hunk silently).
2. `scripts/sync_parity_tooling.py` and `scripts/sync_engine_fixtures.py` against a
   `8ebc1b79` checkout. The fixtures script found nothing missing.

## Windows seam changes

- **Patch 647 dropped.** Upstream fixed the CPU-adapter case itself (T30:
  `3df658d6`, `d3c88afc`, `3c5f3c4e`: a software adapter draws a small
  buffer-limit frame). The patch no longer applied and is no longer needed.
  hiwave-macos #965 can be closed.
- Patches 641, 644, 645, 646, 648 and the inline edits re-applied cleanly;
  642-643 is detected as already applied.

## Deliberately not synced

- `crates/hiwave-app/src/main.rs`, `chrome_state.rs`, `inspector_page_tests.rs`:
  CH3.A10 (chrome state coalesced per turn) is gated to
  `cfg(all(target_os = "macos", feature = "rustkit"))` and rewires the WebView2
  shell's `main.rs`, which Windows keeps its own version of. The native Win32
  shell already pushes differential state. `chrome.html`'s side of it
  (`__hiwaveApplyState`, in-place tab update) is taken, so a later main.rs sync
  needs no HTML change.
- `crates/hiwave-app/src/ui/inspector.html` (the WebKit inspector; Windows only
  carries `inspector_rustkit.html`).

## parity-capture

Windows' `parity-capture` is an older, Windows-adapted file (`run_capture`, no
shield, no `capture(args, sink, inspector)`). The upstream delta in this range
(#966: inline a page's local image files as data URLs in html-file captures, 262
lines) applies cleanly to it. `Cargo.toml` gains `base64`. The two new upstream
tests call `capture(&args, None, &mut None)`; here they call `run_capture(&args)`,
its Windows equivalent. A fuller parity-capture sync (shield, inspector) is a
separate piece of work.

## chrome.html

Windows' `chrome.html` is an older fork (no `data-rustkit-alpha` route). Upstream's
hunks applied except `renderTabs`, which was merged by hand: the in-place update
path is added; the address-bar sync block it sits after does not exist here (the
native shell sets the address itself).

## Receipt

Working tree vs `8ebc1b79` blobs (line endings normalised), `crates/rustkit-*`:
9 files differ, exactly the patch-touched set (`rustkit-core/input.rs`,
`rustkit-engine/{inspector_page_tests,lib,text_control_select_tests,transform_rect_tests}.rs`,
`rustkit-layout/{lib,text}.rs`, `rustkit-renderer/screenshot.rs`,
`rustkit-viewhost/lib.rs`), plus `windows_live_input_tests.rs`, which patch 641
creates. Nothing missing, nothing stray.
