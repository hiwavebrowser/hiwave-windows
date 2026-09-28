"""Static guard: no unguarded use of a nullable diff field.

Why this file exists, plainly: the NOT-MEASURED state was introduced in one
change set and then broken FOUR times in that same change set — a display
format string, two comparison tools, `compare_reports` arithmetic, and finally
the scout print loops. Three of those were caught by a reviewer, one by CI,
none by the author. Each round was fixed by hand and each round missed more.

Trying harder was not working, so this replaces effort with enumeration. It
walks the AST of every parity script and fails on any format, arithmetic, or
ordering comparison applied to a field that can be None, unless the site is
listed below as reviewed-and-guarded.

If you add a new consumer and this fails: do not add it to the allowlist to
make the test pass. Guard the site, then add it with a reason.

SCOPE: measured instrument fields only — the ones that carry None when the
instrument refused. `estimated_diff_pct` is a DIFFERENT producer and a
separate lie; it is deliberately not in NULLABLE, and adding it here without
first making that producer three-state would only move the confusion.

Run: python3 scripts/tests/test_no_unguarded_nullable_diff.py
"""
import ast
import os
import pathlib
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
SCRIPTS = REPO / "scripts"

# Fields that carry None when the instrument refused to measure.
NULLABLE = {
    "diff_pct", "diff_pct_median", "diff_pct_min", "diff_pct_max",
    "diff_pct_variance",  # 65-I.5: assigned None in the all-errored branch
    "avg_diff", "avg_diff_pct", "min_diff_pct", "max_diff_pct",
}

# Calls that consume a value numerically. `sorted` is here for its key= lambda,
# which is where the sorted-key class hid.
NUMERIC_CALLS = {"sum", "min", "max", "abs", "round", "sorted", "statistics.median"}

# Sites reviewed and confirmed guarded. `file: {line-independent reason}` —
# keyed by (file, enclosing function) rather than line number so ordinary
# edits do not silently invalidate the review.
ALLOWED = {
    ("parity_swarm.py", "fmt_diff"): "the None check IS this function",
    ("parity_test.py", "_fmt"): "the None check IS this function",
    ("parity_aggregate.py", "compare_reports"): "early `continue` on either side None",
    ("parity_lib.py", "aggregate_iterations"): "all uses sit inside `if diffs:`",
    ("parity_swarm.py", "run_exploit_phase"): "guarded by `is not None` in the filter",
    ("parity_test.py", "run_test"): "guarded by `is not None` before compare",
    ("wpt_tier1.py", "run_pair"): "explicit `is None` -> ERROR row before the comparison",
}


class Scan(ast.NodeVisitor):
    def __init__(self):
        self.hits = []
        self._fn = []
        self._suppressed = frozenset()

    def visit_FunctionDef(self, node):
        self._fn.append(node.name)
        self.generic_visit(node)
        self._fn.pop()

    visit_AsyncFunctionDef = visit_FunctionDef

    def _field(self, node):
        """The nullable field this expression yields, if any.

        Covers the four surface forms a nullable value arrives in:
        attribute access, subscript by constant, bare name, and `.get("name")`
        — the last because most JSON consumers use it and the first version of
        this guard missed every one of them.
        """
        if isinstance(node, ast.Attribute) and node.attr in NULLABLE:
            return node.attr
        if (isinstance(node, ast.Subscript) and isinstance(node.slice, ast.Constant)
                and node.slice.value in NULLABLE):
            return node.slice.value
        if isinstance(node, ast.Name) and node.id in NULLABLE:
            return node.id
        if (isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute)
                and node.func.attr == "get" and node.args
                and isinstance(node.args[0], ast.Constant)
                and node.args[0].value in NULLABLE):
            # `.get(name)` with a default is NOT safe: a default fires only on
            # a MISSING key, never on a key present holding null.
            return node.args[0].value
        return None

    @staticmethod
    def _is_guarded(node):
        """True if this expression defends itself against None.

        Two idioms count, and only these two, because both are checkable:
        an `is None` / `is not None` test, or an `or <default>` fallback. A
        detector that cries wolf gets allowlisted into uselessness, so it has
        to recognise the guards people actually write.
        """
        for sub in ast.walk(node):
            if isinstance(sub, ast.Compare) and any(
                isinstance(o, (ast.Is, ast.IsNot)) for o in sub.ops
            ):
                return True
            if isinstance(sub, ast.BoolOp) and isinstance(sub.op, ast.Or):
                return True
        return False

    def _contains_field(self, node):
        """The nullable field an expression CONSUMES, if unguarded.

        For a comprehension or generator, only the element expression counts —
        `sum(1 for c in xs if c.diff_pct is not None)` sums the literal 1 and
        merely FILTERS on the field, which is exactly the guard we want people
        to write.
        """
        if isinstance(node, (ast.GeneratorExp, ast.ListComp, ast.SetComp)):
            return self._contains_field(node.elt)
        if isinstance(node, ast.Lambda):
            return self._contains_field(node.body)
        if self._is_guarded(node):
            return None
        for sub in ast.walk(node):
            field = self._field(sub)
            if field:
                return field
        return None

    def _record(self, node, what):
        # `what` embeds the field name in backticks; skip if it is suppressed
        # by an enclosing short-circuit guard.
        if any(f"`{f}`" in what for f in self._suppressed):
            return
        self.hits.append((node.lineno, self._fn[-1] if self._fn else "<module>", what))

    def visit_FormattedValue(self, node):
        field = self._field(node.value)
        if field and node.format_spec is not None:
            self._record(node, f"format spec applied to `{field}`")
        self.generic_visit(node)

    def visit_BoolOp(self, node):
        """`x is not None and <uses x>` short-circuits, so the later operands
        are guarded. Without this the scanner flags the single most common
        correct way to write the check, and a scanner that punishes the right
        answer gets switched off."""
        if isinstance(node.op, ast.And):
            guarded = set()
            for value in node.values:
                if isinstance(value, ast.Compare) and any(
                    isinstance(o, ast.IsNot) for o in value.ops
                ):
                    field = self._field(value.left)
                    if field:
                        guarded.add(field)
                        continue
                # Operands after the guard are protected for those fields.
                if guarded:
                    saved = self._suppressed
                    self._suppressed = saved | guarded
                    self.visit(value)
                    self._suppressed = saved
                    continue
                self.visit(value)
            return
        self.generic_visit(node)

    def visit_UnaryOp(self, node):
        # `-x["diff_pct_median"]` — the sorted-key class. Raises on None.
        if isinstance(node.op, (ast.USub, ast.UAdd)):
            field = self._field(node.operand)
            if field:
                self._record(node, f"unary {'-' if isinstance(node.op, ast.USub) else '+'} on `{field}`")
        self.generic_visit(node)

    def visit_AugAssign(self, node):
        field = self._field(node.value) or self._field(node.target)
        if field:
            self._record(node, f"augmented assignment involving `{field}`")
        self.generic_visit(node)

    def visit_Call(self, node):
        name = None
        if isinstance(node.func, ast.Name):
            name = node.func.id
        elif isinstance(node.func, ast.Attribute):
            name = f"{getattr(node.func.value, 'id', '')}.{node.func.attr}".lstrip(".")

        if name in NUMERIC_CALLS:
            # Arguments, generator/comprehension elements, and key= lambda
            # bodies all consume the value numerically.
            for arg in list(node.args) + [kw.value for kw in node.keywords]:
                field = self._contains_field(arg)
                if field:
                    self._record(node, f"`{name}()` consumes `{field}`")
                    break

        # "{:.2f}".format(x) and its %-operator cousin are handled here and in
        # visit_BinOp respectively.
        if (isinstance(node.func, ast.Attribute) and node.func.attr == "format"
                and isinstance(node.func.value, ast.Constant)):
            for arg in node.args:
                field = self._field(arg)
                if field:
                    self._record(node, f"str.format() on `{field}`")
                    break
        self.generic_visit(node)

    def visit_BinOp(self, node):
        # `"%.2f" % value` — printf-style formatting raises on None too.
        if isinstance(node.op, ast.Mod) and isinstance(node.left, ast.Constant) \
                and isinstance(node.left.value, str):
            field = self._contains_field(node.right)
            if field:
                self._record(node, f"printf-style format of `{field}`")
        if isinstance(node.op, (ast.Sub, ast.Add, ast.Mult, ast.Div,
                                ast.Mod, ast.Pow, ast.FloorDiv)):
            for side in (node.left, node.right):
                field = self._field(side)
                if field:
                    self._record(node, f"arithmetic on `{field}`")
        self.generic_visit(node)

    def visit_Compare(self, node):
        field = self._field(node.left)
        if field and any(isinstance(o, (ast.Lt, ast.Gt, ast.LtE, ast.GtE)) for o in node.ops):
            self._record(node, f"ordering comparison on `{field}`")
        self.generic_visit(node)


def test_no_unguarded_nullable_diff_use():
    offenders = []
    for path in sorted(SCRIPTS.rglob("*.py")):
        if "tests" in path.parts or "__pycache__" in path.parts:
            continue
        scan = Scan()
        scan.visit(ast.parse(path.read_text(errors="replace", encoding="utf-8")))
        for lineno, fn, what in scan.hits:
            if (path.name, fn) in ALLOWED:
                continue
            offenders.append(f"{path.name}:{lineno} in {fn}(): {what}")

    assert not offenders, (
        "unguarded use of a field that can be None:\n  "
        + "\n  ".join(offenders)
        + "\n\nGuard the site, then add (file, function) to ALLOWED with a "
          "reason. Do NOT allowlist it to make this pass."
    )


def _detect(source: str):
    """Run the scanner over a snippet and return its findings."""
    scan = Scan()
    scan.visit(ast.parse(source))
    return [what for _lineno, _fn, what in scan.hits]


def test_guard_catches_every_shape_prometheus_found_it_missing():
    """T-RED for the GUARD itself.

    The first version of this scanner passed green while missing UnaryOp,
    sum-over-comprehension, sorted(key=), .get() and printf formatting — so it
    proved the absence of three surface syntaxes, not the absence of unguarded
    use. A detector that is trusted and incomplete is worse than none, because
    its green is read as evidence.

    Each snippet below was a documented MISS. All must now be HIT.
    """
    shapes = {
        "unary minus (the sorted-key class)":
            'xs.sort(key=lambda x: -x["diff_pct_median"])',
        "sum over a list comprehension":
            'total = sum([a["diff_pct_median"] for a in xs])',
        "sum over a generator":
            'total = sum(a.diff_pct for a in xs)',
        "min()/max() over collected values":
            'lo = min([a["diff_pct_median"] for a in xs])',
        "sorted() consuming the field":
            'ordered = sorted(xs, key=lambda a: a["diff_pct"])',
        ".get() form used in arithmetic":
            'delta = cur.get("diff_pct") - base.get("diff_pct")',
        ".get() form used in a format spec":
            'msg = f"{r.get(\'diff_pct\'):.2f}%"',
        "str.format()":
            'msg = "{:.2f}%".format(r.diff_pct)',
        "printf-style %":
            'msg = "%.2f%%" % r.diff_pct',
        "augmented assignment":
            'total += r.diff_pct',
        "floor division":
            'half = r.diff_pct // 2',
        "abs()":
            'a = abs(r.diff_pct)',
    }
    missed = [name for name, src in shapes.items() if not _detect(src)]
    assert not missed, (
        "the guard no longer catches: " + ", ".join(missed)
        + " — a scanner that misses a shape makes its own green meaningless"
    )


def test_guard_does_not_cry_wolf_on_written_guards():
    """False positives get allowlisted into uselessness, so the two guard
    idioms people actually write must read as safe."""
    safe = {
        "filtering on the field while summing a literal":
            'n = sum(1 for c in xs if c.diff_pct is not None)',
        "or-fallback inside a sort key":
            'ordered = sorted(xs, key=lambda r: (r.get("diff_pct") is not None, r.get("diff_pct") or 0.0))',
        "explicit None check before comparing":
            'ok = r.diff_pct is not None and r.diff_pct <= t',
    }
    noisy = [name for name, src in safe.items() if _detect(src)]
    assert not noisy, "guard flagged already-guarded code: " + ", ".join(noisy)


def test_aggregate_swarm_results_survives_an_unmeasured_cell():
    """65-I (Prometheus): sort raised on unary-minus of None, sum raised, and
    an empty measured set published 100 — the decorative lie one function
    past where it was fixed."""
    source = (SCRIPTS / "parity_swarm.py").read_text(encoding="utf-8")
    assert 'aggregated.sort(key=lambda x: -x["diff_pct_median"])' not in source, (
        "unary-minus sort is back"
    )
    assert 'if all_diffs else 100,' not in source, (
        "empty measured set publishes 100 again"
    )
    assert 'if a["diff_pct_median"] is not None' in source, (
        "all_diffs no longer filters to measured cells"
    )


def test_allowlist_has_no_redundant_entries():
    """Every exemption must still be doing work.

    Once the scanner learned to recognise short-circuit guards, one allowlist
    entry stopped being needed — and an exemption granted for nothing is an
    exemption that will silently cover a real defect the day the code under it
    changes. Removing each entry must produce at least one offender.
    """
    redundant = []
    for entry in ALLOWED:
        reduced = {e: r for e, r in ALLOWED.items() if e != entry}
        found = False
        for path in sorted(SCRIPTS.rglob("*.py")):
            if "tests" in path.parts or "__pycache__" in path.parts:
                continue
            scan = Scan()
            scan.visit(ast.parse(path.read_text(errors="replace", encoding="utf-8")))
            for _lineno, fn, _what in scan.hits:
                if (path.name, fn) == entry and (path.name, fn) not in reduced:
                    found = True
        if not found:
            redundant.append(f"{entry[0]}:{entry[1]}()")
    assert not redundant, (
        "allowlist entries that exempt nothing: " + ", ".join(redundant)
        + " — remove them; a dead exemption covers the next real defect"
    )


def test_allowlist_entries_still_exist():
    """A stale allowlist is worse than none — it grants an exemption to a
    function that no longer exists while reading as reviewed."""
    missing = []
    for (filename, fn) in ALLOWED:
        path = SCRIPTS / filename
        if not path.exists():
            missing.append(f"{filename} (file gone)")
            continue
        names = {
            n.name for n in ast.walk(ast.parse(path.read_text(errors="replace", encoding="utf-8")))
            if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))
        }
        if fn not in names:
            missing.append(f"{filename}:{fn}() (function gone)")
    assert not missing, "stale ALLOWED entries: " + ", ".join(missing)


if __name__ == "__main__":
    os.chdir(REPO)
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: every nullable-diff consumer is guarded or reviewed")
