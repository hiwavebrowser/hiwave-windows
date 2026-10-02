#!/usr/bin/env python3
"""Compare two parity_test_results.json runs case-by-case.

Usage: python3 scripts/compare_parity_runs.py <old.json> <new.json>
"""
import json
import sys


def main():
    old = json.load(open(sys.argv[1], encoding="utf-8"))
    new = json.load(open(sys.argv[2], encoding="utf-8"))
    o = {r["case_id"]: r.get("diff_pct") for r in old["results"]}
    n = {r["case_id"]: r.get("diff_pct") for r in new["results"]}

    print(f'{"case":26s} {"old":>7s} {"new":>7s} {"delta":>7s}')
    tot_o = tot_n = count = 0
    for k in n:
        do, dn = o.get(k), n[k]
        if do is None or dn is None:
            continue
        count += 1
        tot_o += do
        tot_n += dn
        mark = ""
        if abs(dn - do) > 0.3:
            mark = "  improved" if dn < do else "  REGRESSED"
        print(f'{k:26s} {do:7.2f} {dn:7.2f} {dn - do:+7.2f}{mark}')
    print(f'{"AVG":26s} {tot_o / count:7.2f} {tot_n / count:7.2f}')

    op = sum(1 for r in old["results"] if r.get("passed"))
    np_ = sum(1 for r in new["results"] if r.get("passed"))
    print(f"passed: {op}/{len(o)} -> {np_}/{len(n)}")


if __name__ == "__main__":
    main()
