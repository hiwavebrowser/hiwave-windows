"""
test_latency_baseline_from_logs.py — guards for tools/latency/baseline_from_logs.py (LAT0).

A synthetic log (no real site, no real URL path) checks that:
1. a pointer stop between a navigation signal and the next document is counted as `leaving`,
   and a watchdog stop on a live document as `live`;
2. sites are hostnames only and nothing printed carries a path or query;
3. relayouts, holds, forced layouts and image drops land on the right site and view;
4. sheet-extraction time is taken from the same view's `Extracted stylesheets` line.
"""

import importlib.util
import io
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
SPEC = importlib.util.spec_from_file_location(
    "baseline_from_logs", REPO / "tools" / "latency" / "baseline_from_logs.py")
tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tool)

E = "\x1b[2m"  # the logs carry ANSI colour codes
LOG = f"""   Compiling something v0.1.0
{E}2026-11-05T02:00:00.000000Z  INFO rustkit_engine: Loading URL id=EngineViewId(2) url=https://a.example/secret/path?token=XYZ replace=false
2026-11-05T02:00:00.100000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-05T02:00:01.000000Z  INFO rustkit_engine: Forced layouts in the script task id=EngineViewId(2) forced_layouts=3
2026-11-05T02:00:01.400000Z  INFO relayout{{id=EngineViewId(2) view_id=EngineViewId(2)}}:relayout{{id=EngineViewId(2)}}:build_layout_tree: rustkit_engine: Extracted stylesheets and CSS variables inline_count=1 external_count=0 css_var_count=5
2026-11-05T02:00:01.500000Z  INFO relayout{{id=EngineViewId(2) view_id=EngineViewId(2)}}:relayout{{id=EngineViewId(2)}}: rustkit_engine: relayout cause=hover_or_focus causes={{hover_or_focus:1}} mutations={{}} tree_reused=false took_ms=200.0 build_ms=50.0 layout_ms=40.0 display_list_ms=1.0 snapshot_ms=0.0 render_ms=0.0 layout_skipped=false dirty_nodes=0 would_path=full full_reason=no_dom_record restyle_miss="a rule reads the pseudo-class beyond the element's subtree" id=EngineViewId(2)
2026-11-05T02:00:01.500100Z  WARN rustkit_engine::wgate: Content thread hold entry=move ms=201 build_ms=50.0 layout_ms=40.0 display_list_ms=1.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=110.0
2026-11-05T02:00:02.000000Z  WARN rustkit_engine: Image over the subresource budget; rendering without it url=https://img.a.example/x.png?sig=1
2026-11-05T02:00:03.000000Z  WARN rustkit_engine: Script stopped at the script budget source=timers elapsed_ms=5005 late_ms=0
2026-11-05T02:00:04.000000Z  INFO hiwave: Navigating content to: https://b.example/next?q=1
2026-11-05T02:00:04.000100Z  WARN rustkit_engine: Script stopped at the script budget source=event:pointermove elapsed_ms=0 late_ms=0
2026-11-05T02:00:04.000200Z  WARN rustkit_engine: Script stopped at the script budget source=event:mouseleave elapsed_ms=0 late_ms=0
2026-11-05T02:00:04.100000Z  WARN rustkit_engine::wgate: Content thread hold entry=leave ms=90 build_ms=80.0 layout_ms=5.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=5.0
2026-11-05T02:00:04.200000Z  INFO rustkit_engine: Loading URL id=EngineViewId(2) url=https://b.example/next?q=1 replace=false
2026-11-05T02:00:04.300000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-05T02:00:05.000000Z  INFO rustkit_core: Navigation finished navigation_id=NavigationId(2) elapsed_ms=800
2026-11-05T02:00:10.000000Z  INFO rustkit_engine: Loading HTML content id=EngineViewId(3) len=100
2026-11-05T02:00:10.100000Z  INFO relayout{{id=EngineViewId(3) view_id=EngineViewId(3)}}:relayout{{id=EngineViewId(3)}}: rustkit_engine: relayout cause=animation causes={{animation:1}} mutations={{}} tree_reused=false took_ms=6.0 build_ms=3.0 layout_ms=2.0 display_list_ms=0.2 snapshot_ms=0.0 render_ms=0.0 layout_skipped=false dirty_nodes=0 would_path=full full_reason=no_dom_record id=EngineViewId(3)
"""


class BaselineFromLogs(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.NamedTemporaryFile("w", suffix=".log", delete=False)
        self.tmp.write(LOG)
        self.tmp.close()

    def test_leaving_and_live_stops(self):
        s = tool.summarize(tool.parse([self.tmp.name]))
        a = s["a.example"]
        self.assertEqual(a["stops_live"], 1)
        self.assertEqual(a["stops_leaving"], 2)
        self.assertEqual(a["stops"]["event:pointermove"]["leaving"], 1)
        self.assertEqual(a["stops"]["timers"]["live"], 1)
        # The leave hold ran on the document being left.
        self.assertEqual(a["holds"]["leave"]["leaving_ms"]["sum"], 90.0)
        # Each pointer stop's proxy sample is the hold that followed it (both stops -> 90 ms).
        self.assertEqual(a["pointer_stop_to_hold_ms"]["n"], 2)
        self.assertEqual(a["pointer_stop_to_hold_ms"]["p50"], 90.0)

    def test_counts_land_on_the_site(self):
        s = tool.summarize(tool.parse([self.tmp.name]))
        a = s["a.example"]
        self.assertEqual(a["forced_layouts"], 3)
        self.assertEqual(a["image_budget_drops"], 1)
        hover = a["relayouts"]["hover_or_focus"]
        self.assertEqual(hover["took"]["n"], 1)
        self.assertEqual(hover["extract"]["p50"], 100.0)  # 01.400 - (01.500 - 0.200)
        self.assertIn("restyle_miss: a rule reads the pseudo-class beyond the element's subtree",
                      hover["reasons"])
        self.assertEqual(s["b.example"]["navigation_finished_ms"]["p50"], 800.0)

    def test_views_split_the_chrome(self):
        s = tool.summarize(tool.parse([self.tmp.name], by_view=True))
        self.assertIn("(html)#v3", s)
        self.assertEqual(s["(html)#v3"]["relayouts"]["animation"]["took"]["n"], 1)
        self.assertNotIn("animation", s["a.example#v2"]["relayouts"])

    def test_output_has_hostnames_only(self):
        out = io.StringIO()
        tool.print_table(tool.summarize(tool.parse([self.tmp.name])), out)
        tool.print_rank(tool.rank(tool.summarize(tool.parse([self.tmp.name]))), out)
        text = out.getvalue()
        for leak in ("secret", "token", "sig=", "?q=", "/next", "https://"):
            self.assertNotIn(leak, text)


if __name__ == "__main__":
    unittest.main()
