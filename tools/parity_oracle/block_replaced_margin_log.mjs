// Margins of block-level replaced boxes (`<img>` / `<svg>` with display: block) next to
// blocks: what the pinned Chrome lays out. Each case's elements with an id report
// `id=y/height` (getBoundingClientRect), as rustkit-engine's float_margin_tests do.
// Writes docs/diagnostics/2026-11-19/block_replaced_margin_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/block_replaced_margin_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-19/block_replaced_margin_cft148.json');
// A 10x10 red PNG.
const PNG = 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAoAAAAKCAIAAAACUFjqAAAAEklEQVR4nGP4z8CAB+GTG8HSALfKY52fTcuYAAAAAElFTkSuQmCC';
const A = '<div id=a style="height:20px;margin-bottom:30px"></div>';
const N = '<div id=n style="height:10px"></div>';

const CASES = [
  ['img block between margined divs', `${A}<img id=e style="display:block;margin:12px 0" src="${PNG}">${N}`],
  ['img block h0 between margined divs', `${A}<img id=e style="display:block;margin:12px 0;height:0" src="${PNG}">${N}`],
  ['img block larger top margin', `${A}<img id=e style="display:block;margin:40px 0 5px" src="${PNG}"><div id=n style="height:10px;margin-top:20px"></div>`],
  ['img block negative top margin', `${A}<img id=e style="display:block;margin-top:-10px" src="${PNG}">${N}`],
  ['img block first child of a margined parent', `<div id=p style="margin-top:7px"><img id=e style="display:block;margin-top:12px" src="${PNG}"></div>${N}`],
  ['img block last child, parent bottom margin', `<div id=p style="margin-bottom:7px"><img id=e style="display:block;margin-bottom:12px" src="${PNG}"></div>${N}`],
  ['two block imgs', `<img id=e style="display:block;margin-bottom:15px" src="${PNG}"><img id=f style="display:block;margin-top:25px" src="${PNG}">`],
  ['svg block between margined divs', `${A}<svg id=e style="display:block;margin:12px 0" width=10 height=10></svg>${N}`],
  ['inline img (unchanged)', `${A}<div id=w><img id=e style="margin:12px 0" src="${PNG}"></div>${N}`],
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
  await page.setContent(`<!doctype html><style>body{margin:0}</style>${body}`);
  await page.evaluate(() => Promise.all(Array.from(document.images, (i) => i.decode().catch(() => {}))));
  const want = await page.evaluate(() =>
    Array.prototype.map.call(document.querySelectorAll('[id]'), (e) => {
      const r = e.getBoundingClientRect();
      return e.id + '=' + r.y + '/' + r.height;
    }).join(' '));
  console.log(`${label}: ${want}`);
  cases.push({ label, body, want });
}
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
