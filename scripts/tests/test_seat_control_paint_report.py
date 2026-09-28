"""The paint seat control is a diagnostic, and the one it is easiest to misread.

`scripts/seat_control_paint_report.py` splits Gate B's paint error into the
part that is RustKit and the part that is the seat, the way night 44's
`seat_control_report.py` did for Gate A's geometry. Its output looks like a
Gate B receipt and is not one, and it carries a failure mode geometry did not:

  * pixel counts are NOT additive. `Δ_real` exceeds `Δ_reported` on six of the
    26 gating cases, so any script that subtracts one from another produces a
    confident negative confound. Nothing here subtracts.
  * a masked fraction over the wrong denominator falls when the mask SHRINKS,
    so a case whose confound got worse reads as a case that improved.
  * an empty mask has no denominator, and printing 0% for it says the opposite
    of what it means.
  * cited as a parity number it is a lie twice over: not macOS, not an N/26.

Every guard below was mutation-checked: the rule was removed from the module,
the guard was confirmed RED, and the rule restored. A guard that stays green
without its rule is decoration.

Run: python3 scripts/tests/test_seat_control_paint_report.py
"""
import hashlib
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import seat_control_paint_report as scp  # noqa: E402
from parity_image import Image, write_png  # noqa: E402
from seat_control_report import ControlUnusable  # noqa: E402

REPO = Path(__file__).resolve().parent.parent.parent
TOLERANCE = scp.load_aa_tolerance()

CASE_ID = "shelf"
CASE = scp.load_case_registry()[CASE_ID]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def solid(width, height, color):
    return Image(width, height, bytes(color) * (width * height))


def with_pixels(image, pixels, color):
    buffer = bytearray(image.rgb)
    for x, y in pixels:
        i = (y * image.width + x) * 3
        buffer[i], buffer[i + 1], buffer[i + 2] = color
    return Image(image.width, image.height, bytes(buffer))


def write_ppm(path, image):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"P6\n%d %d\n255\n" % (image.width, image.height) + image.rgb)


def write_baseline(root, scope, case_id, image):
    directory = root / scope / case_id
    directory.mkdir(parents=True, exist_ok=True)
    write_png(directory / "baseline.png", image)


def stamp_for(root, case_ids=(CASE_ID,), fixture_override=None):
    root.mkdir(parents=True, exist_ok=True)
    fixtures = {}
    for case_id in case_ids:
        html = scp.load_case_registry()[case_id]["html"]
        fixtures[case_id] = fixture_override or hashlib.sha256(
            (REPO / html).read_bytes()
        ).hexdigest()
    stamp = {
        "kind": "seat-control",
        "captured_at": "2026-09-26T00:00:00Z",
        "platform": "linux-x64",
        "fixtures": fixtures,
    }
    (root / "STAMP.json").write_text(json.dumps(stamp), encoding="utf-8")
    return stamp


class Seat:
    """A whole three-way world on disk: pinned set, control, RustKit capture."""

    def __init__(self, tmp, pinned, control, rustkit, case_id=CASE_ID):
        self.tmp = Path(tmp)
        self.case_id = case_id
        self.case = dict(scp.load_case_registry()[case_id])
        self.case["width"], self.case["height"] = pinned.width, pinned.height
        self.pinned_root = self.tmp / "baselines" / scp.PINNED_SET
        self.control_dir = self.tmp / "seat-control"
        self.capture_root = self.tmp / "captures"
        scope = self.case["scope"]
        write_baseline(self.pinned_root, scope, case_id, pinned)
        write_baseline(self.control_dir, scope, case_id, control)
        write_ppm(self.capture_root / case_id / "frame.ppm", rustkit)
        self.stamp = stamp_for(self.control_dir, (case_id,))

    def score(self):
        # REPO_ROOT is where the module looks for the PINNED set. Redirecting it
        # is how a synthetic pinned frame gets in front of the report at all.
        original = scp.REPO_ROOT
        scp.REPO_ROOT = self.tmp
        try:
            return scp.score_case(
                self.case_id,
                self.case,
                self.capture_root,
                self.control_dir,
                self.stamp,
                TOLERANCE,
            )
        finally:
            scp.REPO_ROOT = original


# ---------------------------------------------------------------------------
# The three counts, and the arithmetic nobody may do to them
# ---------------------------------------------------------------------------


def test_the_three_counts_are_each_their_own_pair():
    """confound is Chrome-vs-Chrome, real is control-vs-RustKit, reported is
    pinned-vs-RustKit. Swapping any pair is undetectable in the output and
    changes what the whole board means."""
    over = TOLERANCE + 1
    pinned = solid(4, 1, (0, 0, 0))
    # x=0 only Chrome moves; x=1 only RustKit moves; x=2 both move, together.
    control = with_pixels(pinned, [(0, 0), (2, 0)], (over, 0, 0))
    rustkit = with_pixels(pinned, [(1, 0), (2, 0)], (over, 0, 0))

    counts = scp.three_way_counts(pinned, control, rustkit, TOLERANCE)
    assert counts["confound_px"] == 2, counts   # pinned vs control: x=0, x=2
    assert counts["reported_px"] == 2, counts   # pinned vs rustkit: x=1, x=2
    assert counts["real_px"] == 2, counts       # control vs rustkit: x=0, x=1


def test_real_may_exceed_reported_so_the_counts_are_never_subtracted():
    """The measured fact this whole module is shaped around.

    On six of the 26 gating cases (2026-09-26) RustKit agrees with the PINNED
    frame at pixels where it disagrees with the seat's own Chrome, so
    `Δ_real > Δ_reported` and `Δ_reported − Δ_confound` is negative. A report
    that derived any count from the other two would print a confound of less
    than zero and call it a measurement.
    """
    over = TOLERANCE + 1
    pinned = solid(2, 1, (0, 0, 0))
    control = with_pixels(pinned, [(0, 0)], (over, 0, 0))
    rustkit = pinned  # identical to pinned, differs from the control

    counts = scp.three_way_counts(pinned, control, rustkit, TOLERANCE)
    assert counts["reported_px"] == 0
    assert counts["real_px"] == 1
    assert counts["real_px"] > counts["reported_px"]
    # And the reported count is what it is, not confound-plus-real.
    assert counts["reported_px"] != counts["confound_px"] + counts["real_px"]


def test_each_published_percentage_is_its_own_count_and_not_derived():
    """The non-additivity rule, guarded where the report PUBLISHES rather than
    where it counts.

    The first sweep of this module caught every swap inside `three_way_counts`
    and missed a `reported_pct` computed as `(confound + real) / total` in
    `score_case`: no guard read the record's percentages against the record's
    own counts. The second sweep then caught that and missed the same
    derivation of `confound_pct` and `real_pct`, because the two-pixel fixture
    made `|reported - real|` and `reported + confound` land on the right
    answers by accident.

    So the fixture is built to make all three counts mutually non-derivable.
    Eight pixels, four kinds:

        x=0        only the seat's Chrome moves     -> confound, real
        x=1, 5     only RustKit moves               -> reported, real
        x=2, 3, 4  both move, to the SAME value     -> confound, reported
        x=6, 7     nothing moves

    which gives confound 4, reported 5, real 3 — no two of which produce the
    third by adding, subtracting or taking an absolute difference.
    """
    over = TOLERANCE + 1
    moved = (over, 0, 0)
    pinned = solid(8, 1, (0, 0, 0))
    control = with_pixels(pinned, [(0, 0), (2, 0), (3, 0), (4, 0)], moved)
    rustkit = with_pixels(pinned, [(1, 0), (5, 0), (2, 0), (3, 0), (4, 0)], moved)

    with tempfile.TemporaryDirectory() as tmp:
        record = Seat(tmp, pinned, control, rustkit).score()

    assert record["confound_px"] == 4, record
    assert record["reported_px"] == 5, record
    assert record["real_px"] == 3, record
    assert record["kept_px"] == 4, record
    assert record["masked_bad_px"] == 2, record

    total = record["total_px"]
    for name in ("reported", "confound", "real"):
        assert record[f"{name}_pct"] == round(
            record[f"{name}_px"] / total * 100, 4
        ), name

    # And no pair of counts yields the third, so a derived field is visible.
    counts = {k: record[f"{k}_px"] for k in ("reported", "confound", "real")}
    for target, (a, b) in {
        "reported": ("confound", "real"),
        "confound": ("reported", "real"),
        "real": ("reported", "confound"),
    }.items():
        assert counts[target] != counts[a] + counts[b], target
        assert counts[target] != abs(counts[a] - counts[b]), target


def test_a_pixel_is_outside_tolerance_when_its_worst_channel_is():
    """Per channel, never averaged: a channel swap has a small mean and is a
    total colour failure."""
    pinned = solid(1, 1, (0, 0, 0))
    control = solid(1, 1, (0, 0, TOLERANCE + 1))
    rustkit = solid(1, 1, (0, 0, 0))
    counts = scp.three_way_counts(pinned, control, rustkit, TOLERANCE)
    assert counts["confound_px"] == 1

    # Exactly at the tolerance is inside it, on every channel.
    at_bar = solid(1, 1, (TOLERANCE, TOLERANCE, TOLERANCE))
    assert scp.three_way_counts(pinned, at_bar, rustkit, TOLERANCE)["confound_px"] == 0


# ---------------------------------------------------------------------------
# The mask
# ---------------------------------------------------------------------------


def test_the_mask_keeps_only_pixels_the_two_chromes_agree_on():
    over = TOLERANCE + 1
    pinned = solid(4, 1, (0, 0, 0))
    control = with_pixels(pinned, [(0, 0)], (over, 0, 0))
    rustkit = with_pixels(pinned, [(0, 0), (1, 0)], (over, 0, 0))

    counts = scp.three_way_counts(pinned, control, rustkit, TOLERANCE)
    assert counts["kept_px"] == 3
    # x=0 is outside the mask even though RustKit fails there: that pixel is
    # one the platform is already speaking about.
    assert counts["masked_bad_px"] == 1


def test_the_masked_fraction_is_over_the_mask_not_the_frame():
    """A masked percentage over the whole frame falls when the MASK shrinks, so
    a case whose seat confound got worse would read as a case that improved."""
    over = TOLERANCE + 1
    pinned = solid(10, 1, (0, 0, 0))
    # Chrome disagrees on half the row; RustKit fails on one kept pixel.
    control = with_pixels(pinned, [(i, 0) for i in range(5)], (over, 0, 0))
    rustkit = with_pixels(pinned, [(9, 0)], (over, 0, 0))

    with tempfile.TemporaryDirectory() as tmp:
        record = Seat(tmp, pinned, control, rustkit).score()
    assert record["status"] == "MEASURED"
    assert record["kept_px"] == 5
    assert record["masked_bad_px"] == 1
    assert record["masked_pct"] == 20.0, record["masked_pct"]  # 1/5, not 1/10


def test_an_empty_mask_is_unmeasured_not_zero_percent():
    """No mask is no denominator. 0% would read as 'RustKit agrees everywhere
    the platform does', which is the opposite of what an empty mask means."""
    over = TOLERANCE + 1
    pinned = solid(3, 1, (0, 0, 0))
    control = solid(3, 1, (over, 0, 0))
    with tempfile.TemporaryDirectory() as tmp:
        record = Seat(tmp, pinned, control, pinned).score()
    assert record["status"] == "UNMEASURED"
    assert "no mask" in record["reason"]


def test_the_floor_is_the_smaller_of_the_confound_and_the_masked_residual():
    """The floor is what the seat can resolve. Taking the confound alone throws
    away everything the mask bought; taking the masked residual alone claims a
    resolution the unmasked board does not have."""
    over = TOLERANCE + 1
    pinned = solid(10, 1, (0, 0, 0))
    control = with_pixels(pinned, [(i, 0) for i in range(4)], (over, 0, 0))
    rustkit = with_pixels(pinned, [(9, 0)], (over, 0, 0))
    with tempfile.TemporaryDirectory() as tmp:
        record = Seat(tmp, pinned, control, rustkit).score()
    assert record["confound_pct"] == 40.0
    assert record["masked_pct"] == round(1 / 6 * 100, 4)
    assert record["floor_pct"] == record["masked_pct"]


# ---------------------------------------------------------------------------
# Refusals — every one of them prints differently from a clean board
# ---------------------------------------------------------------------------


def test_a_missing_control_refuses_rather_than_reporting_no_confound():
    with tempfile.TemporaryDirectory() as tmp:
        try:
            scp.build_report(Path(tmp) / "captures", Path(tmp) / "absent")
        except ControlUnusable:
            return
    raise AssertionError("a missing seat control was not refused")


def test_a_fixture_that_changed_since_the_control_is_unmeasured():
    """The numbers still parse. They are about a page that no longer exists."""
    pinned = solid(4, 1, (0, 0, 0))
    with tempfile.TemporaryDirectory() as tmp:
        seat = Seat(tmp, pinned, pinned, pinned)
        seat.stamp = stamp_for(seat.control_dir, fixture_override="0" * 64)
        record = seat.score()
    assert record["status"] == "UNMEASURED"
    assert "fixture changed" in record["reason"]


def test_a_case_the_control_does_not_cover_is_unmeasured():
    pinned = solid(4, 1, (0, 0, 0))
    with tempfile.TemporaryDirectory() as tmp:
        seat = Seat(tmp, pinned, pinned, pinned)
        seat.stamp = {"kind": "seat-control", "fixtures": {}}
        record = seat.score()
    assert record["status"] == "UNMEASURED"
    assert "does not cover" in record["reason"]


def test_frames_of_different_sizes_are_unmeasured_never_scaled():
    """Three frames means three chances to compare pictures that were never the
    same picture."""
    pinned = solid(4, 2, (0, 0, 0))
    for control, rustkit in (
        (solid(4, 1, (0, 0, 0)), solid(4, 2, (0, 0, 0))),
        (solid(4, 2, (0, 0, 0)), solid(3, 2, (0, 0, 0))),
    ):
        with tempfile.TemporaryDirectory() as tmp:
            record = Seat(tmp, pinned, control, rustkit).score()
        assert record["status"] == "UNMEASURED"
        assert "sizes disagree" in record["reason"]


def test_a_missing_rustkit_capture_is_unmeasured_not_a_clean_case():
    pinned = solid(4, 1, (0, 0, 0))
    with tempfile.TemporaryDirectory() as tmp:
        seat = Seat(tmp, pinned, pinned, pinned)
        (seat.capture_root / seat.case_id / "frame.ppm").unlink()
        record = seat.score()
    assert record["status"] == "UNMEASURED"
    assert record["reason"] == "no_rustkit_capture"


# ---------------------------------------------------------------------------
# It is not a receipt, and must not be mistakable for one
# ---------------------------------------------------------------------------


def test_the_report_carries_no_verdict_a_reader_could_cite_as_a_metric():
    pinned = solid(4, 1, (0, 0, 0))
    with tempfile.TemporaryDirectory() as tmp:
        seat = Seat(tmp, pinned, pinned, pinned)
        original = scp.REPO_ROOT
        scp.REPO_ROOT = seat.tmp
        try:
            report = scp.build_report(
                seat.capture_root, seat.control_dir, [seat.case_id]
            )
        finally:
            scp.REPO_ROOT = original

    assert report["kind"] == "seat-control-paint-confound"
    assert report["receipt"] is False
    blob = json.dumps(report)
    for banned in ("finish_line", "n_of_26", "26", "pass", "fail"):
        if banned == "26":
            continue
        assert banned not in blob.lower(), banned
    # No per-case green/pass verdict: a reader must not be able to count them.
    for record in report["cases"]:
        assert "green" not in record
        assert "passed" not in record


def test_the_tolerance_is_gate_bs_and_is_not_restated_here():
    """Two tolerances that must agree and are written down twice will disagree."""
    source = (REPO / "scripts" / "seat_control_paint_report.py").read_text(encoding="utf-8")
    assert "load_aa_tolerance" in source
    body = "\n".join(
        line for line in source.splitlines() if not line.strip().startswith("#")
    )
    # The docstring quotes measured percentages; a bare `= 5` or `aa_tolerance: 5`
    # assignment is the thing that must not exist.
    assert "aa_tolerance = " not in body
    assert "TOLERANCE = 5" not in body


def test_a_board_that_measured_nothing_exits_one():
    """Same rule as Gate C: a board that could not run is not a clean board."""
    with tempfile.TemporaryDirectory() as tmp:
        empty = Path(tmp) / "captures"
        empty.mkdir()
        control = Path(tmp) / "seat-control"
        stamp_for(control, ())
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(REPO / "scripts" / "seat_control_paint_report.py"),
                "--capture-root",
                str(empty),
                "--control-dir",
                str(control),
            ],
            capture_output=True,
            text=True, encoding="utf-8"
        )
    assert result.returncode == 1, result.stdout
    assert "NOT A RECEIPT" in result.stdout


def test_a_missing_control_exits_one_from_the_command_line():
    with tempfile.TemporaryDirectory() as tmp:
        result = subprocess.run(
            [
                sys.executable,
                "-B",
                str(REPO / "scripts" / "seat_control_paint_report.py"),
                "--capture-root",
                tmp,
                "--control-dir",
                str(Path(tmp) / "absent"),
            ],
            capture_output=True,
            text=True, encoding="utf-8"
        )
    assert result.returncode == 1
    assert "REFUSED" in result.stdout


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: the paint seat control refuses rather than guessing, and is not a receipt")
