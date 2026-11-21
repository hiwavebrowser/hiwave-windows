"""
test_test262_driver.py: guard tests for scripts/test262.py.

1. A runner process that allocates past its cap is stopped and its test recorded as not passing in
   every mode the test runs in; the chunk goes on with the next test. Both the kernel cap (Linux)
   and the resident-size watchdog (the only cap on macOS) are checked, the watchdog on its own.
2. The committed list records pass / fail only; the runs that took the engine down go to the
   private list alone.
3. A listed failure that now takes the engine down is caught against the private list.
4. The job count never puts more than the total budget in flight.

The runner is a fake (a Python script), so no engine build is needed. Its runaway test takes memory
past the cap and then holds it without finishing (it can never print a result of its own), so the
only outcomes are "stopped at the cap" or the chunk's own deadline: the test cannot pass by racing
the watchdog's sampling. It stops growing at 1.5 GB, so a broken cap fails the test without taking
the machine's memory.
"""

import os
import stat
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import test262  # noqa: E402

FAKE_RUNNER = textwrap.dedent(
    """\
    #!/usr/bin/env python3
    import sys, time
    args = sys.argv[1:]
    listing = open(args[args.index("--list") + 1]).read().split()
    for path in listing:
        print("START\\t" + path, flush=True)
        if "grow" in path:
            hold = []
            for _ in range(30):  # 30 x 50 MB = 1.5 GB at most
                hold.append(b"x" * (50 * 1024 * 1024))
            while True:  # hold it until stopped: never print a result for this test
                time.sleep(1)
        for mode in ("sloppy", "strict"):
            print("RESULT\\t%s\\t%s\\tPASS\\t" % (path, mode), flush=True)
    """
)


class DriverTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        root = Path(self.dir.name)
        self.suite = root / "suite"
        (self.suite / "test").mkdir(parents=True)
        for name in ("a_grow.js", "b_ok.js"):
            (self.suite / "test" / name).write_text("/*---\ndescription: x\n---*/\n", encoding="utf-8")
        (self.suite / "test" / "c_strict.js").write_text("/*---\nflags: [onlyStrict]\n---*/\n", encoding="utf-8")
        script = root / "runner"
        script.write_text(FAKE_RUNNER, encoding="utf-8")
        script.chmod(script.stat().st_mode | stat.S_IEXEC)
        # The runner as an argv prefix, so the fake starts on any OS (Windows does not run a
        # script by its shebang) and the watchdog measures and stops the python process itself.
        # The base interpreter, not a venv's: on Windows a venv's python.exe is a launcher that
        # runs the real interpreter as a child, so the watchdog would measure and stop the launcher.
        # The fake runner uses only the standard library, so it needs nothing from the venv.
        self.runner = [getattr(sys, "_base_executable", sys.executable), str(script)]

    def tearDown(self):
        self.dir.cleanup()

    def run_files(self):
        files = ["test/a_grow.js", "test/b_ok.js"]
        # 1 s per run: the chunk's deadline is then about a minute, the bound on a broken cap.
        return test262.run_chunk(self.runner, files, 1000, 200, suite=self.suite)

    def check_stopped(self, results):
        self.assertEqual(results[("test/a_grow.js", "sloppy")][0], "CRASH")
        self.assertNotIn("time", results[("test/a_grow.js", "sloppy")][1], "stopped by the deadline, not the cap")
        self.assertEqual(results[("test/a_grow.js", "strict")][0], "CRASH")
        self.assertEqual(results[("test/b_ok.js", "sloppy")][0], "PASS")
        self.assertEqual(results[("test/b_ok.js", "strict")][0], "PASS")
        self.assertNotIn(("test/a_grow.js", "-"), results)

    def test_a_runaway_process_is_stopped_at_its_cap(self):
        self.check_stopped(self.run_files())

    def test_the_watchdog_alone_holds_the_cap(self):
        # macOS does not enforce RLIMIT_AS: the resident-size watchdog must stop the runner by itself.
        with mock.patch.object(test262, "address_space_cap", lambda mem_mb: (lambda: None)):
            results = self.run_files()
        self.check_stopped(results)
        self.assertIn("memory", results[("test/a_grow.js", "sloppy")][1])

    def test_the_resident_size_of_a_running_process_is_read(self):
        # #1362: the watchdog holds the cap by the runner's resident size. Windows has neither
        # /proc nor ps, so it read nothing and could never stop a runaway runner.
        rss = test262.resident_mb(os.getpid())
        self.assertIsNotNone(rss)
        self.assertGreater(rss, 1.0)

    def test_modes_follow_the_flags(self):
        self.assertEqual(test262.modes_of("test/c_strict.js", self.suite), ["strict"])
        self.assertEqual(test262.modes_of("test/b_ok.js", self.suite), ["sloppy", "strict"])
        self.assertEqual(test262.flags_of("/*---\nflags:\n  - module\n  - async\n---*/"), ["module", "async"])

    def test_the_committed_list_records_pass_or_fail_only(self):
        root = Path(self.dir.name)
        public, private = root / "public.txt", root / "private.txt"
        not_passing = {
            ("test/x.js", "sloppy"): "FAIL",
            ("test/y.js", "strict"): "PANIC",
            ("test/z.js", "sloppy"): "CRASH",
            ("test/z.js", "strict"): "CRASH",
        }
        self.assertEqual(test262.write_lists(not_passing, public, private), (4, 3))
        text = public.read_text(encoding="utf-8")
        for word in ("PANIC", "CRASH", "\t-\n", "TIMEOUT"):
            self.assertNotIn(word, text)
        self.assertEqual(test262.read_list(public), {k: "FAIL" for k in not_passing})
        self.assertEqual(
            test262.read_list(private),
            {k: v for k, v in not_passing.items() if v != "FAIL"},
        )

    def test_a_failure_that_now_takes_the_engine_down_is_caught(self):
        expected = {("test/x.js", "sloppy"): "FAIL", ("test/y.js", "strict"): "FAIL"}
        private = {("test/y.js", "strict"): "PANIC"}
        not_passing = {("test/x.js", "sloppy"): "PANIC", ("test/y.js", "strict"): "PANIC"}
        results = {k: (v, "") for k, v in not_passing.items()}
        regressions, fixed, now_down = test262.compare(not_passing, expected, private, results)
        self.assertEqual((regressions, fixed), ([], []))
        self.assertEqual(now_down, [("test/x.js", "sloppy")])

    def test_jobs_stay_within_the_total_budget(self):
        self.assertEqual(test262.jobs_within_budget(12, 3072, 24576), 8)
        self.assertEqual(test262.jobs_within_budget(8, 4096, 24576), 6)
        self.assertEqual(test262.jobs_within_budget(4, 3072, 24576), 4)
        self.assertEqual(test262.jobs_within_budget(4, 30000, 24576), 1)
        self.assertEqual(test262.DEFAULT_MEM_MB, 3072)
        self.assertEqual(test262.DEFAULT_TOTAL_MEM_MB, 24576)
        self.assertEqual(test262.MAX_JOBS, 8)


if __name__ == "__main__":
    unittest.main()
