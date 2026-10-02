#!/usr/bin/env python3
"""Seat tooling (untracked): run parity_test.py with ~/.cargo/bin on PATH.

The non-interactive seat's shell lacks cargo on PATH; parity_test.py shells
out to `cargo build` before capturing. This wrapper fixes the environment and
execs the real suite, forwarding argv.
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
env = dict(os.environ)
env["PATH"] = os.path.expanduser("~/.cargo/bin") + os.pathsep + env.get("PATH", "")

cmd = [sys.executable, os.path.join(HERE, "parity_test.py"), *sys.argv[1:]]
sys.exit(subprocess.run(cmd, env=env).returncode)
