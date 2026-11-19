"""Print the two measured WPT columns from matching local JSONL runs."""
import argparse
from pathlib import Path
from wpt_subtest_diff import read, scores


def rate(count):
    total = count["total"]
    passed = count["passed"]
    return f"{passed}/{total} ({passed / total:.1%})" if total else "0/0"


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("hiwave", type=Path)
    parser.add_argument("chrome", type=Path)
    args = parser.parse_args()
    chrome = read(args.chrome)
    hw = scores(read(args.hiwave), chrome)
    reference = scores(chrome, chrome)
    print("| Directory | HiWave | Chrome 148.0.7778.216 |")
    print("|---|---:|---:|")
    for directory in sorted(hw):
        print(f"| {directory} | {rate(hw[directory])} | {rate(reference[directory])} |")
