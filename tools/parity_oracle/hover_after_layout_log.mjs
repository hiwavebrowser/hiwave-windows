// CH2.E15: what the oracle's Chrome fires, and when, when layout or a
// scroll moves an element under a pointer that does not move.
// usage: node tools/parity_oracle/hover_after_layout_log.mjs tools/parity_oracle/hover_after_layout_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
await page.evaluate(() => {
  window.__log = [];
  window.__frame = 0;
  const tick = () => { window.__frame++; requestAnimationFrame(tick); };
  requestAnimationFrame(tick);
  const name = (n) => (n && n.id) || (n && n.nodeName) || 'null';
  for (const t of ['pointerover', 'pointerenter', 'pointerout', 'pointerleave', 'pointermove',
                   'mouseover', 'mouseenter', 'mouseout', 'mouseleave', 'mousemove']) {
    document.addEventListener(t, (e) => {
      if (e.target.nodeType !== 1) return;
      window.__log.push(`f${window.__frame} ${t} ${name(e.target)} rel=${name(e.relatedTarget)}`);
    }, true);
  }
});
const hovered = () => page.evaluate(() => [...document.querySelectorAll(':hover')].map((n) => n.id || n.nodeName).join('>'));
const drain = () => page.evaluate(() => { const l = window.__log; window.__log = []; return l; });
const settle = () => page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(r)))));
const step = async (label, fn, sync) => {
  const s = await page.evaluate(fn);
  const before = await drain();
  if (sync) console.log('   sync:', await page.evaluate(() => [...document.querySelectorAll(':hover')].map((n) => n.id || n.nodeName).join('>')), 'frame', s);
  await settle();
  console.log('## ' + label);
  console.log('   events:', JSON.stringify([...before, ...(await drain())]));
  console.log('   :hover', await hovered());
};
console.log('chrome ' + browser.version());
// Pointer over #side's right part: x=50 (tab starts at 100 until collapse).
await page.mouse.move(50, 20);
await settle();
await drain();
console.log('## start at (50,20)  :hover', await hovered());
await step('collapse (tab slides under still pointer)', () => {
  document.getElementById('row').classList.add('collapsed');
  // Same-task reads: does a forced layout update :hover synchronously?
  document.body.offsetWidth;
  return [...document.querySelectorAll(':hover')].map((n) => n.id || n.nodeName).join('>') + ' @f' + window.__frame;
}, true);
await step('expand (tab slides away)', () => { document.getElementById('row').classList.remove('collapsed'); document.body.offsetWidth; return [...document.querySelectorAll(':hover')].map((n) => n.id || n.nodeName).join('>') + ' @f' + window.__frame; }, true);
await step('no-op layout (width of #side 100->90, pointer still over #side)', () => { document.getElementById('side').style.width = '90px'; return ''; });
// Scroll under a still pointer.
await page.mouse.move(10, 70);
await settle();
await drain();
console.log('## pointer at (10,70) :hover', await hovered());
await step('scroller.scrollTop = 40', () => { document.getElementById('scroller').scrollTop = 40; document.body.offsetWidth; return [...document.querySelectorAll(':hover')].map((n) => n.id || n.nodeName).join('>'); }, true);
// A wheel scroll under the pointer (user scroll, not script).
await step('scroller.scrollTop = 0', () => { document.getElementById('scroller').scrollTop = 0; return ''; });
await page.mouse.wheel(0, 40);
await page.waitForTimeout(300);
console.log('## wheel 40 over the scroller, scrollTop', await page.evaluate(() => document.getElementById('scroller').scrollTop));
console.log('   events:', JSON.stringify(await drain()));
console.log('   :hover', await hovered());
await page.waitForTimeout(1000);
console.log('   after 1 s: events', JSON.stringify(await drain()), ':hover', await hovered());
// The hovered element is removed: who hears what, and when.
await step('remove the hovered row', () => { const n = document.querySelector('#scroller div:hover'); n.remove(); document.body.offsetWidth; return [...document.querySelectorAll(':hover')].map((n) => n.id || n.nodeName).join('>'); }, true);
// The page scrolls under the pointer at (50,20).
await page.mouse.move(50, 20);
await settle();
await drain();
await step('window.scrollTo(0, 40) with the pointer at (50,20)', () => { window.scrollTo(0, 40); return ''; });
await browser.close();
