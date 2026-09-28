"""Sync the parity tooling from a hiwave-macos checkout into this repo.

Pete, 2026-09-27: the Windows board runs the same parity cases and gates as
macOS, with changes made ONLY in hiwave-windows (hiwave-macos is untouched).
This script is the whole Windows residual, applied mechanically, so a re-sync
is one command and a reviewer can re-run it to check a sync PR byte for byte:

    python scripts/sync_parity_tooling.py <path-to-hiwave-macos-checkout>

What it copies VERBATIM from macOS (tracked files only):
  scripts/*.py, scripts/tests/**, tools/parity_oracle/*.mjs + package.json,
  cases/*.json, websuite/*.json, docs/VISUAL_DIFF_POLICY.md (the gates parse it)
What it does NOT copy:
  - baselines/: Windows keeps its own Chrome-for-Testing-148 captures
    (baselines/chrome-148/metadata.json says platform win32).
  - *.sh: Windows has .ps1 equivalents; those stay as they are.
  - the two tests of the macOS parity CI lanes (MACOS_CI_TESTS below).
  - tools/parity_oracle/node_modules (gitignored here; `npm ci` locally),
    scripts/cargo-run.mjs (a macOS seat PATH shim), parity-baseline/ (the macOS
    board's own state).
Windows-only files already in this repo are never deleted.

Then it applies three Windows fixes to the copied Python, and nothing else:
  1. encoding="utf-8" on every text-mode open()/io.open(), Path.read_text()/
     write_text(), and text=True subprocess call that lacks one. A bare text
     open on Windows is cp1252 and crashes on the corpus (Aleph #35 class).
     Calls are located with the ast module, not regexes; open() calls whose
     mode is not a string literal are left alone and listed.
  2. the parity-capture binary path gains ".exe" on Windows (exists() checks
     fail otherwise, so --skip-build is ignored and every run rebuilds).
  3. the 'latest' run pointer falls back to latest.txt when creating a symlink
     needs elevation (WinError 1314), and readers accept that file.
  4. the Homebrew PATH prefix joins with os.pathsep. macOS writes
     "/opt/homebrew/bin:" + PATH; on Windows the separator is ";", so the colon
     glued itself onto the first PATH entry and hid whatever lived there.
  5. paths spliced into inline `node -e` JavaScript are quoted with
     json.dumps. macOS writes '{path}' into a JS string literal; a Windows
     path's backslashes then read as escape sequences, which node rejects as a
     SyntaxError, so every compare failed and every case was NOT-MEASURED.
"""
from __future__ import annotations

import ast
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
ENC = 'encoding="utf-8"'
THIS = "scripts/sync_parity_tooling.py"
# Tests of the macOS parity CI lanes (.github/workflows/parity.yml, CoreText
# runners). Windows has no parity CI lane (GitHub's Windows runners have no GPU;
# see scripts/collect_metrics.py), so these would fail forever here.
MACOS_CI_TESTS = {"scripts/tests/test_stability_actually_gates.py",
                  "scripts/tests/test_unit_suites_actually_run.py"}


def tracked(src: Path, *patterns: str) -> list[str]:
    out = subprocess.run(["git", "-C", str(src), "ls-files", "-z", "--", *patterns],
                         capture_output=True, check=True).stdout.decode("utf-8")
    return sorted(f for f in out.split("\0") if f)


def copy_set(src: Path) -> list[str]:
    files = tracked(src, "scripts/*.py", "scripts/tests", "tools/parity_oracle/*.mjs",
                    "tools/parity_oracle/package.json", "cases/*.json", "websuite/*.json",
                    "docs/VISUAL_DIFF_POLICY.md")
    return [f for f in files if "/node_modules/" not in f and f != THIS and f not in MACOS_CI_TESTS]


# --- fix 1: encoding ---------------------------------------------------------

def _mode_of(call: ast.Call, pos: int):
    """The mode argument of an open-like call: str, None (absent), or ... (non-literal)."""
    node = call.args[pos] if len(call.args) > pos else None
    for kw in call.keywords:
        if kw.arg == "mode":
            node = kw.value
    if node is None:
        return None
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return ...


def _needs_encoding(call: ast.Call):
    """True / False, or 'dynamic' for an open() whose mode we cannot see."""
    if any(kw.arg == "encoding" or kw.arg is None for kw in call.keywords):
        return False
    f = call.func
    name = f.id if isinstance(f, ast.Name) else f.attr if isinstance(f, ast.Attribute) else None
    if name == "open":
        # builtin open(file, mode) and io.open(file, mode); Path.open(mode) has mode first.
        is_method = isinstance(f, ast.Attribute) and not (
            isinstance(f.value, ast.Name) and f.value.id in ("io", "codecs", "builtins"))
        if isinstance(f, ast.Attribute) and isinstance(f.value, ast.Name) and f.value.id in ("os", "gzip", "tarfile", "zipfile", "webbrowser"):
            return False
        mode = _mode_of(call, 0 if is_method else 1)
        if mode is ...:
            return "dynamic"
        return "b" not in (mode or "r")
    if name in ("read_text", "write_text"):
        return True
    if name in ("run", "check_output", "Popen", "check_call", "call"):
        return any(kw.arg in ("text", "universal_newlines") and isinstance(kw.value, ast.Constant)
                   and kw.value.value is True for kw in call.keywords)
    return False


def add_encoding(text: str, path: str, report: list[str]) -> str:
    tree = ast.parse(text)
    raw = text.encode("utf-8")
    offs = [0]
    for ln in text.splitlines(keepends=True):
        offs.append(offs[-1] + len(ln.encode("utf-8")))
    edits = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        need = _needs_encoding(node)
        if need == "dynamic":
            report.append(f"{path}:{node.lineno}: open() with a non-literal mode left alone")
            continue
        if not need:
            continue
        end = offs[node.end_lineno - 1] + node.end_col_offset  # byte offset just past ')'
        assert raw[end - 1:end] == b")", (path, node.lineno)
        inner = raw[:end - 1].rstrip()
        sep = b" " if inner.endswith(b",") else (b"" if inner.endswith(b"(") else b", ")
        edits.append((len(inner), sep + ENC.encode()))
    for pos, ins in sorted(edits, reverse=True):
        raw = raw[:pos] + ins + raw[pos:]
    return raw.decode("utf-8")


# --- fix 2 and 3: exact, asserted replacements ------------------------------

EXE = '("parity-capture.exe" if os.name == "nt" else "parity-capture")'
PATH_OLD = 'f"/opt/homebrew/bin:{os.environ.get(\'PATH\', \'\')}"'
PATH_NEW = '"/opt/homebrew/bin" + os.pathsep + os.environ.get(\'PATH\', \'\')'

BIN_REPLACEMENTS = [
    ('"target" / "release" / "parity-capture"', f'"target" / "release" / {EXE}'),
    ('str(REPO / "target/release/parity-capture")', f'str(REPO / "target" / "release" / {EXE})'),
]

LATEST_WRITE_OLD = "    latest_link.symlink_to(timestamp)\n"
LATEST_WRITE_NEW = ("    try:\n"
                    "        latest_link.symlink_to(timestamp)\n"
                    "    except OSError:  # Windows: symlinks need elevation (WinError 1314)\n"
                    "        (history_dir / \"latest.txt\").write_text(timestamp)\n")
LATEST_READ_OLD = ("        if latest_link.is_symlink():\n"
                   "            run_id = latest_link.resolve().name\n")
LATEST_READ_NEW = ("        if latest_link.is_symlink():\n"
                   "            run_id = latest_link.resolve().name\n"
                   "        elif (history_dir / \"latest.txt\").exists():  # Windows fallback, see parity_archive\n"
                   "            run_id = (history_dir / \"latest.txt\").read_text().strip()\n"
                   "            latest_link = history_dir / run_id\n")


NODE_E = '"node", "-e"'
JS_PATH = re.compile(r"'\{([A-Za-z_][A-Za-z_0-9]*(?: / [A-Za-z_][A-Za-z_0-9]*)?)\}'")


def quote_inline_js_paths(rel: str, text: str) -> str:
    """Inside each node -e f-string block, '{expr}' becomes {json.dumps(str(expr))}."""
    out, pos, hits = [], 0, 0
    while True:
        i = text.find(NODE_E, pos)
        if i == -1:
            break
        start = text.find('f"""', i)
        end = text.find('"""', start + 4)
        assert start != -1 and end != -1, f"{rel}: unterminated node -e block"
        block, n = JS_PATH.subn(r"{json.dumps(str(\1))}", text[start:end])
        hits += n
        out.append(text[pos:start] + block)
        pos = end
    out.append(text[pos:])
    text = "".join(out)
    if hits:
        text = ensure_import(text, "json")
    return text


def ensure_import(text: str, mod: str) -> str:
    tree = ast.parse(text)
    if any(isinstance(n, ast.Import) and any(a.name == mod for a in n.names) for n in tree.body):
        return text
    last = max(n.end_lineno for n in tree.body if isinstance(n, (ast.Import, ast.ImportFrom)))
    lines = text.splitlines(keepends=True)
    lines.insert(last, f"import {mod}  # Windows sync\n")
    return "".join(lines)


def ensure_import_os(text: str) -> str:
    tree = ast.parse(text)
    if any(isinstance(n, ast.Import) and any(a.name == "os" for a in n.names) for n in tree.body):
        return text
    last = max(n.end_lineno for n in tree.body if isinstance(n, (ast.Import, ast.ImportFrom)))
    lines = text.splitlines(keepends=True)
    lines.insert(last, "import os  # Windows sync: .exe suffix\n")
    return "".join(lines)


def windows_fixes(rel: str, text: str, report: list[str]) -> str:
    for old, new in BIN_REPLACEMENTS:
        if old in text:
            text = ensure_import_os(text.replace(old, new))
    text = text.replace(PATH_OLD, PATH_NEW)
    text = quote_inline_js_paths(rel, text)
    if rel == "scripts/parity_archive.py":
        assert text.count(LATEST_WRITE_OLD) == 1, "parity_archive latest pointer moved upstream"
        text = text.replace(LATEST_WRITE_OLD, LATEST_WRITE_NEW)
    if rel == "scripts/parity_compare.py":
        assert text.count(LATEST_READ_OLD) == 1, "parity_compare latest reader moved upstream"
        text = text.replace(LATEST_READ_OLD, LATEST_READ_NEW)
    return add_encoding(text, rel, report)


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    src = Path(sys.argv[1]).resolve()
    report: list[str] = []
    changed = 0
    for rel in copy_set(src):
        data = (src / rel).read_bytes()
        if rel.endswith(".py"):
            text = data.decode("utf-8").replace("\r\n", "\n")
            data = windows_fixes(rel, text, report).encode("utf-8")
        dst = REPO / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        if not dst.exists() or dst.read_bytes() != data:
            changed += 1
        dst.write_bytes(data)
    head = subprocess.run(["git", "-C", str(src), "rev-parse", "--short", "HEAD"],
                          capture_output=True, text=True, encoding="utf-8").stdout.strip()
    print(f"synced from hiwave-macos {head}: {changed} file(s) written")
    for line in report:
        print("  note:", line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
