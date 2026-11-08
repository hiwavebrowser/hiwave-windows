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


if __name__ == "__main__":
    unittest.main()
