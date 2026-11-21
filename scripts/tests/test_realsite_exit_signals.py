#!/usr/bin/env python3
"""The real-site board installs its exit handlers only for the signals the platform has.

main() turns SIGTERM and SIGHUP into SystemExit so the oracle's real-window lock is released.
Windows has no SIGHUP: registering it raised AttributeError before the first site was scored.

No browser and no network.
"""

import sys
import types
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import realsite_board  # noqa: E402


def fake_signal_module(*names):
    calls = []
    mod = types.SimpleNamespace(signal=lambda sig, handler: calls.append((sig, handler)))
    for i, name in enumerate(names, start=1):
        setattr(mod, name, i)
    return mod, calls


class ExitSignalTests(unittest.TestCase):
    def test_posix_installs_sigterm_and_sighup(self):
        mod, calls = fake_signal_module("SIGTERM", "SIGHUP")
        handler = object()
        realsite_board.install_exit_signals(handler, sigmod=mod)
        self.assertEqual(calls, [(mod.SIGTERM, handler), (mod.SIGHUP, handler)])

    def test_a_platform_without_sighup_installs_sigterm_only(self):
        mod, calls = fake_signal_module("SIGTERM")  # Windows
        handler = object()
        realsite_board.install_exit_signals(handler, sigmod=mod)
        self.assertEqual(calls, [(mod.SIGTERM, handler)])


if __name__ == "__main__":
    unittest.main()
