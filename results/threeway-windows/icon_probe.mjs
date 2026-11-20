// Every element on the composed chrome page whose own text is one symbol (> U+2000): Chrome's width and face.
const oracle = process.env.ORACLE_DIR;
const { chromium } = await import(`file:///${oracle}/node_modules/playwright/index.mjs`);
const det = await import(`file:///${oracle}/deterministic.mjs`);
const [html] = process.argv.slice(2);
const browser = await chromium.launch(det.getDeterministicLaunchOptions({ requirePinned: true }));
const ctx = await det.createDeterministicContext(browser, 1280, 800, { applyParityReset: false });
const page = await ctx.newPage();
await page.goto(`file:///${html}`, { waitUntil: 'networkidle' });
await page.waitForTimeout(100);
const n = await page.evaluate(() => {
  let i = 0;
  for (const el of document.querySelectorAll('body *')) {
    const own = [...el.childNodes].filter(c => c.nodeType === 3).map(c => c.data).join('').trim();
    const chars = [...own];
    if (chars.length >= 1 && chars.length <= 2 && chars[0].codePointAt(0) > 0x2000) { el.setAttribute('data-icon-probe', String(i++)); }
  }
  return i;
});
const cdp = await ctx.newCDPSession(page);
await cdp.send('DOM.enable'); await cdp.send('CSS.enable');
const doc = await cdp.send('DOM.getDocument', { depth: -1 });
const rows = [];
for (let i = 0; i < n; i++) {
  const sel = `[data-icon-probe="${i}"]`;
  const info = await page.evaluate((s) => { const el = document.querySelector(s); const r = el.getBoundingClientRect();
    const own = [...el.childNodes].filter(c => c.nodeType === 3).map(c => c.data).join('').trim();
    return { text: own, cps: [...own].map(c => 'U+' + c.codePointAt(0).toString(16).toUpperCase()), w: r.width, h: r.height,
             size: getComputedStyle(el).fontSize, family: getComputedStyle(el).fontFamily }; }, sel);
  const { nodeId } = await cdp.send('DOM.querySelector', { nodeId: doc.root.nodeId, selector: sel });
  const fonts = (await cdp.send('CSS.getPlatformFontsForNode', { nodeId })).fonts;
  rows.push({ ...info, fonts: fonts.map(f => f.familyName) });
}
console.log(JSON.stringify(rows));
await browser.close();
