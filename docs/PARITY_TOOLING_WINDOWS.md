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

At the first sync, 30 of 32 cases had a correctly sized Windows baseline. Two
did not, so both read NOT-MEASURED:

| Case | Committed baseline | Case viewport | Remedy |
|---|---|---|---|
| `settings` | 800x600 | 1024x768 | recapture on Windows |
| `chrome_rustkit` | none | 1280x100 | waits for the Windows shell |

`chrome_rustkit` is the macOS shell's chrome strip,
`crates/hiwave-app/src/ui/chrome_rustkit.html`. This repo has no such file,
and Windows shell parity comes after V1.2, so the case stays unmeasured until
then. `scripts/audit_baselines.py` reports both cases.

## Not synced

These are macOS-only and are not synced:

- the parity CI lane: `.github/workflows/parity.yml`, and the two tests that inspect it. GitHub's Windows runners have no GPU.
- the WPT tier: `trench/wpt/`, which needs `scripts/wpt_sync.sh`.
- `parity-baseline/`, the macOS board's own state.
