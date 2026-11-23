#!/usr/bin/env python3
"""The real-site board checks, before any site is loaded, that the parity-capture it was given
accepts every flag the board will pass it. A parity-capture older than the board (one without
--dump-har, which QB-ARCHIVE-INPUT added) once rejected every capture of a live run: all 20
sites scored nothing while the Chrome arm still loaded each of them (hiwave-linux, 2026-11-21).
"""
import os
import stat
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import realsite_board  # noqa: E402

HELP_WITH = """Usage: parity-capture --width <WIDTH> --height <HEIGHT>
Options:
      --url <URL>
      --width <WIDTH>
      --height <HEIGHT>
      --timeout-ms <TIMEOUT_MS>
      --dump-frame <DUMP_FRAME>
      --dump-display-list <DUMP_DISPLAY_LIST>
      --dump-scripts <DUMP_SCRIPTS>
      --dump-har <DUMP_HAR>
"""
HELP_WITHOUT = HELP_WITH.replace("      --dump-har <DUMP_HAR>\n", "")


def fake_capture(directory, help_text):
    """A stand-in parity-capture that prints `help_text` for --help."""
    path = Path(directory) / "parity-capture"
    path.write_text("#!/bin/sh\ncat <<'EOF'\n" + help_text + "EOF\n", encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    return path


@unittest.skipIf(os.name == "nt", "the stand-in binary is a shell script")
class CapturePreflight(unittest.TestCase):
    def test_a_binary_without_a_flag_the_board_passes_is_named(self):
        with tempfile.TemporaryDirectory() as d:
            old = fake_capture(d, HELP_WITHOUT)
            missing = realsite_board.capture_flags_missing(old, realsite_board.capture_flags(archive=True))
            self.assertEqual(missing, ["--dump-har"])

    def test_a_binary_with_every_flag_passes(self):
        with tempfile.TemporaryDirectory() as d:
            new = fake_capture(d, HELP_WITH)
            self.assertEqual(realsite_board.capture_flags_missing(new, realsite_board.capture_flags(archive=True)), [])

    def test_without_archiving_the_har_flag_is_not_required(self):
        with tempfile.TemporaryDirectory() as d:
            old = fake_capture(d, HELP_WITHOUT)
            self.assertEqual(realsite_board.capture_flags_missing(old, realsite_board.capture_flags(archive=False)), [])


if __name__ == "__main__":
    unittest.main()
