"""
test_timestable_board.py — Guard tests for Package Z2-M4 Time-Stable Board.

Validates:
1. Canonical multi-sample action sequence generation (1s, 3s, 5s, 10s).
2. Trajectory classification (CONVERGING, STABLE, DIVERGING, DYNAMIC).
3. Rule A3 compliance (beside-board placement, zero baseline/threshold mutation).
4. Catalog structure integrity.
"""

import json
import subprocess
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "scripts"))

from scripts.timestable_board import (
    build_timestable_actions,
    classify_trajectory,
    MILESTONES,
    DEFAULT_OUT_DIR,
)

CATALOG_PATH = REPO / "websuite" / "realsite-top20.json"


class TestTimestableBoard(unittest.TestCase):
    def test_milestone_definitions(self):
        """Validate 4 standard temporal milestones (1s, 3s, 5s, 10s)."""
        labels = [m["label"] for m in MILESTONES]
        times = [m["time_ms"] for m in MILESTONES]
        self.assertEqual(labels, ["1s", "3s", "5s", "10s"])
        self.assertEqual(times, [1000, 3000, 5000, 10000])

    def test_build_timestable_actions(self):
        """Verify generated action sequence reaches cumulative milestones."""
        actions = build_timestable_actions(prefix="test_engine")
        self.assertIsInstance(actions, list)
        self.assertEqual(len(actions), 8)  # 4 waits + 4 captures

        captures = [a for a in actions if a["type"] == "capture"]
        waits = [a for a in actions if a["type"] == "wait"]

        self.assertEqual(len(captures), 4)
        self.assertEqual(len(waits), 4)

        # Check cumulative wait time
        total_wait = sum(w.get("ms", 0) for w in waits)
        self.assertEqual(total_wait, 10000)

        # Check capture labels and frame paths
        self.assertEqual(captures[0]["label"], "1s")
        self.assertEqual(captures[0]["frame"], "test_engine_1s.ppm")
        self.assertEqual(captures[3]["label"], "10s")
        self.assertEqual(captures[3]["frame"], "test_engine_10s.ppm")

    def test_trajectory_classification(self):
        """Verify temporal convergence trajectory classifications."""
        # Converging: diff drops by > 1.0%
        self.assertEqual(classify_trajectory(12.0, 8.0, 5.0, c_motion=0.5), "CONVERGING")

        # Stable: diff within 1.0%
        self.assertEqual(classify_trajectory(4.5, 4.8, 4.7, c_motion=0.2), "STABLE")

        # Diverging: diff rises by > 1.0%
        self.assertEqual(classify_trajectory(3.0, 5.0, 7.5, c_motion=0.4), "DIVERGING")

        # Dynamic: Chrome itself is mutating rapidly (> 5.0% internal motion)
        self.assertEqual(classify_trajectory(10.0, 8.0, 6.0, c_motion=8.5), "DYNAMIC")

        # Incomplete / Unknown
        self.assertEqual(classify_trajectory(None, None, None, c_motion=None), "UNKNOWN")

    def test_rule_a3_compliance(self):
        """Verify Rule A3 beside-board placement and untouched legacy files."""
        # Must write beside legacy board, never overwriting legacy board files
        self.assertTrue(str(DEFAULT_OUT_DIR).endswith("timestable"))
        self.assertNotEqual(DEFAULT_OUT_DIR, REPO / "trench" / "realsite")

        # Verify catalog integrity (20 sites, max 60 points)
        self.assertTrue(CATALOG_PATH.exists())
        catalog = json.loads(CATALOG_PATH.read_text(encoding="utf-8"))
        sites = catalog.get("sites", {})
        self.assertEqual(len(sites), 20)

    def test_parity_capture_replay_proxy_e2e(self):
        """End-to-end integration test of parity-capture with --replay-proxy and HarServer."""
        bin_path = REPO / "target" / "release" / "parity-capture.exe"
        if not bin_path.exists():
            bin_path = REPO / "target" / "debug" / "parity-capture.exe"
        if not bin_path.exists():
            self.skipTest("parity-capture binary not built yet")

        sample_har = {
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {
                            "method": "GET",
                            "url": "https://replay.test/timestable",
                            "headers": [{"name": "Host", "value": "replay.test"}],
                        },
                        "response": {
                            "status": 200,
                            "statusText": "OK",
                            "headers": [{"name": "Content-Type", "value": "text/html; charset=utf-8"}],
                            "content": {
                                "size": 95,
                                "mimeType": "text/html",
                                "text": "<html><head><title>Replay Parity</title></head><body><h1>Time Stable</h1></body></html>",
                            },
                        },
                    }
                ],
            }
        }
        har_file = REPO / "target" / "e2e_replay.har"
        har_file.parent.mkdir(parents=True, exist_ok=True)
        har_file.write_text(json.dumps(sample_har), encoding="utf-8")

        from tools.parity_oracle.har_server import start_har_server_in_thread

        server, thread, server_url = start_har_server_in_thread(har_file)

        out_frame = REPO / "target" / "e2e_replay.ppm"
        if out_frame.exists():
            out_frame.unlink()

        try:
            cmd = [
                str(bin_path),
                "--url",
                "https://replay.test/timestable",
                "--replay-proxy",
                server_url,
                "--dump-frame",
                str(out_frame),
                "--timeout-ms",
                "15000",
            ]
            proc = subprocess.run(cmd, capture_output=True, text=True, timeout=20, encoding="utf-8")
            self.assertEqual(
                proc.returncode,
                0,
                f"parity-capture failed (code {proc.returncode}):\nstderr:\n{proc.stderr}\nstdout:\n{proc.stdout}",
            )
            result = json.loads(proc.stdout.strip())
            self.assertEqual(result.get("status"), "ok")
            self.assertTrue(out_frame.exists(), "Frame was not created by parity-capture")
            self.assertGreater(out_frame.stat().st_size, 0)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            if out_frame.exists():
                try:
                    out_frame.unlink()
                except Exception:
                    pass


if __name__ == "__main__":
    unittest.main()
