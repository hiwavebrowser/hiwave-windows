#!/usr/bin/env python3
"""Print a RustKit layout.json capture as an indented rect tree.

Usage: python3 scripts/dump_layout_tree.py parity-baseline/captures/<case>/layout.json [max_depth]
"""
import json
import sys


def walk(n, depth, max_depth):
    tag = n.get("tag") or n.get("name") or n.get("node") or "?"
    cls = n.get("classes") or n.get("class") or ""
    if isinstance(cls, list):
        cls = ".".join(cls)
    rect = n.get("border_box") or n.get("rect") or {}
    label = f"{tag}.{cls}" if cls else tag
    if depth <= max_depth:
        print(
            f'{"  " * depth}{label[:44]:44s} '
            f'x={rect.get("x")} y={rect.get("y")} '
            f'w={rect.get("width")} h={rect.get("height")}'
        )
    for c in n.get("children", []):
        walk(c, depth + 1, max_depth)


def main():
    path = sys.argv[1]
    max_depth = int(sys.argv[2]) if len(sys.argv) > 2 else 4
    data = json.load(open(path, encoding="utf-8"))
    root = data.get("root", data)
    walk(root, 0, max_depth)


if __name__ == "__main__":
    main()
