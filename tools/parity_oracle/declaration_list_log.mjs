#!/usr/bin/env node
/**
 * declaration_list_log.mjs - Chromium's answers for the declaration-list
 * parsing cases (Z plan CH2.E27 style attribute, CH2.E28 empty declarations).
 *
 *   PARITY_CHROME_PATH=<pinned Chrome for Testing> node declaration_list_log.mjs <cases.json> [--write]
 *
 * Each case is a whole page (`html`) and one script expression (`expr`) read
 * after load. With --write it stores Chromium's answer in `chrome` and the
 * browser version in `oracle`; without, it prints every case whose stored
 * answer differs and exits 1. Headless always (agent-briefs, 11-06).
 */
import { chromium } from 'playwright';
import { readFileSync, writeFileSync } from 'fs';

const path = process.argv[2];
const write = process.argv.includes('--write');
const doc = JSON.parse(readFileSync(path, 'utf8'));
const browser = await chromium.launch({ headless: true, executablePath: process.env.PARITY_CHROME_PATH || undefined });
const page = await browser.newPage({ viewport: { width: 400, height: 300 } });
let bad = 0;
for (const c of doc.cases) {
  await page.setContent(c.html, { waitUntil: 'load' });
  const read = String(await page.evaluate(c.expr));
  if (write) c.chrome = read;
  else if (c.chrome !== read) { bad++; console.log(c.name, JSON.stringify(c.chrome), JSON.stringify(read)); }
}
const version = browser.version();
console.log('chromium', version);
await browser.close();
if (write) {
  doc.oracle = version;
  writeFileSync(path, JSON.stringify(doc, null, 1) + '\n');
} else if (version !== doc.oracle) {
  console.log('wrong oracle, wanted', doc.oracle);
  process.exit(1);
}
process.exit(bad ? 1 : 0);
