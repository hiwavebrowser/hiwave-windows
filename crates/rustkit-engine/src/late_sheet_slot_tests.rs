//! Portable S6 (#877) / CH2.E20 (#984) document-order assembly of the
//! external stylesheets.
//!
//! The headless companions wait on real fetches. These pin
//! `sheets_in_link_order`, which keeps cascade order when a middle sheet
//! arrives after the load's budget (the bug that left microsoft-scale pages
//! unstyled when a slow sheet sat between two fast ones), gives a link its
//! sheet wherever it stands, makes the same URL in two links two sheets,
//! and `sheet_under_media`, which puts a link's `media` list on its rules.

use super::{sheet_under_media, sheets_in_link_order, SheetLink};
use rustkit_css::Stylesheet;
use rustkit_dom::NodeId;
use std::collections::HashMap;
use url::Url;

fn link(id: usize, path: &str) -> SheetLink {
    SheetLink { owner: NodeId::new(id), url: Url::parse(&format!("http://sheets.test{path}")).unwrap() }
}

fn fetched(entries: &[(&str, Option<&'static str>)]) -> HashMap<Url, Option<&'static str>> {
    entries
        .iter()
        .map(|(path, sheet)| (Url::parse(&format!("http://sheets.test{path}")).unwrap(), *sheet))
        .collect()
}

fn sheets(got: &[(NodeId, &'static str)]) -> Vec<&'static str> {
    got.iter().map(|(_, sheet)| *sheet).collect()
}

#[test]
fn a_middle_late_sheet_lands_between_its_neighbours() {
    let links = [link(1, "/0.css"), link(2, "/1.css"), link(3, "/2.css")];
    let early = fetched(&[("/0.css", Some("fast-0")), ("/2.css", Some("fast-2"))]);
    assert_eq!(sheets(&sheets_in_link_order(&links, &early)), vec!["fast-0", "fast-2"]);
    let mut all = early;
    all.insert(Url::parse("http://sheets.test/1.css").unwrap(), Some("late-1"));
    assert_eq!(sheets(&sheets_in_link_order(&links, &all)), vec!["fast-0", "late-1", "fast-2"]);
}

#[test]
fn a_link_cascades_where_it_stands_not_where_it_was_found() {
    // The load found /b.css; a script then put a link to /a.css before it.
    let links = [link(9, "/a.css"), link(1, "/b.css")];
    let got = sheets_in_link_order(&links, &fetched(&[("/b.css", Some("b")), ("/a.css", Some("a"))]));
    assert_eq!(sheets(&got), vec!["a", "b"]);
    assert_eq!(got.iter().map(|(owner, _)| *owner).collect::<Vec<_>>(), vec![NodeId::new(9), NodeId::new(1)]);
}

#[test]
fn the_same_url_in_two_links_is_two_sheets_and_a_failed_fetch_none() {
    let links = [link(1, "/a.css"), link(2, "/bad.css"), link(3, "/a.css")];
    let got = sheets_in_link_order(&links, &fetched(&[("/a.css", Some("a")), ("/bad.css", None)]));
    assert_eq!(sheets(&got), vec!["a", "a"]);
}

#[test]
fn a_link_whose_sheet_has_not_landed_and_no_links_at_all() {
    let links = [link(1, "/slow.css")];
    assert!(sheets_in_link_order(&links, &fetched(&[])).is_empty());
    assert!(sheets_in_link_order(&[], &fetched(&[("/a.css", Some("a"))])).is_empty());
}

#[test]
fn a_links_media_list_becomes_its_rules_outermost_condition() {
    let sheet = Stylesheet::parse("a { color: red } @media (min-width: 500px) { b { color: blue } }").expect("css");
    let same = sheet_under_media(&sheet, "");
    assert_eq!(same.rules.iter().map(|r| r.media.clone()).collect::<Vec<_>>(), sheet.rules.iter().map(|r| r.media.clone()).collect::<Vec<_>>());
    let print = sheet_under_media(&sheet, "print");
    assert_eq!(print.rules.len(), sheet.rules.len());
    assert!(print.rules.iter().all(|r| r.media.first().map(String::as_str) == Some("print")), "{:?}", print.rules);
    // The inner @media condition stays, after the link's.
    let inner = print.rules.iter().find(|r| r.selector.trim() == "b").expect("rule b");
    assert_eq!(inner.media.len(), 2);
    assert!(!inner.applies_at(400.0, 300.0));
    assert!(!inner.applies_at(800.0, 300.0), "print applies nowhere on screen");
    let screen = sheet_under_media(&sheet, "screen");
    let inner = screen.rules.iter().find(|r| r.selector.trim() == "b").expect("rule b");
    assert!(inner.applies_at(800.0, 300.0));
    assert!(!inner.applies_at(400.0, 300.0));
}
