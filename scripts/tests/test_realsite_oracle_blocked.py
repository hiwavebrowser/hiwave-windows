#!/usr/bin/env python3
"""T19: the real-site board must not score RustKit against Chrome's own error page.

Ported from atlas/trench-realsite (A1 + A2): when the Chrome oracle lands on a
403/challenge page or a chrome-error:// page, the site is `oracle_blocked`:
READABLE/LOOKS RIGHT are oracle failures (not RustKit fails), the summary names
it, and `n/scorable` leaves it out. The oracle is headless by default; headed
(A1) is opt-in (--oracle-headed / REALSITE_ORACLE_HEADED=1), needs an idle
seat, and holds ~/.claude/realwindow.lock for the run (R1 HOLD on #960).

No browser and no network: the Chrome and RustKit captures are stubbed.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import realsite_board  # noqa: E402

PPM_W, PPM_H = 4, 4


def write_ppm(path, striped):
    px = bytearray()
    for y in range(PPM_H):
        for x in range(PPM_W):
            px += bytes((0, 0, 0)) if (striped and x == 0) else bytes((255, 255, 255))
    Path(path).write_bytes(b"P6\n%d %d\n255\n" % (PPM_W, PPM_H) + bytes(px))


def fake_chrome(result):
    def capture(url, png, text_json, width, height, env):
        Path(png).write_bytes(b"png")
        Path(text_json).write_text(json.dumps({"url": url, "text": "access denied"}), encoding="utf-8")
        return dict(result)
    return capture


def fake_rustkit(cmd, timeout, env=None, **_kw):
    frame = Path(cmd[cmd.index("--dump-frame") + 1])
    write_ppm(frame, striped=True)
    Path(cmd[cmd.index("--dump-display-list") + 1]).write_text(
        json.dumps({"commands": [{"op": "text", "text": "access denied", "x": 1, "y": 2}]}), encoding="utf-8")
    return {"status": "ok"}, None, 0


def score(chrome_result):
    out = Path(tempfile.mkdtemp(prefix="t19_"))
    try:
        with mock.patch.object(realsite_board, "chrome_capture", fake_chrome(chrome_result)), \
             mock.patch.object(realsite_board, "run_json", fake_rustkit), \
             mock.patch.object(realsite_board, "probe_access",
                               lambda url: {"status": 200, "vendor": None, "blocked": False}), \
             mock.patch.object(realsite_board, "pixel_diff",
                               lambda a, b, d, env: {"diffPercent": 1.0}), \
             mock.patch.object(realsite_board, "non_background_fraction", lambda p: 0.25):
            return realsite_board.score_site({"id": "x", "url": "https://x.test/"},
                                             Path("/bin/true"), out, 1280, 800, {})
    finally:
        shutil.rmtree(out, ignore_errors=True)


OK = {"status": "ok", "final_url": "https://x.test/", "http_status": 200,
      "browser_version": realsite_board.PINNED_CHROME_VERSION}


class OracleBlocked(unittest.TestCase):

    def test_http_403_oracle_is_oracle_blocked_not_a_pass(self):
        r = score(dict(OK, http_status=403))
        self.assertIsNone(r["oracle_capture"])
        self.assertEqual(r["chrome"][0].get("oracle_blocked"), "top-level document HTTP 403")
        for check in ("readable", "looks_right"):
            self.assertFalse(r[check]["pass"], check)
            self.assertTrue(r[check].get("oracle_failed"), check)
        self.assertIn("HTTP 403", r["readable"]["why"])
        # LOADS is RustKit's own check and is not touched by the oracle.
        self.assertTrue(r["loads"]["pass"])

    def test_chrome_error_page_is_oracle_blocked(self):
        r = score(dict(OK, final_url="chrome-error://chromewebdata/", http_status=None,
                       nav_error="net::ERR_HTTP_RESPONSE_CODE_FAILURE"))
        self.assertIsNone(r["oracle_capture"])
        self.assertIn("chrome error page", r["chrome"][0]["oracle_blocked"])

    def test_good_oracle_still_scores(self):
        r = score(OK)
        self.assertEqual(r["oracle_capture"], "a")
        self.assertNotIn("oracle_blocked", r["chrome"][0])
        self.assertEqual(r["points"], 3)

    def test_summary_names_oracle_blocked_and_leaves_it_out_of_scorable(self):
        blocked = score(dict(OK, http_status=403))
        good = dict(score(OK), id="g")
        s = realsite_board.summarize([blocked, good], "ts", 1280, 800, ["x", "g"])
        self.assertEqual(s["oracle_blocked"], {"x": "top-level document HTTP 403"})
        self.assertEqual(s["points"], 4)          # /6: the plain board is unchanged
        self.assertEqual(s["max_points"], 6)
        self.assertEqual(s["scorable_sites"], 1)  # n/scorable drops x
        self.assertEqual(s["scorable_points"], 3)
        self.assertEqual(s["scorable_max"], 3)

    def test_one_blocked_capture_falls_back_to_the_other(self):
        caps = iter([dict(OK, http_status=403), OK])
        out = Path(tempfile.mkdtemp(prefix="t19_"))
        try:
            def capture(url, png, text_json, width, height, env):
                Path(png).write_bytes(b"png")
                Path(text_json).write_text(json.dumps({"url": url, "text": "access denied"}), encoding="utf-8")
                return dict(next(caps))
            with mock.patch.object(realsite_board, "chrome_capture", capture), \
                 mock.patch.object(realsite_board, "run_json", fake_rustkit), \
                 mock.patch.object(realsite_board, "probe_access",
                                   lambda url: {"status": 200, "vendor": None, "blocked": False}), \
                 mock.patch.object(realsite_board, "pixel_diff",
                                   lambda a, b, d, env: {"diffPercent": 1.0}), \
                 mock.patch.object(realsite_board, "non_background_fraction", lambda p: 0.25):
                r = realsite_board.score_site({"id": "x", "url": "https://x.test/"},
                                              Path("/bin/true"), out, 1280, 800, {})
        finally:
            shutil.rmtree(out, ignore_errors=True)
        self.assertEqual(r["oracle_capture"], "b")
        # Self-noise needs two real oracles; one is an error page.
        self.assertNotIn("chrome_self_diff", r["looks_right"])
        s = realsite_board.summarize([r], "ts", 1280, 800, ["x"])
        self.assertEqual(s["oracle_blocked"], {})  # only when BOTH captures are blocked


NODE = shutil.which("node")
LAUNCH_MJS = REPO / "tools" / "parity_oracle" / "realsite_launch.mjs"


def launch_options(env_extra):
    code = ("import { oracleLaunchOptions } from %s;"
            "console.log(JSON.stringify(oracleLaunchOptions(1280, 800, process.env)));"
            % json.dumps(LAUNCH_MJS.as_uri()))
    env = {k: v for k, v in os.environ.items() if k != "REALSITE_ORACLE_HEADLESS"}
    env.pop("PARITY_CHROME_PATH", None)
    env.update(env_extra)
    p = subprocess.run([NODE, "--input-type=module", "-e", code], capture_output=True,
                       text=True, env=env, timeout=60, encoding="utf-8")
    if p.returncode != 0:
        raise AssertionError("node failed: %s" % p.stderr.strip()[-400:])
    out = json.loads(p.stdout.strip().splitlines()[-1])
    out["_stderr"] = p.stderr
    return out


@unittest.skipUnless(NODE, "node not on PATH")
class OracleHeadedA1(unittest.TestCase):
    """R1 HOLD (#960): headless is the default; headed is opt-in and needs the lock."""

    def test_default_is_headless(self):
        o = launch_options({})
        self.assertIs(o["headless"], True)
        self.assertFalse(any(a.startswith("--window-position") for a in o["args"]))
        self.assertIn("--force-color-profile=srgb", o["args"])  # deterministic flags kept

    def test_headed_only_when_asked_and_the_lock_is_held(self):
        lock = Path(tempfile.mkdtemp(prefix="t19_lock_")) / "realwindow.lock"
        me = str(os.getpid())
        asked = {"REALSITE_ORACLE_HEADED": "1", "REALSITE_REALWINDOW_LOCK": str(lock),
                 "REALSITE_LOCK_OWNER": me}
        try:
            # Asked, but nobody holds the lock: still headless.
            o = launch_options(asked)
            self.assertIs(o["headless"], True)
            # A bare directory (the shell recipe's lock, someone else's) is not ours.
            lock.mkdir()
            o = launch_options(asked)
            self.assertIs(o["headless"], True)
            (lock / "pid").write_text(me, encoding="utf-8")
            o = launch_options(asked)
            self.assertIs(o["headless"], False)
            self.assertIn("--window-position=-2400,0", o["args"])
            self.assertIn("--window-size=1280,800", o["args"])
            # --oracle-headless (compat) wins over a headed request.
            o = launch_options(dict(asked, REALSITE_ORACLE_HEADLESS="1"))
            self.assertIs(o["headless"], True)
        finally:
            shutil.rmtree(lock.parent, ignore_errors=True)

    def test_headed_refused_when_the_lock_names_another_pid(self):
        """R1 (#960): the lock existing is not enough; the caller must own it."""
        lock = Path(tempfile.mkdtemp(prefix="t19_lock_")) / "realwindow.lock"
        try:
            lock.mkdir()
            (lock / "pid").write_text("4242", encoding="utf-8")
            for owner in (str(os.getpid()), None):
                env = {"REALSITE_ORACLE_HEADED": "1", "REALSITE_REALWINDOW_LOCK": str(lock)}
                if owner:
                    env["REALSITE_LOCK_OWNER"] = owner
                o = launch_options(env)
                self.assertIs(o["headless"], True, owner)
                self.assertFalse(any(a.startswith("--window-position") for a in o["args"]))
                self.assertIn("headless", o["_stderr"])
                self.assertIn("4242", o["_stderr"])  # the logged reason names the holder
        finally:
            shutil.rmtree(lock.parent, ignore_errors=True)

    def test_board_flags_exist(self):
        src = (REPO / "scripts" / "realsite_board.py").read_text(encoding="utf-8")
        for flag in ("--oracle-headed", "--oracle-headless", "REALSITE_ORACLE_HEADED"):
            self.assertIn(flag, src)

    def test_board_capture_uses_the_shared_launch_and_records_status(self):
        src = (REPO / "tools" / "parity_oracle" / "realsite.mjs").read_text(encoding="utf-8")
        body = src[src.index("async function captureChrome"):src.index("async function runActionsChrome")]
        self.assertIn("oracleLaunchOptions(", body)
        self.assertIn("http_status", body)
        self.assertIn("isNavigationRequest()", body)


class RealWindowGate(unittest.TestCase):
    """The board's headed oracle holds ~/.claude/realwindow.lock and needs an idle seat."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp(prefix="t19_gate_"))
        self.lock_path = self.tmp / "realwindow.lock"
        patcher = mock.patch.object(realsite_board, "REALWINDOW_LOCK", self.lock_path)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.log = []

    def session(self, want_headed, idle=10_000.0, wait_s=0):
        return realsite_board.OracleSession(want_headed, lock_wait_s=wait_s,
                                            idle_fn=lambda: idle, log=self.log.append,
                                            sleep=lambda s: None)

    def test_headless_by_default_takes_no_lock(self):
        with self.session(False) as o:
            self.assertFalse(o.headed)
            self.assertFalse(self.lock_path.exists())
            env = o.apply({"REALSITE_ORACLE_HEADED": "1", "REALSITE_LOCK_OWNER": "1"})  # a stray env can't sneak through
            self.assertNotIn("REALSITE_LOCK_OWNER", env)
            self.assertNotEqual(env.get("REALSITE_ORACLE_HEADED"), "1")
            self.assertEqual(env.get("REALSITE_ORACLE_HEADLESS"), "1")

    def test_headed_takes_and_releases_the_lock(self):
        with self.session(True) as o:
            self.assertTrue(o.headed)
            self.assertTrue(self.lock_path.is_dir())
            self.assertEqual((self.lock_path / "pid").read_text(encoding="utf-8").strip(), str(os.getpid()))
            env = o.apply({})
            self.assertEqual(env.get("REALSITE_ORACLE_HEADED"), "1")
            self.assertEqual(env.get("REALSITE_REALWINDOW_LOCK"), str(self.lock_path))
            # The launcher goes headed only when the lock's pid file names this owner.
            self.assertEqual(env.get("REALSITE_LOCK_OWNER"), str(os.getpid()))
            self.assertNotIn("REALSITE_ORACLE_HEADLESS", env)
        self.assertFalse(self.lock_path.exists())

    def test_lock_released_when_the_run_raises(self):
        with self.assertRaises(RuntimeError):
            with self.session(True) as o:
                self.assertTrue(self.lock_path.is_dir())
                raise RuntimeError("boom")
        self.assertFalse(self.lock_path.exists())

    def test_lock_busy_means_no_headed_launch(self):
        holder = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        try:
            self.lock_path.mkdir()
            (self.lock_path / "pid").write_text(str(holder.pid), encoding="utf-8")
            with self.assertRaises(realsite_board.OracleLockBusy) as cm:
                with self.session(True):
                    self.fail("headed oracle ran without the lock")
            self.assertIn("ORACLE BLOCKED", str(cm.exception))
            # The other holder's lock is left alone.
            self.assertEqual((self.lock_path / "pid").read_text(encoding="utf-8"), str(holder.pid))
        finally:
            holder.kill()
            holder.wait()

    def test_stale_lock_of_a_dead_pid_is_recovered(self):
        dead = subprocess.Popen([sys.executable, "-c", "pass"])
        dead.wait()
        self.lock_path.mkdir()
        (self.lock_path / "pid").write_text(str(dead.pid), encoding="utf-8")
        with self.session(True) as o:
            self.assertTrue(o.headed)
        self.assertFalse(self.lock_path.exists())

    def _dead_pid(self):
        dead = subprocess.Popen([sys.executable, "-c", "pass"])
        dead.wait()
        return dead.pid

    def test_dead_lock_recovery_loses_to_a_runner_that_recovered_first(self):
        """R1 (#960): two runners both read the same dead pid; exactly one may hold.

        Runner B (a live process) recovers the dead lock between runner A's read
        of the dead pid and A's own recovery. A must go back to waiting and B's
        lock must be left exactly as B made it.
        """
        dead = self._dead_pid()
        self.lock_path.mkdir()
        (self.lock_path / "pid").write_text(str(dead), encoding="utf-8")
        b = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"])
        self.addCleanup(lambda: (b.kill(), b.wait()))
        a = realsite_board.RealWindowLock(self.lock_path, wait_s=0, sleep=lambda s: None)
        real_holder = a.holder
        fired = []

        def holder_then_b_recovers():
            pid = real_holder()
            if not fired and pid == dead:
                fired.append(1)
                # B's whole recovery lands here: dead lock gone, B's live lock made.
                shutil.rmtree(self.lock_path)
                self.lock_path.mkdir()
                (self.lock_path / "pid").write_text(str(b.pid), encoding="utf-8")
            return pid

        with mock.patch.object(a, "holder", holder_then_b_recovers):
            got = a.acquire()
        self.assertTrue(fired)
        self.assertFalse(got, "A took the lock B already held: two holders")
        self.assertFalse(a.held)
        self.assertEqual((self.lock_path / "pid").read_text(encoding="utf-8").strip(), str(b.pid))
        a.release()  # a non-holder's release must not touch B's lock
        self.assertEqual((self.lock_path / "pid").read_text(encoding="utf-8").strip(), str(b.pid))
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["realwindow.lock"])

    def test_dead_lock_already_renamed_away_means_wait(self):
        """The other recoverer renamed the stale dir first: ENOENT -> back to waiting."""
        dead = self._dead_pid()
        self.lock_path.mkdir()
        (self.lock_path / "pid").write_text(str(dead), encoding="utf-8")
        a = realsite_board.RealWindowLock(self.lock_path, wait_s=0, sleep=lambda s: None)
        real_holder = a.holder

        def holder_then_renamed():
            pid = real_holder()
            if pid == dead and self.lock_path.exists():
                os.rename(self.lock_path, self.tmp / "realwindow.lock.stale-other")
            return pid

        with mock.patch.object(a, "holder", holder_then_renamed):
            self.assertFalse(a.acquire())
        self.assertFalse(self.lock_path.exists())

    def test_stale_recovery_renames_then_removes(self):
        dead = self._dead_pid()
        self.lock_path.mkdir()
        (self.lock_path / "pid").write_text(str(dead), encoding="utf-8")
        renames = []
        real_rename = os.rename

        def spy(src, dst, *a, **kw):
            renames.append((Path(src).name, Path(dst).name))
            return real_rename(src, dst, *a, **kw)

        lock = realsite_board.RealWindowLock(self.lock_path, wait_s=0, sleep=lambda s: None)
        with mock.patch.object(realsite_board.os, "rename", spy):
            self.assertTrue(lock.acquire())
        self.assertTrue(any(s == "realwindow.lock" and d.startswith("realwindow.lock.stale-%d-" % dead)
                            for s, d in renames), renames)
        self.assertEqual(sorted(p.name for p in self.tmp.iterdir()), ["realwindow.lock"])
        self.assertEqual((self.lock_path / "pid").read_text(encoding="utf-8").strip(), str(os.getpid()))
        lock.release()
        self.assertFalse(self.lock_path.exists())

    def test_two_concurrent_recoverers_one_holder(self):
        """Two processes race to recover the same dead lock, ten rounds: one holder each time."""
        script = (
            "import sys, time, os\n"
            "sys.path.insert(0, %r)\n"
            "import realsite_board as rb\n"
            "go = sys.argv[2]\n"
            "while not os.path.exists(go): pass\n"
            "l = rb.RealWindowLock(sys.argv[1], wait_s=0, sleep=lambda s: None)\n"
            "got = l.acquire()\n"
            "print(1 if got else 0, flush=True)\n"
            "time.sleep(0.4)\n"
        ) % str(REPO / "scripts")
        for rnd in range(10):
            go = self.tmp / ("go-%d" % rnd)
            if not (self.lock_path / "pid").exists():
                shutil.rmtree(self.lock_path, ignore_errors=True)
                self.lock_path.mkdir()
                (self.lock_path / "pid").write_text(str(self._dead_pid()), encoding="utf-8")
            procs = [subprocess.Popen([sys.executable, "-c", script, str(self.lock_path), str(go)],
                                      stdout=subprocess.PIPE, text=True, encoding="utf-8") for _ in range(2)]
            go.write_text("", encoding="utf-8")
            outs = [p.communicate(timeout=60)[0].strip() for p in procs]
            # The winner exits without releasing: its pid is now dead for the next round.
            self.assertEqual(sorted(outs), ["0", "1"], "round %d: %r" % (rnd, outs))
            leftovers = [p.name for p in self.tmp.iterdir() if p.name.startswith("realwindow.lock.")]
            self.assertEqual(leftovers, [], "round %d" % rnd)

    def test_seat_in_use_falls_back_to_headless_with_the_reason(self):
        with self.session(True, idle=12.0) as o:
            self.assertFalse(o.headed)
            self.assertFalse(self.lock_path.exists())
            self.assertIn("HID idle", o.reason)
            self.assertEqual(o.apply({}).get("REALSITE_ORACLE_HEADLESS"), "1")
            s = realsite_board.summarize([dict(score(OK), id="g")], "ts", 1280, 800, ["g"], oracle=o)
        self.assertIs(s["oracle_headless"], True)
        self.assertIn("HID idle", s["oracle_reason"])
        self.assertTrue(any("HID idle" in line for line in self.log))

    def test_seat_becoming_active_mid_run_drops_to_headless(self):
        idle = [10_000.0]
        o = realsite_board.OracleSession(True, lock_wait_s=0, idle_fn=lambda: idle[0],
                                         log=self.log.append, sleep=lambda s: None)
        with o:
            self.assertTrue(o.headed)
            idle[0] = 3.0
            o.recheck("google")
            self.assertFalse(o.headed)
            self.assertFalse(self.lock_path.exists())
            self.assertIn("google", o.reason)
        self.assertFalse(self.lock_path.exists())


if __name__ == "__main__":
    unittest.main()
