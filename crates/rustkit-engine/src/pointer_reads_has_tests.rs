//! Portable pins for LAT3 / #943: `PointerReads` files `:hover` inside
//! `:has()` and complex `:not()`, and marks relative `+`/`~` `:has()`
//! arguments as `anywhere`.
//!
//! The page oracle in `hover_restyle_tests` is headless-only, so default
//! Linux `--lib` never exercised `note_has` / `not_dropped`. These call
//! `PointerReads::note` directly.

use super::{PointerReads, HOVER_MARK};

fn hover_reads(selector: &str) -> PointerReads {
    let mut reads = PointerReads::default();
    reads.note(selector, ":hover", HOVER_MARK);
    reads
}

#[test]
fn a_hover_inside_has_is_filed_on_the_subject() {
    // `.post:has(.btn:hover)`: the post above a hovered button is the
    // subject; the button compound is filed under `has`, not `anywhere`.
    let reads = hover_reads(".post:has(.btn:hover)");
    assert_eq!(reads.has.len(), 1, "one :has() argument reads :hover");
    assert_eq!(reads.has[0].inner.len(), 1, "the argument's .btn compound");
    assert!(
        reads.has[0]
            .subject
            .classes
            .iter()
            .any(|c| c == "post"),
        "subject keeps .post: {:?}",
        reads.has[0].subject.classes
    );
    assert!(!reads.anywhere);
}

#[test]
fn a_hover_inside_complex_not_is_filed_from_not_dropped() {
    // Marketplace LAT3 shape: `:hover` sits in a complex `:not(:has(...))`
    // member the compound parser leaves in `not_dropped`, then is filed
    // from there so a hover change does not force a full restyle.
    let reads = hover_reads(
        "[data-dbg]:not(:has([data-dbg]:hover)):hover::after",
    );
    assert!(
        !reads.has.is_empty() || !reads.compounds.is_empty(),
        "complex :not/:has hover must be indexed (has={}, compounds={})",
        reads.has.len(),
        reads.compounds.len()
    );
    assert!(
        reads.has.iter().any(|h| !h.inner.is_empty())
            || reads.compounds.iter().any(|c| {
                c.compound.attrs.iter().any(|a| a.contains("data-dbg"))
                    || c.compound.classes.iter().any(|cl| cl == "data-dbg")
            }),
        "data-dbg subject or :has() inner must be present"
    );
}

#[test]
fn a_relative_has_starting_with_sibling_combinator_sets_anywhere() {
    // `:has(+ .b:hover)` names a subject beside the element's ancestors;
    // note_has skips filing and `note` marks `anywhere`.
    let reads = hover_reads(".a:has(+ .b:hover)");
    assert!(reads.has.is_empty(), "relative + is not filed under has");
    assert!(
        reads.anywhere,
        "unfiled :hover inside relative :has() is anywhere"
    );
}
