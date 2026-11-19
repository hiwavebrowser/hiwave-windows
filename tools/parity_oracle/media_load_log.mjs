// MEDIA-AVF Milestone 1 step 2 (docs/design/MEDIA_AVF.md §3.8; pins P-events, P-meta,
// P-error, and the box after metadata): what the pinned Chrome reports while a media
// element loads, without playing. Each case's page is served over http with ranged
// responses; its element #m is in the markup. Recorded: every media event with
// readyState / networkState at the event (in order), then, once `canplaythrough`,
// `error` or 3 s: the element's state, its ranges, and its border box.
// Writes docs/diagnostics/2026-11-18/media_load_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/media_load_log.mjs
import { chromium } from 'playwright';
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '../..');
const RECEIPT = join(ROOT, 'docs/diagnostics/2026-11-18/media_load_cft148.json');
const FILES = {
  '/color-steps.mp4': ['video/mp4', readFileSync(join(ROOT, 'fixtures/media/color-steps.mp4'))],
  '/size-32x24.mp4': ['video/mp4', readFileSync(join(ROOT, 'fixtures/media/size-32x24.mp4'))],
  '/not-media.mp4': ['video/mp4', Buffer.from('this is not a media file '.repeat(40))],
};

const EVENTS = [
  'loadstart', 'progress', 'suspend', 'abort', 'error', 'emptied', 'stalled', 'loadedmetadata',
  'loadeddata', 'canplay', 'canplaythrough', 'durationchange', 'resize', 'waiting',
];

// (label, markup with #m)
const CASES = [
  ['video src preload=auto', '<video id=m src=/color-steps.mp4 preload=auto></video>'],
  ['video src default preload', '<video id=m src=/color-steps.mp4></video>'],
  ['video src preload=none', '<video id=m src=/color-steps.mp4 preload=none></video>'],
  ['video width=100 after metadata', '<video id=m width=100 src=/color-steps.mp4 preload=auto></video>'],
  ['video 32x24 after metadata', '<video id=m src=/size-32x24.mp4 preload=auto></video>'],
  ['video source selection', '<video id=m preload=auto><source src=/a.mp4 type="video/x-unknown"><source src=/color-steps.mp4 type="video/mp4"></video>'],
  ['video 404', '<video id=m src=/missing.mp4 preload=auto></video>'],
  ['video not media', '<video id=m src=/not-media.mp4 preload=auto></video>'],
  ['audio src preload=auto', '<audio id=m src=/color-steps.mp4 preload=auto controls></audio>'],
];

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
const cases = [];
for (const [label, markup] of CASES) {
  const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
  const html =
    '<!doctype html><style>body{margin:0}</style><script>window.__ev=[];' +
    `document.addEventListener('DOMContentLoaded',()=>{});` +
    `${JSON.stringify(EVENTS)}.forEach(t=>document.addEventListener(t,e=>{if(e.target.id==='m')` +
    `__ev.push(t+':rs'+e.target.readyState+':ns'+e.target.networkState)},true));</script>` + markup;
  await page.route('http://media.test/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === '/') return route.fulfill({ contentType: 'text/html', body: html });
    const file = FILES[path];
    if (!file) return route.fulfill({ status: 404, contentType: 'text/plain', body: 'not found' });
    const [type, body] = file;
    const range = route.request().headers()['range'];
    if (!range) return route.fulfill({ contentType: type, headers: { 'accept-ranges': 'bytes' }, body });
    const [, a, b] = /bytes=(\d+)-(\d*)/.exec(range);
    const start = Number(a);
    const end = b ? Math.min(Number(b), body.length - 1) : body.length - 1;
    return route.fulfill({
      status: 206,
      headers: { 'content-type': type, 'accept-ranges': 'bytes', 'content-range': `bytes ${start}-${end}/${body.length}` },
      body: body.subarray(start, end + 1),
    });
  });
  await page.goto('http://media.test/');
  const got = await page.evaluate(async () => {
    const m = document.getElementById('m');
    await new Promise((resolve) => {
      if (m.readyState === 4 || m.error) return resolve();
      m.addEventListener('canplaythrough', resolve, { once: true });
      m.addEventListener('error', resolve, { once: true });
      setTimeout(resolve, 3000);
    });
    // Let the events of the same task land.
    await new Promise((r) => setTimeout(r, 200));
    const ranges = (t) => Array.from({ length: t.length }, (_, i) => [t.start(i), t.end(i)]);
    const r = m.getBoundingClientRect();
    return {
      events: window.__ev.slice(),
      readyState: m.readyState,
      networkState: m.networkState,
      error: m.error ? m.error.code : null,
      currentSrc: m.currentSrc.replace(location.origin, ''),
      duration: Number.isNaN(m.duration) ? 'NaN' : m.duration,
      videoWidth: m.videoWidth === undefined ? null : m.videoWidth,
      videoHeight: m.videoHeight === undefined ? null : m.videoHeight,
      paused: m.paused,
      ended: m.ended,
      seeking: m.seeking,
      currentTime: m.currentTime,
      buffered: ranges(m.buffered),
      seekable: ranges(m.seekable),
      preload: m.preload,
      width: r.width,
      height: r.height,
    };
  });
  console.log(`${label}: ${JSON.stringify(got)}`);
  cases.push({ label, markup, ...got });
  await page.close();
}
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, events_watched: EVENTS, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
