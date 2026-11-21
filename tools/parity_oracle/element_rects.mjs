/**
 * element_rects.mjs - Chrome element rects for any page, in layout-rects.json shape
 *
 *   node tools/parity_oracle/element_rects.mjs <url|html-file> <out.json> [width height settleMs] [--png out.png]
 *
 * The committed baselines carry layout-rects.json for fixtures only. This
 * writes the same file for any URL, so tools/element_diff/element_diff.py can
 * join it against `parity-capture --url ... --dump-layout`.
 *
 * The join key is not reimplemented here: `getSelector` is read verbatim out
 * of capture_baseline.mjs with the extractor verify_selector_key.mjs pins, so
 * a selector from this script is byte-identical to a committed baseline one.
 *
 * Page setup matches the Chrome side the output will be compared with:
 *   - an html file gets capture_baseline.mjs's deterministic context (and
 *     parity reset where that script applies it), so a fixture reproduces
 *     its committed layout-rects.json;
 *   - a URL gets realsite.mjs's context (light scheme, no freeze, no reset),
 *     so it matches the real-site board's Chrome screenshot.
 *
 * Each element carries the baseline fields (selector, tag, rect, client,
 * scroll) plus `id` and `className`. Prints one JSON object on stdout.
 */

import { chromium } from 'playwright';
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'fs';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';
import {
  createDeterministicContext,
  getDeterministicLaunchOptions,
  shouldApplyParityResetForHtmlPath,
} from './deterministic.mjs';
import { extractGetSelectorSource } from './verify_selector_key.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const NAV_TIMEOUT_MS = 30000;
const SKIPPED_TAGS = ['script', 'style', 'meta', 'link', 'head', 'title', 'html'];

// Runs in the page. `fnSrc` is getSelector's source; the filters mirror
// capture_baseline.mjs so the element list is the same one a baseline holds.
function collectRects([fnSrc, skipped]) {
  // eslint-disable-next-line no-new-func
  const getSelector = new Function(`${fnSrc}; return getSelector;`)();
  const out = [];
  for (const el of document.querySelectorAll('*')) {
    const rect = el.getBoundingClientRect();
    if (rect.width === 0 && rect.height === 0) continue;
    const tag = el.tagName.toLowerCase();
    if (skipped.includes(tag)) continue;
    // SVG elements expose className as an SVGAnimatedString.
    const cls = typeof el.className === 'string' ? el.className : el.getAttribute('class');
    out.push({
      selector: getSelector(el),
      tag,
      id: el.id || null,
      className: cls || null,
      rect: {
        x: rect.x, y: rect.y, width: rect.width, height: rect.height,
        top: rect.top, right: rect.right, bottom: rect.bottom, left: rect.left,
      },
      client: { width: el.clientWidth, height: el.clientHeight },
      scroll: {
        width: el.scrollWidth, height: el.scrollHeight,
        top: el.scrollTop, left: el.scrollLeft,
      },
    });
  }
  return out;
}

async function main() {
  const argv = process.argv.slice(2);
  let pngPath = null;
  const pngAt = argv.indexOf('--png');
  if (pngAt >= 0) {
    pngPath = argv[pngAt + 1];
    argv.splice(pngAt, 2);
  }
  const [target, outPath, w = '1280', h = '800', settle = '1500'] = argv;
  if (!target || !outPath) {
    console.error('usage: element_rects.mjs <url|html-file> <out.json> [width height settleMs] [--png out.png]');
    process.exit(2);
  }
  const width = Number(w);
  const height = Number(h);
  const isUrl = /^[a-z][a-z0-9+.-]*:\/\//i.test(target);

  const fnSrc = extractGetSelectorSource(
    readFileSync(join(__dirname, 'capture_baseline.mjs'), 'utf8')
  );

  const result = { target, status: 'ok', error: null, nav_error: null };
  const browser = await chromium.launch(getDeterministicLaunchOptions());
  try {
    let context;
    let url;
    if (isUrl) {
      url = target;
      context = await browser.newContext({
        viewport: { width, height },
        deviceScaleFactor: 1,
        colorScheme: 'light',
        locale: 'en-US',
        extraHTTPHeaders: { 'Sec-CH-Prefers-Color-Scheme': 'light' },
      });
    } else {
      const abs = resolve(target);
      if (!existsSync(abs)) throw new Error(`HTML file not found: ${abs}`);
      url = `file://${abs}`;
      context = await createDeterministicContext(browser, width, height, {
        applyParityReset: shouldApplyParityResetForHtmlPath(abs),
      });
    }
    const page = await context.newPage();
    try {
      await page.goto(url, {
        waitUntil: isUrl ? 'load' : 'networkidle',
        timeout: NAV_TIMEOUT_MS,
      });
    } catch (e) {
      result.nav_error = String(e.message || e).split('\n')[0];
    }
    await page.waitForTimeout(isUrl ? Number(settle) : 50);
    if (pngPath) {
      mkdirSync(dirname(resolve(pngPath)), { recursive: true });
      await page.screenshot({ path: pngPath, type: 'png', fullPage: false });
      result.png_path = pngPath;
    }
    const elements = await page.evaluate(collectRects, [fnSrc, SKIPPED_TAGS]);
    mkdirSync(dirname(resolve(outPath)), { recursive: true });
    writeFileSync(outPath, JSON.stringify({
      timestamp: new Date().toISOString(),
      source: isUrl ? url : target,
      viewport: { width, height },
      browser_version: browser.version(),
      elementCount: elements.length,
      elements,
    }, null, 2));
    result.rects_path = outPath;
    result.element_count = elements.length;
    await context.close();
  } catch (e) {
    result.status = 'error';
    result.error = String(e.message || e).split('\n')[0];
  } finally {
    await browser.close();
  }
  console.log(JSON.stringify(result));
  if (result.status !== 'ok') process.exit(1);
}

await main();
