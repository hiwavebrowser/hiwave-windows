#!/usr/bin/env python3
"""#1104 R1: a capture whose settle was cut short at its real-time budget is timing-bound.

parity-capture's stepped settle stops at a real-time budget, and reports it in
`settle_stats` (`cut_short`, `virtual_ms`). How far a cut-short settle got depends on
the machine's speed, so the board names those sites: they are not comparable run to
run (a slow runner shows diffs that are only timing). Scores are unchanged.

No browser and no network: rows are built by hand.
"""

import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import realsite_board  # noqa: E402


def row(site_id, rustkit):
    return {"id": site_id, "rustkit": rustkit}


class SettleCutShort(unittest.TestCase):
    def test_names_only_sites_whose_settle_was_cut_short(self):
        rows = [
            row("full", {"status": "ok", "settle_stats": {"cut_short": False, "virtual_ms": 5000}}),
            row("cut", {"status": "ok", "settle_stats": {"cut_short": True, "virtual_ms": 50}}),
            row("older-tool", {"status": "ok"}),
            row("timed-out", {"status": "timeout", "error": "capture exceeded 30000 ms"}),
            row("crashed", {"status": "crash", "error": "x", "exit_code": 3}),
        ]
        self.assertEqual(realsite_board.settle_cut_short(rows), ["cut"])

    def test_none_when_no_site_was_cut_short(self):
        self.assertEqual(realsite_board.settle_cut_short([row("a", {"status": "ok"})]), [])


if __name__ == "__main__":
    unittest.main()
