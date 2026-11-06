#!/usr/bin/env node
/**
 * chrome_board.mjs — the Chromium arm of the chrome board (CH3.A1).
 *
 *   node chrome_board.mjs <page.html> <out dir> <width> <height> [actions.json]
 *
 * Loads one composed chrome page (scripts/chrome_board.py writes it: the
 * page, its IPC stub before its script, its board fixture after) in the
 * oracle Chromium under the campaign's deterministic context, runs the
 * state's action list — the SAME list parity-capture runs with `--actions` —
 * then writes to <out dir>:
 *
 *   chrome.png            the frame
 *   layout-rects.json     every visible element's rect, keyed by the join
 *                         selector (extractPageGeometry, capture_baseline.mjs)
 *   computed-styles.json  the key computed properties, same key
 *   chrome.json           action results, page errors, console errors, the
 *                         IPC commands the page posted, the browser version
 *
 * Actions: wait {ms | selector}, click {selector | x,y}, hover {selector | x,y},
 * key {selector?, text | key, modifiers?}, resize {width, height},
 * capture {frame}. Unknown types are an error on the step, as in parity-capture.
 */
import { chromium } from 'playwright';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import { join, resolve } from 'path';
import {
  createDeterministicContext,
  getDeterministicLaunchOptions,
} from './deterministic.mjs';
import { extractPageGeometry } from './capture_baseline.mjs';

const SETTLE_MS = 100;

function readActions(path) {
  if (!path) return [];
  const parsed = JSON.parse(readFileSync(path, 'utf8'));
  return Array.isArray(parsed) ? parsed : parsed.actions || [];
}

// A `selector` may list several selectors to try in order. Split on
// top-level commas only, as parity-capture's split_selector_list does:
// `:is(a, b)` and `[title="x, y"]` stay whole.
function splitSelectorList(selector) {
  const out = [];
  let depth = 0;
  let quote = null;
  let current = '';
  for (const ch of String(selector)) {
    if (quote) {
      current += ch;
      if (ch === quote) quote = null;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      current += ch;
    } else if (ch === '(' || ch === '[') {
      depth += 1;
      current += ch;
    } else if (ch === ')' || ch === ']') {
      depth = Math.max(0, depth - 1);
      current += ch;
    } else if (ch === ',' && depth === 0) {
      out.push(current.trim());
      current = '';
    } else {
      current += ch;
    }
  }
  if (current.trim()) out.push(current.trim());
  return out;
}

function keyChord(a) {
  const mods = (a.modifiers || []).map((m) => {
    const name = String(m);
    const canonical = { control: 'Control', ctrl: 'Control', shift: 'Shift', alt: 'Alt', meta: 'Meta' }[name.toLowerCase()];
    return canonical || name;
  });
  return [...mods, String(a.key)].join('+');
}

async function runActions(page, actions, outDir) {
  const results = [];
  for (let i = 0; i < actions.length; i += 1) {
    const a = actions[i];
    const started = Date.now();
    const res = { step: a.step ?? i, type: a.type, status: 'ok' };
    try {
      if (a.type === 'wait') {
        if (a.selector) await page.waitForSelector(a.selector, { timeout: a.timeout_ms || a.ms || 2000 });
        else await page.waitForTimeout(a.ms || 500);
      } else if (a.type === 'click' || a.type === 'hover') {
        if (a.selector) {
          const candidates = splitSelectorList(a.selector);
          let done = false;
          for (const s of candidates) {
            const el = await page.$(s);
            if (!el) continue;
            // force: the deterministic context freezes requestAnimationFrame
            // (parity-freeze.js), and Playwright's actionability wait needs two
            // frames to call an element stable — it would time out. The pointer
            // still goes to the element's centre, as parity-capture's does.
            if (a.type === 'click') await el.click({ timeout: a.timeout_ms || 2000, force: true });
            else await el.hover({ timeout: a.timeout_ms || 2000, force: true });
            res.selector_used = s;
            done = true;
            break;
          }
          if (!done) {
            res.status = 'selector_not_found';
            res.error = `no matching element found for selector ${a.selector}`;
          }
        } else if (a.x != null && a.y != null) {
          if (a.type === 'click') await page.mouse.click(Number(a.x), Number(a.y));
          else await page.mouse.move(Number(a.x), Number(a.y));
        } else {
          res.status = 'error';
          res.error = `${a.type} action requires selector or (x, y)`;
        }
      } else if (a.type === 'key') {
        if (a.selector) await page.focus(a.selector);
        if (a.text != null) await page.keyboard.type(String(a.text));
        else if (a.key != null) await page.keyboard.press(keyChord(a));
        else {
          res.status = 'error';
          res.error = 'key action requires text or key';
        }
      } else if (a.type === 'resize') {
        await page.setViewportSize({ width: Number(a.width), height: Number(a.height) });
      } else if (a.type === 'capture') {
        const framePath = join(outDir, a.frame);
        await page.screenshot({ path: framePath, fullPage: false });
        res.frame = framePath;
        res.label = a.label || `step_${i}`;
      } else {
        res.status = 'error';
        res.error = `unknown action type: ${a.type}`;
      }
    } catch (err) {
      res.status = 'error';
      res.error = String(err.message || err).split('\n')[0];
    }
    res.elapsed_ms = Date.now() - started;
    results.push(res);
  }
  return results;
}

async function main() {
  const [htmlArg, outArg, widthArg, heightArg, actionsArg] = process.argv.slice(2);
  if (!htmlArg || !outArg || !widthArg || !heightArg) {
    console.error('usage: chrome_board.mjs <page.html> <out dir> <width> <height> [actions.json]');
    return 2;
  }
  const html = resolve(htmlArg);
  if (!existsSync(html)) {
    console.error(`page not found: ${html}`);
    return 2;
  }
  const outDir = resolve(outArg);
  mkdirSync(outDir, { recursive: true });
  const width = Number(widthArg);
  const height = Number(heightArg);
  const actions = readActions(actionsArg);

  const browser = await chromium.launch(getDeterministicLaunchOptions());
  const record = {
    status: 'ok',
    browser: browser.version(),
    viewport: { width, height },
    page_errors: [],
    console_errors: [],
    action_results: [],
    ipc_calls: [],
  };
  try {
    // The chrome pages are product UI, not micro-tests: no parity reset.
    const context = await createDeterministicContext(browser, width, height, { applyParityReset: false });
    const page = await context.newPage();
    page.on('pageerror', (e) => record.page_errors.push(String(e.message || e).split('\n')[0]));
    page.on('console', (m) => {
      if (m.type() === 'error') record.console_errors.push(m.text().slice(0, 200));
    });
    await page.goto(`file://${html}`, { waitUntil: 'load' });
    await page.waitForTimeout(SETTLE_MS);
    record.action_results = await runActions(page, actions, outDir);
    await page.waitForTimeout(SETTLE_MS);

    await page.screenshot({ path: join(outDir, 'chrome.png'), type: 'png', fullPage: false });
    const { styles, rects } = await extractPageGeometry(page);
    const stamp = { viewport: { width, height } };
    writeFileSync(join(outDir, 'layout-rects.json'), JSON.stringify({ ...stamp, elementCount: rects.length, elements: rects }));
    writeFileSync(join(outDir, 'computed-styles.json'), JSON.stringify({ ...stamp, elementCount: styles.length, elements: styles }));
    record.ipc_calls = await page.evaluate(() => (window.__hiwaveBoard ? window.__hiwaveBoard.calls() : []));
    record.fixture_errors = await page.evaluate(() => (window.__hiwaveBoard ? window.__hiwaveBoard.errors : []));
    await context.close();
  } catch (err) {
    record.status = 'error';
    record.error = String(err.message || err).split('\n')[0];
  } finally {
    await browser.close();
  }
  writeFileSync(join(outDir, 'chrome.json'), JSON.stringify(record, null, 2));
  console.log(JSON.stringify(record));
  return record.status === 'ok' ? 0 : 1;
}

process.exit(await main());
