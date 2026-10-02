#!/usr/bin/env python3
"""Three-way merge a Rust source file whose sides both APPENDED test functions.

Written for the trench's branch stack (2026-09-22), where six nights had each
added guards at the same point in `flex.rs`'s `mod tests` and every pairwise
merge conflicted there.

A union resolution — keep both sides of each hunk — is wrong twice over, and
both ways fail quietly:

  * git leaves the lines that CLOSE the last function in the shared context
    AFTER the hunk, so `ours + theirs` drops `ours`' closing braces;
  * where the two sides' tests share body lines (`let mut item_style = ...`),
    git interleaves them into several small hunks, and concatenating the sides
    splices one test's assertions into another test's body. That can still
    compile, and the result is a guard asserting something nobody wrote.

Items are the granularity these merges actually work at: a test function is
added whole, not edited. So keep ours, append the items theirs added that ours
does not have, and REPORT anything edited on both sides instead of guessing.

The engine half (everything before `#[cfg(test)]`) is a real three-way merge
via `git merge-file`; conflicts there are left marked, for hand resolution,
because a union of engine code is not a resolution.

Limitation, stated because it bit once: this handles ONE `#[cfg(test)]` module
per file. `lib.rs` has three, and must be resolved another way.

Usage (from inside a conflicted merge):  scripts/merge_test_module.py <path>...
Exits 1 if any item was edited on both sides.
"""
import re, subprocess, sys

ITEM = re.compile(r'^    (?:pub(?:\(crate\))? )?(?:async )?fn ([A-Za-z0-9_]+)')

def stage(path, n):
    return subprocess.run(['git', 'show', f':{n}:{path}'], capture_output=True,
                          text=True, check=True, encoding="utf-8").stdout.split('\n')

def split(lines):
    tm = next((i for i, l in enumerate(lines) if l.startswith('#[cfg(test)]')), None)
    if tm is None:
        return lines, {}, []
    prelude = lines[:tm]
    body = lines[tm:]
    items, cur, name, depth, head = {}, [], None, 0, []
    # The module's own closing brace sits at column 0; it must not travel
    # inside whichever side's last item happens to precede it, or a merge
    # that appends items from both sides closes the module twice.
    body = [l for l in body if l != '}']
    for l in body:
        m = ITEM.match(l)
        if m and depth <= 1:
            if name:
                items[name] = cur
            name, cur = m.group(1), head + [l]
            head = []
        elif name is None:
            if l.strip().startswith(('///', '#[', '//')) and depth <= 1:
                head.append(l)
            else:
                prelude.append(l) if not head else head.append(l)
        else:
            if depth <= 1 and l.strip().startswith(('///', '#[')) and cur and cur[-1].strip() in ('}', ''):
                items[name] = cur; name, cur, head = None, [], [l]
            else:
                cur.append(l)
        s = l.split('//')[0]
        depth += s.count('{') - s.count('}')
    if name:
        items[name] = cur
    return prelude, items, head

def merge(path):
    base_p, base_i, _ = split(stage(path, 1))
    ours_p, ours_i, ours_tail = split(stage(path, 2))
    _, theirs_i, _ = split(stage(path, 3))

    added = [n for n in theirs_i if n not in ours_i]
    both = [n for n in theirs_i if n in ours_i and n in base_i
            and theirs_i[n] != base_i[n] and ours_i[n] != base_i[n]
            and theirs_i[n] != ours_i[n]]
    # The engine half is a real three-way merge; only the test module is
    # resolved by item identity. Conflicts here are left marked, for hand
    # resolution, because a union of engine code is not a resolution.
    import tempfile, os
    tmp = []
    for txt in (ours_p, base_p, [l for l in split(stage(path, 3))[0]]):
        f = tempfile.NamedTemporaryFile('w', suffix='.rs', delete=False)
        f.write('\n'.join(txt) + '\n'); f.close(); tmp.append(f.name)
    r = subprocess.run(['git', 'merge-file', '-p', '--diff3'] + tmp,
                       capture_output=True, text=True, encoding="utf-8")
    prelude_merged = r.stdout.split('\n')
    if prelude_merged and prelude_merged[-1] == '':
        prelude_merged.pop()
    for f in tmp:
        os.unlink(f)
    if r.returncode != 0:
        print(f"  {path}: engine half has {r.returncode} conflict(s) — left marked")

    out = list(prelude_merged)
    for n, txt in ours_i.items():
        out += txt
    for n in added:
        out += theirs_i[n]
    out += ours_tail
    out.append('}')
    # the module's closing brace travelled with the last item of whichever
    # side wrote it; make sure exactly one is present at the end
    while out and out[-1].strip() == '':
        out.pop()
    open(path, 'w', encoding="utf-8").write('\n'.join(out) + '\n')
    print(f"  {path}: kept {len(ours_i)} ours, appended {len(added)} from theirs"
          + (f", BOTH-EDITED: {both}" if both else ""))
    return both

if __name__ == '__main__':
    bad = []
    for p in sys.argv[1:]:
        bad += merge(p)
    sys.exit(1 if bad else 0)
