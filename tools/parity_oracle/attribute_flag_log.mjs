#!/usr/bin/env node
/**
 * attribute_flag_log.mjs - Chromium's answers for the engine's attribute
 * selector case flag tests (CH2.E18, `[a="v" i]` / `[a="v" s]`).
 *
 *   node attribute_flag_log.mjs
 *
 * Loads attribute_flag_page.html (the page attribute_flag_tests.rs builds)
 * and prints, in document order, the text of every <nav> that is shown, then
 * what querySelectorAll / matches / closest give for flagged selectors.
 */
import { chromium } from 'playwright';
import { readFileSync } from 'fs';

const html = readFileSync(new URL('./attribute_flag_page.html', import.meta.url), 'utf8');
const browser = await chromium.launch();
const page = await browser.newPage();
await page.setContent(html);
const out = await page.evaluate(() => {
  const shown = [...document.querySelectorAll('nav')]
    .filter((n) => getComputedStyle(n).display !== 'none')
    .map((n) => n.textContent);
  const q = (s) => {
    try {
      return [...document.querySelectorAll(s)].map((n) => n.textContent).join(' ');
    } catch (e) {
      return 'throws ' + e.name;
    }
  };
  return {
    version: navigator.userAgent.match(/Chrome\/[\d.]+/)[0],
    shown: shown.join(' '),
    qsa_i: q('[data-a="FOO" i]'),
    qsa_s: q('[data-h="foo" s]'),
    qsa_unquoted: q('[data-b=Foo i]'),
    qsa_bad_flag: q('[data-r="foo" x]'),
    matches_i: document.querySelector('[data-k]').matches('[data-k="foo" i]'),
    closest_i: document.querySelector('[data-m]').firstElementChild.closest('[data-m="open" i]')?.tagName ?? null,
    // Not a flag case: an attribute on an ancestor compound in a query.
    qsa_ancestor_attr: document.querySelectorAll('[data-m] nav').length,
  };
});
console.log(JSON.stringify(out, null, 1));
await browser.close();
