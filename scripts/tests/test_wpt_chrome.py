import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from wpt_chrome import page_command


class ChromeTests(unittest.TestCase):
    def test_page_deadline_in_milliseconds(self):
        self.assertEqual(page_command("http://127.0.0.1/", 12)[-2:], ["http://127.0.0.1/", "12000"])


if __name__ == "__main__":
    unittest.main()
