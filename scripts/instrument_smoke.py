#!/usr/bin/env python3
"""
instrument_smoke.py - Renderer instrument smokes (test-fidelity T5 / W3)

Constant-expectation probes that are hard to fake with page-specific CSS
and need NO Chrome baseline — they assert against known values:

  1. gamma: body{background:#1a1a2e} corner pixel == (26,26,46) EXACT.
     Fail signature ~(90,90,118) = CSS sRGB bytes double-encoded into an
     *UnormSrgb target (the Windows builtins-near-0% root cause,
     2026-07-10). macOS contract: LINEAR target + raw sRGB bytes.
  2. gradient-stops: a 2-stop horizontal linear gradient strip must hit
     the stop colors at the strip's ends and their gamma-space midpoint
     at the center (Chrome's default interpolation).

Runs parity-capture on generated fixtures; exit 1 on any mismatch.
Design invariant: css-color-upload-must-match-target-encoding
(Prometheus, 2026-07-11).
"""

import subprocess
import sys
import tempfile
from pathlib import Path
import os  # Windows sync: .exe suffix

REPO_ROOT = Path(__file__).parent.parent
CAPTURE = REPO_ROOT / "target" / "release" / ("parity-capture.exe" if os.name == "nt" else "parity-capture")

GAMMA_HTML = """<!DOCTYPE html><html><head><style>
body{background:#1a1a2e;margin:0}
</style></head><body></body></html>"""

# Port-back of Athena's gamma-MID fixture (Windows #17): midtones catch
# encode errors that the dark probe misses (the sRGB curve is shallow
# near black on the decode side — a partial double-encode can pass the
# dark probe and still wash mids).
GAMMA_MID_HTML = """<!DOCTYPE html><html><head><style>
body{background:#6a7a8a;margin:0}
</style></head><body></body></html>"""

GRADIENT_HTML = """<!DOCTYPE html><html><head><style>
*{margin:0;padding:0}
body{background:#ffffff}
.strip{width:400px;height:60px;background:linear-gradient(to right,#204080 0%,#c02040 100%)}
</style></head><body><div class="strip"></div></body></html>"""


def capture(html: str, width: int, height: int, out_ppm: Path) -> bool:
    with tempfile.NamedTemporaryFile("w", suffix=".html", delete=False) as f:
        f.write(html)
        path = f.name
    r = subprocess.run(
        [str(CAPTURE), "--html-file", path, "--width", str(width),
         "--height", str(height), "--dump-frame", str(out_ppm)],
        capture_output=True, text=True, timeout=120, encoding="utf-8"
    )
    return r.returncode == 0 and out_ppm.exists()


def read_ppm(path: Path):
    data = path.read_bytes()
    # P6\n<w> <h>\n255\n
    parts = data.split(b"\n", 3)
    w, h = map(int, parts[1].split())
    return w, h, parts[3]


def px(raw, w, x, y):
    i = (y * w + x) * 3
    return tuple(raw[i:i + 3])


def close(a, b, tol):
    return all(abs(x - y) <= tol for x, y in zip(a, b))


def main() -> int:
    failures = []
    tmp = Path(tempfile.mkdtemp(prefix="instrument-smoke-"))

    # Probe 1: gamma
    ppm = tmp / "gamma.ppm"
    if not capture(GAMMA_HTML, 200, 100, ppm):
        failures.append("gamma: capture failed")
    else:
        w, h, raw = read_ppm(ppm)
        got = px(raw, w, 100, 50)
        if got != (26, 26, 46):
            failures.append(
                f"gamma: #1a1a2e rendered as {got}, expected (26,26,46) exact "
                f"(~(90,90,118) = sRGB double-encode)"
            )

    # Probe 1b: gamma midtone (port-back of Athena's Windows fixture)
    ppm = tmp / "gamma-mid.ppm"
    if not capture(GAMMA_MID_HTML, 200, 100, ppm):
        failures.append("gamma-mid: capture failed")
    else:
        w, h, raw = read_ppm(ppm)
        got = px(raw, w, 100, 50)
        if got != (106, 122, 138):
            failures.append(
                f"gamma-mid: #6a7a8a rendered as {got}, expected (106,122,138) exact"
            )

    # Probe 2: gradient stop fidelity (gamma-space interp per Chrome default)
    ppm = tmp / "grad.ppm"
    if not capture(GRADIENT_HTML, 420, 100, ppm):
        failures.append("gradient: capture failed")
    else:
        w, h, raw = read_ppm(ppm)
        y = 30
        left = px(raw, w, 4, y)        # near 0%: #204080 = (32,64,128)
        right = px(raw, w, 395, y)     # near 100%: #c02040 = (192,32,64)
        mid = px(raw, w, 200, y)       # 50%: gamma-space midpoint = (112,48,96)
        if not close(left, (32, 64, 128), 6):
            failures.append(f"gradient: left stop {left}, expected ~(32,64,128)")
        if not close(right, (192, 32, 64), 6):
            failures.append(f"gradient: right stop {right}, expected ~(192,32,64)")
        if not close(mid, (112, 48, 96), 10):
            failures.append(
                f"gradient: midpoint {mid}, expected ~(112,48,96) "
                f"(gamma-space interp; a linear-light midpoint would read ~(143,50,99))"
            )

    if failures:
        print(f"INSTRUMENT SMOKE: {len(failures)} failure(s)")
        for f in failures:
            print(f"  ✗ {f}")
        return 1
    print("INSTRUMENT SMOKE: clean (gamma exact, gradient stops + gamma-space midpoint)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
