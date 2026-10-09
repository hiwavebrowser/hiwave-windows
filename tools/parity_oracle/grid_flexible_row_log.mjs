// How tall is a flexible (fr) row in a grid whose height is auto?
//
// The pinned Chromium lays out each page in grid_flexible_row_cases.json and
// reports, for every element with an id, the top and height of its border
// box as "id:y:height". The engine is tested against these strings
// (crates/rustkit-engine, grid_flexible_row_tests).
//
//   node grid_flexible_row_log.mjs            print
//   node grid_flexible_row_log.mjs --write    print, and store in the case file
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

// A second case file can be named: --file grid_item_min_max_cases.json. A
// file with "boxes": "xywh" gets "id:x:y:width:height" (BOXES_XYWH).
const named = process.argv.indexOf('--file');
const FILE = new URL('./' + (named > 0 ? process.argv[named + 1] : 'grid_flexible_row_cases.json'), import.meta.url);
// The same expression the engine test evaluates.
export const BOXES = `Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {
  var r = e.getBoundingClientRect();
  return e.id + ':' + Math.round(r.top) + ':' + Math.round(r.height);
}).join(' ')`;

export const BOXES_XYWH = `Array.prototype.map.call(document.querySelectorAll('[id]'), function (e) {
  var r = e.getBoundingClientRect();
  return e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height);
}).join(' ')`;

const data = JSON.parse(readFileSync(FILE, 'utf8'));
const [width, height] = data.viewport;
const browser = await chromium.launch(getDeterministicLaunchOptions());
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage({ viewport: { width, height } });
  await page.setContent(c.html);
  c.chrome_boxes = await page.evaluate(data.boxes === 'xywh' ? BOXES_XYWH : BOXES);
  console.log(c.name + '  ' + c.chrome_boxes);
  if (c.html_longhand) {
    // The same page with the shorthand written out as its longhands.
    await page.setContent(c.html_longhand);
    c.chrome_boxes_longhand = await page.evaluate(data.boxes === 'xywh' ? BOXES_XYWH : BOXES);
    console.log('  longhands ' + (c.chrome_boxes_longhand === c.chrome_boxes ? 'the same' : c.chrome_boxes_longhand));
  }
  await page.close();
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
