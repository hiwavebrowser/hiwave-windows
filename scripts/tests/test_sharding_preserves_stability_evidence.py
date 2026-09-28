"""Sharding must not scatter a cell's iterations across shards.

Run: python3 scripts/tests/test_sharding_preserves_stability_evidence.py

The pr_merge gate requires STABILITY_MIN_RUNS *measured* iterations per row and
fails anything it cannot judge (P0a, night 4). The evidence for that judgement
comes from the scout running `--iterations 3`. Between those two facts sits
`shard_work_units`, and until 2026-08-08 it sharded by raw unit index — which
scattered a cell's three iterations across three shards, because units are
generated as consecutive iterations of the same cell.

The result was not a weaker check, it was a permanent red lock: no cell in any
sharded run could ever show three measured iterations, so the gate failed 22 of
26 cases as `stability_unmeasured` on the first PR after the bar tightened. The
four survivors were exploit-phase cases that reached three runs by a different
route.

This is the shape of instrument failure the campaign exists to stop: two changes
that are each correct, and a third component between them that quietly makes
their combination meaningless. So the tests here assert the CONNECTION, not
either end of it:

  * every iteration of a cell lands in exactly one shard
  * the union of shards is the whole work list, with nothing dropped or doubled
  * a sharded run still yields STABILITY_MIN_RUNS runs per cell, checked
    against the real constant rather than a literal 3
  * shards stay balanced, so fixing correctness did not wreck wall clock
"""
import sys
from collections import Counter, defaultdict
from pathlib import Path

# Resolved, not `dirname(__file__) + ".."` — parity_lib derives REPO_ROOT from
# this sys.path entry and an unresolved one sends it looking for
# scripts/tests/cases/registry.json. Same note as test_stability_actually_gates.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from parity_lib import STABILITY_MIN_RUNS  # noqa: E402
from parity_swarm import WorkUnit, shard_work_units  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parent.parent.parent

# The PR lane's real geometry: 4 shards, scout at the stability minimum.
SHARD_COUNT = 4


def build_units(case_count, iterations, viewports=("800x600",)):
    """Mirror generate_work_units' ordering: iterations innermost."""
    units = []
    for c in range(case_count):
        for viewport in viewports:
            width, height = (int(v) for v in viewport.split("x"))
            for i in range(iterations):
                units.append(
                    WorkUnit(
                        case_id=f"case-{c:02d}",
                        html_path=f"fixtures/case-{c:02d}.html",
                        width=width,
                        height=height,
                        case_type="micro",
                        viewport_name=viewport,
                        iteration=i + 1,
                    )
                )
    return units


def shard_all(units, shard_count=SHARD_COUNT):
    return [shard_work_units(units, i, shard_count) for i in range(shard_count)]


def cell(unit):
    return (unit.case_id, unit.viewport_name)


# ---------------------------------------------------------------------------


def test_every_iteration_of_a_cell_lands_in_one_shard():
    """The defect, stated directly. With iterations=3 and 4 shards the old
    modulo scattered each cell across three shards, because 3 and 4 are
    coprime — the two numbers the PR lane actually uses."""
    units = build_units(case_count=26, iterations=STABILITY_MIN_RUNS)
    shards = shard_all(units)

    homes = defaultdict(set)
    for index, shard in enumerate(shards):
        for unit in shard:
            homes[cell(unit)].add(index)

    scattered = {c: sorted(s) for c, s in homes.items() if len(s) > 1}
    assert not scattered, f"cells split across shards: {list(scattered.items())[:3]}"


def test_a_sharded_run_still_yields_the_stability_minimum_per_cell():
    """What the gate actually consumes. Asserted against the pinned constant,
    not a literal — a suite that hardcodes 3 goes green the day someone raises
    the bar and the evidence stops being produced."""
    units = build_units(case_count=26, iterations=STABILITY_MIN_RUNS)
    for shard in shard_all(units):
        counts = Counter(cell(u) for u in shard)
        for c, n in counts.items():
            assert n == STABILITY_MIN_RUNS, f"{c} has {n} runs, need {STABILITY_MIN_RUNS}"


def test_sharding_loses_nothing_and_duplicates_nothing():
    units = build_units(case_count=26, iterations=STABILITY_MIN_RUNS)
    seen = [u for shard in shard_all(units) for u in shard]
    assert len(seen) == len(units)
    key = lambda u: (u.case_id, u.viewport_name, u.iteration)  # noqa: E731
    assert sorted(map(key, seen)) == sorted(map(key, units))


def test_cells_stay_balanced_across_shards():
    """Correctness must not have been bought with wall clock. Every cell
    carries the same iteration count, so shards may differ by at most one
    cell."""
    units = build_units(case_count=26, iterations=STABILITY_MIN_RUNS)
    sizes = [len({cell(u) for u in shard}) for shard in shard_all(units)]
    assert max(sizes) - min(sizes) <= 1, sizes


def test_multiple_viewports_shard_as_separate_cells():
    """The exploit phase runs the same case at several viewports, and the gate
    groups by (case, viewport). A cell is that pair, not the case alone.

    Keying on case alone would still keep every cell intact — a coarser
    grouping keeps subsets together — so the invariant above cannot catch it.
    What it costs is scheduling granularity: a case's viewports become
    indivisible, and with few cases and many shards that idles shards. So the
    property asserted here is the one that actually differs: two viewports of
    the same case CAN be scheduled apart.
    """
    units = build_units(
        case_count=8, iterations=STABILITY_MIN_RUNS, viewports=("800x600", "1280x800")
    )
    shards = shard_all(units)
    homes = defaultdict(set)
    for index, shard in enumerate(shards):
        for unit in shard:
            homes[cell(unit)].add(index)
    assert len(homes) == 16, f"expected 16 cells, got {len(homes)}"
    assert all(len(s) == 1 for s in homes.values()), "a cell must not be split"

    by_case = defaultdict(set)
    for (case_id, _viewport), where in homes.items():
        by_case[case_id] |= where
    assert any(len(where) > 1 for where in by_case.values()), (
        "no case had its viewports scheduled on different shards — the cell key "
        "has collapsed to the case, costing scheduling granularity"
    )


def test_single_iteration_runs_are_unaffected():
    """commit-gate still scouts once; that lane must shard exactly as before."""
    units = build_units(case_count=12, iterations=1)
    shards = shard_all(units)
    assert [len(s) for s in shards] == [3, 3, 3, 3]
    assert sum(len(s) for s in shards) == 12


# The other half of the connection — that both lanes actually REQUEST
# STABILITY_MIN_RUNS scout iterations, so sharding has evidence to preserve —
# is already guarded by test_stability_actually_gates's
# `test_every_lane_that_gates_on_stability_runs_the_iterations`. That one
# parses the workflow with a stdlib regex and predates this file.
#
# A duplicate of it lived here and has been removed. It asserted the same
# property with PyYAML, which is what made this file the one that took
# `script-guards` red on its first run. Verified before deleting: dropping
# --iterations from either lane, or lowering it below the bar, turns night 4's
# test RED — so the property is covered, by the copy that needs no parser.


if __name__ == "__main__":
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: sharding keeps a cell's iterations together, so stability stays measurable")
