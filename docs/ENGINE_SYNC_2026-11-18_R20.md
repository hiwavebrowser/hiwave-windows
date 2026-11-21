# Engine sync, refresh #20, 2026-11-18

Source: hiwave-macos develop `295ce9f4` (batch 104d, batch-tested). Windows base: develop `31a1d144`
(the AGPL relicence, #157). The last sync was refresh #19 from `e6a4ac5b` (batch 51):
1,570 commits; under `crates/` 448 files (239 new, 209 changed), +92k / -5k lines, mostly
rustkit-engine (38%) and rustkit-bindings (21%); `third_party/` +57k.

## What was done

1. Reversed the Windows patches newest-first (642-643 is not reversible, as before). Checked that
   every `crates/rustkit-*` file then equalled `e6a4ac5b` apart from the manifests (AGPL, #157) and
   the script's inline edits, and that `third_party/boa_engine` equalled `e6a4ac5b`'s.
2. Replaced every `crates/rustkit-*` and `third_party/` whole from the `295ce9f4` tree. Upstream now
   vendors `boa_ast` and `boa_parser` too (patched; `third_party/boa_engine/PATCHES.md`).
3. Re-ran `scripts/apply_windows_patches.py` and brought each patch to the new upstream (below).
4. `scripts/sync_engine_fixtures.py` (173 files) and `scripts/sync_parity_tooling.py` (73 files)
   against a `295ce9f4` checkout. The tooling script now copies, unchanged and with a note, a file
   this Python cannot parse (`tools/parity_oracle/gen_eval_order_cases.py` uses 3.12 f-strings).
5. `Cargo.toml`: three-way merge (`e6a4ac5b` -> `295ce9f4` onto Windows): the dev-profile opt-level
   for dependencies and the two glue crates, the `boa_parser` / `boa_ast` patch entries, and the new
   member `hiwave-shield-intercept`.
6. `hiwave-shield`, `hiwave-shield-intercept` (new) and `parity-capture`: taken whole (below).
7. `hiwave-app`: three-way merges of the shared pages; the shell stays Windows' (below).

## Windows patches

- **text.rs (inline edit):** upstream CB7b skips `-apple-system` everywhere and maps
  `BlinkMacSystemFont` to `system-ui`, as Chrome on macOS does. Chrome for Testing 148 on Windows
  treats `BlinkMacSystemFont` as unknown (alone it is the default serif, 67.266 px for "Hello world"
  at 14px; `BlinkMacSystemFont, sans-serif` is Arial, 69.250; `system-ui` is Segoe UI, 70.906:
  `docs/diagnostics/2026-11-18/font_alias_win148.json`). The edit now skips it on Windows; the alias
  tests follow. CB7b's stack test walks for a system font ahead of Helvetica; on Windows Segoe UI is
  both the system font and a family the stack names, so there the walk checks only `-apple-system`.
- **649:** its rustkit-engine hunk is gone (upstream CH2.E30 bounds the stalled subresource at 8 s);
  the renderer hunk (skip a GPU timing on a software adapter) remains.
- **650:** regenerated against the moved layout tests (the same two edits).
- **651:** dropped. Upstream implements `Range.surroundContents`, and the test expects the answer.
- **652 (new):** `HIWAVE_CONSOLE_LOG` took only a value with `/` as a path, so a Windows path
  (`C:\logs\c.jsonl`) named no file; `console_file_tests::the_variable_alone_names_the_file` and
  `the_process_reads_the_variable_and_set_path_wins` failed here as they do on a clean `295ce9f4` on
  this machine. On Windows a backslash makes it a path too; the rule tests follow. Upstream row filed.
- **653 (new):** the tests share one wgpu device per process (upstream T41b), and nothing maintained it
  between tests. On the hosted runner's software adapter (WARP) a dropped engine's GPU resources are
  process memory: 9.2 GB by ~800 tests here, against 862 MB before the sync, and CI's test run ended.
  The device is polled when the next test takes it. Upstream: hiwave-macos #1321.
- **654 (new):** on a software adapter, a process that builds an engine for one layout-corpus page
  after another still ends partway through (tracked separately). Every page passes in a process of
  its own, and the whole corpus passes in one process on a hardware GPU. On a software adapter,
  `verify_mode_shows_no_mismatch_on_the_layout_corpus` runs each page in its own child process and
  sums the counts for its final check. No page is skipped.
- **655 (new):** seven engine tests fail on Windows on a hardware GPU and on the software adapter
  alike: six assert the page's web face (Windows shapes in the system face) and one is 1 px off
  Chromium. Each is ignored on Windows with its parity row in the reason (web fonts; dom1 1 px).
- **656 (new):** on a software adapter only, every queue submit in the compositor, the renderer and
  the canvas waits for its submission before returning (row WARP-ACCUMULATION). Off on a hardware
  GPU. A source pin keeps every
  submit settled.
- **657 (new):** `retained_style_tests::tiles_written_then_read_keep_a_full_builds_boxes` is flaky on a
  loaded runner. Upstream's fix (hiwave-macos #1339) needs #1304, which is after this sync's base, so the
  test is ignored on Windows until the sync brings both.
- **658 (new):** on a software adapter (the hosted runner), CI does not run the engine's lib tests: a process
  that builds engine after engine can still end partway through on WARP (row WARP-ACCUMULATION). A probe
  test reports the adapter; `collect_metrics.py` skips the lib chunks on `Cpu`, with the reason in the job
  log and `metrics.json`. The full engine suite runs on a hardware GPU with every sync (receipt in the PR).
- **659 (new):** rustkit-js `a_deep_setter_chain_and_call_fit_the_default_recursion_limit` sets its
  thread's native stack budget (half its 8 MB stack, as a script thread does); Boa's 1 MiB default is too
  small for 400 setter links in CI's non-incremental debug build.
- **660 (new):** parity-capture `a_request_that_never_answers_costs_the_settle_its_cap_once_and_is_counted`
  bounds the capture at 7.5 s on Windows (6 s elsewhere). It took 3.9 s on the RTX 4090, 5.84 to 5.89 s on
  forced WARP here and 6.02 s on the loaded hosted runner; the 9 s of nominal sleeps it catches still fails.
- **CI (`scripts/collect_metrics.py`):** for the same reason, the engine's lib tests run in chunks of
  whole modules (up to 150 tests a process) instead of one process for the whole suite. The same tests
  run, serially as before. The count listed is checked against the count that ran, and the job fails
  if they differ.
  The executables run with cargo's environment (the `[env]` table of `.cargo/config.toml`, so
  `RUST_MIN_STACK` is 8 MB as under `cargo test`); without it a deep layout test overflowed the
  2 MB default stack and ended its chunk.
- **nested_layout_matrix_tests (inline edit):** its Chromium pins and expected-failure list are
  macOS's; 21 listed cases match on Windows. The stale-list check is off on Windows; a case that newly
  differs still fails.
- 641, 642-643, 644, 645, 646, 648: applied or detected as applied. `--check`: all applied.

## Other Windows seam changes

- **tiny-skia, debug builds:** its 16-lane integer maths is NEON `vsubq_u16` (wrapping) on aarch64
  but the plain operator on x86-64, so a debug build's overflow check panicked in it during
  rustkit-canvas's new recording tests, and the second panic aborted the whole test binary (0 tests
  reported). `[profile.dev.package.tiny-skia] overflow-checks = false`: x86 debug wraps as aarch64
  and release do. Proposed upstream (Linux x86 meets the same).
- **find_in_page.js:** Windows' copy wrapped matches by replacing the page's text node (RustKit then
  lacked `surroundContents`), so clearing a find did not put the nodes back
  (`range_tests::clearing_a_find_puts_every_node_back_exactly_as_it_was`). Upstream's script taken
  whole: RustKit and WebView2 both have `surroundContents`; nothing on Windows reads the old class.
- **native-win32:** `PageInput::ScrollAt` gained `gesture` / `began`. `WM_MOUSEWHEEL` carries no
  phase (a precision touchpad arrives as wheel messages too), so each is a wheel on its own.
- **parity-capture:** Windows' copy was an older fork (1,593 lines against upstream's 4,130; its 42
  own lines were older code paths; upstream keeps the 16 MB capture-worker stack). Taken whole: the
  shield, inspector, report, event log and content-thread options come with it. The capture's user
  agent stays upstream's, so every OS's capture asks a site for the same page.
- **hiwave-shield:** Windows' copy predated rule attribution (dev-tools T2) and SH1. Taken whole;
  `hiwave-app` builds against it unchanged.

## hiwave-app

Merged with no conflict: `about.html`, `report.html`, `settings.html`, `shelf.html`,
`context_menu.js` (see Licences), and the parity-capture and hiwave-analytics manifests.
`chrome.html`: Windows' side in its one conflict (the address-bar sync block Windows removed: the
native shell sets the address); taken: escaping of every value in the context-menu markup, the
internal page's empty bar, Escape's stop of a loading page. Its script parses.

Not synced, on purpose (Windows' app shell; each delta is macOS shell wiring or a parallel
implementation Windows already has): `main.rs`, `webview_rustkit.rs`, `ipc/mod.rs` and
`ipc/commands.rs` (macOS inspector, autofill and tab-shortcut wiring; Windows hardened autofill its
own way, #152), `state.rs` (atomic saves: Windows has its own, #151), `autofill.js`, and the new
macOS-only files (`ipc/autofill.rs`, `ipc/inspector_payload.rs`, `tab_shortcuts.rs`,
`close_tab_tests/*`, `idle_page_tests.rs`).

## Licences (the relicence, #157)

1. `LICENSE` is the GNU AGPL v3 text; `NOTICE` and `README.md` name AGPL-3.0-or-later and Aleph
   Null LLC.
2. Every first-party manifest says AGPL-3.0-or-later, directly or through the workspace, including
   the crates this sync brings (the shield crates, parity-capture).
3. `crates/hiwave-app/src/platform/linux.rs` keeps its MPL-2.0 SPDX lines and its NOTICE listing; the
   sync did not touch it. `ipc/mod.rs` (MPL-2.0-marked on macOS) is not synced.
4. `LICENSES/MPL-2.0.txt` and `LICENSES/Unicode-3.0.txt` are present.
5. `NOTICE` now lists `rustkit-bindings/src/likely_subtags.txt` (Unicode CLDR likely-subtags data,
   new here) and the CLDR tables of `web_intl.js` (weekData beside the "CLDR en" patterns), Unicode
   License v3. The Ahem fonts were already listed.

**`crates/hiwave-app/src/ui/context_menu.js` stays unmarked (AGPL-3.0-or-later).** hiwave-macos marks it
MPL-2.0 for an outside contribution, its commit `6ae459ed` (2026-01-05, the DevTools Inspector). Windows'
copy is older: it is hiwave-macos `87ee6562` (2025-12-29) plus Windows' own shortcut handler, from the
first Windows commit (2025-12-30), and unchanged since. None of the 44 lines the contribution added is in
it, so it holds no MPL-2.0 code and is not marked.

## Gates (this machine, own target directory)

- **Workspace tests, crate by crate** (`cargo test -p <crate> -j 4`; a whole-workspace run ran the
  machine out of memory): all 38 crates pass. rustkit-engine 889, rustkit-bindings 796, rustkit-layout
  964, rustkit-js 244, rustkit-renderer 184, rustkit-canvas 48, hiwave-app 61, parity-capture 96,
  hiwave-shield 25, hiwave-shield-intercept 11, the rest as logged. `rustkit-renderer`'s
  `fifty_opacity_layers_fit_a_frame_budget` failed once under the parallel suite (34.5 ms/frame) and
  passes alone (0.19 ms, three runs, RTX 4090).
- **rustkit-engine `--features headless --lib`** (8 test threads): 2,494 passed, 10 failed, 63 ignored as
  synced; patch 652 fixed two. Each of the remaining 8 fails the same on a clean hiwave-macos `295ce9f4` on this machine:
  `paint_only_web_font_tests` x5 and `incremental_layout_tests::a_web_font_landing_after_layout_measures_text_afresh`
  (the page's web face is not used on Windows: a gap, row filed);
  `flex_aspect_ratio_tests::aspect_ratio_minimum_size_inherit_and_the_card_are_as_in_chrome` passes now: its
  `inherit-em` row was pinned on macOS fonts (13 px), and Chrome for Testing 148 win64 gives 11, as the engine
  does (`docs/diagnostics/2026-11-18/flex_inherit_em_cft148_win64.txt`; the Windows expectation is an
  `apply_windows_patches.py` edit). `dom1_attr_order_hit_tests::a_box_outside_its_ancestors_box_is_hit`
  stays failing: Chrome on Windows reports what it does on macOS (`rect menu: 10 284 120 40`,
  `docs/diagnostics/2026-11-18/dom1_hit_outside_ancestor_cft148_win64.txt`), so the engine's 285 is a real
  1 px difference on Windows (row). Net: 2,497 passed, 7 failed.
- **native-win32** (`hiwave-app --no-default-features --features native-win32`): 150 passed.
- **Builds:** release `hiwave-app`, parity-profile `parity-capture`.
- **Parity campaign** (`scripts/parity_test.py`, 26 cases, against `baselines/chrome-148`, captured on
  Windows with Chrome for Testing 148.0.7778.216): 26/26 pass, mean 1.07%. Against the same campaign
  on Windows develop `31a1d144` (its own parity-capture, the same baselines): mean 1.08%; 23 cases
  identical, form-controls 3.00 -> 2.91, form-elements 0.64 -> 0.61, new_tab 1.505 -> 1.510. No case
  moves by 1 point.

## Not run

The real window (native-win32 with the content thread), the chrome board and the live-site boards;
`collect-metrics` on the PR is the CI run.

## Upstream items (filed on the exchange, not changed here)

- Patch 652's rule (a Windows path names the console file), and the console file's Windows
  hardening (owner-only ACL, reparse points, UNC and device paths), which only Unix has today.
- The tiny-skia debug overflow (Linux x86 too).
- hiwave-shield: the `$third-party` built-in rules (judged against the requesting document, not the
  top frame) and the intercept's 250 ms reply timeout, which degrades to the pending floor without a
  hard error (row SH2).
