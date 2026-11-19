# Three-way rendering comparison: Windows column

Three sets: campaign 26, websuite cases + micro 29, chrome board 42.

RustKit on Windows vs Chrome on Windows. Measured on busybee (Windows 11, RTX 4090), hiwave-windows develop
**f73b8ca0** (= hiwave-macos develop **2f70f02f**, batch 111, plus Windows' own patches: sync r21, #163).

## Environment

- **Chrome:** Chrome for Testing 148.0.7778.216 win64, pinned, run through Playwright as the parity oracle runs
  it (`PARITY_CHROME_PATH`). The board checks `browser.version()` against the pin on every row.
- **RustKit:** release `parity-capture` built from f73b8ca0 (sha256 prefix 33947db9a1bfdc08), headless.
- **Fonts:** the Windows system fonts for both browsers (no fontconfig file on Windows; Linux pins one).
- **Campaign:** `scripts/parity_test.py`. Baselines are Windows Chrome 148 renders in `baselines/chrome-148`
  (captured on Windows with the pinned CfT).
- **Websuite:** `run_websuite.py` (RustKit `parity-capture --html-file`) and `capture_chrome_websuite.mjs`
  (the oracle's deterministic context, with the parity reset injected once `<html>` exists, the Linux column's
  fix), at the campaign registry's size or 1280x800.
- **Board:** `scripts/chrome_board.py` run from a hiwave-macos checkout at **2f70f02f** (the board's UI pages and
  fixtures: Windows keeps its own fork of the app's UI pages and has no `inspector.html` or board fixtures), with
  the Windows-built `parity-capture` and the Windows CfT 148: the same pages as the other columns, Windows'
  renderers. Through `run_board_windows.py`, which reads the pinned version from CfT's `<version>.manifest`
  (on Windows `chrome.exe --version` prints nothing, so the board's up-front probe refuses; follow-up).
- **Network:** local fixtures only; no live sites.

## Metrics (per pair, then the mean over the set)

The Linux column's definitions and script, unchanged (`threeway_metrics.py`):

- `pixels_over_8_pct`: the share of pixels where ANY RGB channel differs by more than 8 (provisional).
- `mean_channel_diff`: the mean absolute RGB channel difference over all pixels, 0-255 (provisional).
- **Board:** `geometry_diffs` and `board_green` come from the board's own `board.json`.
- **Campaign:** the tool's own pixelmatch `%` is kept as `campaign_pixelmatch_pct`.

No image pair differed in size; no pair failed to score.

## Summary, with the Linux column (hiwave-linux a5f9ca21, batch 109) beside it

| set | Windows `pixels_over_8` | Windows `mean_channel_diff` | Linux `pixels_over_8` | Linux `mean_channel_diff` |
|---|---|---|---|---|
| campaign 26 | 2.2243% | 1.0619 | 2.0048% | 1.2423 |
| websuite 29 | 1.5804% | 0.8798 | 1.3741% | 1.0322 |
| chrome board 42 | 1.3203% | 0.7075 | 1.6401% | 0.9173 |

The two columns are not yet on one engine: Linux measured batch 109, Windows batch 111 (Talos may rerun Linux at
2f70f02f).

## Campaign (26 cases)

- **Mean `pixels_over_8_pct`:** 2.2243%
- **Mean `mean_channel_diff`:** 1.0619
- **Campaign pixelmatch mean 1.0729%;** 26/26 pass the campaign's own thresholds.
- **Pass counts at `pixels_over_8_pct` <=** 0.5% / 1% / 2%: 2 / 4 / 18 of 26 (provisional thresholds).

Worst 10 by `pixels_over_8_pct`:

| case | pixels > 8 | mean channel diff |
|---|---|---|
| `about` | 9.1865% | 3.1013 |
| `new_tab` | 7.2277% | 2.1125 |
| `article-typography` | 5.8265% | 4.3233 |
| `form-controls` | 3.5897% | 4.0504 |
| `css-selectors` | 2.867% | 1.1325 |
| `chrome_rustkit` | 2.5438% | 0.7879 |
| `card-grid` | 2.3999% | 0.7545 |
| `backgrounds` | 2.0729% | 1.177 |
| `combinators` | 1.8748% | 0.6848 |
| `specificity` | 1.87% | 0.7586 |

## Websuite cases + micro (29 pages)

- **Mean `pixels_over_8_pct`:** 1.5804%
- **Mean `mean_channel_diff`:** 0.8798
- **Pass counts at `pixels_over_8_pct` <=** 0.5% / 1% / 2%: 5 / 7 / 21 of 29 (provisional thresholds).

Worst 10 by `pixels_over_8_pct`:

| case | pixels > 8 | mean channel diff |
|---|---|---|
| `cases__article-typography` | 5.8265% | 4.3233 |
| `micro__form-controls` | 3.5897% | 4.0504 |
| `cases__css-selectors` | 2.867% | 1.1325 |
| `cases__card-grid` | 2.3999% | 0.7545 |
| `micro__image-intrinsic-size` | 2.2821% | 1.3799 |
| `micro__selectors-specificity` | 2.1916% | 0.9131 |
| `micro__selectors-combinators` | 2.1799% | 0.9063 |
| `micro__backgrounds` | 2.0729% | 1.177 |
| `micro__combinators` | 1.8748% | 0.6848 |
| `micro__specificity` | 1.87% | 0.7586 |

## Chrome board (42 rows)

- **Mean `pixels_over_8_pct`:** 1.3203%
- **Mean `mean_channel_diff`:** 0.7075
- **Board's own rule: 18/42 green; mean geometry diffs 105.0** (`board_out/BOARD.md`).
- **Pass counts at `pixels_over_8_pct` <=** 0.5% / 1% / 2%: 10 / 17 / 34 of 42 (provisional thresholds).

Worst 10 by `pixels_over_8_pct`:

| case | pixels > 8 | mean channel diff |
|---|---|---|
| `inspector/default/1024x700` | 5.279% | 3.0552 |
| `inspector/default/1280x800` | 3.6953% | 2.1387 |
| `chrome/downloads_open/1024x700` | 2.4074% | 1.2729 |
| `inspector/default/1600x1000` | 2.365% | 1.3687 |
| `chrome/address_focused/1024x700` | 2.3006% | 1.4053 |
| `chrome/hover_tab2/1024x700` | 2.1657% | 1.2364 |
| `chrome/default/1024x700` | 2.1588% | 1.2444 |
| `chrome/find_open/1024x700` | 2.0033% | 1.1622 |
| `chrome/address_focused/1280x800` | 1.9116% | 1.1698 |
| `settings/default/1024x700` | 1.7486% | 0.7187 |

## Files

- `campaign.json`, `websuite.json`, `chrome_board.json`: the Linux column's schema (summary + rows).
- `rustkit/<set>/*.png`, `chrome/<set>/*.png`: the image pairs each row was scored from.
- `board_out/BOARD.md`, `board_out/board.json`: the board's own report.
- `threeway_metrics.py` (unchanged), `run_websuite.py`, `capture_chrome_websuite.mjs`, `run_board_windows.py`:
  how each set was captured.

## Follow-ups (not changed here)

- `chrome_board.py`'s pin probe uses `chrome --version`, which prints nothing on Windows; read
  `<version>.manifest` there (as `run_board_windows.py` does) or `browser.version()` alone.
- Windows' app UI pages are a fork without the board's fixtures, so the board runs from a macOS checkout here.
