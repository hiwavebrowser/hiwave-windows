#!/usr/bin/env python3
"""Generates tools/parity_oracle/eval_order_cases.json: small programs full of evaluation-order
hazards (a local read before or after an assignment to it inside the same expression, member
access with a side-effecting key, compound assignment whose right side assigns the left, `i++`
on vars, lets and parameters, closures capturing a variable). The expected value of each comes
from Chrome (`js_cases_log.mjs eval_order_cases.json --write`); the engine test compares.
Seeded: the same file every run."""
import json, random, sys

R = random.Random(20261113)
NUMS = ["a", "b", "c"]
OBJS = ["o", "p"]
ARR = "r"

def num_atom(depth):
    k = R.random()
    if k < 0.35: return R.choice(NUMS)
    if k < 0.5: return str(R.randint(-3, 9))
    if k < 0.7: return f"{R.choice(OBJS)}.{R.choice('xyn')}"
    if k < 0.85: return f"{ARR}[{idx(depth)}]"
    return f"{R.choice(OBJS)}[{key(depth)}]"

def idx(depth):
    k = R.random()
    if k < 0.5: return R.choice(NUMS) + " & 3"
    if k < 0.75: return str(R.randint(0, 3))
    if depth > 0: return f"({R.choice(NUMS)} = ({num(depth-1)}) & 3)"
    return "1"

def key(depth):
    k = R.random()
    if k < 0.4: return "'x'"
    if k < 0.6: return "k"
    if k < 0.8 and depth > 0: return f"(k = {R.choice(['\"x\"', '\"y\"', '\"n\"'])})"
    return "'y'"

def num(depth=2):
    if depth <= 0 or R.random() < 0.25: return num_atom(depth)
    k = R.random()
    if k < 0.45:
        return f"({num(depth-1)} {R.choice(['+', '-', '*'])} {num(depth-1)})"
    if k < 0.55:
        v = R.choice(NUMS)
        return f"({v} = {num(depth-1)})"
    if k < 0.65:
        v = R.choice(NUMS)
        return f"({v} {R.choice(['+=', '-=', '*='])} {num(depth-1)})"
    if k < 0.73:
        v = R.choice(NUMS)
        return f"{R.choice([v + '++', '++' + v, v + '--', '--' + v])}"
    if k < 0.8:
        return f"({R.choice(OBJS)}.{R.choice('xyn')} = {num(depth-1)})"
    if k < 0.86:
        return f"({R.choice(OBJS)}.{R.choice('xyn')} {R.choice(['+=', '-='])} {num(depth-1)})"
    if k < 0.9:
        return f"({ARR}[{idx(depth-1)}] += {num(depth-1)})"
    if k < 0.94:
        return f"({num(depth-1)} < {num(depth-1)} ? {num(depth-1)} : {num(depth-1)})"
    if k < 0.955:
        return f"({num(depth-1)}, {num(depth-1)})"
    if k < 0.965:
        return f"w3({num(depth-1)}, {num(depth-1)}, {num(depth-1)})"
    if k < 0.975:
        return f"[{num(depth-1)}, {num(depth-1)}, {num(depth-1)}][{R.randint(0,2)}]"
    if k < 0.983:
        return f"({{ u: {num(depth-1)}, v: {num(depth-1)} }}).{R.choice('uv')}"
    if k < 0.99:
        v1, v2 = R.choice(NUMS), R.choice(NUMS)
        return f"({v1} = {v2} = {num(depth-1)})"
    if k < 0.995:
        return f"(typeof {num(depth-1)} === 'number' ? {num(depth-1)} : 0)"
    return f"bump({num(depth-1)})"

def stmt(depth=2):
    k = R.random()
    if R.random() < 0.3:
        k = 0.96  # the declaration / local-store kinds below
    if k < 0.3: return f"t += {num(depth)};"
    if k < 0.45: return f"{R.choice(NUMS)} = {num(depth)};"
    if k < 0.6: return f"{R.choice(NUMS)} {R.choice(['+=', '-=', '*='])} {num(depth)};"
    if k < 0.7: return f"{R.choice(NUMS)}{R.choice(['++', '--'])};"
    if k < 0.8: return f"{R.choice(OBJS)}.{R.choice('xyn')} = {num(depth)};"
    if k < 0.88: return f"{ARR}[{idx(depth)}] = {num(depth)};"
    if k < 0.93: return f"if ({num(depth)} > {num(depth)}) {{ t += {num(depth)}; }}"
    if k < 0.955: return f"for (var i = 0; i < 3; i++) {{ t += {num(depth)}; {R.choice(NUMS)} += i; }}"
    # Declarations and stores whose value or target is a local: `var` with an expression (hoisted,
    # so a later one reads the earlier value), a local stored into a member, a call or a member
    # assigned to a local, and a right side that reads the variable it assigns.
    k2 = R.random()
    v = R.choice(NUMS + ["d", "e"])
    if k2 < 0.2: return f"var {R.choice(['d', 'e'])} = {num(depth)};"
    if k2 < 0.35: return f"var d = d + {num(depth - 1)};"
    if k2 < 0.5: return f"{R.choice(OBJS)}.{R.choice('xyn')} = {R.choice(NUMS + ['d', 'e'])};"
    if k2 < 0.6: return f"{ARR}[{idx(depth)}] = {R.choice(NUMS)};"
    if k2 < 0.7: return f"{R.choice(OBJS)}[k] = {num(depth)};"
    if k2 < 0.8: return f"{R.choice(NUMS)} = bump({R.choice(NUMS)});"
    if k2 < 0.9: return f"{R.choice(NUMS)} = {R.choice(OBJS)}.{R.choice('xyn')};"
    return f"{R.choice(NUMS)} = {num(depth)} * {R.choice(NUMS)};"

def start():
    # Non-number starting values too, so `x++` used as a value and every operator convert
    # (numeric strings, undefined, null, booleans, an object with valueOf).
    k = R.random()
    if k < 0.55: return str(R.randint(0, 5))
    return R.choice(["'3'", "'7'", "undefined", "null", "true", "false", "({ valueOf() { return 4 } })", "'x'"])

def program(n):
    kinds = R.choice(["var", "let"])
    body = "\n  ".join(stmt() for _ in range(R.randint(2, 7)))
    # a, b are locals of the chosen kind, c is a parameter; closure `bump` captures `cap`.
    return (
        "(function () { try { var cap = 0; function bump(x) { cap += 1; return x + cap; } function w3(x, y, z) { return x * 100 + y * 10 + z; } "
        f"function f(c) {{ {kinds} a = {start()}, b = {start()}; var t = 0, k = 'x'; "
        "var o = { x: 1, y: 2, n: 3 }, p = { x: 4, y: 5, n: 6 }, r = [7, 8, 9, 10];\n  "
        + body +
        "\n  return JSON.stringify([a, b, c, t, k, o, p, r, cap, typeof d === 'undefined' ? null : d, typeof e === 'undefined' ? null : e]); } "
        f"return f({R.randint(0,5)}); }} catch (e) {{ return 'ERR ' + e.name; }} }})()"
    )

cases = [{"name": f"fuzz-{i:03d}", "js": program(i)} for i in range(int(sys.argv[1]) if len(sys.argv) > 1 else 400)]
json.dump({"cases": cases}, open("tools/parity_oracle/eval_order_cases.json", "w"), indent=1)
print(len(cases))
