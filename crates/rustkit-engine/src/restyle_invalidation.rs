//! Incremental restyle, step 1: record and classify
//! (`docs/design/INCREMENTAL_RESTYLE.md` §2, §3.1, §5 PR 1).
//!
//! - [`InvalidationIndex`]: for each class, id, attribute name and
//!   pseudo-class the sheets' selectors name, where it appears (subject,
//!   ancestor compound, left of a sibling combinator, or somewhere this
//!   cannot place), and whether its rule reads position among siblings.
//!   Built with the rule index (`Engine::build_rule_index`).
//! - [`DirtySet`]: the bindings' per-node [`MutationLog`] run through the
//!   index (the note's §2 table): which nodes the writes could restyle, and
//!   whether only a full restyle would do.
//!
//! Nothing decides on either yet. The engine logs the set on the
//! `relayout cause=...` line and keeps it on the view, and relayout still
//! rebuilds the whole page. Being conservative here costs time later,
//! never correctness: anything the index cannot place is `ANYWHERE`.

use super::SelectorMatcher;
use rustkit_bindings::{MutationKind, MutationLog};
use rustkit_css::css_ident;
use rustkit_dom::{Document, Node, NodeId};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Where a token appears, as [`InvalidationIndex`] flags.
pub(crate) const SUBJECT: u8 = 1;
/// In a compound left of a descendant or child combinator.
pub(crate) const ANCESTOR: u8 = 2;
/// In a compound left of a `+` or `~` combinator.
pub(crate) const SIBLING: u8 = 4;
/// Somewhere the index cannot place (`:has()`, a combinator inside
/// `:is()`/`:not()`, a shadow pseudo, an unparsed selector): any element
/// may change.
pub(crate) const ANYWHERE: u8 = 8;
/// Some rule naming the token also reads position among siblings.
pub(crate) const POSITIONAL: u8 = 16;
/// In a `:has()` argument the index can scope (LAT6): a change to the token
/// can change the `:has()` subjects above the element
/// ([`InvalidationIndex::has_subjects`]).
pub(crate) const IN_HAS: u8 = 32;

/// Pseudo-classes whose selector argument is a compound list matched
/// against the element itself.
const SAME_ELEMENT_LISTS: [&str; 6] = ["not", "is", "where", "matches", "-webkit-any", "-moz-any"];

/// Pseudo-classes decided by an element's place among its siblings, or by
/// its children.
const STRUCTURAL: [&str; 11] = [
    "nth-child",
    "nth-last-child",
    "nth-of-type",
    "nth-last-of-type",
    "first-child",
    "last-child",
    "only-child",
    "first-of-type",
    "last-of-type",
    "only-of-type",
    "empty",
];

/// Pseudo-classes no attribute write can flip. Every other one may read an
/// attribute (`:checked`, `:disabled`, `:link`, `:required`, `:lang()`,
/// ...), so an attribute write consults them all.
const NOT_ATTRIBUTE_STATE: [&str; 9] =
    ["hover", "active", "focus", "focus-within", "focus-visible", "root", "scope", "before", "after"];

/// Where the sheets' selectors name each class, id, attribute and
/// pseudo-class (see the module docs).
#[derive(Default, Debug)]
pub(crate) struct InvalidationIndex {
    pub(crate) classes: HashMap<String, u8>,
    pub(crate) ids: HashMap<String, u8>,
    /// Lowercased attribute names.
    pub(crate) attributes: HashMap<String, u8>,
    /// Lowercased pseudo-class names, without the colon.
    pub(crate) pseudos: HashMap<String, u8>,
    /// Some rule reads position among siblings: a structural pseudo-class
    /// or a `+`/`~` combinator. An insert or a remove can then restyle the
    /// parent's other children.
    pub(crate) positional: bool,
    /// Some rule uses `:has()`: a structure change can restyle ancestors.
    pub(crate) has: bool,
    /// The compounds that carry a `:has()`, as tag, classes, id and
    /// attribute selectors only (every pseudo-class dropped: a wider
    /// match), each with where it sits (`SUBJECT`, `ANCESTOR`, `SIBLING`).
    /// A write under one of their elements, or to a token an argument
    /// names, restyles that element as its place says (LAT6).
    pub(crate) has_subjects: Vec<(String, u8)>,
    /// Some `:has()` these cannot scope: its argument starts with `+` or
    /// `~` (the subject is beside an ancestor), or it sits where the index
    /// places nothing. Every write `has` covers is then full.
    pub(crate) has_anywhere: bool,
}

impl InvalidationIndex {
    /// Record one rule's selector (a list).
    pub(crate) fn note_selector(&mut self, selector: &str) {
        for member in SelectorMatcher::split_top_level_commas(selector) {
            self.note_complex(member.trim(), None, false);
        }
    }

    /// One complex selector. `forced` places every token in it (`ANYWHERE`
    /// inside `:has()`); `outer_positional` is the flag of the selector it
    /// is a member of.
    fn note_complex(&mut self, member: &str, forced: Option<u8>, outer_positional: bool) {
        let tokens = SelectorMatcher.tokenize_selector(member);
        let bare = unquoted(member);
        let count = |c: char| bare.matches(c).count();
        let balanced = count('(') == count(')') && count('[') == count(']');
        let well_formed = tokens.last().is_some_and(|(_, combinator)| combinator.is_empty());
        if !balanced || !well_formed || member.contains('&') {
            // Unparsed, or nesting the sheet parser left in: place nothing.
            self.note_compound(member, ANYWHERE, false);
            return;
        }
        let positional = outer_positional
            || tokens.iter().any(|(_, c)| c == "+" || c == "~")
            || STRUCTURAL.iter().any(|name| member.contains(&format!(":{name}")));
        self.positional |= positional;
        for (compound, combinator) in &tokens {
            let at = forced.unwrap_or(match combinator.as_str() {
                "" => SUBJECT,
                " " | ">" => ANCESTOR,
                "+" | "~" => SIBLING,
                _ => ANYWHERE,
            });
            if compound.to_ascii_lowercase().contains(":has(") {
                match forced.is_none() && at != ANYWHERE {
                    true => self.has_subjects.push((plain_compound(compound), at)),
                    false => self.has_anywhere = true,
                }
            }
            self.note_compound(compound, at, positional);
        }
    }

    /// File every class, id, attribute and pseudo-class of `compound` at
    /// `at`, looking into functional pseudo-class arguments.
    fn note_compound(&mut self, compound: &str, at: u8, positional: bool) {
        let flags = at | if positional { POSITIONAL } else { 0 };
        let chars: Vec<char> = compound.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            match chars[i] {
                '.' | '#' => {
                    let start = i + 1;
                    i = ident_end(&chars, start);
                    let name: String = chars[start..i].iter().collect();
                    if name.is_empty() {
                        continue;
                    }
                    let map = if chars[start - 1] == '.' { &mut self.classes } else { &mut self.ids };
                    *map.entry(css_ident(&name).into_owned()).or_default() |= flags;
                }
                '[' => {
                    let start = i + 1;
                    let end = block_end(&chars, i, '[', ']');
                    let inner: String = chars[start..end.min(chars.len())].iter().collect();
                    let name =
                        inner.split(['=', '~', '^', '$', '*', ']']).next().unwrap_or("").trim().trim_end_matches('|');
                    let name = name.rsplit('|').next().unwrap_or(name);
                    if !name.is_empty() {
                        *self.attributes.entry(css_ident(name).to_ascii_lowercase()).or_default() |= flags;
                    }
                    i = end + 1;
                }
                ':' => {
                    let element = chars.get(i + 1) == Some(&':');
                    let start = i + if element { 2 } else { 1 };
                    i = ident_end(&chars, start);
                    let name = chars[start..i].iter().collect::<String>().to_ascii_lowercase();
                    let argument = (chars.get(i) == Some(&'(')).then(|| {
                        let end = block_end(&chars, i, '(', ')');
                        let argument: String = chars[i + 1..end.min(chars.len())].iter().collect();
                        i = end + 1;
                        argument
                    });
                    if element {
                        // `::slotted(x)`, `::part(x)`, `::cue(x)`: shadow
                        // and media trees the index does not see.
                        if let Some(argument) = argument {
                            self.note_compound(&argument, ANYWHERE, positional);
                        }
                        continue;
                    }
                    match (name.as_str(), argument) {
                        (list, Some(argument)) if SAME_ELEMENT_LISTS.contains(&list) => {
                            for member in SelectorMatcher::split_top_level_commas(&argument) {
                                let member = member.trim();
                                let simple = SelectorMatcher.tokenize_selector(member).len() <= 1;
                                match (simple, at) {
                                    (true, _) => self.note_compound(member, at, positional),
                                    // A complex member of the subject's own
                                    // list has the subject as its subject:
                                    // its tokens place as a rule's would
                                    // (`.dark` of `.x:is(.dark *)` is an
                                    // ancestor). Left of a combinator, or
                                    // forced, its tokens place nowhere.
                                    (false, SUBJECT) => self.note_complex(member, None, positional),
                                    (false, _) => self.note_complex(member, Some(ANYWHERE), positional),
                                }
                            }
                        }
                        ("has", argument) => {
                            self.has = true;
                            for member in SelectorMatcher::split_top_level_commas(&argument.unwrap_or_default()) {
                                let member = member.trim();
                                // `:has(+ .x)` / `:has(~ .x)`: the subject is an
                                // earlier sibling, not an ancestor.
                                let place = match member.starts_with(['+', '~']) {
                                    true => {
                                        self.has_anywhere = true;
                                        ANYWHERE
                                    }
                                    false => IN_HAS,
                                };
                                let member = member.trim_start_matches(['>', '+', '~']).trim();
                                self.note_complex(member, Some(place), false);
                            }
                        }
                        ("nth-child" | "nth-last-child", Some(argument)) => {
                            *self.pseudos.entry(name).or_default() |= flags;
                            if let Some((_, of)) = argument.split_once(" of ") {
                                self.note_selector_anywhere(of);
                            }
                        }
                        ("host" | "host-context", Some(argument)) => {
                            *self.pseudos.entry(name).or_default() |= ANYWHERE;
                            self.note_selector_anywhere(&argument);
                        }
                        // `:lang(en)`, `:dir(rtl)`, `:nth-of-type(2n)`: the
                        // argument is no selector.
                        _ => *self.pseudos.entry(name).or_default() |= flags,
                    }
                }
                _ => i += 1,
            }
        }
    }

    fn note_selector_anywhere(&mut self, list: &str) {
        for member in SelectorMatcher::split_top_level_commas(list) {
            self.note_complex(member.trim(), Some(ANYWHERE), false);
        }
    }

    /// Where the pseudo-classes an attribute write can flip are read: the
    /// union over every pseudo-class outside [`NOT_ATTRIBUTE_STATE`] and
    /// the structural ones.
    fn attribute_state_flags(&self) -> u8 {
        self.pseudos
            .iter()
            .filter(|(name, _)| !NOT_ATTRIBUTE_STATE.contains(&name.as_str()) && !STRUCTURAL.contains(&name.as_str()))
            .fold(0, |acc, (_, flags)| acc | flags)
    }
}

/// `compound` with every pseudo-class and pseudo-element (and its
/// argument) dropped: the tag, classes, id and attribute selectors, which
/// an element must carry for the compound to match it.
fn plain_compound(compound: &str) -> String {
    let chars: Vec<char> = compound.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            ':' => {
                let start = if chars.get(i + 1) == Some(&':') { i + 2 } else { i + 1 };
                i = ident_end(&chars, start);
                if chars.get(i) == Some(&'(') {
                    i = block_end(&chars, i, '(', ')') + 1;
                }
            }
            '[' => {
                let end = block_end(&chars, i, '[', ']').min(chars.len() - 1);
                out.extend(&chars[i..=end]);
                i = end + 1;
            }
            '\\' => {
                out.extend(chars[i..(i + 2).min(chars.len())].iter());
                i += 2;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// `text` without its quoted strings, for counting brackets.
fn unquoted(text: &str) -> String {
    let mut quote = None;
    text.chars()
        .filter(|&c| match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                false
            }
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                false
            }
            None => true,
        })
        .collect()
}

/// The end of the identifier starting at `start` (escapes included).
fn ident_end(chars: &[char], start: usize) -> usize {
    let mut i = start;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii() => i += 1,
            _ => break,
        }
    }
    i.min(chars.len())
}

/// The index of the `close` matching the `open` at `at` (quotes and
/// nesting respected), or the end.
fn block_end(chars: &[char], at: usize, open: char, close: char) -> usize {
    let mut depth = 0usize;
    let mut quote = None;
    let mut i = at;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) if c == '\\' => i += 1,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '\\' => i += 1,
            None if c == open => depth += 1,
            None if c == close => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            None => {}
        }
        i += 1;
    }
    chars.len()
}

/// Recompute this element's cascade.
pub(crate) const STYLE: u8 = 1;
/// Recompute every descendant's cascade.
pub(crate) const SUBTREE_STYLE: u8 = 2;
/// Rebuild the element's boxes (its children changed, or an attribute that
/// decides box generation).
pub(crate) const BOX: u8 = 4;
/// Geometry may change.
pub(crate) const LAYOUT: u8 = 8;
/// A candidate for paint-only: decided after the restyle.
pub(crate) const PAINT: u8 = 16;

/// What one flush of script DOM writes could invalidate, by raw NodeId.
/// Only connected nodes are kept.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub(crate) struct DirtySet {
    pub(crate) nodes: HashMap<usize, u8>,
    /// Why only a full restyle covers the writes, if so.
    pub(crate) full: Option<&'static str>,
}

/// Past this many dirty nodes the set stops growing and is full.
const MAX_DIRTY_NODES: usize = 1 << 16;

impl DirtySet {
    /// A set that covers everything, for `reason`.
    pub(crate) fn full(reason: &'static str) -> Self {
        DirtySet { nodes: HashMap::new(), full: Some(reason) }
    }

    /// `full` or `subtree`: the path an incremental restyle would take.
    pub(crate) fn would_path(&self) -> &'static str {
        if self.full.is_some() {
            "full"
        } else {
            "subtree"
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn bits(&self, node: usize) -> u8 {
        self.nodes.get(&node).copied().unwrap_or(0)
    }

    /// Whether a restyle of this set would recompute `node`'s style: full,
    /// `STYLE` on it, or `SUBTREE_STYLE` on an ancestor.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn covers_style(&self, node: &Rc<Node>) -> bool {
        if self.full.is_some() || self.bits(node.id.raw()) & STYLE != 0 {
            return true;
        }
        let mut at = node.parent();
        while let Some(parent) = at {
            if self.bits(parent.id.raw()) & SUBTREE_STYLE != 0 {
                return true;
            }
            at = parent.parent();
        }
        false
    }

    fn set_full(&mut self, reason: &'static str) {
        self.full.get_or_insert(reason);
    }

    fn mark(&mut self, node: &Rc<Node>, bits: u8) {
        if self.full.is_some() {
            return;
        }
        if bits & (STYLE | SUBTREE_STYLE) != 0 && matches!(node.tag_name(), Some("html" | "body")) {
            // Root and body changes take the full path (note §3.3).
            self.set_full("root_or_body");
            return;
        }
        if self.nodes.len() >= MAX_DIRTY_NODES {
            self.set_full("too_many_nodes");
            return;
        }
        *self.nodes.entry(node.id.raw()).or_default() |= bits;
    }

    /// Restyle what `flags` (an [`InvalidationIndex`] entry) says a change
    /// to a token on `node` can reach.
    fn apply(&mut self, index: &InvalidationIndex, node: &Rc<Node>, flags: u8) {
        if flags & ANYWHERE != 0 {
            self.set_full("anywhere");
        }
        if flags & IN_HAS != 0 {
            self.has_subjects_from(index, node.parent());
        }
        if flags & SUBJECT != 0 {
            self.mark(node, STYLE);
        }
        if flags & ANCESTOR != 0 {
            self.mark(node, SUBTREE_STYLE);
        }
        if flags & SIBLING != 0 {
            let mut next = node.next_sibling();
            while let Some(sibling) = next {
                if sibling.is_element() {
                    self.mark(&sibling, STYLE | SUBTREE_STYLE);
                }
                next = sibling.next_sibling();
            }
        }
    }

    /// The `:has()` subjects at `from` and above it, restyled as their
    /// places say: what a change under them (or to a token an argument
    /// names) can change (LAT6). Root and body are the full path (`mark`).
    fn has_subjects_from(&mut self, index: &InvalidationIndex, from: Option<Rc<Node>>) {
        if index.has_anywhere {
            self.set_full("has");
            return;
        }
        let subjects: Vec<(super::AncestorCompound, u8)> = index
            .has_subjects
            .iter()
            .map(|(compound, at)| (super::AncestorCompound::parse(compound), *at))
            .collect();
        let mut at = from;
        while let Some(node) = at {
            if self.full.is_some() {
                return;
            }
            if let rustkit_dom::NodeType::Element { tag_name, attributes, .. } = &node.node_type {
                let classes = super::element_classes(attributes);
                for (compound, place) in &subjects {
                    if compound.matches_carried(tag_name, &classes, attributes.get("id"), Some(attributes)) {
                        self.apply(index, &node, *place);
                    }
                }
            }
            at = node.parent();
        }
    }

    /// The set for one flush's writes (the note's §2 table). `index` is the
    /// rule index the view was last built with; without one, or past the
    /// log's cap, only a full restyle covers the writes.
    pub(crate) fn compute(log: &MutationLog, document: &Document, index: Option<&InvalidationIndex>) -> Self {
        if log.full {
            return Self::full("log_overflow");
        }
        let Some(index) = index else {
            return Self::full("no_index");
        };
        let mut set = DirtySet::default();
        let mut siblings_done = HashSet::new();
        for record in &log.records {
            if set.full.is_some() {
                break;
            }
            let Some(node) = document.get_node(NodeId::new(record.node)) else {
                continue;
            };
            if in_shadow_scope(document, &node) {
                set.set_full("shadow");
                break;
            }
            if !is_connected(document, &node) {
                continue;
            }
            match record.kind {
                MutationKind::Style => {
                    if is_sheet_owner(&node) {
                        set.set_full("sheet");
                    }
                    set.mark(&node, STYLE | PAINT);
                    set.apply(index, &node, index.attributes.get("style").copied().unwrap_or(0));
                }
                MutationKind::Attribute => {
                    let name = record.attr.as_deref().unwrap_or("");
                    set.attribute(index, &node, name, record.old.as_deref());
                }
                MutationKind::Text => {
                    let Some(parent) = node.parent().filter(|p| p.is_element()) else {
                        continue;
                    };
                    if is_sheet_owner(&parent) {
                        set.set_full("sheet");
                    }
                    set.mark(&parent, LAYOUT);
                    set.apply(index, &parent, index.pseudos.get("empty").copied().unwrap_or(0));
                }
                MutationKind::ChildList => {
                    set.child_list(index, &node, record.child, &mut siblings_done, document);
                }
                MutationKind::FormValue => {
                    let bits = match record.attr.as_deref() {
                        Some("checked") => STYLE,
                        _ => STYLE | BOX | LAYOUT,
                    };
                    set.mark(&node, bits);
                    set.apply(index, &node, index.attribute_state_flags());
                    // An option's selectedness is drawn by its select (the
                    // label a drop-down shows, the rows a list box marks):
                    // the select's box is built again.
                    if node.tag_name().is_some_and(|t| t.eq_ignore_ascii_case("option")) {
                        let mut up = node.parent();
                        while let Some(p) = up {
                            match p.tag_name().map(|t| t.to_ascii_lowercase()).as_deref() {
                                Some("select") => {
                                    set.mark(&p, STYLE | BOX | LAYOUT);
                                    break;
                                }
                                Some("optgroup") => up = p.parent(),
                                _ => break,
                            }
                        }
                    }
                }
            }
        }
        if set.full.is_some() {
            set.nodes.clear();
        }
        set
    }

    fn attribute(&mut self, index: &InvalidationIndex, node: &Rc<Node>, name: &str, old: Option<&str>) {
        if is_sheet_owner(node) {
            self.set_full("sheet");
            return;
        }
        let read = |map: &HashMap<String, u8>, key: &str| map.get(key).copied().unwrap_or(0);
        // The index holds names lowercased; an SVG attribute keeps its case.
        let name = name.to_ascii_lowercase();
        self.apply(index, node, read(&index.attributes, &name));
        match name.as_str() {
            "class" => {
                let split = |v: Option<&str>| -> HashSet<String> {
                    v.unwrap_or("").split_ascii_whitespace().map(str::to_string).collect()
                };
                let (before, after) = (split(old), split(node.get_attribute("class")));
                for class in before.symmetric_difference(&after) {
                    self.apply(index, node, read(&index.classes, class));
                }
            }
            "id" => {
                for id in [old, node.get_attribute("id")].into_iter().flatten() {
                    self.apply(index, node, read(&index.ids, id));
                }
            }
            // No pseudo-class, presentational hint or box decision reads
            // these: only the selectors that name them.
            _ if name.starts_with("data-") || name.starts_with("aria-") => {}
            _ => {
                // A presentational hint, a UA rule (`[hidden]`, `<details
                // open>`) or a state pseudo-class can read any other name.
                self.mark(node, STYLE | BOX);
                self.apply(index, node, index.attribute_state_flags());
                if matches!(name.as_str(), "lang" | "dir") {
                    self.mark(node, SUBTREE_STYLE);
                }
            }
        }
    }

    fn child_list(
        &mut self,
        index: &InvalidationIndex,
        parent: &Rc<Node>,
        child: Option<usize>,
        siblings_done: &mut HashSet<usize>,
        document: &Document,
    ) {
        // The `:has()` subjects above the parent: an argument can match in
        // what was added or taken away.
        if index.has {
            self.has_subjects_from(index, Some(parent.clone()));
        }
        let child = child.and_then(|c| document.get_node(NodeId::new(c)));
        let replaced = child.is_none();
        let subtree = match &child {
            Some(c) => vec![c.clone()],
            None => parent.children(),
        };
        if is_sheet_owner(parent) || subtree.iter().any(contains_sheet_owner) {
            self.set_full("sheet");
            return;
        }
        self.mark(parent, BOX | LAYOUT);
        if replaced {
            // Every child is new.
            self.mark(parent, SUBTREE_STYLE);
        } else if let Some(child) =
            child.filter(|c| c.is_element() && c.parent().is_some_and(|p| Rc::ptr_eq(&p, parent)))
        {
            self.mark(&child, STYLE | SUBTREE_STYLE);
        }
        if index.positional && !replaced && siblings_done.insert(parent.id.raw()) {
            for sibling in parent.children().iter().filter(|c| c.is_element()) {
                self.mark(sibling, STYLE | SUBTREE_STYLE);
            }
        }
        self.apply(index, parent, index.pseudos.get("empty").copied().unwrap_or(0));
    }
}

/// A `<style>` or `<link>`: a write to it changes the sheets themselves.
fn is_sheet_owner(node: &Rc<Node>) -> bool {
    matches!(node.tag_name().map(str::to_ascii_lowercase).as_deref(), Some("style" | "link"))
}

fn contains_sheet_owner(node: &Rc<Node>) -> bool {
    is_sheet_owner(node) || node.children().iter().any(contains_sheet_owner)
}

fn is_connected(document: &Document, node: &Rc<Node>) -> bool {
    let mut at = node.clone();
    loop {
        if Rc::ptr_eq(&at, document.root()) {
            return true;
        }
        match at.parent() {
            Some(parent) => at = parent,
            None => return false,
        }
    }
}

/// In a shadow tree, a shadow host, or slotted into one: shadow sheets and
/// slot assignment are outside the index, so the full path.
fn in_shadow_scope(document: &Document, node: &Rc<Node>) -> bool {
    if !document.has_shadow_roots() {
        return false;
    }
    let host_or_slot = |n: &Rc<Node>| n.shadow_root().is_some() || n.is_slot();
    node.containing_shadow_root().is_some() || host_or_slot(node) || node.parent().is_some_and(|p| host_or_slot(&p))
}
