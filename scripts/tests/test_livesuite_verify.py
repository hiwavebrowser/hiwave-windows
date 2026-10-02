"""The verifier must fail on the shapes it exists to catch.

A snapshot checker that only ever passes is decoration — and this one
already earned its keep by catching a real bug in the freezer (relative
refs left unrewritten, which my own eyeball check had called clean because
they were not *remote*; they were relative and broken).
"""
import json
import subprocess
import sys
from pathlib import Path

VERIFY = Path(__file__).resolve().parents[1] / "livesuite_verify.py"


def make_snapshot(root: Path, name: str, *, html: str, status=200, review="REVIEWED", assets=None):
    d = root / name
    (d / "assets").mkdir(parents=True, exist_ok=True)
    for fname, body in (assets or {}).items():
        (d / "assets" / fname).write_bytes(body)
    (d / "index.html").write_text(html, encoding="utf-8")
    (d / "manifest.json").write_text(json.dumps({
        "name": name, "source_url": "https://example.com",
        "http_status": status, "asset_count": len(assets or {}),
        "review": {"status": review},
    }), encoding="utf-8")
    return d


def run(root: Path):
    p = subprocess.run([sys.executable, str(VERIFY), str(root)],
                       capture_output=True, text=True, encoding="utf-8")
    return p.returncode, p.stdout


def test_a_clean_snapshot_passes(tmp_path):
    # POSITIVE CONTROL. Without it, a verifier that fails everything would
    # look like a working guard.
    make_snapshot(tmp_path, "clean",
                  html='<img src="assets/a.png"><a href="https://elsewhere.example/x">link</a>',
                  assets={"a.png": b"\x89PNG"})
    code, out = run(tmp_path)
    assert code == 0, out
    assert "1/1 snapshots offline-clean" in out


def test_a_remote_fetch_fails(tmp_path):
    # The core claim: rendering touches no network.
    make_snapshot(tmp_path, "remote",
                  html='<img src="https://cdn.example.com/live.png">')
    code, out = run(tmp_path)
    assert code == 1
    assert "still remote at render time" in out


def test_a_dangling_local_ref_fails(tmp_path):
    # The bug the verifier actually caught in the freezer: rewritten to a
    # local path that does not exist. Renders as a hole that looks exactly
    # like an engine defect.
    make_snapshot(tmp_path, "dangling", html='<img src="assets/missing.png">')
    code, out = run(tmp_path)
    assert code == 1
    assert "missing local file" in out


def test_an_unreviewed_snapshot_fails(tmp_path):
    # Only a human can tell the real page from a plausible-looking consent
    # wall or bot challenge.
    make_snapshot(tmp_path, "unreviewed", html="<p>hi</p>", review="PENDING")
    code, out = run(tmp_path)
    assert code == 1
    assert "not REVIEWED" in out


def test_a_non_200_freeze_fails(tmp_path):
    # A 403 snapshot is an interstitial. Scoring it as the site is the
    # false-green this lane exists to prevent.
    make_snapshot(tmp_path, "walled", html="<p>Access denied</p>", status=403)
    code, out = run(tmp_path)
    assert code == 1
    assert "interstitial" in out


def test_anchor_targets_are_left_alone(tmp_path):
    # A link's DESTINATION is not a fetch. Rewriting it would change what
    # the page means; flagging it would make every real page fail.
    make_snapshot(tmp_path, "anchors",
                  html='<a href="https://example.org/deep">go</a>')
    code, out = run(tmp_path)
    assert code == 0, out


# ---------------------------------------------------------------------------
# Standalone runner. The six tests above are pytest-style (tmp_path fixture),
# and the CI guard job invokes each file as `python3 <file>` — which, for a
# module that never calls its functions, executes ZERO assertions and exits 0.
# Both Argos and Prometheus measured exactly that: "PASS" printed over an
# empty run, for every run since the guard job landed. This shim runs every
# module-level test_* function with a real temporary directory standing in
# for tmp_path, and fails loud if it collected nothing — the guard suite's
# own empty-run tripwire, applied to this file.
# ---------------------------------------------------------------------------
import tempfile
import unittest


def _collected():
    return [
        (name, fn)
        for name, fn in sorted(globals().items())
        if name.startswith("test_") and callable(fn)
    ]


class LivesuiteVerifyTests(unittest.TestCase):
    def test_all_module_tests_run(self):
        tests = _collected()
        self.assertGreaterEqual(
            len(tests), 6,
            "collected fewer module tests than exist — the shim went stale"
        )
        for name, fn in tests:
            with self.subTest(name):
                with tempfile.TemporaryDirectory() as td:
                    fn(Path(td))


if __name__ == "__main__":
    unittest.main()
