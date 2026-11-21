use rustkit_cssparser::parse_stylesheet;

#[test]
#[ignore = "release-mode stylesheet parse timing"]
fn parse_declaration_lists() {
    let mut css = String::new();
    for i in 0..4096 {
        css.push_str(&format!(
            ".item-{i} {{ color: red; padding: 4px; display: block; width: 75%; \
            margin: auto; font-family: sans-serif; border: 1px solid; opacity: 0.8; }}\n"
        ));
    }
    let mut samples = Vec::new();
    for _ in 0..21 {
        let start = std::time::Instant::now();
        let sheet = parse_stylesheet(std::hint::black_box(&css)).unwrap();
        samples.push(start.elapsed());
        assert_eq!(sheet.rules.len(), 4096);
        assert_eq!(sheet.rules.last().unwrap().declarations.len(), 8);
        std::hint::black_box(sheet);
    }
    samples.sort_unstable();
    eprintln!("4096 rules, median {:?}", samples[samples.len() / 2]);
}
