/**
 * parity_score.mjs — Windows parity scoreboard for the full mirrored suite.
 *
 * Renders each case with RustKit (parity-capture.exe, headless), compares the
 * frame against the pinned Chrome-for-Testing baseline
 * (baselines/chrome-<ver>/<type>/<case>/baseline.png) via pixelmatch, and emits
 * a per-case + per-type + campaign/holdout scoreboard.
 *
 * This reconciles the previously-forked measure stack: run_oracle.mjs only
 * enumerated the 8 websuite cases from a flat oracle/chromium/*.png layout and
 * never drove RustKit; capture_all_baselines.mjs produced the nested baselines
 * but had no compare/score side and no holdout mapping. This script scores the
 * whole mirrored suite (builtins + websuite + micro + holdout) against those
 * nested baselines, so every W-local change becomes measurable.
 *
 * Usage:
 *   node parity_score.mjs [--baselines baselines/chrome-148]
 *                         [--threshold 15] [--out parity-baseline/score]
 *                         [--types builtins,websuite,micro,holdout] [--tag NAME]
 *
 * Campaign = builtins + websuite + micro (holdout reported separately, matching
 * the macOS scoreboard convention). Diff % is Windows-Chrome vs Windows-RustKit
 * (apples-to-apples on this platform); comparable case-by-case to macOS because
 * the HTML and the pinned oracle build are mirrored.
 */
import { execFileSync } from 'child_process';
import {
  existsSync, readdirSync, readFileSync, writeFileSync, mkdirSync, rmSync,
} from 'fs';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';
import { comparePixels } from './compare_pixels.mjs';

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..', '..');

function argVal(flag, def) {
  const i = process.argv.indexOf(flag);
  return i !== -1 && process.argv[i + 1] ? process.argv[i + 1] : def;
}

const BASELINES = resolve(REPO_ROOT, argVal('--baselines', 'baselines/chrome-148'));
const THRESHOLD = parseFloat(argVal('--threshold', '15'));
const OUT = resolve(REPO_ROOT, argVal('--out', 'parity-baseline/score'));
const TYPES = argVal('--types', 'builtins,websuite,micro,holdout').split(',').map((s) => s.trim());
const TAG = argVal('--tag', '');

// Same source mapping as capture_all_baselines.mjs (incl. the holdout addition).
const SOURCE_BY_TYPE = {
  builtins: (c) => join(REPO_ROOT, 'crates', 'hiwave-app', 'src', 'ui', `${c}.html`),
  micro: (c) => join(REPO_ROOT, 'websuite', 'micro', c, 'index.html'),
  websuite: (c) => join(REPO_ROOT, 'websuite', 'cases', c, 'index.html'),
  holdout: (c) => join(REPO_ROOT, 'websuite', 'holdout', c, 'index.html'),
};

const CAMPAIGN_TYPES = new Set(['builtins', 'websuite', 'micro']);

const CAPTURE_BIN = join(REPO_ROOT, 'target', 'release', 'parity-capture.exe');
if (!existsSync(CAPTURE_BIN)) {
  console.error(`FATAL: RustKit capture binary not found: ${CAPTURE_BIN}\n` +
    `Build it first: cargo build --release -p parity-capture`);
  process.exit(1);
}

const framesDir = join(OUT, 'frames');
const diffsDir = join(OUT, 'diffs');
rmSync(framesDir, { recursive: true, force: true });
mkdirSync(framesDir, { recursive: true });
mkdirSync(diffsDir, { recursive: true });

// Enumerate cases from the baseline tree (source of truth for the scored set).
const plan = [];
for (const type of TYPES) {
  if (!SOURCE_BY_TYPE[type]) continue;
  const typeDir = join(BASELINES, type);
  if (!existsSync(typeDir)) continue;
  for (const caseId of readdirSync(typeDir)) {
    if (caseId === 'oracle') continue;
    const baselinePng = join(typeDir, caseId, 'baseline.png');
    const metaPath = join(typeDir, caseId, 'computed-styles.json');
    if (!existsSync(baselinePng) || !existsSync(metaPath)) continue;
    let viewport;
    try {
      viewport = JSON.parse(readFileSync(metaPath, 'utf8')).viewport;
    } catch {
      viewport = null;
    }
    if (!viewport) continue;
    const src = SOURCE_BY_TYPE[type](caseId);
    if (!existsSync(src)) {
      plan.push({ type, caseId, src, viewport, baselinePng, missing: true });
      continue;
    }
    plan.push({ type, caseId, src, viewport, baselinePng, missing: false });
  }
}

console.log(`parity_score: ${plan.length} cases from ${BASELINES} @ threshold ${THRESHOLD}%\n`);

function renderRustkit(src, viewport, ppmPath) {
  const out = execFileSync(CAPTURE_BIN, [
    '--html-file', src,
    '--width', String(viewport.width),
    '--height', String(viewport.height),
    '--dump-frame', ppmPath,
  ], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'], timeout: 60000 });
  return JSON.parse(out.trim());
}

const results = [];
for (const item of plan) {
  const { type, caseId, src, viewport, baselinePng, missing } = item;
  const label = `${type}/${caseId}`;
  if (missing) {
    results.push({ type, caseId, ok: false, error: 'html source missing', diff_pct: 100, passed: false });
    console.log(`  x ${label.padEnd(38)} SOURCE MISSING`);
    continue;
  }
  const ppmPath = join(framesDir, `${type}__${caseId}.ppm`);
  const diffPath = join(diffsDir, `${type}__${caseId}.diff.png`);
  try {
    const cap = renderRustkit(src, viewport, ppmPath);
    if (cap.status !== 'ok' || !existsSync(ppmPath)) {
      results.push({ type, caseId, ok: false, error: cap.error || 'render failed', diff_pct: 100, passed: false });
      console.log(`  x ${label.padEnd(38)} RENDER FAIL: ${cap.error || 'no frame'}`);
      continue;
    }
    const cmp = await comparePixels(baselinePng, ppmPath, diffPath);
    const passed = cmp.diffPercent <= THRESHOLD;
    results.push({
      type, caseId, ok: true, diff_pct: +cmp.diffPercent.toFixed(2),
      diff_pixels: cmp.diffPixels, total_pixels: cmp.totalPixels, passed,
    });
    console.log(`  ${passed ? '+' : 'x'} ${label.padEnd(38)} ${cmp.diffPercent.toFixed(2)}%`);
  } catch (err) {
    results.push({ type, caseId, ok: false, error: String(err.message || err), diff_pct: 100, passed: false });
    console.log(`  x ${label.padEnd(38)} ERROR: ${String(err.message || err).slice(0, 60)}`);
  }
}

// Aggregate.
function agg(rows) {
  const scored = rows.filter((r) => r.ok);
  const passed = rows.filter((r) => r.passed).length;
  const avg = scored.length ? scored.reduce((s, r) => s + r.diff_pct, 0) / scored.length : 0;
  return { total: rows.length, scored: scored.length, passed, avg_diff: +avg.toFixed(2) };
}

const byType = {};
for (const type of TYPES) {
  const rows = results.filter((r) => r.type === type);
  if (rows.length) byType[type] = agg(rows);
}
const campaign = agg(results.filter((r) => CAMPAIGN_TYPES.has(r.type)));
const holdout = agg(results.filter((r) => r.type === 'holdout'));

const worst = [...results].filter((r) => r.ok).sort((a, b) => b.diff_pct - a.diff_pct).slice(0, 8)
  .map((r) => `${r.type}/${r.caseId} ${r.diff_pct}%`);

const scoreboard = {
  tag: TAG || null,
  baselines: argVal('--baselines', 'baselines/chrome-148'),
  threshold: THRESHOLD,
  campaign, holdout, by_type: byType, worst, cases: results,
};

mkdirSync(OUT, { recursive: true });
writeFileSync(join(OUT, 'score.json'), JSON.stringify(scoreboard, null, 2));

const md = [
  `# Windows parity scoreboard${TAG ? ` — ${TAG}` : ''}`,
  ``,
  `Baselines: \`${argVal('--baselines', 'baselines/chrome-148')}\` · threshold ${THRESHOLD}%`,
  `RustKit: \`target/release/parity-capture.exe\` (headless)`,
  ``,
  `**Campaign: ${campaign.passed}/${campaign.total} pass @ t${THRESHOLD} · avg ${campaign.avg_diff}%**`,
  `**Holdout: ${holdout.passed}/${holdout.total} pass · avg ${holdout.avg_diff}%**`,
  ``,
  `| type | pass/total | avg diff% |`,
  `|------|-----------|-----------|`,
  ...TYPES.filter((t) => byType[t]).map((t) => `| ${t} | ${byType[t].passed}/${byType[t].total} | ${byType[t].avg_diff} |`),
  ``,
  `Worst cases: ${worst.join(' · ') || '(none)'}`,
].join('\n');
writeFileSync(join(OUT, 'score.md'), md);

console.log(`\n--- Scoreboard ---`);
console.log(`Campaign: ${campaign.passed}/${campaign.total} @ t${THRESHOLD} (avg ${campaign.avg_diff}%)`);
console.log(`Holdout:  ${holdout.passed}/${holdout.total} (avg ${holdout.avg_diff}%)`);
for (const t of TYPES) if (byType[t]) console.log(`  ${t}: ${byType[t].passed}/${byType[t].total} (avg ${byType[t].avg_diff}%)`);
console.log(`\nWritten: ${join(OUT, 'score.json')} + score.md`);
