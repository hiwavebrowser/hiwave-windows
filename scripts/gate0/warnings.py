#!/usr/bin/env python3
"""Gate 0's warning census: the compiler warnings in `cargo check` logs, as a set, against a baseline.

Gate 0 compiles the tree after every change and every batch merge, and a warning that is not in the
baseline stops it. The set is the one the batch counts:

  engine + app:  cargo check -p rustkit-engine --features headless
                 cargo check -p hiwave-app --features rustkit
                 -> scripts/gate0/baseline-engine-app.txt
  renderer:      cargo check -p rustkit-renderer
                 -> scripts/gate0/baseline-renderer.txt   (its own set, counted apart)

Each warning is keyed `<message> @ <file>`, the file with `/` separators (a Windows seat's `\`
paths match). The line and column are left out on purpose, so code that moves inside a file does not
read as a new warning; the census counts each key, so a second identical warning in the same file is
new. Across the logs of one gate a key counts at its largest (the app's check repeats the engine's
warnings). A baseline line is the key, with ` xN` after it when the key occurs N > 1 times. Summary lines
(`N warnings generated`, `warning: `crate` (lib) generated`) are not warnings.

usage:
  warnings.py list <log>...                       print the set, sorted
  warnings.py check <baseline> <log>...           exit 1 and name what is new (and say what is gone)
"""
import re
import sys
from collections import Counter


def census(texts):
    """The warnings of the given cargo outputs: a count per key. Within one log every warning
    counts; across logs a key takes its largest count, because each `cargo check` reports the
    warnings of every crate it covers again (the app's check repeats the engine's), and a crate's
    warnings are the same in each log that covers it."""
    out = Counter()
    for text in texts:
        one = Counter()
        lines = re.sub(r"\x1b\[[0-9;]*m", "", text).split("\n")
        for i, line in enumerate(lines):
            m = re.match(r"^warning: (.*)$", line)
            if not m or re.search(r"generated \d+ warnings?|^warning: `", line) \
                    or "warning emitted" in line or "warnings emitted" in line:
                continue
            where = ""
            for k in range(i + 1, min(i + 4, len(lines))):
                mm = re.match(r"^\s*--> ([^:]+):\d+", lines[k])
                if mm:
                    where = mm.group(1).replace("\\", "/")
                    break
            one["%s @ %s" % (m.group(1).strip(), where)] += 1
        out |= one
    return out


def lines_of(counts):
    """A census as baseline lines, sorted: the key, with ` xN` when it occurs N > 1 times."""
    return [key if n == 1 else "%s x%d" % (key, n) for key, n in sorted(counts.items())]


def read_baseline(path):
    counts = Counter()
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.rstrip("\n")
            if not line.strip() or line.startswith("#"):
                continue
            m = re.match(r"^(.*) x(\d+)$", line)
            if m and " @ " in m.group(1):
                counts[m.group(1)] += int(m.group(2))
            else:
                counts[line] += 1
    return counts


def main(argv):
    if len(argv) >= 2 and argv[0] == "list":
        found = census(open(p, errors="replace", encoding="utf-8").read() for p in argv[1:])
        print("\n".join(lines_of(found)))
        return 0
    if len(argv) >= 3 and argv[0] == "check":
        base = read_baseline(argv[1])
        found = census(open(p, errors="replace", encoding="utf-8").read() for p in argv[2:])
        new = sorted(key for key in found if found[key] > base[key])
        gone = sorted(key for key in base if base[key] > found[key])
        # Keys first: the batch's "44 warnings" is the number of keys.
        print("warnings %d keys, %d in all (baseline %d keys, %d in all): %d new, %d gone"
              % (len(found), sum(found.values()), len(base), sum(base.values()), len(new), len(gone)))
        for key in new:
            print("NEW  %s" % key + ("" if base[key] == 0 else " (%d, baseline %d)" % (found[key], base[key])))
        for key in gone:
            print("gone %s" % key + ("" if found[key] == 0 else " (%d, baseline %d)" % (found[key], base[key])))
        return 1 if new else 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
