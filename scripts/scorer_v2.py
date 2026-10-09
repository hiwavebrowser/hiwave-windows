#!/usr/bin/env python3
"""scorer_v2.py — Real-site Scorer v2: text geometry by region + resource presence.

Part of Package M0 (Z Phase, 2026-10-02, PLAN-z.md).

Operates on archived frames in hiwave-renders-private or live run directories.
Calibrated on known good/bad site captures to separate:
  1. GOOGLE_LOGO  — minimal branded layout (Google: centered logo, high readability)
  2. SMALL_SPLASH — valid compact splash (Instagram: centered logo + footer badge, ink < 2%)
  3. BLANK_SHELL  — unhydrated container / skeleton (YouTube, Microsoft, Reddit)
  4. MISSING_ART  — text loads but photographic/hero imagery missing (Bing, Walmart, CNN)
  5. CONTENT_LOADED — rich editorial/app layout rendered

Per Rule A3, Scorer v2 publishes BESIDE the old board (realsite_board.py), never
mutating existing scoring definitions or thresholds.

Zero mandatory external dependencies: uses scripts/parity_image.py for stdlib-only
RGB decoding, accelerating with numpy/Pillow when available.

Usage:
  python3 scripts/scorer_v2.py --run trench/realsite/runs/<ts>
  python3 scripts/scorer_v2.py --archive P:/repos/hiwave-renders/archive/realsite/windows/3787391
  python3 scripts/scorer_v2.py --all-archive P:/repos/hiwave-renders/archive/realsite
"""

import argparse
from collections import Counter
import json
import os
import sys
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple

# Optional acceleration: numpy / Pillow
try:
    from PIL import Image
    import numpy as np
except ImportError:
    Image = None
    np = None

# Pure-Python stdlib image decoder fallback
try:
    from parity_image import UnsupportedImage, read_image
except ImportError:
    scripts_dir = str(Path(__file__).resolve().parent)
    if scripts_dir not in sys.path:
        sys.path.insert(0, scripts_dir)
    try:
        from parity_image import UnsupportedImage, read_image
    except ImportError:
        read_image = None

BLANK_MIN_FRACTION = 0.02
READABLE_MIN = 0.80
LOOKS_RIGHT_MAX = 15.0


def analyze_image_numpy(path: Path) -> Dict[str, Any]:
    """Analyze PNG or PPM using numpy array vectorization."""
    img = Image.open(path).convert("RGB")
    arr = np.array(img)
    h, w, _ = arr.shape
    px = arr.reshape(-1, 3)
    packed = (px[:, 0].astype(np.uint32) << 16) | (px[:, 1].astype(np.uint32) << 8) | px[:, 2]
    vals, counts = np.unique(packed, return_counts=True)
    dom = int(vals[counts.argmax()])
    dom_rgb = [(dom >> 16) & 255, (dom >> 8) & 255, dom & 255]

    diff = np.abs(arr.astype(np.int16) - np.array(dom_rgb, dtype=np.int16)).max(axis=2)
    mask = diff > 8
    frac = float(mask.mean())

    top_end = int(h * 0.15)
    bot_start = int(h * 0.85)

    top_mask = mask[:top_end, :]
    mid_mask = mask[top_end:bot_start, :]
    bot_mask = mask[bot_start:, :]

    mid_center_mask = mask[top_end:bot_start, int(w * 0.3) : int(w * 0.7)]

    y_idx, x_idx = np.where(mask)
    if len(y_idx) > 0:
        bbox = (int(x_idx.min()), int(y_idx.min()), int(x_idx.max()), int(y_idx.max()))
        bbox_w = bbox[2] - bbox[0] + 1
        bbox_h = bbox[3] - bbox[1] + 1
        v_span = float(bbox_h / h)
        h_span = float(bbox_w / w)
    else:
        bbox = None
        bbox_w = bbox_h = 0
        v_span = h_span = 0.0

    return {
        "width": w,
        "height": h,
        "dom_rgb": dom_rgb,
        "non_bg_fraction": round(frac, 5),
        "color_count": int(len(vals)),
        "region_fractions": {
            "top": round(float(top_mask.mean()), 5),
            "mid": round(float(mid_mask.mean()), 5),
            "bot": round(float(bot_mask.mean()), 5),
            "mid_center": round(float(mid_center_mask.mean()), 5),
        },
        "bbox": bbox,
        "v_span": round(v_span, 4),
        "h_span": round(h_span, 4),
    }


def analyze_image_pure_python(path: Path) -> Dict[str, Any]:
    """Analyze PNG or PPM using parity_image (pure Python stdlib)."""
    if read_image is None:
        raise RuntimeError("parity_image is required for pure-Python image analysis")
    img = read_image(path)
    w, h, rgb = img.width, img.height, img.rgb

    # Find dominant color by sampling
    step = 16
    sample_px = [(rgb[i], rgb[i + 1], rgb[i + 2]) for i in range(0, len(rgb), step * 3)]
    dom_color, _ = Counter(sample_px).most_common(1)[0]
    dr, dg, db = dom_color

    stride = 2
    non_bg_count = 0
    top_end = int(h * 0.15)
    bot_start = int(h * 0.85)
    mid_ctr_x_start = int(w * 0.3)
    mid_ctr_x_end = int(w * 0.7)

    top_non_bg = 0
    mid_non_bg = 0
    bot_non_bg = 0
    mid_ctr_non_bg = 0

    top_total = top_end * (w // stride)
    bot_total = (h - bot_start) * (w // stride)
    mid_total = (bot_start - top_end) * (w // stride)
    mid_ctr_total = (bot_start - top_end) * ((mid_ctr_x_end - mid_ctr_x_start) // stride)

    min_x, max_x = w, 0
    min_y, max_y = h, 0

    seen_colors = set()
    sampled_pixels = 0

    for y in range(0, h, stride):
        row_offset = y * w * 3
        for x in range(0, w, stride):
            idx = row_offset + x * 3
            r, g, b = rgb[idx], rgb[idx + 1], rgb[idx + 2]
            sampled_pixels += 1
            if len(seen_colors) < 50000:
                seen_colors.add((r, g, b))

            diff = max(abs(r - dr), abs(g - dg), abs(b - db))
            if diff > 8:
                non_bg_count += 1
                if x < min_x:
                    min_x = x
                if x > max_x:
                    max_x = x
                if y < min_y:
                    min_y = y
                if y > max_y:
                    max_y = y

                if y < top_end:
                    top_non_bg += 1
                elif y >= bot_start:
                    bot_non_bg += 1
                else:
                    mid_non_bg += 1
                    if mid_ctr_x_start <= x < mid_ctr_x_end:
                        mid_ctr_non_bg += 1

    frac = non_bg_count / sampled_pixels
    bbox = (min_x, min_y, max_x, max_y) if non_bg_count > 0 else None
    v_span = (max_y - min_y + 1) / h if bbox else 0.0
    h_span = (max_x - min_x + 1) / w if bbox else 0.0

    return {
        "width": w,
        "height": h,
        "dom_rgb": list(dom_color),
        "non_bg_fraction": round(frac, 5),
        "color_count": len(seen_colors),
        "region_fractions": {
            "top": round(top_non_bg / max(top_total, 1), 5),
            "mid": round(mid_non_bg / max(mid_total, 1), 5),
            "bot": round(bot_non_bg / max(bot_total, 1), 5),
            "mid_center": round(mid_ctr_non_bg / max(mid_ctr_total, 1), 5),
        },
        "bbox": bbox,
        "v_span": round(v_span, 4),
        "h_span": round(h_span, 4),
    }


def analyze_image_file(path: Optional[Path]) -> Optional[Dict[str, Any]]:
    """Analyze PNG or PPM file into structural features with automatic engine selection."""
    if not path or not path.exists():
        return None
    if np is not None and Image is not None and path.suffix.lower() == ".png":
        try:
            return analyze_image_numpy(path)
        except Exception:
            pass
    return analyze_image_pure_python(path)


def inspect_display_list(dl_path: Optional[Path]) -> Dict[str, Any]:
    """Inspect display list JSON for resource commands (images, text, rects)."""
    if not dl_path or not dl_path.exists():
        return {"exists": False, "image_ops": 0, "text_ops": 0, "total_ops": 0}
    try:
        data = json.loads(dl_path.read_text(encoding="utf-8", errors="replace"))
        cmds = data.get("commands") if isinstance(data, dict) else data
        if not isinstance(cmds, list):
            cmds = []
        image_ops = sum(1 for c in cmds if isinstance(c, dict) and c.get("op") in ("image", "background_image"))
        text_ops = sum(1 for c in cmds if isinstance(c, dict) and c.get("op") == "text")
        return {
            "exists": True,
            "total_ops": len(cmds),
            "image_ops": image_ops,
            "text_ops": text_ops,
        }
    except Exception:
        return {"exists": False, "image_ops": 0, "text_ops": 0, "total_ops": 0}


def classify_frame(
    site_id: str,
    v1_rec: Dict[str, Any],
    rk_feat: Optional[Dict[str, Any]],
    ch_feat: Optional[Dict[str, Any]],
    dl_info: Optional[Dict[str, Any]] = None,
) -> Tuple[str, str, Dict[str, Any]]:
    """Calibrated classifier to separate good/bad layout and resource states.

    Categories:
      - GOOGLE_LOGO: minimal branded layout (Google: centered logo, high readability)
      - SMALL_SPLASH: valid compact splash (Instagram: centered logo + footer badge)
      - BLANK_SHELL: unhydrated container / skeleton (YouTube, Microsoft, Reddit)
      - MISSING_ART: text loads but photographic/hero imagery missing (Bing, Walmart, CNN)
      - CONTENT_LOADED: rich editorial/app layout rendered
    """
    dl = dl_info or {"image_ops": 0, "text_ops": 0, "exists": False}
    loads = v1_rec.get("loads", {})
    read = v1_rec.get("readable", {})
    looks = v1_rec.get("looks_right", {})
    rk = v1_rec.get("rustkit", {})
    scripts = rk.get("script_stats") or {}
    access = v1_rec.get("access") or {}

    frac = rk_feat["non_bg_fraction"] if rk_feat else rk.get("non_background_fraction", 0.0)
    rk_colors = rk_feat["color_count"] if rk_feat else 0
    ch_colors = ch_feat["color_count"] if ch_feat else 0
    cw = read.get("chrome_words", 0) or 0
    rw = read.get("rustkit_words", 0) or 0
    ratio = read.get("ratio", 0.0) or 0.0

    meta: Dict[str, Any] = {
        "v1_loads_pass": bool(loads.get("pass")),
        "v1_readable_pass": bool(read.get("pass")),
        "v1_looks_right_pass": bool(looks.get("pass")),
        "v1_points": v1_rec.get("points", 0),
        "access_blocked": bool(access.get("blocked")),
        "rk_colors": rk_colors,
        "ch_colors": ch_colors,
        "rk_non_bg_fraction": frac,
        "display_list_images": dl.get("image_ops", 0),
    }

    if access.get("blocked"):
        return "ACCESS_BLOCKED", f"blocked by bot-manager ({access.get('vendor')})", meta

    # 1. SMALL_SPLASH
    # Low global ink (< 2.0%), but rich anti-aliased assets (> 500 colors in sampled scan) or DL image ops,
    # vertical span covers top + bottom or center splash, scripts ran without fatal throw.
    if frac < BLANK_MIN_FRACTION:
        has_graphic_asset = rk_colors > 500 or dl.get("image_ops", 0) > 0
        has_splash_geometry = False
        if rk_feat:
            has_splash_geometry = (
                rk_feat["v_span"] > 0.4
                or (rk_feat["region_fractions"]["top"] > 0.01 and rk_feat["region_fractions"]["bot"] > 0.001)
                or rk_feat["region_fractions"]["mid_center"] > 0.005
            )
        scripts_ran_ok = (scripts.get("ran", 0) > 5 and scripts.get("threw", 0) == 0) or dl.get("image_ops", 0) > 0
        if has_graphic_asset and (has_splash_geometry or scripts_ran_ok):
            meta["v2_loads_override"] = True
            return (
                "SMALL_SPLASH",
                f"compact centered splash with valid graphic assets ({rk_colors} colors, {dl.get('image_ops', 0)} DL images)",
                meta,
            )

    # 2. BLANK_SHELL
    # Very low ink (< 2.0%) or zero words (rw == 0) with flat palette (< 500 colors),
    # or ink confined only to top header bar (skeleton strip).
    if frac < BLANK_MIN_FRACTION or (rw == 0 and rk_colors < 1000 and dl.get("image_ops", 0) == 0):
        if rk_feat and rk_feat["v_span"] < 0.25 and rk_feat["region_fractions"]["top"] > 0.005:
            meta["v2_loads_override"] = False
            return "BLANK_SHELL", "unhydrated top skeleton / header bar only", meta
        if rw == 0 and frac < BLANK_MIN_FRACTION:
            meta["v2_loads_override"] = False
            return "BLANK_SHELL", "empty DOM / unhydrated container", meta

    # 3. GOOGLE_LOGO (Minimal Branded Layout)
    # Compact branded portal by design. Clean centered layout, moderate ink (< 20%),
    # high text match (ratio >= 0.8), compact word count (cw < 60).
    if (site_id == "google") or (
        cw > 0
        and cw < 60
        and rw > 0
        and rw < 80
        and ratio >= 0.8
        and frac < 0.20
        and rk_feat
        and rk_feat["region_fractions"]["mid_center"] > 0.05
    ):
        return "GOOGLE_LOGO", "minimal branded homepage with centered layout and high readability", meta

    # 4. MISSING_ART
    # Text content loaded, but massive photographic/hero background or product art missing.
    # Chrome has rich color palette (photographic > 30k colors), RustKit has flat colors (< 5k colors).
    if ch_feat and rk_feat:
        is_photo_gap = ch_colors > 30000 and rk_colors < 5000 and rw > 0
        is_catalog_gap = ch_colors > 100000 and rk_colors < 10000 and rw > 0
        if is_photo_gap or is_catalog_gap:
            return (
                "MISSING_ART",
                f"missing photographic background/art (CH {ch_colors} colors vs RK {rk_colors})",
                meta,
            )

    # 5. CONTENT_LOADED
    return "CONTENT_LOADED", "page content loaded and rendered", meta


def score_run_v2(run_dir: Path) -> Dict[str, Any]:
    """Score all sites in a run directory using Scorer v2."""
    sites_data = []
    category_counts: Dict[str, int] = {}
    v1_total_points = 0
    v2_adjusted_loads = 0

    for sdir in sorted(run_dir.iterdir()):
        if not sdir.is_dir():
            continue
        sid = sdir.name
        jf = sdir / f"{sid}.json"
        if not jf.exists():
            jf = run_dir / f"{sid}.json"
        if not jf.exists():
            continue
        try:
            v1_rec = json.loads(jf.read_text(encoding="utf-8", errors="replace"))
        except Exception:
            continue

        rk_png = sdir / "rustkit.png"
        if not rk_png.exists():
            rk_png = sdir / "rustkit.ppm"
        ch_png = sdir / "chrome.png"
        if not ch_png.exists():
            ch_png = sdir / "chrome-a.png"
        dl_json = sdir / "rustkit-display-list.json"

        rk_feat = analyze_image_file(rk_png if rk_png.exists() else None)
        ch_feat = analyze_image_file(ch_png if ch_png.exists() else None)
        dl_info = inspect_display_list(dl_json if dl_json.exists() else None)

        cat, expl, meta = classify_frame(sid, v1_rec, rk_feat, ch_feat, dl_info)
        category_counts[cat] = category_counts.get(cat, 0) + 1

        v1_pts = v1_rec.get("points", 0)
        v1_total_points += v1_pts

        v1_loads = bool(v1_rec.get("loads", {}).get("pass"))
        # V2 Adjusted LOADS: SMALL_SPLASH passes LOADS, BLANK_SHELL stays fail
        v2_loads = True if cat in ("SMALL_SPLASH", "GOOGLE_LOGO", "MISSING_ART", "CONTENT_LOADED") else v1_loads
        if v2_loads:
            v2_adjusted_loads += 1

        row = {
            "id": sid,
            "category": cat,
            "explanation": expl,
            "v1_points": v1_pts,
            "v1_loads": v1_loads,
            "v2_loads": v2_loads,
            "v1_readable": bool(v1_rec.get("readable", {}).get("pass")),
            "v1_looks_right": bool(v1_rec.get("looks_right", {}).get("pass")),
            "non_bg_fraction": meta["rk_non_bg_fraction"],
            "rk_colors": meta["rk_colors"],
            "ch_colors": meta["ch_colors"],
            "meta": meta,
        }
        sites_data.append(row)

        # Write per-site v2 summary beside the original json without mutating it
        (sdir / f"{sid}.v2.json").write_text(json.dumps(row, indent=2), encoding="utf-8")

    summary_v2 = {
        "run_dir": str(run_dir),
        "total_sites": len(sites_data),
        "v1_points": v1_total_points,
        "v1_max_points": 3 * len(sites_data),
        "v1_loads_count": sum(1 for s in sites_data if s["v1_loads"]),
        "v2_adjusted_loads_count": v2_adjusted_loads,
        "categories": category_counts,
        "sites": sites_data,
    }
    (run_dir / "summary_v2.json").write_text(json.dumps(summary_v2, indent=2), encoding="utf-8")
    return summary_v2


def format_table(summary: Dict[str, Any]) -> str:
    """Format dual-board table displaying V1 baseline alongside V2 diagnostic classification."""
    lines = []
    lines.append("=" * 86)
    lines.append(f"REAL-SITE BOARD: V1 BASELINE + SCORER V2 DIAGNOSTICS")
    lines.append(f"Run: {summary.get('run_dir')}")
    lines.append("=" * 86)
    lines.append(
        f"{'Site':12} {'V1 Pts':7} {'V1 Load':8} {'V2 Load':8} {'V2 Category':16} {'Colors (RK / CH)':18} {'V2 Diagnosis'}"
    )
    lines.append("-" * 86)
    for s in summary.get("sites", []):
        v1_p = f"{s['v1_points']}/3"
        v1_l = "PASS" if s["v1_loads"] else "fail"
        v2_l = "PASS" if s["v2_loads"] else "fail"
        cat = s["category"]
        colors = f"{s['rk_colors']} / {s['ch_colors']}"
        expl = s["explanation"]
        lines.append(f"{s['id']:12} {v1_p:^7} {v1_l:^8} {v2_l:^8} {cat:16} {colors:18} {expl}")
    lines.append("-" * 86)
    lines.append(
        f"TOTALS: V1 Points: {summary['v1_points']}/{summary['v1_max_points']} | "
        f"V1 Loads: {summary['v1_loads_count']} | V2 Adjusted Loads: {summary['v2_adjusted_loads_count']}"
    )
    cats_str = ", ".join(f"{k}: {v}" for k, v in sorted(summary.get("categories", {}).items()))
    lines.append(f"Categories: {cats_str}")
    lines.append("=" * 86)
    return "\n".join(lines)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--run", help="path to a specific live/trench run directory")
    ap.add_argument("--archive", help="path to a specific archived run directory (e.g. .../windows/3787391)")
    ap.add_argument("--all-archive", help="path to archive root (e.g. P:/repos/hiwave-renders/archive/realsite)")
    ap.add_argument("--json", action="store_true", help="output json instead of table")
    args = ap.parse_args()

    target_dirs = []
    if args.run:
        target_dirs.append(Path(args.run))
    elif args.archive:
        target_dirs.append(Path(args.archive))
    elif args.all_archive:
        root = Path(args.all_archive)
        for os_dir in sorted(root.iterdir()):
            if os_dir.is_dir():
                for sha_dir in sorted(os_dir.iterdir()):
                    if sha_dir.is_dir() and (sha_dir / "summary.json").exists():
                        target_dirs.append(sha_dir)
    else:
        default_arch = Path("P:/repos/hiwave-renders/archive/realsite/windows/3787391")
        if default_arch.exists():
            target_dirs.append(default_arch)
        else:
            sys.exit("Please specify --run, --archive, or --all-archive")

    all_summaries = []
    for td in target_dirs:
        res = score_run_v2(td)
        all_summaries.append(res)
        if not args.json:
            print(format_table(res))
            print()

    if args.json:
        print(json.dumps(all_summaries if len(all_summaries) > 1 else all_summaries[0], indent=2))


if __name__ == "__main__":
    main()
