#!/usr/bin/env python3
"""test_holdout_board.py — Verification and guard tests for Package Z2-N1.

Tests holdout site set manifest integrity, dual-scorer execution (V1 + V2),
Rule A3 invariants (non-mutation of baselines), and overfitting guard metrics.

Pure Python standard library only (no external dependencies).
"""

import json
from pathlib import Path
import shutil
import sys
import tempfile
import unittest

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import holdout_board
import scorer_v2


class TestHoldoutBoard(unittest.TestCase):

    def setUp(self):
        self.tmp_dir = tempfile.mkdtemp(prefix="holdout_test_")

    def tearDown(self):
        shutil.rmtree(self.tmp_dir, ignore_errors=True)

    def test_holdout_manifest_integrity(self):
        """Websuite realsite-holdout20.json must exist, be valid JSON, and have exactly 20 sites."""
        manifest_path = REPO / "websuite" / "realsite-holdout20.json"
        self.assertTrue(manifest_path.exists(), f"Missing holdout manifest: {manifest_path}")

        data = json.loads(manifest_path.read_text(encoding="utf-8"))
        self.assertIn("viewport", data)
        self.assertEqual(data["viewport"]["width"], 1280)
        self.assertEqual(data["viewport"]["height"], 800)

        sites = data.get("sites", [])
        self.assertEqual(len(sites), 20, f"Expected exactly 20 sites, got {len(sites)}")

        seen_ids = set()
        for site in sites:
            self.assertIn("id", site)
            self.assertIn("url", site)
            self.assertTrue(site["url"].startswith("https://"))
            self.assertNotIn(site["id"], seen_ids, f"Duplicate site ID: {site['id']}")
            seen_ids.add(site["id"])

    def test_holdout_zero_overlap_with_top20(self):
        """Holdout sites must be completely unseen — 0 overlap with realsite-top20.json."""
        top20_path = REPO / "websuite" / "realsite-top20.json"
        holdout_path = REPO / "websuite" / "realsite-holdout20.json"
        self.assertTrue(top20_path.exists())
        self.assertTrue(holdout_path.exists())

        top20_data = json.loads(top20_path.read_text(encoding="utf-8"))
        holdout_data = json.loads(holdout_path.read_text(encoding="utf-8"))

        top20_ids = {s["id"] for s in top20_data.get("sites", [])}
        holdout_ids = {s["id"] for s in holdout_data.get("sites", [])}

        overlap = top20_ids.intersection(holdout_ids)
        self.assertEqual(
            len(overlap),
            0,
            f"Holdout set must not contain any top20 sites. Overlap found: {overlap}",
        )

    def test_top25_extends_top20_and_stays_out_of_holdout(self):
        """realsite-top25.json is the pinned top 20, same order, plus five; none of the 25 is held out."""
        top20 = json.loads((REPO / "websuite" / "realsite-top20.json").read_text(encoding="utf-8"))
        top25 = json.loads((REPO / "websuite" / "realsite-top25.json").read_text(encoding="utf-8"))
        holdout = json.loads((REPO / "websuite" / "realsite-holdout20.json").read_text(encoding="utf-8"))

        self.assertEqual(top25["viewport"], top20["viewport"])
        # The pinned 20 plus the sites Pete has added since (five on 2026-10-06, one on 2026-10-08).
        self.assertGreaterEqual(len(top25["sites"]), 25)
        self.assertEqual(top25["sites"][:20], top20["sites"])
        ids = [s["id"] for s in top25["sites"]]
        self.assertEqual(len(ids), len(set(ids)))
        overlap = set(ids) & {s["id"] for s in holdout["sites"]}
        self.assertEqual(overlap, set(), f"top25 sites must not be in the holdout: {overlap}")

    def test_holdout_subset_of_top80(self):
        """All 20 holdout sites must be drawn from realsite-top80.json."""
        top80_path = REPO / "websuite" / "realsite-top80.json"
        holdout_path = REPO / "websuite" / "realsite-holdout20.json"
        self.assertTrue(top80_path.exists())
        self.assertTrue(holdout_path.exists())

        top80_data = json.loads(top80_path.read_text(encoding="utf-8"))
        holdout_data = json.loads(holdout_path.read_text(encoding="utf-8"))

        top80_ids = {s["id"] for s in top80_data.get("sites", [])}
        holdout_ids = {s["id"] for s in holdout_data.get("sites", [])}

        missing_from_top80 = holdout_ids - top80_ids
        self.assertEqual(
            len(missing_from_top80),
            0,
            f"Holdout sites must belong to top80. Missing: {missing_from_top80}",
        )

    def test_dual_scoring_execution_and_rule_a3(self):
        """Dual-scoring runner must score both V1 and V2 side-by-side without mutating V1 files."""
        run_dir = Path(self.tmp_dir) / "run_20261003T010000Z"
        run_dir.mkdir(parents=True)

        # Create two site mock records in run_dir
        # Site 1: amazon (V1 pass, Content Loaded)
        amz_dir = run_dir / "amazon"
        amz_dir.mkdir()
        amz_v1 = {
            "id": "amazon",
            "loads": {"pass": True, "why": "35.20% non-background"},
            "readable": {"pass": True, "chrome_words": 150, "rustkit_words": 135, "ratio": 0.90},
            "looks_right": {"pass": False, "diff": 22.4, "chrome_self_diff": 2.1},
            "rustkit": {"non_background_fraction": 0.352, "script_stats": {"ran": 42, "threw": 0}},
            "points": 2,
        }
        (amz_dir / "amazon.json").write_text(json.dumps(amz_v1, indent=2), encoding="utf-8")

        # Site 2: chatgpt (V1 fail LOADS, V2 small splash)
        gpt_dir = run_dir / "chatgpt"
        gpt_dir.mkdir()
        gpt_v1 = {
            "id": "chatgpt",
            "loads": {"pass": False, "why": "blank frame (0.85% non-background)"},
            "readable": {"pass": False, "chrome_words": 30, "rustkit_words": 0, "ratio": 0.0},
            "looks_right": {"pass": False, "diff": 55.0, "chrome_self_diff": 1.0},
            "rustkit": {"non_background_fraction": 0.0085, "script_stats": {"ran": 20, "threw": 0}},
            "points": 0,
        }
        (gpt_dir / "chatgpt.json").write_text(json.dumps(gpt_v1, indent=2), encoding="utf-8")

        v1_summary = {
            "ts": "20261003T010000Z",
            "sites": 2,
            "points": 2,
            "max_points": 6,
            "loads": 1,
            "readable": 1,
            "looks_right": 0,
            "per_site": {"amazon": 2, "chatgpt": 0},
        }
        (run_dir / "summary.json").write_text(json.dumps(v1_summary, indent=2), encoding="utf-8")

        # Execute dual scoring via holdout_board
        dual_res = holdout_board.score_dual_run(run_dir)

        # 1. Verify V1 original summary is untouched (Rule A3)
        loaded_v1 = json.loads((run_dir / "summary.json").read_text(encoding="utf-8"))
        self.assertEqual(loaded_v1["points"], 2)
        self.assertEqual(loaded_v1["loads"], 1)

        # 2. Verify V2 summary is generated beside V1
        self.assertTrue((run_dir / "summary_v2.json").exists())
        self.assertTrue((amz_dir / "amazon.v2.json").exists())
        self.assertTrue((gpt_dir / "chatgpt.v2.json").exists())

        # 3. Verify dual-board fields
        self.assertIn("v1_summary", dual_res)
        self.assertIn("v2_summary", dual_res)
        self.assertEqual(dual_res["v1_summary"]["points"], 2)
        self.assertEqual(dual_res["v2_summary"]["v1_points"], 2)

    def test_overfitting_comparison_metrics(self):
        """compare_boards must compute pass rates, generalization gap, and category deltas."""
        top20_v1 = {"sites": 20, "points": 18, "max_points": 60, "loads": 12, "readable": 5, "looks_right": 1}
        top20_v2 = {
            "total_sites": 20,
            "v1_points": 18,
            "v1_loads_count": 12,
            "v2_adjusted_loads_count": 14,
            "categories": {"BLANK_SHELL": 6, "SMALL_SPLASH": 2, "CONTENT_LOADED": 11, "GOOGLE_LOGO": 1},
        }
        top20_summary = {"v1": top20_v1, "v2": top20_v2}

        holdout_v1 = {"sites": 20, "points": 14, "max_points": 60, "loads": 10, "readable": 4, "looks_right": 0}
        holdout_v2 = {
            "total_sites": 20,
            "v1_points": 14,
            "v1_loads_count": 10,
            "v2_adjusted_loads_count": 12,
            "categories": {"BLANK_SHELL": 8, "SMALL_SPLASH": 2, "CONTENT_LOADED": 10},
        }
        holdout_summary = {"v1": holdout_v1, "v2": holdout_v2}

        comparison = holdout_board.compare_boards(top20_summary, holdout_summary)

        self.assertIn("top20_points_rate", comparison)
        self.assertIn("holdout_points_rate", comparison)
        self.assertIn("points_rate_gap", comparison)
        self.assertIn("v1_loads_rate_gap", comparison)
        self.assertIn("v2_adjusted_loads_rate_gap", comparison)
        self.assertIn("category_deltas", comparison)

        # 18/60 = 30.0%, 14/60 = 23.33% -> gap ~ 6.67%
        self.assertAlmostEqual(comparison["top20_points_rate"], 30.0, places=1)
        self.assertAlmostEqual(comparison["holdout_points_rate"], 23.33, places=1)
        self.assertAlmostEqual(comparison["points_rate_gap"], 6.67, places=1)

    def test_realsite_board_holdout_support(self):
        """realsite_board must export DEFAULT_SITES_FILE and HOLDOUT_SITES_FILE."""
        import realsite_board
        self.assertTrue(realsite_board.DEFAULT_SITES_FILE.exists())
        self.assertTrue(realsite_board.HOLDOUT_SITES_FILE.exists())
        self.assertEqual(realsite_board.DEFAULT_SITES_FILE.name, "realsite-top20.json")
        self.assertEqual(realsite_board.HOLDOUT_SITES_FILE.name, "realsite-holdout20.json")

    def test_comparison_table_formatting(self):
        """format_comparison_table must format output containing header, metrics and categories."""
        comp = {
            "top20_sites": 20,
            "holdout_sites": 20,
            "top20_points_rate": 30.0,
            "holdout_points_rate": 23.33,
            "points_rate_gap": 6.67,
            "top20_v1_loads_rate": 60.0,
            "holdout_v1_loads_rate": 50.0,
            "v1_loads_rate_gap": 10.0,
            "top20_v2_adjusted_loads_rate": 70.0,
            "holdout_v2_adjusted_loads_rate": 60.0,
            "v2_adjusted_loads_rate_gap": 10.0,
            "category_deltas": {
                "BLANK_SHELL": {"top20": 6, "holdout": 8, "delta": 2},
                "CONTENT_LOADED": {"top20": 11, "holdout": 10, "delta": -1},
            },
        }
        table = holdout_board.format_comparison_table(comp)
        self.assertIn("OVERFITTING GENERALIZATION REPORT", table)
        self.assertIn("V1 Points Rate", table)
        self.assertIn("BLANK_SHELL", table)
        self.assertIn("CONTENT_LOADED", table)


if __name__ == "__main__":
    unittest.main()
