#!/usr/bin/env python3
"""test_scorer_v2.py — Unit and calibration tests for Scorer v2 (Package M0).

Runs with Python standard library only (no Pillow or numpy required).
Run: python3 scripts/tests/test_scorer_v2.py
"""

import json
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import scorer_v2


class TestScorerV2(unittest.TestCase):

    def test_classify_small_splash(self):
        # Small splash: low ink (<2%), rich colors (>500), vertical span
        v1_rec = {
            "id": "instagram",
            "loads": {"pass": False, "why": "blank frame (0.60% non-background)"},
            "readable": {"pass": False, "chrome_words": 46, "rustkit_words": 0},
            "looks_right": {"pass": False},
            "rustkit": {"script_stats": {"ran": 50, "threw": 0}},
        }
        rk_feat = {
            "non_bg_fraction": 0.006,
            "color_count": 2300,
            "region_fractions": {"top": 0.03, "mid": 0.0, "bot": 0.005, "mid_center": 0.0},
            "v_span": 0.8,
            "h_span": 0.5,
        }
        ch_feat = {"color_count": 50000, "non_bg_fraction": 0.20}
        cat, expl, meta = scorer_v2.classify_frame("instagram", v1_rec, rk_feat, ch_feat)
        self.assertEqual(cat, "SMALL_SPLASH")
        self.assertTrue(meta.get("v2_loads_override"))

    def test_classify_blank_shell(self):
        # Blank shell: thin top strip only, low colors
        v1_rec = {
            "id": "youtube",
            "loads": {"pass": False, "why": "blank frame (0.25% non-background)"},
            "readable": {"pass": False, "chrome_words": 22, "rustkit_words": 0},
            "looks_right": {"pass": False},
            "rustkit": {"script_stats": {"ran": 0, "threw": 0, "over_budget": 40}},
        }
        rk_feat = {
            "non_bg_fraction": 0.0025,
            "color_count": 16,
            "region_fractions": {"top": 0.017, "mid": 0.0, "bot": 0.0, "mid_center": 0.0},
            "v_span": 0.04,
            "h_span": 0.9,
        }
        ch_feat = {"color_count": 1000, "non_bg_fraction": 0.05}
        cat, expl, meta = scorer_v2.classify_frame("youtube", v1_rec, rk_feat, ch_feat)
        self.assertEqual(cat, "BLANK_SHELL")
        self.assertFalse(meta.get("v2_loads_override", False))

    def test_classify_google_logo(self):
        v1_rec = {
            "id": "google",
            "loads": {"pass": True},
            "readable": {"pass": True, "chrome_words": 24, "rustkit_words": 32, "ratio": 1.0},
            "looks_right": {"pass": True, "diff": 8.1},
            "points": 3,
        }
        rk_feat = {
            "non_bg_fraction": 0.087,
            "color_count": 1600,
            "region_fractions": {"top": 0.05, "mid": 0.11, "bot": 0.0, "mid_center": 0.20},
            "v_span": 0.45,
            "h_span": 0.95,
        }
        ch_feat = {"color_count": 300, "non_bg_fraction": 0.02}
        cat, expl, meta = scorer_v2.classify_frame("google", v1_rec, rk_feat, ch_feat)
        self.assertEqual(cat, "GOOGLE_LOGO")

    def test_classify_missing_art(self):
        # Content loaded with words, but massive photographic art missing
        v1_rec = {
            "id": "bing",
            "loads": {"pass": True},
            "readable": {"pass": False, "chrome_words": 40, "rustkit_words": 6, "ratio": 0.125},
            "looks_right": {"pass": False, "diff": 78.8},
            "points": 1,
        }
        rk_feat = {
            "non_bg_fraction": 0.36,
            "color_count": 226,  # Flat solid colors
            "region_fractions": {"top": 0.38, "mid": 0.22, "bot": 1.0, "mid_center": 0.22},
            "v_span": 1.0,
            "h_span": 1.0,
        }
        ch_feat = {
            "non_bg_fraction": 0.95,
            "color_count": 217000,  # Full photographic wallpaper
        }
        cat, expl, meta = scorer_v2.classify_frame("bing", v1_rec, rk_feat, ch_feat)
        self.assertEqual(cat, "MISSING_ART")

    def test_archive_calibration(self):
        # If the local archive exists, verify calibration against ground truth
        arch_dir = Path("P:/repos/hiwave-renders/archive/realsite/windows/3787391")
        if not arch_dir.exists():
            return

        summary = scorer_v2.score_run_v2(arch_dir)
        categories = {s["id"]: s["category"] for s in summary["sites"]}

        self.assertEqual(categories.get("google"), "GOOGLE_LOGO")
        self.assertEqual(categories.get("instagram"), "SMALL_SPLASH")
        self.assertEqual(categories.get("youtube"), "BLANK_SHELL")
        self.assertEqual(categories.get("microsoft"), "BLANK_SHELL")
        self.assertEqual(categories.get("reddit"), "BLANK_SHELL")
        self.assertEqual(categories.get("bing"), "MISSING_ART")
        self.assertIn(categories.get("apple"), ("CONTENT_LOADED", "MISSING_ART"))


if __name__ == "__main__":
    unittest.main()
