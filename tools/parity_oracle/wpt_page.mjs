import { chromium } from 'playwright';
import { getDeterministicLaunchOptions } from './deterministic.mjs';

if (!process.env.PARITY_CHROME_PATH) throw Error('PARITY_CHROME_PATH is required');
const options = getDeterministicLaunchOptions();
options.args.push('--disable-background-networking', '--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE 127.0.0.1');
const browser = await chromium.launch(options);
let crashed = false;
browser.on('disconnected', () => { crashed = true; });
try {
  if (browser.version() !== '148.0.7778.216') throw Error('Expected Chrome 148.0.7778.216');
  const [target, timeout] = process.argv.slice(2);
  if (target !== '--verify') {
    const origin = new URL(target);
    if (origin.hostname !== '127.0.0.1' || origin.protocol !== 'http:') throw Error('Expected loopback');
    const context = await browser.newContext({ serviceWorkers: 'block' });
    await context.route('**/*', route => new URL(route.request().url()).origin === origin.origin
      ? route.continue() : route.abort());
    const page = await context.newPage();
    page.on('crash', () => { crashed = true; });
    await page.goto(target, { waitUntil: 'commit', timeout: Number(timeout) });
    const result = await page.waitForFunction(() => globalThis.__hiwaveWptResult,
      undefined, { timeout: Number(timeout) });
    console.log('WPT_ENCODED ' + await result.jsonValue());
  }
} catch (error) {
  if (crashed) process.exitCode = 4;
  else if (error.name === 'TimeoutError') process.exitCode = 3;
  else throw error;
} finally {
  await browser.close();
}
