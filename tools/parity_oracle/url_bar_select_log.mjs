// The address bar must replace its text when the user clicks into it and
// types (Handtest, #730 smoke 2026-10-22: typing appended to the URL).
// Loads the shipped chrome UI (crates/hiwave-app/src/ui/chrome.html) in the
// Playwright's WebKit (default) or the oracle Chromium with the IPC stubbed, shows a URL in the bar the way the
// app does (renderTabs), then clicks into the bar with a real mouse press
// and types. Prints one line per case; exits 1 if any case fails.
// The app draws this UI in a WKWebView, so WebKit is the engine that matters:
// WebKit undoes a select() made in a focus handler on the mouseup of the
// click that focused the field (Chromium does not).
// usage: node tools/parity_oracle/url_bar_select_log.mjs crates/hiwave-app/src/ui/chrome.html [webkit|chromium]
import { chromium, webkit } from 'playwright';
import { resolve } from 'path';
import { pathToFileURL } from 'url';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const URL = 'https://example.com/some/long/path';
const engine = process.argv[3] || 'webkit';
const browser = engine === 'chromium'
  ? await chromium.launch(getDeterministicLaunchOptions())
  : await webkit.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 800 } });
page.on('pageerror', (e) => console.log('pageerror ' + e.message));
await page.addInitScript(() => {
  window.ipc = { postMessage: () => {} };
});
// goto (not setContent) so the init script's IPC stub is in place first.
await page.goto(pathToFileURL(resolve(process.argv[2])).href);
await page.waitForFunction(() => window.hiwaveChrome && document.getElementById('urlInput'));

const show = (url) => page.evaluate((u) => {
  document.getElementById('urlInput').blur();
  window.hiwaveChrome.renderTabs([{ id: '1', title: 't', url: u, active: true }]);
}, url);
const box = async () => (await page.locator('#urlInput').boundingBox());
let failed = 0;
const check = (label, got, want) => {
  const ok = got === want;
  failed += ok ? 0 : 1;
  console.log(`${ok ? 'PASS' : 'FAIL'} ${label}: value=${JSON.stringify(got)} want=${JSON.stringify(want)}`);
};

console.log(engine + ' ' + browser.version());

// 1. One click near the right end of the bar, then type: replaces.
await show(URL);
let b = await box();
await page.mouse.click(b.x + b.width - 20, b.y + b.height / 2);
await page.keyboard.type('abc');
check('click at the end, type', await page.inputValue('#urlInput'), 'abc');

// 2. One click near the left of the bar, then type: replaces.
await show(URL);
b = await box();
await page.mouse.click(b.x + 30, b.y + b.height / 2);
await page.keyboard.type('xyz');
check('click at the start, type', await page.inputValue('#urlInput'), 'xyz');

// 3. A second click in the already-focused bar places the caret (no select-all).
await show(URL);
b = await box();
await page.mouse.click(b.x + b.width - 20, b.y + b.height / 2);
await page.mouse.click(b.x + b.width - 20, b.y + b.height / 2);
await page.keyboard.press('End');
await page.keyboard.type('Q');
check('second click keeps the text', await page.inputValue('#urlInput'), URL + 'Q');

// 4. A drag on the first press selects only what was dragged over.
await show(URL);
b = await box();
await page.mouse.move(b.x + 60, b.y + b.height / 2);
await page.mouse.down();
await page.mouse.move(b.x + b.width - 5, b.y + b.height / 2, { steps: 8 });
await page.mouse.up();
const sel = await page.evaluate(() => {
  const i = document.getElementById('urlInput');
  return [i.selectionStart, i.selectionEnd];
});
// Chromium keeps the focus handler's select-all through a drag, with or
// without the fix; the app's engine is WebKit, so only WebKit is gated here.
if (engine === 'chromium') console.log(`NOTE drag (not gated on chromium): selection ${sel}`);
else check('drag selects a range, not forced to all', String(sel[0] > 0 && sel[1] > sel[0]), 'true');

await browser.close();
process.exit(failed ? 1 : 0);
