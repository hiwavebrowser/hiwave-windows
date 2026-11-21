// Where does shadow DOM content render?
//
// The pinned Chromium loads each page in shadow_tree_cases.json (through
// the HTML parser, so declarative shadow roots attach), runs the case's
// `script` if it has one, and reports:
//   chrome_boxes  "id:x:y:width:height" for every element with an id, the
//                 document's first, then each shadow tree's (BOXES);
//   chrome_hits   for each [x, y] in `probes`, the id of the innermost
//                 element there, inside shadow trees (HITS);
//   chrome_colors when the case sets `colors`: "id:color/background" for
//                 every element with an id that has a box, in BOXES order,
//                 "id:-" for one without (COLORS).
//   chrome_painted when the case sets `painted`: "id:P" for every element
//                 with an id whose background is painted, "id:-" for one
//                 that is not (no box, `visibility` not visible, or an
//                 ancestor in the flat tree at opacity 0), in BOXES order
//                 (PAINTED). Every such element of the case has its own
//                 background colour, so the engine finds it in its display
//                 list.
// The engine is tested against these strings (crates/rustkit-engine,
// shadow_tree_tests).
//
//   node shadow_tree_log.mjs            print
//   node shadow_tree_log.mjs --write    print, and store in the case file
import { readFileSync, writeFileSync } from 'node:fs';
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const FILE = new URL('./shadow_tree_cases.json', import.meta.url);
// The same expressions the engine test evaluates.
export const BOXES = `(function () {
  var out = [];
  function walk(root) {
    Array.prototype.forEach.call(root.querySelectorAll('[id]'), function (e) {
      var r = e.getBoundingClientRect();
      out.push(e.id + ':' + Math.round(r.left) + ':' + Math.round(r.top) + ':' + Math.round(r.width) + ':' + Math.round(r.height));
    });
    Array.prototype.forEach.call(root.querySelectorAll('*'), function (e) { if (e.shadowRoot) walk(e.shadowRoot); });
  }
  walk(document);
  return out.join(' ');
})()`;
export const COLORS = `(function () {
  var out = [];
  function walk(root) {
    Array.prototype.forEach.call(root.querySelectorAll('[id]'), function (e) {
      var r = e.getBoundingClientRect();
      if (!r.width && !r.height) { out.push(e.id + ':-'); return; }
      var s = getComputedStyle(e);
      out.push(e.id + ':' + s.color.replace(/ /g, '') + '/' + s.backgroundColor.replace(/ /g, ''));
    });
    Array.prototype.forEach.call(root.querySelectorAll('*'), function (e) { if (e.shadowRoot) walk(e.shadowRoot); });
  }
  walk(document);
  return out.join(' ');
})()`;
export const PAINTED = `(function () {
  var out = [];
  function shown(e) {
    if (getComputedStyle(e).visibility !== 'visible') return false;
    for (var n = e; n; n = n.assignedSlot || n.parentNode || n.host) {
      if (n.nodeType === 1 && parseFloat(getComputedStyle(n).opacity) === 0) return false;
    }
    return true;
  }
  function walk(root) {
    Array.prototype.forEach.call(root.querySelectorAll('[id]'), function (e) {
      var r = e.getBoundingClientRect();
      out.push(e.id + ':' + ((r.width && r.height && shown(e)) ? 'P' : '-'));
    });
    Array.prototype.forEach.call(root.querySelectorAll('*'), function (e) { if (e.shadowRoot) walk(e.shadowRoot); });
  }
  walk(document);
  return out.join(' ');
})()`;
const HITS = (probes) => `(function () {
  return ${JSON.stringify(probes)}.map(function (p) {
    var e = document.elementFromPoint(p[0], p[1]);
    while (e && e.shadowRoot) {
      var inner = e.shadowRoot.elementFromPoint(p[0], p[1]);
      if (!inner || inner === e) break;
      e = inner;
    }
    return e ? (e.id || e.localName) : 'none';
  }).join(' ');
})()`;

const data = JSON.parse(readFileSync(FILE, 'utf8'));
const [width, height] = data.viewport;
const browser = await chromium.launch(getDeterministicLaunchOptions());
console.log('chrome ' + browser.version());
for (const c of data.cases) {
  const page = await browser.newPage({ viewport: { width, height } });
  // A navigation, not setContent: declarative shadow roots attach only in
  // a document the parser builds.
  await page.route('http://case.test/', (route) => route.fulfill({ contentType: 'text/html', body: c.html }));
  await page.goto('http://case.test/');
  if (c.script) await page.evaluate(c.script);
  c.chrome_boxes = await page.evaluate(BOXES);
  console.log(c.name + '  ' + c.chrome_boxes);
  if (c.colors) {
    c.chrome_colors = await page.evaluate(COLORS);
    console.log('  colors ' + c.chrome_colors);
  }
  if (c.painted) {
    c.chrome_painted = await page.evaluate(PAINTED);
    console.log('  painted ' + c.chrome_painted);
  }
  if (c.probes) {
    c.chrome_hits = await page.evaluate(HITS(c.probes));
    console.log('  hits ' + c.chrome_hits);
  }
  await page.close();
}
if (process.argv.includes('--write')) {
  data.chrome = browser.version();
  writeFileSync(FILE, JSON.stringify(data, null, 1) + '\n');
}
await browser.close();
