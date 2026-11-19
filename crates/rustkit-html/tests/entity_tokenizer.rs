use rustkit_html::tokenizer::{tokenize, Token};

// Columns from Chrome for Testing 148.0.7778.216: source, text/RCDATA,
// and the value of a quoted or unquoted attribute.
#[test]
fn chrome_character_reference_cases() {
    for row in include_str!("character_references.tsv").lines() {
        let mut fields = row.split('\t');
        let source = fields.next().unwrap();
        let text = fields.next().unwrap();
        let attribute = fields.next().unwrap();
        assert_eq!(rustkit_html::entities::decode(source), text, "{source}");
        let html = format!(
            "<p a='{source}' b=\"{source}\" c={source}>{source}</p><textarea>{source}</textarea>"
        );
        let tokens = tokenize(&html).unwrap();
        let Token::StartTag { attrs, .. } = &tokens[0] else {
            panic!("expected p")
        };
        for name in ["a", "b", "c"] {
            assert_eq!(
                attrs.get(name).map(String::as_str),
                Some(attribute),
                "{source}"
            );
        }
        let actual: String = tokens
            .iter()
            .filter_map(|token| match token {
                Token::Character(c) => Some(*c),
                _ => None,
            })
            .collect();
        assert_eq!(actual, text.repeat(2), "{source}");
    }
}

#[test]
fn references_in_text_rcdata_and_attributes() {
    let source = "&amp;&#128;&#x1F600;&unknown;&é;&;&#x;&unfinished";
    let expected = "&€😀&unknown;&é;&;&#x;&unfinished";
    let html = format!(
        "<p a='{source}' b=\"{source}\" c={source}>{source}</p><textarea>{source}</textarea>"
    );
    let tokens = tokenize(&html).unwrap();
    let Token::StartTag { attrs, .. } = &tokens[0] else {
        panic!("expected a start tag");
    };
    for name in ["a", "b", "c"] {
        assert_eq!(attrs.get(name).map(String::as_str), Some(expected));
    }
    let text: String = tokens
        .iter()
        .filter_map(|token| match token {
            Token::Character(c) => Some(*c),
            _ => None,
        })
        .collect();
    assert_eq!(text, expected.repeat(2));
}

#[test]
#[ignore = "release-mode tokenizer timing"]
fn entity_tokenizer_timing() {
    for (label, value) in [("plain", "abcdef"), ("entities", "&amp;&#65;&#x41;&lt;")] {
        let html = format!("<p title='{value}'>{value}</p>").repeat(2048);
        let mut samples = Vec::new();
        for _ in 0..31 {
            let start = std::time::Instant::now();
            std::hint::black_box(tokenize(std::hint::black_box(&html)).unwrap());
            samples.push(start.elapsed());
        }
        samples.sort_unstable();
        eprintln!(
            "{label}: median {:?} per document",
            samples[samples.len() / 2]
        );
    }
}
