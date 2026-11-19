// What the pinned Chrome reports for getComputedStyle(fill / stroke) on
// inline SVG elements, with no transition (A2.1 of the CSS-transitions
// brief): presentation attributes, author rules, the style attribute,
// !important, inheritance, currentcolor, url() paints, none, invalid
// values. One page per case; each reads the listed elements.
// Writes docs/diagnostics/2026-11-17/svg_computed_paint_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/svg_computed_paint_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/svg_computed_paint_cft148.json');

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) {
  throw new Error(`expected Chrome for Testing ${PIN}, got ${browser.version()}`);
}
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
console.log('chrome ' + browser.version());
const cases = [];

async function probe(label, css, svg, ids) {
  await page.setContent('<!doctype html><style>body{margin:0}' + css + '</style>' +
    '<svg id=s width=100 height=100>' + svg + '</svg>');
  const out = await page.evaluate((ids) => ids.map((id) => {
    const cs = getComputedStyle(document.getElementById(id));
    return id + ' fill=' + cs.fill + ' stroke=' + cs.stroke;
  }), ids);
  console.log(`${label}: ${out.join(' | ')}`);
  cases.push({ label, css, svg, read: out });
}

const R = '<rect id=p width=10 height=10';
await probe('initial', '', R + ' />', ['s', 'p']);
await probe('attributes', '', '<g id=g fill=blue stroke=red>' + R + ' /></g>', ['g', 'p']);
await probe('css beats attribute', '#p{fill:lime}', R + ' fill=blue stroke=red />', ['p']);
await probe('inherited from svg css', '#s{fill:blue;stroke:red}', '<g id=g>' + R + ' /></g>', ['s', 'g', 'p']);
await probe('style attribute beats css', '#p{fill:lime}', R + ' style="fill:blue" />', ['p']);
await probe('css !important beats style attribute', '#p{fill:lime!important}', R + ' style="fill:blue" />', ['p']);
await probe('style !important beats css !important', '#p{fill:lime!important}', R + ' style="fill:blue!important" />', ['p']);
await probe('specificity', '#p{fill:blue} rect{fill:lime}', R + ' />', ['p']);
await probe('url paint', '', '<defs><linearGradient id=gr /></defs>' + R + ' fill="url(#gr)" stroke="url(#gr) red" />', ['p']);
await probe('url paint css', '#p{fill:url(#gr);stroke:url("#gr") none}', '<defs><linearGradient id=gr /></defs>' + R + ' />', ['p']);
await probe('none', '#p{fill:none}', R + ' stroke=none />', ['p']);
await probe('currentcolor css', '#s{color:blue}#p{fill:currentcolor;stroke:currentColor}', R + ' />', ['p']);
await probe('currentcolor attribute', '#s{color:rgb(0,128,0)}', R + ' fill=currentColor />', ['p']);
await probe('alpha and hsl', '#p{fill:rgba(0,0,255,.5);stroke:hsl(120,100%,50%)}', R + ' />', ['p']);
await probe('named colour attribute', '', R + ' fill=coral stroke=transparent />', ['p']);
await probe('invalid attribute is ignored', '', '<g fill=blue>' + R + ' fill=bogus /></g>', ['p']);
await probe('inherit keyword over attribute', '#p{fill:inherit}', '<g fill=blue>' + R + ' fill=red /></g>', ['p']);
await probe('inherited from an html ancestor', 'body{fill:blue;stroke:red}', R + ' />', ['s', 'p']);

await probe('quoted url with a parenthesis', '', R + ` fill='url("a)b")' stroke='url("a)b") red' />`, ['p']);
await probe('a long url() chain is invalid', '', '<g fill=blue>' + R + ' fill="' + 'url(a)'.repeat(1000) + '" /></g>', ['p']);

// A presentation attribute written by script inside a display:none
// subtree, read before and after the write (R1 on #1294).
{
  await page.setContent('<!doctype html><div style="display:none"><svg id=s><g id=g fill=blue><rect id=p width=10 height=10 /></g></svg></div>');
  const out = await page.evaluate(() => {
    const read = () => getComputedStyle(document.getElementById('p')).fill;
    const before = read();
    document.getElementById('g').setAttribute('fill', 'red');
    const after = read();
    document.getElementById('p').setAttribute('fill', 'lime');
    return [before, after, read()];
  });
  console.log(`attribute writes in a display:none subtree: ${out.join(' | ')}`);
  cases.push({ label: 'attribute writes in a display:none subtree', reads: out });
}

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
