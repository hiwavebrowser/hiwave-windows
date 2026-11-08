//! Live developer tools: what the app's Inspector panel asks of the current
//! tab's view while someone inspects it. Pick an element under the pointer,
//! describe it (breadcrumb, box geometry, computed style), list a bounded
//! slice of the DOM, outline an element on the page, and turn the view's
//! Inspector log into console lines.
//!
//! On demand and bounded. Every query borrows the view's last committed DOM
//! and layout between script turns (as `Engine::element_diagnostics`, which
//! it builds on), runs no script and triggers no layout; a tree is capped at
//! [`DEVTOOLS_TREE_MAX_NODES`] nodes. Nothing leaves the process: the
//! payloads go to the app's own Inspector panel over its IPC, and to the
//! clipboard only when the user asks for "Copy details".
//!
//! The payloads are what the panel (`hiwave-app/src/ui/inspector.html`)
//! renders, so they use its camelCase field names; a node is named by its
//! `path`, `rk:<node id>`.

use super::*;
use crate::diagnostics::DiagRect;
use crate::inspector::{InspectorRecord, InspectorSnapshot};
use rustkit_layout::DisplayCommand;
use serde::Serialize;
use std::collections::BTreeMap;

/// Most nodes one tree payload carries; past it a node's children are
/// counted in `childrenOmitted`.
pub const DEVTOOLS_TREE_MAX_NODES: usize = 3000;
/// Depth the tree is expanded to, below the node asked for.
pub const DEVTOOLS_TREE_DEPTH: usize = 6;
/// Why `ruleSources` is empty: the cascade keeps no stylesheet URL on a
/// rule, nor which rule won each property.
pub const RULE_SOURCES_UNAVAILABLE: &str =
    "the cascade does not record which rule set a property, or a rule's stylesheet URL";

/// The panel's name for a node.
pub fn devtools_path(node_id: usize) -> String {
    format!("rk:{node_id}")
}

/// The node id a panel path names (`rk:42`), if it is one.
pub fn devtools_node_of(path: &str) -> Option<usize> {
    path.strip_prefix("rk:")?.parse().ok()
}

/// One node of the DOM tree the panel draws.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevtoolsNode {
    pub path: String,
    pub node_id: usize,
    /// `element`, `text` or `comment`.
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// A text or comment node's text, trimmed and capped.
    pub text_content: Option<String>,
    pub children: Vec<DevtoolsNode>,
    /// Children not in `children` (depth or node cap); the panel asks for
    /// them when the node is expanded.
    pub children_omitted: usize,
}

/// A rectangle in viewport CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct DevtoolsRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl From<DiagRect> for DevtoolsRect {
    fn from(r: DiagRect) -> Self {
        Self { x: r.x, y: r.y, width: r.w, height: r.h }
    }
}

/// The box model, in the panel's field names: the used margin, border and
/// padding widths and the content box's size.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevtoolsBoxModel {
    pub margin_top: f32,
    pub margin_right: f32,
    pub margin_bottom: f32,
    pub margin_left: f32,
    pub border_top: f32,
    pub border_right: f32,
    pub border_bottom: f32,
    pub border_left: f32,
    pub padding_top: f32,
    pub padding_right: f32,
    pub padding_bottom: f32,
    pub padding_left: f32,
    pub width: f32,
    pub height: f32,
    pub margin_box: DevtoolsRect,
    pub border_box: DevtoolsRect,
    pub padding_box: DevtoolsRect,
    pub content_box: DevtoolsRect,
}

/// One ancestor (or the element itself) in the breadcrumb.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DevtoolsCrumb {
    pub path: String,
    /// `tag#id.class.class`.
    pub label: String,
}

/// Everything the panel shows for the selected element.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevtoolsElement {
    pub path: String,
    pub node_id: usize,
    pub tag: String,
    pub id: Option<String>,
    pub classes: Vec<String>,
    /// Unique structural selector (`html > body > div:nth-child(2) > p`).
    pub selector: String,
    /// From `html` down to the element itself.
    pub breadcrumb: Vec<DevtoolsCrumb>,
    /// The border box (`getBoundingClientRect`); `None` without a box.
    pub rect: Option<DevtoolsRect>,
    pub box_model: Option<DevtoolsBoxModel>,
    pub no_box_reason: Option<String>,
    /// Computed values of the diagnostics properties the engine answers.
    pub computed_styles: BTreeMap<String, String>,
    /// Why a property (or, under `style`, all of them) has no value.
    pub style_unavailable: BTreeMap<String, String>,
    /// Property -> `selector @ stylesheet`; empty, see `ruleSourcesUnavailable`.
    pub rule_sources: BTreeMap<String, String>,
    pub rule_sources_unavailable: Option<&'static str>,
    /// What "Copy details" puts on the clipboard.
    pub details_text: String,
}

/// One console line, in the panel's shape.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DevtoolsConsoleEntry {
    /// The record's `seq`.
    pub id: u64,
    /// The document it was logged in (`InspectorRecord::doc`): the panel
    /// keeps old documents' lines, marked, until the ring drops them.
    pub doc: u32,
    /// `log`, `info`, `warn`, `error` or `debug`.
    pub level: &'static str,
    /// The Inspector event it came from (`console`, `script_error`, ...).
    pub kind: &'static str,
    pub message: String,
    /// Where it happened, innermost first; at most one frame.
    pub stack: Vec<DevtoolsFrame>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DevtoolsFrame {
    pub file: String,
    pub line: u32,
    pub column: u32,
}

/// The console lines since a `seq`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevtoolsConsole {
    pub entries: Vec<DevtoolsConsoleEntry>,
    /// Pass as `since` next time.
    pub next_seq: u64,
    /// Records the view's ring buffer dropped.
    pub dropped: u64,
    /// A new document committed since `since`: node ids from before name
    /// nothing, so the panel drops its selection and outline.
    pub page_changed: bool,
    /// The document laid out for the first time or finished loading since
    /// `since` (or a new one committed): the panel's tree is stale.
    pub tree_stale: bool,
    /// The page thread panicked holding the view's Inspector lock; the
    /// lines are what it recorded before (T6 R1 condition 2).
    pub page_crashed: bool,
    /// Console calls a running script made and the view has not recorded
    /// yet (past the live rate cap): "K console lines held by a running
    /// script" (`Inspector::console_held`, read without the page).
    pub held: usize,
}

/// One request of the current document, in the panel's shape (T7).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevtoolsRequest {
    /// The view's request id.
    pub id: u64,
    /// Scrubbed when recorded: origin and path, no query or fragment.
    pub url: String,
    pub method: String,
    pub initiator: Option<String>,
    /// `pending`, `response` or `failure`.
    pub state: &'static str,
    /// The response's status.
    pub status: Option<u16>,
    /// The failure's stage and reason, in the engine's words.
    pub failure: Option<String>,
    pub duration_ms: Option<f64>,
}

/// The current document's requests, in the order they started (T7). Read
/// by the devtools thread from the view's Inspector, without the page.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevtoolsNetwork {
    /// The document they belong to (`InspectorRecord::doc`): records of
    /// any other document, a late cancel of the last page's request
    /// included, are not listed (T7 C5).
    pub doc: u32,
    pub requests: Vec<DevtoolsRequest>,
    /// Records the view's ring buffer dropped.
    pub dropped: u64,
}

/// The requests `snapshot` holds for document `doc`: each `request` with its
/// `response` or `failure`, if one has been recorded.
pub fn devtools_network(snapshot: &InspectorSnapshot, doc: u32) -> DevtoolsNetwork {
    let mut requests: Vec<DevtoolsRequest> = Vec::new();
    // Request id to its place in `requests`: a terminal record finds its
    // request at once, not by a scan (T8).
    let mut at: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    for record in snapshot.records.iter().filter(|r| r.doc == doc) {
        match &record.event {
            InspectorEvent::Request { id, url, method, initiator, .. } => {
                at.entry(*id).or_insert(requests.len());
                requests.push(DevtoolsRequest {
                id: *id,
                url: url.clone(),
                method: method.clone(),
                initiator: initiator.clone(),
                state: "pending",
                status: None,
                failure: None,
                duration_ms: None,
                })
            }
            InspectorEvent::Response { id, status, duration_ms, .. } => {
                if let Some(r) = at.get(id).map(|&i| &mut requests[i]) {
                    r.state = "response";
                    r.status = Some(*status);
                    r.duration_ms = Some(*duration_ms);
                }
            }
            InspectorEvent::Failure { id, stage, reason, duration_ms, .. } => {
                if let Some(r) = at.get(id).map(|&i| &mut requests[i]) {
                    let stage = serde_json::to_value(stage).ok().and_then(|v| v.as_str().map(str::to_string));
                    r.state = "failure";
                    r.failure = Some(format!("{}: {reason}", stage.unwrap_or_default()));
                    r.duration_ms = Some(*duration_ms);
                }
            }
            _ => {}
        }
    }
    DevtoolsNetwork { doc, requests, dropped: snapshot.dropped }
}

/// `tag#id.class.class`.
fn crumb_label(tag: &str, id: Option<&str>, classes: &[String]) -> String {
    let mut s = tag.to_string();
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        s.push('#');
        s.push_str(id);
    }
    for c in classes {
        s.push('.');
        s.push_str(c);
    }
    s
}

fn classes_of(node: &Node) -> Vec<String> {
    node.get_attribute("class")
        .map(|c| c.split_ascii_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// A text or comment node as the panel shows it: its length, not the
/// page's words (Pete 2026-10-23, the Inspector is shape-only).
fn text_shape(text: &str) -> String {
    format!("<text len={}>", text.chars().count())
}

/// A URL as the panel shows it: no query, fragment or credentials, whatever
/// the Inspector's own setting keeps (Pete 2026-10-23).
fn url_shape(text: &str) -> String {
    match Url::parse(text) {
        Ok(url) => crate::inspector::scrub_url(&url, false),
        Err(_) => text.split(['?', '#']).next().unwrap_or_default().to_string(),
    }
}

/// Whether the panel shows `node`: elements, non-blank text, comments.
fn shown(node: &Node) -> bool {
    match &node.node_type {
        NodeType::Element { .. } | NodeType::Comment(_) => true,
        NodeType::Text(t) => !t.trim().is_empty(),
        _ => false,
    }
}

/// `node` and its shown descendants, `depth` levels down, the ancestors
/// in `reveal` expanded past it, within `budget` nodes.
fn tree_node(node: &Rc<Node>, depth: usize, reveal: &[usize], budget: &mut usize) -> DevtoolsNode {
    *budget = budget.saturating_sub(1);
    let (kind, tag, id, classes, text) = match &node.node_type {
        NodeType::Element { tag_name, .. } => (
            "element",
            tag_name.to_ascii_lowercase(),
            node.get_attribute("id").map(str::to_string),
            classes_of(node),
            None,
        ),
        NodeType::Text(t) => ("text", String::new(), None, Vec::new(), Some(text_shape(t))),
        NodeType::Comment(t) => ("comment", String::new(), None, Vec::new(), Some(text_shape(t))),
        _ => ("document", String::new(), None, Vec::new(), None),
    };
    let raw = node.id.raw();
    let kids: Vec<Rc<Node>> = node.children().into_iter().filter(|c| shown(c)).collect();
    let expand = depth > 0 || reveal.contains(&raw);
    let mut children = Vec::new();
    if expand {
        for kid in &kids {
            if *budget == 0 {
                break;
            }
            children.push(tree_node(kid, depth.saturating_sub(1), reveal, budget));
        }
    }
    DevtoolsNode {
        path: devtools_path(raw),
        node_id: raw,
        kind,
        tag,
        id,
        classes,
        text_content: text,
        children_omitted: kids.len() - children.len(),
        children,
    }
}

/// The first layout box `node_id` generated, in tree order.
fn box_of(root: &LayoutBox, node_id: usize) -> Option<&LayoutBox> {
    let mut stack = vec![root];
    while let Some(b) = stack.pop() {
        if b.node_id == Some(node_id) {
            return Some(b);
        }
        stack.extend(b.children.iter().rev());
    }
    None
}

fn fmt_px(v: f32) -> String {
    let r = (v * 100.0).round() / 100.0;
    if r == r.trunc() { format!("{}", r as i64) } else { format!("{r}") }
}

fn fmt_rect(r: &DevtoolsRect) -> String {
    format!("x={} y={} w={} h={}", fmt_px(r.x), fmt_px(r.y), fmt_px(r.width), fmt_px(r.height))
}

impl DevtoolsElement {
    /// The plain-text block "Copy details" copies: selector path, geometry
    /// and computed style. No page text and no URL.
    fn render_details(&self) -> String {
        let mut out = format!("selector: {}\n", self.selector);
        let crumbs: Vec<&str> = self.breadcrumb.iter().map(|c| c.label.as_str()).collect();
        out.push_str(&format!("path: {}\n", crumbs.join(" > ")));
        match &self.box_model {
            Some(m) => {
                out.push_str(&format!("margin box:  {}\n", fmt_rect(&m.margin_box)));
                out.push_str(&format!("border box:  {}\n", fmt_rect(&m.border_box)));
                out.push_str(&format!("padding box: {}\n", fmt_rect(&m.padding_box)));
                out.push_str(&format!("content box: {}\n", fmt_rect(&m.content_box)));
                let edges = |t: f32, r: f32, b: f32, l: f32| {
                    format!("{} {} {} {}", fmt_px(t), fmt_px(r), fmt_px(b), fmt_px(l))
                };
                out.push_str(&format!(
                    "margin: {} | border: {} | padding: {}\n",
                    edges(m.margin_top, m.margin_right, m.margin_bottom, m.margin_left),
                    edges(m.border_top, m.border_right, m.border_bottom, m.border_left),
                    edges(m.padding_top, m.padding_right, m.padding_bottom, m.padding_left),
                ));
            }
            None => out.push_str(&format!(
                "no box: {}\n",
                self.no_box_reason.as_deref().unwrap_or("not laid out")
            )),
        }
        out.push_str("computed style:\n");
        for (k, v) in &self.computed_styles {
            out.push_str(&format!("  {k}: {v};\n"));
        }
        for (k, why) in &self.style_unavailable {
            out.push_str(&format!("  /* {k}: unavailable, {why} */\n"));
        }
        out
    }
}

/// Fill colours of the highlight overlay, as Chrome's DevTools draws it.
const HIGHLIGHT_CONTENT: (u8, u8, u8, f32) = (111, 168, 220, 0.66);
const HIGHLIGHT_PADDING: (u8, u8, u8, f32) = (147, 196, 125, 0.55);
const HIGHLIGHT_BORDER: (u8, u8, u8, f32) = (255, 229, 153, 0.66);
const HIGHLIGHT_MARGIN: (u8, u8, u8, f32) = (246, 178, 107, 0.66);

/// The parts of `outer` outside `inner` as up to four rectangles.
fn ring(outer: Rect, inner: Rect) -> impl Iterator<Item = Rect> {
    let top = inner.y - outer.y;
    let bottom = outer.y + outer.height - (inner.y + inner.height);
    let left = inner.x - outer.x;
    let right = outer.x + outer.width - (inner.x + inner.width);
    [
        Rect { x: outer.x, y: outer.y, width: outer.width, height: top },
        Rect { x: outer.x, y: inner.y + inner.height, width: outer.width, height: bottom },
        Rect { x: outer.x, y: inner.y, width: left, height: inner.height },
        Rect { x: inner.x + inner.width, y: inner.y, width: right, height: inner.height },
    ]
    .into_iter()
    .filter(|r| r.width > 0.0 && r.height > 0.0)
}

/// The overlay drawn over `b`: content, padding, border and margin areas,
/// in the display list's (document) coordinates, and a 1px outline.
pub(crate) fn highlight_commands(b: &LayoutBox) -> Vec<DisplayCommand> {
    let d = &b.dimensions;
    let fill = |(r, g, bl, a): (u8, u8, u8, f32), rect: Rect| {
        DisplayCommand::SolidColor(rustkit_css::Color::new(r, g, bl, a), rect)
    };
    let mut out = vec![fill(HIGHLIGHT_CONTENT, d.content)];
    out.extend(ring(d.padding_box(), d.content).map(|r| fill(HIGHLIGHT_PADDING, r)));
    out.extend(ring(d.border_box(), d.padding_box()).map(|r| fill(HIGHLIGHT_BORDER, r)));
    out.extend(ring(d.margin_box(), d.border_box()).map(|r| fill(HIGHLIGHT_MARGIN, r)));
    out.push(DisplayCommand::Border {
        color: rustkit_css::Color::new(26, 115, 232, 1.0),
        rect: d.border_box(),
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    });
    out
}

/// `list` with the highlight of `node_id` drawn over it, when the node has
/// a box in `layout`; else `list` itself. Copies the list: only while the
/// user points at elements.
pub(crate) fn with_highlight(
    list: Option<Arc<DisplayList>>,
    layout: Option<&LayoutBox>,
    node_id: Option<usize>,
) -> Option<Arc<DisplayList>> {
    let (Some(base), Some(b)) = (list.as_ref(), node_id.and_then(|n| layout.and_then(|l| box_of(l, n)))) else {
        return list;
    };
    let mut combined = DisplayList::clone(base);
    combined.commands.extend(highlight_commands(b));
    Some(Arc::new(combined))
}

fn frame(loc: &Option<SourceLocation>) -> Vec<DevtoolsFrame> {
    loc.iter()
        .map(|l| DevtoolsFrame { file: url_shape(&l.url), line: l.line, column: l.column })
        .collect()
}

/// The console line a record stands for, if it is one.
fn console_entry(record: &InspectorRecord) -> Option<DevtoolsConsoleEntry> {
    let entry = |level, message: String, stack| DevtoolsConsoleEntry {
        id: record.seq,
        doc: record.doc,
        level,
        kind: record.event.kind(),
        message,
        stack,
    };
    Some(match &record.event {
        InspectorEvent::Console { level, message, truncated, location, .. } => {
            let level = match level {
                ConsoleLevel::Log => "log",
                ConsoleLevel::Info => "info",
                ConsoleLevel::Warn => "warn",
                ConsoleLevel::Error => "error",
                ConsoleLevel::Debug => "debug",
            };
            let text = if *truncated { format!("{message} …") } else { message.clone() };
            entry(level, text, frame(location))
        }
        InspectorEvent::ScriptError { name, message, location, unhandled_rejection, value, message_len, script, .. } => {
            let what = if name.is_empty() { value.as_str() } else { name.as_str() };
            let mut text = format!("Uncaught {}{what}", if *unhandled_rejection { "(in promise) " } else { "" });
            match (message, message_len) {
                (Some(m), _) => text.push_str(&format!(": {m}")),
                (None, Some(n)) => text.push_str(&format!(" (message hidden, {n} chars)")),
                (None, None) => {}
            }
            text.push_str(&format!(" [{script}]"));
            entry("error", text, frame(location))
        }
        InspectorEvent::UnsupportedApi { interface, member, location } => {
            entry("warn", format!("Not supported by RustKit: {interface}.{member}"), frame(location))
        }
        InspectorEvent::LoadFinished { scripts_over_budget, .. } if *scripts_over_budget > 0 => entry(
            "warn",
            format!("{scripts_over_budget} script(s) skipped or stopped: the page's script budget was spent"),
            Vec::new(),
        ),
        InspectorEvent::LoadEndedEarly { reason, detail, .. } => {
            let reason = match reason {
                EndReason::NavigatedAway => "navigated away",
                EndReason::Failed => "failed",
            };
            entry("warn", format!("Load ended early ({reason}): {detail}"), Vec::new())
        }
        InspectorEvent::NavigationStart { url } => {
            let url = url.as_deref().map_or_else(|| "(inline document)".to_string(), url_shape);
            entry("info", format!("Navigated to {url}"), Vec::new())
        }
        _ => return None,
    })
}

/// The console lines of `snapshot`, oldest first.
pub fn devtools_console_entries(snapshot: &InspectorSnapshot) -> DevtoolsConsole {
    DevtoolsConsole {
        entries: snapshot.records.iter().filter_map(console_entry).collect(),
        next_seq: snapshot.next_seq,
        dropped: snapshot.dropped,
        // Keyed on the events, not on a nav id: the ring can drop records,
        // and the window reads a new document's commit at most one poll late.
        page_changed: snapshot.records.iter().any(|r| matches!(r.event, InspectorEvent::NavigationCommit { .. })),
        tree_stale: snapshot.records.iter().any(|r| {
            matches!(
                r.event,
                InspectorEvent::NavigationCommit { .. } | InspectorEvent::FirstLayout | InspectorEvent::LoadFinished { .. }
            )
        }),
        page_crashed: false,
        held: 0,
    }
}

/// The panel's line for `held` console calls, or none.
pub fn devtools_held_text(held: usize) -> Option<String> {
    (held > 0).then(|| format!("{held} console lines held by a running script"))
}

impl Engine {
    /// Start (or stop) recording view `id`'s console, errors and load
    /// events for the panel. On: the ring buffer records and the current
    /// document's console is hooked now, not at its next commit. Off:
    /// recording goes back to the view's default and nothing recorded is
    /// dropped, so the panel reopens with its history (T6b).
    pub fn devtools_attach(&mut self, id: EngineViewId, on: bool) -> Result<(), EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        if on {
            view.inspector.set_buffer_enabled(true);
        } else {
            view.inspector.detach_panel();
        }
        if on {
            if let Some(bindings) = view.bindings.as_ref() {
                inspector_glue::attach(bindings, &view.inspector);
            }
        }
        Ok(())
    }

    /// The panel's console lines recorded since `since`.
    pub fn devtools_console(&self, id: EngineViewId, since: u64) -> Result<DevtoolsConsole, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        Ok(devtools_console_entries(&view.inspector.snapshot_since(since)))
    }

    /// View `id`'s requests for its current document (T7). URLs are the
    /// ones the Inspector scrubbed when it recorded them (`scrub_url` on
    /// `request`, `scrub_text` on `fail`); this path scrubs nothing again.
    pub fn devtools_network(&self, id: EngineViewId) -> Result<DevtoolsNetwork, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let (snapshot, doc) = view.inspector.network_for_current_doc();
        Ok(devtools_network(&snapshot, doc))
    }

    /// The element under viewport point (`x`, `y`) in CSS px, as
    /// `elementFromPoint` finds it (text selects its element; outside the
    /// viewport, nothing). One hit test: cheap enough for every pointer
    /// move while picking.
    pub fn devtools_node_at(&self, id: EngineViewId, x: f32, y: f32) -> Result<Option<usize>, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let Some((w, h)) = self.view_viewport(id) else { return Ok(None) };
        let (Some(layout), Some(document)) = (view.layout.as_deref(), view.document.as_deref()) else {
            return Ok(None);
        };
        if !(x >= 0.0 && y >= 0.0 && x < w && y < h) {
            return Ok(None);
        }
        let (sx, sy) = view.scroll_offset;
        let mut at = layout
            .hit_test(x + sx, y + sy)
            .and_then(|hit| hit.node_id)
            .and_then(|raw| document.get_node(rustkit_dom::NodeId::new(raw)));
        while let Some(n) = at {
            if matches!(n.node_type, NodeType::Element { .. }) {
                return Ok(Some(n.id.raw()));
            }
            at = n.parent();
        }
        Ok(None)
    }

    /// The DOM below `root` (the document when `None`), expanded
    /// [`DEVTOOLS_TREE_DEPTH`] levels and along the ancestors of `reveal`,
    /// within [`DEVTOOLS_TREE_MAX_NODES`] nodes. `None` when the view has
    /// no document or `root` is not in it.
    pub fn devtools_dom_tree(
        &self,
        id: EngineViewId,
        root: Option<usize>,
        reveal: Option<usize>,
    ) -> Result<Option<DevtoolsNode>, EngineError> {
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let Some(document) = view.document.as_deref() else { return Ok(None) };
        let start = match root {
            Some(raw) => match document.get_node(rustkit_dom::NodeId::new(raw)) {
                Some(n) => n,
                None => return Ok(None),
            },
            None => document.root().clone(),
        };
        let mut chain = Vec::new();
        let mut at = reveal.and_then(|raw| document.get_node(rustkit_dom::NodeId::new(raw)));
        while let Some(n) = at {
            at = n.parent();
            chain.push(n.id.raw());
        }
        let mut budget = DEVTOOLS_TREE_MAX_NODES;
        Ok(Some(tree_node(&start, DEVTOOLS_TREE_DEPTH, &chain, &mut budget)))
    }

    /// What the panel shows for element `node_id`; `None` when it is not an
    /// element of the view's document.
    pub fn devtools_element(&self, id: EngineViewId, node_id: usize) -> Result<Option<DevtoolsElement>, EngineError> {
        let Some(record) =
            self.element_diagnostics(id, &DiagnosticTarget::Node(node_id), &DiagnosticsOptions::default())?
        else {
            return Ok(None);
        };
        let view = self.views.get(&id).ok_or(EngineError::ViewNotFound(id))?;
        let document = view.document.as_deref().ok_or_else(|| EngineError::RenderError("No document available".into()))?;
        let mut breadcrumb = Vec::new();
        let mut at = document.get_node(rustkit_dom::NodeId::new(node_id));
        while let Some(n) = at {
            if let NodeType::Element { tag_name, .. } = &n.node_type {
                let label = crumb_label(&tag_name.to_ascii_lowercase(), n.get_attribute("id"), &classes_of(&n));
                breadcrumb.push(DevtoolsCrumb { path: devtools_path(n.id.raw()), label });
            }
            at = n.parent();
        }
        breadcrumb.reverse();
        let lb = &record.layout_box;
        let box_model = match (lb.border_box, lb.padding_box, lb.content_box, view.layout.as_deref()) {
            (Some(border), Some(padding), Some(content), Some(layout)) => box_of(layout, node_id).map(|b| {
                let d = &b.dimensions;
                let margin_box = DevtoolsRect {
                    x: border.x - d.margin.left,
                    y: border.y - d.margin.top,
                    width: border.w + d.margin.horizontal(),
                    height: border.h + d.margin.vertical(),
                };
                DevtoolsBoxModel {
                    margin_top: d.margin.top,
                    margin_right: d.margin.right,
                    margin_bottom: d.margin.bottom,
                    margin_left: d.margin.left,
                    border_top: d.border.top,
                    border_right: d.border.right,
                    border_bottom: d.border.bottom,
                    border_left: d.border.left,
                    padding_top: d.padding.top,
                    padding_right: d.padding.right,
                    padding_bottom: d.padding.bottom,
                    padding_left: d.padding.left,
                    width: content.w,
                    height: content.h,
                    margin_box,
                    border_box: border.into(),
                    padding_box: padding.into(),
                    content_box: content.into(),
                }
            }),
            _ => None,
        };
        let computed_styles = record
            .style
            .iter()
            .filter_map(|(k, v)| v.as_ref().map(|v| (k.to_string(), v.clone())))
            .collect();
        let style_unavailable = record
            .unavailable
            .iter()
            .filter_map(|(k, why)| {
                let prop = if k == "style" { "*" } else { k.strip_prefix("style.")? };
                Some((prop.to_string(), why.clone()))
            })
            .collect();
        let identity = record.identity;
        let mut element = DevtoolsElement {
            path: devtools_path(node_id),
            node_id,
            tag: identity.tag,
            id: identity.id,
            classes: identity.classes,
            selector: identity.selector,
            breadcrumb,
            rect: lb.border_box.map(Into::into),
            box_model,
            no_box_reason: lb.no_box_reason.clone(),
            computed_styles,
            style_unavailable,
            rule_sources: BTreeMap::new(),
            rule_sources_unavailable: Some(RULE_SOURCES_UNAVAILABLE),
            details_text: String::new(),
        };
        element.details_text = element.render_details();
        Ok(Some(element))
    }

    /// Outline element `node_id` on view `id`'s page (margin, border,
    /// padding and content areas), or clear the outline with `None`.
    /// Returns whether it changed; a change is drawn at the next render.
    pub fn devtools_highlight(&mut self, id: EngineViewId, node_id: Option<usize>) -> Result<bool, EngineError> {
        let view = self.views.get_mut(&id).ok_or(EngineError::ViewNotFound(id))?;
        if view.devtools_highlight == node_id {
            return Ok(false);
        }
        view.devtools_highlight = node_id;
        // A new frame: the presenter commits the list again, with or
        // without the overlay.
        view.frame_generation += 1;
        Ok(true)
    }
}
