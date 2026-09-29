# Parity tooling on Windows

Since 2026-09-27 the parity tooling in this repo (the `scripts/*.py`
pipeline, `scripts/tests/`, `tools/parity_oracle/*.mjs`, `cases/`, and
`websuite/*.json`) is a synced copy of hiwave-macos, so the Windows board runs
the same cases and gates as macOS. Pete's rule: the Windows changes live only
in this repo, and hiwave-macos is untouched.

## Re-sync

Run this after an engine refresh, from the same hiwave-macos commit the crates
came from:

```
python scripts/sync_parity_tooling.py <path-to-hiwave-macos-checkout>
```

The script copies the files verbatim, then applies the Windows fixes. Its
docstring lists each fix and each exclusion. Run it a second time and it
writes 0 files, so a reviewer can re-run it to check a sync PR byte for byte.

Windows-only files it never touches:

- the `.ps1` equivalents of the macOS `.sh` scripts
- `scripts/collect_metrics.py`, which CI uses
- `tools/parity_oracle/parity_score.mjs`
- `scripts/conftest.py`
- `baselines/`

## Running

```
cd tools\parity_oracle
npm install --no-save playwright@1.57.0 pngjs@7.0.0 pixelmatch@5.3.0
cd ..\..
set PYTHONUTF8=1
set PARITY_CHROME_PATH=C:\Users\petec\chrome-for-testing\chrome\win64-148.0.7778.216\chrome-win64\chrome.exe
python scripts\parity_swarm.py --scope all --iterations 3 --jobs 2 --skip-build
python -m pytest -q scripts\tests
```

- **node packages:** the versions above are the ones hiwave-macos vendors. `node_modules` is gitignored here.
- **Chrome path:** `PARITY_CHROME_PATH` is needed only for capturing baselines.
- **UTF-8 mode:** `PYTHONUTF8=1` makes piped Python children write UTF-8. Without it, a gate's message can decode to `None` on Windows. `scripts/conftest.py` sets it for the tests.

## Baselines

`baselines/chrome-148/` holds Windows captures from Chrome for Testing
148.0.7778.216. Its `metadata.json` says `platform: win32`. They are not
copied from macOS, because font rasterisation differs by OS.

Capture them with the same tools macOS uses, so both platforms describe the
same process. Set `PARITY_CHROME_PATH` to the pinned binary, then:

```
set PARITY_BASELINE_SET=chrome-148-recap
python scripts\generate_baselines.py
```

Chrome is deterministic here: recapturing an unchanged case gives a
byte-identical PNG. Compare the scratch set against `chrome-148`, copy over the
cases that changed, and delete the scratch set. On 2026-09-29 eight cases had
changed (`new_tab`, `about`, `settings`, `card-grid`, `image-gallery`,
`combinators`, `gradient-no-radius`, `gradient-radius-only`). The July capture
had not applied the micro-suite parity reset the way the current tooling does,
so the micro cases read `line-height: 1.5` on one side and `normal` on the
other. `scripts/audit_baselines.py` now reports all 32 cases clean.

## Where Windows stands against macOS

The comparison is per case against macOS's `metrics/latest-develop.json` on the
`metrics-history` branch of hiwave-macos. Pete's target (2026-09-29) is to be
within about 5 points on every case.

| | Windows | macOS |
|---|---|---|
| cases passing | 26 / 26 | 26 / 26 |
| mean diff over all cases | 1.6% | 1.2% |
| worst case over macOS | `about`, +3.8 points | |

Three things moved the numbers from 23/26 and 5.97% on 2026-09-29:

1. **`settings.html` fixture.** The Windows copy had a single-layer
   `background: radial-gradient(...) var(--bg-primary)`, which RustKit paints
   white. macOS's two-layer form works. 47.83% to 2.06%.
2. **The font chain.** Chrome on Windows does not know `-apple-system` or
   `BlinkMacSystemFont`, so `-apple-system, BlinkMacSystemFont, sans-serif`
   falls through to `sans-serif`, which is **Arial** there. RustKit mapped the
   two aliases to `system-ui` (Segoe UI) and put Segoe UI first for
   `sans-serif`. A 14px line is 16px in Arial and 19px in Segoe UI, so every
   line was 3px too tall and the error accumulated down the page (css-selectors
   drifted about 96px). css-selectors 31.74% to 2.25%; flex-positioning 10.62%
   to 0.94%. `system-ui` itself stays Segoe UI, which is what Chrome uses.
3. **Stale micro baselines** (see above). combinators 11.51% to 0.84%.

`chrome_rustkit` is now measured too: the macOS shell's chrome strip
(`crates/hiwave-app/src/ui/chrome_rustkit.html`) is copied verbatim as a parity
fixture, with a Windows-captured baseline. It is unused by the Windows shell.

`about` and `shelf` are different pages on the two platforms by design. `about`
carries an extra RustKit card, which is why one gate test in
`scripts/tests/test_layout_oracle_gate.py` pins macOS's box count and is marked
as an expected failure in `scripts/conftest.py`.

## Windows-local engine patches

Rule #528: platform-specific differences live in this repo, not upstream.

- `crates/rustkit-layout/src/text.rs`: on Windows, `sans-serif` resolves to
  Arial first, and the macOS-only `-apple-system` / `BlinkMacSystemFont` names
  are skipped. Both are `cfg(windows)`, so macOS and Linux are unchanged. A
  refresh copies the crate verbatim and then re-applies this patch, the same way
  it re-applies the sidecar pin.

## Not synced

These are macOS-only and are not synced:

- the parity CI lane: `.github/workflows/parity.yml`, and the two tests that inspect it. GitHub's Windows runners have no GPU.
- the WPT tier: `trench/wpt/`, which needs `scripts/wpt_sync.sh`.
- `parity-baseline/`, the macOS board's own state.
