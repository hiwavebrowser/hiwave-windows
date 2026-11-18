// JS-ROBUST: what Chrome for Testing 148 does on the second set of string-building paths past the maximum
// length. Output: docs/diagnostics/2026-11-16/string_paths2_chrome148.json. Run with PARITY_CHROME_PATH set.
import { chromium } from 'playwright';
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH, headless: true });
const cases = {
  json_strings: `var a = new Array(2 ** 15).fill('x'.repeat(2 ** 14)); try { JSON.stringify(a).length; } catch (e) { e.name + ': ' + e.message; }`,
  json_tojson_calls: `var n = 0, big = 'x'.repeat(2 ** 14), a = []; for (let i = 0; i < 2 ** 16; i++) a.push({ toJSON() { n++; return big; } }); try { JSON.stringify(a); 'no throw'; } catch (e) { e.name + ' after ' + n; }`,
  json_object_values: `var o = {}, big = 'x'.repeat(2 ** 14); for (let i = 0; i < 2 ** 15 + 1; i++) o['k' + i] = big; try { JSON.stringify(o).length; } catch (e) { e.name + ': ' + e.message; }`,
  json_gap: `var a = [[[[[[[[[[1]]]]]]]]]], deep = a; for (let i = 0; i < 12; i++) deep = [deep, deep]; try { JSON.stringify(deep, null, 10).length; } catch (e) { e.name + ': ' + e.message; }`,
  json_under: `JSON.stringify(new Array(2 ** 10).fill('x'.repeat(2 ** 10))).length`,
  string_raw: `var raw = new Array(2 ** 15 + 1).fill('x'.repeat(2 ** 14)); try { String.raw({ raw }).length; } catch (e) { e.name + ': ' + e.message; }`,
  string_raw_subs: `var s = 'x'.repeat(2 ** 28); try { String.raw({ raw: ['', '', ''] }, s, s).length; } catch (e) { e.name + ': ' + e.message; }`,
  escape_over: `var s = '\\u0100'.repeat(2 ** 27); try { escape(s).length; } catch (e) { e.name + ': ' + e.message; }`,
  encodeuri_over: `var s = '\\u0800'.repeat(2 ** 26); try { encodeURI(s).length; } catch (e) { e.name + ': ' + e.message; }`,
  encodeuricomp_over: `var s = '\\u0800'.repeat(2 ** 26); try { encodeURIComponent(s).length; } catch (e) { e.name + ': ' + e.message; }`,
  upper_sharp_s: `var s = '\\u00df'.repeat(2 ** 28); try { s.toUpperCase().length; } catch (e) { e.name + ': ' + e.message; }`,
  locale_upper_sharp_s: `var s = '\\u00df'.repeat(2 ** 28); try { s.toLocaleUpperCase().length; } catch (e) { e.name + ': ' + e.message; }`,
  normalize_nfkd: `var s = '\\ufdfa'.repeat(2 ** 25); try { s.normalize('NFKD').length; } catch (e) { e.name + ': ' + e.message; }`,
  anchor_quotes: `var q = '"'.repeat(2 ** 27); try { 'x'.anchor(q).length; } catch (e) { e.name + ': ' + e.message; }`,
  link_long: `var s = 'x'.repeat(2 ** 28); try { s.link(s).length; } catch (e) { e.name + ': ' + e.message; }`,
  fromcharcode_many: `var codes = new Array(2 ** 20).fill(65); String.fromCharCode.apply(null, codes).length`,
  json_proxy_2_53: `var n = 0, p = new Proxy([], { get(t, k) { if (k === 'length') return 2 ** 53 - 1; n++; return 1; } }); try { JSON.stringify(p); 'ok'; } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  json_new_array_2_32: `try { JSON.stringify(new Array(2 ** 32 - 1)); } catch (e) { e.name + ': ' + e.message; }`,
  json_quote_ctrl: `try { JSON.stringify('\\x01'.repeat(2 ** 29 - 24)).length; } catch (e) { e.name + ': ' + e.message; }`,
  json_quote_quotes: `try { JSON.stringify('"'.repeat(2 ** 28)).length; } catch (e) { e.name + ': ' + e.message; }`,
  json_quote_key: `var o = {}; o['\\x01'.repeat(2 ** 27)] = 1; try { JSON.stringify(o).length; } catch (e) { e.name + ': ' + e.message; }`,
  json_quote_under: `JSON.stringify('a"b\\\\c\\x01\\n').length`,
  normalize_nfkd_2_24: `try { '\\ufdfa'.repeat(2 ** 24).normalize('NFKD').length; } catch (e) { e.name + ': ' + e.message; }`,
  upper_under: `'stra\\u00dfe'.toUpperCase()`,
  escape_under: `escape('a b\\u0100')`,
  encode_under: `encodeURIComponent('a b\\u0800')`,
  normalize_under: `'\\ufdfa'.normalize('NFKD').length`,
};
const out = { chrome: browser.version(), cases: {} };
for (const [name, src] of Object.entries(cases)) {
  const page = await browser.newPage();
  const run = page.evaluate((src) => {
    const t = performance.now();
    try { return { value: String((0, eval)(src)), ms: Math.round(performance.now() - t) }; }
    catch (e) { return { error: e.name, message: e.message, ms: Math.round(performance.now() - t) }; }
  }, src).catch(() => ({ tab: 'crashed or closed' }));
  const limit = new Promise((r) => setTimeout(() => r({ tab: 'still running after 120 s' }), 120000));
  out.cases[name] = await Promise.race([run, limit]);
  console.error(name, JSON.stringify(out.cases[name]));
  await page.close().catch(() => {});
}
console.log(JSON.stringify(out, null, 1));
await browser.close();
