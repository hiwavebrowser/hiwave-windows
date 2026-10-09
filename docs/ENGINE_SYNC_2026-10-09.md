# Engine sync, 2026-10-09

Source: hiwave-macos develop `f54103d9a1902e0d5160a187ee75f5222ba6f1c5`.
Windows base: develop `5fdcba4`. The macOS checkout was fast-forwarded from
`2616ec28`; its existing untracked captures were preserved.

This refresh imports all tracked RustKit crates, parity-capture, shield and
analytics code, the shield adapter, and the parity tooling and its fixtures.
It includes Boa 0.22, DOM APIs, table/flex/grid layout, selector fixes, glyph
atlas recovery, navigation state reset, live page processing, and HAR replay.
Rust is pinned to upstream's 1.91.0. The Windows lockfile was resolved locally.

## Windows adaptations

- Preserve the Windows workspace dependencies, native shell, WebView2 modes,
  16 MB stack reserve, Arial/font-alias patches, and screenshot metadata test.
- Keep one native Tokio runtime across loads and live turns. Pump all views,
  honor script navigation in the originating view, reset clocks on navigation,
  and render changed views. Use the live 15-second script budget and interruption.
- Connect native pointer events to page click/hover/scroll APIs. Return default
  link navigation to the shell and honor preventDefault; secondary clicks do
  not activate primary links. Coalesce ViewHost resize events.
- Enable the shared shield adapter in native mode as well as the hybrid build.
- Import oracle JSON/HTML/Python dependencies and design documents in tooling
  sync; exclude the macOS CI-only JS guard. Set parity progress stdout to UTF-8.

`python scripts/apply_windows_patches.py` reproduces the engine/layout residual;
the two files in `scripts/windows-patches` carry the input adapter and pending
upstream test fixes. Reverse-application checks make test-only patches idempotent.
A fresh replay from the macOS files was compared against all four resulting
Windows files and matched exactly. Keep the separate screenshot metadata test
when copying renderer sources.

## Validation

Windows x64, Rust 1.91.0, local GPU:

- Workspace: 2,533 passing tests and 6 ignored, combining the full workspace
  run's non-engine results with the corrected engine run. Engine: 437/437,
  `cargo test -p rustkit-engine --features headless --lib -- --test-threads=1`.
- Standalone default-feature engine test compilation passes. Native shell:
  41/41 tests. Standard app, native app, parity-capture, hiwave-smoke, and
  render-test build; WebView2-only fallback passes `cargo check --locked`.
- Python: 315 passed, 5 skipped, 1 expected failure, 1 non-strict unexpected pass.
- Visual board: 26/26 cases stable across three iterations, mean pixel difference 1.13%, matching the previous
  sync's reported mean. Windows Chrome baselines were retained.
- Native GPU solid colors: seven colored 100x100 blocks have exactly 10,000
  pixels each at their authored RGB values. Live smoke: a 6.5-second page timer
  changes the native content screenshot from red to green after load.

The first parallel engine run hit the existing 120-second GPU-guard timeout;
the affected selector test passed alone and the complete serial suite passed.
It also exposed the upstream fixture bug described below. No tests were disabled
to obtain the passing result. Upstream formatting whitespace and compiler
warnings are retained with the source copy.

Capture build used release optimization with LTO disabled and 16 codegen units
(equivalent to the imported parity profile for pixels; no timing claim).
Receipts are outside the repository at
`P:/repos/hiwave-renders/sync-2026-10-09` and
`P:/repos/hiwave-windows-sync-*.log`.

## Bugs reported upstream

- [#641](https://github.com/hiwavebrowser/hiwave-macos/issues/641): native pointer
  dispatcher discarded clicks and scrolling; fixed in the Windows adapter.
- [#642](https://github.com/hiwavebrowser/hiwave-macos/issues/642): default engine
  tests could not compile without headless feature unification.
- [#643](https://github.com/hiwavebrowser/hiwave-macos/issues/643): ancestor-slice
  test contradicted itself by checking the same slice as scoped and untracked.
- [#644](https://github.com/hiwavebrowser/hiwave-macos/issues/644): redirected
  cp1252 stdout crashed the parity board on its first Unicode progress mark.

The platform-independent test fixes are submitted as
[macOS PR #646](https://github.com/hiwavebrowser/hiwave-macos/pull/646).
They are also included here pending upstream integration. macOS execution is
left to that project's CI; all validation above was performed on Windows.
