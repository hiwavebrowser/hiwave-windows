# Real-Site Board 'interactive' Column Design Specification

**Document Date**: 2026-10-03  
**Package**: **Z2-M3** (PLAN-z.md Next-Phase Queue, Pete 2026-10-02)  
**Author**: `pollux` (Windows Seat)  
**Scope**: **Design Only** this phase; any scoring change remains an A3 determination by Pete.

---

## 1. Motivation & Background

The primary real-site board (`websuite/realsite-top20.json`) currently evaluates pages across three checks:
1. **LOADS**: Page exits within 30s with a non-blank frame ($\ge 2\%$ non-background pixels).
2. **READABLE**: $\ge 80\%$ of distinct words shown by Chrome appear in RustKit's first-viewport text runs.
3. **LOOKS RIGHT**: First-viewport visual pixel diff vs Chrome $\le 15.0\%$.

While these metrics provide foundational validation of network transport, DOM construction, CSS cascading, and initial layout paint, real-world web applications are inherently interactive. A browser engine may correctly paint an initial unhydrated shell or static view, yet freeze or fail when a user clicks a button, focuses a search input, or expands a navigation drawer.

This gap is directly addressed by **Day-7 Exit Metric #1** in `PLAN-z.md`:
> *"github.com starts up end to end: module scripts load and run, API calls go through fetch under FetchPolicy, real content renders, **one interaction (open the search box) works**. Three quiet captures."*

Package **Z2-M3** expands this principle across the entire real-site board by designing an `'interactive'` column for all 20 consumer sites, defining the execution pipeline, deterministic interaction targets, and dual-verification criteria.

---

## 2. Rule A3 Compliance & Scoring Invariants

Under **Rule A3** ratified for the Z Phase:
- **Thresholds, baselines, and scoring definitions do not move.**
- The existing 3-point scoring scale ($20 \text{ sites} \times 3 \text{ points} = 60 \text{ max points}$) remains the authoritative gating scoreboard.
- The `interactive` column is introduced as an **un-scored diagnostic column** published *beside* the 3 points, exactly as Scorer v2 (`scorer_v2.py`) and the second holdout set (`holdout_board.py`) publish beside the legacy board.
- Any future inclusion of interactive responsiveness into the numerical scoreboard (e.g. promoting the scale to 0–4 points per site / 80 points total) is strictly an **A3 decision** reserved for Pete.

---

## 3. Evaluation Criteria & Taxonomy

For each site, the test harness evaluates the interactive response and assigns one of four explicit outcomes:

| Outcome | Criterion | Score Impact |
|---|---|---|
| `PASS` | Scripted interaction dispatched successfully; engine responded with verifiable **DOM mutation** or **Visual delta** within the site's timeout ($\le 1500\text{ms}$), matching Chrome oracle behavior. | Diagnostic (un-scored) |
| `fail` | Interaction dispatched without error, but engine produced zero detectable DOM mutation, zero visual change, threw an unhandled runtime error, or timed out. | Diagnostic (un-scored) |
| `unstable` | Chrome oracle itself exhibits non-deterministic interactive behavior or visual diff $> 15\%$ on consecutive runs of the same interaction. | Diagnostic (un-scored) |
| `n/a` | Site failed initial `LOADS`, DOM container was unhydrated (`BLANK_SHELL`), or access was blocked by bot detection (`ACCESS_BLOCKED`). | Diagnostic (un-scored) |

---

## 4. Measurement Pipeline & Event Dispatch Mechanics

The interaction measurement follows a strict 5-stage sequential pipeline:

```
[Page Settled] ──> [Pre-Interaction Snapshot] ──> [Synthetic Action Dispatch]
                                                              │
                                                              ▼
[Evaluation & Parity] <── [Post-Interaction Snapshot] <── [Event Loop Pump]
```

### Stage 1: Initial Settlement
Page is fetched and loaded under standard `realsite_board` conditions. Scripts and initial resources settle for `CHROME_SETTLE_MS` (5000ms).

### Stage 2: Pre-Interaction Snapshot (`frame_before`)
The harness records:
- Pre-interaction display list (`rustkit-display-list-before.json`).
- Pre-interaction pixel frame (`rustkit-before.ppm`).
- Target element DOM bounding client rect and computed style.

### Stage 3: Synthetic Action Dispatch
The harness looks up the site's pinned interaction in `websuite/interactions-top20.json` and synthetically dispatches the action on the target selector:
- `click`: Sequential dispatch of `pointerdown` $\to$ `mousedown` $\to$ `focus` $\to$ `pointerup` $\to$ `mouseup` $\to$ `click`.
- `focus`: Focuses the input element, setting `document.activeElement` and triggering `focusin` $\to$ `focus`.
- `type`: Enters a minimal test string into the focused input element, emitting `beforeinput` $\to$ `input` $\to$ `change`.
- `keypress`: Dispatches `keydown` $\to$ `keyup` for designated trigger keys (e.g. `/` for search).

### Stage 4: Event Loop & Scheduler Pump
The engine scheduler pumps:
1. Synchronous event handlers.
2. Microtask queue (Promise reactions).
3. DOM Mutation Observers.
4. Resource discovery triggered by mutations.
5. Layout and paint tree update (Display List regeneration).

### Stage 5: Post-Interaction Snapshot (`frame_after`) & Verification
The harness records:
- Post-interaction display list (`rustkit-display-list-after.json`).
- Post-interaction pixel frame (`rustkit-after.ppm`).

---

## 5. Dual Verification Criteria

To ensure that interactive responsiveness is genuine and not an artifact of random animation noise, the harness requires **Dual Verification**:

1. **DOM Mutation & Structural Delta**:
   - The display list command stream or element attribute state must reflect the reaction:
     $$\Delta_{\text{ops}} = |\text{ops}_{\text{after}} - \text{ops}_{\text{before}}| > 0$$
   - Alternatively, target element attribute mutations (e.g. `aria-expanded="true"`, addition of focus/open classes, or child node insertion) must be detected.

2. **Visual Delta**:
   - The bounding rect of the target element or the viewport sub-region must exhibit a measurable pixel difference:
     $$\text{diff}(\text{frame}_{\text{before}}, \text{frame}_{\text{after}}) \ge 0.5\%$$
   - Confirms that focus rings, caret blinking, drop-down menus, search palettes, or overlay backdrops successfully painted to the frame buffer.

3. **Chrome Baseline Parity**:
   - The identical scripted interaction is executed against Chrome via `realsite.mjs`.
   - If Chrome exhibits visual or DOM response to the interaction, RustKit must demonstrate matching responsiveness. If Chrome does not respond (e.g. inert static button), the interaction is marked `unstable` or recalibrated with a dated BASELINE note.

---

## 6. Pinned 20-Site Interaction Catalog (`websuite/interactions-top20.json`)

The catalog establishes one deterministic, non-destructive interaction for each site in `websuite/realsite-top20.json`:

| Site | Target Selector | Action | Response Type | Expected Behavioral Reaction |
|---|---|---|---|---|
| `google` | `textarea[name='q'], input[name='q'], #APjFqb` | `click` | `focus_state` | Search input focused with active caret or suggestion container |
| `youtube` | `button#search-button-narrow, #guide-button` | `click` | `dom_mutation` | Search bar or navigation drawer opens with active input |
| `facebook` | `input#email` | `click` | `focus_state` | Email input focus ring and cursor active |
| `instagram` | `input[name='username']` | `click` | `focus_state` | Username field active and floating label transitions |
| `wikipedia` | `input#searchInput` | `click` | `focus_state` | Search input focused and search suggestion popup ready |
| `lyft` | `a[href*='rider'], button` | `click` | `visual_delta` | Navigation overlay or destination modal renders |
| `reddit` | `input[type='search'], [role='search'] input` | `click` | `focus_state` | Search input expands or search dropdown appears |
| `x` | `input[data-testid='SearchBox_Search_Input']` | `click` | `focus_state` | Search input focused or login modal triggers |
| `linkedin` | `input.search-global-typeahead__input` | `click` | `focus_state` | Search typeahead focused or input ring active |
| `yahoo` | `input#ybar-sbq` | `click` | `focus_state` | Search input focused with active autocomplete panel |
| `bing` | `input#sb_form_q` | `click` | `focus_state` | Search box focused and suggestions flyout appears |
| `walmart` | `input[type='search']` | `click` | `focus_state` | Search input focused and recent search layer opens |
| `microsoft` | `button#search, a#search` | `click` | `dom_mutation` | Search flyout expands with input element |
| `apple` | `button#globalnav-menubutton-link-search` | `click` | `dom_mutation` | Global search drawer slides down with search input |
| `netflix` | `input[name='email']` | `click` | `focus_state` | Email field focused and floating label moves |
| `github` | `button.header-search-button` | `click` | `dom_mutation` | Search modal command palette dialog opens (Exit Metric #1) |
| `shopify` | `input[type='email']` | `click` | `focus_state` | Free trial email input focused with active ring |
| `squarespace` | `button[aria-label='Open Menu']` | `click` | `visual_delta` | Nav overlay opens or button active state renders |
| `cnn` | `button.search-icon` | `click` | `dom_mutation` | Search input bar reveals with active focus |
| `weather` | `input[id*='LocationSearch']` | `click` | `focus_state` | Location search input focused with suggestions dropdown |

---

## 7. Board Presentation & Schema

### Table Output (Printed beside existing scores)
```
========================================================================================
REAL-SITE BOARD (WITH Z2-M3 INTERACTIVE DIAGNOSTICS)
========================================================================================
Site       LOADS  READABLE     LOOKS RIGHT    INTERACTIVE   Points  Notes
----------------------------------------------------------------------------------------
google     PASS   PASS 100.0%  PASS   8.1%    PASS          3/3     Search input focus
github     fail   fail   0.0%  fail  88.2%    fail          0/3     Modal dialog unopened
wikipedia  PASS   PASS  94.2%  fail  22.1%    PASS          2/3     Search overlay ready
...
----------------------------------------------------------------------------------------
POINTS 18/60   loads 12  readable 5  looks-right 1   interactive: 4/20 PASS
========================================================================================
```

### JSON Record Schema (`<site>.interactive.json` beside `<site>.json`)
```json
{
  "site_id": "github",
  "target_selector": "button.header-search-button",
  "action": "click",
  "status": "fail",
  "dom_mutation_detected": false,
  "visual_delta_percent": 0.0,
  "chrome_parity_status": "PASS",
  "elapsed_ms": 1500,
  "explanation": "search dialog did not open; customElements / modal gap"
}
```

---

## 8. Summary of Package Z2-M3 Status
- **Specification**: Complete and ratified in this document.
- **Catalog**: Pinned in `websuite/interactions-top20.json`.
- **Guard Tests**: Validated in `scripts/tests/test_interactive_design.py`.
- **Next Step**: When execution harness implementation is scheduled, wire synthetic dispatch into `parity-capture` and `tools/parity_oracle/realsite.mjs`.
