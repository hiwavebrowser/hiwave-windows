# RustKit parity ledger — Windows

**Measured 2026-08-05 on `develop`.** The mandate is 100% RustKit and
independent. This is the map everything else is measured against; without it
"how far are we" stays an opinion.

## The finding that reshapes the problem

RustKit **already renders the web on Windows.** Verified by run, not by reading:

```
cargo run --no-default-features --features native-win32 -- \
    --render-test --render-test-url https://example.com

rustkit_engine: Loading URL https://example.com/
GPU readback content 1060x800  -> image inspected: correct render
```

`rustkit-net` fetched over the network, the engine parsed / styled / laid out,
the compositor GPU-painted it. `wry` appears **once** in `native/win32.rs`.
The about page renders at 71,720 colour vertices.

**So the gap is not the engine. It is the browser around it.** There are two
shells, and the one that is the default is not the one using our engine.

| | lines | capability |
|---|---|---|
| hybrid `main.rs` (default) | 3,725 | 92 `UserEvent` variants — WebView2 paints |
| native `win32.rs` (opt-in) | 781 | 12 public methods — **RustKit paints** |

## Parity: 9 of 92 (9%)

Hand-mapped from the native shell's twelve public methods. **Not** substring
matching — a first pass using name matching claimed 50/92, including
`ActivateTabByIndex` on a shell with no tab handling at all. That number was
vacuous and is recorded here so nobody resurrects it.

| Hybrid event | Native implementation | Note |
|---|---|---|
| `ExpandChrome` | `expand_chrome` | |
| `CollapseChrome` | `collapse_chrome` | |
| `ExpandChromeSmall` | `expand_chrome` | **partial** — one size only |
| `ExpandShelf` | `expand_shelf` | |
| `CollapseShelf` | `collapse_shelf` | |
| `Navigate` | `navigate` | |
| `EvaluateScript` | `execute_script` | |
| `EvaluateContentScript` | `execute_script` | **partial** — no view targeting |
| `SetRightPanelOpen` | `toggle_sidebar` | **partial** — toggles, cannot set |

Three of the nine are partial. Counting generously that is 9; counting strictly
it is 6 complete.

## The 83 missing, by subsystem — this is the port order

| Subsystem | Count | Why this position |
|---|---|---|
| **navigation** | 5 | `GoBack`/`GoForward`/`Reload`/`Stop`/`RecordVisit`. A browser that cannot go back is not usable. Smallest set, highest value. |
| **tabs / session** | 6 | `NewTab`, `CloseActiveTab`, `ActivateTabByIndex`, Cellar restore. The second thing anyone touches. |
| find / zoom | 7 | user-visible, self-contained |
| vault / credentials | 8 | security surface — port carefully, not quickly |
| focus mode | 8 | product differentiator |
| import / export | 9 | |
| settings | 12 | large but mechanical |
| analytics | 3 | |
| other | 25 | `ClearBrowsingData`, `PrintPage`, `OpenCommandPalette`, `NavigationStateChanged`, … |

## Amendment — 2026-08-05, the rented five, measured then built

The first version of this ledger said the 5 rented capabilities "must be
built." That was wrong twice over, in opposite directions, and this section
replaces it:

| Capability | First claim | Measured truth | Status |
|---|---|---|---|
| back / forward | must be built | `SessionHistory` existed all along (43 fns, **test-only orphan**) beside NSM's live `Vec<Url>` — TWO STACKS | **OURS** — SessionHistory canonical per the 2026-08-05 pin; Vec deleted as owner; `Engine::go_back/go_forward` load via replace-disposition so traversal never truncates the forward stack |
| reload | must be built | same wire gap | **OURS** — `Engine::reload`, replace-disposition |
| stop | "cancel exists" (WRONG — that was DownloadManager, file downloads) | page loads had NO cancellation | **OURS** — generation-gated (`Engine::stop`); stop-as-observed, socket abort is a separate loader unit |
| print | must be built | absent from every crate, positive-controlled | **still a PROJECT** — paginating layout to a page box; not before the rest |

The native shell's three IPC TODOs (`go_back`/`go_forward`/`reload`) are live
handlers now. The hybrid shell still rents all five from Chromium and keeps
doing so until the default flips — Pete's call, not a builder's.

Grep lesson recorded so the next reader doesn't repeat it: "no history in the
engine" came from searching ONE FILE. `SessionHistory` was one crate over. An
absence claim needs a workspace-wide search AND a positive control.

## Ladder

- [x] **(a) label honesty** — `#78`. The tree no longer claims RustKit paints
      content when WebView2 does. Zero behaviour change.
- [ ] **(b) dead module** — `webview_rustkit.rs` is labelled, not deleted. It is
      the adapter step (c) needs; deleting it means rewriting it. Divergence
      from the published ladder, stated deliberately.
- [ ] **(c) port navigation + tabs into the native shell** — 11 events, the
      minimum for a usable browser.
- [ ] **(d) the remaining 72**, subsystem by subsystem, each its own PR with the
      falsification harness applied.
- [ ] **(e) flip the default** — PETE'S DECISION, not a builder's.
- [ ] **(f) delete `wry` / `tao` from `Cargo.toml`.** *Independence is only real
      when the dependency is GONE, not merely unused* — a dormant dependency is
      a dormant fallback.

## What is NOT claimed

`example.com` is a trivial page. **Untested:** JS-heavy sites, forms, media,
complex CSS, anything at scale. "RustKit renders the web" is proven for one
simple live page and our own about page — that is the whole evidence base.

The parity board would answer the rest and is deliberately not quoted: the old
harness can score a collapsed 141px column `3.71 GOOD` while a correct tree
scores `33.87 FAIL` (Argos's shelf-Goodhart finding). A lying instrument is
worse than no instrument.
