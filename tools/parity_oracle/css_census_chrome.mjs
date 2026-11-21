#!/usr/bin/env node
/**
 * css_census_chrome.mjs - the oracle arm of the CSS property census (dev-tools T13).
 *
 *   node css_census_chrome.mjs <work dir> <width> <height>
 *
 * Reads <work>/manifest.json (fixture ids), loads <work>/fixtures/<id>.html, and
 * writes <work>/chrome/<id>.png and <id>.txt (the page's CENSUS: console line).
 * Prints the browser version last, for the caller to pin.
 */

import { chromium } from 'playwright';
import { readFileSync, writeFileSync } from 'fs';
import { join, resolve } from 'path';
import { getDeterministicLaunchOptions, createDeterministicContext } from './deterministic.mjs';

const [workArg, width, height] = process.argv.slice(2);
const work = resolve(workArg);
const ids = JSON.parse(readFileSync(join(work, 'manifest.json'), 'utf8'));

const browser = await chromium.launch(getDeterministicLaunchOptions());
const context = await createDeterministicContext(browser, Number(width), Number(height));
const page = await context.newPage();
let line = '';
page.on('console', (m) => {
  if (m.text().startsWith('CENSUS:')) line = m.text();
});
for (const id of ids) {
  line = '';
  await page.goto('file://' + join(work, 'fixtures', id + '.html'), { waitUntil: 'load' });
  await page.screenshot({ path: join(work, 'chrome', id + '.png') });
  writeFileSync(join(work, 'chrome', id + '.txt'), line);
}
console.log(browser.version());
await browser.close();
