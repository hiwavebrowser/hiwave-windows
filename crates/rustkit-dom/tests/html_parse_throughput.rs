use rustkit_dom::Document;
use std::{hint::black_box, time::Instant};

fn time_document(html: &str) {
    let mut samples = Vec::new();
    for _ in 0..15 {
        let start = Instant::now();
        let doc = Document::parse_html_scripted(black_box(html), true).unwrap();
        samples.push(start.elapsed());
        black_box(doc);
    }
    samples.sort_unstable();
    eprintln!("{} bytes, median parse {:?}", html.len(), samples[7]);
}

#[test]
#[ignore = "run in release mode to measure document parsing"]
fn text_heavy_document() {
    let text = "A short sentence about the browser. ".repeat(16_384);
    time_document(&format!("<article>{text}</article>"));
}

#[test]
#[ignore = "run in release mode to measure document parsing"]
fn markup_heavy_document() {
    time_document(&"<p class='item'><b>Title</b> and some text.</p>".repeat(4096));
}
