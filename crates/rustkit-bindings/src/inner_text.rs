//! `HTMLElement.innerText` (HTML §3.2.7), getter and setter.
//!
//! The setter is exact: it replaces the element's children with the
//! "rendered text fragment", Text nodes split by `<br>` elements.
//!
//! The getter runs the "rendered text collection steps" on the computed
//! `display` and `visibility` of the last layout (the caller forces one
//! after a script write, as for geometry reads). An element the layout has
//! no style for (no layout yet, or a node outside it) falls back to the UA
//! stylesheet defaults for its tag and the `hidden` attribute. CH2.E13: the
//! getter used only those defaults, so author CSS that hid an element or
//! changed its display was not seen. `white-space` is still read from the
//! tag (`PRESERVE`).

use rustkit_dom::{Document, Node, NodeType};
use std::rc::Rc;

/// Elements the UA stylesheet gives `display: none`. `noscript` is in it
/// because scripting is on whenever this getter can run.
const HIDDEN: &[&str] = &[
    "area", "base", "basefont", "datalist", "head", "link", "meta", "noembed", "noframes",
    "noscript", "param", "rp", "script", "style", "template", "title",
];

/// Elements the UA stylesheet makes block-level (one required line break
/// before and after). `p` is handled apart: it asks for two.
const BLOCK: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "caption",
    "center",
    "dd",
    "details",
    "dialog",
    "dir",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hgroup",
    "hr",
    "html",
    "legend",
    "li",
    "listing",
    "main",
    "menu",
    "nav",
    "ol",
    "optgroup",
    "option",
    "plaintext",
    "pre",
    "search",
    "section",
    "summary",
    "table",
    "ul",
    "xmp",
];

/// Elements whose text keeps its white space (`white-space: pre` or
/// `pre-wrap` in the UA stylesheet).
const PRESERVE: &[&str] = &["listing", "plaintext", "pre", "textarea", "xmp"];

/// What the last layout's computed style says about one element.
#[derive(Clone, Debug, Default)]
pub(crate) struct Computed {
    /// The `display` keyword (`none`, `block`, `inline`, `flex`, ...).
    pub display: String,
    /// `visibility` is `visible`.
    pub visible: bool,
}

impl Computed {
    /// From the `name\tvalue` lines `DomBindings::set_computed_styles`
    /// publishes. `None` when the lines carry no `display`.
    pub(crate) fn from_lines(lines: &str) -> Option<Computed> {
        let mut display = None;
        let mut visible = true;
        for line in lines.lines() {
            match line.split_once('\t') {
                Some(("display", v)) => display = Some(v.trim().to_string()),
                Some(("visibility", v)) => visible = v.trim() == "visible",
                _ => {}
            }
        }
        Some(Computed { display: display?, visible })
    }

    /// Block-level, or `table-caption`: one required line break each side.
    fn block_level(&self) -> bool {
        matches!(
            self.display.as_str(),
            "block" | "list-item" | "flex" | "grid" | "table" | "flow-root" | "table-caption"
        )
    }
}

/// The computed style of an element, if the layout has one for it.
pub(crate) type StyleOf<'a> = &'a dyn Fn(&Node) -> Option<Computed>;

fn is_hidden(node: &Node, style: StyleOf) -> bool {
    if let Some(c) = style(node) {
        return c.display == "none";
    }
    node.tag_name().is_some_and(|t| HIDDEN.contains(&t)) || node.get_attribute("hidden").is_some()
}

/// `visibility` of an element is `visible` (an element with no computed
/// style is taken as visible).
fn is_visible(node: &Node, style: StyleOf) -> bool {
    style(node).is_none_or(|c| c.visible)
}

/// Is `node` in a document and outside every UA-hidden subtree? Anything
/// else is "not being rendered", and its innerText is its textContent.
fn is_rendered(node: &Rc<Node>, style: StyleOf) -> bool {
    let mut current = Some(node.clone());
    while let Some(n) = current {
        match n.node_type {
            NodeType::Document => return true,
            NodeType::Element { .. } if is_hidden(&n, style) => return false,
            _ => {}
        }
        current = n.parent();
    }
    false
}

fn preserves_space(node: &Rc<Node>) -> bool {
    let mut current = node.parent();
    while let Some(n) = current {
        if n.tag_name().is_some_and(|t| PRESERVE.contains(&t)) {
            return true;
        }
        current = n.parent();
    }
    false
}

/// The rendered text, built as the collection steps walk the tree. White
/// space collapses as it arrives: a run of collapsible spaces is held as
/// one pending space, written only if more text follows on the same line.
struct Collector<'a> {
    style: StyleOf<'a>,
    out: String,
    /// The required line break count waiting to be written (the largest of
    /// a run; dropped if no text follows).
    breaks: usize,
    space: bool,
}

impl Collector<'_> {
    fn flush_breaks(&mut self) {
        if self.breaks > 0 && !self.out.is_empty() {
            self.out.push_str(&"\n".repeat(self.breaks));
        }
        self.breaks = 0;
    }

    fn text(&mut self, data: &str, preserve: bool) {
        for c in data.chars() {
            if !preserve && matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
                // A space at the start of a line is removed.
                self.space = self.breaks == 0 && !self.out.is_empty() && !self.out.ends_with('\n');
                continue;
            }
            self.flush_breaks();
            if self.space {
                self.out.push(' ');
                self.space = false;
            }
            self.out.push(c);
        }
    }

    /// A literal line feed (`<br>`, a table row end): kept even at the
    /// start or end, unlike a required line break.
    fn line_feed(&mut self) {
        self.flush_breaks();
        self.space = false;
        self.out.push('\n');
    }

    fn required_breaks(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.space = false;
        self.breaks = self.breaks.max(n);
    }

    fn walk(&mut self, node: &Rc<Node>) {
        match &node.node_type {
            NodeType::Text(data) => {
                // A text node's `visibility` is its parent's.
                if node.parent().is_some_and(|p| !is_visible(&p, self.style)) {
                    return;
                }
                self.text(data, preserves_space(node))
            }
            NodeType::Element { tag_name, .. } => {
                if is_hidden(node, self.style) {
                    return;
                }
                // Not `visible`: its children still count (one of them can
                // be visible again), but nothing of its own does, not its
                // line breaks, a `<br>` or a cell's tab.
                if !is_visible(node, self.style) {
                    for child in node.children() {
                        self.walk(&child);
                    }
                    return;
                }
                let tag = tag_name.as_str();
                let before = match (tag, (self.style)(node)) {
                    ("p", _) => 2,
                    ("tr", _) => 0,
                    (_, Some(c)) => usize::from(c.block_level()),
                    (t, None) if BLOCK.contains(&t) => 1,
                    _ => 0,
                };
                if tag == "br" {
                    self.line_feed();
                    return;
                }
                self.required_breaks(before);
                for child in node.children() {
                    self.walk(&child);
                }
                match tag {
                    // A cell that is not its row's last is followed by a tab.
                    "td" | "th" if next_cell(node) => {
                        self.flush_breaks();
                        self.space = false;
                        self.out.push('\t');
                    }
                    "tr" if next_row(node) => self.line_feed(),
                    _ => self.required_breaks(before),
                }
            }
            _ => {}
        }
    }
}

fn next_cell(node: &Rc<Node>) -> bool {
    let mut next = node.next_sibling();
    while let Some(n) = next {
        if matches!(n.tag_name(), Some("td" | "th")) {
            return true;
        }
        next = n.next_sibling();
    }
    false
}

/// Is `row` followed by another row of its table (in this or a later row
/// group)?
fn next_row(row: &Rc<Node>) -> bool {
    let mut scope = row.clone();
    loop {
        let mut next = scope.next_sibling();
        while let Some(n) = next {
            match n.tag_name() {
                Some("tr") => return true,
                Some("thead" | "tbody" | "tfoot")
                    if n.children().iter().any(|c| c.tag_name() == Some("tr")) =>
                {
                    return true
                }
                _ => {}
            }
            next = n.next_sibling();
        }
        match scope.parent() {
            Some(p) if matches!(p.tag_name(), Some("thead" | "tbody" | "tfoot")) => scope = p,
            _ => return false,
        }
    }
}

/// The innerText getter, on the computed style `style` gives.
pub(crate) fn inner_text(element: &Rc<Node>, style: StyleOf) -> String {
    if !is_rendered(element, style) {
        return element.text_content();
    }
    let mut collector = Collector { style, out: String::new(), breaks: 0, space: false };
    for child in element.children() {
        collector.walk(&child);
    }
    collector.out
}

/// The innerText setter: replace `element`'s children with Text nodes for
/// the lines of `text`, a `<br>` for each line break (CRLF, CR or LF).
pub(crate) fn set_inner_text(document: &Document, element: &Rc<Node>, text: &str) {
    for child in element.children() {
        child.remove_from_parent();
    }
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    for (i, line) in normalized.split('\n').enumerate() {
        if i > 0 {
            element.append_child(document.create_node(NodeType::Element {
                tag_name: String::from("br"),
                namespace: String::from("http://www.w3.org/1999/xhtml"),
                attributes: Default::default(),
            }));
        }
        if !line.is_empty() {
            element.append_child(document.create_node(NodeType::Text(line.to_string())));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{DomBindings, DomDirty};
    use rustkit_dom::Document;
    use rustkit_js::{JsRuntime, JsValue};
    use std::rc::Rc;

    fn bound(html: &str) -> DomBindings {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        bindings
            .set_document(Rc::new(Document::parse_html(html).unwrap()))
            .unwrap();
        bindings
    }

    fn eval_string(bindings: &DomBindings, script: &str) -> String {
        match bindings.evaluate(script).unwrap() {
            JsValue::String(s) => s,
            other => panic!("{script} evaluated to {other:?}"),
        }
    }

    fn inner(html: &str) -> String {
        eval_string(&bound(html), "document.getElementById('t').innerText")
    }

    #[test]
    fn inner_text_collapses_white_space_and_breaks_blocks() {
        assert_eq!(
            inner("<div id=t>  Hello,\n   <b>big</b>  world  <p>para</p>tail<div>x</div></div>"),
            "Hello, big world\n\npara\n\ntail\nx"
        );
        assert_eq!(
            inner("<div id=t><span>a </span> <span> b</span></div>"),
            "a b"
        );
        assert_eq!(inner("<div id=t>one<br>two<br></div>"), "one\ntwo\n");
        assert_eq!(inner("<div id=t><pre>  a\n  b</pre></div>"), "  a\n  b");
    }

    #[test]
    fn inner_text_skips_ua_hidden_content() {
        assert_eq!(
            inner(
                "<div id=t>a<script>var x;</script><style>p{}</style><span hidden>h</span>b</div>"
            ),
            "ab"
        );
        // Not being rendered: the textContent.
        let b = bound("<div id=t><script>  var x;</script></div>");
        assert_eq!(
            eval_string(&b, "document.querySelector('script').innerText"),
            "  var x;"
        );
        assert_eq!(
            eval_string(
                &b,
                "var d = document.createElement('div'); d.textContent = ' a  b '; d.innerText"
            ),
            " a  b "
        );
    }

    #[test]
    fn inner_text_lays_out_table_cells_and_rows() {
        assert_eq!(
            inner("<table id=t><tr><td>a</td><td>b</td></tr><tr><th>c</th><td>d</td></tr></table>"),
            "a\tb\nc\td"
        );
    }

    #[test]
    fn inner_text_setter_writes_lines_split_by_br() {
        let b = bound("<div id=t><p>old</p></div>");
        let _ = b.take_dirty();
        assert_eq!(
            eval_string(
                &b,
                "var t = document.getElementById('t'); t.innerText = 'one\\r\\ntwo\\n\\nthree'; \
                 t.innerHTML"
            ),
            "one<br>two<br><br>three"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
        assert_eq!(
            eval_string(
                &b,
                "t.innerText = ''; t.innerHTML + '|' + t.childNodes.length"
            ),
            "|0"
        );
        assert_eq!(
            eval_string(&b, "t.innerText = null; t.innerHTML + '|' + t.innerText"),
            "|"
        );
    }
}
