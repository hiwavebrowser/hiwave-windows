"""
test_actions_script.py — Guard tests for Package Z2-I1 (Actions Script & Mirror).

Validates:
1. Action script schema and supported action primitives (wait, click, key, resize, capture).
2. Translation of websuite/interactions-top20.json catalog entries into action sequences.
3. Execution and parsing of parity-capture action results.
4. Per-step frame diff calculation and reporting.
"""

import json
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "scripts"))

CATALOG_PATH = REPO / "websuite" / "interactions-top20.json"


class TestActionsScript(unittest.TestCase):
    def test_catalog_to_action_sequence(self):
        """Verify translating catalog interactions into well-formed action sequences."""
        from scripts.interactive_board import catalog_to_actions

        self.assertTrue(CATALOG_PATH.exists(), f"catalog missing: {CATALOG_PATH}")
        catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
        interactions = catalog.get("interactions", {})
        self.assertEqual(len(interactions), 20)

        for site_id, item in interactions.items():
            actions = catalog_to_actions(site_id, item)
            self.assertIsInstance(actions, list)
            self.assertGreaterEqual(len(actions), 3)  # before capture, action(s), wait, after capture

            types = [a["type"] for a in actions]
            self.assertIn("capture", types)
            self.assertIn("wait", types)
            self.assertTrue(any(t in ("click", "key", "resize") for t in types))

    def test_action_validation(self):
        """Validate action schema validator catches invalid actions."""
        from scripts.interactive_board import validate_action_sequence

        valid_seq = [
            {"type": "capture", "label": "before", "frame": "before.png"},
            {"type": "click", "selector": "button.search"},
            {"type": "wait", "ms": 500},
            {"type": "key", "text": "hello"},
            {"type": "resize", "width": 1024, "height": 768},
            {"type": "capture", "label": "after", "frame": "after.png"},
        ]
        errors = validate_action_sequence(valid_seq)
        self.assertEqual(errors, [])

        invalid_seq = [
            {"type": "unknown_action"},
            {"type": "click"},  # missing selector and x/y
            {"type": "wait", "ms": -10},
            {"type": "resize", "width": 0},
            {"type": "capture"},  # missing frame
        ]
        errors = validate_action_sequence(invalid_seq)
        self.assertGreaterEqual(len(errors), 5)

    def test_per_step_diff_report(self):
        """Verify per-step frame diff aggregation between engines."""
        from scripts.interactive_board import compute_step_diffs

        chrome_captures = [
            {"step": 0, "label": "before", "frame": "c_before.png"},
            {"step": 3, "label": "after", "frame": "c_after.png"},
        ]
        rustkit_captures = [
            {"step": 0, "label": "before", "frame": "r_before.png"},
            {"step": 3, "label": "after", "frame": "r_after.png"},
        ]

        def dummy_diff(a, b):
            return {"diffPercent": 5.0 if "before" in str(a) else 8.5}

        report = compute_step_diffs(chrome_captures, rustkit_captures, diff_fn=dummy_diff)
        self.assertEqual(len(report), 2)
        self.assertEqual(report[0]["label"], "before")
        self.assertEqual(report[0]["diff_percent"], 5.0)
        self.assertEqual(report[1]["label"], "after")
        self.assertEqual(report[1]["diff_percent"], 8.5)

    def test_all_catalog_entries_validate(self):
        """Ensure all 20 catalog entries translate to 100% schema-valid action sequences."""
        from scripts.interactive_board import catalog_to_actions, validate_action_sequence

        catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8")).get("interactions", {})
        for site_id, item in catalog.items():
            actions = catalog_to_actions(site_id, item)
            errors = validate_action_sequence(actions)
            self.assertEqual(errors, [], f"Catalog entry for {site_id} produced invalid actions: {errors}")


if __name__ == "__main__":
    unittest.main()
