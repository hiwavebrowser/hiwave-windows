// MEDIA-AVF Milestone 1 step 3 (docs/design/MEDIA_AVF.md §3.3, pins P-fit and the first
// frame): what a paused <video> paints once its first frame is in, in the pinned Chrome.
// Each case is one <video src=color-steps.mp4 preload=auto> on a white page, at (0, 0),
// never played: Chrome shows frame 0 (encoded (255, 0, 0)). After `loadeddata` and two
// animation frames, the page is screenshotted and the pixels at fixed points are read:
// across the box's middle row (the letterbox edges of `object-fit: contain`), and its
// top and bottom rows. The first frame's own centre pixel (drawImage onto a canvas) is
// recorded too: the colour the decoder gives frame 0.
// Writes docs/diagnostics/2026-11-19/media_frame_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/media_frame_log.mjs
import { chromium } from 'playwright';
import { PNG } from 'pngjs';
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '../..');
const RECEIPT = join(ROOT, 'docs/diagnostics/2026-11-19/media_frame_cft148.json');
const FIXTURE = readFileSync(join(ROOT, 'fixtures/media/color-steps.mp4'));

// [label, the video's style]. Every box but `natural` is 200x100 (the 64x48 video is
// 4:3, so `contain` letterboxes it to 133.3x100 at x 33.3..166.7).
const CASES = [
  ['natural', ''],
  ['contain', 'width:200px;height:100px'],
  ['cover', 'width:200px;height:100px;object-fit:cover'],
  ['fill', 'width:200px;height:100px;object-fit:fill'],
  ['contain-left', 'width:200px;height:100px;object-position:0 50%'],
];
// The points read, in page pixels: the middle row across, then the top and bottom
// rows at the centre.
const ROW = [2, 30, 36, 100, 164, 170, 197].map((x) => [x, 50]);
const POINTS = { natural: [[32, 24], [2, 2], [61, 45]], default: [...ROW, [100, 2], [100, 97]] };

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);

const cases = [];
for (const [label, style] of CASES) {
  const page = await browser.newPage({ viewport: { width: 300, height: 200 }, deviceScaleFactor: 1 });
  await page.route('http://media.test/**', async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === '/') {
      return route.fulfill({
        contentType: 'text/html',
        body: `<!doctype html><style>html,body{margin:0;background:#fff}video{display:block}</style>` +
          `<video id=m preload=auto src="/color-steps.mp4" style="${style}"></video>`,
      });
    }
    const range = route.request().headers()['range'];
    if (range) {
      const [, a, b] = /bytes=(\d+)-(\d*)/.exec(range);
      const start = Number(a);
      const end = b ? Math.min(Number(b), FIXTURE.length - 1) : FIXTURE.length - 1;
      return route.fulfill({
        status: 206,
        headers: {
          'content-type': 'video/mp4',
          'accept-ranges': 'bytes',
          'content-range': `bytes ${start}-${end}/${FIXTURE.length}`,
        },
        body: FIXTURE.subarray(start, end + 1),
      });
    }
    return route.fulfill({ contentType: 'video/mp4', headers: { 'accept-ranges': 'bytes' }, body: FIXTURE });
  });
  await page.goto('http://media.test/');
  const state = await page.evaluate(async () => {
    const v = document.getElementById('m');
    if (v.readyState < 2) {
      await new Promise((r) => {
        v.addEventListener('loadeddata', r, { once: true });
        v.addEventListener('error', r, { once: true });
        setTimeout(r, 5000);
      });
    }
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const c = document.createElement('canvas');
    c.width = 64;
    c.height = 48;
    const ctx = c.getContext('2d');
    ctx.drawImage(v, 0, 0);
    const r = v.getBoundingClientRect();
    return {
      readyState: v.readyState,
      paused: v.paused,
      currentTime: v.currentTime,
      box: [r.x, r.y, r.width, r.height],
      frame0_centre: Array.from(ctx.getImageData(32, 24, 1, 1).data.slice(0, 3)),
    };
  });
  await page.waitForTimeout(200);
  const png = PNG.sync.read(await page.screenshot({ clip: { x: 0, y: 0, width: 300, height: 200 } }));
  const points = (POINTS[label] || POINTS.default).map(([x, y]) => {
    const i = (y * png.width + x) * 4;
    return { x, y, rgb: [png.data[i], png.data[i + 1], png.data[i + 2]] };
  });
  cases.push({ label, style, ...state, points });
  console.log(label, JSON.stringify(state), JSON.stringify(points.map((p) => `${p.x},${p.y}:${p.rgb}`)));
  await page.close();
}
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(
  RECEIPT,
  JSON.stringify({ chrome: PIN, fixture: 'fixtures/media/color-steps.mp4', frame0_encoded: [255, 0, 0], cases }, null, 2) + '\n',
);
console.log('receipt ' + RECEIPT);
