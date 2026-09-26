//! HTML tree builder.
//!
//! Implements a simplified HTML5 tree construction algorithm with
//! insertion modes and error recovery.

use crate::tokenizer::Token;
use crate::{ParseResult, TreeSink};
use tracing::trace;

/// Quirks mode for the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuirksMode {
    /// No quirks mode (standards mode)
    #[default]
    NoQuirks,
    /// Limited quirks mode (almost standards mode)
    LimitedQuirks,
    /// Full quirks mode
    Quirks,
}

/// Context for fragment parsing (innerHTML, insertAdjacentHTML, etc.)
#[derive(Debug, Clone)]
pub struct FragmentContext {
    /// The context element's tag name (e.g., "div", "body", "template")
    pub context_element: String,
    /// Whether the context element is in the HTML namespace
    pub html_namespace: bool,
}

impl FragmentContext {
    /// Create a new fragment context for a given element.
    pub fn new(context_element: &str) -> Self {
        Self {
            context_element: context_element.to_lowercase(),
            html_namespace: true,
        }
    }
}

/// Insertion mode for tree construction.
///
/// Many modes are currently unused but required for full HTML5 spec compliance
/// (HTML5 § 8.2.5 Tree Construction). The simplified parser uses a subset but
/// keeps all modes defined for future expansion to full spec-compliant parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum InsertionMode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    // Table modes
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    // Select modes
    InSelect,
    InSelectInTable,
    // Template mode
    InTemplate,
    // After modes
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// Void elements that cannot have children.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
    "source", "track", "wbr",
];

/// Elements that cause implicit closure of p elements.
const P_CLOSING_ELEMENTS: &[&str] = &[
    "address", "article", "aside", "blockquote", "div", "dl", "fieldset", "figcaption",
    "figure", "footer", "form", "h1", "h2", "h3", "h4", "h5", "h6", "header", "hgroup", "hr",
    "main", "nav", "ol", "p", "pre", "section", "table", "ul",
];

/// Table section elements (tbody, thead, tfoot).
const TABLE_SECTION_ELEMENTS: &[&str] = &["tbody", "thead", "tfoot"];

/// Table cell elements.
const TABLE_CELL_ELEMENTS: &[&str] = &["td", "th"];

/// Elements that are table scope limiters.
const TABLE_SCOPE_ELEMENTS: &[&str] = &["html", "table", "template"];

/// Elements that can be in table context but trigger foster parenting.
/// Required for spec-compliant table parsing (HTML5 § 8.2.5.3).
/// Currently unused but needed for full foster parenting algorithm.
#[allow(dead_code)]
const TABLE_CONTEXT_ELEMENTS: &[&str] = &[
    "table", "tbody", "tfoot", "thead", "tr",
];

/// Formatting elements that participate in the Adoption Agency Algorithm.
const FORMATTING_ELEMENTS: &[&str] = &[
    "a", "b", "big", "code", "em", "font", "i", "nobr",
    "s", "small", "strike", "strong", "tt", "u",
];

/// Special elements that break formatting recovery (block-like elements).
const SPECIAL_ELEMENTS: &[&str] = &[
    "address", "applet", "area", "article", "aside", "base", "basefont", "bgsound",
    "blockquote", "body", "br", "button", "caption", "center", "col", "colgroup",
    "dd", "details", "dir", "div", "dl", "dt", "embed", "fieldset", "figcaption",
    "figure", "footer", "form", "frame", "frameset", "h1", "h2", "h3", "h4", "h5",
    "h6", "head", "header", "hgroup", "hr", "html", "iframe", "img", "input",
    "keygen", "li", "link", "listing", "main", "marquee", "menu", "meta", "nav",
    "noembed", "noframes", "noscript", "object", "ol", "p", "param", "plaintext",
    "pre", "script", "section", "select", "source", "style", "summary", "table",
    "tbody", "td", "template", "textarea", "tfoot", "th", "thead", "title", "tr",
    "track", "ul", "wbr", "xmp",
];

/// Marker in active formatting elements (used to mark scope boundaries).
#[derive(Debug, Clone)]
enum FormattingEntry<NodeId: Clone> {
    /// A formatting element
    Element {
        name: String,
        attrs: Vec<(String, String)>,
        #[allow(dead_code)]
        node_id: NodeId,
    },
    /// Scope marker (e.g., pushed when entering table, button, etc.)
    Marker,
}

/// HTML tree builder.
pub struct TreeBuilder<S: TreeSink> {
    sink: S,
    mode: InsertionMode,
    /// Original insertion mode (used when switching to InTableText or Text mode)
    original_mode: Option<InsertionMode>,
    open_elements: Vec<(String, S::NodeId)>,
    /// Active formatting elements list for AAA
    active_formatting_elements: Vec<FormattingEntry<S::NodeId>>,
    /// Template insertion modes stack
    template_insertion_modes: Vec<InsertionMode>,
    /// Foster parenting mode for handling misplaced table content.
    /// Required for spec-compliant table parsing (HTML5 § 8.2.5.3).
    foster_parenting: bool,
    /// Buffer for accumulating consecutive text characters
    text_buffer: String,
    /// HTML §13.2.6.4.7, "pre"/"listing" start tags: "If the next token is
    /// a U+000A LINE FEED character token, then ignore that token". Set by
    /// the start tag, consumed by whatever token comes next.
    ignore_lf: bool,
    /// Pending table character tokens (for InTableText mode)
    pending_table_chars: Vec<char>,
    /// Document quirks mode
    quirks_mode: QuirksMode,
    /// Fragment parsing context (None for full document parsing)
    fragment_context: Option<FragmentContext>,
    /// Head element pointer (for implicit head handling)
    head_element: Option<S::NodeId>,
    /// Form element pointer (for form owner tracking).
    /// Required for spec-compliant form association (HTML5 § 4.10.18.3).
    /// Currently unused but needed when form control elements track their owner form.
    #[allow(dead_code)]
    form_element: Option<S::NodeId>,
}

impl<S: TreeSink> TreeBuilder<S> {
    /// Create a new tree builder for full document parsing.
    pub fn new(sink: S) -> Self {
        Self {
            sink,
            mode: InsertionMode::Initial,
            original_mode: None,
            open_elements: Vec::new(),
            active_formatting_elements: Vec::new(),
            template_insertion_modes: Vec::new(),
            foster_parenting: false,
            text_buffer: String::new(),
            ignore_lf: false,
            pending_table_chars: Vec::new(),
            quirks_mode: QuirksMode::NoQuirks,
            fragment_context: None,
            head_element: None,
            form_element: None,
        }
    }

    /// Create a new tree builder for fragment parsing (innerHTML, insertAdjacentHTML).
    ///
    /// The context_element determines the initial parsing state:
    /// - "template": starts in InTemplate with template contents
    /// - "title", "textarea": RCDATA parsing
    /// - "style", "xmp", "iframe", "noembed", "noframes": RAWTEXT parsing
    /// - "script": script data parsing
    /// - "plaintext": PLAINTEXT parsing
    /// - "select": InSelect mode
    /// - "table": InTable mode
    /// - others: normal InBody parsing
    pub fn new_fragment(sink: S, context: FragmentContext) -> Self {
        // Determine initial insertion mode based on context element
        let (mode, template_modes) = match context.context_element.as_str() {
            "title" | "textarea" => (InsertionMode::InBody, vec![]), // RCDATA handled by tokenizer
            "style" | "xmp" | "iframe" | "noembed" | "noframes" => (InsertionMode::InBody, vec![]),
            "script" => (InsertionMode::InBody, vec![]), // Script data handled by tokenizer
            "plaintext" => (InsertionMode::InBody, vec![]),
            "html" => (InsertionMode::BeforeHead, vec![]),
            "head" => (InsertionMode::InHead, vec![]),
            "template" => (InsertionMode::InTemplate, vec![InsertionMode::InTemplate]),
            "select" => (InsertionMode::InSelect, vec![]),
            "table" => (InsertionMode::InTable, vec![]),
            "tbody" | "thead" | "tfoot" => (InsertionMode::InTableBody, vec![]),
            "tr" => (InsertionMode::InRow, vec![]),
            "td" | "th" => (InsertionMode::InCell, vec![]),
            "frameset" => (InsertionMode::InFrameset, vec![]),
            "body" | "div" | "span" | "p" | _ => (InsertionMode::InBody, vec![]),
        };

        Self {
            sink,
            mode,
            original_mode: None,
            open_elements: Vec::new(),
            active_formatting_elements: Vec::new(),
            template_insertion_modes: template_modes,
            foster_parenting: false,
            text_buffer: String::new(),
            ignore_lf: false,
            pending_table_chars: Vec::new(),
            quirks_mode: QuirksMode::NoQuirks,
            fragment_context: Some(context),
            head_element: None,
            form_element: None,
        }
    }

    /// Get the current quirks mode.
    pub fn quirks_mode(&self) -> QuirksMode {
        self.quirks_mode
    }

    /// Check if we're in fragment parsing mode.
    pub fn is_fragment_parsing(&self) -> bool {
        self.fragment_context.is_some()
    }
    
    fn flush_text(&mut self) {
        if !self.text_buffer.is_empty() {
            let text = std::mem::take(&mut self.text_buffer);
            self.sink.text(text);
        }
    }

    fn current_node(&self) -> Option<&(String, S::NodeId)> {
        self.open_elements.last()
    }

    fn current_node_name(&self) -> Option<&str> {
        self.current_node().map(|(name, _)| name.as_str())
    }

    fn has_element_in_scope(&self, tag_name: &str) -> bool {
        for (name, _) in self.open_elements.iter().rev() {
            if name == tag_name {
                return true;
            }
            // Scope-limiting elements
            if matches!(
                name.as_str(),
                "applet" | "caption" | "html" | "table" | "td" | "th" | "marquee" | "object" | "template"
            ) {
                return false;
            }
        }
        false
    }

    fn pop_until(&mut self, tag_name: &str) {
        while let Some((name, _)) = self.open_elements.pop() {
            if name == tag_name {
                break;
            }
        }
    }

    fn close_p_element(&mut self) {
        if self.has_element_in_scope("p") {
            self.sink.close_p_element_in_button_scope();
            self.pop_until("p");
        }
    }

    /// Check if an element is in table scope.
    fn has_element_in_table_scope(&self, tag_name: &str) -> bool {
        for (name, _) in self.open_elements.iter().rev() {
            if name == tag_name {
                return true;
            }
            if TABLE_SCOPE_ELEMENTS.contains(&name.as_str()) {
                return false;
            }
        }
        false
    }

    /// Clear the stack back to a table context.
    fn clear_stack_to_table_context(&mut self) {
        while let Some((name, _)) = self.current_node() {
            if name == "table" || name == "template" || name == "html" {
                break;
            }
            let (tag_name, _) = self.open_elements.pop().unwrap();
            self.sink.end_element(tag_name);
        }
    }

    /// Clear the stack back to a table body context.
    fn clear_stack_to_table_body_context(&mut self) {
        while let Some((name, _)) = self.current_node() {
            if TABLE_SECTION_ELEMENTS.contains(&name.as_str())
                || name == "template"
                || name == "html"
            {
                break;
            }
            let (tag_name, _) = self.open_elements.pop().unwrap();
            self.sink.end_element(tag_name);
        }
    }

    /// Clear the stack back to a table row context.
    fn clear_stack_to_table_row_context(&mut self) {
        while let Some((name, _)) = self.current_node() {
            if name == "tr" || name == "template" || name == "html" {
                break;
            }
            let (tag_name, _) = self.open_elements.pop().unwrap();
            self.sink.end_element(tag_name);
        }
    }

    /// Find the index of the last table element in the stack.
    #[allow(dead_code)]
    fn find_last_table_index(&self) -> Option<usize> {
        for i in (0..self.open_elements.len()).rev() {
            if self.open_elements[i].0 == "table" {
                return Some(i);
            }
        }
        None
    }

    /// Get the foster parent location (element before table, or table's parent).
    #[allow(dead_code)]
    fn get_foster_parent_index(&self) -> usize {
        if let Some(table_idx) = self.find_last_table_index() {
            if table_idx > 0 {
                table_idx - 1
            } else {
                0
            }
        } else {
            // No table, use current node
            self.open_elements.len().saturating_sub(1)
        }
    }

    /// Insert element, respecting foster parenting if active.
    #[allow(dead_code)]
    fn insert_element(&mut self, name: String, attrs: Vec<(String, String)>, self_closing: bool) -> S::NodeId {
        if self.foster_parenting {
            if let Some(table_idx) = self.find_last_table_index() {
                let (_table_name, table_id) = self.open_elements[table_idx].clone();
                // Create the element without appending it to current node
                let node_id = self.sink.create_element(name.clone(), attrs);
                // Foster parent it to just before the table
                self.sink.foster_parent(table_id, node_id.clone());
                // Still add to open elements if not self-closing
                if !self_closing && !VOID_ELEMENTS.contains(&name.as_str()) {
                    self.open_elements.push((name, node_id.clone()));
                }
                return node_id;
            }
        }

        // Normal insertion
        let node_id = self.sink.start_element(name.clone(), attrs, self_closing);
        if !self_closing && !VOID_ELEMENTS.contains(&name.as_str()) {
            self.open_elements.push((name, node_id.clone()));
        }
        node_id
    }

    /// Insert text, respecting foster parenting if active.
    #[allow(dead_code)]
    fn insert_text(&mut self, text: String) {
        if self.foster_parenting {
            // For foster parented text, we call the sink's text method
            // which should handle it appropriately
            // Note: Real foster parenting for text would need more complex handling
            self.sink.text(text);
        } else {
            self.sink.text(text);
        }
    }

    /// Close cell and switch to InRow mode.
    fn close_cell(&mut self) {
        self.flush_text();
        // Pop until we find td or th
        while let Some((name, _)) = self.open_elements.pop() {
            self.sink.end_element(name.clone());
            if TABLE_CELL_ELEMENTS.contains(&name.as_str()) {
                break;
            }
        }
        self.mode = InsertionMode::InRow;
    }

    // ==================== ADOPTION AGENCY ALGORITHM ====================

    /// Push a formatting element onto the active formatting elements list.
    fn push_formatting_element(&mut self, name: String, attrs: Vec<(String, String)>, node_id: S::NodeId) {
        // Noah's Ark clause: If there are already 3 elements with the same tag name,
        // attributes, and values, remove the oldest one.
        let mut matching_count = 0;
        let mut oldest_matching_index = None;

        for (i, entry) in self.active_formatting_elements.iter().enumerate().rev() {
            match entry {
                FormattingEntry::Marker => break,
                FormattingEntry::Element { name: entry_name, attrs: entry_attrs, .. } => {
                    if entry_name == &name && entry_attrs == &attrs {
                        matching_count += 1;
                        if matching_count >= 3 {
                            oldest_matching_index = Some(i);
                        }
                    }
                }
            }
        }

        if let Some(idx) = oldest_matching_index {
            self.active_formatting_elements.remove(idx);
        }

        self.active_formatting_elements.push(FormattingEntry::Element {
            name,
            attrs,
            node_id,
        });
    }

    /// Push a scope marker onto the active formatting elements list.
    fn push_formatting_marker(&mut self) {
        self.active_formatting_elements.push(FormattingEntry::Marker);
    }

    /// Clear active formatting elements up to (and including) the last marker.
    #[allow(dead_code)]
    fn clear_active_formatting_to_marker(&mut self) {
        while let Some(entry) = self.active_formatting_elements.pop() {
            if matches!(entry, FormattingEntry::Marker) {
                break;
            }
        }
    }

    /// Find a formatting element in the active formatting list by name.
    #[allow(dead_code)]
    fn find_formatting_element(&self, tag_name: &str) -> Option<(usize, S::NodeId)> {
        for (i, entry) in self.active_formatting_elements.iter().enumerate().rev() {
            match entry {
                FormattingEntry::Marker => return None,
                FormattingEntry::Element { name, node_id, .. } => {
                    if name == tag_name {
                        return Some((i, node_id.clone()));
                    }
                }
            }
        }
        None
    }

    /// Find the position of a node in the open elements stack.
    #[allow(dead_code)]
    fn find_in_open_elements(&self, node_id: &S::NodeId) -> Option<usize>
    where
        S::NodeId: PartialEq
    {
        self.open_elements.iter().position(|(_, id)| id == node_id)
    }

    /// Check if an element is in the active formatting elements list.
    #[allow(dead_code)]
    fn is_in_active_formatting(&self, node_id: &S::NodeId) -> bool
    where
        S::NodeId: PartialEq
    {
        self.active_formatting_elements.iter().any(|entry| {
            matches!(entry, FormattingEntry::Element { node_id: id, .. } if id == node_id)
        })
    }

    /// Reconstruct active formatting elements.
    /// Called before inserting character tokens or certain elements.
    fn reconstruct_active_formatting(&mut self) {
        // If the list is empty, do nothing
        if self.active_formatting_elements.is_empty() {
            return;
        }

        // Check if there's anything to reconstruct
        let last_idx = self.active_formatting_elements.len() - 1;

        // If the last entry is a marker, nothing to do
        if matches!(&self.active_formatting_elements[last_idx], FormattingEntry::Marker) {
            return;
        }

        // Check if the last formatting element is already in the open elements
        // We use name-based tracking: if formatting element name is in open_elements, it's "active"
        let last_name = match &self.active_formatting_elements[last_idx] {
            FormattingEntry::Element { name, .. } => name.clone(),
            FormattingEntry::Marker => return,
        };

        // Check if this element is already on the stack as the current node or near top
        // Simple heuristic: if the element name is anywhere in open_elements after the body, skip
        let body_idx = self.open_elements.iter().position(|(n, _)| n == "body").unwrap_or(0);
        for i in (body_idx + 1)..self.open_elements.len() {
            if self.open_elements[i].0 == last_name {
                // Already have this formatting element open, don't reconstruct
                return;
            }
        }

        // Find the first element that needs to be reconstructed
        // Walk back from the end to find entries not in the stack
        let mut reconstruct_start = last_idx;

        while reconstruct_start > 0 {
            reconstruct_start -= 1;
            match &self.active_formatting_elements[reconstruct_start] {
                FormattingEntry::Marker => {
                    reconstruct_start += 1;
                    break;
                }
                FormattingEntry::Element { name, .. } => {
                    // Check if in open elements (after body)
                    let in_stack = self.open_elements[body_idx + 1..].iter()
                        .any(|(n, _)| n == name);
                    if in_stack {
                        reconstruct_start += 1;
                        break;
                    }
                }
            }
        }

        // Now reopen elements from reconstruct_start to the end
        while reconstruct_start < self.active_formatting_elements.len() {
            let (name, attrs) = match &self.active_formatting_elements[reconstruct_start] {
                FormattingEntry::Element { name, attrs, .. } => (name.clone(), attrs.clone()),
                FormattingEntry::Marker => {
                    reconstruct_start += 1;
                    continue;
                }
            };

            // Create new element
            let new_node_id = self.sink.start_element(name.clone(), attrs.clone(), false);
            self.open_elements.push((name.clone(), new_node_id.clone()));

            // Update the entry with new node id
            self.active_formatting_elements[reconstruct_start] = FormattingEntry::Element {
                name,
                attrs,
                node_id: new_node_id,
            };

            reconstruct_start += 1;
        }
    }

    /// Adoption Agency Algorithm for formatting elements.
    /// Handles misnested tags like <b><i></b></i>.
    fn adoption_agency_algorithm(&mut self, tag_name: &str) -> bool {
        // Step 1: If the current node is the formatting element, just pop it
        if let Some((name, _)) = self.current_node() {
            if name == tag_name {
                // Pop from open elements
                let (popped_name, _) = self.open_elements.pop().unwrap();
                self.sink.end_element(popped_name);

                // Remove from active formatting list if present
                if let Some(idx) = self.active_formatting_elements.iter().rposition(|entry| {
                    matches!(entry, FormattingEntry::Element { name, .. } if name == tag_name)
                }) {
                    self.active_formatting_elements.remove(idx);
                }
                return true;
            }
        }

        // Step 2-4: Find the formatting element in the list
        let formatting_idx = match self.find_formatting_element_index(tag_name) {
            Some(idx) => idx,
            None => {
                // No matching formatting element - treat as any other end tag
                return false;
            }
        };

        // Get formatting element info
        let (fe_name, fe_node_idx) = match &self.active_formatting_elements[formatting_idx] {
            FormattingEntry::Element { name, .. } => {
                // Find in open elements
                let stack_idx = self.open_elements.iter().rposition(|(n, _)| n == name);
                match stack_idx {
                    Some(idx) => (name.clone(), idx),
                    None => {
                        // Not in open elements - remove from formatting list
                        self.active_formatting_elements.remove(formatting_idx);
                        return true;
                    }
                }
            }
            FormattingEntry::Marker => return false,
        };

        // Step 5: Check if formatting element is in scope
        if !self.has_element_in_scope(&fe_name) {
            // Parse error - do nothing
            return false;
        }

        // Step 6: Check if formatting element is the current node
        if fe_node_idx == self.open_elements.len() - 1 {
            // Just pop it
            let (popped_name, _) = self.open_elements.pop().unwrap();
            self.sink.end_element(popped_name);
            self.active_formatting_elements.remove(formatting_idx);
            return true;
        }

        // Step 7-8: Find furthest block
        let furthest_block_idx = self.find_furthest_block(fe_node_idx);

        if furthest_block_idx.is_none() {
            // No furthest block - pop elements up to and including formatting element
            while self.open_elements.len() > fe_node_idx {
                let (popped_name, _) = self.open_elements.pop().unwrap();
                self.sink.end_element(popped_name);
            }
            self.active_formatting_elements.remove(formatting_idx);
            return true;
        }

        // Simplified AAA: For complex cases, just close the formatting element
        // and remove it from the active formatting list.
        // Full AAA requires DOM manipulation not supported by current TreeSink.

        // Close all elements up to and including the formatting element
        while self.open_elements.len() > fe_node_idx {
            let (popped_name, _) = self.open_elements.pop().unwrap();
            self.sink.end_element(popped_name);
        }

        // Remove from active formatting
        self.active_formatting_elements.remove(formatting_idx);

        true
    }

    /// Find formatting element index in active formatting list.
    fn find_formatting_element_index(&self, tag_name: &str) -> Option<usize> {
        for (i, entry) in self.active_formatting_elements.iter().enumerate().rev() {
            match entry {
                FormattingEntry::Marker => return None,
                FormattingEntry::Element { name, .. } if name == tag_name => return Some(i),
                _ => continue,
            }
        }
        None
    }

    /// Find the furthest block after the formatting element.
    fn find_furthest_block(&self, fe_idx: usize) -> Option<usize> {
        for i in (fe_idx + 1)..self.open_elements.len() {
            let (name, _) = &self.open_elements[i];
            if SPECIAL_ELEMENTS.contains(&name.as_str()) {
                return Some(i);
            }
        }
        None
    }

    /// Check if a tag is a formatting element.
    fn is_formatting_element(tag_name: &str) -> bool {
        FORMATTING_ELEMENTS.contains(&tag_name)
    }

    pub fn build(mut self, tokens: Vec<Token>) -> ParseResult<S> {
        for token in tokens {
            self.process_token(token)?;
        }
        Ok(self.sink)
    }

    fn process_token(&mut self, token: Token) -> ParseResult<()> {
        trace!(mode = ?self.mode, token = ?token, "Processing token");

        match self.mode {
            InsertionMode::Initial => self.handle_initial(token)?,
            InsertionMode::BeforeHtml => self.handle_before_html(token)?,
            InsertionMode::BeforeHead => self.handle_before_head(token)?,
            InsertionMode::InHead => self.handle_in_head(token)?,
            InsertionMode::InHeadNoscript => self.handle_in_head_noscript(token)?,
            InsertionMode::AfterHead => self.handle_after_head(token)?,
            InsertionMode::InBody => self.handle_in_body(token)?,
            InsertionMode::Text => self.handle_text(token)?,
            // Table modes
            InsertionMode::InTable => self.handle_in_table(token)?,
            InsertionMode::InTableText => self.handle_in_table_text(token)?,
            InsertionMode::InCaption => self.handle_in_caption(token)?,
            InsertionMode::InColumnGroup => self.handle_in_column_group(token)?,
            InsertionMode::InTableBody => self.handle_in_table_body(token)?,
            InsertionMode::InRow => self.handle_in_row(token)?,
            InsertionMode::InCell => self.handle_in_cell(token)?,
            // Select modes
            InsertionMode::InSelect => self.handle_in_select(token)?,
            InsertionMode::InSelectInTable => self.handle_in_select_in_table(token)?,
            // Template mode
            InsertionMode::InTemplate => self.handle_in_template(token)?,
            // After modes
            InsertionMode::AfterBody => self.handle_after_body(token)?,
            InsertionMode::InFrameset => self.handle_in_frameset(token)?,
            InsertionMode::AfterFrameset => self.handle_after_frameset(token)?,
            InsertionMode::AfterAfterBody => self.handle_after_after_body(token)?,
            InsertionMode::AfterAfterFrameset => self.handle_after_after_frameset(token)?,
        }

        Ok(())
    }

    fn handle_initial(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Doctype {
                name,
                public_id,
                system_id,
            } => {
                // Determine quirks mode based on doctype
                self.quirks_mode = self.determine_quirks_mode(&name, &public_id, &system_id);
                self.sink.doctype(name, public_id, system_id);
                self.mode = InsertionMode::BeforeHtml;
            }
            Token::Character(ch) if ch.is_whitespace() => {
                // Ignore whitespace
            }
            Token::Comment(data) => {
                self.sink.comment(data);
            }
            _ => {
                // No doctype, switch to quirks mode
                self.quirks_mode = QuirksMode::Quirks;
                self.mode = InsertionMode::BeforeHtml;
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    /// Determine quirks mode based on doctype.
    fn determine_quirks_mode(&self, name: &str, public_id: &str, system_id: &str) -> QuirksMode {
        // HTML5 doctype: <!DOCTYPE html>
        if name.eq_ignore_ascii_case("html") && public_id.is_empty() && system_id.is_empty() {
            return QuirksMode::NoQuirks;
        }

        // Check for known quirks-triggering public identifiers
        let public_lower = public_id.to_lowercase();

        // Full quirks mode triggers
        let quirks_public_ids = [
            "-//w3o//dtd w3 html strict 3.0//en//",
            "-/w3c/dtd html 4.0 transitional/en",
            "html",
        ];

        for quirks_id in &quirks_public_ids {
            if public_lower.starts_with(quirks_id) {
                return QuirksMode::Quirks;
            }
        }

        // HTML 4.01 Transitional/Frameset without system identifier = quirks
        if public_lower.contains("html 4.01") && system_id.is_empty() {
            if public_lower.contains("transitional") || public_lower.contains("frameset") {
                return QuirksMode::Quirks;
            }
        }

        // XHTML 1.0 Transitional/Frameset = limited quirks
        if public_lower.contains("xhtml 1.0") {
            if public_lower.contains("transitional") || public_lower.contains("frameset") {
                return QuirksMode::LimitedQuirks;
            }
        }

        // HTML 4.01 Transitional/Frameset with system identifier = limited quirks
        if public_lower.contains("html 4.01") && !system_id.is_empty() {
            if public_lower.contains("transitional") || public_lower.contains("frameset") {
                return QuirksMode::LimitedQuirks;
            }
        }

        // Default to no quirks for valid doctypes
        QuirksMode::NoQuirks
    }

    fn handle_before_html(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::StartTag {
                name, attrs, self_closing: _
            } if name == "html" => {
                let node_id = self.sink.start_element(name.clone(), attrs.into_iter().collect(), false);
                self.open_elements.push((name, node_id));
                self.mode = InsertionMode::BeforeHead;
            }
            Token::Character(ch) if ch.is_whitespace() => {
                // Ignore
            }
            Token::Comment(data) => {
                self.sink.comment(data);
            }
            _ => {
                // Implied html start tag
                let node_id = self.sink.start_element("html".to_string(), vec![], false);
                self.open_elements.push(("html".to_string(), node_id));
                self.mode = InsertionMode::BeforeHead;
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    fn handle_before_head(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::StartTag {
                name, attrs, self_closing: _
            } if name == "head" => {
                let node_id = self.sink.start_element(name.clone(), attrs.into_iter().collect(), false);
                self.open_elements.push((name, node_id));
                self.mode = InsertionMode::InHead;
            }
            Token::Character(ch) if ch.is_whitespace() => {
                // Ignore
            }
            Token::Comment(data) => {
                self.sink.comment(data);
            }
            _ => {
                // Implied head start tag
                let node_id = self.sink.start_element("head".to_string(), vec![], false);
                self.open_elements.push(("head".to_string(), node_id));
                self.mode = InsertionMode::InHead;
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    fn handle_in_head(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Character(ch) => {
                // Add all characters to the text buffer (whitespace and non-whitespace)
                self.text_buffer.push(ch);
            }
            Token::Comment(data) => {
                self.flush_text();
                self.sink.comment(data);
            }
            Token::StartTag {
                name, attrs, self_closing: _
            } if matches!(name.as_str(), "base" | "basefont" | "bgsound" | "link" | "meta") => {
                let _node_id = self.sink.start_element(name.clone(), attrs.into_iter().collect(), true);
                // Void element, no end tag needed
            }
            Token::StartTag {
                name, attrs, self_closing: _
            } if matches!(name.as_str(), "title" | "style" | "script") => {
                let node_id = self.sink.start_element(name.clone(), attrs.into_iter().collect(), false);
                self.open_elements.push((name, node_id));
                // Note: In a full implementation, we'd switch to RCDATA/RAWTEXT mode here
            }
            Token::EndTag { name } if matches!(name.as_str(), "title" | "style" | "script") => {
                self.flush_text();
                if let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
            }
            Token::EndTag { name } if name == "head" => {
                self.flush_text();
                if let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
                self.mode = InsertionMode::AfterHead;
            }
            Token::EndTag { name } if matches!(name.as_str(), "body" | "html" | "br") => {
                // Act as if </head> seen
                self.flush_text();
                if let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
                self.mode = InsertionMode::AfterHead;
                self.process_token(Token::EndTag { name })?;
            }
            Token::StartTag { .. } => {
                // Any other start tag - close head implicitly
                self.flush_text();
                if let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
                self.mode = InsertionMode::AfterHead;
                self.process_token(token)?;
            }
            _ => {
                // Ignore or handle as error
            }
        }
        Ok(())
    }

    fn handle_after_head(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Character(ch) if ch.is_whitespace() => {
                self.text_buffer.push(ch);
            }
            Token::Comment(data) => {
                self.sink.comment(data);
            }
            Token::StartTag {
                name, attrs, self_closing: _
            } if name == "body" => {
                let node_id = self.sink.start_element(name.clone(), attrs.into_iter().collect(), false);
                self.open_elements.push((name, node_id));
                self.mode = InsertionMode::InBody;
            }
            _ => {
                // Implied body start tag
                let node_id = self.sink.start_element("body".to_string(), vec![], false);
                self.open_elements.push(("body".to_string(), node_id));
                self.mode = InsertionMode::InBody;
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    fn handle_in_body(&mut self, token: Token) -> ParseResult<()> {
        // Armed by a "pre"/"listing" start tag for exactly the NEXT token.
        let ignore_lf = std::mem::take(&mut self.ignore_lf);
        match token {
            Token::Character(ch) => {
                if ch == '\0' {
                    // Ignore null characters
                    return Ok(());
                }
                // HTML §13.2.6.4.7: the newline authors put right after
                // `<pre>` is markup, not content. Chrome drops it; keeping
                // it gave every `<pre>\n...` block an empty first line
                // once `white-space: pre` made newlines line boxes.
                if ignore_lf && ch == '\n' {
                    return Ok(());
                }

                // Reconstruct active formatting elements before inserting text
                self.reconstruct_active_formatting();

                // Accumulate text characters
                self.text_buffer.push(ch);
            }
            Token::Comment(data) => {
                self.flush_text();
                self.sink.comment(data);
            }
            Token::StartTag {
                name,
                attrs,
                self_closing,
            } => {
                // Flush any pending text before starting a new element
                self.flush_text();

                // Handle table specially - switch to InTable mode
                if name == "table" {
                    self.close_p_element();
                    let node_id = self.sink.start_element(
                        name.clone(),
                        attrs.into_iter().collect(),
                        false,
                    );
                    self.open_elements.push((name, node_id));
                    // Push marker when entering table (scope boundary)
                    self.push_formatting_marker();
                    self.mode = InsertionMode::InTable;
                    return Ok(());
                }

                // Handle button - push marker
                if name == "button" {
                    if self.has_element_in_scope("button") {
                        // Parse error - close the button
                        self.adoption_agency_algorithm("button");
                    }
                    self.reconstruct_active_formatting();
                    let attrs_vec: Vec<(String, String)> = attrs.into_iter().collect();
                    let node_id = self.sink.start_element(name.clone(), attrs_vec, false);
                    self.open_elements.push((name, node_id));
                    self.push_formatting_marker();
                    return Ok(());
                }

                // Handle anchor specially - check for existing anchor in formatting list
                if name == "a" {
                    // If there's an 'a' element in the active formatting list, use AAA
                    if self.find_formatting_element_index("a").is_some() {
                        self.adoption_agency_algorithm("a");
                    }
                    self.reconstruct_active_formatting();
                    let attrs_vec: Vec<(String, String)> = attrs.into_iter().collect();
                    let node_id = self.sink.start_element(name.clone(), attrs_vec.clone(), false);
                    self.open_elements.push((name.clone(), node_id.clone()));
                    self.push_formatting_element(name, attrs_vec, node_id);
                    return Ok(());
                }

                // Close p element if necessary
                if P_CLOSING_ELEMENTS.contains(&name.as_str()) {
                    self.close_p_element();
                }

                // Reconstruct active formatting elements before most elements
                if !SPECIAL_ELEMENTS.contains(&name.as_str()) {
                    self.reconstruct_active_formatting();
                }

                let is_void = VOID_ELEMENTS.contains(&name.as_str());
                let attrs_vec: Vec<(String, String)> = attrs.into_iter().collect();
                let node_id = self.sink.start_element(
                    name.clone(),
                    attrs_vec.clone(),
                    self_closing || is_void,
                );

                if !is_void && !self_closing {
                    self.open_elements.push((name.clone(), node_id.clone()));

                    if matches!(name.as_str(), "pre" | "listing") {
                        self.ignore_lf = true;
                    }

                    // Track formatting elements
                    if Self::is_formatting_element(&name) {
                        self.push_formatting_element(name, attrs_vec, node_id);
                    }
                }
            }
            Token::EndTag { name } => {
                self.flush_text();

                // Handle formatting elements with AAA
                if Self::is_formatting_element(&name) {
                    self.adoption_agency_algorithm(&name);
                    return Ok(());
                }

                // Find matching open element
                let mut found = false;
                for i in (0..self.open_elements.len()).rev() {
                    if self.open_elements[i].0 == name {
                        // Close all elements up to and including this one
                        for _ in i..self.open_elements.len() {
                            let (tag_name, _) = self.open_elements.pop().unwrap();
                            self.sink.end_element(tag_name);
                        }
                        found = true;
                        break;
                    }
                }

                if !found {
                    // Parse error - ignore
                    trace!("Unmatched end tag: {}", name);
                }

                // Check if we closed body
                if name == "body" {
                    self.mode = InsertionMode::AfterBody;
                }
            }
            Token::Eof => {
                self.flush_text();

                // Close all open elements
                while let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ==================== TABLE MODE HANDLERS ====================

    fn handle_in_table(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Character(ch) if ch.is_whitespace() => {
                // Switch to InTableText mode to accumulate whitespace
                self.original_mode = Some(InsertionMode::InTable);
                self.mode = InsertionMode::InTableText;
                self.pending_table_chars.push(*ch);
            }
            Token::Character(_) => {
                // Non-whitespace character - foster parent it
                self.foster_parenting = true;
                self.handle_in_body(token)?;
                self.foster_parenting = false;
            }
            Token::Comment(data) => {
                self.sink.comment(data.clone());
            }
            Token::StartTag { name, attrs, .. } => {
                match name.as_str() {
                    "caption" => {
                        self.clear_stack_to_table_context();
                        let node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            false,
                        );
                        self.open_elements.push((name.clone(), node_id));
                        self.mode = InsertionMode::InCaption;
                    }
                    "colgroup" => {
                        self.clear_stack_to_table_context();
                        let node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            false,
                        );
                        self.open_elements.push((name.clone(), node_id));
                        self.mode = InsertionMode::InColumnGroup;
                    }
                    "col" => {
                        // Implied colgroup
                        self.clear_stack_to_table_context();
                        let node_id = self.sink.start_element(
                            "colgroup".to_string(),
                            vec![],
                            false,
                        );
                        self.open_elements.push(("colgroup".to_string(), node_id));
                        self.mode = InsertionMode::InColumnGroup;
                        self.process_token(token)?;
                    }
                    "tbody" | "tfoot" | "thead" => {
                        self.clear_stack_to_table_context();
                        let node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            false,
                        );
                        self.open_elements.push((name.clone(), node_id));
                        self.mode = InsertionMode::InTableBody;
                    }
                    "td" | "th" | "tr" => {
                        // Implied tbody
                        self.clear_stack_to_table_context();
                        let node_id = self.sink.start_element(
                            "tbody".to_string(),
                            vec![],
                            false,
                        );
                        self.open_elements.push(("tbody".to_string(), node_id));
                        self.mode = InsertionMode::InTableBody;
                        self.process_token(token)?;
                    }
                    "table" => {
                        // Parse error - close current table and reprocess
                        if self.has_element_in_table_scope("table") {
                            self.pop_until("table");
                            self.reset_insertion_mode();
                            self.process_token(token)?;
                        }
                    }
                    _ => {
                        // Anything else - foster parent
                        self.foster_parenting = true;
                        self.handle_in_body(token)?;
                        self.foster_parenting = false;
                    }
                }
            }
            Token::EndTag { name } => {
                match name.as_str() {
                    "table" => {
                        if self.has_element_in_table_scope("table") {
                            self.pop_until("table");
                            self.sink.end_element("table".to_string());
                            self.reset_insertion_mode();
                        }
                    }
                    "body" | "caption" | "col" | "colgroup" | "html" | "tbody"
                    | "td" | "tfoot" | "th" | "thead" | "tr" => {
                        // Parse error - ignore
                    }
                    _ => {
                        // Anything else - foster parent
                        self.foster_parenting = true;
                        self.handle_in_body(token)?;
                        self.foster_parenting = false;
                    }
                }
            }
            Token::Eof => {
                self.handle_in_body(token)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_in_table_text(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Character(ch) => {
                if ch == '\0' {
                    // Ignore
                } else {
                    self.pending_table_chars.push(ch);
                }
            }
            _ => {
                // Process pending characters
                let chars: Vec<char> = std::mem::take(&mut self.pending_table_chars);
                let has_non_whitespace = chars.iter().any(|c| !c.is_whitespace());

                if has_non_whitespace {
                    // Foster parent all characters
                    self.foster_parenting = true;
                    for ch in chars {
                        self.text_buffer.push(ch);
                    }
                    self.flush_text();
                    self.foster_parenting = false;
                } else {
                    // Insert whitespace normally
                    for ch in chars {
                        self.text_buffer.push(ch);
                    }
                    self.flush_text();
                }

                // Switch back to original mode
                if let Some(mode) = self.original_mode.take() {
                    self.mode = mode;
                }
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    fn handle_in_caption(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::EndTag { name } if name == "caption" => {
                if self.has_element_in_table_scope("caption") {
                    // Flush any pending text before closing caption
                    self.flush_text();
                    // Pop until caption
                    while let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag.clone());
                        if tag == "caption" {
                            break;
                        }
                    }
                    self.mode = InsertionMode::InTable;
                }
            }
            Token::StartTag { name, .. }
                if matches!(
                    name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr"
                ) =>
            {
                // Close caption and reprocess
                if self.has_element_in_table_scope("caption") {
                    // Flush any pending text before closing caption
                    self.flush_text();
                    while let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag.clone());
                        if tag == "caption" {
                            break;
                        }
                    }
                    self.mode = InsertionMode::InTable;
                    self.process_token(token)?;
                }
            }
            Token::EndTag { name } if name == "table" => {
                // Close caption and reprocess
                if self.has_element_in_table_scope("caption") {
                    // Flush any pending text before closing caption
                    self.flush_text();
                    while let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag.clone());
                        if tag == "caption" {
                            break;
                        }
                    }
                    self.mode = InsertionMode::InTable;
                    self.process_token(token)?;
                }
            }
            _ => {
                // Process as InBody
                self.handle_in_body(token)?;
            }
        }
        Ok(())
    }

    fn handle_in_column_group(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Character(ch) if ch.is_whitespace() => {
                self.text_buffer.push(*ch);
            }
            Token::Comment(data) => {
                self.sink.comment(data.clone());
            }
            Token::StartTag { name, attrs, self_closing } if name == "col" => {
                let _node_id = self.sink.start_element(
                    name.clone(),
                    attrs.clone().into_iter().collect(),
                    true, // col is void
                );
                let _ = self_closing; // col is always void
            }
            Token::EndTag { name } if name == "colgroup" => {
                if self.current_node_name() == Some("colgroup") {
                    let (tag, _) = self.open_elements.pop().unwrap();
                    self.sink.end_element(tag);
                    self.mode = InsertionMode::InTable;
                }
            }
            Token::EndTag { name } if name == "col" => {
                // Parse error - ignore
            }
            Token::Eof => {
                self.handle_in_body(token)?;
            }
            _ => {
                // Close colgroup and reprocess
                if self.current_node_name() == Some("colgroup") {
                    let (tag, _) = self.open_elements.pop().unwrap();
                    self.sink.end_element(tag);
                    self.mode = InsertionMode::InTable;
                    self.process_token(token)?;
                }
            }
        }
        Ok(())
    }

    fn handle_in_table_body(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::StartTag { name, attrs, .. } if name == "tr" => {
                self.clear_stack_to_table_body_context();
                let node_id = self.sink.start_element(
                    name.clone(),
                    attrs.clone().into_iter().collect(),
                    false,
                );
                self.open_elements.push((name.clone(), node_id));
                self.mode = InsertionMode::InRow;
            }
            Token::StartTag { name, .. } if TABLE_CELL_ELEMENTS.contains(&name.as_str()) => {
                // Implied tr
                self.clear_stack_to_table_body_context();
                let node_id = self.sink.start_element("tr".to_string(), vec![], false);
                self.open_elements.push(("tr".to_string(), node_id));
                self.mode = InsertionMode::InRow;
                self.process_token(token)?;
            }
            Token::EndTag { name } if TABLE_SECTION_ELEMENTS.contains(&name.as_str()) => {
                if self.has_element_in_table_scope(name) {
                    self.clear_stack_to_table_body_context();
                    if let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag);
                    }
                    self.mode = InsertionMode::InTable;
                }
            }
            Token::StartTag { name, .. }
                if matches!(name.as_str(), "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead") =>
            {
                // Close current tbody/thead/tfoot and reprocess
                if self.has_element_in_table_scope("tbody")
                    || self.has_element_in_table_scope("thead")
                    || self.has_element_in_table_scope("tfoot")
                {
                    self.clear_stack_to_table_body_context();
                    if let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag);
                    }
                    self.mode = InsertionMode::InTable;
                    self.process_token(token)?;
                }
            }
            Token::EndTag { name } if name == "table" => {
                // Close current tbody/thead/tfoot and reprocess
                if self.has_element_in_table_scope("tbody")
                    || self.has_element_in_table_scope("thead")
                    || self.has_element_in_table_scope("tfoot")
                {
                    self.clear_stack_to_table_body_context();
                    if let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag);
                    }
                    self.mode = InsertionMode::InTable;
                    self.process_token(token)?;
                }
            }
            Token::EndTag { name }
                if matches!(name.as_str(), "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr") =>
            {
                // Parse error - ignore
            }
            _ => {
                self.handle_in_table(token)?;
            }
        }
        Ok(())
    }

    fn handle_in_row(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::StartTag { name, attrs, .. } if TABLE_CELL_ELEMENTS.contains(&name.as_str()) => {
                self.clear_stack_to_table_row_context();
                let node_id = self.sink.start_element(
                    name.clone(),
                    attrs.clone().into_iter().collect(),
                    false,
                );
                self.open_elements.push((name.clone(), node_id));
                self.mode = InsertionMode::InCell;
            }
            Token::EndTag { name } if name == "tr" => {
                if self.has_element_in_table_scope("tr") {
                    self.clear_stack_to_table_row_context();
                    if let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag);
                    }
                    self.mode = InsertionMode::InTableBody;
                }
            }
            Token::StartTag { name, .. }
                if matches!(
                    name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                // Close tr and reprocess
                if self.has_element_in_table_scope("tr") {
                    self.clear_stack_to_table_row_context();
                    if let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag);
                    }
                    self.mode = InsertionMode::InTableBody;
                    self.process_token(token)?;
                }
            }
            Token::EndTag { name } if name == "table" => {
                // Close tr and reprocess
                if self.has_element_in_table_scope("tr") {
                    self.clear_stack_to_table_row_context();
                    if let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag);
                    }
                    self.mode = InsertionMode::InTableBody;
                    self.process_token(token)?;
                }
            }
            Token::EndTag { name } if TABLE_SECTION_ELEMENTS.contains(&name.as_str()) => {
                if self.has_element_in_table_scope(name) {
                    // Close tr first
                    if self.has_element_in_table_scope("tr") {
                        self.clear_stack_to_table_row_context();
                        if let Some((tag, _)) = self.open_elements.pop() {
                            self.sink.end_element(tag);
                        }
                        self.mode = InsertionMode::InTableBody;
                        self.process_token(token)?;
                    }
                }
            }
            Token::EndTag { name }
                if matches!(name.as_str(), "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th") =>
            {
                // Parse error - ignore
            }
            _ => {
                self.handle_in_table(token)?;
            }
        }
        Ok(())
    }

    fn handle_in_cell(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::EndTag { name } if TABLE_CELL_ELEMENTS.contains(&name.as_str()) => {
                if self.has_element_in_table_scope(name) {
                    self.flush_text();
                    // Pop until the cell
                    while let Some((tag, _)) = self.open_elements.pop() {
                        self.sink.end_element(tag.clone());
                        if TABLE_CELL_ELEMENTS.contains(&tag.as_str()) {
                            break;
                        }
                    }
                    self.mode = InsertionMode::InRow;
                }
            }
            Token::StartTag { name, .. }
                if matches!(
                    name.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead" | "tr"
                ) =>
            {
                // Close cell and reprocess
                if self.has_element_in_table_scope("td") || self.has_element_in_table_scope("th") {
                    self.close_cell();
                    self.process_token(token)?;
                }
            }
            Token::EndTag { name }
                if matches!(name.as_str(), "body" | "caption" | "col" | "colgroup" | "html") =>
            {
                // Parse error - ignore
            }
            Token::EndTag { name }
                if matches!(name.as_str(), "table" | "tbody" | "tfoot" | "thead" | "tr") =>
            {
                if self.has_element_in_table_scope(name) {
                    // Close cell and reprocess
                    self.close_cell();
                    self.process_token(token)?;
                }
            }
            _ => {
                // Process as InBody
                self.handle_in_body(token)?;
            }
        }
        Ok(())
    }

    /// Reset insertion mode based on the current stack of open elements.
    fn reset_insertion_mode(&mut self) {
        for i in (0..self.open_elements.len()).rev() {
            let (name, _) = &self.open_elements[i];
            let last = i == 0;

            match name.as_str() {
                "select" => {
                    // Check if we're in table context
                    for j in (0..i).rev() {
                        match self.open_elements[j].0.as_str() {
                            "template" => break,
                            "table" => {
                                self.mode = InsertionMode::InSelectInTable;
                                return;
                            }
                            _ => {}
                        }
                    }
                    self.mode = InsertionMode::InSelect;
                    return;
                }
                "td" | "th" if !last => {
                    self.mode = InsertionMode::InCell;
                    return;
                }
                "tr" => {
                    self.mode = InsertionMode::InRow;
                    return;
                }
                "tbody" | "thead" | "tfoot" => {
                    self.mode = InsertionMode::InTableBody;
                    return;
                }
                "caption" => {
                    self.mode = InsertionMode::InCaption;
                    return;
                }
                "colgroup" => {
                    self.mode = InsertionMode::InColumnGroup;
                    return;
                }
                "table" => {
                    self.mode = InsertionMode::InTable;
                    return;
                }
                "template" => {
                    self.mode = *self.template_insertion_modes.last()
                        .unwrap_or(&InsertionMode::InTemplate);
                    return;
                }
                "head" if !last => {
                    self.mode = InsertionMode::InHead;
                    return;
                }
                "body" => {
                    self.mode = InsertionMode::InBody;
                    return;
                }
                "frameset" => {
                    self.mode = InsertionMode::InFrameset;
                    return;
                }
                "html" => {
                    if self.head_element.is_none() {
                        self.mode = InsertionMode::BeforeHead;
                    } else {
                        self.mode = InsertionMode::AfterHead;
                    }
                    return;
                }
                _ => {}
            }
        }
        self.mode = InsertionMode::InBody;
    }

    // ==================== END TABLE MODE HANDLERS ====================

    // ==================== SELECT MODE HANDLERS ====================

    fn handle_in_select(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Character('\0') => {
                // Ignore null character
            }
            Token::Character(ch) => {
                self.text_buffer.push(*ch);
            }
            Token::Comment(data) => {
                self.flush_text();
                self.sink.comment(data.clone());
            }
            Token::StartTag { name, attrs, self_closing } => {
                self.flush_text();
                match name.as_str() {
                    "html" => {
                        // Process using InBody rules
                        self.handle_in_body(token)?;
                    }
                    "option" => {
                        // Close current option if open
                        if self.current_node_name() == Some("option") {
                            let (tag, _) = self.open_elements.pop().unwrap();
                            self.sink.end_element(tag);
                        }
                        let node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            *self_closing,
                        );
                        self.open_elements.push((name.clone(), node_id));
                    }
                    "optgroup" => {
                        // Close current option if open
                        if self.current_node_name() == Some("option") {
                            let (tag, _) = self.open_elements.pop().unwrap();
                            self.sink.end_element(tag);
                        }
                        // Close current optgroup if open
                        if self.current_node_name() == Some("optgroup") {
                            let (tag, _) = self.open_elements.pop().unwrap();
                            self.sink.end_element(tag);
                        }
                        let node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            *self_closing,
                        );
                        self.open_elements.push((name.clone(), node_id));
                    }
                    "select" => {
                        // Parse error - close select
                        if self.has_element_in_select_scope("select") {
                            self.pop_until("select");
                            self.sink.end_element("select".to_string());
                            self.reset_insertion_mode();
                        }
                    }
                    "input" | "keygen" | "textarea" => {
                        // Parse error - close select and reprocess
                        if self.has_element_in_select_scope("select") {
                            self.pop_until("select");
                            self.sink.end_element("select".to_string());
                            self.reset_insertion_mode();
                            self.process_token(token)?;
                        }
                    }
                    "script" | "template" => {
                        self.handle_in_head(token)?;
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::EndTag { name } => {
                self.flush_text();
                match name.as_str() {
                    "optgroup" => {
                        // If current is option and previous is optgroup, close option first
                        if self.current_node_name() == Some("option") {
                            let len = self.open_elements.len();
                            if len >= 2 && self.open_elements[len - 2].0 == "optgroup" {
                                let (tag, _) = self.open_elements.pop().unwrap();
                                self.sink.end_element(tag);
                            }
                        }
                        if self.current_node_name() == Some("optgroup") {
                            let (tag, _) = self.open_elements.pop().unwrap();
                            self.sink.end_element(tag);
                        }
                    }
                    "option" => {
                        if self.current_node_name() == Some("option") {
                            let (tag, _) = self.open_elements.pop().unwrap();
                            self.sink.end_element(tag);
                        }
                    }
                    "select" => {
                        if self.has_element_in_select_scope("select") {
                            self.pop_until("select");
                            self.sink.end_element("select".to_string());
                            self.reset_insertion_mode();
                        }
                    }
                    "template" => {
                        self.handle_in_head(token)?;
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::Eof => {
                self.handle_in_body(token)?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_in_select_in_table(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::StartTag { name, .. }
                if matches!(
                    name.as_str(),
                    "caption" | "table" | "tbody" | "tfoot" | "thead" | "tr" | "td" | "th"
                ) =>
            {
                // Parse error - close select and reprocess
                self.flush_text();
                self.pop_until("select");
                self.sink.end_element("select".to_string());
                self.reset_insertion_mode();
                self.process_token(token)?;
            }
            Token::EndTag { name }
                if matches!(
                    name.as_str(),
                    "caption" | "table" | "tbody" | "tfoot" | "thead" | "tr" | "td" | "th"
                ) =>
            {
                // Parse error - close select if element is in scope
                if self.has_element_in_table_scope(name) {
                    self.flush_text();
                    self.pop_until("select");
                    self.sink.end_element("select".to_string());
                    self.reset_insertion_mode();
                    self.process_token(token)?;
                }
            }
            _ => {
                self.handle_in_select(token)?;
            }
        }
        Ok(())
    }

    /// Check if an element is in select scope.
    fn has_element_in_select_scope(&self, tag_name: &str) -> bool {
        for (name, _) in self.open_elements.iter().rev() {
            if name == tag_name {
                return true;
            }
            // Select scope limiters are only optgroup and option
            if name != "optgroup" && name != "option" {
                return false;
            }
        }
        false
    }

    // ==================== TEMPLATE MODE HANDLERS ====================

    fn handle_in_template(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Character(_) | Token::Comment(_) => {
                self.handle_in_body(token)?;
            }
            Token::StartTag { name, .. } => {
                match name.as_str() {
                    "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes"
                    | "script" | "style" | "template" | "title" => {
                        self.handle_in_head(token)?;
                    }
                    "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => {
                        self.template_insertion_modes.pop();
                        self.template_insertion_modes.push(InsertionMode::InTable);
                        self.mode = InsertionMode::InTable;
                        self.process_token(token)?;
                    }
                    "col" => {
                        self.template_insertion_modes.pop();
                        self.template_insertion_modes.push(InsertionMode::InColumnGroup);
                        self.mode = InsertionMode::InColumnGroup;
                        self.process_token(token)?;
                    }
                    "tr" => {
                        self.template_insertion_modes.pop();
                        self.template_insertion_modes.push(InsertionMode::InTableBody);
                        self.mode = InsertionMode::InTableBody;
                        self.process_token(token)?;
                    }
                    "td" | "th" => {
                        self.template_insertion_modes.pop();
                        self.template_insertion_modes.push(InsertionMode::InRow);
                        self.mode = InsertionMode::InRow;
                        self.process_token(token)?;
                    }
                    _ => {
                        self.template_insertion_modes.pop();
                        self.template_insertion_modes.push(InsertionMode::InBody);
                        self.mode = InsertionMode::InBody;
                        self.process_token(token)?;
                    }
                }
            }
            Token::EndTag { name } => {
                match name.as_str() {
                    "template" => {
                        self.handle_in_head(token)?;
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::Eof => {
                // Check if template is on the stack
                if !self.open_elements.iter().any(|(n, _)| n == "template") {
                    // Stop parsing
                } else {
                    // Parse error - pop until template
                    while let Some((name, _)) = self.open_elements.pop() {
                        self.sink.end_element(name.clone());
                        if name == "template" {
                            break;
                        }
                    }
                    self.template_insertion_modes.pop();
                    self.reset_insertion_mode();
                    self.process_token(token)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ==================== FRAMESET MODE HANDLERS ====================

    fn handle_in_frameset(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Character(ch) if ch.is_whitespace() => {
                self.text_buffer.push(*ch);
            }
            Token::Comment(data) => {
                self.flush_text();
                self.sink.comment(data.clone());
            }
            Token::StartTag { name, attrs, .. } => {
                self.flush_text();
                match name.as_str() {
                    "html" => {
                        self.handle_in_body(token)?;
                    }
                    "frameset" => {
                        let node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            false,
                        );
                        self.open_elements.push((name.clone(), node_id));
                    }
                    "frame" => {
                        let _node_id = self.sink.start_element(
                            name.clone(),
                            attrs.clone().into_iter().collect(),
                            true, // frame is void
                        );
                    }
                    "noframes" => {
                        self.handle_in_head(token)?;
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::EndTag { name } => {
                self.flush_text();
                match name.as_str() {
                    "frameset" => {
                        if self.current_node_name() == Some("html") {
                            // Parse error - ignore
                        } else {
                            if let Some((tag, _)) = self.open_elements.pop() {
                                self.sink.end_element(tag);
                            }
                            // If not fragment parsing and current is not frameset
                            if self.fragment_context.is_none()
                                && self.current_node_name() != Some("frameset")
                            {
                                self.mode = InsertionMode::AfterFrameset;
                            }
                        }
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::Eof => {
                if self.current_node_name() != Some("html") {
                    // Parse error
                }
                // Stop parsing
            }
            _ => {
                // Parse error - ignore
            }
        }
        Ok(())
    }

    fn handle_after_frameset(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Character(ch) if ch.is_whitespace() => {
                self.text_buffer.push(*ch);
            }
            Token::Comment(data) => {
                self.flush_text();
                self.sink.comment(data.clone());
            }
            Token::StartTag { name, .. } => {
                self.flush_text();
                match name.as_str() {
                    "html" => {
                        self.handle_in_body(token)?;
                    }
                    "noframes" => {
                        self.handle_in_head(token)?;
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::EndTag { name } => {
                self.flush_text();
                if name == "html" {
                    self.mode = InsertionMode::AfterAfterFrameset;
                }
                // Other end tags - parse error, ignore
            }
            Token::Eof => {
                // Stop parsing
            }
            _ => {
                // Parse error - ignore
            }
        }
        Ok(())
    }

    fn handle_after_after_frameset(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Comment(data) => {
                self.sink.comment(data.clone());
            }
            Token::Character(ch) if ch.is_whitespace() => {
                self.handle_in_body(token)?;
            }
            Token::StartTag { name, .. } => {
                match name.as_str() {
                    "html" => {
                        self.handle_in_body(token)?;
                    }
                    "noframes" => {
                        self.handle_in_head(token)?;
                    }
                    _ => {
                        // Parse error - ignore
                    }
                }
            }
            Token::Eof => {
                // Stop parsing
            }
            _ => {
                // Parse error - ignore
            }
        }
        Ok(())
    }

    // ==================== TEXT MODE HANDLER ====================

    fn handle_text(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Character(ch) => {
                self.text_buffer.push(ch);
            }
            Token::EndTag { name } => {
                self.flush_text();
                if let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
                let _ = name; // Verify tag matches, but be lenient
                if let Some(mode) = self.original_mode.take() {
                    self.mode = mode;
                } else {
                    self.mode = InsertionMode::InBody;
                }
            }
            Token::Eof => {
                // Parse error - close element
                self.flush_text();
                if let Some((tag_name, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag_name);
                }
                if let Some(mode) = self.original_mode.take() {
                    self.mode = mode;
                } else {
                    self.mode = InsertionMode::InBody;
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ==================== IN HEAD NOSCRIPT HANDLER ====================

    fn handle_in_head_noscript(&mut self, token: Token) -> ParseResult<()> {
        match &token {
            Token::Comment(data) => {
                self.sink.comment(data.clone());
            }
            Token::StartTag { name, .. }
                if matches!(
                    name.as_str(),
                    "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style"
                ) =>
            {
                self.handle_in_head(token)?;
            }
            Token::EndTag { name } if name == "noscript" => {
                if let Some((tag, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag);
                }
                self.mode = InsertionMode::InHead;
            }
            Token::Character(ch) if ch.is_whitespace() => {
                self.handle_in_head(token)?;
            }
            Token::EndTag { name } if name == "br" => {
                // Act as if </noscript> seen
                if let Some((tag, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag);
                }
                self.mode = InsertionMode::InHead;
                self.process_token(token)?;
            }
            Token::StartTag { name, .. } if matches!(name.as_str(), "head" | "noscript") => {
                // Parse error - ignore
            }
            _ => {
                // Parse error - close noscript and reprocess
                if let Some((tag, _)) = self.open_elements.pop() {
                    self.sink.end_element(tag);
                }
                self.mode = InsertionMode::InHead;
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    // ==================== AFTER MODE HANDLERS ====================

    fn handle_after_body(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Character(ch) if ch.is_whitespace() => {
                // Reprocess in "in body" mode
                self.mode = InsertionMode::InBody;
                self.process_token(Token::Character(ch))?;
                self.mode = InsertionMode::AfterBody;
            }
            Token::Comment(data) => {
                self.sink.comment(data);
            }
            Token::EndTag { name } if name == "html" => {
                self.mode = InsertionMode::AfterAfterBody;
            }
            Token::Eof => {
                // Done
            }
            _ => {
                // Parse error - reprocess in "in body" mode
                self.mode = InsertionMode::InBody;
                self.process_token(token)?;
            }
        }
        Ok(())
    }

    fn handle_after_after_body(&mut self, token: Token) -> ParseResult<()> {
        match token {
            Token::Comment(data) => {
                self.sink.comment(data);
            }
            Token::Character(ch) if ch.is_whitespace() => {
                self.mode = InsertionMode::InBody;
                self.process_token(Token::Character(ch))?;
                self.mode = InsertionMode::AfterAfterBody;
            }
            Token::Eof => {
                // Done
            }
            _ => {
                // Parse error - reprocess in "in body" mode
                self.mode = InsertionMode::InBody;
                self.process_token(token)?;
            }
        }
        Ok(())
    }
}

/// Build a DOM tree from tokens (full document parsing).
pub fn build_tree<S: TreeSink>(tokens: Vec<Token>, sink: S) -> ParseResult<S> {
    let builder = TreeBuilder::new(sink);
    builder.build(tokens)
}

/// Build a DOM tree from tokens in fragment parsing mode.
///
/// This is used for innerHTML, insertAdjacentHTML, and similar APIs.
/// The context element determines how the fragment is parsed.
pub fn build_tree_fragment<S: TreeSink>(
    tokens: Vec<Token>,
    sink: S,
    context: FragmentContext,
) -> ParseResult<S> {
    let builder = TreeBuilder::new_fragment(sink, context);
    builder.build(tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::tokenize;

    #[derive(Debug)]
    struct TestSink {
        events: Vec<String>,
    }

    impl TestSink {
        fn new() -> Self {
            Self { events: Vec::new() }
        }
    }

    impl TreeSink for TestSink {
        type NodeId = usize;

        fn doctype(&mut self, name: String, _public_id: String, _system_id: String) {
            self.events.push(format!("doctype:{}", name));
        }

        fn start_element(
            &mut self,
            name: String,
            attrs: Vec<(String, String)>,
            _self_closing: bool,
        ) -> Self::NodeId {
            let attr_str = attrs
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join(" ");
            if attr_str.is_empty() {
                self.events.push(format!("start:{}", name));
            } else {
                self.events.push(format!("start:{}[{}]", name, attr_str));
            }
            self.events.len()
        }

        fn end_element(&mut self, name: String) {
            self.events.push(format!("end:{}", name));
        }

        fn text(&mut self, data: String) {
            if !data.trim().is_empty() {
                self.events.push(format!("text:{}", data));
            }
        }

        fn comment(&mut self, data: String) {
            self.events.push(format!("comment:{}", data));
        }

        fn current_node(&self) -> Option<Self::NodeId> {
            Some(0)
        }

        fn in_scope(&self, _tag_name: &str) -> bool {
            false
        }

        fn pop_until(&mut self, _tag_name: &str) {}

        fn close_p_element_in_button_scope(&mut self) {}

        fn reconstruct_active_formatting_elements(&mut self) {}

        // AAA methods - simplified for testing
        fn create_element(&mut self, name: String, _attrs: Vec<(String, String)>) -> Self::NodeId {
            self.events.push(format!("create:{}", name));
            self.events.len()
        }

        fn append_child(&mut self, _parent: Self::NodeId, _child: Self::NodeId) {}
        fn remove_from_parent(&mut self, _node: Self::NodeId) {}
        fn reparent_children(&mut self, _from: Self::NodeId, _to: Self::NodeId) {}
        fn insert_before(&mut self, _parent: Self::NodeId, _node: Self::NodeId, _reference: Option<Self::NodeId>) {}
        fn get_parent(&self, _node: Self::NodeId) -> Option<Self::NodeId> { None }
        fn get_tag_name(&self, _node: Self::NodeId) -> Option<String> { None }
    }

    #[test]
    fn test_simple_document() {
        let html = "<html><head></head><body></body></html>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        
        assert!(result.events.contains(&"start:html".to_string()));
        assert!(result.events.contains(&"start:head".to_string()));
        assert!(result.events.contains(&"start:body".to_string()));
    }

    #[test]
    fn pre_drops_the_newline_right_after_its_start_tag_only() {
        // HTML §13.2.6.4.7: `<pre>\n` — the first LF is ignored; every later
        // one (and one after any other tag) is content.
        let html = "<pre>\nfirst\n\nthird\n</pre><div>\nx</div>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        let texts: Vec<&String> = result
            .events
            .iter()
            .filter(|e| e.starts_with("text:"))
            .collect();
        assert_eq!(texts, ["text:first\n\nthird\n", "text:\nx"]);
    }

    #[test]
    fn test_implicit_tags() {
        let html = "<p>Hello</p>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        
        // Should have implicit html, head, body
        assert!(result.events.contains(&"start:html".to_string()));
        assert!(result.events.contains(&"start:body".to_string()));
        assert!(result.events.contains(&"start:p".to_string()));
    }

    #[test]
    fn test_doctype() {
        let html = "<!DOCTYPE html><html></html>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        
        assert!(result.events.contains(&"doctype:html".to_string()));
    }

    #[test]
    fn test_nested_elements() {
        let html = "<div><span>text</span></div>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        
        assert!(result.events.contains(&"start:div".to_string()));
        assert!(result.events.contains(&"start:span".to_string()));
        assert!(result.events.contains(&"end:span".to_string()));
        assert!(result.events.contains(&"end:div".to_string()));
    }

    #[test]
    fn test_void_elements() {
        let html = "<p>Line 1<br>Line 2</p>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        
        assert!(result.events.contains(&"start:br".to_string()));
        // br should not have an end tag
        assert!(!result.events.contains(&"end:br".to_string()));
    }

    #[test]
    fn test_malformed_nesting() {
        let html = "<div><span></div></span>";
        let tokens = tokenize(html).unwrap();
        let sink = TestSink::new();
        let result = build_tree(tokens, sink).unwrap();
        
        // Should recover gracefully
        assert!(!result.events.is_empty());
    }
}

