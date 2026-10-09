// What the oracle's Chrome sends to a page for mouse moves, a press, a drag
// and a release: the log the engine test
// `a_mouse_move_reaches_the_page_as_chrome_sends_it` expects (Z I0).
// usage: node tools/parity_oracle/pointer_event_log.mjs tools/parity_oracle/pointer_event_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const TYPES = ['pointerover', 'pointerenter', 'pointerout', 'pointerleave', 'pointermove',
  'mouseover', 'mouseenter', 'mouseout', 'mouseleave', 'mousemove',
  'pointerdown', 'mousedown', 'pointerup', 'mouseup', 'click'];
const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
await page.evaluate((types) => {
  window.log = [];
  const id = (n) => (n ? (n.id || n.nodeName || 'window') : 'null');
  types.forEach((t) => {
    // capture on the window hears the non-bubbling ones only at their target
    document.querySelectorAll('[id]').forEach((el) => {
      el.addEventListener(t, (e) => {
        if (e.eventPhase !== 2) return;
        log.push([e.type, id(e.target), id(e.relatedTarget), e.bubbles, e.cancelable, e.composed,
          e.button, e.buttons, e.which, e.detail, e instanceof PointerEvent, e.clientX, e.clientY,
          e.offsetX, e.offsetY, e.movementX, e.isTrusted].join(':'));
      });
    });
    document.addEventListener(t, (e) => { log.push('  doc<-' + e.type + ':' + id(e.target)); });
  });
}, TYPES);
const step = async (label, fn) => {
  await page.evaluate(() => { window.log.length = 0; });
  await fn();
  const log = await page.evaluate(() => window.log);
  console.log('## ' + label);
  log.forEach((l) => console.log(l));
};
console.log('chrome ' + browser.version());
await step('move to a (12,20) from nowhere', () => page.mouse.move(12, 20));
await step('move within a (30,25)', () => page.mouse.move(30, 25));
await step('move a -> i (12,70)', () => page.mouse.move(12, 70));
await step('move i -> o (12,120)', () => page.mouse.move(12, 120));
await step('move o -> a (12,20)', () => page.mouse.move(12, 20));
await step('press at a', () => page.mouse.down());
await step('drag a -> i with the button down (12,70)', () => page.mouse.move(12, 70));
await step('release at i', () => page.mouse.up());
await browser.close();
