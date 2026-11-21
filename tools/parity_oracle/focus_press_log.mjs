// When the oracle's Chrome moves the focus during a press and release: the
// log the engine test `the_focus_moves_at_the_press` expects (Z I0).
// usage: node tools/parity_oracle/focus_press_log.mjs tools/parity_oracle/focus_press_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
await page.evaluate(() => {
  window.log = [];
  const nm = (n) => (n ? (n.id || n.nodeName) : 'null');
  ['pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click', 'focus', 'blur', 'focusin', 'focusout', 'change']
    .forEach((t) => {
      // type:target:relatedTarget:the active element as the listener sees it
      document.addEventListener(t, (e) => {
        log.push([e.type, nm(e.target), nm(e.relatedTarget), nm(document.activeElement)].join(':'));
      }, true);
    });
  document.getElementById('opt').addEventListener('mousedown', (e) => e.preventDefault());
  document.getElementById('pd').addEventListener('pointerdown', (e) => e.preventDefault());
});
const step = async (label, fn) => {
  await page.evaluate(() => { window.log.length = 0; });
  await fn();
  const log = await page.evaluate(() => window.log);
  console.log('## ' + label);
  log.forEach((l) => console.log(l));
};
const press = async (x, y) => { await page.mouse.move(x, y); await page.mouse.down(); };
console.log('chrome ' + browser.version());
await step('press in #f (12,15)', () => press(12, 15));
await step('release in #f', () => page.mouse.up());
await step('press in #g (12,45)', () => press(12, 45));
await step('release in #g', () => page.mouse.up());
await step('press on #opt, whose mousedown is cancelled (12,80)', () => press(12, 80));
await step('release on #opt', () => page.mouse.up());
await step('press on plain text #d (12,120)', () => press(12, 120));
await step('release on #d', () => page.mouse.up());
await step('press in #f (12,15)', () => press(12, 15));
await step('release over #drag (12,160)', async () => { await page.mouse.move(12, 160); await page.mouse.up(); });
await step('press on #pd, whose pointerdown is cancelled (12,200)', () => press(12, 200));
await step('release on #pd', () => page.mouse.up());
await browser.close();
