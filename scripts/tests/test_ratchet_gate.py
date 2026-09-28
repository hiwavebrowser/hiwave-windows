"""The ratchet must fail on regression, hold on equal red, and never gate blind.

Every rule in ratchet_gate.compare gets a probe in BOTH directions where the
rule has two sides (the paint variance band). The exit-code split is the
load-bearing contract: 1 = regression (the only workflow-failing code),
2 = absolute red without regression, 0 = clean or RATCHET OFF.
"""
import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

RATCHET = Path(__file__).resolve().parents[1] / "ratchet_gate.py"


def gate_a(cases):
    # Production schema: geometry_failures / join_failures are COUNTS
    # (pinned against a verbatim excerpt of real gate output below).
    return {"gate": "A", "cases": [
        {"case_id": cid, "measured": m.get("measured", True),
         "green": m.get("green", not m.get("geo", [])),
         "geometry_failures": len(m.get("geo", [])),
         "join_failures": len(m.get("join", []))}
        for cid, m in cases.items()]}


def gate_b(cases):
    # Production schema: per-case fraction is within_fraction; discrete
    # failures live in failures[] flagged "discrete": true, alongside a
    # non-discrete paint_below_bar row this builder includes so tests
    # cannot pass by treating every failures[] entry as an id.
    #
    # `withheld` is OMITTED unless a case asks for it, so every pre-existing
    # probe keeps exercising the old-floor path (no jurisdiction recorded)
    # rather than being silently moved onto the new one.
    out = []
    for cid, m in cases.items():
        row = {
            "case_id": cid, "measured": m.get("measured", True),
            "green": m.get("green", not m.get("disc", []) and m.get("pct", 1.0) >= 0.99),
            "within_fraction": m.get("pct", 1.0),
            "discrete_failures": len(m.get("disc", [])),
            "failures": (
                [{"kind": "paint_below_bar", "selector": None, "discrete": False}]
                if m.get("pct", 1.0) < 0.99 else []
            ) + [{"kind": k, "selector": s, "discrete": True}
                 for k, s in m.get("disc", [])],
        }
        if "withheld" in m:
            row["discrete_withheld_selectors"] = m["withheld"]
        out.append(row)
    return {"gate": "B", "cases": out}


def run(tmp, a, b, baseline=None, extra=None):
    ap, bp = Path(tmp) / "a.json", Path(tmp) / "b.json"
    ap.write_text(json.dumps(a), encoding="utf-8")
    bp.write_text(json.dumps(b), encoding="utf-8")
    cmd = [sys.executable, str(RATCHET), "--gate-a", str(ap), "--gate-b", str(bp)]
    blp = Path(tmp) / "ratchet.json"
    if baseline is not None:
        blp.write_text(json.dumps(baseline), encoding="utf-8")
    cmd += ["--baseline", str(blp)]
    cmd += extra or []
    p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8")
    return p.returncode, p.stdout + p.stderr


SEED_PROV = ["--engine-sha", "deadbeef", "--receipt-run", "12345",
             "--stability-runs", "3"]


def seed_from(tmp, a, b):
    rc, _ = run(tmp, a, b, baseline=None,
                extra=["--write-seed", str(Path(tmp) / "ratchet.json")] + SEED_PROV)
    assert rc == 0
    return json.loads((Path(tmp) / "ratchet.json").read_text(encoding="utf-8"))


BASE_A = {"clean": {}, "red": {"geo": ["s1", "s2"], "green": False}}
BASE_B = {"clean": {}, "red": {"pct": 0.8, "disc": [("missing_clip", "s1")],
                               "green": False}}


class RatchetTests(unittest.TestCase):
    def _seeded(self, tmp):
        return seed_from(tmp, gate_a(BASE_A), gate_b(BASE_B))

    def test_hold_equal_red_exits_2(self):
        with tempfile.TemporaryDirectory() as t:
            rc, out = run(t, gate_a(BASE_A), gate_b(BASE_B), self._seeded(t))
            self.assertEqual(rc, 2, out)
            self.assertIn("none worse", out)

    def test_geometry_count_up_exits_1(self):
        with tempfile.TemporaryDirectory() as t:
            a = {"clean": {}, "red": {"geo": ["s1", "s2", "s3"], "green": False}}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), self._seeded(t))
            self.assertEqual(rc, 1, out)
            self.assertIn("geometry failures 2 -> 3", out)

    def test_join_count_up_exits_1(self):
        with tempfile.TemporaryDirectory() as t:
            a = {"clean": {"join": ["j1"]}, "red": BASE_A["red"]}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), self._seeded(t))
            self.assertEqual(rc, 1, out)
            self.assertIn("join failures", out)

    def test_new_discrete_id_exits_1(self):
        with tempfile.TemporaryDirectory() as t:
            b = {"clean": {}, "red": {"pct": 0.8, "green": False,
                                      "disc": [("missing_clip", "s1"),
                                               ("wrong_solid_color", "s9")]}}
            rc, out = run(t, gate_a(BASE_A), gate_b(b), self._seeded(t))
            self.assertEqual(rc, 1, out)
            self.assertIn("NEW discrete failure wrong_solid_color::s9", out)

    def test_paint_below_band_exits_1_within_band_holds(self):
        with tempfile.TemporaryDirectory() as t:
            seeded = self._seeded(t)
            below = {"clean": {}, "red": {"pct": 0.6, "green": False,
                                          "disc": [("missing_clip", "s1")]}}
            rc, out = run(t, gate_a(BASE_A), gate_b(below), seeded)
            self.assertEqual(rc, 1, out)
        with tempfile.TemporaryDirectory() as t:
            seeded = self._seeded(t)
            within = {"clean": {}, "red": {"pct": 0.79, "green": False,
                                           "disc": [("missing_clip", "s1")]}}
            rc, out = run(t, gate_a(BASE_A), gate_b(within), seeded)
            self.assertEqual(rc, 2, out)  # 0.01 drop < 0.1 band: not a regression

    def test_green_to_red_flip_exits_1(self):
        with tempfile.TemporaryDirectory() as t:
            a = {"clean": {"geo": [], "green": False}, "red": BASE_A["red"]}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), self._seeded(t))
            self.assertEqual(rc, 1, out)
            self.assertIn("geometry_green flipped green -> red", out)

    def test_unmeasured_when_baseline_measured_exits_1(self):
        with tempfile.TemporaryDirectory() as t:
            a = {"clean": {}, "red": {"measured": False, "green": False}}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), self._seeded(t))
            self.assertEqual(rc, 1, out)
            self.assertIn("UNMEASURED now", out)

    def test_improvement_reports_tighten_and_does_not_fail(self):
        with tempfile.TemporaryDirectory() as t:
            a = {"clean": {}, "red": {"geo": ["s1"], "green": False}}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), self._seeded(t))
            self.assertEqual(rc, 2, out)
            self.assertIn("tighten-eligible", out)
            self.assertIn("red", out)

    def test_no_baseline_is_ratchet_off_exit_0(self):
        with tempfile.TemporaryDirectory() as t:
            rc, out = run(t, gate_a(BASE_A), gate_b(BASE_B), baseline=None)
            self.assertEqual(rc, 0, out)
            self.assertIn("RATCHET OFF", out)

    def test_missing_gate_report_fails_loud(self):
        with tempfile.TemporaryDirectory() as t:
            blp = Path(t) / "ratchet.json"
            blp.write_text(json.dumps({"schema": 1, "cases": {}}), encoding="utf-8")
            p = subprocess.run(
                [sys.executable, str(RATCHET), "--gate-a", str(Path(t) / "nope.json"),
                 "--gate-b", str(Path(t) / "b.json"), "--baseline", str(blp)],
                capture_output=True, text=True, encoding="utf-8")
            self.assertEqual(p.returncode, 1, p.stdout)
            self.assertIn("did not run", p.stdout)

    def test_seed_round_trip_holds(self):
        with tempfile.TemporaryDirectory() as t:
            rc, out = run(t, gate_a(BASE_A), gate_b(BASE_B), self._seeded(t))
            self.assertIn(rc, (0, 2), out)
            self.assertNotIn("REGRESSION", out)


class SeedLawTests(unittest.TestCase):
    def test_seed_without_provenance_refused(self):
        with tempfile.TemporaryDirectory() as t:
            rc, out = run(t, gate_a(BASE_A), gate_b(BASE_B), baseline=None,
                          extra=["--write-seed", str(Path(t) / "s.json")])
            self.assertEqual(rc, 1, out)
            self.assertIn("provenance missing", out)
            self.assertFalse((Path(t) / "s.json").exists())

    def test_seed_below_three_runs_refused(self):
        with tempfile.TemporaryDirectory() as t:
            rc, out = run(t, gate_a(BASE_A), gate_b(BASE_B), baseline=None,
                          extra=["--write-seed", str(Path(t) / "s.json"),
                                 "--engine-sha", "x", "--receipt-run", "y",
                                 "--stability-runs", "1"])
            self.assertEqual(rc, 1, out)
            self.assertIn("< 3", out)

    def test_seed_carries_provenance(self):
        with tempfile.TemporaryDirectory() as t:
            seed = seed_from(t, gate_a(BASE_A), gate_b(BASE_B))
            self.assertEqual(seed["provenance"]["engine_sha"], "deadbeef")
            self.assertEqual(seed["provenance"]["stability_runs"], 3)


class NewlyMeasurableTests(unittest.TestCase):
    """A ratchet must not read a widened jurisdiction as a regression.

    Gate B only speaks about elements whose geometry Gate A would call exact,
    so every geometry fix ENLARGES the set it may report on. A defect that was
    always there then appears for the first time. Measured on 2026-08-30:
    develop's `gradient-backgrounds .linear-6` corner notch — withheld on
    2026-08-12 because the card was 18px out of place — read as a REGRESSION
    against master's floor while nothing had regressed, and would have
    red-locked the develop->master promote.

    The floor therefore carries the jurisdiction, and the direction of every
    rule below is the point: the classification may only ever soften a NEW id
    the baseline could not have seen, never one it could.
    """

    # A floor whose run examined `s1` and withheld `s9`.
    BASE_B_J = {"clean": {"withheld": []},
                "red": {"pct": 0.8, "green": False,
                        "disc": [("missing_clip", "s1")],
                        "withheld": ["s9"]}}

    def _seeded(self, tmp):
        return seed_from(tmp, gate_a(BASE_A), gate_b(self.BASE_B_J))

    def test_a_defect_on_a_baseline_withheld_element_is_not_a_regression(self):
        with tempfile.TemporaryDirectory() as t:
            now = {"clean": {"withheld": []},
                   "red": {"pct": 0.8, "green": False,
                           "disc": [("missing_clip", "s1"),
                                    ("missing_clip", "s9")],
                           "withheld": []}}
            rc, out = run(t, gate_a(BASE_A), gate_b(now), self._seeded(t))
            self.assertEqual(rc, 2, out)
            self.assertNotIn("REGRESSION", out)
            self.assertIn("newly-measurable", out)
            self.assertIn("missing_clip::s9", out)
            self.assertIn("RATCHET tighten-eligible", out)

    def test_a_defect_on_a_baseline_ADMITTED_element_is_still_a_regression(self):
        """The other direction, and the one the rule must not swallow.

        `s7` was inside the baseline's jurisdiction and clean; a failure on it
        now is a real regression. Without this probe the softening rule could
        be written to pass everything and the test above would not notice.
        """
        with tempfile.TemporaryDirectory() as t:
            now = {"clean": {"withheld": []},
                   "red": {"pct": 0.8, "green": False,
                           "disc": [("missing_clip", "s1"),
                                    ("wrong_solid_color", "s7")],
                           "withheld": ["s9"]}}
            rc, out = run(t, gate_a(BASE_A), gate_b(now), self._seeded(t))
            self.assertEqual(rc, 1, out)
            self.assertIn("NEW discrete failure wrong_solid_color::s7", out)

    def test_a_floor_that_cannot_answer_fails_loud_and_names_the_remedy(self):
        """An old floor carries no jurisdiction. Unknown is not permission.

        #167's committed seed is exactly this shape. The safe direction is to
        keep failing — and to say why, so a reader knows a re-seed resolves it
        rather than assuming the gate is broken.
        """
        with tempfile.TemporaryDirectory() as t:
            old_floor = seed_from(t, gate_a(BASE_A), gate_b(BASE_B))
            self.assertIsNone(old_floor["cases"]["red"]["discrete_withheld"])
            now = {"clean": {"withheld": []},
                   "red": {"pct": 0.8, "green": False,
                           "disc": [("missing_clip", "s1"),
                                    ("missing_clip", "s9")],
                           "withheld": []}}
            rc, out = run(t, gate_a(BASE_A), gate_b(now), old_floor)
            self.assertEqual(rc, 1, out)
            self.assertIn("floor predates discrete_withheld", out)

    def test_an_id_that_vanished_because_we_stopped_looking_is_not_a_tighten(self):
        """The mirror image, and the one that would bake a lie into a floor.

        `s1` is absent now only because the element left Gate B's
        jurisdiction. Calling that an improvement invites a floor commit
        recording zero discrete failures on evidence nobody collected.
        """
        with tempfile.TemporaryDirectory() as t:
            now = {"clean": {"withheld": []},
                   "red": {"pct": 0.8, "green": False, "disc": [],
                           "withheld": ["s1", "s9"]}}
            rc, out = run(t, gate_a(BASE_A), gate_b(now), self._seeded(t))
            self.assertIn("newly-UNMEASURABLE", out)
            self.assertIn("missing_clip::s1", out)
            self.assertNotIn("RATCHET tighten-eligible", out)

    def test_an_unknown_CURRENT_jurisdiction_cannot_claim_anything_was_fixed(self):
        """The survivor from the 2026-08-31 sweep, and it is the same shape.

        Every other probe here supplies the jurisdiction on BOTH sides, so the
        unknown-current path was untested and `fixed, withdrawn = set(), gone`
        could be reversed with the suite still green. It is reachable: a Gate B
        that did not publish the set — an older gate binary, or a case it never
        opened — produces exactly this row, and the reversed form would let a
        floor be tightened to zero discrete failures on no evidence at all.
        """
        with tempfile.TemporaryDirectory() as t:
            now = {"clean": {"withheld": []},
                   "red": {"pct": 0.8, "green": False, "disc": []}}  # no withheld
            rc, out = run(t, gate_a(BASE_A), gate_b(now), self._seeded(t))
            self.assertIn("newly-UNMEASURABLE", out)
            self.assertIn("missing_clip::s1", out)
            self.assertNotIn("RATCHET tighten-eligible", out)

    def test_an_id_that_was_actually_fixed_still_reads_as_tighten(self):
        """Same shape as above with the element still in jurisdiction."""
        with tempfile.TemporaryDirectory() as t:
            now = {"clean": {"withheld": []},
                   "red": {"pct": 0.8, "green": False, "disc": [],
                           "withheld": ["s9"]}}
            rc, out = run(t, gate_a(BASE_A), gate_b(now), self._seeded(t))
            self.assertIn("RATCHET tighten-eligible", out)
            self.assertNotIn("newly-UNMEASURABLE", out)

    def test_the_id_is_split_on_the_first_separator(self):
        """A selector may contain `::`. Splitting on the last one renames the
        element the ratchet is about to make a decision about."""
        sys.path.insert(0, str(RATCHET.parent))
        import ratchet_gate

        self.assertEqual(
            ratchet_gate.selector_of("missing_clip::div.card::before"),
            "div.card::before",
        )

    def test_withheld_none_is_never_coerced_to_an_empty_set(self):
        sys.path.insert(0, str(RATCHET.parent))
        import ratchet_gate

        self.assertIsNone(ratchet_gate.withheld_selectors({"case_id": "x"}))
        self.assertIsNone(
            ratchet_gate.withheld_selectors(
                {"case_id": "x", "discrete_withheld_selectors": None}
            )
        )
        self.assertEqual(
            ratchet_gate.withheld_selectors(
                {"case_id": "x", "discrete_withheld_selectors": ["b", "a"]}
            ),
            ["a", "b"],
        )


class GeometryBandTests(unittest.TestCase):
    """The band exists; it is OFF by default, and that is the measured choice.

    Paint has a variance band because Chrome is not bit-stable against itself.
    Gate A reads `layout.json` — layout output, not pixels — so on one binary
    and one font stack its failure count is deterministic (measured
    2026-08-31, 3 identical captures, byte-identical layout dumps). A band
    defaulting to 1 would absorb no jitter and hide one real regressed box per
    case, so the default is 0 and a floor may raise it explicitly.
    """

    def test_one_count_over_is_a_regression_by_default(self):
        with tempfile.TemporaryDirectory() as t:
            seeded = seed_from(t, gate_a(BASE_A), gate_b(BASE_B))
            self.assertEqual(seeded["geometry_band"], 0)
            a = {"clean": {}, "red": {"geo": ["s1", "s2", "s3"], "green": False}}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), seeded)
            self.assertEqual(rc, 1, out)

    def test_a_floor_may_raise_the_band_and_it_is_read_from_the_floor(self):
        with tempfile.TemporaryDirectory() as t:
            seeded = seed_from(t, gate_a(BASE_A), gate_b(BASE_B))
            seeded["geometry_band"] = 1
            a = {"clean": {}, "red": {"geo": ["s1", "s2", "s3"], "green": False}}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), seeded)
            self.assertEqual(rc, 2, out)
            # and one past the raised band still fails
            a2 = {"clean": {}, "red": {"geo": ["s1", "s2", "s3", "s4"],
                                       "green": False}}
            rc2, out2 = run(t, gate_a(a2), gate_b(BASE_B), seeded)
            self.assertEqual(rc2, 1, out2)

    def test_the_band_covers_join_failures_too(self):
        with tempfile.TemporaryDirectory() as t:
            seeded = seed_from(t, gate_a(BASE_A), gate_b(BASE_B))
            seeded["geometry_band"] = 1
            a = {"clean": {"join": ["j1"]}, "red": BASE_A["red"]}
            rc, out = run(t, gate_a(a), gate_b(BASE_B), seeded)
            self.assertNotIn("join failures", out)
            a2 = {"clean": {"join": ["j1", "j2"]}, "red": BASE_A["red"]}
            rc2, out2 = run(t, gate_a(a2), gate_b(BASE_B), seeded)
            self.assertEqual(rc2, 1, out2)
            self.assertIn("join failures", out2)


FIXTURES = Path(__file__).parent / "fixtures"


class ProductionSchemaTests(unittest.TestCase):
    """Verbatim excerpts of REAL gate output (master run 31624231006).

    The first version of the ratchet crashed on its first contact with
    production data because every synthetic fixture encoded the author's
    misreading of the schema (lists where production emits counts). These
    excerpts pin the real shape; if a gate's schema changes, this fails
    before CI does.
    """

    def test_production_schema_excerpt(self):
        with tempfile.TemporaryDirectory() as t:
            a = json.loads((FIXTURES / "gate-a-excerpt.json").read_text(encoding="utf-8"))
            b = json.loads((FIXTURES / "gate-b-excerpt.json").read_text(encoding="utf-8"))
            # seed from the excerpt, then hold against itself: exercises
            # snapshot() + compare() on the real shape end to end
            rc, out = run(t, a, b, baseline=None,
                          extra=["--write-seed", str(Path(t) / "ratchet.json")]
                          + SEED_PROV)
            self.assertEqual(rc, 0, out)
            seed = json.loads((Path(t) / "ratchet.json").read_text(encoding="utf-8"))
            about = seed["cases"]["about"]
            self.assertGreater(about["geometry_fail_count"], 0)
            self.assertIsInstance(about["geometry_fail_count"], int)
            self.assertGreater(about["join_fail_count"], 0)
            self.assertLess(about["paint_pct"], 0.99)
            self.assertTrue(seed["cases"]["bg-pure"]["paint_green"])
            rc, out = run(t, a, b, baseline=seed)
            self.assertEqual(rc, 2, out)
            self.assertNotIn("REGRESSION", out)


if __name__ == "__main__":
    unittest.main()
