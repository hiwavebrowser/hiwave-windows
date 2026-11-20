// What the pinned Chrome reports for transitions of `@property` registered
// custom properties (group F of the CSS-transitions brief): the value at
// currentTime 0, 1, 50 and 99 of a 100ms linear transition (paused and
// seeked, no timers) for each syntax, inherits true / false, an
// unregistered property (discrete), CSS.registerProperty() from script, and
// a gradient that reads a registered colour (its painted pixel at 50ms).
// #h is a 100x50 box holding #c, a 50x20 child.
// Writes docs/diagnostics/2026-11-20/css_transitions_property_cft148.json.
// usage: PARITY_CHROME_PATH=... node tools/parity_oracle/css_transitions_property_log.mjs
import { chromium } from 'playwright';
import { writeFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)),
  '../../docs/diagnostics/2026-11-20/css_transitions_property_cft148.json');

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

// `css` styles #h (the element the class goes on) and #c; `readId` and
// `prop` are what is read back; `script` runs before the first style.
async function probe(label, css, readId, prop, script = '') {
  await page.setContent('<!doctype html><style>body{margin:0}#h{width:100px;height:50px}' +
    '#c{width:50px;height:20px}' + css + '</style>' +
    (script ? '<script>' + script + '</script>' : '') + '<div id=h><div id=c></div></div>');
  await page.evaluate(() => document.body.offsetWidth);
  const out = await page.evaluate(async ([readId, prop]) => {
    const el = document.getElementById('h');
    const target = document.getElementById(readId);
    const events = window.__events = [];
    for (const t of ['transitionrun', 'transitionstart', 'transitionend', 'transitioncancel']) {
      el.addEventListener(t, (e) => { if (e.target === el) events.push(e.type.slice(10) + ':' + e.propertyName); });
    }
    const before = getComputedStyle(target).getPropertyValue(prop);
    el.classList.add('on');
    getComputedStyle(target).getPropertyValue(prop);
    const anims = document.getAnimations();
    const at = {};
    for (const t of [0, 1, 50, 99]) {
      anims.forEach((a) => { a.pause(); a.currentTime = t; });
      at[t] = getComputedStyle(target).getPropertyValue(prop);
    }
    const names = anims.map((a) => a.effect.target.id + ':' + a.transitionProperty);
    anims.forEach((a) => { a.currentTime = 50; });
    return { before, at, transitions: names };
  }, [readId, prop]);
  const shot = await page.screenshot({ clip: { x: 0, y: 0, width: 100, height: 50 } });
  const px = await page.evaluate(async (b64) => {
    const img = new Image();
    img.src = 'data:image/png;base64,' + b64;
    await img.decode();
    const c = document.createElement('canvas');
    c.width = 100; c.height = 50;
    const ctx = c.getContext('2d');
    ctx.drawImage(img, 0, 0);
    const at = (x, y) => Array.from(ctx.getImageData(x, y, 1, 1).data).join(',');
    return { left: at(2, 40), centre: at(50, 40), right: at(97, 40) };
  }, shot.toString('base64'));
  const end = await page.evaluate(async ([readId, prop]) => {
    document.getAnimations().forEach((a) => a.finish());
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    return { end: getComputedStyle(document.getElementById(readId)).getPropertyValue(prop), events: window.__events };
  }, [readId, prop]);
  console.log(`${label}: before ${out.before} | ${JSON.stringify(out.at)} | painted at 50 ${JSON.stringify(px)} | ` +
    `anims [${out.transitions}] | events [${end.events}] | end ${end.end}`);
  cases.push({ label, css, script, read: readId + ' ' + prop, ...out, painted_at_50: px, ...end });
}

const reg = (name, syntax, inherits, initial) =>
  `@property ${name}{syntax:'${syntax}';inherits:${inherits};initial-value:${initial}}`;
const T = (p) => `transition:${p} 100ms linear`;

// One per syntax.
await probe('<color>', reg('--x', '<color>', false, 'black') + `#h{--x:rgb(0,0,255);${T('--x')}}#h.on{--x:red}`, 'h', '--x');
await probe('<length>', reg('--x', '<length>', false, '0px') + `#h{--x:10px;${T('--x')}}#h.on{--x:110px}`, 'h', '--x');
await probe('<length> em to px', reg('--x', '<length>', false, '0px') + `#h{font-size:10px;--x:1em;${T('--x')}}#h.on{--x:30px}`, 'h', '--x');
await probe('<length-percentage>', reg('--x', '<length-percentage>', false, '0px') + `#h{--x:10px;${T('--x')}}#h.on{--x:50%}`, 'h', '--x');
await probe('<number>', reg('--x', '<number>', false, '0') + `#h{--x:0;${T('--x')}}#h.on{--x:10}`, 'h', '--x');
await probe('<integer>', reg('--x', '<integer>', false, '0') + `#h{--x:0;${T('--x')}}#h.on{--x:10}`, 'h', '--x');
await probe('<percentage>', reg('--x', '<percentage>', false, '0%') + `#h{--x:0%;${T('--x')}}#h.on{--x:100%}`, 'h', '--x');
await probe('<angle>', reg('--x', '<angle>', false, '0deg') + `#h{--x:0deg;${T('--x')}}#h.on{--x:1turn}`, 'h', '--x');
await probe('<length>+ list', reg('--x', '<length>+', false, '0px') + `#h{--x:0px 0px;${T('--x')}}#h.on{--x:10px 20px}`, 'h', '--x');
await probe('<color># list', reg('--x', '<color>#', false, 'black') + `#h{--x:blue, red;${T('--x')}}#h.on{--x:red, blue}`, 'h', '--x');
await probe('* syntax (discrete)', reg('--x', '*', false, '0px') + `#h{--x:0px;${T('--x')}}#h.on{--x:100px}`, 'h', '--x');
// Computed values with no transition change.
await probe('<color> computed value of a keyword', reg('--x', '<color>', false, 'black') + '#h{--x:red}', 'h', '--x');
await probe('<color> initial value when undeclared', reg('--x', '<color>', false, 'rgb(1, 2, 3)'), 'h', '--x');
await probe('<length> invalid declaration is the initial value', reg('--x', '<length>', false, '7px') + '#h{--x:red}', 'h', '--x');
// Inheritance.
await probe('inherits:true child follows', reg('--x', '<length>', true, '0px') + `#h{--x:0px;${T('--x')}}#h.on{--x:100px}`, 'c', '--x');
await probe('inherits:false child has the initial value', reg('--x', '<length>', false, '5px') + `#h{--x:0px;${T('--x')}}#h.on{--x:100px}`, 'c', '--x');
// Unregistered: discrete, no transition, no events.
await probe('unregistered (discrete)', `#h{--u:0px;${T('--u')}}#h.on{--u:100px}`, 'h', '--u');
await probe('unregistered, transition all', `#h{--u:0px;${T('all')}}#h.on{--u:100px}`, 'h', '--u');
// `all` covers a registered property.
await probe('registered, transition all', reg('--x', '<length>', false, '0px') + `#h{--x:0px;${T('all')}}#h.on{--x:100px}`, 'h', '--x');
// A property that reads the registered one moves with it.
await probe('background-color reads a registered colour', reg('--x', '<color>', false, 'black') +
  `#h{--x:rgb(0,0,255);background-color:var(--x);${T('--x')}}#h.on{--x:rgb(255,0,0)}`, 'h', 'background-color');
await probe('width reads a registered length', reg('--x', '<length>', false, '0px') +
  `#h{--x:20px;width:var(--x);${T('--x')}}#h.on{--x:80px}`, 'h', 'width');
// Gradient: the painted pixel at 50ms (both stops move, so the box is one colour).
await probe('gradient of a registered colour', reg('--x', '<color>', false, 'black') +
  `#h{--x:rgb(0,0,255);background-image:linear-gradient(var(--x),var(--x));${T('--x')}}#h.on{--x:rgb(255,0,0)}`, 'h', 'background-image');
// Tailwind v4's gradient stops, as it registers them.
const TW = `@property --tw-gradient-from{syntax:"<color>";inherits:false;initial-value:#0000}` +
  `@property --tw-gradient-to{syntax:"<color>";inherits:false;initial-value:#0000}` +
  `@property --tw-gradient-position{syntax:"*";inherits:false}` +
  `@property --tw-gradient-stops{syntax:"*";inherits:false}`;
await probe('tailwind gradient stops', TW +
  `#h{--tw-gradient-position:to right;--tw-gradient-from:rgb(0,0,255);--tw-gradient-to:rgb(0,0,255);` +
  `--tw-gradient-stops:var(--tw-gradient-position),var(--tw-gradient-from) 0%,var(--tw-gradient-to) 100%;` +
  `background-image:linear-gradient(var(--tw-gradient-stops));` +
  `transition:--tw-gradient-from 100ms linear,--tw-gradient-to 100ms linear}` +
  `#h.on{--tw-gradient-from:rgb(255,0,0);--tw-gradient-to:rgb(0,255,0)}`, 'h', '--tw-gradient-from');
// CSS.registerProperty from script.
await probe('CSS.registerProperty <color>', `#h{--r:rgb(0,0,255);${T('--r')}}#h.on{--r:rgb(255,0,0)}`, 'h', '--r',
  "CSS.registerProperty({name:'--r',syntax:'<color>',inherits:false,initialValue:'black'})");

const registerErrors = await page.evaluate(() => {
  const tryReg = (d) => { try { CSS.registerProperty(d); return 'ok'; } catch (e) { return e.name; } };
  return {
    twice: [tryReg({ name: '--twice', syntax: '<length>', inherits: false, initialValue: '0px' }),
      tryReg({ name: '--twice', syntax: '<length>', inherits: false, initialValue: '0px' })],
    no_dashes: tryReg({ name: 'x', syntax: '<length>', inherits: false, initialValue: '0px' }),
    no_initial: tryReg({ name: '--noinit', syntax: '<length>', inherits: false }),
    bad_initial: tryReg({ name: '--bad', syntax: '<length>', inherits: false, initialValue: 'red' }),
    universal_no_initial: tryReg({ name: '--any', syntax: '*', inherits: false }),
  };
});
console.log('registerProperty errors: ' + JSON.stringify(registerErrors));

await browser.close();
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, sampled_at_ms: [0, 1, 50, 99], duration_ms: 100, timing: 'linear',
  cases, register_property_errors: registerErrors }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
