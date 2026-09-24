//! # RustKit DOM
//!
//! DOM implementation for the RustKit browser engine.
//! Uses `rustkit-html` (our own HTML5 parser) and constructs a traversable DOM tree.
//!
//! ## Design Goals
//!
//! 1. **Spec-compliant parsing**: rustkit-html implements the HTML5 parsing algorithm
//! 2. **Efficient tree structure**: Arena-based allocation for cache-friendly traversal
//! 3. **Query support**: Element lookup by ID, class, tag name
//! 4. **Mutation support**: Node insertion, removal, attribute modification
//! 5. **Event dispatch**: DOM Events with capture/bubble phases

pub mod events;
pub mod forms;
pub mod images;

pub use events::{
    AddEventListenerOptions, DomEvent, Event, EventDispatcher, EventId, EventListenerCallback,
    EventPhase, EventTarget, FocusEventData, InputEventData, KeyboardEventData, MouseEventData,
};
pub use forms::{
    CheckableState, FormDataEntry, FormDataValue, FormEnctype, FormMethod, FormState, InputType,
    SelectionDirection, SelectionRange, TextEditState,
};
pub use images::{
    CrossOrigin, FaviconLink, ImageDecoding, ImageElement, ImageElementManager, ImageLoading,
    ImageLoadingState, PictureElement, PictureSource,
};

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use thiserror::Error;
use tracing::debug;

/// Errors that can occur in DOM operations.
#[derive(Error, Debug)]
pub enum DomError {
    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Node not found")]
    NodeNotFound,

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

/// Unique identifier for a DOM node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(usize);

impl NodeId {
    /// Create a new NodeId.
    pub fn new(id: usize) -> Self {
        Self(id)
    }

    /// Get the raw ID value.
    pub fn raw(&self) -> usize {
        self.0
    }
}

/// Type of DOM node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeType {
    Document,
    DocumentType {
        name: String,
        public_id: String,
        system_id: String,
    },
    Element {
        tag_name: String,
        namespace: String,
        attributes: HashMap<String, String>,
    },
    Text(String),
    Comment(String),
    ProcessingInstruction {
        target: String,
        data: String,
    },
}

/// A DOM node.
#[derive(Debug)]
pub struct Node {
    /// Unique ID for this node.
    pub id: NodeId,
    /// Node type and associated data.
    pub node_type: NodeType,
    /// Parent node (weak reference to avoid cycles).
    parent: RefCell<Option<Weak<Node>>>,
    /// Child nodes.
    children: RefCell<Vec<Rc<Node>>>,
    /// Previous sibling.
    prev_sibling: RefCell<Option<Weak<Node>>>,
    /// Next sibling.
    next_sibling: RefCell<Option<Weak<Node>>>,
    /// Event target mixin for event handling.
    pub event_target: EventTarget,
}

impl Node {
    /// Create a new node.
    pub fn new(id: NodeId, node_type: NodeType) -> Rc<Self> {
        Rc::new(Self {
            id,
            node_type,
            parent: RefCell::new(None),
            children: RefCell::new(Vec::new()),
            prev_sibling: RefCell::new(None),
            next_sibling: RefCell::new(None),
            event_target: EventTarget::new(),
        })
    }

    /// Get the tag name for element nodes.
    pub fn tag_name(&self) -> Option<&str> {
        match &self.node_type {
            NodeType::Element { tag_name, .. } => Some(tag_name),
            _ => None,
        }
    }

    /// Get an attribute value.
    pub fn get_attribute(&self, name: &str) -> Option<&str> {
        match &self.node_type {
            NodeType::Element { attributes, .. } => attributes.get(name).map(|s| s.as_str()),
            _ => None,
        }
    }

    /// Get the text content.
    pub fn text_content(&self) -> String {
        let mut result = String::new();
        self.collect_text(&mut result);
        result
    }

    fn collect_text(&self, result: &mut String) {
        match &self.node_type {
            NodeType::Text(text) => result.push_str(text),
            _ => {
                for child in self.children.borrow().iter() {
                    child.collect_text(result);
                }
            }
        }
    }

    /// Get parent node.
    pub fn parent(&self) -> Option<Rc<Node>> {
        self.parent.borrow().as_ref().and_then(|w| w.upgrade())
    }

    /// Get child nodes.
    pub fn children(&self) -> Vec<Rc<Node>> {
        self.children.borrow().clone()
    }

    /// Get first child.
    pub fn first_child(&self) -> Option<Rc<Node>> {
        self.children.borrow().first().cloned()
    }

    /// Get last child.
    pub fn last_child(&self) -> Option<Rc<Node>> {
        self.children.borrow().last().cloned()
    }

    /// Get previous sibling.
    pub fn previous_sibling(&self) -> Option<Rc<Node>> {
        self.prev_sibling
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
    }

    /// Get next sibling.
    pub fn next_sibling(&self) -> Option<Rc<Node>> {
        self.next_sibling
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
    }

    /// Check if this is an element node.
    pub fn is_element(&self) -> bool {
        matches!(self.node_type, NodeType::Element { .. })
    }

    /// Check if this is a text node.
    pub fn is_text(&self) -> bool {
        matches!(self.node_type, NodeType::Text(_))
    }

    /// Append a child node.
    pub fn append_child(self: &Rc<Self>, child: Rc<Node>) {
        // Update child's parent
        *child.parent.borrow_mut() = Some(Rc::downgrade(self));

        // Update sibling links
        if let Some(last) = self.last_child() {
            *last.next_sibling.borrow_mut() = Some(Rc::downgrade(&child));
            *child.prev_sibling.borrow_mut() = Some(Rc::downgrade(&last));
        }

        // Add to children
        self.children.borrow_mut().push(child);
    }

    /// Remove this node from its parent.
    pub fn remove_from_parent(self: &Rc<Self>) {
        if let Some(parent) = self.parent() {
            // Update sibling links
            if let Some(prev) = self.previous_sibling() {
                *prev.next_sibling.borrow_mut() = self.next_sibling.borrow().clone();
            }
            if let Some(next) = self.next_sibling() {
                *next.prev_sibling.borrow_mut() = self.prev_sibling.borrow().clone();
            }

            // Remove from parent's children
            parent.children.borrow_mut().retain(|c| !Rc::ptr_eq(c, self));

            // Clear our parent reference
            *self.parent.borrow_mut() = None;
            *self.prev_sibling.borrow_mut() = None;
            *self.next_sibling.borrow_mut() = None;
        }
    }

    /// Insert a child node before a reference node.
    pub fn insert_before(self: &Rc<Self>, new_child: Rc<Node>, reference: Rc<Node>) {
        // Find the index of the reference node
        let mut children = self.children.borrow_mut();
        let ref_idx = children.iter().position(|c| Rc::ptr_eq(c, &reference));

        if let Some(idx) = ref_idx {
            // Update new_child's parent
            *new_child.parent.borrow_mut() = Some(Rc::downgrade(self));

            // Update sibling links
            *new_child.next_sibling.borrow_mut() = Some(Rc::downgrade(&reference));
            if idx > 0 {
                let prev = &children[idx - 1];
                *new_child.prev_sibling.borrow_mut() = Some(Rc::downgrade(prev));
                *prev.next_sibling.borrow_mut() = Some(Rc::downgrade(&new_child));
            }
            *reference.prev_sibling.borrow_mut() = Some(Rc::downgrade(&new_child));

            // Insert into children
            children.insert(idx, new_child);
        } else {
            // Reference not found, append at end
            drop(children);
            self.append_child(new_child);
        }
    }
}

/// A complete DOM document.
pub struct Document {
    /// Root node of the document.
    root: Rc<Node>,
    /// All nodes indexed by ID.
    nodes: HashMap<NodeId, Rc<Node>>,
    /// Elements indexed by ID attribute.
    elements_by_id: HashMap<String, Rc<Node>>,
    /// Next node ID.
    next_id: Cell<usize>,
}

/// Sink for building a Document from HTML parsing.
struct DocumentSink {
    doc: Document,
    /// Stack of open elements during parsing.
    open_elements: Vec<Rc<Node>>,
}

impl DocumentSink {
    fn new() -> Self {
        Self {
            doc: Document::new(),
            open_elements: vec![],
        }
    }

    fn current_parent(&self) -> Rc<Node> {
        self.open_elements
            .last()
            .cloned()
            .unwrap_or_else(|| self.doc.root.clone())
    }

    fn create_node(&mut self, node_type: NodeType) -> Rc<Node> {
        let id = NodeId::new(self.doc.next_id.get());
        self.doc.next_id.set(self.doc.next_id.get() + 1);

        let node = Node::new(id, node_type);
        self.doc.nodes.insert(id, node.clone());
        node
    }
}

impl rustkit_html::TreeSink for DocumentSink {
    type NodeId = Rc<Node>;

    fn doctype(&mut self, name: String, public_id: String, system_id: String) {
        let node = self.create_node(NodeType::DocumentType {
            name,
            public_id,
            system_id,
        });
        self.doc.root.append_child(node);
    }

    fn start_element(
        &mut self,
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
    ) -> Self::NodeId {
        let mut attributes = HashMap::new();
        for (key, value) in attrs {
            attributes.insert(key, value);
        }

        let node = self.create_node(NodeType::Element {
            tag_name: name,
            namespace: String::from("http://www.w3.org/1999/xhtml"),
            attributes,
        });

        // Index by ID attribute
        if let Some(id) = node.get_attribute("id") {
            self.doc.elements_by_id.insert(id.to_string(), node.clone());
        }

        let parent = self.current_parent();
        parent.append_child(node.clone());

        // Push onto stack for nested elements (but not void/self-closing elements)
        // CRITICAL: Void elements like <meta>, <br>, <img> must not stay on the stack
        // or they will corrupt the DOM tree by becoming incorrect parents
        if !self_closing {
            self.open_elements.push(node.clone());
        }

        node
    }

    fn end_element(&mut self, _name: String) {
        self.open_elements.pop();
    }

    fn text(&mut self, data: String) {
        if !data.is_empty() {
            let node = self.create_node(NodeType::Text(data));
            let parent = self.current_parent();
            parent.append_child(node);
        }
    }

    fn comment(&mut self, data: String) {
        let node = self.create_node(NodeType::Comment(data));
        let parent = self.current_parent();
        parent.append_child(node);
    }

    fn current_node(&self) -> Option<Self::NodeId> {
        self.open_elements.last().cloned()
    }

    fn in_scope(&self, tag_name: &str) -> bool {
        for node in self.open_elements.iter().rev() {
            if node.tag_name() == Some(tag_name) {
                return true;
            }
        }
        false
    }

    fn pop_until(&mut self, tag_name: &str) {
        while let Some(node) = self.open_elements.last() {
            let should_stop = node.tag_name() == Some(tag_name);
            self.open_elements.pop();
            if should_stop {
                break;
            }
        }
    }

    fn close_p_element_in_button_scope(&mut self) {
        // Simplified: just pop until we find a p element
        while let Some(node) = self.open_elements.last() {
            let is_p = node.tag_name() == Some("p");
            self.open_elements.pop();
            if is_p {
                break;
            }
        }
    }

    fn reconstruct_active_formatting_elements(&mut self) {
        // Simplified: not implemented for this initial version
    }

    // ==================== AAA (Adoption Agency Algorithm) Methods ====================

    fn create_element(&mut self, name: String, attrs: Vec<(String, String)>) -> Self::NodeId {
        let mut attributes = HashMap::new();
        for (key, value) in attrs {
            attributes.insert(key, value);
        }

        self.create_node(NodeType::Element {
            tag_name: name,
            namespace: String::from("http://www.w3.org/1999/xhtml"),
            attributes,
        })
    }

    fn append_child(&mut self, parent: Self::NodeId, child: Self::NodeId) {
        parent.append_child(child);
    }

    fn remove_from_parent(&mut self, node: Self::NodeId) {
        node.remove_from_parent();
    }

    fn reparent_children(&mut self, from: Self::NodeId, to: Self::NodeId) {
        // Move all children from 'from' to 'to'
        let children = from.children();
        for child in children {
            child.remove_from_parent();
            to.append_child(child);
        }
    }

    fn insert_before(&mut self, parent: Self::NodeId, node: Self::NodeId, reference: Option<Self::NodeId>) {
        if let Some(ref_node) = reference {
            parent.insert_before(node, ref_node);
        } else {
            parent.append_child(node);
        }
    }

    fn get_parent(&self, node: Self::NodeId) -> Option<Self::NodeId> {
        node.parent()
    }

    fn get_tag_name(&self, node: Self::NodeId) -> Option<String> {
        node.tag_name().map(|s| s.to_string())
    }
}

impl Document {
    /// Create a new empty document.
    pub fn new() -> Self {
        let root = Node::new(NodeId::new(0), NodeType::Document);
        let mut nodes = HashMap::new();
        nodes.insert(NodeId::new(0), root.clone());

        Self {
            root,
            nodes,
            elements_by_id: HashMap::new(),
            next_id: Cell::new(1),
        }
    }

    /// Parse HTML and create a document (new rustkit-html parser).
    pub fn parse_html(html: &str) -> Result<Self, DomError> {
        debug!(len = html.len(), "Parsing HTML (rustkit-html)");

        let sink = DocumentSink::new();
        let sink = rustkit_html::parse(html, sink).map_err(|e| DomError::ParseError(e.to_string()))?;

        debug!(node_count = sink.doc.nodes.len(), "HTML parsed");
        Ok(sink.doc)
    }


    /// Get the document root.
    pub fn root(&self) -> &Rc<Node> {
        &self.root
    }

    /// Get the document element (<html>).
    pub fn document_element(&self) -> Option<Rc<Node>> {
        self.root
            .children()
            .into_iter()
            .find(|n| n.tag_name() == Some("html"))
    }

    /// Get the <head> element.
    pub fn head(&self) -> Option<Rc<Node>> {
        self.document_element()?
            .children()
            .into_iter()
            .find(|n| n.tag_name() == Some("head"))
    }

    /// Get the <body> element.
    pub fn body(&self) -> Option<Rc<Node>> {
        // First try to find body as a direct child of the document element (html)
        // This is the correct DOM structure per spec
        if let Some(html_element) = self.document_element() {
            if let Some(body) = html_element
                .children()
                .into_iter()
                .find(|n| n.tag_name() == Some("body"))
            {
                return Some(body);
            }
        }

        // Fallback: search the entire document for a body element
        // This handles cases where the parser might incorrectly place the body
        // (e.g., as a sibling of html or nested incorrectly)
        self.get_elements_by_tag_name("body").into_iter().next()
    }

    /// Get element by ID.
    pub fn get_element_by_id(&self, id: &str) -> Option<Rc<Node>> {
        self.elements_by_id.get(id).cloned()
    }

    /// Get elements by tag name, in document order.
    ///
    /// Iterating `self.nodes` (a HashMap) here returned a RANDOM order per
    /// process, and callers that cascade stylesheets (`<style>`/`<link>`
    /// extraction) silently depend on document order — CSS rule order breaks
    /// ties, so random sheet order made identical renders differ run-to-run.
    pub fn get_elements_by_tag_name(&self, tag_name: &str) -> Vec<Rc<Node>> {
        let tag_name_lower = tag_name.to_lowercase();
        let mut result = Vec::new();
        self.traverse(|n| {
            if n.tag_name()
                .map(|t| t.to_lowercase() == tag_name_lower)
                .unwrap_or(false)
            {
                result.push(n.clone());
            }
        });
        result
    }

    /// Get elements by class name, in document order (see
    /// `get_elements_by_tag_name` for why traversal, not `nodes`, is used).
    pub fn get_elements_by_class_name(&self, class_name: &str) -> Vec<Rc<Node>> {
        let mut result = Vec::new();
        self.traverse(|n| {
            if n.get_attribute("class")
                .map(|c| c.split_whitespace().any(|cls| cls == class_name))
                .unwrap_or(false)
            {
                result.push(n.clone());
            }
        });
        result
    }

    /// Get node by ID.
    pub fn get_node(&self, id: NodeId) -> Option<Rc<Node>> {
        self.nodes.get(&id).cloned()
    }

    /// Get the title of the document.
    pub fn title(&self) -> Option<String> {
        let head = self.head()?;
        let title_elem = head
            .children()
            .into_iter()
            .find(|n| n.tag_name() == Some("title"))?;
        Some(title_elem.text_content())
    }

    /// Traverse all nodes depth-first.
    pub fn traverse<F>(&self, mut callback: F)
    where
        F: FnMut(&Rc<Node>),
    {
        self.traverse_node(&self.root, &mut callback);
    }

    #[allow(clippy::only_used_in_recursion)]
    fn traverse_node<F>(&self, node: &Rc<Node>, callback: &mut F)
    where
        F: FnMut(&Rc<Node>),
    {
        callback(node);
        for child in node.children() {
            self.traverse_node(&child, callback);
        }
    }
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

/// Query selector support (basic).
pub struct QuerySelector;

impl QuerySelector {
    /// Select elements matching a simple selector.
    /// Supports: tag, #id, .class
    pub fn select(doc: &Document, selector: &str) -> Vec<Rc<Node>> {
        let selector = selector.trim();

        if let Some(id) = selector.strip_prefix('#') {
            // ID selector
            doc.get_element_by_id(id).into_iter().collect()
        } else if let Some(class) = selector.strip_prefix('.') {
            // Class selector
            doc.get_elements_by_class_name(class)
        } else {
            // Tag selector
            doc.get_elements_by_tag_name(selector)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_html() {
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body><p id="main">Hello, world!</p></body>
</html>"#;

        let doc = Document::parse_html(html).unwrap();

        // Check structure
        assert!(doc.document_element().is_some());
        assert!(doc.head().is_some());
        assert!(doc.body().is_some());

        // Check title
        assert_eq!(doc.title(), Some("Test".to_string()));

        // Check ID lookup
        let main = doc.get_element_by_id("main").unwrap();
        assert_eq!(main.tag_name(), Some("p"));
        assert_eq!(main.text_content(), "Hello, world!");
    }

    #[test]
    fn test_get_elements_by_tag_name_document_order() {
        // Regression: HashMap-backed lookup returned styles in random order
        // per process, which randomized the CSS cascade (rule order breaks
        // specificity ties). Order must be document order, deterministically.
        let html = r#"<html>
<head>
    <style>/*first*/</style>
    <style>/*second*/</style>
</head>
<body>
    <style>/*third*/</style>
    <p class="x">a</p>
    <p class="x">b</p>
</body>
</html>"#;

        for _ in 0..8 {
            let doc = Document::parse_html(html).unwrap();
            let styles = doc.get_elements_by_tag_name("style");
            let contents: Vec<String> = styles
                .iter()
                .map(|s| s.text_content().trim().to_string())
                .collect();
            assert_eq!(contents, vec!["/*first*/", "/*second*/", "/*third*/"]);

            let ps = doc.get_elements_by_class_name("x");
            let texts: Vec<String> = ps.iter().map(|p| p.text_content()).collect();
            assert_eq!(texts, vec!["a", "b"]);
        }
    }

    #[test]
    fn test_query_selector() {
        let html = r#"<html>
<body>
    <div class="container">
        <p id="first">First</p>
        <p class="highlight">Second</p>
        <span class="highlight">Third</span>
    </div>
</body>
</html>"#;

        let doc = Document::parse_html(html).unwrap();

        // ID selector
        let by_id = QuerySelector::select(&doc, "#first");
        assert_eq!(by_id.len(), 1);
        assert_eq!(by_id[0].text_content(), "First");

        // Class selector
        let by_class = QuerySelector::select(&doc, ".highlight");
        assert_eq!(by_class.len(), 2);

        // Tag selector
        let by_tag = QuerySelector::select(&doc, "p");
        assert_eq!(by_tag.len(), 2);
    }

    #[test]
    fn test_traversal() {
        let html = "<html><head></head><body><div><p>Text</p></div></body></html>";
        let doc = Document::parse_html(html).unwrap();

        let mut count = 0;
        doc.traverse(|_| count += 1);
        assert!(count > 0);
    }

    #[test]
    fn test_node_relationships() {
        let html = "<html><body><p>A</p><p>B</p><p>C</p></body></html>";
        let doc = Document::parse_html(html).unwrap();

        let body = doc.body().unwrap();
        let paragraphs: Vec<_> = body
            .children()
            .into_iter()
            .filter(|n| n.is_element())
            .collect();

        assert_eq!(paragraphs.len(), 3);

        // Check sibling relationships
        let first = &paragraphs[0];
        let second = &paragraphs[1];
        let _third = &paragraphs[2];

        assert!(
            first.previous_sibling().is_none() || !first.previous_sibling().unwrap().is_element()
        );
        assert_eq!(
            second
                .previous_sibling()
                .map(|n| n.text_content().trim().to_string()),
            Some("A".to_string())
        );
    }

    #[test]
    fn test_void_elements_not_on_stack() {
        // CRITICAL: Void elements like <meta>, <br>, <img> must not stay on the stack
        // or they will corrupt the DOM tree
        let html = r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Test</title>
</head>
<body>
<p id="test">Hello</p>
</body>
</html>"#;

        let doc = Document::parse_html(html).unwrap();

        assert!(doc.document_element().is_some(), "should have html");
        assert!(doc.head().is_some(), "should have head");
        assert!(doc.body().is_some(), "should have body - void elements must not corrupt DOM");

        let body = doc.body().unwrap();
        let p = body
            .children()
            .into_iter()
            .find(|n| n.tag_name() == Some("p"));
        assert!(p.is_some(), "should have p in body");
    }

    #[test]
    fn test_large_style_block() {
        // Large CSS block like chrome.html has
        let css = ".a{color:red;} ".repeat(1000);
        let html = format!(
            r#"<!DOCTYPE html>
<html>
<head>
<style>{}</style>
</head>
<body>
<p id="test">Hello</p>
</body>
</html>"#,
            css
        );

        let doc = Document::parse_html(&html).unwrap();

        assert!(doc.document_element().is_some(), "should have html");
        assert!(doc.head().is_some(), "should have head");
        assert!(doc.body().is_some(), "should have body");

        let body = doc.body().unwrap();
        let p = body
            .children()
            .into_iter()
            .find(|n| n.tag_name() == Some("p"));
        assert!(p.is_some(), "should have p in body");
    }

    #[test]
    fn test_meta_and_title() {
        let html = r#"<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Test</title>
</head>
<body>
<p>Hello</p>
</body>
</html>"#;

        let doc = Document::parse_html(html).unwrap();

        assert!(doc.body().is_some(), "should have body with meta and title");
    }
}
