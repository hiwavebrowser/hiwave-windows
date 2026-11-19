#!/usr/bin/env python3
"""Three-way comparison metrics for one set of RustKit vs Chrome image pairs (Linux column).

For each pair: the share of pixels where ANY RGB channel differs by more than 8
(provisional definition, agreed on the exchange 2026-11-19), the mean absolute RGB channel difference (0-255, over
all pixels), and the two sizes. A size mismatch is reported, never silently cropped:
the metrics then cover the common top-left area and the row says so.

usage: threeway_metrics.py --set NAME --pairs pairs.json --out result.json
pairs.json: [{"case": str, "rustkit": path, "chrome": path}, ...]
"""
import argparse
import json

from PIL import Image, ImageChops


def metrics(rk_path, ch_path):
    rk = Image.open(rk_path).convert("RGB")
    ch = Image.open(ch_path).convert("RGB")
    row = {"rustkit_size": list(rk.size), "chrome_size": list(ch.size)}
    w, h = min(rk.width, ch.width), min(rk.height, ch.height)
    if rk.size != ch.size:
        row["size_mismatch"] = True
        rk, ch = rk.crop((0, 0, w, h)), ch.crop((0, 0, w, h))
    diff = ImageChops.difference(rk, ch)
    hist_r, hist_g, hist_b = diff.split()
    # Any channel > 8: the per-pixel max of the three channel differences.
    worst = ImageChops.lighter(ImageChops.lighter(hist_r, hist_g), hist_b)
    over = sum(worst.histogram()[9:])
    total = w * h
    channel_sum = sum(i * n for band in (hist_r, hist_g, hist_b) for i, n in enumerate(band.histogram()))
    row["pixels_over_8_pct"] = round(100.0 * over / total, 4) if total else None
    row["mean_channel_diff"] = round(channel_sum / (3 * total), 4) if total else None
    return row


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--set", required=True)
    ap.add_argument("--pairs", required=True)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    pairs = json.load(open(args.pairs))
    rows = []
    for p in pairs:
        r = {"case": p["case"]}
        try:
            r.update(metrics(p["rustkit"], p["chrome"]))
        except Exception as e:  # a missing or unreadable image is a row, not a crash
            r["error"] = f"{type(e).__name__}: {e}"
        rows.append(r)
    scored = [r for r in rows if "pixels_over_8_pct" in r]
    summary = {
        "set": args.set,
        "cases": len(rows),
        "scored": len(scored),
        "errors": [r["case"] for r in rows if "error" in r],
        "size_mismatches": [r["case"] for r in rows if r.get("size_mismatch")],
        "mean_pixels_over_8_pct": round(sum(r["pixels_over_8_pct"] for r in scored) / len(scored), 4) if scored else None,
        "mean_channel_diff": round(sum(r["mean_channel_diff"] for r in scored) / len(scored), 4) if scored else None,
        "worst10_by_pixels_over_8": [
            {"case": r["case"], "pixels_over_8_pct": r["pixels_over_8_pct"], "mean_channel_diff": r["mean_channel_diff"]}
            for r in sorted(scored, key=lambda r: r["pixels_over_8_pct"], reverse=True)[:10]
        ],
    }
    json.dump({"summary": summary, "rows": rows}, open(args.out, "w"), indent=2)
    print(json.dumps(summary)[:600])


if __name__ == "__main__":
    main()
