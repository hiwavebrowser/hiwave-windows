#!/usr/bin/env node
/**
 * Parity Oracle - Chromium baseline capture and pixel diff tool
 * 
 * Usage:
 *   node run_oracle.mjs capture --case css-selectors --output parity-baseline/oracle
 *   node run_oracle.mjs compare --case css-selectors --rustkit parity-baseline/captures/css-selectors.ppm
 *   node run_oracle.mjs full --cases top --output parity-baseline
 *   node run_oracle.mjs styles --case css-selectors --output parity-baseline/computed-styles
 */

import { captureChrome } from './capture_chrome.mjs';
import { comparePixels, ppmToRgba } from './compare_pixels.mjs';
import { exportStyles } from './export_styles.mjs';
import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'fs';
import { join, dirname, resolve } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '../..');

// Case definitions (must match parity_baseline.py)
const CASES = {
  // Built-ins (60% weight)
  'new_tab': { path: 'crates/hiwave-app/src/ui/new_tab.html', width: 1280, height: 800, type: 'builtin' },
  'about': { path: 'crates/hiwave-app/src/ui/about.html', width: 800, height: 600, type: 'builtin' },
  'settings': { path: 'crates/hiwave-app/src/ui/settings.html', width: 1024, height: 768, type: 'builtin' },
  'chrome_rustkit': { path: 'crates/hiwave-app/src/ui/chrome_rustkit.html', width: 1280, height: 100, type: 'builtin' },
  'shelf': { path: 'crates/hiwave-app/src/ui/shelf.html', width: 1280, height: 120, type: 'builtin' },
  
  // Websuite (40% weight)
  'article-typography': { path: 'websuite/cases/article-typography/index.html', width: 1280, height: 800, type: 'websuite' },
  'card-grid': { path: 'websuite/cases/card-grid/index.html', width: 1280, height: 800, type: 'websuite' },
  'css-selectors': { path: 'websuite/cases/css-selectors/index.html', width: 800, height: 1200, type: 'websuite' },
  'flex-positioning': { path: 'websuite/cases/flex-positioning/index.html', width: 800, height: 1000, type: 'websuite' },
  'form-elements': { path: 'websuite/cases/form-elements/index.html', width: 800, height: 600, type: 'websuite' },
  'gradient-backgrounds': { path: 'websuite/cases/gradient-backgrounds/index.html', width: 800, height: 600, type: 'websuite' },
  'image-gallery': { path: 'websuite/cases/image-gallery/index.html', width: 1280, height: 800, type: 'websuite' },
  'sticky-scroll': { path: 'websuite/cases/sticky-scroll/index.html', width: 1280, height: 800, type: 'websuite' },
};

// Top 3 worst cases (for --scope top)
const TOP_CASES = ['css-selectors', 'image-gallery', 'sticky-scroll'];
const BUILTIN_CASES = Object.entries(CASES).filter(([_, v]) => v.type === 'builtin').map(([k]) => k);
const WEBSUITE_CASES = Object.entries(CASES).filter(([_, v]) => v.type === 'websuite').map(([k]) => k);

function parseArgs() {
  const args = process.argv.slice(2);
  const opts = {
    command: args[0] || 'help',
    case: null,
    cases: null,
    scope: 'top',  // top, builtins, websuite, all
    output: 'parity-baseline',
    rustkit: null,
    threshold: 25,
    verbose: false,
  };
  
  for (let i = 1; i < args.length; i++) {
    const arg = args[i];
    if (arg === '--case' && args[i + 1]) {
      opts.case = args[++i];
    } else if (arg === '--cases' && args[i + 1]) {
      opts.cases = args[++i].split(',');
    } else if (arg === '--scope' && args[i + 1]) {
      opts.scope = args[++i];
    } else if (arg === '--output' && args[i + 1]) {
      opts.output = args[++i];
    } else if (arg === '--rustkit' && args[i + 1]) {
      opts.rustkit = args[++i];
    } else if (arg === '--threshold' && args[i + 1]) {
      opts.threshold = parseFloat(args[++i]);
    } else if (arg === '-v' || arg === '--verbose') {
      opts.verbose = true;
    }
  }
  
  return opts;
}

function getCasesToRun(opts) {
  if (opts.case) {
    return [opts.case];
  }
  if (opts.cases) {
    return opts.cases;
  }
  switch (opts.scope) {
    case 'top': return TOP_CASES;
    case 'builtins': return BUILTIN_CASES;
    case 'websuite': return WEBSUITE_CASES;
    case 'all': return Object.keys(CASES);
    default: return TOP_CASES;
  }
}

async function runCapture(opts) {
  const cases = getCasesToRun(opts);
  const outputDir = join(opts.output, 'oracle', 'chromium');
  mkdirSync(outputDir, { recursive: true });
  
  console.log(`\n=== Chromium Oracle: Capture ===`);
  console.log(`Cases: ${cases.join(', ')}`);
  console.log(`Output: ${outputDir}\n`);
  
  const results = {};
  
  for (const caseId of cases) {
    const caseInfo = CASES[caseId];
    if (!caseInfo) {
      console.error(`Unknown case: ${caseId}`);
      continue;
    }
    
    const htmlPath = join(REPO_ROOT, caseInfo.path);
    const outputPath = join(outputDir, `${caseId}.png`);
    
    console.log(`  Capturing ${caseId}...`, '');
    
    try {
      await captureChrome(htmlPath, outputPath, caseInfo.width, caseInfo.height);
      results[caseId] = { success: true, path: outputPath };
      console.log('OK');
    } catch (err) {
      results[caseId] = { success: false, error: err.message };
      console.log(`FAIL: ${err.message}`);
    }
  }
  
  // Save capture manifest
  const manifestPath = join(outputDir, 'manifest.json');
  writeFileSync(manifestPath, JSON.stringify({
    timestamp: new Date().toISOString(),
    browser: 'chromium',
    cases: results,
  }, null, 2));
  
  console.log(`\nManifest saved to: ${manifestPath}`);
  return results;
}

async function runCompare(opts) {
  const cases = getCasesToRun(opts);
  const oracleDir = join(opts.output, 'oracle', 'chromium');
  const capturesDir = join(opts.output, 'captures');
  const diffsDir = join(opts.output, 'diffs');
  mkdirSync(diffsDir, { recursive: true });
  
  console.log(`\n=== Chromium Oracle: Compare ===`);
  console.log(`Cases: ${cases.join(', ')}`);
  console.log(`Oracle: ${oracleDir}`);
  console.log(`RustKit: ${capturesDir}`);
  console.log(`Diffs: ${diffsDir}\n`);
  
  const results = {};
  
  for (const caseId of cases) {
    const chromePath = join(oracleDir, `${caseId}.png`);
    const rustkitPath = opts.rustkit || join(capturesDir, `${caseId}.ppm`);
    const diffPath = join(diffsDir, `${caseId}.diff.png`);
    
    console.log(`  Comparing ${caseId}...`, '');
    
    if (!existsSync(chromePath)) {
      results[caseId] = { success: false, error: 'No Chrome baseline' };
      console.log('SKIP (no Chrome baseline)');
      continue;
    }
    
    if (!existsSync(rustkitPath)) {
      results[caseId] = { success: false, error: 'No RustKit capture' };
      console.log('SKIP (no RustKit capture)');
      continue;
    }
    
    try {
      const result = await comparePixels(chromePath, rustkitPath, diffPath);
      results[caseId] = {
        success: true,
        diff_pct: result.diffPercent,
        diff_pixels: result.diffPixels,
        total_pixels: result.totalPixels,
        diff_path: diffPath,
        passed: result.diffPercent <= opts.threshold,
      };
      
      const status = result.diffPercent <= opts.threshold ? '✓' : '✗';
      console.log(`${status} ${result.diffPercent.toFixed(2)}% diff`);
    } catch (err) {
      results[caseId] = { success: false, error: err.message };
      console.log(`FAIL: ${err.message}`);
    }
  }
  
  // Save comparison results
  const resultsPath = join(opts.output, 'oracle_results.json');
  writeFileSync(resultsPath, JSON.stringify({
    timestamp: new Date().toISOString(),
    threshold: opts.threshold,
    cases: results,
  }, null, 2));
  
  // Print summary
  const successful = Object.values(results).filter(r => r.success);
  const passed = successful.filter(r => r.passed);
  const avgDiff = successful.length > 0 
    ? successful.reduce((sum, r) => sum + r.diff_pct, 0) / successful.length 
    : 100;
  
  console.log(`\n--- Summary ---`);
  console.log(`Passed: ${passed.length}/${successful.length} (threshold: ${opts.threshold}%)`);
  console.log(`Average Diff: ${avgDiff.toFixed(2)}%`);
  console.log(`Results saved to: ${resultsPath}`);
  
  return results;
}

async function runFull(opts) {
  console.log(`\n=== Chromium Oracle: Full Pipeline ===\n`);
  
  // Step 1: Capture Chrome baselines
  const captureResults = await runCapture(opts);
  
  // Step 2: Compare with RustKit
  const compareResults = await runCompare(opts);
  
  return { capture: captureResults, compare: compareResults };
}

async function runStyles(opts) {
  const cases = getCasesToRun(opts);
  const outputDir = join(opts.output, 'computed-styles');
  mkdirSync(outputDir, { recursive: true });
  
  console.log(`\n=== Chromium Oracle: Export Computed Styles ===`);
  console.log(`Cases: ${cases.join(', ')}`);
  console.log(`Output: ${outputDir}\n`);
  
  const results = {};
  
  for (const caseId of cases) {
    const caseInfo = CASES[caseId];
    if (!caseInfo) {
      console.error(`Unknown case: ${caseId}`);
      continue;
    }
    
    const htmlPath = join(REPO_ROOT, caseInfo.path);
    const outputPath = join(outputDir, `${caseId}.styles.json`);
    
    console.log(`  Exporting ${caseId}...`, '');
    
    try {
      const styles = await exportStyles(htmlPath, caseInfo.width, caseInfo.height);
      writeFileSync(outputPath, JSON.stringify(styles, null, 2));
      results[caseId] = { success: true, path: outputPath, elementCount: styles.length };
      console.log(`OK (${styles.length} elements)`);
    } catch (err) {
      results[caseId] = { success: false, error: err.message };
      console.log(`FAIL: ${err.message}`);
    }
  }
  
  console.log(`\nStyles exported to: ${outputDir}`);
  return results;
}

function printHelp() {
  console.log(`
Parity Oracle - Chromium baseline capture and pixel diff tool

Usage:
  node run_oracle.mjs <command> [options]

Commands:
  capture   Capture Chrome baselines for cases
  compare   Compare RustKit captures against Chrome baselines
  full      Run capture + compare pipeline
  styles    Export computed styles from Chrome
  help      Show this help message

Options:
  --case <name>       Run single case
  --cases <a,b,c>     Run specific cases (comma-separated)
  --scope <scope>     Case scope: top, builtins, websuite, all (default: top)
  --output <dir>      Output directory (default: parity-baseline)
  --rustkit <path>    Path to RustKit capture (for single-case compare)
  --threshold <pct>   Pass threshold percentage (default: 25)
  -v, --verbose       Verbose output

Examples:
  node run_oracle.mjs capture --scope all
  node run_oracle.mjs compare --case css-selectors
  node run_oracle.mjs full --scope builtins
  node run_oracle.mjs styles --case css-selectors
`);
}

// Main
async function main() {
  const opts = parseArgs();
  
  switch (opts.command) {
    case 'capture':
      await runCapture(opts);
      break;
    case 'compare':
      await runCompare(opts);
      break;
    case 'full':
      await runFull(opts);
      break;
    case 'styles':
      await runStyles(opts);
      break;
    case 'help':
    default:
      printHelp();
      break;
  }
}

main().catch(err => {
  console.error('Error:', err.message);
  process.exit(1);
});



