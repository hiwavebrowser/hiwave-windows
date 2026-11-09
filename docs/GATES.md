# Gates — every parity threshold in one place

Pete, 2026-11-06: "keep these figures handy so we can tweak them later." Each gate's
number lives in exactly one constant; this file says where, what it compares, and why
it is the value it is. Change the constant and this table together.

| Gate | Compares | Threshold | Constant | Set / changed |
|---|---|---|---|---|
| Chrome board (our own UI pages, 42 rows) | pixels and geometry vs Chromium 143 on `chrome_fixtures/board.json` | **green at ≤ 0.5 % pixels, OR 0 geometry diffs (and 0 join failures) and ≤ 1.5 % pixels**; a row whose geometry compare did not run has only the first rule; `BOARD.md`'s `gate` column says which rule made a row green | `scripts/chrome_board.py` `PIXEL_GATE_PCT` (0.5), `GEOMETRY_PIXEL_GATE_PCT` (1.5), decided only in `row_is_green()` / `green_rule()`; `--rescore <board.json>` re-scores an old run | 0.5 % CH0 definition of done (11-04); second rule added 11-06 (Pete): a row whose geometry matches Chrome to 0.5 px and differs only by rasterisation (CoreText vs Skia anti-aliasing) is done |
| Chrome board geometry | (box, axis) pairs vs Chrome rects on the join selector | 0.5 px per pair; the count is reported, the second pixel rule needs 0 | `scripts/layout_oracle_gate.py` `compare_case` | CH3.A1 |
| 26-case campaign | pixels vs banked Chromium captures, per reduced case | regression only: a case fails if its diff grows by more than the allowed step vs the banked baseline; the mean (~1.1 %) is reported, not gated | `scripts/parity_gate.py` `--fail-on-regression` (Linux gate runner uses +1.0 point) | since the parity trench |
| Real-site board LOOKS RIGHT | first-viewport pixels vs Chrome, per live site | **≤ 20 %** (was 15 %); Chrome-vs-Chrome noise above the bound scores "unstable", not fail | `scripts/realsite_board.py` `LOOKS_RIGHT_MAX` (scorer_v2.py's unused copy removed) | 15 % at T1; 20 % 11-06 (Pete: "for now") |
| Real-site board READABLE | first-viewport words Chrome shows that RustKit's text runs contain | ≥ 80 % | `scripts/realsite_board.py` `READABLE_MIN` | T1 |
| Batch A/B (19 sites) | candidate binary vs develop binary, same page | no absolute: a "mover" is a change beyond the site's own run-to-run noise; pixel = any channel differs by > 8 | batch method, `ab<N>.py` in the batch scratch dir | batch 17 |
| WPT tier-1 sample (349 cases) | assertions, not pixels; a pixel case must also fail when its property is removed to count as proven | pass / fail / unproven | `scripts/wpt_coverage.py` | T15/T15b |
| Latency exit metric (LAT0) | Pete's hand-test logs | script-budget stops/min ≤ 1, content-thread hold p95 ≤ 50 ms, input-to-present p95 ≤ 100 ms | `trench/latency/BASELINE.md` | 11-05; p95 needs LAT1's summary line |

Rules of thumb: an absolute pixel gate is only honest where we control both sides of the
page (the chrome board). Engine correctness is gated by regression (campaign, A/B) and by
assertions (WPT). Live sites get a loose bound because their content moves.
