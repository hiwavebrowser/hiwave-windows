#!/usr/bin/env python3
"""test_cascade_bench.py — Unit and regression tests for matched-workload cascade benchmark (Package Z2-M1).

Pure Python standard library only (compatible with script-guards CI lane).
Run: python3 scripts/tests/test_cascade_bench.py
"""

from __future__ import annotations

import gzip
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

# Add scripts directory to path
sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import cascade_bench


class TestCascadeBench(unittest.TestCase):

    def test_cat_of(self):
        # Exact mappings
        self.assertEqual(cascade_bench.cat_of("UpdateLayoutTree"), "style")
        self.assertEqual(cascade_bench.cat_of("RecalculateStyles"), "style")
        self.assertEqual(cascade_bench.cat_of("ScheduleStyleRecalculation"), "style")
        self.assertEqual(cascade_bench.cat_of("Layout"), "layout")
        self.assertEqual(cascade_bench.cat_of("UpdateLayerTree"), "layout")
        self.assertEqual(cascade_bench.cat_of("EvaluateScript"), "script")
        self.assertEqual(cascade_bench.cat_of("ParseHTML"), "parse")
        self.assertEqual(cascade_bench.cat_of("Paint"), "paint")
        self.assertEqual(cascade_bench.cat_of("MinorGC"), "gc")

        # Prefix mappings
        self.assertEqual(cascade_bench.cat_of("Document::recalcStyle"), "style")
        self.assertEqual(cascade_bench.cat_of("StyleEngine::update"), "style")
        self.assertEqual(cascade_bench.cat_of("LayoutNG::Block"), "layout")
        self.assertEqual(cascade_bench.cat_of("LocalFrameView::performLayout"), "layout")
        self.assertEqual(cascade_bench.cat_of("V8.GCScavenger"), "gc")
        self.assertEqual(cascade_bench.cat_of("v8.compile"), "script")
        self.assertEqual(cascade_bench.cat_of("HTMLDocumentParser::append"), "parse")
        self.assertEqual(cascade_bench.cat_of("RasterTask::Run"), "paint")

        # Unknown
        self.assertIsNone(cascade_bench.cat_of("CustomUnknownOperation"))

    def test_summarise_chrome_trace_nested_self_time(self):
        # Synthetic trace:
        # CrRendererMain thread has:
        # - Parent task: ts=1000, dur=10000 (10ms)
        #   - UpdateLayoutTree: ts=2000, dur=4000 (4ms, style)
        #   - Layout: ts=7000, dur=3000 (3ms, layout)
        events = [
            {"ph": "M", "name": "thread_name", "pid": 100, "tid": 1, "args": {"name": "CrRendererMain"}},
            {"ph": "M", "name": "thread_name", "pid": 100, "tid": 2, "args": {"name": "ThreadPool"}},
            # Background thread events (must be ignored)
            {"ph": "X", "name": "UpdateLayoutTree", "pid": 100, "tid": 2, "ts": 1000, "dur": 50000},
            # Main thread events
            {
                "ph": "X", "name": "RunTask", "pid": 100, "tid": 1, "ts": 1000, "dur": 10000,
            },
            {
                "ph": "X", "name": "UpdateLayoutTree", "pid": 100, "tid": 1, "ts": 2000, "dur": 4000,
                "args": {"elementCount": 142},
            },
            {
                "ph": "X", "name": "Layout", "pid": 100, "tid": 1, "ts": 7000, "dur": 3000,
                "args": {"beginData": {"dirtyObjects": 18}},
            },
        ]
        summary = cascade_bench.summarise_chrome_trace(events)
        self.assertIsNotNone(summary)
        self.assertEqual(summary["style_recalcs"], 1)
        self.assertEqual(summary["style_elements"], 142)
        self.assertEqual(summary["layout"], 1)
        self.assertEqual(summary["layout_dirty_objects"], 18)

        # Total main thread duration: 10000 us = 10.0 ms
        self.assertEqual(summary["main_thread_busy_ms"], 10.0)

        # Style self-time: 4000 us = 4.0 ms
        self.assertEqual(summary["self_ms"]["style"], 4.0)

        # Layout self-time: 3000 us = 3.0 ms
        self.assertEqual(summary["self_ms"]["layout"], 3.0)

        # Other (RunTask self time): 10000 - 4000 - 3000 = 3000 us = 3.0 ms
        self.assertEqual(summary["self_ms"]["other"], 3.0)

    def test_extract_timings(self):
        stderr = (
            "[INFO  rustkit_engine] Initializing engine\n"
            "\x1b[32m[INFO  rustkit_engine]\x1b[0m Cascade timing parse_ms=15.4 cascade_ms=120.6\n"
            "Some intermediate logs\n"
            "[INFO  rustkit_engine] Cascade timing parse_ms=5.1 cascade_ms=85.2\n"
        )
        timings = cascade_bench.extract_timings(stderr)
        self.assertEqual(len(timings), 2)
        self.assertEqual(timings[0], (15.4, 120.6))
        self.assertEqual(timings[1], (5.1, 85.2))

    def test_rule_a3_compliance(self):
        # Rule A3: legacy baselines and definitions do not move.
        self.assertIn("cnn", cascade_bench.CHROME_STYLE_MS_LEGACY)
        self.assertIn("github", cascade_bench.CHROME_STYLE_MS_LEGACY)
        self.assertIn("wikipedia", cascade_bench.CHROME_STYLE_MS_LEGACY)

        self.assertEqual(cascade_bench.CHROME_STYLE_MS_LEGACY["cnn"], 210.0)
        self.assertEqual(cascade_bench.CHROME_STYLE_MS_LEGACY["github"], 110.0)
        self.assertEqual(cascade_bench.CHROME_STYLE_MS_LEGACY["wikipedia"], 20.0)

        # Test format_table publishes both legacy and matched columns
        results = {
            "wikipedia": {
                "runs": [{"builds": 2, "cascade_ms": 160.0}],
                "median_cascade_ms": 160.0,
                "median_parse_ms": 12.0,
                "chrome_legacy_style_ms": 20.0,
                "chrome_matched_style_ms": 15.0,
                "legacy_ratio": 8.0,
                "matched_ratio": 10.7,
            }
        }
        table = cascade_bench.format_table(results, 1)
        self.assertIn("Chrome legacy ms", table)
        self.assertIn("Chrome matched ms", table)
        self.assertIn("Legacy ratio", table)
        self.assertIn("Matched ratio", table)
        self.assertIn("8.0×", table)
        self.assertIn("10.7×", table)

    def test_load_chrome_trace_json_and_gz(self):
        trace_data = {
            "traceEvents": [
                {"ph": "M", "name": "thread_name", "pid": 1, "tid": 1, "args": {"name": "CrRendererMain"}},
                {"ph": "X", "name": "UpdateLayoutTree", "pid": 1, "tid": 1, "ts": 100, "dur": 5000},
            ]
        }
        with tempfile.TemporaryDirectory() as tmpdir:
            json_file = Path(tmpdir) / "test_trace.json"
            gz_file = Path(tmpdir) / "test_trace.json.gz"

            with open(json_file, "w", encoding="utf-8") as f:
                json.dump(trace_data, f)

            with gzip.open(gz_file, "wt", encoding="utf-8") as f:
                json.dump(trace_data, f)

            res_json = cascade_bench.load_chrome_trace(json_file)
            self.assertIsNotNone(res_json)
            self.assertEqual(res_json["self_ms"]["style"], 5.0)

            res_gz = cascade_bench.load_chrome_trace(gz_file)
            self.assertIsNotNone(res_gz)
            self.assertEqual(res_gz["self_ms"]["style"], 5.0)


if __name__ == "__main__":
    unittest.main()
