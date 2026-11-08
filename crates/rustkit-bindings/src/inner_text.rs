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
//! changed its display was not seen. CH2.E13b: `white-space`, the table
//! cell tab and row line feed, and a `<p>`'s two breaks come from computed
//! style too (`white-space` and display of the layout), so a `white-space:
//! pre` div keeps its spaces, a `display: table-cell` span gets its tab, a
//! `<td>` set to `display: block` gets none, and a `display: contents` `<p>`
//! (no box) adds no breaks, as in Chromium 143.

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
    /// The `white-space` keyword, if the lines carry one.
    pub white_space: Option<String>,
}

impl Computed {
    /// From the `name\tvalue` lines `DomBindings::set_computed_styles`
    /// publishes. `None` when the lines carry no `display`.
    pub(crate) fn from_lines(lines: &str) -> Option<Computed> {
        let mut display = None;
        let mut visible = true;
        let mut white_space = None;
        for line in lines.lines() {
            match line.split_once('\t') {
                Some(("display", v)) => display = Some(v.trim().to_string()),
                Some(("visibility", v)) => visible = v.trim() == "visible",
                Some(("white-space", v)) => white_space = Some(v.trim().to_string()),
                _ => {}
            }
        }
        Some(Computed { display: display?, visible, white_space })
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

/// How a text node's white space is processed (CSS Text 3 §4.1).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Space {
    /// `normal`, `nowrap`: every run of white space is one space.
    Collapse,
    /// `pre-line`: spaces and tabs collapse, line feeds are kept.
    PreLine,
    /// `pre`, `pre-wrap`, `break-spaces`: everything is kept.
    Preserve,
}

/// A text node's white-space processing: its parent's computed
/// `white-space` (the property inherits, and a text node has no style of
/// its own); with no style for the parent, the UA rule of the nearest
/// ancestor tag (`PRESERVE`).
fn space_of(node: &Rc<Node>, style: StyleOf) -> Space {
    if let Some(ws) = node.parent().and_then(|p| style(&p)).and_then(|c| c.white_space) {
        return match ws.as_str() {
            "pre" | "pre-wrap" | "break-spaces" => Space::Preserve,
            "pre-line" => Space::PreLine,
            _ => Space::Collapse,
        };
    }
    let mut current = node.parent();
    while let Some(n) = current {
        if n.tag_name().is_some_and(|t| PRESERVE.contains(&t)) {
            return Space::Preserve;
        }
        current = n.parent();
    }
    Space::Collapse
}

/// The element's computed display is one of `displays`; with no computed
/// style, its tag is one of `tags` (the UA stylesheet's table parts).
fn display_is(node: &Node, style: StyleOf, displays: &[&str], tags: &[&str]) -> bool {
    match style(node) {
        Some(c) => displays.contains(&c.display.as_str()),
        None => node.tag_name().is_some_and(|t| tags.contains(&t)),
    }
}

fn is_cell(node: &Node, style: StyleOf) -> bool {
    display_is(node, style, &["table-cell"], &["td", "th"])
}

fn is_row(node: &Node, style: StyleOf) -> bool {
    display_is(node, style, &["table-row"], &["tr"])
}

fn is_row_group(node: &Node, style: StyleOf) -> bool {
    display_is(
        node,
        style,
        &["table-row-group", "table-header-group", "table-footer-group"],
        &["thead", "tbody", "tfoot"],
    )
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

    fn text(&mut self, data: &str, space: Space) {
        for c in data.chars() {
            if space == Space::PreLine && c == '\n' {
                // A kept line feed; the spaces either side of it go.
                self.line_feed();
                continue;
            }
            if space != Space::Preserve && matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}') {
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
                self.text(data, space_of(node, self.style))
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
                    // `display: contents` makes no box, and Chromium gives a
                    // boxless `<p>` no breaks.
                    (_, Some(c)) if c.display == "contents" => 0,
                    ("p", _) => 2,
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
                if is_cell(node, self.style) {
                    // A cell that is not its row's last is followed by a tab.
                    if next_cell(node, self.style) {
                        self.flush_breaks();
                        self.space = false;
                        self.out.push('\t');
                    }
                } else if is_row(node, self.style) {
                    if next_row(node, self.style) {
                        self.line_feed();
                    }
                } else {
                    self.required_breaks(before);
                }
            }
            _ => {}
        }
    }
}

fn is_element(node: &Node) -> bool {
    matches!(node.node_type, NodeType::Element { .. })
}

/// Is `node` followed by another cell of its row?
fn next_cell(node: &Rc<Node>, style: StyleOf) -> bool {
    let mut next = node.next_sibling();
    while let Some(n) = next {
        if is_element(&n) && is_cell(&n, style) {
            return true;
        }
        next = n.next_sibling();
    }
    false
}

/// Is `row` followed by another row of its table (in this or a later row
/// group)?
fn next_row(row: &Rc<Node>, style: StyleOf) -> bool {
    let mut scope = row.clone();
    loop {
        let mut next = scope.next_sibling();
        while let Some(n) = next {
            if is_element(&n) {
                if is_row(&n, style) {
                    return true;
                }
                if is_row_group(&n, style)
                    && n.children().iter().any(|c| is_element(c) && is_row(c, style))
                {
                    return true;
                }
            }
            next = n.next_sibling();
        }
        match scope.parent() {
            Some(p) if is_row_group(&p, style) => scope = p,
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

    /// CH2.E13: when the last layout published computed styles, `display:none`
    /// and `visibility` drive the walk (the engine oracle is macOS+headless).
    #[test]
    fn inner_text_honours_published_display_none_and_visibility() {
        use super::{inner_text, Computed};
        use rustkit_dom::Node;
        use std::collections::HashMap;

        assert_eq!(
            Computed::from_lines("color\tred\ndisplay\tnone\nvisibility\thidden")
                .map(|c| (c.display, c.visible)),
            Some(("none".into(), false))
        );
        assert!(Computed::from_lines("color\tred").is_none());

        let document = Document::parse_html(
            "<div id=t><span id=h>hid</span><span id=v>vis</span>\
             <span id=i>invis<span id=r>ok</span></span></div>",
        )
        .unwrap();
        let t = document.get_element_by_id("t").unwrap();
        let ids: HashMap<&str, usize> = ["h", "v", "i", "r", "t"]
            .into_iter()
            .map(|id| (id, document.get_element_by_id(id).unwrap().id.raw()))
            .collect();
        let style = |n: &Node| -> Option<Computed> {
            let id = n.id.raw();
            if id == ids["h"] {
                Some(Computed {
                    display: "none".into(),
                    visible: true,
                    white_space: None,
                })
            } else if id == ids["i"] {
                Some(Computed {
                    display: "inline".into(),
                    visible: false,
                    white_space: None,
                })
            } else if id == ids["r"] || id == ids["v"] {
                Some(Computed {
                    display: "inline".into(),
                    visible: true,
                    white_space: None,
                })
            } else if id == ids["t"] {
                Some(Computed {
                    display: "block".into(),
                    visible: true,
                    white_space: None,
                })
            } else {
                None
            }
        };
        assert_eq!(inner_text(&t, &style), "visok");

        // Through DomBindings: published rows are what the getter reads.
        let doc = Rc::new(
            Document::parse_html("<div id=t><span id=h>hid</span>keep</div>").unwrap(),
        );
        let hid_id = doc.get_element_by_id("h").unwrap().id.raw();
        let t_id = doc.get_element_by_id("t").unwrap().id.raw();
        let b = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        b.set_document(doc).unwrap();
        let mut styles = HashMap::new();
        styles.insert(t_id, "display\tblock\nvisibility\tvisible".into());
        styles.insert(hid_id, "display\tnone\nvisibility\tvisible".into());
        b.set_computed_styles(styles);
        assert_eq!(
            eval_string(&b, "document.getElementById('t').innerText"),
            "keep"
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
