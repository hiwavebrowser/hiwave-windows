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
import { writeFileSync, readFileSync } from 'fs';
import { join as pathJoin } from 'path';
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

async function runActionsChrome(url, actionsArg, outDir, width, height, settleMs) {
  const started = Date.now();
  let actions = [];
  try {
    const raw = String(actionsArg).trim();
    if (raw.startsWith('[') || raw.startsWith('{')) {
      const parsed = JSON.parse(raw);
      actions = Array.isArray(parsed) ? parsed : (parsed.actions || []);
    } else {
      const content = readFileSync(actionsArg, 'utf8');
      const parsed = JSON.parse(content);
      actions = Array.isArray(parsed) ? parsed : (parsed.actions || []);
    }
  } catch (e) {
    return { status: 'error', error: `failed to parse actions: ${e.message}` };
  }

  const browser = await chromium.launch(getDeterministicLaunchOptions());
  const result = { url, status: 'ok', error: null, captures: [], action_results: [] };
  try {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      colorScheme: 'light',
      locale: 'en-US',
      extraHTTPHeaders: { 'Sec-CH-Prefers-Color-Scheme': 'light' },
    });
    const page = await context.newPage();
    try {
      await page.goto(url, { waitUntil: 'load', timeout: NAV_TIMEOUT_MS });
    } catch (e) {
      result.nav_error = String(e.message || e).split('\n')[0];
    }
    await page.waitForTimeout(settleMs);

    for (let i = 0; i < actions.length; i++) {
      const a = actions[i];
      const aStart = Date.now();
      const aRes = { step: a.step ?? i, type: a.type, status: 'ok' };
      try {
        if (a.type === 'wait') {
          if (a.selector) {
            await page.waitForSelector(a.selector, { timeout: a.timeout_ms || a.ms || 2000 });
          } else {
            await page.waitForTimeout(a.ms || 500);
          }
        } else if (a.type === 'click') {
          if (a.selector) {
            const selectors = a.selector.split(',').map((s) => s.trim());
            let clicked = false;
            for (const s of selectors) {
              try {
                const el = await page.$(s);
                if (el) {
                  await el.click({ timeout: a.timeout_ms || 2000 });
                  clicked = true;
                  aRes.selector_used = s;
                  break;
                }
              } catch (_) {}
            }
            if (!clicked) {
              aRes.status = 'selector_not_found';
            }
          } else if (a.x != null && a.y != null) {
            await page.mouse.click(Number(a.x), Number(a.y));
          }
        } else if (a.type === 'key') {
          if (a.selector) {
            try { await page.focus(a.selector); } catch (_) {}
          }
          if (a.text) {
            await page.keyboard.type(String(a.text));
          } else if (a.key) {
            await page.keyboard.press(String(a.key));
          }
        } else if (a.type === 'resize') {
          await page.setViewportSize({ width: Number(a.width), height: Number(a.height) });
        } else if (a.type === 'capture') {
          const framePath = outDir ? pathJoin(outDir, a.frame) : a.frame;
          try {
            await page.screenshot({ path: framePath, fullPage: false });
          } catch (_) {
            await page.waitForTimeout(500);
            await page.screenshot({ path: framePath, fullPage: false });
          }
          aRes.frame = framePath;
          aRes.label = a.label || `step_${i}`;
          result.captures.push({ step: aRes.step, label: aRes.label, frame: framePath });
        }
      } catch (err) {
        aRes.status = 'error';
        aRes.error = String(err.message || err).split('\n')[0];
      }
      aRes.elapsed_ms = Date.now() - aStart;
      result.action_results.push(aRes);
    }
  } catch (e) {
    result.status = 'error';
    result.error = String(e.message || e).split('\n')[0];
  } finally {
    await browser.close();
  }
  result.elapsed_ms = Date.now() - started;
  return result;
}

async function recordHarChrome(url, harPath, width, height, settleMs) {
  const started = Date.now();
  const browser = await chromium.launch(getDeterministicLaunchOptions());
  const result = { url, status: 'ok', har_path: harPath, error: null };
  try {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      colorScheme: 'light',
      locale: 'en-US',
      extraHTTPHeaders: { 'Sec-CH-Prefers-Color-Scheme': 'light' },
      recordHar: {
        path: harPath,
        mode: 'minimal',
      },
    });
    const page = await context.newPage();
    try {
      await page.goto(url, { waitUntil: 'load', timeout: NAV_TIMEOUT_MS });
    } catch (e) {
      result.nav_error = String(e.message || e).split('\n')[0];
    }
    await page.waitForTimeout(settleMs);
    await context.close();
  } catch (e) {
    result.status = 'error';
    result.error = String(e.message || e).split('\n')[0];
  } finally {
    await browser.close();
  }
  result.elapsed_ms = Date.now() - started;
  return result;
}

async function runTimestableChrome(url, outDir, prefix, harPath, width, height) {
  const started = Date.now();
  const browser = await chromium.launch(getDeterministicLaunchOptions());
  const result = { url, status: 'ok', error: null, captures: [] };
  const milestones = [
    { label: '1s', targetMs: 1000 },
    { label: '3s', targetMs: 3000 },
    { label: '5s', targetMs: 5000 },
    { label: '10s', targetMs: 10000 },
  ];

  try {
    const context = await browser.newContext({
      viewport: { width, height },
      deviceScaleFactor: 1,
      colorScheme: 'light',
      locale: 'en-US',
      extraHTTPHeaders: { 'Sec-CH-Prefers-Color-Scheme': 'light' },
    });
    const page = await context.newPage();

    const isReplay = Boolean(harPath);
    if (isReplay) {
      await page.routeFromHAR(harPath, { notFound: 'abort' });
      // Pinned fixed epoch: 2026-10-01T00:00:00Z
      await page.clock.install({ time: 1727740800000 });
    }

    try {
      await page.goto(url, { waitUntil: 'load', timeout: NAV_TIMEOUT_MS });
    } catch (e) {
      result.nav_error = String(e.message || e).split('\n')[0];
    }

    let currentMs = 0;
    for (const m of milestones) {
      const delta = m.targetMs - currentMs;
      if (isReplay) {
        await page.clock.fastForward(delta);
      } else {
        await page.waitForTimeout(delta);
      }
      currentMs = m.targetMs;

      const framePath = pathJoin(outDir, `${prefix}_${m.label}.png`);
      await page.screenshot({ path: framePath, fullPage: false });
      result.captures.push({
        label: m.label,
        time_ms: m.targetMs,
        frame: framePath,
      });
    }
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
  } else if (mode === 'actions') {
    const [url, actionsArg, outDir = '.', w = '1280', h = '800', settle = '5000'] = rest;
    result = await runActionsChrome(url, actionsArg, outDir, Number(w), Number(h), Number(settle));
  } else if (mode === 'record-har') {
    const [url, harPath, w = '1280', h = '800', settle = '10000'] = rest;
    result = await recordHarChrome(url, harPath, Number(w), Number(h), Number(settle));
  } else if (mode === 'timestable') {
    const [url, outDir = '.', prefix = 'chrome', harPath = '', w = '1280', h = '800'] = rest;
    result = await runTimestableChrome(url, outDir, prefix, harPath || null, Number(w), Number(h));
  } else if (mode === 'diff') {
    const [a, b, diffPath] = rest;
    result = await comparePixels(a, b, diffPath || null);
  } else {
    console.error('usage: realsite.mjs chrome|actions|record-har|timestable|diff ...');
    process.exit(2);
  }
  console.log(JSON.stringify(result));
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
