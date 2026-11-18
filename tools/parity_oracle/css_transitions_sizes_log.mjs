// What the pinned Chrome reports for transitions of group C of the
// CSS-transitions brief (line-height, border-radius, background-size) and
// for length-percentage mixes through calc(): the value at currentTime 50
// of a 100ms linear transition (paused and seeked, no timers), which
// transitions run, and their events once finished.
// Writes docs/diagnostics/2026-11-17/css_transitions_sizes_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_sizes_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/css_transitions_sizes_cft148.json');

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
  await page.setContent('<!doctype html><style>body{margin:0}#h{width:100px;height:50px;font:16px sans-serif;' +
    'background:linear-gradient(red,blue) no-repeat;transition:all 100ms linear}' + css + '</style><div id=h>text</div>');
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
    const names = anims.map((a) => a.transitionProperty);
    anims.forEach((a) => a.finish());
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    return { mid, transitions: names, events, end: getComputedStyle(el).getPropertyValue(read) };
  }, read);
  console.log(`${label}: ${out.mid} | anims [${out.transitions}] | events [${out.events}] | end ${out.end}`);
  cases.push({ label, css, read, ...out });
}

// line-height
await probe('line-height px', '#h{line-height:20px}#h.on{line-height:40px}', 'line-height');
await probe('line-height number', '#h{line-height:1}#h.on{line-height:2}', 'line-height');
await probe('line-height percent', '#h{line-height:100%}#h.on{line-height:200%}', 'line-height');
await probe('line-height number->px', '#h{line-height:1}#h.on{line-height:40px}', 'line-height');
await probe('line-height normal->px', '#h{line-height:normal}#h.on{line-height:40px}', 'line-height');
// border-radius
await probe('border-radius px', '#h{border-radius:0px}#h.on{border-radius:20px}', 'border-top-left-radius');
await probe('border-radius all corners', '#h{border-radius:0px}#h.on{border-radius:10px 20px 30px 40px}', 'border-radius');
await probe('border-radius elliptical', '#h{border-radius:10px / 20px}#h.on{border-radius:30px / 40px}', 'border-top-left-radius');
await probe('border-radius percent', '#h{border-radius:0%}#h.on{border-radius:50%}', 'border-top-left-radius');
await probe('border-radius px->percent', '#h{border-radius:10px}#h.on{border-radius:50%}', 'border-top-left-radius');
await probe('border-top-right-radius named', '#h{border-top-right-radius:0px;transition:border-top-right-radius 100ms linear}#h.on{border-top-right-radius:20px}', 'border-top-right-radius');
// background-size
await probe('background-size px', '#h{background-size:10px 20px}#h.on{background-size:30px 40px}', 'background-size');
await probe('background-size percent', '#h{background-size:0% 0%}#h.on{background-size:100% 50%}', 'background-size');
await probe('background-size auto->px', '#h{background-size:auto}#h.on{background-size:30px 40px}', 'background-size');
await probe('background-size cover->px', '#h{background-size:cover}#h.on{background-size:30px 40px}', 'background-size');
await probe('background-size px auto', '#h{background-size:10px auto}#h.on{background-size:30px auto}', 'background-size');
await probe('background-size layers', '#h{background:linear-gradient(red,blue) no-repeat,linear-gradient(green,blue) no-repeat;background-size:10px 10px,20px 20px}#h.on{background-size:30px 30px,40px 40px}', 'background-size');
// length-percentage through calc()
await probe('width px->percent', '#h{width:100px}#h.on{width:50%}', 'width');
await probe('padding-left px->percent', '#h{padding-left:10px}#h.on{padding-left:10%}', 'padding-left');
await probe('translate px->percent', '#h{translate:10px}#h.on{translate:50%}', 'translate');

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: 50, duration_ms: 100, timing: 'linear', cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
