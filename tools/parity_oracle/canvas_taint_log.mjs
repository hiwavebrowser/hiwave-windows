// Canvas origin and size checks (STR-7): what Chrome does when a canvas
// reads back pixels after drawing images of other origins, and when
// getImageData asks for a huge rect. Two local servers (two ports, so two
// origins) serve a 2x2 PNG; the page is served by the first, which also
// redirects /to-other to the second's image and /to-same to its own. Writes
// canvas_taint_cases.json: per case, the name of the exception thrown
// (or "ok") and its message.
//
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node canvas_taint_log.mjs [--write]
import { createServer } from 'node:http';
import { writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const PNG = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAEUlEQVR4nGP4z8AAQv8ZYAwAQ84H+VjtZqAAAAAASUVORK5CYII=', 'base64');
function serve(page) {
  const s = createServer((req, res) => {
    const u = new URL(req.url, 'http://x');
    // Redirects (page server): to the other origin, or to its own image.
    if (u.pathname === '/to-other') { res.writeHead(302, { location: B + '/img.png', 'cache-control': 'no-store' }); res.end(); return; }
    if (u.pathname === '/to-same') { res.writeHead(302, { location: '/img.png', 'cache-control': 'no-store' }); res.end(); return; }
    if (u.pathname === '/img.png') {
      const h = { 'content-type': 'image/png', 'cache-control': 'no-store' };
      if (u.searchParams.get('cors')) h['access-control-allow-origin'] = '*';
      res.writeHead(200, h); res.end(PNG); return;
    }
    res.writeHead(200, { 'content-type': 'text/html' }); res.end(page || '');
  });
  return new Promise(r => s.listen(0, '127.0.0.1', () => r(s)));
}
let B = '';
const other = await serve();
B = 'http://127.0.0.1:' + other.address().port;
// Each case: how the image is made, then what is read.
const cases = [
  ['same-origin-img', "img('/img.png', null, function (i) { x.drawImage(i, 0, 0); read(); })"],
  ['cross-origin-img', "img(B + '/img.png', null, function (i) { x.drawImage(i, 0, 0); read(); })"],
  ['data-url-img', "img('data:image/png;base64," + PNG.toString('base64') + "', null, function (i) { x.drawImage(i, 0, 0); read(); })"],
  ['cors-anonymous-img', "img(B + '/img.png?cors=1', 'anonymous', function (i) { x.drawImage(i, 0, 0); read(); })"],
  ['cross-origin-pattern', "img(B + '/img.png', null, function (i) { x.fillStyle = x.createPattern(i, 'repeat'); x.fillRect(0, 0, 4, 4); read(); })"],
  ['tainted-canvas-drawn', "img(B + '/img.png', null, function (i) { var o = document.createElement('canvas'); o.width = o.height = 4; o.getContext('2d').drawImage(i, 0, 0); x.drawImage(o, 0, 0); read(); })"],
  ['tainted-canvas-pattern', "img(B + '/img.png', null, function (i) { var o = document.createElement('canvas'); o.width = o.height = 4; o.getContext('2d').drawImage(i, 0, 0); x.fillStyle = x.createPattern(o, 'repeat'); x.fillRect(0, 0, 4, 4); read(); })"],
  ['cross-origin-pattern-unused', "img(B + '/img.png', null, function (i) { x.createPattern(i, 'repeat'); read(); })"],
  ['redirect-same-to-other-origin', "img('/to-other', null, function (i) { x.drawImage(i, 0, 0); read(); })"],
  ['redirect-same-to-same-origin', "img('/to-same', null, function (i) { x.drawImage(i, 0, 0); read(); })"],
  ['huge-get-image-data', "try { x.getImageData(0, 0, 100000, 100000); done('ok', ''); } catch (e) { done(e.name, e.message); }"],
];
const page = "<!doctype html><body><canvas id=c width=8 height=8></canvas><script>var B = '" + B + "', x = document.getElementById('c').getContext('2d');" +
  "function done(n, m) { window.__r = { name: n, message: m }; }" +
  "function read() { try { x.getImageData(0, 0, 1, 1); done('ok', ''); } catch (e) { done(e.name, e.message); } }" +
  "function img(src, co, f) { var i = new Image(); if (co) i.crossOrigin = co; i.onload = function () { f(i); }; i.onerror = function () { done('load-error', ''); }; i.src = src; }" +
  "window.run = function (k) { eval(k); };</script></body>";
const main = await serve(page);
const A = 'http://127.0.0.1:' + main.address().port;
const browser = await chromium.launch({ ...getDeterministicLaunchOptions(), headless: true });
console.log('chrome ' + browser.version());
const out = { chrome: browser.version(), cases: [] };
for (const [name, script] of cases) {
  const p = await browser.newPage();
  await p.goto(A + '/');
  await p.evaluate(s => window.run(s), script);
  await p.waitForFunction(() => window.__r);
  const r = await p.evaluate(() => window.__r);
  console.log(name.padEnd(30) + r.name + '  ' + r.message);
  out.cases.push({ name, script, chrome: r });
  await p.close();
}
await browser.close(); main.close(); other.close();
if (process.argv.includes('--write')) writeFileSync(new URL('./canvas_taint_cases.json', import.meta.url), JSON.stringify(out, null, 1) + '\n');
