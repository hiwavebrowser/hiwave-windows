// What the pinned Chrome reports for the colour transitions of group A
// (outline-color, text-decoration-color, fill, stroke): the value at the
// midpoint of a 100ms linear transition, read at a fixed currentTime
// (the transition is paused and seeked, no timers), which transitions
// start, and the transition events they fire once finished.
// Writes the receipt docs/diagnostics/2026-11-17/css_transitions_colors_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_colors_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-17/css_transitions_colors_cft148.json');

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

// `css` styles #h; `on` is added to `target` (#h, or #s, the svg rect);
// `read` is read back at currentTime 50 of 100; then the transitions are
// finished and the events that reached the target are listed.
async function probe(label, css, read, target = 'h') {
  await page.setContent('<!doctype html><style>body{margin:0}' + css + '</style>' +
    '<div id=h>text <span id=c>child</span></div>' +
    '<svg width=50 height=50><rect id=s class=r width=50 height=50 /></svg>');
  await page.evaluate(() => document.body.offsetWidth);
  const out = await page.evaluate(async ([read, target]) => {
    const el = document.getElementById(target);
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
  }, [read, target]);
  console.log(`${label}: ${out.mid} | anims [${out.transitions}] | events [${out.events}] | end ${out.end}`);
  cases.push({ label, css, read, target, ...out });
}

const T = 'transition:all 100ms linear;';
await probe('outline-color rgb', `#h{outline:2px solid rgb(0,0,255);${T}}#h.on{outline-color:rgb(255,0,0)}`, 'outline-color');
await probe('outline-color currentcolor->red', `#h{color:rgb(0,0,255);outline:2px solid;${T}}#h.on{outline-color:rgb(255,0,0)}`, 'outline-color');
await probe('outline-color red->currentcolor', `#h{color:rgb(0,0,255);outline:2px solid rgb(255,0,0);${T}}#h.on{outline-color:currentcolor}`, 'outline-color');
await probe('outline-color currentcolor, color jumps', `#h{color:rgb(0,0,255);outline:2px solid;transition:outline-color 100ms linear}#h.on{color:rgb(255,0,0)}`, 'outline-color');
await probe('outline-color transparent->red', `#h{outline:2px solid transparent;${T}}#h.on{outline-color:rgb(255,0,0)}`, 'outline-color');
await probe('outline shorthand named', `#h{outline:2px solid rgb(0,0,255);transition:outline 100ms linear}#h.on{outline-color:rgb(255,0,0)}`, 'outline-color');
await probe('text-decoration-color rgb', `#h{text-decoration:underline;text-decoration-color:rgb(0,0,255);${T}}#h.on{text-decoration-color:rgb(255,0,0)}`, 'text-decoration-color');
await probe('text-decoration-color transparent->red', `#h{text-decoration:underline;text-decoration-color:transparent;${T}}#h.on{text-decoration-color:rgb(255,0,0)}`, 'text-decoration-color');
await probe('text-decoration shorthand named', `#h{text-decoration:underline;text-decoration-color:rgb(0,0,255);transition:text-decoration 100ms linear}#h.on{text-decoration-color:rgb(255,0,0)}`, 'text-decoration-color');
await probe('fill rgb', `.r{fill:rgb(0,0,255);${T}}.r.on{fill:rgb(255,0,0)}`, 'fill', 's');
await probe('fill none->red', `.r{fill:none;${T}}.r.on{fill:rgb(255,0,0)}`, 'fill', 's');
await probe('stroke rgb', `.r{stroke:rgb(0,0,255);${T}}.r.on{stroke:rgb(255,0,0)}`, 'stroke', 's');
await probe('stroke none->red', `.r{stroke:none;${T}}.r.on{stroke:rgb(255,0,0)}`, 'stroke', 's');

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: 50, duration_ms: 100, timing: 'linear', cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
