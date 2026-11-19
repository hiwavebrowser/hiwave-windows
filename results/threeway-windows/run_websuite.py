#!/usr/bin/env python3
"""Set (b) of the three-way comparison, Windows column: websuite/cases (8) + websuite/micro (21).

Each page: RustKit (parity-capture --html-file) and Chrome for Testing 148 win64 (capture_chrome_websuite.mjs:
the oracle's deterministic context, with the parity reset for micro pages injected once <html> exists), at
the campaign registry's size or 1280x800. Both use the system fonts (Windows has no fontconfig file).
Adapted from the Linux column's run_websuite.py (hiwave-linux talos/threeway-linux-results).
"""
import glob, json, os, subprocess
REPO = r"P:\repos\hiwave-wt\win-3way"
CAP = os.path.join(REPO, "target", "release", "parity-capture.exe")
CFT = r"P:\repos\hiwave-wt\_cft148_win64\chrome-win64\chrome.exe"
OUT = os.path.join(REPO, "results", "threeway-windows")
W = os.path.join(OUT, "websuite_work")
os.chdir(REPO)
env = dict(os.environ, PARITY_CHROME_PATH=CFT, ORACLE_DIR=os.path.join(REPO, "tools", "parity_oracle").replace("\\", "/"),
           RESET_CSS_PATH=os.path.join(REPO, "baselines", "common", "parity-reset.css"),
           NODE_PATH=os.path.join(REPO, "tools", "parity_oracle", "node_modules"))
reg = {c["html"]: (n, c["width"], c["height"], c.get("capture_args", [])) for n, c in json.load(open("cases/registry.json", encoding="utf-8"))["cases"].items()}
pages = sorted(glob.glob("websuite/cases/*/index.html")) + sorted(glob.glob("websuite/micro/*/*.html"))
pages = [p.replace("\\", "/") for p in pages]
pairs, meta, jobs = [], [], []
for p in pages:
    name = p.replace("websuite/", "").replace("/index.html", "").replace(".html", "").replace("/", "__")
    _, w, h, extra = reg.get(p, (name, 1280, 800, []))
    d = os.path.join(W, name); os.makedirs(d, exist_ok=True)
    rk = subprocess.run([CAP, "--html-file", p, "--width", str(w), "--height", str(h), "--dump-frame", os.path.join(d, "rustkit.png"), *extra],
                        capture_output=True, text=True, timeout=180, env=env)
    jobs.append({"html": os.path.join(REPO, p).replace("\\", "/"), "out": os.path.join(d, "chrome").replace("\\", "/"), "width": w, "height": h})
    meta.append({"case": name, "html": p, "width": w, "height": h, "size_from": "registry" if p in reg else "default 1280x800",
                 "rustkit_exit": rk.returncode})
    pairs.append({"case": name, "rustkit": os.path.join(d, "rustkit.png"), "chrome": os.path.join(d, "chrome", "baseline.png")})
    print(name, w, h, rk.returncode, flush=True)
json.dump(jobs, open(os.path.join(W, "chrome_jobs.json"), "w"), indent=1)
ch = subprocess.run(["node", os.path.join(OUT, "capture_chrome_websuite.mjs"), os.path.join(W, "chrome_jobs.json")],
                    capture_output=True, text=True, timeout=1800, env=env, cwd=os.path.join(REPO, "tools", "parity_oracle"))
print(ch.stdout[-3000:], ch.stderr[-2000:], flush=True)
json.dump(pairs, open(os.path.join(OUT, "websuite_pairs.json"), "w"), indent=1)
json.dump(meta, open(os.path.join(OUT, "websuite_meta.json"), "w"), indent=2)
