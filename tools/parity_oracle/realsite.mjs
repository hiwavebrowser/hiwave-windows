/**
 * realsite.mjs - Chrome side of the real-site board (scripts/realsite_board.py)
 *
 *   node realsite.mjs chrome <url> <out.png> <out-text.json> [width height settleMs]
 *     Load a live URL in pinned Chrome (PARITY_CHROME_PATH) with the oracle's
 *     deterministic launch options, let it settle, screenshot the first
 *     viewport and record the text Chrome shows in that viewport.
 *
 *   node realsite.mjs diff <chrome.png> <other.png|.ppm> [diff.png]
 *     Pixel diff with the campaign's pixelmatch settings (compare_pixels.mjs).
 *
 * Both print one JSON object on stdout.
 *
 * Deliberately NOT applied for live sites: parity-freeze.js and the parity
 * reset. Those normalise fixture pages; on a real site freezing Date/timers
 * changes what the page does, and the board's oracle is "what Chrome shows".
 */

import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';
import { comparePixels } from './compare_pixels.mjs';

const NAV_TIMEOUT_MS = 30000;

// Text nodes whose client rects intersect the first viewport and whose
// element is actually visible (visibility, opacity, display, content-visibility).
function collectViewportText() {
  const W = window.innerWidth;
  const H = window.innerHeight;
  const out = [];
  const root = document.body || document.documentElement;
  if (!root) return '';
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  const range = document.createRange();
  let n;
  while ((n = walker.nextNode())) {
    const t = n.nodeValue;
    if (!t || !t.trim()) continue;
    const el = n.parentElement;
    if (!el || el.closest('script,style,noscript,template')) continue;
    if (!el.checkVisibility({ opacityProperty: true, visibilityProperty: true })) continue;
    range.selectNodeContents(n);
    for (const r of range.getClientRects()) {
      if (r.width > 0 && r.height > 0 && r.bottom > 0 && r.right > 0 && r.top < H && r.left < W) {
        out.push(t);
        break;
      }
    }
  }
  return out.join(' ');
}

async function captureChrome(url, pngPath, textPath, width, height, settleMs) {
  const started = Date.now();
  const browser = await chromium.launch(getDeterministicLaunchOptions());
  const result = { url, status: 'ok', error: null, nav_error: null };
  try {
    // colorScheme is pinned: left unset, headless Chrome followed the seat's
    // OS appearance and served Google's dark theme, which RustKit never gets.
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      colorScheme: 'light',
      locale: 'en-US',
      // The media-query emulation alone is not enough: Google themes from
      // the client hint header, which still followed the OS (one of two
      // launches in a run came back dark).
      extraHTTPHeaders: { 'Sec-CH-Prefers-Color-Scheme': 'light' },
    });
    const page = await context.newPage();
    try {
      await page.goto(url, { waitUntil: 'load', timeout: NAV_TIMEOUT_MS });
    } catch (e) {
      // Heavy sites can miss the load event; keep what has painted.
      result.nav_error = String(e.message || e).split('\n')[0];
    }
    await page.waitForTimeout(settleMs);
    await page.screenshot({ path: pngPath, fullPage: false });
    const text = await page.evaluate(collectViewportText);
    result.final_url = page.url();
    result.title = await page.title();
    result.browser_version = browser.version();
    writeFileSync(textPath, JSON.stringify({ url, text }, null, 0));
    result.text_path = textPath;
    result.png_path = pngPath;
  } catch (e) {
    result.status = 'error';
    result.error = String(e.message || e).split('\n')[0];
  } finally {
    await browser.close();
  }
  result.elapsed_ms = Date.now() - started;
  return result;
}

async function main() {
  const [mode, ...rest] = process.argv.slice(2);
  let result;
  if (mode === 'chrome') {
    const [url, png, textJson, w = '1280', h = '800', settle = '5000'] = rest;
    result = await captureChrome(url, png, textJson, Number(w), Number(h), Number(settle));
  } else if (mode === 'diff') {
    const [a, b, diffPath] = rest;
    result = await comparePixels(a, b, diffPath || null);
  } else {
    console.error('usage: realsite.mjs chrome|diff ...');
    process.exit(2);
  }
  console.log(JSON.stringify(result));
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
