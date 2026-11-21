//! Portable pins for LAT20 / #1062: quoted and escaped `:hover` text is not
//! a pointer read, and an unplaced read names its shape for `restyle_miss`.
//!
//! The page oracle in `hover_restyle_tests` is headless-only, so default
//! Linux `--lib` never exercised `pseudo_named` / `anywhere_shape`. These
//! call the pure helpers and `PointerReads::note` directly. Escaped class
//! names are passed through [`encode_selector_escapes`] first, as
//! `Stylesheet::parse` does before the rule index calls `note`.

use rustkit_css::encode_selector_escapes;

use super::{anywhere_shape, pseudo_named, PointerReads, HOVER_MARK};

fn hover_reads(selector: &str) -> PointerReads {
    let encoded = encode_selector_escapes(selector);
    let mut reads = PointerReads::default();
    reads.note(encoded.as_ref(), ":hover", HOVER_MARK);
    reads
}

#[test]
fn quoted_and_escaped_hover_text_is_not_named() {
    assert_eq!(pseudo_named(r#"[title=":hover"]"#, ":hover"), 0);
    assert_eq!(pseudo_named("[data-k='a:hover b']", ":hover"), 0);
    assert_eq!(pseudo_named(r".md\:hover\:underline", ":hover"), 0);
    assert_eq!(pseudo_named(r".group-hover\:text-blue", ":hover"), 0);
    // A real pseudo-class still counts, beside quoted/escaped noise.
    assert_eq!(
        pseudo_named(r#"[title=":hover"] a:hover"#, ":hover"),
        1
    );
    assert_eq!(
        pseudo_named(r".md\:hover\:underline:hover", ":hover"),
        1
    );
    // After Stylesheet::parse, escaped colons are stand-ins — only the
    // real `:hover` remains as ASCII colon+name.
    let encoded = encode_selector_escapes(r".md\:hover\:underline:hover");
    assert_eq!(encoded.matches(':').count(), 1);
    assert_eq!(pseudo_named(encoded.as_ref(), ":hover"), 1);
    // Longer names that start with the mark are not counted.
    assert_eq!(pseudo_named(":focus-within", ":focus"), 0);
    assert_eq!(pseudo_named(":hovered", ":hover"), 0);
    assert_eq!(pseudo_named("a:focus", ":focus"), 1);
}

#[test]
fn anywhere_shape_names_the_form_never_the_text() {
    assert_eq!(anywhere_shape(r#"[title=":hover"]"#), "quoted");
    assert_eq!(anywhere_shape("[data-k='a:hover b']"), "quoted");
    assert_eq!(anywhere_shape(r".md\:hover\:underline"), "escaped");
    assert_eq!(anywhere_shape(".post:has(~ .post:hover)"), "has");
    assert_eq!(anywhere_shape(":host(:hover)"), "shadow");
    assert_eq!(anywhere_shape("::slotted(:hover)"), "shadow");
    assert_eq!(anywhere_shape(":nth-child(2n of :hover)"), "nth_of");
    assert_eq!(anywhere_shape(":where(:hover)"), "where");
    assert_eq!(anywhere_shape(".weird"), "unplaced");
}

#[test]
fn quoted_or_escaped_hover_alone_does_not_index_anywhere() {
    // Text-only "hover" never enters note's placement loop.
    let quoted = hover_reads(r#"[title=":hover"]"#);
    assert!(!quoted.anywhere);
    assert!(quoted.anywhere_shape.is_none());
    assert!(quoted.compounds.is_empty());
    assert!(quoted.has.is_empty());

    let escaped = hover_reads(r".md\:hover\:underline");
    assert!(!escaped.anywhere);
    assert!(escaped.anywhere_shape.is_none());
    assert!(escaped.compounds.is_empty());
}

#[test]
fn a_real_hover_beside_quoted_noise_stays_on_the_short_path() {
    let reads = hover_reads(r#"[title=":hover"] a:hover, [data-k='a:hover b']"#);
    assert!(
        !reads.anywhere,
        "quoted :hover must not force anywhere; shape={:?}",
        reads.anywhere_shape
    );
    assert!(
        !reads.compounds.is_empty() || !reads.has.is_empty(),
        "the real a:hover must still be indexed"
    );
}

#[test]
fn an_escaped_class_with_a_real_hover_stays_on_the_short_path() {
    let reads = hover_reads(
        r".md\:hover\:underline:hover, .group:hover .group-hover\:text-blue",
    );
    assert!(
        !reads.anywhere,
        "escaped class text must not force anywhere; shape={:?}",
        reads.anywhere_shape
    );
    assert!(
        !reads.compounds.is_empty(),
        "the real :hover compounds must be indexed"
    );
}

#[test]
fn an_unplaced_relative_has_names_has_in_anywhere_shape() {
    let reads = hover_reads(".post:has(~ .post:hover)");
    assert!(reads.anywhere, "relative ~ :has() is unplaced");
    assert_eq!(reads.anywhere_shape, Some("has"));
}
