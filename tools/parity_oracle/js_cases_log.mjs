// Chrome's answer for each case in a JSON file whose "cases" are {name, js} (js is an expression
// returning a string, evaluated as global code), written into each case's "chrome" with --write.
//   node js_cases_log.mjs <cases.json> [--write]      PARITY_CHROME_PATH=<Chrome for Testing>
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
const file = process.argv[2];
const data = JSON.parse(readFileSync(file, 'utf8'));
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH });
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage();
  await page.setContent('<html><body></body></html>');
  c.chrome = await page.evaluate((js) => Promise.resolve((0, eval)(js)).then(String), c.js);
  await page.close();
  console.log(c.name + '  ' + c.chrome.slice(0, 120));
}
if (process.argv.includes('--write')) {
  data.chrome_version = browser.version();
  writeFileSync(file, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
