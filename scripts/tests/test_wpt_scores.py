import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wpt_subtest_diff import scores


class ScoresTests(unittest.TestCase):
    def test_nested_css_directory(self):
        rows = {"css/css-values/parsing/test.html": {"status": "OK", "subtests": []}}
        self.assertIn("css/css-values", scores(rows, rows))

    def test_most_specific_configured_directory_wins(self):
        rows = {"/custom/api/parsing/example.html": {"status": "OK", "subtests": []}}
        self.assertEqual(list(scores(rows, rows, {"custom", "custom/api/parsing"})),
                         ["custom/api/parsing"])


if __name__ == "__main__":
    unittest.main()
