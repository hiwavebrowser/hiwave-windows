// Chrome's Function.prototype.toString for class constructors (YT-CLASS-CTOR): the receipt for
// boa_engine's `test_class_to_string` (third_party/boa_engine/src/tests/to_string.rs) and for the
// Polymer feature test YouTube's kevlar_base runs, `/\bclass\b/.test(class{}.toString())`.
// Usage: PARITY_CHROME_PATH=<Chrome for Testing 148.0.7778.216> node class_source_text_log.mjs
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
console.log('chrome ' + browser.version());
const page = await browser.newPage();
await page.setContent('<html><body></body></html>');
const out = await page.evaluate(() => {
  const src = `
    class Base {}
    class Rect extends Base {
        constructor(h, w) { super(); this.h = h; /* } */ this.w = w; }
        area() { return this.h * this.w; }
    }
    const Anon = class{};
    const NamedRef = class   Named   { static x = 1; m() {} } ;
    class Plain { get v() { return 1 } }
    ({
      decl: Rect.toString(),
      anon: Anon.toString(),
      named: NamedRef.toString(),
      no_ctor: Plain.toString(),
      inline: (class{}).toString(),
      detect: String(/\\bclass\\b/.test(class{}.toString())),
    })`;
  const r = (0, eval)(src);
  // The YouTube path: a class called through Function.prototype.apply.
  try { (class X {}).apply({}); r.apply = 'no throw'; } catch (e) { r.apply = e.constructor.name + ': ' + e.message; }
  return r;
});
for (const [k, v] of Object.entries(out)) console.log(k + ': ' + JSON.stringify(v));
await browser.close();
