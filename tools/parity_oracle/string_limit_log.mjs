// JS-ROBUST: what Chrome for Testing 148 does when a script builds an over-long string.
// Output: docs/diagnostics/2026-11-16/string_limit_chrome148.json. Run with PARITY_CHROME_PATH set.
import { chromium } from 'playwright';
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH, headless: true });
const page = await browser.newPage();
await page.setContent('<!doctype html><title>probe</title>');
const cases = {
  replace_reduced: `function puff(x, n) { while (x.length < n) x += x; return x.substring(0, n); }
    var x = puff("1", 1 << 20), rep = puff("$1", 1 << 16);
    x.replace(/(.+)/g, rep).length`,
  repeat_max: `'x'.repeat(2 ** 29 - 24).length`,
  repeat_over: `'x'.repeat(2 ** 29 - 23).length`,
  concat_over: `var s = 'x'.repeat(2 ** 28); (s + s).length`,
  concat_max: `var s = 'x'.repeat(2 ** 28); (s + s.slice(24)).length`,
  join_over: `new Array(2 ** 15 + 1).fill('x'.repeat(2 ** 14)).join('').length`,
  template_over: `var s = 'x'.repeat(2 ** 28); \`\${s}\${s}\`.length`,
  pad_over: `'x'.padEnd(2 ** 29).length`,
  replace_fn_calls: `var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20), n = 0;
    try { x.replace(/x/g, () => (n++, y)); 'no throw'; } catch (e) { e.name + ' after ' + n; }`,
  replaceall_fn_calls: `var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20), n = 0;
    try { x.replaceAll('x', () => (n++, y)); 'no throw'; } catch (e) { e.name + ' after ' + n; }`,
  named_getter_calls: `var x = 'ab'.repeat(2 ** 10), rep = '$<g>'.repeat(2 ** 4), z = 'z'.repeat(2 ** 16), calls = 0;
    var re = /(?<g>ab)/g, orig = RegExp.prototype.exec;
    re.exec = function (s) { var r = orig.call(this, s); if (r) r.groups = { get g() { calls++; return z; } }; return r; };
    try { re[Symbol.replace](x, rep); 'no throw'; } catch (e) { e.name + ' after ' + calls; }`,
  many_replace_str: `var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20); try { x.replace(/x/g, y).length; } catch (e) { e.name + ': ' + e.message; }`,
  many_replaceall_str: `var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20); try { x.replaceAll('x', y).length; } catch (e) { e.name + ': ' + e.message; }`,
  many_replaceall_re: `var x = 'x'.repeat(2 ** 10), y = 'y'.repeat(2 ** 20); try { x.replaceAll(/x/g, y).length; } catch (e) { e.name + ': ' + e.message; }`,
  tolocale_over: `try { new Array(2 ** 15 + 1).fill('x'.repeat(2 ** 14)).toLocaleString().length; } catch (e) { e.name + ': ' + e.message; }`,
  concat_method_over: `var s = 'x'.repeat(2 ** 28); try { s.concat(s).length; } catch (e) { e.name + ': ' + e.message; }`,
  plus_eq_over: `var s = 'x'.repeat(2 ** 28), t = s; try { t += s; t.length; } catch (e) { e.name + ': ' + e.message; }`,
  pad_start_over: `try { 'x'.padStart(2 ** 29).length; } catch (e) { e.name + ': ' + e.message; }`,
  join_sep_over: `try { new Array(2 ** 15).fill('x'.repeat(2 ** 14 - 1)).join('--').length; } catch (e) { e.name + ': ' + e.message; }`,
  repeat_big_ok: `'x'.repeat(2 ** 28).length`,
  under_limit_a: `'ab'.repeat(3) + 'c'.padEnd(3, '.') + ['x', 'y'].join('-')`,
  under_limit_b: `'aXbXc'.replace(/X/g, '$&$&').replaceAll('b', '$\`')`,
  join_huge_length: `var n = 0; var o = { length: 2 ** 32 - 1, get 0() { n++; return 'a'; } };
    try { Array.prototype.join.call(o, ','); 'no throw'; } catch (e) { e.name + ': ' + e.message + ' after ' + n + ' getter calls'; }`,
  join_huge_length_empty_sep: `var n = 0; var o = { length: 2 ** 20, get 0() { n++; return 'a'; } };
    Array.prototype.join.call(o, '').length + ' after ' + n + ' getter calls'`,
  join_running: `var n = 0, big = 'x'.repeat(2 ** 14); var o = { length: 2 ** 16 };
    for (let i = 0; i < 2 ** 16; i++) Object.defineProperty(o, i, { get() { n++; return big; } });
    try { Array.prototype.join.call(o, ''); 'no throw'; } catch (e) { e.name + ' after ' + n + ' getter calls'; }`,
  json_over: `JSON.stringify(new Array(2 ** 15).fill('x'.repeat(2 ** 14))).length`,
};
const out = { chrome: browser.version(), cases: {} };
for (const [name, src] of Object.entries(cases)) {
  out.cases[name] = await page.evaluate((src) => {
    const t = performance.now();
    try { return { value: (0, eval)(src), ms: Math.round(performance.now() - t) }; }
    catch (e) { return { error: e.name, message: e.message, ms: Math.round(performance.now() - t) }; }
  }, src);
}
console.log(JSON.stringify(out, null, 1));
await browser.close();
