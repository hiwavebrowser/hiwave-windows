// What the pinned Chrome reports for transitions of group D of the
// CSS-transitions brief (box-shadow, filter): the value at currentTime 50
// of a 100ms linear transition (paused and seeked, no timers), which
// transitions run, and their events once finished.
// Writes docs/diagnostics/2026-11-17/css_transitions_lists_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_lists_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/css_transitions_lists_cft148.json');

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

// box-shadow
await probe('box-shadow px+color', '#h{box-shadow:0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:10px 20px 30px 4px rgb(255,0,0)}', 'box-shadow');
await probe('box-shadow none->shadow', '#h{box-shadow:none}#h.on{box-shadow:10px 20px 30px 4px rgb(255,0,0)}', 'box-shadow');
await probe('box-shadow shadow->none', '#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:none}', 'box-shadow');
await probe('box-shadow 1->2 shadows', '#h{box-shadow:10px 10px 0px 0px rgb(255,0,0)}#h.on{box-shadow:20px 20px 0px 0px rgb(255,0,0),4px 4px 0px 0px rgb(0,0,255)}', 'box-shadow');
await probe('box-shadow inset->inset', '#h{box-shadow:inset 0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:inset 10px 10px 10px 2px rgb(255,0,0)}', 'box-shadow');
await probe('box-shadow inset mismatch', '#h{box-shadow:0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:inset 10px 10px 10px 2px rgb(255,0,0)}', 'box-shadow');
await probe('box-shadow currentcolor', '#h{color:rgb(0,0,255);box-shadow:0px 0px 0px 0px}#h.on{box-shadow:10px 10px 0px 0px rgb(255,0,0)}', 'box-shadow');
await probe('box-shadow tailwind ring', '#h{box-shadow:0 0 0 0px rgba(59,130,246,0.5)}#h.on{box-shadow:0 0 0 4px rgba(59,130,246,0.5)}', 'box-shadow');
// box-shadow CSS-wide keywords (R1 on #1290): each replaces the list
await probe('box-shadow initial', '#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:initial}', 'box-shadow');
await probe('box-shadow unset', '#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:unset}', 'box-shadow');
await probe('box-shadow revert', '#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:revert}', 'box-shadow');
await probe('box-shadow revert-layer', '#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:revert-layer}', 'box-shadow');
await probe('box-shadow inherit', 'body{box-shadow:10px 20px 30px 4px rgb(255,0,0),2px 2px 0px 0px rgb(0,0,255)}#h.on{box-shadow:inherit}', 'box-shadow');
await probe('box-shadow inherit none', 'body{box-shadow:none}#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:inherit}', 'box-shadow');
// revert-layer inside a layer rolls back to the earlier layer's shadow (R1 on #1290)
await probe('box-shadow revert-layer in a layer', '@layer a{#h.on{box-shadow:10px 20px 30px 4px rgb(255,0,0)}}@layer b{#h{box-shadow:0px 0px 0px 0px rgb(0,0,255)}#h.on{box-shadow:revert-layer}}', 'box-shadow');
await probe('box-shadow revert-layer to no earlier layer', '@layer a{#h{color:red}}@layer b{#h{box-shadow:10px 20px 30px 4px rgb(255,0,0)}#h.on{box-shadow:revert-layer}}', 'box-shadow');
// filter
await probe('filter blur', '#h{filter:blur(0px)}#h.on{filter:blur(10px)}', 'filter');
await probe('filter none->blur', '#h{filter:none}#h.on{filter:blur(10px)}', 'filter');
await probe('filter blur->none', '#h{filter:blur(10px)}#h.on{filter:none}', 'filter');
await probe('filter none->grayscale', '#h{filter:none}#h.on{filter:grayscale(1)}', 'filter');
await probe('filter brightness', '#h{filter:brightness(1)}#h.on{filter:brightness(2)}', 'filter');
await probe('filter two functions', '#h{filter:blur(0px) brightness(1)}#h.on{filter:blur(10px) brightness(2)}', 'filter');
await probe('filter prefix list', '#h{filter:blur(0px)}#h.on{filter:blur(10px) brightness(2)}', 'filter');
await probe('filter mismatch', '#h{filter:blur(10px)}#h.on{filter:grayscale(1)}', 'filter');
await probe('filter drop-shadow', '#h{filter:drop-shadow(0px 0px 0px rgb(0,0,255))}#h.on{filter:drop-shadow(10px 10px 4px rgb(255,0,0))}', 'filter');
await probe('filter hue-rotate', '#h{filter:hue-rotate(0deg)}#h.on{filter:hue-rotate(180deg)}', 'filter');
await probe('filter opacity/saturate/contrast/invert/sepia', '#h{filter:opacity(1) saturate(1) contrast(1) invert(0) sepia(0)}#h.on{filter:opacity(0) saturate(2) contrast(2) invert(1) sepia(1)}', 'filter');
await probe('backdrop-filter blur', '#h{backdrop-filter:blur(0px)}#h.on{backdrop-filter:blur(10px)}', 'backdrop-filter');

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: 50, duration_ms: 100, timing: 'linear', cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
