//! # RustKit HTML
//!
//! HTML5 parser for the RustKit browser engine.
//!
//! This crate provides a tokenizer and tree builder that work together
//! to parse HTML into a DOM tree via a sink interface.
//!
//! ## Features
//!
//! - Full document parsing via [`parse`]
//! - Fragment parsing via [`parse_fragment`] (for innerHTML, insertAdjacentHTML)
//! - Quirks mode detection based on doctype
//! - Table parsing with implicit element insertion
//! - Adoption Agency Algorithm for misnested formatting elements
//!
//! ## Example
//!
//! ```ignore
//! use rustkit_html::{parse, TreeSink};
//!
//! let html = "<html><body><p>Hello</p></body></html>";
//! let sink = MySink::new();
//! let result = parse(html, sink).unwrap();
//! ```

pub mod entities;
pub mod tokenizer;
pub mod tree_builder;

// Re-export commonly used types from tree_builder
pub use tree_builder::{FragmentContext, QuirksMode};

use thiserror::Error;

/// Errors that can occur during HTML parsing.
#[derive(Error, Debug)]
pub enum ParseError {
    #[error("Tokenizer error: {0}")]
    TokenizerError(String),

    #[error("Tree builder error: {0}")]
    TreeBuilderError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Result type for HTML parsing operations.
pub type ParseResult<T> = Result<T, ParseError>;

/// Trait for receiving parse events and building a tree structure.
///
/// This trait is implemented by DOM builders that want to construct
/// a tree from HTML parsing events.
pub trait TreeSink {
    /// The type used to identify nodes in the tree.
    type NodeId: Clone;

    /// Called when a doctype declaration is encountered.
    fn doctype(&mut self, name: String, public_id: String, system_id: String);

    /// Called when a start tag is encountered.
    /// Returns the node ID for the newly created element.
    fn start_element(
        &mut self,
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    ) -> Self::NodeId;

    /// Called when an end tag is encountered.
    fn end_element(&mut self, name: String);

    /// Called when text content is encountered.
    fn text(&mut self, data: String);

    /// Called when a comment is encountered.
    fn comment(&mut self, data: String);

    /// Called to get the current node being processed.
    fn current_node(&self) -> Option<Self::NodeId>;

    /// Called to check if the parser is in a specific context.
    fn in_scope(&self, tag_name: &str) -> bool;

    /// Called to pop elements until a specific tag is found.
    fn pop_until(&mut self, tag_name: &str);

    /// Called to close a p element if one is in button scope.
    fn close_p_element_in_button_scope(&mut self);

    /// Called to reconstruct active formatting elements.
    fn reconstruct_active_formatting_elements(&mut self);

    // ==================== AAA (Adoption Agency Algorithm) Methods ====================

    /// Create an element without appending it to the tree.
    /// Used by AAA to create cloned formatting elements.
    fn create_element(&mut self, name: String, attrs: Vec<(String, String)>) -> Self::NodeId;

    /// Append a node as a child of the given parent.
    /// Used by AAA for reparenting operations.
    fn append_child(&mut self, parent: Self::NodeId, child: Self::NodeId);

    /// Remove a node from its parent (detach it from the tree).
    /// The node still exists but is no longer in the tree.
    fn remove_from_parent(&mut self, node: Self::NodeId);

    /// Move all children from one node to another.
    /// Used by AAA to transfer children during adoption.
    fn reparent_children(&mut self, from: Self::NodeId, to: Self::NodeId);

    /// Insert a node before a reference node within a parent.
    /// If reference is None, append at the end.
    fn insert_before(&mut self, parent: Self::NodeId, node: Self::NodeId, reference: Option<Self::NodeId>);

    /// Get the parent of a node, if any.
    fn get_parent(&self, node: Self::NodeId) -> Option<Self::NodeId>;

    /// Get the tag name of a node.
    fn get_tag_name(&self, node: Self::NodeId) -> Option<String>;

    // ==================== Extended TreeSink Methods ====================

    /// Insert a node at the foster parent location (for table foster parenting).
    /// When content appears inside a table but isn't table-related, it gets
    /// "foster parented" to just before the table element.
    ///
    /// The `table` parameter is the table element, and `node` is the content to foster parent.
    /// Default implementation just appends to the table's parent.
    fn foster_parent(&mut self, table: Self::NodeId, node: Self::NodeId) {
        if let Some(parent) = self.get_parent(table.clone()) {
            self.insert_before(parent, node, Some(table));
        }
    }

    /// Insert a text node at the foster parent location: immediately before
    /// `table` in the table's parent (HTML §13.2.6.1, "appropriate place for
    /// inserting a node" with foster parenting). Used for non-whitespace
    /// text that appears directly inside a table, tbody, tfoot, thead or tr.
    ///
    /// The default is built only from the primitives above, so existing
    /// sinks get it without changes: the text is created inside a scratch
    /// element, the scratch element is detached, and then the table is
    /// moved after the text. That relies on the table being its parent's
    /// last child, which holds while the table is open (everything else
    /// goes inside the table or, fostered, before it). Sinks that can create
    /// a detached text node should override this with a direct insert.
    fn foster_parent_text(&mut self, table: Self::NodeId, data: String) {
        let Some(parent) = self.get_parent(table.clone()) else {
            self.text(data);
            return;
        };
        let scratch = self.start_element("span".to_string(), Vec::new(), false);
        self.text(data);
        self.end_element("span".to_string());
        self.remove_from_parent(scratch.clone());
        self.remove_from_parent(table.clone());
        self.reparent_children(scratch, parent.clone());
        self.append_child(parent, table);
    }

    /// Called when a parse error is encountered.
    /// The default implementation ignores errors.
    fn parse_error(&mut self, _error: &str) {
        // Default: ignore parse errors
    }

    /// Set the document quirks mode.
    /// Called after DOCTYPE processing.
    fn set_quirks_mode(&mut self, _mode: QuirksMode) {
        // Default: ignore quirks mode
    }

    /// Get the template contents document fragment.
    /// For template elements, returns the fragment that holds template contents.
    fn template_contents(&self, _template: Self::NodeId) -> Option<Self::NodeId> {
        // Default: templates not supported
        None
    }

    /// Check if an element is a template element.
    fn is_template_element(&self, node: Self::NodeId) -> bool {
        self.get_tag_name(node).is_some_and(|n| n == "template")
    }

    /// Mark a script element as "already started" per HTML5 spec.
    /// This prevents script execution on subsequent insertions.
    fn mark_script_already_started(&mut self, _script: Self::NodeId) {
        // Default: no-op
    }

    /// Get the encoding confidence (tentative, certain, irrelevant).
    /// Returns "irrelevant" by default.
    fn get_encoding_confidence(&self) -> &'static str {
        "irrelevant"
    }
}

/// Parse HTML from a string using the provided sink.
pub fn parse<S: TreeSink>(html: &str, mut sink: S) -> ParseResult<S> {
    let tokens = tokenizer::tokenize(html)?;
    sink = tree_builder::build_tree(tokens, sink)?;
    Ok(sink)
}

/// Parse HTML from bytes using the provided sink.
pub fn parse_bytes<S: TreeSink>(html: &[u8], sink: S) -> ParseResult<S> {
    let html_str = std::str::from_utf8(html)
        .map_err(|e| ParseError::TokenizerError(format!("Invalid UTF-8: {}", e)))?;
    parse(html_str, sink)
}

/// Parse an HTML fragment in the context of a given element.
///
/// This is used for innerHTML, insertAdjacentHTML, and similar APIs where
/// HTML is parsed as if it were the contents of a specific element.
///
/// # Arguments
///
/// * `html` - The HTML fragment to parse
/// * `sink` - The tree sink to build the DOM
/// * `context_element` - The tag name of the context element (e.g., "div", "body")
///
/// # Example
///
/// ```ignore
/// // Parse as if the content were inside a <div>
/// let sink = MySink::new();
/// let result = parse_fragment("<p>Hello</p>", sink, "div").unwrap();
/// ```
pub fn parse_fragment<S: TreeSink>(
    html: &str,
    sink: S,
    context_element: &str,
) -> ParseResult<S> {
    let tokens = tokenizer::tokenize(html)?;
    let context = FragmentContext::new(context_element);
    tree_builder::build_tree_fragment(tokens, sink, context)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestNode {
        name: String,
        attrs: Vec<(String, String)>,
        children: Vec<TestNode>,
    }

    struct TestSink {
        nodes: Vec<TestNode>,
        stack: Vec<usize>,
    }

    impl TestSink {
        fn new() -> Self {
            Self {
                nodes: vec![TestNode {
                    name: "root".to_string(),
                    attrs: vec![],
                    children: vec![],
                }],
                stack: vec![0],
            }
        }
    }

    impl TreeSink for TestSink {
        type NodeId = usize;

        fn doctype(&mut self, _name: String, _public_id: String, _system_id: String) {
            // Test sink ignores doctype
        }

        fn start_element(
            &mut self,
            name: String,
            attrs: Vec<(String, String)>,
            _self_closing: bool,
        ) -> Self::NodeId {
            let node = TestNode {
                name,
                attrs,
                children: vec![],
            };
            let node_id = self.nodes.len();
            self.nodes.push(node);
            
            if let Some(&_parent_id) = self.stack.last() {
                // In a real implementation, we'd add to parent's children
                // For this test, we just track the structure
            }
            
            self.stack.push(node_id);
            node_id
        }

        fn end_element(&mut self, _name: String) {
            self.stack.pop();
        }

        fn text(&mut self, _data: String) {
            // Test sink ignores text for now
        }

        fn comment(&mut self, _data: String) {
            // Test sink ignores comments
        }

        fn current_node(&self) -> Option<Self::NodeId> {
            self.stack.last().copied()
        }

        fn in_scope(&self, _tag_name: &str) -> bool {
            false
        }

        fn pop_until(&mut self, _tag_name: &str) {
            // Simplified for tests
        }

        fn close_p_element_in_button_scope(&mut self) {
            // Simplified for tests
        }

        fn reconstruct_active_formatting_elements(&mut self) {
            // Simplified for tests
        }

        // AAA methods - simplified implementations for testing

        fn create_element(&mut self, name: String, attrs: Vec<(String, String)>) -> Self::NodeId {
            let node = TestNode {
                name,
                attrs,
                children: vec![],
            };
            let node_id = self.nodes.len();
            self.nodes.push(node);
            node_id
        }

        fn append_child(&mut self, _parent: Self::NodeId, _child: Self::NodeId) {
            // Simplified for tests - in real impl would modify parent's children
        }

        fn remove_from_parent(&mut self, _node: Self::NodeId) {
            // Simplified for tests
        }

        fn reparent_children(&mut self, _from: Self::NodeId, _to: Self::NodeId) {
            // Simplified for tests
        }

        fn insert_before(&mut self, _parent: Self::NodeId, _node: Self::NodeId, _reference: Option<Self::NodeId>) {
            // Simplified for tests
        }

        fn get_parent(&self, _node: Self::NodeId) -> Option<Self::NodeId> {
            // Simplified for tests
            None
        }

        fn get_tag_name(&self, node: Self::NodeId) -> Option<String> {
            self.nodes.get(node).map(|n| n.name.clone())
        }
    }

    #[test]
    fn test_basic_parse() {
        let html = "<html><body><p>Hello</p></body></html>";
        let sink = TestSink::new();
        parse(html, sink).unwrap();
        // Basic smoke test - more detailed tests in submodules
    }

    #[test]
    fn test_empty_html() {
        let html = "";
        let sink = TestSink::new();
        parse(html, sink).unwrap();
    }

    #[test]
    fn test_parse_bytes() {
        let html = b"<html></html>";
        let sink = TestSink::new();
        parse_bytes(html, sink).unwrap();
    }
}

