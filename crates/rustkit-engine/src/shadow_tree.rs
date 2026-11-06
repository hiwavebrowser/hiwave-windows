//! The flat tree in the box-tree build, and the style scoping of shadow
//! trees.
//!
//! The build walks `Node::flat_children` (rustkit-dom `shadow.rs`), so a
//! shadow host's boxes come from its shadow tree and a slot's from its
//! assigned nodes; `<slot>` is `display: contents`, so its children's boxes
//! take its place.
//!
//! A node assigned to a slot sits in the flat tree under the slot, but the
//! selectors that match it read its own tree: its ancestors are its host
//! and the host's ancestors, not the slot and the shadow tree around it.
//! Each host records that chain while its shadow tree is built, and the
//! slot hands it to its assigned nodes.
//!
//! Style scoping (CSS Scoping 1). Each tree has its own sheets: the
//! document's `<style>`/`<link>` sheets, and for a shadow tree the
//! `<style>` elements inside it ([`TreeStyle`]). The build matches an
//! element against the sheets of the tree it is in, and nothing else:
//! the host passes its shadow tree's sheets down to its shadow children in
//! place of its own, and a slot passes its host's tree's sheets to the
//! nodes assigned to it. A rule therefore records its tree scope by the
//! sheet list it sits in; matching never asks.
//!
//! Three kinds of rule in a shadow sheet reach elements outside it:
//! - `:host`, `:host(sel)`, `:host-context(sel)` as the subject style the
//!   host;
//! - `::slotted(sel)` styles the nodes assigned to a slot of the tree;
//! - `:host...` left of a combinator (`:host(.dark) .label`) styles the
//!   shadow tree, reading the host as the shadow children's outermost
//!   ancestor. The rule is rewritten to name marker classes, and the host
//!   is put in the chain as an ancestor carrying only those marks, so the
//!   ordinary ancestor walk matches it.
//!
//! The first two are cascaded with the outer tree's rules for the element
//! (CSS Cascade 4 §6.1 "context"): among normal declarations the outer
//! tree's win whatever their specificity, among `!important` ones the
//! inner tree's do. See [`take_extras`] and `compute_style_for_element`.
//!
//! A document with no shadow root installs none of this: [`active`] is
//! false, the per-element entry points return at that one branch, and the
//! build hands every element the sheets it always did.

use super::{Ancestor, SelectorMatcher, SiblingContext};
use rustkit_css::{Rule, Stylesheet};
use rustkit_dom::{Document, Node, NodeType};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// The class every host carries as a shadow-tree ancestor, and that a
/// rewritten `:host` names.
const HOST_MARK: &str = "-rk-host-";
/// The tag of the host as a shadow-tree ancestor: no type selector
/// matches it (the host is featureless there, CSS Scoping §3.1).
const HOST_TAG: &str = "-rk-host";

fn host_arg_mark(n: usize) -> String {
    format!("-rk-host{n}-")
}

thread_local! {
    /// The shadow hosts the build is inside, innermost last.
    static HOSTS: RefCell<Vec<HostEntry>> = const { RefCell::new(Vec::new()) };
    /// Is a build of a document with shadow roots in progress?
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    /// That build's state.
    static BUILD: RefCell<Option<BuildState>> = const { RefCell::new(None) };
    /// The custom element names defined in the document last built, for
    /// `:defined`. Script queries read these too, between builds.
    static DEFINED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// The names that document's load settled (rustkit-dom
    /// `settle_custom_element_definitions`), installed only while a build
    /// runs: they are `:defined` for the cascade, never for script.
    static SETTLED: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    /// The slots whose assigned nodes the build is inside, innermost last.
    static SLOTS: RefCell<Vec<SlotEntry>> = const { RefCell::new(Vec::new()) };
    /// Rules from other trees that matched the element whose cascade runs
    /// next (`:host` rules for a host, `::slotted` rules for a slotted node).
    static EXTRAS: RefCell<Vec<ExtraRule>> = const { RefCell::new(Vec::new()) };
}

struct HostEntry {
    host: usize,
    /// The chain the host's own children are matched against: the host and
    /// its ancestors.
    light_chain: Vec<Ancestor>,
    /// The sheets of the tree the host is in.
    outer: Rc<TreeStyle>,
}

struct SlotEntry {
    /// The host whose children this slot's assigned nodes are.
    host: usize,
    /// The `::slotted` rules of the slot's tree whose slot part matched the
    /// slot.
    rules: Vec<(Rc<TreeStyle>, usize)>,
}

struct BuildState {
    /// The scope stack: the tree whose elements are being built is last.
    current: Vec<Rc<TreeStyle>>,
    viewport: Option<(f32, f32)>,
    javascript_enabled: bool,
    /// Each shadow root's sheets, parsed once per build.
    by_root: HashMap<usize, Rc<TreeStyle>>,
    /// Shadow trees with the same `<style>` text (every instance of one
    /// component) share one parse.
    by_text: HashMap<String, Rc<TreeStyle>>,
}

/// One tree's sheets.
pub(crate) struct TreeStyle {
    /// The rules for the tree's own elements.
    pub(crate) sheets: Rc<Vec<Stylesheet>>,
    /// Rules whose subject is the host (`:host`, `:host(x)`,
    /// `:host-context(x)`).
    host_rules: Vec<ScopedRule>,
    /// `::slotted(x)` rules.
    slotted_rules: Vec<ScopedRule>,
    /// The `:host(x)` / `:host-context(x)` arguments left of a combinator:
    /// the host carries `host_arg_mark(n)` when it matches the n-th.
    host_args: Vec<(String, bool)>,
    /// Does any rule read the host as an ancestor?
    host_in_chain: bool,
}

/// A rule of a shadow sheet that styles an element outside the tree.
struct ScopedRule {
    /// The declarations, layer and original selector (for traces).
    rule: Rule,
    /// Host rules: what the host itself must match (`*` for any).
    /// Slotted rules: what the slot must match.
    target: String,
    /// Host rules: the `:host-context` argument. Slotted rules: the
    /// `::slotted` argument.
    arg: Option<String>,
    specificity: (usize, usize, usize),
}

/// A rule from another tree matched for the element whose cascade runs next.
pub(crate) struct ExtraRule {
    tree: Rc<TreeStyle>,
    host: bool,
    index: usize,
}

impl ExtraRule {
    fn scoped(&self) -> &ScopedRule {
        match self.host {
            true => &self.tree.host_rules[self.index],
            false => &self.tree.slotted_rules[self.index],
        }
    }

    pub(crate) fn rule(&self) -> &Rule {
        &self.scoped().rule
    }

    pub(crate) fn specificity(&self) -> (usize, usize, usize) {
        self.scoped().specificity
    }
}

/// Is a build of a document with shadow roots in progress? Every
/// per-element entry point below returns at this branch otherwise.
pub(crate) fn active() -> bool {
    ACTIVE.get()
}

/// `:defined` for a valid custom element name: has script defined it, or,
/// in the cascade only, did a load that lost a script settle the name
/// (rustkit-dom `settle_custom_element_definitions`)? Outside a build (a
/// script's `matches`, `querySelector(All)`, `closest`) the settled set is
/// empty, so script sees the registry alone, as Chromium does.
pub(crate) fn is_defined(tag: &str) -> bool {
    DEFINED.with(|d| d.borrow().contains(tag)) || SETTLED.with(|s| s.borrow().contains(tag))
}

/// Alive for one box-tree build: installs the document's defined custom
/// element names, its settled names (for this build's cascade only) and,
/// when the document has shadow roots, the scope stack with the document's
/// sheets at the bottom. Restores the settled names and the scope stack
/// installed before when dropped; the defined names stay for script.
pub(crate) struct BuildScope {
    previous: Option<(bool, Option<BuildState>)>,
    settled_before: HashSet<String>,
}

impl BuildScope {
    pub(crate) fn install(
        document: &Document,
        sheets: &Rc<Vec<Stylesheet>>,
        viewport: Option<(f32, f32)>,
        javascript_enabled: bool,
    ) -> Self {
        DEFINED.with(|d| *d.borrow_mut() = document.defined_custom_elements());
        let settled_before = SETTLED.with(|s| s.replace(document.settled_custom_elements()));
        if !document.has_shadow_roots() {
            return BuildScope {
                previous: None,
                settled_before,
            };
        }
        let state = BuildState {
            current: vec![Rc::new(TreeStyle {
                sheets: sheets.clone(),
                host_rules: Vec::new(),
                slotted_rules: Vec::new(),
                host_args: Vec::new(),
                host_in_chain: false,
            })],
            viewport,
            javascript_enabled,
            by_root: HashMap::new(),
            by_text: HashMap::new(),
        };
        let previous = BUILD.with(|b| b.borrow_mut().replace(state));
        let was_active = ACTIVE.replace(true);
        BuildScope {
            previous: Some((was_active, previous)),
            settled_before,
        }
    }
}

impl Drop for BuildScope {
    fn drop(&mut self) {
        SETTLED.with(|s| *s.borrow_mut() = std::mem::take(&mut self.settled_before));
        if let Some((was_active, previous)) = self.previous.take() {
            BUILD.with(|b| *b.borrow_mut() = previous);
            ACTIVE.set(was_active);
            EXTRAS.with(|e| e.borrow_mut().clear());
        }
    }
}

fn current_tree() -> Option<Rc<TreeStyle>> {
    BUILD.with(|b| b.borrow().as_ref().and_then(|s| s.current.last().cloned()))
}

fn push_tree(tree: Rc<TreeStyle>) {
    BUILD.with(|b| {
        if let Some(s) = b.borrow_mut().as_mut() {
            s.current.push(tree);
        }
    });
}

fn pop_tree() {
    BUILD.with(|b| {
        if let Some(s) = b.borrow_mut().as_mut() {
            s.current.pop();
        }
    });
}

/// The sheets of the shadow tree under `root`, parsed once per build.
fn tree_style(root: &Rc<Node>) -> Option<Rc<TreeStyle>> {
    let (viewport, javascript_enabled) = BUILD.with(|b| {
        let b = b.borrow();
        let s = b.as_ref()?;
        Some((s.viewport, s.javascript_enabled))
    })?;
    if let Some(hit) = BUILD.with(|b| b.borrow().as_ref()?.by_root.get(&root.id.raw()).cloned()) {
        return Some(hit);
    }
    let texts = style_texts(root, javascript_enabled);
    let key = texts.join("\u{0}");
    let cached = BUILD.with(|b| b.borrow().as_ref()?.by_text.get(&key).cloned());
    let tree = match cached {
        Some(tree) => tree,
        None => Rc::new(parse_tree_style(&texts, viewport)),
    };
    BUILD.with(|b| {
        if let Some(s) = b.borrow_mut().as_mut() {
            s.by_root.insert(root.id.raw(), tree.clone());
            s.by_text.insert(key, tree.clone());
        }
    });
    Some(tree)
}

/// The text of every `<style>` in the tree under `root`, in tree order;
/// nested shadow trees are their own and are not entered.
fn style_texts(root: &Rc<Node>, javascript_enabled: bool) -> Vec<String> {
    fn walk(node: &Rc<Node>, in_noscript: bool, javascript_enabled: bool, out: &mut Vec<String>) {
        for child in node.children() {
            let NodeType::Element { tag_name, .. } = &child.node_type else {
                continue;
            };
            // As `Engine::sheet_element_counts`: with scripting on, what is
            // inside `<noscript>` holds no sheet.
            let noscript = in_noscript || tag_name.eq_ignore_ascii_case("noscript");
            if tag_name.eq_ignore_ascii_case("style") {
                if noscript && javascript_enabled {
                    continue;
                }
                let mut text = String::new();
                for t in child.children() {
                    if let NodeType::Text(s) = &t.node_type {
                        text.push_str(s);
                    }
                }
                if !text.is_empty() {
                    out.push(text);
                }
                continue;
            }
            walk(&child, noscript, javascript_enabled, out);
        }
    }
    let mut out = Vec::new();
    walk(root, false, javascript_enabled, &mut out);
    // Adopted sheets follow the tree's own `<style>` elements (CSS
    // Cascade: `adoptedStyleSheets` is after the shadow root's style
    // sheets). A disabled sheet is stored as an empty string.
    out.extend(root.adopted_styles().into_iter().filter(|text| !text.is_empty()));
    out
}

fn parse_tree_style(texts: &[String], viewport: Option<(f32, f32)>) -> TreeStyle {
    let mut sheets: Vec<Stylesheet> = texts
        .iter()
        .filter_map(|t| Stylesheet::parse(t).ok())
        .collect();
    // `@media` and `@layer` as for the document's sheets; a shadow tree's
    // layers are its own (CSS Cascade 5 §6.4.1: per tree).
    super::Engine::keep_rules_for_viewport(&mut sheets, viewport);
    rustkit_css::assign_layer_order(&mut sheets);
    let mut tree = TreeStyle {
        sheets: Rc::new(Vec::new()),
        host_rules: Vec::new(),
        slotted_rules: Vec::new(),
        host_args: Vec::new(),
        host_in_chain: false,
    };
    for sheet in &mut sheets {
        let rules = std::mem::take(&mut sheet.rules);
        for mut rule in rules {
            let mut inner = Vec::new();
            for member in SelectorMatcher::split_top_level_commas(&rule.selector) {
                let member = member.trim();
                match classify(member) {
                    Member::Inner => inner.push(member.to_string()),
                    Member::HostChain => {
                        tree.host_in_chain = true;
                        inner.push(rewrite_host(member, &mut tree.host_args));
                    }
                    Member::Host { target, context, specificity } => {
                        let mut r = rule.clone();
                        r.selector = member.to_string();
                        tree.host_rules.push(ScopedRule { rule: r, target, arg: context, specificity });
                    }
                    Member::Slotted { slot, arg, specificity } => {
                        let slot = match slot.contains(":host") {
                            true => {
                                tree.host_in_chain = true;
                                rewrite_host(&slot, &mut tree.host_args)
                            }
                            false => slot,
                        };
                        let mut r = rule.clone();
                        r.selector = member.to_string();
                        tree.slotted_rules.push(ScopedRule { rule: r, target: slot, arg: Some(arg), specificity });
                    }
                    Member::Never => {}
                }
            }
            if !inner.is_empty() {
                rule.selector = inner.join(", ");
                sheet.rules.push(rule);
            }
        }
    }
    tree.sheets = Rc::new(sheets);
    tree
}

/// What one selector-list member of a shadow sheet targets.
#[derive(Debug, PartialEq)]
enum Member {
    /// Elements of the tree, `:host` not involved.
    Inner,
    /// Elements of the tree, with `:host...` left of a combinator.
    HostChain,
    /// The host: it must match `target`, and when `context` is set, it or
    /// an ancestor must match that.
    Host { target: String, context: Option<String>, specificity: (usize, usize, usize) },
    /// Nodes assigned to a slot matching `slot` that match `arg`.
    Slotted { slot: String, arg: String, specificity: (usize, usize, usize) },
    /// Matches nothing (`.a :host`, `::slotted(x)::before`, ...).
    Never,
}

/// Byte offsets where each top-level compound of `selector` starts, and
/// where `::slotted(` starts at the top level, if it does.
fn compounds(selector: &str) -> (Vec<usize>, Option<usize>) {
    let bytes = selector.as_bytes();
    let mut starts = vec![0];
    let mut depth = 0i32;
    let mut in_brackets = false;
    let mut quote: Option<u8> = None;
    let mut slotted = None;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match c {
            b'"' | b'\'' => quote = Some(c),
            b'[' => in_brackets = true,
            b']' => in_brackets = false,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b' ' | b'\t' | b'\n' | b'>' | b'+' | b'~' if depth == 0 && !in_brackets => {
                let mut j = i;
                while j < bytes.len() && matches!(bytes[j], b' ' | b'\t' | b'\n' | b'>' | b'+' | b'~') {
                    j += 1;
                }
                if j < bytes.len() {
                    starts.push(j);
                }
                i = j;
                continue;
            }
            b':' if depth == 0 && !in_brackets && selector[i..].starts_with("::slotted(") => {
                slotted.get_or_insert(i);
            }
            _ => {}
        }
        i += 1;
    }
    (starts, slotted)
}

/// The byte offset of the `)` closing the `(` at `open`.
fn closing_paren(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s[open..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `:host` occurrence at `at` in `s`: its kind, its argument, and the
/// offset just past it. `None` when `:host` there is the start of some
/// other name.
fn host_at(s: &str, at: usize) -> Option<(bool, Option<&str>, usize)> {
    let rest = &s[at..];
    if rest.starts_with(":host-context(") {
        let open = at + ":host-context".len();
        let close = closing_paren(s, open)?;
        return Some((true, Some(&s[open + 1..close]), close + 1));
    }
    if rest.starts_with(":host(") {
        let open = at + ":host".len();
        let close = closing_paren(s, open)?;
        return Some((false, Some(&s[open + 1..close]), close + 1));
    }
    let tail = rest.strip_prefix(":host")?;
    match tail.chars().next() {
        Some(c) if c.is_alphanumeric() || c == '-' || c == '_' => None,
        _ => Some((false, None, at + ":host".len())),
    }
}

/// Every top-level `:host` occurrence in `s`, as `host_at` reads it.
fn host_occurrences(s: &str) -> Vec<(usize, bool, Option<&str>, usize)> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_brackets = false;
    let mut quote: Option<char> = None;
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '[' => in_brackets = true,
            ']' => in_brackets = false,
            '(' => depth += 1,
            ')' => depth -= 1,
            ':' if depth == 0 && !in_brackets => {
                if let Some((context, arg, end)) = host_at(s, i) {
                    out.push((i, context, arg, end));
                }
            }
            _ => {}
        }
    }
    out
}

fn add((a, b, c): (usize, usize, usize), (x, y, z): (usize, usize, usize)) -> (usize, usize, usize) {
    (a + x, b + y, c + z)
}

fn specificity(selector: &str) -> (usize, usize, usize) {
    if selector.trim().is_empty() || selector.trim() == "*" {
        return (0, 0, 0);
    }
    SelectorMatcher.selector_specificity(selector)
}

fn classify(member: &str) -> Member {
    let (starts, slotted) = compounds(member);
    if let Some(at) = slotted {
        let open = at + "::slotted".len();
        let Some(close) = closing_paren(member, open) else {
            return Member::Never;
        };
        // `::slotted()` ends the selector: nothing may follow it here
        // (`::slotted(x)::before` is not covered).
        if !member[close + 1..].trim().is_empty() {
            return Member::Never;
        }
        let arg = member[open + 1..close].trim().to_string();
        let prefix = &member[..at];
        let slot = match prefix.trim_end() {
            "" => "*".to_string(),
            p if prefix.len() != p.len() || p.ends_with(['>', '+', '~']) => format!("{p} *"),
            p => p.to_string(),
        };
        // CSS Scoping §3.2.2: the pseudo-element plus its argument.
        let specificity = add(add(specificity(&slot), (0, 0, 1)), specificity(&arg));
        return Member::Slotted { slot, arg, specificity };
    }
    let hosts = host_occurrences(member);
    if hosts.is_empty() {
        return Member::Inner;
    }
    let subject = *starts.last().unwrap_or(&0);
    let in_subject: Vec<_> = hosts.iter().filter(|h| h.0 >= subject).collect();
    match in_subject.as_slice() {
        [] => Member::HostChain,
        // The host is in no tree an outer compound could walk: `.a :host`
        // never matches (`:host-context` is the way to ask that).
        [_] if subject > 0 => Member::Never,
        [&(at, context, arg, end)] => {
            let rest = &member[end..];
            let before = &member[..at];
            if rest.contains("::") || !before.trim().is_empty() {
                return Member::Never;
            }
            let (target, ctx) = match (context, arg) {
                (true, Some(a)) => (rest.to_string(), Some(a.trim().to_string())),
                (_, a) => (format!("{}{}", a.map(str::trim).unwrap_or(""), rest), None),
            };
            // Not `*.x`: the matcher reads a compound that starts with `*`
            // and goes on as `*` alone.
            let target = match target.trim() {
                "" => "*".to_string(),
                t => t.to_string(),
            };
            let specificity = add(
                add((0, 1, 0), specificity(&target)),
                ctx.as_deref().map(specificity).unwrap_or((0, 0, 0)),
            );
            Member::Host { target, context: ctx, specificity }
        }
        _ => Member::Never,
    }
}

/// `member` with each top-level `:host...` replaced by the marker classes
/// the host carries as an ancestor; the arguments go into `args`.
fn rewrite_host(member: &str, args: &mut Vec<(String, bool)>) -> String {
    let mut out = String::with_capacity(member.len() + 16);
    let mut last = 0;
    for (at, context, arg, end) in host_occurrences(member) {
        out.push_str(&member[last..at]);
        out.push('.');
        out.push_str(HOST_MARK);
        if let Some(arg) = arg {
            let key = (arg.trim().to_string(), context);
            let n = match args.iter().position(|a| *a == key) {
                Some(n) => n,
                None => {
                    args.push(key);
                    args.len() - 1
                }
            };
            out.push('.');
            out.push_str(&host_arg_mark(n));
        }
        last = end;
    }
    out.push_str(&member[last..]);
    out
}

/// Does the host (`tag`, `attributes`, its ancestors `ancestors`) match
/// `selector`, or, for `:host-context`, does it or an ancestor?
fn host_matches(
    selector: &str,
    context: bool,
    tag: &str,
    attributes: &HashMap<String, String>,
    ancestors: &[Ancestor],
    sib: SiblingContext,
) -> bool {
    if SelectorMatcher.selector_matches(selector, tag, attributes, &[], &[], sib) {
        return true;
    }
    context
        && SelectorMatcher.selector_matches(&format!("{selector} *"), tag, attributes, ancestors, &[], sib)
}

/// Alive while a shadow host's flat children are built: the host's tree
/// is pushed as the scope, its sheets go to the children, and the chain
/// they start from holds only the host as a featureless ancestor (or
/// nothing, when no rule reads it). A node that is not a host, or a build
/// with no shadow roots, records nothing.
pub(crate) struct HostScope {
    entered: bool,
    tree: Option<Rc<TreeStyle>>,
    chain: Vec<Ancestor>,
}

impl HostScope {
    pub(crate) fn enter(
        node: &Rc<Node>,
        tag: &str,
        attributes: &HashMap<String, String>,
        sib: SiblingContext,
        light_chain: &[Ancestor],
    ) -> Self {
        let inert = HostScope { entered: false, tree: None, chain: Vec::new() };
        if !active() {
            return inert;
        }
        let Some(root) = node.shadow_root() else {
            return inert;
        };
        let (Some(tree), Some(outer)) = (tree_style(&root), current_tree()) else {
            return inert;
        };
        let chain = match tree.host_in_chain {
            false => Vec::new(),
            true => {
                let mut classes = vec![HOST_MARK.to_string()];
                let ancestors = light_chain.get(1..).unwrap_or(&[]);
                for (n, (arg, context)) in tree.host_args.iter().enumerate() {
                    if host_matches(arg, *context, tag, attributes, ancestors, sib) {
                        classes.push(host_arg_mark(n));
                    }
                }
                vec![Rc::new((HOST_TAG.to_string(), classes, None, None))]
            }
        };
        HOSTS.with(|h| {
            h.borrow_mut().push(HostEntry {
                host: node.id.raw(),
                light_chain: light_chain.to_vec(),
                outer,
            })
        });
        push_tree(tree.clone());
        HostScope { entered: true, tree: Some(tree), chain }
    }

    /// The sheets the host's shadow children are matched against.
    pub(crate) fn sheets(&self) -> Option<&[Stylesheet]> {
        self.tree.as_ref().map(|t| t.sheets.as_slice())
    }

    /// The chain the host's shadow children start from.
    pub(crate) fn chain(&self) -> Option<&[Ancestor]> {
        self.entered.then_some(self.chain.as_slice())
    }
}

impl Drop for HostScope {
    fn drop(&mut self) {
        if self.entered {
            HOSTS.with(|h| {
                h.borrow_mut().pop();
            });
            pop_tree();
        }
    }
}

/// Alive while a slot's assigned nodes are built. They are children of a
/// host, not of the slot: they are matched against their host's chain and
/// the sheets of their host's tree, plus the slot tree's `::slotted` rules
/// whose slot part matches this slot.
pub(crate) struct SlotScope {
    chain: Vec<Ancestor>,
    outer: Rc<TreeStyle>,
}

impl SlotScope {
    /// `None` for anything but a slot showing assigned nodes.
    pub(crate) fn enter(
        slot: &Rc<Node>,
        attributes: &HashMap<String, String>,
        ancestors: &[Ancestor],
        sib: SiblingContext,
        flat: &[Rc<Node>],
    ) -> Option<Self> {
        if !active() || !slot.is_slot() {
            return None;
        }
        let host = flat.first()?.parent()?;
        if host.id == slot.id {
            return None;
        }
        let (chain, outer) = HOSTS.with(|h| {
            h.borrow()
                .iter()
                .rev()
                .find(|e| e.host == host.id.raw())
                .map(|e| (e.light_chain.clone(), e.outer.clone()))
        })?;
        let rules = match current_tree() {
            Some(tree) => (0..tree.slotted_rules.len())
                .filter(|&i| {
                    SelectorMatcher.selector_matches(
                        &tree.slotted_rules[i].target,
                        "slot",
                        attributes,
                        ancestors,
                        &[],
                        sib,
                    )
                })
                .map(|i| (tree.clone(), i))
                .collect(),
            None => Vec::new(),
        };
        SLOTS.with(|s| s.borrow_mut().push(SlotEntry { host: host.id.raw(), rules }));
        push_tree(outer.clone());
        Some(SlotScope { chain, outer })
    }

    pub(crate) fn chain(&self) -> &[Ancestor] {
        &self.chain
    }

    pub(crate) fn sheets(&self) -> &[Stylesheet] {
        self.outer.sheets.as_slice()
    }
}

impl Drop for SlotScope {
    fn drop(&mut self) {
        SLOTS.with(|s| {
            s.borrow_mut().pop();
        });
        pop_tree();
    }
}

/// Alive around one element's own cascade: the rules of other trees that
/// match it (its shadow tree's `:host` rules when it is a host, its slot's
/// `::slotted` rules when it is slotted), for `take_extras`.
pub(crate) struct OwnRules {
    installed: bool,
}

impl OwnRules {
    pub(crate) fn install(
        node: &Rc<Node>,
        tag: &str,
        attributes: &HashMap<String, String>,
        ancestors: &[Ancestor],
        sib: SiblingContext,
    ) -> Self {
        if !active() {
            return OwnRules { installed: false };
        }
        let mut extras = Vec::new();
        if let Some(tree) = node.shadow_root().and_then(|root| tree_style(&root)) {
            for (index, r) in tree.host_rules.iter().enumerate() {
                let matched = host_matches(&r.target, false, tag, attributes, ancestors, sib)
                    && r.arg
                        .as_deref()
                        .is_none_or(|ctx| host_matches(ctx, true, tag, attributes, ancestors, sib));
                if matched {
                    extras.push(ExtraRule { tree: tree.clone(), host: true, index });
                }
            }
        }
        let parent = node.parent().map(|p| p.id.raw());
        SLOTS.with(|s| {
            let s = s.borrow();
            let Some(slot) = s.last().filter(|e| Some(e.host) == parent) else {
                return;
            };
            for (tree, index) in &slot.rules {
                let arg = tree.slotted_rules[*index].arg.as_deref().unwrap_or("*");
                if SelectorMatcher.selector_matches(arg, tag, attributes, &[], &[], sib) {
                    extras.push(ExtraRule { tree: tree.clone(), host: false, index: *index });
                }
            }
        });
        if extras.is_empty() {
            return OwnRules { installed: false };
        }
        EXTRAS.with(|e| *e.borrow_mut() = extras);
        OwnRules { installed: true }
    }
}

impl Drop for OwnRules {
    fn drop(&mut self) {
        if self.installed {
            EXTRAS.with(|e| e.borrow_mut().clear());
        }
    }
}

/// The rules `OwnRules` installed for the cascade running now, taken so
/// that no other element's cascade sees them.
pub(crate) fn take_extras() -> Vec<ExtraRule> {
    EXTRAS.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_sheet_members_are_sorted_by_what_they_target() {
        assert_eq!(classify("p"), Member::Inner);
        assert_eq!(classify("div > .a:hover"), Member::Inner);
        assert_eq!(classify(":hostess"), Member::Inner);
        assert_eq!(classify(":host .a"), Member::HostChain);
        assert_eq!(classify(":host(.dark) > slot"), Member::HostChain);
        assert_eq!(
            classify(":host"),
            Member::Host { target: "*".into(), context: None, specificity: (0, 1, 0) }
        );
        assert_eq!(
            classify(":host(.wide)"),
            Member::Host { target: ".wide".into(), context: None, specificity: (0, 2, 0) }
        );
        assert_eq!(
            classify(":host(x-a.b):hover"),
            Member::Host { target: "x-a.b:hover".into(), context: None, specificity: (0, 3, 1) }
        );
        assert_eq!(
            classify(":host-context(.dark)"),
            Member::Host { target: "*".into(), context: Some(".dark".into()), specificity: (0, 2, 0) }
        );
        assert_eq!(classify(".a :host"), Member::Never);
        assert_eq!(classify(":host::before"), Member::Never);
        assert_eq!(
            classify("::slotted(p)"),
            Member::Slotted { slot: "*".into(), arg: "p".into(), specificity: (0, 0, 2) }
        );
        assert_eq!(
            classify("slot[name=n]::slotted(.x)"),
            Member::Slotted { slot: "slot[name=n]".into(), arg: ".x".into(), specificity: (0, 2, 2) }
        );
        assert_eq!(
            classify(".wrap > ::slotted(*)"),
            Member::Slotted { slot: ".wrap > *".into(), arg: "*".into(), specificity: (0, 1, 1) }
        );
        assert_eq!(classify("::slotted(p)::before"), Member::Never);
    }

    #[test]
    fn host_left_of_a_combinator_becomes_marker_classes() {
        let mut args = Vec::new();
        assert_eq!(rewrite_host(":host .a", &mut args), ".-rk-host- .a");
        assert_eq!(rewrite_host(":host(.dark) .a", &mut args), ".-rk-host-.-rk-host0- .a");
        assert_eq!(rewrite_host(":host-context(.dark) > b", &mut args), ".-rk-host-.-rk-host1- > b");
        assert_eq!(rewrite_host(":host(.dark) i", &mut args), ".-rk-host-.-rk-host0- i");
        assert_eq!(args, [(".dark".to_string(), false), (".dark".to_string(), true)]);
    }

    #[test]
    fn a_shadow_sheet_is_split_by_target() {
        let tree = parse_tree_style(
            &[":host{color:red} p, :host(.a){margin:0} ::slotted(i){x:y} .b{z:w}".to_string()],
            None,
        );
        let inner: Vec<&str> = tree.sheets[0].rules.iter().map(|r| r.selector.as_str()).collect();
        assert_eq!(inner, ["p", ".b"]);
        assert_eq!(tree.host_rules.len(), 2);
        assert_eq!(tree.slotted_rules.len(), 1);
        assert!(!tree.host_in_chain);
    }
}
