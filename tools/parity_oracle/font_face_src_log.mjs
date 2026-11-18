// Which font URLs Chrome requests for an `@font-face` `src`, per case in font_face_src_cases.json
// ({name, src} or {name, css} for a whole rule body), written into each case's "chrome" with --write
// as the requested paths in request order ("" when it asks for none).
// The page uses the family, and the server answers every font path with a real font of the type
// its extension names (Ahem as .ttf/.otf/.woff/.woff2; anything else gets bytes that are no font),
// so a second request means Chrome fell through after a failed decode.
//   node font_face_src_log.mjs [--write]      PARITY_CHROME_PATH=<Chrome for Testing>
import { readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { chromium } from 'playwright';
const file = new URL('./font_face_src_cases.json', import.meta.url);
const fixture = (name) => readFileSync(new URL('../../crates/rustkit-text/tests/fixtures/' + name, import.meta.url));
const fonts = { ttf: fixture('Ahem.ttf'), otf: fixture('Ahem.ttf'), woff: fixture('Ahem.woff'), woff2: fixture('Ahem.woff2') };
const data = JSON.parse(readFileSync(file, 'utf8'));
let page_html = '', seen = [];
const server = createServer((req, res) => {
  const path = req.url.split('?')[0];
  if (path === '/') { res.writeHead(200, { 'Content-Type': 'text/html' }); res.end(page_html); return; }
  if (path === '/favicon.ico') { res.writeHead(404); res.end(); return; }
  seen.push(req.url);
  const body = fonts[path.split('.').pop()] || Buffer.from('not a font');
  res.writeHead(200, { 'Content-Type': 'application/octet-stream', 'Cache-Control': 'no-store' });
  res.end(body);
});
await new Promise((r) => server.listen(0, '127.0.0.1', r));
const origin = 'http://127.0.0.1:' + server.address().port;
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH, headless: true });
console.log('chrome ' + browser.version());
export const rule = (c) => c.css || 'font-family: T; src: ' + c.src + ';';
for (const c of data.cases) {
  seen = [];
  page_html = '<!doctype html><html><head><style>@font-face { ' + rule(c) + ' } p { font-family: T; }</style></head><body><p>text</p></body></html>';
  const page = await browser.newPage();
  await page.goto(origin + '/', { waitUntil: 'networkidle' });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(150);
  await page.close();
  c.chrome = seen.join(' ');
  console.log(c.name + '  [' + c.chrome + ']');
}
if (process.argv.includes('--write')) {
  data.chrome_version = browser.version();
  writeFileSync(file, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
server.close();
