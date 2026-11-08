//! Portable S6 (#877) document-order slot insert for late stylesheets.
//!
//! The headless companion waits on a real late fetch. These pin the
//! `partition_point` insert that keeps cascade order when a middle sheet
//! arrives after the load's budget — the bug that left microsoft-scale
//! pages unstyled when a slow sheet sat between two fast ones.

use super::insert_sheet_at_document_order;

#[test]
fn a_middle_late_sheet_lands_between_its_neighbours() {
    let mut slots = vec![0usize, 2];
    let mut sheets = vec!["fast-0", "fast-2"];
    insert_sheet_at_document_order(&mut slots, &mut sheets, 1, "late-1");
    assert_eq!(slots, vec![0, 1, 2]);
    assert_eq!(sheets, vec!["fast-0", "late-1", "fast-2"]);
}

#[test]
fn a_late_sheet_before_or_after_every_arrived_slot() {
    let mut slots = vec![1usize, 2];
    let mut sheets = vec!["a", "b"];
    insert_sheet_at_document_order(&mut slots, &mut sheets, 0, "first");
    assert_eq!(slots, vec![0, 1, 2]);
    assert_eq!(sheets, vec!["first", "a", "b"]);

    insert_sheet_at_document_order(&mut slots, &mut sheets, 5, "last");
    assert_eq!(slots, vec![0, 1, 2, 5]);
    assert_eq!(sheets, vec!["first", "a", "b", "last"]);
}

#[test]
fn an_empty_slot_list_takes_the_first_late_sheet() {
    let mut slots = Vec::<usize>::new();
    let mut sheets = Vec::<&str>::new();
    insert_sheet_at_document_order(&mut slots, &mut sheets, 3, "only");
    assert_eq!(slots, vec![3]);
    assert_eq!(sheets, vec!["only"]);
}
