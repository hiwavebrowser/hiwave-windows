/**
 * compare_baseline.mjs - Compare RustKit capture against Chrome baseline
 *
 * Triple verification:
 * 1. Pixel diff (primary)
 * 2. Computed-style comparison
 * 3. Layout rect comparison
 */

import pixelmatch from 'pixelmatch';
import { PNG } from 'pngjs';
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'fs';
import { join, dirname } from 'path';
import { attributeDiff } from './attribute_diff.mjs';
import { buildChromeStyleIndex, classifyContributor } from './classify_diff.mjs';

/**
 * Load a PNG file as RGBA buffer
 */
async function loadPng(pngPath) {
  return new Promise((resolve, reject) => {
    const buffer = readFileSync(pngPath);
    const png = new PNG();

    png.parse(buffer, (err, data) => {
      if (err) reject(err);
      else resolve({ width: data.width, height: data.height, data: data.data });
    });
  });
}

/**
 * Convert PPM (P6 binary) to RGBA buffer
 */
function ppmToRgba(ppmPath) {
  const buffer = readFileSync(ppmPath);
  let idx = 0;

  // Skip magic number "P6"
  while (buffer[idx] !== 0x0A) idx++;
  idx++;

  // Skip comments
  while (buffer[idx] === 0x23) {
    while (buffer[idx] !== 0x0A) idx++;
    idx++;
  }

  // Read width
  let widthStr = '';
  while (buffer[idx] !== 0x20 && buffer[idx] !== 0x0A) {
    widthStr += String.fromCharCode(buffer[idx]);
    idx++;
  }
  idx++;

  // Read height
  let heightStr = '';
  while (buffer[idx] !== 0x20 && buffer[idx] !== 0x0A) {
    heightStr += String.fromCharCode(buffer[idx]);
    idx++;
  }
  idx++;

  // Read max value
  while (buffer[idx] !== 0x0A) idx++;
  idx++;

  const width = parseInt(widthStr, 10);
  const height = parseInt(heightStr, 10);

  // Convert RGB to RGBA
  const rgbaData = Buffer.alloc(width * height * 4);
  const pixelCount = width * height;

  for (let i = 0; i < pixelCount; i++) {
    const srcIdx = idx + i * 3;
    const dstIdx = i * 4;
    rgbaData[dstIdx] = buffer[srcIdx];
    rgbaData[dstIdx + 1] = buffer[srcIdx + 1];
    rgbaData[dstIdx + 2] = buffer[srcIdx + 2];
    rgbaData[dstIdx + 3] = 255;
  }

  return { width, height, data: rgbaData };
}

/**
 * Save RGBA buffer as PNG
 */
function savePng(data, width, height, outputPath) {
  const png = new PNG({ width, height });
  png.data = data;
  const buffer = PNG.sync.write(png);
  writeFileSync(outputPath, buffer);
}

function drawRectOutline(rgba, width, height, rect, color) {
  const x0 = Math.max(0, Math.floor(rect.x || 0));
  const y0 = Math.max(0, Math.floor(rect.y || 0));
  const x1 = Math.min(width - 1, Math.floor((rect.x || 0) + (rect.width || 0)));
  const y1 = Math.min(height - 1, Math.floor((rect.y || 0) + (rect.height || 0)));

  const [cr, cg, cb, ca] = color;

  const setPx = (x, y) => {
    if (x < 0 || y < 0 || x >= width || y >= height) return;
    const idx = (y * width + x) * 4;
    rgba[idx] = cr;
    rgba[idx + 1] = cg;
    rgba[idx + 2] = cb;
    rgba[idx + 3] = ca;
  };

  for (let x = x0; x <= x1; x++) {
    setPx(x, y0);
    setPx(x, y1);
  }
  for (let y = y0; y <= y1; y++) {
    setPx(x0, y);
    setPx(x1, y);
  }
}

/**
 * Compare pixel data and generate diff image + heatmap
 */
export async function comparePixels(chromePath, rustkitPath, outputDir, options = {}) {
  mkdirSync(outputDir, { recursive: true });

  // Load images
  const chrome = await loadPng(chromePath);
  let rustkit;
  if (rustkitPath.endsWith('.ppm')) {
    rustkit = ppmToRgba(rustkitPath);
  } else {
    rustkit = await loadPng(rustkitPath);
  }

  // Dimension mismatch is a hard failure, not a silent crop (lie #8). Comparing
  // a rustkit capture against a wrong-sized baseline cropped to the overlap
  // produces a meaningless diff number that reads as partial success. Return the
  // worst score with a dimension_mismatch taxonomy so it can never masquerade as
  // parity. RK_ALLOW_CROP=1 restores the old crop path for local debugging only.
  if (
    (chrome.width !== rustkit.width || chrome.height !== rustkit.height) &&
    process.env.RK_ALLOW_CROP !== '1'
  ) {
    const totalPixels = chrome.width * chrome.height;
    return {
      diffPixels: totalPixels,
      totalPixels,
      diffPercent: 100,
      width: chrome.width,
      height: chrome.height,
      diffPath: null,
      heatmapPath: null,
      overlayPath: null,
      attribution: null,
      taxonomy: { instrument: 'dimension_mismatch' },
      dimensionMismatch: {
        chrome: [chrome.width, chrome.height],
        rustkit: [rustkit.width, rustkit.height],
      },
      error:
        `dimension_mismatch: chrome ${chrome.width}x${chrome.height} vs ` +
        `rustkit ${rustkit.width}x${rustkit.height} ` +
        `(regenerate the baseline; RK_ALLOW_CROP=1 to force-crop for debugging)`,
    };
  }

  // Same size (or RK_ALLOW_CROP debug path): crop to the overlap.
  const width = Math.min(chrome.width, rustkit.width);
  const height = Math.min(chrome.height, rustkit.height);

  if (chrome.width !== rustkit.width || chrome.height !== rustkit.height) {
    console.warn(`  RK_ALLOW_CROP: cropping Chrome ${chrome.width}x${chrome.height} / RustKit ${rustkit.width}x${rustkit.height}`);
  }

  // Crop to same size
  const cropImage = (data, srcWidth, srcHeight, dstWidth, dstHeight) => {
    const cropped = Buffer.alloc(dstWidth * dstHeight * 4);
    for (let y = 0; y < dstHeight; y++) {
      for (let x = 0; x < dstWidth; x++) {
        const srcIdx = (y * srcWidth + x) * 4;
        const dstIdx = (y * dstWidth + x) * 4;
        cropped[dstIdx] = data[srcIdx];
        cropped[dstIdx + 1] = data[srcIdx + 1];
        cropped[dstIdx + 2] = data[srcIdx + 2];
        cropped[dstIdx + 3] = data[srcIdx + 3];
      }
    }
    return cropped;
  };

  const chromeData = cropImage(chrome.data, chrome.width, chrome.height, width, height);
  const rustkitData = cropImage(rustkit.data, rustkit.width, rustkit.height, width, height);

  const totalPixels = width * height;
  const diffData = Buffer.alloc(width * height * 4);

  // Run pixelmatch
  const diffPixels = pixelmatch(
    chromeData,
    rustkitData,
    diffData,
    width,
    height,
    {
      threshold: options.threshold || 0.1,
      includeAA: true,
      alpha: 0.1,
    }
  );

  const diffPercent = (diffPixels / totalPixels) * 100;

  // Save diff image
  const diffPath = join(outputDir, 'diff.png');
  savePng(diffData, width, height, diffPath);

  // Generate heatmap
  const heatmapData = Buffer.alloc(width * height * 4);
  for (let i = 0; i < width * height; i++) {
    const srcIdx = i * 4;
    const r = diffData[srcIdx];
    const g = diffData[srcIdx + 1];
    const b = diffData[srcIdx + 2];
    const intensity = Math.max(r, g, b);

    // Map to heatmap colors
    let hr, hg, hb;
    if (intensity < 64) {
      hr = 0; hg = intensity * 4; hb = 255 - intensity * 4;
    } else if (intensity < 128) {
      hr = (intensity - 64) * 4; hg = 255; hb = 0;
    } else if (intensity < 192) {
      hr = 255; hg = 255 - (intensity - 128) * 4; hb = 0;
    } else {
      hr = 255; hg = 0; hb = (intensity - 192) * 4;
    }

    heatmapData[srcIdx] = hr;
    heatmapData[srcIdx + 1] = hg;
    heatmapData[srcIdx + 2] = hb;
    heatmapData[srcIdx + 3] = intensity > 0 ? 255 : 0;
  }

  const heatmapPath = join(outputDir, 'heatmap.png');
  savePng(heatmapData, width, height, heatmapPath);

  // Optional: element attribution (Chrome rects + styles)
  let attribution = null;
  let overlayPath = null;
  let taxonomy = null;

  if (options.chromeRectsPath && existsSync(options.chromeRectsPath)) {
    try {
      const chromeRectsJson = JSON.parse(readFileSync(options.chromeRectsPath, 'utf-8'));
      attribution = attributeDiff(diffData, width, height, chromeRectsJson, {
        topN: options.attributionTopN || 10,
      });

      // Taxonomy classification needs styles (optional)
      taxonomy = {};
      let styleIndex = null;
      if (options.chromeStylesPath && existsSync(options.chromeStylesPath)) {
        const chromeStylesJson = JSON.parse(readFileSync(options.chromeStylesPath, 'utf-8'));
        styleIndex = buildChromeStyleIndex(chromeStylesJson);
      }

      if (attribution?.top_contributors?.length) {
        for (const c of attribution.top_contributors) {
          const label = classifyContributor(c, styleIndex);
          taxonomy[label] = (taxonomy[label] || 0) + (c.contribution_percent || 0);
          c.likely_cause = label;
        }
      }

      // Overlay top contributor rects on heatmap for fast debugging.
      const overlay = Buffer.from(heatmapData);
      const colors = [
        [255, 255, 255, 255],
        [0, 255, 0, 255],
        [255, 255, 0, 255],
        [0, 255, 255, 255],
        [255, 0, 255, 255],
      ];
      (attribution.top_contributors || []).slice(0, 10).forEach((c, i) => {
        drawRectOutline(overlay, width, height, c.rect, colors[i % colors.length]);
      });

      overlayPath = join(outputDir, 'overlay.png');
      savePng(overlay, width, height, overlayPath);

      const attributionPath = join(outputDir, 'attribution.json');
      writeFileSync(
        attributionPath,
        JSON.stringify(
          {
            width,
            height,
            diffPercent,
            totalDiffPixels: attribution.total_diff_pixels,
            unattributedDiffPixels: attribution.unattributed_diff_pixels,
            taxonomy,
            topContributors: attribution.top_contributors,
          },
          null,
          2
        )
      );
    } catch (e) {
      // Attribution is best-effort; never break the pipeline.
      attribution = { error: String(e?.message || e) };
    }
  }

  return {
    diffPixels,
    totalPixels,
    diffPercent,
    width,
    height,
    diffPath,
    heatmapPath,
    overlayPath,
    attribution,
    taxonomy,
  };
}

/**
 * Compare computed styles between Chrome and RustKit
 */
export function compareStyles(chromeStylesPath, rustkitStylesPath) {
  if (!existsSync(chromeStylesPath)) {
    return { error: 'Chrome styles not found' };
  }
  if (!existsSync(rustkitStylesPath)) {
    return { error: 'RustKit styles not found' };
  }

  const chromeStyles = JSON.parse(readFileSync(chromeStylesPath, 'utf-8'));
  const rustkitStyles = JSON.parse(readFileSync(rustkitStylesPath, 'utf-8'));

  const chromeMap = new Map(chromeStyles.elements.map(e => [e.selector, e]));
  const rustkitMap = new Map(rustkitStyles.elements?.map(e => [e.selector, e]) || []);

  const results = {
    matched: 0,
    mismatched: 0,
    chromeOnly: 0,
    rustkitOnly: 0,
    differences: [],
  };

  // Key properties to compare
  const keyProps = [
    'display', 'width', 'height', 'margin-top', 'margin-left',
    'padding-top', 'padding-left', 'position', 'color', 'background-color',
  ];

  for (const [selector, chrome] of chromeMap) {
    const rustkit = rustkitMap.get(selector);

    if (!rustkit) {
      results.chromeOnly++;
      continue;
    }

    const diffs = [];
    for (const prop of keyProps) {
      const chromeVal = chrome.styles?.[prop];
      const rustkitVal = rustkit.styles?.[prop];

      if (chromeVal !== rustkitVal) {
        diffs.push({ property: prop, chrome: chromeVal, rustkit: rustkitVal });
      }
    }

    if (diffs.length > 0) {
      results.mismatched++;
      results.differences.push({ selector, diffs });
    } else {
      results.matched++;
    }
  }

  for (const selector of rustkitMap.keys()) {
    if (!chromeMap.has(selector)) {
      results.rustkitOnly++;
    }
  }

  return results;
}

/**
 * Compare layout rects between Chrome and RustKit
 */
export function compareRects(chromeRectsPath, rustkitRectsPath, tolerance = 5) {
  if (!existsSync(chromeRectsPath)) {
    return { error: 'Chrome rects not found' };
  }
  if (!existsSync(rustkitRectsPath)) {
    return { error: 'RustKit rects not found' };
  }

  const chromeRects = JSON.parse(readFileSync(chromeRectsPath, 'utf-8'));
  const rustkitRects = JSON.parse(readFileSync(rustkitRectsPath, 'utf-8'));

  const chromeMap = new Map(chromeRects.elements.map(e => [e.selector, e]));
  const rustkitMap = new Map(rustkitRects.elements?.map(e => [e.selector, e]) || []);

  const results = {
    matched: 0,
    mismatched: 0,
    chromeOnly: 0,
    rustkitOnly: 0,
    differences: [],
  };

  for (const [selector, chrome] of chromeMap) {
    const rustkit = rustkitMap.get(selector);

    if (!rustkit) {
      results.chromeOnly++;
      continue;
    }

    const cr = chrome.rect;
    const rr = rustkit.rect || rustkit.content_rect || {};

    const diffs = [];
    if (Math.abs((cr.width || 0) - (rr.width || 0)) > tolerance) {
      diffs.push({ prop: 'width', chrome: cr.width, rustkit: rr.width });
    }
    if (Math.abs((cr.height || 0) - (rr.height || 0)) > tolerance) {
      diffs.push({ prop: 'height', chrome: cr.height, rustkit: rr.height });
    }
    if (Math.abs((cr.x || 0) - (rr.x || 0)) > tolerance) {
      diffs.push({ prop: 'x', chrome: cr.x, rustkit: rr.x });
    }
    if (Math.abs((cr.y || 0) - (rr.y || 0)) > tolerance) {
      diffs.push({ prop: 'y', chrome: cr.y, rustkit: rr.y });
    }

    if (diffs.length > 0) {
      results.mismatched++;
      results.differences.push({ selector, diffs });
    } else {
      results.matched++;
    }
  }

  for (const selector of rustkitMap.keys()) {
    if (!chromeMap.has(selector)) {
      results.rustkitOnly++;
    }
  }

  return results;
}

/**
 * Full triple comparison
 */
export async function tripleCompare(baselineDir, rustkitCaptureDir, outputDir) {
  mkdirSync(outputDir, { recursive: true });

  const results = {
    pixel: null,
    styles: null,
    rects: null,
    summary: {},
  };

  // 1. Pixel diff
  const chromePng = join(baselineDir, 'baseline.png');
  const rustkitPpm = join(rustkitCaptureDir, 'frame.ppm');
  const rustkitPng = join(rustkitCaptureDir, 'frame.png');

  const rustkitImage = existsSync(rustkitPpm) ? rustkitPpm : rustkitPng;

  if (existsSync(chromePng) && existsSync(rustkitImage)) {
    results.pixel = await comparePixels(chromePng, rustkitImage, outputDir);
    results.summary.pixelDiff = results.pixel.diffPercent;
  } else {
    results.summary.pixelDiff = null;
    results.summary.pixelError = 'Missing images';
  }

  // 2. Computed styles
  const chromeStyles = join(baselineDir, 'computed-styles.json');
  const rustkitStyles = join(rustkitCaptureDir, 'computed-styles.json');

  if (existsSync(chromeStyles)) {
    results.styles = compareStyles(chromeStyles, rustkitStyles);
    results.summary.styleMatched = results.styles.matched;
    results.summary.styleMismatched = results.styles.mismatched;
  }

  // 3. Layout rects
  const chromeRects = join(baselineDir, 'layout-rects.json');
  const rustkitRects = join(rustkitCaptureDir, 'layout.json');

  if (existsSync(chromeRects)) {
    results.rects = compareRects(chromeRects, rustkitRects);
    results.summary.rectMatched = results.rects.matched;
    results.summary.rectMismatched = results.rects.mismatched;
  }

  // Save full results
  const resultsPath = join(outputDir, 'comparison.json');
  writeFileSync(resultsPath, JSON.stringify(results, null, 2));

  return results;
}
