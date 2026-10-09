#!/usr/bin/env python3
"""test_interactive_design.py — Guard and validation tests for Package Z2-M3.

Validates the design specification and pinned interaction catalog for the
'interactive' column on the real-site board (Package Z2-M3, PLAN-z.md).

Pure Python standard library only (compatible with CI script-guards).
"""

import json
from pathlib import Path
import sys
import unittest

REPO = Path(__file__).resolve().parent.parent.parent


class TestInteractiveDesign(unittest.TestCase):

    def test_design_document_exists_and_covers_invariants(self):
        """Design doc docs/REAL_SITE_INTERACTIVE_DESIGN_2026-10-03.md must exist and uphold Rule A3."""
        doc_path = REPO / "docs" / "REAL_SITE_INTERACTIVE_DESIGN_2026-10-03.md"
        self.assertTrue(doc_path.exists(), f"Missing design document: {doc_path}")

        content = doc_path.read_text(encoding="utf-8")

        # Invariant 1: Rule A3 citation
        self.assertIn("Rule A3", content)
        self.assertIn("un-scored", content.lower())

        # Invariant 2: Day-7 exit metric reference
        self.assertIn("github.com", content)
        self.assertIn("Exit Metric", content)

        # Invariant 3: Dual verification criteria
        self.assertIn("DOM mutation", content)
        self.assertIn("Visual delta", content)

        # Invariant 4: Outcome taxonomy
        for outcome in ("PASS", "fail", "unstable", "n/a"):
            self.assertIn(outcome, content)

    def test_interaction_catalog_covers_all_top20_sites(self):
        """Catalog websuite/interactions-top20.json must exist and cover 100% of realsite-top20.json."""
        manifest_path = REPO / "websuite" / "interactions-top20.json"
        top20_path = REPO / "websuite" / "realsite-top20.json"

        self.assertTrue(manifest_path.exists(), f"Missing interaction catalog: {manifest_path}")
        self.assertTrue(top20_path.exists(), f"Missing top20 manifest: {top20_path}")

        top20 = json.loads(top20_path.read_text(encoding="utf-8"))
        top20_ids = {s["id"] for s in top20.get("sites", [])}
        self.assertEqual(len(top20_ids), 20)

        catalog = json.loads(manifest_path.read_text(encoding="utf-8"))
        interactions = catalog.get("interactions", {})

        # Every top20 site must have a defined scripted interaction
        missing = top20_ids - set(interactions.keys())
        self.assertEqual(len(missing), 0, f"Missing interactions for sites: {missing}")

        valid_actions = {"click", "focus", "type", "keypress"}
        valid_response_types = {"dom_mutation", "visual_delta", "focus_state", "class_toggle"}

        for site_id in top20_ids:
            entry = interactions[site_id]
            self.assertIn("target_selector", entry, f"Missing target_selector for {site_id}")
            self.assertTrue(len(entry["target_selector"]) > 0)

            self.assertIn("action", entry, f"Missing action for {site_id}")
            self.assertIn(entry["action"], valid_actions, f"Invalid action for {site_id}: {entry['action']}")

            self.assertIn("expected_response", entry, f"Missing expected_response for {site_id}")
            self.assertTrue(len(entry["expected_response"]) > 0)

            self.assertIn("response_type", entry, f"Missing response_type for {site_id}")
            self.assertIn(
                entry["response_type"],
                valid_response_types,
                f"Invalid response_type for {site_id}: {entry['response_type']}",
            )

    def test_github_interaction_matches_day7_exit_criteria(self):
        """GitHub interaction must specifically exercise opening the search box."""
        manifest_path = REPO / "websuite" / "interactions-top20.json"
        self.assertTrue(manifest_path.exists())

        catalog = json.loads(manifest_path.read_text(encoding="utf-8"))
        gh = catalog.get("interactions", {}).get("github")
        self.assertIsNotNone(gh)
        self.assertIn("search", gh["target_selector"].lower())
        self.assertIn("search", gh["expected_response"].lower())


if __name__ == "__main__":
    unittest.main()
