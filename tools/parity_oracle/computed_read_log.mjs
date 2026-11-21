#!/usr/bin/env node
/**
 * computed_read_log.mjs - Chromium's getComputedStyle answers for the engine's
 * computed-read tests (Z plan C9 item 0).
 *
 *   node computed_read_log.mjs [--write]
 *
 * Reads computed_read_cases.json. Each case is one declaration on one element;
 * cases with an empty `value` read an unstyled element, Chromium's initial value.
 * `inline` declares it in a style attribute. `gap` names a case the engine does
 * not answer yet, and why: the test requires it to stay wrong, so the list
 * cannot go stale.
 * With --write it stores Chromium's answer in `chrome`; without, it prints
 * any case whose stored answer differs and exits 1.
 *
 * Plain Playwright context on purpose: deterministic.mjs freezes transitions and
 * animations, so the census's oracle reads `animation-duration: 2s` as `0s`.
 * The page is the one the Rust test builds (computed_read_tests.rs).
 */
import { chromium } from 'playwright';
import { readFileSync, writeFileSync } from 'fs';

const url = new URL('./computed_read_cases.json', import.meta.url);
const doc = JSON.parse(readFileSync(url, 'utf8'));
const write = process.argv.includes('--write');

function pageOf(cases) {
  const decl = (c) => `${c.base || ''};${c.value ? `${c.prop}:${c.value}` : ''}`.replace(/^;/, '');
  const rules = cases.map((c, i) => c.inline ? '' : `#c${i}{${decl(c)}}`).join('\n');
  const divs = cases.map((c, i) =>
    `<div id=c${i} class=box${c.inline ? ` style="${decl(c)}"` : ''}>x</div>`).join('');
  return `<!doctype html><style>body{margin:0}.box{width:100px;height:50px}\n${rules}</style><body>${divs}</body>`;
}

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 400, height: 300 } });
let bad = 0;
for (const family of Object.keys(doc.families)) {
  const cases = doc.families[family];
  await page.setContent(pageOf(cases), { waitUntil: 'load' });
  const reads = await page.evaluate((props) => props.map((p, i) =>
    getComputedStyle(document.getElementById('c' + i)).getPropertyValue(p)), cases.map((c) => c.prop));
  reads.forEach((r, i) => {
    if (write) cases[i].chrome = r;
    else if (cases[i].chrome !== r) { bad++; console.log(family, cases[i].prop, cases[i].value, JSON.stringify(cases[i].chrome), JSON.stringify(r)); }
  });
}
const version = browser.version();
console.log('chromium', version);
await browser.close();
if (version !== doc.oracle) { console.log('wrong oracle, wanted', doc.oracle); process.exit(1); }
if (write) writeFileSync(url, JSON.stringify(doc, null, 1) + '\n');
process.exit(bad ? 1 : 0);
