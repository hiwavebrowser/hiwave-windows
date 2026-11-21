// CSS 2.1 §9.5 (row REPLACED-FLOAT-AVOID): the border box of a block-level replaced
// element, a table, or a block that establishes a new block formatting context must not
// overlap the margin box of a float in the same formatting context. It is placed beside
// the float when it fits in the width the float leaves, else moved down until it fits.
// What the pinned Chrome lays out: each case's elements with an id report
// `id=x,y,width,height` (getBoundingClientRect), in a 400px-wide container.
// Writes docs/diagnostics/2026-11-19/replaced_float_avoid_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/replaced_float_avoid_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-19/replaced_float_avoid_cft148.json');
// A 10x10 red PNG.
const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAIAAAACUFjqAAAAEklEQVR4nGP4z8CAB+GTG8HSALfKY52fTcuYAAAAAElFTkSuQmCC';
const L = '<div id=f style="float:left;width:100px;height:80px"></div>';
const R = '<div id=f style="float:right;width:100px;height:80px"></div>';
const IMG = (style) => `<img id=e style="display:block;${style}" src="${PNG}">`;
const N = '<div id=n style="height:10px"></div>';

const CASES = [
  ['img beside a left float', `${L}${IMG('width:150px;height:40px')}${N}`],
  ['img beside a right float', `${R}${IMG('width:150px;height:40px')}${N}`],
  ['img too wide moves below the float', `${L}${IMG('width:350px;height:40px')}${N}`],
  ['img exactly fits', `${L}${IMG('width:300px;height:40px')}`],
  ['img at its natural size beside a float', `${L}${IMG('')}`],
  ['img with a left margin beside a left float', `${L}${IMG('width:150px;height:40px;margin-left:20px')}`],
  ['img with a right margin beside a right float', `${R}${IMG('width:150px;height:40px;margin-right:20px')}`],
  ['img past a short float then a wide one', `<div id=f style="float:left;width:100px;height:30px"></div><div id=g style="float:left;clear:left;width:300px;height:30px"></div>${IMG('width:250px;height:40px')}`],
  ['img with a margin-left:auto beside a left float', `${L}${IMG('width:150px;height:40px;margin-left:auto')}`],
  ['svg beside a left float', `${L}<svg id=e style="display:block" width=150 height=40></svg>`],
  ['overflow hidden block beside a left float', `${L}<div id=e style="overflow:hidden;height:40px"></div>${N}`],
  ['overflow hidden block beside a right float', `${R}<div id=e style="overflow:hidden;height:40px"></div>`],
  ['overflow hidden block too wide moves below', `${L}<div id=e style="overflow:hidden;width:350px;height:40px"></div>`],
  ['flow-root block beside a left float', `${L}<div id=e style="display:flow-root;height:40px"></div>`],
  ['table beside a left float', `${L}<table id=e style="border-spacing:0;width:150px;height:40px"><tr><td style="padding:0"></td></tr></table>`],
  ['auto-width table beside a left float', `${L}<table id=e style="border-spacing:0;height:40px"><tr><td style="padding:0;width:60px"></td></tr></table>`],
  ['table too wide moves below', `${L}<table id=e style="border-spacing:0;width:350px;height:40px"><tr><td style="padding:0"></td></tr></table>`],
  ['plain block still overlaps the float (control)', `${L}<div id=e style="height:40px"></div>`],
  ['img block margin-top after a margined div, beside a float', `<div id=a style="height:20px;margin-bottom:30px"></div>${L}${IMG('width:150px;height:40px;margin-top:12px')}${N}`],
  ['img block with clear:left after a float', `<div id=a style="height:20px;margin-bottom:30px"></div>${L}${IMG('width:150px;height:40px;margin-top:12px;clear:left')}${N}`],
  // R1 (#1356): margins of float-avoiding boxes.
  ['overflow hidden block with margin-left beside a left float', `${L}<div id=e style="overflow:hidden;height:40px;margin-left:20px"></div>`],
  ['overflow hidden block with margin-left past a left float', `${L}<div id=e style="overflow:hidden;height:40px;margin-left:150px"></div>`],
  ['overflow hidden block with margin-right beside a right float', `${R}<div id=e style="overflow:hidden;height:40px;margin-right:20px"></div>`],
  ['img margin 0 auto beside a left float', `${L}${IMG('width:150px;height:40px;margin:0 auto')}`],
  ['img negative margin-left beside a right float', `${R}${IMG('width:150px;height:40px;margin-left:-20px')}`],
  ['img negative margin-left moved below a left float', `${L}${IMG('width:350px;height:40px;margin-left:-20px')}`],
  ['inline img with clear beside a float', `${L}<div id=w>text <img id=e style="clear:both;width:20px;height:20px;vertical-align:top" src="${PNG}"> more</div>`],
  ['img block float:left after a block img', `${IMG('width:50px;height:20px;margin-bottom:15px')}<img id=g style="float:left;width:50px;height:20px;margin-top:5px" src="${PNG}">${N}`],
];

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
const cases = [];
for (const [label, body] of CASES) {
  await page.setContent(`<!doctype html><style>body{margin:0}#c{width:400px}</style><div id=c>${body}</div>`);
  await page.evaluate(() => Promise.all(Array.from(document.images, (i) => i.decode().catch(() => {}))));
  const want = await page.evaluate(() =>
    Array.prototype.map.call(document.querySelectorAll('#c [id]'), (e) => {
      const r = e.getBoundingClientRect();
      return `${e.id}=${r.x},${r.y},${r.width},${r.height}`;
    }).join(' '));
  console.log(`${label}: ${want}`);
  cases.push({ label, body, want });
}
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, container_width: 400, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
