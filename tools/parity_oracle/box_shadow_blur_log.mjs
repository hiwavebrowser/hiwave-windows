// SHADOW-BLUR: Chrome for Testing 148's pixels for blurred box shadows, as lines of RGB samples through each
// shadow, for crates/rustkit-engine/src/box_shadow_blur_tests.rs (which builds the same pages from the same
// CSS). Each case is a 400x300 page, body margin 0, one 120x80 box at (140, 110) carrying the declaration.
// Lines: row 150 (through the box's middle, x 0..399), column 200 (y 0..299), and the diagonal out of the
// box's top-left corner, (140 - k, 110 - k) for k 0..109.
//   PARITY_CHROME_PATH=<Chrome for Testing 148> node box_shadow_blur_log.mjs
// Output: box_shadow_blur_cases.json beside this script. Fails unless PARITY_CHROME_PATH is set.
import { chromium } from 'playwright';
import { PNG } from 'pngjs';
import { writeFileSync } from 'fs';
import { getPinnedExecutablePath } from './deterministic.mjs';

const W = 400, H = 300;
// [name, body background, box CSS]. Boxes are white unless the case says otherwise.
export const CASES = [
  ['blur4', '#fff', 'background:#fff;box-shadow:0 0 4px rgba(0,0,0,.6)'],
  ['blur16', '#fff', 'background:#fff;box-shadow:0 0 16px rgba(0,0,0,.6)'],
  ['blur32', '#fff', 'background:#fff;box-shadow:0 0 32px rgba(0,0,0,.6)'],
  ['blur64', '#fff', 'background:#fff;box-shadow:0 0 64px rgba(0,0,0,.6)'],
  ['blur16_spread8', '#fff', 'background:#fff;box-shadow:0 0 16px 8px rgba(0,0,0,.6)'],
  ['blur32_spread_neg8', '#fff', 'background:#fff;box-shadow:0 0 32px -8px rgba(0,0,0,.6)'],
  ['offset', '#fff', 'background:#fff;box-shadow:10px 20px 16px rgba(0,0,0,.6)'],
  ['round', '#fff', 'background:#fff;border-radius:24px;box-shadow:0 0 32px rgba(0,0,0,.6)'],
  ['pill_list', '#fff', 'background:#fff;border-radius:40px;box-shadow:0 4px 16px #00000014,0 0 32px #0000001f'],
  ['list_first_on_top', '#fff', 'background:#fff;box-shadow:0 0 8px rgba(255,0,0,.8),0 0 24px rgba(0,0,255,.8)'],
  ['on_colour', '#3b82f6', 'background:#fff;box-shadow:0 0 16px rgba(0,0,0,.6)'],
  ['colour_on_colour', '#fde68a', 'background:#fff;box-shadow:0 0 24px 4px rgba(220,38,38,.7)'],
  ['transparent_box', '#fff', 'box-shadow:0 0 16px rgba(0,0,0,.6)'],
  ['inset16', '#fff', 'background:#fff;box-shadow:inset 0 0 16px rgba(0,0,0,.6)'],
  ['inset_offset_round', '#fff', 'background:#fff;border-radius:16px;box-shadow:inset 6px 8px 12px 2px rgba(0,0,0,.6)'],
  ['inset_ring', '#fff', 'background:#fff;box-shadow:inset 0 0 0 3px #2563eb'],
  // R1: a bordered box's inset shadow is inside the padding box, under inner radii.
  ['inset_bordered', '#fff', 'background:#fff;border:6px solid #94a3b8;border-radius:20px;box-shadow:inset 0 3px 10px 1px rgba(0,0,0,.6)'],
];

export const page = (bg, css) =>
  `<!DOCTYPE html><html><head><style>html,body{margin:0}body{background:${bg}}` +
  `#b{position:absolute;left:140px;top:110px;width:120px;height:80px;${css}}</style></head>` +
  `<body><div id="b"></div></body></html>`;

const browser = await chromium.launch({ executablePath: getPinnedExecutablePath({ required: true }), headless: true });
const ctx = await browser.newContext({ viewport: { width: W, height: H }, deviceScaleFactor: 1 });
const p = await ctx.newPage();
const out = { chrome: browser.version(), width: W, height: H, box: [140, 110, 120, 80], cases: [] };
for (const [name, bg, css] of CASES) {
  await p.setContent(page(bg, css));
  const png = PNG.sync.read(await p.screenshot());
  const px = (x, y) => { const i = (y * W + x) * 4; return [png.data[i], png.data[i + 1], png.data[i + 2]]; };
  const row = [], column = [], diagonal = [];
  for (let x = 0; x < W; x++) row.push(px(x, 150));
  for (let y = 0; y < H; y++) column.push(px(200, y));
  for (let k = 0; k < 110; k++) diagonal.push(px(140 - k, 110 - k));
  out.cases.push({ name, background: bg, css, row_y: 150, row, column_x: 200, column, diagonal });
  console.error(name, JSON.stringify(row.slice(40, 140).map((c) => c[0])));
}
await browser.close();
writeFileSync(new URL('./box_shadow_blur_cases.json', import.meta.url), JSON.stringify(out) + '\n');
