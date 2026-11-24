// Row CONTROL-LINE-HANG: a line that holds only a button. Each 16px block (Arial, and on
// Windows also Segoe UI and Times New Roman) holds one button; the block's height is the line
// box. Chrome makes the line the button's height whenever the button hangs further below the
// baseline than the block's strut: the button's below-baseline part is its label line's plus its
// bottom padding and border. The same page is the rustkit-engine test
// button_line_height_tests::a_line_holding_only_a_button_is_the_buttons_height (Arial rows).
// Writes docs/diagnostics/2026-11-21/button_line_hang_cft148.json.
//   PARITY_CHROME_PATH=<pinned Chrome for Testing> node tools/parity_oracle/button_line_hang_log.mjs
import { chromium } from 'playwright';
import { writeFileSync, mkdirSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

const PIN = '148.0.7778.216';
const RECEIPT = join(dirname(fileURLToPath(import.meta.url)), '../../docs/diagnostics/2026-11-21/button_line_hang_cft148.json');

const FAMILIES = ['Arial', "'Segoe UI'", "'Times New Roman'"];
// [label, the button's style]
const BUTTONS = [
  ['settings .btn', 'padding:.6rem 1rem;font-size:.85rem;font-weight:500;border:1px solid transparent'],
  ['padding 8px 12px, 14px', 'padding:8px 12px;font-size:14px'],
  ['bare', ''],
  ['bare 16px', 'font-size:16px'],
  // #1414 R1 nit 1: a used height above the intrinsic one centres the label, as `height` does.
  ['min-height 50px', 'min-height:50px'],
  ['settings .btn, min-height 50px', 'padding:.6rem 1rem;font-size:.85rem;font-weight:500;border:1px solid transparent;min-height:50px'],
  ['max-height 10px', 'max-height:10px'],
];
// #1414 R1 nit 2: the <input> buttons, bare and with the settings .btn style.
const INPUTS = [
  ['input type=button', 'button', ''],
  ['input type=submit', 'submit', ''],
  ['input type=reset', 'reset', ''],
  ['input type=button, settings .btn', 'button', 'padding:.6rem 1rem;font-size:.85rem;font-weight:500;border:1px solid transparent'],
];

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  headless: true,
  executablePath: getPinnedExecutablePath({ required: true }),
});
const cases = [];
try {
  if (browser.version() !== PIN) throw new Error(`expected ${PIN}, got ${browser.version()}`);
  const page = await browser.newPage({ viewport: { width: 800, height: 400 } });
  for (const family of FAMILIES) {
    for (const [label, style] of BUTTONS) {
      await page.setContent('<!doctype html><style>*{box-sizing:border-box}body{margin:0}' +
        `div{font:16px ${family};width:400px}</style><div id=l><button id=b style="${style}">Clear Data</button></div>`);
      const got = await page.evaluate(() => {
        const b = document.getElementById('b');
        return {
          line: document.getElementById('l').getBoundingClientRect().height,
          button: b.getBoundingClientRect().height,
          button_font: getComputedStyle(b).fontFamily + ' ' + getComputedStyle(b).fontSize,
        };
      });
      console.log(family, label, JSON.stringify(got));
      cases.push({ family, label, style, ...got });
    }
    for (const [label, type, style] of INPUTS) {
      await page.setContent('<!doctype html><style>*{box-sizing:border-box}body{margin:0}' +
        `div{font:16px ${family};width:400px}</style><div id=l><input id=b type=${type} value="Clear Data" style="${style}"></div>`);
      const got = await page.evaluate(() => {
        const b = document.getElementById('b');
        return {
          line: document.getElementById('l').getBoundingClientRect().height,
          button: b.getBoundingClientRect().height,
          button_font: getComputedStyle(b).fontFamily + ' ' + getComputedStyle(b).fontSize,
        };
      });
      console.log(family, label, JSON.stringify(got));
      cases.push({ family, label, style, input_type: type, ...got });
    }
  }
} finally {
  await browser.close();
}
mkdirSync(dirname(RECEIPT), { recursive: true });
writeFileSync(RECEIPT, JSON.stringify({ chrome: PIN, platform: process.platform, cases }, null, 2) + '\n');
console.log('receipt ' + RECEIPT);
