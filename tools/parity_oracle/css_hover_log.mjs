// What the oracle's Chrome styles as `:hover` and `:active` while the mouse
// moves, presses and releases: the widths the engine test
// `css_hover_and_active_follow_the_pointer` expects (Z I0).
// usage: node tools/parity_oracle/css_hover_log.mjs tools/parity_oracle/css_hover_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
// width@left of #b, #c and #sub: every rule of the page shows in one of them.
const read = () => page.evaluate(() => ['b', 'c', 'sub'].map((i) => {
  const r = document.getElementById(i).getBoundingClientRect();
  return r.width + '@' + r.left;
}).join(' '));
const step = async (label, fn) => {
  await fn();
  console.log('## ' + label);
  console.log(await read());
};
console.log('chrome ' + browser.version());
await step('loaded', async () => {});
await step('move over #b (12,20)', () => page.mouse.move(12, 20));
await step('move over #c (12,60)', () => page.mouse.move(12, 60));
await step('move over #t (12,100)', () => page.mouse.move(12, 100));
await step('move over #b (12,20)', () => page.mouse.move(12, 20));
await step('press on #b', () => page.mouse.down());
await step('held, move over #c (12,60)', () => page.mouse.move(12, 60));
await step('held, move back over #b (12,20)', () => page.mouse.move(12, 20));
await step('held, move over #c (12,60)', () => page.mouse.move(12, 60));
await step('release over #c', () => page.mouse.up());
await step('move over #t (12,100)', () => page.mouse.move(12, 100));
await step('press on #t', () => page.mouse.down());
await step('release on #t', () => page.mouse.up());
await browser.close();
