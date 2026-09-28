/**
 * probe_reset.mjs — session-7 diagnostic: does the parity-reset init script
 * actually land in the DOM of a micro fixture, and what does h1 compute to?
 * Usage: node probe_reset.mjs <html-path>
 */
import { chromium } from 'playwright';
import {
  createDeterministicContext,
  getDeterministicLaunchOptions,
  shouldApplyParityResetForHtmlPath,
} from './deterministic.mjs';
import { resolve } from 'path';

const htmlPath = resolve(process.argv[2]);
const browser = await chromium.launch(getDeterministicLaunchOptions());
const ctx = await createDeterministicContext(browser, 800, 400, {
  applyParityReset: shouldApplyParityResetForHtmlPath(htmlPath),
});
const page = await ctx.newPage();
page.on('pageerror', (e) => console.log('PAGEERROR:', e.message));
await page.goto(`file://${htmlPath}`, { waitUntil: 'networkidle' });
await page.waitForTimeout(50);
const probe = await page.evaluate(() => {
  const s = document.querySelector('style[data-parity-reset]');
  const h1 = document.querySelector('h1');
  const cs = h1 ? getComputedStyle(h1) : null;
  return {
    resetPresent: !!s,
    resetParent: s ? s.parentElement.tagName : null,
    resetIndexInParent: s ? Array.from(s.parentElement.children).indexOf(s) : null,
    h1LineHeight: cs && cs.lineHeight,
    h1FontSize: cs && cs.fontSize,
    h1FontFamily: cs && cs.fontFamily,
    bodyMargin: getComputedStyle(document.body).marginTop,
    h1Rect: h1 && h1.getBoundingClientRect().toJSON(),
  };
});
console.log(JSON.stringify(probe, null, 2));
await browser.close();
