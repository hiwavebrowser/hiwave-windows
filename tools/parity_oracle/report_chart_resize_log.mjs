#!/usr/bin/env node
/**
 * report_chart_resize_log.mjs - Chrome's answers for REPORT-CHART-0x0 (E10b):
 * when a ResizeObserver callback runs relative to the animation frame
 * callbacks already asked for, and what the app's Chart.js 4.4 does with
 * report.html's shape (charts built while `#content` is display:none,
 * shown in the same task, default animation on).
 *
 *   PARITY_CHROME_PATH=<pinned Chrome for Testing> node report_chart_resize_log.mjs \
 *     > docs/diagnostics/2026-11-19/report_chart_resize_cft148.json
 *
 * The pages are the ones in rustkit-engine `load_geometry_read_tests.rs`
 * (`a_resize_observation_waits_for_the_frame_already_asked_for`,
 * `real_chart_js_built_hidden_resizes_both_charts_when_shown`,
 * `a_resize_observation_under_an_endless_frame_loop_is_delivered_in_the_load`); keep them equal.
 * `iframe` is Chrome's answer to whether a frame tree's frame callbacks hold the top document's
 * observations (they do: one rendering update for the tree).
 */
import { chromium } from 'playwright';
import { readFileSync } from 'fs';
import { dirname, join } from 'path';
import { fileURLToPath } from 'url';
import { getDeterministicLaunchOptions, getPinnedExecutablePath } from './deterministic.mjs';

// The campaign pin: this receipt is Chrome for Testing 148.0.7778.216 or nothing (#1361 R1).
const PIN = '148.0.7778.216';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const CHART = readFileSync(join(ROOT, 'crates/hiwave-app/src/ui/chart.umd.min.js'), 'utf8');

// A frame loop asked for before the observed box changes (Chart.js's animator),
// then a ResizeObserver callback that asks for a frame of its own (Chart.js's
// `throttled` resize).
export const ORDER_PAGE = "<!doctype html><html><head><style>body{margin:0} #cc{height:300px}</style></head><body>" +
  "<div id=content style='display:none'><div id=cc></div></div><div id=out>x</div><script>" +
  "var log=[];function L(s){log.push(s);document.getElementById('out').textContent='R:'+log.join(' ');}" +
  "var n=0;function A(){n++;L('A'+n);if(n<3)requestAnimationFrame(A);}" +
  "document.addEventListener('DOMContentLoaded',function(){Promise.resolve().then(function(){" +
  "requestAnimationFrame(A);" +
  "new ResizeObserver(function(es){var r=es[0].contentRect;L('ro='+r.width+'x'+r.height);" +
  "requestAnimationFrame(function(){L('B');});}).observe(document.getElementById('cc'));" +
  "document.getElementById('content').style.display='block';});});" +
  "</script></body></html>";

// R1 (#1361): an endless frame loop (a spinner, a canvas loop) running when the observed box
// is shown: the observation is still delivered, after the loop's frame.
export const ENDLESS_PAGE = "<!doctype html><html><head><style>body{margin:0} #cc{height:300px}</style></head><body>" +
  "<div id=content style='display:none'><div id=cc></div></div><div id=out>x</div><script>" +
  "var frames=0;function A(){frames++;requestAnimationFrame(A);}requestAnimationFrame(A);" +
  "new ResizeObserver(function(es){var r=es[0].contentRect;" +
  "document.getElementById('out').textContent='R:ro='+r.width+'x'+r.height+' after '+(frames>0?'a frame':'no frame');" +
  "}).observe(document.getElementById('cc'));" +
  "document.getElementById('content').style.display='block';" +
  "</script></body></html>";

// R1 nit (#1361): a same-origin iframe's frame loop and the top document's observation are one
// rendering update: does the top document's resize observation come after the iframe's frame
// callbacks of that update?
export const IFRAME_PAGE = "<!doctype html><html><head><style>body{margin:0} #cc{height:300px}</style></head><body>" +
  "<div id=content style='display:none'><div id=cc></div></div><div id=out>x</div>" +
  "<iframe id=f srcdoc='<p>f</p>'></iframe><script>" +
  "var log=[];function L(s){log.push(s);document.getElementById('out').textContent='R:'+log.join(' ');}" +
  "addEventListener('load',function(){var w=document.getElementById('f').contentWindow;" +
  "var n=0;function F(){n++;L('F'+n);if(n<3)w.requestAnimationFrame(F);}w.requestAnimationFrame(F);" +
  "new ResizeObserver(function(es){var r=es[0].contentRect;L('ro='+r.width+'x'+r.height);}).observe(document.getElementById('cc'));" +
  "document.getElementById('content').style.display='block';});" +
  "</script></body></html>";

export function chartPage(chart) {
  return "<!doctype html><html><head><style>body{margin:0;padding:20px}" +
    ".chart-container{position:relative;height:300px}</style></head><body>" +
    "<div id=content style='display:none'><div class=chart-container><canvas id=activityChart></canvas></div>" +
    "<div class=chart-container><canvas id=protectionChart></canvas></div></div>" +
    "<div id=out>x</div><script>" + chart + "</script><script>" +
    // report.html's default period ('today'): one label, its colours (a dataset with colours of
    // its own has no colour animation left to run once the chart is drawn at 0x0).
    "function mk(id,type){return new Chart(document.getElementById(id).getContext('2d'),{type:type," +
    "data:{labels:['Today'],datasets:[{data:[42],borderColor:'#06b6d4',backgroundColor:'rgba(6,182,212,0.1)',fill:true,tension:0.4}," +
    "{data:[12],borderColor:'#10b981',backgroundColor:'rgba(16,185,129,0.1)',fill:true,tension:0.4}]}," +
    "options:{responsive:true,maintainAspectRatio:false,scales:{y:{beginAtZero:true}}}});}" +
    "function sizes(){return ['activityChart','protectionChart'].map(function(i){var c=document.getElementById(i);" +
    "return i+'='+c.width+'x'+c.height;}).join(' ');}" +
    "document.addEventListener('DOMContentLoaded',function(){Promise.resolve().then(function(){" +
    "mk('activityChart','line');mk('protectionChart','bar');" +
    "document.getElementById('content').style.display='block';" +
    "document.getElementById('out').textContent='R:built '+sizes();" +
    "setTimeout(function(){document.getElementById('out').textContent='R:later '+sizes();},300);});});" +
    "</script></body></html>";
}

const browser = await chromium.launch({
  ...getDeterministicLaunchOptions(),
  executablePath: getPinnedExecutablePath({ required: true }),
  headless: true,
});
if (browser.version() !== PIN) {
  await browser.close();
  throw new Error(`not the pinned Chrome: ${browser.version()} (want ${PIN})`);
}
const out = { version: browser.version(), viewport: '800x600' };
for (const [name, html] of [['order', ORDER_PAGE], ['chart', chartPage(CHART)], ['endless', ENDLESS_PAGE], ['iframe', IFRAME_PAGE]]) {
  const page = await browser.newPage({ viewport: { width: 800, height: 600 } });
  await page.setContent(html);
  await page.waitForTimeout(600);
  out[name] = await page.evaluate(() => document.getElementById('out').textContent);
  await page.close();
}
console.log(JSON.stringify(out, null, 1));
await browser.close();
