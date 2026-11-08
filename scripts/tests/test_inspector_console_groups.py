#!/usr/bin/env python3
"""The devtools panel groups console lines by document (T6b).

WHAT THIS GUARDS
----------------
Since T6b the panel keeps its console history across a navigation, and
every line carries the document it was logged in (`doc`). The panel's
`consoleRows` (crates/hiwave-app/src/ui/inspector.html) turns the lines it
shows into rows: a divider wherever the document changes, and lines from
documents before the newest one marked stale. With one document in view
there are no dividers. A line without a doc (the console's own input and
results) belongs to the line before it.

The function is pure, so this test cuts it out of the page and runs it in
node. Skips when node is not installed.

Run: python3 scripts/tests/test_inspector_console_groups.py
"""
import json
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PANEL = REPO_ROOT / "crates" / "hiwave-app" / "src" / "ui" / "inspector.html"


def _function_source(name):
    text = PANEL.read_text(encoding="utf-8")
    start = text.find(f"function {name}(")
    assert start >= 0, f"{name} is not in {PANEL.name}"
    depth = 0
    for i in range(text.index("{", start), len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[start : i + 1]
    raise AssertionError(f"{name} has no closing brace")


def _rows(entries):
    """`consoleRows(entries)`, each row as ("divider", doc, stale) or
    ("line", id, stale)."""
    script = (
        _function_source("consoleRows")
        + "\nconst rows = consoleRows(JSON.parse(process.argv[1]));"
        + "\nconsole.log(JSON.stringify(rows.map(r => r.entry"
        + " ? ['line', r.entry.id, r.stale] : ['divider', r.divider, r.stale])));"
    )
    out = subprocess.run(
        ["node", "-e", script, json.dumps(entries)],
        capture_output=True,
        text=True,
        check=True, encoding="utf-8"
    )
    return [tuple(r) for r in json.loads(out.stdout)]


def _line(id, doc=None):
    entry = {"id": id, "level": "log", "message": f"line {id}", "stack": []}
    if doc is not None:
        entry["doc"] = doc
    return entry


def test_one_document_has_no_dividers():
    assert _rows([_line(1, 3), _line(2, 3)]) == [("line", 1, False), ("line", 2, False)]


def test_a_navigation_puts_a_divider_and_marks_the_old_lines():
    rows = _rows([_line(1, 1), _line(2, 1), _line(3, 2), _line(4, 2)])
    assert rows == [
        ("divider", 1, True),
        ("line", 1, True),
        ("line", 2, True),
        ("divider", 2, False),
        ("line", 3, False),
        ("line", 4, False),
    ]


def test_a_line_without_a_doc_belongs_to_the_line_before_it():
    rows = _rows([_line(1, 1), _line(2), _line(3, 2), _line(4)])
    assert rows == [
        ("divider", 1, True),
        ("line", 1, True),
        ("line", 2, True),
        ("divider", 2, False),
        ("line", 3, False),
        ("line", 4, False),
    ]


def test_no_lines_no_rows():
    assert _rows([]) == []


if __name__ == "__main__":
    if shutil.which("node") is None:
        print("SKIP: node is not installed")
        sys.exit(0)
    tests = [v for k, v in sorted(globals().items()) if k.startswith("test_")]
    for t in tests:
        t()
        print(f"ok {t.__name__}")
    print(f"{len(tests)} passed")
