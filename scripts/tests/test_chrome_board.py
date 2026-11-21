"""
test_chrome_board.py — guards for the chrome board runner (CH3.A1).

1. A case whose fixture is missing is refused before anything is captured.
   A board row rendered without its state would score an empty page and
   call the chrome green.
2. Chromium and RustKit run the SAME action list: both commands name one
   actions file and that file holds the state's actions, verbatim. Two
   action lists would compare two different states and call the delta an
   engine defect.

Run: python3 scripts/tests/test_chrome_board.py
"""
import json
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "scripts"))

BOARD_PATH = REPO / "tools" / "parity_oracle" / "chrome_fixtures" / "board.json"


class TestChromeBoard(unittest.TestCase):
    def test_board_matrix_names_pages_states_and_sizes(self):
        board = json.loads(BOARD_PATH.read_text(encoding="utf-8"))
        self.assertEqual(sorted(board["pages"]), ["chrome", "inspector", "report", "settings", "shelf"])
        self.assertEqual(board["sizes"], [[1280, 800], [1024, 700], [1600, 1000]])
        for page, spec in board["pages"].items():
            self.assertTrue((REPO / spec["html"]).exists(), f"{page}: html {spec['html']} missing")
            for state in spec["states"]:
                self.assertIn(state, board["states"], f"{page}: state {state} not defined")

    def test_a_case_whose_fixture_is_missing_is_refused_before_any_capture(self):
        from scripts import chrome_board

        board = json.loads(BOARD_PATH.read_text(encoding="utf-8"))
        board["pages"]["chrome"]["fixture"] = "does-not-exist.js"
        with tempfile.TemporaryDirectory() as tmp:
            bad = Path(tmp) / "board.json"
            bad.write_text(json.dumps(board), encoding="utf-8")
            with self.assertRaises(chrome_board.BoardError) as ctx:
                chrome_board.plan(chrome_board.load_board(bad), REPO, Path(tmp) / "work")
            self.assertIn("chrome", str(ctx.exception))
            self.assertIn("does-not-exist.js", str(ctx.exception))

            captured = []
            rc = chrome_board.main(
                ["--parity-capture", "/nonexistent/parity-capture", "--board", str(bad),
                 "--work", str(Path(tmp) / "work"), "--out", str(Path(tmp) / "out")],
                capture=lambda *a, **k: captured.append(a),
            )
            self.assertNotEqual(rc, 0, "a missing fixture is a refusal, not a green board")
            self.assertEqual(captured, [], "nothing is captured once a fixture is missing")
            self.assertFalse((Path(tmp) / "out" / "BOARD.md").exists(), "no board is written")

    def test_a_page_this_platform_does_not_ship_is_named_not_refused(self):
        # hiwave-windows ships no Inspector page: its absent html refused the
        # whole board, even with --only. A page whose board.json entry lists this
        # platform in `not_shipped_on` is now left out of the plan (not scored,
        # not green) and named. Any OTHER absent html (a typo, a rename) is still
        # a refusal (R1 nit), as is a missing fixture (test above).
        from scripts import chrome_board

        board = json.loads(BOARD_PATH.read_text(encoding="utf-8"))
        board["pages"]["inspector"]["html"] = "crates/hiwave-app/src/ui/not-shipped-here.html"
        board["pages"]["inspector"]["not_shipped_on"] = ["windows"]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "board.json"
            path.write_text(json.dumps(board), encoding="utf-8")
            loaded = chrome_board.load_board(path)
            loaded["_dir"] = str(BOARD_PATH.parent)  # the real fixtures; only the html moved
            work = Path(tmp) / "work"
            self.assertEqual(chrome_board.pages_not_shipped(loaded, REPO, platform="windows"), ["inspector"])
            rows = chrome_board.plan(loaded, REPO, work, platform="windows")
            self.assertEqual(sorted({r.page for r in rows}), ["chrome", "report", "settings", "shelf"])
            only = chrome_board.plan(loaded, REPO, work, only="settings", platform="windows")
            self.assertEqual({r.page for r in only}, {"settings"})
            census = Path(tmp) / "census.json"
            census.write_text(json.dumps({"red": [], "properties": [], "meta": {}}), encoding="utf-8")
            red = chrome_board.census_red_lists(loaded, REPO, census, platform="windows")
            self.assertNotIn("inspector", red["pages"])
            # --only on the page not shipped here is a clear refusal, not an empty board.
            with self.assertRaises(chrome_board.BoardError) as ctx:
                chrome_board.plan(loaded, REPO, work, only="inspector", platform="windows")
            self.assertIn("not shipped on windows", str(ctx.exception))
            # The same absence on a platform the page is NOT declared absent on is a refusal.
            with self.assertRaises(chrome_board.BoardError) as ctx:
                chrome_board.plan(loaded, REPO, work, platform="macos")
            self.assertIn("inspector: html", str(ctx.exception))

    def test_an_undeclared_absent_page_is_still_refused(self):
        # R1 nit: a renamed or typo'd html path must not drop a page silently.
        from scripts import chrome_board

        board = json.loads(BOARD_PATH.read_text(encoding="utf-8"))
        board["pages"]["settings"]["html"] = "crates/hiwave-app/src/ui/setings.html"
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "board.json"
            path.write_text(json.dumps(board), encoding="utf-8")
            loaded = chrome_board.load_board(path)
            loaded["_dir"] = str(BOARD_PATH.parent)
            for platform in ("macos", "windows", "linux"):
                self.assertEqual(chrome_board.pages_not_shipped(loaded, REPO, platform=platform), [])
                with self.assertRaises(chrome_board.BoardError) as ctx:
                    chrome_board.plan(loaded, REPO, Path(tmp) / "work", platform=platform)
                self.assertIn("settings: html crates/hiwave-app/src/ui/setings.html missing", str(ctx.exception))

    def test_the_board_declares_the_inspector_not_shipped_on_windows(self):
        board = json.loads(BOARD_PATH.read_text(encoding="utf-8"))
        self.assertEqual(board["pages"]["inspector"].get("not_shipped_on"), ["windows"])
        others = {p: s.get("not_shipped_on") for p, s in board["pages"].items() if p != "inspector"}
        self.assertEqual(others, {p: None for p in others})

    def test_the_board_names_a_page_not_shipped_here(self):
        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            chrome_board.write_board(out, {
                "run": {"at": "now", "sha": "f", "base": "b", "chromium": "148.0.7778.216",
                        "chromium_path": "~/c", "chromium_pinned": "148.0.7778.216", "parity_capture": "pc", "gate_pct": 0.5},
                "sizes": [], "states": {}, "rows": [], "census": {},
                "not_shipped": {"inspector": "crates/hiwave-app/src/ui/inspector.html"},
            })
            md = (out / "BOARD.md").read_text(encoding="utf-8")
            self.assertIn("Not shipped here", md)
            self.assertIn("inspector", md)
            self.assertIn("| inspector | 0 | 0 | not shipped here |", md)

    def test_chromium_and_rustkit_run_the_same_action_list(self):
        from scripts import chrome_board

        board = chrome_board.load_board(BOARD_PATH)
        with tempfile.TemporaryDirectory() as tmp:
            rows = chrome_board.plan(board, REPO, Path(tmp) / "work")
            self.assertEqual(len(rows), 14 * 3, "page x state x size")
            seen_states = set()
            for row in rows:
                chromium = chrome_board.chromium_command(row)
                rustkit = chrome_board.rustkit_command(row, Path("/bin/parity-capture"))
                self.assertIn(str(row.actions_path), chromium)
                self.assertIn("--actions", rustkit)
                self.assertEqual(rustkit[rustkit.index("--actions") + 1], str(row.actions_path))
                self.assertIn("--run-inline-scripts", rustkit, "the page's own script runs in RustKit too")
                on_disk = json.loads(row.actions_path.read_text(encoding="utf-8"))
                self.assertEqual(on_disk, board["states"][row.state], f"{row.page}/{row.state}: actions differ")
                self.assertEqual([str(row.width), str(row.height)], [rustkit[rustkit.index("--width") + 1], rustkit[rustkit.index("--height") + 1]])
                self.assertIn(str(row.width), chromium)
                self.assertIn(str(row.height), chromium)
                seen_states.add(row.state)
            self.assertIn("hover_tab2", seen_states)
            self.assertIn("focus_mode", seen_states)

    # REPORT-CHART-0x0: a page whose script resizes from a ResizeObserver through an animation
    # frame (report.html's charts) needs RustKit's live loop after the load and after the actions,
    # long enough for Chart.js's 1 s animation to finish. A page's `live_ms` asks for it; a page
    # without one turns none. report.html's is 1500 (with #1365, Chrome's arm draws the charts).
    def test_a_page_with_live_ms_turns_the_live_loop_after_the_load_and_the_actions(self):
        from scripts import chrome_board

        board = json.loads(BOARD_PATH.read_text(encoding="utf-8"))
        self.assertEqual(board["pages"]["report"].get("live_ms"), 1500)
        board["pages"]["report"]["live_ms"] = 100
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "board.json"
            path.write_text(json.dumps(board), encoding="utf-8")
            board_dir = Path(chrome_board.load_board(BOARD_PATH)["_dir"])
            loaded = chrome_board.load_board(path)
            loaded["_dir"] = str(board_dir)
            rows = chrome_board.plan(loaded, REPO, Path(tmp) / "work")
            for row in rows:
                rustkit = chrome_board.rustkit_command(row, Path("/bin/parity-capture"))
                if row.page == "report":
                    self.assertEqual(rustkit[rustkit.index("--live-ms") + 1], "100", row.case_id)
                    self.assertEqual(rustkit[rustkit.index("--actions-live-ms") + 1], "100", row.case_id)
                else:
                    self.assertNotIn("--live-ms", rustkit, row.case_id)
                    self.assertNotIn("--actions-live-ms", rustkit, row.case_id)

    def test_the_composed_page_carries_the_stub_before_and_the_fixture_after_the_page_script(self):
        from scripts import chrome_board

        board = chrome_board.load_board(BOARD_PATH)
        with tempfile.TemporaryDirectory() as tmp:
            rows = chrome_board.plan(board, REPO, Path(tmp) / "work")
            chrome_rows = [r for r in rows if r.page == "chrome"]
            html = chrome_rows[0].html_path.read_text(encoding="utf-8")
            stub = html.index("window.__hiwaveBoard = {")
            page_script = html.index("window.hiwaveChrome = {")
            fixture = html.index("chrome.renderTabs(")
            self.assertLess(stub, page_script, "the IPC stub precedes the page's script")
            self.assertLess(page_script, fixture, "the fixture follows the page's script")
            self.assertIn('data-chrome-board="freeze"', html, "both engines see the oracle's transition freeze")
            report = [r for r in rows if r.page == "report"][0].html_path.read_text(encoding="utf-8")
            self.assertNotIn("<!-- Chart.js is injected by Rust before page load -->", report)

    # #898 R1 (Prometheus): the board is committed, so a row's notes carry
    # shapes (arm, step, status, error class, length), never error text — a
    # page error or a stderr tail can quote a value.
    def test_row_notes_carry_shapes_not_error_text(self):
        from scripts import chrome_board

        row = chrome_board.Row(page="chrome", state="default", width=10, height=10,
                               html_path=Path("x.html"), actions_path=Path("a.json"), out_dir=Path("o"))
        result = {
            "status": "chromium_failed",
            "chromium": {"status": "error", "error": "Error: token=abc123 at file.mjs:5",
                         "page_errors": ["TypeError: Cannot read properties of undefined (reading 'secretField')"],
                         "fixture_errors": ["get_shelf: ReferenceError: hidden-value is not defined"],
                         "action_results": [{"step": 0, "type": "click", "status": "error", "error": "selector #urlInput?q=abc"}]},
            "rustkit": {"status": "error", "error": "Inline script failed: value=xyz"},
        }
        entry = chrome_board.summarize(row, result)
        blob = json.dumps(entry)
        for leaked in ("abc123", "secretField", "hidden-value", "q=abc", "value=xyz"):
            self.assertNotIn(leaked, blob, f"error text leaked into committed notes: {leaked}")
        self.assertTrue(any("chromium" in n and "error" in n for n in entry["notes"]), entry["notes"])
        self.assertTrue(any("pageerror" in n and "TypeError" in n for n in entry["notes"]), entry["notes"])
        self.assertTrue(any("fixture" in n and "get_shelf" in n for n in entry["notes"]), entry["notes"])

    def test_board_header_names_the_head_and_its_develop_base(self):
        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            chrome_board.write_board(out, {
                "run": {"at": "now", "sha": "feedface", "base": "0badf00d", "chromium": "143", "parity_capture": "pc", "gate_pct": 0.5},
                "sizes": [], "states": {}, "rows": [], "census": {},
            })
            header = (out / "BOARD.md").read_text(encoding="utf-8").splitlines()[2]
            self.assertIn("head `feedface`", header)
            self.assertIn("develop `0badf00d`", header)
            self.assertNotIn("develop `feedface`", header)

    def test_an_unset_or_missing_chrome_pin_is_refused(self):
        # The board once scored against Playwright's bundled Chromium 143
        # because PARITY_CHROME_PATH was unset; that fallback is a refusal.
        from scripts import chrome_board

        with self.assertRaises(chrome_board.BoardError):
            chrome_board.pinned_chrome_path({})
        with self.assertRaises(chrome_board.BoardError):
            chrome_board.pinned_chrome_path({"PARITY_CHROME_PATH": "/nonexistent/chrome-for-testing"})
        with tempfile.TemporaryDirectory() as tmp:
            pin = {"PARITY_CHROME_PATH": tmp}
            self.assertEqual(chrome_board.pinned_chrome_path(pin, version_of=lambda p: "148.0.7778.216"), Path(tmp))
            # The path existing is not enough: the binary must be exactly the pin.
            for other in ("143.0.7499.4", "148.0.7778.215", None):
                with self.assertRaises(chrome_board.BoardError):
                    chrome_board.pinned_chrome_path(pin, version_of=lambda p, v=other: v)

    def test_the_pin_probe_reads_the_cft_manifest_when_version_prints_nothing(self):
        # On Windows `chrome.exe --version` prints nothing, so the probe read no
        # version and the board refused the pinned binary itself. Chrome for
        # Testing ships `<version>.manifest` beside chrome.exe; the probe falls
        # back to it (and still returns None when there is no such file).
        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp) / "chrome.exe"  # never runs: the probe's run fails, as a silent one would
            self.assertIsNone(chrome_board.probe_chrome_version(binary, windows=True))
            (Path(tmp) / "148.0.7778.216.manifest").write_text("<assembly/>", encoding="utf-8")
            (Path(tmp) / "chrome.VisualElementsManifest.xml").write_text("<x/>", encoding="utf-8")
            self.assertEqual(chrome_board.probe_chrome_version(binary, windows=True), "148.0.7778.216")
            # Windows only (R1 nit): elsewhere a silent --version stays None.
            self.assertIsNone(chrome_board.probe_chrome_version(binary, windows=False))
            # Two versioned manifests are ambiguous (R1 nit: a sort would pick 100.x
            # before 99.x): no version, so the pin check refuses rather than guesses.
            (Path(tmp) / "100.0.1.2.manifest").write_text("<assembly/>", encoding="utf-8")
            self.assertIsNone(chrome_board.probe_chrome_version(binary, windows=True))

    def test_main_refuses_without_the_pin_before_any_capture(self):
        import os
        from unittest import mock
        from scripts import chrome_board

        env = {k: v for k, v in os.environ.items() if k != "PARITY_CHROME_PATH"}
        with mock.patch.dict(os.environ, env, clear=True), \
                mock.patch.object(chrome_board, "capture", side_effect=AssertionError("captured")):
            self.assertEqual(chrome_board.main(["--parity-capture", "/bin/sh", "--only", "shelf"]), 2)

    def test_a_row_that_reports_no_browser_version_is_refused(self):
        import os
        from unittest import mock
        from scripts import chrome_board

        for chromium in ({"status": "ok"}, {"status": "ok", "browser": "143.0.7499.4"}):
            result = {"status": "ok", "chromium": chromium, "rustkit": {"status": "ok"},
                      "pixels": {"diffPercent": 0.1, "top": []},
                      "geometry": {"geometry_failures": 0, "join_failures": 0, "worst_selectors": []}}
            with tempfile.TemporaryDirectory() as tmp, \
                    mock.patch.dict(os.environ, {"PARITY_CHROME_PATH": sys.executable}):
                rc = chrome_board.main(
                    ["--parity-capture", sys.executable, "--only", "shelf",
                     "--work", str(Path(tmp) / "work"), "--out", str(Path(tmp) / "out")],
                    capture=lambda *a, **k: result, chrome_version=lambda p: "148.0.7778.216")
                self.assertEqual(rc, 2, chromium)
                self.assertFalse((Path(tmp) / "out" / "board.json").exists())

    def test_census_section_names_the_census_oracle_when_it_is_not_the_boards(self):
        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            chrome_board.write_board(out, {
                "run": {"at": "now", "sha": "f", "base": "b", "chromium": "148.0.7778.216",
                        "chromium_path": "~/c", "chromium_pinned": "148.0.7778.216", "parity_capture": "pc", "gate_pct": 0.5},
                "sizes": [], "states": {}, "rows": [],
                "census": {"source": "docs/census/CSS_PROPERTY_CENSUS_2026-11-03.json", "date": "2026-11-03",
                           "oracle": "143.0.7499.4", "red_total": 1, "pages": {}},
            })
            md = (out / "BOARD.md").read_text(encoding="utf-8")
            self.assertIn("Census: the 2026-11-03 Chromium 143 census", md)
            self.assertIn("NOT this board's oracle (148.0.7778.216)", md)

    def test_board_header_names_the_full_pinned_oracle_version(self):
        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            chrome_board.write_board(out, {
                "run": {"at": "now", "sha": "feedface", "base": "0badf00d", "chromium": "148.0.7778.216",
                        "chromium_path": "/cft/chrome", "chromium_pinned": "148.0.7778.216",
                        "parity_capture": "pc", "gate_pct": 0.5},
                "sizes": [], "states": {}, "rows": [], "census": {},
            })
            header = (out / "BOARD.md").read_text(encoding="utf-8").splitlines()[2]
            self.assertIn("oracle Chrome for Testing 148.0.7778.216 (PARITY_CHROME_PATH)", header)
            self.assertIn("metrics start on 148 from 2026-11-17; earlier rows were measured against Chromium 143",
                          (out / "BOARD.md").read_text(encoding="utf-8"))
        self.assertIn("unpinned", chrome_board.oracle_text({"chromium": "143.0.7499.4"}))
        self.assertIn("NOT the pin", chrome_board.oracle_text(
            {"chromium": "149.0.1.2", "chromium_path": "/x", "chromium_pinned": "148.0.7778.216"}))

    def test_census_source_names_a_committed_report_not_a_build_dir_file(self):
        from scripts import chrome_board

        board = chrome_board.load_board(BOARD_PATH)
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp) / "target" / "chrome_board" / "census"
            work.mkdir(parents=True)
            census = work / "CSS_PROPERTY_CENSUS_2026-11-05.json"
            census.write_text(json.dumps({"meta": {"date": "2026-11-05", "oracle": "143"}, "red": ["display"],
                                          "properties": [{"name": "display", "class": "effect", "sites": 1}]}),
                              encoding="utf-8")
            info = chrome_board.census_red_lists(board, REPO, census)
            self.assertNotIn("target/", info["source"])
            self.assertEqual(info["report"], "trench/chrome/census/CSS_PROPERTY_CENSUS_2026-11-05.md")
            self.assertEqual([h["property"] for h in info["pages"]["chrome"]["red"]], ["display"])


def _row(pixel, geo, join=0, status="ok"):
    return {"page": "shelf", "state": "default", "size": "1280x800", "width": 1280, "height": 800,
            "status": status, "pixel_diff_pct": pixel, "geometry_diffs": geo, "join_failures": join,
            "top": [], "notes": []}


class GateTests(unittest.TestCase):
    """Pete 2026-11-06: green at <= 0.5 % pixels, OR at 0 geometry diffs and
    <= 1.5 % pixels. Geometry that matches Chrome leaves only rasterisation."""

    def test_constants(self):
        from scripts import chrome_board

        self.assertEqual(chrome_board.PIXEL_GATE_PCT, 0.5)
        self.assertEqual(chrome_board.GEOMETRY_PIXEL_GATE_PCT, 1.5)

    def test_half_a_percent_is_green_whatever_the_geometry(self):
        from scripts import chrome_board

        self.assertTrue(chrome_board.row_is_green(_row(0.5, 40)))
        self.assertEqual(chrome_board.green_rule(_row(0.5, 40)), "pixel")
        self.assertTrue(chrome_board.row_is_green(_row(0.3, None)))

    def test_zero_geometry_and_1_2_pct_is_green(self):
        from scripts import chrome_board

        self.assertTrue(chrome_board.row_is_green(_row(1.2, 0)))
        self.assertEqual(chrome_board.green_rule(_row(1.2, 0)), "geometry")
        self.assertTrue(chrome_board.row_is_green(_row(1.5, 0)), "1.5 % is inclusive")

    def test_zero_geometry_and_1_6_pct_is_red(self):
        from scripts import chrome_board

        self.assertFalse(chrome_board.row_is_green(_row(1.6, 0)))
        self.assertIsNone(chrome_board.green_rule(_row(1.6, 0)))

    def test_three_geometry_diffs_and_1_pct_is_red(self):
        from scripts import chrome_board

        self.assertFalse(chrome_board.row_is_green(_row(1.0, 3)))

    def test_geometry_not_run_and_1_pct_is_red(self):
        from scripts import chrome_board

        self.assertFalse(chrome_board.row_is_green(_row(1.0, None, join=None)))

    def test_a_failed_join_is_not_matching_geometry(self):
        from scripts import chrome_board

        self.assertFalse(chrome_board.row_is_green(_row(1.0, 0, join=2)))
        self.assertTrue(chrome_board.row_is_green(_row(0.4, 0, join=2)), "the pixel rule alone still holds")

    def test_an_unscored_row_is_never_green(self):
        from scripts import chrome_board

        self.assertFalse(chrome_board.row_is_green(_row(None, None, join=None, status="rustkit_failed")))

    def test_board_names_both_rules_and_the_rule_per_row(self):
        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            chrome_board.write_board(out, {
                "run": {"at": "now", "sha": "feedface", "base": "0badf00d", "chromium": "143", "parity_capture": "pc",
                        "gate_pct": 0.5, "gate_geometry_pixel_pct": 1.5},
                "sizes": [], "states": {},
                "rows": [_row(0.2, 5), _row(1.2, 0), _row(1.6, 0), _row(1.0, 3)],
                "census": {},
            })
            md = (out / "BOARD.md").read_text(encoding="utf-8")
            self.assertIn("**2/4 rows green**", md)
            self.assertIn("0 geometry diffs and ≤ 1.5% pixel diff", md)
            self.assertIn("| gate |", md)
            body = [l for l in md.splitlines() if l.startswith("| shelf | default | 1280x800 |")]
            gates = sorted(l.split("|")[6].strip() for l in body)
            self.assertEqual(gates, ["geometry", "pixel", "red", "red"])

    def test_board_json_records_the_geometry_pixel_gate(self):
        from scripts import chrome_board

        board = chrome_board.load_board(BOARD_PATH)
        ok = {"status": "ok", "chromium": {"status": "ok", "browser": "148.0.7778.216"}, "rustkit": {"status": "ok"},
              "pixels": {"diffPercent": 1.0, "top": []},
              "geometry": {"geometry_failures": 0, "join_failures": 0, "worst_selectors": []}}
        import os
        from unittest import mock
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.dict(os.environ, {"PARITY_CHROME_PATH": sys.executable}):
            rc = chrome_board.main(
                ["--parity-capture", sys.executable, "--board", str(BOARD_PATH), "--only", "shelf",
                 "--work", str(Path(tmp) / "work"), "--out", str(Path(tmp) / "out")],
                capture=lambda *a, **k: ok,
                chrome_version=lambda p: "148.0.7778.216",
            )
            self.assertEqual(rc, 0)
            run = json.loads((Path(tmp) / "out" / "board.json").read_text(encoding="utf-8"))["run"]
            self.assertEqual(run["chromium_path"], chrome_board.home_relative(Path(sys.executable)))
            self.assertNotIn(str(Path.home()), run["chromium_path"])
            self.assertEqual(run["chromium_pinned"], "148.0.7778.216")
            self.assertEqual(run["gate_pct"], 0.5)
            self.assertEqual(run["gate_geometry_pixel_pct"], 1.5)
            md = (Path(tmp) / "out" / "BOARD.md").read_text(encoding="utf-8")
            n = len(chrome_board.plan(board, REPO, Path(tmp) / "w2", only="shelf"))
            self.assertIn(f"**{n}/{n} rows green**", md)

    def test_rescore_reports_before_and_after_without_a_binary(self):
        import contextlib
        import io

        from scripts import chrome_board

        with tempfile.TemporaryDirectory() as tmp:
            src = Path(tmp) / "board.json"
            src.write_text(json.dumps({"run": {"gate_pct": 0.5}, "rows": [_row(0.2, 5), _row(1.2, 0), _row(1.6, 0)]}),
                           encoding="utf-8")
            buf = io.StringIO()
            with contextlib.redirect_stdout(buf):
                rc = chrome_board.main(["--rescore", str(src)])
            self.assertEqual(rc, 0)
            text = buf.getvalue()
            self.assertIn("before 1/3", text)
            self.assertIn("after 2/3", text)
            self.assertIn("newly green: shelf default 1280x800 1.200% geo 0", text)


class RealSiteGateTests(unittest.TestCase):
    def test_compare_geometry_joins_across_class_whitespace(self):
        """The board's geometry column goes through the same join: a two-space
        class attribute (chrome.html's locked workspace tab) is one compared box,
        not a missing_box + phantom_box pair that blocks the geometry rule."""
        from scripts import chrome_board

        chrome = {"elements": [{"selector": "body > div.workspace-tab  locked", "tag": "div",
                                "rect": {"x": 0, "y": 0, "width": 10, "height": 10}}]}
        box = {"x": 0, "y": 0, "width": 10, "height": 10}
        rustkit = {"root": {"type": "block", "border_box": {"x": 0, "y": 0, "width": 800, "height": 600},
                            "children": [{"type": "block", "selector": "body > div.workspace-tab locked",
                                          "tag": "div", "element_id": 1, "border_box": box,
                                          "content_rect": box, "children": []}]}}
        with tempfile.TemporaryDirectory() as tmp:
            rects = Path(tmp) / "layout-rects.json"
            layout = Path(tmp) / "rustkit-layout.json"
            rects.write_text(json.dumps(chrome), encoding="utf-8")
            layout.write_text(json.dumps(rustkit), encoding="utf-8")
            geometry = chrome_board.compare_geometry("t", rects, layout)
        self.assertEqual(geometry["join_failures"], 0)
        self.assertEqual(geometry["compared"], 1)
        self.assertEqual(geometry["geometry_failures"], 0)

    def test_looks_right_is_20_pct_for_now(self):
        from scripts import realsite_board

        self.assertEqual(realsite_board.LOOKS_RIGHT_MAX, 20.0)


if __name__ == "__main__":
    unittest.main()
