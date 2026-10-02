/**
 * verify_selector_key.mjs — the join key is a committed contract, not a style choice.
 *
 * Plan: docs/PARITY_FINISH_LINE_PLAN_2026-08-04.md §2. Rules:
 * trench/BASELINE-parity-finish-line.md.
 *
 * WHAT THIS PROTECTS
 * ------------------
 * Gates A, B and C all join RustKit's layout tree to Chrome's committed rects
 * on ONE string: the selector produced by `getSelector` in capture_baseline.mjs
 * and mirrored in the engine (P0a-0). If that function's output changes by even
 * one character, the join does not fail loudly — it silently stops matching,
 * and every element that falls out of the join is scored as "no geometry error"
 * because it is never compared at all. Night 1 hit exactly this: a classed
 * inline <svg> dropped three elements out of the join and the unit tests stayed
 * green.
 *
 * So the contract is asserted against the committed baselines themselves:
 *
 *     every selector in baselines/<set>/<scope>/<case>/layout-rects.json
 *     is reproduced by the CURRENT getSelector, running in Chromium,
 *     against the case's real DOM.
 *
 * Extra selectors are fine and expected — the capture filters out zero-size
 * elements and script/style/meta/head/title/html, so this verifier generates
 * keys for elements the baseline deliberately omits. The baseline is the
 * authority on WHICH elements are compared; this file is only the authority on
 * whether their keys can still be produced.
 *
 * WHY THE FUNCTION IS EXTRACTED, NOT COPIED
 * -----------------------------------------
 * The source of `getSelector` is read out of capture_baseline.mjs by brace
 * matching and evaluated here. A copy would drift from the script it is meant
 * to pin, which is the entire failure this file exists to prevent — a green
 * verifier testing its own stale duplicate is worse than no verifier, because
 * it reads as coverage.
 *
 * ON THE `/\\s+/` IN THAT FUNCTION
 * --------------------------------
 * It matches a literal backslash followed by `s`, not whitespace, so the split
 * is a no-op and the raw `className` survives into the key: a two-class element
 * keys as `div.card featured`, space intact. That is not valid CSS and it looks
 * like a typo, but it is what 305 committed baseline selectors say, so it is
 * the contract. Do not "fix" it without regenerating every baseline — this
 * verifier will go red if you do, which is the point.
 *
 * USAGE
 *     node tools/parity_oracle/verify_selector_key.mjs [--verbose]
 *
 * Exit 0 only when every committed selector was reproduced AND at least one
 * case was actually checked. Exit 1 on any miss, on a missing baseline, on a
 * duplicate key, or on an empty run.
 *
 * Env:
 *   PARITY_BASELINE_SET   baseline set directory name (default chrome-148)
 *   PARITY_CHROMIUM_PATH  explicit Chromium binary, for seats where the
 *                         bundled Playwright browser revision does not match
 */

import { chromium } from 'playwright';
import { readFileSync, existsSync } from 'fs';
import { dirname, resolve, join } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..', '..');
const BASELINE_SET = process.env.PARITY_BASELINE_SET || 'chrome-148';
const VERBOSE = process.argv.includes('--verbose');

/**
 * Read `function getSelector(el) { ... }` verbatim out of capture_baseline.mjs.
 *
 * Throws rather than falling back if the function cannot be found. A verifier
 * that shrugs and passes when it cannot locate the thing it verifies is the
 * blank-row-reads-as-green failure from the baseline file's fleet rule.
 */
export function extractGetSelectorSource(scriptText) {
  const start = scriptText.indexOf('function getSelector(el)');
  if (start < 0) {
    throw new Error(
      'getSelector not found in capture_baseline.mjs — it was renamed, moved or ' +
      'deleted. This verifier cannot pin a function it cannot find; fix the ' +
      'extraction rather than deleting the check.'
    );
  }
  let depth = 0;
  let end = -1;
  for (let i = scriptText.indexOf('{', start); i < scriptText.length; i++) {
    const ch = scriptText[i];
    if (ch === '{') depth++;
    else if (ch === '}') {
      depth--;
      if (depth === 0) { end = i + 1; break; }
    }
  }
  if (end < 0) throw new Error('getSelector body is unbalanced — extraction failed');
  return scriptText.slice(start, end);
}

function loadRegistry() {
  return JSON.parse(readFileSync(join(REPO_ROOT, 'cases', 'registry.json'), 'utf8')).cases;
}

function baselineRectsPath(caseId, scope) {
  return join(REPO_ROOT, 'baselines', BASELINE_SET, scope, caseId, 'layout-rects.json');
}

async function main() {
  const scriptText = readFileSync(join(__dirname, 'capture_baseline.mjs'), 'utf8');
  const fnSrc = extractGetSelectorSource(scriptText);

  const registry = loadRegistry();
  const launchOpts = {};
  if (process.env.PARITY_CHROMIUM_PATH) {
    launchOpts.executablePath = process.env.PARITY_CHROMIUM_PATH;
  }
  const browser = await chromium.launch(launchOpts);

  const failures = [];
  let checkedCases = 0;
  let checkedSelectors = 0;

  try {
    for (const [caseId, spec] of Object.entries(registry)) {
      const rectsPath = baselineRectsPath(caseId, spec.scope);
      if (!existsSync(rectsPath)) {
        // Not a skip. A registry case with no committed rects cannot be joined,
        // and reporting that as "nothing to check" is how a corpus quietly
        // shrinks. Gate A reports the same condition as unmeasured.
        failures.push(`${caseId} · no committed layout-rects.json at ${rectsPath}`);
        continue;
      }
      const htmlPath = resolve(REPO_ROOT, spec.html);
      if (!existsSync(htmlPath)) {
        failures.push(`${caseId} · registry html missing: ${spec.html}`);
        continue;
      }

      const want = JSON.parse(readFileSync(rectsPath, 'utf8')).elements.map(e => e.selector);
      const dupes = want.filter((s, i) => want.indexOf(s) !== i);
      if (dupes.length) {
        // An ambiguous key joins one of two boxes and calls the case scored.
        failures.push(`${caseId} · baseline has duplicate keys: ${[...new Set(dupes)].slice(0, 3).join(', ')}`);
      }

      const page = await browser.newPage({
        viewport: { width: spec.width, height: spec.height },
      });
      let got;
      try {
        await page.goto('file://' + htmlPath, { waitUntil: 'load' });
        got = await page.evaluate((src) => {
          const getSelector = new Function('return (' + src + ')')();
          return Array.from(document.querySelectorAll('*')).map(getSelector);
        }, fnSrc);
      } finally {
        await page.close();
      }

      const produced = new Set(got);
      const missing = want.filter(s => !produced.has(s));
      checkedCases += 1;
      checkedSelectors += want.length;
      for (const sel of missing) {
        failures.push(`${caseId} · not reproduced · ${sel}`);
      }
      if (VERBOSE) {
        console.log(`${caseId}: ${want.length - missing.length}/${want.length} reproduced`);
      }
    }
  } finally {
    await browser.close();
  }

  // Empty-run tripwire. A run that checked nothing must not exit 0 — that is
  // the shape every silent-instrument failure in this campaign has taken.
  if (checkedCases === 0) {
    console.error('FAIL: verify_selector_key checked 0 cases. The join key is unverified, not verified.');
    process.exit(1);
  }

  if (failures.length) {
    console.error(`FAIL: ${failures.length} selector-key problem(s) across ${checkedCases} case(s):`);
    for (const line of failures) console.error(`  ${line}`);
    console.error(
      '\nThe join key changed. Gates A/B/C join on this string; a key that no ' +
      'longer matches does not report a failure, it silently drops the element ' +
      'from comparison. Either revert the change to getSelector, or regenerate ' +
      'every baseline and re-mirror the engine side (P0a-0).'
    );
    process.exit(1);
  }

  console.log(
    `OK: ${checkedSelectors}/${checkedSelectors} committed selectors reproduced across ${checkedCases} cases ` +
    `(baseline set ${BASELINE_SET}).`
  );
}

if (import.meta.url === `file://${process.argv[1]}`) {
  await main();
}
