import subprocess
import sys
import unittest
from unittest.mock import patch

from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wpt_testharness import run_page


class RunnerTests(unittest.TestCase):
    @patch("subprocess.run", side_effect=subprocess.TimeoutExpired("host", 5))
    def test_timeout_is_a_page_result(self, run):
        result = run_page(["host"], "/example.html", 5)
        self.assertEqual(result["status"], "TIMEOUT")
        self.assertEqual(result["subtests"], [])

    @patch("subprocess.run", return_value=subprocess.CompletedProcess([], 1, ""))
    def test_host_failure_is_a_page_result(self, run):
        self.assertEqual(run_page(["host"], "/example.html", 5)["status"], "ERROR")

    @patch("subprocess.run", return_value=subprocess.CompletedProcess(
        [], 0, 'WPT_RESULT {"status":"OK","subtests":[{"name":"unicode π","status":"PASS"}]}\n'))
    def test_preserves_subtest(self, run):
        result = run_page(["host"], "/example.html", 5)
        self.assertEqual(result["subtests"], [{"name": "unicode π", "status": "PASS"}])

    def test_recovers_after_child_exits_abnormally(self):
        crashed = run_page([sys.executable, "-c", "raise SystemExit(4)"], "/first", 10)
        self.assertEqual(crashed["status"], "CRASH")
        payload = 'WPT_ENCODED %7B%22status%22%3A%22OK%22%2C%22subtests%22%3A%5B%5D%7D'
        completed = run_page([sys.executable, "-c", f"print({payload!r})"], "/second", 10)
        self.assertEqual(completed, {"test": "/second", "status": "OK", "subtests": []})


if __name__ == "__main__":
    unittest.main()
