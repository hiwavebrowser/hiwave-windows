"""Compare local WPT JSONL runs and produce aggregate directory counts."""
import argparse
import json
from collections import Counter
from pathlib import Path

EXPECTATIONS = Path(__file__).with_name("wpt_expectations.json")

def read(path):
    rows = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        row = json.loads(line)
        if row["test"] in rows:
            raise ValueError("duplicate test page")
        rows[row["test"]] = row
    return rows


def scores(rows, reference, directories=None):
    totals = {}
    if rows.keys() != reference.keys():
        raise ValueError("page sets differ")
    if directories is None:
        directories = json.loads(EXPECTATIONS.read_text(encoding="utf-8"))
    for page, row in rows.items():
        parts = page.strip("/").split("/")[:-1]
        # Prefer the most specific recorded suite. Give new suites a stable
        # two-level group until their expectations have been recorded.
        directory = "/".join(parts[:2])
        for depth in range(len(parts), 0, -1):
            prefix = "/".join(parts[:depth])
            if prefix in directories:
                directory = prefix
                break
        counts = totals.setdefault(directory, Counter(passed=0, total=0, failed_pages=0))
        total = len(row["subtests"])
        if row["status"] != "OK":
            total = max(total, len(reference[page]["subtests"]), 1)
        counts["passed"] += sum(t["status"] == "PASS" for t in row["subtests"])
        counts["total"] += total
        counts["failed_pages"] += row["status"] != "OK"
    return totals


def subtests(rows):
    results = {}
    for page, row in rows.items():
        seen = Counter()
        for t in row["subtests"]:
            key = (page, t["name"], seen[t["name"]])
            results[key] = t["status"]
            seen[t["name"]] += 1
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["compare", "summary", "check"])
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    args = parser.parse_args()
    before, after = read(args.first), read(args.second)
    if args.mode == "check":
        expected = json.loads(EXPECTATIONS.read_text(encoding="utf-8"))
        if scores(before, after, expected) != expected:
            raise SystemExit("Directory counts differ from committed expectations")
        return
    if args.mode == "summary":
        print(json.dumps(scores(before, after), sort_keys=True, indent=2))
        return
    if before.keys() != after.keys():
        parser.error("page sets differ")
    old, new = subtests(before), subtests(after)
    for key in sorted(old.keys() | new.keys()):
        previous, current = old.get(key, "ABSENT"), new.get(key, "ABSENT")
        if (previous == "PASS") != (current == "PASS"):
            print(json.dumps(dict(test=key[0], name=key[1], occurrence=key[2],
                                  previous=previous, current=current)))


if __name__ == "__main__":
    main()
