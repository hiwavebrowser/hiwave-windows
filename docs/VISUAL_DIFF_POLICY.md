# Visual Diff Policy

This document defines the pixel-diff policy for RustKit visual regression testing.

## Overview

Visual regression testing compares RustKit renders against Chromium baselines to ensure rendering correctness. The diff policy defines:

1. **What constitutes a "pass"**
2. **Tolerance thresholds for antialiasing (AA) variance**
3. **Size mismatch handling**
4. **Per-suite configuration**

## Policy Configuration

### Default Policy

```json
{
  "aa_tolerance": 5,
  "strict_size": true,
  "max_diff_percent": 0.0,
  "fail_on_size_mismatch": true
}
```

### Field Definitions

| Field | Type | Description |
|-------|------|-------------|
| `aa_tolerance` | `integer` | Max RGB delta (0-255) for AA-related pixel variance. Pixels within this tolerance are marked as "tolerated" but not counted as true diffs. |
| `strict_size` | `boolean` | If `true`, dimension mismatches fail immediately. |
| `max_diff_percent` | `float` | Max percentage of pixels allowed to differ (after AA tolerance). `0.0` = strict. |
| `fail_on_size_mismatch` | `boolean` | If `true`, fail when RustKit and baseline dimensions differ. |

## Suite-Specific Policies

### Built-in Pages (`builtins-*`)

**Policy:** Strict (0-diff)

```json
{
  "aa_tolerance": 5,
  "strict_size": true,
  "max_diff_percent": 0.0
}
```

**Rationale:** Built-in pages are fully controlled and should render identically.

### Fixtures (`fixtures/`)

**Policy:** Strict (0-diff)

```json
{
  "aa_tolerance": 5,
  "strict_size": true,
  "max_diff_percent": 0.0
}
```

**Rationale:** Fixtures are deterministic test cases designed for regression testing.

### WebSuite (`websuite/cases/`)

**Policy:** Strict with documented exceptions

```json
{
  "aa_tolerance": 5,
  "strict_size": true,
  "max_diff_percent": 0.0
}
```

**Rationale:** WebSuite emulates real websites but remains deterministic.

### Live Sites (non-gating)

**Policy:** Tolerant (non-gating)

```json
{
  "aa_tolerance": 10,
  "strict_size": false,
  "max_diff_percent": 5.0
}
```

**Rationale:** Live sites are nondeterministic (ads, dynamic content). Used for trend tracking, not gating.

## Diff Image Legend

| Color | Meaning |
|-------|---------|
| **Faded (alpha=64)** | Exact match |
| **Yellow (alpha=128)** | Within AA tolerance |
| **Red (alpha=255)** | True difference |

## Canary Report Schema

Each canary run produces a summary JSON with the following schema:

```json
{
  "timestamp": "ISO8601",
  "git_sha": "string",
  "renderer": "rustkit | chromium",
  "dpr": 2.0,
  "policy": {
    "aa_tolerance": 5,
    "strict_size": true,
    "max_diff_percent": 0.0
  },
  "total": 5,
  "passed": 5,
  "failed": 0,
  "captures": [
    {
      "page_id": "string",
      "source_file": "string",
      "viewport": { "width": 1280, "height": 800 },
      "frame": "filename.ppm | null",
      "status": "ok | fail | skip",
      "perf": {
        "layout_ms": 1.5,
        "paint_ms": 2.0,
        "render_ms": 30.0,
        "capture_ms": 8.0
      }
    }
  ]
}
```

## Diff Report Schema

Each diff run produces a summary JSON with the following schema:

```json
{
  "timestamp": "ISO8601",
  "policy": { ... },
  "total": 5,
  "passed": 4,
  "failed": 1,
  "pages": [
    {
      "page_id": "string",
      "status": "pass | diff | skip | error",
      "exact_diff_pixels": 0,
      "tolerated_diff_pixels": 150,
      "total_pixels": 1024000,
      "diff_percent": 0.0,
      "tolerated_percent": 0.0146,
      "size_mismatch": false,
      "diff_image": "page.diff.png",
      "reason": "optional string explaining failure"
    }
  ]
}
```

## Workflow

1. **Capture:** Run `scripts/builtins_capture.sh` or `scripts/websuite_capture.sh`
2. **Baseline:** Run Playwright capture if needed
3. **Diff:** Run comparison script
4. **Report:** Check summary.json for pass/fail
5. **Triage:** Inspect diff images for failures

## Exception Handling

When a test case legitimately differs from Chromium (e.g., intentional style differences):

1. Document the exception in a `*.exception.json` file
2. The exception file overrides the default policy for that case
3. Exceptions must include a rationale and expiry date

Example exception file:

```json
{
  "page_id": "settings",
  "reason": "Custom scrollbar styling differs from Chromium",
  "expires": "2026-06-01",
  "override_policy": {
    "max_diff_percent": 0.5
  }
}
```

