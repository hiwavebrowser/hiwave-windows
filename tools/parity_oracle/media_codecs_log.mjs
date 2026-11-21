// MEDIA-AVF, the first oracle check (docs/design/MEDIA_AVF.md §5): does the pinned
// Chrome for Testing decode H.264 and AAC? It plays fixtures/media/color-steps.mp4
// (64x48 H.264 + AAC, frame n a solid colour) served over http, and records:
// canPlayType / MediaSource.isTypeSupported for the MIME strings of pin P-canplay, the
// element's events up to `canplaythrough`, its metadata, the decoded byte counts after a
// short muted play (audio and video separately), and the colour of the frame shown after
// a paused seek to 0.55 s (frame 5: (255, 0, 255)).
// Writes docs/diagnostics/2026-11-18/media_codecs_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/media_codecs_log.mjs
import { chromium } from 'playwright';
import { readFileSync, writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const ROOT = join(dirname(fileURLToPath(import.meta.url)), '../..');
const RECEIPT = join(ROOT, 'docs/diagnostics/2026-11-18/media_codecs_cft148.json');
const FIXTURE = readFileSync(join(ROOT, 'fixtures/media/color-steps.mp4'));

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
const page = await browser.newPage({ viewport: { width: 200, height: 200 } });
// Ranged responses, as a media server answers.
await page.route('http://media.test/**', async (route) => {
  const url = new URL(route.request().url());
  if (url.pathname === '/') {
    return route.fulfill({ contentType: 'text/html', body: '<!doctype html><body></body>' });
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

const TYPES = [
  'video/mp4',
  'video/mp4; codecs="avc1.42E01E"',
  'video/mp4; codecs="avc1.42E01E, mp4a.40.2"',
  'video/mp4; codecs="hvc1.1.6.L93.B0"',
  'video/mp4; codecs="av01.0.04M.08"',
  'video/webm',
  'video/webm; codecs="vp9"',
  'video/webm; codecs="vp9, opus"',
  'audio/mp4; codecs="mp4a.40.2"',
  'audio/mpeg',
  'application/vnd.apple.mpegurl',
  'video/x-unknown',
];

const out = await page.evaluate(async (types) => {
  const v = document.createElement('video');
  const can = {};
  for (const t of types) {
    can[t] = { canPlayType: v.canPlayType(t), isTypeSupported: window.MediaSource ? MediaSource.isTypeSupported(t) : null };
  }
  const events = [];
  for (const e of ['loadstart', 'durationchange', 'loadedmetadata', 'loadeddata', 'canplay', 'canplaythrough', 'error']) {
    v.addEventListener(e, () => events.push(e + ':rs' + v.readyState + ':ns' + v.networkState));
  }
  v.muted = true;
  v.preload = 'auto';
  v.src = '/color-steps.mp4';
  document.body.appendChild(v);
  await new Promise((resolve) => {
    v.addEventListener('canplaythrough', resolve, { once: true });
    v.addEventListener('error', resolve, { once: true });
    setTimeout(resolve, 5000);
  });
  const meta = {
    error: v.error && v.error.code,
    duration: v.duration,
    videoWidth: v.videoWidth,
    videoHeight: v.videoHeight,
    readyState: v.readyState,
  };
  let decoded = null;
  let frame = null;
  if (!v.error) {
    await v.play().catch((e) => (meta.play = String(e)));
    await new Promise((r) => setTimeout(r, 600));
    v.pause();
    decoded = { audio: v.webkitAudioDecodedByteCount, video: v.webkitVideoDecodedByteCount };
    v.currentTime = 0.55;
    await new Promise((r) => v.addEventListener('seeked', r, { once: true }));
    const c = document.createElement('canvas');
    c.width = 64;
    c.height = 48;
    const ctx = c.getContext('2d');
    ctx.drawImage(v, 0, 0);
    frame = Array.from(ctx.getImageData(32, 24, 1, 1).data).join(',');
  }
  return { can, events, meta, decoded, frame_at_055_centre: frame };
}, TYPES);

const h264 = out.meta.error == null && out.decoded && out.decoded.video > 0;
const aac = out.meta.error == null && out.decoded && out.decoded.audio > 0;
console.log(`chrome ${browser.version()}: H.264 ${h264 ? 'decodes' : 'does NOT decode'}, AAC ${aac ? 'decodes' : 'does NOT decode'}`);
console.log(JSON.stringify(out, null, 1));
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, fixture: 'fixtures/media/color-steps.mp4', h264, aac, ...out }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
