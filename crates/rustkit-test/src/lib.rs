//! # RustKit Test
//!
//! WPT-style test harness for the RustKit browser engine.
//!
//! ## Test Types
//!
//! 1. **Parse tests**: Verify HTML/CSS parsing produces correct trees
//! 2. **Style tests**: Verify CSS cascade and computed values
//! 3. **Layout tests**: Verify box dimensions and positions
//! 4. **Reference tests**: Compare rendered output against references
//!
//! ## Usage
//!
//! ```rust,ignore
//! use rustkit_test::{TestHarness, TestResult};
//!
//! let harness = TestHarness::new();
//! let results = harness.run_all("tests/wpt")?;
//! println!("Passed: {}/{}", results.passed, results.total);
//! ```

#![allow(dead_code, unused_imports)]

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{debug, info};

pub mod events;
pub mod layout;
pub mod navigation;
pub mod parse;
pub mod reftest;
pub mod security;
pub mod style;

pub use layout::LayoutTestRunner;
pub use parse::ParseTestRunner;
pub use reftest::RefTestRunner;
pub use style::StyleTestRunner;

/// Errors that can occur in testing.
#[derive(Error, Debug)]
pub enum TestError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Assertion failed: {0}")]
    Assertion(String),

    #[error("Test not found: {0}")]
    NotFound(String),

    #[error("Invalid test format: {0}")]
    InvalidFormat(String),
}

/// Test result status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TestStatus {
    Pass,
    Fail,
    Skip,
    Timeout,
    Error,
}

/// Individual test result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub status: TestStatus,
    pub duration_ms: u64,
    pub message: Option<String>,
    pub expected: Option<String>,
    pub actual: Option<String>,
}

impl TestResult {
    pub fn pass(name: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Pass,
            duration_ms,
            message: None,
            expected: None,
            actual: None,
        }
    }

    pub fn fail(name: impl Into<String>, duration_ms: u64, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Fail,
            duration_ms,
            message: Some(message.into()),
            expected: None,
            actual: None,
        }
    }

    pub fn fail_with_diff(
        name: impl Into<String>,
        duration_ms: u64,
        expected: impl Into<String>,
        actual: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Fail,
            duration_ms,
            message: Some("Output mismatch".into()),
            expected: Some(expected.into()),
            actual: Some(actual.into()),
        }
    }

    pub fn skip(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Skip,
            duration_ms: 0,
            message: Some(reason.into()),
            expected: None,
            actual: None,
        }
    }

    pub fn error(name: impl Into<String>, duration_ms: u64, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: TestStatus::Error,
            duration_ms,
            message: Some(message.into()),
            expected: None,
            actual: None,
        }
    }
}

/// Aggregated test results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub skipped: usize,
    pub errors: usize,
    pub duration_ms: u64,
    pub results: Vec<TestResult>,
}

impl TestSummary {
    pub fn new() -> Self {
        Self {
            total: 0,
            passed: 0,
            failed: 0,
            skipped: 0,
            errors: 0,
            duration_ms: 0,
            results: Vec::new(),
        }
    }

    pub fn add(&mut self, result: TestResult) {
        self.total += 1;
        self.duration_ms += result.duration_ms;

        match result.status {
            TestStatus::Pass => self.passed += 1,
            TestStatus::Fail => self.failed += 1,
            TestStatus::Skip => self.skipped += 1,
            TestStatus::Timeout | TestStatus::Error => self.errors += 1,
        }

        self.results.push(result);
    }

    pub fn merge(&mut self, other: TestSummary) {
        self.total += other.total;
        self.passed += other.passed;
        self.failed += other.failed;
        self.skipped += other.skipped;
        self.errors += other.errors;
        self.duration_ms += other.duration_ms;
        self.results.extend(other.results);
    }

    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 / self.total as f64 * 100.0
        }
    }
}

impl Default for TestSummary {
    fn default() -> Self {
        Self::new()
    }
}

/// Test configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    /// Test directory path.
    pub test_dir: PathBuf,
    /// Pattern to match test files.
    pub pattern: String,
    /// Timeout per test in milliseconds.
    pub timeout_ms: u64,
    /// Skip tests matching these patterns.
    pub skip_patterns: Vec<String>,
    /// Only run tests matching these patterns.
    pub filter_patterns: Vec<String>,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            test_dir: PathBuf::from("tests/wpt"),
            pattern: "*.html".to_string(),
            timeout_ms: 5000,
            skip_patterns: Vec::new(),
            filter_patterns: Vec::new(),
        }
    }
}

/// Main test harness.
pub struct TestHarness {
    #[allow(dead_code)]
    config: TestConfig,
    parse_runner: ParseTestRunner,
    style_runner: StyleTestRunner,
    layout_runner: LayoutTestRunner,
    reftest_runner: RefTestRunner,
}

impl TestHarness {
    /// Create a new test harness with default configuration.
    pub fn new() -> Self {
        Self::with_config(TestConfig::default())
    }

    /// Create a new test harness with custom configuration.
    pub fn with_config(config: TestConfig) -> Self {
        Self {
            config,
            parse_runner: ParseTestRunner::new(),
            style_runner: StyleTestRunner::new(),
            layout_runner: LayoutTestRunner::new(),
            reftest_runner: RefTestRunner::new(),
        }
    }

    /// Run all tests in the test directory.
    pub fn run_all(&self, test_dir: impl AsRef<Path>) -> Result<TestSummary, TestError> {
        let test_dir = test_dir.as_ref();
        info!(?test_dir, "Running all tests");

        let mut summary = TestSummary::new();

        // Run parse tests
        let parse_dir = test_dir.join("parse");
        if parse_dir.exists() {
            let parse_results = self.parse_runner.run_all(&parse_dir)?;
            summary.merge(parse_results);
        }

        // Run style tests
        let style_dir = test_dir.join("style");
        if style_dir.exists() {
            let style_results = self.style_runner.run_all(&style_dir)?;
            summary.merge(style_results);
        }

        // Run layout tests
        let layout_dir = test_dir.join("layout");
        if layout_dir.exists() {
            let layout_results = self.layout_runner.run_all(&layout_dir)?;
            summary.merge(layout_results);
        }

        // Run reference tests
        let reftest_dir = test_dir.join("reftest");
        if reftest_dir.exists() {
            let reftest_results = self.reftest_runner.run_all(&reftest_dir)?;
            summary.merge(reftest_results);
        }

        info!(
            total = summary.total,
            passed = summary.passed,
            failed = summary.failed,
            skipped = summary.skipped,
            "Test run complete"
        );

        Ok(summary)
    }

    /// Run a specific test file.
    pub fn run_file(&self, path: impl AsRef<Path>) -> Result<TestResult, TestError> {
        let path = path.as_ref();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        debug!(?path, "Running test file");

        // Determine test type from path
        let parent = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str());

        match parent {
            Some("parse") => self.parse_runner.run_file(path),
            Some("style") => self.style_runner.run_file(path),
            Some("layout") => self.layout_runner.run_file(path),
            Some("reftest") => self.reftest_runner.run_file(path),
            _ => Ok(TestResult::skip(name, "Unknown test type")),
        }
    }
}

impl Default for TestHarness {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to diff two strings.
pub fn diff_strings(expected: &str, actual: &str) -> String {
    use similar::{ChangeTag, TextDiff};

    let diff = TextDiff::from_lines(expected, actual);
    let mut output = String::new();

    for change in diff.iter_all_changes() {
        let sign = match change.tag() {
            ChangeTag::Delete => "-",
            ChangeTag::Insert => "+",
            ChangeTag::Equal => " ",
        };
        output.push_str(&format!("{}{}", sign, change));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_pass() {
        let result = TestResult::pass("test1", 100);
        assert_eq!(result.status, TestStatus::Pass);
        assert_eq!(result.duration_ms, 100);
    }

    #[test]
    fn test_result_fail() {
        let result = TestResult::fail("test1", 100, "assertion failed");
        assert_eq!(result.status, TestStatus::Fail);
        assert!(result.message.is_some());
    }

    #[test]
    fn test_summary() {
        let mut summary = TestSummary::new();
        summary.add(TestResult::pass("test1", 100));
        summary.add(TestResult::pass("test2", 200));
        summary.add(TestResult::fail("test3", 150, "failed"));

        assert_eq!(summary.total, 3);
        assert_eq!(summary.passed, 2);
        assert_eq!(summary.failed, 1);
        assert!((summary.pass_rate() - 66.67).abs() < 1.0);
    }

    #[test]
    fn test_diff_strings() {
        let expected = "line1\nline2\nline3";
        let actual = "line1\nmodified\nline3";
        let diff = diff_strings(expected, actual);
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+modified"));
    }
}
