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
