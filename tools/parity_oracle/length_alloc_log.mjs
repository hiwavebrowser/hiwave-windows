// JS-ROBUST: what Chrome for Testing 148 does when an array-like's `length` is far larger than what it
// holds. Output: docs/diagnostics/2026-11-16/length_alloc_chrome148.json. Run with PARITY_CHROME_PATH set.
// Each case runs on its own page with a 60 s limit: Chrome walks a sparse length in sort.
import { chromium } from 'playwright';
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH, headless: true });
const f = 'var n = 0, o = { get 0() { n++; return 1; } };';
const cases = {
  apply_2_32: `${f} o.length = 2 ** 32 - 1; try { (function () {}).apply(null, o); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  apply_2_27_plus_1: `${f} o.length = 2 ** 27 + 1; try { (function () {}).apply(null, o); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  reflect_apply_2_32: `${f} o.length = 2 ** 32 - 1; try { Reflect.apply(function () {}, null, o); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  reflect_apply_2_27_plus_1: `${f} o.length = 2 ** 27 + 1; try { Reflect.apply(function () {}, null, o); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  reflect_construct_2_32: `${f} o.length = 2 ** 32 - 1; try { Reflect.construct(function () {}, o); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  reflect_construct_2_27_plus_1: `${f} o.length = 2 ** 27 + 1; try { Reflect.construct(function () {}, o); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  apply_2_27: `try { (function () {}).apply(null, { length: 2 ** 27 }); 'ok'; } catch (e) { e.name + ': ' + e.message; }`,
  apply_2_17_plus_1: `var n = 0, o = { length: 2 ** 17 + 1 }; for (let i = 0; i < 2 ** 17 + 1; i++) Object.defineProperty(o, i, { get() { n++; return i; } }); try { (function () {}).apply(null, o); 'ok'; } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  apply_2_16: `(function () { return arguments.length; }).apply(null, { length: 2 ** 16 })`,
  spread_2_17_plus_1: `try { Math.max(...new Array(2 ** 17 + 1).fill(1)); } catch (e) { e.name + ': ' + e.message; }`,
  apply_2_15: `(function () { return arguments.length; }).apply(null, { length: 2 ** 15 })`,
  spread_2_15: `Math.max(...new Array(2 ** 15).fill(1))`,
  spread_2_16: `Math.max(...new Array(2 ** 16).fill(1))`,
  new_spread_2_17_plus_1: `try { new Array(...new Array(2 ** 17 + 1).fill(1)).length; } catch (e) { e.name + ': ' + e.message; }`,
  array_of_2_27: `try { Array.of.apply(null, { length: 2 ** 27 }).length; } catch (e) { e.name + ': ' + e.message; }`,
  ownkeys_2_27_plus_1: `var n = 0, keys = { length: 2 ** 27 + 1, get 0() { n++; return 'a'; } }; try { Reflect.ownKeys(new Proxy({}, { ownKeys() { return keys; } })); 'ok'; } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  ownkeys_small: `Reflect.ownKeys(new Proxy({}, { ownKeys() { return ['b', 'a']; } })).join()`,
  ta_join_over: `try { new Uint8Array(2 ** 20).join('x'.repeat(2 ** 12)).length; } catch (e) { e.name + ': ' + e.message; }`,
  ta_join_over2: `try { new Uint8Array(2 ** 28).join('xx').length; } catch (e) { e.name + ': ' + e.message; }`,
  ta_join_under: `new Uint8Array([1, 2, 3]).join('-')`,
  ta_join_big_under: `new Uint8Array(2 ** 20).join('').length`,
  ta_tolocale_under: `new Uint8Array([1, 2, 3]).toLocaleString()`,
  apply_small: `(function () { return [].join.call(arguments, '-'); }).apply(null, { length: 3, 0: 'a', 1: 'b', 2: 'c' })`,
  json_2_32: `var n = 0, a = []; Object.defineProperty(a, 0, { get() { n++; return 1; } }); a.length = 2 ** 32 - 1; try { JSON.stringify(a); } catch (e) { e.name + ': ' + e.message + ' ' + n; }`,
  json_2_20: `var a = []; a.length = 2 ** 20; JSON.stringify(a).length`,
  sort_2_24: `var a = [3, 1, 2]; a.length = 2 ** 24; var o = { length: 2 ** 24, 0: 3, 1: 1, 2: 2 }; a.sort(); Array.prototype.sort.call(o); [a[0], a[1], a[2], a[3], a.length, o[0], o[1], o[2], o.length].join()`,
  sort_2_32: `var a = [3, 1, 2]; a.length = 2 ** 32 - 1; a.sort(); a[0]`,
};
const out = { chrome: browser.version(), cases: {} };
for (const [name, src] of Object.entries(cases)) {
  const page = await browser.newPage();
  const run = page.evaluate((src) => {
    const t = performance.now();
    try { return { value: String((0, eval)(src)), ms: Math.round(performance.now() - t) }; }
    catch (e) { return { error: e.name, message: e.message, ms: Math.round(performance.now() - t) }; }
  }, src).catch((e) => ({ tab: 'crashed or closed' }));
  const limit = new Promise((r) => setTimeout(() => r({ tab: 'still running after 60 s' }), 60000));
  out.cases[name] = await Promise.race([run, limit]);
  console.error(name, JSON.stringify(out.cases[name]));
  await page.close().catch(() => {});
}
console.log(JSON.stringify(out, null, 1));
await browser.close();
