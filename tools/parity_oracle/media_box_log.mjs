// MEDIA-AVF Milestone 1 step 1 (docs/design/MEDIA_AVF.md §3.8, pins P-size and P-ua):
// what the pinned Chrome lays out for <video> and <audio> elements with no media loaded.
// For each case: the element's border box (getBoundingClientRect), and its computed
// display and object-fit. No source is set, so nothing here depends on a decoder.
// Writes docs/diagnostics/2026-11-18/media_box_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/media_box_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-18/media_box_cft148.json');

// (label, markup of #m); every case sits after a 10x10 inline-block so its x shows that
// it is inline-level.
const CASES = [
  ['video', '<video id=m></video>'],
  ['video with fallback text', '<video id=m>fallback text that is never shown</video>'],
  ['video width=100 height=50', '<video id=m width=100 height=50></video>'],
  ['video width=100', '<video id=m width=100></video>'],
  ['video height=60', '<video id=m height=60></video>'],
  ['video css width:120px', '<video id=m style="width:120px"></video>'],
  ['video controls', '<video id=m controls></video>'],
  ['video display:block', '<video id=m style="display:block"></video>'],
  ['video border 5px padding 3px', '<video id=m style="border:5px solid;padding:3px"></video>'],
  ['audio', '<audio id=m></audio>'],
  ['audio controls', '<audio id=m controls></audio>'],
];

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
const cases = [];
for (const [label, markup] of CASES) {
  const html =
    '<!doctype html><style>body{margin:0;font:16px sans-serif}#pre{display:inline-block;width:10px;height:10px}</style>' +
    '<div><span id=pre></span>' + markup + '</div>';
  await page.setContent(html);
  const got = await page.evaluate(() => {
    const m = document.getElementById('m');
    const r = m.getBoundingClientRect();
    const cs = getComputedStyle(m);
    return { x: r.x, y: r.y, width: r.width, height: r.height, display: cs.display, object_fit: cs.objectFit };
  });
  console.log(`${label}: ${JSON.stringify(got)}`);
  cases.push({ label, html, ...got });
}
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
