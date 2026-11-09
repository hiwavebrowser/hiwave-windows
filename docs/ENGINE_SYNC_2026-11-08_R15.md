# Engine sync, refresh #15, 2026-11-08

Source: hiwave-macos develop `27129415` (batch 40: #993, CH2.B30 UA block margins in em,
resolved against the element's font size). Four upstream commits past `8ebc1b79`
(refresh #14). Windows base: develop `3e6fb6d`. Upstream changed two files:
`crates/rustkit-engine/src/lib.rs` and the new `ua_em_margin_tests.rs`.

## What was done
Same procedure as refresh #14: reverse the Windows patches, copy the two upstream
files from the `27129415` blobs, delete the patch-created `windows_live_input_tests.rs`,
re-run `scripts/apply_windows_patches.py`. Every patch re-applied; the layout inline edits
reported "already applied" (upstream did not touch those files); no patch file dropped.
The tooling and fixtures syncs have nothing to do: upstream changed no scripts, tools,
docs, `Cargo.toml` or `Cargo.lock`.

## Receipt
After the script, the working tree differs from develop by exactly the upstream change:
`lib.rs` (+42 / -29) and the new test file. The residual against `27129415` is the same
as refresh #14's: the patch files (648, 641, 642-643, 645, 646, 644 and the file 641
creates) plus the script's inline edits to `rustkit-layout` `text.rs` and `lib.rs`.

## Gates (this machine)
- `cargo build --workspace`: exit 0
- `cargo test --workspace -- --test-threads=1`: exit 0, **4607 passed / 0 failed / 52
  ignored** (refresh #14: 4605 / 0 / 52; +2 are the new upstream tests)
- the two new tests alone: `ua_em_margin_tests` 2 passed
- WebView2 `hiwave-app`, `--no-default-features --features native-win32`, release
  `parity-capture`: all build

## Not run
No parity swarm, live-site board or native launch smoke; `collect-metrics` on the PR is the
authoritative run.

## Update: moved to develop `022aa2fa` (batch 41)

Atlas posted a newer target while this PR was open ("one open refresh at a time; each
supersedes the last"), so the same branch now carries a second commit up to hiwave-macos
develop **`022aa2fa`**: batch 41, #992 (LAT13: the renderer records a frame into one
encoder with one vertex/index arena buffer and shared layer-texture slots; a glyph-atlas
mid-frame reset discards and re-records once; idle layer slots are trimmed). Six commits
past `27129415`; seven files: engine `lib.rs`, `presenter.rs`, the new
`layer_frame_time_tests.rs`, and renderer `lib.rs`, `glyph.rs`, `frame_stats.rs`, the new
`layer_cost_tests.rs`. None of them is a file the Windows patches carry except engine
`lib.rs` (641), which was re-applied the usual way: every patch re-applied, none dropped.

Gates for the final tree:
- `cargo test --workspace -- --test-threads=1`: exit 0, **4614 passed / 0 failed / 54
  ignored** (the first commit: 4607 / 0 / 52; the new upstream tests account for the rest,
  two of them ignored)
- `cargo test -p rustkit-renderer -- --test-threads=1` alone: 157 passed, 3 ignored
  (macOS reports renderer 157 for its own run)
- WebView2 `hiwave-app`, native-win32, release `parity-capture`: all build

Not run: the renderer's `layer_cost_table` timing (it is a GPU measurement on a Mac and is
not a Windows claim), parity swarm, live-site board, launch smoke.

## Update 2: CI failure explained (patch 649), and develop `f712df45` (batches 42-43)

**The CI failure.** `collect-metrics` on the previous head (`c3e2b58`) failed after 2 h 03 m: one
`rustkit-engine` test failed and cargo stopped, so the renderer and later crates never ran. The
collector swallows cargo's output, so a probe branch (`athena/probe-r15-ci`, workflow only, run
42527292839) re-ran `cargo test --workspace -- --test-threads=1 --no-fail-fast` on the hosted
runner: **4612 passed / 2 failed / 54 ignored**, exactly:

1. `rustkit-engine` `page_script_tests::a_stalled_subresource_is_dropped_at_the_subresource_budget`
   waited 2.83 s against its 2.5 s bound.
2. `rustkit-renderer` `layer_cost_tests::fifty_opacity_layers_fit_a_frame_budget`, the 4 ms GPU
   frame-budget pin.

Both pass on a machine with a real GPU. The hosted runner's only adapter is a CPU rasteriser under
load; I did not reproduce either failure on a software adapter locally, so that is the reading of the
evidence, not a measurement. The CI re-run of this PR is the proof.

**Patch 649** (`scripts/windows-patches/649-hosted-runner-timing-tests.patch`): widens the first
bound to 3.5 s (the point of the test, not waiting out the network client's 30 s timeout, still
holds; the same value hiwave-macos row CH2.E30 uses) and skips the second on a software adapter
(`Gpu.software`, from `device_type == Cpu`, as T30 did for the scroll-cost test). Upstream row for
the second: hiwave-macos #999. Remove each hunk when upstream lands its row. Round-trips through
`apply_windows_patches.py` and is detected as applied by `--check`.

**Refresh to `f712df45`.** Atlas's newer target: batch 42 (#996, one behaviour-preserving
extraction in `rustkit-bindings` plus pins) and batch 43 (#994 braces inside CSS strings and CRLF
folding in `rustkit-cssparser`; #964 `<style media>`, `.sheet.disabled`, sheet generation in
`rustkit-dom` / `rustkit-engine` / `rustkit-bindings`). 21 upstream commits past `022aa2fa`, 13
files; every Windows patch re-applied, none dropped.

Gates for this tree (this machine): `cargo test --workspace -- --test-threads=1` exit 0,
**4635 passed / 0 failed / 54 ignored**; WebView2 `hiwave-app`, native-win32 and release
`parity-capture` build. (A first local run died with a link error and a corrupt test executable
left by an interrupted earlier run, not by this tree; I removed the six damaged executables and
re-ran the whole workspace.)

Receipt: the residual against `f712df45` is the patch files 641-649 plus the script's inline edits
to `rustkit-layout` `text.rs` / `lib.rs`, as before.
