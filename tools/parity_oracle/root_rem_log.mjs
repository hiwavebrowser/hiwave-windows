// Chrome's answers for docs/diagnostics/2026-11-21/root_rem_{pct,px,em,rem}.html (ROOT-REM):
// window.report() after load and window.report2() after script sets the root to 20px, at
// 800x600, written beside the pages as root_rem_cft148.txt with --write. Usage:
//   PARITY_CHROME_PATH=<Chrome for Testing 148.0.7778.216> node root_rem_log.mjs [--write]
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
const out = ['# Chrome for Testing ' + browser.version() + ', 800x600, root_rem_<case>.html: [case] report() then [case after] report2()'];
for (const k of ['pct', 'px', 'em', 'rem']) {
  const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
  await page.goto(new URL('../../docs/diagnostics/2026-11-21/root_rem_' + k + '.html', import.meta.url).href);
  out.push('[' + k + ']', await page.evaluate(() => window.report()));
  out.push('[' + k + ' after]', await page.evaluate(() => window.report2()));
  await page.close();
}
console.log(out.join('\n'));
if (process.argv.includes('--write')) {
  writeFileSync(new URL('../../docs/diagnostics/2026-11-21/root_rem_cft148.txt', import.meta.url), out.join('\n') + '\n');
}
await browser.close();
