// Chrome side of the per-glyph probe: for each selector on a composed board page, the text's code
// points, the element's rect, and the platform fonts Chrome drew it with (CDP CSS.getPlatformFontsForNode).
// argv: <composed page html> <width> <height> <selectors.json> ; env ORACLE_DIR, PARITY_CHROME_PATH
import { readFileSync } from 'fs';
const oracle = process.env.ORACLE_DIR;
const { chromium } = await import(`file:///${oracle}/node_modules/playwright/index.mjs`);
const det = await import(`file:///${oracle}/deterministic.mjs`);
const [html, w, h, selFile] = process.argv.slice(2);
const selectors = JSON.parse(readFileSync(selFile, 'utf8'));
const browser = await chromium.launch(det.getDeterministicLaunchOptions({ requirePinned: true }));
const ctx = await det.createDeterministicContext(browser, Number(w), Number(h), { applyParityReset: false });
const page = await ctx.newPage();
await page.goto(`file:///${html}`, { waitUntil: 'networkidle' });
await page.waitForTimeout(100);
const cdp = await ctx.newCDPSession(page);
await cdp.send('DOM.enable'); await cdp.send('CSS.enable');
const doc = await cdp.send('DOM.getDocument', { depth: -1 });
const out = [];
for (const sel of selectors) {
  const info = await page.evaluate((s) => {
    const el = document.querySelector(s);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const cs = getComputedStyle(el);
    const text = el.textContent;
    return { text, cps: [...text].map(c => 'U+' + c.codePointAt(0).toString(16).toUpperCase()),
             rect: [r.x, r.y, r.width, r.height], font: cs.fontFamily, size: cs.fontSize, lineHeight: cs.lineHeight };
  }, sel);
  if (!info) { out.push({ sel, missing: true }); continue; }
  let fonts = null;
  try {
    const { nodeId } = await cdp.send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: sel });
    fonts = (await cdp.send('CSS.getPlatformFontsForNode', { nodeId })).fonts;
  } catch (e) { fonts = String(e.message).slice(0, 120); }
  out.push({ sel, ...info, platformFonts: fonts });
}
console.log(JSON.stringify({ browser: browser.version(), rows: out }, null, 1));
await browser.close();
