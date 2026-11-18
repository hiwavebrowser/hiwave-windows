// Chrome's answers for the sort cases in sort_nan_cases.json (each "js" is an expression returning
// a string), written into the file's "chrome" field with --write. Usage:
//   node sort_cases_log.mjs [--write]
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
const FILE = new URL('./sort_nan_cases.json', import.meta.url);
const data = JSON.parse(readFileSync(FILE, 'utf8'));
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH });
console.log('chrome ' + browser.version());
const page = await browser.newPage();
await page.setContent('<html><body></body></html>');
for (const c of data.cases) {
  c.chrome = await page.evaluate(c.js);
  console.log(c.name + '  ' + c.chrome.slice(0, 150));
}
if (process.argv.includes('--write')) {
  data.chrome_version = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
