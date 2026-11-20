#!/usr/bin/env python3
"""sync_parity_tooling.py copies the chrome board's fixtures, not only its manifest.

tools/parity_oracle/chrome_fixtures/board.json arrived through the *.json pattern, but the
scripts it names (ipc_stub.js and one fixture per page) did not, so the chrome board refused
to run from this repository ("prelude ipc_stub.js missing; chrome: fixture chrome.js missing").

No network: a throwaway git repository stands in for the macOS checkout.

Run: python scripts/tests/test_sync_parity_tooling_copy_set.py
"""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import sync_parity_tooling  # noqa: E402

FILES = [
    "tools/parity_oracle/chrome_fixtures/board.json",
    "tools/parity_oracle/chrome_fixtures/ipc_stub.js",
    "tools/parity_oracle/chrome_fixtures/chrome.js",
    "tools/parity_oracle/chrome_board.mjs",
    "tools/parity_oracle/node_modules/playwright/index.mjs",
    "crates/hiwave-app/src/ui/chrome.html",
]


class CopySetTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.mac = Path(self._tmp.name)
        for rel in FILES:
            p = self.mac / rel
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_text("x\n", encoding="utf-8")
        git = ["git", "-C", str(self.mac), "-c", "user.name=t", "-c", "user.email=t@t"]
        subprocess.run(git[:3] + ["init", "-q"], check=True)
        subprocess.run(git + ["add", "-f", "."], check=True)
        subprocess.run(git + ["commit", "-q", "-m", "tooling"], check=True)

    def tearDown(self):
        self._tmp.cleanup()

    def test_the_chrome_board_fixtures_are_copied(self):
        files = sync_parity_tooling.copy_set(self.mac)
        for rel in ("tools/parity_oracle/chrome_fixtures/board.json",
                    "tools/parity_oracle/chrome_fixtures/ipc_stub.js",
                    "tools/parity_oracle/chrome_fixtures/chrome.js"):
            self.assertIn(rel, files)

    def test_node_modules_and_the_app_ui_pages_are_not(self):
        files = sync_parity_tooling.copy_set(self.mac)
        self.assertNotIn("tools/parity_oracle/node_modules/playwright/index.mjs", files)
        self.assertNotIn("crates/hiwave-app/src/ui/chrome.html", files)  # the Windows fork


if __name__ == "__main__":
    unittest.main()
