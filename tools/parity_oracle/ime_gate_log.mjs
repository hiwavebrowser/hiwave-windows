// Keys, dead keys and compositions where nothing editable has focus, and the
// composition's end on blur and on script selection (row CH2.E3, R1 HOLD on
// #918). Two halves:
//  - OS keys: a headed Chromium gets real key events through hwdrive (the
//    real-window tool, tools/real_window/hwdrive.swift, compiled by the
//    caller), so the browser's own NSTextInputContext sees a dead key just
//    as a user's would. Needs the Accessibility grant.
//  - CDP compositions: imeSetComposition, then script's select() /
//    setSelectionRange() or a blur, to see whether the composition ends.
// Prints one line per event: target, type, key, keyCode, isComposing, data.
// usage: node tools/parity_oracle/ime_gate_log.mjs <hwdrive>
import { chromium } from 'playwright';
import { execFileSync } from 'child_process';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const hwdrive = process.argv[2];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const opts = getDeterministicLaunchOptions();
// No keychain prompt for the test profile's safe storage.
const browser = await chromium.launch({ ...opts, args: [...(opts.args || []), '--use-mock-keychain'], headless: !hwdrive });
const page = await browser.newPage();
console.log('chromium ' + browser.version());
await page.setContent(`<p id="p" tabindex="-1">no field</p><input id="i" value="ab"><div id="ce" contenteditable>ab</div>
<script>
window.log = [];
const TYPES = ['focusout', 'blur', 'focusin', 'focus', 'keydown', 'keypress', 'keyup', 'compositionstart', 'compositionupdate', 'compositionend', 'beforeinput', 'input'];
for (const type of TYPES) {
  window.addEventListener(type, (e) => {
    const t = e.target.id || e.target.localName || 'window';
    window.log.push([t, type, e.key === undefined ? '-' : JSON.stringify(e.key),
      e.keyCode === undefined ? '-' : e.keyCode,
      e.isComposing === undefined ? '-' : e.isComposing,
      e.data === undefined ? '-' : JSON.stringify(e.data),
      e.inputType === undefined ? '-' : e.inputType].join(' '));
  }, true);
}
</script>`);
const flush = async (label) => {
  const log = await page.evaluate(() => { const l = window.log; window.log = []; return l; });
  console.log('== ' + label);
  for (const line of log) console.log('  ' + line);
};
const value = (id) => page.evaluate((id) => { const e = document.getElementById(id); return JSON.stringify(e.value ?? e.textContent); }, id);

if (hwdrive) {
  // The newest browser process (not a helper): the one Playwright launched.
  const pid = execFileSync('pgrep', ['-n', '-f', '/Contents/MacOS/(Google Chrome for Testing|Chromium) --'], { encoding: 'utf8' }).trim();
  console.log('pid ' + pid);
  const hw = (...a) => execFileSync(hwdrive, a.map(String), { encoding: 'utf8' });
  await sleep(800);
  // HID keys go to the frontmost app. macOS may refuse to bring a
  // Playwright-launched window forward; then stop rather than type into
  // whatever else is in front.
  const front = JSON.parse(hw('activate', pid));
  await sleep(500);
  if (!front.frontmost) {
    console.log('== OS keys NOT RUN: the browser is not frontmost ' + JSON.stringify(front));
  } else
  for (const [label, focus] of [['no field (body)', () => document.activeElement.blur()],
                                 ['no field (tabindex p)', () => document.getElementById('p').focus()],
                                 ['input', () => { const e = document.getElementById('i'); e.focus(); e.setSelectionRange(2, 2); }],
                                 ['contenteditable', () => { const e = document.getElementById('ce'); e.focus(); getSelection().collapse(e.firstChild, 2); }]]) {
    await page.evaluate(focus);
    await sleep(200);
    await page.evaluate(() => { window.log = []; });
    hw('key', pid, 14, 'alt'); // Option+e: the dead acute on a US layout
    await sleep(300);
    hw('key', pid, 14); // e
    await sleep(400);
    await flush(label + ': Option+e, e; input ' + await value('i') + ' ce ' + await value('ce'));
    hw('key', pid, 38); // j, a plain shortcut key
    await sleep(300);
    await flush(label + ': j');
  }
}

// CDP: a composition, then script moves the selection or the field blurs.
const cdp = await page.context().newCDPSession(page);
const setComp = (text, sel) => cdp.send('Input.imeSetComposition', { text, selectionStart: sel, selectionEnd: sel });
for (const [label, act] of [['select()', () => document.getElementById('i').select()],
                             ['setSelectionRange(0,0)', () => document.getElementById('i').setSelectionRange(0, 0)],
                             ['blur()', () => document.getElementById('i').blur()],
                             ['focus another field', () => document.getElementById('ce').focus()]]) {
  await page.evaluate(() => { const e = document.getElementById('i'); e.value = 'ab'; e.focus(); e.setSelectionRange(2, 2); window.log = []; });
  await setComp('n', 1);
  await flush('cdp mark "n"');
  await page.evaluate(act);
  await flush('cdp then ' + label + ' value ' + await value('i'));
  await setComp('ni', 2);
  await flush('cdp then mark "ni" value ' + await value('i'));
  await cdp.send('Input.insertText', { text: 'X' });
  await flush('cdp then insertText "X" value ' + await value('i'));
}
await browser.close();
