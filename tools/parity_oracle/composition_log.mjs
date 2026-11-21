// Composition (IME / dead key) event order in an <input> and a <textarea>
// (row CH2.E3). macOS hands a browser marked text through NSTextInputClient:
// `setMarkedText:` while a dead key or an IME composes, `insertText:` to
// commit, `unmarkText` to keep what is marked. Chromium turns those into the
// renderer's ImeSetComposition / ImeCommitText / ImeFinishComposingText,
// the same calls CDP's Input.imeSetComposition and Input.insertText drive, so
// this log is the oracle for the engine's composition events.
// Prints one line per event: type, data, inputType, isComposing, value.
// usage: node tools/parity_oracle/composition_log.mjs
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage();
console.log('chromium ' + browser.version());
await page.setContent(`<input id="i" value="ab"><textarea id="t">ab</textarea>
<script>
window.log = [];
for (const el of [document.getElementById('i'), document.getElementById('t')]) {
  for (const type of ['keydown', 'keyup', 'compositionstart', 'compositionupdate', 'compositionend', 'beforeinput', 'input']) {
    el.addEventListener(type, (e) => {
      window.log.push([type, e.data === undefined ? '-' : JSON.stringify(e.data),
        e.inputType === undefined ? '-' : e.inputType,
        e.isComposing === undefined ? '-' : e.isComposing,
        JSON.stringify(el.value), el.selectionStart + ',' + el.selectionEnd,
        e.constructor.name, e.cancelable].join(' '));
    });
  }
}
</script>`);
const cdp = await page.context().newCDPSession(page);
const flush = async (label) => {
  const log = await page.evaluate(() => { const l = window.log; window.log = []; return l; });
  console.log('== ' + label);
  for (const line of log) console.log('  ' + line);
};
const setComp = (text, sel) => cdp.send('Input.imeSetComposition', {
  text, selectionStart: sel, selectionEnd: sel,
});

for (const id of ['i', 't']) {
  // Caret at the end of "ab".
  await page.evaluate((id) => { const e = document.getElementById(id); e.focus(); e.setSelectionRange(2, 2); }, id);

  // Dead key: Option+e marks "´", then e commits "é".
  await setComp('´', 1);
  await flush(id + ' dead key: setMarkedText "´"');
  await cdp.send('Input.insertText', { text: 'é' });
  await flush(id + ' dead key: insertText "é"');

  // IME: two updates, then commit different text.
  await setComp('n', 1);
  await flush(id + ' ime: setMarkedText "n"');
  await setComp('ni', 2);
  await flush(id + ' ime: setMarkedText "ni"');
  await cdp.send('Input.insertText', { text: '你' });
  await flush(id + ' ime: insertText "你"');

  // IME: marked, then cancelled (setMarkedText with an empty string, as
  // Escape in an IME's candidate window does): the marked text goes.
  await setComp('x', 1);
  await flush(id + ' ime: setMarkedText "x"');
  await setComp('', 0);
  await flush(id + ' ime: setMarkedText "" (cancel)');

  // unmarkText: Chromium's ImeFinishComposingText keeps the marked text.
  // CDP has no verb for it; a blur finishes the composition the same way.
  await setComp('k', 1);
  await flush(id + ' ime: setMarkedText "k"');
  await page.evaluate(() => document.activeElement.blur());
  await flush(id + ' ime: finish composing (blur) keeps "k"');
  await page.evaluate((id) => { const e = document.getElementById(id); e.focus(); e.setSelectionRange(e.value.length, e.value.length); }, id);

  // Plain insertText with no composition (a press-and-hold accent pick
  // after the popover replaced nothing, emoji, dictation).
  await cdp.send('Input.insertText', { text: 'z' });
  await flush(id + ' insertText "z" with nothing marked');
}
await browser.close();
