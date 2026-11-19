// Canvas drawImage of a loaded <img> (STR-7 slice 2): each page in
// canvas_img_cases.json draws a data: PNG into a canvas once the image
// loads and leaves window.__r = hex RGBA of the canvas's getImageData.
// The engine test (crates/rustkit-engine, canvas_context_tests) loads the
// same page and compares.
//
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node canvas_img_log.mjs [--write]
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const FILE = new URL('./canvas_img_cases.json', import.meta.url);
const data = JSON.parse(readFileSync(FILE, 'utf8'));
const browser = await chromium.launch({ ...getDeterministicLaunchOptions(), headless: true });
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage({ viewport: { width: 200, height: 200 }, deviceScaleFactor: 1 });
  await page.setContent(c.html);
  await page.waitForFunction(() => typeof window.__r === 'string');
  c.chrome_rgba = await page.evaluate(() => window.__r);
  console.log(c.name + '  ' + c.chrome_rgba.length / 8 + ' pixels');
  await page.close();
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
