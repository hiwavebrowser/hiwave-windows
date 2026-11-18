// Canvas 2D pixels (STR-7): what each script in canvas_raster_cases.json
// draws on a fresh 40x40 canvas, read back with getImageData as hex RGBA.
// The script sees the context as `x`. The engine test runs the same scripts
// (crates/rustkit-bindings, web_canvas_raster_tests) and compares pixels.
//
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node canvas_raster_log.mjs [--write] [--file other_cases.json]
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const named = process.argv.indexOf('--file');
const FILE = new URL('./' + (named > 0 ? process.argv[named + 1] : 'canvas_raster_cases.json'), import.meta.url);
const data = JSON.parse(readFileSync(FILE, 'utf8'));
const [w, h] = data.size;
const browser = await chromium.launch({ ...getDeterministicLaunchOptions(), headless: true });
console.log('chrome ' + browser.version());
const page = await browser.newPage({ viewport: { width: 200, height: 200 }, deviceScaleFactor: 1 });
await page.setContent('<!doctype html><body></body>');
for (const c of data.cases) {
  c.chrome_rgba = await page.evaluate(([script, w, h]) => {
    var cv = document.createElement('canvas');
    cv.width = w; cv.height = h;
    var x = cv.getContext('2d');
    new Function('x', script)(x);
    var d = x.getImageData(0, 0, w, h).data, hex = '';
    for (var i = 0; i < d.length; i++) hex += (d[i] < 16 ? '0' : '') + d[i].toString(16);
    return hex;
  }, [c.script, w, h]);
  let painted = 0;
  for (let i = 6; i < c.chrome_rgba.length; i += 8) if (c.chrome_rgba.substr(i, 2) !== '00') painted++;
  console.log(c.name + '  painted ' + painted);
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
