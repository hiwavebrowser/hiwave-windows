// CH2.E12: what the oracle's Chrome does with the selection of a filled
// <input>: a click, then Cmd/Ctrl+A, then typing; a focus handler that
// calls select(); setSelectionRange from script; the caret a click places.
// The values the engine tests in text_control_select_tests.rs expect.
// usage: node tools/parity_oracle/text_control_select_log.mjs
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const STYLE = 'position:absolute;left:10px;width:300px;height:30px;margin:0;padding:0;border:0;font-family:Menlo;font-size:20px';
const PAGE = `<html><body style="margin:0">
<input id="a" value="old-value" style="${STYLE};top:10px">
<input id="b" value="old-value" style="${STYLE};top:60px">
<input id="c" value="old-value" style="${STYLE};top:110px">
<input id="d" value="old-value" style="${STYLE};top:160px">
<script>document.getElementById('b').addEventListener('focus', function () { this.select(); });</script>
</body></html>`;

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(PAGE);
console.log('chrome ' + browser.version());
const sel = (id) => page.evaluate((i) => {
  const e = document.getElementById(i);
  return `${e.selectionStart},${e.selectionEnd}`;
}, id);
const val = (id) => page.inputValue('#' + id);
const reset = () => page.evaluate(() => {
  document.activeElement && document.activeElement.blur();
  for (const i of ['a', 'b', 'c', 'd']) document.getElementById(i).value = 'old-value';
});
const adv = await page.evaluate(() => {
  const c = document.createElement('canvas').getContext('2d');
  c.font = '20px Menlo';
  return c.measureText('o').width;
});
console.log(`advance ${adv}`);

// (a) click near the end of a filled input, Cmd+A, type.
await reset();
await page.mouse.click(250, 25);
console.log(`a click: sel=${await sel('a')}`);
await page.keyboard.press('ControlOrMeta+a');
console.log(`a cmd+a: sel=${await sel('a')}`);
await page.keyboard.type('new');
console.log(`a typed: value=${JSON.stringify(await val('a'))}`);

// (b) a focus handler calls select(): by script focus, then by a click.
await reset();
await page.evaluate(() => document.getElementById('b').focus());
console.log(`b script focus: sel=${await sel('b')}`);
await page.keyboard.type('new');
console.log(`b script focus typed: value=${JSON.stringify(await val('b'))}`);
await reset();
await page.mouse.click(40, 75);
console.log(`b click: sel=${await sel('b')}`);
await page.keyboard.type('new');
console.log(`b click typed: value=${JSON.stringify(await val('b'))}`);

// (c) setSelectionRange(2, 5) from script, then type.
await reset();
await page.evaluate(() => { const c = document.getElementById('c'); c.focus(); c.setSelectionRange(2, 5); });
console.log(`c range: sel=${await sel('c')}`);
await page.keyboard.type('X');
console.log(`c typed: value=${JSON.stringify(await val('c'))} sel=${await sel('c')}`);
// select() then read, without focus.
await reset();
await page.evaluate(() => document.getElementById('c').select());
console.log(`c select() unfocused: sel=${await sel('c')}`);

// selectionStart/End follow the user's caret: arrows, Shift+arrow.
await reset();
await page.evaluate(() => document.getElementById('c').focus());
await page.keyboard.press('End');
await page.keyboard.press('ArrowLeft');
await page.keyboard.press('ArrowLeft');
console.log(`c left left: sel=${await sel('c')}`);
await page.keyboard.press('Shift+ArrowLeft');
console.log(`c shift+left: sel=${await sel('c')}`);
await page.keyboard.type('ab');
console.log(`c typed ab: value=${JSON.stringify(await val('c'))} sel=${await sel('c')}`);

// (d) the caret a click places.
for (const f of [0.2, 1.4, 3.3, 3.7, 8.6, 12]) {
  await reset();
  await page.mouse.click(10 + f * adv, 175);
  console.log(`d click x=10+${f}*adv: sel=${await sel('d')}`);
}
await reset();
await page.mouse.click(10 + 3.3 * adv, 175);
await page.keyboard.type('Z');
console.log(`d click 3.3 typed: value=${JSON.stringify(await val('d'))}`);
await browser.close();
