#!/usr/bin/env python3
"""Baseline dimension audit — the lie #8 guard.

Reads cases/registry.json (the single source of truth) and checks every
baseline PNG's dimensions against the case's declared width x height. Comparing
a rustkit capture against a wrong-sized baseline silently crops or scales and
produces a meaningless parity number; this catches it with no Chrome needed.

Ratcheting: cases whose registry `baseline_status` is already `dim_mismatch` or
`missing` are grandfathered (known-broken, awaiting baseline regen) and only
warned. A case declared `ok` that does NOT match — or a grandfathered case that
is now fixed and should be promoted — is a hard failure, so the ledger can only
improve.

Exit 0 if the audit holds, 1 otherwise.
"""
import json
import os
import sys

from PIL import Image

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REGISTRY = os.path.join(REPO, "cases", "registry.json")


def baseline_path(scope, case_id, baseline_set):
    return os.path.join(REPO, "baselines", baseline_set, scope, case_id, "baseline.png")


def main():
    with open(REGISTRY, encoding="utf-8") as f:
        reg = json.load(f)
    baseline_set = reg["pin"]["baseline_set"]

    hard_fail = 0
    grandfathered = 0
    ok = 0
    for cid, c in reg["cases"].items():
        want = (c["width"], c["height"])
        declared = c.get("baseline_status", "ok")
        path = baseline_path(c["scope"], cid, baseline_set)

        if not os.path.exists(path):
            if declared == "missing":
                print(f"  warn  {cid:22s} baseline MISSING (grandfathered)")
                grandfathered += 1
            else:
                print(f"  FAIL  {cid:22s} baseline MISSING but registry says '{declared}'")
                hard_fail += 1
            continue

        with Image.open(path) as im:
            got = im.size

        if got == want:
            if declared != "ok":
                # It's been regenerated correctly — must be promoted in the registry.
                print(f"  FAIL  {cid:22s} baseline now matches {want} — promote "
                      f"baseline_status to 'ok' in registry (ratchet)")
                hard_fail += 1
            else:
                ok += 1
        else:
            if declared == "dim_mismatch":
                print(f"  warn  {cid:22s} baseline {got} != declared {want} (grandfathered)")
                grandfathered += 1
            else:
                print(f"  FAIL  {cid:22s} baseline {got} != declared {want} "
                      f"(registry says '{declared}')")
                hard_fail += 1

    print()
    print(f"baseline audit: {ok} ok, {grandfathered} grandfathered (need regen), "
          f"{hard_fail} hard-fail")
    return 1 if hard_fail else 0


if __name__ == "__main__":
    sys.exit(main())
