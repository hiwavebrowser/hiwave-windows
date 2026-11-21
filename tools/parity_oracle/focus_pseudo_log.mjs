// CH2.E22: what the oracle's Chrome matches for `:focus`, `:focus-within`
// and `:focus-visible` after a click, a Tab and script's focus()/blur():
// the values the engine tests in `focus_pseudo_tests.rs` pin.
// usage: node tools/parity_oracle/focus_pseudo_log.mjs tools/parity_oracle/focus_pseudo_page.html
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch(getDeterministicLaunchOptions());
const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
await page.setContent(readFileSync(process.argv[2], 'utf8'));
const state = () => page.evaluate(() => {
  const $ = (id) => document.getElementById(id);
  const cs = (id, p) => getComputedStyle($(id)).getPropertyValue(p);
  const a = document.activeElement;
  return [
    'active=' + (a ? (a.id || a.nodeName) : 'null'),
    'i.border=' + cs('i', 'border-top-color'),
    'f.bg=' + cs('f', 'background-color'),
    'b.bg=' + cs('b', 'background-color'),
    'b.outline=' + cs('b', 'outline-style') + ' ' + cs('b', 'outline-width'),
    'i:focus-visible=' + $('i').matches(':focus-visible'),
    'b:focus-visible=' + $('b').matches(':focus-visible'),
  ].join(' ');
});
const step = async (label, fn) => { await fn(); console.log(label + ': ' + await state()); };
const rect = async (sel) => page.evaluate((s) => {
  const r = document.querySelector(s).getBoundingClientRect();
  return [r.x + r.width / 2, r.y + r.height / 2];
}, sel);
console.log('chrome ' + browser.version());
await step('at rest', async () => {});
const [ix, iy] = await rect('#i');
const [bx, by] = await rect('#b');
const [dx, dy] = await rect('#d');
await step('click #i', () => page.mouse.click(ix, iy));
await step('click #d (blur)', () => page.mouse.click(dx, dy));
await step('click #b', () => page.mouse.click(bx, by));
await step('click #d', () => page.mouse.click(dx, dy));
await step('Tab', () => page.keyboard.press('Tab'));
await step('Tab', () => page.keyboard.press('Tab'));
await step('Shift+Tab', () => page.keyboard.press('Shift+Tab'));
await step('click #d', () => page.mouse.click(dx, dy));
await step('script b.focus() after a click', () => page.evaluate(() => document.getElementById('b').focus()));
await step('script b.blur()', () => page.evaluate(() => document.getElementById('b').blur()));
await step('click #b', () => page.mouse.click(bx, by));
await step('click #d', () => page.mouse.click(dx, dy));
await step('script b.focus() after a mouse focus', () => page.evaluate(() => document.getElementById('b').focus()));
await step('click #b', () => page.mouse.click(bx, by));
await step('script i.focus() after a mouse focus', () => page.evaluate(() => document.getElementById('i').focus()));
await step('script b.focus() from a focus-visible input', () => page.evaluate(() => document.getElementById('b').focus()));
await page.setContent(readFileSync(process.argv[2], 'utf8'));
await step('fresh page, script b.focus()', () => page.evaluate(() => document.getElementById('b').focus()));
await step('fresh page, script i.focus()', () => page.evaluate(() => document.getElementById('i').focus()));
await browser.close();
