// Chrome's `Audio` named constructor (HTML §4.8.10.1, AUDIO-CTOR): the receipt for
// rustkit-bindings' `audio_ctor` tests. YouTube's kevlar_base runs `new Audio` in
// loadVoiceFeedbackAudio.
// Usage: PARITY_CHROME_PATH=<Chrome for Testing 148.0.7778.216> node audio_ctor_log.mjs
import { chromium } from 'playwright';

const WANT = '148.0.7778.216';
if (!process.env.PARITY_CHROME_PATH) {
  console.error('PARITY_CHROME_PATH must name Chrome for Testing ' + WANT);
  process.exit(2);
}
const browser = await chromium.launch({ executablePath: process.env.PARITY_CHROME_PATH });
if (browser.version() !== WANT) {
  console.error('expected Chrome ' + WANT + ', got ' + browser.version());
  await browser.close();
  process.exit(2);
}
console.log('chrome ' + browser.version());
const page = await browser.newPage();
// A real document URL so relative `src` resolves against it.
await page.route('http://oracle.test/**', (r) =>
  r.fulfill({ contentType: 'text/html', body: '<!doctype html><body></body>' }));
await page.goto('http://oracle.test/dir/page.html');
const out = await page.evaluate(() => {
  const r = {};
  const a = new Audio();
  r.instanceofAudio = a instanceof HTMLAudioElement;
  r.instanceofMedia = a instanceof HTMLMediaElement;
  r.tagName = a.tagName;
  r.preload = a.preload;
  r.preloadAttr = a.getAttribute('preload');
  r.srcEmpty = a.src;
  r.hasSrcAttr = a.hasAttribute('src');
  r.isConnected = a.isConnected;
  r.parentNode = String(a.parentNode);
  r.inDocument = document.getElementsByTagName('audio').length;
  r.outerHTML = a.outerHTML;
  const b = new Audio('sounds/x.mp3');
  r.srcResolved = b.src;
  r.srcAttr = b.getAttribute('src');
  r.outerHTMLSrc = b.outerHTML;
  r.undefinedArg = new Audio(undefined).hasAttribute('src');
  r.nullArg = new Audio(null).getAttribute('src');
  r.distinct = new Audio() !== new Audio();
  r.protoIsAudio = Audio.prototype === HTMLAudioElement.prototype;
  r.protoOf = Object.getPrototypeOf(a) === HTMLAudioElement.prototype;
  r.name = Audio.name;
  r.length = Audio.length;
  r.typeofAudio = typeof Audio;
  r.ownGlobalDesc = JSON.stringify(Object.getOwnPropertyDescriptor(window, 'Audio'), (k, v) => typeof v === 'function' ? 'fn' : v);
  r.constructorProp = a.constructor === HTMLAudioElement;
  try { Audio(); r.callWithoutNew = 'no throw'; } catch (e) { r.callWithoutNew = e.constructor.name + ': ' + e.message; }
  r.paused = a.paused;
  r.readyState = a.readyState;
  r.networkState = a.networkState;
  return r;
});
for (const [k, v] of Object.entries(out)) console.log(k + ': ' + JSON.stringify(v));
await browser.close();
