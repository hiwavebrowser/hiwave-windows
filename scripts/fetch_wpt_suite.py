"""Prepare the starter testharness suite before running either browser."""
import argparse
import hashlib
import json
from pathlib import Path
from urllib.request import urlopen

PIN = "a24eb4e7a77917cfa5a6f64e8b02288ca1df1877"
MANIFEST = Path(__file__).with_name("wpt_suite.json")


def verified_file(path, digest):
    return path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest() == digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    for name, digest in json.loads(MANIFEST.read_text(encoding="utf-8")).items():
        path = args.directory / name
        if verified_file(path, digest):
            continue
        with urlopen(f"https://raw.githubusercontent.com/web-platform-tests/wpt/{PIN}/{name}", timeout=60) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != digest:
            raise ValueError(f"SHA-256 mismatch for {name}")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)


if __name__ == "__main__":
    main()
