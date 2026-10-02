/**
 * capture_seat_control.mjs — capture a SEAT CONTROL baseline.
 *
 * WHAT THIS IS
 * ------------
 * The pinned baseline in `baselines/chrome-148/` was captured by Chrome 148 on
 * macOS. Gate A compares RustKit against it, and that comparison is the
 * campaign's receipt — but only when RustKit is ALSO running on macOS.
 *
 * On any other seat (the Linux trench seat, a contributor's machine) the two
 * sides no longer share a platform: fontconfig substitutes for the fonts the
 * fixtures name, and the rasterizer and Chromium build differ. Every geometry
 * delta Gate A reports there is then a SUM of two things that look identical in
 * the output:
 *
 *     Δ_reported = (real RustKit box-math defect) + (platform confound)
 *
 * trench/digest-parity-finish-line.md (2026-08-04, night 4) recorded that the
 * split "needs a macOS run to make, not a cleverer analysis of this one". That
 * is not so — it needs a CONTROL, which is what this script captures: the same
 * cases, through the same `captureBaseline` code that produced the pinned set,
 * with the seat's own browser and the seat's own fonts on BOTH sides.
 *
 *     Δ_confound = Chrome_seat  − Chrome_pinned      <- this script + report
 *     Δ_real     = RustKit_seat − Chrome_seat        <- Gate A, seat set
 *     Δ_reported = RustKit_seat − Chrome_pinned      <- Gate A, pinned set
 *
 * WHAT THIS IS NOT
 * ----------------
 * **NEVER A RECEIPT.** A seat control is a diagnostic. It cannot produce an
 * `N/26`, it cannot be cited in a PR as a parity number, and it says nothing
 * about macOS. The macOS `chrome-148` set remains the only baseline the metric
 * is defined against (trench/BASELINE-parity-finish-line.md). The output
 * directory is deliberately gitignored so a seat control can never be committed
 * and mistaken for the pinned set.
 *
 * Reusing `captureBaseline` is load-bearing, not convenience: a control taken by
 * different capture code would fold that difference into Δ_confound and quietly
 * credit it to the platform.
 *
 * Usage:
 *     node tools/parity_oracle/capture_seat_control.mjs
 *     node tools/parity_oracle/capture_seat_control.mjs --case settings
 *     node tools/parity_oracle/capture_seat_control.mjs --out <dir>
 *
 * Then:
 *     PARITY_BASELINE_SET=seat-control python3 scripts/layout_oracle_gate.py \
 *         --layout-root <rustkit captures>
 *     python3 scripts/seat_control_report.py --layout-root <rustkit captures>
 */

import { execFileSync } from 'child_process';
import { createHash } from 'crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'fs';
import { dirname, join, resolve } from 'path';
import { fileURLToPath } from 'url';

import { captureBaseline } from './capture_baseline.mjs';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '../..');

// The scope that is canary-only until the 26-case gate set is green (plan §3.6).
// Mirrors NON_GATING_SCOPES in scripts/layout_oracle_gate.py.
const NON_GATING_SCOPES = new Set(['holdout']);

const DEFAULT_OUT = join(REPO_ROOT, 'baselines', 'seat-control');

function parseArgs(argv) {
  const args = { out: DEFAULT_OUT, only: null };
  for (let i = 0; i < argv.length; i += 1) {
    if (argv[i] === '--out') args.out = resolve(argv[++i]);
    else if (argv[i] === '--case') args.only = argv[++i];
    else throw new Error(`unknown argument: ${argv[i]}`);
  }
  return args;
}

/**
 * Record what this control actually is. A control whose provenance is unknown
 * cannot be reasoned about later, and a stale one silently reports a confound
 * that no longer exists.
 */
function writeStamp(outDir, captured) {
  // A control captured before a fixture changed reports a confound that no
  // longer exists, and reports it silently — the numbers still parse, they are
  // just about a page that is gone. Recording the fixture each case was
  // captured from lets the report refuse instead of guessing.
  const stampPath = join(outDir, 'STAMP.json');
  let fixtures = {};
  if (existsSync(stampPath)) {
    try {
      const prior = JSON.parse(readFileSync(stampPath, 'utf8'));
      if (prior.kind === 'seat-control' && prior.fixtures) fixtures = prior.fixtures;
    } catch {
      // A stamp we cannot read is a stamp we do not carry forward.
    }
  }
  for (const [id, sha] of captured) fixtures[id] = sha;

  const stamp = {
    kind: 'seat-control',
    not_a_receipt: true,
    captured_at: new Date().toISOString(),
    platform: `${process.platform}-${process.arch}`,
    node: process.version,
    cases: Object.keys(fixtures).sort(),
    fixtures,
  };

  try {
    const pkg = JSON.parse(
      readFileSync(join(__dirname, 'node_modules', 'playwright-core', 'package.json'), 'utf8'),
    );
    stamp.playwright = pkg.version;
  } catch {
    stamp.playwright = 'unknown';
  }

  // The seat's font resolution for the families the corpus names. This is the
  // confound, written down: on macOS these resolve to the real faces, and a
  // reader comparing two stamps can see exactly what substituted.
  stamp.font_resolution = {};
  for (const family of ['Georgia', 'Times New Roman', 'Helvetica', 'Arial', '-apple-system', 'system-ui']) {
    try {
      // `:family=X` pattern form, not a bare name: a family such as
      // `-apple-system` starts with a dash and fc-match would read it as a flag.
      stamp.font_resolution[family] = execFileSync('fc-match', ['-f', '%{file}', `:family=${family}`], {
        encoding: 'utf8',
        stdio: ['ignore', 'pipe', 'ignore'],
      }).trim();
    } catch {
      stamp.font_resolution[family] = 'fc-match unavailable';
    }
  }

  writeFileSync(stampPath, `${JSON.stringify(stamp, null, 2)}\n`);
  return stamp;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const registry = JSON.parse(readFileSync(join(REPO_ROOT, 'cases/registry.json'), 'utf8')).cases;

  const jobs = Object.entries(registry)
    .filter(([id, c]) => !NON_GATING_SCOPES.has(c.scope) && (!args.only || id === args.only))
    .sort(([a], [b]) => a.localeCompare(b));

  if (jobs.length === 0) {
    console.error(`no cases matched${args.only ? ` --case ${args.only}` : ''}`);
    return 1;
  }

  mkdirSync(args.out, { recursive: true });
  console.log(`seat control -> ${args.out}`);
  console.log('NOT A RECEIPT: a seat control is a diagnostic, never an N/26.\n');

  let failed = 0;
  const captured = [];
  for (const [id, c] of jobs) {
    const dir = join(args.out, c.scope, id);
    const html = resolve(REPO_ROOT, c.html);
    if (!existsSync(html)) {
      console.log(`FAIL ${id}: missing fixture ${c.html}`);
      failed += 1;
      continue;
    }
    try {
      const r = await captureBaseline(html, dir, c.width, c.height);
      console.log(`ok   ${id} (${r.elementCount} elements)`);
      captured.push([id, createHash('sha256').update(readFileSync(html)).digest('hex')]);
    } catch (err) {
      console.log(`FAIL ${id}: ${err.message}`);
      failed += 1;
    }
  }

  const stamp = writeStamp(args.out, captured);
  console.log(`\ncaptured ${captured.length}, failed ${failed}`);
  console.log(`stamp: playwright ${stamp.playwright}, Georgia -> ${stamp.font_resolution.Georgia}`);
  return failed ? 1 : 0;
}

process.exit(await main());
