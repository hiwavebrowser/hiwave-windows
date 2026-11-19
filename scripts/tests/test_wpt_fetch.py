import sys
import tempfile
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from fetch_wpt_suite import verified_file


class CachedSuiteTests(unittest.TestCase):
    def test_rejects_corrupt_cache(self):
        with tempfile.TemporaryDirectory() as base:
            path = Path(base) / "test"
            path.write_bytes(b"changed")
            self.assertFalse(verified_file(path, "0" * 64))


if __name__ == "__main__":
    unittest.main()
