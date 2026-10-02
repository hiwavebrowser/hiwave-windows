/**
 * attribute_diff.mjs
 *
 * Attribute pixel diffs to DOM elements using Chrome layout rects.
 *
 * Input diff buffer is the pixelmatch output RGBA buffer.
 * We treat any non-zero intensity pixel as a diff pixel.
 */

const DEFAULT_TILE_SIZE = 64;

function clampInt(v, lo, hi) {
  return Math.max(lo, Math.min(hi, v | 0));
}

function rectArea(r) {
  return Math.max(0, (r.width || 0)) * Math.max(0, (r.height || 0));
}

function pointInRect(px, py, r) {
  const x0 = r.x || 0;
  const y0 = r.y || 0;
  const x1 = x0 + (r.width || 0);
  const y1 = y0 + (r.height || 0);
  return px >= x0 && px < x1 && py >= y0 && py < y1;
}

function buildTileIndex(rects, width, height, tileSize) {
  const tilesX = Math.ceil(width / tileSize);
  const tilesY = Math.ceil(height / tileSize);
  const bins = Array.from({ length: tilesX * tilesY }, () => []);

  rects.forEach((el, idx) => {
    const r = el.rect;
    if (!r) return;
    if ((r.width || 0) <= 0 || (r.height || 0) <= 0) return;

    const x0 = clampInt(Math.floor(r.x || 0), 0, width - 1);
    const y0 = clampInt(Math.floor(r.y || 0), 0, height - 1);
    const x1 = clampInt(Math.ceil((r.x || 0) + (r.width || 0)), 0, width);
    const y1 = clampInt(Math.ceil((r.y || 0) + (r.height || 0)), 0, height);

    const tx0 = clampInt(Math.floor(x0 / tileSize), 0, tilesX - 1);
    const ty0 = clampInt(Math.floor(y0 / tileSize), 0, tilesY - 1);
    const tx1 = clampInt(Math.floor((x1 - 1) / tileSize), 0, tilesX - 1);
    const ty1 = clampInt(Math.floor((y1 - 1) / tileSize), 0, tilesY - 1);

    for (let ty = ty0; ty <= ty1; ty++) {
      for (let tx = tx0; tx <= tx1; tx++) {
        bins[ty * tilesX + tx].push(idx);
      }
    }
  });

  return { tilesX, tilesY, bins, tileSize };
}

function getBinCandidates(tileIndex, x, y) {
  const { tilesX, tileSize, bins } = tileIndex;
  const tx = Math.floor(x / tileSize);
  const ty = Math.floor(y / tileSize);
  const idx = ty * tilesX + tx;
  return bins[idx] || [];
}

export function attributeDiff(diffRgba, width, height, chromeRectsJson, options = {}) {
  const tileSize = options.tileSize || DEFAULT_TILE_SIZE;
  const cornerSize = options.cornerSize || 12;

  const elements = chromeRectsJson?.elements || [];
  const rects = elements
    .map((e) => ({ selector: e.selector, tag: e.tag, rect: e.rect }))
    .filter((e) => e.rect && (e.rect.width || 0) > 0 && (e.rect.height || 0) > 0);

  const tileIndex = buildTileIndex(rects, width, height, tileSize);

  const totalPixels = width * height;
  let totalDiffPixels = 0;
  let unattributed = 0;

  // selector -> { diffPixels, cornerPixels }
  const stats = new Map();

  for (let i = 0; i < totalPixels; i++) {
    const idx = i * 4;
    const r = diffRgba[idx];
    const g = diffRgba[idx + 1];
    const b = diffRgba[idx + 2];
    const a = diffRgba[idx + 3];

    const intensity = Math.max(r, g, b);
    if (a === 0 || intensity === 0) continue;

    totalDiffPixels++;

    const x = i % width;
    const y = (i / width) | 0;
    const px = x + 0.5;
    const py = y + 0.5;

    const candidates = getBinCandidates(tileIndex, x, y);
    if (!candidates.length) {
      unattributed++;
      continue;
    }

    let bestIdx = -1;
    let bestArea = Number.POSITIVE_INFINITY;

    for (const candIdx of candidates) {
      const el = rects[candIdx];
      if (!el) continue;
      if (!pointInRect(px, py, el.rect)) continue;
      const area = rectArea(el.rect);
      if (area > 0 && area < bestArea) {
        bestArea = area;
        bestIdx = candIdx;
      }
    }

    if (bestIdx === -1) {
      unattributed++;
      continue;
    }

    const el = rects[bestIdx];
    const key = el.selector;
    const prev = stats.get(key) || { diffPixels: 0, cornerPixels: 0 };
    prev.diffPixels++;

    const rx = el.rect.x || 0;
    const ry = el.rect.y || 0;
    const rw = el.rect.width || 0;
    const rh = el.rect.height || 0;
    const localX = px - rx;
    const localY = py - ry;
    const inCorner =
      (localX <= cornerSize && localY <= cornerSize) ||
      (localX >= rw - cornerSize && localY <= cornerSize) ||
      (localX <= cornerSize && localY >= rh - cornerSize) ||
      (localX >= rw - cornerSize && localY >= rh - cornerSize);
    if (inCorner) prev.cornerPixels++;

    stats.set(key, prev);
  }

  const contributors = [];
  for (const [selector, s] of stats.entries()) {
    const el = rects.find((r) => r.selector === selector);
    if (!el) continue;
    const area = rectArea(el.rect) || 1;
    contributors.push({
      selector,
      tag: el.tag || null,
      rect: {
        x: el.rect.x,
        y: el.rect.y,
        width: el.rect.width,
        height: el.rect.height,
      },
      diff_pixels: s.diffPixels,
      contribution_percent: totalDiffPixels > 0 ? (s.diffPixels / totalDiffPixels) * 100 : 0,
      element_diff_percent: (s.diffPixels / area) * 100,
      corner_ratio: s.diffPixels > 0 ? s.cornerPixels / s.diffPixels : 0,
    });
  }

  contributors.sort((a, b) => b.diff_pixels - a.diff_pixels);

  return {
    total_diff_pixels: totalDiffPixels,
    unattributed_diff_pixels: unattributed,
    top_contributors: contributors.slice(0, options.topN || 10),
    all_elements: contributors,
  };
}


