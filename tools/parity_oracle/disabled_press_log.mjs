// What the oracle's Chrome sends for a move onto, a press on and a release
// on a disabled form control: the log the engine test
// `a_disabled_control_hears_pointer_events_and_no_mouse_events` expects (Z I0).
// usage: node tools/parity_oracle/disabled_press_log.mjs tools/parity_oracle/disabled_press_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
await page.evaluate(() => {
  window.log = [];
  ['pointerover', 'mouseover', 'pointermove', 'mousemove', 'pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click',
    'pointerout', 'mouseout']
    .forEach((t) => {
      document.addEventListener(t, (e) => { log.push(e.type + ':' + (e.target.id || e.target.nodeName)); }, true);
    });
});
const step = async (label, fn) => {
  await page.evaluate(() => { window.log.length = 0; });
  await fn();
  const log = await page.evaluate(() => window.log);
  console.log('## ' + label);
  console.log(log.join(' '));
};
console.log('chrome ' + browser.version());
for (const id of ['on', 'off', 'cb', 'tx', 'in', 'fs', 'lg', 'plain']) {
  const box = await page.evaluate((i) => {
    const r = document.getElementById(i).getBoundingClientRect();
    return [r.left + 8, r.top + 10];
  }, id);
  await page.mouse.move(400, 500);
  await step('move onto #' + id, () => page.mouse.move(box[0], box[1]));
  await step('press on #' + id, () => page.mouse.down());
  await step('release on #' + id, () => page.mouse.up());
  await step('move off #' + id, () => page.mouse.move(400, 500));
}
await browser.close();
