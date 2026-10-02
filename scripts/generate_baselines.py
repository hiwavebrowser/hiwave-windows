#!/usr/bin/env python3
"""
generate_baselines.py - Generate Chrome baselines for visual parity testing

This script captures Chrome baselines for all test cases:
- baseline.png: Screenshot
- computed-styles.json: CSS computed values
- layout-rects.json: DOMRect for all elements

Usage:
    python3 scripts/generate_baselines.py [--scope <scope>] [--case <name>]
    
Examples:
    python3 scripts/generate_baselines.py                    # All cases
    python3 scripts/generate_baselines.py --scope builtins   # Built-ins only
    python3 scripts/generate_baselines.py --case new_tab     # Single case
"""

import json
import os
import subprocess
import sys
from datetime import datetime
from pathlib import Path

# Case definitions
# Case tables come from the single source of truth: cases/registry.json
# (via parity_lib). This file used to carry its own diverging copy.
from parity_lib import BUILTINS, WEBSUITE, MICRO_TESTS, _cases_for_scope  # noqa: F401,E402

MICRO = MICRO_TESTS
HOLDOUT = _cases_for_scope("holdout")

REPO_ROOT = Path(__file__).parent.parent
# Campaign pin (trench/BASELINE-macos.md): Chrome for Testing 148. Keep in
# lockstep with parity_test.py — these two diverging is how three cases
# spent the campaign measured against wrong-dimension baselines.
BASELINES_DIR = REPO_ROOT / "baselines" / os.environ.get("PARITY_BASELINE_SET", "chrome-148")
ORACLE_SCRIPT = REPO_ROOT / "tools" / "parity_oracle" / "capture_baseline.mjs"


def get_git_commit():
    """Get current git commit hash."""
    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            timeout=5,
            cwd=REPO_ROOT, encoding="utf-8"
        )
        if result.returncode == 0:
            return result.stdout.strip()[:12]
    except Exception:
        pass
    return "unknown"


def check_node_deps():
    """Check if Node.js dependencies are installed."""
    node_modules = REPO_ROOT / "tools" / "parity_oracle" / "node_modules"
    if not node_modules.exists():
        print("Error: Node.js dependencies not installed.")
        print("Run: cd tools/parity_oracle && npm install")
        return False
    return True


def capture_case(case_id: str, html_path: str, width: int, height: int, output_dir: Path) -> dict:
    """Capture baseline for a single case using Node.js oracle."""
    case_dir = output_dir / case_id
    case_dir.mkdir(parents=True, exist_ok=True)
    
    # Use Node.js capture_baseline.mjs
    cmd = [
        "node", "-e",
        f"""
import {{ captureBaseline }} from './capture_baseline.mjs';
const result = await captureBaseline(
    {json.dumps(str(REPO_ROOT / html_path))},
    {json.dumps(str(case_dir))},
    {width},
    {height}
);
console.log(JSON.stringify(result));
""",
    ]
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=60,
            cwd=REPO_ROOT / "tools" / "parity_oracle", encoding="utf-8"
        )
        
        if result.returncode == 0:
            # Parse JSON output
            for line in result.stdout.strip().split('\n'):
                if line.startswith('{'):
                    return json.loads(line)
            return {"success": True, "output_dir": str(case_dir)}
        else:
            return {"success": False, "error": result.stderr[:200]}
    except subprocess.TimeoutExpired:
        return {"success": False, "error": "Timeout after 60s"}
    except Exception as e:
        return {"success": False, "error": str(e)}


def main():
    scope = "all"
    single_case = None
    
    # Parse arguments
    args = sys.argv[1:]
    i = 0
    while i < len(args):
        if args[i] == "--scope" and i + 1 < len(args):
            scope = args[i + 1]
            i += 2
        elif args[i] == "--case" and i + 1 < len(args):
            single_case = args[i + 1]
            i += 2
        elif args[i] in ["-h", "--help"]:
            print(__doc__)
            sys.exit(0)
        else:
            i += 1
    
    # Check dependencies
    if not check_node_deps():
        sys.exit(1)
    
    print("=" * 60)
    print("Chrome Baseline Generator")
    print("=" * 60)
    print(f"Output: {BASELINES_DIR}")
    print(f"Scope: {scope}")
    if single_case:
        print(f"Single case: {single_case}")
    print(f"Timestamp: {datetime.now().isoformat()}")
    print()
    
    # Determine cases to run (scope groups come from the registry)
    scope_groups = [
        ("builtins", BUILTINS),
        ("websuite", WEBSUITE),
        ("micro", MICRO),
        ("holdout", HOLDOUT),
    ]
    cases = []
    if single_case:
        for case_type, group in scope_groups:
            for c in group:
                if c[0] == single_case:
                    cases = [(c[0], c[1], c[2], c[3], case_type)]
        if not cases:
            print(f"Error: Unknown case '{single_case}'")
            sys.exit(1)
    else:
        for case_type, group in scope_groups:
            if scope in ["all", case_type]:
                cases.extend([(c[0], c[1], c[2], c[3], case_type) for c in group])

    # Capture baselines
    results = {case_type: {} for case_type, _ in scope_groups}
    
    for case_id, html_path, width, height, case_type in cases:
        output_dir = BASELINES_DIR / case_type
        print(f"  Capturing {case_id}...", end=" ", flush=True)
        
        result = capture_case(case_id, html_path, width, height, output_dir)
        results[case_type][case_id] = result
        
        if result.get("success"):
            print(f"OK ({result.get('elementCount', '?')} elements)")
        else:
            print(f"FAIL: {result.get('error', 'Unknown')[:50]}")
    
    # Update metadata
    metadata_path = BASELINES_DIR.parent / "metadata.json"
    if metadata_path.exists():
        with open(metadata_path, encoding="utf-8") as f:
            metadata = json.load(f)
    else:
        metadata = {}
    
    metadata["last_updated"] = datetime.now().isoformat()
    metadata["git_commit"] = get_git_commit()
    
    with open(metadata_path, "w", encoding="utf-8") as f:
        json.dump(metadata, f, indent=2)
    
    # Summary
    print()
    print("=" * 60)
    print("Summary")
    print("=" * 60)
    
    total = 0
    passed = 0
    for case_type, case_results in results.items():
        for case_id, result in case_results.items():
            total += 1
            if result.get("success"):
                passed += 1
    
    print(f"Captured: {passed}/{total}")
    print(f"Metadata updated: {metadata_path}")
    print()
    
    if passed < total:
        print("Some captures failed. Check Node.js dependencies:")
        print("  cd tools/parity_oracle && npm install")
        sys.exit(1)


if __name__ == "__main__":
    main()



