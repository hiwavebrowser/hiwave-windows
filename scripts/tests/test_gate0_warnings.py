"""
test_gate0_warnings.py — guards for scripts/gate0/warnings.py (gate 0's warning census).

1. A warning is `<message> @ <file>`; its line and column do not count, and summary lines are not
   warnings.
2. Within one log identical warnings in one file are counted, and a second one is new; across logs
   (the app's check repeats the engine's) a key counts at its largest; a Windows path is the same key.
3. `check` passes on the baseline's own set, names a new warning and fails, and reports a warning
   that went away without failing.
4. The committed baselines are sorted, unique and in the census's own form.
"""

import importlib.util
import io
import sys
import tempfile
import unittest
from contextlib import redirect_stdout
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent.parent
SPEC = importlib.util.spec_from_file_location("gate0_warnings", REPO / "scripts" / "gate0" / "warnings.py")
tool = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(tool)

LOG = """\
\x1b[33mwarning\x1b[0m: unused import: `Duration`
   --> crates/rustkit-engine/src/content_thread.rs:12:5
    |
warning: unused import: `Duration`
   --> crates/rustkit-engine/src/content_thread.rs:840:9
warning: method `ms` is never used
  --> crates/rustkit-engine/src/page_protocol.rs:77:12
warning: `rustkit-engine` (lib) generated 3 warnings
warning: 2 warnings emitted
"""


class Census(unittest.TestCase):
    def test_a_warning_is_its_message_and_file(self):
        # The two `Duration` warnings are on different lines of one file: one key, counted twice.
        self.assertEqual(dict(tool.census([LOG])), {
            "unused import: `Duration` @ crates/rustkit-engine/src/content_thread.rs": 2,
            "method `ms` is never used @ crates/rustkit-engine/src/page_protocol.rs": 1,
        })

    def run_check(self, baseline_lines, log):
        with tempfile.TemporaryDirectory() as d:
            base = Path(d) / "base.txt"
            base.write_text("# a comment\n" + "\n".join(baseline_lines) + "\n", encoding="utf-8")
            logf = Path(d) / "check.log"
            logf.write_text(log, encoding="utf-8")
            out = io.StringIO()
            with redirect_stdout(out):
                code = tool.main(["check", str(base), str(logf)])
            return code, out.getvalue()

    def test_check_passes_on_the_baseline_and_fails_on_a_new_warning(self):
        known = tool.lines_of(tool.census([LOG]))
        self.assertEqual(self.run_check(known, LOG)[0], 0)
        code, out = self.run_check(known[:1], LOG)
        self.assertEqual(code, 1)
        self.assertIn("NEW  unused import: `Duration` @ crates/rustkit-engine/src/content_thread.rs\n", out)

    def test_a_warning_that_went_away_is_reported_and_does_not_fail(self):
        known = tool.lines_of(tool.census([LOG])) + ["unused variable: `x` @ crates/hiwave-app/src/main.rs"]
        code, out = self.run_check(known, LOG)
        self.assertEqual(code, 0)
        self.assertIn("gone unused variable: `x` @ crates/hiwave-app/src/main.rs", out)

    def test_a_second_identical_warning_in_the_same_file_is_new(self):
        # Keyed without the line, two identical warnings in one file are one key: the census counts
        # them, so a second one is a new warning, not hidden behind the first.
        second = LOG.replace("warning: method `ms` is never used\n",
                             "warning: method `ms` is never used\n  --> crates/rustkit-engine/src/page_protocol.rs:90:12\n"
                             "warning: method `ms` is never used\n", 1)
        self.assertEqual(tool.census([second])["method `ms` is never used @ crates/rustkit-engine/src/page_protocol.rs"], 2)
        code, out = self.run_check(tool.lines_of(tool.census([LOG])), second)
        self.assertEqual(code, 1)
        self.assertIn("NEW  method `ms` is never used @ crates/rustkit-engine/src/page_protocol.rs (2, baseline 1)", out)

    def test_a_crates_warnings_repeated_in_a_second_log_count_once(self):
        # `check -p hiwave-app` reports the engine's warnings again: the two logs of one gate.
        self.assertEqual(tool.census([LOG, LOG]), tool.census([LOG]))

    def test_a_windows_path_is_the_same_key(self):
        windows = LOG.replace("crates/rustkit-engine/src/content_thread.rs", "crates\\rustkit-engine\\src\\content_thread.rs")
        self.assertIn("\\", windows)
        self.assertEqual(tool.census([windows]), tool.census([LOG]))

    def test_the_committed_baselines_are_in_the_census_form(self):
        for name in ("baseline-engine-app.txt", "baseline-renderer.txt"):
            path = REPO / "scripts" / "gate0" / name
            entries = [line.rstrip("\n") for line in path.read_text(encoding="utf-8").splitlines()
                       if line.strip() and not line.startswith("#")]
            self.assertTrue(entries, name)
            self.assertEqual(entries, sorted(entries), name + " is not sorted")
            for entry in entries:
                self.assertRegex(entry, r"^.+ @ (crates|third_party)/\S+\.rs( x\d+)?$", name)


if __name__ == "__main__":
    unittest.main()
