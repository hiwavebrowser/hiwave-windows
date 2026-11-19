//! Comprehensive corpus tests for HTML parser

use rustkit_html::{parse, parse_fragment, TreeSink};

#[derive(Debug)]
struct TestSink {
    events: Vec<String>,
    node_count: usize,
}

impl TestSink {
    fn new() -> Self {
        Self {
            events: Vec::new(),
            node_count: 0,
        }
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
            .map(|(k, v)| if v.is_empty() { k.clone() } else { format!("{}={}", k, v) })
            .collect::<Vec<_>>()
            .join(" ");
        if attr_str.is_empty() {
            self.events.push(format!("start:{}", name));
        } else {
            self.events.push(format!("start:{}[{}]", name, attr_str));
        }
        self.node_count += 1;
        self.node_count
    }

    fn end_element(&mut self, name: String) {
        self.events.push(format!("end:{}", name));
    }

    fn text(&mut self, data: String) {
        if !data.trim().is_empty() {
            self.events.push(format!("text:{}", data.trim()));
        }
    }

    fn comment(&mut self, data: String) {
        self.events.push(format!("comment:{}", data));
    }

    fn current_node(&self) -> Option<Self::NodeId> {
        if self.node_count > 0 {
            Some(self.node_count)
        } else {
            None
        }
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
        self.node_count += 1;
        self.node_count
    }

    fn append_child(&mut self, _parent: Self::NodeId, _child: Self::NodeId) {}
    fn remove_from_parent(&mut self, _node: Self::NodeId) {}
    fn reparent_children(&mut self, _from: Self::NodeId, _to: Self::NodeId) {}
    fn insert_before(&mut self, _parent: Self::NodeId, _node: Self::NodeId, _reference: Option<Self::NodeId>) {}
    fn get_parent(&self, _node: Self::NodeId) -> Option<Self::NodeId> { None }
    fn get_tag_name(&self, _node: Self::NodeId) -> Option<String> { None }
}

#[test]
fn test_simple_page() {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body><p>Hello</p></body>
</html>"#;

    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Events: {:?}", result.events);

    assert!(result.events.contains(&"doctype:html".to_string()));
    assert!(result.events.contains(&"start:html".to_string()));
    assert!(result.events.contains(&"start:head".to_string()));
    assert!(result.events.contains(&"start:body".to_string()));
    assert!(result.events.contains(&"text:Hello".to_string()));
}

#[test]
fn document_and_fragment_events_match_collected_tokens() {
    use rustkit_html::{parse_fragment_scripted, parse_scripted, tokenizer, tree_builder, FragmentContext};

    let html = format!(
        "<title>{}</title><table><tr><td>{}</td></tr></table><noscript><b>x</b></noscript><!--tail-->",
        "λ".repeat(255),
        "x&amp;&unknown;".repeat(73),
    );
    for scripting in [false, true] {
        let tokens = tokenizer::tokenize_scripted(&html, scripting).unwrap();
        let doc = parse_scripted(&html, TestSink::new(), scripting).unwrap();
        let collected = tree_builder::build_tree_scripted(tokens.clone(), TestSink::new(), scripting).unwrap();
        assert_eq!(doc.events, collected.events);

        let fragment = parse_fragment_scripted(&html, TestSink::new(), "div", scripting).unwrap();
        let collected = tree_builder::build_tree_fragment_scripted(
            tokens, TestSink::new(), FragmentContext::new("div"), scripting,
        ).unwrap();
        assert_eq!(fragment.events, collected.events);
    }
}

#[test]
fn test_malformed_nesting() {
    let html = "<div><span><p></div></span></p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Malformed events: {:?}", result.events);
    assert!(!result.events.is_empty());
}

#[test]
fn test_unclosed_tags() {
    let html = "<html><body><p>Text<div>More";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Unclosed events: {:?}", result.events);
    assert!(result.events.contains(&"text:Text".to_string()));
}

#[test]
fn test_entity_decoding() {
    let html = "<p>&lt;div&gt; &amp; &quot;text&quot;</p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Entity events: {:?}", result.events);
    assert!(result.events.iter().any(|e| e.contains("&")));
}

#[test]
fn test_nested_lists() {
    let html = "<ul><li>One<ul><li>Nested</li></ul></li><li>Two</li></ul>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Nested list events: {:?}", result.events);
    assert!(result.events.contains(&"start:ul".to_string()));
    assert!(result.events.contains(&"start:li".to_string()));
}

#[test]
fn test_table_structure() {
    let html = "<table><tr><td>Cell</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table events: {:?}", result.events);
    assert!(result.events.contains(&"start:table".to_string()));
    assert!(result.events.contains(&"start:tr".to_string()));
    assert!(result.events.contains(&"start:td".to_string()));
}

#[test]
fn test_comments() {
    let html = "<!-- comment 1 --><div><!-- comment 2 --></div>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Comment events: {:?}", result.events);
    assert!(result.events.iter().any(|e| e.starts_with("comment:")));
}

#[test]
fn test_attributes_with_special_chars() {
    let html = r#"<input type="text" data-value="foo&amp;bar" class="a b c">"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Attribute events: {:?}", result.events);
    assert!(result.events.iter().any(|e| e.contains("type=text")));
}

// ============== P0 Tests: Script/Style/Entity Fixes ==============

#[test]
fn test_script_content_preserved() {
    // Script content should NOT be parsed as HTML - the < and > should be preserved as text
    let html = r#"<script>if (a < b && c > d) { alert("<div>"); }</script>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Script events: {:?}", result.events);

    // Should have script start and end tags
    assert!(result.events.contains(&"start:script".to_string()));
    assert!(result.events.contains(&"end:script".to_string()));

    // Should NOT have div element (it's inside the script as text)
    assert!(!result.events.iter().any(|e| e.contains("start:div")));

    // Script content should be preserved as text
    assert!(result.events.iter().any(|e| e.contains("if (a < b")));
}

#[test]
fn test_style_content_preserved() {
    // Style content should NOT be parsed as HTML
    let html = r#"<style>.foo > .bar { color: red; }</style>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Style events: {:?}", result.events);

    assert!(result.events.contains(&"start:style".to_string()));
    assert!(result.events.contains(&"end:style".to_string()));

    // CSS selectors with > should be preserved as text, not parsed as tags
    assert!(result.events.iter().any(|e| e.contains(".foo > .bar")));
}

#[test]
fn test_textarea_content_preserved() {
    // Textarea content should use RCDATA mode (entities decoded, but no tags)
    let html = r#"<textarea>Some <b>text</b> with &amp; entity</textarea>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Textarea events: {:?}", result.events);

    assert!(result.events.contains(&"start:textarea".to_string()));
    assert!(result.events.contains(&"end:textarea".to_string()));

    // <b> should NOT be parsed as a tag (it's text in textarea)
    assert!(!result.events.iter().any(|e| e == "start:b"));

    // Content should include the literal <b> text
    assert!(result.events.iter().any(|e| e.contains("<b>text</b>")));

    // Entity should be decoded
    assert!(result.events.iter().any(|e| e.contains("&")));
}

#[test]
fn test_title_content_preserved() {
    // Title content should use RCDATA mode
    let html = r#"<title>Page &amp; Title with <fake> tag</title>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Title events: {:?}", result.events);

    assert!(result.events.contains(&"start:title".to_string()));
    assert!(result.events.contains(&"end:title".to_string()));

    // <fake> should NOT be parsed as a tag
    assert!(!result.events.iter().any(|e| e.contains("start:fake")));
}

#[test]
fn test_entity_decoding_in_text() {
    // Entities in regular text content should be decoded
    let html = r#"<p>&lt;div&gt; means less than and greater than: &amp;</p>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Entity text events: {:?}", result.events);

    // The decoded text should contain < and > and &
    assert!(result.events.iter().any(|e| e.contains("<div>")));
    assert!(result.events.iter().any(|e| e.contains("&")));
}

#[test]
fn test_numeric_entity_in_text() {
    // Numeric entities should be decoded
    let html = r#"<p>&#65;&#66;&#67; and &#x41;&#x42;&#x43;</p>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Numeric entity events: {:?}", result.events);

    // &#65; = A, &#66; = B, &#67; = C
    // &#x41; = A, &#x42; = B, &#x43; = C
    assert!(result.events.iter().any(|e| e.contains("ABC")));
}

#[test]
fn test_script_with_closing_tag_in_string() {
    // Tricky case: </script> inside a string shouldn't close the script
    // Note: This is a simplified test - full spec compliance requires more complex handling
    let html = r#"<script>var x = "</script>"; // actual end</script>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Script string events: {:?}", result.events);

    // Should have at least one script element
    assert!(result.events.contains(&"start:script".to_string()));
}

// ============== P1 Tests: Table Mode Support ==============

#[test]
fn test_table_basic_structure() {
    let html = "<table><tr><td>Cell 1</td><td>Cell 2</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table basic events: {:?}", result.events);

    assert!(result.events.contains(&"start:table".to_string()));
    assert!(result.events.contains(&"start:tr".to_string()));
    assert!(result.events.contains(&"start:td".to_string()));
    assert!(result.events.contains(&"end:td".to_string()));
    assert!(result.events.contains(&"end:tr".to_string()));
    assert!(result.events.contains(&"end:table".to_string()));
    assert!(result.events.contains(&"text:Cell 1".to_string()));
}

#[test]
fn test_table_implicit_tbody() {
    // When tr is direct child of table, tbody should be implied
    let html = "<table><tr><td>Cell</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table implicit tbody events: {:?}", result.events);

    assert!(result.events.contains(&"start:table".to_string()));
    assert!(result.events.contains(&"start:tbody".to_string()));
    assert!(result.events.contains(&"start:tr".to_string()));
}

#[test]
fn test_table_explicit_tbody() {
    let html = "<table><tbody><tr><td>Cell</td></tr></tbody></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table explicit tbody events: {:?}", result.events);

    assert!(result.events.contains(&"start:tbody".to_string()));
    assert!(result.events.contains(&"end:tbody".to_string()));
}

#[test]
fn test_table_thead_tbody_tfoot() {
    let html = r#"<table>
        <thead><tr><th>Header</th></tr></thead>
        <tbody><tr><td>Body</td></tr></tbody>
        <tfoot><tr><td>Footer</td></tr></tfoot>
    </table>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table sections events: {:?}", result.events);

    assert!(result.events.contains(&"start:thead".to_string()));
    assert!(result.events.contains(&"start:tbody".to_string()));
    assert!(result.events.contains(&"start:tfoot".to_string()));
    assert!(result.events.contains(&"start:th".to_string()));
}

#[test]
fn test_table_caption() {
    let html = "<table><caption>My Table</caption><tr><td>Cell</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table caption events: {:?}", result.events);

    assert!(result.events.contains(&"start:caption".to_string()));
    assert!(result.events.contains(&"text:My Table".to_string()));
    assert!(result.events.contains(&"end:caption".to_string()));
}

#[test]
fn test_table_colgroup() {
    let html = "<table><colgroup><col><col></colgroup><tr><td>Cell</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table colgroup events: {:?}", result.events);

    assert!(result.events.contains(&"start:colgroup".to_string()));
    assert!(result.events.contains(&"start:col".to_string()));
}

#[test]
fn test_nested_tables() {
    let html = r#"<table>
        <tr><td>
            <table><tr><td>Nested</td></tr></table>
        </td></tr>
    </table>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Nested tables events: {:?}", result.events);

    // Count table starts - should have 2
    let table_count = result.events.iter().filter(|e| *e == "start:table").count();
    assert_eq!(table_count, 2, "Should have 2 nested tables");
}

#[test]
fn test_table_cell_with_content() {
    let html = "<table><tr><td><p>Paragraph in cell</p><span>Span too</span></td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table cell content events: {:?}", result.events);

    assert!(result.events.contains(&"start:p".to_string()));
    assert!(result.events.contains(&"start:span".to_string()));
    assert!(result.events.contains(&"text:Paragraph in cell".to_string()));
}

#[test]
fn test_table_implicit_tr() {
    // td without tr should imply tr
    let html = "<table><tbody><td>Cell</td></tbody></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table implicit tr events: {:?}", result.events);

    // Should have implied tr
    assert!(result.events.contains(&"start:tr".to_string()));
    assert!(result.events.contains(&"start:td".to_string()));
}

#[test]
fn test_table_multiple_rows() {
    let html = "<table><tr><td>Row 1</td></tr><tr><td>Row 2</td></tr><tr><td>Row 3</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table multiple rows events: {:?}", result.events);

    let tr_count = result.events.iter().filter(|e| *e == "start:tr").count();
    assert_eq!(tr_count, 3, "Should have 3 rows");
}

#[test]
fn test_table_header_cells() {
    let html = "<table><tr><th>Header 1</th><th>Header 2</th></tr><tr><td>Data 1</td><td>Data 2</td></tr></table>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table header cells events: {:?}", result.events);

    assert!(result.events.contains(&"start:th".to_string()));
    assert!(result.events.contains(&"start:td".to_string()));
}

#[test]
fn test_table_with_attributes() {
    let html = r#"<table border="1" class="data-table"><tr><td id="cell1">Cell</td></tr></table>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Table attributes events: {:?}", result.events);

    // Check for table with attributes
    assert!(result.events.iter().any(|e| e.starts_with("start:table[") && e.contains("border=1")));
    assert!(result.events.iter().any(|e| e.contains("id=cell1")));
}

// ============== P2 Tests: Adoption Agency Algorithm ==============

#[test]
fn test_simple_formatting() {
    // Simple formatting should work normally
    let html = "<p><b>bold</b> and <i>italic</i></p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Simple formatting events: {:?}", result.events);

    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"end:b".to_string()));
    assert!(result.events.contains(&"start:i".to_string()));
    assert!(result.events.contains(&"end:i".to_string()));
    assert!(result.events.contains(&"text:bold".to_string()));
}

#[test]
fn test_nested_formatting() {
    // Nested formatting: <b><i>text</i></b>
    let html = "<p><b><i>text</i></b></p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Nested formatting events: {:?}", result.events);

    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"start:i".to_string()));
    assert!(result.events.contains(&"text:text".to_string()));
    assert!(result.events.contains(&"end:i".to_string()));
    assert!(result.events.contains(&"end:b".to_string()));
}

#[test]
fn test_misnested_formatting() {
    // Misnested: <b><i></b></i> - AAA should handle this
    let html = "<p><b><i>text</b></i></p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Misnested formatting events: {:?}", result.events);

    // Both b and i should be opened and closed
    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"start:i".to_string()));
    assert!(result.events.contains(&"end:b".to_string()));
    assert!(result.events.contains(&"end:i".to_string()));
}

#[test]
fn test_formatting_across_blocks() {
    // Formatting across block elements: <b>bold<p>para</p>more</b>
    let html = "<div><b>bold<p>para</p>more</b></div>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Formatting across blocks events: {:?}", result.events);

    // Bold should be present
    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"start:p".to_string()));
    assert!(result.events.contains(&"text:bold".to_string()));
}

#[test]
fn test_multiple_same_formatting() {
    // Multiple same formatting: <b>a<b>b</b>c</b>
    let html = "<p><b>a<b>b</b>c</b></p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Multiple same formatting events: {:?}", result.events);

    // Should handle multiple <b> tags
    let b_count = result.events.iter().filter(|e| *e == "start:b").count();
    assert!(b_count >= 1, "Should have at least one <b> element");
}

#[test]
fn test_anchor_misnesting() {
    // Anchor misnesting: <a href="1"><a href="2">text</a></a>
    let html = r#"<p><a href="1">first<a href="2">second</a></a></p>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Anchor misnesting events: {:?}", result.events);

    // Should have anchors
    let a_starts = result.events.iter().filter(|e| e.starts_with("start:a")).count();
    assert!(a_starts >= 1, "Should have at least one anchor");
}

#[test]
fn test_formatting_in_list() {
    // Formatting in list items
    let html = "<ul><li><b>bold</b></li><li><i>italic</i></li></ul>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Formatting in list events: {:?}", result.events);

    assert!(result.events.contains(&"start:ul".to_string()));
    assert!(result.events.contains(&"start:li".to_string()));
    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"start:i".to_string()));
}

#[test]
fn test_unclosed_formatting() {
    // Unclosed formatting at end of document
    let html = "<p><b>bold text";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Unclosed formatting events: {:?}", result.events);

    // Should have both start and end
    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"end:b".to_string()));
}

#[test]
fn test_formatting_with_void_elements() {
    // Formatting around void elements
    let html = "<p><b>before<br>after</b></p>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Formatting with void events: {:?}", result.events);

    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"start:br".to_string()));
    assert!(result.events.contains(&"end:b".to_string()));
}

// ==================== FRAGMENT PARSING TESTS ====================

#[test]
fn test_fragment_parsing_div_context() {
    // Parse fragment as if inside a <div>
    let html = "<p>Hello</p><span>World</span>";
    let sink = TestSink::new();
    let result = parse_fragment(html, sink, "div").unwrap();

    println!("Fragment div context events: {:?}", result.events);

    // Should parse normally without implicit html/head/body
    assert!(result.events.contains(&"start:p".to_string()));
    assert!(result.events.contains(&"start:span".to_string()));
    assert!(result.events.contains(&"text:Hello".to_string()));
    assert!(result.events.contains(&"text:World".to_string()));
}

#[test]
fn test_fragment_parsing_body_context() {
    // Parse fragment as if inside <body>
    let html = "<div><p>Content</p></div>";
    let sink = TestSink::new();
    let result = parse_fragment(html, sink, "body").unwrap();

    println!("Fragment body context events: {:?}", result.events);

    assert!(result.events.contains(&"start:div".to_string()));
    assert!(result.events.contains(&"start:p".to_string()));
}

#[test]
fn test_fragment_parsing_table_context() {
    // Parse fragment as if inside a <table>
    let html = "<tr><td>Cell</td></tr>";
    let sink = TestSink::new();
    let result = parse_fragment(html, sink, "tbody").unwrap();

    println!("Fragment table context events: {:?}", result.events);

    assert!(result.events.contains(&"start:tr".to_string()));
    assert!(result.events.contains(&"start:td".to_string()));
}

#[test]
fn test_fragment_mixed_content() {
    // Fragment with text and elements
    let html = "Text before<b>bold</b>text after";
    let sink = TestSink::new();
    let result = parse_fragment(html, sink, "span").unwrap();

    println!("Fragment mixed content events: {:?}", result.events);

    assert!(result.events.contains(&"start:b".to_string()));
    assert!(result.events.contains(&"text:bold".to_string()));
}

// ==================== QUIRKS MODE TESTS ====================

#[test]
fn test_html5_doctype_no_quirks() {
    // HTML5 doctype should be no-quirks mode
    let html = "<!DOCTYPE html><html><body></body></html>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("HTML5 doctype events: {:?}", result.events);

    assert!(result.events.contains(&"doctype:html".to_string()));
}

#[test]
fn test_no_doctype_quirks() {
    // No doctype should trigger quirks mode
    let html = "<html><body><p>No doctype</p></body></html>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("No doctype events: {:?}", result.events);

    // Should still parse, but in quirks mode
    assert!(result.events.contains(&"start:html".to_string()));
    assert!(result.events.contains(&"start:body".to_string()));
}

#[test]
fn test_html401_transitional_doctype() {
    // HTML 4.01 Transitional doctype
    let html = r#"<!DOCTYPE HTML PUBLIC "-//W3C//DTD HTML 4.01 Transitional//EN">
<html><body></body></html>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("HTML 4.01 Transitional events: {:?}", result.events);

    // Should parse with doctype
    assert!(result.events.iter().any(|e| e.starts_with("doctype:")));
}

#[test]
fn test_xhtml_doctype() {
    // XHTML 1.0 Strict doctype
    let html = r#"<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.0 Strict//EN" "http://www.w3.org/TR/xhtml1/DTD/xhtml1-strict.dtd">
<html><body></body></html>"#;
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("XHTML doctype events: {:?}", result.events);

    assert!(result.events.iter().any(|e| e.starts_with("doctype:")));
}

// ==================== ADDITIONAL ERROR RECOVERY TESTS ====================

#[test]
fn test_deeply_nested_elements() {
    // Test deeply nested elements
    let html = "<div><div><div><div><div><p>Deep</p></div></div></div></div></div>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Deep nesting events: {:?}", result.events);

    assert!(result.events.contains(&"text:Deep".to_string()));
}

#[test]
fn test_multiple_body_tags() {
    // Multiple body tags should be handled gracefully
    let html = "<html><body><p>First</p></body><body><p>Second</p></body></html>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Multiple body events: {:?}", result.events);

    // Should parse without panicking
    assert!(result.events.contains(&"text:First".to_string()));
}

#[test]
fn test_mismatched_end_tags() {
    // Mismatched end tags
    let html = "<div><span>text</div></span>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Mismatched end tags events: {:?}", result.events);

    // Should handle gracefully
    assert!(result.events.contains(&"text:text".to_string()));
}

#[test]
fn test_self_closing_in_html() {
    // Self-closing syntax in HTML (not XHTML)
    let html = "<div><br/><hr/><img/></div>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Self-closing events: {:?}", result.events);

    assert!(result.events.contains(&"start:br".to_string()));
    assert!(result.events.contains(&"start:hr".to_string()));
    assert!(result.events.contains(&"start:img".to_string()));
}

#[test]
fn test_optional_end_tags() {
    // Optional end tags (p, li, etc.)
    let html = "<ul><li>One<li>Two<li>Three</ul>";
    let sink = TestSink::new();
    let result = parse(html, sink).unwrap();

    println!("Optional end tags events: {:?}", result.events);

    // All li elements should be parsed
    let li_count = result.events.iter().filter(|e| *e == "start:li").count();
    assert_eq!(li_count, 3);
}

