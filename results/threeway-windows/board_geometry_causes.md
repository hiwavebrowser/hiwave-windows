# Windows chrome board: geometry causes (Atlas 1898 item 2)

The Windows counterpart of the Linux study (Talos, hiwave-linux b37be7fd).

## Setup

- **Rows:** the 36 board rows of the Windows column of the three-way comparison: hiwave-windows develop
  f73b8ca0 = hiwave-macos 2f70f02f (batch 111).
  - The pages are the macOS UI pages at 2f70f02f; the renderers are Windows': RustKit release `parity-capture`,
    and CfT 148.0.7778.216 win64.
  - That is the board measured at 18/42 green, mean geometry diffs 105.0.
  - Inspector is excluded: not shipped on Windows.
- **Method:**
  - `layout_oracle_gate.compare_case` on every row's Chrome rects and RustKit layout.
  - Each failing (box, axis) is CARRIED when its nearest Chrome-measured ancestor (from the selector chain) moves by
    the same delta within 0.5 px (`geometry_attribution.py`'s rule); otherwise it is an ORIGINATOR.
  - Caveat: id-rooted selectors (`#rightDownloadsList`) break the chain, so some carried boxes count as originators.
    The cause split below is not affected by that.
- **Causes:** named from numbers in both arms.
  - Chrome: `CSS.getPlatformFontsForNode` per element (the face Chrome actually drew), and rects.
  - RustKit: `parity-capture --dump-display-list` (each text run's face and advances) and its layout.
- **Scripts:** `board_causes.py` (the split), `glyph_probe.mjs` and `icon_probe.mjs` (the Chrome probes), in this directory.

## Cause table

| page | rows | A fallback face | B line union | W widget `<select>` | M monospace generic | C chart 0x0 | E sub-pixel | all |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| chrome | 21 | 360 / 360 | 777 / 483 | 1725 / 855 | - | - | 414 / 204 | 3276 |
| settings | 3 | - | - | - | 6 / 6 | - | 195 / 75 | 201 |
| report | 6 | - | 48 / 18 | - | - | 24 / 24 | 27 / 27 | 99 |
| shelf | 6 | - | - | - | 72 / 48 | - | - | 72 |
| **all** | 36 | 360 / 360 | 825 / 501 | 1725 / 855 | 78 / 54 | 24 / 24 | 636 / 306 | 3648 |
| share of all diffs | | 9.9% | 22.6% | 47.3% | 2.1% | 0.7% | 17.4% | |

Cells are failing (box, axis) pairs / originators among them, summed over the page's rows.

## The causes, with numbers

### A. Fallback face ignores Emoji_Presentation (Linux (a), on Windows)

- RustKit draws every character the primary face lacks from **Segoe UI Emoji**. Chrome (DirectWrite system fallback)
  draws text-presentation symbols from **Segoe UI Symbol**.
- 14 px icons, Chrome face and width vs RustKit's advance:

  | character | Emoji_Presentation | Chrome face | Chrome width | RustKit advance |
  |---|---|---|---:|---:|
  | U+1F6E1 SHIELD | No | Segoe UI Symbol | 14.00 | 19.22 |
  | U+2699 GEAR | No | Segoe UI Symbol | 11.81 | 19.22 |
  | U+2B07 DOWNWARDS BLACK ARROW | No | Segoe UI Symbol | 7.66 | 19.22 |
  | U+1F6AB, U+1F4C8, U+1F50D, U+1F50E, U+1F551 | Yes | Segoe UI Emoji | 19.23 | 19.22 |

- The deltas +5.22, +7.41 and +11.56 (and +5.78, half of 11.56, for a centred icon) are the chrome page's largest
  originators. Labels beside the icons shift by the same amounts.
- **Where:**
  - `rustkit-layout/src/text.rs`, Windows path: the advance of a character missing from the primary face comes
    from Segoe UI Emoji.
  - `rustkit-renderer/src/glyph.rs`: paints it from Segoe UI Emoji.
  - Both are synced crates, so the fix is an upstream hiwave-macos PR: take Emoji_Presentation into account, as
    Linux #150 does in `linux.rs`, or use `IDWriteFontFallback::MapCharacters` as Chrome does.

### B. Line not united with the fallback face (Linux (b), on Windows)

- Every line holding a fallback-face icon is **3 px shorter** in RustKit:
  - right-sidebar tabs: 41 vs 38, and 39 vs 36;
  - right-sidebar actions: 56 vs 53;
  - report export buttons: 35 vs 32. U+1F504 is in the 4th button, and the flex row stretches the others.
- The -6 px below the right sidebar's two rows is this, twice.
- **Where:** `rustkit-layout`'s `measure_text_advanced` unites a fallback face into the line only when the platform
  measurement reports the united ascent. Windows' does not:
  `test_an_emoji_label_lifts_a_styled_buttons_line` prints "SKIP: this platform does not unite fallback faces
  into a line" on Windows. It passes here only by skipping, and it fails on Linux.
- **Shared with Linux (b).** The line-box rule is already shared; what is missing is each platform's measurement
  reporting the fallback face's ascent and descent. Fix the Windows path in `text.rs` (upstream), alongside
  Linux's.

### W. UA/widget metrics: inline `<select>`

- `#modeSelector` (a `<select>`) has the same box in both engines in the default state (12, 50, 195x31). The line
  under it (`#modeDescription`) starts **+0.71 px** lower in RustKit. That is the inline select's line box (baseline
  and strut) being 0.71 taller, and the rest of the left sidebar carries it (+0.71, -1.29, -0.79 chains).
- In the collapsed states (sidebar_collapsed, focus_mode, downloads_open), Chrome shrinks the select to **24 px**
  wide and RustKit keeps **105 px** (+81 px): the widget's intrinsic or min-content width in a narrow container.
- Not in Linux's table (Linux's left sidebar may carry it under other numbers); worth a check there.
  `form_control_intrinsic_size` is shared code.

### M. Generic `monospace` face (Windows only)

- Shelf shortcut chips and settings' `#focusShortcutKey` are `font-family: monospace`. Chrome on Windows draws them
  in **Consolas**; RustKit is a constant **+0.036 em per glyph** wider:
  - 10 px: +0.36 px/glyph, so +4.33 on "Ctrl+Shift+S" (12 glyphs) and +2.16 on 6;
  - 13 px: +0.47 px/glyph.
- That is the advance of a 0.586 em face (Cascadia Mono) against Consolas' 0.55 em. Inferred from the widths, not
  yet read from the face name: these runs go through the per-character path, which records no face.
- The neighbouring label shrinks by the same amount (-4.33). The chips are also 1 px short (15 vs 16).
- **Where:** the Windows generic-family map (the `monospace` entry). Windows only.

### C. Chart.js canvases 0x0 (Linux (c))

- `#activityChart` / `#protectionChart`: 1150x300 in Chrome, 0x0 in RustKit, on every report row. Same as Linux
  (c), Mac lane (E10b).

### E. Sub-pixel text and line metrics (Linux (e))

- settings: labels drift +0.8 to +1.0 px, growing per section, with a 12.8 px font and 19.2 px line-height
  (fractional line boxes), as on Linux.
- report: the period buttons are +1.55 px wide on 11 Arial glyphs (0.14 px per glyph). The oracle runs with
  `--disable-font-subpixel-positioning`, which may round Chrome's advances, against RustKit's fractional ones. To
  confirm.

## Shared with Linux, so fix once

| cause | Linux row | fix lives in | shared? |
|---|---|---|---|
| A fallback face, Emoji_Presentation | (a), #150 (`linux.rs`) | `rustkit-layout/src/text.rs` + `rustkit-renderer/src/glyph.rs`, Windows paths | same rule, per-platform code: an upstream PR for the Windows paths |
| B line union with the fallback face | (b) | each platform's measurement feeding the shared `measure_text_advanced` | the rule is shared; Windows and Linux both lack the united ascent |
| C Chart.js 0x0 | (c) | Mac lane | yes |
| E sub-pixel line metrics | (e) | shared layout | yes |
| W `<select>` line box and narrow width | not in Linux's table | `form_control_intrinsic_size` / inline-block baseline (shared) | check on Linux |
| M `monospace` = Consolas | none | Windows generic-family map | Windows only |

**Suggested order by board effect:**

1. B (23% of diffs, every icon row).
2. A (10%, the largest originators).
3. W (47%: mostly the +0.71 carried down the sidebar, one root).
4. M.
5. E.
