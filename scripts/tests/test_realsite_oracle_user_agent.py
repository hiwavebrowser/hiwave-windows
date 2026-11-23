#!/usr/bin/env python3
"""The real-site oracle's user agent is Chrome's own, or Chrome's own without the Headless marker.

REALSITE_ORACLE_HEADED_UA=1 on a headless oracle sends exactly what the same Chrome sends headed:
its navigator.userAgent with "HeadlessChrome/" read as "Chrome/", nothing else. No other
variable can set the oracle's UA to an arbitrary string (an earlier REALSITE_ORACLE_UA could).
"""
import json
import os
import shutil
import subprocess
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
NODE = shutil.which("node")
LAUNCH_MJS = REPO / "tools" / "parity_oracle" / "realsite_launch.mjs"

HEADLESS_UA = ("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
               "HeadlessChrome/148.0.0.0 Safari/537.36")
HEADED_UA = HEADLESS_UA.replace("HeadlessChrome/", "Chrome/")


def oracle_user_agent(env_extra, headless, chrome_ua=HEADLESS_UA):
    code = ("import { oracleUserAgent } from %s;"
            "const ua = oracleUserAgent(%s, %s, process.env);"
            "console.log(JSON.stringify(ua === undefined ? null : ua));"
            % (json.dumps(LAUNCH_MJS.as_uri()), json.dumps(chrome_ua), json.dumps(headless)))
    env = {k: v for k, v in os.environ.items() if not k.startswith("REALSITE_ORACLE")}
    env.update(env_extra)
    p = subprocess.run([NODE, "--input-type=module", "-e", code], capture_output=True,
                       text=True, env=env, timeout=60, encoding="utf-8")
    if p.returncode != 0:
        raise AssertionError("node failed: %s" % p.stderr.strip()[-400:])
    return json.loads(p.stdout.strip().splitlines()[-1])


@unittest.skipUnless(NODE, "node not on PATH")
class OracleUserAgent(unittest.TestCase):
    def test_unset_leaves_chromes_own(self):
        self.assertIsNone(oracle_user_agent({}, headless=True))

    def test_headed_ua_drops_only_the_headless_marker(self):
        self.assertEqual(oracle_user_agent({"REALSITE_ORACLE_HEADED_UA": "1"}, headless=True), HEADED_UA)

    def test_a_headed_oracle_is_left_alone(self):
        self.assertIsNone(oracle_user_agent({"REALSITE_ORACLE_HEADED_UA": "1"}, headless=False))

    def test_no_variable_sets_an_arbitrary_ua(self):
        anything = {"REALSITE_ORACLE_UA": "Mozilla/5.0 (Windows NT 10.0) Anything/1.0"}
        self.assertIsNone(oracle_user_agent(anything, headless=True))
        anything["REALSITE_ORACLE_HEADED_UA"] = "1"
        self.assertEqual(oracle_user_agent(anything, headless=True), HEADED_UA)


if __name__ == "__main__":
    sys.exit(unittest.main())
