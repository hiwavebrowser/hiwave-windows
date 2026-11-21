// What the oracle's Chrome sends to a page for a right press, a double
// and a triple click, what a double click selects, and the computed
// `cursor` of the boxes the app maps to an NSCursor (CH2.E4). The engine
// tests in `mouse_surface_tests.rs` expect this log.
// usage: node tools/parity_oracle/mouse_surface_log.mjs tools/parity_oracle/mouse_surface_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const TYPES = ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'auxclick',
  'dblclick', 'contextmenu', 'pointermove', 'mousemove'];
const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
await page.evaluate((types) => {
  window.log = [];
  window.prevent = false;
  types.forEach((t) => document.addEventListener(t, (e) => {
    if (t === 'contextmenu' && window.prevent) e.preventDefault();
    log.push([e.type, e.target.id, e.button, e.buttons, e.detail, e instanceof PointerEvent,
      e.cancelable, e.defaultPrevented].join(':'));
  }));
}, TYPES);
const step = async (label, fn) => {
  await page.evaluate(() => { window.log.length = 0; getSelection().removeAllRanges(); });
  await fn();
  await page.waitForTimeout(50);
  const log = await page.evaluate(() => window.log);
  const sel = await page.evaluate(() => JSON.stringify(String(getSelection())));
  console.log('## ' + label);
  log.forEach((l) => console.log(l));
  console.log('selection ' + sel);
};
console.log('chrome ' + browser.version());
await page.mouse.move(60, 10);
await step('right click on "bravo" (60,10)', () => page.mouse.click(60, 10, { button: 'right' }));
await page.evaluate(() => { window.prevent = true; });
await step('right click, contextmenu prevented', () => page.mouse.click(60, 10, { button: 'right' }));
await page.evaluate(() => { window.prevent = false; });
await step('double click on "bravo" (60,10)', () => page.mouse.dblclick(60, 10));
await step('triple click on "bravo" (60,10)', () => page.mouse.click(60, 10, { clickCount: 3 }));
await step('drag (60,10) -> (120,10)', async () => {
  await page.mouse.down(); await page.mouse.move(120, 10); await page.mouse.up();
});
const cursors = await page.evaluate(() => ['p', 'link', 'text', 'ew', 'inner', 'na', 'grab', 'mv', 'def']
  .map((id) => id + '=' + getComputedStyle(document.getElementById(id)).cursor));
console.log('## computed cursor');
console.log(cursors.join(' '));
await browser.close();
