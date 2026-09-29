//! `document` / `Node` / `Element` read access backed by the Rust DOM
//! (js-ladder rung 0, read slice; design pin: trench/DESIGN-dom-bindings-rung0.md).
//!
//! The split follows the pin's §1/§2:
//! - Rust owns the nodes. Script reaches them only through host functions
//!   that take and return primitives: a node crosses the boundary as its
//!   raw `NodeId` number, a list of nodes as a space-separated id string.
//! - Each wrapper is a plain JS object whose one host payload is a hidden
//!   slot `{ id, gen }`. The identity cache (`Map<NodeId, wrapper>`) lives
//!   in the bindings' JS closure, so the same node always yields the same
//!   object and Rust never holds a `JsObject`.
//! - `gen` is the document generation. NodeIds restart at 1 in every
//!   Document, so a bare id is ambiguous once a new document is bound.
//!   Binding one bumps `gen` and drops the cache; a host function called
//!   with a stale `gen` answers null, so old wrappers fail soft instead of
//!   reading the new page's nodes.
//!
//! Tree moves (`appendChild`/`insertBefore`/`removeChild`/`remove`) write
//! the Rust tree and mark the §3 `DomDirty` bucket, which the engine flushes
//! with one relayout when the script settles. Writes to a node's own data
//! (attributes, text) go through `Document::replace_node_data`, which keeps
//! the NodeId, so wrappers and the identity cache are untouched by them.

use crate::DomDirty;
use rustkit_dom::{Document, Node, NodeId, NodeType, QuerySelector};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Does this element match this selector list? `None` means the list is
/// invalid, and script throws `SyntaxError`.
pub type SelectorMatchFn = Rc<dyn Fn(&Rc<Node>, &str) -> Option<bool>>;

/// The document the host functions read, and its generation.
#[derive(Default)]
pub(crate) struct DomHost {
    document: Option<Rc<Document>>,
    generation: u32,
    /// The injected selector matcher (`DomBindings::set_selector_matcher`).
    pub(crate) matcher: Option<SelectorMatchFn>,
}

pub(crate) type SharedDomHost = Rc<RefCell<DomHost>>;

impl DomHost {
    /// Bind `document`, returning the new generation.
    pub(crate) fn bind(&mut self, document: Rc<Document>) -> u32 {
        self.document = Some(document);
        self.generation += 1;
        self.generation
    }

    fn document_for(&self, generation: &JsValue) -> Option<&Rc<Document>> {
        match generation {
            JsValue::Number(n) if *n == self.generation as f64 => self.document.as_ref(),
            _ => None,
        }
    }

    /// The node named by `args[0]` (generation) and `args[1]` (NodeId).
    fn node(&self, args: &[JsValue]) -> Option<Rc<Node>> {
        self.node_at(args, 1)
    }

    /// The node whose NodeId is `args[index]`, in the generation `args[0]`.
    fn node_at(&self, args: &[JsValue], index: usize) -> Option<Rc<Node>> {
        let document = self.document_for(args.first()?)?;
        match args.get(index)? {
            JsValue::Number(n) if *n >= 0.0 && n.fract() == 0.0 => {
                document.get_node(NodeId::new(*n as usize))
            }
            _ => None,
        }
    }
}

/// The elements under `scope` in document order, `scope` excluded.
fn descendant_elements(scope: &Rc<Node>, out: &mut Vec<Rc<Node>>) {
    for child in scope.children() {
        if child.is_element() {
            out.push(child.clone());
        }
        descendant_elements(&child, out);
    }
}

/// Is `node` in `document`'s tree? Removed nodes stay in the Document's
/// node table (their wrappers keep working), so tables that predate the
/// removal, like the id index, need this check.
fn is_connected(node: &Rc<Node>, document: &Document) -> bool {
    let root = document.root().id;
    let mut current = Some(node.clone());
    while let Some(n) = current {
        if n.id == root {
            return true;
        }
        current = n.parent();
    }
    false
}

/// The first element in tree order whose id is `id`.
fn first_with_id(document: &Document, id: &str) -> Option<Rc<Node>> {
    let mut found = None;
    document.traverse(|n| {
        if found.is_none() && n.get_attribute("id") == Some(id) {
            found = Some(n.clone());
        }
    });
    found
}

/// Is `ancestor` `node` or one of its ancestors?
fn is_inclusive_ancestor(ancestor: &Rc<Node>, node: &Rc<Node>) -> bool {
    ancestor.id == node.id || is_descendant(node, ancestor)
}

fn is_child_of(node: &Rc<Node>, parent: &Rc<Node>) -> bool {
    node.parent().is_some_and(|p| p.id == parent.id)
}

/// DOM §4.2.3 "pre-insert" `node` into `parent` before `child` (`None`
/// appends). Returns the DOMException name on a failed validity check,
/// before anything is touched.
fn pre_insert(
    parent: &Rc<Node>,
    node: &Rc<Node>,
    child: Option<Rc<Node>>,
) -> Result<(), &'static str> {
    // Only elements take children here; a Document's one-element rules and
    // DocumentFragment come with a later rung.
    if !parent.is_element() {
        return Err("HierarchyRequestError");
    }
    if is_inclusive_ancestor(node, parent) {
        return Err("HierarchyRequestError");
    }
    if child.as_ref().is_some_and(|c| !is_child_of(c, parent)) {
        return Err("NotFoundError");
    }
    if matches!(
        node.node_type,
        NodeType::Document | NodeType::DocumentType { .. }
    ) {
        return Err("HierarchyRequestError");
    }
    // Inserting a node before itself inserts it before its next sibling.
    let child = match child {
        Some(c) if c.id == node.id => node.next_sibling(),
        other => other,
    };
    node.remove_from_parent();
    match child {
        Some(c) => parent.insert_before(node.clone(), c),
        None => parent.append_child(node.clone()),
    }
    Ok(())
}

/// `mutate(gen, op, parentId, nodeId, childId)`: one tree write. Answers
/// null on success, else the DOMException name to throw.
fn mutate(host: &DomHost, args: &[JsValue]) -> Result<(), &'static str> {
    let (Some(parent), Some(node)) = (host.node_at(args, 2), host.node_at(args, 3)) else {
        return Err("NotFoundError");
    };
    match string_arg(args, 1) {
        Some("insert") => {
            let child = match args.get(4) {
                None | Some(JsValue::Null) | Some(JsValue::Undefined) => None,
                Some(_) => Some(host.node_at(args, 4).ok_or("NotFoundError")?),
            };
            pre_insert(&parent, &node, child)
        }
        Some("remove") if is_child_of(&node, &parent) => {
            node.remove_from_parent();
            Ok(())
        }
        Some("remove") => Err("NotFoundError"),
        _ => Err("NotSupportedError"),
    }
}

/// Is `name` usable as an element or attribute name? A simplified XML
/// `Name` check: non-empty, and none of the characters that end a name in
/// the HTML tokenizer. Failing it is DOM's InvalidCharacterError.
fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.chars().any(|c| {
            c.is_ascii_whitespace() || matches!(c, '\0' | '/' | '>' | '=' | '"' | '\'' | '<')
        })
}

/// `write(gen, op, id, a, b)`: one write to a node's own data, or a node
/// creation. `Ok` carries what to return to script (a new node's id, or
/// null) and the §3 bucket the write dirtied.
fn write(host: &DomHost, args: &[JsValue]) -> Result<(JsValue, DomDirty), &'static str> {
    let document = args
        .first()
        .and_then(|g| host.document_for(g))
        .ok_or("NotFoundError")?;
    let op = string_arg(args, 1).ok_or("NotSupportedError")?;
    if let Some(kind) = op.strip_prefix("create:") {
        let data = string_arg(args, 2).unwrap_or("").to_string();
        let node_type = match kind {
            "element" if is_valid_name(&data) => NodeType::Element {
                // An HTML document lowercases the name it is given.
                tag_name: data.to_ascii_lowercase(),
                namespace: String::from("http://www.w3.org/1999/xhtml"),
                attributes: Default::default(),
            },
            "element" => return Err("InvalidCharacterError"),
            "text" => NodeType::Text(data),
            "comment" => NodeType::Comment(data),
            _ => return Err("NotSupportedError"),
        };
        // A detached node is in no tree, so nothing needs a restyle yet.
        return Ok((
            node_id(Some(document.create_node(node_type))),
            DomDirty::Clean,
        ));
    }
    let node = host.node_at(args, 2).ok_or("NotFoundError")?;
    match (op, &node.node_type) {
        (
            "setAttr" | "removeAttr",
            NodeType::Element {
                tag_name,
                namespace,
                attributes,
            },
        ) => {
            let name = string_arg(args, 3).ok_or("InvalidCharacterError")?;
            if !is_valid_name(name) {
                return Err("InvalidCharacterError");
            }
            let name = if is_html_element(&node) {
                name.to_ascii_lowercase()
            } else {
                name.to_string()
            };
            let mut attributes = attributes.clone();
            let changed = if op == "setAttr" {
                let value = string_arg(args, 4).unwrap_or("").to_string();
                attributes.insert(name, value.clone()) != Some(value)
            } else {
                attributes.remove(&name).is_some()
            };
            if !changed {
                return Ok((JsValue::Null, DomDirty::Clean));
            }
            document.replace_node_data(
                node.id,
                NodeType::Element {
                    tag_name: tag_name.clone(),
                    namespace: namespace.clone(),
                    attributes,
                },
            );
            // Pin §3.3 restyles `style`/`class`/`id`; any other attribute
            // restyles too, since attribute selectors can match on it.
            Ok((JsValue::Null, DomDirty::Style))
        }
        ("setText", NodeType::Text(_) | NodeType::Comment(_)) => {
            let data = string_arg(args, 3).unwrap_or("").to_string();
            let node_type = match node.node_type {
                NodeType::Text(_) => NodeType::Text(data),
                _ => NodeType::Comment(data),
            };
            document.replace_node_data(node.id, node_type);
            // Pin §3.3: a text change relayouts.
            Ok((JsValue::Null, DomDirty::Layout))
        }
        // DOM §4.4 textContent setter on an element: replace all children
        // with one Text node (none for the empty string).
        ("setText", NodeType::Element { .. }) => {
            for child in node.children() {
                child.remove_from_parent();
            }
            let data = string_arg(args, 3).unwrap_or("");
            if !data.is_empty() {
                node.append_child(document.create_node(NodeType::Text(data.to_string())));
            }
            Ok((JsValue::Null, DomDirty::Style))
        }
        // Documents and doctypes ignore textContent writes.
        ("setText", _) => Ok((JsValue::Null, DomDirty::Clean)),
        // HTML §8.5 innerHTML setter: parse as the element's contents (the
        // fragment parsing algorithm), then replace all children with it.
        ("setHTML", NodeType::Element { tag_name, .. }) => {
            let html = string_arg(args, 3).unwrap_or("");
            let nodes = document
                .parse_fragment(html, tag_name)
                .map_err(|_| "SyntaxError")?;
            for child in node.children() {
                child.remove_from_parent();
            }
            for child in nodes {
                node.append_child(child);
            }
            Ok((JsValue::Null, DomDirty::Style))
        }
        _ => Err("NotSupportedError"),
    }
}

fn node_id(node: Option<Rc<Node>>) -> JsValue {
    node.map_or(JsValue::Null, |n| JsValue::Number(n.id.raw() as f64))
}

fn id_list(nodes: impl IntoIterator<Item = Rc<Node>>) -> JsValue {
    let ids: Vec<String> = nodes.into_iter().map(|n| n.id.raw().to_string()).collect();
    JsValue::String(ids.join(" "))
}

fn string_arg(args: &[JsValue], index: usize) -> Option<&str> {
    match args.get(index) {
        Some(JsValue::String(s)) => Some(s),
        _ => None,
    }
}

fn is_html_element(node: &Node) -> bool {
    matches!(&node.node_type, NodeType::Element { namespace, .. }
        if namespace.is_empty() || namespace == "http://www.w3.org/1999/xhtml")
}

/// Is `node` a descendant of `scope` (not `scope` itself)?
fn is_descendant(node: &Rc<Node>, scope: &Rc<Node>) -> bool {
    let mut current = node.parent();
    while let Some(parent) = current {
        if parent.id == scope.id {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// HTML void elements: serialized with no end tag and no children.
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "basefont", "bgsound", "br", "col", "embed", "frame", "hr", "img", "input",
    "keygen", "link", "meta", "param", "source", "track", "wbr",
];

/// Elements whose Text children serialize unescaped.
const RAW_TEXT_ELEMENTS: &[&str] = &[
    "style", "script", "xmp", "iframe", "noembed", "noframes", "plaintext",
];

/// HTML §13.3 "serializing HTML fragments" for one node (`outerHTML`).
/// Attributes come out sorted by name: rustkit-dom keeps them in a
/// HashMap, so source order is gone.
fn serialize_node(node: &Rc<Node>, out: &mut String) {
    match &node.node_type {
        NodeType::Element {
            tag_name,
            attributes,
            ..
        } => {
            out.push('<');
            out.push_str(tag_name);
            let mut names: Vec<&String> = attributes.keys().collect();
            names.sort();
            for name in names {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                escape_into(&attributes[name], true, out);
                out.push('"');
            }
            out.push('>');
            if !VOID_ELEMENTS.contains(&tag_name.as_str()) {
                serialize_children(node, out);
                out.push_str("</");
                out.push_str(tag_name);
                out.push('>');
            }
        }
        NodeType::Text(data) => {
            let raw = node
                .parent()
                .and_then(|p| p.tag_name().map(|t| RAW_TEXT_ELEMENTS.contains(&t)))
                .unwrap_or(false);
            if raw {
                out.push_str(data);
            } else {
                escape_into(data, false, out);
            }
        }
        NodeType::Comment(data) => {
            out.push_str("<!--");
            out.push_str(data);
            out.push_str("-->");
        }
        NodeType::ProcessingInstruction { target, data } => {
            out.push_str("<?");
            out.push_str(target);
            out.push(' ');
            out.push_str(data);
            out.push('>');
        }
        NodeType::DocumentType { name, .. } => {
            out.push_str("<!DOCTYPE ");
            out.push_str(name);
            out.push('>');
        }
        NodeType::Document => serialize_children(node, out),
    }
}

/// The children of `node`, serialized (`innerHTML`).
fn serialize_children(node: &Rc<Node>, out: &mut String) {
    for child in node.children() {
        serialize_node(&child, out);
    }
}

/// HTML §13.3 "escaping a string": `&`, NBSP, `<` and `>` always, and `"`
/// in attribute mode. (`<`/`>` in attribute values too, as Chrome 138+ does.)
fn escape_into(s: &str, attribute: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '\u{a0}' => out.push_str("&nbsp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// `info(gen, id, field)`: one read of one node.
fn node_info(node: &Rc<Node>, field: &str) -> JsValue {
    match field {
        "type" => JsValue::Number(match node.node_type {
            NodeType::Element { .. } => 1.0,
            NodeType::Text(_) => 3.0,
            NodeType::ProcessingInstruction { .. } => 7.0,
            NodeType::Comment(_) => 8.0,
            NodeType::Document => 9.0,
            NodeType::DocumentType { .. } => 10.0,
        }),
        "name" => JsValue::String(match &node.node_type {
            NodeType::Element { tag_name, .. } if is_html_element(node) => {
                tag_name.to_ascii_uppercase()
            }
            NodeType::Element { tag_name, .. } => tag_name.clone(),
            NodeType::Text(_) => "#text".to_string(),
            NodeType::Comment(_) => "#comment".to_string(),
            NodeType::Document => "#document".to_string(),
            NodeType::DocumentType { name, .. } => name.clone(),
            NodeType::ProcessingInstruction { target, .. } => target.clone(),
        }),
        "local" => match &node.node_type {
            NodeType::Element { tag_name, .. } => JsValue::String(tag_name.clone()),
            _ => JsValue::Null,
        },
        // DOM §4.4 textContent: null for documents and doctypes, the data
        // of character data, and the concatenated Text descendants of an
        // element (comments excluded).
        "text" => match &node.node_type {
            NodeType::Document | NodeType::DocumentType { .. } => JsValue::Null,
            NodeType::Text(data) | NodeType::Comment(data) => JsValue::String(data.clone()),
            NodeType::ProcessingInstruction { data, .. } => JsValue::String(data.clone()),
            NodeType::Element { .. } => JsValue::String(node.text_content()),
        },
        "parent" => node_id(node.parent()),
        "first" => node_id(node.first_child()),
        "last" => node_id(node.last_child()),
        "next" => node_id(node.next_sibling()),
        "prev" => node_id(node.previous_sibling()),
        "children" => id_list(node.children()),
        "innerHTML" => {
            let mut out = String::new();
            serialize_children(node, &mut out);
            JsValue::String(out)
        }
        "outerHTML" => {
            let mut out = String::new();
            serialize_node(node, &mut out);
            JsValue::String(out)
        }
        _ => JsValue::Undefined,
    }
}

/// Register the host functions and install the wrapper layer over the
/// stub `document`. Call after the stub globals exist. Tree writes mark
/// `dirty`.
pub(crate) fn install(
    runtime: &mut JsRuntime,
    host: &SharedDomHost,
    dirty: &Rc<Cell<DomDirty>>,
) -> Result<(), JsError> {
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_root",
        2,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(document) = args.first().and_then(|g| host.document_for(g)) else {
                return JsValue::Null;
            };
            match string_arg(args, 1) {
                Some("document") => node_id(Some(document.root().clone())),
                Some("documentElement") => node_id(document.document_element()),
                Some("head") => node_id(document.head()),
                Some("body") => node_id(document.body()),
                _ => JsValue::Null,
            }
        }),
    )?;

    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_by_id",
        2,
        Box::new(move |args| {
            let host = h.borrow();
            match (
                args.first().and_then(|g| host.document_for(g)),
                string_arg(args, 1),
            ) {
                // The id table is first-come and keeps removed nodes, so a
                // stale entry (say, content replaced by innerHTML that
                // reuses the id) falls back to the first match in tree order.
                // A connected hit is trusted: with duplicate ids (invalid
                // HTML) it may not be the first in tree order.
                (Some(document), Some(id)) if !id.is_empty() => node_id(
                    document
                        .get_element_by_id(id)
                        .filter(|n| is_connected(n, document))
                        .or_else(|| first_with_id(document, id)),
                ),
                _ => JsValue::Null,
            }
        }),
    )?;

    // collect(gen, scopeId, kind, arg): elements in document order, limited
    // to the descendants of `scopeId` (the document root means all).
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_collect",
        4,
        Box::new(move |args| {
            let host = h.borrow();
            let (Some(document), Some(scope)) = (
                args.first().and_then(|g| host.document_for(g)),
                host.node(args),
            ) else {
                return JsValue::Null;
            };
            let arg = string_arg(args, 3).unwrap_or("");
            if let (Some("selector"), Some(matcher)) = (string_arg(args, 2), host.matcher.as_ref())
            {
                // The scope itself decides validity, so an invalid selector
                // throws even where nothing could match. Matching reads each
                // element's whole ancestor chain, so `.outer p` scoped to
                // an inner element still sees `.outer` above the scope.
                if matcher(&scope, arg).is_none() {
                    return JsValue::Boolean(false);
                }
                let mut all = Vec::new();
                descendant_elements(&scope, &mut all);
                return id_list(all.into_iter().filter(|n| matcher(n, arg) == Some(true)));
            }
            let found = match string_arg(args, 2) {
                Some("tag") if arg == "*" => {
                    let mut all = Vec::new();
                    document.traverse(|n| {
                        if n.is_element() {
                            all.push(n.clone());
                        }
                    });
                    all
                }
                Some("tag") => document.get_elements_by_tag_name(arg),
                Some("class") => document.get_elements_by_class_name(arg),
                Some("selector") => QuerySelector::select(document, arg),
                _ => return JsValue::Null,
            };
            if scope.id == document.root().id {
                // `#id` selectors read the id table, which keeps removed nodes.
                id_list(found.into_iter().filter(|n| is_connected(n, document)))
            } else {
                id_list(found.into_iter().filter(|n| is_descendant(n, &scope)))
            }
        }),
    )?;

    // matches(gen, id, selector): a boolean, null for a stale node, or
    // "SyntaxError" for an invalid selector.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_matches",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            let (Some(node), Some(selector)) = (host.node(args), string_arg(args, 2)) else {
                return JsValue::Null;
            };
            let matched = match (&host.matcher, &host.document) {
                (Some(matcher), _) => matcher(&node, selector),
                (None, Some(document)) => Some(
                    QuerySelector::select(document, selector)
                        .iter()
                        .any(|n| n.id == node.id),
                ),
                (None, None) => return JsValue::Null,
            };
            match matched {
                Some(b) => JsValue::Boolean(b),
                None => JsValue::String("SyntaxError".to_string()),
            }
        }),
    )?;

    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_info",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            match (host.node(args), string_arg(args, 2)) {
                (Some(node), Some(field)) => node_info(&node, field),
                _ => JsValue::Null,
            }
        }),
    )?;

    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_attr",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            let (Some(node), Some(name)) = (host.node(args), string_arg(args, 2)) else {
                return JsValue::Null;
            };
            // HTML attribute names are matched ASCII-lowercased.
            let name = if is_html_element(&node) {
                name.to_ascii_lowercase()
            } else {
                name.to_string()
            };
            node.get_attribute(&name)
                .map_or(JsValue::Null, |v| JsValue::String(v.to_string()))
        }),
    )?;

    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_mutate",
        5,
        Box::new(move |args| match mutate(&h.borrow(), args) {
            Ok(()) => {
                // Pin §3.3: a structure insert/remove/move restyles.
                d.set(d.get().max(DomDirty::Style));
                JsValue::Null
            }
            Err(name) => JsValue::String(name.to_string()),
        }),
    )?;

    // write(gen, op, id, a, b): a string answer is the DOMException name
    // to throw; anything else is the result (a new node's id, or null).
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_write",
        5,
        Box::new(move |args| match write(&h.borrow(), args) {
            Ok((result, bucket)) => {
                d.set(d.get().max(bucket));
                result
            }
            Err(name) => JsValue::String(name.to_string()),
        }),
    )?;

    runtime.evaluate_script(WRAPPERS_JS)?;
    Ok(())
}

/// Wrapper prototypes, the identity cache, and the `document` read
/// surface. `window.__rustkit_dom_reset(gen)` rebinds it to a new document.
const WRAPPERS_JS: &str = r#"
(function (g) {
    var N = {
        root: __rustkit_dom_root, byId: __rustkit_dom_by_id,
        collect: __rustkit_dom_collect, info: __rustkit_dom_info,
        attr: __rustkit_dom_attr, mutate: __rustkit_dom_mutate,
        write: __rustkit_dom_write, matches: __rustkit_dom_matches
    };
    ['root', 'by_id', 'collect', 'info', 'attr', 'mutate', 'write', 'matches'].forEach(function (n) {
        delete g['__rustkit_dom_' + n];
    });

    if (typeof g.DOMException !== 'function') {
        g.DOMException = function DOMException(message, name) {
            if (!(this instanceof DOMException)) illegal();
            this.message = message === undefined ? '' : String(message);
            this.name = name === undefined ? 'Error' : String(name);
        };
        g.DOMException.prototype = Object.create(Error.prototype, {
            constructor: { value: g.DOMException, writable: true, configurable: true }
        });
    }
    var DOMException = g.DOMException;

    var SLOT = Symbol('rustkit.node');
    var gen = 0;
    var cache = new Map();

    function illegal() { throw new TypeError('Illegal constructor'); }
    function iface(name, parent) {
        var ctor = function () { illegal(); };
        Object.defineProperty(ctor, 'name', { value: name });
        if (parent) Object.setPrototypeOf(ctor.prototype, parent.prototype);
        Object.defineProperty(ctor.prototype, Symbol.toStringTag, { value: name });
        g[name] = ctor;
        return ctor;
    }
    var EventTarget = iface('EventTarget');
    var Node = iface('Node', EventTarget);
    var Document = iface('Document', Node);
    var CharacterData = iface('CharacterData', Node);
    var Text = iface('Text', CharacterData);
    var Comment = iface('Comment', CharacterData);
    var Element = iface('Element', Node);
    var HTMLElement = iface('HTMLElement', Element);
    var NodeList = iface('NodeList');
    var HTMLCollection = iface('HTMLCollection');

    var types = { ELEMENT_NODE: 1, TEXT_NODE: 3, PROCESSING_INSTRUCTION_NODE: 7,
                  COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10 };
    Object.keys(types).forEach(function (k) { Node[k] = Node.prototype[k] = types[k]; });

    function slotOf(o) {
        var s = o != null ? o[SLOT] : undefined;
        if (!s) throw new TypeError('Illegal invocation');
        return s;
    }
    function wrap(id) {
        if (typeof id !== 'number') return null;
        var w = cache.get(id);
        if (w) return w;
        var t = N.info(gen, id, 'type');
        var proto = t === 1 ? HTMLElement.prototype : t === 3 ? Text.prototype
                  : t === 8 ? Comment.prototype : Node.prototype;
        w = Object.create(proto);
        Object.defineProperty(w, SLOT, { value: { id: id, gen: gen } });
        cache.set(id, w);
        return w;
    }
    // A wrapper from an older document answers null/empty: its gen no
    // longer matches, so the host functions return null for it.
    function info(o, field) { var s = slotOf(o); return N.info(s.gen, s.id, field); }
    function related(o, field) { var s = slotOf(o); return s.gen === gen ? wrap(N.info(s.gen, s.id, field)) : null; }
    function list(proto, ids, onlyElements) {
        var out = Object.create(proto), n = 0;
        if (typeof ids === 'string' && ids !== '') {
            ids.split(' ').forEach(function (id) {
                var w = wrap(Number(id));
                if (w && (!onlyElements || w.nodeType === 1)) out[n++] = w;
            });
        }
        Object.defineProperty(out, 'length', { value: n });
        return out;
    }
    function collect(o, kind, arg) {
        var s = slotOf(o);
        return s.gen === gen ? N.collect(s.gen, s.id, kind, String(arg)) : '';
    }
    function getter(proto, name, fn) {
        Object.defineProperty(proto, name, { get: fn, configurable: true, enumerable: true });
    }

    [NodeList, HTMLCollection].forEach(function (C) {
        C.prototype.item = function (i) { return this[i >>> 0] || null; };
        C.prototype[Symbol.iterator] = Array.prototype.values;
    });
    NodeList.prototype.forEach = Array.prototype.forEach;
    NodeList.prototype.entries = Array.prototype.entries;
    NodeList.prototype.keys = Array.prototype.keys;
    NodeList.prototype.values = Array.prototype.values;

    getter(Node.prototype, 'nodeType', function () { return info(this, 'type'); });
    getter(Node.prototype, 'nodeName', function () { return info(this, 'name'); });
    function accessor(proto, name, get, set) {
        Object.defineProperty(proto, name, { get: get, set: set, configurable: true, enumerable: true });
    }
    // Writes to a node's own data. A string answer is the DOMException to
    // throw; an old-document wrapper names no node (-1) and throws NotFoundError.
    function setData(o, op, a, b, method) {
        var s = slotOf(o);
        var r = N.write(gen, op, s.gen === gen ? s.id : -1, a, b);
        if (typeof r === 'string') {
            throw new DOMException("Failed to execute '" + method + "'.", r);
        }
        return r;
    }
    function create(kind, data, method) {
        var r = N.write(gen, 'create:' + kind, data);
        if (typeof r === 'string') {
            throw new DOMException("Failed to execute '" + method + "' on 'Document'.", r);
        }
        return wrap(r);
    }
    // textContent = null writes the empty string (DOM: [LegacyNullToEmptyString]).
    function text(v) { return v === null ? '' : String(v); }
    accessor(Node.prototype, 'textContent', function () { return info(this, 'text'); },
        function (v) { setData(this, 'setText', text(v), null, 'textContent'); });
    getter(Node.prototype, 'parentNode', function () { return related(this, 'parent'); });
    getter(Node.prototype, 'parentElement', function () {
        var p = related(this, 'parent'); return p && p.nodeType === 1 ? p : null;
    });
    getter(Node.prototype, 'firstChild', function () { return related(this, 'first'); });
    getter(Node.prototype, 'lastChild', function () { return related(this, 'last'); });
    getter(Node.prototype, 'nextSibling', function () { return related(this, 'next'); });
    getter(Node.prototype, 'previousSibling', function () { return related(this, 'prev'); });
    getter(Node.prototype, 'childNodes', function () {
        var s = slotOf(this);
        return list(NodeList.prototype, s.gen === gen ? N.info(s.gen, s.id, 'children') : '', false);
    });
    getter(Node.prototype, 'ownerDocument', function () {
        return info(this, 'type') === 9 ? null : g.document;
    });
    Node.prototype.hasChildNodes = function () { return this.firstChild !== null; };

    // Tree moves. Each is one host write; a failed validity check throws
    // the DOMException the host names and leaves the tree untouched.
    function nodeArg(o, method) {
        if (o == null || !o[SLOT]) {
            throw new TypeError("Failed to execute '" + method +
                "' on 'Node': parameter 1 is not of type 'Node'.");
        }
        return o[SLOT];
    }
    function write(op, parent, node, child, method) {
        // A wrapper from an older document names no node here: -1 makes
        // the host answer NotFoundError instead of reusing its bare id.
        function here(s) { return s.gen === gen ? s.id : -1; }
        var p = slotOf(parent), n = nodeArg(node, method);
        var c = child == null ? null : here(nodeArg(child, method));
        var err = N.mutate(gen, op, here(p), here(n), c);
        if (err) throw new DOMException("Failed to execute '" + method + "' on 'Node'.", err);
        return node;
    }
    Node.prototype.appendChild = function (node) {
        return write('insert', this, node, null, 'appendChild');
    };
    Node.prototype.insertBefore = function (node, child) {
        if (arguments.length < 2) {
            throw new TypeError("Failed to execute 'insertBefore' on 'Node': 2 arguments required.");
        }
        return write('insert', this, node, child, 'insertBefore');
    };
    Node.prototype.removeChild = function (child) {
        return write('remove', this, child, null, 'removeChild');
    };
    // ChildNode.remove(): a no-op for a node without a parent.
    [Element, CharacterData].forEach(function (C) {
        C.prototype.remove = function () {
            var p = this.parentNode;
            if (p) p.removeChild(this);
        };
    });
    // DOM §4.2.3 replace: validate by inserting `node` first (insertBefore
    // throws before touching the tree), then take `child` out.
    Node.prototype.replaceChild = function (node, child) {
        if (arguments.length < 2) {
            throw new TypeError("Failed to execute 'replaceChild' on 'Node': 2 arguments required.");
        }
        nodeArg(node, 'replaceChild');
        nodeArg(child, 'replaceChild');
        if (node === child) {
            if (child.parentNode !== this) {
                throw new DOMException("Failed to execute 'replaceChild' on 'Node'.", 'NotFoundError');
            }
            return child;
        }
        write('insert', this, node, child, 'replaceChild');
        return write('remove', this, child, null, 'replaceChild');
    };

    // ParentNode.append/prepend and ChildNode.before/after/replaceWith
    // (DOM §4.2.6, §4.2.8) take nodes or strings; a string becomes a Text
    // node. DOM gathers them into a DocumentFragment; there are no
    // fragments yet, so the nodes are detached first and then inserted one
    // by one at the same reference point, which gives the same tree. A
    // failed validity check part-way leaves the earlier nodes inserted.
    function toNodes(args) {
        var nodes = Array.prototype.map.call(args, function (a) {
            return a != null && a[SLOT] ? a : g.document.createTextNode(String(a));
        });
        nodes.forEach(function (n) { var p = n.parentNode; if (p) p.removeChild(n); });
        return nodes;
    }
    function insertAll(parent, nodes, ref, method) {
        nodes.forEach(function (n) { write('insert', parent, n, ref, method); });
    }
    function viableSibling(node, field, nodes) {
        var s = node[field];
        while (s && nodes.indexOf(s) >= 0) s = s[field];
        return s;
    }
    Element.prototype.append = function () {
        insertAll(this, toNodes(arguments), null, 'append');
    };
    Element.prototype.prepend = function () {
        var nodes = toNodes(arguments);
        insertAll(this, nodes, this.firstChild, 'prepend');
    };
    Element.prototype.replaceChildren = function () {
        var nodes = toNodes(arguments);
        while (this.firstChild) this.removeChild(this.firstChild);
        insertAll(this, nodes, null, 'replaceChildren');
    };
    [Element, CharacterData].forEach(function (C) {
        C.prototype.before = function () {
            var p = this.parentNode;
            if (!p) return;
            var args = Array.prototype.slice.call(arguments);
            var prev = viableSibling(this, 'previousSibling', args);
            var nodes = toNodes(args);
            insertAll(p, nodes, prev ? prev.nextSibling : p.firstChild, 'before');
        };
        C.prototype.after = function () {
            var p = this.parentNode;
            if (!p) return;
            var args = Array.prototype.slice.call(arguments);
            var next = viableSibling(this, 'nextSibling', args);
            insertAll(p, toNodes(args), next, 'after');
        };
        C.prototype.replaceWith = function () {
            var p = this.parentNode;
            if (!p) return;
            var args = Array.prototype.slice.call(arguments);
            var next = viableSibling(this, 'nextSibling', args);
            var nodes = toNodes(args);
            if (this.parentNode === p) {
                insertAll(p, nodes, this, 'replaceWith');
                p.removeChild(this);
            } else {
                insertAll(p, nodes, next, 'replaceWith');
            }
        };
    });
    Node.prototype.contains = function (other) {
        for (var n = other; n; n = n.parentNode) if (n === this) return true;
        return false;
    };
    ['data', 'nodeValue'].forEach(function (k) {
        accessor(CharacterData.prototype, k, function () { return info(this, 'text'); },
            function (v) { setData(this, 'setText', text(v), null, k); });
    });
    getter(CharacterData.prototype, 'length', function () { return (info(this, 'text') || '').length; });

    getter(Element.prototype, 'tagName', function () { return info(this, 'name'); });
    accessor(Element.prototype, 'innerHTML', function () { return info(this, 'innerHTML'); },
        function (v) { setData(this, 'setHTML', text(v), null, 'innerHTML'); });
    getter(Element.prototype, 'outerHTML', function () { return info(this, 'outerHTML'); });
    getter(Element.prototype, 'localName', function () { return info(this, 'local'); });
    [['id', 'id'], ['className', 'class']].forEach(function (p) {
        accessor(Element.prototype, p[0], function () { return this.getAttribute(p[1]) || ''; },
            function (v) { this.setAttribute(p[1], v); });
    });
    getter(Element.prototype, 'children', function () {
        var s = slotOf(this);
        return list(HTMLCollection.prototype, s.gen === gen ? N.info(s.gen, s.id, 'children') : '', true);
    });

    // ParentNode / NonDocumentTypeChildNode element traversal (DOM §4.2.6,
    // §4.2.7), over the same tree reads as childNodes and the siblings.
    function elementChildren(o) {
        return Array.prototype.filter.call(o.childNodes, function (n) { return n.nodeType === 1; });
    }
    function elementSibling(o, field) {
        var n = o[field];
        while (n && n.nodeType !== 1) n = n[field];
        return n;
    }
    [Element, Document].forEach(function (C) {
        getter(C.prototype, 'firstElementChild', function () { return elementChildren(this)[0] || null; });
        getter(C.prototype, 'lastElementChild', function () {
            var c = elementChildren(this); return c[c.length - 1] || null;
        });
        getter(C.prototype, 'childElementCount', function () { return elementChildren(this).length; });
    });
    getter(Document.prototype, 'children', function () {
        var s = slotOf(this);
        return list(HTMLCollection.prototype, s.gen === gen ? N.info(s.gen, s.id, 'children') : '', true);
    });
    [Element, CharacterData].forEach(function (C) {
        getter(C.prototype, 'nextElementSibling', function () { return elementSibling(this, 'nextSibling'); });
        getter(C.prototype, 'previousElementSibling', function () {
            return elementSibling(this, 'previousSibling');
        });
    });
    // A node is connected when its root is the current document; an old
    // document's wrappers have no parent and are never connected.
    getter(Node.prototype, 'isConnected', function () {
        var n = this;
        while (n.parentNode) n = n.parentNode;
        return n === g.document && slotOf(n).gen === gen;
    });

    // HTMLElement reflected attributes (HTML §3.2.6) and dataset (§3.2.6.6).
    ['title', 'lang', 'dir'].forEach(function (k) {
        accessor(HTMLElement.prototype, k, function () { return this.getAttribute(k) || ''; },
            function (v) { this.setAttribute(k, v); });
    });
    accessor(HTMLElement.prototype, 'hidden', function () { return this.hasAttribute('hidden'); },
        function (v) { this.toggleAttribute('hidden', !!v); });
    var datasets = new WeakMap();
    function dataAttr(p) {
        if (typeof p !== 'string' || /-[a-z]/.test(p)) return null;
        return 'data-' + p.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); });
    }
    getter(HTMLElement.prototype, 'dataset', function () {
        slotOf(this);
        var el = this, d = datasets.get(el);
        if (d) return d;
        d = new Proxy({}, {
            get: function (t, p) {
                var a = dataAttr(p);
                if (!a) return undefined;
                var v = el.getAttribute(a);
                return v === null ? undefined : v;
            },
            set: function (t, p, v) {
                var a = dataAttr(p);
                if (!a) {
                    throw new DOMException("Failed to set a named property on 'DOMStringMap': '" +
                        String(p) + "' is not a valid property name.", 'SyntaxError');
                }
                el.setAttribute(a, String(v));
                return true;
            },
            has: function (t, p) { var a = dataAttr(p); return !!a && el.hasAttribute(a); },
            deleteProperty: function (t, p) {
                var a = dataAttr(p);
                if (a) el.removeAttribute(a);
                return true;
            }
        });
        datasets.set(el, d);
        return d;
    });

    Element.prototype.getAttribute = function (name) {
        var s = slotOf(this); return N.attr(s.gen, s.id, String(name));
    };
    Element.prototype.hasAttribute = function (name) { return this.getAttribute(name) !== null; };
    Element.prototype.setAttribute = function (name, value) {
        if (arguments.length < 2) {
            throw new TypeError("Failed to execute 'setAttribute' on 'Element': 2 arguments required.");
        }
        setData(this, 'setAttr', String(name), String(value), 'setAttribute');
    };
    Element.prototype.removeAttribute = function (name) {
        setData(this, 'removeAttr', String(name), null, 'removeAttribute');
    };
    Element.prototype.toggleAttribute = function (name, force) {
        var has = this.hasAttribute(name);
        var want = force === undefined ? !has : !!force;
        if (want && !has) this.setAttribute(name, '');
        if (!want && has) this.removeAttribute(name);
        return want;
    };

    // classList: a DOMTokenList over the class attribute, one per element.
    var DOMTokenList = iface('DOMTokenList');
    var TOKENS = Symbol('rustkit.tokens');
    function tokens(list) {
        var v = list[TOKENS].getAttribute('class') || '';
        var out = [];
        v.split(/[\t\n\f\r ]+/).forEach(function (t) { if (t && out.indexOf(t) < 0) out.push(t); });
        return out;
    }
    function checkToken(t, method) {
        t = String(t);
        if (t === '') throw new DOMException("Failed to execute '" + method + "' on 'DOMTokenList': The token provided must not be empty.", 'SyntaxError');
        if (/[\t\n\f\r ]/.test(t)) throw new DOMException("Failed to execute '" + method + "' on 'DOMTokenList': The token provided contains HTML space characters.", 'InvalidCharacterError');
        return t;
    }
    function store(list, ts) { list[TOKENS].setAttribute('class', ts.join(' ')); }
    getter(DOMTokenList.prototype, 'length', function () { return tokens(this).length; });
    accessor(DOMTokenList.prototype, 'value', function () { return this[TOKENS].getAttribute('class') || ''; },
        function (v) { this[TOKENS].setAttribute('class', String(v)); });
    DOMTokenList.prototype.toString = function () { return this.value; };
    DOMTokenList.prototype.item = function (i) { var ts = tokens(this); i = i >>> 0; return i < ts.length ? ts[i] : null; };
    DOMTokenList.prototype.contains = function (t) { return tokens(this).indexOf(String(t)) >= 0; };
    DOMTokenList.prototype.add = function () {
        var args = Array.prototype.map.call(arguments, function (t) { return checkToken(t, 'add'); });
        var ts = tokens(this);
        args.forEach(function (t) { if (ts.indexOf(t) < 0) ts.push(t); });
        store(this, ts);
    };
    DOMTokenList.prototype.remove = function () {
        var args = Array.prototype.map.call(arguments, function (t) { return checkToken(t, 'remove'); });
        var ts = tokens(this).filter(function (t) { return args.indexOf(t) < 0; });
        // DOM's update steps skip writing a class attribute that isn't there.
        if (this[TOKENS].hasAttribute('class')) store(this, ts);
    };
    DOMTokenList.prototype.toggle = function (t, force) {
        t = checkToken(t, 'toggle');
        var ts = tokens(this), i = ts.indexOf(t);
        if (i >= 0 && force !== true) { ts.splice(i, 1); store(this, ts); return false; }
        if (i < 0 && force !== false) { ts.push(t); store(this, ts); return true; }
        return i >= 0;
    };
    DOMTokenList.prototype.replace = function (a, b) {
        a = checkToken(a, 'replace'); b = checkToken(b, 'replace');
        var ts = tokens(this), i = ts.indexOf(a);
        if (i < 0) return false;
        var j = ts.indexOf(b);
        if (j >= 0 && j !== i) ts.splice(i, 1); else ts[i] = b;
        store(this, ts);
        return true;
    };
    DOMTokenList.prototype.forEach = function (cb, self) { tokens(this).forEach(cb, self); };
    DOMTokenList.prototype[Symbol.iterator] = function () { return tokens(this)[Symbol.iterator](); };
    var classLists = new WeakMap();
    getter(Element.prototype, 'classList', function () {
        slotOf(this);
        var l = classLists.get(this);
        if (!l) {
            l = Object.create(DOMTokenList.prototype);
            Object.defineProperty(l, TOKENS, { value: this });
            classLists.set(this, l);
        }
        return l;
    });

    // style: a CSSStyleDeclaration over the style attribute, one per
    // element. Reads parse the attribute; writes reserialize it, so the
    // cascade sees them through the attribute (and the write marks Style).
    // A `;` inside a string or url() is not yet split correctly.
    var CSSStyleDeclaration = iface('CSSStyleDeclaration');
    var OWNER = Symbol('rustkit.styleOwner');
    function decls(st) {
        var out = [];
        (st[OWNER].getAttribute('style') || '').split(';').forEach(function (d) {
            var i = d.indexOf(':');
            if (i < 0) return;
            var name = d.slice(0, i).trim().toLowerCase(), value = d.slice(i + 1).trim();
            var important = /!\s*important$/i.test(value);
            if (important) value = value.replace(/\s*!\s*important$/i, '');
            if (!name || !value) return;
            out = out.filter(function (x) { return x.name !== name; });
            out.push({ name: name, value: value, important: important });
        });
        return out;
    }
    function storeDecls(st, ds) {
        st[OWNER].setAttribute('style', ds.map(function (d) {
            return d.name + ': ' + d.value + (d.important ? ' !important' : '') + ';';
        }).join(' '));
    }
    function cssName(prop) {
        if (prop === 'cssFloat') return 'float';
        if (prop.indexOf('-') >= 0) return prop.toLowerCase();
        return prop.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); })
                   .replace(/^(webkit|moz|ms)-/, '-$1-');
    }
    var styleMethods = {
        getPropertyValue: function (name) {
            name = String(name).trim().toLowerCase();
            var d = decls(this).filter(function (x) { return x.name === name; })[0];
            return d ? d.value : '';
        },
        getPropertyPriority: function (name) {
            name = String(name).trim().toLowerCase();
            var d = decls(this).filter(function (x) { return x.name === name; })[0];
            return d && d.important ? 'important' : '';
        },
        setProperty: function (name, value, priority) {
            name = String(name).trim().toLowerCase();
            value = value == null ? '' : String(value).trim();
            if (value === '') { this.removeProperty(name); return; }
            var ds = decls(this), d = ds.filter(function (x) { return x.name === name; })[0];
            var important = String(priority || '').toLowerCase() === 'important';
            if (d) { d.value = value; d.important = important; }
            else ds.push({ name: name, value: value, important: important });
            storeDecls(this, ds);
        },
        removeProperty: function (name) {
            name = String(name).trim().toLowerCase();
            var ds = decls(this), old = this.getPropertyValue(name);
            var kept = ds.filter(function (x) { return x.name !== name; });
            if (kept.length !== ds.length) storeDecls(this, kept);
            return old;
        },
        item: function (i) { var ds = decls(this); i = i >>> 0; return i < ds.length ? ds[i].name : ''; }
    };
    Object.keys(styleMethods).forEach(function (k) { CSSStyleDeclaration.prototype[k] = styleMethods[k]; });
    getter(CSSStyleDeclaration.prototype, 'length', function () { return decls(this).length; });
    accessor(CSSStyleDeclaration.prototype, 'cssText', function () {
        return this[OWNER].getAttribute('style') ? decls(this).map(function (d) {
            return d.name + ': ' + d.value + (d.important ? ' !important' : '') + ';';
        }).join(' ') : '';
    }, function (v) { this[OWNER].setAttribute('style', v == null ? '' : String(v)); });
    // Property names (el.style.backgroundColor) go through a Proxy, so any
    // CSS property reads and writes without a per-property table.
    var styles = new WeakMap();
    function styleFor(el) {
        var st = styles.get(el);
        if (st) return st;
        var target = Object.create(CSSStyleDeclaration.prototype);
        Object.defineProperty(target, OWNER, { value: el });
        st = new Proxy(target, {
            get: function (t, p) {
                if (typeof p !== 'string' || p in t) return Reflect.get(t, p, st);
                if (/^\d+$/.test(p)) return t.item(Number(p)) || undefined;
                return t.getPropertyValue(cssName(p));
            },
            set: function (t, p, v) {
                if (typeof p !== 'string' || p in t) return Reflect.set(t, p, v, st);
                t.setProperty(cssName(p), v);
                return true;
            }
        });
        styles.set(el, st);
        return st;
    }
    accessor(Element.prototype, 'style', function () { slotOf(this); return styleFor(this); },
        function (v) { this.style.cssText = v; });

    // querySelector/All, getElementsBy* on both Document and Element. The
    // collections are static snapshots (pin §4: live HTMLCollection later).
    function select(o, sel, method) {
        var ids = collect(o, 'selector', sel);
        if (ids === false) {
            throw new DOMException("Failed to execute '" + method + "': '" + sel +
                "' is not a valid selector.", 'SyntaxError');
        }
        return ids;
    }
    var queries = {
        querySelector: function (sel) {
            var ids = select(this, sel, 'querySelector');
            return ids ? wrap(Number(ids.split(' ')[0])) : null;
        },
        querySelectorAll: function (sel) {
            return list(NodeList.prototype, select(this, sel, 'querySelectorAll'), false);
        },
        getElementsByTagName: function (tag) {
            return list(HTMLCollection.prototype, collect(this, 'tag', tag), false);
        },
        getElementsByClassName: function (cls) {
            return list(HTMLCollection.prototype, collect(this, 'class', cls), false);
        }
    };
    Object.keys(queries).forEach(function (k) {
        Element.prototype[k] = queries[k];
        Document.prototype[k] = queries[k];
    });
    function matches(el, sel, method) {
        var s = slotOf(el);
        if (s.gen !== gen) return false;
        var r = N.matches(s.gen, s.id, String(sel));
        if (r === 'SyntaxError') {
            throw new DOMException("Failed to execute '" + method + "' on 'Element': '" +
                sel + "' is not a valid selector.", 'SyntaxError');
        }
        return r === true;
    }
    Element.prototype.matches = function (sel) { return matches(this, sel, 'matches'); };
    Element.prototype.webkitMatchesSelector = function (sel) {
        return matches(this, sel, 'webkitMatchesSelector');
    };
    Element.prototype.closest = function (sel) {
        for (var el = this; el; el = el.parentElement) {
            if (matches(el, sel, 'closest')) return el;
        }
        return null;
    };
    Document.prototype.createElement = function (tag) {
        tag = String(tag);
        // Form controls keep their JS stubs for now: their editing state
        // (value, selection) lives there, not in the Rust DOM.
        if (/^(input|textarea|form)$/i.test(tag)) return stubCreateElement.call(this, tag);
        return create('element', tag, 'createElement');
    };
    Document.prototype.createTextNode = function (data) {
        return create('text', String(data), 'createTextNode');
    };
    Document.prototype.createComment = function (data) {
        return create('comment', String(data), 'createComment');
    };
    Document.prototype.getElementById = function (id) {
        var s = slotOf(this);
        return s.gen === gen ? wrap(N.byId(s.gen, String(id))) : null;
    };
    ['documentElement', 'head', 'body'].forEach(function (k) {
        getter(Document.prototype, k, function () {
            var s = slotOf(this); return s.gen === gen ? wrap(N.root(s.gen, k)) : null;
        });
    });

    // EventTarget (DOM §2.7) for node wrappers, document and window: a
    // JS-side listener registry, and dispatch through capture, target and
    // bubble phases along the wrapper tree (then document, then window).
    // Engine input events still dispatch Rust-side (events.rs, pin §4);
    // this is what page script registers and fires itself.
    var LISTENERS = new WeakMap();
    var STOP = Symbol('rustkit.stop'), STOP_NOW = Symbol('rustkit.stopNow');
    function thisTarget(o) { return o == null ? g : o; }
    function flag(opts, key) {
        return typeof opts === 'boolean' ? key === 'capture' && opts : !!(opts && opts[key]);
    }
    EventTarget.prototype.addEventListener = function (type, cb, opts) {
        if (typeof cb !== 'function' && !(cb && typeof cb.handleEvent === 'function')) return;
        var t = thisTarget(this), all = LISTENERS.get(t);
        if (!all) { all = {}; LISTENERS.set(t, all); }
        type = String(type);
        var list = all[type] || (all[type] = []), capture = flag(opts, 'capture');
        for (var i = 0; i < list.length; i++) {
            if (list[i].cb === cb && list[i].capture === capture) return;
        }
        list.push({ cb: cb, capture: capture, once: flag(opts, 'once'), removed: false });
    };
    EventTarget.prototype.removeEventListener = function (type, cb, opts) {
        var all = LISTENERS.get(thisTarget(this)), list = all && all[String(type)];
        if (!list) return;
        var capture = flag(opts, 'capture');
        for (var i = 0; i < list.length; i++) {
            if (list[i].cb === cb && list[i].capture === capture) {
                list[i].removed = true;
                list.splice(i, 1);
                return;
            }
        }
    };
    // A listener's exception goes to the engine's error log, not to the
    // dispatcher (as in a browser, where it reaches window.onerror).
    function callListener(t, cb, event) {
        try {
            return typeof cb === 'function' ? cb.call(t, event) : cb.handleEvent(event);
        } catch (e) {
            var msg;
            try { msg = String(e); } catch (_) { msg = '<unprintable exception>'; }
            if (g.__rustkit_errors) g.__rustkit_errors.push(msg);
        }
    }
    function invoke(t, event, phase, capture) {
        event.currentTarget = t;
        event.eventPhase = phase;
        var all = LISTENERS.get(t), list = all && all[event.type];
        if (list) {
            list = list.slice();
            for (var i = 0; i < list.length && !event[STOP_NOW]; i++) {
                var l = list[i];
                if (l.removed || l.capture !== capture) continue;
                if (l.once) t.removeEventListener(event.type, l.cb, { capture: l.capture });
                callListener(t, l.cb, event);
            }
        }
        // The on<type> handler runs with the non-capture listeners.
        var h = capture || event[STOP_NOW] ? null : t['on' + event.type];
        if (typeof h === 'function' && callListener(t, h, event) === false && event.cancelable) {
            event.defaultPrevented = true;
        }
    }
    // A node's parent, then document's is window (except for `load`, which
    // never reaches window from a node).
    function eventParent(n, event) {
        if (n === g) return null;
        if (n === doc) return event.type === 'load' ? null : g;
        return n[SLOT] ? n.parentNode : null;
    }
    EventTarget.prototype.dispatchEvent = function (event) {
        if (event == null || typeof event !== 'object' || event.type === undefined) {
            throw new TypeError("Failed to execute 'dispatchEvent' on 'EventTarget': " +
                "parameter 1 is not of type 'Event'.");
        }
        var t = thisTarget(this), path = [], i;
        for (var n = t; n; n = eventParent(n, event)) path.push(n);
        event.target = t;
        for (i = path.length - 1; i > 0 && !event[STOP]; i--) invoke(path[i], event, 1, true);
        if (!event[STOP]) invoke(t, event, 2, true);
        if (!event[STOP]) invoke(t, event, 2, false);
        if (event.bubbles) {
            for (i = 1; i < path.length && !event[STOP]; i++) invoke(path[i], event, 3, false);
        }
        event[STOP] = event[STOP_NOW] = false;
        event.currentTarget = null;
        event.eventPhase = 0;
        return !event.defaultPrevented;
    };

    function Event(type, init) {
        if (!(this instanceof Event)) {
            throw new TypeError("Failed to construct 'Event': Please use the 'new' operator.");
        }
        if (arguments.length < 1) {
            throw new TypeError("Failed to construct 'Event': 1 argument required, but only 0 present.");
        }
        init = init || {};
        this.type = String(type);
        this.bubbles = !!init.bubbles;
        this.cancelable = !!init.cancelable;
        this.composed = !!init.composed;
        this.defaultPrevented = false;
        this.target = null;
        this.currentTarget = null;
        this.eventPhase = 0;
        this.isTrusted = false;
        this.timeStamp = Date.now();
    }
    Event.prototype.preventDefault = function () { if (this.cancelable) this.defaultPrevented = true; };
    Event.prototype.stopPropagation = function () { this[STOP] = true; };
    Event.prototype.stopImmediatePropagation = function () { this[STOP] = this[STOP_NOW] = true; };
    getter(Event.prototype, 'srcElement', function () { return this.target; });
    Object.defineProperty(Event.prototype, Symbol.toStringTag, { value: 'Event' });
    ['NONE', 'CAPTURING_PHASE', 'AT_TARGET', 'BUBBLING_PHASE'].forEach(function (k, i) {
        Event[k] = Event.prototype[k] = i;
    });
    function CustomEvent(type, init) {
        if (!(this instanceof CustomEvent)) {
            throw new TypeError("Failed to construct 'CustomEvent': Please use the 'new' operator.");
        }
        Event.apply(this, arguments);
        this.detail = init && init.detail !== undefined ? init.detail : null;
    }
    CustomEvent.prototype = Object.create(Event.prototype, {
        constructor: { value: CustomEvent, writable: true, configurable: true }
    });
    Object.defineProperty(CustomEvent.prototype, Symbol.toStringTag, { value: 'CustomEvent' });
    g.Event = Event;
    g.CustomEvent = CustomEvent;
    HTMLElement.prototype.click = function () {
        this.dispatchEvent(new Event('click', { bubbles: true, cancelable: true }));
    };

    // The global `document` becomes the Document wrapper.
    var doc = g.document;
    Object.setPrototypeOf(doc, Document.prototype);
    // The stub's own factories would shadow the Rust-backed ones.
    var stubCreateElement = doc.createElement;
    delete doc.createElement;
    delete doc.createTextNode;
    // Document and window trade the lifecycle's per-object listener lists
    // for the shared EventTarget, so element events bubble up to them.
    ['addEventListener', 'removeEventListener', 'dispatchEvent'].forEach(function (k) {
        delete doc[k];
        g[k] = EventTarget.prototype[k];
    });

    g.__rustkit_dom_reset = function (newGen) {
        gen = newGen;
        cache = new Map();
        var root = N.root(gen, 'document');
        Object.defineProperty(doc, SLOT, { value: { id: root, gen: gen }, configurable: true });
        if (typeof root === 'number') cache.set(root, doc);
    };
    g.__rustkit_dom_reset(0);
})(globalThis);
"#;
