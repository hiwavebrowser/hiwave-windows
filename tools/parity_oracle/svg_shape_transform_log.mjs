// Row SVG-SHAPE-TRANSFORM: the rect of an inline <svg>'s shape under a CSS transform on the <svg>
// (or an ancestor). getBoundingClientRect() on the shape is post-transform; the inspector's
// expanded DOM toggle (`.dom-toggle.expanded svg { transform: rotate(90deg) }`, path
// `M9 6l6 6-6 6` in a 24 viewBox at 10px) is the board's case. Each case's page is the
// rustkit-engine test svg_shape_visual_rect_tests (same markup).
// Writes docs/diagnostics/2026-11-21/svg_shape_transform_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/svg_shape_transform_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-21/svg_shape_transform_cft148.json');

// [label, the svg's style, the wrapper's style]
const CASES = [
  ['rotate(90deg) on the svg', 'transform:rotate(90deg)', ''],
  ['no transform', '', ''],
  ['translate(5px, 7px) on the svg', 'transform:translate(5px,7px)', ''],
  ['rotate(90deg) on the wrapper', '', 'transform:rotate(90deg)'],
  // #1415 R1 nit 1: an origin other than the svg's centre, and the composition order.
  ['rotate(90deg) on the wrapper about its top left', '', 'transform:rotate(90deg);transform-origin:0 0'],
  ['translate(5px, 7px) on the wrapper, rotate(90deg) on the svg', 'transform:rotate(90deg)', 'transform:translate(5px,7px)'],
  ['rotate(90deg) on the wrapper about its top left, translate(5px, 0) on the svg', 'transform:translate(5px,0)', 'transform:rotate(90deg);transform-origin:0 0'],
];

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
const cases = [];
try {
  if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 400, height: 200 } });
  for (const [label, svgStyle, wrapStyle] of CASES) {
    await page.setContent('<!doctype html><style>body{margin:0}</style>' +
      `<div id=w style="position:absolute;left:20px;top:30px;width:16px;height:16px;${wrapStyle}">` +
      `<svg id=s viewBox="0 0 24 24" style="display:block;margin:3px;width:10px;height:10px;${svgStyle}">` +
      '<path id=p d="M9 6l6 6-6 6"/></svg></div>');
    const got = await page.evaluate(() => {
      const r = (id) => { const b = document.getElementById(id).getBoundingClientRect(); return [b.x, b.y, b.width, b.height]; };
      return { svg: r('s'), path: r('p') };
    });
    console.log(label, JSON.stringify(got));
    cases.push({ label, svg_style: svgStyle, wrapper_style: wrapStyle, ...got });
  }
} finally {
  await browser.close();
}
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, platform: process.platform, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
