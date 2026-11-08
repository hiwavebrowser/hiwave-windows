// CH2.E7 R1 (#900): what Chrome reports for a styled scroller's border,
// client box and flex geometry, and which events the page hears for a
// press on a styled scrollbar. Expected values in
// crates/rustkit-engine/src/styled_scrollbar_tests.rs (the R1 section).
// node tools/parity_oracle/scrollbar_events_log.mjs
//
// Chrome for Testing 153.0.8010.12, Linux, 2026-11-05:
//   border:  3px 2px 2 2 294 200 305 305 294   (borderRight/BottomWidth,
//            clientLeft/Top, clientWidth/Height, offsetWidth, rect width,
//            child width; border 2px, right 3px, bar 6px)
//   noborder: 0px 0 294 300
//   flexrow: 100 280 200 274 200 274 380
//   flexcol: 50 300 350 294 350 294
//   thumb/track/drag (also released off the bar):
//            pointerdown:s,mousedown:s@297,pointerup:s,mouseup:s  (no click)
//   focus stays on the focused input; a cancelled mousedown still pages (175)
//   content: pointerdown:c,mousedown:c@90,pointerup:c,mouseup:c,click:c
//   viewport bar over a fixed 100vw box:
//            pointerdown:HTML,mousedown:HTML,pointerup:HTML,mouseup:HTML
import { chromium } from 'playwright';

const S = `#s::-webkit-scrollbar{width:6px}#s::-webkit-scrollbar-track{background:rgb(0,0,255)}#s::-webkit-scrollbar-thumb{background:#888}`;
const LOG = `<script>window.log=[];for (const t of ['pointerdown','mousedown','pointerup','mouseup','click'])
  document.addEventListener(t, e=>log.push(t+':'+(e.target.id||e.target.nodeName)+(t=='mousedown'?'@'+e.offsetX:'')), true);</script>`;
const BOX = `#s{position:absolute;left:10px;top:20px;width:300px;height:200px;overflow:auto}`;

const browser = await chromium.launch({ ignoreDefaultArgs: ['--hide-scrollbars'], ...(process.env.PW_CHROMIUM ? { executablePath: process.env.PW_CHROMIUM } : {}) });
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
const out = { chrome: browser.version() };

await page.setContent(`<style>body{margin:0}${BOX}#s{border:2px solid black;border-right-width:3px}${S}#c{height:600px}</style><div id=s><div id=c></div></div>`);
out.border = await page.evaluate(() => { const s = document.getElementById('s'), cs = getComputedStyle(s);
  return [cs.borderRightWidth, cs.borderBottomWidth, s.clientLeft, s.clientTop, s.clientWidth, s.clientHeight, s.offsetWidth,
    s.getBoundingClientRect().width, document.getElementById('c').getBoundingClientRect().width].join(' '); });
await page.setContent(`<style>body{margin:0}${BOX}${S}#c{height:600px}</style><div id=s><div id=c></div></div>`);
out.noborder = await page.evaluate(() => { const s = document.getElementById('s');
  return [getComputedStyle(s).borderRightWidth, s.clientLeft, s.clientWidth, s.offsetWidth].join(' '); });

await page.setContent(`<style>body{margin:0}#f{display:flex;width:500px;height:200px}#a{width:100px;flex:none}#s{flex:1;overflow:auto;min-width:0}${S}#c{height:600px}#d{width:120px;flex:none}</style><div id=f><div id=a></div><div id=s><div id=c></div></div><div id=d></div></div>`);
out.flexrow = await page.evaluate(() => { const s = document.getElementById('s'), r = s.getBoundingClientRect();
  return [r.x, r.width, r.height, s.clientWidth, s.clientHeight, document.getElementById('c').getBoundingClientRect().width,
    document.getElementById('d').getBoundingClientRect().x].join(' '); });
await page.setContent(`<style>body{margin:0}#f{display:flex;flex-direction:column;width:300px;height:400px}#h{height:50px;flex:none}#s{flex:1;overflow:auto;min-height:0}${S}#c{height:600px}</style><div id=f><div id=h></div><div id=s><div id=c></div></div></div>`);
out.flexcol = await page.evaluate(() => { const s = document.getElementById('s'), r = s.getBoundingClientRect();
  return [r.y, r.width, r.height, s.clientWidth, s.clientHeight, document.getElementById('c').getBoundingClientRect().width].join(' '); });

await page.setContent(`<style>body{margin:0}${BOX}${S}#c{height:600px}</style><div id=s><div id=c></div></div><input id=i style="position:absolute;top:300px">${LOG}`);
await page.focus('#i');
async function press(x, y, x2, y2) {
  await page.evaluate(() => { log.length = 0; });
  await page.mouse.move(x, y); await page.mouse.down();
  if (x2 !== undefined) await page.mouse.move(x2, y2, { steps: 3 });
  await page.mouse.up();
  return page.evaluate(() => log.join(','));
}
out.thumb = await press(307, 30);
out.track = await press(307, 170);
out.drag_off_bar = await press(307, 30, 450, 80);
out.focus_after = await page.evaluate(() => document.activeElement.id);
out.content = await press(100, 100);
await page.evaluate(() => { const s = document.getElementById('s'); s.scrollTop = 0; s.addEventListener('mousedown', e => e.preventDefault()); });
await press(307, 170);
await page.waitForTimeout(500);
out.cancelled_scrolltop = await page.evaluate(() => document.getElementById('s').scrollTop);

await page.setContent(`<style>body{margin:0}::-webkit-scrollbar{width:6px}::-webkit-scrollbar-thumb{background:#888}#c{height:2400px}#fx{position:fixed;left:0;top:0;width:100vw;height:100vh}</style><div id=c></div><div id=fx></div>${LOG}`);
out.viewport_bar = await press(797, 300);

console.log(JSON.stringify(out, null, 1));
await browser.close();
