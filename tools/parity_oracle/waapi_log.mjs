// What the oracle's Chrome reports for the cases of the engine's
// web_animations_tests (Element.animate, S8): the r4 fade-in hero, fill
// forwards and none, cancel, the finish event order, and the keyframe and
// control shapes.
// usage: node tools/parity_oracle/waapi_log.mjs
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const STYLE = '<style>body{margin:0;background:#fff}#n{height:68px;background:#eee}' +
  '#h{height:300px;background:#123;color:#fff;font:40px sans-serif}#b{height:20px;background:#0a0}</style>' +
  '<div id=n></div><div id=h style="opacity:0">Placeholder hero</div><div id=b></div>';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
console.log('chrome ' + browser.version());
const wait = (ms) => page.waitForTimeout(ms);
const ev = (f, arg) => page.evaluate(f, arg);

// r4: the hero after the animation's 200 ms.
await page.setContent('<!doctype html>' + STYLE + "<script>var h=document.getElementById('h');" +
  "try{h.animate([{opacity:0},{opacity:1}],200).finished.then(()=>{h.style.opacity=''})}catch(e){window.__err=String(e)}</script>");
await wait(400);
console.log('r4: ' + await ev(() => [String(window.__err), JSON.stringify(h.style.opacity), getComputedStyle(h).opacity].join(' | ')));

const start = (fill) => page.setContent('<!doctype html>' + STYLE).then(() => ev((fill) => {
  window.h = document.getElementById('h'); window.log = [];
  window.a = h.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 200, fill });
  a.finished.then((x) => log.push('resolved:' + (x === a)), (e) => log.push('rejected:' + e.name));
  return a.playState;
}, fill));

console.log('forwards start: ' + await start('forwards'));
await wait(100);
console.log('forwards mid: ' + await ev(() => getComputedStyle(h).opacity));
await wait(300);
console.log('forwards end: ' + await ev(() => [a.playState, log.join(), getComputedStyle(h).opacity, JSON.stringify(h.getAttribute('style'))].join(' | ')));

await start('none');
await wait(400);
console.log('none end: ' + await ev(() => [a.playState, getComputedStyle(h).opacity, h.style.opacity].join(' | ')));

await start('forwards');
await ev(() => { a.oncancel = (e) => log.push('cancel:' + e.type); });
await wait(100);
await ev(() => a.cancel());
await wait(50);
console.log('cancel: ' + await ev(() => [a.playState, log.join(), getComputedStyle(h).opacity, h.style.opacity].join(' | ')));

await start('none');
await ev(() => {
  a.onfinish = function (e) { log.push('on:' + e.type + ':' + (this === a)); };
  a.addEventListener('finish', (e) => log.push('listener:' + (e.target === a)));
});
await wait(400);
console.log('finish order: ' + await ev(() => log.join()));

await page.setContent('<!doctype html>' + STYLE);
await ev(() => {
  window.h = document.getElementById('h'); window.log = [];
  window.a = h.animate({ transform: ['translateX(0px)', 'translateX(100px)'] }, { duration: 1000, delay: 100, easing: 'linear' });
  a.ready.then(() => log.push('ready'));
  window.counts = h.getAnimations().length + ',' + document.getAnimations().length;
});
await wait(600);
console.log('shapes: counts ' + await ev(() => counts) + ' | transform at ~600ms ' + await ev(() => getComputedStyle(h).transform));
await ev(() => { a.pause(); window.held = a.currentTime; });
await wait(300);
console.log('paused: ' + await ev(() => a.playState + ',' + (a.currentTime === held)));
console.log('finish(): ' + await ev(() => { a.finish(); return a.playState + ',' + a.currentTime; }));
await wait(50);
console.log('after: ' + await ev(() => [log.join(), h.getAnimations().length, JSON.stringify(h.getAttribute('style'))].join(' | ')));

await browser.close();
