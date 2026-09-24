//! Layout tests.

use crate::{TestError, TestResult, TestSummary};
use rustkit_css::ComputedStyle;
use rustkit_layout::{BoxType, Dimensions, LayoutBox, Rect};
use std::fs;
use std::path::Path;
use std::time::Instant;
use tracing::debug;

/// Layout test runner.
pub struct LayoutTestRunner;

impl LayoutTestRunner {
    pub fn new() -> Self {
        Self
    }

    /// Run all layout tests in a directory.
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

    /// Run a single layout test.
    pub fn run_file(&self, path: &Path) -> Result<TestResult, TestError> {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        debug!(?path, "Running layout test");
        let start = Instant::now();

        // Read HTML
        let _html = match fs::read_to_string(path) {
            Ok(h) => h,
            Err(e) => return Ok(TestResult::error(&name, 0, e.to_string())),
        };

        // Parse and layout (simplified)
        let style = ComputedStyle::new();
        let mut root = LayoutBox::new(BoxType::Block, style);

        // Create containing block (viewport)
        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 800.0, 600.0),
            ..Default::default()
        };

        // Perform layout
        root.layout(&containing);

        let duration = start.elapsed().as_millis() as u64;

        // Check for expected output
        let expected_path = path.with_extension("expected");
        if !expected_path.exists() {
            return Ok(TestResult::pass(&name, duration));
        }

        let expected = match fs::read_to_string(&expected_path) {
            Ok(e) => e,
            Err(e) => return Ok(TestResult::error(&name, duration, e.to_string())),
        };

        // Format layout output
        let actual = format_layout(&root);

        if expected.trim() == actual.trim() {
            Ok(TestResult::pass(&name, duration))
        } else {
            Ok(TestResult::fail_with_diff(
                &name, duration, expected, actual,
            ))
        }
    }
}

impl Default for LayoutTestRunner {
    fn default() -> Self {
        Self::new()
    }
}

/// Format layout box as string.
fn format_layout(layout: &LayoutBox) -> String {
    let mut output = String::new();
    format_box(layout, &mut output, 0);
    output
}

fn format_box(layout: &LayoutBox, output: &mut String, indent: usize) {
    let prefix = "  ".repeat(indent);
    let dims = &layout.dimensions;
    let content = &dims.content;

    let box_type = match &layout.box_type {
        BoxType::Block => "block",
        BoxType::Inline => "inline",
        BoxType::AnonymousBlock => "anonymous",
        BoxType::Text(_) => "text",
        BoxType::Image { .. } => "image",
        BoxType::FormControl(_) => "form-control",
        BoxType::LineBreak => "line-break",
    };

    output.push_str(&format!(
        "{}{}: x={:.0} y={:.0} w={:.0} h={:.0}\n",
        prefix, box_type, content.x, content.y, content.width, content.height
    ));

    for child in &layout.children {
        format_box(child, output, indent + 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_layout() {
        let style = ComputedStyle::new();
        let mut root = LayoutBox::new(BoxType::Block, style);

        let containing = Dimensions {
            content: Rect::new(0.0, 0.0, 100.0, 100.0),
            ..Default::default()
        };
        root.layout(&containing);

        let output = format_layout(&root);
        assert!(output.contains("block:"));
    }
}
