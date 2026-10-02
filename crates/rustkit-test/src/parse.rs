//! HTML/CSS parse tests.

use crate::{TestError, TestResult, TestSummary};
use rustkit_dom::{Document, Node, NodeType};
use std::fs;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;
use tracing::debug;

/// Parse test runner.
pub struct ParseTestRunner;

impl ParseTestRunner {
    pub fn new() -> Self {
        Self
    }

    /// Run all parse tests in a directory.
    pub fn run_all(&self, dir: &Path) -> Result<TestSummary, TestError> {
        let mut summary = TestSummary::new();

        if !dir.exists() {
            return Ok(summary);
        }

        for entry in walkdir::WalkDir::new(dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "html"))
        {
            let result = self.run_file(entry.path())?;
            summary.add(result);
        }

        Ok(summary)
    }

    /// Run a single parse test.
    pub fn run_file(&self, path: &Path) -> Result<TestResult, TestError> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        debug!(?path, "Running parse test");
        let start = Instant::now();

        // Read HTML
        let html = match fs::read_to_string(path) {
            Ok(h) => h,
            Err(e) => return Ok(TestResult::error(&name, 0, e.to_string())),
        };

        // Check for expected output file
        let expected_path = path.with_extension("expected");

        // Parse HTML
        let doc = match Document::parse_html(&html) {
            Ok(d) => d,
            Err(e) => {
                let duration = start.elapsed().as_millis() as u64;
                return Ok(TestResult::error(&name, duration, e.to_string()));
            }
        };

        let duration = start.elapsed().as_millis() as u64;

        // If no expected file, just check parsing succeeded
        if !expected_path.exists() {
            return Ok(TestResult::pass(&name, duration));
        }

        // Compare against expected output
        let expected = match fs::read_to_string(&expected_path) {
            Ok(e) => e,
            Err(e) => return Ok(TestResult::error(&name, duration, e.to_string())),
        };

        // Serialize parsed document
        let actual = format_document(&doc);

        if expected.trim() == actual.trim() {
            Ok(TestResult::pass(&name, duration))
        } else {
            Ok(TestResult::fail_with_diff(
                &name, duration, expected, actual,
            ))
        }
    }
}

impl Default for ParseTestRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Format a document as a string for comparison.
fn format_document(doc: &Document) -> String {
    let mut output = String::new();
    format_node(doc.root(), &mut output, 0);
    output
}

fn format_node(node: &Rc<Node>, output: &mut String, indent: usize) {
    let prefix = "  ".repeat(indent);

    match &node.node_type {
        NodeType::Document => {
            output.push_str("#document\n");
        }
        NodeType::Element {
            tag_name,
            attributes,
            ..
        } => {
            output.push_str(&format!("{}<{}>\n", prefix, tag_name));
            for (name, value) in attributes {
                output.push_str(&format!("{}  {}=\"{}\"\n", prefix, name, value));
            }
        }
        NodeType::Text(text) => {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                output.push_str(&format!("{}\"{}\"", prefix, trimmed));
                output.push('\n');
            }
        }
        NodeType::Comment(text) => {
            output.push_str(&format!("{}<!-- {} -->\n", prefix, text));
        }
        NodeType::DocumentType { .. } => {
            output.push_str(&format!("{}<!DOCTYPE>\n", prefix));
        }
        NodeType::ProcessingInstruction { .. } => {
            output.push_str(&format!("{}<?...?>\n", prefix));
        }
        NodeType::DocumentFragment => {
            output.push_str(&format!("{}#document-fragment\n", prefix));
        }
    }

    for child in node.children() {
        format_node(&child, output, indent + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_runner_initialization() {
        // Just verify the runner can be constructed
        let _ = ParseTestRunner::new();
    }
}
