#!/usr/bin/env python3
"""non_background_fraction works without numpy, and gives the same value as the numpy path.

A Linux live run stopped at its first site because the board imported numpy, which the gate
box's Python does not have (hiwave-linux, 2026-11-21). The fallback must agree with the numpy
path exactly, including which colour is "dominant" when two colours tie (numpy's unique + argmax
takes the smallest packed RGB value).
"""
import builtins
import os
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))

import realsite_board  # noqa: E402

BLACK, WHITE, NEAR_WHITE = (0, 0, 0), (255, 255, 255), (250, 250, 250)


def write_ppm(directory, pixels, name="f.ppm"):
    path = Path(directory) / name
    path.write_bytes(b"P6\n%d 1\n255\n" % len(pixels) + bytes(c for px in pixels for c in px))
    return path


class WithoutNumpy:
    """Make `import numpy` fail inside the block, as on a Python without it."""

    def __enter__(self):
        self.real_import = builtins.__import__

        def blocked(name, *args, **kwargs):
            if name == "numpy" or name.startswith("numpy."):
                raise ModuleNotFoundError("No module named 'numpy'")
            return self.real_import(name, *args, **kwargs)

        builtins.__import__ = blocked
        return self

    def __exit__(self, *exc):
        builtins.__import__ = self.real_import


CASES = [
    ("one colour", [WHITE] * 4, 0.0),
    ("one pixel off by more than 8", [WHITE] * 3 + [BLACK], 0.25),
    ("off by exactly 8 is background", [WHITE] * 3 + [(247, 255, 255)], 0.0),
    # black and white tie at 2; the smallest packed value (black) is dominant, so white and
    # near-white are both non-background: 3/5 (taking white would give 2/5).
    ("a tie goes to the smallest RGB", [BLACK, BLACK, WHITE, WHITE, NEAR_WHITE], 0.6),
]


class BackgroundFraction(unittest.TestCase):
    def test_without_numpy(self):
        with tempfile.TemporaryDirectory() as d:
            for name, pixels, want in CASES:
                path = write_ppm(d, pixels)
                with WithoutNumpy():
                    got = realsite_board.non_background_fraction(path)
                self.assertAlmostEqual(got, want, msg=name)

    def test_same_value_as_numpy(self):
        try:
            import numpy  # noqa: F401
        except ModuleNotFoundError:
            if os.environ.get("REALSITE_NUMPY_REFERENCE") == "1":
                # The CI step that exists to run this comparison: a skip there would be a green
                # check on a comparison that never ran.
                self.fail("REALSITE_NUMPY_REFERENCE=1 but numpy is not installed for this Python")
            self.skipTest("numpy not installed: the fallback is checked against hand values only")
        with tempfile.TemporaryDirectory() as d:
            for name, pixels, _ in CASES:
                path = write_ppm(d, pixels)
                with WithoutNumpy():
                    pure = realsite_board.non_background_fraction(path)
                self.assertEqual(pure, realsite_board.non_background_fraction(path), msg=name)


if __name__ == "__main__":
    unittest.main()
