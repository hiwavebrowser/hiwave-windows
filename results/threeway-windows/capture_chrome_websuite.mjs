// Windows column: imports and page URLs as file:/// URLs (drive-letter paths); otherwise the Linux column's script.
// Experiment: the parity reset injected once <html> exists (deterministic.mjs's init script calls
// document.documentElement.appendChild before <html> exists, throws, and never applies it).
// argv: <list.json> [{html, out, width, height}]. Micro pages get the reset; others don't.
import { readFileSync, mkdirSync } from 'fs';
import { join } from 'path';
const oracle = process.env.ORACLE_DIR;
const { chromium } = await import(`file:///${oracle}/node_modules/playwright/index.mjs`);
const det = await import(`file:///${oracle}/deterministic.mjs`);
const RESET = readFileSync(process.env.RESET_CSS_PATH, 'utf8');
const browser = await chromium.launch(det.getDeterministicLaunchOptions({ requirePinned: true }));
for (const job of JSON.parse(readFileSync(process.argv[2], 'utf8'))) {
  try {
    const ctx = await det.createDeterministicContext(browser, job.width, job.height, { applyParityReset: false });
    if (det.shouldApplyParityResetForHtmlPath(job.html)) {
      await ctx.addInitScript({ content: `(() => {
        const css = ${JSON.stringify(RESET)};
        const put = () => { const s = document.createElement('style'); s.setAttribute('data-parity-reset','1'); s.textContent = css;
          (document.head || document.documentElement).insertBefore(s, (document.head || document.documentElement).firstChild); };
        if (document.documentElement) put(); else new MutationObserver((m, o) => { if (document.documentElement) { o.disconnect(); put(); } }).observe(document, { childList: true });
      })();` });
    }
    const page = await ctx.newPage();
    await page.goto(`file:///${job.html}`, { waitUntil: 'networkidle' });
    await page.waitForTimeout(50);
    mkdirSync(job.out, { recursive: true });
    await page.screenshot({ path: join(job.out, 'baseline.png'), type: 'png', fullPage: false });
    await ctx.close();
    console.log('ok', job.out);
  } catch (e) { console.log('FAIL', job.out, e.message); }
}
await browser.close();
