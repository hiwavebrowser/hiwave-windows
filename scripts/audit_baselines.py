#!/usr/bin/env python3
"""
audit_baselines.py - Instrument-integrity audit (R0, VIEWPORT_RESOLUTION_PLAN P0.3)

Asserts, for every case in cases/registry.json:
  1. The fixture HTML exists.
  2. The baseline PNG exists under the active baseline set.
  3. The baseline PNG dimensions equal registry (width, height) * dpr exactly.
And for the baseline set itself:
  4. Its metadata.json browserVersion matches the registry pin.

A size mismatch here is the disease behind measurement lie #8: comparePixels
used to soft-crop mismatched frames and emit a plausible-looking diff%. The
compare now hard-fails at runtime; this audit catches drift at PR time,
before a wrong-dimension baseline ever meets a capture.

Exit 0 = clean, exit 1 = drift (CI fails the PR).
"""

import json
import os
import struct
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).parent.parent
REGISTRY_PATH = REPO_ROOT / "cases" / "registry.json"


def png_size(path: Path):
    with open(path, "rb") as f:
        header = f.read(24)
    if header[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    return struct.unpack(">II", header[16:24])


def main() -> int:
    registry = json.loads(REGISTRY_PATH.read_text(encoding="utf-8"))
    pin = registry["pin"]
    baseline_set = os.environ.get("PARITY_BASELINE_SET", pin["baseline_set"])
    baselines_dir = REPO_ROOT / "baselines" / baseline_set
    dpr = pin.get("dpr", 1)

    problems = []

    set_meta_path = baselines_dir / "metadata.json"
    if not set_meta_path.exists():
        problems.append(f"missing {set_meta_path.relative_to(REPO_ROOT)}")
    else:
        set_meta = json.loads(set_meta_path.read_text(encoding="utf-8"))
        got = set_meta.get("browserVersion") or set_meta.get("chrome_version")
        want = pin["chrome_version"]
        if got != want:
            problems.append(
                f"baseline set pin drift: {baseline_set}/metadata.json says {got}, registry pins {want}"
            )

    for case_id, case in registry["cases"].items():
        html = REPO_ROOT / case["html"]
        if not html.exists():
            problems.append(f"{case_id}: fixture missing: {case['html']}")

        baseline = baselines_dir / case["scope"] / case_id / "baseline.png"
        if not baseline.exists():
            problems.append(f"{case_id}: baseline missing: {baseline.relative_to(REPO_ROOT)}")
            continue

        size = png_size(baseline)
        if size is None:
            problems.append(f"{case_id}: baseline is not a PNG")
            continue

        expected = (case["width"] * dpr, case["height"] * dpr)
        if size != expected:
            problems.append(
                f"{case_id}: baseline {size[0]}x{size[1]} != registry {expected[0]}x{expected[1]}"
                f" (w{case['width']} h{case['height']} dpr{dpr})"
            )

    if problems:
        print(f"BASELINE AUDIT: {len(problems)} problem(s) [{baseline_set}]")
        for p in problems:
            print(f"  ✗ {p}")
        return 1

    print(
        f"BASELINE AUDIT: clean — {len(registry['cases'])} cases @ {baseline_set} "
        f"(pin {pin['chrome_version']}, dpr {dpr})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
