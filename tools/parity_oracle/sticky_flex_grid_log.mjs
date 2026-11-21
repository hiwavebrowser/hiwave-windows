// Plan row C7: what Chromium reads for each sticky-in-flex/grid/table case in
// sticky_flex_grid_cases.json. Same page and script bytes as
// crates/rustkit-engine/src/sticky_flex_grid_tests.rs; one fresh 800x600 page
// per case. The script runs inside a function, since a global `var top`
// cannot replace `window.top` in Chromium.
// usage: node sticky_flex_grid_log.mjs
import fs from 'node:fs';
import { chromium } from 'playwright';
const here = new URL('.', import.meta.url).pathname;
const { at, cases } = JSON.parse(fs.readFileSync(here + 'sticky_flex_grid_cases.json', 'utf8'));
const b = await chromium.launch();
console.log('chromium', b.version());
for (const [name, c] of Object.entries(cases)) {
  const p = await b.newPage({ viewport: { width: 800, height: 600 } });
  await p.route('http://probe.test/**', r => r.fulfill({ contentType: 'text/html', body: c.page }));
  await p.goto('http://probe.test/page');
  const got = await p.evaluate((src) => String((0, eval)(src)), '(function () { ' + at + 'return ' + c.script + '; })()');
  console.log(name, '=>', got);
  await p.close();
}
await b.close();
