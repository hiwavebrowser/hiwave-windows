//! Element diagnostics: one compact, deterministic record per element.
//!
//! When a page renders wrong, the first two questions are WHICH element and
//! WHICH stage (DOM, style, layout, paint). This module answers them for one
//! element from what the engine already holds after a layout: the DOM, the
//! committed layout tree (every box carries its node id and computed style)
//! and the view's scroll and viewport. `tools/inspector` writes the same
//! record from Chromium and names the first element whose record differs.
//!
//! Read-only and on demand. Nothing here runs during layout or paint: the
//! per-box context (containing block, clip, opacity chain) is rebuilt from
//! the committed tree each time a record is asked for, so a page that asks
//! for no diagnostics pays nothing. It runs no script and borrows only the
//! view's last committed layout and DOM, so it can answer between script
//! turns even while the page has script work queued (see
//! `Engine::element_diagnostics`).
//!
//! A field the engine cannot answer is `null` with a reason under
//! `unavailable`, keyed by the field's path; nothing is silently omitted.
//! Maps are `BTreeMap`s so key order is stable; there are no timestamps.

use super::*;
use rustkit_css::{Display, GridLine, Length, Overflow, TrackSize};
use serde::Serialize;
use std::collections::BTreeMap;

/// Version tag carried by every record and dump.
pub const DIAGNOSTICS_SCHEMA: &str = "hiwave-element-diagnostics/1";

/// Default cap on the number of records in a whole-page dump.
pub const DEFAULT_MAX_ELEMENTS: usize = 5000;

/// The computed properties a record carries, in the names `getComputedStyle`
/// uses. `tools/inspector` reads the same list in Chromium.
pub const STYLE_PROPERTIES: &[&str] = &[
    "align-items", "align-self", "aspect-ratio", "background-color", "background-image",
    "border-bottom-width", "border-left-width", "border-right-width", "border-top-width",
    "bottom", "box-sizing", "clear", "color", "column-gap", "contain", "content-visibility",
    "display", "flex-basis", "flex-direction", "flex-grow", "flex-shrink", "flex-wrap",
    "float", "font-family", "font-size", "font-weight", "gap", "grid-auto-columns",
    "grid-auto-flow", "grid-auto-rows", "grid-column-end", "grid-column-start",
    "grid-row-end", "grid-row-start", "grid-template-columns", "grid-template-rows",
    "height", "inset", "justify-content", "justify-self", "left", "line-height",
    "margin-bottom", "margin-left", "margin-right", "margin-top", "max-height", "max-width",
    "min-height", "min-width", "object-fit", "opacity", "order", "overflow-x", "overflow-y",
    "padding-bottom", "padding-left", "padding-right", "padding-top", "position", "right",
    "row-gap", "text-align", "top", "transform", "vertical-align", "visibility",
    "white-space", "width", "z-index",
];

/// Which element to describe.
#[derive(Debug, Clone, PartialEq)]
pub enum DiagnosticTarget {
    /// A raw DOM `NodeId`.
    Node(usize),
    /// The first element in document order matching a CSS selector.
    Selector(String),
    /// The element hit at a point, in viewport CSS px.
    Point { x: f32, y: f32 },
}

/// How a record is built.
#[derive(Debug, Clone)]
pub struct DiagnosticsOptions {
    /// Keep the page URL's query and fragment (stripped by default: they can
    /// carry tokens).
    pub include_url_query: bool,
    /// Cap for `Engine::all_element_diagnostics`.
    pub max_elements: usize,
}

impl Default for DiagnosticsOptions {
    fn default() -> Self {
        Self { include_url_query: false, max_elements: DEFAULT_MAX_ELEMENTS }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DiagRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EngineBuild {
    pub version: String,
    /// `HIWAVE_BUILD_COMMIT` at compile time, when the build set it.
    pub commit: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PageInfo {
    pub url: Option<String>,
    pub engine: EngineBuild,
    pub viewport: (u32, u32),
    pub device_scale: Option<f32>,
    pub scroll: (f32, f32),
    /// False on builds whose text shaper is the fixed-advance stub (Linux):
    /// every text-sized box is then measured against a ruler, not a font.
    pub text_metrics_font_derived: bool,
    /// Why a page field above is null.
    pub unavailable: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Identity {
    pub node_id: usize,
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// Unique structural selector, `html > body > div:nth-child(2) > p`.
    pub selector: String,
    /// The parity oracle's join key (`getSelector` in capture_baseline.mjs).
    pub oracle_key: String,
    /// `tag#id.class` entries, nearest ancestor first, up to `html`.
    pub ancestry: Vec<String>,
    /// Index among the parent's element children; `None` for the root.
    pub child_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TreeState {
    pub in_dom: bool,
    pub connected: bool,
    pub in_shadow_tree: Option<bool>,
    pub shadow_host: Option<String>,
    pub assigned_slot: Option<String>,
    /// Element children (`children.length`).
    pub child_count: usize,
    /// `textContent.length`, in UTF-16 units. Text itself is never reported.
    pub text_length: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContainingBlock {
    /// Selector of the containing block's element; `None` = the viewport.
    pub selector: Option<String>,
    pub content_box: DiagRect,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct BoxInfo {
    pub has_box: bool,
    pub no_box_reason: Option<String>,
    /// Viewport coordinates, as `getBoundingClientRect`.
    pub border_box: Option<DiagRect>,
    pub padding_box: Option<DiagRect>,
    pub content_box: Option<DiagRect>,
    pub containing_block: Option<ContainingBlock>,
    /// `block`, `inline`, `float`, `flex item of <sel>`, `grid item of <sel>`,
    /// `table cell`, `absolute relative to <sel|viewport>`, `fixed`.
    pub formatting_context: Option<String>,
    /// Placed `[column-start, column-end, row-start, row-end]` of a grid item.
    pub grid_lines: Option<[i32; 4]>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PaintInfo {
    pub display_commands: Option<usize>,
    pub command_kinds: Option<Vec<String>>,
    pub clipped: Option<bool>,
    pub clipped_by: Option<String>,
    pub outside_viewport: Option<bool>,
    pub effective_opacity: Option<f32>,
    pub in_opacity_layer: Option<bool>,
    pub in_transform_layer: Option<bool>,
}

/// One element's record. Serialises to well under 4 KB of JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ElementDiagnostics {
    pub schema: &'static str,
    /// Hoisted to the dump's top level in `all_element_diagnostics`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<PageInfo>,
    pub identity: Identity,
    pub tree: TreeState,
    pub style: BTreeMap<&'static str, Option<String>>,
    #[serde(rename = "box")]
    pub layout_box: BoxInfo,
    pub paint: PaintInfo,
    pub notes: Vec<String>,
    pub unavailable: BTreeMap<String, String>,
}

/// Every element of a page (`Engine::all_element_diagnostics`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiagnosticsDump {
    pub schema: &'static str,
    pub page: PageInfo,
    pub total_elements: usize,
    pub truncated: bool,
    pub elements: Vec<ElementDiagnostics>,
}

// ---------------------------------------------------------------------------
// The layout index: built on demand from the committed tree.
// ---------------------------------------------------------------------------

/// What a box's ancestors decide about it.
struct BoxContext<'a> {
    boxes: Vec<&'a LayoutBox>,
    /// The nearest layout ancestor (possibly anonymous).
    parent: Option<&'a LayoutBox>,
    /// `None` = the initial containing block.
    containing_block: Option<&'a LayoutBox>,
    clipped_by: Option<&'a LayoutBox>,
    opacity: f32,
    opacity_layer: bool,
    transform_layer: bool,
    /// Page-space affine of this box's own and its ancestors' transforms
    /// (the painter's, `own_transform_affine`); `None` = untransformed.
    affine: Option<[f32; 6]>,
    /// The nearest ancestor element box that scrolls its overflow.
    scroller: Option<&'a LayoutBox>,
}

#[derive(Clone, Copy)]
struct Inherited<'a> {
    parent: Option<&'a LayoutBox>,
    block_cb: Option<&'a LayoutBox>,
    positioned_cb: Option<&'a LayoutBox>,
    opacity: f32,
    opacity_layer: bool,
    transform_layer: bool,
    affine: Option<[f32; 6]>,
    scroller: Option<&'a LayoutBox>,
}

/// Clip ancestors, nearest first, as a stack-allocated list.
struct ClipLink<'a, 'b> {
    clip: &'a LayoutBox,
    up: Option<&'b ClipLink<'a, 'b>>,
}

fn clips(b: &LayoutBox) -> bool {
    b.style.overflow_x != Overflow::Visible || b.style.overflow_y != Overflow::Visible
}

fn is_block_container(b: &LayoutBox) -> bool {
    !matches!(b.box_type, BoxType::Inline | BoxType::Text(_) | BoxType::LineBreak)
}

fn contains(outer: &Rect, inner: &Rect) -> bool {
    const EPS: f32 = 0.01;
    inner.x >= outer.x - EPS
        && inner.y >= outer.y - EPS
        && inner.x + inner.width <= outer.x + outer.width + EPS
        && inner.y + inner.height <= outer.y + outer.height + EPS
}

fn index_layout<'a>(root: &'a LayoutBox) -> HashMap<usize, BoxContext<'a>> {
    fn walk<'a>(
        b: &'a LayoutBox,
        inh: Inherited<'a>,
        clip: Option<&ClipLink<'a, '_>>,
        out: &mut HashMap<usize, BoxContext<'a>>,
    ) {
        let s = &b.style;
        let opacity = inh.opacity * s.opacity;
        let transformed = !s.effective_transform().is_identity();
        let opacity_layer = inh.opacity_layer || s.opacity < 1.0;
        let transform_layer = inh.transform_layer || transformed;
        let affine = match (inh.affine, own_transform_affine(b)) {
            (Some(outer), Some(own)) => Some(compose_affine(outer, own)),
            (outer, own) => outer.or(own),
        };
        if let Some(node) = b.node_id {
            match out.get_mut(&node) {
                Some(ctx) => ctx.boxes.push(b),
                None => {
                    let containing_block = match s.position {
                        rustkit_css::Position::Fixed => None,
                        rustkit_css::Position::Absolute => inh.positioned_cb,
                        _ => inh.block_cb,
                    };
                    let bb = b.dimensions.border_box();
                    let mut clipped_by = None;
                    let mut link = clip;
                    while let Some(l) = link {
                        if !contains(&l.clip.dimensions.padding_box(), &bb) {
                            clipped_by = Some(l.clip);
                            break;
                        }
                        link = l.up;
                    }
                    out.insert(
                        node,
                        BoxContext {
                            boxes: vec![b],
                            parent: inh.parent,
                            containing_block,
                            clipped_by,
                            opacity,
                            opacity_layer,
                            transform_layer,
                            affine,
                            scroller: inh.scroller,
                        },
                    );
                }
            }
        }
        let element = b.node_id.is_some();
        let next = Inherited {
            parent: Some(b),
            block_cb: if element && is_block_container(b) { Some(b) } else { inh.block_cb },
            positioned_cb: if element && (s.position != rustkit_css::Position::Static || transformed) {
                Some(b)
            } else {
                inh.positioned_cb
            },
            opacity,
            opacity_layer,
            transform_layer,
            affine,
            scroller: if element && (s.overflow_x.is_scrollable() || s.overflow_y.is_scrollable()) {
                Some(b)
            } else {
                inh.scroller
            },
        };
        let own = ClipLink { clip: b, up: clip };
        let child_clip = if element && clips(b) { Some(&own) } else { clip };
        for child in &b.children {
            walk(child, next, child_clip, out);
        }
    }
    let mut out = HashMap::new();
    let inh = Inherited {
        parent: None,
        block_cb: None,
        positioned_cb: None,
        opacity: 1.0,
        opacity_layer: false,
        transform_layer: false,
        affine: None,
        scroller: None,
    };
    walk(root, inh, None, &mut out);
    out
}

// ---------------------------------------------------------------------------
// Building records
// ---------------------------------------------------------------------------

/// Borrowed state one record is built from.
pub(crate) struct DiagnosticsSource<'a> {
    pub document: &'a Document,
    pub layout: Option<&'a LayoutBox>,
    pub scroll: (f32, f32),
    pub viewport: (u32, u32),
    pub checked: std::cell::Ref<'a, HashMap<usize, bool>>,
}

struct Builder<'a> {
    src: &'a DiagnosticsSource<'a>,
    index: HashMap<usize, BoxContext<'a>>,
    text_len: HashMap<usize, usize>,
    /// The `html` element, when its geometry is the anonymous layout root's.
    root_stand_in: Option<usize>,
    sibs: Siblings,
}

fn element_parts(node: &Node) -> Option<(String, &HashMap<String, String>)> {
    match &node.node_type {
        NodeType::Element { tag_name, attributes, .. } => Some((tag_name.to_lowercase(), attributes)),
        _ => None,
    }
}

fn element_children(node: &Node) -> Vec<Rc<Node>> {
    node.children().into_iter().filter(|c| matches!(c.node_type, NodeType::Element { .. })).collect()
}

fn parent_element(node: &Node) -> Option<Rc<Node>> {
    node.parent().filter(|p| matches!(p.node_type, NodeType::Element { .. }))
}

fn classes_of(attrs: &HashMap<String, String>) -> Vec<String> {
    attrs.get("class").map(|c| c.split_whitespace().map(str::to_string).collect()).unwrap_or_default()
}

/// `tag#id.class.class`, as DevTools' breadcrumb writes it.
fn short_label(node: &Node) -> String {
    let Some((tag, attrs)) = element_parts(node) else { return String::new() };
    let mut out = tag;
    if let Some(id) = attrs.get("id").filter(|i| !i.is_empty()) {
        out.push('#');
        out.push_str(id);
    }
    for c in classes_of(attrs) {
        out.push('.');
        out.push_str(&c);
    }
    out
}

/// Where an element sits among its parent's element children.
#[derive(Clone, Copy)]
struct SiblingPos {
    index: usize,
    count: usize,
    /// 1-based among same-tag siblings, and how many there are.
    of_type: usize,
    of_type_count: usize,
}

/// Every connected element's `SiblingPos`, listed once per parent. Asking a
/// parent for its children per record made a whole-page dump quadratic in
/// the sibling count (20k siblings: 12 s).
#[derive(Default)]
struct Siblings(HashMap<usize, SiblingPos>);

impl Siblings {
    fn of_tree(root: &Rc<Node>) -> Self {
        let mut map = HashMap::new();
        let mut stack = vec![root.clone()];
        while let Some(n) = stack.pop() {
            let kids = element_children(&n);
            Self::fill(&kids, &mut map);
            stack.extend(n.children());
        }
        Siblings(map)
    }

    fn fill(kids: &[Rc<Node>], map: &mut HashMap<usize, SiblingPos>) {
        let tags: Vec<String> = kids.iter().map(|k| element_parts(k).map(|(t, _)| t).unwrap_or_default()).collect();
        let mut per_tag: HashMap<&str, usize> = HashMap::new();
        for t in &tags {
            *per_tag.entry(t).or_default() += 1;
        }
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for (i, (k, t)) in kids.iter().zip(&tags).enumerate() {
            let nth = seen.entry(t).or_default();
            *nth += 1;
            map.insert(
                k.id.raw(),
                SiblingPos { index: i, count: kids.len(), of_type: *nth, of_type_count: per_tag[t.as_str()] },
            );
        }
    }

    /// `None` for an element without a parent node.
    fn of(&self, n: &Rc<Node>) -> Option<SiblingPos> {
        if let Some(p) = self.0.get(&n.id.raw()) {
            return Some(*p);
        }
        // Not in the connected tree (removed by script): list its parent now.
        let mut map = HashMap::new();
        let parent = n.parent()?;
        Self::fill(&element_children(&parent), &mut map);
        map.get(&n.id.raw()).copied()
    }
}

/// Unique structural selector: tag and `:nth-child(n)` when the element has
/// element siblings, from the root element down.
fn structural_selector(node: &Rc<Node>, sibs: &Siblings) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        let Some((tag, _)) = element_parts(&n) else { break };
        let seg = match sibs.of(&n) {
            Some(p) if p.count > 1 => format!("{tag}:nth-child({})", p.index + 1),
            _ => tag,
        };
        parts.push(seg);
        cur = parent_element(&n);
    }
    parts.reverse();
    parts.join(" > ")
}

/// The oracle's `getSelector` key, through the same segment builder the
/// layout identity uses, for an element that has no box to carry one.
fn oracle_key(node: &Rc<Node>, sibs: &Siblings) -> String {
    let mut parts = Vec::new();
    let mut cur = Some(node.clone());
    let mut foreign = false;
    // Foreign content is decided top-down; find whether any ancestor is svg/math.
    let mut a = Some(node.clone());
    while let Some(n) = a {
        if let Some((tag, _)) = element_parts(&n) {
            if Engine::enters_foreign_content(&tag) {
                foreign = true;
            }
        }
        a = parent_element(&n);
    }
    while let Some(n) = cur {
        let Some((tag, attrs)) = element_parts(&n) else { break };
        let (index, total) = sibs.of(&n).map_or((1, 1), |p| (p.of_type, p.of_type_count));
        parts.push(Engine::selector_segment(&tag, attrs, index, total, foreign));
        if Engine::enters_foreign_content(&tag) {
            foreign = false;
        }
        let parent = parent_element(&n);
        if parent.as_ref().and_then(|p| element_parts(p)).is_some_and(|(t, _)| t == "body") {
            parts.push("body".into());
            break;
        }
        cur = parent;
    }
    parts.reverse();
    let path = parts.join(" > ");
    match element_parts(node) {
        Some((_, attrs)) => Engine::reported_selector(&path, attrs),
        None => path,
    }
}

fn text_lengths(root: &Rc<Node>, out: &mut HashMap<usize, usize>) -> usize {
    let len = match &root.node_type {
        NodeType::Text(t) => t.encode_utf16().count(),
        NodeType::Comment(_) | NodeType::ProcessingInstruction { .. } => 0,
        _ => root.children().iter().map(|c| text_lengths(c, out)).sum(),
    };
    if matches!(root.node_type, NodeType::Element { .. }) {
        out.insert(root.id.raw(), len);
    }
    len
}

fn to_viewport(r: Rect, scroll: (f32, f32)) -> DiagRect {
    let round = |v: f32| (v * 100.0).round() / 100.0 + 0.0;
    DiagRect { x: round(r.x - scroll.0), y: round(r.y - scroll.1), w: round(r.width), h: round(r.height) }
}

fn union(a: Rect, b: Rect) -> Rect {
    let (l, t) = (a.x.min(b.x), a.y.min(b.y));
    let (r, bt) = ((a.x + a.width).max(b.x + b.width), (a.y + a.height).max(b.y + b.height));
    Rect { x: l, y: t, width: r - l, height: bt - t }
}

/// html's own box. The layout root that stands in for html is the viewport
/// (the initial containing block); html is as wide as the root and as tall
/// as its in-flow content. Root margins do not collapse, so that content
/// ends at its children's bottom margin edges, each with the margins that
/// collapse through its open bottom edge (`collapsed_bottom`); html is a
/// formatting context root, so floats in it count. The engine lays html out
/// with no margin, border or padding.
fn html_box(
    root: &LayoutBox,
    scroll: (f32, f32),
    viewport: (u32, u32),
    layout_box: &mut BoxInfo,
    paint: &mut PaintInfo,
    notes: &mut Vec<String>,
) {
    let top = root.dimensions.content.y;
    let bottom = root
        .children
        .iter()
        .filter(|c| !matches!(c.style.position, rustkit_css::Position::Absolute | rustkit_css::Position::Fixed))
        .map(collapsed_bottom)
        .fold(top, f32::max);
    let bottom = root.children.iter().fold(bottom, |b, c| float_bottom(c, b));
    let content = root.dimensions.content;
    let r = to_viewport(Rect { x: content.x, y: top, width: content.width, height: bottom - top }, scroll);
    layout_box.border_box = Some(r);
    layout_box.padding_box = Some(r);
    layout_box.content_box = Some(r);
    let (vw, vh) = (viewport.0 as f32, viewport.1 as f32);
    paint.outside_viewport = Some(r.x + r.w <= 0.0 || r.y + r.h <= 0.0 || r.x >= vw || r.y >= vh);
    notes.push("html's box: the layout root's width and its in-flow content's height (the root itself is the viewport); the engine applies none of html's own margin, border, padding or height".into());
}

/// The bottom margin edge of an in-flow block as its BFC root sees it: its
/// border box's bottom plus its own bottom margin collapsed (CSS 2.1 §8.3.1)
/// with its last in-flow child's, and so on down, while each bottom edge on
/// the way is open: no bottom border or padding, `height: auto`, no
/// `min-height`, not a formatting root. The layout keeps no collapsed value,
/// so this walks the same tests it uses (`should_collapse_with_last_child`,
/// `establishes_bfc`).
fn collapsed_bottom(b: &LayoutBox) -> f32 {
    use rustkit_layout::{establishes_bfc, should_collapse_with_last_child, CollapsibleMargin};
    let border_bottom = b.dimensions.margin_box().y + b.dimensions.margin_box().height - b.dimensions.margin.bottom;
    let mut margin = CollapsibleMargin::from_margin(b.dimensions.margin.bottom);
    let mut node = b;
    while should_collapse_with_last_child(&node.style, node.float, node.dimensions.border.bottom, node.dimensions.padding.bottom) {
        // Out-of-flow boxes take no part; a line box ends the chain.
        let last = node.children.iter().rev().find(|c| {
            !matches!(c.style.position, rustkit_css::Position::Absolute | rustkit_css::Position::Fixed)
                && c.float == rustkit_layout::Float::None
        });
        let Some(child) = last else { break };
        if !matches!(child.box_type, BoxType::Block) || child.style.display.is_atomic_inline() {
            break;
        }
        // A box that `max-height` clamps below its content closes its bottom
        // edge, as Chromium does; the layout's test does not look at it.
        let content = node.dimensions.content;
        let child_border = child.dimensions.border_box();
        if child_border.y + child_border.height > content.y + content.height + 0.5 {
            break;
        }
        margin.adjoin(child.dimensions.margin.bottom);
        // A formatting root adjoins its own margin, but nothing collapses through it.
        if establishes_bfc(&child.style, child.float) {
            break;
        }
        node = child;
    }
    border_bottom + margin.resolve()
}

/// The lowest float margin edge in the root's formatting context: floats
/// inside a box that is not itself a formatting root overflow into it.
fn float_bottom(b: &LayoutBox, bottom: f32) -> f32 {
    if matches!(b.style.position, rustkit_css::Position::Absolute | rustkit_css::Position::Fixed) {
        return bottom;
    }
    if b.float != rustkit_layout::Float::None {
        let m = b.dimensions.margin_box();
        return bottom.max(m.y + m.height);
    }
    if rustkit_layout::establishes_bfc(&b.style, b.float) {
        return bottom;
    }
    b.children.iter().fold(bottom, |acc, c| float_bottom(c, acc))
}

/// The page URL as a record reports it: never a user name or password, and
/// no query, fragment or `;` path parameters (`;jsessionid=`) unless asked.
/// Known limit: a session ID in a whole path segment (ASP.NET cookieless
/// sessions, `/(S(…))/page.aspx`) is not recognised and stays. A `data:` URL's payload is the
/// document itself, so it is reduced to `data:<mime>` even then.
pub(crate) fn report_url(url: &Url, include_query: bool) -> String {
    match url.scheme() {
        // With no comma there is no media type, only payload.
        "data" => {
            let mime = url.path().split_once(',').map_or("", |(m, _)| m.split(';').next().unwrap_or("").trim());
            return format!("data:{mime}");
        }
        // A script URL is all payload.
        "javascript" => return "javascript:".into(),
        // `set_username` cannot reach the userinfo inside a blob URL: report
        // only its origin (no userinfo, no UUID).
        "blob" => {
            return match url.origin() {
                o @ url::Origin::Tuple(..) => format!("blob:{}", o.ascii_serialization()),
                url::Origin::Opaque(_) => "blob:".into(),
            }
        }
        _ => {}
    }
    let mut u = url.clone();
    let _ = u.set_username("");
    let _ = u.set_password(None);
    if !include_query {
        u.set_query(None);
        u.set_fragment(None);
        // The parser's path: `\` already reads as `/` for a special scheme.
        let path = drop_path_params(u.path(), false);
        u.set_path(&path);
    }
    u.to_string()
}

/// A path without its `;` parameters (`/cart;jsessionid=S/x` is `/cart/x`):
/// a servlet puts the session cookie there when cookies are off. Each one
/// runs from `;` (or `%3B`, to record less) to the end of its segment.
/// The query and fragment must already be cut. `backslash`: `\` also ends a
/// segment, as it does for a special scheme or a relative URL.
fn drop_path_params(path: &str, backslash: bool) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while !rest.is_empty() {
        let at = rest.find(';').into_iter().chain(rest.find("%3B")).chain(rest.find("%3b")).min();
        let Some(at) = at else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find(|c: char| c == '/' || (backslash && c == '\\')).unwrap_or(rest.len());
        rest = &rest[end..];
    }
    out
}

impl<'a> Builder<'a> {
    fn new(src: &'a DiagnosticsSource<'a>) -> Self {
        let mut index = src.layout.map(index_layout).unwrap_or_default();
        // The layout root is an anonymous stand-in for `html`: its geometry
        // is html's, its style is the default (html's own only feeds
        // inheritance), so html is given the root's box but no style.
        let mut root_stand_in = None;
        if let (Some(root), Some(html)) = (src.layout.filter(|r| r.node_id.is_none()), src.document.document_element()) {
            let raw = html.id.raw();
            if let std::collections::hash_map::Entry::Vacant(e) = index.entry(raw) {
                e.insert(BoxContext {
                    boxes: vec![root],
                    parent: None,
                    containing_block: None,
                    clipped_by: None,
                    opacity: 1.0,
                    opacity_layer: false,
                    transform_layer: false,
                    affine: None,
                    scroller: None,
                });
                root_stand_in = Some(raw);
            }
        }
        let mut text_len = HashMap::new();
        text_lengths(src.document.root(), &mut text_len);
        let sibs = Siblings::of_tree(src.document.root());
        Self { src, index, text_len, root_stand_in, sibs }
    }

    fn selector_of_node(&self, raw: usize) -> Option<String> {
        self.src.document.get_node(rustkit_dom::NodeId::new(raw)).map(|n| structural_selector(&n, &self.sibs))
    }

    fn record(&self, node: &Rc<Node>) -> Option<ElementDiagnostics> {
        let (tag, attrs) = element_parts(node)?;
        let raw = node.id.raw();
        let mut unavailable = BTreeMap::new();
        let mut notes = Vec::new();
        let ctx = self.index.get(&raw);

        let mut connected = false;
        let mut root = node.clone();
        while let Some(p) = root.parent() {
            root = p;
        }
        if matches!(root.node_type, NodeType::Document) && Rc::ptr_eq(&root, self.src.document.root()) {
            connected = true;
        }
        let parent = parent_element(node);
        let identity = Identity {
            node_id: raw,
            tag: tag.clone(),
            id: attrs.get("id").filter(|i| !i.is_empty()).cloned(),
            classes: classes_of(attrs),
            selector: structural_selector(node, &self.sibs),
            oracle_key: ctx
                .and_then(|c| c.boxes[0].identity().map(|i| i.selector.clone()))
                .unwrap_or_else(|| oracle_key(node, &self.sibs)),
            ancestry: {
                let mut v = Vec::new();
                let mut cur = parent.clone();
                while let Some(p) = cur {
                    v.push(short_label(&p));
                    cur = parent_element(&p);
                }
                v
            },
            child_index: self.sibs.of(node).map(|p| p.index),
        };

        const SHADOW: &str = "shadow trees and slots live in script only (web_shadow.js)";
        let tree = TreeState {
            // Reachable from the document: false for an element script removed.
            in_dom: connected,
            connected,
            in_shadow_tree: if connected { Some(false) } else { None },
            shadow_host: None,
            assigned_slot: None,
            child_count: element_children(node).len(),
            text_length: self.text_len.get(&raw).copied().unwrap_or_else(|| text_lengths(node, &mut HashMap::new())),
        };
        if !connected {
            unavailable.insert("tree.in_shadow_tree".into(), SHADOW.into());
        }
        unavailable.insert("tree.shadow_host".into(), SHADOW.into());
        unavailable.insert("tree.assigned_slot".into(), SHADOW.into());

        const NO_ATTRIBUTION: &str = "display-list commands carry no node id";
        unavailable.insert("paint.display_commands".into(), NO_ATTRIBUTION.into());
        unavailable.insert("paint.command_kinds".into(), NO_ATTRIBUTION.into());

        let (style, layout_box, paint) = match ctx {
            Some(ctx) => {
                let (mut style, mut layout_box, mut paint) = self.boxed(ctx, &mut notes, &mut unavailable);
                if tag == "body" && self.body_background_moved(ctx.boxes[0]) {
                    for p in ["background-color", "background-image"] {
                        style.insert(p, None);
                        unavailable.insert(
                            format!("style.{p}"),
                            "moved to the canvas (CSS 2.1 §14.2); the body box keeps none".into(),
                        );
                    }
                }
                if self.root_stand_in == Some(raw) {
                    html_box(ctx.boxes[0], self.src.scroll, self.src.viewport, &mut layout_box, &mut paint, &mut notes);
                    let standards = self.src.document.root().children().iter().any(|c| {
                        matches!(&c.node_type, rustkit_dom::NodeType::DocumentType { name, .. } if name.eq_ignore_ascii_case("html"))
                    });
                    if !standards {
                        notes.push("no <!DOCTYPE html>: Chromium lays this page out in quirks mode, where html fills the viewport; the engine has no quirks mode".into());
                    }
                    style.values_mut().for_each(|v| *v = None);
                    unavailable.retain(|k, _| !k.starts_with("style."));
                    unavailable.insert("style".into(), "html is laid out by an anonymous root box with the default style".into());
                }
                (style, layout_box, paint)
            }
            None => {
                let reason = self.no_box_reason(node, &tag, connected);
                let style: BTreeMap<&'static str, Option<String>> =
                    STYLE_PROPERTIES.iter().map(|p| (*p, None)).collect();
                // No box does not mean display:none: the box tree builder also
                // drops elements it does not render (an empty span, an option),
                // and it keeps no style for either. Unknown, not a guess.
                unavailable.insert("style".into(), "no layout box: the engine keeps computed style only on boxes".into());
                unavailable.insert(
                    "style.display".into(),
                    "no layout box: display:none and an element the box tree builder dropped look the same".into(),
                );
                for f in ["box", "paint"] {
                    unavailable.insert(f.into(), "no layout box".into());
                }
                (style, BoxInfo { no_box_reason: Some(reason), ..Default::default() }, PaintInfo::default())
            }
        };

        Some(ElementDiagnostics {
            schema: DIAGNOSTICS_SCHEMA,
            page: None,
            identity,
            tree,
            style,
            layout_box,
            paint,
            notes,
            unavailable,
        })
    }

    /// The engine transfers body's background to the root box when html's
    /// is transparent and clears it from body, so body's own is not kept.
    fn body_background_moved(&self, body: &LayoutBox) -> bool {
        let has = |s: &rustkit_css::ComputedStyle| {
            s.background_color.a > 0.0 || s.background_gradient.is_some() || !s.background_layers.is_empty()
        };
        self.src.layout.is_some_and(|root| root.node_id.is_none() && has(&root.style)) && !has(&body.style)
    }

    fn no_box_reason(&self, node: &Rc<Node>, tag: &str, connected: bool) -> String {
        if !connected {
            return "not connected to the document".into();
        }
        if self.src.layout.is_none() {
            return "the view has no layout yet".into();
        }
        if matches!(tag, "html") {
            return "the layout root is an anonymous stand-in for html; html's own box is not built".into();
        }
        if matches!(tag, "head" | "script" | "style" | "meta" | "link" | "title" | "template" | "noscript" | "base") {
            return format!("not rendered: {tag} is display:none in the UA sheet");
        }
        let parent = parent_element(node);
        let mut cur = parent.clone();
        let mut svg = false;
        while let Some(p) = cur {
            svg |= element_parts(&p).is_some_and(|(t, _)| t == "svg");
            if self.index.contains_key(&p.id.raw()) {
                return if svg {
                    "SVG content: painted by the <svg> box, which builds no CSS box for it".into()
                } else if parent.as_ref().is_some_and(|q| q.id == p.id) {
                    "display:none, or dropped by the box tree builder".into()
                } else {
                    format!("an ancestor has no box (nearest boxed ancestor: {})", structural_selector(&p, &self.sibs))
                };
            }
            cur = parent_element(&p);
        }
        "no ancestor has a box".into()
    }

    fn boxed(
        &self,
        ctx: &BoxContext<'a>,
        notes: &mut Vec<String>,
        unavailable: &mut BTreeMap<String, String>,
    ) -> (BTreeMap<&'static str, Option<String>>, BoxInfo, PaintInfo) {
        let b = ctx.boxes[0];
        let scroll = self.src.scroll;
        let (mut border, mut padding, mut content) =
            (b.dimensions.border_box(), b.dimensions.padding_box(), b.dimensions.content);
        for other in &ctx.boxes[1..] {
            border = union(border, other.dimensions.border_box());
            padding = union(padding, other.dimensions.padding_box());
            content = union(content, other.dimensions.content);
        }
        if ctx.boxes.len() > 1 {
            notes.push(format!("laid out as {} boxes; rects are their union", ctx.boxes.len()));
        }
        let mut style = style_record(b, unavailable);
        let position = b.style.position;
        if position == rustkit_css::Position::Fixed && scroll != (0.0, 0.0) {
            notes.push("fixed box: viewport rect is layout minus scroll, as getBoundingClientRect here".into());
        }

        let selector_of = |l: &LayoutBox| l.node_id.and_then(|n| self.selector_of_node(n));
        let containing_block = Some(match ctx.containing_block {
            Some(cb) => ContainingBlock { selector: selector_of(cb), content_box: to_viewport(cb.dimensions.content, scroll) },
            None => {
                let (w, h) = self.src.viewport;
                ContainingBlock { selector: None, content_box: DiagRect { x: 0.0, y: 0.0, w: w as f32, h: h as f32 } }
            }
        });
        insets(&mut style, b, ctx, scroll, self.src.viewport, unavailable);
        let parent_display = ctx.parent.map(|p| p.style.display);
        // min-width/min-height: auto computes to 0 except on flex and grid
        // items, where getComputedStyle keeps `auto`.
        if !parent_display.is_some_and(|d| d.is_flex() || matches!(d, Display::Grid | Display::InlineGrid)) {
            for p in ["min-width", "min-height"] {
                if style.get(p).and_then(|v| v.as_deref()) == Some("auto") {
                    style.insert(p, Some("0px".into()));
                }
            }
        }
        let parent_sel = || ctx.parent.and_then(selector_of).unwrap_or_else(|| "anonymous".into());
        let mut grid_item = false;
        let fc = match position {
            rustkit_css::Position::Fixed => "fixed".to_string(),
            rustkit_css::Position::Absolute => format!(
                "absolute relative to {}",
                ctx.containing_block.and_then(selector_of).unwrap_or_else(|| "viewport".into())
            ),
            _ if b.style.float != rustkit_css::Float::None => "float".into(),
            _ if b.style.display == Display::TableCell => "table cell".into(),
            _ if parent_display.is_some_and(|d| d.is_flex()) => format!("flex item of {}", parent_sel()),
            _ if parent_display.is_some_and(|d| matches!(d, Display::Grid | Display::InlineGrid)) => {
                grid_item = true;
                format!("grid item of {}", parent_sel())
            }
            _ if matches!(b.box_type, BoxType::Inline | BoxType::Text(_))
                || matches!(
                    b.style.display,
                    Display::Inline | Display::InlineBlock | Display::InlineFlex | Display::InlineGrid | Display::InlineTable
                ) =>
            {
                "inline".into()
            }
            _ => "block".into(),
        };
        if grid_item {
            unavailable.insert("box.grid_lines".into(), "grid placement is not kept on the box".into());
        }
        if position == rustkit_css::Position::Absolute && ctx.clipped_by.is_some() {
            notes.push("clip ancestry does not model an abspos box escaping a non-positioned clip".into());
        }

        // getBoundingClientRect is the bounding box of the transformed
        // border box. Padding and content boxes stay rects only while every
        // transform on the chain keeps axes (translate, scale).
        let (mut padding_box, mut content_box) = (Some(padding), Some(content));
        if let Some(m) = ctx.affine {
            let map = |r: Rect| {
                let (x, y, w, h) = transformed_bounds(m, r.x, r.y, r.width, r.height);
                Rect { x, y, width: w, height: h }
            };
            border = map(border);
            if m[1].abs() < 1e-6 && m[2].abs() < 1e-6 {
                padding_box = Some(map(padding));
                content_box = Some(map(content));
            } else {
                padding_box = None;
                content_box = None;
                for f in ["box.padding_box", "box.content_box"] {
                    unavailable.insert(f.into(), "not a rect under a rotating or skewing transform".into());
                }
            }
            notes.push("containing block and clip ancestry are computed on untransformed boxes".into());
        }
        // body's and html's overflow scroll the viewport, which is applied.
        if ctx.scroller.is_some_and(|sc| sc.element_scroll.get() != (0.0, 0.0)) {
            notes.push("inside a scrolled container: this is the position before its element scroll".into());
        }
        let viewport_border = to_viewport(border, scroll);
        let (vw, vh) = (self.src.viewport.0 as f32, self.src.viewport.1 as f32);
        let outside = viewport_border.x + viewport_border.w <= 0.0
            || viewport_border.y + viewport_border.h <= 0.0
            || viewport_border.x >= vw
            || viewport_border.y >= vh;
        let layout_box = BoxInfo {
            has_box: true,
            border_box: Some(viewport_border),
            padding_box: padding_box.map(|r| to_viewport(r, scroll)),
            content_box: content_box.map(|r| to_viewport(r, scroll)),
            containing_block,
            formatting_context: Some(fc),
            ..Default::default()
        };
        let paint = PaintInfo {
            clipped: Some(ctx.clipped_by.is_some()),
            clipped_by: ctx.clipped_by.and_then(selector_of),
            outside_viewport: Some(outside),
            effective_opacity: Some((ctx.opacity * 1000.0).round() / 1000.0),
            in_opacity_layer: Some(ctx.opacity_layer),
            in_transform_layer: Some(ctx.transform_layer),
            ..Default::default()
        };
        (style, layout_box, paint)
    }
}

// ---------------------------------------------------------------------------
// Computed values, serialised as getComputedStyle does
// ---------------------------------------------------------------------------

fn num(v: f32) -> String {
    let r = (v * 1000.0).round() / 1000.0 + 0.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// A specified length that has not been resolved against a box: absolute
/// and font/viewport-relative lengths in px, percentages as written.
fn length(l: &Length, none_for_auto: bool, font_px: f32, viewport: (f32, f32)) -> String {
    match l {
        Length::Auto if none_for_auto => "none".into(),
        Length::Auto => "auto".into(),
        Length::Zero => "0px".into(),
        Length::Px(p) => css_px(*p),
        Length::Percent(p) => format!("{}%", num(*p)),
        Length::FitContent => "fit-content".into(),
        other => css_px(other.to_px_with_viewport(font_px, 16.0, 0.0, viewport.0, viewport.1)),
    }
}

fn track(t: &TrackSize) -> String {
    match t {
        TrackSize::Px(p) => css_px(*p),
        TrackSize::Percent(p) => format!("{}%", num(*p)),
        TrackSize::Fr(f) => format!("{}fr", num(*f)),
        TrackSize::MinContent => "min-content".into(),
        TrackSize::MaxContent => "max-content".into(),
        TrackSize::Auto => "auto".into(),
        TrackSize::MinMax(a, b) => format!("minmax({}, {})", track(a), track(b)),
        TrackSize::FitContent(p) => format!("fit-content({})", css_px(*p)),
    }
}

fn template(t: &rustkit_css::GridTemplate) -> String {
    if t.tracks.is_empty() && t.repeats.is_empty() {
        return "none".into();
    }
    let mut parts: Vec<String> = t.tracks.iter().map(|d| track(&d.size)).collect();
    for (at, rep) in t.repeats.iter().rev() {
        let (count, defs) = match rep {
            rustkit_css::TrackRepeat::Count(n, d) => (n.to_string(), d),
            rustkit_css::TrackRepeat::AutoFill(d) => ("auto-fill".to_string(), d),
            rustkit_css::TrackRepeat::AutoFit(d) => ("auto-fit".to_string(), d),
        };
        let inner: Vec<String> = defs.iter().map(|d| track(&d.size)).collect();
        parts.insert((*at).min(parts.len()), format!("repeat({count}, {})", inner.join(" ")));
    }
    parts.join(" ")
}

fn grid_line(l: &GridLine) -> String {
    match l {
        GridLine::Auto => "auto".into(),
        GridLine::Number(n) => n.to_string(),
        GridLine::Name(s) => s.clone(),
        GridLine::Span(n) => format!("span {n}"),
        GridLine::SpanName(s) => format!("span {s}"),
    }
}

/// `margin`-style shorthand collapsing of four sides.
fn four(t: String, r: String, b: String, l: String) -> String {
    if r == l {
        if t == b {
            if t == r { t } else { format!("{t} {r}") }
        } else {
            format!("{t} {r} {b}")
        }
    } else {
        format!("{t} {r} {b} {l}")
    }
}

/// `url("…")` without the query, `data:` contents elided, gradients named
/// but not spelled out. tools/inspector normalises Chromium's value the same way.
pub(crate) fn background_image(b: &LayoutBox) -> String {
    let mut out = Vec::new();
    // The engine stores layers bottom first (index 0 paints first); CSS and
    // getComputedStyle list them top first, as written.
    for layer in b.style.background_layers.iter().rev() {
        out.push(match &layer.image {
            rustkit_css::BackgroundImage::None => "none".to_string(),
            rustkit_css::BackgroundImage::Url(u) => format!("url(\"{}\")", scrub_url(u)),
            rustkit_css::BackgroundImage::Gradient(g) => gradient_name(&format!("{g:?}")),
        });
    }
    if out.is_empty() || out.iter().all(|v| v == "none") {
        if let Some(g) = &b.style.background_gradient {
            return gradient_name(&format!("{g:?}"));
        }
        return "none".into();
    }
    out.join(", ")
}

fn gradient_name(debug: &str) -> String {
    let kind = debug.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("");
    let repeating = debug.contains("repeating: true");
    let base = match kind {
        k if k.starts_with("Radial") => "radial-gradient",
        k if k.starts_with("Conic") => "conic-gradient",
        _ => "linear-gradient",
    };
    format!("{}{base}(…)", if repeating { "repeating-" } else { "" })
}

/// A URL inside a record: no user name or password, no query, fragment or
/// `;` path parameters, `data:` contents elided, `javascript:` and `blob:`
/// as `report_url` gives them. The URL may be relative, as specified.
pub(crate) fn scrub_url(u: &str) -> String {
    // Every decision is made on the URL as the parser reads it: it drops
    // ASCII tab and newline anywhere, and leading and trailing C0 controls
    // and spaces, before it reads the scheme (also any leading Unicode
    // space, to record less). Schemes are case-insensitive.
    let u: String = u.chars().filter(|c| !matches!(c, '\t' | '\n' | '\r')).collect();
    let u = u.trim_start_matches(|c: char| c <= ' ' || c.is_whitespace()).trim_end_matches(|c: char| c <= ' ');
    let parsed = Url::parse(u).ok();
    if u.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("data:")) || parsed.as_ref().is_some_and(|p| p.scheme() == "data") {
        return "data:…".into();
    }
    // A script URL is all payload; a blob URL is reported by its origin.
    if let Some(p) = parsed.as_ref().filter(|p| matches!(p.scheme(), "javascript" | "blob")) {
        return report_url(p, false);
    }
    let u = u.split(['?', '#']).next().unwrap_or("");
    // Userinfo can only sit in an authority: after `scheme://` or a leading
    // `//`. The parser reads `\` as `/` there, and for a special scheme
    // (`https:u:p@host`) it needs no slashes at all.
    let slash = |c: char| c == '/' || c == '\\';
    let scheme_end = u.find(':').filter(|&i| {
        i > 0 && u[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    // `\` ends a path segment where the parser reads it as `/`: a special
    // scheme, or a relative URL (resolved against an http(s) page).
    let backslash = scheme_end.is_none() || parsed.as_ref().is_some_and(|p| p.is_special());
    let authority_at = match scheme_end {
        Some(i) if parsed.as_ref().is_some_and(|p| p.is_special() && p.scheme() != "file") => {
            u[i + 1..].find(|c: char| !slash(c)).map_or(u.len(), |j| i + 1 + j)
        }
        Some(i) if u[i + 1..].starts_with("//") => i + 3,
        Some(i) => return format!("{}{}", &u[..=i], drop_path_params(&u[i + 1..], backslash)),
        None if u.starts_with(|c: char| slash(c)) && u[1..].starts_with(|c: char| slash(c)) => 2,
        None => return drop_path_params(u, backslash),
    };
    let rest = &u[authority_at..];
    let authority_end = rest.find(slash).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    let host = authority.rfind('@').map_or(authority, |at| &authority[at + 1..]);
    format!("{}{host}{}", &u[..authority_at], drop_path_params(&rest[authority_end..], backslash))
}

fn style_record(
    b: &LayoutBox,
    unavailable: &mut BTreeMap<String, String>,
) -> BTreeMap<&'static str, Option<String>> {
    let s = &*b.style;
    let d = &b.dimensions;
    // The lines script reads (display, position, used sizes and edges, ...).
    let mut m: BTreeMap<&'static str, Option<String>> = BTreeMap::new();
    let base = computed_style_lines(b);
    let base: HashMap<&str, &str> = base.lines().filter_map(|l| l.split_once('\t')).collect();
    let kw = |v: &dyn std::fmt::Debug| css_keyword(&format!("{v:?}"));
    let inline_non_replaced = matches!(b.box_type, BoxType::Inline);
    let font_px = match s.font_size {
        Length::Px(p) => p,
        _ => 16.0,
    };
    let len = |l: &Length, none_for_auto| length(l, none_for_auto, font_px, b.viewport);
    // getComputedStyle's width/height follow box-sizing.
    let border_box_sizing = s.box_sizing == rustkit_css::BoxSizing::BorderBox;
    for p in STYLE_PROPERTIES {
        let v = match *p {
            "width" | "height" if inline_non_replaced => Some("auto".into()),
            "width" if border_box_sizing => Some(css_px(d.border_box().width)),
            "height" if border_box_sizing => Some(css_px(d.border_box().height)),
            "clear" => Some(kw(&s.clear)),
            "min-width" => Some(len(&s.min_width, false)),
            "min-height" => Some(len(&s.min_height, false)),
            "max-width" => Some(len(&s.max_width, true)),
            "max-height" => Some(len(&s.max_height, true)),
            // Resolved against the containing block in `insets`.
            "top" | "right" | "bottom" | "left" | "inset" => None,
            "opacity" => Some(num(s.opacity)),
            "transform" => Some({
                let t = s.effective_transform();
                if t.is_identity() {
                    "none".into()
                } else {
                    let bb = d.border_box();
                    let m6 = t.to_matrix(bb.width, bb.height);
                    // Six decimals, as Chromium serialises matrix entries.
                    let six = |v: f32| format!("{}", (v as f64 * 1e6).round() / 1e6 + 0.0);
                    format!("matrix({})", m6.iter().map(|v| six(*v)).collect::<Vec<_>>().join(", "))
                }
            }),
            "flex-wrap" => Some(kw(&s.flex_wrap)),
            "flex-grow" => Some(num(s.flex_grow)),
            "flex-shrink" => Some(num(s.flex_shrink)),
            "flex-basis" => Some(match s.flex_basis {
                rustkit_css::FlexBasis::Auto => "auto".into(),
                rustkit_css::FlexBasis::Content => "content".into(),
                rustkit_css::FlexBasis::Length(px) => css_px(px),
                rustkit_css::FlexBasis::Percent(pc) => format!("{}%", num(pc)),
            }),
            "align-items" => Some(kw(&s.align_items)),
            "align-self" => Some(kw(&s.align_self)),
            "justify-content" => Some(kw(&s.justify_content)),
            "justify-self" => Some(kw(&s.justify_self)),
            "order" => Some(s.order.to_string()),
            "row-gap" => Some(len(&s.row_gap, false)),
            "column-gap" => Some(len(&s.column_gap, false)),
            "gap" => {
                let (r, c) = (len(&s.row_gap, false), len(&s.column_gap, false));
                Some(if r == c { r } else { format!("{r} {c}") })
            }
            "grid-template-columns" | "grid-template-rows" => {
                let t = if *p == "grid-template-columns" { &s.grid_template_columns } else { &s.grid_template_rows };
                if has_line_names(t) {
                    unavailable.insert(format!("style.{p}"), "line names are not serialised".into());
                    None
                } else if !matches!(s.display, Display::Grid | Display::InlineGrid) {
                    // Not a grid container: getComputedStyle reports the computed list.
                    Some(template(t))
                } else {
                    // A grid container's value is its resolved tracks in px, which
                    // the engine does not keep. Only an all-px list is already that.
                    px_template(t).or_else(|| {
                        unavailable.insert(
                            format!("style.{p}"),
                            "a grid container reports resolved px tracks; the engine keeps only the specified list".into(),
                        );
                        None
                    })
                }
            }
            "grid-auto-flow" => Some(match s.grid_auto_flow {
                rustkit_css::GridAutoFlow::Row => "row".into(),
                rustkit_css::GridAutoFlow::Column => "column".into(),
                rustkit_css::GridAutoFlow::RowDense => "row dense".into(),
                rustkit_css::GridAutoFlow::ColumnDense => "column dense".into(),
            }),
            "grid-auto-rows" => Some(track(&s.grid_auto_rows)),
            "grid-auto-columns" => Some(track(&s.grid_auto_columns)),
            "grid-column-start" => Some(grid_line(&s.grid_column_start)),
            "grid-column-end" => Some(grid_line(&s.grid_column_end)),
            "grid-row-start" => Some(grid_line(&s.grid_row_start)),
            "grid-row-end" => Some(grid_line(&s.grid_row_end)),
            "aspect-ratio" => Some(s.aspect_ratio.map_or("auto".into(), |r| {
                format!("{}{} / 1", if s.aspect_ratio_auto { "auto " } else { "" }, num(r))
            })),
            "vertical-align" => Some(match s.vertical_align {
                rustkit_css::VerticalAlign::Length(px) => css_px(px),
                v => kw(&v),
            }),
            "background-image" => Some(background_image(b)),
            "object-fit" => Some(s.object_fit.clone()),
            // The computed style keeps `auto` as 0: an explicit `z-index: 0`
            // and `auto` cannot be told apart, and they stack differently.
            "z-index" if s.z_index == 0 => {
                unavailable.insert(
                    "style.z-index".into(),
                    "the engine stores auto as 0; auto and 0 cannot be told apart".into(),
                );
                None
            }
            "content-visibility" | "contain" => {
                unavailable.insert(format!("style.{p}"), "not modelled by the engine".into());
                None
            }
            other => base.get(other).map(|v| v.to_string()),
        };
        m.insert(p, v);
    }
    m
}

fn has_line_names(t: &rustkit_css::GridTemplate) -> bool {
    let named = |defs: &[rustkit_css::TrackDefinition]| defs.iter().any(|d| !d.line_names.is_empty());
    !t.final_line_names.is_empty()
        || named(&t.tracks)
        || t.repeats.iter().any(|(_, r)| match r {
            rustkit_css::TrackRepeat::Count(_, d)
            | rustkit_css::TrackRepeat::AutoFill(d)
            | rustkit_css::TrackRepeat::AutoFit(d) => named(d),
        })
}

/// `top`/`right`/`bottom`/`left`/`inset` as getComputedStyle resolves them:
/// - static: the computed value (a percentage as written, `auto` kept);
/// - relative: percentages against the containing block's content box, and
///   `auto` as the negated opposite side or `0px`;
/// - sticky: `auto` kept, percentages unknown (they resolve against the
///   scrollport, which the record does not model);
/// - absolute/fixed: the used offsets, read from the laid-out margin box
///   against the containing block's padding box (the viewport for fixed).
fn insets(
    style: &mut BTreeMap<&'static str, Option<String>>,
    b: &LayoutBox,
    ctx: &BoxContext<'_>,
    scroll: (f32, f32),
    viewport: (u32, u32),
    unavailable: &mut BTreeMap<String, String>,
) {
    use rustkit_css::Position;
    let s = &*b.style;
    let font_px = match s.font_size {
        Length::Px(p) => p,
        _ => 16.0,
    };
    let vp = (viewport.0 as f32, viewport.1 as f32);
    let icb = Rect { x: 0.0, y: 0.0, width: vp.0, height: vp.1 };
    // [top, right, bottom, left] and the axis size each side's % refers to.
    let specified = [&s.top, &s.right, &s.bottom, &s.left];
    let auto = |l: &Option<Length>| matches!(l, None | Some(Length::Auto));
    let values: [Option<String>; 4] = match s.position {
        Position::Static => specified.map(|l| match l {
            None => Some("auto".to_string()),
            Some(l) => Some(length(l, false, font_px, vp)),
        }),
        Position::Sticky => specified.map(|l| match l {
            None | Some(Length::Auto) => Some("auto".to_string()),
            Some(Length::Percent(_)) => None,
            Some(l) => Some(length(l, false, font_px, vp)),
        }),
        Position::Relative => {
            let cb = ctx.containing_block.map_or(icb, |c| c.dimensions.content);
            let sizes = [cb.height, cb.width, cb.height, cb.width];
            let px = |i: usize| -> Option<f32> {
                match specified[i] {
                    None | Some(Length::Auto) => None,
                    Some(Length::Percent(p)) => Some(p / 100.0 * sizes[i]),
                    Some(l) => Some(l.to_px_with_viewport(font_px, 16.0, sizes[i], vp.0, vp.1)),
                }
            };
            let mut out: [Option<String>; 4] = Default::default();
            for i in 0..4 {
                let v = match px(i) {
                    Some(v) => v,
                    // Over-constrained or both auto: the opposite side wins.
                    None if !auto(specified[(i + 2) % 4]) => -px((i + 2) % 4).unwrap_or(0.0),
                    None => 0.0,
                };
                out[i] = Some(css_px(v + 0.0));
            }
            out
        }
        Position::Absolute | Position::Fixed => {
            let (m, cb) = if s.position == Position::Fixed {
                let m = b.dimensions.margin_box();
                (Rect { x: m.x - scroll.0, y: m.y - scroll.1, ..m }, icb)
            } else {
                (b.dimensions.margin_box(), ctx.containing_block.map_or(icb, |c| c.dimensions.padding_box()))
            };
            [
                m.y - cb.y,
                cb.x + cb.width - (m.x + m.width),
                cb.y + cb.height - (m.y + m.height),
                m.x - cb.x,
            ]
            .map(|v| Some(css_px(v + 0.0)))
        }
    };
    for (name, v) in ["top", "right", "bottom", "left"].into_iter().zip(values.iter()) {
        if v.is_none() {
            unavailable.insert(
                format!("style.{name}"),
                "a sticky box's percentage inset resolves against its scrollport, which is not modelled".into(),
            );
        }
        style.insert(name, v.clone());
    }
    let inset = match values {
        [Some(t), Some(r), Some(bt), Some(l)] => Some(four(t, r, bt, l)),
        _ => {
            unavailable.insert("style.inset".into(), "a side is unknown".into());
            None
        }
    };
    style.insert("inset", inset);
}

/// A track list that is px only (fixed repeats expanded), as getComputedStyle
/// resolves it; `None` for anything that must be resolved by layout, or for
/// no list at all (Chromium then reports the implicit tracks).
fn px_template(t: &rustkit_css::GridTemplate) -> Option<String> {
    let px = |d: &rustkit_css::TrackDefinition| match d.size {
        TrackSize::Px(p) => Some(css_px(p)),
        _ => None,
    };
    let mut parts: Vec<String> = t.tracks.iter().map(px).collect::<Option<_>>()?;
    for (at, rep) in t.repeats.iter().rev() {
        let rustkit_css::TrackRepeat::Count(n, defs) = rep else { return None };
        let one: Vec<String> = defs.iter().map(px).collect::<Option<_>>()?;
        let expanded = (0..*n).flat_map(|_| one.iter().cloned());
        let at = (*at).min(parts.len());
        parts.splice(at..at, expanded);
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// Brackets, parentheses and quotes close. The shared matcher accepts some
/// unclosed selectors (`p[`, `div:has(`) and matches nothing, which reads
/// as "no element"; `querySelector` throws on them.
fn selector_is_balanced(sel: &str) -> bool {
    let mut open = Vec::new();
    let mut quote = None;
    let mut chars = sel.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (_, '\\') => {
                if chars.next().is_none() {
                    return false;
                }
            }
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '[' | '(') => open.push(c),
            (None, ']') if open.pop() != Some('[') => return false,
            (None, ')') if open.pop() != Some('(') => return false,
            _ => {}
        }
    }
    quote.is_none() && open.is_empty()
}

impl Engine {
    fn diagnostics_page(&self, view: &ViewState, opts: &DiagnosticsOptions) -> PageInfo {
        let mut unavailable = BTreeMap::new();
        let viewport = match view.headless_bounds {
            Some(b) => (b.width, b.height),
            None => self
                .presenter
                .as_ref()
                .and_then(|presenter| presenter.surface_size(view.viewhost_id).ok())
                .unwrap_or((0, 0)),
        };
        let commit = option_env!("HIWAVE_BUILD_COMMIT").map(str::to_string);
        if commit.is_none() {
            unavailable.insert("engine.commit".into(), "HIWAVE_BUILD_COMMIT was not set at compile time".into());
        }
        let device_scale = view.headless_bounds.map(|_| 1.0);
        if device_scale.is_none() {
            unavailable.insert("device_scale".into(), "the engine lays out in CSS px and does not track the window's backing scale".into());
        }
        PageInfo {
            url: view.url.as_ref().map(|u| report_url(u, opts.include_url_query)),
            engine: EngineBuild { version: env!("CARGO_PKG_VERSION").to_string(), commit },
            viewport,
            device_scale,
            scroll: view.scroll_offset,
            text_metrics_font_derived: rustkit_layout::TEXT_METRICS_ARE_FONT_DERIVED,
            unavailable,
        }
    }

    fn diagnostics_source<'a>(&self, view: &'a ViewState, page: &PageInfo) -> Result<DiagnosticsSource<'a>, EngineError> {
        let document = view
            .document
            .as_deref()
            .ok_or_else(|| EngineError::RenderError("No document available".into()))?;
        Ok(DiagnosticsSource {
            document,
            layout: view.layout.as_deref(),
            scroll: view.scroll_offset,
            viewport: page.viewport,
            checked: view.lay.checked_states.borrow(),
        })
    }

    /// The diagnostics record of one element, or `None` when no element
    /// matches the target.
    ///
    /// Read-only over the view's LAST COMMITTED layout tree and DOM: it takes
    /// `&self`, runs no script, triggers no layout, and borrows the DOM's
    /// `RefCell`s (children, node map) only for the duration of the call. An
    /// app can therefore call it from the window thread between turns, and it
    /// answers from the last completed layout even while the page has script
    /// work queued. It cannot run DURING a script turn on the same thread:
    /// script holds `&mut Engine` for the turn. A selector is matched with the cascade's matcher; an
    /// invalid one is an error. A point is in viewport CSS px; one outside
    /// the viewport matches nothing, as `elementFromPoint`.
    pub fn element_diagnostics(
        &self,
        id: EngineViewId,
        target: &DiagnosticTarget,
        opts: &DiagnosticsOptions,
    ) -> Result<Option<ElementDiagnostics>, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let page = self.diagnostics_page(view, opts);
        let src = self.diagnostics_source(view, &page)?;
        let node = match target {
            DiagnosticTarget::Node(raw) => src.document.get_node(rustkit_dom::NodeId::new(*raw)),
            // As `elementFromPoint`: a point outside the viewport hits nothing,
            // even where the document has content there.
            DiagnosticTarget::Point { x, y }
                if !(*x >= 0.0 && *y >= 0.0 && *x < src.viewport.0 as f32 && *y < src.viewport.1 as f32) =>
            {
                None
            }
            DiagnosticTarget::Point { x, y } => src
                .layout
                .and_then(|l| l.hit_test(x + src.scroll.0, y + src.scroll.1))
                .and_then(|hit| hit.node_id)
                .and_then(|raw| src.document.get_node(rustkit_dom::NodeId::new(raw))),
            DiagnosticTarget::Selector(sel) if !selector_is_balanced(sel) => {
                return Err(EngineError::RenderError(format!("invalid selector: {sel}")));
            }
            DiagnosticTarget::Selector(sel) => {
                let mut stack = vec![src.document.root().clone()];
                let mut found = None;
                while let Some(n) = stack.pop() {
                    match SelectorMatcher.node_matches(&n, sel, &src.checked) {
                        None => return Err(EngineError::RenderError(format!("invalid selector: {sel}"))),
                        Some(true) => {
                            found = Some(n);
                            break;
                        }
                        Some(false) => {}
                    }
                    stack.extend(n.children().into_iter().rev());
                }
                found
            }
        };
        let Some(node) = node else { return Ok(None) };
        let builder = Builder::new(&src);
        Ok(builder.record(&node).map(|mut r| {
            r.page = Some(page);
            r
        }))
    }

    /// A record for every element of the document in tree order, with or
    /// without a box, up to `opts.max_elements` (`truncated` says when the
    /// cap cut the list). Same borrowing as [`Engine::element_diagnostics`].
    pub fn all_element_diagnostics(
        &self,
        id: EngineViewId,
        opts: &DiagnosticsOptions,
    ) -> Result<DiagnosticsDump, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let page = self.diagnostics_page(view, opts);
        let src = self.diagnostics_source(view, &page)?;
        let builder = Builder::new(&src);
        let mut elements = Vec::new();
        let mut total = 0;
        let mut stack = vec![src.document.root().clone()];
        while let Some(n) = stack.pop() {
            if matches!(n.node_type, NodeType::Element { .. }) {
                total += 1;
                if elements.len() < opts.max_elements {
                    elements.extend(builder.record(&n));
                }
            }
            stack.extend(n.children().into_iter().rev());
        }
        Ok(DiagnosticsDump {
            schema: DIAGNOSTICS_SCHEMA,
            page,
            total_elements: total,
            truncated: total > elements.len(),
            elements,
        })
    }
}
