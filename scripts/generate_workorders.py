#!/usr/bin/env python3
"""
generate_workorders.py - Generate WorkOrders from layout comparison results

This script reads the layout_comparison.json from a parity run and generates
WorkOrder JSON files for the AI orchestrator based on issue categories.

Usage:
    python3 scripts/generate_workorders.py <run_dir> [--output-dir <dir>]
"""

import json
import sys
from pathlib import Path
from datetime import datetime
from typing import Dict, List, Any

def create_workorder(
    category: str,
    issues: List[Dict[str, Any]],
    run_dir: str,
) -> Dict[str, Any]:
    """Create a WorkOrder for a category of issues."""
    
    # Map category to workorder details
    category_details = {
        "flex-positioning": {
            "title": "Fix Flex Item Positioning",
            "description": "Flex items are positioned incorrectly (at origin or wrong location)",
            "priority": "high",
            "scope": ["crates/rustkit-layout/src/flex.rs", "crates/rustkit-layout/src/lib.rs"],
            "acceptance_gates": [
                "All flex items have non-zero positions when they should",
                "Flex items are positioned correctly relative to their container",
                "Nested flex containers work correctly"
            ]
        },
        "flex-form-controls": {
            "title": "Fix Form Control Sizing in Flex",
            "description": "Form controls (inputs, buttons) have zero size or wrong dimensions in flex containers",
            "priority": "high",
            "scope": ["crates/rustkit-layout/src/flex.rs", "crates/rustkit-layout/src/forms.rs"],
            "acceptance_gates": [
                "Form controls have correct intrinsic dimensions",
                "Form controls respect flex-basis and flex-grow",
                "Checkboxes and radio buttons are correctly sized"
            ]
        },
        "layout-overflow": {
            "title": "Fix Layout Overflow Issues",
            "description": "Elements are positioned outside the viewport when they shouldn't be",
            "priority": "medium",
            "scope": ["crates/rustkit-layout/src/lib.rs", "crates/rustkit-layout/src/scroll.rs"],
            "acceptance_gates": [
                "Content is positioned within the viewport",
                "Overflow handling is correct",
                "Scroll containers work properly"
            ]
        },
        "text-layout": {
            "title": "Fix Text Layout Issues",
            "description": "Text box count or positioning doesn't match Chromium",
            "priority": "medium",
            "scope": ["crates/rustkit-layout/src/text.rs", "crates/rustkit-text/src/"],
            "acceptance_gates": [
                "Text runs match Chromium count",
                "Text is positioned correctly",
                "Line breaking matches Chromium"
            ]
        },
        "capture-size": {
            "title": "Fix Viewport/Capture Size Mismatch",
            "description": "RustKit and Chromium captures have different viewport sizes",
            "priority": "low",
            "scope": ["scripts/parity_gate.sh", "tools/websuite-baseline/"],
            "acceptance_gates": [
                "Both captures use the same viewport size",
                "Device pixel ratio is handled correctly"
            ]
        },
        "unknown": {
            "title": "Investigate Unknown Layout Issues",
            "description": "Layout issues that don't fit into known categories",
            "priority": "low",
            "scope": ["crates/rustkit-layout/"],
            "acceptance_gates": [
                "Issue is classified and assigned to a specific category",
                "Root cause is identified"
            ]
        }
    }
    
    details = category_details.get(category, category_details["unknown"])
    
    # Collect affected cases
    affected_cases = list(set(issue["case_id"] for issue in issues))
    
    # Create sample issues for context
    sample_issues = [issue["issue"] for issue in issues[:5]]
    
    workorder = {
        "id": f"layout-{category}-{datetime.now().strftime('%Y%m%d')}",
        "title": details["title"],
        "description": details["description"],
        "priority": details["priority"],
        "status": "pending",
        "created_at": datetime.now().isoformat(),
        "source": {
            "run_dir": run_dir,
            "category": category,
            "issue_count": len(issues),
            "affected_cases": affected_cases
        },
        "scope": {
            "files": details["scope"],
            "tests": [f"websuite/cases/{case}/index.html" for case in affected_cases[:3]]
        },
        "acceptance_gates": details["acceptance_gates"],
        "context": {
            "sample_issues": sample_issues,
            "total_issues": len(issues)
        }
    }
    
    return workorder

def main():
    if len(sys.argv) < 2:
        print("Usage: python3 scripts/generate_workorders.py <run_dir> [--output-dir <dir>]")
        sys.exit(1)
    
    run_dir = Path(sys.argv[1])
    output_dir = Path(".ai/work_orders")
    
    if "--output-dir" in sys.argv:
        idx = sys.argv.index("--output-dir")
        if idx + 1 < len(sys.argv):
            output_dir = Path(sys.argv[idx + 1])
    
    # Load layout comparison results
    comparison_file = run_dir / "layout_comparison.json"
    if not comparison_file.exists():
        print(f"Error: Layout comparison not found: {comparison_file}")
        sys.exit(1)
    
    with open(comparison_file, encoding="utf-8") as f:
        data = json.load(f)
    
    issue_categories = data.get("issue_categories", {})
    
    if not issue_categories:
        print("No issues found, no WorkOrders to generate")
        return
    
    # Create output directory
    output_dir.mkdir(parents=True, exist_ok=True)
    
    print(f"Generating WorkOrders from: {comparison_file}")
    print(f"Output directory: {output_dir}")
    print()
    
    generated = []
    
    for category, issues in issue_categories.items():
        if not issues:
            continue
        
        workorder = create_workorder(category, issues, str(run_dir))
        
        # Save WorkOrder
        output_file = output_dir / f"{workorder['id']}.json"
        with open(output_file, "w", encoding="utf-8") as f:
            json.dump(workorder, f, indent=2)
        
        print(f"  Generated: {output_file}")
        print(f"    Category: {category}")
        print(f"    Issues: {len(issues)}")
        print(f"    Priority: {workorder['priority']}")
        print()
        
        generated.append(workorder)
    
    # Summary
    print(f"Generated {len(generated)} WorkOrders")
    
    # Print priority breakdown
    priority_counts = {}
    for wo in generated:
        p = wo["priority"]
        priority_counts[p] = priority_counts.get(p, 0) + 1
    
    print("\nBy Priority:")
    for priority in ["high", "medium", "low"]:
        if priority in priority_counts:
            print(f"  {priority}: {priority_counts[priority]}")

if __name__ == "__main__":
    main()

