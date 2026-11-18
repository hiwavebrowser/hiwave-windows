// Canvas on screen (STR-7b): each page in canvas_screen_cases.json draws
// into a <canvas> at the page origin. This takes Chrome's SCREENSHOT of the
// top-left `size` region (not getImageData): what the compositor shows,
// with the canvas bitmap scaled to its CSS box. The engine test
// (crates/rustkit-engine, canvas_context_tests
// `canvas_frames_match_chrome_screenshots`) captures the same pages through
// the renderer and compares.
//
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node canvas_screen_log.mjs [--write]
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { PNG } from 'pngjs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const FILE = new URL('./canvas_screen_cases.json', import.meta.url);
const data = JSON.parse(readFileSync(FILE, 'utf8'));
const [W, H] = data.size;
const browser = await chromium.launch({ ...getDeterministicLaunchOptions(), headless: true });
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage({ viewport: { width: 200, height: 200 }, deviceScaleFactor: 1 });
  await page.setContent(c.html);
  await page.waitForFunction(() => window.__done === true);
  const png = PNG.sync.read(await page.screenshot({ clip: { x: 0, y: 0, width: W, height: H } }));
  let hex = '';
  for (let i = 0; i < W * H; i++) {
    for (let k = 0; k < 3; k++) hex += png.data[i * 4 + k].toString(16).padStart(2, '0');
  }
  c.chrome_rgb = hex;
  console.log(c.name + '  ' + hex.length / 6 + ' pixels');
  await page.close();
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
