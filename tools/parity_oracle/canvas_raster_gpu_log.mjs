// GPU canvas A3 bounds (GPU-AA): how far Chrome's own GPU canvas is from
// its software canvas on every raster pin (canvas_raster_cases.json and
// canvas_raster2_cases.json, whose chrome_rgba is Chrome's software
// raster). The same scripts on a fresh 40x40 canvas, in a page that has
// drawn a frame, read back with getImageData, with Chrome launched with
// Metal (ANGLE) and GPU rasterization on instead of the pins' software
// launch. Metric as the pins (rustkit-renderer canvas_gpu_chrome_tests):
// largest premultiplied channel difference, mean over the software
// image's painted pixels, pixels over 2.
//
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node canvas_raster_gpu_log.mjs [--write]
//
// --write records the numbers in canvas_raster_gpu_bounds.json (its "cpu"
// entries, the CPU bitmap's distance, are written by hand from the
// renderer test's output and kept).
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getPinnedExecutablePath } from './deterministic.mjs';

const FILES = ['canvas_raster_cases.json', 'canvas_raster2_cases.json'];
const OUT = new URL('./canvas_raster_gpu_bounds.json', import.meta.url);
const ARGS = [
  '--force-color-profile=srgb',
  '--disable-lcd-text',
  '--disable-font-subpixel-positioning',
  '--use-angle=metal',
  '--enable-gpu-rasterization',
  '--enable-accelerated-2d-canvas',
  '--ignore-gpu-blocklist',
];

const unhex = (h) => Uint8Array.from(h.match(/../g).map((b) => parseInt(b, 16)));
const pm = (p, i) => {
  const a = p[i + 3];
  return [0, 1, 2].map((k) => Math.floor((p[i + k] * a + 127) / 255)).concat([a]);
};
function distance(ours, chrome) {
  let max = 0, sum = 0, painted = 0, over = 0;
  for (let i = 0; i < chrome.length; i += 4) {
    if (chrome[i + 3] !== 0) painted++;
    const a = pm(ours, i), b = pm(chrome, i);
    const d = Math.max(...a.map((v, k) => Math.abs(v - b[k])));
    max = Math.max(max, d);
    sum += d;
    if (d > 2) over++;
  }
  return { max, mean: Math.round((sum / Math.max(painted, 1)) * 100) / 100, over2: over };
}

const browser = await chromium.launch({ headless: true, executablePath: getPinnedExecutablePath({ required: true }), args: ARGS });
console.log('chrome ' + browser.version());
const page = await browser.newPage({ viewport: { width: 200, height: 200 }, deviceScaleFactor: 1 });
await page.setContent('<!doctype html><body></body>');
const cases = {};
let differing = 0;
for (const f of FILES) {
  const data = JSON.parse(readFileSync(new URL('./' + f, import.meta.url), 'utf8'));
  const [w, h] = data.size;
  for (const c of data.cases) {
    const hex = await page.evaluate(async ([script, w, h]) => {
      const cv = document.createElement('canvas');
      cv.width = w; cv.height = h;
      document.body.appendChild(cv);
      const x = cv.getContext('2d');
      new Function('x', script)(x);
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
      const d = x.getImageData(0, 0, w, h).data;
      let s = '';
      for (let i = 0; i < d.length; i++) s += (d[i] < 16 ? '0' : '') + d[i].toString(16);
      cv.remove();
      return s;
    }, [c.script, w, h]);
    const d = distance(unhex(hex), unhex(c.chrome_rgba));
    if (d.max > 0) differing++;
    cases[c.name] = d;
    console.log(`${c.name}  max ${d.max} mean ${d.mean} >2 ${d.over2}`);
  }
}
// The software launch reproduces the pins byte for byte; a GPU launch that
// took no GPU raster would too. Some cases must differ.
if (differing === 0) throw new Error('no case differs from the software pins: the GPU canvas was not used');
console.log(`${differing} cases differ from the software raster`);
if (process.argv.includes('--write')) {
  const out = existsSync(OUT) ? JSON.parse(readFileSync(OUT, 'utf8')) : { cases: {} };
  out.chrome = browser.version();
  out.chrome_gpu_args = ARGS;
  for (const [name, d] of Object.entries(cases)) {
    out.cases[name] = { ...(out.cases[name] || {}), chrome_gpu: d };
  }
  writeFileSync(OUT, JSON.stringify(out, null, 1) + '\n');
}
await browser.close();
