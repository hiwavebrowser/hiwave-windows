// What the oracle's Chrome reports for the cases of the engine's
// waapi_layer_tests (S13c): the style attribute and a MutationObserver
// during an animation and after commitStyles(), getAnimations() with the
// CSS-created objects, playbackRate/reverse/updatePlaybackRate, composite
// add/accumulate, and new Animation(new KeyframeEffect(...)).
// usage: node tools/parity_oracle/waapi_layer_log.mjs
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const page_ = (css, body, script) => '<!doctype html><style>body{margin:0}#h{width:100px;height:100px;background:#123}' +
  css + '</style>' + body + "<script>var h=document.getElementById('h');window.log=[];" + script + '</script>';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
console.log('chrome ' + browser.version());
const wait = (ms) => page.waitForTimeout(ms);
const ev = (f, arg) => page.evaluate(f, arg);

await page.setContent(page_('', '<div id=h style="opacity:0"></div>',
  "window.recs=0;new MutationObserver(function(m){recs+=m.length}).observe(h,{attributes:true});" +
  "window.a=h.animate([{opacity:0},{opacity:1}],{duration:200,fill:'forwards',easing:'linear'});"));
await wait(96);
console.log('observer mid: ' + await ev(() => [getComputedStyle(h).opacity, JSON.stringify(h.getAttribute('style')), recs].join(' | ')));
await wait(300);
console.log('observer end: ' + await ev(() => [getComputedStyle(h).opacity, JSON.stringify(h.getAttribute('style')), recs].join(' | ')));
await ev(() => a.commitStyles());
await wait(16);
console.log('commitStyles: ' + await ev(() => [h.style.opacity, recs].join(' | ')));

await page.setContent(page_("#h{opacity:0;animation:fade 1s linear forwards;transition:width 1s linear}#h.w{width:200px}@keyframes fade{to{opacity:1}}",
  '<div id=h></div>', "h.addEventListener('animationcancel',function(e){log.push('cancel:'+e.animationName)});"));
await wait(16);
console.log('css list: ' + await ev(() => {
  window.a = h.getAnimations()[0];
  return [h.getAnimations().length, document.getAnimations().length, a instanceof CSSAnimation, a instanceof Animation, a.animationName,
    a.playState, typeof a.currentTime, a.effect.target === h, a.effect.getTiming().duration, a.effect.getTiming().fill,
    a.timeline === document.timeline].join();
}));
const t1 = await ev(() => a.currentTime);
await wait(100);
console.log('css currentTime: ' + t1 + ' -> ' + await ev(() => a.currentTime) + ', same object ' + await ev(() => h.getAnimations()[0] === a));
await ev(() => h.classList.add('w'));
await wait(16);
console.log('css order: ' + await ev(() => h.getAnimations().map((x) => x.constructor.name + ':' + (x.transitionProperty || x.animationName)).join()));
await ev(() => a.pause());
await wait(16);
const held = await ev(() => getComputedStyle(h).opacity);
await wait(200);
console.log('css pause: ' + await ev(() => a.playState) + ', held ' + held + ' now ' + await ev(() => getComputedStyle(h).opacity));
await ev(() => a.play());
await wait(100);
console.log('css play: ' + await ev(() => getComputedStyle(h).opacity));
await ev(() => a.cancel());
await wait(32);
console.log('css cancel: ' + await ev(() => [getComputedStyle(h).opacity, log.join(), h.getAnimations().filter((x) => x instanceof CSSAnimation).length].join(' | ')));

await page.setContent(page_('', '<div id=h></div>',
  "window.a=h.animate([{opacity:0},{opacity:1}],{duration:200,fill:'both',easing:'linear'});a.playbackRate=2;"));
await wait(128);
console.log('rate 2: ' + await ev(() => [a.playState, a.currentTime, getComputedStyle(h).opacity].join()));
await ev(() => a.reverse());
await wait(16);
console.log('reverse: ' + await ev(() => [a.playState, a.playbackRate].join()));
await wait(32);
console.log('reverse mid: ' + await ev(() => getComputedStyle(h).opacity));
await wait(100);
console.log('reverse end: ' + await ev(() => [a.playState, a.currentTime, getComputedStyle(h).opacity].join()));
await ev(() => { a.updatePlaybackRate(0.5); a.play(); });
await wait(16);
console.log('rate .5: ' + await ev(() => [a.playbackRate, a.playState].join()));
await wait(100);
console.log('rate .5 mid: ' + await ev(() => getComputedStyle(h).opacity));

await page.setContent(page_('#h{opacity:.5;transform:translateX(100px)}', '<div id=h></div>',
  "window.a=h.animate({transform:['translateX(50px)','translateX(50px)'],opacity:[0.25,0.25]},{duration:100,composite:'add',fill:'forwards'});"));
await wait(150);
console.log('add: ' + await ev(() => [getComputedStyle(h).transform, getComputedStyle(h).opacity].join(' | ')));
await ev(() => { a.cancel(); h.style.transform = 'scale(2)'; window.b = h.animate({ transform: ['scale(2)', 'scale(2)'] }, { duration: 100, composite: 'accumulate', fill: 'forwards' }); });
await wait(150);
console.log('accumulate: ' + await ev(() => [getComputedStyle(h).transform, getComputedStyle(h).opacity].join(' | ')));

await page.setContent(page_('#h{opacity:0}', '<div id=h></div>',
  "window.e=new KeyframeEffect(h,[{opacity:0},{opacity:1}],{duration:100,fill:'forwards',easing:'linear'});window.a=new Animation(e,document.timeline);"));
console.log('ctor: ' + await ev(() => [a.playState, a.effect === e, a.timeline === document.timeline, typeof document.timeline.currentTime, h.getAnimations().length].join()));
await ev(() => a.play());
await wait(50);
console.log('ctor mid: ' + await ev(() => [getComputedStyle(h).opacity, e.getComputedTiming().progress].join(' | ')));
await wait(100);
console.log('ctor end: ' + await ev(() => [a.playState, getComputedStyle(h).opacity].join()));

await browser.close();
