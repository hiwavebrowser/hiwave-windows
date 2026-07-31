//! Reference (visual) tests.
//!
//! Reference tests compare rendered output against reference images
//! or reference HTML that should produce identical output.
//!
//! # THIS RUNNER DOES NOT RENDER - see rustkit-engine instead
//!
//! `run_single` below compares NORMALIZED HTML TEXT, not rendered output.
//! That is inverted from what a reference test is for: the point of a reftest
//! is that two DIFFERENT source documents produce the SAME rendering, so a
//! genuine pair FAILS this comparison while a trivially identical pair passes.
//! The checked-in `color-red` pair (`color: red` vs `color: #ff0000`) is
//! exactly such a genuine pair.
//!
//! Nothing in the workspace depends on this crate, so it is not run.
//!
//! The working Tier 1 harness is `display_list_reftests` in
//! `crates/rustkit-engine/src/lib.rs`. It compares DISPLAY LISTS, needs no GPU,
//! runs in CI, and carries a negative control that aborts the run if the
//! comparison is inert. Use that. This module is left in place rather than
//! deleted because removing a public API is a separate decision, but do not
//! mistake it for coverage.

use crate::{TestError, TestResult, TestSummary};
use std::fs;
use std::path::Path;
use std::time::Instant;
use tracing::debug;

/// Reference test types.
#[derive(Debug, Clone, Copy)]
pub enum RefTestType {
    /// Test output should match reference.
    Match,
    /// Test output should NOT match reference.
    Mismatch,
}

/// Reference test runner.
pub struct RefTestRunner;

impl RefTestRunner {
    pub fn new() -> Self {
        Self
    }

    /// Run all reference tests in a directory.
    pub fn run_all(&self, dir: &Path) -> Result<TestSummary, TestError> {
        let mut summary = TestSummary::new();

        if !dir.exists() {
            return Ok(summary);
        }

        // Look for reftest.list files
        let list_path = dir.join("reftest.list");
        if list_path.exists() {
            let results = self.run_list(&list_path)?;
            summary.merge(results);
        }

        // Also run any standalone HTML files
        for entry in walkdir::WalkDir::new(dir)
            .max_depth(1)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "html"))
        {
            let result = self.run_file(entry.path())?;
            summary.add(result);
        }

        Ok(summary)
    }

    /// Run tests from a reftest.list file.
    pub fn run_list(&self, path: &Path) -> Result<TestSummary, TestError> {
        let mut summary = TestSummary::new();

        let content = fs::read_to_string(path)?;
        let dir = path.parent().unwrap_or(Path::new("."));

        for line in content.lines() {
            let line = line.trim();

            // Skip comments and empty lines
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Parse: == test.html reference.html
            // or:    != test.html reference.html
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 3 {
                continue;
            }

            let (test_type, test_file, ref_file) = match parts[0] {
                "==" => (RefTestType::Match, parts[1], parts[2]),
                "!=" => (RefTestType::Mismatch, parts[1], parts[2]),
                _ => continue,
            };

            let test_path = dir.join(test_file);
            let ref_path = dir.join(ref_file);

            let result = self.run_comparison(&test_path, &ref_path, test_type)?;
            summary.add(result);
        }

        Ok(summary)
    }

    /// Run a single reference test.
    pub fn run_file(&self, path: &Path) -> Result<TestResult, TestError> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Look for corresponding -ref.html file
        let ref_name = name.replace(".html", "-ref.html");
        let ref_path = path.with_file_name(ref_name);

        if !ref_path.exists() {
            return Ok(TestResult::skip(&name, "No reference file found"));
        }

        self.run_comparison(path, &ref_path, RefTestType::Match)
    }

    /// Compare test output against reference.
    fn run_comparison(
        &self,
        test_path: &Path,
        ref_path: &Path,
        test_type: RefTestType,
    ) -> Result<TestResult, TestError> {
        let name = test_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        debug!(?test_path, ?ref_path, "Running reference test");
        let start = Instant::now();

        // Read both files
        let test_html = match fs::read_to_string(test_path) {
            Ok(h) => h,
            Err(e) => return Ok(TestResult::error(&name, 0, e.to_string())),
        };

        let ref_html = match fs::read_to_string(ref_path) {
            Ok(h) => h,
            Err(e) => return Ok(TestResult::error(&name, 0, e.to_string())),
        };

        // For now, just compare normalized HTML
        // In a full implementation, we would:
        // 1. Parse both documents
        // 2. Apply styles
        // 3. Layout both
        // 4. Compare display lists or rendered pixels

        let test_normalized = normalize_html(&test_html);
        let ref_normalized = normalize_html(&ref_html);

        let duration = start.elapsed().as_millis() as u64;

        let matches = test_normalized == ref_normalized;

        match test_type {
            RefTestType::Match => {
                if matches {
                    Ok(TestResult::pass(&name, duration))
                } else {
                    Ok(TestResult::fail(
                        &name,
                        duration,
                        "Output does not match reference",
                    ))
                }
            }
            RefTestType::Mismatch => {
                if !matches {
                    Ok(TestResult::pass(&name, duration))
                } else {
                    Ok(TestResult::fail(
                        &name,
                        duration,
                        "Output unexpectedly matches reference",
                    ))
                }
            }
        }
    }
}

impl Default for RefTestRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalize HTML for comparison.
fn normalize_html(html: &str) -> String {
    html.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_html() {
        let html = "  <div>  \n  test  \n  </div>  ";
        let normalized = normalize_html(html);
        assert_eq!(normalized, "<div>\ntest\n</div>");
    }
}
