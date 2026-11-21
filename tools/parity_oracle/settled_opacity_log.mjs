// What the oracle's Chrome settles on for the cases of the engine's
// keyframes_opacity_tests: finite animations after they end, infinite ones at
// t=0 (the engine paints one static frame, RD3).
// usage: node tools/parity_oracle/settled_opacity_log.mjs
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const cases = [
  ['infinite, 0% sets none, own .75', 'opacity:.75;animation:s .05s infinite', '@keyframes s{0%{transform:scale(1)}100%{transform:scale(1.5);opacity:0}}'],
  ['forwards, 100% .25', 'opacity:.75;animation:x .05s forwards', '@keyframes x{100%{opacity:.25}}'],
  ['both, to .25', 'opacity:.75;animation:x .05s both', '@keyframes x{to{opacity:.25}}'],
  ['fade-in forwards', 'opacity:0;animation:x .05s forwards', '@keyframes x{from{opacity:0}to{opacity:1}}'],
  ['fade-in, no fill, own 1', 'animation:x .05s ease-in', '@keyframes x{from{opacity:0}to{opacity:1}}'],
  ['fade-in, no fill, own .5', 'opacity:.5;animation:x .05s', '@keyframes x{from{opacity:0}to{opacity:1}}'],
  ['infinite, 0% .2 in a list', 'animation:x .05s infinite', '@keyframes x{0%,100%{opacity:.2}50%{opacity:1}}'],
  ['forwards, no opacity, own .5', 'opacity:.5;animation:x .05s forwards', '@keyframes x{to{transform:scale(2)}}'],
  ['reverse forwards', 'animation:x .05s reverse forwards', '@keyframes x{from{opacity:.25}to{opacity:.75}}'],
  ['2 alternate forwards', 'animation:x .05s 2 alternate forwards', '@keyframes x{from{opacity:.25}to{opacity:.75}}'],
  ['3 alternate forwards', 'animation:x .05s 3 alternate forwards', '@keyframes x{from{opacity:.25}to{opacity:.75}}'],
  ['2 alternate-reverse forwards', 'animation:x .05s 2 alternate-reverse forwards', '@keyframes x{from{opacity:.25}to{opacity:.75}}'],
  ['infinite reverse', 'animation:x .05s infinite reverse', '@keyframes x{from{opacity:.25}to{opacity:.75}}'],
  ['no such keyframes, own 0', 'opacity:0;animation:nosuch .05s forwards', ''],
];

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 400, height: 300 } });
console.log('chrome ' + browser.version());
for (const [label, decl, frames] of cases) {
  await page.setContent(`<!doctype html><style>#r{${decl}}${frames}</style><div id=r></div>`);
  await page.waitForTimeout(400);
  const got = await page.evaluate(() => {
    const el = document.getElementById('r');
    for (const a of el.getAnimations()) {
      if (a.effect.getTiming().iterations === Infinity) {
        a.pause();
        a.currentTime = 0;
      }
    }
    return getComputedStyle(el).opacity;
  });
  console.log(label + ': ' + got);
}
await browser.close();
