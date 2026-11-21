#!/usr/bin/env node
/**
 * canvas_shim_log.mjs - Chrome's answers for the canvas 2D members' argument
 * and receiver checks (CANVAS-SHIM, web_canvas_tests.rs
 * `canvas_members_keep_their_argument_and_receiver_checks` and
 * `a_canvas_path_member_converts_only_its_own_arguments`).
 *
 *   PARITY_CHROME_PATH=<pinned Chrome for Testing> node canvas_shim_log.mjs > canvas_shim_chrome.json
 */
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

const browser = await chromium.launch({ ...getDeterministicLaunchOptions(), headless: true });
const page = await browser.newPage();
await page.setContent('<!doctype html><body><canvas id=c></canvas></body>');
const out = await page.evaluate(() => {
  const x = document.getElementById('c').getContext('2d');
  const P = CanvasRenderingContext2D.prototype;
  const msg = (f) => { try { f(); return 'none'; } catch (e) { return e.name + ': ' + e.message; } };
  const checks = {};
  for (const c of [
    'x.arc(1, 2, 3, 4)', 'x.moveTo(1)', 'x.lineTo()', 'x.clearRect(1, 2, 3)', 'x.rotate()', 'x.bezierCurveTo(1, 2, 3, 4, 5)',
    'x.arc(1, 2, 3, 4, undefined)', 'x.beginPath.call({})', 'x.stroke.call(null)', 'x.lineTo.call(5, 1, 2)',
    "Object.getOwnPropertyDescriptor(P, 'lineWidth').set.call({}, 3)",
    "Object.getOwnPropertyDescriptor(P, 'strokeStyle').set.call({}, 'red')",
    // Which check comes first: the receiver, or the argument count?
    'P.arc.call({})', 'P.moveTo.call({}, 1)', 'P.clearRect.call(null)', 'P.translate.call({})',
  ]) checks[c] = msg(() => eval(c));
  let read = 0;
  const o = { valueOf() { read++; return 1; } };
  x.beginPath(o); x.closePath(o, o); x.moveTo(1, 2, o); x.lineTo(1, 2, o, o); x.rect(1, 2, 3, 4, o);
  x.quadraticCurveTo(1, 2, 3, 4, o);
  const pathReads = read;
  read = 0;
  x.save(); x.scale(1, 1, o); x.rotate(0, o); x.translate(0, 0, o); x.restore();
  const transformReads = read;
  const seen = [];
  const n = (v) => ({ valueOf() { seen.push(v); return v; } });
  x.arc(n(1), n(2), n(3), n(4), n(5)); x.lineTo(n(6), n(7)); x.translate(n(8), n(9)); x.lineWidth = n(10);
  x.strokeStyle = 'RED'; x.fillStyle = 'nope'; x.lineWidth = 4; x.lineWidth = -1; x.lineCap = 'round';
  return {
    version: navigator.userAgent.match(/Chrome\/[\d.]+/)[0],
    checks,
    path_member_extra_argument_reads: pathReads,
    transform_member_extra_argument_reads: transformReads,
    conversion_order: seen.join(),
    styles: [x.strokeStyle, x.fillStyle, x.lineWidth, x.lineCap].join(),
  };
});
console.log(JSON.stringify(out, null, 1));
await browser.close();
