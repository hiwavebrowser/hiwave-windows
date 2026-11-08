// backdrop_filter_probe.mjs - Chromium 143 pixels for
// crates/rustkit-engine/src/backdrop_filter_tests.rs (the same page).
//   node backdrop_filter_probe.mjs [outdir]   (writes <case>.html + chrome_<case>.png, prints row 140)
import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';
import { writeFileSync, mkdirSync } from 'fs';
import { PNG } from 'pngjs';
import { readFileSync } from 'fs';
const out = process.argv[2] || '.';
mkdirSync(out, { recursive: true });
const page = ([even, odd], bar) => `<!DOCTYPE html><html><head><style>html,body{margin:0}#s{display:flex;width:400px;height:300px}#s i{flex:none;width:10px;height:300px;background:${odd}}#s i:nth-child(odd){background:${even}}#bar{position:absolute;left:50px;top:100px;width:300px;height:80px;${bar}}</style></head><body><div id="s">${'<i></i>'.repeat(40)}</div><div id="bar"></div></body></html>`;
const BW=['#000','#fff'], RB=['#f00','#00f'];
const cases = {
 none: page(BW,''),
 blur8: page(BW,'backdrop-filter:blur(8px)'),
 sat0: page(RB,'backdrop-filter:saturate(0)'),
 bright_then_sat: page(RB,'backdrop-filter:brightness(2) saturate(0)'),
 sat_then_bright: page(RB,'backdrop-filter:saturate(0) brightness(2)'),
 blur_sat: page(RB,'backdrop-filter:blur(8px) saturate(0)'),
 contrast: page(RB,'backdrop-filter:contrast(0.5)'),
 opacity: page(BW,'backdrop-filter:opacity(0.5)'),
 bg_over: page(BW,'background:rgba(255,255,255,0.5);backdrop-filter:blur(8px)'),
 own_opacity: page(BW,'opacity:0.5;backdrop-filter:blur(8px)'),
 invalid: page(BW,'backdrop-filter:blur(8px) foo(1)'),
 invalid_keeps: page(RB,'backdrop-filter:saturate(0);backdrop-filter:blur(2px) foo(1)'),
 radius: page(BW,'border-radius:20px;backdrop-filter:blur(8px)'),
};
const b = await chromium.launch(getDeterministicLaunchOptions());
console.error(b.version());
const p = await b.newPage({viewport:{width:400,height:300}, deviceScaleFactor:1});
for (const [k,html] of Object.entries(cases)) {
  writeFileSync(`${out}/${k}.html`, html);
  const png = `${out}/chrome_${k}.png`;
  await p.setContent(html); await p.screenshot({path: png});
  const img = PNG.sync.read(readFileSync(png));
  const px = (x, y) => { const i = (y * img.width + x) * 4; return `${img.data[i]},${img.data[i+1]},${img.data[i+2]}`; };
  console.log(k, [50, 55, 65, 75, 200, 349].map((x) => `x${x}=${px(x, 140)}`).join(' '), `(52,102)=${px(52, 102)}`);
}
await b.close();
