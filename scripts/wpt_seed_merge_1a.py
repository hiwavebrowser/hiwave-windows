#!/usr/bin/env python3
"""One-shot W0b seed merge (2026-08-05 session): fill MANIFEST to seed_cap from
the bucket-1A scout proposals, content-blind — round-robin across the three 1A
directories, alphabetical within each. The rule is deterministic and blind to
render outcomes so the selection cannot cherry-pick passes."""
import json
from itertools import zip_longest

mpath = 'trench/wpt/MANIFEST.json'
m = json.load(open(mpath, encoding="utf-8"))
d = json.load(open('trench/wpt/seed-scout-1A.json', encoding="utf-8"))
cands = [p for p in d['proposals'] if p['verdict'] == 'CANDIDATE']

by_dir = {}
for p in sorted(cands, key=lambda p: p['path']):
    by_dir.setdefault(p['dir'], []).append(p)

ordered = []
for row in zip_longest(*[by_dir[k] for k in sorted(by_dir)]):
    ordered.extend(p for p in row if p)

room = m['seed_cap'] - len(m['entries'])
picked = ordered[:room]
seeded_paths = {e['path'] for e in m['entries']}
added = []
for p in picked:
    assert p['path'] not in seeded_paths, p['path']
    m['entries'].append({
        'id': p['id'],
        'path': p['path'],
        'ref': p['ref'],
        'kind': 'reftest',
        'tier': '1A',
        'maps_to': 'slice-0',
    })
    added.append(p['id'])

m['seed_n'] = len(m['entries'])
with open(mpath, 'w', encoding="utf-8") as f:
    json.dump(m, f, indent=2)
    f.write('\n')
print(f"added {len(added)} -> seed_n {m['seed_n']} (cap {m['seed_cap']})")
for a in added:
    print(' ', a)
