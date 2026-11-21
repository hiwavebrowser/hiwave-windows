// CH2.E7: Chrome's styled-scrollbar geometry, for the RustKit engine tests
// in crates/rustkit-engine/src/styled_scrollbar_tests.rs.
// node tools/parity_oracle/scrollbar_geometry_log.mjs
import { chromium } from 'playwright';
import { PNG } from 'pngjs';

const CASES = {
  webkit6: `<style>body{margin:0}#s{width:300px;height:200px;overflow:auto;background:#fff}
    #s::-webkit-scrollbar{width:6px} #s::-webkit-scrollbar-track{background:rgb(0,0,255)}
    #s::-webkit-scrollbar-thumb{background:rgb(255,0,0);border-radius:3px}
    #c{height:600px}</style><div id=s><div id=c></div></div>`,
  none: `<style>body{margin:0}#s{width:300px;height:200px;overflow:auto;scrollbar-width:none}
    #s::-webkit-scrollbar{width:6px}#c{height:600px}</style><div id=s><div id=c></div></div>`,
  displaynone: `<style>body{margin:0}#s{width:300px;height:200px;overflow:auto}
    #s::-webkit-scrollbar{display:none}#c{height:600px}</style><div id=s><div id=c></div></div>`,
  thin: `<style>body{margin:0}#s{width:300px;height:200px;overflow:auto;scrollbar-width:thin;scrollbar-color:rgb(255,0,0) rgb(0,0,255)}
    #c{height:600px}</style><div id=s><div id=c></div></div>`,
  scroll_short: `<style>body{margin:0}#s{width:300px;height:200px;overflow:scroll}
    #s::-webkit-scrollbar{width:6px;height:6px}#s::-webkit-scrollbar-thumb{background:red}#c{height:50px}</style><div id=s><div id=c></div></div>`,
  root: `<style>body{margin:0}::-webkit-scrollbar{width:6px}::-webkit-scrollbar-thumb{background:rgb(255,0,0)}
    #c{height:2400px}</style><div id=c></div>`,
};

const browser = await chromium.launch({ ignoreDefaultArgs: ['--hide-scrollbars'], ...(process.env.PW_CHROMIUM ? { executablePath: process.env.PW_CHROMIUM } : {}) });
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
for (const [name, html] of Object.entries(CASES)) {
  await page.setContent(html);
  const geo = await page.evaluate(() => {
    const s = document.getElementById('s') || document.documentElement;
    const c = document.getElementById('c');
    return { clientWidth: s.clientWidth, offsetWidth: s.offsetWidth, cWidth: c.getBoundingClientRect().width,
      scrollHeight: s.scrollHeight, sw: getComputedStyle(s).scrollbarWidth, w: getComputedStyle(s).width };
  });
  // Find the red thumb's vertical extent in column x.
  const thumbSpan = async (x) => {
    const png = PNG.sync.read(await page.screenshot());
    let top = -1, bottom = -1;
    for (let y = 0; y < png.height; y++) {
      const i = (y * png.width + x) * 4;
      const red = png.data[i] > 200 && png.data[i + 1] < 60 && png.data[i + 2] < 60;
      if (red) { if (top < 0) top = y; bottom = y; }
    }
    return [top, bottom + 1];
  };
  const x = name === 'root' ? 797 : 297;
  const before = await thumbSpan(x);
  if (name === 'root') await page.evaluate(() => window.scrollTo(0, 600));
  else await page.evaluate(() => { const s = document.getElementById('s'); if (s) s.scrollTop = 200; });
  const after = await thumbSpan(x);
  console.log(name, JSON.stringify(geo), 'thumb', before, 'after-scroll', after);
}
// Drag: 50px on the thumb of webkit6.
await page.setContent(CASES.webkit6);
for (const steps of [1, 10]) {
  await page.setContent(CASES.webkit6);
  await page.mouse.move(297, 10); await page.mouse.down(); await page.mouse.move(297, 60, { steps }); await page.waitForTimeout(300);
  console.log('drag50 steps', steps, 'scrollTop', await page.evaluate(() => document.getElementById('s').scrollTop));
  await page.mouse.up();
}
await page.setContent(CASES.webkit6);
await page.mouse.move(297, 150); await page.mouse.down(); await page.waitForTimeout(60); await page.mouse.up(); await page.waitForTimeout(800);
console.log('track-click scrollTop', await page.evaluate(() => document.getElementById('s').scrollTop));
// R1 on #900: what script reads of a scroller with a bar, flex items, the
// events a bar press fires, and the shortest thumb.
const SB = '#s::-webkit-scrollbar{width:6px}#s::-webkit-scrollbar-thumb{background:rgb(255,0,0)}';
await page.setContent(`<style>body{margin:0}#s{position:absolute;left:10px;top:20px;width:300px;height:200px;overflow:auto;border:2px solid black;padding:4px}${SB}#c{height:600px}</style><div id=s><div id=c></div></div>`);
console.log('geometry', await page.evaluate(() => { const s = document.getElementById('s'); const r = s.getBoundingClientRect(); const cs = getComputedStyle(s);
  return [cs.borderRightWidth, cs.width, s.clientLeft, s.clientTop, s.clientWidth, s.clientHeight, s.offsetWidth, s.offsetHeight, r.x, r.y, r.width, r.height, s.scrollWidth, document.getElementById('c').getBoundingClientRect().width].join(' '); }));
await page.setContent(`<style>body{margin:0}#f{display:flex;height:200px;width:500px}#a{width:100px}#s{flex:1;overflow:auto}${SB}#c{height:600px}</style><div id=f><div id=a></div><div id=s><div id=c></div></div></div>`);
console.log('flex-row', await page.evaluate(() => { const s = document.getElementById('s'); return [s.clientWidth, s.offsetWidth, document.getElementById('c').getBoundingClientRect().width].join(' '); }));
await page.setContent(`<style>body{margin:0}#f{display:flex;flex-direction:column;height:300px;width:400px}#h{height:50px}#s{flex:1;min-height:0;overflow:auto}${SB}#c{height:600px}</style><div id=f><div id=h></div><div id=s><div id=c></div></div></div>`);
console.log('flex-column', await page.evaluate(() => { const s = document.getElementById('s'); return [s.clientWidth, s.clientHeight, s.offsetWidth, document.getElementById('c').getBoundingClientRect().width].join(' '); }));
const LOG = `<script>window.log=[];for(const t of ['pointerdown','mousedown','pointerup','mouseup','click'])document.addEventListener(t,e=>log.push(t+':'+(e.target.id||e.target.tagName)),true)</script>`;
for (const [name, html, x, y] of [
  ['events-element-thumb', CASES.webkit6 + LOG, 297, 10],
  ['events-element-track', CASES.webkit6 + LOG, 297, 150],
  ['events-viewport-thumb', CASES.root + '<div style="position:fixed;top:0;left:0;width:100vw;height:600px"></div>' + LOG, 797, 30],
]) {
  await page.setContent(html);
  await page.mouse.move(x, y); await page.mouse.down(); await page.mouse.move(x, y + 20, { steps: 2 }); await page.mouse.up(); await page.waitForTimeout(100);
  console.log(name, await page.evaluate(() => log.join(' ')), 'scrollY', await page.evaluate(() => window.scrollY));
}
// long_doc: the shortest thumb (17px at every width and track length tried).
await page.setContent(`<style>body{margin:0}#s{width:300px;height:200px;overflow:auto}${SB}#c{height:100000px}</style><div id=s><div id=c></div></div>`);
{
  const png = PNG.sync.read(await page.screenshot()); let n = 0;
  for (let y = 0; y < 200; y++) { const i = (y * png.width + 297) * 4; if (png.data[i] > 200 && png.data[i + 1] < 60) n++; }
  console.log('long_doc thumb', n);
}
await browser.close();
