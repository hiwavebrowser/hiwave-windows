//! What the inline svg pre-pass keeps between builds (LAT4b).
//!
//! Before every build, each inline `<svg>` is serialized (with the
//! `<symbol>`s its `<use>`s name from elsewhere in the page) and hashed into
//! the key its parsed document is cached under, and the author paint
//! declarations of every svg element are matched from every rule of every
//! sheet. The symbols were found by walking the whole document once per
//! icon, and the box build serialized every icon a second time, so a
//! sprite-icon page paid icons x nodes on every relayout, a hover
//! restyle's included.
//!
//! Kept here, per layout context:
//! - an id index per tree root, built once per DOM mutation generation
//!   (`rustkit_dom::mutation_generation`), so a `<use>` resolves with one
//!   lookup;
//! - per inline `<svg>` element, its cache key and the nodes its markup was
//!   made from (the svg's subtree and every referenced subtree, with each
//!   node's author paint style). With the DOM and the paint map unchanged
//!   the key is taken as it is; after a DOM change the nodes are collected
//!   again (the svg's own subtrees, not the document) and compared, and the
//!   markup is serialized again only when they differ. A node's data is not
//!   interior-mutable (an attribute or text change replaces the node), so
//!   equal node pointers in equal order mean equal markup;
//! - the author paint map per document, keyed by the DOM generation, the
//!   view's `<link>` sheet list and the viewport (`<style>` text and adopted
//!   sheets are DOM, so they move the generation);
//! - the last whole pass: with the document, the generation and the paint
//!   map unchanged, the pre-pass does nothing at all.
//!
//! Weak pointers are kept, never strong ones: a `Weak` keeps its
//! allocation, so a pointer compared here cannot have been reused by
//! another node, and it keeps nothing of the node's contents alive.

use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};

use rustkit_css::Stylesheet;
use rustkit_dom::Node;

use crate::{SvgPaintRules, SvgPaintStyle};

pub(crate) type PaintMap = HashMap<usize, SvgPaintStyle>;

/// What a `<svg>`'s markup is made of: the svg, then (when any `<use>`
/// names an id it does not define itself) a trailing `<defs>` holding
/// every element so named, followed through nested `<use>`s.
pub(crate) struct SvgParts {
    /// The svg first, then each referenced element in markup order.
    pub(crate) roots: Vec<Rc<Node>>,
    /// Whether the markup ends with a `<defs>` (also an empty one, when
    /// nothing named resolves).
    pub(crate) defs: bool,
    /// Every node of every root's subtree, in the order they were walked.
    pub(crate) nodes: Vec<Rc<Node>>,
}

/// Only the elements rustkit-svg can instantiate are `<use>` targets: an id
/// naming an HTML container would serialize its whole subtree into the
/// markup (and the key) to paint nothing.
pub(crate) const REFERABLE: &[&str] = &[
    "symbol", "g", "svg", "use", "rect", "circle", "ellipse", "line", "path", "polyline", "polygon", "text",
];

fn walk(node: &Rc<Node>, f: &mut impl FnMut(&Rc<Node>)) {
    f(node);
    for child in node.children() {
        walk(&child, f);
    }
}

/// The root of the tree `node` is in: the document, a shadow root, or a
/// detached subtree's top.
pub(crate) fn tree_root(node: &Rc<Node>) -> Rc<Node> {
    let mut root = node.clone();
    while let Some(parent) = root.parent() {
        root = parent;
    }
    root
}

/// The first referable element (tree order) carrying each id under `root`.
pub(crate) fn id_index(root: &Rc<Node>) -> HashMap<String, Weak<Node>> {
    let mut by_id: HashMap<String, Weak<Node>> = HashMap::new();
    walk(root, &mut |n| {
        if let Some(id) = n.get_attribute("id") {
            if n.tag_name().is_some_and(|t| REFERABLE.iter().any(|r| t.eq_ignore_ascii_case(r))) {
                by_id.entry(id.to_string()).or_insert_with(|| Rc::downgrade(n));
            }
        }
    });
    by_id
}

/// `svg`'s parts, with `resolve` naming the element an id refers to.
pub(crate) fn svg_parts(svg: &Rc<Node>, resolve: &mut dyn FnMut(&str) -> Option<Rc<Node>>) -> SvgParts {
    fn references(
        node: &Rc<Node>,
        defined: &mut HashSet<String>,
        wanted: &mut Vec<String>,
        nodes: &mut Vec<Rc<Node>>,
    ) {
        walk(node, &mut |n| {
            nodes.push(n.clone());
            if let Some(id) = n.get_attribute("id") {
                defined.insert(id.to_string());
            }
            if n.tag_name().is_some_and(|t| t.eq_ignore_ascii_case("use")) {
                let href = n.get_attribute("href").or_else(|| n.get_attribute("xlink:href"));
                if let Some(id) = href.and_then(|h| h.trim().strip_prefix('#')) {
                    wanted.push(id.to_string());
                }
            }
        });
    }
    let mut nodes = Vec::new();
    let mut defined = HashSet::new();
    let mut wanted = Vec::new();
    references(svg, &mut defined, &mut wanted, &mut nodes);
    wanted.retain(|id| !defined.contains(id));
    let mut roots = vec![svg.clone()];
    if wanted.is_empty() {
        return SvgParts { roots, defs: false, nodes };
    }
    // A referenced symbol may itself `<use>` another one: follow those
    // too, each id once.
    while let Some(id) = wanted.pop() {
        if !defined.insert(id.clone()) {
            continue;
        }
        if let Some(target) = resolve(&id) {
            let mut nested = Vec::new();
            references(&target, &mut defined, &mut nested, &mut nodes);
            roots.push(target);
            wanted.extend(nested.into_iter().filter(|n| !defined.contains(n)));
        }
    }
    SvgParts { roots, defs: true, nodes }
}

/// One inline `<svg>`'s cache key and what it was made from.
struct Entry {
    gen: u64,
    paint: Weak<PaintMap>,
    nodes: Vec<(Weak<Node>, Option<SvgPaintStyle>)>,
    key: String,
}

impl Entry {
    fn made_from(&self, nodes: &[Rc<Node>], paint: &PaintMap) -> bool {
        self.nodes.len() == nodes.len()
            && self.nodes.iter().zip(nodes).all(|((was, style), now)| {
                std::ptr::eq(was.as_ptr(), Rc::as_ptr(now)) && style.as_ref() == paint.get(&now.id.raw())
            })
    }
}

/// The author paint map's inputs besides the document's own tree.
pub(crate) struct PaintKey {
    pub(crate) gen: u64,
    pub(crate) external: Weak<Vec<Stylesheet>>,
    pub(crate) viewport: Option<(u32, u32)>,
    pub(crate) javascript_enabled: bool,
    /// CH2.E19: the element state the paint rules read (hover, active,
    /// focus, checkedness), when they read any (`SvgPaintRules::reads_state`).
    pub(crate) state: Option<crate::SvgElementState>,
}

impl PaintKey {
    fn same(&self, other: &PaintKey) -> bool {
        self.gen == other.gen
            && std::ptr::eq(self.external.as_ptr(), other.external.as_ptr())
            && self.viewport == other.viewport
            && self.javascript_enabled == other.javascript_enabled
            && self.state == other.state
    }
}

#[derive(Default)]
pub(crate) struct SvgPrepass {
    /// The generation `indexes` were built at; any DOM change clears them.
    index_gen: u64,
    /// Per tree root (by pointer): the root and its id index.
    indexes: HashMap<usize, (Weak<Node>, HashMap<String, Weak<Node>>)>,
    /// Per inline `<svg>` element (by pointer).
    entries: HashMap<usize, (Weak<Node>, Entry)>,
    /// Per document root (by pointer): the paint map and what it was for.
    paint: HashMap<usize, (Weak<Node>, PaintKey, Rc<PaintMap>)>,
    /// The last whole pre-pass: document root, generation, paint map.
    last_pass: Option<(Weak<Node>, u64, Weak<PaintMap>)>,
    /// Keys whose markup failed to parse: not parsed again.
    pub(crate) failed: HashSet<String>,
    /// The paint rules of the sheets last read (`svg_paint_rules`).
    pub(crate) paint_rules: Option<Rc<SvgPaintRules>>,
    /// Per svg element (by pointer), when the paint rules read only the
    /// element and its ancestors: the element and its ancestors, the
    /// rules, and the element's paint style under them; and (CH2.E19) the
    /// element-state marks of the chain it was computed under.
    element_paint: HashMap<usize, (Vec<Weak<Node>>, Weak<SvgPaintRules>, Vec<u8>, Option<SvgPaintStyle>)>,
}

impl SvgPrepass {
    /// Forget what was built for a DOM generation other than `gen`.
    fn at_generation(&mut self, gen: u64) {
        if self.index_gen != gen {
            self.index_gen = gen;
            self.indexes.clear();
            self.entries.retain(|_, (svg, _)| svg.strong_count() > 0);
            self.paint.retain(|_, (root, ..)| root.strong_count() > 0);
            self.element_paint.retain(|_, (chain, ..)| chain[0].strong_count() > 0);
        }
    }

    /// `element`'s paint style under `rules` (which read only the element
    /// and its ancestors): computed by `compute` unless the element and
    /// every ancestor are the nodes they were when it was last computed.
    /// CH2.E19: `marks` gives each element's state marks; an element whose
    /// chain has the marks it had keeps its style, so a hover recomputes
    /// only the svg elements under the elements that gained or lost it.
    pub(crate) fn element_paint(
        &mut self,
        element: &Rc<Node>,
        rules: &Rc<SvgPaintRules>,
        marks: &dyn Fn(usize) -> u8,
        compute: &mut dyn FnMut(&Rc<Node>) -> Option<SvgPaintStyle>,
    ) -> Option<SvgPaintStyle> {
        self.at_generation(rustkit_dom::mutation_generation());
        let mut chain = vec![element.clone()];
        while let Some(parent) = chain.last().and_then(|n| n.parent()) {
            chain.push(parent);
        }
        let chain_marks: Vec<u8> = chain.iter().map(|n| marks(n.id.raw())).collect();
        let at = Rc::as_ptr(element) as usize;
        if let Some((was, kept_rules, was_marks, style)) = self.element_paint.get(&at) {
            let same = std::ptr::eq(kept_rules.as_ptr(), Rc::as_ptr(rules))
                && was.len() == chain.len()
                && was.iter().zip(&chain).all(|(w, n)| std::ptr::eq(w.as_ptr(), Rc::as_ptr(n)))
                && *was_marks == chain_marks;
            if same {
                return style.clone();
            }
        }
        let style = compute(element);
        self.element_paint.insert(
            at,
            (chain.iter().map(Rc::downgrade).collect(), Rc::downgrade(rules), chain_marks, style.clone()),
        );
        style
    }

    fn resolve(&mut self, root: &Rc<Node>, id: &str) -> Option<Rc<Node>> {
        let at = Rc::as_ptr(root) as usize;
        let (_, index) = self
            .indexes
            .entry(at)
            .or_insert_with(|| (Rc::downgrade(root), id_index(root)));
        index.get(id).and_then(Weak::upgrade)
    }

    /// `svg`'s parts, resolved through the id index.
    pub(crate) fn parts(&mut self, svg: &Rc<Node>) -> SvgParts {
        self.at_generation(rustkit_dom::mutation_generation());
        let root = tree_root(svg);
        svg_parts(svg, &mut |id| self.resolve(&root, id))
    }

    /// The cache key of `svg`'s markup under `paint`, serialized by
    /// `serialize` only when it is not known.
    pub(crate) fn key_for(
        &mut self,
        svg: &Rc<Node>,
        paint: &Rc<PaintMap>,
        serialize: impl FnOnce(&SvgParts, &PaintMap) -> String,
    ) -> String {
        let gen = rustkit_dom::mutation_generation();
        self.at_generation(gen);
        let at = Rc::as_ptr(svg) as usize;
        if let Some((_, entry)) = self.entries.get(&at) {
            if entry.gen == gen && std::ptr::eq(entry.paint.as_ptr(), Rc::as_ptr(paint)) {
                return entry.key.clone();
            }
        }
        let root = tree_root(svg);
        let parts = svg_parts(svg, &mut |id| self.resolve(&root, id));
        if let Some((_, entry)) = self.entries.get_mut(&at) {
            if entry.made_from(&parts.nodes, paint) {
                entry.gen = gen;
                entry.paint = Rc::downgrade(paint);
                return entry.key.clone();
            }
        }
        let key = serialize(&parts, paint);
        let nodes = parts
            .nodes
            .iter()
            .map(|n| (Rc::downgrade(n), paint.get(&n.id.raw()).cloned()))
            .collect();
        self.entries.insert(
            at,
            (
                Rc::downgrade(svg),
                Entry {
                    gen,
                    paint: Rc::downgrade(paint),
                    nodes,
                    key: key.clone(),
                },
            ),
        );
        key
    }

    /// The paint map kept for `document_root` under `key`, if any.
    pub(crate) fn paint_for(&mut self, document_root: &Rc<Node>, key: &PaintKey) -> Option<Rc<PaintMap>> {
        self.at_generation(key.gen);
        let (_, kept, map) = self.paint.get(&(Rc::as_ptr(document_root) as usize))?;
        kept.same(key).then(|| map.clone())
    }

    pub(crate) fn keep_paint(&mut self, document_root: &Rc<Node>, key: PaintKey, map: Rc<PaintMap>) {
        self.paint
            .insert(Rc::as_ptr(document_root) as usize, (Rc::downgrade(document_root), key, map));
    }

    /// Whether the whole pre-pass already ran for this document, DOM
    /// generation and paint map.
    pub(crate) fn pass_done(&self, document_root: &Rc<Node>, paint: &Rc<PaintMap>) -> bool {
        self.last_pass.as_ref().is_some_and(|(root, gen, map)| {
            std::ptr::eq(root.as_ptr(), Rc::as_ptr(document_root))
                && *gen == rustkit_dom::mutation_generation()
                && std::ptr::eq(map.as_ptr(), Rc::as_ptr(paint))
        })
    }

    pub(crate) fn note_pass(&mut self, document_root: &Rc<Node>, paint: &Rc<PaintMap>) {
        self.last_pass = Some((
            Rc::downgrade(document_root),
            rustkit_dom::mutation_generation(),
            Rc::downgrade(paint),
        ));
    }
}

#[cfg(test)]
mod tests {
    //! LAT4b pure helpers: id index referability and `<use>` part collection.
    //! The headless cost/correctness suite is `inline_svg_prepass_tests`.

    use super::{id_index, svg_parts, tree_root, REFERABLE};
    use rustkit_dom::Document;
    use std::rc::Rc;

    fn doc(html: &str) -> rustkit_dom::Document {
        Document::parse_html(html).unwrap()
    }

    #[test]
    fn the_id_index_keeps_only_referable_elements_and_the_first_of_each_id() {
        let document = doc(
            r##"<html><body>
              <div id="skip">no</div>
              <svg xmlns="http://www.w3.org/2000/svg">
                <symbol id="a"><path d="M0 0"/></symbol>
                <g id="a"><circle/></g>
                <path id="ok" d="M1 1"/>
              </svg>
            </body></html>"##,
        );
        let root = tree_root(&document.root());
        let index = id_index(&root);
        assert!(index.get("skip").is_none(), "HTML containers are not <use> targets");
        let a = index.get("a").and_then(|w| w.upgrade()).expect("symbol a");
        assert_eq!(a.tag_name().map(|t| t.to_ascii_lowercase()).as_deref(), Some("symbol"));
        let ok = index.get("ok").and_then(|w| w.upgrade()).expect("path ok");
        assert!(REFERABLE.iter().any(|r| ok.tag_name().unwrap().eq_ignore_ascii_case(r)));
    }

    #[test]
    fn svg_parts_follows_nested_use_and_xlink_href_into_defs() {
        let document = doc(
            r##"<html><body>
              <svg id="icon" xmlns="http://www.w3.org/2000/svg"><use href="#outer"/></svg>
              <svg xmlns="http://www.w3.org/2000/svg" style="display:none">
                <symbol id="outer"><use xlink:href="#inner"/></symbol>
                <symbol id="inner"><path id="leaf" d="M0 0h1v1H0z"/></symbol>
              </svg>
            </body></html>"##,
        );
        let icon = document.get_element_by_id("icon").expect("icon");
        let index = id_index(&tree_root(&icon));
        let parts = svg_parts(&icon, &mut |id| index.get(id).and_then(|w| w.upgrade()));
        assert!(parts.defs, "external references wrap in a trailing <defs>");
        assert_eq!(parts.roots.len(), 3, "svg + outer + inner");
        let tags: Vec<String> = parts
            .roots
            .iter()
            .map(|n| n.tag_name().unwrap().to_ascii_lowercase())
            .collect();
        assert_eq!(tags, vec!["svg", "symbol", "symbol"]);
        assert!(
            parts.nodes.iter().any(|n| n.get_attribute("id").as_deref() == Some("leaf")),
            "nested target body is part of the markup walk"
        );
    }

    #[test]
    fn a_self_contained_svg_needs_no_defs() {
        let document = doc(
            r##"<html><body>
              <svg id="icon" xmlns="http://www.w3.org/2000/svg">
                <symbol id="local"><path d="M0 0"/></symbol>
                <use href="#local"/>
              </svg>
            </body></html>"##,
        );
        let icon = document.get_element_by_id("icon").expect("icon");
        let index = id_index(&tree_root(&icon));
        let parts = svg_parts(&icon, &mut |id| index.get(id).and_then(|w| w.upgrade()));
        assert!(!parts.defs);
        assert_eq!(parts.roots.len(), 1);
        assert!(Rc::ptr_eq(&parts.roots[0], &icon));
    }
}
