# Nested layout matrix, 2026-10 (D1, test half)

One layout mode inside another, against Chromium. 609 small pages; on
develop **378 match Chromium and 231 do not**. On the PR #663 branch
(`atlas/z-grid-in-flex-width`, head `7e9b2b34`) **417 match and 192 do
not**: #663 makes 39 cases match and makes none fail.

Update, 2026-10-12, on the #663 branch with develop `2b75e93b` merged in
(#670, #672 and #673 landed in between): **449 match and 160 do not.** 48
lines left the ledger with this branch and none joined it: 28 of
`grid-item-of-flex-laid-out-as-block`, 17 of
`grid-max-content-width-no-grid-arm` and all 3 of
`grid-auto-rows-skips-first-row`. The cause sections below are as written
for develop.

## What is in the matrix

- Generator: `tools/parity_oracle/nested_layout_matrix.mjs`. Run it with no
  argument to list the cases, with `--write` to capture them in Chromium and
  write `nested_layout_matrix_cases.json` (one case per line). Add an outer
  mode, an inner mode or a rider there and recapture.
- Reference: Chromium **141.0.7390.37** (Playwright's build 1194), headless,
  1280x800, device scale 1. Each element with an id has its border box from
  `getBoundingClientRect()` to two decimals. The pinned Chromium
  (143.0.7499.4) could not be downloaded in the container. As a check, the
  committed `grid_item_min_max_cases.json` (captured with 143) was recaptured
  with 141: every box that holds no text is identical; the only differences
  are the widths of text boxes (39 against 38), which is the Linux font, not
  the browser. No page in this matrix has text. Recapture with the pinned
  build when a seat has it.
- Test: `nested_layout_modes_are_as_in_chromium` in
  `crates/rustkit-engine/src/nested_layout_matrix_tests.rs`. It is a plain
  `#[cfg(test)]` test (not `headless`), so it runs in the Linux container:
  `cargo test -p rustkit-engine --lib nested_layout_matrix` (about 1 s for the
  609 pages, one engine). Each page is laid out as `relayout` lays out a view
  (viewport set, the margin-collapse entry point, the root's children as
  formatting roots) and read the way `getBoundingClientRect` reads the layout
  (`geometry_snapshot`). Every element's x, y, width and height must be
  within 1px of Chromium. (In review, all 609 pages were also run through
  `create_headless_view` + `load_html` + script `getBoundingClientRect`: 608
  give identical boxes; the 609th is cause 17's dropped box, reported as
  missing here and as 0x0 there, wrong against Chromium both ways.)
- Font independence: every page sets `font-size: 0; line-height: 0` on the
  body and has no text. Every leaf is an empty box whose size is fixed, or
  comes only from stretch, a percentage or its container. With no strut, a
  baseline would decide nothing, so the inline-block outer has a 10x10
  inline-block `#s` beside it on its line: `#s` sits on the outer box's
  baseline, which is how cause 4 shows without a font. (Without `#s`, the
  same 25 cases fail as soon as the body has a normal font: their line box
  is a strut descent too tall.)
- Ledger: `tools/parity_oracle/nested_layout_matrix_expected_failures.txt`
  lists each case that does not match yet with one cause tag. A case not
  listed must match; a listed case must still differ, so a fix that makes a
  case match fails the test until its line is removed. Every tag must be a
  cause heading in this document (the test checks it). A case may have more
  than one cause: **the tag names the cause of the first box, in document
  order, that differs on develop**; the `#r` line box and the `#s` probe
  count only when nothing else differs. `NESTED_MATRIX_DUMP=<file>` writes
  every case's engine and Chromium boxes; `NESTED_MATRIX_CASES=<file>` runs
  another case file (a reduced page) and checks nothing against the ledger.

### The axes

Core (480 cases): outer mode {block, float, inline-block, flex row, flex
column, grid, absolutely positioned, table cell} x inner mode {block, flex
row, flex column, grid (`auto auto` columns), inline-block} x inner width
{auto, 50%, 200px, intrinsic} x inner height {auto, 50%, 120px}. The
intrinsic width is `min-content` with height auto, `max-content` with 50%
and `fit-content` with 120px, so each keyword meets every outer and inner
mode. The outer box is 300px tall in every mode; the shrink-to-fit outers
(float, inline-block, absolute) have no width. The inner box holds a 50x20
box, a 70x30 box and a box that is only `height: 10px`.

Riders (129 cases), each crossed with the outer modes where it means
something (mostly block, flex row, flex column, grid): `aspect-ratio` alone,
with a fixed width, with a fixed height, and an item with a ratio in a grid
cell; a percentage height two levels down through an auto-height box and
through a 200px one; grid column and row spans with and without
`grid-auto-rows: 40px`; `min-width`, `max-width`, `min-height`, `max-height`
on the inner container; an absolutely positioned child sized by `inset`, by
`top`+`bottom`, by `left`+`right`; `margin: auto` on a flex item and on a grid
item; `gap` in a wrapping flex row and in a grid; `box-sizing: border-box`
with padding and border on a flex and a grid container; a wrapping flex row
at auto and fixed width; `align-items`/`justify-content` center against
stretch in flex and grid; an empty `flex: 1 1 0` item and an item whose
content keeps it from shrinking; a flex container (grow, wrap, column) in a
float, an inline-block and an absolutely positioned box.

Not covered: images (`<img>` with a ratio from width/height attributes; the
matrix uses boxes with `aspect-ratio`), text and anything font-dependent,
tables as the inner mode, `position: sticky`, writing modes, `contain`,
scroll containers, `@media`.

## Causes, most cases first

Counts are by the tag (the first differing box on develop). "On #663" is how
many of that tag's cases match on the #663 branch; where a case stops at a
second cause, it is named under its first.

| # | cause tag | cases | match on #663 | where (by reading) |
|---|---|---|---|---|
| 1 | `intrinsic-width-keyword-unparsed` | 36 | 0 | rustkit-css `parse_length` |
| 2 | `grid-item-of-flex-laid-out-as-block` | 36 | 26 | flex.rs step 11 (#663 cause 1) |
| 3 | `fit-content-width-is-zero` | 27 | 0 | rustkit-layout `calculate_block_width` |
| 4 | `inline-block-baseline-of-flex-or-grid-content` | 25 | 0 | rustkit-layout `inline_block_baseline_y` / `baseline_is_bottom_edge` |
| 5 | `grid-max-content-width-no-grid-arm` | 27 | 13 | grid.rs `estimate_max_content_width` (#663 cause 2) |
| 6 | `flex-item-min-width-is-its-own-width` | 20 | 0 | flex.rs `create_flex_item` with grid.rs `own_min_content_width` |
| 7 | `table-cell-percent-height` | 16 | 0 | table.rs `layout_cell` / `stretch_cell` |
| 8 | `grid-auto-rows-not-stretched-to-non-px-height` | 9 | 0 | grid.rs `layout_grid_container`, `own_definite_height` |
| 9 | `gap-two-value-shorthand-dropped` | 6 | 0 | rustkit-engine `apply_style_property`, `"gap"` arm |
| 10 | `column-flex-item-stretches-to-outer-width` | 5 | 0 | flex.rs, not located |
| 11 | `fit-content-grid-item-stretches` | 5 | 0 | grid.rs `apply_justify_self` |
| 12 | `grid-max-content-width-zero-with-column-span` | 5 | 0 | grid.rs content-width estimate |
| 13 | `aspect-ratio-width-not-derived-from-height` | 4 | 0 | rustkit-layout `calculate_block_width`; flex.rs for the flex item |
| 14 | `grid-auto-rows-skips-first-row` | 3 | 0 | grid.rs, not located |
| 15 | `aspect-ratio-height-of-column-flex-item` | 2 | 0 | flex.rs, not located |
| 16 | `grid-item-auto-margins-ignored` | 2 | 0 (block axis fixed, inline not) | grid.rs `place_item_in_area` |
| 17 | `aspect-ratio-only-empty-block-dropped` | 1 | 0 | rustkit-engine `empty_block_affects_layout` |
| 18 | `percent-height-chain-in-stretched-grid-item` | 1 | 0 | unknown |
| 19 | `max-height-ignored-on-stretched-item` | 1 | 0 | grid.rs stretch; flex.rs cross size |
| | **total** | **231** | **39** | |

Of the cases tagged 2 or 5 that still fail on #663 (10 and 14): 5 stop at
cause 4 (the inline-block outer's baseline), 10 at cause 8 (auto rows not
stretched), 3 at cause 3 (`width: fit-content` is 0), 2 at cause 16, 2 at
cause 9, 1 at cause 14 and 1 at cause 19.

Each "smallest page" below is the body of a page whose `<head>` is
`<style>body{margin:0;font-size:0;line-height:0}</style>`; boxes are
`x, y, width, height`.

### 1. `intrinsic-width-keyword-unparsed` (36)

**Rule:** `width: min-content` and `width: max-content` size the box to its
content; the engine drops the declaration and the box fills its containing
block as `auto`.

```html
<div id="i" style="width:min-content"><div id="b" style="width:70px;height:30px"></div></div>
```

`#i`: engine 0, 0, 1280, 30; Chromium 0, 0, 70, 30.

Where: `parse_length` in `crates/rustkit-css/src/lib.rs` returns `None` for
`min-content` and `max-content` (it knows `auto` and `fit-content`), and
`Length` has no variant for them; the engine's own `parse_track_size` reads
them only as grid track sizes. In a shrink-to-fit outer (float,
inline-block, absolute) and for an inline-block inner the box shrinks anyway,
so those cases pass by accident; it shows in block, flex, grid and table-cell
outers.

### 2. `grid-item-of-flex-laid-out-as-block` (36; 26 match on #663)

**Rule:** a grid container that is a flex item lays its children out on its
grid; the engine stacks them as blocks.

```html
<div id="o" style="display:flex;width:600px"><div id="i" style="display:grid;grid-template-columns:auto auto">
  <div id="a" style="width:50px;height:20px"></div><div id="b" style="width:70px;height:30px"></div></div></div>
```

`#i`: engine 0, 0, 70, 50; Chromium 0, 0, 120, 30. `#b`: engine 0, 20;
Chromium 50, 0.

Where: flex step 11 (`crates/rustkit-layout/src/flex.rs`), cause 1 of PR
#663. Left on #663: `core--flex-row--grid--w-*--h-auto` (4) stop at cause 8,
the grid stretched to the row's 300px and its auto rows not;
`gap--flex-{row,col}--grid` at cause 9; `margin-auto--flex-{row,col}--grid`
at cause 16; `grid-span--flex-col--auto-rows-40` at cause 14;
`clamp--flex-row--max-height-grid` at cause 19.

### 3. `fit-content-width-is-zero` (27, and 3 more behind cause 5)

**Rule:** `width: fit-content` on a block is `min(max-content,
max(min-content, available))`; the engine makes it 0.

```html
<div id="i" style="width:fit-content"><div id="b" style="width:70px;height:30px"></div></div>
```

`#i`: engine 0, 0, 0, 30; Chromium 0, 0, 70, 30.

Where: `calculate_block_width` in `crates/rustkit-layout/src/lib.rs` has an
arm for `Length::Auto` and sends every other value, `Length::FitContent`
included, through `length_to_px`, which is 0 for it (`Length::to_px` in
rustkit-css: "Context-dependent, exactly as Auto"). In a flex row outer the
item is content-sized by the flex pass and passes. In a grid outer the item
is stretched instead (cause 11).

### 4. `inline-block-baseline-of-flex-or-grid-content` (25)

**Rule:** an inline-block whose content is a flex or grid container takes
its baseline from that container's first item (an item with no baseline of
its own gives its bottom edge); the engine walks it as blocks, finds no
baseline, and puts the inline-block's baseline at its bottom edge.

```html
<div id="o" style="display:inline-block"><div id="i" style="display:flex">
  <div id="a" style="width:50px;height:20px"></div><div id="b" style="width:70px;height:30px"></div></div></div><div
  id="s" style="display:inline-block;width:10px;height:10px"></div>
```

`#s`: engine 120, 20; Chromium 120, 10 (the baseline at `#a`'s bottom, 20,
not `#o`'s, 30). The same with `display: inline-flex` in place of the
wrapper and `#i`. With a block inner (`core--inline-block--block--*`) both
use the bottom edge and match. With a normal body font and no `#s`, the
same 25 cases show it as a line box one strut descent too tall (`#r` 311.12
in the engine, 308 in Chromium, for `core--inline-block--flex-row--w-auto--h-auto`).

Where: `inline_block_baseline_y` and `baseline_is_bottom_edge` in
`crates/rustkit-layout/src/lib.rs`: the walk looks for the last descendant
with a baseline (text, a control, an image) through any block, flex or grid
child, so a flex or grid container of empty items gives none.

### 5. `grid-max-content-width-no-grid-arm` (27; 13 match on #663)

**Rule:** the max-content width of a grid is the sum of its columns; the
engine takes its widest child, so a grid in a float, an inline-block or an
absolutely positioned box is too narrow.

```html
<div id="o" style="float:left"><div id="i" style="display:grid;grid-template-columns:auto auto">
  <div id="a" style="width:50px;height:20px"></div><div id="b" style="width:70px;height:30px"></div></div></div>
```

`#o` and `#i`: engine 70 wide; Chromium 120.

Where: `estimate_max_content_width` / `own_max_content_width` in
`crates/rustkit-layout/src/grid.rs`, cause 2 of PR #663. Left on #663: of the
8 other inline-block outer cases, 5 stop at cause 4 and 3 (`*--h-50pct`) at cause
8 with cause 4 behind it; `core--abspos--grid--*--h-50pct` (3) stop at
cause 8; `core--{float,inline-block,abspos}--grid--w-fit-content--h-120px`
(3) stop at cause 3 (the inner grid is 0 wide once the outer box is 120).

### 6. `flex-item-min-width-is-its-own-width` (20)

**Rule:** a flex item's automatic minimum width is the smaller of its
specified width and its content's min-content width; the engine uses its
specified width for both, so an item with a `width` never shrinks below it.

```html
<div id="i" style="display:flex;width:60px">
  <div id="a" style="width:50px;height:20px"></div><div id="b" style="width:70px;height:30px"></div></div>
```

`#a`: engine 50 wide, Chromium 25. `#b`: engine 70 at x 50, Chromium 35 at
x 25. With `min-width: 0` on both items the engine matches. With words in
the items (two `width: 250px` items in a 300px row): Chromium 150 and 150,
engine 250 and 250.

Where: `create_flex_item` in `crates/rustkit-layout/src/flex.rs` takes the
content size suggestion from `estimate_min_content_width`
(`crates/rustkit-layout/src/grid.rs`), and `own_min_content_width` returns the
box's own `width` when it is in px ("An explicit pixel width fixes the
contribution regardless of content"). That is right for a contribution to a
parent and wrong for the item's own content size suggestion. Also seen as: a
flex row with `width: 50%` in a shrink-to-fit outer, a flex row with
`max-width`, and both `flex-min-auto` riders.

### 7. `table-cell-percent-height` (16)

**Rule:** a percentage height inside a table cell resolves against the
cell's height; the engine resolves it as `auto`.

```html
<div style="display:table;width:600px;height:300px"><div id="o" style="display:table-cell">
  <div id="i" style="height:50%"></div></div></div>
```

`#i`: engine 600 x 0; Chromium 600 x 150. The same with `height: 300px` on
the cell instead of the table.

Where: `layout_cell` in `crates/rustkit-layout/src/table.rs` lays the cell's
content out with the cell's height set aside (`auto`) and a containing block
0 tall; `stretch_cell` then makes the cell taller and moves the content for
`vertical-align` but does not lay it out again, so a percentage never sees
the row's height.

### 8. `grid-auto-rows-not-stretched-to-non-px-height` (9, and 10 more behind causes 2 and 5 on #663)

**Rule:** a grid with a definite height stretches its `auto` rows to fill it
(`align-content: normal`); the engine does this only when the height is in
px (or a font or viewport unit), not when it is a percentage or comes from
the grid being stretched by its parent.

```html
<div style="height:300px"><div id="i" style="display:grid;height:50%">
  <div id="a" style="height:20px"></div><div id="b" style="height:20px"></div></div></div>
```

`#b`: engine y 20; Chromium y 75 (rows of 75 and 75).

Where: `layout_grid_container` in `crates/rustkit-layout/src/grid.rs` calls
`stretch_auto_tracks` with the container height, which comes from
`own_definite_height`; that reads only px and font/viewport-relative
heights. Read, not confirmed by a change.

### 9. `gap-two-value-shorthand-dropped` (6, and 2 more behind cause 2)

**Rule:** `gap: <row> <column>` sets both gaps; the engine drops any `gap`
with two values (one value and the longhands work).

```html
<div id="i" style="display:grid;grid-template-columns:50px 50px;gap:10px 20px">
  <div id="a" style="height:20px"></div><div id="b" style="height:20px"></div><div id="c" style="height:20px"></div></div>
```

`#b`: engine x 50, Chromium 70. `#c`: engine y 20, Chromium 30.

Where: the `"gap" | "grid-gap"` arm of the style property match in
`crates/rustkit-engine/src/lib.rs` passes the whole value to `parse_length`,
which fails on `10px 20px`, and nothing is set.

### 10. `column-flex-item-stretches-to-outer-width` (5)

**Rule:** the items of a column flex container that is itself a row flex
item stretch to that container's width; the engine stretches them to the
outer row's width.

```html
<div id="o" style="display:flex;width:600px"><div id="i" style="display:flex;flex-direction:column">
  <div id="b" style="width:70px;height:30px"></div><div id="c" style="height:10px"></div></div></div>
```

`#i` is 70 wide in both. `#c`: engine 600 wide, Chromium 70.

Where: in `crates/rustkit-layout/src/flex.rs`; not located. A block inner
(`display: block` in place of the column flex) matches.

### 11. `fit-content-grid-item-stretches` (5)

**Rule:** a grid item with `width: fit-content` is content-sized, not
stretched; the engine stretches it to its column.

```html
<div id="o" style="display:grid;width:600px"><div id="i" style="width:fit-content">
  <div id="b" style="width:70px;height:30px"></div></div></div>
```

`#i`: engine 600 wide, Chromium 70.

Where: `apply_justify_self` in `crates/rustkit-layout/src/grid.rs` maps
`Length::Auto | Length::FitContent` to the cell width.

### 12. `grid-max-content-width-zero-with-column-span` (5)

**Rule:** a grid's max-content width counts its fixed columns whatever its
items span; the engine's estimate is 0 when an item spans columns, so a
shrink-to-fit grid (in a float, an inline-block, an absolute box, or as a
row flex item) with a spanning item is 0 wide. On develop this coincides
with cause 5 (the widest child of these empty items is 0), but #663's grid
arm does not cover it.

```html
<div id="o" style="float:left"><div id="i" style="display:grid;grid-template-columns:repeat(3,50px)">
  <div id="a" style="grid-column:span 2;height:20px"></div><div id="b" style="height:20px"></div></div></div>
```

`#o` and `#i`: engine 0 wide on develop and on #663; Chromium 150. Without
`grid-column: span 2`, #663 gives 150 and matches (develop 0).

Where: the grid content-width estimate in `crates/rustkit-layout/src/grid.rs`
(`estimate_max_content_width`); PR #663 says its new arm "keeps the old
answer" for a grid with an item placed by line or spanning. Four of these
five (the `auto-rows-40` ones) also fail for cause 14 behind it.

### 13. `aspect-ratio-width-not-derived-from-height` (4)

**Rule:** a box with `aspect-ratio` and a definite height (fixed, or
stretched as a flex item) and an auto width takes its width from the height;
the engine only ever derives the height from the width.

```html
<div id="o" style="width:600px"><div id="i" style="height:60px;aspect-ratio:2/1"></div></div>
```

`#i`: engine 600 x 60; Chromium 120 x 60. As a row flex item with a
stretched height of 300 and no width: engine 0 wide, Chromium 600.

Where: the `Length::Auto` arm of `calculate_block_width`
(`crates/rustkit-layout/src/lib.rs`) does not read `aspect_ratio`;
`aspect_ratio_content_height` is only used for the height. The flex item
case is decided in flex.rs.

### 14. `grid-auto-rows-skips-first-row` (3 as first cause; all 8 `auto-rows-40` cases fail for it)

**Rule:** `grid-auto-rows` sizes every implicit row; the engine leaves the
first one content-sized.

```html
<div id="i" style="display:grid;width:100px;grid-template-columns:50px 50px;grid-auto-rows:40px">
  <div id="a"></div><div id="b"></div><div id="c"></div></div>
```

`#a` and `#b`: engine 0 tall; Chromium 40. `#c`: engine y 0, 40 tall
(over the first row); Chromium y 40. `#i`: engine 40 tall, Chromium 80.
This is the `span-by-row-end-in-a-row` gap #663 lists. Every
`grid-span--*--auto-rows-40` case fails for it; in four of them cause 12
comes first and in one cause 2.

Where: in `crates/rustkit-layout/src/grid.rs`; `GridLayout::new` takes the
auto row size as `_auto_rows` and does not use it, and implicit rows are
added in `ensure_tracks`. Which path makes the first row is not located.

### 15. `aspect-ratio-height-of-column-flex-item` (2)

**Rule:** a column flex item with `aspect-ratio` and a definite width takes
its height from the ratio; the engine makes it 0.

```html
<div id="o" style="display:flex;flex-direction:column;width:600px">
  <div id="i" style="width:100px;aspect-ratio:2/1"></div></div>
```

`#i`: engine 100 x 0; Chromium 100 x 50. The same box in a block or a row
flex container matches.

Where: in `crates/rustkit-layout/src/flex.rs` (the main size of a column
item); not located.

### 16. `grid-item-auto-margins-ignored` (2, and 2 more behind cause 2)

**Rule:** `margin: auto` on a grid item centres it in its area; the engine
leaves it at the start.

```html
<div id="i" style="display:grid;grid-template-columns:200px;grid-template-rows:100px">
  <div id="a" style="width:50px;height:20px;margin:auto"></div></div>
```

`#a`: engine 0, 0; Chromium 75, 40. On #663 the item moves to y 40 (its
block-axis fix, `7e9b2b34`) and stays at x 0.

Where: `place_item_in_area` in `crates/rustkit-layout/src/grid.rs` takes the
margins from `item_margins`, where `auto` is 0.

### 17. `aspect-ratio-only-empty-block-dropped` (1)

**Rule:** an empty block whose only size is `aspect-ratio` has a box; the
engine drops it from the tree.

```html
<div id="o" style="width:600px"><div id="i" style="aspect-ratio:2/1"></div></div>
```

`#i`: engine has no box (and `#o` is 0 tall); Chromium 600 x 300.

Where: the box tree build in `crates/rustkit-engine/src/lib.rs` keeps an
empty child only if `has_visible_styling` or `empty_block_affects_layout`
says so, and neither reads `aspect_ratio`. A box with a background is kept;
this is real for spacers and placeholders sized only by `aspect-ratio`.

### 18. `percent-height-chain-in-stretched-grid-item` (1)

**Rule:** a grid item stretched to a definite row is definite, so a
percentage two levels below it resolves; the engine resolves the first
level and not the second.

```html
<div id="o" style="display:grid;width:600px;height:300px"><div id="m">
  <div id="i" style="height:50%"><div id="a" style="height:50%"></div></div></div></div>
```

`#i` is 150 in both. `#a`: engine 0, Chromium 75. Cause unknown.

### 19. `max-height-ignored-on-stretched-item` (1, and 1 more behind cause 2)

**Rule:** a stretched item is clamped by its `max-height`; the engine
stretches it past it.

```html
<div id="o" style="display:grid;width:600px;height:300px"><div id="i" style="max-height:20px">
  <div id="a" style="height:30px"></div></div></div>
```

`#i`: engine 300 tall; Chromium 20. In a row flex container a grid item
with `max-height: 20px` and a 30px child: engine 30, Chromium 20 (develop
and #663). Two code paths: the grid stretch in
`crates/rustkit-layout/src/grid.rs` (`stretches_to_its_row` /
`apply_align_self`) and the flex cross size in flex.rs. Not located more
closely.

### 20. `fit-content-width-of-column-flex-item-is-zero` (5, behind cause 3)

Found once cause 3 was fixed (PR #672): the 5 `core--flex-col--*--w-fit-content--h-120px`
cases, tagged 3 on develop, stop here.

**Rule:** a column flex item with `width: fit-content` is as wide as its
content (its cross size is not stretched); the engine makes it 0.

```html
<div id="o" style="display:flex;flex-direction:column;width:600px"><div id="i" style="width:fit-content">
  <div id="b" style="width:70px;height:30px"></div></div></div>
```

`#i`: engine 0 wide (with #672), Chromium 70.

Where: the cross size of a column flex item in
`crates/rustkit-layout/src/flex.rs`; not located.

## Where #663 stands against this matrix

39 cases go from failing to matching and none the other way: 26 tagged 2
and 13 tagged 5. If #663 merges first, those 39 lines come out of the ledger
in the same change; if this matrix merges first, #663 has to remove them
(the test names them).

## Runs

- `cargo test -p rustkit-engine --lib nested_layout_matrix`: passes on
  develop with the ledger (378 match, 231 listed).
- The same test on the #663 branch with this ledger fails, as it should,
  naming the 39 cases that match there.
- `cargo test -p rustkit-layout` on develop in the Linux container: 835 pass,
  3 fail, 13 ignored. The three (`a_long_first_run_keeps_its_last_line_open_for_the_next_sibling`,
  `bare_control_widths_match_chrome`,
  `justified_wrapped_lines_fill_the_container_except_the_last`) measure text
  and fail on this container's fonts; this change does not touch
  rustkit-layout.
- Tolerance: every passing case matches within 0.005 px; 1px is the
  tolerance the work package set. It could be tightened to 0.05 to catch a
  half-pixel regression.
