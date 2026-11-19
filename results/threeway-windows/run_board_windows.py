#!/usr/bin/env python3
"""Run scripts/chrome_board.py on Windows (Windows column of the three-way comparison).

On Windows `chrome.exe --version` prints nothing, so chrome_board.py's up-front pin probe reads no version
and refuses. Chrome for Testing ships `<version>.manifest` beside chrome.exe; this reads the version from it
and otherwise runs the board unchanged. chrome_board.mjs still checks browser.version() against the pin on
every row.

usage (from a hiwave-macos checkout at the compared commit, which has the board's UI pages and fixtures):
    PARITY_CHROME_PATH=<CfT 148 chrome.exe> python run_board_windows.py <checkout> --parity-capture <exe> --out <dir> --work <dir>
"""
import re
import sys
from pathlib import Path

checkout = Path(sys.argv[1]).resolve()
sys.argv = [str(checkout / "scripts" / "chrome_board.py")] + sys.argv[2:]
sys.path.insert(0, str(checkout / "scripts"))
import chrome_board  # noqa: E402


def manifest_version(path: Path):
    for manifest in path.parent.glob("*.manifest"):
        m = re.fullmatch(r"(\d+\.\d+\.\d+\.\d+)\.manifest", manifest.name)
        if m:
            return m.group(1)
    return None


chrome_board.probe_chrome_version = manifest_version
sys.exit(chrome_board.main(chrome_version=manifest_version))
