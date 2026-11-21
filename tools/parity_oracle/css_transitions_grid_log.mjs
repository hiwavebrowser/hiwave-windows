// What the pinned Chrome reports for transitions of group G of the
// CSS-transitions brief (grid-template-columns / -rows): the value at
// currentTime 50 of a 100ms linear transition (paused and seeked, no
// timers; also at 25 and 75), the layout (the second item's offset and
// size, the third's offset), the transition's keyframes, which
// transitions run, and their events once finished. #h is a 400x200 grid
// with three empty items.
// Writes docs/diagnostics/2026-11-17/css_transitions_grid_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_grid_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/css_transitions_grid_cft148.json');

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

async function probe(label, css, read) {
  await page.setContent('<!doctype html><style>body{margin:0}#h{display:grid;width:400px;height:200px;' +
    'font:16px sans-serif;transition:all 100ms linear}#h>i{display:block}' + css +
    '</style><div id=h><i id=a></i><i id=b></i><i id=c></i></div>');
  await page.evaluate(() => document.body.offsetWidth);
  const out = await page.evaluate(async (read) => {
    const el = document.getElementById('h');
    const b = document.getElementById('b');
    const c = document.getElementById('c');
    const events = [];
    for (const t of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel']) {
      el.addEventListener(t, (e) => events.push(e.type.slice(10) + ':' + e.propertyName));
    }
    const offsets = () => [b.offsetLeft, b.offsetTop, b.offsetWidth, b.offsetHeight, c.offsetLeft, c.offsetTop].join(' ');
    const before = offsets();
    el.classList.add('on');
    getComputedStyle(el).getPropertyValue(read);
    const anims = document.getAnimations();
    const at = {};
    for (const t of [25, 75]) {
      anims.forEach((a) => { a.pause(); a.currentTime = t; });
      at[t] = getComputedStyle(el).getPropertyValue(read) + ' | ' + offsets();
    }
    anims.forEach((a) => { a.pause(); a.currentTime = 50; });
    const mid = getComputedStyle(el).getPropertyValue(read);
    const midOffsets = offsets();
    const names = anims.map((a) => a.transitionProperty);
    const keyframes = anims.map((a) => a.effect.getKeyframes().map((k) => k[Object.keys(k).find((p) => p.startsWith('grid'))]));
    anims.forEach((a) => a.finish());
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    return { mid, at, before, midOffsets, endOffsets: offsets(), keyframes, transitions: names, events,
      end: getComputedStyle(el).getPropertyValue(read) };
  }, read);
  console.log(`${label}: ${out.mid} | at ${JSON.stringify(out.at)} | offsets ${out.before} -> ${out.midOffsets} -> ${out.endOffsets} | ` +
    `kf ${JSON.stringify(out.keyframes)} | anims [${out.transitions}] | events [${out.events}] | end ${out.end}`);
  cases.push({ label, css, read, ...out });
}

const C = 'grid-template-columns';
const R = 'grid-template-rows';
await probe('px px', '#h{grid-template-columns:100px 100px}#h.on{grid-template-columns:200px 50px}', C);
await probe('px fr', '#h{grid-template-columns:100px 1fr}#h.on{grid-template-columns:200px 2fr}', C);
await probe('fr fr', '#h{grid-template-columns:1fr 1fr}#h.on{grid-template-columns:1fr 3fr}', C);
await probe('repeat', '#h{grid-template-columns:repeat(2,100px)}#h.on{grid-template-columns:repeat(2,200px)}', C);
await probe('repeat vs list', '#h{grid-template-columns:repeat(2,100px)}#h.on{grid-template-columns:200px 200px}', C);
await probe('percent', '#h{grid-template-columns:10% 100px}#h.on{grid-template-columns:100px 100px}', C);
await probe('auto keyword', '#h{grid-template-columns:100px auto}#h.on{grid-template-columns:200px auto}', C);
await probe('minmax', '#h{grid-template-columns:minmax(100px,1fr) 50px}#h.on{grid-template-columns:minmax(200px,2fr) 50px}', C);
await probe('fit-content', '#h{grid-template-columns:fit-content(100px) 50px}#h.on{grid-template-columns:fit-content(200px) 50px}', C);
await probe('line names same', '#h{grid-template-columns:[a] 100px [b] 100px}#h.on{grid-template-columns:[a] 200px [b] 50px}', C);
await probe('line names differ', '#h{grid-template-columns:[a] 100px 100px}#h.on{grid-template-columns:[z] 200px 50px}', C);
await probe('mismatched length', '#h{grid-template-columns:100px 100px}#h.on{grid-template-columns:200px 200px 200px}', C);
await probe('mismatched kind px/fr', '#h{grid-template-columns:100px 1fr}#h.on{grid-template-columns:200px 100px}', C);
await probe('mismatched kind px/auto', '#h{grid-template-columns:100px 100px}#h.on{grid-template-columns:200px auto}', C);
await probe('none -> list', '#h{grid-template-columns:none}#h.on{grid-template-columns:100px 200px}', C);
await probe('list -> none', '#h{grid-template-columns:100px 200px}#h.on{grid-template-columns:none}', C);
await probe('rows px', '#h{grid-template-rows:20px 20px}#h.on{grid-template-rows:60px 100px}', R);
await probe('rows px fr', '#h{grid-template-rows:20px 1fr}#h.on{grid-template-rows:60px 3fr}', R);

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: 50, duration_ms: 100, timing: 'linear', cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
