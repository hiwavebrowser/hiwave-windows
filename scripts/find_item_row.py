#!/usr/bin/env python3
"""Seat tooling (untracked): print x/width of any sibling row of >=5 boxes 200px wide."""
import json
import sys

d = json.load(open(sys.argv[1], encoding="utf-8"))
root = d.get("root", d)


def rect(n):
    return n.get("border_box") or n.get("rect") or {}


def walk(n):
    ch = n.get("children", [])
    wide = [c for c in ch if rect(c).get("width") == 200.0]
    if len(wide) >= 5:
        print([(rect(c).get("x"), rect(c).get("width")) for c in ch])
    for c in ch:
        walk(c)


walk(root)
