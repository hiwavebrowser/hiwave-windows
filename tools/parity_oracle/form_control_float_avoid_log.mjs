// CSS 2.1 §9.5 (row FORM-CONTROL-FLOAT-AVOID): a block-level form control (input, select,
// textarea, button with display:block) and a fieldset do not overlap the margin box of a float
// in the same formatting context, like a block-level replaced element (#1356): placed beside the
// float when it fits in the width the float leaves, else moved down until it fits. An auto-width
// fieldset takes the width the float leaves. What the pinned Chrome lays out: each case's
// elements with an id report `id=x,y,width,height` (getBoundingClientRect), in a 400px-wide
// container. Sizes are explicit (border-box) so font metrics do not enter.
// Writes docs/diagnostics/2026-11-20/form_control_float_avoid_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/form_control_float_avoid_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-20/form_control_float_avoid_cft148.json');
const L = '<div id=f style="float:left;width:100px;height:80px"></div>';
const R = '<div id=f style="float:right;width:100px;height:80px"></div>';
const N = '<div id=n style="height:10px"></div>';
const BOX = 'display:block;box-sizing:border-box;margin:0;padding:0;border:1px solid';
const CTRL = {
  input: (s) => `<input id=e style="${BOX};${s}">`,
  select: (s) => `<select id=e style="${BOX};${s}"></select>`,
  textarea: (s) => `<textarea id=e style="${BOX};resize:none;${s}"></textarea>`,
  button: (s) => `<button id=e style="${BOX};${s}"></button>`,
};
const FS = (s) => `<fieldset id=e style="margin:0;padding:0;border:1px solid;min-inline-size:0;${s}"></fieldset>`;

const CASES = [];
for (const [tag, make] of Object.entries(CTRL)) {
  CASES.push([`${tag} beside a left float`, `${L}${make('width:150px;height:40px')}${N}`]);
  CASES.push([`${tag} beside a right float`, `${R}${make('width:150px;height:40px')}${N}`]);
  CASES.push([`${tag} too wide moves below the float`, `${L}${make('width:350px;height:40px')}${N}`]);
}
CASES.push(
  ['input exactly fits', `${L}${CTRL.input('width:300px;height:40px')}`],
  ['input with a left margin beside a left float', `${L}${CTRL.input('width:150px;height:40px;margin-left:20px')}`],
  ['button with a left margin past a left float', `${L}${CTRL.button('width:150px;height:40px;margin-left:150px')}`],
  ['textarea with a right margin beside a right float', `${R}${CTRL.textarea('width:150px;height:40px;margin-right:20px')}`],
  ['select margin 0 auto beside a left float', `${L}${CTRL.select('width:150px;height:40px;margin:0 auto')}`],
  ['input margin-left:auto beside a left float', `${L}${CTRL.input('width:150px;height:40px;margin-left:auto')}`],
  ['button negative margin-left moved below a left float', `${L}${CTRL.button('width:350px;height:40px;margin-left:-20px')}`],
  ['input with margin-top after a margined div, beside a float', `<div id=a style="height:20px;margin-bottom:30px"></div>${L}${CTRL.input('width:150px;height:40px;margin-top:12px')}${N}`],
  ['fieldset auto width beside a left float', `${L}${FS('height:40px')}${N}`],
  ['fieldset auto width beside a right float', `${R}${FS('height:40px')}${N}`],
  ['fieldset too wide moves below the float', `${L}${FS('width:350px;height:40px')}${N}`],
  ['fieldset with margin-left beside a left float', `${L}${FS('height:40px;margin-left:20px')}`],
  ['fieldset with margin-left past a left float', `${L}${FS('height:40px;margin-left:150px')}`],
  ['fieldset with UA style beside a left float', `${L}<fieldset id=e style="height:40px"></fieldset>`],
  ['inline-block input beside a float (control)', `${L}<input id=e style="box-sizing:border-box;margin:0;padding:0;border:1px solid;width:150px;height:40px;vertical-align:top">`],
  ['plain block still overlaps the float (control)', `${L}<div id=e style="height:40px"></div>`],
);

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
