// What the pinned Chrome reports for transitions of the individual
// transform properties (css-transforms-2 §5: translate, rotate, scale),
// group B of the CSS-transitions brief: the value at currentTime 50 of a
// 100ms linear transition (paused and seeked, no timers), which
// transitions run, and their events once finished.
// Writes docs/diagnostics/2026-11-17/css_transitions_individual_transforms_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_individual_transforms_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/css_transitions_individual_transforms_cft148.json');

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
  await page.setContent('<!doctype html><style>body{margin:0}#h{width:100px;height:100px;' +
    'transition:all 100ms linear}' + css + '</style><div id=h></div>');
  await page.evaluate(() => document.body.offsetWidth);
  const out = await page.evaluate(async (read) => {
    const el = document.getElementById('h');
    const events = [];
    for (const t of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel']) {
      el.addEventListener(t, (e) => events.push(e.type.slice(10) + ':' + e.propertyName));
    }
    el.classList.add('on');
    getComputedStyle(el).getPropertyValue(read);
    const anims = document.getAnimations();
    anims.forEach((a) => { a.pause(); a.currentTime = 50; });
    const mid = getComputedStyle(el).getPropertyValue(read);
    const transform = getComputedStyle(el).transform;
    const names = anims.map((a) => a.transitionProperty);
    anims.forEach((a) => a.finish());
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    return { mid, transform, transitions: names, events, end: getComputedStyle(el).getPropertyValue(read) };
  }, read);
  console.log(`${label}: ${out.mid} | anims [${out.transitions}] | events [${out.events}] | end ${out.end}`);
  cases.push({ label, css, read, ...out });
}

await probe('translate none->100px 50px', '#h.on{translate:100px 50px}', 'translate');
await probe('translate 0->100px', '#h{translate:0px}#h.on{translate:100px}', 'translate');
await probe('translate 10px 20px->30px 60px', '#h{translate:10px 20px}#h.on{translate:30px 60px}', 'translate');
await probe('translate 0%->100%', '#h{translate:0%}#h.on{translate:100%}', 'translate');
await probe('translate 0px->50%', '#h{translate:0px}#h.on{translate:50%}', 'translate');
await probe('translate 10px->50%', '#h{translate:10px}#h.on{translate:50%}', 'translate');
await probe('translate one value->two', '#h{translate:10px}#h.on{translate:30px 40px}', 'translate');
await probe('translate two values->one', '#h{translate:30px 40px}#h.on{translate:10px}', 'translate');
await probe('translate rem->percent', '#h{translate:1rem}#h.on{translate:50%}', 'translate');
await probe('scale one value->two', '#h{scale:1}#h.on{scale:2 3}', 'scale');
await probe('scale two values->one', '#h{scale:2 3}#h.on{scale:1}', 'scale');
await probe('translate 100px->none', '#h{translate:100px}#h.on{translate:none}', 'translate');
await probe('rotate none->90deg', '#h.on{rotate:90deg}', 'rotate');
await probe('rotate 45deg->405deg', '#h{rotate:45deg}#h.on{rotate:405deg}', 'rotate');
await probe('rotate 0deg->1turn', '#h{rotate:0deg}#h.on{rotate:1turn}', 'rotate');
await probe('rotate z 0deg->z 90deg', '#h{rotate:z 0deg}#h.on{rotate:z 90deg}', 'rotate');
await probe('rotate 90deg->none', '#h{rotate:90deg}#h.on{rotate:none}', 'rotate');
await probe('scale none->2', '#h.on{scale:2}', 'scale');
await probe('scale 1 2->3 4', '#h{scale:1 2}#h.on{scale:3 4}', 'scale');
await probe('scale 50%->150%', '#h{scale:50%}#h.on{scale:150%}', 'scale');

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: 50, duration_ms: 100, timing: 'linear', cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
