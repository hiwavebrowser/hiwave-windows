"""
test_latency_baseline_from_logs.py — guards for tools/latency/baseline_from_logs.py (LAT0).

A synthetic log (no real site, no real URL path) checks that:
1. a pointer stop between a navigation signal and the next document is counted as `leaving`,
   and a watchdog stop on a live document as `live`;
2. sites are hostnames only and nothing printed carries a path or query;
3. relayouts, holds, forced layouts and image drops land on the right site and view;
4. sheet-extraction time is taken from the same view's `Extracted stylesheets` line;
5. (LAT1) the per-visit `Content thread holds n=...` summary lines, merged with the logged
   holds of 50 ms or more, give the exact hold p95 over all entries when it is 50 ms or more
   and "under 50" when it is not; the relayout line's `extract_ms`, the forced-layout `ms`
   and the `Input to present` windows feed the exit metric.
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

# LAT1: two visits. a.example: a full window of 100 entries and a 20-entry visit-end window,
# with three logged holds of 50 ms or more (fewer than the 7 above rank 114 of 120): p95 under 50.
# b.example: 20 entries, three logged holds (60, 70, 400): rank 19 of 20 is the 70.
LAT1_LOG = f"""{E}2026-11-06T10:00:00.000000Z  INFO rustkit_engine: Loading URL id=EngineViewId(1) url=https://a.example/lat1path?x=1 replace=false
2026-11-06T10:00:00.100000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-06T10:00:01.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=move ms=120 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=120.0
2026-11-06T10:00:02.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=move ms=80 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=80.0
2026-11-06T10:00:03.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=live_turn ms=300 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=300.0 other_ms=0.0
2026-11-06T10:00:04.000000Z  INFO rustkit_engine::wgate: Content thread holds n=100 p50_ms=2.0 p95_ms=40.0 max_ms=300.0 over_50=3 by_entry=move:90:30.0,live_turn:10:300.0 at=every_100
2026-11-06T10:00:05.000000Z  INFO rustkit_engine: Forced layouts in the script task id=EngineViewId(1) forced_layouts=3 ms=45.5 cause=style:offset_width causes={{style:offset_width:3}}
2026-11-06T10:00:05.500000Z  INFO relayout{{id=EngineViewId(1)}}: rustkit_engine: relayout cause=timer causes={{timer:1}} mutations={{}} tree_reused=false took_ms=30.0 build_ms=10.0 layout_ms=5.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 extract_ms=12.5 layout_skipped=false dirty_nodes=0 would_path=full full_reason=no_dom_record id=EngineViewId(1)
2026-11-06T10:00:06.000000Z  INFO rustkit_engine::presenter: Input to present p50_ms=10 p95_ms=80 max_ms=90 n=100
2026-11-06T10:00:07.000000Z  INFO rustkit_engine::presenter: Input to present p50_ms=12 p95_ms=120 max_ms=200 n=100
2026-11-06T10:01:00.000000Z  INFO rustkit_engine::wgate: Content thread holds n=20 p50_ms=1.0 p95_ms=3.0 max_ms=4.0 over_50=0 by_entry=move:20:3.0 at=navigation
2026-11-06T10:01:00.000100Z  INFO rustkit_engine: Loading URL id=EngineViewId(1) url=https://b.example/lat1other?y=2 replace=false
2026-11-06T10:01:00.100000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-06T10:01:01.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=click ms=60 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=60.0
2026-11-06T10:01:02.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=click ms=70 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=70.0
2026-11-06T10:01:03.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=live_turn ms=400 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=400.0
2026-11-06T10:01:30.000000Z  INFO rustkit_engine::wgate: Content thread holds n=20 p50_ms=5.0 p95_ms=70.0 max_ms=400.0 over_50=3 by_entry=click:12:70.0,live_turn:8:400.0 at=drop
"""


class Lat1HoldSummary(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.NamedTemporaryFile("w", suffix=".log", delete=False)
        self.tmp.write(LAT1_LOG)
        self.tmp.close()
        self.s = tool.summarize(tool.parse([self.tmp.name]))

    def test_summary_lines_count_every_entry_by_kind(self):
        a = self.s["a.example"]
        self.assertEqual(a["hold_entries"]["n"], 120)
        self.assertEqual(a["hold_entries"]["by_entry"], {"move": 110, "live_turn": 10})
        self.assertEqual(self.s["b.example"]["hold_entries"]["by_entry"], {"click": 12, "live_turn": 8})

    def test_hold_p95_is_exact_at_or_over_the_threshold_and_under_it_otherwise(self):
        a = self.s["a.example"]["exit_metric"]
        self.assertIsNone(a["hold_p95_ms"])
        self.assertTrue(a["hold_p95_under_threshold"])
        b = self.s["b.example"]["exit_metric"]
        self.assertEqual(b["hold_p95_ms"], 70.0)
        self.assertFalse(b["hold_p95_under_threshold"])
        self.assertFalse(b["pass"])

    def test_extract_forced_ms_and_input_to_present_feed_the_metric(self):
        a = self.s["a.example"]
        self.assertEqual(a["forced_layout_ms"], 45.5)
        self.assertEqual(a["relayouts"]["timer"]["extract"]["p50"], 12.5)
        m = a["exit_metric"]
        self.assertEqual(m["input_to_present_p95_worst_ms"], 120.0)
        self.assertEqual(m["stops_live_per_min"], 0.0)
        # Holds pass, input to present fails (120 > 100): the site fails.
        self.assertFalse(m["pass"])

    def test_no_summary_line_means_no_hold_metric(self):
        tmp = tempfile.NamedTemporaryFile("w", suffix=".log", delete=False)
        tmp.write(LOG)
        tmp.close()
        m = tool.summarize(tool.parse([tmp.name]))["a.example"]["exit_metric"]
        self.assertIsNone(m["hold_p95_ms"])
        self.assertIsNone(m["hold_p95_under_threshold"])
        self.assertIsNone(m["pass"])

    def test_printed_metric_has_hostnames_only(self):
        out = io.StringIO()
        tool.print_table(self.s, out)
        text = out.getvalue()
        self.assertIn("exit metric", text)
        for leak in ("?x=", "?y=", "lat1path", "lat1other", "https://"):
            self.assertNotIn(leak, text)


# LAT1b: a headless capture on the window thread: the load and one render, summarised at the
# engine's drop. That is no content visit; its p95 is the whole-load hold, so it gives no metric.
DROP_ONLY_LOG = f"""{E}2026-11-07T10:00:00.000000Z  INFO rustkit_engine: Loading URL id=EngineViewId(1) url=https://d.example/dpath?z=1 replace=false
2026-11-07T10:00:00.100000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-07T10:00:09.000000Z  WARN rustkit_engine::wgate: Window thread hold entry=load ms=9000 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=9000.0
2026-11-07T10:00:09.500000Z  INFO rustkit_engine::wgate: Window thread holds n=2 p50_ms=12.0 p95_ms=9000.0 max_ms=9000.0 over_50=1 by_entry=load:1:9000.0,render_view:1:12.0 at=drop
"""

# LAT1b: the per-visit lines as the content thread writes them, tagged with view and host. The
# a.example visit's line lands after the window has already logged the next load (another
# thread), so the tag, not the log position, names the site. a.example: 100 + 30 entries, logged
# holds 300, 200, 120, 90, 80, 70, 60 (7 = 130 - 124 + 1): rank 124 of 130 is the 60.
# b.example: 10 entries, none logged: under 50. The window thread's drop line is ignored.
TAGGED_LOG = f"""{E}2026-11-07T11:00:00.000000Z  INFO rustkit_engine: Loading URL id=EngineViewId(4) url=https://a.example/tpath?t=1 replace=false
2026-11-07T11:00:00.100000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-07T11:00:01.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=load ms=300 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=300.0
2026-11-07T11:00:02.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=live_turn ms=200 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=200.0 other_ms=0.0
2026-11-07T11:00:03.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=move ms=120 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=120.0
2026-11-07T11:00:04.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=move ms=90 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=90.0
2026-11-07T11:00:05.000000Z  INFO rustkit_engine::wgate: Content thread holds view=4 host=a.example n=100 p50_ms=2.0 p95_ms=90.0 max_ms=300.0 over_50=4 by_entry=move:95:20.0,live_turn:4:200.0,load:1:300.0 at=every_100
2026-11-07T11:00:06.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=move ms=80 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=80.0
2026-11-07T11:00:07.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=click ms=70 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=70.0
2026-11-07T11:00:08.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=click ms=60 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=60.0
2026-11-07T11:01:00.000000Z  INFO hiwave: Navigating content to: https://b.example/next?q=1
2026-11-07T11:01:00.000100Z  INFO rustkit_engine: Loading URL id=EngineViewId(4) url=https://b.example/next?q=1 replace=false
2026-11-07T11:01:00.000200Z  INFO rustkit_engine::wgate: Content thread holds view=4 host=a.example n=30 p50_ms=1.0 p95_ms=70.0 max_ms=80.0 over_50=3 by_entry=move:20:80.0,click:10:70.0 at=navigation
2026-11-07T11:01:00.100000Z  INFO rustkit_engine: Running page script source=inline#1 bytes=10
2026-11-07T11:01:30.000000Z  INFO rustkit_engine::wgate: Content thread holds view=4 host=b.example n=10 p50_ms=3.0 p95_ms=9.0 max_ms=9.0 over_50=0 by_entry=load:1:9.0,move:9:4.0 at=drop
2026-11-07T11:01:30.500000Z  INFO rustkit_engine::wgate: Window thread holds n=2 p50_ms=12.0 p95_ms=900.0 max_ms=900.0 over_50=1 by_entry=load_html:1:900.0,render_view:1:12.0 at=drop
"""


# LAT1b: a capture logged at warn plus the wgate lines: no `Loading URL` line, so a hold line has no
# site by its log position. The tagged summary lists its entries of 50 ms or more itself.
UNPLACED_LOG = f"""{E}2026-11-07T12:00:01.000000Z  WARN rustkit_engine::wgate: Content thread hold entry=load ms=1200 build_ms=0.0 layout_ms=0.0 display_list_ms=0.0 snapshot_ms=0.0 render_ms=0.0 script_ms=0.0 other_ms=1200.0
2026-11-07T12:00:01.500000Z  INFO rustkit_engine::wgate: Content thread holds view=1 host=c.example n=2 p50_ms=0.5 p95_ms=1200.3 max_ms=1200.3 over_50=1 over_50_ms=1200.3 by_entry=load:1:1200.3,render_view:1:0.5 at=drop
"""


def _summary(text, **kw):
    tmp = tempfile.NamedTemporaryFile("w", suffix=".log", delete=False)
    tmp.write(text)
    tmp.close()
    return tool.summarize(tool.parse([tmp.name], **kw))


class Lat1bPerVisitSummary(unittest.TestCase):
    def test_a_drop_line_only_gives_no_hold_metric(self):
        s = _summary(DROP_ONLY_LOG)
        m = s["d.example"]["exit_metric"]
        self.assertIsNone(m["hold_thread"])
        self.assertEqual(m["hold_n"], 0)
        self.assertIsNone(m["hold_p95_ms"])
        self.assertIsNone(m["hold_p95_under_threshold"])
        self.assertIsNone(m["pass"])
        out = io.StringIO()
        tool.print_exit_metric(s, out)
        row = [l for l in out.getvalue().splitlines() if l.startswith("d.example")]
        self.assertEqual(len(row), 1, out.getvalue())
        self.assertIn("n/a", row[0])
        self.assertNotIn("NO", row[0])

    def test_tagged_lines_group_by_host_and_compute_p95(self):
        s = _summary(TAGGED_LOG)
        a = s["a.example"]
        self.assertEqual(a["hold_entries"]["thread"], "Content")
        self.assertEqual(a["hold_entries"]["n"], 130)
        self.assertEqual(a["hold_entries"]["by_entry"], {"move": 115, "live_turn": 4, "load": 1, "click": 10})
        m = a["exit_metric"]
        self.assertEqual(m["hold_p95_ms"], 60.0)
        self.assertFalse(m["hold_p95_under_threshold"])
        self.assertFalse(m["pass"])
        b = s["b.example"]
        self.assertEqual(b["hold_entries"]["n"], 10)
        self.assertTrue(b["exit_metric"]["hold_p95_under_threshold"])
        self.assertTrue(b["exit_metric"]["pass"])

    def test_a_tagged_line_carries_its_own_long_entries(self):
        m = _summary(UNPLACED_LOG)["c.example"]["exit_metric"]
        self.assertEqual(m["hold_n"], 2)
        self.assertEqual(m["hold_p95_ms"], 1200.3)
        self.assertFalse(m["pass"])

    def test_tagged_lines_split_by_view(self):
        s = _summary(TAGGED_LOG, by_view=True)
        self.assertEqual(s["a.example#v4"]["hold_entries"]["n"], 130)
        self.assertEqual(s["b.example#v4"]["hold_entries"]["n"], 10)


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
