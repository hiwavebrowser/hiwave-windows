#!/usr/bin/env node
/**
 * wpt_pair_chromium.mjs - Chromium's own test against its own reference (T15b).
 *
 *   node wpt_pair_chromium.mjs <width> <height>  < pairs.json
 *
 * stdin is [{"id", "test", "ref"}], absolute paths to the staged pages. Each
 * page is rendered file:// at the given viewport, once its fonts are ready, and the
 * two frames compared at scripts/wpt_tier1.py's rule, exact RGB. Prints
 * {"version": V, "diffs": {id: pixels differing}}. A size mismatch is a full-frame diff.
 */

import { chromium } from 'playwright';
import { PNG } from 'pngjs';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions, createDeterministicContext } from './deterministic.mjs';

const [width, height] = process.argv.slice(2).map(Number);
const pairs = JSON.parse(readFileSync(0, 'utf8'));

const browser = await chromium.launch(getDeterministicLaunchOptions());
const context = await createDeterministicContext(browser, width, height);
const page = await context.newPage();

async function frame(path) {
  await page.goto('file://' + path, { waitUntil: 'load' });
  await page.evaluate(() => document.fonts.ready.then(() => {}));
  return PNG.sync.read(await page.screenshot());
}

const diffs = {};
for (const { id, test, ref } of pairs) {
  const a = await frame(test);
  const b = await frame(ref);
  if (a.width !== b.width || a.height !== b.height) {
    diffs[id] = Math.max(a.width * a.height, b.width * b.height);
    continue;
  }
  let differing = 0;
  for (let i = 0; i < a.data.length; i += 4) {
    if (a.data[i] !== b.data[i] || a.data[i + 1] !== b.data[i + 1] || a.data[i + 2] !== b.data[i + 2]) {
      differing++;
    }
  }
  diffs[id] = differing;
}

console.log(JSON.stringify({ version: browser.version(), diffs }));
await browser.close();
