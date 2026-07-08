/**
 * capture_all_baselines.mjs - Regenerate the full pinned-Chrome baseline tree.
 *
 * Derives the case list and per-case viewports from an existing baseline tree
 * (so the new tree is structurally identical), captures each case with the
 * pinned browser (PARITY_CHROME_PATH), and writes metadata.json recording the
 * exact binary version. Cases whose source HTML no longer exists are reported,
 * never silently skipped.
 *
 * Usage:
 *   PARITY_CHROME_PATH=<pinned chrome.exe> node capture_all_baselines.mjs \
 *     [--from baselines/chrome-120] [--to baselines/chrome-148]
 */

import { chromium } from 'playwright';
import { readFileSync, writeFileSync, existsSync, readdirSync, mkdirSync } from 'fs';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';
import { captureBaseline } from './capture_baseline.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '../..');

const args = process.argv.slice(2);
function argVal(name, dflt) {
  const i = args.indexOf(name);
  return i >= 0 && args[i + 1] ? args[i + 1] : dflt;
}
const FROM = resolve(REPO_ROOT, argVal('--from', 'baselines/chrome-120'));
const TO = resolve(REPO_ROOT, argVal('--to', 'baselines/chrome-148'));

const SOURCE_BY_TYPE = {
  builtins: (c) => join(REPO_ROOT, 'crates', 'hiwave-app', 'src', 'ui', `${c}.html`),
  micro: (c) => join(REPO_ROOT, 'websuite', 'micro', c, 'index.html'),
  websuite: (c) => join(REPO_ROOT, 'websuite', 'cases', c, 'index.html'),
};

// Enumerate cases from the reference tree.
const plan = [];
const problems = [];
for (const type of Object.keys(SOURCE_BY_TYPE)) {
  const typeDir = join(FROM, type);
  if (!existsSync(typeDir)) continue;
  for (const caseId of readdirSync(typeDir)) {
    if (caseId === 'oracle') continue; // chromium oracle snapshots, not a case
    const refMeta = join(typeDir, caseId, 'computed-styles.json');
    if (!existsSync(refMeta)) continue;
    const viewport = JSON.parse(readFileSync(refMeta, 'utf8')).viewport;
    const src = SOURCE_BY_TYPE[type](caseId);
    if (!existsSync(src)) {
      problems.push(`${type}/${caseId}: source missing (${src})`);
      continue;
    }
    plan.push({ type, caseId, src, viewport });
  }
}

console.log(`Capturing ${plan.length} cases from ${FROM} layout into ${TO}`);
if (!process.env.PARITY_CHROME_PATH) {
  console.error('FATAL: PARITY_CHROME_PATH not set — refusing to capture an unpinned baseline.');
  process.exit(1);
}

// Record the exact browser version once.
const probe = await chromium.launch({
  headless: true, executablePath: process.env.PARITY_CHROME_PATH,
});
const browserVersion = probe.version();
await probe.close();

let ok = 0;
const failures = [];
for (const { type, caseId, src, viewport } of plan) {
  const outDir = join(TO, type, caseId);
  try {
    const r = await captureBaseline(src, outDir, viewport.width, viewport.height);
    ok++;
    console.log(`OK  ${type}/${caseId} (${r.elementCount} elements)`);
  } catch (e) {
    failures.push(`${type}/${caseId}: ${e.message.split('\n')[0]}`);
    console.log(`FAIL ${type}/${caseId}: ${e.message.split('\n')[0]}`);
  }
}

mkdirSync(TO, { recursive: true });
writeFileSync(join(TO, 'metadata.json'), JSON.stringify({
  browserVersion,
  executable: 'Chrome for Testing (pinned via PARITY_CHROME_PATH)',
  capturedAt: new Date().toISOString(),
  platform: process.platform,
  glBackend: 'angle-swiftshader',
  cases: ok,
  failures,
  sourceMissing: problems,
}, null, 2));

console.log(`\nDone: ${ok}/${plan.length} captured, ${failures.length} failures, ${problems.length} missing sources`);
if (problems.length) console.log('Missing sources:\n  ' + problems.join('\n  '));
process.exit(failures.length ? 2 : 0);
