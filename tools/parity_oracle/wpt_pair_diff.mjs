/**
 * wpt_pair_diff.mjs - the campaign's pixel rule on a WPT test/ref frame pair.
 *
 * Same pixelmatch call as compare_baseline.mjs (threshold 0.1, includeAA).
 * Prints {"diff_pixels": N, "diff_pct": P}. A size mismatch is a full-frame
 * diff, as in scripts/wpt_tier1.py.
 *
 * Usage: node wpt_pair_diff.mjs <test.ppm> <ref.ppm>
 */

import pixelmatch from 'pixelmatch';
import { ppmToRgba } from './compare_pixels.mjs';

const test = ppmToRgba(process.argv[2]);
const ref = ppmToRgba(process.argv[3]);

if (test.width !== ref.width || test.height !== ref.height) {
  const total = Math.max(test.width * test.height, ref.width * ref.height);
  console.log(JSON.stringify({ diff_pixels: total, diff_pct: 100 }));
} else {
  const diffPixels = pixelmatch(test.data, ref.data, null, test.width, test.height, {
    threshold: 0.1,
    includeAA: true,
  });
  const diffPct = (diffPixels / (test.width * test.height)) * 100;
  console.log(JSON.stringify({ diff_pixels: diffPixels, diff_pct: diffPct }));
}
