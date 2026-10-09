# Real-Site Board Time-Stable & Record/Replay Design Specification

**Document Date**: 2026-10-03  
**Package**: **Z2-M4** (PLAN-z.md / Directive Atlas #609 & #610)  
**Author**: `pollux` (Windows Seat)  
**Scope**: Time-stable multi-sample measurement ($t = 1, 3, 5, 10\text{ s}$), Playwright HAR record/replay with pinned clock, local RustKit replay server with safe test-only origin mapping, and beside-board diagnostic reporting under **Rule A3**.

---

## 1. Motivation & Background

Live real-site evaluations currently measure a single headless first frame captured after an arbitrary settlement delay (typically $5\text{ s}$ on Chrome, $30\text{ s}$ wall budget on RustKit).

Pete's empirical testing on live web pages identified three critical instrumentation vulnerabilities inherent to single-shot live captures:
1. **Dynamic Temporal Mutation**: Live pages change moment-to-moment due to ad bidding, carousels, network latency fluctuations, and periodic polling. A single snapshot diff against Chrome conflates real layout rendering regressions with mere temporal drift.
2. **Late Content Pop-in**: Content that loads or hydrates after the initial paint (e.g. client-side hydration, delayed subresources, or async image decodes) is invisible to an early snapshot, or conversely misclassified if one engine captures before hydration while the other captures after.
3. **Network & Bot Detection Volatility**: WAFs and edge CDNs (Akamai, Cloudflare, DataDome, AWS WAF) trigger intermittent blocks and rate-limits that corrupt longitudinal tracking.

As directed by **Atlas #609** and refined in **Atlas #610**, Package **Z2-M4** addresses these challenges by establishing:
- A **Time-Stable Board** sampling visual states at multi-point time horizons ($t = 1, 3, 5, 10\text{ s}$) in both engines.
- A **Deterministic Record/Replay Pipeline** capturing frozen network bytes (HAR) and running against a frozen virtual clock.

---

## 2. Rule A3 Compliance & Placement

Under **Rule A3**:
- **Baselines, thresholds, and scoring definitions do not move.**
- The existing 3-point scoring scale ($20 \text{ sites} \times 3 \text{ points} = 60 \text{ points max}$) remains the sole gating scoreboard.
- The time-stable board publishes **beside the legacy board** as a separate diagnostic artifact (`trench/realsite/timestable/`), exactly as Scorer v2 (`scorer_v2.py`) and the interactive runner (`interactive_board.py`) publish beside the legacy board.
- No existing gate, threshold, or baseline file is modified.

---

## 3. Multi-Point Time Horizons ($t = 1, 3, 5, 10\text{ s}$)

Rather than comparing a single arbitrary instant, the time-stable runner captures and compares visual frames across 4 standardized temporal milestones:

| Milestone | Target State | Diagnostic Purpose |
|---|---|---|
| **$t = 1\text{ s}$** | First Contentful Paint / Shell | Evaluates initial paint speed, inline stylesheet application, and early DOM layout before heavy script execution. |
| **$t = 3\text{ s}$** | Critical Subresources Loaded | Evaluates web font loading, hero images, and primary layout construction. |
| **$t = 5\text{ s}$** | Initial Settlement (Legacy Parity) | Direct equivalent of the legacy board settle point; evaluates client script hydration and DOM readiness. |
| **$t = 10\text{ s}$** | Steady-State & Late Content | Evaluates delayed timers, async content pop-in, carousel stabilization, and detects runaway rAF/timer layout thrashing. |

### Temporal Stability Deltas:
For each engine $E \in \{\text{Chrome}, \text{RustKit}\}$, the harness measures internal temporal motion:
$$\Delta_{\text{motion}}(t_a, t_b) = \text{diff}(frame_E(t_a), frame_E(t_b))$$
A site is considered **temporally stable** if $\Delta_{\text{motion}}(5\text{s}, 10\text{s}) \le 1.0\%$.

---

## 4. Deterministic Record/Replay Architecture

To eliminate network drift, CDN rate limits, and clock jitter, both engines execute against a **single frozen recording of network bytes and virtual time**.

```
                           [ Live Web Target ]
                                    │
                       (One-time recordHar pass)
                                    ▼
                     [ Pinned HAR Archive File ]
                     (hiwave-renders-private/har)
                                    │
           ┌────────────────────────┴────────────────────────┐
           ▼                                                 ▼
[ Chrome (Playwright) ]                           [ Local HAR Replay Server ]
- routeFromHAR(archive)                           - 127.0.0.1:<port>
- page.clock.install(pinned_epoch)                - Serves exact byte entries
- clock.fastForward(1s, 3s, 5s, 10s)                         │
                                                             ▼
                                                  [ RustKit Engine ]
                                                  - --replay-proxy test flag
                                                  - RequestInterceptor
                                                  - Virtual timer ticks
```

### 4.1. Chrome Pinned Replay (Playwright)
Playwright provides native primitives for deterministic replay:
1. **Byte Freezing**:
   ```javascript
   const context = await browser.newContext({
     recordHar: { path: harPath, mode: 'minimal' }
   });
   // Replay:
   await page.routeFromHAR(harPath, { notFound: 'abort' });
   ```
2. **Clock Freezing**:
   ```javascript
   // Pinned fixed epoch (e.g. 2024-10-01T00:00:00Z)
   await page.clock.install({ time: 1727740800000 });
   await page.goto(url, { waitUntil: 'load' });

   // Advance clock deterministically to each target milestone:
   await page.clock.fastForward(1000);
   await page.screenshot({ path: 'chrome_1s.png' });
   await page.clock.fastForward(2000); // reaches 3s
   await page.screenshot({ path: 'chrome_3s.png' });
   await page.clock.fastForward(2000); // reaches 5s
   await page.screenshot({ path: 'chrome_5s.png' });
   await page.clock.fastForward(5000); // reaches 10s
   await page.screenshot({ path: 'chrome_10s.png' });
   ```

### 4.2. RustKit Local HAR Replay & Safe HTTPS Mapping
RustKit must consume the exact same HAR archive without altering the production browser's security architecture.

#### Local Replay Server:
A lightweight local server (`tools/parity_oracle/har_server.py` or Node service) loads the `.har` file into memory, indexing entries by `(method, url)`. It listens exclusively on `http://127.0.0.1:<random-port>`.

#### Safe HTTPS Mapping (Security & Policy Invariants):
*Directive Atlas #610 requirement*: **"say in the design how https hosts are mapped without weakening any policy check (test-only flag, off by default, never in the shipped app)."**

1. **Test-Only Surface**:
   - The CLI flag `--replay-proxy <http://127.0.0.1:port>` is added **strictly to `crates/parity-capture`**.
   - It is **never** added to the shipping browser binary (`crates/hiwave-app`).
   - The shipping application contains zero code referencing or allowing `--replay-proxy`.
2. **Policy Integrity**:
   - The document URL remains the original canonical HTTPS URL (e.g. `https://en.wikipedia.org/wiki/Web_browser`).
   - All document security origins, cookies, same-origin policies, and DOM `window.location` properties evaluate against the genuine `https://...` origin.
   - Outbound network requests pass through `ResourceLoader`:
     - Rewrites the socket destination to `http://127.0.0.1:<port>` keyed on loader-internal `is_replay_proxied` flag.
     - Preserves the original `Host: <original_domain>` header.
     - Adds `X-Original-URL: <full_https_url>`.
   - The engine's security sandbox, CSP evaluator, and FetchPolicy checks continue to run unmodified on the original origin.

### 4.3. Storage Separation
- All recorded HAR archives are stored in `hiwave-renders-private/har/<site>.har`.
- HAR archives contain third-party proprietary responses and cookies/headers; they **must not** be checked into the public `hiwave-macos` repository.

---

## 5. Diagnostic Metrics & Output Taxonomy

The time-stable harness produces a comprehensive JSON report and tabular summary:

| Field | Description |
|---|---|
| `site_id` | Pinned identifier from `websuite/realsite-top20.json`. |
| `diff_1s` | Pixel diff percentage between Chrome and RustKit at $t=1\text{ s}$. |
| `diff_3s` | Pixel diff percentage between Chrome and RustKit at $t=3\text{ s}$. |
| `diff_5s` | Pixel diff percentage between Chrome and RustKit at $t=5\text{ s}$ (legacy settle comparison). |
| `diff_10s` | Pixel diff percentage between Chrome and RustKit at $t=10\text{ s}$. |
| `chrome_stability` | $\Delta_{\text{motion}}(5\text{s}, 10\text{s})$ on Chrome (measures site's inherent dynamism). |
| `rustkit_stability` | $\Delta_{\text{motion}}(5\text{s}, 10\text{s})$ on RustKit. |
| `trajectory` | Classification: `CONVERGING` (diff decreases over time), `STABLE` (constant low diff), `DIVERGING` (diff increases over time), or `DYNAMIC` (engine or oracle fluctuates due to animations/carousels). |

---

## 6. Implementation Roadmap

1. **Step 1 (Har Replay Server)**: Implement `tools/parity_oracle/har_server.py` serving parsed HAR archives with method/URL matching and fallback handling.
2. **Step 2 (Playwright Replay & Pinned Clock)**: Add `timestable` action to `tools/parity_oracle/realsite.mjs` incorporating `routeFromHAR`, `recordHar`, and `page.clock`.
3. **Step 3 (RustKit Test-Only Proxy)**: Add `--replay-proxy` to `crates/parity-capture/src/main.rs` wiring `rustkit_net::RequestInterceptor`.
4. **Step 4 (Test Harness & Guards)**: Implement `scripts/timestable_board.py` and guard tests in `scripts/tests/test_timestable_board.py`.
