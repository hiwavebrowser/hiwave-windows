// What does each script in event_target_ctor_cases.json evaluate to?
//
// The pinned Chromium runs every case's `js` as a classic script on an empty
// page and its completion value, as a string, is the answer. The engine's
// `new EventTarget()` is tested against these answers
// (crates/rustkit-bindings, event_target_ctor_tests).
//
//   node event_target_ctor_log.mjs            print
//   node event_target_ctor_log.mjs --write    print, and store in the case file
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const FILE = new URL('./event_target_ctor_cases.json', import.meta.url);
const data = JSON.parse(readFileSync(FILE, 'utf8'));
const browser = await chromium.launch(getDeterministicLaunchOptions());
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage();
  await page.setContent('<!DOCTYPE html><html><head><title>T</title></head><body></body></html>');
  // Indirect eval: the case runs as a script at global scope, as the
  // engine's evaluate does.
  c.chrome = await page.evaluate((js) => String((0, eval)(js)), c.js);
  console.log(c.name + '\n   ' + c.chrome);
  await page.close();
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
