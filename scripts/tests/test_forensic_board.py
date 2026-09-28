"""Gate C must publish, and must never quietly not publish.

Run: python3 scripts/tests/test_forensic_board.py

Gate C is the only gate whose numbers cannot fail a PR, which makes it the one
gate that can rot without anyone noticing — the way you would notice a forensic
board has stopped being published is by reading it. So the tests are organised
around the ways a non-gating instrument goes bad:

  * "non-gating" quietly becoming "always exits 0", including when it
    observed nothing at all
  * the tolerance sweep growing its own constants instead of deriving them
    from the one pinned in docs/VISUAL_DIFF_POLICY.md
  * the board disagreeing with Gate B about the same frame (off-by-one on a
    threshold that is `>` in one file and `>=` in the other)
  * worst-N ranking on raw pixels, so every case's worst tile is the same
    block of antialiased text forever and the board stops being read
  * tile attribution naming `html > body`, which is true and useless
  * cases that failed to load being dropped from the board rather than
    reported, so a shrinking board reads as a complete one

The structural-vs-noise tests use a real committed Chrome baseline with a
defect injected into a copy, not a hand-drawn fixture. Night 1's lesson was
that unit tests stayed green while three real elements silently fell out of
the join, and only the corpus caught it.
"""
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from parity_image import Image, read_png, write_png  # noqa: E402
from paint_oracle_gate import count_outside_tolerance, load_aa_tolerance  # noqa: E402
from forensic_board import (  # noqa: E402
    DEFAULT_WORST_TILES,
    SWEEP_MULTIPLIERS,
    TILE_PX,
    analyse_case,
    attribute_tile,
    board_ran,
    build_board,
    build_heatmap_lut,
    count_above,
    delta_histogram,
    delta_map,
    render_markdown,
    sweep_shape,
    tile_stats,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
BASELINES = REPO_ROOT / "baselines" / "chrome-148"
TOLERANCE = load_aa_tolerance()

# Small enough that a pure-Python per-pixel pass is fast, real enough that the
# attribution tests have a genuine element tree to resolve against.
SMALL_CASE = ("shelf", "builtins")


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def solid(width, height, color):
    return Image(width, height, bytes(color) * (width * height))


def write_ppm(path: Path, image: Image) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(
        f"P6\n{image.width} {image.height}\n255\n".encode("ascii") + image.rgb
    )


def load_small_baseline():
    case_id, scope = SMALL_CASE
    return case_id, scope, read_png(BASELINES / scope / case_id / "baseline.png")


def small_case_elements():
    case_id, scope = SMALL_CASE
    with open(BASELINES / scope / case_id / "layout-rects.json", encoding="utf-8") as handle:
        return json.load(handle)["elements"]


def perturbed(image: Image, box, delta):
    """Copy of `image` with `delta` added to every channel inside `box`."""
    x0, y0, x1, y1 = box
    rgb = bytearray(image.rgb)
    for y in range(y0, y1):
        for x in range(x0, x1):
            i = (y * image.width + x) * 3
            for c in range(3):
                rgb[i + c] = max(0, min(255, rgb[i + c] + delta))
    return Image(image.width, image.height, bytes(rgb))


def staged_run(case_id, scope, rustkit: Image, root: Path):
    """Put a frame where find_frame will look for it, at the registry viewport."""
    from forensic_board import load_case_registry

    spec = load_case_registry()[case_id]
    path = root / case_id / f"{spec['width']}x{spec['height']}" / "iter-1" / "capture" / "frame.ppm"
    write_ppm(path, rustkit)
    return path


# ---------------------------------------------------------------------------
# Non-gating is not the same as always-green
# ---------------------------------------------------------------------------


def test_board_that_measured_nothing_is_not_a_clean_run():
    assert not board_ran({"summary": {"measured": 0, "total_cases": 26}})
    assert board_ran({"summary": {"measured": 1, "total_cases": 26}})


def test_process_exits_nonzero_when_it_measured_nothing():
    """The whole point of the file. A board covering zero cases published
    nothing, and publishing nothing must not report clean."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        result = subprocess.run(
            [
                sys.executable,
                str(REPO_ROOT / "scripts" / "forensic_board.py"),
                "--capture-root", str(tmp / "empty"),
                "--out", str(tmp / "out"),
            ],
            capture_output=True,
            text=True, encoding="utf-8"
        )
    assert result.returncode == 1, result.stdout + result.stderr
    assert "DID NOT RUN" in result.stderr


def test_process_exits_zero_when_the_numbers_are_catastrophic():
    """Non-gating, enforced end to end: a frame that agrees with Chrome on
    nothing at all still exits 0. If this ever goes to 1, Gate C has started
    gating and the mean-pixel-diff is back in the acceptance path."""
    case_id, scope, chrome = load_small_baseline()
    inverted = Image(
        chrome.width, chrome.height, bytes(255 - b for b in chrome.rgb)
    )
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        staged_run(case_id, scope, inverted, tmp / "run")
        result = subprocess.run(
            [
                sys.executable,
                str(REPO_ROOT / "scripts" / "forensic_board.py"),
                "--capture-root", str(tmp / "run"),
                "--out", str(tmp / "out"),
                "--case", case_id,
                "--no-heatmaps",
            ],
            capture_output=True,
            text=True, encoding="utf-8"
        )
        board = json.loads((tmp / "out" / "board.json").read_text(encoding="utf-8"))
    assert result.returncode == 0, result.stdout + result.stderr
    assert "PUBLISHED" in result.stdout
    # And it really was catastrophic — otherwise this test passes for the
    # wrong reason and stops proving anything.
    assert board["cases"][0]["raw_diff_pct"] > 90.0
    assert board["gating"] is False


def test_unmeasured_cases_stay_on_the_board_and_are_labelled_not_a_pass():
    with tempfile.TemporaryDirectory() as tmp:
        report = build_board(Path(tmp) / "empty", None, case_ids=["shelf"])
    assert report["summary"]["total_cases"] == 1
    assert report["summary"]["measured"] == 0
    assert report["cases"][0]["measured"] is False
    markdown = render_markdown(report)
    assert "not a pass" in markdown
    assert "shelf" in markdown


# ---------------------------------------------------------------------------
# The sweep derives from the one pinned constant
# ---------------------------------------------------------------------------


def test_sweep_thresholds_are_multiples_of_the_pinned_tolerance():
    """Plan §2 permits exactly one tolerance to exist. A sweep with its own
    literals would be three more of them, and the argument about which number
    was in force starts again."""
    case_id, scope, chrome = load_small_baseline()
    record, _ = analyse_case(
        case_id, scope, chrome, chrome, [], TOLERANCE, DEFAULT_WORST_TILES
    )
    for multiplier in SWEEP_MULTIPLIERS:
        assert record["sweep"][str(multiplier)]["threshold"] == TOLERANCE * multiplier


def test_sweep_follows_the_tolerance_when_it_moves():
    case_id, scope, chrome = load_small_baseline()
    moved = TOLERANCE + 7
    record, _ = analyse_case(
        case_id, scope, chrome, chrome, [], moved, DEFAULT_WORST_TILES
    )
    for multiplier in SWEEP_MULTIPLIERS:
        assert record["sweep"][str(multiplier)]["threshold"] == moved * multiplier


def test_board_and_gate_b_agree_on_the_same_frame():
    """`count_above` is strictly-greater, matching Gate B's `> tolerance`. An
    off-by-one here makes the board and the gate disagree about one frame, and
    the board loses that argument for no reason at all."""
    case_id, scope, chrome = load_small_baseline()
    box = (0, 0, 60, 40)
    rustkit = perturbed(chrome, box, TOLERANCE + 3)
    hist = delta_histogram(delta_map(chrome, rustkit))
    assert count_above(hist, TOLERANCE) == count_outside_tolerance(
        chrome, rustkit, TOLERANCE
    )


def test_count_above_is_strictly_greater_not_greater_or_equal():
    hist = delta_histogram(bytearray([0, TOLERANCE, TOLERANCE, TOLERANCE + 1]))
    assert count_above(hist, TOLERANCE) == 1


# ---------------------------------------------------------------------------
# The sweep separates noise from structure
# ---------------------------------------------------------------------------


def test_subtolerance_noise_reads_as_aa_noise_not_structure():
    case_id, scope, chrome = load_small_baseline()
    noisy = perturbed(chrome, (0, 0, chrome.width, chrome.height), TOLERANCE - 1)
    record, _ = analyse_case(
        case_id, scope, chrome, noisy, [], TOLERANCE, DEFAULT_WORST_TILES
    )
    assert record["raw_diff_pct"] > 90.0, "the whole frame should differ rawly"
    assert sweep_shape(record) == "aa-noise"


def test_a_hard_colour_error_reads_as_structural_however_small():
    """The defect covers well under 1% of the frame — a percentage would
    forgive it, which is exactly why Gate B has a discrete half and why the
    board reports shape rather than size."""
    case_id, scope, chrome = load_small_baseline()
    box = (10, 10, 10 + TILE_PX, 10 + TILE_PX)
    broken = perturbed(chrome, box, 200)
    record, _ = analyse_case(
        case_id, scope, chrome, broken, [], TOLERANCE, DEFAULT_WORST_TILES
    )
    assert record["raw_diff_pct"] < 1.0
    assert sweep_shape(record) == "structural"


# ---------------------------------------------------------------------------
# Worst-N has to stay worth reading
# ---------------------------------------------------------------------------


def test_tiles_rank_on_above_tolerance_pixels_not_raw_ones():
    """A large field of sub-tolerance noise must not outrank a small hard
    error. Ranking raw puts the same antialiased text block at the top of
    every case on every run, and a board that says the same thing every night
    is one nobody opens."""
    width = height = TILE_PX * 3
    chrome = solid(width, height, (0, 0, 0))
    rgb = bytearray(chrome.rgb)
    # Tile (0,0): every pixel differs, all of it under tolerance.
    for y in range(TILE_PX):
        for x in range(TILE_PX):
            i = (y * width + x) * 3
            for c in range(3):
                rgb[i + c] = TOLERANCE - 1
    # Tile (1,1): a quarter of the pixels differ, hard.
    for y in range(TILE_PX, TILE_PX + TILE_PX // 2):
        for x in range(TILE_PX, TILE_PX + TILE_PX // 2):
            i = (y * width + x) * 3
            for c in range(3):
                rgb[i + c] = 255
    rustkit = Image(width, height, bytes(rgb))

    tiles = tile_stats(delta_map(chrome, rustkit), width, height, TOLERANCE)
    assert tiles, "expected at least one tile above tolerance"
    top = tiles[0]
    assert (top["x"], top["y"]) == (TILE_PX, TILE_PX)
    # The noisy tile has four times the RAW pixels and must still not be first.
    noisy = [t for t in tiles if (t["x"], t["y"]) == (0, 0)]
    assert not noisy, "a wholly sub-tolerance tile must not be listed at all"


def test_saturated_tiles_break_their_tie_on_severity_not_position():
    """Whole regions saturate at once — every tile over a mis-positioned block
    has all its pixels above tolerance — so without a severity tiebreak
    worst-N is just the top-left corner of the first defect, and a worse one
    further down the page never appears."""
    width = height = TILE_PX * 2
    chrome = solid(width, height, (0, 0, 0))
    rgb = bytearray(chrome.rgb)
    # Both tiles fully above tolerance; the LATER one (bottom-right) is worse.
    for ty, delta in ((0, TOLERANCE + 2), (TILE_PX, 250)):
        for y in range(ty, ty + TILE_PX):
            for x in range(ty, ty + TILE_PX):
                i = (y * width + x) * 3
                for c in range(3):
                    rgb[i + c] = delta
    tiles = tile_stats(delta_map(chrome, Image(width, height, bytes(rgb))), width, height, TOLERANCE)
    assert tiles[0]["above_tolerance_px"] == tiles[1]["above_tolerance_px"], (
        "both tiles should be saturated for this to test the tiebreak"
    )
    assert (tiles[0]["x"], tiles[0]["y"]) == (TILE_PX, TILE_PX)
    assert tiles[0]["max_delta"] > tiles[1]["max_delta"]


def test_tile_attribution_prefers_the_most_specific_element():
    """The page is tiled by body and its block descendants, so the largest
    overlapping element is always something true and useless."""
    elements = [
        {"selector": "html > body", "rect": {"x": 0, "y": 0, "width": 800, "height": 600}},
        {"selector": "body > div.panel", "rect": {"x": 0, "y": 0, "width": 400, "height": 300}},
        {"selector": "#closeBtn", "rect": {"x": 0, "y": 0, "width": 40, "height": 40}},
    ]
    tile = {"x": 0, "y": 0, "w": TILE_PX, "h": TILE_PX}
    assert attribute_tile(tile, elements, 800, 600)[0] == "#closeBtn"


def test_tile_attribution_ignores_elements_that_do_not_overlap():
    elements = [
        {"selector": "#far", "rect": {"x": 500, "y": 500, "width": 40, "height": 40}},
        {"selector": "#near", "rect": {"x": 0, "y": 0, "width": 40, "height": 40}},
    ]
    tile = {"x": 0, "y": 0, "w": TILE_PX, "h": TILE_PX}
    assert attribute_tile(tile, elements, 800, 600) == ["#near"]


def test_worst_tiles_carry_attribution_on_a_real_case():
    case_id, scope, chrome = load_small_baseline()
    box = (10, 10, 10 + TILE_PX, 10 + TILE_PX)
    broken = perturbed(chrome, box, 200)
    record, _ = analyse_case(
        case_id, scope, chrome, broken, small_case_elements(), TOLERANCE,
        DEFAULT_WORST_TILES,
    )
    assert record["worst_tiles"]
    assert record["worst_tiles"][0]["elements"], "worst tile named no element"


# ---------------------------------------------------------------------------
# Heatmap
# ---------------------------------------------------------------------------


def test_agreement_is_not_pure_black_so_an_empty_map_differs_from_a_failed_write():
    lut = build_heatmap_lut(TOLERANCE)
    assert lut[0] != (0, 0, 0)


def test_the_heatmap_breaks_visibly_at_the_pinned_tolerance():
    """A continuous ramp renders AA fringing and a wrong-coloured button as
    the same warm smear — the exact conflation Gate B's two halves exist to
    separate."""
    lut = build_heatmap_lut(TOLERANCE)
    below = lut[TOLERANCE]
    above = lut[TOLERANCE + 1]
    jump = sum(abs(a - b) for a, b in zip(below, above))
    assert jump > 120, f"tolerance boundary is not visible: {below} -> {above}"


def test_heatmap_is_written_and_reads_back_at_frame_size():
    case_id, scope, chrome = load_small_baseline()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        staged_run(case_id, scope, chrome, tmp / "run")
        report = build_board(tmp / "run", tmp / "out", case_ids=[case_id])
        assert report["summary"]["measured"] == 1
        heatmap = read_png(tmp / "out" / case_id / "heatmap.png")
    assert heatmap.size == chrome.size


# ---------------------------------------------------------------------------
# Refusals
# ---------------------------------------------------------------------------


def test_a_size_mismatch_is_unmeasured_and_never_compared():
    """Scaling one side to fit would make every number a comparison between
    two images that were never the same picture, and the heatmap a picture of
    the resize."""
    case_id, scope, chrome = load_small_baseline()
    wrong = solid(chrome.width // 2, chrome.height, (0, 0, 0))
    record, deltas = analyse_case(
        case_id, scope, chrome, wrong, [], TOLERANCE, DEFAULT_WORST_TILES
    )
    assert record["measured"] is False
    assert record["reason"].startswith("size_mismatch")
    assert deltas is None
    assert record["heatmap"] is None


def test_an_off_viewport_capture_is_refused_rather_than_scored():
    case_id, scope, chrome = load_small_baseline()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        path = tmp / "run" / case_id / "999x999" / "iter-1" / "capture" / "frame.ppm"
        write_ppm(path, chrome)
        report = build_board(tmp / "run", None, case_ids=[case_id])
    assert report["cases"][0]["measured"] is False
    assert report["cases"][0]["reason"] == "no_native_viewport_capture"


def test_an_unreadable_frame_is_reported_not_skipped():
    case_id, scope, _ = load_small_baseline()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        path = tmp / "run" / case_id / "frame.ppm"
        path.parent.mkdir(parents=True)
        path.write_bytes(b"P6\n800 600\n255\n" + bytes(12))
        report = build_board(tmp / "run", None, case_ids=[case_id])
    assert report["cases"][0]["measured"] is False
    assert report["cases"][0]["reason"].startswith("unreadable_capture")


def test_the_holdout_scope_is_not_charted_by_default():
    """Canary-only until the 26 are green (plan §3.6). Charting them by
    default would put six extra rows in the mean and quietly dilute it."""
    with tempfile.TemporaryDirectory() as tmp:
        gating = build_board(Path(tmp), None)
        with_holdout = build_board(Path(tmp), None, include_non_gating=True)
    assert gating["summary"]["total_cases"] == 26
    assert with_holdout["summary"]["total_cases"] == 32


# ---------------------------------------------------------------------------
# The published board says what it is
# ---------------------------------------------------------------------------


def test_markdown_states_that_these_numbers_cannot_fail_the_pr():
    case_id, scope, chrome = load_small_baseline()
    with tempfile.TemporaryDirectory() as tmp:
        tmp = Path(tmp)
        staged_run(case_id, scope, chrome, tmp / "run")
        report = build_board(tmp / "run", None, case_ids=[case_id], write_heatmaps=False)
    markdown = render_markdown(report)
    assert "cannot pass or fail" in markdown
    assert "non-gating" in markdown.lower()
    # The trap this whole campaign exists to close, said out loud on the board
    # a human actually reads.
    assert "Gate A" in markdown and "regresses" in markdown


if __name__ == "__main__":
    assert BASELINES.exists(), f"no baselines at {BASELINES}"
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: Gate C publishes, and refuses to report clean when it did not")
