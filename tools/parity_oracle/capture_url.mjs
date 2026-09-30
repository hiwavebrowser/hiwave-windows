/**
 * capture_url.mjs - Oracle screenshot of a URL (scripts run, as in a browser)
 *
 * The js-ladder fixtures are served over http://127.0.0.1 so that Chrome and
 * `parity-capture --url` load the exact same URL. Same deterministic launch
 * and context as every other oracle capture; no parity reset.
 *
 *   node tools/parity_oracle/capture_url.mjs <url> <out.png> <width> <height>
 */

import { chromium } from 'playwright';
import { dirname } from 'path';
import { mkdirSync } from 'fs';
import { createDeterministicContext, getDeterministicLaunchOptions } from './deterministic.mjs';

export async function captureUrl(url, outputPath, width, height) {
  mkdirSync(dirname(outputPath), { recursive: true });
  const browser = await chromium.launch(getDeterministicLaunchOptions());
  try {
    const context = await createDeterministicContext(browser, width, height);
    const page = await context.newPage();
    const errors = [];
    page.on('pageerror', (e) => errors.push(String(e?.message || e)));
    await page.goto(url, { waitUntil: 'networkidle' });
    await page.waitForTimeout(50); // animations are frozen; let layout/fonts settle
    await page.screenshot({ path: outputPath, type: 'png', fullPage: false });
    await context.close();
    return { success: true, pageErrors: errors };
  } finally {
    await browser.close();
  }
}

if (process.argv[1] && process.argv[1].endsWith('capture_url.mjs')) {
  const [url, out, w, h] = process.argv.slice(2);
  const r = await captureUrl(url, out, Number(w), Number(h));
  console.log(JSON.stringify(r));
}
