// What the pinned Chrome reports for getComputedStyle of border-radius
// (the shorthand and the four corners) on a 200x100 box with 16px text
// in an 800x600 viewport: percentages, calc() mixes, em/rem/vw, and
// elliptical corners. Writes
// docs/diagnostics/2026-11-17/border_radius_computed_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/border_radius_computed_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/border_radius_computed_cft148.json');

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) {
  throw new Error(`expected Chrome for Testing ${PIN}, got ${browser.version()}`);
}
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
console.log('chrome ' + browser.version());
const cases = [];
const PROPS = ['border-radius', 'border-top-left-radius', 'border-top-right-radius',
  'border-bottom-right-radius', 'border-bottom-left-radius'];

async function probe(label, css) {
  await page.setContent('<!doctype html><style>html{font-size:16px}body{margin:0}' +
    '#b{width:200px;height:100px;font:16px sans-serif;' + css + '}</style><div id=b></div>');
  const out = await page.evaluate((props) => {
    const cs = getComputedStyle(document.getElementById('b'));
    return Object.fromEntries(props.map((p) => [p, cs.getPropertyValue(p)]));
  }, PROPS);
  console.log(`${label}: ${PROPS.map((p) => out[p]).join(' | ')}`);
  cases.push({ label, css, ...out });
}

await probe('px', 'border-radius:10px');
await probe('percent', 'border-radius:50%');
await probe('percent elliptical', 'border-radius:10% / 20%');
await probe('mixed corners', 'border-radius:10px 50% 2em 1rem');
await probe('em and rem', 'border-radius:2em / 1.5rem');
await probe('calc mix', 'border-radius:calc(10px + 5%)');
await probe('calc minus', 'border-top-left-radius:calc(50% - 10px)');
await probe('calc with em', 'border-top-left-radius:calc(1em + 10%)');
await probe('calc single unit', 'border-top-left-radius:calc(2 * 10px)');
await probe('corner longhand two values', 'border-top-left-radius:10px 20%');
await probe('percent over 50', 'border-radius:80%');
await probe('vw', 'border-top-left-radius:10vw');

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, viewport: [800, 600], cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
