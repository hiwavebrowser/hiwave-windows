//! DOM1 cost check, not a correctness test: how long the parser takes to
//! build a document and how long attribute lookups take, for the attribute
//! store. Run by hand, in release, before and after a change to the store:
//!
//! `cargo test -p rustkit-dom --release --lib attr_cost -- --ignored --nocapture`
//!
//! Two pages: the browser's own chrome page (the largest HTML file in the
//! repository), and a generated one of 20,000 elements with six attributes
//! each, which is the shape a list-heavy page has.

use crate::{Document, NodeType};

fn median_ms(mut runs: Vec<f64>) -> f64 {
    runs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    runs[runs.len() / 2]
}

fn measure(label: &str, html: &str) {
    let mut parse = Vec::new();
    let mut doc = None;
    for _ in 0..15 {
        let started = std::time::Instant::now();
        doc = Some(Document::parse_html(html).expect("parse"));
        parse.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let doc = doc.unwrap();
    let mut lookup = Vec::new();
    let (mut elements, mut attributes, mut found) = (0usize, 0usize, 0usize);
    for run in 0..15 {
        let started = std::time::Instant::now();
        doc.traverse(|node| {
            if let NodeType::Element { attributes: attrs, .. } = &node.node_type {
                if run == 0 {
                    elements += 1;
                    attributes += attrs.len();
                }
                // What the cascade and the bindings ask for: hits and misses.
                for name in ["class", "id", "style", "href", "data-row", "aria-hidden", "title", "hidden"] {
                    if node.get_attribute(name).is_some() {
                        found += 1;
                    }
                }
            }
        });
        lookup.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "{label}: {elements} elements, {attributes} attributes; parse {:.2} ms; 8 lookups per element {:.3} ms ({found} hits over 15 runs)",
        median_ms(parse),
        median_ms(lookup)
    );
}

#[test]
#[ignore = "a timing, run by hand in release"]
fn attr_cost() {
    measure("chrome.html", include_str!("../../hiwave-app/src/ui/chrome.html"));
    let mut page = String::from("<!doctype html><html><body><ul>");
    for i in 0..20_000 {
        page.push_str(&format!(
            "<li id=\"row{i}\" class=\"row item-{}\" data-row=\"{i}\" title=\"Row {i}\" style=\"color: red\" aria-hidden=\"false\">r</li>",
            i % 7
        ));
    }
    page.push_str("</ul></body></html>");
    measure("generated 20k x 6", &page);
}
