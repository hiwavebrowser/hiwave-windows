// Row ANIM-READ-JUMP: a CSS transition (or animation) read at fixed times, with and
// without `getAnimations()` reads between the samples. Chrome's values do not depend on
// whether script listed the animations: listing them is a read, it moves nothing.
// Each case toggles `.on` on `#d` (100 ms linear), then at 25, 50 and 75 ms of the
// animation's own time (paused and seeked, so the times are exact) records the computed
// value and what `getAnimations()` lists (`kind:name@currentTime`). The end value is
// read after the animation finished.
// Writes docs/diagnostics/2026-11-19/anim_read_jump_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/anim_read_jump_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-19/anim_read_jump_cft148.json');

// [label, css, the property read]
const CASES = [
  ['width transition', '#d{width:0;height:10px;background:blue;transition:width 100ms linear}#d.on{width:100px}', 'width'],
  ['color transition', '#d{width:10px;height:10px;color:rgb(0,0,0);transition:color 100ms linear}#d.on{color:rgb(200,100,0)}', 'color'],
  ['opacity transition', '#d{width:10px;height:10px;opacity:0;transition:opacity 100ms linear}#d.on{opacity:1}', 'opacity'],
  ['transform transition', '#d{width:10px;height:10px;transform:translateX(0px);transition:transform 100ms linear}#d.on{transform:translateX(100px)}', 'transform'],
  ['width animation', '@keyframes grow{from{width:0}to{width:100px}}#d{width:0;height:10px}#d.on{animation:grow 100ms linear forwards}', 'width'],
];
const TIMES = [25, 50, 75];

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
const page = await browser.newPage({ viewport: { width: 400, height: 200 } });
const cases = [];
for (const [label, css, prop] of CASES) {
  await page.setContent(`<!doctype html><style>body{margin:0}${css}</style><div id=d></div>`);
  const got = await page.evaluate(({ prop, times }) => {
    const d = document.getElementById('d');
    getComputedStyle(d).getPropertyValue(prop);
    d.classList.add('on');
    getComputedStyle(d).getPropertyValue(prop);
    const anim = d.getAnimations()[0];
    anim.pause();
    const list = () => d.getAnimations().map((a) =>
      (a.transitionProperty ? 'transition:' + a.transitionProperty : 'animation:' + a.animationName) + '@' + a.currentTime).join();
    const at = {};
    const listed = {};
    for (const t of times) {
      anim.currentTime = t;
      at[t] = getComputedStyle(d).getPropertyValue(prop);
      listed[t] = list();
    }
    anim.finish();
    return { at, listed, end: getComputedStyle(d).getPropertyValue(prop), after_end: d.getAnimations().length };
  }, { prop, times: TIMES });
  console.log(label, JSON.stringify(got));
  cases.push({ label, css, prop, ...got });
}
// #1359 R1: inside an animation frame, the callback's time is the timeline's time.
await page.setContent('<!doctype html><div id=d></div>');
const raf = await page.evaluate(() => new Promise((resolve) => {
  const out = [];
  function step(t) {
    out.push(t === document.timeline.currentTime);
    if (out.length < 5) requestAnimationFrame(step); else resolve(out);
  }
  requestAnimationFrame(step);
}));
console.log('raf t === timeline', JSON.stringify(raf));
await browser.close();
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, duration_ms: 100, times: TIMES, cases, raf_t_is_timeline: raf }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
