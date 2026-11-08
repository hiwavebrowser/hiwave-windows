"""T14 (W0b): the WPT reftest runner may not score a frame it cannot vouch for.

Pins the holes the first network sync found:
- `css-flexbox/align-self-015` links `support/flexbox.css`, which the manifest
  never listed. `--check` passed and both sides rendered without the sheet.
- The "no script host" skip outlived the script host (#799 --run-inline-scripts).
- last-run.json named no develop SHA and no oracle.

Run: python3 scripts/tests/test_wpt_tier1.py
"""
import json
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

REPO = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts"))
import wpt_coverage  # noqa: E402
import wpt_import  # noqa: E402
import wpt_tier1  # noqa: E402


class MissingResources(unittest.TestCase):
    def page(self, html, files=()):
        root = pathlib.Path(self.td.name)
        for rel in files:
            target = root / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("", encoding="utf-8")
        page = root / "css" / "t.html"
        page.parent.mkdir(parents=True, exist_ok=True)
        page.write_text(html, encoding="utf-8")
        return page, root

    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.addCleanup(self.td.cleanup)

    def test_absent_relative_stylesheet_is_named(self):
        page, root = self.page('<link href="support/x.css" rel="stylesheet">')
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["support/x.css"])

    def test_present_stylesheet_is_not_named(self):
        page, root = self.page(
            '<link rel=stylesheet href="support/x.css">', files=["css/support/x.css"]
        )
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_root_absolute_resolves_against_the_wpt_root(self):
        page, root = self.page(
            '<link rel="stylesheet" href="/fonts/a.css"><img src="/fonts/b.png">',
            files=["fonts/a.css"],
        )
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["/fonts/b.png"])

    def test_metadata_links_and_remote_urls_are_not_resources(self):
        page, root = self.page(
            '<link rel="author" href="mailto:a@b.c">'
            '<link rel="help" href="https://drafts.csswg.org/x">'
            '<link rel="match" href="absent-ref.html">'
            '<img src="data:image/png;base64,AAAA">'
            '<style>a { background: url(https://x.test/a.png) }</style>'
        )
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_css_url_and_import_are_resources(self):
        page, root = self.page(
            '<style>@import "a.css"; b { background: url(c.png) }</style>'
        )
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["a.css", "c.png"])

    def test_a_linked_sheet_is_followed_relative_to_itself(self):
        page, root = self.page(
            '<link rel="stylesheet" href="support/x.css">', files=["css/support/x.css"]
        )
        (root / "css/support/x.css").write_text('a { background: url("y.png") }', encoding="utf-8")
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["y.png"])

    def test_unquoted_missing_sheet_is_named(self):
        page, root = self.page("<link rel=stylesheet href=support/x.css>")
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["support/x.css"])

    def test_unquoted_missing_image_is_named(self):
        page, root = self.page("<img src=missing.png>")
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["missing.png"])

    def test_unquoted_value_ends_at_the_tag(self):
        page, root = self.page("<img src=a.png><img alt=x src=b.png width=1>", files=["css/a.png"])
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["b.png"])

    def test_url_in_a_comment_is_not_a_resource(self):
        page, root = self.page(
            "<!-- <img src=gone.png> url(gone2.png) -->"
            "<style>/* url(gone3.png) */ a { color: red }</style>"
        )
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_url_in_a_linked_sheets_comment_is_not_a_resource(self):
        page, root = self.page('<link rel="stylesheet" href="x.css">', files=["css/x.css"])
        (root / "css/x.css").write_text("/* url(gone.png) */ a { color: red }", encoding="utf-8")
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_meta_content_is_not_a_resource(self):
        page, root = self.page('<meta name="assert" content="uses url(gone.png) here">')
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_data_src_is_not_a_resource(self):
        page, root = self.page('<img data-src="gone.png" src="here.png">', files=["css/here.png"])
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_a_script_assignment_is_not_a_resource(self):
        page, root = self.page('<script>img.src = next; a.src="gone.png";</script>')
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_a_script_element_src_is_a_resource(self):
        page, root = self.page("<script src=gone.js></script>")
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["gone.js"])

    def test_src_inside_another_attributes_quotes_is_not_a_resource(self):
        page, root = self.page("""<div title="src=gone.png"></div><p data-x='src="gone2.png"'></p>""")
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_a_close_angle_inside_quotes_does_not_end_the_tag(self):
        page, root = self.page('<img alt="a>b" src=gone.png>')
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["gone.png"])

    def test_a_close_angle_inside_quotes_does_not_end_a_link(self):
        page, root = self.page('<link title="a>b" rel=stylesheet href=gone.css>')
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["gone.css"])

    def test_page_text_is_not_a_resource(self):
        page, root = self.page("<p>write url(gone.png) in your sheet</p>")
        self.assertEqual(wpt_tier1.missing_resources(page, root), [])

    def test_a_style_attribute_is_scanned(self):
        page, root = self.page('<div style="background: url(gone.png)"></div>')
        self.assertEqual(wpt_tier1.missing_resources(page, root), ["gone.png"])

    def test_local_resources_lists_present_files_for_the_importer(self):
        page, root = self.page(
            '<link rel="stylesheet" href="support/x.css"><img src="here.png">',
            files=["css/support/x.css", "css/here.png"],
        )
        (root / "css/support/x.css").write_text("a { background: url(y.png) }", encoding="utf-8")
        (root / "css/support/y.png").write_text("", encoding="utf-8")
        found = [t.relative_to(root).as_posix() for _, t in wpt_tier1.local_resources(page, root)]
        self.assertEqual(sorted(found), ["css/here.png", "css/support/x.css", "css/support/y.png"])


class Unquoted(unittest.TestCase):
    """The rest of the runner reads the same attributes the gate does."""

    def test_rel_match_href_may_be_unquoted(self):
        with tempfile.TemporaryDirectory() as td:
            page = pathlib.Path(td) / "t.html"
            page.write_text("<link rel=match href=t-ref.html>", encoding="utf-8")
            self.assertEqual(wpt_tier1.rel_match_hrefs(page), ["t-ref.html"])

    def test_rel_match_with_a_close_angle_in_quotes(self):
        with tempfile.TemporaryDirectory() as td:
            page = pathlib.Path(td) / "t.html"
            page.write_text('<link title="a>b" rel="match" href="t-ref.html"><link rel=mismatch href=no.html>', encoding="utf-8")
            self.assertEqual(wpt_tier1.rel_match_hrefs(page), ["t-ref.html"])

    def test_root_absolute_unquoted_href_is_staged(self):
        with tempfile.TemporaryDirectory() as td:
            page = pathlib.Path(td) / "t.html"
            page.write_text("<link rel=stylesheet href=/fonts/absent.css><img src=/x.png>", encoding="utf-8")
            staged = wpt_tier1.stage_html(page).read_text(encoding="utf-8")
            self.assertNotIn("href=/fonts", staged)
            self.assertIn(str(wpt_tier1.WPT_ROOT / "fonts/absent.css"), staged)
            self.assertIn(str(wpt_tier1.WPT_ROOT / "x.png"), staged)

    def test_unquoted_fonts_href_declares_a_web_font(self):
        self.assertTrue(wpt_tier1.webfont_dependent("<link rel=stylesheet href=/fonts/ahem.css>", ""))
        self.assertTrue(wpt_tier1.webfont_dependent("<meta name=flags content=ahem>", ""))

    def test_unquoted_fuzzy_is_unrunnable(self):
        self.assertIn("fuzzy", wpt_tier1.unrunnable_reason("<meta name=fuzzy content=1>", ""))


class CampaignDiffReasons(unittest.TestCase):
    """Every way the reported diff can be missing says which way it was."""

    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.addCleanup(self.td.cleanup)
        oracle = pathlib.Path(self.td.name)
        (oracle / "node_modules").mkdir()
        patcher = mock.patch.object(wpt_tier1, "ORACLE_DIR", oracle)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.oracle = oracle

    def diff(self, returncode=0, stdout="", stderr="", raises=None):
        done = subprocess.CompletedProcess([], returncode, stdout, stderr)
        with mock.patch.object(wpt_tier1.subprocess, "run", return_value=done, side_effect=raises):
            return wpt_tier1.campaign_diff(pathlib.Path("t"), pathlib.Path("r"))

    def test_a_measurement_has_no_reason(self):
        self.assertEqual(self.diff(stdout='{"diff_pct": 1.23456}'), (1.2346, None))

    def test_no_node(self):
        pct, why = self.diff(raises=FileNotFoundError("node"))
        self.assertIsNone(pct)
        self.assertIn("node", why)

    def test_no_node_modules(self):
        (self.oracle / "node_modules").rmdir()
        pct, why = self.diff(stdout='{"diff_pct": 1}')
        self.assertIsNone(pct)
        self.assertIn("node_modules", why)

    def test_a_crash_says_its_exit_and_last_line(self):
        pct, why = self.diff(returncode=3, stderr="at x\nTypeError: boom\n")
        self.assertIsNone(pct)
        self.assertIn("exit 3", why)
        self.assertIn("boom", why)

    def test_a_timeout(self):
        pct, why = self.diff(raises=subprocess.TimeoutExpired("node", 60))
        self.assertIsNone(pct)
        self.assertIn("timeout", why)

    def test_bad_json(self):
        pct, why = self.diff(stdout="not json")
        self.assertIsNone(pct)
        self.assertIn("JSON", why)

    def test_json_that_is_not_an_object_does_not_raise(self):
        pct, why = self.diff(stdout="[1, 2]")
        self.assertIsNone(pct)
        self.assertIn("diff_pct", why)

    def test_a_non_number(self):
        pct, why = self.diff(stdout='{"diff_pct": "x"}')
        self.assertIsNone(pct)
        self.assertIn("diff_pct", why)

    def test_a_bool_is_not_a_number(self):
        pct, why = self.diff(stdout='{"diff_pct": true}')
        self.assertIsNone(pct)
        self.assertIn("diff_pct", why)


class GatePrecedence(unittest.TestCase):
    def test_a_missing_sheet_outranks_reftest_wait(self):
        update = wpt_tier1.gate("<html class=reftest-wait>", "", ["support/x.css"])
        self.assertEqual(update["status"], "ERROR")
        self.assertIn("support/x.css", update["reason"])

    def test_reftest_wait_with_its_resources_is_skip(self):
        self.assertEqual(wpt_tier1.gate("<html class=reftest-wait>", "", [])["status"], "SKIP")

    def test_a_page_with_nothing_wrong_goes_on_to_pixels(self):
        self.assertIsNone(wpt_tier1.gate("<p>x</p>", "<p>x</p>", []))


class ProofCase(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.addCleanup(self.td.cleanup)
        self.dir = pathlib.Path(self.td.name)
        self.page = self.dir / "t.html"
        self.page.write_text('<style>a { color: red }</style><p style="margin:0">a b</p>', encoding="utf-8")
        self.ref = self.dir / "ref.ppm"

    def prove(self, entry, diffs, capture_says=None):
        """`diffs` is what each probe's frame differs from the reference by."""
        frames = iter(diffs)
        with mock.patch.object(wpt_tier1, "capture", return_value=capture_says), mock.patch.object(
            wpt_tier1, "compare_frames", side_effect=lambda *_: (next(frames), 0.0)
        ):
            return wpt_tier1.prove_pass(entry, self.page, self.ref, {"width": 1, "height": 1}, self.dir)


class Proof(ProofCase):
    """A PASS has to go red under a mutation, or it leaves the pass count."""

    def test_a_page_that_goes_red_without_style_is_proven(self):
        self.assertIsNone(self.prove({}, [100]))

    def test_a_page_that_matches_without_style_is_unproven(self):
        self.assertIn("style removed", self.prove({}, [0]))

    def test_a_mutation_that_changes_nothing_is_unproven(self):
        entry = {"mutations": [{"find": "a b", "replace": "a c"}]}
        self.assertIn("a b", self.prove(entry, [100, 0]))

    def test_every_mutation_must_go_red(self):
        entry = {"mutations": [{"find": "a b", "replace": "a c"}, {"find": "color", "replace": "margin"}]}
        self.assertIsNone(self.prove(entry, [100, 5, 5]))
        self.assertIn("color", self.prove(entry, [100, 5, 0]))

    def test_a_mutation_that_no_longer_applies_is_unproven_and_says_so(self):
        entry = {"mutations": [{"find": "not on the page", "replace": "x"}]}
        self.assertIn("does not apply", self.prove(entry, [100]))

    def test_a_probe_that_cannot_render_is_unproven(self):
        self.assertIn("no frame", self.prove({}, [], capture_says="capture exit 1"))

    def test_style_removed_takes_sheets_elements_and_attributes(self):
        out = wpt_tier1.style_removed(
            '<link rel="stylesheet" href="a.css"><link rel="match" href="r.html">'
            "<STYLE>a { }</STYLE><p style='x'>t</p><p title=\"style=y\">u</p>"
        )
        self.assertNotIn("a.css", out)
        self.assertNotIn("<STYLE", out)
        self.assertNotIn("'x'", out)
        self.assertIn("r.html", out)
        self.assertIn('title="style=y"', out)

    def test_apply_mutation_replaces_every_occurrence(self):
        self.assertEqual(wpt_tier1.apply_mutation("aXbX", {"find": "X", "replace": "y"}), "ayby")
        self.assertIsNone(wpt_tier1.apply_mutation("ab", {"find": "X", "replace": "y"}))


class AreaProbe(ProofCase):
    """T15b (#856 R1): a page reacting to its style does not show the engine does the
    thing the test asserts. All 14 outline passes matched with outline forced off."""

    def test_a_pass_that_still_matches_under_its_area_probe_is_unproven(self):
        why = self.prove({"area": "css-ui-outline"}, [100, 0])
        self.assertIn("css-ui-outline", why)
        self.assertIn("outline", why)

    def test_a_pass_that_goes_red_under_its_area_probe_is_proven(self):
        self.assertIsNone(self.prove({"area": "css-ui-outline"}, [100, 5]))

    def test_the_area_probe_comes_with_the_mutations_not_instead_of_them(self):
        entry = {"area": "css-ui-outline", "mutations": [{"find": "a b", "replace": "a c"}]}
        self.assertIn("a b", self.prove(entry, [100, 5, 0]))

    def test_the_seed_has_no_area_probe(self):
        self.assertIsNone(wpt_tier1.area_mutated("<p>x</p>", "seed"))

    def test_the_rule_comes_last_and_forces_the_property(self):
        out = wpt_tier1.area_mutated("<style>p { outline: 1px solid }</style><p>x</p>", "css-ui-outline")
        self.assertTrue(out.startswith("<style>p { outline: 1px solid }</style><p>x</p>"))
        self.assertIn("outline: none !important", out)

    def test_each_area_forces_off_what_it_asserts(self):
        for area, word in (
            ("css-masking-clip-path", "clip-path"),
            ("css-masking-mask", "mask"),
            ("css-transforms-2d", "transform"),
            ("css-transforms-3d", "transform"),
            ("css-position-sticky", "position"),
            ("css-position-absolute", "position"),
            ("css-overflow", "overflow"),
            ("css-lists", "list-style"),
            ("css-text-basics", "letter-spacing"),
        ):
            self.assertIn(f"{word}", wpt_tier1.area_mutated("", area), area)

    def test_every_imported_area_has_a_probe(self):
        self.assertEqual(sorted(wpt_tier1.AREA_PROBES), sorted(wpt_import.AREAS))


class ProbePages(ProofCase):
    """T15b: one run left 260 `.__probe-*` files in third_party/wpt."""

    def left_over(self):
        return sorted(p.name for p in self.dir.iterdir() if ".__probe-" in p.name)

    def test_probe_pages_are_on_disk_for_the_capture_and_gone_after(self):
        seen = []

        def capture(html_file, *_):
            seen.append(html_file.is_file())

        with mock.patch.object(wpt_tier1, "capture", side_effect=capture), mock.patch.object(
            wpt_tier1, "compare_frames", return_value=(5, 0.0)
        ):
            why = wpt_tier1.prove_pass(
                {"area": "css-ui-outline"}, self.page, self.ref, {"width": 1, "height": 1}, self.dir
            )
        self.assertIsNone(why)
        self.assertEqual(seen, [True, True])
        self.assertEqual(self.left_over(), [])

    def test_a_probe_that_cannot_render_leaves_nothing_either(self):
        self.prove({}, [], capture_says="capture exit 1")
        self.assertEqual(self.left_over(), [])

    def test_the_stage_cache_forgets_a_deleted_probe_page(self):
        self.prove({}, [100])
        self.assertEqual([p for p in wpt_tier1._STAGED if ".__probe-" in p.name], [])


class ZeroInk(unittest.TestCase):
    """T15b: under 480 inked pixels read as blank, so thin frames with real ink were ERRORs."""

    W, H = 100, 100

    def frame(self, dark=0):
        pixels = bytearray(b"\xff" * (self.W * self.H * 3))
        pixels[: dark * 3] = b"\x00" * (dark * 3)
        return f"P6\n{self.W} {self.H}\n255\n".encode() + bytes(pixels)

    def run_it(self, test_dark, ref_dark):
        frames = iter((test_dark, ref_dark))

        def capture(_html, out, _viewport):
            out.write_bytes(self.frame(next(frames)))

        with tempfile.TemporaryDirectory() as td, mock.patch.object(
            wpt_tier1, "capture", side_effect=capture
        ), mock.patch.object(wpt_tier1, "campaign_diff", return_value=(None, "not here")):
            return wpt_tier1.run_pair(
                pathlib.Path("t.html"), pathlib.Path("r.html"), {}, pathlib.Path(td), stage=False
            )

    def test_a_thin_frame_with_ink_is_scored_and_can_match(self):
        self.assertEqual(self.run_it(3, 3)["status"], "PASS")

    def test_a_thin_frame_with_ink_is_scored_and_can_fail(self):
        out = self.run_it(3, 4)
        self.assertEqual((out["status"], out["diff_pixels"]), ("FAIL", 1))

    def test_zero_ink_on_either_side_names_that_side(self):
        for test_dark, ref_dark, side in ((0, 3, "test"), (3, 0, "ref"), (0, 0, "test+ref")):
            out = self.run_it(test_dark, ref_dark)
            self.assertEqual(out["status"], "ERROR")
            self.assertIn(f"zero-ink frame ({side})", out["reason"])

    def test_a_full_frame_of_one_colour_has_ink(self):
        self.assertEqual(self.run_it(self.W * self.H, self.W * self.H)["status"], "PASS")


class ChromiumRecheck(unittest.TestCase):
    """T15b: tab-stop-threshold-001 differs by 0.05% here, and by 129 px in Chromium's own
    test and ref. A small FAIL Chromium cannot match either is not an engine verdict."""

    ENTRIES = [{"id": "small", "path": "a/small.html", "ref": "a/small-ref.html"}]
    VIEWPORT = {"width": 1024, "height": 768}

    def case(self, status="FAIL", diff_pct=0.05, id_="small"):
        return {"id": id_, "status": status, "diff_pct": diff_pct, "reason": None}

    def recheck(self, cases, answer):
        with mock.patch.object(wpt_tier1, "stage_html", side_effect=lambda p: p):
            wpt_tier1.recheck_small_fails(cases, self.ENTRIES, self.VIEWPORT, lambda pairs, viewport: answer)
        return cases

    def test_a_small_fail_chromium_also_differs_on_becomes_skip_with_the_reason(self):
        case = self.recheck([self.case()], ({"small": 129}, "143.0", None))[0]
        self.assertEqual(case["status"], "SKIP")
        self.assertIn("129", case["reason"])
        self.assertIn("143.0", case["reason"])
        self.assertEqual(case["chromium_diff_pixels"], 129)

    def test_a_small_fail_chromium_matches_on_stays_a_fail(self):
        case = self.recheck([self.case()], ({"small": 0}, "143.0", None))[0]
        self.assertEqual(case["status"], "FAIL")
        self.assertEqual(case["chromium_diff_pixels"], 0)

    def test_only_fails_under_the_limit_are_asked_about(self):
        asked = []
        cases = [
            self.case(diff_pct=0.1, id_="at"),
            self.case(diff_pct=5.0, id_="big"),
            self.case(status="PASS", diff_pct=0.0, id_="pass"),
        ]
        wpt_tier1.recheck_small_fails(
            cases, self.ENTRIES, self.VIEWPORT, lambda pairs, viewport: asked.extend(pairs) or ({}, "v", None)
        )
        self.assertEqual(asked, [])
        self.assertEqual([c["status"] for c in cases], ["FAIL", "FAIL", "PASS"])

    def test_the_question_is_about_the_staged_pages(self):
        asked = []
        with mock.patch.object(wpt_tier1, "stage_html", side_effect=lambda p: p.with_suffix(".s")):
            wpt_tier1.recheck_small_fails(
                [self.case()],
                self.ENTRIES,
                self.VIEWPORT,
                lambda pairs, viewport: asked.extend(pairs) or ({"small": 0}, "v", None),
            )
        self.assertEqual(asked[0]["test"], str(wpt_tier1.WPT_ROOT / "a/small.s"))
        self.assertEqual(asked[0]["ref"], str(wpt_tier1.WPT_ROOT / "a/small-ref.s"))

    def test_when_chromium_cannot_answer_the_fail_stays_and_says_why(self):
        case = self.recheck([self.case()], (None, None, "no chromium"))[0]
        self.assertEqual(case["status"], "FAIL")
        self.assertEqual(case["chromium_not_measured"], "no chromium")

    def test_a_case_missing_from_the_answer_stays_a_fail_and_says_so(self):
        case = self.recheck([self.case()], ({}, "143.0", None))[0]
        self.assertEqual(case["status"], "FAIL")
        self.assertIn("no answer", case["chromium_not_measured"])

    def test_oracle_failures_each_have_a_reason(self):
        pairs = [{"id": "a", "test": "t", "ref": "r"}]
        for proc, word in (
            (mock.Mock(returncode=1, stdout="", stderr="boom"), "boom"),
            (mock.Mock(returncode=0, stdout="not json", stderr=""), "JSON"),
            (mock.Mock(returncode=0, stdout="[1]", stderr=""), "JSON"),
        ):
            with mock.patch.object(wpt_tier1.subprocess, "run", return_value=proc), mock.patch.object(
                pathlib.Path, "is_dir", return_value=True
            ):
                diffs, _version, why = wpt_tier1.chromium_pair_diffs(pairs, self.VIEWPORT)
            self.assertIsNone(diffs)
            self.assertIn(word, why)

    def test_the_oracle_answer_is_read(self):
        out = json.dumps({"version": "143.0", "diffs": {"a": 7}})
        proc = mock.Mock(returncode=0, stdout=out, stderr="")
        with mock.patch.object(wpt_tier1.subprocess, "run", return_value=proc), mock.patch.object(
            pathlib.Path, "is_dir", return_value=True
        ):
            self.assertEqual(wpt_tier1.chromium_pair_diffs([], self.VIEWPORT), ({"a": 7}, "143.0", None))

    def test_chromium_is_asked_at_the_runs_viewport(self):
        """T15c (#857 R1): the call hard-coded 800x600 whatever the manifest said."""
        out = json.dumps({"version": "143.0", "diffs": {}})
        proc = mock.Mock(returncode=0, stdout=out, stderr="")
        with mock.patch.object(wpt_tier1.subprocess, "run", return_value=proc) as run, mock.patch.object(
            pathlib.Path, "is_dir", return_value=True
        ):
            wpt_tier1.chromium_pair_diffs([], self.VIEWPORT)
        self.assertEqual(run.call_args.args[0][-2:], ["1024", "768"])

    def test_the_recheck_passes_on_the_runs_viewport(self):
        seen = []
        with mock.patch.object(wpt_tier1, "stage_html", side_effect=lambda p: p):
            wpt_tier1.recheck_small_fails(
                [self.case()],
                self.ENTRIES,
                self.VIEWPORT,
                lambda pairs, viewport: seen.append(viewport) or ({"small": 0}, "v", None),
            )
        self.assertEqual(seen, [self.VIEWPORT])


class ChromiumFonts(unittest.TestCase):
    """T15c (#857 R1): the frame was taken on `load`, so a font still arriving was not
    in it. The page answers `document.fonts.ready` late and paints only then."""

    PAGE = """<!doctype html><style>html { background: white }</style><script>
      const late = new Promise(done => setTimeout(() => {
        document.documentElement.style.background = "rgb(0, 128, 0)";
        done();
      }, 400));
      Object.defineProperty(document.fonts, "ready", { get: () => late });
    </script>"""
    REF = '<!doctype html><style>html { background: rgb(0, 128, 0) }</style>'

    @unittest.skipUnless((wpt_tier1.ORACLE_DIR / "node_modules").is_dir(), "no tools/parity_oracle/node_modules")
    def test_the_frame_waits_for_fonts_ready(self):
        with tempfile.TemporaryDirectory() as td:
            test, ref = pathlib.Path(td) / "t.html", pathlib.Path(td) / "r.html"
            test.write_text(self.PAGE, encoding="utf-8")
            ref.write_text(self.REF, encoding="utf-8")
            pairs = [{"id": "fonts", "test": str(test), "ref": str(ref)}]
            diffs, _version, why = wpt_tier1.chromium_pair_diffs(pairs, {"width": 200, "height": 100})
        self.assertIsNone(why)
        self.assertEqual(diffs, {"fonts": 0})


class ChromiumOnAreaProbes(unittest.TestCase):
    """T15c (#857 R1): 13 of the 47 area-probe demotions differ in Chromium's own test and
    reference. Such a page says nothing either way, and its row has to say that."""

    ENTRIES = [{"id": "probed", "path": "a/p.html", "ref": "a/p-ref.html", "area": "css-ui-outline"}]
    VIEWPORT = {"width": 1024, "height": 768}
    WHY = "still matches under the css-ui-outline probe (outline: none !important)"

    def case(self, reason=WHY, status="UNPROVEN", id_="probed"):
        return {
            "id": id_, "area": "css-ui-outline", "status": status, "reason": reason,
            "chromium_diff_pixels": None, "chromium_not_measured": None,
        }

    def recheck(self, cases, answer):
        asked = []

        def oracle(pairs, viewport):
            asked.append((pairs, viewport))
            return answer

        with mock.patch.object(wpt_tier1, "stage_html", side_effect=lambda p: p):
            wpt_tier1.recheck_area_probes(cases, self.ENTRIES, self.VIEWPORT, oracle)
        return asked

    def test_a_page_chromium_cannot_match_says_it_is_no_evidence(self):
        case = self.case()
        self.recheck([case], ({"probed": 12}, "143.0", None))
        self.assertEqual(case["status"], "UNPROVEN")
        self.assertEqual(case["chromium_diff_pixels"], 12)
        self.assertIn(self.WHY, case["reason"])
        self.assertIn("143.0", case["reason"])
        self.assertIn("12 px", case["reason"])
        self.assertIn("evidence either way", case["reason"])

    def test_a_page_chromium_matches_keeps_its_reason_and_records_zero(self):
        case = self.case()
        self.recheck([case], ({"probed": 0}, "143.0", None))
        self.assertEqual((case["status"], case["reason"]), ("UNPROVEN", self.WHY))
        self.assertEqual(case["chromium_diff_pixels"], 0)

    def test_only_area_probe_demotions_are_asked_about(self):
        cases = [
            self.case("still matches with all its style removed", id_="unstyled"),
            self.case(None, status="PASS", id_="pass"),
            self.case("zero-ink frame (test)", status="ERROR", id_="error"),
        ]
        self.assertEqual(self.recheck(cases, ({}, "v", None)), [])

    def test_the_question_is_at_the_runs_viewport(self):
        asked = self.recheck([self.case()], ({"probed": 0}, "v", None))
        self.assertEqual(asked[0][1], self.VIEWPORT)

    def test_when_chromium_cannot_answer_the_row_says_why(self):
        case = self.case()
        self.recheck([case], (None, None, "no chromium"))
        self.assertEqual((case["reason"], case["chromium_not_measured"]), (self.WHY, "no chromium"))

    def test_the_run_counts_the_split_of_its_own(self):
        differ, match, unanswered = self.case(id_="d"), self.case(id_="m"), self.case(id_="u")
        differ["chromium_diff_pixels"] = 12
        match["chromium_diff_pixels"] = 0
        unanswered["chromium_not_measured"] = "timeout"
        others = [
            self.case("still matches with all its style removed", id_="unstyled"),
            self.case(None, status="FAIL", id_="small"),
        ]
        self.assertEqual(
            wpt_tier1.area_probe_chromium_counts([differ, match, unanswered] + others),
            {"asked": 3, "differ": 1, "match": 1, "not_measured": 1},
        )

    def test_a_row_that_says_no_evidence_still_counts_as_an_area_probe(self):
        case = self.case()
        self.recheck([case], ({"probed": 12}, "143.0", None))
        self.assertEqual(wpt_tier1.area_probe_chromium_counts([case])["differ"], 1)

    def test_a_gap_line_names_the_question_chromium_did_not_answer(self):
        area = self.case()
        area["chromium_not_measured"] = "timeout"
        small = self.case(None, status="FAIL", id_="small")
        small["chromium_not_measured"] = "timeout"
        self.assertEqual(
            wpt_tier1.chromium_gap_lines([area, small]),
            ["area probe not checked in Chromium: timeout (1)", "small FAIL not checked in Chromium: timeout (1)"],
        )

    def test_a_timed_out_area_batch_is_not_called_small_fails(self):
        cases = [self.case(id_=f"a{i}") for i in range(47)]
        for case in cases:
            case["chromium_not_measured"] = "timeout"
        self.assertEqual(
            wpt_tier1.chromium_gap_lines(cases), ["area probe not checked in Chromium: timeout (47)"]
        )


class StagedCopies(unittest.TestCase):
    """T15c (#857 R1): one run left 602 `.__staged` copies in third_party/wpt."""

    def setUp(self):
        td = tempfile.TemporaryDirectory()
        self.addCleanup(td.cleanup)
        self.root = pathlib.Path(td.name)
        (self.root / "a").mkdir()
        self.page = self.root / "a" / "t.html"
        self.page.write_text("<p>x</p>", encoding="utf-8")
        self.entries = [{"id": "t"}]
        patcher = mock.patch.object(wpt_tier1, "WPT_ROOT", self.root)
        patcher.start()
        self.addCleanup(patcher.stop)
        self.addCleanup(wpt_tier1._STAGED.clear)

    def left_over(self):
        return sorted(p.name for p in self.root.rglob("*") if p.is_file() and wpt_tier1.STAGED_SUFFIX in p.name)

    def staging_case(self, entry, viewport):
        wpt_tier1.stage_html(self.page)
        return {"id": entry["id"], "status": "PASS", "reason": None, "diff_pct": 0.0, "chromium_diff_pixels": None}

    def test_removal_takes_staged_copies_and_leaves_the_fixtures(self):
        stale = self.root / "a" / "old.__staged.css"
        stale.write_text("a {}", encoding="utf-8")
        staged = wpt_tier1.stage_html(self.page)
        self.assertTrue(staged.is_file())
        wpt_tier1.remove_staged()
        self.assertEqual(self.left_over(), [])
        self.assertTrue(self.page.is_file())

    def test_a_page_staged_after_removal_is_written_again(self):
        wpt_tier1.stage_html(self.page)
        wpt_tier1.remove_staged()
        self.assertTrue(wpt_tier1.stage_html(self.page).is_file())

    def test_a_run_leaves_none(self):
        with mock.patch.object(wpt_tier1, "run_case", side_effect=self.staging_case):
            cases = wpt_tier1.run_cases(self.entries, {}, jobs=1, verbose=False)
        self.assertEqual([c["id"] for c in cases], ["t"])
        self.assertEqual(self.left_over(), [])

    def test_the_recheck_still_has_its_staged_pages(self):
        seen = []

        def recheck(*_):
            seen.append(self.left_over())

        with mock.patch.object(wpt_tier1, "run_case", side_effect=self.staging_case), mock.patch.object(
            wpt_tier1, "recheck_small_fails", side_effect=recheck
        ), mock.patch.object(wpt_tier1, "recheck_area_probes"):
            wpt_tier1.run_cases(self.entries, {}, jobs=1, verbose=False)
        self.assertEqual(seen, [["t.__staged.html"]])
        self.assertEqual(self.left_over(), [])

    def test_a_run_that_dies_leaves_none(self):
        def dies(entry, viewport):
            self.staging_case(entry, viewport)
            raise RuntimeError("boom")

        with mock.patch.object(wpt_tier1, "run_case", side_effect=dies), self.assertRaises(RuntimeError):
            wpt_tier1.run_cases(self.entries, {}, jobs=1, verbose=False)
        self.assertEqual(self.left_over(), [])


class ReadmeSaysWhatDecides(unittest.TestCase):
    """T15c (#857 R1): the README said no Chrome is involved, and Chromium decides SKIP."""

    def test_the_readme_names_chromium_for_small_fails(self):
        text = (wpt_tier1.REPO_ROOT / "trench" / "wpt" / "README.md").read_text(encoding="utf-8")
        self.assertNotIn("No Chrome is involved", text)
        self.assertRegex(text, r"Chromium[^.]*\bSKIP\b")

    def test_the_run_record_names_it_too(self):
        self.assertIn("Chromium", json.dumps(wpt_tier1.oracle_record()))


class BlindSpots(unittest.TestCase):
    """T15b: UNPROVEN on these pages means the probes cannot reach what they assert."""

    def test_an_svg_transform_attribute_is_named(self):
        self.assertIn("SVG transform", wpt_tier1.probe_blind_spot('<svg><rect transform="scale(2)"/></svg>'))

    def test_a_transform_attribute_set_from_script_is_named(self):
        html = "<script>r.setAttribute('transform', 'scale(2)')</script>"
        self.assertIn("SVG transform", wpt_tier1.probe_blind_spot(html))

    def test_a_class_toggled_from_script_is_named(self):
        for line in ("e.classList.add('a')", "e.className = 'a'", "e.setAttribute('class', 'a')"):
            self.assertIn("class", wpt_tier1.probe_blind_spot(f"<script>{line}</script>"), line)

    def test_plain_pages_and_css_transforms_have_none(self):
        self.assertIsNone(wpt_tier1.probe_blind_spot('<p style="transform: scale(2)" data-transform="x">a</p>'))
        self.assertIsNone(wpt_tier1.probe_blind_spot("<p>classList is only a word here</p>"))

    def test_the_case_row_says_so(self):
        with tempfile.TemporaryDirectory() as td:
            root = pathlib.Path(td)
            (root / "t.html").write_text('<link rel="match" href="r.html"><svg><g transform="scale(2)"/></svg>', encoding="utf-8")
            (root / "r.html").write_text("<p>x</p>", encoding="utf-8")
            entry = {"id": "t", "path": "t.html", "ref": "r.html", "kind": "reftest", "area": "css-transforms-2d"}
            with mock.patch.object(wpt_tier1, "WPT_ROOT", root), mock.patch.object(
                wpt_tier1, "run_pair", return_value={"status": "PASS", "reason": None}
            ), mock.patch.object(wpt_tier1, "prove_pass", return_value="still matches"):
                case = wpt_tier1.run_case(entry, {})
        self.assertEqual(case["status"], "UNPROVEN")
        self.assertIn("still matches", case["reason"])
        self.assertIn("SVG transform", case["reason"])


class Counts(unittest.TestCase):
    CASES = [
        {"id": "a", "status": "PASS", "area": "x"},
        {"id": "b", "status": "UNPROVEN", "area": "x", "reason": "why"},
        {"id": "c", "status": "FAIL", "area": "y"},
        {"id": "d", "status": "SKIP", "area": "y"},
        {"id": "e", "status": "ERROR", "area": "y"},
    ]

    def test_an_unproven_case_leaves_the_pass_count(self):
        counts = wpt_tier1.count_statuses(self.CASES)
        self.assertEqual(counts, {"PASS": 1, "FAIL": 1, "UNPROVEN": 1, "SKIP": 1, "ERROR": 1})

    def test_the_rate_keeps_an_unproven_case_in_the_denominator(self):
        self.assertEqual(wpt_tier1.pass_rate(wpt_tier1.count_statuses(self.CASES)), round(1 / 3, 4))

    def test_per_area_table(self):
        areas = wpt_tier1.per_area(self.CASES)
        self.assertEqual(areas["x"], {"PASS": 1, "FAIL": 0, "UNPROVEN": 1, "SKIP": 0, "ERROR": 0})
        self.assertEqual(areas["y"], {"PASS": 0, "FAIL": 1, "UNPROVEN": 0, "SKIP": 1, "ERROR": 1})

    def test_unproven_passes_carries_the_reason(self):
        self.assertEqual(wpt_tier1.unproven_passes(self.CASES), {"b": "why"})

    def test_campaign_counts_score_an_unproven_case(self):
        cases = [{"status": "UNPROVEN", "campaign_diff_pct": 0.0}]
        self.assertEqual(wpt_tier1.campaign_counts(cases), {"within": 1, "over": 0, "not_measured": 0})


class Staging(unittest.TestCase):
    def test_a_shared_reference_is_staged_once(self):
        with tempfile.TemporaryDirectory() as td:
            page = pathlib.Path(td) / "ref.html"
            page.write_text("<p>one</p>", encoding="utf-8")
            first = wpt_tier1.stage_html(page)
            first_mtime = first.stat().st_mtime_ns
            page.write_text("<p>two</p>", encoding="utf-8")
            self.assertEqual(wpt_tier1.stage_html(page), first)
            self.assertEqual(first.stat().st_mtime_ns, first_mtime)


class Runnability(unittest.TestCase):
    def test_inline_script_without_a_wait_is_scored(self):
        self.assertIsNone(wpt_tier1.unrunnable_reason("<script>a()</script>", ""))

    def test_reftest_wait_stays_unscored(self):
        reason = wpt_tier1.unrunnable_reason("<html class=reftest-wait>", "")
        self.assertIn("reftest-wait", reason)
        self.assertNotIn("no script host", reason)

    def test_an_external_script_cannot_run_on_either_side(self):
        self.assertIn("external script", wpt_tier1.unrunnable_reason("<script src=a.js></script>", ""))
        self.assertIn("external script", wpt_tier1.unrunnable_reason("", '<script src="a.js"></script>'))

    def test_a_capture_failure_is_the_tools_own_error_line(self):
        self.assertEqual(
            wpt_tier1.capture_failure('{"status":"error","error":"Inline script failed"}\n', "2026 WARN x\n"),
            "Inline script failed",
        )
        self.assertEqual(wpt_tier1.capture_failure("not json\n", "last\n"), "last")
        self.assertEqual(wpt_tier1.capture_failure("", ""), "no output")

    def test_capture_turns_script_on_only_for_a_page_with_a_script(self):
        with tempfile.TemporaryDirectory() as td:
            page = pathlib.Path(td) / "p.html"
            page.write_text("<SCRIPT>a()</SCRIPT>", encoding="utf-8")
            self.assertIn("--run-inline-scripts", wpt_tier1.capture_args(page, page, {"width": 1, "height": 1}))
            page.write_text("<p>no script</p>", encoding="utf-8")
            self.assertNotIn("--run-inline-scripts", wpt_tier1.capture_args(page, page, {"width": 1, "height": 1}))


class Provenance(unittest.TestCase):
    def test_capture_binary_is_the_parity_profile(self):
        self.assertEqual(wpt_tier1.capture_bin({}), REPO / "target/parity/parity-capture")

    def test_develop_sha_is_never_head_by_default(self):
        # Its own repository: with TMPDIR inside a checkout, a bare temp dir
        # finds that checkout's origin/develop.
        with tempfile.TemporaryDirectory() as td:
            subprocess.run(["git", "init", "-q", td], check=True)
            self.assertIsNone(wpt_tier1.develop_sha_at(pathlib.Path(td)))

    def test_capture_binary_honours_cargo_target_dir(self):
        self.assertEqual(
            wpt_tier1.capture_bin({"CARGO_TARGET_DIR": "/elsewhere/t"}),
            pathlib.Path("/elsewhere/t/parity/parity-capture"),
        )

    def test_a_relative_cargo_target_dir_is_from_the_repo(self):
        self.assertEqual(
            wpt_tier1.capture_bin({"CARGO_TARGET_DIR": "t2"}), REPO / "t2/parity/parity-capture"
        )

    def test_an_empty_cargo_target_dir_is_the_default(self):
        self.assertEqual(
            wpt_tier1.capture_bin({"CARGO_TARGET_DIR": ""}), REPO / "target/parity/parity-capture"
        )

    def test_capture_binary_label_survives_a_target_outside_the_repo(self):
        self.assertEqual(
            wpt_tier1.capture_label(pathlib.Path("/elsewhere/t/parity/parity-capture")),
            "/elsewhere/t/parity/parity-capture",
        )
        self.assertEqual(
            wpt_tier1.capture_label(REPO / "target/parity/parity-capture"),
            "target/parity/parity-capture",
        )

    def test_oracle_names_both_rules(self):
        oracle = wpt_tier1.oracle_record()
        self.assertEqual(oracle["status_rule"]["max_diff_pct"], 0.0)
        self.assertEqual(oracle["campaign_rule"]["threshold_pct"], 15)
        self.assertEqual(oracle["campaign_rule"]["pixelmatch_threshold"], 0.1)
        json.dumps(oracle)

    def test_campaign_counts_never_grade_a_missing_measurement(self):
        cases = [
            {"status": "FAIL", "campaign_diff_pct": 3.0},
            {"status": "PASS", "campaign_diff_pct": 0.0},
            {"status": "FAIL", "campaign_diff_pct": 40.0},
            {"status": "FAIL", "campaign_diff_pct": None},
            {"status": "SKIP", "campaign_diff_pct": None},
        ]
        self.assertEqual(
            wpt_tier1.campaign_counts(cases), {"within": 2, "over": 1, "not_measured": 1}
        )


class Manifest(unittest.TestCase):
    def test_flexbox_support_sheet_is_listed(self):
        manifest = json.loads((REPO / "trench/wpt/MANIFEST.json").read_text(encoding="utf-8"))
        self.assertIn("css/css-flexbox/support/flexbox.css", manifest["support_paths"]["paths"])

    def test_every_manifest_page_has_its_resources_when_synced(self):
        if not wpt_tier1.WPT_ROOT.is_dir():
            self.skipTest("third_party/wpt not synced")
        manifest = json.loads((REPO / "trench/wpt/MANIFEST.json").read_text(encoding="utf-8"))
        for entry in manifest["entries"]:
            for rel in (entry["path"], entry["ref"]):
                page = wpt_tier1.WPT_ROOT / rel
                if page.exists():
                    self.assertEqual(wpt_tier1.missing_resources(page, wpt_tier1.WPT_ROOT), [], rel)

    def test_sync_fetches_without_blobs(self):
        text = (REPO / "scripts/wpt_sync.sh").read_text(encoding="utf-8")
        self.assertIn("--filter=blob:none", text)
        self.assertNotIn("NOT YET RUN", text)

    def test_sync_check_passes_or_names_what_is_missing(self):
        proc = subprocess.run(
            ["bash", str(REPO / "scripts/wpt_sync.sh"), "--dry-run"], capture_output=True, text=True, encoding="utf-8"
        )
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("css/css-flexbox/support/flexbox.css", proc.stdout)


EIGHT = {
    "css-text/overflow-wrap/overflow-wrap-004",
    "css-text/word-break/break-word-overflow-wrap-interactions",
    "css-inline/br-font-size",
    "css-inline/br-line-height",
    "css-inline/empty-text-node-001",
    "css-flexbox/align-self-015",
    "css-flexbox/align-items-baseline-overflow-non-visible",
    "css-text/word-break/break-boundary-2-chars-002",
}


class Coverage(unittest.TestCase):
    MANIFEST = json.loads((REPO / "trench/wpt/MANIFEST.json").read_text(encoding="utf-8"))

    def test_each_of_the_eight_has_a_second_mutation(self):
        marked = {e["id"] for e in self.MANIFEST["entries"] if e.get("mutations")}
        self.assertTrue(EIGHT <= marked, sorted(EIGHT - marked))

    def test_no_entry_is_marked_unproven_by_hand(self):
        self.assertEqual([e["id"] for e in self.MANIFEST["entries"] if e.get("unproven")], [])

    def test_every_named_area_has_cases_and_a_row(self):
        areas = {}
        for e in self.MANIFEST["entries"]:
            if e.get("area"):
                areas.setdefault(e["area"], []).append(e)
        self.assertEqual(sorted(areas), sorted(wpt_import.AREAS))
        for name, entries in areas.items():
            self.assertGreaterEqual(len(entries), 10, name)
            self.assertEqual({e["maps_to"] for e in entries}, {wpt_import.AREAS[name]["row"]}, name)

    def test_the_seed_is_kept(self):
        self.assertEqual(sum(1 for e in self.MANIFEST["entries"] if not e.get("area")), 30)

    def test_ids_are_unique(self):
        ids = [e["id"] for e in self.MANIFEST["entries"]]
        self.assertEqual(len(ids), len(set(ids)))

    def test_mutations_apply_to_the_page_when_synced(self):
        if not (wpt_tier1.WPT_ROOT / "css").is_dir():
            self.skipTest("third_party/wpt not synced")
        for e in self.MANIFEST["entries"]:
            text = (wpt_tier1.WPT_ROOT / e["path"]).read_text(errors="replace", encoding="utf-8")
            for m in e.get("mutations", []):
                self.assertIn(m["find"], text, e["id"])


class OneCommand(unittest.TestCase):
    def test_sync_then_build_then_run(self):
        names = [c[1] for c in wpt_coverage.steps([])]
        self.assertEqual(names, ["scripts/wpt_sync.sh", "build", "scripts/wpt_tier1.py"])

    def test_a_seat_can_leave_out_sync_and_build_and_pass_the_rest_on(self):
        planned = wpt_coverage.steps(["--no-sync", "--no-build", "--allow-stale"])
        self.assertEqual(len(planned), 1)
        self.assertEqual(planned[0][1:], ["scripts/wpt_tier1.py", "--allow-stale"])


class Importer(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.addCleanup(self.td.cleanup)
        self.root = pathlib.Path(self.td.name)

    def write(self, rel, text=""):
        target = self.root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8")

    def reftest(self, rel, ref="t-ref.html", extra=""):
        self.write(rel, f'<link rel="match" href="{ref}">{extra}<p>x</p>')
        self.write(str(pathlib.Path(rel).parent / ref), "<p>x</p>")

    def test_a_plain_reftest_is_usable_with_its_reference(self):
        self.reftest("css/a/t.html")
        self.assertEqual(wpt_import.usable_ref(self.root / "css/a/t.html", self.root), "css/a/t-ref.html")

    def test_what_the_runner_cannot_score_is_not_imported(self):
        self.reftest("css/a/w.html", extra='<html class="reftest-wait">')
        self.reftest("css/a/f.html", extra='<meta name="fuzzy" content="1">')
        self.reftest("css/a/m.html", extra='<link rel="mismatch" href="t-ref.html">')
        self.reftest("css/a/two.html", extra='<link rel="match" href="t-ref.html">')
        self.write("css/a/noref.html", "<p>x</p>")
        self.reftest("css/a/gone.html", extra='<img src="absent.png">')
        self.write("css/a/noreftarget.html", '<link rel="match" href="absent-ref.html">')
        for name in ("w", "f", "m", "two", "noref", "gone", "noreftarget"):
            self.assertIsNone(wpt_import.usable_ref(self.root / f"css/a/{name}.html", self.root), name)

    def test_candidates_leave_out_references_support_and_staged_copies(self):
        for rel in (
            "css/a/t.html",
            "css/a/t-ref.html",
            "css/a/reference/r.html",
            "css/a/support/s.html",
            "css/a/t.__staged.html",
            "css/a/t.tentative.html",
            "css/a/t.xht",
        ):
            self.write(rel)
        found = wpt_import.candidates(self.root, {"dir": "css/a", "include": "."})
        self.assertEqual(found, ["css/a/t.html"])

    def test_include_filters_on_the_path_inside_the_area(self):
        self.write("css/a/sticky/one.html")
        self.write("css/a/two.html")
        found = wpt_import.candidates(self.root, {"dir": "css/a", "include": "^sticky/"})
        self.assertEqual(found, ["css/a/sticky/one.html"])

    def test_the_spread_is_even_and_stable(self):
        names = [f"n{i:02}" for i in range(10)]
        self.assertEqual(wpt_import.spread(names, 3), ["n00", "n03", "n06"])
        self.assertEqual(wpt_import.spread(names, 20), names)

    def test_support_closure_excludes_the_test_and_ref(self):
        self.reftest("css/a/t.html", extra='<link rel="stylesheet" href="s.css">')
        self.write("css/a/s.css", "a { background: url(i.png) }")
        self.write("css/a/i.png")
        self.assertEqual(
            wpt_import.support_of(["css/a/t.html", "css/a/t-ref.html"], self.root),
            ["css/a/i.png", "css/a/s.css"],
        )


if __name__ == "__main__":
    unittest.main()
