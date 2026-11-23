// Chrome's line rects for docs/diagnostics/2026-11-21/wrapped_inline_align.html (YT-CARD-SUBTITLE):
// `window.report()` at 800x600, written beside the page as wrapped_inline_align_cft148.txt
// with --write. Usage:
//   PARITY_CHROME_PATH=<Chrome for Testing 148.0.7778.216> node wrapped_inline_align_log.mjs [--write]
import { writeFileSync } from 'node:fs';
import { chromium } from 'playwright';

const WANT = '148.0.7778.216';
if (!process.env.PARITY_CHROME_PATH) {
  console.error('PARITY_CHROME_PATH must name Chrome for Testing ' + WANT);
  process.exit(2);
}
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH });
if (browser.version() !== WANT) {
  console.error('expected Chrome ' + WANT + ', got ' + browser.version());
  await browser.close();
  process.exit(2);
}
const PAGE = new URL('../../docs/diagnostics/2026-11-21/wrapped_inline_align.html', import.meta.url);
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.goto(PAGE.href);
const report = await page.evaluate(() => window.report());
console.log(report);
if (process.argv.includes('--write')) {
  const out = new URL('../../docs/diagnostics/2026-11-21/wrapped_inline_align_cft148.txt', import.meta.url);
  writeFileSync(out, '# Chrome for Testing ' + browser.version() + ', 800x600, window.report() of wrapped_inline_align.html\n' + report + '\n');
}
await browser.close();
