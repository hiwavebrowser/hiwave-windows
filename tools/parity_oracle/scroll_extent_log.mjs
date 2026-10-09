// How far can the user scroll each page in scroll_extent_cases.json?
//
// The pinned Chromium is asked two things per page at 800x600: what the
// wheel reaches (the mouse over the page, one very large wheel turn, then
// window.scrollY), and what script reaches (window.scrollTo to a very large
// y). The engine's scrollable extent is tested against these numbers
// (crates/rustkit-engine, scroll_extent_tests).
//
//   node scroll_extent_log.mjs            print
//   node scroll_extent_log.mjs --write    print, and store in the case file
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const FILE = new URL('./scroll_extent_cases.json', import.meta.url);
const data = JSON.parse(readFileSync(FILE, 'utf8'));
const [width, height] = data.viewport;
const browser = await chromium.launch(getDeterministicLaunchOptions());
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage({ viewport: { width, height } });
  await page.setContent(c.html);
  await page.mouse.move(400, 300);
  await page.mouse.wheel(0, 100000);
  await page.waitForTimeout(300);
  const wheel = await page.evaluate(() => window.scrollY);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.evaluate(() => window.scrollTo(0, 1e7));
  const script = await page.evaluate(() => window.scrollY);
  c.chrome_wheel_y = Math.round(wheel);
  c.chrome_script_y = Math.round(script);
  console.log([c.name, 'wheel', c.chrome_wheel_y, 'script', c.chrome_script_y].join(' '));
  await page.close();
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
