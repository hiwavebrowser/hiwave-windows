//! Table tree construction (HTML §13.2.6.4.9 to §13.2.6.4.15), asserted on
//! the resulting element tree rather than on the event stream.
//!
//! `TreeSink` here builds a real tree with the same semantics as
//! rustkit-dom's `DocumentSink`: `start_element` appends to the top of the
//! sink's own stack and pushes, `end_element` pops, `text` appends a text
//! node to the stack top, and `insert_before`/`append_child` do NOT detach
//! the node first (this sink panics if asked to attach an attached node, so
//! a tree builder that relies on implicit detaching fails loudly here).

use rustkit_html::{parse, TreeSink};

#[derive(Debug)]
enum Kind {
    Root,
    Element(String),
    Text(String),
    Comment(String),
}

#[derive(Debug)]
struct Node {
    kind: Kind,
    parent: Option<usize>,
    children: Vec<usize>,
}

struct Sink {
    nodes: Vec<Node>,
    stack: Vec<usize>,
}

impl Sink {
    fn new() -> Self {
        Self {
            nodes: vec![Node { kind: Kind::Root, parent: None, children: vec![] }],
            stack: vec![],
        }
    }

    fn top(&self) -> usize {
        self.stack.last().copied().unwrap_or(0)
    }

    fn new_node(&mut self, kind: Kind) -> usize {
        self.nodes.push(Node { kind, parent: None, children: vec![] });
        self.nodes.len() - 1
    }

    fn attach(&mut self, parent: usize, child: usize, before: Option<usize>) {
        assert!(
            self.nodes[child].parent.is_none(),
            "node {:?} attached while still attached elsewhere",
            self.nodes[child].kind
        );
        let idx = before
            .and_then(|r| self.nodes[parent].children.iter().position(|&c| c == r))
            .unwrap_or(self.nodes[parent].children.len());
        self.nodes[parent].children.insert(idx, child);
        self.nodes[child].parent = Some(parent);
    }

    fn name(&self, id: usize) -> Option<&str> {
        match &self.nodes[id].kind {
            Kind::Element(n) => Some(n),
            _ => None,
        }
    }

    fn find(&self, id: usize, tag: &str) -> Option<usize> {
        if self.name(id) == Some(tag) {
            return Some(id);
        }
        self.nodes[id].children.iter().find_map(|&c| self.find(c, tag))
    }

    /// Dump `id`'s children. Adjacent text nodes are merged (DocumentSink
    /// does not merge them; the spec does). With `full`, whitespace-only
    /// text and comments are shown too.
    fn dump_children(&self, id: usize, full: bool) -> String {
        let mut parts: Vec<String> = Vec::new();
        let mut text = String::new();
        let flush = |text: &mut String, parts: &mut Vec<String>| {
            if !text.is_empty() {
                if full || !text.trim().is_empty() {
                    parts.push(format!("{:?}", if full { text.as_str() } else { text.trim() }));
                }
                text.clear();
            }
        };
        for &c in &self.nodes[id].children {
            match &self.nodes[c].kind {
                Kind::Text(t) => text.push_str(t),
                Kind::Comment(t) => {
                    flush(&mut text, &mut parts);
                    if full {
                        parts.push(format!("<!--{}-->", t));
                    }
                }
                Kind::Element(n) => {
                    flush(&mut text, &mut parts);
                    let inner = self.dump_children(c, full);
                    if inner.is_empty() {
                        parts.push(n.clone());
                    } else {
                        parts.push(format!("{}({})", n, inner));
                    }
                }
                Kind::Root => unreachable!(),
            }
        }
        flush(&mut text, &mut parts);
        parts.join(", ")
    }
}

impl TreeSink for Sink {
    type NodeId = usize;

    fn doctype(&mut self, _: String, _: String, _: String) {}

    fn start_element(&mut self, name: String, _attrs: Vec<(String, String)>, self_closing: bool) -> usize {
        let id = self.new_node(Kind::Element(name));
        let parent = self.top();
        self.attach(parent, id, None);
        if !self_closing {
            self.stack.push(id);
        }
        id
    }

    fn end_element(&mut self, _name: String) {
        self.stack.pop();
    }

    fn text(&mut self, data: String) {
        if !data.is_empty() {
            let id = self.new_node(Kind::Text(data));
            let parent = self.top();
            self.attach(parent, id, None);
        }
    }

    fn comment(&mut self, data: String) {
        let id = self.new_node(Kind::Comment(data));
        let parent = self.top();
        self.attach(parent, id, None);
    }

    fn current_node(&self) -> Option<usize> {
        self.stack.last().copied()
    }

    fn in_scope(&self, tag_name: &str) -> bool {
        self.stack.iter().any(|&n| self.name(n) == Some(tag_name))
    }

    fn pop_until(&mut self, tag_name: &str) {
        while let Some(n) = self.stack.pop() {
            if self.name(n) == Some(tag_name) {
                break;
            }
        }
    }

    // Same simplification as DocumentSink.
    fn close_p_element_in_button_scope(&mut self) {
        self.pop_until("p");
    }

    fn reconstruct_active_formatting_elements(&mut self) {}

    fn create_element(&mut self, name: String, _attrs: Vec<(String, String)>) -> usize {
        self.new_node(Kind::Element(name))
    }

    fn append_child(&mut self, parent: usize, child: usize) {
        self.attach(parent, child, None);
    }

    fn remove_from_parent(&mut self, node: usize) {
        if let Some(p) = self.nodes[node].parent.take() {
            self.nodes[p].children.retain(|&c| c != node);
        }
    }

    fn reparent_children(&mut self, from: usize, to: usize) {
        for c in std::mem::take(&mut self.nodes[from].children) {
            self.nodes[c].parent = None;
            self.attach(to, c, None);
        }
    }

    fn insert_before(&mut self, parent: usize, node: usize, reference: Option<usize>) {
        self.attach(parent, node, reference);
    }

    fn get_parent(&self, node: usize) -> Option<usize> {
        self.nodes[node].parent
    }

    fn get_tag_name(&self, node: usize) -> Option<String> {
        self.name(node).map(str::to_string)
    }
}

fn parse_sink(html: &str) -> Sink {
    parse(html, Sink::new()).unwrap()
}

/// Children of `<body>`: elements and non-whitespace text.
fn body(html: &str) -> String {
    let s = parse_sink(html);
    let b = s.find(0, "body").expect("no body");
    s.dump_children(b, false)
}

/// Children of `<body>` including whitespace-only text and comments.
fn body_full(html: &str) -> String {
    let s = parse_sink(html);
    let b = s.find(0, "body").expect("no body");
    s.dump_children(b, true)
}

// (a) The #621 repro: explicit tbody, newlines between rows.
#[test]
fn a_explicit_tbody_rows_separated_by_newlines() {
    let html = "<table><tbody>\n<tr><td>a</td></tr>\n<tr><td>b</td></tr>\n</tbody></table>\n<p>after</p>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(td("a")), tr(td("b")))), p("after")"#
    );
}

// (b) Implied tbody: one tbody for all rows.
#[test]
fn b_implied_tbody_rows_separated_by_newlines() {
    let html = "<table>\n<tr><td>a</td></tr>\n<tr><td>b</td></tr>\n</table>\n<p>after</p>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(td("a")), tr(td("b")))), p("after")"#
    );
}

// (c) No whitespace at all: already correct, pinned.
#[test]
fn c_no_whitespace_between_rows() {
    let html = "<table><tbody><tr><td>a</td></tr><tr><td>b</td></tr></tbody></table><p>after</p>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(td("a")), tr(td("b")))), p("after")"#
    );
}

// (d) Whitespace between <td>s and between <table> and the first <tr> is
// inserted as text where it appears (§13.2.6.4.10).
#[test]
fn d_whitespace_between_cells_and_before_first_row() {
    let html = "<table>\n <tr>\n <td>a</td>\n <td>b</td>\n </tr>\n</table>";
    assert_eq!(body(html), r#"table(tbody(tr(td("a"), td("b"))))"#);
    assert_eq!(
        body_full(html),
        r#"table("\n ", tbody(tr("\n ", td("a"), "\n ", td("b"), "\n "), "\n"))"#
    );
}

// (e) Content after </table> is a sibling of the table.
#[test]
fn e_content_after_table_is_a_sibling() {
    let rows = "<tr><td>a</td></tr>\n<tr><td>b</td></tr>\n";
    assert_eq!(
        body(&format!("<table>\n{rows}</table>\n<p>x</p>")),
        r#"table(tbody(tr(td("a")), tr(td("b")))), p("x")"#
    );
    assert_eq!(
        body(&format!("<table>\n{rows}</table>\nbare text")),
        r#"table(tbody(tr(td("a")), tr(td("b")))), "bare text""#
    );
    assert_eq!(
        body(&format!("<table>\n{rows}</table>\n<table>\n{rows}</table>")),
        r#"table(tbody(tr(td("a")), tr(td("b")))), table(tbody(tr(td("a")), tr(td("b"))))"#
    );
}

// (f) thead/tbody/tfoot with whitespace between sections and rows.
#[test]
fn f_sections_with_whitespace_one_each() {
    let html = "<table>\n<thead>\n<tr><th>h</th></tr>\n</thead>\n<tbody>\n<tr><td>a</td></tr>\n<tr><td>b</td></tr>\n</tbody>\n<tfoot>\n<tr><td>f</td></tr>\n</tfoot>\n</table>\n<p>after</p>";
    assert_eq!(
        body(html),
        r#"table(thead(tr(th("h"))), tbody(tr(td("a")), tr(td("b"))), tfoot(tr(td("f")))), p("after")"#
    );
}

// (g) Two explicit tbody elements stay two.
#[test]
fn g_two_explicit_tbodies_stay_two() {
    let html = "<table>\n<tbody>\n<tr><td>a</td></tr>\n</tbody>\n<tbody>\n<tr><td>b</td></tr>\n</tbody>\n</table>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(td("a"))), tbody(tr(td("b"))))"#
    );
}

// (h) Nested table in a cell, whitespace everywhere.
#[test]
fn h_nested_table_in_cell() {
    let html = "<table>\n<tr>\n<td>\n<table>\n<tr>\n<td>in</td>\n</tr>\n</table>\n</td>\n<td>next</td>\n</tr>\n<tr><td>row2</td></tr>\n</table>\n<p>after</p>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(td(table(tbody(tr(td("in"))))), td("next")), tr(td("row2")))), p("after")"#
    );
}

// (i) caption and colgroup/col before the rows.
#[test]
fn i_caption_and_colgroup_before_rows() {
    let html = "<table>\n<caption>cap</caption>\n<colgroup><col><col></colgroup>\n<tr><td>a</td></tr>\n<tr><td>b</td></tr>\n</table>\n<p>after</p>";
    assert_eq!(
        body(html),
        r#"table(caption("cap"), colgroup(col, col), tbody(tr(td("a")), tr(td("b")))), p("after")"#
    );
}

// (j) Wikipedia-shaped infobox.
#[test]
fn j_wikipedia_infobox_then_article_text() {
    let html = "<table class=\"infobox\"><tbody>\n<tr><th colspan=\"2\">Title</th></tr>\n<tr><td>k</td><td>v</td></tr>\n</tbody></table>\n<p>Article text</p>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(th("Title")), tr(td("k"), td("v")))), p("Article text")"#
    );
}

// (k) Non-whitespace text directly in <table> or <tr> is foster-parented
// before the table (§13.2.6.4.10, "in table text", anything else).
#[test]
fn k_text_in_table_or_row_is_foster_parented() {
    assert_eq!(
        body("<table>stray<tr><td>a</td></tr></table>"),
        r#""stray", table(tbody(tr(td("a"))))"#
    );
    assert_eq!(
        body("<table><tr>stray<td>a</td></tr></table>"),
        r#""stray", table(tbody(tr(td("a"))))"#
    );
}

// (l) A <div> directly in <table> is foster-parented before the table.
#[test]
fn l_div_in_table_is_foster_parented() {
    assert_eq!(
        body("<table><div>d</div><tr><td>a</td></tr></table><p>after</p>"),
        r#"div("d"), table(tbody(tr(td("a")))), p("after")"#
    );
    // Unclosed fostered element: </table> still closes the table.
    assert_eq!(
        body("<table><div>d</table><p>after</p>"),
        r#"div("d"), table, p("after")"#
    );
}

// (m) Missing end tags.
#[test]
fn m_missing_end_tags() {
    assert_eq!(
        body("<table><tr><td>a<tr><td>b</table><p>x"),
        r#"table(tbody(tr(td("a")), tr(td("b")))), p("x")"#
    );
}

// (n) </table> while a <td> is open closes cell, row, body and table.
#[test]
fn n_end_table_while_cell_open() {
    assert_eq!(
        body("<table>\n<tbody>\n<tr>\n<td>a\n</table>\n<p>after</p>"),
        r#"table(tbody(tr(td("a")))), p("after")"#
    );
}

// (o) Comments between rows stay where they are.
#[test]
fn o_comments_between_rows() {
    let html = "<table><tbody>\n<tr><td>a</td></tr>\n<!--c--><tr><td>b</td></tr>\n</tbody></table>";
    assert_eq!(
        body_full(html),
        r#"table(tbody("\n", tr(td("a")), "\n", <!--c-->, tr(td("b")), "\n"))"#
    );
}

// (p) CR, tab and form feed are ASCII whitespace like LF.
#[test]
fn p_cr_tab_ff_are_whitespace() {
    for ws in ["\r", "\t", "\x0C", "\r\n", " \t\x0C\r\n "] {
        let html = format!("<table>{ws}<tbody>{ws}<tr>{ws}<td>a</td>{ws}</tr>{ws}<tr><td>b</td></tr>{ws}</tbody>{ws}</table>{ws}<p>after</p>");
        assert_eq!(
            body(&html),
            r#"table(tbody(tr(td("a")), tr(td("b")))), p("after")"#,
            "whitespace {ws:?}"
        );
    }
}

// ---- Neighbours ----

#[test]
fn select_template_script_in_cells_undisturbed() {
    let html = "<table>\n<tr>\n<td><select>\n<option>o</option>\n</select></td>\n<td><template><b>t</b></template></td>\n<td><script>var a = 1;</script></td>\n</tr>\n</table>\n<p>after</p>";
    assert_eq!(
        body(html),
        r#"table(tbody(tr(td(select(option("o"))), td(template(b("t"))), td(script("var a = 1;"))))), p("after")"#
    );
}

#[test]
fn script_and_style_directly_in_table_stay_in_table() {
    // §13.2.6.4.9: "style", "script", "template" start tags in "in table"
    // are processed using the "in head" rules: inserted in place.
    assert_eq!(
        body("<table>\n<script>s</script>\n<style>x{}</style>\n<tr><td>a</td></tr>\n</table>"),
        r#"table(script("s"), style("x{}"), tbody(tr(td("a"))))"#
    );
}

#[test]
fn whitespace_in_cell_text_preserved() {
    assert_eq!(
        body_full("<table><tr><td>  a \n b  </td></tr></table>"),
        r#"table(tbody(tr(td("  a \n b  "))))"#
    );
}

#[test]
fn pre_and_textarea_leading_newline_unchanged_in_cells() {
    assert_eq!(
        body_full("<table><tr><td><pre>\nx\n</pre></td></tr></table>"),
        r#"table(tbody(tr(td(pre("x\n")))))"#
    );
    // Textarea content is RCDATA; its leading newline handling is whatever
    // the tokenizer does today, pinned by comparing against a non-table use.
    let in_body = body_full("<textarea>\nx</textarea>");
    let in_cell = body_full("<table><tr><td><textarea>\nx</textarea></td></tr></table>");
    assert_eq!(in_cell, format!("table(tbody(tr(td({in_body}))))"));
}

#[test]
fn table_inside_p_closes_the_p() {
    assert_eq!(
        body("<!DOCTYPE html><p>before<table>\n<tr><td>a</td></tr>\n</table>after"),
        r#"p("before"), table(tbody(tr(td("a")))), "after""#
    );
}

#[test]
fn text_in_fostered_element_stays_in_it_when_a_row_starts() {
    assert_eq!(
        body("<table><tr><div>x<td>a</td></tr></table>"),
        r#"div("x"), table(tbody(tr(td("a"))))"#
    );
}

#[test]
fn hidden_input_in_table_stays_in_table() {
    assert_eq!(
        body("<table><input type=hidden><tr><td>a</td></tr></table>"),
        r#"table(input, tbody(tr(td("a"))))"#
    );
}
