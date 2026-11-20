#!/usr/bin/env python3
"""sync_engine_fixtures.py: out-of-crate fixtures that synced engine crates embed follow hiwave-macos.

A refresh copies crates/rustkit-* verbatim; files those crates include from OUTSIDE their crate
(fixtures/, tools/, docs/diagnostics/, ...) come from this script. It used to copy only MISSING
files, so a fixture the Mac changed stayed at its old Windows copy and the Windows tests compared
against a stale expectation (media_box_cft148.json, refresh #21). It now also refreshes a CHANGED
file outside crates/. Under crates/ an existing file is never touched: synced crates own their
files (with Windows' patches) and hiwave-app's UI pages are a Windows fork. A copy that differs
only in line endings (core.autocrlf) is not a change.

No network: a throwaway git repository stands in for the macOS checkout.

Run: python scripts/tests/test_sync_engine_fixtures.py
"""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import sync_engine_fixtures  # noqa: E402

LIB_RS = """
const A: &str = include_str!("../../../fixtures/stale.html");
const B: &str = include_str!("../../../fixtures/missing.html");
const C: &str = include_str!("../../../fixtures/crlf.html");
const D: &str = include_str!("../../hiwave-app/src/ui/page.html");
"""


def write(root: Path, rel: str, data: bytes) -> None:
    p = root / rel
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(data)


class SyncEngineFixturesTests(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        tmp = Path(self._tmp.name)
        self.mac, self.win = tmp / "mac", tmp / "win"
        mac_files = {
            "crates/rustkit-x/src/lib.rs": LIB_RS.encode(),
            "fixtures/stale.html": b"<p>new</p>\n",
            "fixtures/missing.html": b"<p>added</p>\n",
            "fixtures/crlf.html": b"<p>same</p>\n",
            "crates/hiwave-app/src/ui/page.html": b"<p>mac ui</p>\n",
        }
        for rel, data in mac_files.items():
            write(self.mac, rel, data)
        git = ["git", "-C", str(self.mac), "-c", "user.name=t", "-c", "user.email=t@t", "-c", "core.autocrlf=false"]
        subprocess.run(git[:3] + ["init", "-q"], check=True)
        subprocess.run(git + ["add", "."], check=True)
        subprocess.run(git + ["commit", "-q", "-m", "fixtures"], check=True)
        write(self.win, "crates/rustkit-x/src/lib.rs", LIB_RS.encode())
        write(self.win, "fixtures/stale.html", b"<p>old</p>\n")
        write(self.win, "fixtures/crlf.html", b"<p>same</p>\r\n")
        write(self.win, "crates/hiwave-app/src/ui/page.html", b"<p>windows fork</p>\n")
        self._repo = sync_engine_fixtures.REPO
        self._argv = sys.argv
        sync_engine_fixtures.REPO = self.win
        sys.argv = ["sync_engine_fixtures.py", str(self.mac)]

    def tearDown(self):
        sync_engine_fixtures.REPO = self._repo
        sys.argv = self._argv
        self._tmp.cleanup()

    def test_a_missing_fixture_is_copied(self):
        self.assertEqual(sync_engine_fixtures.main(), 0)
        self.assertEqual((self.win / "fixtures/missing.html").read_bytes(), b"<p>added</p>\n")

    def test_a_changed_fixture_outside_crates_is_refreshed(self):
        self.assertEqual(sync_engine_fixtures.main(), 0)
        self.assertEqual((self.win / "fixtures/stale.html").read_bytes(), b"<p>new</p>\n")

    def test_a_line_ending_only_difference_is_not_a_change(self):
        self.assertEqual(sync_engine_fixtures.main(), 0)
        self.assertEqual((self.win / "fixtures/crlf.html").read_bytes(), b"<p>same</p>\r\n")

    def test_an_existing_file_under_crates_is_never_overwritten(self):
        self.assertEqual(sync_engine_fixtures.main(), 0)
        self.assertEqual((self.win / "crates/hiwave-app/src/ui/page.html").read_bytes(), b"<p>windows fork</p>\n")


if __name__ == "__main__":
    unittest.main()
