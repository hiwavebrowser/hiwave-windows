// What the pinned Chrome reports for fill / stroke transitions on SVG
// elements (A2 of the CSS-transitions brief): the value at currentTime 25,
// 50 and 75 of a 100ms linear transition (paused and seeked, no timers),
// the painted pixel at the shape's centre, which transitions run (and on
// which element), and the events (with their target) once finished.
// #s is a 100x100 inline svg holding #p, a 100x100 rect with stroke-width 20.
// Writes docs/diagnostics/2026-11-17/css_transitions_svg_paint_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_svg_paint_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-17/css_transitions_svg_paint_cft148.json');

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

// `toggle` is the element the class goes on; `read` the element and the
// property read back.
async function probe(label, css, toggle, readId, prop, attrs = '') {
  await page.setContent('<!doctype html><style>body{margin:0}' + css + '</style>' +
    '<svg id=s width=100 height=100><defs><linearGradient id=g><stop offset=0 stop-color=lime /></linearGradient></defs>' +
    '<rect id=p x=10 y=10 width=80 height=80 stroke-width=20 ' + attrs + ' /></svg>');
  await page.evaluate(() => document.body.offsetWidth);
  const out = await page.evaluate(async ([toggle, readId, prop]) => {
    const el = document.getElementById(toggle);
    const target = document.getElementById(readId);
    const events = window.__events = [];
    for (const id of ['s', 'p']) {
      for (const t of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel']) {
        document.getElementById(id).addEventListener(t, (e) => {
          if (e.target === e.currentTarget) events.push(id + ':' + e.type.slice(10) + ':' + e.propertyName);
        });
      }
    }
    el.classList.add('on');
    getComputedStyle(target).getPropertyValue(prop);
    const anims = document.getAnimations();
    const at = {};
    for (const t of [25, 50, 75]) {
      anims.forEach((a) => { a.pause(); a.currentTime = t; });
      at[t] = getComputedStyle(target).getPropertyValue(prop);
    }
    const names = anims.map((a) => a.effect.target.id + ':' + a.transitionProperty);
    anims.forEach((a) => { a.currentTime = 50; });
    return { at, transitions: names };
  }, [toggle, readId, prop]);
  // The painted centre (fill) and the stroke band at the midpoint.
  const shot = await page.screenshot({ clip: { x: 0, y: 0, width: 100, height: 100 } });
  const px = await page.evaluate(async (b64) => {
    const img = new Image();
    img.src = 'data:image/png;base64,' + b64;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = 100; c.height = 100;
    const ctx = c.getContext('2d');
    ctx.drawImage(img, 0, 0);
    const at = (x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data).join(',');
    return { centre: at(50, 50), stroke: at(12, 50) };
  }, shot.toString('base64'));
  const end = await page.evaluate(async ([readId, prop]) => {
    document.getAnimations().forEach((a) => a.finish());
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    return { end: getComputedStyle(document.getElementById(readId)).getPropertyValue(prop), events: window.__events };
  }, [readId, prop]);
  console.log(`${label}: ${JSON.stringify(out.at)} | painted at 50 ${JSON.stringify(px)} | anims [${out.transitions}] | ` +
    `events [${end.events}] | end ${end.end}`);
  cases.push({ label, css, toggle, read: readId + ' ' + prop, attrs, ...out, painted_at_50: px, ...end });
}

const T = 'transition:all 100ms linear';
await probe('fill blue->red', `#p{fill:blue;${T}}#p.on{fill:red}`, 'p', 'p', 'fill');
await probe('stroke blue->red', `#p{fill:none;stroke:blue;${T}}#p.on{stroke:red}`, 'p', 'p', 'stroke');
await probe('fill named transition', '#p{fill:blue;transition:fill 100ms linear}#p.on{fill:red}', 'p', 'p', 'fill');
await probe('fill none->red', `#p{fill:none;${T}}#p.on{fill:red}`, 'p', 'p', 'fill');
await probe('fill red->none', `#p{fill:red;${T}}#p.on{fill:none}`, 'p', 'p', 'fill');
await probe('fill url->red', `#p{fill:url(#g);${T}}#p.on{fill:red}`, 'p', 'p', 'fill');
await probe('fill attr->css', `#p{${T}}#p.on{fill:red}`, 'p', 'p', 'fill', 'fill=blue');
await probe('fill on svg, inherited by rect', `#s{fill:blue;${T}}#s.on{fill:red}`, 's', 'p', 'fill');
await probe('fill on svg read on svg', `#s{fill:blue;${T}}#s.on{fill:red}`, 's', 's', 'fill');
await probe('fill currentcolor, color moves', `#s{color:blue;${T}}#s.on{color:red}#p{fill:currentcolor}`, 's', 'p', 'fill');
await probe('hover-like: class on svg, rule on rect', `#p{fill:blue;${T}}#s.on #p{fill:red}`, 's', 'p', 'fill');

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: [25, 50, 75], duration_ms: 100, timing: 'linear', cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
