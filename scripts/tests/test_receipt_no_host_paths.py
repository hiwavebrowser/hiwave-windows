#!/usr/bin/env python3
"""test_receipt_no_host_paths.py — receipt.py output goes into public PR bodies and
comments, so it must not carry the paths of the machine that ran it.

A file inside the repository prints repo-relative; a file outside it prints as its
basename. --host-paths keeps the old absolute form for local use.

Pure Python standard library only (compatible with script-guards CI lane).
Run: python3 scripts/tests/test_receipt_no_host_paths.py
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent.parent
REPO = SCRIPTS.parent
RECEIPT = SCRIPTS / "receipt.py"


def run_receipt(*args, cwd=REPO, env=None):
    out = subprocess.run([sys.executable, str(RECEIPT), "--package", "T", *args],
                         capture_output=True, text=True, cwd=cwd, timeout=120,
                         env={**os.environ, **env} if env else None, encoding="utf-8")
    assert out.returncode == 0, out.stderr
    return out.stdout


class TestReceiptNoHostPaths(unittest.TestCase):

    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        # resolve(): macOS hands out /var/... which is a link to /private/var/...
        self.outside = Path(self.tmp.name).resolve()
        (self.outside / "parity").mkdir()
        self.binary = self.outside / "parity" / "parity-capture"
        self.binary.write_bytes(b"not a real binary")
        self.campaign = self.outside / "parity_test_results.json"
        self.campaign.write_text(json.dumps({"results": [{"pixel": {"diffPercent": 1.5}}], "passed": 1, "failed": 0}), encoding="utf-8")
        self.run_file = self.outside / "ab_table.txt"
        self.run_file.write_text("row\n", encoding="utf-8")

    def tearDown(self):
        self.tmp.cleanup()

    def host_strings(self):
        return [str(self.outside), self.tmp.name, str(REPO), str(Path.home())]

    def assert_clean(self, text):
        for s in self.host_strings():
            self.assertNotIn(s, text, "host path in receipt output")

    def test_files_outside_the_repo_print_as_basenames(self):
        out = run_receipt("--binary", str(self.binary), "--campaign", str(self.campaign),
                          "--runs", str(self.run_file))
        self.assert_clean(out)
        rec = json.loads(out)
        self.assertEqual(rec["binary"], "parity-capture")
        self.assertEqual(rec["campaign"]["file"], "parity_test_results.json")
        self.assertEqual(rec["raw_runs"][0]["file"], "ab_table.txt")
        # the facts about the files are unchanged
        self.assertEqual(rec["profile"], "parity")
        self.assertEqual(rec["campaign"]["cases"], 1)
        self.assertEqual(rec["raw_runs"][0]["bytes"], 4)
        self.assertIsNotNone(rec["binary_sha256"])

    def test_files_inside_the_repo_print_repo_relative(self):
        out = run_receipt("--binary", str(RECEIPT), "--runs", "scripts/receipt.py", str(REPO / "Cargo.lock"))
        self.assert_clean(out)
        rec = json.loads(out)
        self.assertEqual(rec["binary"], "scripts/receipt.py")
        self.assertEqual([r["file"] for r in rec["raw_runs"]], ["scripts/receipt.py", "Cargo.lock"])

    def test_relative_arguments_are_resolved_from_the_working_directory(self):
        out = run_receipt("--runs", "receipt.py", cwd=SCRIPTS)
        self.assertEqual(json.loads(out)["raw_runs"][0]["file"], "scripts/receipt.py")

    def test_a_missing_file_still_prints_no_host_path(self):
        out = run_receipt("--runs", str(self.outside / "gone.txt"))
        self.assert_clean(out)
        rec = json.loads(out)
        self.assertEqual(rec["raw_runs"][0], {"file": "gone.txt", "sha256": None, "bytes": None})

    def test_a_campaign_that_does_not_parse_prints_no_host_path(self):
        self.campaign.write_text("{", encoding="utf-8")
        out = run_receipt("--campaign", str(self.campaign))
        self.assert_clean(out)
        self.assertEqual(json.loads(out)["campaign"]["file"], "parity_test_results.json")

    def test_paths_typed_into_the_note_are_stripped(self):
        note = f"bins at {self.binary} and {REPO / 'scripts' / 'receipt.py'}; see {self.run_file}."
        out = run_receipt("--note", note)
        self.assert_clean(out)
        self.assertEqual(json.loads(out)["note"], "bins at parity-capture and scripts/receipt.py; see ab_table.txt.")

    def test_note_text_that_is_not_a_host_path_is_left_alone(self):
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for t in ["quiet board 27/60 and/or 32/78, ratio 1.5/2.5",
                  "see https://github.com/hiwavebrowser/hiwave-macos/pull/660 and /search?q=x",
                  "git@github.com:hiwavebrowser/hiwave-macos.git",
                  "./rel/x.txt ../up/y.txt a/b/c"]:
            self.assertEqual(receipt.public_text(t, REPO), t)

    def test_other_spellings_of_a_host_path_in_the_note(self):
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        home = Path.home()
        for t, want in [(r"built at C:\Users\pete\hiwave\target\parity\parity-capture.exe.", "built at parity-capture.exe."),
                        ("D:/a/b.txt, then", "b.txt, then"),
                        (f"file://{home}/frames/y.html", "y.html"),
                        ("~/zz-not-a-repo/a/b.txt", "b.txt"),
                        (f"dir {home}/bins/ (and {home}/x/c.txt)", "dir bins (and c.txt)"),
                        # the four from the first review of #661
                        ("linked -L/Users/example/lib and -I/Users/example/inc", "linked -Llib and -Iinc"),
                        ("path:/Users/example/a/b", "path:b"),
                        ("PATH=/usr/bin:/Users/example/.cargo/bin", "PATH=bin:bin"),
                        (r"\\fileserver\example\x.txt", "x.txt"),
                        ("pushed to https://user:token@example.com/org/repo.git", "pushed to https://example.com/org/repo.git"),
                        ("--remap-path-prefix=/Users/example/hiwave=. -C link-arg=/opt/x/y.o", "--remap-path-prefix=hiwave=. -C link-arg=y.o")]:
            self.assertEqual(receipt.public_text(t, REPO), want)

    def test_a_shell_variable_path_in_the_note_is_not_mangled(self):
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for t in ["${HOME}/x/y", "$HOME/x/y"]:
            self.assertEqual(receipt.public_text(t, REPO), t)

    def test_rustflags_and_engine_flag_values_are_stripped(self):
        env = {"RUSTFLAGS": f"--remap-path-prefix={self.outside}/hiwave=. -L{self.outside}/lib",
               "RUSTKIT_TRACE_OUT": f"{self.outside}/trace.json", "RUSTKIT_L0": "0"}
        out = run_receipt(env=env)
        self.assert_clean(out)
        rec = json.loads(out)
        self.assertEqual(rec["rustflags"], "--remap-path-prefix=hiwave=. -Llib")
        self.assertEqual(rec["effective_engine_flags"]["RUSTKIT_TRACE_OUT"], "trace.json")
        self.assertEqual(rec["effective_engine_flags"]["RUSTKIT_L0"], "0")
        kept = json.loads(run_receipt("--host-paths", env=env))
        self.assertEqual(kept["rustflags"], env["RUSTFLAGS"])
        self.assertEqual(kept["effective_engine_flags"]["RUSTKIT_TRACE_OUT"], env["RUSTKIT_TRACE_OUT"])

    def clone_with_origin(self, origin):
        wt = self.outside / "wt"
        wt.mkdir(exist_ok=True)
        for cmd in (["git", "init", "-q"], ["git", "remote", "remove", "origin"], ["git", "remote", "add", "origin", origin]):
            subprocess.run(cmd, cwd=wt, capture_output=True, timeout=60)
        return wt

    def test_a_local_path_origin_prints_as_its_basename(self):
        wt = self.clone_with_origin(str(self.outside / "hiwave-mirror"))
        out = run_receipt(cwd=wt)
        self.assert_clean(out)
        self.assertEqual(json.loads(out)["repo"], "hiwave-mirror")
        self.assertEqual(json.loads(run_receipt("--host-paths", cwd=wt))["repo"], str(self.outside / "hiwave-mirror"))

    def test_origin_urls_keep_their_address_and_lose_credentials(self):
        for origin, want in [("https://user:token@example.com/org/repo.git", "https://example.com/org/repo.git"),
                             ("https://github.com/hiwavebrowser/hiwave-macos.git", "https://github.com/hiwavebrowser/hiwave-macos.git"),
                             ("git@github.com:hiwavebrowser/hiwave-macos.git", "git@github.com:hiwavebrowser/hiwave-macos.git"),
                             (f"file://{self.outside}/hiwave-mirror", "hiwave-mirror")]:
            out = run_receipt(cwd=self.clone_with_origin(origin))
            self.assert_clean(out)
            self.assertNotIn("token", out)
            self.assertEqual(json.loads(out)["repo"], want)

    def test_the_machine_name_is_printed_only_with_host_paths(self):
        import platform
        self.assertIsNone(json.loads(run_receipt())["host"])
        self.assertEqual(json.loads(run_receipt("--host-paths"))["host"], platform.node())

    def test_a_binary_under_a_linked_target_prints_as_typed(self):
        wt = self.clone_with_origin("https://example.com/org/repo.git")
        try:
            (wt / "target").symlink_to(self.outside, target_is_directory=True)
        except OSError:
            self.skipTest("no symlinks here")
        rec = json.loads(run_receipt("--binary", "target/parity/parity-capture", cwd=wt))
        self.assertEqual(rec["binary"], "target/parity/parity-capture")
        self.assertEqual(rec["profile"], "parity")
        self.assertIsNotNone(rec["binary_sha256"])

    # ── the second review of #661 ───────────────────────────────────────────

    def test_a_home_directory_prints_as_a_tilde_not_the_account_name(self):
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for t, want in [("cd /Users/example && make", "cd ~ && make"),
                        ("PATH=/usr/bin:/Users/example:/bin", "PATH=bin:~:/bin"),
                        ("/home/example", "~"),
                        ("/Users/example/", "~"),
                        (r"C:\Users\example", "~"),
                        (f"at {Path.home()}.", "at ~."),
                        ("~example/hiwave/x.txt and ~example/", "x.txt and ~")]:
            self.assertEqual(receipt.public_text(t, REPO), want)
        self.assertEqual(receipt.public_path("/Users/example", REPO), "~")
        self.assertEqual(receipt.public_path(str(Path.home()), REPO), "~")
        rec = json.loads(run_receipt("--runs", "/Users/example", env={"RUSTKIT_HOME": "/Users/example"}))
        self.assertEqual(rec["effective_engine_flags"]["RUSTKIT_HOME"], "~")
        self.assertEqual(rec["raw_runs"][0]["file"], "~")

    def test_an_option_word_glued_to_a_path_is_stripped(self):
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for t, want in [("-isystem/Users/example/inc", "-isysteminc"),
                        ("-iquote/Users/example/inc -isysroot/Users/example/sdk", "-iquoteinc -isysrootsdk"),
                        ("--L/Users/example/lib", "--Llib"),
                        # not options: a word with a dash inside it is a relative path
                        ("rustkit-layout/src/grid.rs and x-y/a/b", "rustkit-layout/src/grid.rs and x-y/a/b")]:
            self.assertEqual(receipt.public_text(t, REPO), want)
        rec = json.loads(run_receipt(env={"RUSTFLAGS": "-C opt-level=3 -isystem/Users/example/inc"}))
        self.assertEqual(rec["rustflags"], "-C opt-level=3 -isysteminc")

    def test_nothing_is_written_to_stderr(self):
        # Python 3.12+ shows a DeprecationWarning raised in __main__ with the
        # script's absolute path in it.
        out = subprocess.run([sys.executable, "-W", "error::DeprecationWarning", str(RECEIPT), "--package", "T",
                              "--binary", str(self.binary)], capture_output=True, text=True, cwd=REPO, timeout=120, encoding="utf-8")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stderr, "")
        rec = json.loads(out.stdout)
        for field in ("utc", "binary_mtime_utc"):
            self.assertRegex(rec[field], r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?Z$")

    def test_the_base_is_cleaned_like_the_note(self):
        rec = json.loads(run_receipt("--base", f"{self.outside}/x"))
        self.assertEqual(rec["base_sha"], "x")
        self.assertEqual(json.loads(run_receipt("--base", "5feb459a"))["base_sha"], "5feb459a")

    def test_an_origin_with_a_remote_path_or_a_query_keeps_only_its_address(self):
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for origin, want in [("example@nas.local:/Users/example/m.git", "nas.local:m.git"),
                             ("nas.local:/srv/git/m.git", "nas.local:m.git"),
                             ("example@nas.local:~/m.git", "nas.local:m.git"),
                             ("https://t@github.com/o/r?token=abc", "https://github.com/o/r"),
                             ("git@github.com:hiwavebrowser/hiwave-macos.git", "git@github.com:hiwavebrowser/hiwave-macos.git")]:
            self.assertEqual(receipt.public_repo(origin), want)
        self.assertEqual(receipt.public_text("got https://t@github.com/o/r?token=abc&ref=main", REPO),
                         "got https://github.com/o/r?token=REDACTED&ref=main")

    def test_markdown_output_is_clean_too(self):
        out = run_receipt("--markdown", "--binary", str(self.binary), "--runs", str(self.run_file))
        self.assert_clean(out)

    def test_host_paths_flag_keeps_the_absolute_form(self):
        out = run_receipt("--host-paths", "--binary", str(self.binary), "--runs", str(self.run_file))
        rec = json.loads(out)
        self.assertEqual(rec["binary"], str(self.binary))
        self.assertEqual(rec["raw_runs"][0]["file"], str(self.run_file))

    # ── #661 residuals the PR body named but did not pin ────────────────────

    def test_a_path_with_a_space_in_the_note_is_cut_at_the_space(self):
        # ABS_PATH's segment alphabet has no space, so the leading directories
        # up to the space are stripped and the rest of the path stays. Pin
        # the residual so a wider match cannot silently re-leak a home dir.
        # These expected values are the known partial leak, not the goal:
        # tighten them to the basename once ABS_PATH handles spaces.
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for t, want in [
            ("/Users/example/My Documents/out/parity-capture",
             "My Documents/out/parity-capture"),
            ("bins at /Users/example/My Documents/out/parity-capture done",
             "bins at My Documents/out/parity-capture done"),
            (r"C:\Users\example\My Documents\out.bin",
             r"My Documents\out.bin"),
        ]:
            self.assertEqual(receipt.public_text(t, REPO), want)
        out = run_receipt("--note", f"bins at {self.outside / 'My Documents' / 'parity-capture'}")
        self.assert_clean(out)
        self.assertEqual(
            json.loads(out)["note"],
            "bins at My Documents/parity-capture",
        )

    def test_a_schemeless_wiki_path_in_the_note_is_shortened_to_its_basename(self):
        # Two-or-more-segment absolute paths match ABS_PATH even when they
        # are wiki-style URL paths typed without a scheme. Schemed URLs are
        # left alone (credentials stripped elsewhere).
        sys.path.insert(0, str(SCRIPTS))
        import receipt
        for t, want in [
            ("/w/load.php?modules=site", "load.php?modules=site"),
            ("see /w/load.php?modules=site", "see load.php?modules=site"),
            ("https://example.com/w/load.php?modules=site",
             "https://example.com/w/load.php?modules=site"),
            ("https://en.wikipedia.org/w/load.php?modules=site",
             "https://en.wikipedia.org/w/load.php?modules=site"),
        ]:
            self.assertEqual(receipt.public_text(t, REPO), want)


if __name__ == "__main__":
    unittest.main()
