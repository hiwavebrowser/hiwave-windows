// Each document has its own module map (HTML §8.1.5.5 "module map", one per
// environment settings object): a second iframe that loads the same module
// URLs runs them again in its own window, and a frame whose module imports a
// module the parent page already ran runs that module again too.
// The receipt for rustkit-engine's `frames_stage2_tests::
// a_second_frame_runs_the_same_modules_again` (FRAME-MODULE-MAP). Speedometer 3.1
// loads every workload in a new iframe per iteration; from the second
// iteration on, TodoMVC-WebComponents and the Angular workloads ran none of
// their modules in HiWave (docs/diagnostics/2026-11-21/STRESS-TRUTH-2026-11-21.md).
// Usage: PARITY_CHROME_PATH=<Chrome for Testing 148.0.7778.216> node frame_module_realm_log.mjs [out.json]
import { chromium } from 'playwright';
import fs from 'fs';

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
// The same pages as the engine test, byte for byte.
const TOP = `<!doctype html><body><script type=module src=/top.mjs></script><script>
window.results = []; window.done = false;
function frame(then) {
  var f = document.createElement('iframe');
  f.src = '/mod.html';
  f.addEventListener('load', function () {
    var w = f.contentWindow;
    results.push(w.order.join() + '|' + w.document.getElementById('x').textContent);
    f.remove();
    then();
  });
  document.body.appendChild(f);
}
window.addEventListener('load', function () { frame(function () { frame(function () { done = true; }); }); });
</script></body>`;
const ROUTES = {
  '/': ['text/html', TOP],
  '/mod.html': ['text/html', `<!doctype html><body><x-hello id=x></x-hello><script type=module src=/main.mjs></script><script>window.order = ['classic'];</script></body>`],
  '/main.mjs': ['text/javascript', `import { dep } from './dep.mjs'; order.push(dep, 'main'); customElements.define('x-hello', class extends HTMLElement { connectedCallback() { this.textContent = 'upgraded'; } });`],
  '/dep.mjs': ['text/javascript', `if (window.order) order.push('dep'); else window.topDep = (window.topDep || 0) + 1; export const dep = 'dep-value';`],
  '/top.mjs': ['text/javascript', `import { dep } from './dep.mjs'; window.topSaw = dep;`],
};
const page = await browser.newPage();
await page.route('http://oracle.test/**', (r) => {
  const p = new URL(r.request().url()).pathname;
  const hit = ROUTES[p];
  if (!hit) return r.fulfill({ status: 404, body: '' });
  return r.fulfill({ contentType: hit[0], body: hit[1] });
});
await page.goto('http://oracle.test/');
await page.waitForFunction('window.done === true', null, { timeout: 20000 });
const out = await page.evaluate(() => ({
  frames: window.results,
  topDep: window.topDep,
  topSaw: window.topSaw,
}));
out.browser = browser.version();
for (const [k, v] of Object.entries(out)) console.log(k + ': ' + JSON.stringify(v));
if (process.argv[2]) fs.writeFileSync(process.argv[2], JSON.stringify(out, null, 1) + '\n');
await browser.close();
