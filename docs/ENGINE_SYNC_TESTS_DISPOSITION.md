# Engine sync — disposition of the 65 Windows-only inline tests

Companion to `ENGINE_SYNC_2026-09-24.md`. The Windows tree carried 65 test
functions that did not exist in the hiwave-macos crates. They could not be
copied (they drove Windows-era internals: `Engine::apply_declaration`,
`Engine {..}` struct literals, `parse_flex_basis`, `ElementCtx`,
`wrap_segment`, `ROOT_FONT_SIZE_PX`, `resolve_radius`; ~130 compile errors
verbatim). Each family was instead **rewritten against the macOS API and run
on the macOS engine code**. Every red was either a Windows fix the macOS
engine lacked (ported upstream, with the red test as the receipt) or a pin of
an internal that no longer exists (dropped, stated below).

All numbers measured on Windows, rustc 1.90, synced tree.

## Totals

| disposition | count |
|---|---:|
| ported, green on the macOS engine | 52 |
| already carried by hiwave-macos #234 (stop/history) | 4 |
| dropped with a stated reason | 8 |
| a helper miscounted as a test (`test_compositor`) | 1 |
| **total** | **65** |

Fixes the reds found (all upstream to hiwave-macos):

| red test | fix | PR |
|---|---|---|
| `the_single_number_shorthand_zeroes_the_basis`, `a_two_value_shorthand_distinguishes_shrink_from_basis` | `flex: <n>` sets shrink 1 / basis 0; a non-numeric 2nd value is the basis | #240 |
| `a_value_naming_no_line_keyword_leaves_the_line_alone` | colour-only `text-decoration` leaves the line alone | #240 |
| `an_elliptical_radius_takes_the_horizontal_half` | `border-radius: h / v` takes the horizontal radii instead of dropping the declaration | H |
| `radial_gradient_positions_parse_to_normalised_centres` | single-value `at 30%` keeps y at center (both axes were set) | H |

## rustkit-engine (38)

| family (Windows module) | tests | disposition |
|---|---:|---|
| stop_navigation_tests / history_traversal_tests | 4 | carried in **#234** with the navigation port |
| transform / animation / position / overflow-decoration / flex wiring | 10 | ported as `cascade_wire_tests` (**#240**): `Engine::new` behind the init mutex driving `apply_style_property`; 3 were red → fixes in #240 |
| border_radius_engine_path | 4 | ported in `windows_engine_pins` (H); elliptical form was red → fix in H |
| box_shadow_paint_tests | 4 | ported in `windows_engine_pins` (H) |
| display_list_reftests (negative control) | 1 | ported in `windows_engine_pins` (H) |
| descendant selector ×3, child_combinator malformed group | 4 | ported in `windows_engine_pins` (H) against `selector_matches(&self, ..) -> bool` + `selector_specificity`; the Windows `Option<specificity>` return no longer exists |
| ua_default_gap_tests (heading scale) | 1 | ported in `windows_engine_pins` (H) via `compute_style_for_element` |
| external_css_lifetime_tests | 1 | ported in `windows_engine_pins` (H) against `ViewState.external_stylesheets` |
| a_leg_engine_path_guards | 8 | ported as `windows_a_leg_pins` (H); `line-height: normal` pinned to be metrics-derived and multiplier-scaled rather than to the macOS-font number 18.4; radial single-value position was red → fix in H |
| `test_compositor` | 1 | a helper, not a test |

## rustkit-layout (16)

| Windows test | disposition |
|---|---|
| flex.rs: positions in absolute frame, subtree relaid, column width, container auto height, wrap packing, stretch equalises, explicit height not stretched, auto basis uses max-content | ported verbatim as `windows_flex_pins` (8) — `layout_flex_container` has the same signature |
| flex.rs: `test_auto_basis_uses_pre_pass_measurement` | **dropped**: pinned the old Windows flex model (pre-pass rect as auto basis); this tree measures max-content (#184, #202), pinned by the sibling test |
| lib.rs: `border_radius_emit_tests` ×3 | ported verbatim (H) |
| lib.rs: `wrap_text` ×4 (breaks on words, single line fits, long word overflows, nowrap/pre suppress) | **dropped**: `wrap_text` is a Windows-only API replaced by the inline line-box model; behaviours are pinned by `test_text_wraps_into_line_boxes`, `test_text_nowrap_stays_single_line` and the `line_break` tests |

## rustkit-dom (5)

Parser robustness pins (the shell's `chrome.html`, ~100 KB `<style>` blocks,
`<meta>` / charset / title-only heads): ported verbatim as
`windows_parser_pins` (H).

## rustkit-css (2)

`BoxShadow::is_visible` pins: ported verbatim as `windows_shadow_pins` (H);
the method exists on this tree.

## rustkit-renderer (3)

| Windows test | disposition |
|---|---|
| screenshot.rs `test_metadata_serialization` | ported as `windows_capture_metadata_pins` (cfg windows; needs `CaptureMetadata` from #236) — rides in the Windows sync PR |
| `test_gradient_interp_is_gamma_space`, `test_srgb_linear_roundtrip_dark` | **dropped**: they pin Windows-only helpers (`eval_gradient_stops`, `srgb_channel_to_linear`) written for sRGB render targets. This renderer targets `Rgba8Unorm` (linear) by design. Verified on Windows by a dark-colour smoke through the native shell: body `#1a1a2e` reads back (26,26,46), `#5a5a76` → (90,90,118), `#ff0000` exact — no double encoding |

## rustkit-text (1)

`macos.rs::test_create_backend` — a Core Text backend test that lived in a
Windows-tree copy of `macos.rs`; **dropped** (the macOS tree has its own).
