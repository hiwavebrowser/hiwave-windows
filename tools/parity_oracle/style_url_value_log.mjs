// SEC-STYLE-URL: which inline style values Chrome for Testing 148 keeps and which it refuses as holding a bad
// url, written through el.style.setProperty on a custom property (which accepts any value but a bad url, so
// the answer is the url rule alone) and through the style attribute. Each value is the CSS text itself.
// Each case also carries css_units (UTF-16 code units), the exact value: a lone surrogate does not survive
// JSON text into Rust. by_property: the same check on background-image, for the native setter's refuse set.
// Output: docs/diagnostics/2026-11-17/style_url_value_chrome148.json. Fails unless PARITY_CHROME_PATH is set.
import { chromium } from 'playwright';
import { getPinnedExecutablePath } from './deterministic.mjs';
const browser = await chromium.launch({ executablePath: getPinnedExecutablePath({ required: true }), headless: true });
const values = [
  'url(a b)', 'url(ok.png)', 'URL(a b)', 'none, url(a(b)',
  '\\75rl(a b)', '\\75 rl(a b)', 'u\\72l(a b)', 'ur\\6c(a b)', '\\55 RL(a b)', '\\55\\52\\4c(a b)',
  '\\75\\72\\6c(a b)', '\\000075rl(a b)', '\\00075 rl(a b)', 'u\\rl(a b)', '\\u\\r\\l(a b)', 'U\\52 L(a b)',
  '\\75rl(ok.png)', 'u\\72l(ok.png)', '\\75rx(a b)', 'x\\75rl(a b)', '-\\75rl(a b)', '\\-url(a b)',
  'f(\\75rl(a b))', '"\\75rl(a b)"', 'url(a\\ b)', '\\75rl(a\\ b)', 'u\\\nrl(a b)', '\\75rl(a"b)',
  '/* url(a b) */', 'url(a\u00a0b)', 'url( ok.png )', 'url(ok.png', 'url(a\\\nb)', 'url(a\u0001b)', 'url(a\tb)',
  '#url(a b)', '1url(a b)', 'url(a b', 'url(\ta b)', "url( 'a b' )", 'url(a)b(c d)',
  // Added for the R1 follow-ups: a non-ASCII character before the name; CRLF / form feed after a hex escape;
  // a surrogate-range or seven-digit escape in the name; a comment between the name and '('; an escaped name
  // after '@' and after '+'; a backslash at the end of a url body.
  '\u00a0url(a b)', '\u00b7url(a b)', '\u00a0\\75rl(a b)', '\\75\r\nrl(a b)', '\\75\frl(a b)', '\\75\rrl(a b)',
  '\\d800rl(a b)', '\\0000075rl(a b)', '\\110000rl(a b)', 'url/**/(a b)', '@\\75rl(a b)', '@url(a b)',
  '+\\75rl(a b)', '+url(a b)', 'url(a\\', 'url(a\\ ', '\\75rl(a\\',
  // Edge inputs: unterminated comment / string, backslash at the end of the value, NUL, a lone surrogate,
  // url( running to the end, a url function holding a string.
  '/* url(a b)', '"url(a b)', 'a\\', 'url(a\u0000b)', '\\75\u0000rl(a b)', 'url(a\ud800b)', '\ud800url(a b)',
  'url(', 'url( ', 'url("a b")', 'url("a b" )', '\\75rl("a b")',
  // The rest of the native setter's list (ATHENA) on the custom property.
  'U\\52L(a b)', 'ur\\l(a b)', 'url\\(a b)', 'u\\72l(a.png)', 'image-set(url(a b) 1x)', 'url( a.png )',
  // A C1 control and a Unicode space inside an unquoted url (delta R1 follow-up).
  'url(a\u0085b)', 'url(a\u2003b)',
];
// [property, value] pairs on regular properties: a non-CSS-whitespace character around a value is kept
// in it, not trimmed (delta R1 follow-up).
const regular = [['color', 'red\u00a0'], ['color', '\u00a0red'], ['color', 'red\ufeff'], ['color', '\ufeffred'],
  ['color', 'red\u2003'], ['color', ' red\t'], ['width', '10px\u00a0']];
const athena = ['url(a b)', 'u\\72l(a b)', '\\75rl(a b)', '\\55 RL(a b)', '\\75 rl(a b)', 'U\\52L(a b)', 'ur\\l(a b)',
  'url\\(a b)', 'u\\72l(a.png)', 'url(a\\\nb)', 'image-set(url(a b) 1x)', 'url( a.png )', '"url(a b)"'];
const units = (v) => Array.from({ length: v.length }, (_, i) => v.charCodeAt(i));
// Text fields are written well-formed (a lone surrogate as U+FFFD, css_lossy: true); css_units is exact.
const wf = (o) => Object.fromEntries(Object.entries(o).map(([k, x]) => [k, typeof x === 'string' ? x.toWellFormed() : x]));
const text = (v) => (v.isWellFormed() ? { css: v } : { css: v.toWellFormed(), css_lossy: true });
const out = { chrome: browser.version(), property: '--x', cases: [], by_property: [] };
const page = await browser.newPage();
await page.setContent('<div id=a></div>');
for (const v of values) {
  const r = await page.evaluate((u) => {
    const v = String.fromCharCode.apply(null, u), el = document.getElementById('a');
    el.removeAttribute('style');
    el.style.setProperty('--x', 'before');
    el.style.setProperty('--x', v);
    const viaSet = el.style.getPropertyValue('--x');
    el.setAttribute('style', '--x: ' + v);
    const viaAttr = el.style.length ? el.style.getPropertyValue('--x') : null;
    return { kept_via_setProperty: viaSet !== 'before', value_via_setProperty: viaSet, kept_via_attribute: viaAttr !== null, value_via_attribute: viaAttr };
  }, units(v));
  out.cases.push({ ...text(v), css_units: units(v), ...wf(r) });
  console.error(JSON.stringify(v), JSON.stringify(r));
}
for (const v of athena) {
  const r = await page.evaluate((u) => {
    const v = String.fromCharCode.apply(null, u), el = document.getElementById('a');
    el.removeAttribute('style');
    el.style.setProperty('background-image', v);
    const viaSet = el.style.getPropertyValue('background-image');
    return { kept_via_setProperty: viaSet !== '', value_via_setProperty: viaSet };
  }, units(v));
  out.by_property.push({ property: 'background-image', ...text(v), css_units: units(v), ...wf(r) });
}
for (const [prop, v] of regular) {
  const r = await page.evaluate(([prop, u]) => {
    const v = String.fromCharCode.apply(null, u), el = document.getElementById('a');
    el.removeAttribute('style');
    el.style.setProperty(prop, v);
    const viaSet = el.style.getPropertyValue(prop);
    el.setAttribute('style', prop + ': ' + v);
    const viaAttr = el.style.getPropertyValue(prop);
    return { kept_via_setProperty: viaSet !== '', value_via_setProperty: viaSet, kept_via_attribute: viaAttr !== '', value_via_attribute: viaAttr };
  }, [prop, units(v)]);
  out.by_property.push({ property: prop, ...text(v), css_units: units(v), ...wf(r) });
}
// Built-ins replaced by page script do not change the answer.
out.replaced_builtins = await page.evaluate(() => {
  const el = document.getElementById('a');
  String.prototype.substr = () => 'x'; String.prototype.toLowerCase = function () { return 'x'; };
  String.prototype.charAt = () => 'x'; RegExp.prototype.test = () => false; String.prototype.indexOf = () => -1;
  el.style.setProperty('--x', 'before');
  el.style.setProperty('--x', 'url(a b)');
  return el.style.getPropertyValue('--x');
});
console.log(JSON.stringify(out, null, 1));
await browser.close();
