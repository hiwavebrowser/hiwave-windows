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

use crate::{inner_text, DomDirty, MutationKind, MutationLog, MutationRecord, ScriptMutations};
use rustkit_dom::{Document, Node, NodeId, NodeType, QuerySelector};
use rustkit_js::{JsError, JsRuntime, JsValue};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

pub(crate) mod editing;
mod node_apis;
mod shadow;

/// Does this element match this selector list? `None` means the list is
/// invalid, and script throws `SyntaxError`.
pub type SelectorMatchFn = Rc<dyn Fn(&Rc<Node>, &str, &HashMap<usize, bool>) -> Option<bool>>;

/// Where the layout put one element, in document coordinates, as the engine
/// last measured it (`DomBindings::set_geometry`). The border box is what
/// `getBoundingClientRect()` and `offsetWidth` report; the client box is the
/// padding box (`clientWidth`); the scroll size is the extent of the content
/// and descendants inside the padding box (`scrollWidth`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoxGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Left and top border widths (`clientLeft`, `clientTop`).
    pub border_left: f32,
    pub border_top: f32,
    pub client_width: f32,
    pub client_height: f32,
    pub scroll_width: f32,
    pub scroll_height: f32,
    /// 0 static, 1 positioned (relative, absolute, sticky), 2 fixed.
    pub position: u8,
    /// The nearest element scroller or transformed box that moves this box
    /// (`x`, `y` are where layout put it, before any of them). Follow it up
    /// to add them up.
    pub scroller: Option<usize>,
    /// This box's own transform, about its origin, in layout coordinates, as
    /// `[a, b, c, d, e, f]`.
    pub transform: Option<[f32; 6]>,
    /// Whether this box is an element scroller itself.
    pub scrolls: bool,
    /// `position: sticky`: where it moves to. A sticky box is on the
    /// `scroller` chain of the boxes under it, since it moves them too.
    pub sticky: Option<rustkit_css::StickyRule>,
    /// In a fixed box the window's scroll does not move: its rect is where
    /// layout put it, wherever the page is.
    pub held: bool,
}

/// Element ids on a box's `scroller` chain that actually scroll (#850 /
/// ec0b831d). Transformed boxes ride the same chain so rects follow paint,
/// but `scrollChain` / `setOwn` must skip them.
pub(crate) fn scrolling_chain_ids(
    geometry: &HashMap<usize, BoxGeometry>,
    start: Option<usize>,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at = start;
    while let Some(id) = at {
        match geometry.get(&id) {
            Some(g) => {
                if g.scrolls {
                    out.push(id);
                }
                at = g.scroller;
            }
            None => break,
        }
    }
    out
}

/// What moves a box from where layout put it (`DomHost::moved_by`).
#[derive(Clone, Copy, Default)]
struct MovedBy {
    /// By the element scrollers above it.
    scrolled: (f32, f32),
    /// By sticky boxes: its own shift and those above it.
    stuck: (f32, f32),
    /// The part of `stuck` from inside the nearest scroller.
    above: (f32, f32),
}

/// The offsets script scrolled a view's elements to, by raw NodeId: one table
/// the bindings and the engine share. Script writes it and reads it back at
/// once; the engine applies it when script settles (paint) and on a hit test.
/// It outlives the boxes: an element that goes `display: none` has its offset
/// back when it returns.
#[derive(Debug, Default)]
pub struct ElementScrolls {
    pub offsets: HashMap<usize, (f32, f32)>,
    /// Bumped by each write, so a layout tree knows it is behind.
    pub generation: u64,
    /// Whether a write has not been painted yet.
    pub unpainted: bool,
}

impl ElementScrolls {
    /// A new document: node ids restart.
    pub fn clear(&mut self) {
        self.offsets.clear();
        self.generation += 1;
        self.unpainted = false;
    }
}

/// What `DomBindings::share_element_scrolls` takes.
pub type SharedElementScrolls = Rc<std::cell::RefCell<ElementScrolls>>;

/// The animated-style layer page script drives through Web Animations,
/// shared between the bindings (script writes it) and the engine's build
/// (which overlays it on each element's cascaded style, above its CSS
/// animations and transitions, and leaves the `style` attribute alone),
/// plus the engine's own CSS animations as script sees them.
#[derive(Debug, Default)]
pub struct AnimatedStyles {
    /// Per raw NodeId, in animation order: `(property, value, composite)`
    /// with composite `replace`, `add` or `accumulate`. An entry composes
    /// on what the entries before it (and the CSS) left.
    pub script: HashMap<usize, Vec<(String, String, String)>>,
    /// Bumped by each write.
    pub generation: u64,
    /// The engine's CSS animations and transitions as of the last layout,
    /// what `getAnimations()` lists.
    pub css: Vec<CssAnimationInfo>,
    /// The properties those animate, per raw NodeId: a `getComputedStyle`
    /// read keeps the published row for them rather than the inline text.
    pub css_props: HashMap<usize, Vec<String>>,
    /// Script's `pause()`/`play()`/`cancel()`/`finish()` on those, for the
    /// engine to apply at its next layout.
    pub controls: Vec<CssAnimationControl>,
}

/// One of the engine's CSS animations or transitions, for script.
#[derive(Debug, Clone, PartialEq)]
pub struct CssAnimationInfo {
    /// The engine's id for this one animation or transition, unique in the
    /// view: script's `CSSAnimation`/`CSSTransition` object is keyed on it,
    /// so a restart or a same-name replacement is a new object.
    pub id: u64,
    /// The element: the originating element of a pseudo-element's.
    pub node: usize,
    /// `::before` or `::after` for a pseudo-element's animation
    /// (`effect.pseudoElement`), `None` for the element's own.
    pub pseudo: Option<&'static str>,
    /// `animation` or `transition`.
    pub kind: &'static str,
    /// The `animation` list index, or the transition's property index.
    pub index: usize,
    /// The `@keyframes` name, or the transitioned property.
    pub name: String,
    /// When it started (its delay counts from here), on the page's clock.
    pub start_ms: f64,
    /// The local time it is held at, when paused.
    pub paused_ms: Option<f64>,
    pub delay_ms: f64,
    pub duration_ms: f64,
    /// `f32::INFINITY` for `infinite`.
    pub iterations: f32,
    pub direction: &'static str,
    pub fill: &'static str,
    pub easing: String,
    /// Its active phase is over.
    pub ended: bool,
}

/// Script's control of one of the engine's CSS animations.
#[derive(Debug, Clone, PartialEq)]
pub struct CssAnimationControl {
    /// The animation's `CssAnimationInfo::id`.
    pub id: u64,
    pub node: usize,
    pub kind: String,
    /// `pause`, `play`, `cancel` or `finish`.
    pub op: String,
}

/// What `DomBindings::share_animated_styles` takes.
pub type SharedAnimatedStyles = Rc<std::cell::RefCell<AnimatedStyles>>;

/// Builds the geometry and computed-style maps of the layout it was made
/// for (`DomBindings::set_geometry_source`).
pub type GeometrySource =
    Box<dyn FnOnce() -> (HashMap<usize, BoxGeometry>, HashMap<usize, String>)>;

/// One element's computed style as `name\tvalue` lines, formatted on the
/// first read of that element (LAT8): a read of one element no longer
/// formats every element's. `None` when the layout has no style for it.
pub type ComputedLines = Rc<dyn Fn(usize) -> Option<String>>;

/// The node under a viewport point, from the layout's hit test. `scroll`
/// is the view's offset. `None` is a miss.
pub type HitTester = Rc<dyn Fn(f32, f32, (f32, f32)) -> Option<usize>>;

/// Where characters `[start, end)` of a text node (by NodeId, offsets in
/// characters) are in the layout: `[x, y, width, height]` per line, in
/// document coordinates (`Range.getClientRects`).
///
/// The second pair is the same span counted after white-space collapsing
/// (each run of white space as one character): the numbering of a run whose
/// white space collapses.
pub type TextRects = Rc<dyn Fn(usize, (usize, usize), (usize, usize)) -> Vec<[f32; 4]>>;

/// Control writes script made that the engine has not taken yet. A forced
/// layout copies them into the engine's edit state before it builds, and
/// drains what it applied (`ForcedLayout`).
#[derive(Default)]
pub struct ControlWrites {
    pub values: Vec<(usize, String)>,
    pub checked: Vec<(usize, Option<bool>)>,
    /// LAT19: the only write since the last published layout is Web
    /// Animations' layer: an animation frame over the published boxes
    /// answers the read without a build when it can. LAT22: whatever
    /// properties the layer holds, not only the paint ones (`transform`,
    /// `opacity`, colours); the frame itself refuses when a value a
    /// layout places moved (`LayoutCtx::animation_frame`), and the read
    /// then builds. Until 2026-11-12 one property outside the paint list
    /// (a constant `width` beside a `transform`) made every read of every
    /// frame build the whole tree.
    pub layer_only: bool,
    /// LAT22 probe: what the read was and what was pending, for the
    /// engine's log line when the read builds the tree.
    pub probe: ForcedReadProbe,
}

/// LAT22 probe (diagnostics only; shapes, never page content): the read
/// that asked for a forced layout and the writes pending at it.
#[derive(Default, Clone, Copy, Debug)]
pub struct ForcedReadProbe {
    /// The read's kind (`rect`, `offset`, `computed_style`, ...).
    pub read: &'static str,
    /// The kind of the first write since the last published layout.
    pub cause: &'static str,
    /// The element read, when the read is of one element.
    pub read_node: Option<usize>,
    /// A write other than Web Animations' layer is pending (a style,
    /// attribute, child-list, text or form write, logged or not).
    pub other_write_pending: bool,
    /// A write a layout can see is pending (false: only paint-layer
    /// writes, which is what `layer_only` says).
    pub layout_write_pending: bool,
}

/// A layout of the DOM as it is now: what a forced layout publishes.
pub struct ForcedGeometry {
    /// Builds the geometry and computed-style maps of this layout, on the
    /// first read that needs them (as `DomBindings::set_geometry_source`).
    pub source: GeometrySource,
    pub hit_tester: Option<HitTester>,
    /// The layout's text geometry, for `Range` rects.
    pub text_rects: Option<TextRects>,
    /// Formats one element's computed style of this layout on its first
    /// read (LAT8); `None` keeps whatever `source` returns.
    pub computed_lines: Option<ComputedLines>,
    /// The furthest the view can scroll over this layout, as
    /// `DomBindings::set_scroll_state` takes it.
    pub max_scroll: (f32, f32),
    /// LAT19: answered from an animation frame over the published boxes
    /// (`ControlWrites::layer_only`), not a build: not a forced layout.
    pub framed: bool,
}

/// Lays the page out again for a read of geometry or computed style that
/// follows a script write in the same task (forced layout,
/// `DomBindings::set_forced_layout`). `None` when it cannot now: the read
/// answers from the last layout published.
pub type ForcedLayout = Rc<dyn Fn(&mut ControlWrites) -> Option<ForcedGeometry>>;

/// How many forced layouts one script task gets before its reads answer
/// from the last layout (Prometheus, #812 review): a write-read loop that
/// never settles cannot hold the page thread forever. cnn's header loop
/// needs 8. A count, not a time: a time limit made the page's answer depend
/// on the machine's load (at 1 s, cnn's loop ran out on a loaded seat and
/// laid the header out as before forced layout).
pub(crate) const FORCED_LAYOUTS_PER_TASK: u64 = 256;

/// The document the host functions read, and its generation.
#[derive(Default)]
pub(crate) struct DomHost {
    document: Option<Rc<Document>>,
    generation: u32,
    /// The injected selector matcher (`DomBindings::set_selector_matcher`).
    pub(crate) matcher: Option<SelectorMatchFn>,
    /// Where each element's box is, by NodeId; empty until the engine has
    /// laid the page out (`DomBindings::set_geometry`).
    pub(crate) geometry: HashMap<usize, BoxGeometry>,
    /// The computed style of each laid-out element, by NodeId, as
    /// `name\tvalue` lines (`DomBindings::set_computed_styles`); with
    /// `computed_lines`, the elements read so far.
    pub(crate) computed: RefCell<HashMap<usize, String>>,
    /// Formats an element's computed style of the last layout on its first
    /// read (`DomBindings::set_computed_lines`, LAT8).
    pub(crate) computed_lines: Option<ComputedLines>,
    /// What builds `geometry` and `computed` from the last layout, until
    /// script first reads either (`DomBindings::set_geometry_source`).
    pub(crate) geometry_source: Option<GeometrySource>,
    /// How many times a pending source was built (`DomBindings::geometry_builds`).
    pub(crate) geometry_builds: u64,
    /// Lays out the document the engine last published a layout of.
    pub(crate) forced_layout: Option<ForcedLayout>,
    /// LAT22 probe: the element of the geometry read in progress.
    pub(crate) forced_read_node: Option<usize>,
    /// Forced layouts since the engine last took the count
    /// (`DomBindings::take_forced_layouts`).
    pub(crate) forced_layouts: u64,
    /// The forced layouts against this task's budget, and their time (ms,
    /// for the decline log), since the engine last published a layout or
    /// took the count.
    forced_in_task: u64,
    forced_ms: f64,
    /// The forced layouts since the engine last took the count, by cause,
    /// with their time (`DomBindings::take_forced_report`).
    forced_report: crate::ForcedLayoutReport,
    /// Whether the page's sheets have a `:has()` rule now (the engine's
    /// rule index), so a write deep in a subtree can restyle an ancestor.
    /// `None` until the engine installs it: assumed yes.
    pub(crate) has_rules: Option<Rc<dyn Fn() -> bool>>,
    /// Whether some rule of the page's sheets reads the `style` attribute (`[style*=..]`) or
    /// uses `:has()` (the engine's rule index), so an inline style write can change which rules
    /// match. `None` until the engine installs it: assumed yes (TREE-REUSE step (2)).
    pub(crate) style_attr_rules: Option<Rc<dyn Fn() -> bool>>,
    /// NATIVE-SETPROPERTY: each element's inline declarations as last parsed, with the attribute text
    /// they were parsed from (`style_decls`). A pure function of the text, so an entry is right for any
    /// element whose attribute holds that text; at most `STYLE_DECLS_KEPT` are kept.
    style_cache: RefCell<HashMap<usize, (String, Rc<Vec<crate::inline_style::Decl>>)>>,
    /// Whether a read since the count was taken answered from the last
    /// layout instead (budget spent, or the build state busy): logged once.
    forced_declined: bool,
    /// The view's scroll, in the same coordinates as `geometry`, so a
    /// viewport point can be turned into a document point.
    scroll: (f32, f32),
    /// The view's element scroll offsets (web_scroll.js).
    pub(crate) element_scrolls: SharedElementScrolls,
    /// The view's animated-style layer and CSS animations (web_animations.js).
    pub(crate) animated: SharedAnimatedStyles,
    /// The scroll state `window.scrollTo` clamps against (web_scroll.rs);
    /// a forced layout moves its maximum.
    pub(crate) scroll_state: crate::web_scroll::SharedScroll,
    /// Layout hit test `(viewport x, viewport y, scroll) -> node id`,
    /// installed with a layout. Absent until then: a point answers null.
    hit_tester: Option<HitTester>,
    /// Something on the page can hear the user's wheel: a `wheel` or
    /// `mousewheel` listener, or an `onwheel` / `onmousewheel` handler
    /// (assigned, or an attribute). Without one a scroll runs no script.
    wants_wheel: std::cell::Cell<bool>,
    /// One of those can cancel it: a listener that is not passive, or a
    /// handler (never passive). Without one the engine does not ask which
    /// listeners are on the path; the wheel goes after the scroll.
    wheel_blocking: std::cell::Cell<bool>,
    /// A wheel handler attribute (`onwheel`, `onmousewheel`) was parsed or
    /// written in this document: it can hear and cancel the wheel.
    wheel_handler_attribute: std::cell::Cell<bool>,
    /// The layout's text geometry (`TextRects`), installed with a layout.
    text_rects: Option<TextRects>,
    /// HTML §4.10.5.4 "dirty value" of the `<input>`/`<textarea>` controls
    /// that script or the user changed, by NodeId. A control missing here
    /// shows its default value (the `value` attribute, or a textarea's text).
    values: HashMap<usize, String>,
    /// Script value writes the engine has not yet copied into its edit
    /// state, which is what layout paints (`DomBindings::take_value_writes`).
    value_writes: Vec<(usize, String)>,
    /// Checkedness changes the engine has not yet taken
    /// (`DomBindings::take_checked_writes`): (NodeId, the control's
    /// checkedness, or `None` when it follows its `checked` attribute
    /// again). The state itself is script's (web_forms.js).
    checked_writes: Vec<(usize, Option<bool>)>,
    /// The checkedness script holds for each control that no longer
    /// follows its `checked` attribute, by NodeId. The selector matcher
    /// reads it, so `:checked` in a query answers what the page paints.
    checked: HashMap<usize, bool>,
    /// Forms whose `submit` event was not cancelled, with the button that
    /// submitted each, by NodeId (`DomBindings::take_submit_requests`).
    submit_requests: Vec<(usize, Option<usize>)>,
    /// URLs script asked this document to navigate to (`location.href =`,
    /// `link.click()`, `form.submit()`), oldest first
    /// (`DomBindings::take_navigation_requests`).
    /// Each with whether it replaces the current history entry.
    navigation_requests: Vec<(String, bool)>,
    /// Script elements script inserted with a `src` (HTML §4.12.1.1
    /// "prepare the script element"), oldest first: the engine fetches and
    /// runs them (`DomBindings::take_inserted_scripts`).
    inserted_scripts: Vec<crate::InsertedScript>,
    /// HTML §4.12.3 template contents: each `<template>`'s content
    /// fragment, by the template's NodeId (see `adopt_template_contents`).
    templates: RefCell<HashMap<usize, Rc<Node>>>,
    /// The `<img>` elements (by NodeId) whose `src` or `srcset` script has
    /// written since the engine last took them, in or out of the document
    /// and whether or not the value changed: writing either starts the
    /// image's load over (`DomBindings::take_image_writes`).
    pub(crate) image_writes: RefCell<HashSet<usize>>,
}

pub(crate) type SharedDomHost = Rc<RefCell<DomHost>>;

/// How many elements' inline declarations `DomHost::style_decls` keeps (then it starts again).
const STYLE_DECLS_KEPT: usize = 4096;

impl DomHost {
    /// The inline declarations of element `node` whose `style` attribute is `text`: parsed once per
    /// text, then kept (NATIVE-SETPROPERTY; `inline_style::parse_decls`).
    pub(crate) fn style_decls(&self, node: usize, text: &str) -> Rc<Vec<crate::inline_style::Decl>> {
        let mut cache = self.style_cache.borrow_mut();
        if let Some((kept, decls)) = cache.get(&node) {
            if kept == text {
                return decls.clone();
            }
        }
        let decls = Rc::new(crate::inline_style::parse_decls(text));
        if cache.len() >= STYLE_DECLS_KEPT {
            cache.clear();
        }
        cache.insert(node, (text.to_string(), decls.clone()));
        decls
    }

    /// Keep `decls` as element `node`'s declarations for attribute text `text`, which a native write is
    /// about to write: only a list that reads back from `text` as itself
    /// (`inline_style::reads_back_as_itself`).
    pub(crate) fn keep_style_decls(&self, node: usize, text: &str, decls: Vec<crate::inline_style::Decl>) {
        let mut cache = self.style_cache.borrow_mut();
        if cache.len() >= STYLE_DECLS_KEPT {
            cache.clear();
        }
        cache.insert(node, (text.to_string(), Rc::new(decls)));
    }

    /// Build the geometry and computed styles from the pending source, if
    /// the layout changed since script last read them.
    pub(crate) fn ensure_geometry(&mut self) {
        if let Some(source) = self.geometry_source.take() {
            let computed;
            (self.geometry, computed) = source();
            // A source with `computed_lines` returns no styles: what was
            // formatted for this layout already stays.
            if !computed.is_empty() {
                *self.computed.borrow_mut() = computed;
            }
            self.geometry_builds += 1;
        }
    }

    /// An element's computed-style lines from the last layout: formatted on
    /// the first read when the layout publishes them lazily (LAT8).
    pub(crate) fn computed_of(&self, node: usize) -> Option<String> {
        if let Some(lines) = self.computed.borrow().get(&node) {
            return Some(lines.clone());
        }
        let lines = (self.computed_lines.as_ref()?)(node)?;
        self.computed.borrow_mut().insert(node, lines.clone());
        Some(lines)
    }

    /// The rect of a box under a transform, its own or one above it, as x, y,
    /// width, height: the bounding box of its corners taken up the chain. At
    /// each box its transform, about where layout put it, then its sticky
    /// shift, which carries the transform's origin along; then the scroll of
    /// the scroller above, inside that scroller's own transform. `None` when
    /// no transform is on the chain, and `moved_by` is the whole answer.
    fn transformed_rect(&self, geometry: &BoxGeometry) -> Option<(f32, f32, f32, f32)> {
        let mut chain = vec![(None, geometry)];
        let mut at = geometry.scroller;
        while let Some((node, above)) = at.and_then(|node| Some((node, self.geometry.get(&node)?))) {
            chain.push((Some(node), above));
            at = above.scroller;
        }
        if chain.iter().all(|(_, g)| g.transform.is_none()) {
            return None;
        }
        let scrolls = self.element_scrolls.borrow();
        // Each shift depends on those above it, so these go from the top.
        let mut shifts = vec![(0.0, 0.0); chain.len()];
        let mut moved = MovedBy::default();
        for (i, (node, g)) in chain.iter().enumerate().rev() {
            let before = moved.stuck;
            self.add_sticky(g, &scrolls.offsets, &mut moved);
            shifts[i] = (moved.stuck.0 - before.0, moved.stuck.1 - before.1);
            if node.is_some() && g.scrolls {
                moved.above = (0.0, 0.0);
            }
        }
        let (x, y, r, b) = (geometry.x, geometry.y, geometry.x + geometry.width, geometry.y + geometry.height);
        let mut corners = [(x, y), (r, y), (x, b), (r, b)];
        for (i, (node, g)) in chain.iter().enumerate() {
            let scroll = node.filter(|_| g.scrolls).and_then(|node| scrolls.offsets.get(&node));
            let (dx, dy) = scroll.copied().unwrap_or((0.0, 0.0));
            for c in corners.iter_mut() {
                let (px, py) = (c.0 - dx, c.1 - dy);
                let (tx, ty) = match g.transform {
                    Some(m) => (m[0] * px + m[2] * py + m[4], m[1] * px + m[3] * py + m[5]),
                    None => (px, py),
                };
                *c = (tx + shifts[i].0, ty + shifts[i].1);
            }
        }
        let (mut left, mut top) = corners[0];
        let (mut right, mut bottom) = corners[0];
        for c in &corners[1..] {
            left = left.min(c.0);
            top = top.min(c.1);
            right = right.max(c.0);
            bottom = bottom.max(c.1);
        }
        Some((left, top, right - left, bottom - top))
    }

    /// Where the window is now, script's own scroll included.
    fn window_scroll(&self) -> (f32, f32) {
        let state = self.scroll_state.borrow();
        (state.x, state.y)
    }

    /// What the scrollers and sticky boxes above a box, and the box itself
    /// when it is sticky, have moved it by. Live: script reads a rect right
    /// after a scroll, before the engine hears of it.
    fn moved_by(&self, geometry: &BoxGeometry) -> MovedBy {
        let scrolls = self.element_scrolls.borrow();
        let mut moved = match geometry.scroller {
            Some(above) => self.moved_by_chain(above, &scrolls.offsets),
            None => MovedBy::default(),
        };
        self.add_sticky(geometry, &scrolls.offsets, &mut moved);
        moved
    }

    /// `moved_by` for what is under the box `node`, its own scroll included.
    fn moved_by_chain(&self, node: usize, offsets: &HashMap<usize, (f32, f32)>) -> MovedBy {
        let Some(geometry) = self.geometry.get(&node) else {
            return MovedBy::default();
        };
        let mut moved = match geometry.scroller {
            Some(above) => self.moved_by_chain(above, offsets),
            None => MovedBy::default(),
        };
        self.add_sticky(geometry, offsets, &mut moved);
        if geometry.scrolls {
            if let Some((dx, dy)) = offsets.get(&node) {
                moved.scrolled = (moved.scrolled.0 + dx, moved.scrolled.1 + dy);
            }
            moved.above = (0.0, 0.0);
        }
        moved
    }

    fn add_sticky(&self, geometry: &BoxGeometry, offsets: &HashMap<usize, (f32, f32)>, moved: &mut MovedBy) {
        let Some(rule) = geometry.sticky else {
            return;
        };
        let scroll = match rule.port {
            rustkit_css::StickyPort::Window => self.window_scroll(),
            rustkit_css::StickyPort::Still => (0.0, 0.0),
            rustkit_css::StickyPort::Element(node) => offsets.get(&node).copied().unwrap_or((0.0, 0.0)),
        };
        let (x, y) = rule.shift(scroll, moved.above);
        moved.stuck = (moved.stuck.0 + x, moved.stuck.1 + y);
        moved.above = (moved.above.0 + x, moved.above.1 + y);
    }

    /// What `ensure_geometry` gives, from a layout of the DOM as it is now:
    /// when script wrote to the DOM since the last layout was published, the
    /// page is laid out again first (forced layout), as in a browser.
    /// `read` names the read for the forced-layout log (`offset`, `rect`,
    /// `computed_style`, ...).
    pub(crate) fn ensure_fresh_geometry(host: &SharedDomHost, dirty: &crate::DirtyMark, read: &'static str) {
        Self::force_layout_if_written(host, dirty, read);
        host.borrow_mut().ensure_geometry();
    }

    /// The forced layout half of `ensure_fresh_geometry`, for reads that
    /// need the layout current but not the geometry maps (the scroll
    /// maximum).
    pub(crate) fn force_layout_if_written(host: &SharedDomHost, dirty: &crate::DirtyMark, read: &'static str) {
        if !dirty.needs_layout_for(read) {
            return;
        }
        // Only a paint write is pending: an inline style's (TREE-REUSE step (2)), which names
        // itself, or Web Animations' layer (LAT7c), which does not.
        let cause = match dirty.written_since_layout() {
            true => dirty.cause().unwrap_or("other"),
            false => dirty.cause().unwrap_or("animation"),
        };
        let forced = {
            let mut h = host.borrow_mut();
            let spent = h.forced_in_task >= FORCED_LAYOUTS_PER_TASK;
            match (h.forced_layout.clone(), spent) {
                (Some(_), true) => {
                    h.note_forced_decline("budget spent");
                    None
                }
                (forced, _) => forced,
            }
        };
        let Some(forced) = forced else {
            return;
        };
        let started = std::time::Instant::now();
        // Outside the host borrow: the build does not read the host, and
        // runs no script.
        let mut writes = {
            let mut h = host.borrow_mut();
            ControlWrites {
                values: std::mem::take(&mut h.value_writes),
                checked: std::mem::take(&mut h.checked_writes),
                layer_only: dirty.only_layer_written_since_layout(),
                probe: ForcedReadProbe {
                    read,
                    cause,
                    read_node: h.forced_read_node.take(),
                    other_write_pending: dirty.beyond_animation_layer(),
                    layout_write_pending: dirty.written_since_layout(),
                },
            }
        };
        let laid = forced(&mut writes);
        let mut h = host.borrow_mut();
        let ms = started.elapsed().as_secs_f64() * 1000.0;
        h.forced_ms += ms;
        // What the layout did not apply stays for the engine's flush.
        writes.values.append(&mut h.value_writes);
        h.value_writes = writes.values;
        writes.checked.append(&mut h.checked_writes);
        h.checked_writes = writes.checked;
        let Some(laid) = laid else {
            h.note_forced_decline("build state in use");
            return;
        };
        h.geometry.clear();
        h.computed.borrow_mut().clear();
        h.computed_lines = laid.computed_lines;
        h.geometry_source = Some(laid.source);
        h.hit_tester = laid.hit_tester;
        h.text_rects = laid.text_rects;
        // The document may have grown or shrunk: scrollTo clamps to the new
        // end, and an offset past it comes back to it.
        let scroll = h.scroll_state.clone();
        let mut s = scroll.borrow_mut();
        (s.max_x, s.max_y) = laid.max_scroll;
        s.x = s.x.min(s.max_x.max(0.0));
        s.y = s.y.min(s.max_y.max(0.0));
        h.scroll = (s.x, s.y);
        h.forced_in_task += 1;
        if laid.framed {
            // LAT19: a frame over the published boxes, not a layout.
            h.forced_report.framed += 1;
            h.forced_report.framed_ms += ms;
        } else {
            h.forced_layouts += 1;
            h.forced_report.note(cause, read, ms);
        }
        dirty.laid_out();
    }

    /// A read that wanted a forced layout answers from the last layout:
    /// one log line per script task (until `take_forced_layouts`).
    fn note_forced_decline(&mut self, why: &'static str) {
        if !std::mem::replace(&mut self.forced_declined, true) {
            tracing::info!(
                why,
                forced_layouts = self.forced_in_task,
                forced_ms = self.forced_ms,
                "Forced layout declined; reads answer from the last layout"
            );
        }
    }

    /// A new budget for forced layouts: the engine took the task's count,
    /// or published a layout of its own.
    pub(crate) fn renew_forced_budget(&mut self) {
        self.forced_in_task = 0;
        self.forced_ms = 0.0;
        self.forced_declined = false;
    }

    /// The forced layouts since the last call, which starts a new task's
    /// count and budget.
    pub(crate) fn take_forced_layouts(&mut self) -> u64 {
        self.renew_forced_budget();
        self.forced_report = crate::ForcedLayoutReport::default();
        std::mem::take(&mut self.forced_layouts)
    }

    /// `take_forced_layouts`, with the causes and time of the layouts.
    pub(crate) fn take_forced_report(&mut self) -> crate::ForcedLayoutReport {
        let report = std::mem::take(&mut self.forced_report);
        let count = self.take_forced_layouts();
        crate::ForcedLayoutReport { count, ..report }
    }

    pub(crate) fn note_scroll(&mut self, offset: (f32, f32)) {
        self.scroll = offset;
    }

    pub(crate) fn set_hit_tester(&mut self, tester: Option<HitTester>) {
        self.hit_tester = tester;
    }

    pub(crate) fn wants_wheel(&self) -> bool {
        self.wants_wheel.get()
    }

    pub(crate) fn wheel_blocking(&self) -> bool {
        self.wheel_blocking.get()
    }

    /// A wheel handler attribute was written or parsed: something can hear
    /// and cancel the wheel.
    pub(crate) fn note_wheel_handler_attribute(&self, name: &str) {
        if name.eq_ignore_ascii_case("onwheel") || name.eq_ignore_ascii_case("onmousewheel") {
            self.wheel_handler_attribute.set(true);
            self.wants_wheel.set(true);
            self.wheel_blocking.set(true);
        }
    }

    pub(crate) fn set_text_rects(&mut self, rects: Option<TextRects>) {
        self.text_rects = rects;
    }

    /// Bind `document`, returning the new generation.
    pub(crate) fn bind(&mut self, document: Rc<Document>) -> u32 {
        // A new document: what can hear the wheel is learned again, from
        // its handler attributes now and its listeners as they come.
        self.wants_wheel.set(false);
        self.wheel_blocking.set(false);
        self.wheel_handler_attribute.set(false);
        let mut pending = vec![document.root().clone()];
        while let Some(node) = pending.pop() {
            if let NodeType::Element { attributes, .. } = &node.node_type {
                for name in attributes.keys() {
                    self.note_wheel_handler_attribute(name);
                }
            }
            pending.extend(node.children());
        }
        let templates = self.templates.get_mut();
        templates.clear();
        // The page's own document owns itself and every node it has; what
        // is made or moved in later takes its document from there.
        document.root().set_owner(document.root().id.raw());
        adopt_template_contents(&document, document.root(), templates);
        self.document = Some(document);
        self.generation += 1;
        // The engine installs one for the new document with its first layout.
        self.forced_layout = None;
        self.values.clear();
        self.value_writes.clear();
        self.checked_writes.clear();
        self.checked.clear();
        self.submit_requests.clear();
        self.navigation_requests.clear();
        self.inserted_scripts.clear();
        self.image_writes.get_mut().clear();
        self.generation
    }

    /// Record the value the user typed into a control, so script reads it.
    pub(crate) fn sync_value(&mut self, node: usize, value: String) {
        self.values.insert(node, value);
    }

    pub(crate) fn take_value_writes(&mut self) -> Vec<(usize, String)> {
        std::mem::take(&mut self.value_writes)
    }

    pub(crate) fn take_checked_writes(&mut self) -> Vec<(usize, Option<bool>)> {
        std::mem::take(&mut self.checked_writes)
    }

    pub(crate) fn take_submit_requests(&mut self) -> Vec<(usize, Option<usize>)> {
        std::mem::take(&mut self.submit_requests)
    }

    pub(crate) fn navigation_requested(&self) -> bool {
        !self.navigation_requests.is_empty()
    }

    pub(crate) fn take_navigation_requests(&mut self) -> Vec<(String, bool)> {
        std::mem::take(&mut self.navigation_requests)
    }

    pub(crate) fn take_inserted_scripts(&mut self) -> Vec<crate::InsertedScript> {
        std::mem::take(&mut self.inserted_scripts)
    }

    fn document_for(&self, generation: &JsValue) -> Option<&Rc<Document>> {
        match generation {
            JsValue::Number(n) if *n == self.generation as f64 => self.document.as_ref(),
            _ => None,
        }
    }

    /// The NodeId of the node `args[0]` (generation) and `args[1]` name,
    /// if it is in the bound document.
    pub(crate) fn node_of(host: &SharedDomHost, args: &[JsValue]) -> Option<usize> {
        host.borrow().node(args).map(|n| n.id.raw())
    }

    /// The `src` and `srcset` attributes of the bound document's node
    /// `node` as they are now, when it is an `<img>` element.
    pub(crate) fn img_sources(&self, node: usize) -> Option<(Option<String>, Option<String>)> {
        let node = self.document.as_ref()?.get_node(NodeId::new(node))?;
        if !node.tag_name().is_some_and(|tag| tag.eq_ignore_ascii_case("img")) {
            return None;
        }
        Some((node.get_attribute("src").map(str::to_string), node.get_attribute("srcset").map(str::to_string)))
    }

    /// The `sandbox` attribute of the bound document's node `node`, as it
    /// is now (`None`: no such attribute, or no such node).
    pub(crate) fn frame_sandbox(&self, node: usize) -> Option<String> {
        let node = self.document.as_ref()?.get_node(NodeId::new(node))?;
        node.get_attribute("sandbox").map(str::to_string)
    }

    /// The bound document's iframes that have a `sandbox` attribute, with
    /// its value, by NodeId.
    pub(crate) fn frames_with_sandbox(&self) -> HashMap<usize, String> {
        let mut found = HashMap::new();
        if let Some(document) = &self.document {
            for frame in document.get_elements_by_tag_name("iframe") {
                if let Some(value) = frame.get_attribute("sandbox") {
                    found.insert(frame.id.raw(), value.to_string());
                }
            }
        }
        found
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

/// DOM §4.2.2.1 "host-including inclusive ancestor": is `ancestor` `node`
/// or one of its ancestors, climbing from a shadow root to its host, so a
/// host cannot be inserted into its own shadow tree?
fn is_host_including_inclusive_ancestor(ancestor: &Rc<Node>, node: &Rc<Node>) -> bool {
    let mut current = Some(node.clone());
    while let Some(n) = current {
        if n.id == ancestor.id {
            return true;
        }
        current = n.parent().or_else(|| n.shadow_host());
    }
    false
}

fn is_child_of(node: &Rc<Node>, parent: &Rc<Node>) -> bool {
    node.parent().is_some_and(|p| p.id == parent.id)
}

/// DOM §4.2.3 "pre-insert" `node` into `parent` before `child` (`None`
/// appends). Returns the DOMException name on a failed validity check,
/// before anything is touched. `replacing`: the caller takes `child` out
/// next (replaceChild, replaceWith), so a Document is checked by the
/// "replace" rules, which do not count `child` itself.
fn pre_insert(
    parent: &Rc<Node>,
    node: &Rc<Node>,
    child: Option<Rc<Node>>,
    replacing: bool,
) -> Result<(), &'static str> {
    let document = matches!(parent.node_type, NodeType::Document);
    if !document && !parent.is_element() && !is_fragment(parent) {
        return Err("HierarchyRequestError");
    }
    if is_host_including_inclusive_ancestor(node, parent) || node.is_shadow_root() {
        return Err("HierarchyRequestError");
    }
    if child.as_ref().is_some_and(|c| !is_child_of(c, parent)) {
        return Err("NotFoundError");
    }
    match node.node_type {
        NodeType::Document => return Err("HierarchyRequestError"),
        NodeType::DocumentType { .. } if !document => return Err("HierarchyRequestError"),
        NodeType::Text(_) if document => return Err("HierarchyRequestError"),
        _ => {}
    }
    if document {
        document_child_check(parent, node, child.as_ref(), replacing)?;
    }
    // Inserting a node before itself inserts it before its next sibling.
    let child = match child {
        Some(c) if c.id == node.id => node.next_sibling(),
        other => other,
    };
    // Inserting a fragment inserts its children, in order, and empties it.
    let nodes = if is_fragment(node) {
        node.children()
    } else {
        vec![node.clone()]
    };
    for node in nodes {
        node.remove_from_parent();
        match &child {
            Some(c) => parent.insert_before(node, c.clone()),
            None => parent.append_child(node),
        }
    }
    Ok(())
}

/// DOM §4.2.3 step 6 of "ensure pre-insert validity" (and step 6 of
/// "replace"): a Document has at most one element child and one doctype,
/// the doctype before the element.
fn document_child_check(
    document: &Rc<Node>,
    node: &Rc<Node>,
    child: Option<&Rc<Node>>,
    replacing: bool,
) -> Result<(), &'static str> {
    let is_doctype = |n: &Rc<Node>| matches!(n.node_type, NodeType::DocumentType { .. });
    let kids = document.children();
    // When replacing, `child` is about to leave and does not count.
    let counted = |n: &&Rc<Node>| !(replacing && child.is_some_and(|c| c.id == n.id));
    let has_element = kids.iter().filter(counted).any(|n| n.is_element());
    let has_doctype = kids.iter().filter(counted).any(is_doctype);
    let at = child.and_then(|c| kids.iter().position(|k| k.id == c.id));
    let doctype_after = at.is_some_and(|i| kids[i + 1..].iter().any(is_doctype));
    let element_before = at.is_some_and(|i| kids[..i].iter().any(|n| n.is_element()));
    let before_doctype = !replacing && child.is_some_and(is_doctype);
    let one_element = |elements: usize| {
        elements == 1 && (has_element || before_doctype || doctype_after)
    };
    let bad = match node.node_type {
        NodeType::DocumentFragment => {
            let inner = node.children();
            let elements = inner.iter().filter(|n| n.is_element()).count();
            elements > 1
                || inner.iter().any(|n| matches!(n.node_type, NodeType::Text(_)))
                || one_element(elements)
        }
        NodeType::Element { .. } => one_element(1),
        NodeType::DocumentType { .. } => {
            has_doctype || element_before || (child.is_none() && has_element)
        }
        _ => false,
    };
    match bad {
        true => Err("HierarchyRequestError"),
        false => Ok(()),
    }
}

fn is_fragment(node: &Node) -> bool {
    matches!(node.node_type, NodeType::DocumentFragment)
}

fn is_template(node: &Node) -> bool {
    is_html_element(node) && node.tag_name() == Some("template")
}

/// HTML §4.12.3: a parsed `<template>`'s children belong to its content,
/// a DocumentFragment outside the tree, not to the element. Gives every
/// template in `root`'s subtree (`root` included, and templates nested in
/// the moved content) a content fragment and moves its children there.
/// Run on freshly parsed nodes only: children script appends to a
/// template later stay real children, as in the spec.
/// Where the table of template contents keeps the inert document that
/// owns them all (HTML §4.12.3 "template contents owner document"). No
/// node has NodeId 0.
const TEMPLATE_OWNER: usize = 0;

/// The document that owns every template's contents, made on first use.
fn template_owner(document: &Document, templates: &mut HashMap<usize, Rc<Node>>) -> usize {
    templates
        .entry(TEMPLATE_OWNER)
        .or_insert_with(|| {
            let doc = document.create_node(NodeType::Document);
            doc.set_owner(doc.id.raw());
            doc
        })
        .id
        .raw()
}

fn adopt_template_contents(
    document: &Document,
    root: &Rc<Node>,
    templates: &mut HashMap<usize, Rc<Node>>,
) {
    let owner = template_owner(document, templates);
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if is_template(&node) {
            let content = templates
                .entry(node.id.raw())
                .or_insert_with(|| {
                    // A template's contents are in the inert document that
                    // holds every template's contents (its owner document).
                    let fragment = document.create_node(NodeType::DocumentFragment);
                    fragment.set_owner(owner);
                    fragment
                })
                .clone();
            for child in node.children() {
                child.remove_from_parent();
                content.append_child(child);
            }
            pending.push(content);
        }
        pending.extend(node.shadow_root());
        pending.extend(node.children());
    }
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
            let replacing = matches!(args.get(5), Some(JsValue::Boolean(true)));
            pre_insert(&parent, &node, child, replacing)
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
        // DOMParser (HTML §8.5.1): parse `data` as a whole HTML document,
        // then copy it under a detached Document node here. Its nodes share
        // this Document's node table (so wrappers and tree moves work on
        // them) but are in no rendered tree.
        // Range.createContextualFragment: the fragment parsing algorithm
        // with `data` as the context element's tag and the HTML in the
        // next argument. The fragment is detached. Script marks its
        // `<script>` elements so they run when the fragment is inserted.
        if kind == "fragmentHTML" {
            let context = if data.is_empty() { "body" } else { data.as_str() };
            let html = string_arg(args, 3).unwrap_or("");
            // The fragment is the range's document's (`args[4]`, its
            // Document node) and is parsed with that document's scripting
            // flag: on only for the page's own document when it runs
            // script; off for any other, or when the document is unknown.
            let owner = number_arg(args, 4)
                .filter(|n| *n >= 0.0)
                .and_then(|n| document.get_node(NodeId::new(n as usize)))
                .filter(|n| matches!(n.node_type, NodeType::Document))
                .map_or(0, |n| n.owner_document_id());
            let scripting = document.scripting() && owner == document.root().id.raw();
            let nodes = document
                .parse_fragment_scripted(html, context, true, false, scripting)
                .map_err(|_| "SyntaxError")?;
            let fragment = document.create_node(NodeType::DocumentFragment);
            fragment.set_owner(owner);
            let templates = &mut host.templates.borrow_mut();
            for child in nodes {
                adopt_template_contents(document, &child, templates);
                fragment.append_child(child);
            }
            return Ok((node_id(Some(fragment)), DomDirty::Clean));
        }
        if kind == "document" {
            let parsed = Document::parse_html(&data).map_err(|_| "SyntaxError")?;
            let doc = document.create_node(NodeType::Document);
            // Scripting is off in it: what is put in it is its.
            doc.set_owner(doc.id.raw());
            let templates = &mut host.templates.borrow_mut();
            for child in parsed.root().children() {
                // The parsed tree's NodeIds are its own: clone with an
                // empty template table, then give its templates content.
                doc.append_child(clone_node(document, &child, true, &mut HashMap::new()));
            }
            // An HTML document always has a body, but rustkit-html drops
            // it (and the text) for text-only input; supply an empty one.
            // A frameset document's body element is its frameset (§3.1.3).
            if let Some(html) = doc
                .children()
                .into_iter()
                .find(|n| n.tag_name() == Some("html"))
            {
                if !html.children().iter().any(|n| matches!(n.tag_name(), Some("body") | Some("frameset"))) {
                    html.append_child(document.create_node(NodeType::Element {
                        tag_name: String::from("body"),
                        namespace: String::from("http://www.w3.org/1999/xhtml"),
                        attributes: Default::default(),
                    }));
                }
            }
            adopt_template_contents(document, &doc, templates);
            return Ok((node_id(Some(doc)), DomDirty::Clean));
        }
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
            "fragment" => NodeType::DocumentFragment,
            _ => return Err("NotSupportedError"),
        };
        // A detached node is in no tree, so nothing needs a restyle yet.
        let node = document.create_node(node_type);
        adopt_template_contents(document, &node, &mut host.templates.borrow_mut());
        return Ok((node_id(Some(node)), DomDirty::Clean));
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
            // An `<img>` in a template's contents is in an inert document:
            // Chrome does not fetch it, so the engine is not told of it.
            if op == "setAttr" && tag_name == "img" && matches!(name.as_str(), "src" | "srcset") {
                let mut top = node.clone();
                while let Some(parent) = top.parent() {
                    top = parent;
                }
                let in_template = host.templates.borrow().values().any(|content| Rc::ptr_eq(content, &top));
                if !in_template {
                    host.image_writes.borrow_mut().insert(node.id.raw());
                }
            }
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
        ("setText", NodeType::Text(old) | NodeType::Comment(old)) => {
            let data = string_arg(args, 3).unwrap_or("").to_string();
            // LAT7: the same data again changes nothing.
            if *old == data {
                return Ok((JsValue::Null, DomDirty::Clean));
            }
            let (node_type, bucket) = match node.node_type {
                // Pin §3.3: a text change relayouts.
                NodeType::Text(_) => (NodeType::Text(data), DomDirty::Layout),
                // A comment is never styled or laid out.
                _ => (NodeType::Comment(data), DomDirty::Clean),
            };
            document.replace_node_data(node.id, node_type);
            Ok((JsValue::Null, bucket))
        }
        // DOM §4.4 textContent setter on an element or fragment: replace
        // all children with one Text node (none for the empty string).
        ("setText", NodeType::Element { .. } | NodeType::DocumentFragment) => {
            let removed: usize = node.children().iter().map(|child| child.subtree_len()).sum();
            let tag = node.tag_name().unwrap_or("-").to_string();
            let id = node.get_attribute("id").unwrap_or("").to_string();
            rustkit_dom::note_large_clear(&tag, &id, removed);
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
        // Cloning a document needs a second Document; not yet.
        ("clone", NodeType::Document) => Err("NotSupportedError"),
        // The clone is detached, so nothing needs a restyle yet.
        ("clone", _) => {
            let deep = matches!(args.get(3), Some(JsValue::Boolean(true)));
            let templates = &mut host.templates.borrow_mut();
            let copy = clone_node(document, &node, deep, templates);
            Ok((node_id(Some(copy)), DomDirty::Clean))
        }
        // `adoptTo(doc)`: the node (just made by, adopted by or imported
        // into the document `doc`) takes that document's scripting state.
        ("adoptTo", _) => {
            let owner = number_arg(args, 3)
                .filter(|n| *n >= 0.0)
                .and_then(|n| document.get_node(NodeId::new(n as usize)))
                .ok_or("NotFoundError")?;
            if !matches!(owner.node_type, NodeType::Document) {
                return Err("NotSupportedError");
            }
            // Every Document node owns itself, the page's included.
            node.set_owner(owner.owner_document_id());
            Ok((JsValue::Null, DomDirty::Clean))
        }
        // HTML §8.5 innerHTML setter: parse as the element's contents (the
        // fragment parsing algorithm), then replace all children with it.
        // A template's innerHTML is its content's (HTML §4.12.3).
        ("setHTML", NodeType::Element { tag_name, .. }) => {
            let html = string_arg(args, 3).unwrap_or("");
            // The fragment's scripting flag is its context element's
            // document's (HTML §13.4): off in a template's contents and in
            // a document `DOMParser` or `createHTMLDocument` made, whose
            // nodes live in this Document under a Document node of their
            // own, so `<noscript>` content there is elements.
            let mut top = node.clone();
            while let Some(parent) = top.parent() {
                top = parent;
            }
            let inert = tag_name.eq_ignore_ascii_case("template")
                || node.owner_document_id() != document.root().id.raw()
                || (matches!(top.node_type, NodeType::Document) && !Rc::ptr_eq(&top, document.root()));
            let nodes = document
                .parse_fragment_scripted(
                    html,
                    tag_name,
                    is_in_html_content(&node),
                    node.holds_svg_content(),
                    document.scripting() && !inert,
                )
                .map_err(|_| "SyntaxError")?;
            let templates = &mut host.templates.borrow_mut();
            let target = templates
                .get(&node.id.raw())
                .cloned()
                .unwrap_or(node.clone());
            let removed: usize = target.children().iter().map(|child| child.subtree_len()).sum();
            let tag = target.tag_name().unwrap_or("-").to_string();
            let id = target.get_attribute("id").unwrap_or("").to_string();
            rustkit_dom::note_large_clear(&tag, &id, removed);
            for child in target.children() {
                child.remove_from_parent();
            }
            for child in nodes {
                adopt_template_contents(document, &child, templates);
                target.append_child(child);
            }
            Ok((JsValue::Null, DomDirty::Style))
        }
        ("setInnerText", NodeType::Element { .. }) => {
            inner_text::set_inner_text(document, &node, string_arg(args, 3).unwrap_or(""));
            Ok((JsValue::Null, DomDirty::Style))
        }
        _ => Err("NotSupportedError"),
    }
}

/// The per-node record a `write` op makes (the `MutationLog`), read before
/// the write so `class` and `id` keep their old value. `None` for an op
/// that writes no connected data (creation, clone) or an unknown node.
fn write_record(host: &DomHost, args: &[JsValue]) -> Option<MutationRecord> {
    let node = host.node_at(args, 2)?;
    let record = |kind, attr: Option<String>, old: Option<String>| MutationRecord {
        node: node.id.raw(),
        kind,
        attr,
        old,
        child: None,
    };
    match (string_arg(args, 1)?, &node.node_type) {
        ("setAttr" | "removeAttr", NodeType::Element { .. }) => {
            let name = string_arg(args, 3)?;
            // Every attribute write passes here: a wheel handler attribute
            // can hear and cancel the wheel (`note_wheel_handler_attribute`).
            if string_arg(args, 1) == Some("setAttr") {
                host.note_wheel_handler_attribute(name);
            }
            let name = if is_html_element(&node) {
                name.to_ascii_lowercase()
            } else {
                name.to_string()
            };
            let old = matches!(name.as_str(), "class" | "id")
                .then(|| node.get_attribute(&name).map(str::to_string))
                .flatten();
            let kind = match name.as_str() {
                "style" => MutationKind::Style,
                _ => MutationKind::Attribute,
            };
            Some(record(kind, Some(name), old))
        }
        ("setText", NodeType::Text(_) | NodeType::Comment(_)) => Some(record(MutationKind::Text, None, None)),
        // The element's children are replaced.
        ("setText" | "setHTML" | "setInnerText", _) => Some(record(MutationKind::ChildList, None, None)),
        _ => None,
    }
}

/// The per-node records a `mutate` op makes: the parent with the node it
/// gained or lost. A move also records the parent it left; a fragment
/// inserts its children, not itself.
fn mutate_records(host: &DomHost, args: &[JsValue]) -> Vec<MutationRecord> {
    let (Some(parent), Some(node)) = (host.node_at(args, 2), host.node_at(args, 3)) else {
        return Vec::new();
    };
    let record = |parent: usize, child: usize| MutationRecord {
        node: parent,
        kind: MutationKind::ChildList,
        attr: None,
        old: None,
        child: Some(child),
    };
    match string_arg(args, 1) {
        Some("insert") if matches!(node.node_type, NodeType::DocumentFragment) => node
            .children()
            .iter()
            .map(|c| record(parent.id.raw(), c.id.raw()))
            .collect(),
        Some("insert") => node
            .parent()
            .map(|old| record(old.id.raw(), node.id.raw()))
            .into_iter()
            .chain([record(parent.id.raw(), node.id.raw())])
            .collect(),
        Some("remove") => vec![record(parent.id.raw(), node.id.raw())],
        _ => Vec::new(),
    }
}

/// Diagnostic (`RUST_LOG=rustkit_bindings::dom=trace`): the write that
/// dirtied the page, and whether its node (the parent, for a tree write) is
/// in the document at all.
fn trace_dirty(host: &DomHost, args: &[JsValue], bucket: DomDirty) {
    if bucket == DomDirty::Clean || !tracing::enabled!(tracing::Level::TRACE) {
        return;
    }
    let node = host.node_at(args, 2);
    let connected = match (&node, args.first().and_then(|g| host.document_for(g))) {
        (Some(node), Some(document)) => is_connected(node, document),
        _ => false,
    };
    let tag = node.as_ref().and_then(|n| n.tag_name()).unwrap_or("").to_string();
    let detail: String = string_arg(args, 3).unwrap_or("").chars().take(48).collect();
    tracing::trace!(op = string_arg(args, 1).unwrap_or(""), %tag, connected, %detail, ?bucket, "DOM write dirtied the page");
}

/// The kind of a dirtying `write` op, for the relayout cause log.
fn write_kind(args: &[JsValue]) -> MutationKind {
    match string_arg(args, 1) {
        Some("setAttr" | "removeAttr")
            if string_arg(args, 3).is_some_and(|name| name.eq_ignore_ascii_case("style")) =>
        {
            MutationKind::Style
        }
        Some("setAttr" | "removeAttr") => MutationKind::Attribute,
        Some("setText" | "setInnerText") => MutationKind::Text,
        _ => MutationKind::ChildList,
    }
}

/// The most characters of a tag name or an id the node label keeps.
const LABEL_PART_MAX: usize = 48;

/// `part` as the relayout log may print it: at most `LABEL_PART_MAX`
/// characters (an ellipsis marks the cut) and no control characters (each
/// becomes `?`). The page chooses its ids, so the log line it lands in must
/// not grow with them or take a newline from them.
fn label_part(part: &str) -> String {
    let mut out: String = part
        .chars()
        .take(LABEL_PART_MAX)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if part.chars().nth(LABEL_PART_MAX).is_some() {
        out.push('…');
    }
    out
}

/// `tag#id` of the node at `args[index]` (the tag alone without an id,
/// empty for a non-element or a stale id): the relayout log's node label,
/// each part clipped by `label_part`.
fn node_label(host: &DomHost, args: &[JsValue], index: usize) -> String {
    let Some(node) = host.node_at(args, index) else {
        return String::new();
    };
    let NodeType::Element {
        tag_name,
        attributes,
        ..
    } = &node.node_type
    else {
        return String::new();
    };
    match attributes.get("id") {
        Some(id) if !id.is_empty() => format!("{}#{}", label_part(tag_name), label_part(id)),
        _ => label_part(tag_name),
    }
}

/// Whether a script write can change a box of the published layout, which
/// decides whether a geometry read after it lays the page out again (LAT7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reach {
    /// It can: the read forces a layout.
    Seen,
    /// It cannot: the node is in no document, or inside a `display: none`
    /// subtree the write cannot show. The engine still lays it out when
    /// script settles (the bucket and the mutation log are kept), so what
    /// it builds next is the same; only the read in between is spared.
    Unseen,
    /// TREE-REUSE step (2): an inline `style` write that changed only paint properties
    /// (`paint_style_reads`, the read bits it carries). No box moves: only a read that sees
    /// the values (`getComputedStyle`, and a hit test after `opacity`) lays the page out.
    Paint(u8),
}

impl Reach {
    fn mark(self, dirty: &crate::DirtyMark, bucket: DomDirty, cause: &'static str, logged: crate::Logged) {
        match self {
            Reach::Seen => dirty.mark(bucket, cause, logged),
            Reach::Unseen => dirty.mark_unseen(bucket, logged),
            Reach::Paint(reads) => dirty.mark_paint_style(bucket, cause, reads, logged),
        }
    }
}

/// Inline style properties a change of which moves no box (TREE-REUSE step (2)), with the reads
/// that see the change beyond `getComputedStyle`. Longhands and the one colour shorthand only:
/// `background` and `border` also set what lays out.
const PAINT_STYLE_PROPERTIES: &[(&str, u8)] = &[
    ("color", crate::LAYER_PAINT),
    ("background-color", crate::LAYER_PAINT),
    ("border-color", crate::LAYER_PAINT),
    ("border-top-color", crate::LAYER_PAINT),
    ("border-right-color", crate::LAYER_PAINT),
    ("border-bottom-color", crate::LAYER_PAINT),
    ("border-left-color", crate::LAYER_PAINT),
    ("outline-color", crate::LAYER_PAINT),
    ("text-decoration-color", crate::LAYER_PAINT),
    ("caret-color", crate::LAYER_PAINT),
    ("opacity", crate::LAYER_PAINT | crate::LAYER_STACKING),
];

/// An inline style's declarations as `name -> value` (names ASCII-lowercased, the last of a name
/// wins), or `None` when the text holds anything a split on `;` and `:` could misread: a quote, an
/// escape, a comment, a brace, or brackets left open across a `;`.
fn plain_declarations(text: &str) -> Option<HashMap<String, String>> {
    if text.contains(['"', '\'', '\\', '{', '}']) || text.contains("/*") {
        return None;
    }
    let mut declarations = HashMap::new();
    for declaration in text.split(';') {
        let declaration = declaration.trim();
        if declaration.is_empty() {
            continue;
        }
        if declaration.matches('(').count() != declaration.matches(')').count() {
            return None;
        }
        let (name, value) = declaration.split_once(':')?;
        declarations.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Some(declarations)
}

/// The read bits of a write of `node`'s `style` attribute from `old` to `new` (`None`: removed)
/// when every declaration it adds, removes or changes is a paint property
/// (`PAINT_STYLE_PROPERTIES`); `None` when one may move a box, or when the page has a rule that
/// reads the attribute or a `:has()` (`DomHost::style_attr_rules`), through which the write can
/// change which rules match.
fn paint_style_reads(host: &DomHost, old: Option<&str>, new: Option<&str>) -> Option<u8> {
    if host.style_attr_rules.as_ref().is_none_or(|rules| rules()) {
        return None;
    }
    let old = plain_declarations(old.unwrap_or(""))?;
    let new = plain_declarations(new.unwrap_or(""))?;
    let mut reads = 0;
    for name in old.keys().chain(new.keys()) {
        if old.get(name) == new.get(name) {
            continue;
        }
        let (_, bits) = PAINT_STYLE_PROPERTIES.iter().find(|(property, _)| property == name)?;
        reads |= bits;
    }
    (reads != 0).then_some(reads)
}

/// Elements a write under or to which may change something outside its
/// box even when hidden: sheets, metadata, scripts, templates and slots,
/// what a select or a picture draws from, replaced and embedded content,
/// and the root elements.
const UNSEEN_DENIED: &[&str] = &[
    "html", "head", "body", "style", "link", "meta", "base", "title", "script", "noscript", "template",
    "slot", "select", "option", "optgroup", "datalist", "picture", "source", "track", "img", "video",
    "audio", "canvas", "iframe", "frame", "object", "embed", "input", "textarea", "label", "form",
    "details", "summary", "dialog", "svg", "math",
];

/// In the document, a shadow tree's nodes through their host.
fn rendered_connected(node: &Rc<Node>, document: &Document) -> bool {
    let root = document.root().id;
    let mut current = Some(node.clone());
    while let Some(n) = current {
        if n.id == root {
            return true;
        }
        current = n.parent().or_else(|| n.shadow_host());
    }
    false
}

/// Whether a write to `container` itself (its attributes, children or
/// text) is inside a `display: none` subtree of the published layout that
/// it cannot show: a strict ancestor of `container` computed `display:
/// none`, and every element from `container` up to it a plain HTML element
/// (`UNSEEN_DENIED`). Only answered from what is already known: the
/// layout must be current (no write since it), its computed styles built
/// or formattable (a read built them; LAT8 formats each on its read), the page without `:has()` rules or shadow roots.
/// Anything else answers no, and the read lays out as before.
fn under_hidden(host: &DomHost, container: &Rc<Node>, dirty: &crate::DirtyMark) -> bool {
    if dirty.written_since_layout()
        || host.geometry_source.is_some()
        || (host.computed_lines.is_none() && host.computed.borrow().is_empty())
    {
        return false;
    }
    if host.document.as_ref().is_none_or(|d| d.has_shadow_roots()) {
        return false;
    }
    if host.has_rules.as_ref().is_none_or(|has| has()) {
        return false;
    }
    let plain = |n: &Rc<Node>| {
        is_html_element(n) && n.tag_name().is_some_and(|t| !UNSEEN_DENIED.iter().any(|d| t.eq_ignore_ascii_case(d)))
    };
    let display_none = |lines: &str| lines.lines().any(|l| l == "display\tnone");
    if !plain(container) {
        return false;
    }
    // A box of its own: shown.
    if host.computed_of(container.id.raw()).is_some_and(|l| !display_none(&l)) {
        return false;
    }
    let mut up = container.parent();
    while let Some(n) = up {
        if !plain(&n) {
            return false;
        }
        if let Some(lines) = host.computed_of(n.id.raw()) {
            return display_none(&lines);
        }
        up = n.parent();
    }
    false
}

/// What a `write` op would reach, and its cause name for the forced-layout
/// log, read before the write (a `textContent` write takes the children
/// out).
fn write_reach(host: &DomHost, args: &[JsValue], dirty: &crate::DirtyMark) -> (Reach, &'static str) {
    let Some(node) = host.node_at(args, 2) else {
        return (Reach::Seen, "other");
    };
    let op = string_arg(args, 1).unwrap_or("");
    let cause = match op {
        "setAttr" | "removeAttr" => match string_arg(args, 3).map(str::to_ascii_lowercase).as_deref() {
            Some("class") => "class",
            Some("style") => "style",
            _ => "attribute",
        },
        "setHTML" => "html",
        _ => "text",
    };
    let Some(document) = args.first().and_then(|g| host.document_for(g)) else {
        return (Reach::Seen, cause);
    };
    if !rendered_connected(&node, document) {
        return (Reach::Unseen, cause);
    }
    let container = if node.is_element() { Some(node.clone()) } else { node.parent() };
    match container {
        Some(c) if under_hidden(host, &c, dirty) => (Reach::Unseen, cause),
        _ if cause == "style" && is_html_element(&node) => {
            let new = (op == "setAttr").then(|| string_arg(args, 4).unwrap_or(""));
            let old = node.get_attribute("style");
            match paint_style_reads(host, old.as_deref(), new) {
                Some(reads) => (Reach::Paint(reads), cause),
                None => (Reach::Seen, cause),
            }
        }
        _ => (Reach::Seen, cause),
    }
}

/// `write_reach` for a `mutate` op: an insert of a node from no document
/// into none, or into a hidden subtree, and a removal from either.
fn mutate_reach(host: &DomHost, args: &[JsValue], dirty: &crate::DirtyMark) -> (Reach, &'static str) {
    let cause = match string_arg(args, 1) {
        Some("remove") => "remove",
        _ => "insert",
    };
    let (Some(parent), Some(node), Some(document)) = (
        host.node_at(args, 2),
        host.node_at(args, 3),
        args.first().and_then(|g| host.document_for(g)),
    ) else {
        return (Reach::Seen, cause);
    };
    let node_in = cause == "insert" && rendered_connected(&node, document);
    if node_in {
        // A move takes it out of where it was laid out.
        return (Reach::Seen, cause);
    }
    if !rendered_connected(&parent, document) || under_hidden(host, &parent, dirty) {
        return (Reach::Unseen, cause);
    }
    (Reach::Seen, cause)
}

/// `__rustkit_dom_write(gen, op, id, a, b)`: the write, its mutation record, its trace and its dirty
/// mark. A string answer is the DOMException name to throw. The inline style's native write
/// (`__rustkit_style_write`) takes this same path.
fn dom_write(
    h: &SharedDomHost,
    d: &crate::DirtyMark,
    m: &RefCell<ScriptMutations>,
    l: &RefCell<MutationLog>,
    args: &[JsValue],
) -> JsValue {
    let record = write_record(&h.borrow(), args);
    let (reach, cause) = write_reach(&h.borrow(), args, d);
    match write(&h.borrow(), args) {
        Ok((result, bucket)) => {
            let logged = match record.filter(|_| bucket != DomDirty::Clean) {
                Some(record) => {
                    l.borrow_mut().push(record);
                    crate::Logged::Yes
                }
                None => crate::Logged::No,
            };
            trace_dirty(&h.borrow(), args, bucket);
            note_mutation(m, bucket, write_kind(args), || node_label(&h.borrow(), args, 2));
            reach.mark(d, bucket, cause, logged);
            result
        }
        Err(name) => JsValue::String(name.to_string()),
    }
}

/// Record a write that dirtied the page (`bucket` not `Clean`).
fn note_mutation(
    mutations: &RefCell<ScriptMutations>,
    bucket: DomDirty,
    kind: MutationKind,
    label: impl FnOnce() -> String,
) {
    if bucket != DomDirty::Clean {
        mutations.borrow_mut().record(kind, label);
    }
}

/// The `MutationLog` record of a control value (`attr` `None`) or
/// checkedness (`checked`) write.
fn control_record(node: usize, attr: Option<&str>) -> MutationRecord {
    MutationRecord {
        node,
        kind: MutationKind::FormValue,
        attr: attr.map(str::to_string),
        old: None,
        child: None,
    }
}

/// `value(gen, id[, v])`: a text control's value (HTML §4.10.5.4, value
/// mode "value"). With `v` a string, sets it; with `v` null, resets the
/// control to its default (form reset). Answers the value, or null when
/// the node is stale or not an `<input>`/`<textarea>`. A write marks
/// `Layout`: the engine repaints the control from its edit state.
fn control_value(host: &mut DomHost, args: &[JsValue]) -> (JsValue, DomDirty) {
    let Some(node) = host.node(args) else {
        return (JsValue::Null, DomDirty::Clean);
    };
    let textarea = match node.tag_name() {
        Some(t) if t.eq_ignore_ascii_case("textarea") => true,
        Some(t) if t.eq_ignore_ascii_case("input") => false,
        _ => return (JsValue::Null, DomDirty::Clean),
    };
    let default = || {
        if textarea {
            node.text_content()
        } else {
            node.get_attribute("value").unwrap_or("").to_string()
        }
    };
    let raw = node.id.raw();
    let value = match args.get(2) {
        Some(JsValue::String(v)) => {
            // The value sanitization algorithms: a text input drops line
            // breaks; a textarea normalizes them to LF.
            let v = if textarea {
                v.replace("\r\n", "\n").replace('\r', "\n")
            } else {
                v.chars().filter(|c| !matches!(c, '\r' | '\n')).collect()
            };
            host.values.insert(raw, v.clone());
            v
        }
        Some(JsValue::Null) => {
            host.values.remove(&raw);
            default()
        }
        _ => {
            let v = host.values.get(&raw).cloned().unwrap_or_else(default);
            return (JsValue::String(v), DomDirty::Clean);
        }
    };
    host.value_writes.push((raw, value.clone()));
    (JsValue::String(value), DomDirty::Layout)
}

/// `checked(gen, id, v)`: an input's checkedness (HTML §4.10.5.4) became
/// `v`, or with `v` null follows its `checked` attribute again (form
/// reset). Script keeps the state; this tells the engine, which styles
/// (`:checked`) and paints the control from it.
fn control_checked(host: &mut DomHost, args: &[JsValue]) -> DomDirty {
    let Some(node) = host.node(args) else {
        return DomDirty::Clean;
    };
    let state = match args.get(2) {
        Some(JsValue::Boolean(checked)) => Some(*checked),
        _ => None,
    };
    host.checked_writes.push((node.id.raw(), state));
    match state {
        Some(checked) => host.checked.insert(node.id.raw(), checked),
        None => host.checked.remove(&node.id.raw()),
    };
    DomDirty::Style
}

/// `submit(gen, form, submitter)`: the form's `submit` event ran and no
/// listener cancelled it. The engine builds the submission and navigates.
fn form_submit(host: &mut DomHost, args: &[JsValue]) {
    let Some(form) = host.node(args) else {
        return;
    };
    let submitter = host.node_at(args, 2).map(|n| n.id.raw());
    host.submit_requests.push((form.id.raw(), submitter));
}

/// DOM §4.4 "clone a node": a detached copy with fresh NodeIds, its
/// descendants copied too when `deep`. A template's copy gets a content
/// fragment of its own, holding copies of the content when `deep`.
fn clone_node(
    document: &Document,
    node: &Rc<Node>,
    deep: bool,
    templates: &mut HashMap<usize, Rc<Node>>,
) -> Rc<Node> {
    let copy = document.create_node(node.node_type.clone());
    if deep {
        for child in node.children() {
            copy.append_child(clone_node(document, &child, true, templates));
        }
    }
    // A clone is in its original's document.
    copy.set_owner(node.owner_document_id());
    if let Some(content) = templates.get(&node.id.raw()).cloned() {
        let content = clone_node(document, &content, deep, templates);
        // A template's contents are always the template contents' document's.
        content.set_owner(template_owner(document, templates));
        templates.insert(copy.id.raw(), content);
    }
    copy
}

fn node_id(node: Option<Rc<Node>>) -> JsValue {
    node.map_or(JsValue::Null, |n| JsValue::Number(n.id.raw() as f64))
}

fn id_list(nodes: impl IntoIterator<Item = Rc<Node>>) -> JsValue {
    let ids: Vec<String> = nodes.into_iter().map(|n| n.id.raw().to_string()).collect();
    JsValue::String(ids.join(" "))
}

/// A UTF-16 offset in a text node's data as the two character counts the
/// layout numbers a run by: the characters before it, and those characters
/// after white-space collapsing (each run of white space as one).
fn text_offsets(data: &str, offset: usize) -> (usize, usize) {
    let (mut units, mut chars, mut collapsed, mut in_space) = (0, 0, 0, false);
    for c in data.chars() {
        if units >= offset {
            break;
        }
        units += c.len_utf16();
        chars += 1;
        let space = matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}');
        if !(space && in_space) {
            collapsed += 1;
        }
        in_space = space;
    }
    (chars, collapsed)
}

fn number_arg(args: &[JsValue], index: usize) -> Option<f32> {
    match args.get(index) {
        Some(JsValue::Number(n)) if n.is_finite() => Some(*n as f32),
        _ => None,
    }
}

/// The deepest element whose published box contains the document point
/// `viewport + scroll`. A later node wins a tie.
fn box_hit(host: &DomHost, x: f32, y: f32) -> Option<usize> {
    let document = host.document.as_ref()?;
    let px = x + host.scroll.0;
    let py = y + host.scroll.1;
    let mut best: Option<(usize, usize)> = None;
    for (&id, g) in &host.geometry {
        if g.width <= 0.0 || g.height <= 0.0 {
            continue;
        }
        if px < g.x || py < g.y || px >= g.x + g.width || py >= g.y + g.height {
            continue;
        }
        let mut depth = 0usize;
        let mut node = document.get_node(NodeId::new(id));
        while let Some(n) = node {
            depth += 1;
            node = n.parent();
        }
        let replace = match best {
            None => true,
            Some((d, _)) if depth > d => true,
            Some((d, prev)) if depth == d && id > prev => true,
            _ => false,
        };
        if replace {
            best = Some((depth, id));
        }
    }
    best.map(|(_, id)| id)
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

/// Is `node` an HTML element for the fragment parser? The parser gives an
/// element in `<math>` the HTML namespace, so one there (and not under a
/// `foreignObject`) is foreign by its ancestors.
fn is_in_html_content(node: &Node) -> bool {
    if !is_html_element(node) {
        return false;
    }
    let mut up = node.parent();
    while let Some(ancestor) = up {
        match ancestor.tag_name().map(str::to_ascii_lowercase).as_deref() {
            Some("foreignobject") => return true,
            Some("svg") | Some("math") => return false,
            _ => {}
        }
        up = ancestor.parent();
    }
    true
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

/// Elements whose Text children serialize unescaped, wherever they are.
/// `noscript` is one more only where scripting is on for the node
/// (`noscript_is_raw`).
const RAW_TEXT_ELEMENTS: &[&str] = &["style", "script", "xmp", "iframe", "noembed", "noframes", "plaintext"];

/// Whether a `<noscript>` under `node` writes its text children as they
/// are (HTML §13.3: when scripting is enabled for the node). That is when
/// the node's document is the page's own and the page runs script, in the
/// tree or not (HTML2, Chrome for Testing 148). Its text is escaped like
/// any other when its document is one `DOMParser` or `createHTMLDocument`
/// made, or a template's contents, and when no path has given the node a
/// document (`Node::owner_document_id`, which follows the node through
/// creation, insertion, adoption, import, cloning and data writes).
fn noscript_is_raw(node: &Rc<Node>, page: Option<&Rc<Document>>) -> bool {
    let Some(page) = page.filter(|d| d.scripting()) else {
        return false;
    };
    // Only a node positively of the page's own document: one of another
    // document, or one no path has given a document, is escaped.
    node.owner_document_id() == page.root().id.raw()
}

/// HTML §13.3 "serializing HTML fragments" for one node (`outerHTML`).
/// Attributes come out in the element's own order (the order they were
/// written or set in), as Chrome writes them.
///
/// `raw_noscript`: `noscript_is_raw` for this tree.
fn serialize_node(node: &Rc<Node>, out: &mut String, templates: &HashMap<usize, Rc<Node>>, raw_noscript: bool) {
    match &node.node_type {
        NodeType::Element {
            tag_name,
            attributes,
            ..
        } => {
            out.push('<');
            out.push_str(tag_name);
            for (name, value) in attributes.iter() {
                out.push(' ');
                out.push_str(name);
                out.push_str("=\"");
                escape_into(value, true, out);
                out.push('"');
            }
            out.push('>');
            if !VOID_ELEMENTS.contains(&tag_name.as_str()) {
                serialize_children(node, out, templates, raw_noscript);
                out.push_str("</");
                out.push_str(tag_name);
                out.push('>');
            }
        }
        NodeType::Text(data) => {
            let raw = node
                .parent()
                // HTML elements only: an SVG or MathML `style`, `script`
                // or `noscript` writes its text escaped (Chrome for Testing 148).
                .filter(|p| is_html_element(p))
                .and_then(|p| p.tag_name().map(|t| RAW_TEXT_ELEMENTS.contains(&t) || (raw_noscript && t == "noscript")))
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
        NodeType::Document | NodeType::DocumentFragment => serialize_children(node, out, templates, raw_noscript),
    }
}

/// The children of `node`, serialized (`innerHTML`); a template's are
/// its content's children.
fn serialize_children(node: &Rc<Node>, out: &mut String, templates: &HashMap<usize, Rc<Node>>, raw_noscript: bool) {
    // A template's contents are in an inert document: scripting is off.
    let (source, raw_noscript) = match templates.get(&node.id.raw()) {
        Some(contents) => (contents, false),
        None => (node, raw_noscript),
    };
    for child in source.children() {
        serialize_node(&child, out, templates, raw_noscript);
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
fn node_info(
    node: &Rc<Node>,
    field: &str,
    templates: &HashMap<usize, Rc<Node>>,
    page: Option<&Rc<Document>>,
) -> JsValue {
    match field {
        "type" => JsValue::Number(match node.node_type {
            NodeType::Element { .. } => 1.0,
            NodeType::Text(_) => 3.0,
            NodeType::ProcessingInstruction { .. } => 7.0,
            NodeType::Comment(_) => 8.0,
            NodeType::Document => 9.0,
            NodeType::DocumentType { .. } => 10.0,
            NodeType::DocumentFragment => 11.0,
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
            NodeType::DocumentFragment => "#document-fragment".to_string(),
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
            NodeType::Element { .. } | NodeType::DocumentFragment => {
                JsValue::String(node.text_content())
            }
        },
        "parent" => node_id(node.parent()),
        // web_cssom.js: the lifetime of a <style>'s sheet (new children or
        // a move make a new one); a sheet's `disabled` belongs to one.
        "sheetGen" => JsValue::Number(node.sheet_generation() as f64),
        "first" => node_id(node.first_child()),
        "last" => node_id(node.last_child()),
        "next" => node_id(node.next_sibling()),
        "prev" => node_id(node.previous_sibling()),
        "children" => id_list(node.children()),
        // Space-separated (a name never holds whitespace), in the element's
        // own order.
        "attrNames" => match &node.node_type {
            NodeType::Element { attributes, .. } => {
                let names: Vec<&str> = attributes.keys().map(String::as_str).collect();
                JsValue::String(names.join(" "))
            }
            _ => JsValue::Null,
        },
        "innerHTML" => {
            let mut out = String::new();
            serialize_children(node, &mut out, templates, noscript_is_raw(node, page));
            JsValue::String(out)
        }
        "outerHTML" => {
            let mut out = String::new();
            serialize_node(node, &mut out, templates, noscript_is_raw(node, page));
            JsValue::String(out)
        }
        "content" => node_id(templates.get(&node.id.raw()).cloned()),
        // The NodeId of the node's Document node, 0 for the page's (HTML2).
        "owner" => JsValue::Number(node.owner_document_id() as f64),
        _ => JsValue::Undefined,
    }
}

/// Register the host functions and install the wrapper layer over the
/// stub `document`. Call after the stub globals exist. Tree writes mark
/// `dirty`.
pub(crate) fn install(
    runtime: &mut JsRuntime,
    host: &SharedDomHost,
    dirty: &Rc<crate::DirtyMark>,
    mutations: &Rc<RefCell<ScriptMutations>>,
    log: &Rc<RefCell<MutationLog>>,
) -> Result<(), JsError> {
    // `__rustkit_dom_resolve(base, relative)`: the absolute URL `relative`
    // names against `base`, or null when either does not parse. Backs the
    // URL-reflecting attributes (`script.src`).
    runtime.register_host_function(
        "__rustkit_dom_resolve",
        2,
        Box::new(|args| {
            let (Some(base), Some(relative)) = (string_arg(args, 0), string_arg(args, 1)) else {
                return JsValue::Null;
            };
            match url::Url::parse(base).and_then(|b| b.join(relative)) {
                Ok(u) => JsValue::String(u.to_string()),
                Err(_) => JsValue::Null,
            }
        }),
    )?;

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
                if matcher(&scope, arg, &host.checked).is_none() {
                    return JsValue::Boolean(false);
                }
                let mut all = Vec::new();
                descendant_elements(&scope, &mut all);
                return id_list(all.into_iter().filter(|n| matcher(n, arg, &host.checked) == Some(true)));
            }
            // HTML §7.2.2.3 named access on the Window object: the elements
            // whose id is `arg`, or whose name is `arg` on an embed, form,
            // img or object, in tree order. "frame" when none is but an
            // iframe/frame is named `arg` (a browsing-context name, which
            // is not supported). The spec gives a child browsing context's
            // name precedence over element matches; with iframe access
            // unsupported, an element match wins over a same-named iframe.
            // Shadow trees and template contents are not children here, so
            // only document-tree elements are seen (pinned in the tests).
            if string_arg(args, 2) == Some("windowNamed") {
                if arg.is_empty() {
                    return JsValue::String(String::new());
                }
                let (mut all, mut frame) = (Vec::new(), false);
                document.traverse(|n| {
                    let Some(tag) = n.tag_name() else { return };
                    let is = |names: &[&str]| names.iter().any(|k| tag.eq_ignore_ascii_case(k));
                    if n.get_attribute("id") == Some(arg)
                        || (n.get_attribute("name") == Some(arg) && is(&["embed", "form", "img", "object"]))
                    {
                        all.push(n.clone());
                    } else if n.get_attribute("name") == Some(arg) && is(&["iframe", "frame"]) {
                        frame = true;
                    }
                });
                if all.is_empty() && frame {
                    return JsValue::String("frame".into());
                }
                return id_list(all.into_iter().filter(|n| is_connected(n, document)));
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
                (Some(matcher), _) => matcher(&node, selector, &host.checked),
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
    let d = dirty.clone();
    runtime.register_host_function_by_op(
        "__rustkit_dom_info",
        3,
        2,
        Box::new(move |args| {
            // innerText reads computed display/visibility (CH2.E13).
            // `geom:<read>` names the geometry read for the forced-layout log.
            let (field, why) = match string_arg(args, 2) {
                Some(f) => match f.split_once(':') {
                    Some((f, why)) => (Some(f), Some(why)),
                    None => (Some(f), None),
                },
                None => (None, None),
            };
            let read = match (field, why) {
                (Some("geom"), Some("offset")) => Some("offset"),
                (Some("geom"), Some("client")) => Some("client"),
                (Some("geom"), Some("scroll_size")) => Some("scroll_size"),
                (Some("geom"), _) => Some("rect"),
                (Some("laidout"), _) => Some("laid_out"),
                (Some("computed"), _) => Some("computed_style"),
                (Some("scroll"), _) => Some("element_scroll"),
                (Some("scrollChain"), _) => Some("scroll_chain"),
                (Some("innerText"), _) => Some("inner_text"),
                _ => None,
            };
            if let Some(read) = read {
                // LAT22 probe: which element the read is of.
                {
                    let mut host = h.borrow_mut();
                    host.forced_read_node = host.node(args).map(|n| n.id.raw());
                }
                DomHost::ensure_fresh_geometry(&h, &d, read);
                h.borrow_mut().forced_read_node = None;
            }
            let host = h.borrow();
            match (host.node(args), field) {
                (Some(_), Some("stale")) => {
                    JsValue::String(if d.written_since_layout() { "1".into() } else { String::new() })
                }
                (Some(_), Some("laidout")) => {
                    JsValue::String(if host.geometry.is_empty() { String::new() } else { "1".into() })
                }
                (Some(node), Some("computed")) => JsValue::String(
                    host.computed_of(node.id.raw()).unwrap_or_default(),
                ),
                (Some(node), Some("geom")) => match host.geometry.get(&node.id.raw()) {
                    Some(g) => {
                        // A non-finite edge would make getBoundingClientRect
                        // report NaN, and parseInt of that feeds elementFromPoint
                        // a double WebIDL rejects.
                        let n = |v: f32| if v.is_finite() { v } else { 0.0 };
                        // The first two and the last two are for rects: where
                        // the scrollers, sticky boxes and transforms put the
                        // box. The rest, `offsetTop` among them, leave the
                        // scrollers and transforms out.
                        let moved = host.moved_by(g);
                        let (x, y) = (g.x + moved.stuck.0, g.y + moved.stuck.1);
                        let (rx, ry, rw, rh) = host
                            .transformed_rect(g)
                            .unwrap_or((x - moved.scrolled.0, y - moved.scrolled.1, g.width, g.height));
                        // The rect's reader takes the window's scroll off.
                        let page = if g.held { host.window_scroll() } else { (0.0, 0.0) };
                        JsValue::String(format!(
                            "{} {} {} {} {} {} {} {} {} {} {} {} {} {} {}",
                            n(rx + page.0), n(ry + page.1), n(g.width), n(g.height), n(g.border_left), n(g.border_top),
                            n(g.client_width), n(g.client_height), n(g.scroll_width), n(g.scroll_height), g.position,
                            n(x), n(y), n(rw), n(rh)
                        ))
                    }
                    None => JsValue::String(String::new()),
                },
                // "x y" of an element that can scroll now, else "".
                (Some(node), Some("scroll")) => {
                    let raw = node.id.raw();
                    if host.geometry.get(&raw).is_some_and(|g| g.scrolls) {
                        let scrolls = host.element_scrolls.borrow();
                        let (x, y) = scrolls.offsets.get(&raw).copied().unwrap_or((0.0, 0.0));
                        JsValue::String(format!("{x} {y}"))
                    } else {
                        JsValue::String(String::new())
                    }
                }
                // What scrolling to this element moves: "w" when the window
                // does ("f" under a fixed box), then the scrollers, innermost
                // first. "" when it has no box.
                (Some(node), Some("scrollChain")) => match host.geometry.get(&node.id.raw()) {
                    Some(g) => {
                        let mut fixed = false;
                        let mut up = Some(node.clone());
                        while let (false, Some(n)) = (fixed, up) {
                            fixed = host.geometry.get(&n.id.raw()).is_some_and(|g| g.position == 2);
                            up = n.parent();
                        }
                        let mut chain = String::from(if fixed { "f" } else { "w" });
                        for scroller in scrolling_chain_ids(&host.geometry, g.scroller) {
                            chain.push_str(&format!(" {scroller}"));
                        }
                        JsValue::String(chain)
                    }
                    None => JsValue::String(String::new()),
                },
                (Some(node), Some("innerText")) if node.is_element() => {
                    let style = |n: &rustkit_dom::Node| {
                        host.computed_of(n.id.raw()).and_then(|l| inner_text::Computed::from_lines(&l))
                    };
                    JsValue::String(inner_text::inner_text(&node, &style))
                }
                (Some(node), Some(field)) => {
                    node_info(&node, field, &host.templates.borrow(), host.document.as_ref())
                }
                _ => JsValue::Null,
            }
        }),
    )?;

    // scroll_element(gen, id, x, y): store where script scrolled an element.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_scroll_element",
        4,
        Box::new(move |args| {
            let host = h.borrow();
            if let (Some(node), Some(x), Some(y)) = (host.node(args), number_arg(args, 2), number_arg(args, 3)) {
                let mut scrolls = host.element_scrolls.borrow_mut();
                scrolls.offsets.insert(node.id.raw(), (x, y));
                scrolls.generation += 1;
                scrolls.unpainted = true;
            }
            JsValue::Undefined
        }),
    )?;

    // animate(gen, id, entries): what Web Animations put on an element's
    // animated-style layer, as JSON `[[property, value, composite], ...]`
    // in animation order, or null to take the layer off. A script write
    // that lays the page out (the layer is read by the build).
    let (h, d, l) = (host.clone(), dirty.clone(), log.clone());
    runtime.register_host_function(
        "__rustkit_dom_animate",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(node) = host.node(args) else {
                return JsValue::Undefined;
            };
            let entries: Option<Vec<(String, String, String)>> = match args.get(2) {
                Some(JsValue::String(json)) => serde_json::from_str(json).ok(),
                _ => None,
            };
            let mut layer = host.animated.borrow_mut();
            // What the layer held and holds for the element: the reads a
            // change can reach are those of both (LAT7c).
            let (changed, reads) = match entries {
                Some(entries) => {
                    let reads = layer_reads(entries.iter().map(|e| e.0.as_str()));
                    let old = layer.script.insert(node.id.raw(), entries.clone());
                    let old_reads = old.as_ref().map_or(Some(0), |o| layer_reads(o.iter().map(|e| e.0.as_str())));
                    (old.as_ref() != Some(&entries), reads.zip(old_reads).map(|(a, b)| a | b))
                }
                None => match layer.script.remove(&node.id.raw()) {
                    Some(old) => (true, layer_reads(old.iter().map(|e| e.0.as_str()))),
                    None => (false, None),
                },
            };
            if changed {
                layer.generation += 1;
                match reads {
                    // Paint properties only: the boxes did not move.
                    Some(reads) => d.mark_animation_paint(crate::DomDirty::Layout, reads),
                    None => d.mark_animation_layer(crate::DomDirty::Layout, "animation"),
                }
                // The element restyles as if its inline style changed, so
                // the next build walks it rather than keep its box.
                l.borrow_mut().push(MutationRecord {
                    node: node.id.raw(),
                    kind: MutationKind::Style,
                    attr: None,
                    old: None,
                    child: None,
                });
            }
            JsValue::Undefined
        }),
    )?;
    // cssAnimations(): the engine's CSS animations and transitions as of
    // the last layout, as JSON, for `getAnimations()`.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_css_animations",
        0,
        Box::new(move |_| {
            let host = h.borrow();
            let layer = host.animated.borrow();
            let items: Vec<String> = layer
                .css
                .iter()
                .map(|a| {
                    format!(
                        "{{\"id\":{},\"node\":{},\"pseudo\":{},\"kind\":{:?},\"index\":{},\"name\":{},\"start\":{},\"paused\":{},\"delay\":{},\
                         \"duration\":{},\"iterations\":{},\"direction\":{:?},\"fill\":{:?},\"easing\":{},\"ended\":{}}}",
                        a.id,
                        a.node,
                        a.pseudo.map_or("null".to_string(), |p| format!("{p:?}")),
                        a.kind,
                        a.index,
                        serde_json::to_string(&a.name).unwrap_or_default(),
                        a.start_ms,
                        a.paused_ms.map_or("null".to_string(), |p| p.to_string()),
                        a.delay_ms,
                        a.duration_ms,
                        if a.iterations.is_infinite() { "null".to_string() } else { a.iterations.to_string() },
                        a.direction,
                        a.fill,
                        serde_json::to_string(&a.easing).unwrap_or_default(),
                        a.ended
                    )
                })
                .collect();
            JsValue::String(format!("[{}]", items.join(",")))
        }),
    )?;
    // animated(gen, id, property): whether the property is animated on the
    // element, by script's layer or by a CSS animation or transition, so a
    // computed read keeps the published (animated) row over the inline
    // text (#861's at-once read of a plain inline value).
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_animated",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            let (Some(node), Some(property)) = (host.node(args), string_arg(args, 2)) else {
                return JsValue::Boolean(false);
            };
            let layer = host.animated.borrow();
            let raw = node.id.raw();
            let by_script = layer
                .script
                .get(&raw)
                .is_some_and(|entries| entries.iter().any(|(p, _, _)| p == property));
            let by_css = layer
                .css_props
                .get(&raw)
                .is_some_and(|props| props.iter().any(|p| p == property));
            JsValue::Boolean(by_script || by_css)
        }),
    )?;
    // cssControl(gen, id, kind, animationId, op): script's pause/play/
    // cancel/finish of one of them, by the engine's animation id; the
    // engine applies it at its next build.
    let (h, d) = (host.clone(), dirty.clone());
    runtime.register_host_function(
        "__rustkit_dom_css_control",
        5,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(node) = host.node(args) else {
                return JsValue::Undefined;
            };
            let (Some(JsValue::String(kind)), Some(id), Some(JsValue::String(op))) =
                (args.get(2), number_arg(args, 3), args.get(4))
            else {
                return JsValue::Undefined;
            };
            host.animated.borrow_mut().controls.push(CssAnimationControl {
                id: id.max(0.0) as u64,
                node: node.id.raw(),
                kind: kind.clone(),
                op: op.clone(),
            });
            d.mark(crate::DomDirty::Layout, "animation", crate::Logged::No);
            JsValue::Undefined
        }),
    )?;

    // noteWheel(listening, blocking): whether a wheel or mousewheel
    // listener or handler is registered now, and whether one of them is
    // not passive. Kept up to date as listeners come and go.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_note_wheel",
        2,
        Box::new(move |args| {
            let host = h.borrow();
            let attribute = host.wheel_handler_attribute.get();
            host.wants_wheel.set(matches!(args.first(), Some(JsValue::Boolean(true))) || attribute);
            host.wheel_blocking.set(matches!(args.get(1), Some(JsValue::Boolean(true))) || attribute);
            JsValue::Null
        }),
    )?;

    // wheelAttribute(): a wheel handler attribute was parsed or written.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_wheel_attribute",
        0,
        Box::new(move |_| JsValue::Boolean(h.borrow().wheel_handler_attribute.get())),
    )?;

    // rangeTextRects(gen, "node from to;node from to;..."): where the
    // characters [from, to) of each of those text nodes are (offsets in
    // UTF-16 units, as a Range has them), as a JSON array with one flat
    // [x, y, w, h, ...] array per node, in document coordinates; an empty
    // one before a layout or for a node with no box. One call for a whole
    // range: a selection over a long page asks about thousands of nodes.
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_range_text_rects",
        2,
        Box::new(move |args| {
            DomHost::ensure_fresh_geometry(&h, &d, "text_rects");
            let host = h.borrow();
            let asks = string_arg(args, 1).unwrap_or("");
            let document = args.first().and_then(|g| host.document_for(g));
            let answers: Vec<String> = asks
                .split(';')
                .filter(|ask| !ask.is_empty())
                .map(|ask| {
                    let mut numbers = ask.split(' ').filter_map(|n| n.parse::<usize>().ok());
                    let (Some(id), Some(from), Some(to)) = (numbers.next(), numbers.next(), numbers.next()) else {
                        return "[]".to_string();
                    };
                    let (Some(rects), Some(node)) =
                        (host.text_rects.as_ref(), document.and_then(|d| d.get_node(NodeId::new(id))))
                    else {
                        return "[]".to_string();
                    };
                    let NodeType::Text(data) = &node.node_type else {
                        return "[]".to_string();
                    };
                    let (from, to) = (text_offsets(data, from), text_offsets(data, to));
                    let flat: Vec<String> = rects(id, (from.0, to.0), (from.1, to.1))
                        .iter()
                        .flat_map(|r| r.iter().map(|v| if v.is_finite() { v.to_string() } else { "0".to_string() }))
                        .collect();
                    format!("[{}]", flat.join(","))
                })
                .collect();
            JsValue::String(format!("[{}]", answers.join(",")))
        }),
    )?;

    // svgHit(gen, svg, x, y, width, height): the shape of an `<svg>` a point
    // is on (from the top left of its content box, whose size is given),
    // as its NodeId; -1 when it is on none.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_svg_hit",
        6,
        Box::new(move |args| {
            let host = h.borrow();
            let number = |i: usize| match args.get(i) {
                Some(JsValue::Number(n)) if n.is_finite() => Some(*n),
                _ => None,
            };
            let (Some(svg), Some(x), Some(y), Some(w), Some(height)) =
                (host.node(args), number(2), number(3), number(4), number(5))
            else {
                return JsValue::Number(-1.0);
            };
            crate::svg_geometry::shape_at(&svg, (x, y), (w, height))
                .map_or(JsValue::Number(-1.0), |id| JsValue::Number(id as f64))
        }),
    )?;

    // hit(x, y): the node under a viewport point, or null before a layout
    // and on a miss. A layout's hit test wins; published boxes are the
    // stand-in when a test wrote them by hand.
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_hit",
        2,
        Box::new(move |args| {
            DomHost::ensure_fresh_geometry(&h, &d, "hit_test");
            let host = h.borrow();
            if host.geometry.is_empty() {
                return JsValue::Null;
            }
            let (Some(x), Some(y)) = (number_arg(args, 0), number_arg(args, 1)) else {
                return JsValue::Null;
            };
            // With a layout, a point no box is under answers -1: the
            // root element's, in the wrappers.
            if let Some(tester) = host.hit_tester.clone() {
                return tester(x, y, host.window_scroll())
                    .map_or(JsValue::Number(-1.0), |id| JsValue::Number(id as f64));
            }
            box_hit(&host, x, y).map_or(JsValue::Null, |id| JsValue::Number(id as f64))
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
    let m = mutations.clone();
    let l = log.clone();
    runtime.register_host_function(
        "__rustkit_dom_mutate",
        5,
        Box::new(move |args| {
            let records = mutate_records(&h.borrow(), args);
            let (reach, cause) = mutate_reach(&h.borrow(), args, &d);
            match mutate(&h.borrow(), args) {
            Ok(()) => {
                let logged = if records.is_empty() { crate::Logged::No } else { crate::Logged::Yes };
                let mut log = l.borrow_mut();
                for record in records {
                    log.push(record);
                }
                drop(log);
                trace_dirty(&h.borrow(), args, DomDirty::Style);
                note_mutation(&m, DomDirty::Style, MutationKind::ChildList, || {
                    node_label(&h.borrow(), args, 2)
                });
                // Pin §3.3: a structure insert/remove/move restyles.
                reach.mark(&d, DomDirty::Style, cause, logged);
                JsValue::Null
            }
            Err(name) => JsValue::String(name.to_string()),
            }
        }),
    )?;

    // write(gen, op, id, a, b): a string answer is the DOMException name
    // to throw; anything else is the result (a new node's id, or null).
    let h = host.clone();
    let d = dirty.clone();
    let m = mutations.clone();
    let l = log.clone();
    runtime.register_host_function_by_op(
        "__rustkit_dom_write",
        5,
        1,
        Box::new(move |args| dom_write(&h, &d, &m, &l, args)),
    )?;

    // NATIVE-SETPROPERTY: an element's inline style (`el.style`) read and written in Rust
    // (`inline_style`). style_read(gen, id, op, arg): `value` / `priority` of the property named `arg`,
    // `item` (`arg` an index), `length`, `text` (cssText). style_write(gen, id, op, name, value,
    // priority): `set` answers whether the attribute was written, `remove` the value the property had
    // (written exactly when it had one); either answers -1 when the element is not found. A write
    // takes `__rustkit_dom_write`'s own path (`dom_write`), as `setAttribute('style', text)` does.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_style_read",
        4,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(node) = host.node(args) else {
                return JsValue::Number(-1.0);
            };
            let raw = node.get_attribute("style");
            let decls = host.style_decls(node.id.raw(), raw.as_deref().unwrap_or(""));
            match string_arg(args, 2).unwrap_or("") {
                "value" | "priority" => {
                    let name = crate::inline_style::js_trim(string_arg(args, 3).unwrap_or("")).to_lowercase();
                    let found = decls.iter().find(|d| d.name == name);
                    JsValue::String(match (string_arg(args, 2), found) {
                        (Some("value"), Some(d)) => d.value.clone(),
                        (_, Some(d)) if d.important => "important".to_string(),
                        _ => String::new(),
                    })
                }
                "item" => {
                    let i = number_arg(args, 3).unwrap_or(0.0);
                    JsValue::String(
                        decls.get(i as usize).filter(|_| i >= 0.0).map(|d| d.name.clone()).unwrap_or_default(),
                    )
                }
                "length" => JsValue::Number(decls.len() as f64),
                // An empty or absent attribute reads as no text at all.
                "text" if raw.as_deref().is_some_and(|t| !t.is_empty()) => {
                    JsValue::String(crate::inline_style::serialize_decls(&decls))
                }
                _ => JsValue::String(String::new()),
            }
        }),
    )?;
    let h = host.clone();
    let d = dirty.clone();
    let m = mutations.clone();
    let l = log.clone();
    runtime.register_host_function(
        "__rustkit_style_write",
        6,
        Box::new(move |args| {
            let found = h.borrow().node(args).map(|node| {
                let raw = node.get_attribute("style");
                (node.id.raw(), h.borrow().style_decls(node.id.raw(), raw.as_deref().unwrap_or("")))
            });
            let Some((id, decls)) = found else {
                return JsValue::Number(-1.0);
            };
            let name = string_arg(args, 3).unwrap_or("");
            let (answer, written) = match string_arg(args, 2).unwrap_or("") {
                "remove" => {
                    let (old, written) = crate::inline_style::remove_property(&decls, name);
                    (JsValue::String(old), written)
                }
                _ => {
                    let written = crate::inline_style::set_property(
                        &decls,
                        name,
                        string_arg(args, 4).unwrap_or(""),
                        string_arg(args, 5).unwrap_or(""),
                    );
                    (JsValue::Boolean(written.is_some()), written)
                }
            };
            if let Some(decls) = written {
                let text = crate::inline_style::serialize_decls(&decls);
                // The next read or write of this text needs no parse, when the list reads back as itself
                // (the JS copy's `exactOf`). Kept before the write: a write the page's own code changes
                // again (a custom element's callback) leaves other text, which is parsed.
                if crate::inline_style::reads_back_as_itself(&decls) {
                    h.borrow().keep_style_decls(id, &text, decls);
                }
                let write_args =
                    [args[0].clone(), JsValue::String("setAttr".into()), JsValue::Number(id as f64), JsValue::String("style".into()), JsValue::String(text)];
                if let JsValue::String(_) = dom_write(&h, &d, &m, &l, &write_args) {
                    return JsValue::Number(-1.0);
                }
            }
            answer
        }),
    )?;

    // Whether script has written to the DOM since the engine last took the
    // mark, i.e. whether the published geometry is stale and a layout is owed.
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_dirty",
        0,
        Box::new(move |_| JsValue::Boolean(d.get() != DomDirty::Clean)),
    )?;

    let h = host.clone();
    let d = dirty.clone();
    let m = mutations.clone();
    let l = log.clone();
    runtime.register_host_function(
        "__rustkit_dom_value",
        3,
        Box::new(move |args| {
            let (result, bucket) = control_value(&mut h.borrow_mut(), args);
            let mut logged = crate::Logged::No;
            if let Some(node) = h.borrow().node(args).filter(|_| bucket != DomDirty::Clean) {
                l.borrow_mut().push(control_record(node.id.raw(), None));
                logged = crate::Logged::Yes;
            }
            note_mutation(&m, bucket, MutationKind::FormValue, || {
                node_label(&h.borrow(), args, 1)
            });
            d.mark(bucket, "form", logged);
            result
        }),
    )?;

    let h = host.clone();
    let d = dirty.clone();
    let m = mutations.clone();
    let l = log.clone();
    runtime.register_host_function(
        "__rustkit_dom_checked",
        3,
        Box::new(move |args| {
            let bucket = control_checked(&mut h.borrow_mut(), args);
            let mut logged = crate::Logged::No;
            if let Some(node) = h.borrow().node(args).filter(|_| bucket != DomDirty::Clean) {
                l.borrow_mut().push(control_record(node.id.raw(), Some("checked")));
                logged = crate::Logged::Yes;
            }
            note_mutation(&m, bucket, MutationKind::FormValue, || {
                node_label(&h.borrow(), args, 1)
            });
            d.mark(bucket, "form", logged);
            JsValue::Null
        }),
    )?;

    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_submit",
        3,
        Box::new(move |args| {
            form_submit(&mut h.borrow_mut(), args);
            JsValue::Null
        }),
    )?;

    // `navigate(url, replace)`: script asked for another document at this
    // absolute URL, replacing the current history entry or not. Nothing
    // navigates here; the embedder takes the request.
    let h = host.clone();
    runtime.register_host_function(
        "__rustkit_dom_navigate",
        2,
        Box::new(move |args| {
            if let Some(JsValue::String(url)) = args.first() {
                let replace = matches!(args.get(1), Some(JsValue::Boolean(true)));
                h.borrow_mut().navigation_requests.push((url.clone(), replace));
                crate::note_location(url, replace);
            }
            JsValue::Null
        }),
    )?;

    // `inserted_script(id, url, in_order)`: script inserted this element
    // with this absolute `src`; `in_order` when it is not async (it runs
    // after the in-order scripts inserted before it). Nothing is fetched
    // here; the engine takes the queue, with where the page inserted it.
    let h = host.clone();
    runtime.register_host_function_with_caller(
        "__rustkit_dom_inserted_script",
        3,
        Box::new(move |args, caller| {
            if let (Some(JsValue::Number(id)), Some(JsValue::String(url))) = (args.first(), args.get(1)) {
                let in_order = matches!(args.get(2), Some(JsValue::Boolean(true)));
                let mut host = h.borrow_mut();
                // Bounds what a page can park between two engine drains.
                if host.inserted_scripts.len() < 256 {
                    host.inserted_scripts.push(crate::InsertedScript {
                        node: *id as usize,
                        url: url.clone(),
                        in_order,
                        inserted_by: caller,
                        inserted_at: std::time::Instant::now(),
                    });
                }
            }
            JsValue::Null
        }),
    )?;

    // `set_adopted(gen, id, json)`: CSS text of `node`'s adoptedStyleSheets,
    // a JSON array of strings in array order. An empty string is a disabled
    // sheet. The next layout applies them. Nothing is injected into the tree.
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_set_adopted",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(node) = host.node(args) else {
                return JsValue::Null;
            };
            let Some(json) = string_arg(args, 2) else {
                return JsValue::Null;
            };
            let Ok(texts) = serde_json::from_str::<Vec<String>>(json) else {
                return JsValue::Null;
            };
            node.set_adopted_styles(texts);
            d.mark(DomDirty::Style, "sheet", crate::Logged::No);
            JsValue::Null
        }),
    )?;

    // `register_property(name, syntax, inherits, initialValue)`:
    // `CSS.registerProperty()` (css-properties-values-api-1 §3.1). The
    // descriptor is validated here and kept on the document as `@property`
    // text, which the next layout reads after the document's sheets.
    // Answers `''`, or the DOMException name to throw.
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_register_property",
        4,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(document) = host.document.clone() else {
                return JsValue::String(String::new());
            };
            let name = string_arg(args, 0).unwrap_or("");
            let syntax = string_arg(args, 1).unwrap_or("*");
            let inherits = matches!(args.get(2), Some(JsValue::Boolean(true)));
            let initial = string_arg(args, 3);
            let registration = match rustkit_css::Registration::new(name, syntax, Some(inherits), initial) {
                Ok(r) => r,
                Err(e) => return JsValue::String(e.to_string()),
            };
            let rule = rustkit_css::registered_property::registration_rule(
                &registration.name,
                syntax,
                inherits,
                initial,
            );
            if !document.register_property(name, rule) {
                return JsValue::String("InvalidModificationError".into());
            }
            d.mark(DomDirty::Style, "sheet", crate::Logged::No);
            JsValue::String(String::new())
        }),
    )?;

    // `set_sheet_disabled(gen, id, on)`: `<style>.sheet.disabled`. The
    // engine leaves the element's sheet out of the cascade while its text
    // is the text it had now (web_cssom.js calls again after a CSSOM edit).
    let h = host.clone();
    let d = dirty.clone();
    runtime.register_host_function(
        "__rustkit_dom_set_sheet_disabled",
        3,
        Box::new(move |args| {
            let host = h.borrow();
            let Some(node) = host.node(args) else {
                return JsValue::Null;
            };
            let Some(document) = args.first().and_then(|g| host.document_for(g)) else {
                return JsValue::Null;
            };
            let on = matches!(args.get(2), Some(JsValue::Boolean(true)));
            document.set_style_sheet_disabled(&node, on);
            d.mark(DomDirty::Style, "sheet", crate::Logged::No);
            JsValue::Null
        }),
    )?;

    // Its listener and handler errors go to the host natives (`__rk`).
    runtime.evaluate_with_host(WRAPPERS_JS)?;
    shadow::install(runtime, host, dirty)?;
    node_apis::install(runtime, host)
}

/// Wrapper prototypes, the identity cache, and the `document` read
/// surface. `__rk.domReset(gen)` rebinds it to a new document.
#[cfg(test)]
pub(crate) const WRAPPERS_JS_FOR_TESTS: &str = WRAPPERS_JS;

const WRAPPERS_JS: &str = r#"
(function (g) {
    var N = {
        root: __rustkit_dom_root, byId: __rustkit_dom_by_id,
        collect: __rustkit_dom_collect, info: __rustkit_dom_info,
        attr: __rustkit_dom_attr, mutate: __rustkit_dom_mutate,
        write: __rustkit_dom_write, matches: __rustkit_dom_matches,
        value: __rustkit_dom_value, resolve: __rustkit_dom_resolve,
        checked: __rustkit_dom_checked, submit: __rustkit_dom_submit,
        navigate: __rustkit_dom_navigate, insertedScript: __rustkit_dom_inserted_script,
        hit: __rustkit_dom_hit, svgHit: __rustkit_dom_svg_hit, noteWheel: __rustkit_dom_note_wheel,
        wheelAttribute: __rustkit_dom_wheel_attribute,
        rangeTextRects: __rustkit_dom_range_text_rects, setAdopted: __rustkit_dom_set_adopted,
        setSheetDisabled: __rustkit_dom_set_sheet_disabled,
        registerProperty: __rustkit_dom_register_property,
        scrollElement: __rustkit_dom_scroll_element,
        animate: __rustkit_dom_animate, cssAnimations: __rustkit_dom_css_animations,
        cssControl: __rustkit_dom_css_control, animated: __rustkit_dom_animated,
        styleRead: __rustkit_style_read, styleWrite: __rustkit_style_write
    };
    delete g.__rustkit_style_read;
    delete g.__rustkit_style_write;
    ['root', 'by_id', 'collect', 'info', 'attr', 'mutate', 'write', 'matches', 'value', 'resolve', 'checked', 'submit', 'navigate',
        'inserted_script', 'hit', 'svg_hit', 'note_wheel', 'wheel_attribute', 'range_text_rects', 'set_adopted', 'set_sheet_disabled', 'register_property', 'scroll_element', 'animate', 'css_animations', 'css_control', 'animated'].forEach(function (n) {
        delete g['__rustkit_dom_' + n];
    });
    // The DOM's dirty flag (a native registered as a global), for
    // web_observers_live.js.
    __rk.domDirty = g.__rustkit_dom_dirty;
    // The frame tree's top window as the engine knows it: this window,
    // until it is attached as a frame (`DomBindings::attach_frame`).
    __rk.realTop = g;
    delete g.__rustkit_dom_dirty;

    // Which events the engine made (DOM §2.2 `isTrusted`). The sets are
    // this closure's and are read with methods taken now, before any page
    // script: a page cannot add to them, and replacing `WeakSet.prototype.has`
    // or `Object.defineProperty` afterwards changes nothing here.
    var uncurry = Function.prototype.bind.bind(Function.prototype.call);
    // The constructor of install time too, for a set made later.
    var WeakSetNow = WeakSet;
    var setAdd = uncurry(WeakSet.prototype.add), setHas = uncurry(WeakSet.prototype.has),
        setDrop = uncurry(WeakSet.prototype['delete']);
    var defineOwn = Object.defineProperty;
    // The documents of a frame tree share these (`__rk.shared` is
    // one object for every realm of the runtime): an event made in one
    // window and dispatched in another is stamped once, and reads the same.
    var SHARED = __rk.shared;
    var TRUSTED = SHARED.trusted || (SHARED.trusted = new WeakSet()),
        UNDISPATCHED = SHARED.undispatched || (SHARED.undispatched = new WeakSet()),
        STAMPED = SHARED.stamped || (SHARED.stamped = new WeakSet());
    var TRUST_GET = SHARED.trustGet || (SHARED.trustGet =
        Object.getOwnPropertyDescriptor({ get isTrusted() { return setHas(TRUSTED, this); } }, 'isTrusted').get);
    // Which events are cancelled (DOM §2.2 "canceled flag"). Held here, so
    // `defaultPrevented` is read-only and only `preventDefault()` (or
    // `returnValue = false`, or an on<type> handler returning false) by a
    // listener that may cancel sets it: a passive listener cannot. Shared
    // by the frame tree, as the trust sets are: an event cancelled in one
    // window reads cancelled in the window that made it.
    var CANCELED = SHARED.canceled || (SHARED.canceled = new WeakSet());
    // Whether an event may be cancelled (WHL3): shared the same way, so
    // `cancelable` reads the same in every window of the frame tree. (The
    // slot's name is not an event member's name.)
    var CANCELABLE = SHARED.cancelables || (SHARED.cancelables = new WeakSet());
    function setCancelable(ev, on) { if (on) setAdd(CANCELABLE, ev); else setDrop(CANCELABLE, ev); }
    // A real event is one whose prototype chain holds the `Event.prototype`
    // of some window of the frame tree, not only this one's: an event made
    // in a frame and dispatched here keeps the canceled flag, not a
    // `defaultPrevented` property. Each realm adds its own prototype when
    // its `Event` is defined (below).
    var isProto = uncurry(Object.prototype.isPrototypeOf);
    var EVENT_PROTOS = SHARED.eventProtos || (SHARED.eventProtos = []);
    function isRealEvent(ev) {
        for (var k = 0; k < EVENT_PROTOS.length; k++) if (isProto(EVENT_PROTOS[k], ev)) return true;
        return false;
    }
    // Give an event object its `isTrusted`: an own accessor with no setter
    // that cannot be redefined ([LegacyUnforgeable]). Throws a TypeError
    // for an object that will not take it.
    function stamp(ev) {
        if (setHas(STAMPED, ev)) return;
        defineOwn(ev, 'isTrusted', getterDesc(TRUST_GET));
        setAdd(STAMPED, ev);
    }
    // The engine made this event. It reads trusted through its one
    // dispatch, the next `dispatchEvent` of it; one dispatched again (by
    // a page that kept it) is not trusted any more.
    //
    // Only an event this code just made with a constructor kept at install
    // (`make`) can be trusted, and once: whatever a page has put on the
    // window since never builds an event the engine marks.
    //
    // The constructors and prototypes are each window's own (an event is
    // built on the prototype of the window that makes it). What is kept
    // about an event is the frame tree's, as the trust sets are: one
    // window makes an event, another may dispatch it, and a listener may
    // reach it through either window's prototypes; all of them read and
    // write the one record.
    var CTORS = Object.create(null), MADE = SHARED.made || (SHARED.made = new WeakSet());
    // The engine builds its events with nothing a page can reach: these
    // built-ins are taken now, and every descriptor has no prototype.
    var objCreate = Object.create, hasOwn = uncurry(Object.prototype.hasOwnProperty), isArray = Array.isArray, dateNow = Date.now;
    var setProto = Object.setPrototypeOf, mathMax = Math.max, mathRound = Math.round, mathAbs = Math.abs;
    // What describes an engine event (the embedder's literal) inherits
    // from nothing before the engine reads or writes a field of it.
    function bare(o) { return setProto(o, null); }
    // An event's dispatch state under the engine's own keys: own
    // properties of an engine event from the start (`make`). The keys are
    // the frame tree's, like the event state above: an event stopped
    // through one window's prototype is stopped in another's dispatch,
    // and its path reads the same through either.
    var STOP = SHARED.stop || (SHARED.stop = Symbol('rustkit.stop')),
        STOP_NOW = SHARED.stopNow || (SHARED.stopNow = Symbol('rustkit.stopNow'));
    var ORIGIN = SHARED.origin || (SHARED.origin = Symbol('rustkit.origin')),
        PATH = SHARED.path || (SHARED.path = Symbol('rustkit.path'));
    function getterDesc(get) {
        var d = objCreate(null);
        d.get = get; d.set = undefined; d.enumerable = true; d.configurable = false;
        return d;
    }
    function fixed(o, k, v) {
        var d = objCreate(null);
        d.value = v; d.writable = true; d.enumerable = true; d.configurable = false;
        defineOwn(o, k, d);
    }
    // What the engine adds to its event past the interface's fields (a
    // point's page and offset coordinates), defined as the fields are.
    function extra(ev, at) {
        for (var k in at) {
            if (!hasOwn(at, k)) continue;
            if (hasOwn(ev, k)) { fixed(ev, k, at[k]); continue; }
            var d = objCreate(null);
            d.value = at[k]; d.writable = false; d.enumerable = true; d.configurable = true;
            defineOwn(ev, k, d);
        }
    }
    // This window becomes a frame's (`DomBindings::attach_frame`): its
    // `parent`, the frame tree's `top` and its `frameElement`, defined with
    // the built-ins of install whatever the frame's page has done since.
    __rk.becomeFrame = function (up, top, el) {
        function own(name, value, writable) {
            var d = objCreate(null);
            d.value = value; d.writable = writable; d.enumerable = true; d.configurable = true;
            defineOwn(g, name, d);
        }
        own('parent', up, true);
        own('top', top, false);
        own('frameElement', el, false);
        __rk.realTop = top;
    };
    // An interface's own fields (`[name, default]` pairs), its parent and
    // a finishing step (a direct call; WheelEvent's legacy deltas).
    var FIELDS = Object.create(null), PARENT = Object.create(null), FINISH = Object.create(null);
    // The install scripts give the engine its constructors here (host
    // object only; the last one an install script gives wins).
    var PROTOS = Object.create(null), EVTYPE = SHARED.evType || (SHARED.evType = new WeakMap()),
        KIND = SHARED.kind || (SHARED.kind = new WeakMap());
    // An event's dispatch state, private: `target`, `currentTarget` and
    // `eventPhase` are read-only getters over these (as in Chrome), and an
    // event being dispatched is in DISPATCHING, which the initializers
    // check. Shared by the frame tree: an event in flight in one window
    // is in flight for every window's `initEvent` and `dispatchEvent`, and
    // its target reads the same through every window's `Event.prototype`.
    var EV_TARGET = SHARED.evTarget || (SHARED.evTarget = new WeakMap()),
        EV_CURRENT = SHARED.evCurrent || (SHARED.evCurrent = new WeakMap()),
        EV_PHASE = SHARED.evPhase || (SHARED.evPhase = new WeakMap()),
        DISPATCHING = SHARED.dispatching || (SHARED.dispatching = new WeakSet());
    __rk.isDispatching = function (e) { return setHas(DISPATCHING, e); };
    var mapGet = uncurry(WeakMap.prototype.get), mapSet = uncurry(WeakMap.prototype.set), mapHas = uncurry(WeakMap.prototype.has);
    __rk.keepCtor = function (name, C, parent, fields) {
        if (typeof C !== 'function') return;
        CTORS[name] = C;
        PROTOS[name] = C.prototype;
        if (typeof parent === 'string') { PARENT[name] = parent; FIELDS[name] = fields || []; }
        // An interface's prototype cannot be replaced (Web IDL).
        try { defineOwn(C, 'prototype', { writable: false }); } catch (_) {}
    };
    // An event's type as its constructor or initEvent set it: what dispatch
    // and activation read, whatever a page defines on the prototypes.
    // An engine event is always in the map; only a page's own event, which
    // is never trusted, falls back to its `type`.
    function evType(e) { return mapHas(EVTYPE, e) ? mapGet(EVTYPE, e) : e.type; }
    __rk.keepFinish = function (name, fn) { if (typeof fn === 'function') FINISH[name] = fn; };
    // A click made by the engine as a mouse or pointer event, or made by
    // script with the MouseEvent of install time.
    function isMouseClick(e) {
        var k = mapGet(KIND, e);
        if (k !== undefined) return k === 'MouseEvent' || k === 'PointerEvent';
        return !!PROTOS.MouseEvent && isProto(PROTOS.MouseEvent, e);
    }
    __rk.hasCtor = function (name) { return typeof CTORS[name] === 'function'; };
    // The engine's event: an object of the interface's prototype with
    // every field an own, non-configurable property, built here and
    // nowhere else (no constructor, no call through a built-in a page can
    // replace). `init` is the engine's own object literal.
    function make(name, type, init) {
        if (!PROTOS[name] || (name !== 'Event' && !FIELDS[name])) name = 'Event';
        var i = init || objCreate(null);
        function arg(k) { return hasOwn(i, k) ? i[k] : undefined; }
        var ev = objCreate(PROTOS[name]);
        mapSet(EVTYPE, ev, '' + type);
        defineOwn(ev, 'type', getterDesc(TYPE_GET));
        fixed(ev, 'bubbles', !!arg('bubbles'));
        setCancelable(ev, !!arg('cancelable'));
        fixed(ev, 'composed', !!arg('composed'));
        stamp(ev);
        // On performance.now()'s clock, the page clock (`__rk.perfNow`, out of page script's reach).
        fixed(ev, 'timeStamp', typeof __rk.perfNow === 'function' ? __rk.perfNow() : dateNow());
        fixed(ev, ORIGIN, null); fixed(ev, PATH, null);
        fixed(ev, STOP, false); fixed(ev, STOP_NOW, false);
        // The fields, the root interface's first.
        var chain = objCreate(null), depth = 0, n;
        for (n = name; n !== 'Event' && FIELDS[n]; n = PARENT[n]) chain[depth++] = n;
        while (depth > 0) {
            var fields = FIELDS[chain[--depth]];
            for (var j = 0; j < fields.length; j++) {
                var f = fields[j], v = arg(f[0]);
                fixed(ev, f[0], v === undefined ? (isArray(f[1]) ? [] : f[1]) : v);
            }
        }
        var finish = FINISH[name];
        if (finish) finish(ev, i);
        mapSet(KIND, ev, name);
        setAdd(MADE, ev);
        return ev;
    }
    __rk.make = make;
    function trust(ev) {
        if (!setHas(MADE, ev) || !mapHas(EVTYPE, ev)) throw new TypeError('not an engine-made event');
        setDrop(MADE, ev);
        stamp(ev);
        setAdd(TRUSTED, ev);
        setAdd(UNDISPATCHED, ev);
        return ev;
    }
    // For the embedder's other scripts, which hold the host object.
    __rk.trust = trust;
    // A field the user edited (it commits with `change` when it loses the
    // focus): kept here, not as a property of the element.
    var EDITED = new WeakSet();
    __rk.markEdited = function (el) { setAdd(EDITED, el); };
    __rk.takeEdited = function (el) { var was = setHas(EDITED, el); if (was) setDrop(EDITED, el); return was; };

    if (typeof g.DOMException !== 'function') {
        g.DOMException = function DOMException(message, name) {
            if (!(this instanceof DOMException)) illegal();
            this.message = message === undefined ? '' : String(message);
            this.name = name === undefined ? 'Error' : String(name);
        };
        // The legacy `code` of the named errors (DOMException.code), as Chrome reports them.
        var DOM_CODES = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5,
            NoModificationAllowedError: 7, NotFoundError: 8, NotSupportedError: 9, InUseAttributeError: 10,
            InvalidStateError: 11, SyntaxError: 12, InvalidModificationError: 13, NamespaceError: 14, InvalidAccessError: 15,
            TypeMismatchError: 17, SecurityError: 18, NetworkError: 19, AbortError: 20, URLMismatchError: 21,
            QuotaExceededError: 22, TimeoutError: 23, InvalidNodeTypeError: 24, DataCloneError: 25 };
        g.DOMException.prototype = Object.create(Error.prototype, {
            constructor: { value: g.DOMException, writable: true, configurable: true },
            code: { get: function () { return DOM_CODES[this.name] || 0; }, configurable: true, enumerable: true }
        });
    }
    var DOMException = g.DOMException;

    var SLOT = Symbol('rustkit.node');
    // The record behind a wrapper: engine-owned, so it inherits nothing (a page's Object.prototype
    // cannot answer for a field it does not have). Fields: id, gen, and the children caches.
    function slotRecord(id) { var r = objCreate(null); r.id = id; r.gen = gen; return r; }
    // Hooks web_shadow.js fills in so that events and tree queries can cross
    // a shadow boundary: `shadowParent(root, event)` is the host a composed
    // event goes on to from a shadow root, `retarget(target, against)` the
    // target as seen from another node's tree, `hostOf(root)` a shadow
    // root's host (or null for any other node). `mutation` is the
    // MutationObserver's (web_mutation_observer.js): set from the first
    // observe(), it sees every write below before it happens and gets back
    // what to record once it has. `scripts` is web_scripts.js's: it hears of
    // each element script creates, each insert before it happens (and gets
    // back what to run once it has), and each write to a node's own data.
    // It inherits from nothing and owns every hook from the start, so a
    // hook nobody installed reads as its own `undefined`.
    var HOOKS = objCreate(null);
    HOOKS.shadowParent = HOOKS.retarget = HOOKS.hostOf = HOOKS.mutation = HOOKS.scripts = undefined;
    HOOKS.windowNamed = HOOKS.hasListener = HOOKS.loadSnapshot = HOOKS.loadReplay = HOOKS.nodeId = HOOKS.byTag = HOOKS.frames = HOOKS.attrs = HOOKS.watched = undefined;
    // On the host object, for the embedder's other scripts.
    __rk.domHooks = HOOKS;
    var gen = 0;
    // For web_frames.js (installed only with nested browsing contexts on):
    // the node id of a wrapper of this document, or -1.
    HOOKS.nodeId = function (el) {
        var s = el != null ? el[SLOT] : undefined;
        return s && s.gen === gen ? s.id : -1;
    };
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
    // EventTarget is the one interface of this family script may construct
    // (DOM §2.7), on its own or as the base of a class. PLAIN holds the
    // objects made that way: they have no on<type> handler attributes.
    var PLAIN = new WeakSet();
    var EventTarget = function EventTarget() {
        if (!new.target) {
            throw new TypeError("Failed to construct 'EventTarget': Please use the 'new' operator, " +
                "this DOM object constructor cannot be called as a function.");
        }
        PLAIN.add(this);
    };
    Object.defineProperty(EventTarget.prototype, Symbol.toStringTag, { value: 'EventTarget' });
    g.EventTarget = EventTarget;
    var Node = iface('Node', EventTarget);
    var Document = iface('Document', Node);
    var CharacterData = iface('CharacterData', Node);
    var Text = iface('Text', CharacterData);
    var Comment = iface('Comment', CharacterData);
    var Element = iface('Element', Node);
    var HTMLElement = iface('HTMLElement', Element);
    var HTMLInputElement = iface('HTMLInputElement', HTMLElement);
    var HTMLTextAreaElement = iface('HTMLTextAreaElement', HTMLElement);
    var HTMLFormElement = iface('HTMLFormElement', HTMLElement);
    var HTMLScriptElement = iface('HTMLScriptElement', HTMLElement);
    var elementProtos = { input: HTMLInputElement.prototype,
                          textarea: HTMLTextAreaElement.prototype, form: HTMLFormElement.prototype,
                          script: HTMLScriptElement.prototype };
    // The rest of the HTML element interfaces (HTML §4), so `el instanceof
    // HTMLAnchorElement` and `typeof HTMLImageElement` work on real nodes.
    // Tags without an interface of their own stay plain HTMLElement.
    (function () {
        var HTMLMediaElement = iface('HTMLMediaElement', HTMLElement);
        var byTag = {
            a: 'HTMLAnchorElement', area: 'HTMLAreaElement', audio: 'HTMLAudioElement', base: 'HTMLBaseElement',
            body: 'HTMLBodyElement', br: 'HTMLBRElement', button: 'HTMLButtonElement', canvas: 'HTMLCanvasElement',
            data: 'HTMLDataElement', details: 'HTMLDetailsElement', dialog: 'HTMLDialogElement', div: 'HTMLDivElement',
            dl: 'HTMLDListElement', embed: 'HTMLEmbedElement', fieldset: 'HTMLFieldSetElement', frameset: 'HTMLFrameSetElement', head: 'HTMLHeadElement',
            h1: 'HTMLHeadingElement', h2: 'HTMLHeadingElement', h3: 'HTMLHeadingElement', h4: 'HTMLHeadingElement',
            h5: 'HTMLHeadingElement', h6: 'HTMLHeadingElement', hr: 'HTMLHRElement', html: 'HTMLHtmlElement',
            iframe: 'HTMLIFrameElement', img: 'HTMLImageElement', label: 'HTMLLabelElement', legend: 'HTMLLegendElement',
            li: 'HTMLLIElement', link: 'HTMLLinkElement', map: 'HTMLMapElement', meta: 'HTMLMetaElement',
            meter: 'HTMLMeterElement', object: 'HTMLObjectElement', ol: 'HTMLOListElement', optgroup: 'HTMLOptGroupElement',
            option: 'HTMLOptionElement', output: 'HTMLOutputElement', p: 'HTMLParagraphElement', picture: 'HTMLPictureElement',
            pre: 'HTMLPreElement', progress: 'HTMLProgressElement', blockquote: 'HTMLQuoteElement', q: 'HTMLQuoteElement',
            select: 'HTMLSelectElement', slot: 'HTMLSlotElement', source: 'HTMLSourceElement', span: 'HTMLSpanElement',
            style: 'HTMLStyleElement', table: 'HTMLTableElement', td: 'HTMLTableCellElement', th: 'HTMLTableCellElement',
            tr: 'HTMLTableRowElement', thead: 'HTMLTableSectionElement', tbody: 'HTMLTableSectionElement',
            tfoot: 'HTMLTableSectionElement', template: 'HTMLTemplateElement', time: 'HTMLTimeElement',
            title: 'HTMLTitleElement', track: 'HTMLTrackElement', ul: 'HTMLUListElement', video: 'HTMLVideoElement'
        };
        var made = {};
        Object.keys(byTag).forEach(function (tag) {
            var name = byTag[tag];
            if (!made[name]) {
                var parent = (name === 'HTMLAudioElement' || name === 'HTMLVideoElement') ? HTMLMediaElement : HTMLElement;
                made[name] = iface(name, parent);
            }
            elementProtos[tag] = made[name].prototype;
        });
        // Canvas contexts are web_canvas.js's, which takes this way to a canvas's node
        // from the host object.
        __rk.canvasSlot = function (el) { var s = slotOf(el); return [s.gen, s.id]; };
        // MEDIA-AVF M1.2: the media element's state is the engine's, pushed with each
        // event (`__rk.mediaEvent` below) and read here; `load()` and `canPlayType()`
        // ask the engine (`__rk.mediaCall`, web_media.rs). Playing, pausing and seeking
        // come in later steps: `play()` still rejects and `pause()` does nothing.
        var media = HTMLMediaElement.prototype;
        ['HAVE_NOTHING', 'HAVE_METADATA', 'HAVE_CURRENT_DATA', 'HAVE_FUTURE_DATA', 'HAVE_ENOUGH_DATA'].forEach(function (k, i) {
            HTMLMediaElement[k] = media[k] = i;
        });
        ['NETWORK_EMPTY', 'NETWORK_IDLE', 'NETWORK_LOADING', 'NETWORK_NO_SOURCE'].forEach(function (k, i) {
            HTMLMediaElement[k] = media[k] = i;
        });
        function mstate(el) { slotOf(el); return mediaStates.get(el) || MEDIA_INITIAL; }
        function getter(proto, name, read, write) {
            Object.defineProperty(proto, name, { get: read, set: write, enumerable: true, configurable: true });
        }
        media.load = function load() { var s = slotOf(this); __rk.mediaCall(s.gen, s.id, 'load'); };
        media.pause = function pause() {};
        media.canPlayType = function canPlayType(type) {
            if (arguments.length < 1) throw new TypeError("Failed to execute 'canPlayType' on 'HTMLMediaElement': 1 argument required, but only 0 present.");
            return __rk.mediaCall(0, 0, 'canPlayType', String(type));
        };
        media.play = function play() {
            return Promise.reject(new g.DOMException('The element has no supported sources.', 'NotSupportedError'));
        };
        getter(media, 'src', function () {
            var v = this.getAttribute('src');
            if (v === null) return '';
            try { return new g.URL(v, document.baseURI).href; } catch (_) { return v; }
        }, function (v) { this.setAttribute('src', String(v)); });
        getter(media, 'preload', function () {
            var v = this.getAttribute('preload');
            if (v === null) return 'metadata';
            v = v.toLowerCase();
            return v === '' ? 'auto' : (v === 'none' || v === 'metadata' || v === 'auto') ? v : 'metadata';
        }, function (v) { this.setAttribute('preload', String(v)); });
        getter(media, 'readyState', function () { return mstate(this).rs; });
        getter(media, 'networkState', function () { return mstate(this).ns; });
        getter(media, 'duration', function () { var d = mstate(this).duration; return d === null ? NaN : d; });
        getter(media, 'currentSrc', function () { return mstate(this).src; });
        getter(media, 'paused', function () { return true; });
        getter(media, 'ended', function () { return false; });
        getter(media, 'seeking', function () { return false; });
        getter(media, 'currentTime', function () { return 0; }, function () {});
        getter(media, 'error', function () {
            var st = mstate(this);
            if (st.error === null) return null;
            if (!st.errorObject) {
                st.errorObject = Object.create(MediaError.prototype);
                Object.defineProperty(st.errorObject, 'code', { value: st.error, enumerable: true });
                Object.defineProperty(st.errorObject, 'message', { value: '', enumerable: true });
            }
            return st.errorObject;
        });
        // HTML §4.8.11.7: TimeRanges, read-only and not constructible.
        var TimeRanges = iface('TimeRanges');
        var rangesOf = new WeakMap();
        function ranges(list) {
            var r = Object.create(TimeRanges.prototype);
            rangesOf.set(r, list.slice());
            return r;
        }
        function rangeAt(r, i, end) {
            var list = rangesOf.get(r);
            if (!list) throw new TypeError('Illegal invocation');
            i = Number(i) >>> 0;
            if (i >= list.length) throw new g.DOMException("The index provided (" + i + ") is greater than or equal to the maximum bound (" + list.length + ").", 'IndexSizeError');
            return list[i][end ? 1 : 0];
        }
        getter(TimeRanges.prototype, 'length', function () { var l = rangesOf.get(this); if (!l) throw new TypeError('Illegal invocation'); return l.length; });
        TimeRanges.prototype.start = function start(i) { return rangeAt(this, i, false); };
        TimeRanges.prototype.end = function end(i) { return rangeAt(this, i, true); };
        getter(media, 'buffered', function () { return ranges(mstate(this).buffered); });
        getter(media, 'seekable', function () { return ranges(mstate(this).seekable); });
        var video = made.HTMLVideoElement.prototype;
        getter(video, 'videoWidth', function () { return mstate(this).w; });
        getter(video, 'videoHeight', function () { return mstate(this).h; });
        // HTML §4.8.11. `MediaError` is the interface `video.error` would be,
        // and pages compare against the code constants. There is no playback
        // here, so the constructor stays illegal and `error` stays null.
        var MediaError = iface('MediaError');
        [['MEDIA_ERR_ABORTED', 1], ['MEDIA_ERR_NETWORK', 2], ['MEDIA_ERR_DECODE', 3], ['MEDIA_ERR_SRC_NOT_SUPPORTED', 4]].forEach(function (pair) {
            MediaError[pair[0]] = MediaError.prototype[pair[0]] = pair[1];
        });
        // Interface objects with no element of their own here: the checks
        // `x instanceof HTMLUnknownElement` / `SVGElement` still need a RHS.
        iface('HTMLUnknownElement', HTMLElement);
        var SVGElement = iface('SVGElement', Element);
        var SVGGraphicsElement = iface('SVGGraphicsElement', SVGElement);
        elementProtos.svg = iface('SVGSVGElement', SVGGraphicsElement).prototype;
    })();
    var DocumentFragment = iface('DocumentFragment', Node);
    var NodeList = iface('NodeList');
    var HTMLCollection = iface('HTMLCollection');

    var types = { ELEMENT_NODE: 1, TEXT_NODE: 3, PROCESSING_INSTRUCTION_NODE: 7,
                  COMMENT_NODE: 8, DOCUMENT_NODE: 9, DOCUMENT_TYPE_NODE: 10,
                  DOCUMENT_FRAGMENT_NODE: 11 };
    Object.keys(types).forEach(function (k) { Node[k] = Node.prototype[k] = types[k]; });

    function slotOf(o) {
        var s = o != null ? o[SLOT] : undefined;
        if (!s) throw new TypeError('Illegal invocation');
        return s;
    }
    // An element's interface; node_apis.js extends this by namespace.
    var elementProto = function (id) {
        return elementProtos[N.info(gen, id, 'local')] || HTMLElement.prototype;
    };
    // A shadow root is a fragment the DOM links to a host (dom/shadow.rs);
    // ShadowRoot is defined later, by web_interfaces.js.
    var fragmentProto = function () { return DocumentFragment.prototype; };
    function wrap(id) {
        if (typeof id !== 'number') return null;
        var w = cache.get(id);
        if (w) return w;
        var t = N.info(gen, id, 'type');
        var proto = t === 1 ? elementProto(id)
                  : t === 3 ? Text.prototype : t === 9 ? Document.prototype
                  : t === 8 ? Comment.prototype : t === 11 ? fragmentProto(id)
                  : Node.prototype;
        w = objCreate(proto);
        defineOwn(w, SLOT, { value: slotRecord(id) });
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
    // A node's children as the collection `children` (elements) or `childNodes` (nodes): the same
    // object while the child ids the engine reports are the same, as Chrome's live collections are (a
    // new one when they change; a held one stays the snapshot it was). The record keeps two things:
    // `list`, the collection the page gets, and `items`/`n`, the engine's own copy that its readers
    // (firstElementChild and the rest) use, so nothing a page does to its collection reaches them.
    // Built with plain loops and install-time built-ins. Each index of `list` is non-writable and
    // non-configurable, so a page's redefinition throws as Chrome's does.
    var lsSplit = uncurry(String.prototype.split), lsDef = Object.defineProperty;
    function cachedKids(o, onlyElements) {
        var s = slotOf(o);
        var ids = s.gen === gen ? N.info(s.gen, s.id, 'children') : '';
        var key = onlyElements ? 'kidsE' : 'kidsN';
        var c = s[key];
        if (c !== undefined && c.ids === ids) return c;
        var out = objCreate(onlyElements ? HTMLCollection.prototype : NodeList.prototype), n = 0, items = objCreate(null);
        if (typeof ids === 'string' && ids !== '') {
            var parts = lsSplit(ids, ' ');
            for (var i = 0; i < parts.length; i++) {
                var id = +parts[i], w = wrap(id);
                if (w && (!onlyElements || N.info(gen, id, 'type') === 1)) {
                    lsDef(out, n, { value: w, enumerable: true });
                    items[n] = w;
                    n++;
                }
            }
        }
        lsDef(out, 'length', { value: n });
        c = objCreate(null);
        c.ids = ids; c.list = out; c.items = items; c.n = n;
        s[key] = c;
        return c;
    }
    // The document's elements of one tag, for the embedder's scripts
    // (web_frames.js's scan): the native's answer read with plain loops,
    // no string or array method.
    HOOKS.byTag = function (tag) {
        var s = doc ? doc[SLOT] : undefined, out = objCreate(null), n = 0;
        out.length = 0;
        if (!s || s.gen !== gen) return out;
        var ids = N.collect(s.gen, s.id, 'tag', tag);
        if (typeof ids !== 'string') return out;
        var id = -1;
        for (var i = 0; i <= ids.length; i++) {
            var c = i < ids.length ? ids[i] : ' ';
            if (c >= '0' && c <= '9') { id = (id < 0 ? 0 : id * 10) + (+c); continue; }
            if (id >= 0) { var w = wrap(id); if (w) out[n++] = w; }
            id = -1;
        }
        out.length = n;
        return out;
    };
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
        var m = HOOKS.mutation && HOOKS.mutation.data(o, op, a);
        // Custom-element reaction: `HOOKS.attrs` answers a closure for a watched attribute (`HOOKS.watched`: the names some definition observes).
        var ar = HOOKS.attrs && (op === 'setAttr' || op === 'removeAttr') && HOOKS.attrs(o, op, a);
        // The steps that follow an attribute write act on three kinds of name only: `on<type>` (handlers),
        // `async` and `src` (scripts), `src` (frames), `src` and `srcset` (images). Whether the name begins with
        // o, s or a (either case) is one index read, with no call; every other name skips them.
        var c0 = a[0];
        var acts = !(op === 'setAttr' || op === 'removeAttr') ||
            c0 === 'o' || c0 === 's' || c0 === 'a' || c0 === 'O' || c0 === 'S' || c0 === 'A';
        // Removing an absent attribute runs no attribute-change steps (DOM
        // "remove an attribute by name"), so it must not drop an assigned
        // on<type> handler. N.attr matches the name as the write does.
        var changed = acts && (op === 'setAttr' ||
            (op === 'removeAttr' && s.gen === gen && N.attr(s.gen, s.id, a) !== null));
        var r = N.write(gen, op, s.gen === gen ? s.id : -1, a, b);
        if (typeof r === 'string') {
            throw new DOMException("Failed to execute '" + method + "'.", r);
        }
        if (m) m();
        if (acts) {
            if (changed) attributeChanged(o, a);
            if (HOOKS.scripts) HOOKS.scripts.changed(o, op, a);
            if (HOOKS.frames) HOOKS.frames.changed(o, op, a);
        }
        if (ar) ar();
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
    getter(Node.prototype, 'childNodes', function () { return cachedKids(this, false).list; });
    // A node in a DOMParser document is owned by it; every other node by
    // the global document.
    getter(Node.prototype, 'ownerDocument', function () {
        if (info(this, 'type') === 9) return null;
        // A node of another document, in a tree or not (HTML2).
        var owner = info(this, 'owner');
        if (typeof owner === 'number' && owner > 0) {
            if (g.document && g.document[SLOT] && g.document[SLOT].id === owner) return g.document;
            var od = wrap(owner); if (od) return od;
        }
        var n = this;
        while (n.parentNode) n = n.parentNode;
        return n.nodeType === 9 ? n : g.document;
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
    // `replacing`: `child` is the node this call takes out (replaceChild,
    // and replaceWith while `this` is still in place), so the Document
    // checks do not count it.
    function write(op, parent, node, child, method, replacing) {
        // A wrapper from an older document names no node here: -1 makes
        // the host answer NotFoundError instead of reusing its bare id.
        function here(s) { return s.gen === gen ? s.id : -1; }
        var p = slotOf(parent), n = nodeArg(node, method);
        var c = child == null ? null : here(nodeArg(child, method));
        var m = HOOKS.mutation && HOOKS.mutation.tree(op, parent, node, child);
        var inserted = op === 'insert' && HOOKS.scripts && HOOKS.scripts.inserting(node, parent);
        var err = N.mutate(gen, op, here(p), here(n), c, !!replacing);
        if (err) throw new DOMException("Failed to execute '" + method + "' on 'Node'.", err);
        if (m) m();
        if (inserted) inserted();
        if (HOOKS.frames) HOOKS.frames.tree(op, node, parent);
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
        var self = this;
        return batch(function () {
            write('insert', self, node, child, 'replaceChild', true);
            return write('remove', self, child, null, 'replaceChild');
        });
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
    function insertAll(parent, nodes, ref, method, replacing) {
        nodes.forEach(function (n) { write('insert', parent, n, ref, method, replacing); });
    }
    // DOM queues one record per call of these; they make several writes,
    // which an observer merges into one while `fn` runs.
    function batch(fn) {
        var m = HOOKS.mutation;
        if (!m) return fn();
        m.batch(1);
        try { return fn(); } finally { m.batch(-1); }
    }
    function viableSibling(node, field, nodes) {
        var s = node[field];
        while (s && nodes.indexOf(s) >= 0) s = s[field];
        return s;
    }
    [Element, DocumentFragment].forEach(function (C) {
        C.prototype.append = function () {
            var self = this, nodes = toNodes(arguments);
            batch(function () { insertAll(self, nodes, null, 'append'); });
        };
        C.prototype.prepend = function () {
            var self = this, nodes = toNodes(arguments);
            batch(function () { insertAll(self, nodes, self.firstChild, 'prepend'); });
        };
        C.prototype.replaceChildren = function () {
            var self = this, nodes = toNodes(arguments);
            batch(function () {
                while (self.firstChild) self.removeChild(self.firstChild);
                insertAll(self, nodes, null, 'replaceChildren');
            });
        };
    });
    [Element, CharacterData].forEach(function (C) {
        C.prototype.before = function () {
            var p = this.parentNode;
            if (!p) return;
            var args = Array.prototype.slice.call(arguments);
            var prev = viableSibling(this, 'previousSibling', args);
            var nodes = toNodes(args);
            batch(function () { insertAll(p, nodes, prev ? prev.nextSibling : p.firstChild, 'before'); });
        };
        C.prototype.after = function () {
            var p = this.parentNode;
            if (!p) return;
            var args = Array.prototype.slice.call(arguments);
            var next = viableSibling(this, 'nextSibling', args);
            var nodes = toNodes(args);
            batch(function () { insertAll(p, nodes, next, 'after'); });
        };
        C.prototype.replaceWith = function () {
            var p = this.parentNode;
            if (!p) return;
            var args = Array.prototype.slice.call(arguments);
            var next = viableSibling(this, 'nextSibling', args);
            var self = this, nodes = toNodes(args);
            batch(function () {
                if (self.parentNode === p) {
                    insertAll(p, nodes, self, 'replaceWith', true);
                    p.removeChild(self);
                } else {
                    insertAll(p, nodes, next, 'replaceWith');
                }
            });
        };
    });
    Node.prototype.contains = function (other) {
        for (var n = other; n; n = n.parentNode) if (n === this) return true;
        return false;
    };
    Node.prototype.isSameNode = function (other) { return this === other; };
    Node.prototype.getRootNode = function (options) {
        var n = this;
        while (n.parentNode) n = n.parentNode;
        // { composed: true } continues through shadow roots to their hosts.
        for (var host; options && options.composed && HOOKS.hostOf && (host = HOOKS.hostOf(n));) {
            n = host;
            while (n.parentNode) n = n.parentNode;
        }
        return n;
    };
    Node.prototype.cloneNode = function (deep) {
        var copy = wrap(setData(this, 'clone', !!deep, null, 'cloneNode'));
        if (HOOKS.scripts) HOOKS.scripts.cloned(this, copy);
        return copy;
    };
    // DOM §4.4 "equals": same type, name, attributes (in any order) and
    // data, and equal children in order.
    function equalNodes(a, b) {
        if (a.nodeType !== b.nodeType || a.nodeName !== b.nodeName) return false;
        if (a.nodeType === 1) {
            // The same attributes, in any order (DOM §4.4 "equals").
            var names = a.getAttributeNames();
            if (names.length !== b.getAttributeNames().length) return false;
            for (var k = 0; k < names.length; k++) {
                if (!b.hasAttribute(names[k]) || a.getAttribute(names[k]) !== b.getAttribute(names[k])) return false;
            }
        } else if (a.nodeType === 3 || a.nodeType === 7 || a.nodeType === 8) {
            if (a.textContent !== b.textContent) return false;
        }
        var ac = a.childNodes, bc = b.childNodes;
        if (ac.length !== bc.length) return false;
        for (var i = 0; i < ac.length; i++) if (!equalNodes(ac[i], bc[i])) return false;
        return true;
    }
    Node.prototype.isEqualNode = function (other) {
        slotOf(this);
        if (other === null) return false;
        nodeArg(other, 'isEqualNode');
        return equalNodes(this, other);
    };
    var positions = { DOCUMENT_POSITION_DISCONNECTED: 1, DOCUMENT_POSITION_PRECEDING: 2,
                      DOCUMENT_POSITION_FOLLOWING: 4, DOCUMENT_POSITION_CONTAINS: 8,
                      DOCUMENT_POSITION_CONTAINED_BY: 16,
                      DOCUMENT_POSITION_IMPLEMENTATION_SPECIFIC: 32 };
    Object.keys(positions).forEach(function (k) { Node[k] = Node.prototype[k] = positions[k]; });
    function ancestry(n) { var out = []; for (; n; n = n.parentNode) out.unshift(n); return out; }
    // DOM §4.4 compareDocumentPosition, answered for `other` relative to
    // `this`. Nodes in different trees get a consistent order by NodeId.
    Node.prototype.compareDocumentPosition = function (other) {
        var o = nodeArg(other, 'compareDocumentPosition'), s = slotOf(this);
        if (this === other) return 0;
        var a = ancestry(this), b = ancestry(other);
        if (a[0] !== b[0]) return 1 | 32 | (s.id < o.id ? 4 : 2);
        var i = 0;
        while (i < a.length && i < b.length && a[i] === b[i]) i++;
        if (i === a.length) return 16 | 4;
        if (i === b.length) return 8 | 2;
        for (var n = a[i]; n; n = n.nextSibling) if (n === b[i]) return 4;
        return 2;
    };
    // DOM §4.4 normalize: merge adjacent Text nodes, drop empty ones.
    Node.prototype.normalize = function () {
        for (var n = this.firstChild; n; ) {
            var next = n.nextSibling;
            if (n.nodeType === 3) {
                var data = n.data;
                while (next && next.nodeType === 3) {
                    data += next.data;
                    var after = next.nextSibling;
                    this.removeChild(next);
                    next = after;
                }
                if (data === '') this.removeChild(n);
                else if (data !== n.data) n.data = data;
            } else {
                n.normalize();
            }
            n = next;
        }
    };
    ['data', 'nodeValue'].forEach(function (k) {
        accessor(CharacterData.prototype, k, function () { return info(this, 'text'); },
            function (v) { setData(this, 'setText', text(v), null, k); });
    });
    getter(CharacterData.prototype, 'length', function () { return (info(this, 'text') || '').length; });

    getter(Element.prototype, 'tagName', function () { return info(this, 'name'); });
    accessor(Element.prototype, 'innerHTML', function () { return info(this, 'innerHTML'); },
        function (v) { setData(this, 'setHTML', text(v), null, 'innerHTML'); });
    // The outerHTML setter: the markup, parsed in the context of the
    // element's parent by a holder of the element's document, replaces the
    // element. As Chrome for Testing 148, and not as the HTML spec (which
    // has a no-op and a body context there): an element with no parent, or
    // whose parent is not an element, throws NoModificationAllowedError.
    accessor(Element.prototype, 'outerHTML', function () { return info(this, 'outerHTML'); }, function (v) {
        var parent = this.parentNode;
        if (!parent) {
            throw new DOMException("Failed to set the 'outerHTML' property on 'Element': This element has no parent node.", 'NoModificationAllowedError');
        }
        if (parent.nodeType !== 1) {
            throw new DOMException("Failed to set the 'outerHTML' property on 'Element': This element's parent is of type '" +
                parent.nodeName + "', which is not an element node.", 'NoModificationAllowedError');
        }
        var ctx = parent.localName;
        var holder = parent.namespaceURI === 'http://www.w3.org/2000/svg'
            ? document.createElementNS(parent.namespaceURI, ctx)
            : create('element', ctx, 'outerHTML');
        ownedBy(holder, this.ownerDocument);
        holder.innerHTML = text(v);
        var self = this;
        Array.prototype.slice.call(holder.childNodes).forEach(function (n) { parent.insertBefore(n, self); });
        parent.removeChild(self);
    });
    getter(Element.prototype, 'localName', function () { return info(this, 'local'); });
    accessor(HTMLElement.prototype, 'innerText', function () { return info(this, 'innerText'); },
        function (v) { setData(this, 'setInnerText', text(v), null, 'innerText'); });
    // `id` and `className` read the attribute with the engine native and write it with the same
    // write `setAttribute` makes (the mutation record included), without the steps that only act
    // on another name (`on<type>` handlers, `async`, `src`): neither name is one.
    var accStr = String;
    // `name` is a literal that is already lower case, so the custom-element reaction is only asked for
    // when some definition watches it.
    function putPlain(el, name, text) {
        var s = slotOf(el);
        var m = HOOKS.mutation && HOOKS.mutation.data(el, 'setAttr', name);
        var ar = HOOKS.attrs && HOOKS.watched[name] === true && HOOKS.attrs(el, 'setAttr', name, true);
        var r = N.write(gen, 'setAttr', s.gen === gen ? s.id : -1, name, text);
        if (typeof r === 'string') throw new DOMException("Failed to execute 'setAttribute'.", r);
        if (m) m();
        if (ar) ar();
    }
    // setAttribute and removeAttribute for a name that none of the steps after a write act on (below): the
    // same write and the same mutation record as `setData`, any case of name.
    function putAttr(el, op, name, text, method) {
        var s = slotOf(el);
        var m = HOOKS.mutation && HOOKS.mutation.data(el, op, name);
        var ar = HOOKS.attrs && HOOKS.attrs(el, op, name);
        var r = N.write(gen, op, s.gen === gen ? s.id : -1, name, text);
        if (typeof r === 'string') throw new DOMException("Failed to execute '" + method + "'.", r);
        if (m) m();
        if (ar) ar();
    }
    [['id', 'id'], ['className', 'class']].forEach(function (p) {
        var attr = p[1];
        accessor(Element.prototype, p[0], function () {
            var s = slotOf(this), v = N.attr(s.gen, s.id, attr);
            return v === null ? '' : v;
        }, function (v) { putPlain(this, attr, accStr(v)); });
    });
    getter(Element.prototype, 'children', function () { return cachedKids(this, true).list; });

    // ParentNode / NonDocumentTypeChildNode element traversal (DOM §4.2.6,
    // §4.2.7), over the same tree reads as childNodes and the siblings.
    function elementChildren(o) { return cachedKids(o, true); }
    function elementSibling(o, field) {
        var n = o[field];
        while (n && n.nodeType !== 1) n = n[field];
        return n;
    }
    [Element, Document, DocumentFragment].forEach(function (C) {
        getter(C.prototype, 'firstElementChild', function () { var c = elementChildren(this); return c.n ? c.items[0] : null; });
        getter(C.prototype, 'lastElementChild', function () {
            var c = elementChildren(this); return c.n ? c.items[c.n - 1] : null;
        });
        getter(C.prototype, 'childElementCount', function () { return elementChildren(this).n; });
    });
    [Document, DocumentFragment].forEach(function (C) {
        getter(C.prototype, 'children', function () { return cachedKids(this, true).list; });
    });
    [Element, CharacterData].forEach(function (C) {
        getter(C.prototype, 'nextElementSibling', function () { return elementSibling(this, 'nextSibling'); });
        getter(C.prototype, 'previousElementSibling', function () {
            return elementSibling(this, 'previousSibling');
        });
    });
    // A node is connected when its root is a document (the current one,
    // or a DOMParser one); an old document's wrappers have no parent and
    // are never connected.
    getter(Node.prototype, 'isConnected', function () {
        var n = this;
        while (n.parentNode) n = n.parentNode;
        // A node in a shadow tree is connected when its host is.
        for (var host; HOOKS.hostOf && (host = HOOKS.hostOf(n));) {
            n = host;
            while (n.parentNode) n = n.parentNode;
        }
        return n.nodeType === 9 && slotOf(n).gen === gen;
    });

    // HTML §4.12.3 template.content: the fragment its parsed children
    // were moved into (the host keeps one per template).
    getter(elementProtos.template, 'content', function () { return related(this, 'content'); });

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
    // The DOMStringMap's names (HTML §3.2.6.6): each `data-*` attribute
    // with no ASCII upper alpha after the prefix, `-x` camel-cased.
    function dataNames(el) {
        var out = [];
        el.getAttributeNames().forEach(function (a) {
            if (a.slice(0, 5) === 'data-' && !/[A-Z]/.test(a.slice(5))) {
                out.push(a.slice(5).replace(/-([a-z])/g, function (m, c) { return c.toUpperCase(); }));
            }
        });
        return out;
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
            // Enumeration (Object.keys, for-in, JSON.stringify).
            ownKeys: function () { return dataNames(el); },
            getOwnPropertyDescriptor: function (t, p) {
                var a = dataAttr(p), v = a ? el.getAttribute(a) : null;
                return v === null ? undefined
                    : { value: v, writable: true, enumerable: true, configurable: true };
            },
            deleteProperty: function (t, p) {
                var a = dataAttr(p);
                if (a) el.removeAttribute(a);
                return true;
            }
        });
        datasets.set(el, d);
        return d;
    });

    // HTMLScriptElement.src (HTML §4.12.1): the `src` attribute resolved
    // against the document's URL, '' when absent. webpack's `publicPath`
    // reads `document.currentScript.src`.
    accessor(HTMLScriptElement.prototype, 'src', function () {
        var v = this.getAttribute('src');
        if (v === null) return '';
        var r = N.resolve(g.location && g.location.href, v);
        return r === null ? v : r;
    }, function (v) { this.setAttribute('src', v); });

    // Form controls (HTML §4.10). A text control's value lives host-side
    // (its dirty value, which the engine's edit state mirrors for paint);
    // the other input value modes read and write the `value` attribute.
    function controlValue(el, v) {
        var s = slotOf(el);
        var r = s.gen === gen ? (v === undefined ? N.value(s.gen, s.id) : N.value(s.gen, s.id, v)) : null;
        return r === null ? '' : r;
    }
    function inputMode(el) {
        var t = el.type;
        if (t === 'checkbox' || t === 'radio') return 'default/on';
        return /^(hidden|submit|image|reset|button)$/.test(t) ? 'default' : 'value';
    }
    var INPUT_TYPES = /^(hidden|text|search|tel|url|email|password|date|month|week|time|datetime-local|number|range|color|checkbox|radio|file|submit|image|reset|button)$/;
    accessor(HTMLInputElement.prototype, 'type', function () {
        var t = (this.getAttribute('type') || '').toLowerCase();
        return INPUT_TYPES.test(t) ? t : 'text';
    }, function (v) { this.setAttribute('type', v); });
    accessor(HTMLInputElement.prototype, 'value', function () {
        var m = inputMode(this);
        if (m === 'value') return controlValue(this);
        var a = this.getAttribute('value');
        return a === null ? (m === 'default/on' ? 'on' : '') : a;
    }, function (v) {
        v = v === null ? '' : String(v);
        if (inputMode(this) !== 'value') return this.setAttribute('value', v);
        setValueMovingCaret(this, v);
    });
    accessor(HTMLTextAreaElement.prototype, 'value', function () { return controlValue(this); },
        function (v) { setValueMovingCaret(this, v === null ? '' : String(v)); });
    // Setting a different value moves both ends of the selection to its
    // end; the same value leaves the selection alone (HTML §4.10.5.4).
    function setValueMovingCaret(el, v) {
        if (controlValue(el) === v) return;
        var n = controlValue(el, v).length;
        selectionOf(el).start = selectionOf(el).end = n;
    }
    // Script's view of the selection (HTML §4.10.5.2.10).
    // Once a control has an edit state (it was focused, typed in, or
    // script set its selection), the engine's state is the selection
    // (`engineSelection`, from dom/editing.js; CH2.E12); this copy stands
    // in before that.
    var selections = new WeakMap();
    var engineSelection = null;
    function selectionOf(el) {
        var s = selections.get(el);
        if (!s) { s = { start: 0, end: 0, dir: 'none' }; selections.set(el, s); }
        var e = engineSelection ? engineSelection(el) : null;
        if (e) { s.start = e.start; s.end = e.end; s.dir = e.dir; return s; }
        var n = controlValue(el).length;
        s.start = Math.min(s.start, n); s.end = Math.min(s.end, n);
        return s;
    }
    function setSelection(el, start, end, dir) {
        var s = selectionOf(el), n = el.value.length;
        s.end = Math.min(end >>> 0, n);
        s.start = Math.min(start >>> 0, s.end);
        s.dir = dir === 'forward' || dir === 'backward' ? dir : 'none';
        var e = engineSelection ? engineSelection(el, s.start, s.end, s.dir) : null;
        if (e) { s.start = e.start; s.end = e.end; s.dir = e.dir; }
    }
    // Validity (HTML §4.10.20.3) for the constraints this rung models.
    var customValidity = new WeakMap();
    function validityOf(el) {
        var v = el.value, min = el.minLength, max = el.maxLength, pat = el.getAttribute('pattern');
        var patternMismatch = false;
        if (pat !== null && v !== '') {
            try { patternMismatch = !new RegExp('^(?:' + pat + ')$').test(v); } catch (e) {}
        }
        var r = {
            valueMissing: el.required && v === '',
            tooShort: min > 0 && v !== '' && v.length < min,
            tooLong: max >= 0 && v.length > max,
            patternMismatch: patternMismatch,
            typeMismatch: false, stepMismatch: false, rangeUnderflow: false,
            rangeOverflow: false, badInput: false,
            customError: !!customValidity.get(el)
        };
        r.valid = !Object.keys(r).some(function (k) { return r[k]; });
        return r;
    }
    [HTMLInputElement, HTMLTextAreaElement].forEach(function (C) {
        var P = C.prototype;
        getter(P, 'selectionStart', function () { return selectionOf(this).start; });
        getter(P, 'selectionEnd', function () { return selectionOf(this).end; });
        getter(P, 'selectionDirection', function () { return selectionOf(this).dir; });
        P.setSelectionRange = function (start, end, dir) { setSelection(this, start, end, dir); };
        P.select = function () { setSelection(this, 0, this.value.length); };
        getter(P, 'validity', function () { return validityOf(this); });
        getter(P, 'willValidate', function () { return !this.disabled && !this.readOnly; });
        getter(P, 'validationMessage', function () { return customValidity.get(this) || ''; });
        P.setCustomValidity = function (msg) { customValidity.set(this, String(msg)); };
        P.checkValidity = function () {
            if (!this.willValidate || validityOf(this).valid) return true;
            this.dispatchEvent(new Event('invalid', { cancelable: true }));
            return false;
        };
        P.reportValidity = P.checkValidity;
        [['maxLength', 'maxlength'], ['minLength', 'minlength']].forEach(function (p) {
            accessor(P, p[0], function () {
                var n = parseInt(this.getAttribute(p[1]), 10);
                return n >= 0 ? n : -1;
            }, function (v) { this.setAttribute(p[1], String(v)); });
        });
    });
    // HTML §4.10.5.3.2: how many characters wide the field is; 20 unless
    // the attribute is a number above zero.
    // The attribute is read by the rules for parsing non-negative integers
    // (leading whitespace, an optional "+", then digits up to the first
    // other character) and is 20 unless that gives 1 to 2147483647; layout
    // reads it the same way (engine `input_size_attribute`). Setting the
    // property converts to unsigned long: 0 throws, and a value above
    // 2147483647 writes 20 (Chrome for Testing 148).
    accessor(HTMLInputElement.prototype, 'size', function () {
        var m = /^[\t\n\f\r ]*\+?([0-9]+)/.exec(this.getAttribute('size') || '');
        var n = m ? Number(m[1]) : 0;
        return n >= 1 && n <= 2147483647 ? n : 20;
    }, function (v) {
        v = v >>> 0;
        if (v === 0) {
            throw new g.DOMException("Failed to set the 'size' property on 'HTMLInputElement': The value provided is 0, which is an invalid size.", 'IndexSizeError');
        }
        this.setAttribute('size', String(v > 2147483647 ? 20 : v));
    });
    [['rows', 2], ['cols', 20]].forEach(function (p) {
        accessor(HTMLTextAreaElement.prototype, p[0], function () {
            var n = parseInt(this.getAttribute(p[0]), 10);
            return n > 0 ? n : p[1];
        }, function (v) { this.setAttribute(p[0], String(v)); });
    });
    getter(HTMLTextAreaElement.prototype, 'textLength', function () { return this.value.length; });
    getter(HTMLTextAreaElement.prototype, 'type', function () { return 'textarea'; });
    accessor(HTMLTextAreaElement.prototype, 'defaultValue', function () { return this.textContent; },
        function (v) { this.textContent = v; });
    accessor(HTMLInputElement.prototype, 'defaultValue', function () {
        return this.getAttribute('value') || '';
    }, function (v) { this.setAttribute('value', v); });
    [HTMLInputElement, HTMLTextAreaElement].forEach(function (C) {
        ['name', 'placeholder'].forEach(function (k) {
            accessor(C.prototype, k, function () { return this.getAttribute(k) || ''; },
                function (v) { this.setAttribute(k, v); });
        });
        [['disabled', 'disabled'], ['readOnly', 'readonly'], ['required', 'required']].forEach(function (p) {
            accessor(C.prototype, p[0], function () { return this.hasAttribute(p[1]); },
                function (v) { this.toggleAttribute(p[1], !!v); });
        });
        getter(C.prototype, 'form', function () { return this.closest('form'); });
    });
    // HTML §4.10.21 form reset, for the controls this rung models: each text
    // control drops its dirty value and shows its default again.
    function controls(form, tags) {
        return Array.prototype.filter.call(form.getElementsByTagName('*'), function (el) {
            return tags.test(el.localName);
        });
    }
    // The form's reset (HTML: "reset" a form): the cancelable `reset`
    // event, then the text controls. Whether it went ahead. The engine's
    // own resets (a reset button) call this, not the form's `reset`
    // property, which a control named `reset` shadows and a page can
    // replace.
    function resetForm(form) {
        // Trusted, also when script called reset() (Chrome 148).
        var e = make('Event', 'reset', { bubbles: true, cancelable: true });
        trust(e);
        if (!dispatchTo(form, e)) return false;
        controls(form, /^(input|textarea)$/).forEach(function (el) {
            var s = slotOf(el);
            if (s.gen === gen && (el.localName === 'textarea' || inputMode(el) === 'value')) {
                N.value(s.gen, s.id, null);
            }
        });
        return true;
    }
    HTMLFormElement.prototype.reset = function reset() { resetForm(this); };
    // Navigation from script is not wired; submission stays engine-side.
    HTMLFormElement.prototype.submit = function () {};
    // `action` is web_reflect.js's: the attribute resolved against the
    // document's base URL.
    [['name', 'name'], ['target', 'target']].forEach(function (p) {
        accessor(HTMLFormElement.prototype, p[0], function () { return this.getAttribute(p[1]) || ''; },
            function (v) { this.setAttribute(p[1], v); });
    });
    getter(HTMLFormElement.prototype, 'method', function () {
        return (this.getAttribute('method') || '').toLowerCase() === 'post' ? 'post' : 'get';
    });
    // A static snapshot, like the other collections here.
    getter(HTMLFormElement.prototype, 'elements', function () {
        var els = controls(this, /^(input|textarea|select|button)$/);
        var out = Object.create(HTMLCollection.prototype);
        els.forEach(function (el, i) { out[i] = el; });
        Object.defineProperty(out, 'length', { value: els.length });
        return out;
    });
    // focus()/blur() and document.activeElement are in node_apis.js.

    Element.prototype.getAttribute = function (name) {
        var s = slotOf(this); return N.attr(s.gen, s.id, String(name));
    };
    Element.prototype.hasAttribute = function (name) { return this.getAttribute(name) !== null; };
    // A write to an `<img>`'s `src` or `srcset` starts its load over
    // (E54): the marks script reads `complete` and `decode()` from are
    // updated at once, not on the next event.
    function imageWritten(el, name) {
        var n = String(name);
        if (n.length !== 3 && n.length !== 6) return;
        n = n.toLowerCase();
        if ((n === 'src' || n === 'srcset') && el.tagName === 'IMG' && __rk.imageWritten) {
            __rk.imageWritten(el);
        }
    }
    Element.prototype.setAttribute = function (name, value) {
        if (arguments.length < 2) {
            throw new TypeError("Failed to execute 'setAttribute' on 'Element': 2 arguments required.");
        }
        var n = String(name), c0 = n[0];
        // A name that begins with o, s or a (either case) may be one the steps after a write act on
        // (`on<type>`, `async`, `src`, `srcset`): those take the full write. Any other name is a plain write.
        if (c0 === 'o' || c0 === 's' || c0 === 'a' || c0 === 'O' || c0 === 'S' || c0 === 'A') {
            setData(this, 'setAttr', n, String(value), 'setAttribute');
            imageWritten(this, n);
        } else {
            putAttr(this, 'setAttr', n, String(value), 'setAttribute');
        }
    };
    Element.prototype.removeAttribute = function (name) {
        var n = String(name), c0 = n[0];
        if (c0 === 'o' || c0 === 's' || c0 === 'a' || c0 === 'O' || c0 === 'S' || c0 === 'A') {
            setData(this, 'removeAttr', n, null, 'removeAttribute');
            imageWritten(this, n);
        } else {
            putAttr(this, 'removeAttr', n, null, 'removeAttribute');
        }
    };
    Element.prototype.toggleAttribute = function (name, force) {
        var has = this.hasAttribute(name);
        var want = force === undefined ? !has : !!force;
        if (want && !has) this.setAttribute(name, '');
        if (!want && has) this.removeAttribute(name);
        return want;
    };
    Element.prototype.getAttributeNames = function () {
        var names = info(this, 'attrNames');
        return names ? names.split(' ') : [];
    };
    Element.prototype.hasAttributes = function () { return this.getAttributeNames().length > 0; };

    // DOM "insert adjacent" (insertAdjacentElement/Text) and HTML §8.5
    // insertAdjacentHTML. Answers the parent and the node to insert before.
    function adjacentSpot(el, where, method) {
        var w = String(where).toLowerCase();
        if (w === 'beforebegin') return [el.parentNode, el];
        if (w === 'afterbegin') return [el, el.firstChild];
        if (w === 'beforeend') return [el, null];
        if (w === 'afterend') return [el.parentNode, el.nextSibling];
        throw new DOMException("Failed to execute '" + method + "' on 'Element': The value provided ('" +
            where + "') is not one of 'beforeBegin', 'afterBegin', 'beforeEnd', or 'afterEnd'.",
            'SyntaxError');
    }
    Element.prototype.insertAdjacentElement = function (where, element) {
        if (!(element instanceof Element)) {
            throw new TypeError("Failed to execute 'insertAdjacentElement' on 'Element': " +
                "parameter 2 is not of type 'Element'.");
        }
        var spot = adjacentSpot(this, where, 'insertAdjacentElement');
        if (!spot[0]) return null;
        return spot[0].insertBefore(element, spot[1]);
    };
    Element.prototype.insertAdjacentText = function (where, data) {
        var t = g.document.createTextNode(String(data));
        var spot = adjacentSpot(this, where, 'insertAdjacentText');
        if (spot[0]) spot[0].insertBefore(t, spot[1]);
    };
    Element.prototype.insertAdjacentHTML = function (where, html) {
        var spot = adjacentSpot(this, where, 'insertAdjacentHTML');
        if (!spot[0] || spot[0].nodeType === 9) {
            throw new DOMException("Failed to execute 'insertAdjacentHTML' on 'Element': " +
                "The element has no parent.", 'NoModificationAllowedError');
        }
        // The fragment parses in the context of the element it lands in
        // (<html> parses as <body>), through a detached holder of that name.
        var ctx = spot[0].localName === 'html' ? 'body' : spot[0].localName;
        var holder = spot[0].namespaceURI === 'http://www.w3.org/2000/svg'
            ? document.createElementNS(spot[0].namespaceURI, ctx)
            : create('element', ctx, 'insertAdjacentHTML');
        // The holder is the target's document's, so the markup parses with
        // that document's scripting flag (Chrome for Testing 148).
        ownedBy(holder, spot[0].nodeType === 9 ? spot[0] : spot[0].ownerDocument);
        holder.innerHTML = String(html);
        Array.prototype.slice.call(holder.childNodes).forEach(function (n) {
            spot[0].insertBefore(n, spot[1]);
        });
    };

    // classList: a DOMTokenList over the class attribute, one per element.
    //
    // Every built-in used here is taken at install time (a page's own
    // `String.prototype.split`, `Array.prototype.indexOf`, `Element.prototype.
    // setAttribute` or `Reflect.get` cannot change what the list does), and the
    // class attribute is read and written through the engine's natives, not
    // through the element's JS methods. The token set is kept with the attribute
    // text it was made from: the same text is not parsed twice, and a token array
    // is never changed after it is stored (a change makes a new one), so one a
    // forEach or iterator holds stays a snapshot.
    var DOMTokenList = iface('DOMTokenList');
    var tlState = new WeakMap();
    var tlStr = String, tlSlice = uncurry(String.prototype.slice), tlCode = uncurry(String.prototype.charCodeAt);
    var tlTest = uncurry(RegExp.prototype.test), tlJoin = uncurry(Array.prototype.join), tlGet = Reflect.get;
    var tlCall = uncurry(Function.prototype.call), tlValues = uncurry(Array.prototype.values);
    var TL_SPACE = /[\t\n\f\r ]/, TL_INDEX = /^(0|[1-9][0-9]*)$/;
    function tlOf(list) {
        var st = mapGet(tlState, list);
        if (!st) throw new TypeError('Illegal invocation');
        return st;
    }
    function tlSpace(c) { return c === 32 || c === 9 || c === 10 || c === 12 || c === 13; }
    function tlIndexOf(ts, t) {
        for (var i = 0; i < ts.length; i++) if (ts[i] === t) return i;
        return -1;
    }
    function tlWithout(ts, at) {
        var out = [];
        for (var i = 0; i < ts.length; i++) if (i !== at) out[out.length] = ts[i];
        return out;
    }
    function tlWith(ts, t) {
        var out = [];
        for (var i = 0; i < ts.length; i++) out[i] = ts[i];
        out[out.length] = t;
        return out;
    }
    // The ordered set of tokens in `v`, and whether `v` already is the serialized
    // set (one space between tokens, none at the ends, no repeats).
    function tlParse(st, v) {
        var out = [], n = v.length, i = 0, clean = true;
        while (i < n) {
            if (tlSpace(tlCode(v, i))) {
                if (i === 0 || i === n - 1 || tlCode(v, i) !== 32 || tlSpace(tlCode(v, i + 1))) clean = false;
                i++;
                continue;
            }
            var j = i + 1;
            while (j < n && !tlSpace(tlCode(v, j))) j++;
            var t = tlSlice(v, i, j);
            if (tlIndexOf(out, t) < 0) out[out.length] = t; else clean = false;
            i = j;
        }
        st.ts = out;
        st.clean = clean;
        return out;
    }
    // The tokens now; `st.raw` is the class attribute's text (null: no attribute).
    function tokens(st) {
        var s = slotOf(st.el), v = N.attr(s.gen, s.id, 'class');
        if (st.ts !== null && v === st.raw) return st.ts;
        st.raw = v;
        return tlParse(st, v === null ? '' : v);
    }
    function checkToken(t, method) {
        t = tlStr(t);
        if (t === '') throw new DOMException("Failed to execute '" + method + "' on 'DOMTokenList': The token provided must not be empty.", 'SyntaxError');
        if (tlTest(TL_SPACE, t)) throw new DOMException("Failed to execute '" + method + "' on 'DOMTokenList': The token provided contains HTML space characters.", 'InvalidCharacterError');
        return t;
    }
    // The update steps: the class attribute becomes the serialized set. `text`
    // is that serialization when the caller has it already.
    // setData for the one attribute name `class`, without the steps that only act on another
    // name: `attributeChanged` (on<type> attributes), `HOOKS.scripts.changed` (`async`, `src`)
    // and `HOOKS.frames.changed` (`src`). It keeps the custom-element reaction (`HOOKS.attrs`). Same write, same mutation record, same errors.
    function putClass(el, text) {
        var s = slotOf(el);
        var m = HOOKS.mutation && HOOKS.mutation.data(el, 'setAttr', 'class');
        var ar = HOOKS.attrs && HOOKS.watched.class === true && HOOKS.attrs(el, 'setAttr', 'class', true);
        var r = N.write(gen, 'setAttr', s.gen === gen ? s.id : -1, 'class', text);
        if (typeof r === 'string') throw new DOMException("Failed to execute 'setAttribute'.", r);
        if (m) m();
        if (ar) ar();
    }
    function store(st, ts, text) {
        if (text === undefined) text = tlJoin(ts, ' ');
        putClass(st.el, text);
        st.raw = text; st.ts = ts; st.clean = true;
    }
    getter(DOMTokenList.prototype, 'length', function () { return tokens(tlOf(this)).length; });
    accessor(DOMTokenList.prototype, 'value', function () {
            var st = tlOf(this), s = slotOf(st.el), v = N.attr(s.gen, s.id, 'class');
            return v === null ? '' : v;
        },
        function (v) { putClass(tlOf(this).el, tlStr(v)); });
    DOMTokenList.prototype.toString = function () { return this.value; };
    DOMTokenList.prototype.item = function (i) { var ts = tokens(tlOf(this)); i = i >>> 0; return i < ts.length ? ts[i] : null; };
    DOMTokenList.prototype.contains = function (t) { return tlIndexOf(tokens(tlOf(this)), tlStr(t)) >= 0; };
    DOMTokenList.prototype.add = function () {
        var st = tlOf(this), n = arguments.length, args = [];
        for (var k = 0; k < n; k++) args[k] = checkToken(arguments[k], 'add');
        var ts = tokens(st), raw = st.raw, clean = st.clean, grown = false;
        for (var m = 0; m < n; m++) {
            if (tlIndexOf(ts, args[m]) < 0) { ts = tlWith(ts, args[m]); grown = true; }
        }
        // A token already there still rewrites the attribute (DOM's update steps
        // run, so the text is normalized and an observer gets a record).
        if (n === 1 && grown && clean) store(st, ts, raw === null || raw === '' ? args[0] : raw + ' ' + args[0]);
        else if (!grown && clean && raw !== null) store(st, ts, raw);
        else store(st, ts);
    };
    DOMTokenList.prototype.remove = function () {
        var st = tlOf(this), n = arguments.length, args = [];
        for (var k = 0; k < n; k++) args[k] = checkToken(arguments[k], 'remove');
        var ts = tokens(st), kept = ts;
        for (var m = 0; m < n; m++) {
            var at = tlIndexOf(kept, args[m]);
            if (at >= 0) kept = tlWithout(kept, at);
        }
        // DOM's update steps skip writing a class attribute that isn't there.
        if (st.raw === null) return;
        if (kept === ts && st.clean) store(st, ts, st.raw);
        else store(st, kept);
    };
    DOMTokenList.prototype.toggle = function (t, force) {
        t = checkToken(t, 'toggle');
        var st = tlOf(this), ts = tokens(st), i = tlIndexOf(ts, t);
        if (i >= 0 && force !== true) { store(st, tlWithout(ts, i)); return false; }
        if (i < 0 && force !== false) {
            store(st, tlWith(ts, t), st.clean && (st.raw === null || st.raw === '') ? t : st.clean ? st.raw + ' ' + t : undefined);
            return true;
        }
        return i >= 0;
    };
    DOMTokenList.prototype.replace = function (a, b) {
        a = checkToken(a, 'replace'); b = checkToken(b, 'replace');
        var st = tlOf(this), ts = tokens(st), i = tlIndexOf(ts, a);
        if (i < 0) return false;
        // The new token takes the place of whichever of the two came first; the other goes (DOM replace).
        var j = tlIndexOf(ts, b), out = [], first = j >= 0 && j < i ? j : i;
        for (var k = 0; k < ts.length; k++) {
            if (k === first) out[out.length] = b; else if (k !== i && k !== j) out[out.length] = ts[k];
        }
        store(st, out);
        return true;
    };
    // The callback gets (token, index, the list), as Chrome's does, never the kept array,
    // and the set is read again at each step (a callback that removes a class changes what
    // follows, Chrome 148).
    DOMTokenList.prototype.forEach = function (cb, self) {
        var st = tlOf(this), ts = tokens(st);
        if (typeof cb !== 'function') throw new TypeError("Failed to execute 'forEach' on 'DOMTokenList': parameter 1 is not of type 'Function'.");
        for (var i = 0; i < ts.length; i++) {
            tlCall(cb, self, ts[i], i, this);
            ts = tokens(st);
        }
    };
    // The iterator walks the cached token array itself. Safe only while that array is never
    // mutated after it is stored: every update builds a new array (tlWith/tlWithout/tlParse).
    DOMTokenList.prototype[Symbol.iterator] = function () { return tlValues(tokens(tlOf(this))); };
    var classLists = new WeakMap();
    getter(Element.prototype, 'classList', function () {
        slotOf(this);
        var l = mapGet(classLists, this);
        if (!l) {
            var st = { el: this, raw: null, ts: null, clean: true };
            // The indexed getter (`classList[0]`) answers like item(). A property
            // name that does not start with a digit cannot be an index.
            l = new Proxy(Object.create(DOMTokenList.prototype), {
                get: function (t, p, r) {
                    if (typeof p === 'string') {
                        var c = tlCode(p, 0);
                        if (c >= 48 && c <= 57 && tlTest(TL_INDEX, p)) return tokens(st)[+p];
                    }
                    return tlGet(t, p, r);
                }
            });
            mapSet(tlState, l, st);
            mapSet(classLists, this, l);
        }
        return l;
    });

    // Layout geometry (CSSOM View). The engine publishes where each element's
    // box is after a layout (`set_geometry_source`, built on the first read);
    // a read answers from the last
    // layout, so it can be stale between a script's own DOM writes and the
    // next layout. An element with no box (display:none, not rendered, or
    // measured before any layout) reads as all zeros, and has no offsetParent.
    function finite(v) { v = Number(v); return isFinite(v) ? v : 0; }
    function geom(el, why) {
        var raw = info(el, why ? 'geom:' + why : 'geom');
        if (!raw) return null;
        var p = raw.split(' ');
        var n = [];
        for (var i = 0; i < p.length; i++) n.push(finite(p[i]));
        return { x: n[0], y: n[1], w: n[2], h: n[3], bl: n[4], bt: n[5], cw: n[6], ch: n[7], sw: n[8], sh: n[9], pos: n[10], lx: n[11], ly: n[12], rw: n[13], rh: n[14] };
    }
    // What web_scroll.js scrolls an element with; it takes this and deletes
    // it. read: null when the element cannot scroll now.
    g.__rustkit_element_scroll = {
        read: function (el) {
            var raw = info(el, 'scroll');
            if (!raw) return null;
            var p = raw.split(' ');
            return { x: finite(p[0]), y: finite(p[1]) };
        },
        write: function (el, x, y) { var s = slotOf(el); N.scrollElement(s.gen, s.id, x, y); },
        chain: function (el) {
            var raw = info(el, 'scrollChain');
            if (!raw) return null;
            var p = raw.split(' ');
            return { window: p[0] === 'w', scrollers: p.slice(1).map(function (id) { return wrap(+id); }) };
        },
        byId: function (id) { return wrap(id); }
    };
    // What web_animations.js drives the engine's animated-style layer
    // with, kept on the host object. write: the element's entries,
    // `[[property, value, composite], ...]`, or null for none. css: the
    // engine's CSS animations and transitions as of the last layout.
    // control: pause/play/cancel/finish one of them.
    __rk.animatedStyle = {
        write: function (el, entries) {
            var s = slotOf(el);
            N.animate(s.gen, s.id, entries ? JSON.stringify(entries) : null);
        },
        css: function () { return JSON.parse(N.cssAnimations()); },
        control: function (el, kind, id, op) { var s = slotOf(el); N.cssControl(s.gen, s.id, kind, id, op); },
        byId: function (id) { return wrap(id); }
    };
    function scrollOffset(axis) {
        var v = axis === 'x' ? (g.scrollX || g.pageXOffset) : (g.scrollY || g.pageYOffset);
        return finite(typeof v === 'number' ? v : 0);
    }
    // DOMRect is installed after this layer (web_interfaces.js), so it is
    // looked up when a rect is made. Edges are finite: a NaN here is what
    // `Number.parseInt(getComputedStyle(e).borderRadius) / 2` used to hand
    // to elementFromPoint.
    function rect(x, y, w, h) {
        x = finite(x); y = finite(y); w = finite(w); h = finite(h);
        return typeof g.DOMRect === 'function'
            ? new g.DOMRect(x, y, w, h)
            : { x: x, y: y, width: w, height: h, top: y, left: x, right: x + w, bottom: y + h };
    }
    Element.prototype.getBoundingClientRect = function () {
        var b = geom(this);
        if (!b) return rect(0, 0, 0, 0);
        return rect(b.x - scrollOffset('x'), b.y - scrollOffset('y'), b.rw, b.rh);
    };
    Element.prototype.getClientRects = function () {
        var b = geom(this);
        var out = b ? [rect(b.x - scrollOffset('x'), b.y - scrollOffset('y'), b.rw, b.rh)] : [];
        return out;
    };
    function metric(name, pick) {
        getter(HTMLElement.prototype, name, function () {
            var b = geom(this, 'offset');
            return b ? pick(b, this) : 0;
        });
    }
    function round(v) { return Math.round(v); }
    metric('offsetWidth', function (b) { return round(b.w); });
    metric('offsetHeight', function (b) { return round(b.h); });
    // offsetParent: the nearest positioned ancestor with a box, else body
    // (CSSOM View). A static element also stops at the nearest `td`, `th`
    // or `table` with a box: by element name, whatever its display, as
    // Chromium does. null when the element has no box, is fixed, or is the
    // body itself.
    var TABLE_PARENTS = { TD: 1, TH: 1, TABLE: 1 };
    function offsetParentOf(el) {
        var b = geom(el, 'offset');
        if (!b || b.pos === 2) return null;
        var body = document.body;
        if (el === body || el === document.documentElement) return null;
        var isStatic = b.pos === 0;
        for (var p = el.parentElement; p; p = p.parentElement) {
            if (p === body) return body;
            var pb = geom(p, 'offset');
            if (!pb) continue;
            if (pb.pos !== 0) return p;
            if (isStatic && p.namespaceURI === 'http://www.w3.org/1999/xhtml' && TABLE_PARENTS[p.tagName]) return p;
        }
        return body;
    }
    getter(HTMLElement.prototype, 'offsetParent', function () { return offsetParentOf(this); });
    // Offsets are from the offsetParent's padding edge; from the document
    // origin when that is the body.
    function offsetFrom(el, axis) {
        var b = geom(el, 'offset');
        if (!b) return 0;
        var parent = offsetParentOf(el);
        var base = 0;
        if (parent && parent !== document.body) {
            var pb = geom(parent, 'offset');
            if (pb) base = axis === 'x' ? pb.lx + pb.bl : pb.ly + pb.bt;
        }
        return round((axis === 'x' ? b.lx : b.ly) - base);
    }
    getter(HTMLElement.prototype, 'offsetLeft', function () { return offsetFrom(this, 'x'); });
    getter(HTMLElement.prototype, 'offsetTop', function () { return offsetFrom(this, 'y'); });
    // getComputedStyle. The engine publishes each styled element's computed
    // style (`set_computed_styles`), with or without a box; a read answers
    // from the last layout, with the element's own inline declarations laid
    // over it so that `el.style.display = 'none'` is seen at once. An
    // element with no published style after a layout was not styled (a
    // descendant of display:none, or <head>, <script>, <style>, ...): its
    // display reads 'none'. Before any layout everything reads as the empty
    // string. Custom properties come as `custom` rows on the nearest
    // element (self or ancestor) whose set is its own; `customVars` walks up.
    var COMPUTED_NAMES = {};
    // What a property reads when the engine published no row for it: Chromium's
    // serialized initial value (`computed_read.rs` publishes only what differs).
    var COMPUTED_INITIAL = {
        '-webkit-box-orient': 'horizontal', '-webkit-clip-path': 'none', '-webkit-filter': 'none',
        '-webkit-line-clamp': 'none', '-webkit-mask': 'none', '-webkit-mask-clip': 'border-box',
        '-webkit-mask-image': 'none', '-webkit-mask-origin': 'border-box',
        '-webkit-mask-position': '0% 0%', '-webkit-mask-repeat': 'repeat',
        '-webkit-mask-size': 'auto', '-webkit-text-fill-color': 'rgb(0, 0, 0)',
        'align-content': 'normal', 'align-items': 'normal', 'align-self': 'auto',
        'animation': 'none 0s ease 0s 1 normal none running', 'animation-delay': '0s',
        'animation-direction': 'normal', 'animation-duration': '0s', 'animation-fill-mode': 'none',
        'animation-iteration-count': '1', 'animation-name': 'none',
        'animation-play-state': 'running', 'animation-timing-function': 'ease',
        'aspect-ratio': 'auto', 'backdrop-filter': 'none',
        'background': 'rgba(0, 0, 0, 0) none repeat scroll 0% 0% / auto padding-box border-box',
        'background-clip': 'border-box', 'background-image': 'none',
        'background-origin': 'padding-box', 'background-position': '0% 0%',
        'background-position-x': '0%', 'background-position-y': '0%',
        'background-repeat': 'repeat', 'background-size': 'auto',
        'border': '0px none rgb(0, 0, 0)', 'border-block': '0px none rgb(0, 0, 0)',
        'border-block-end': '0px none rgb(0, 0, 0)', 'border-block-start': '0px none rgb(0, 0, 0)',
        'border-bottom': '0px none rgb(0, 0, 0)', 'border-bottom-color': 'rgb(0, 0, 0)',
        'border-bottom-style': 'none', 'border-collapse': 'separate',
        'border-color': 'rgb(0, 0, 0)', 'border-end-end-radius': '0px',
        'border-end-start-radius': '0px', 'border-inline': '0px none rgb(0, 0, 0)',
        'border-inline-end': '0px none rgb(0, 0, 0)',
        'border-inline-start': '0px none rgb(0, 0, 0)', 'border-left': '0px none rgb(0, 0, 0)',
        'border-left-color': 'rgb(0, 0, 0)', 'border-left-style': 'none',
        'border-right': '0px none rgb(0, 0, 0)', 'border-right-color': 'rgb(0, 0, 0)',
        'border-right-style': 'none', 'border-spacing': '0px', 'border-start-end-radius': '0px',
        'border-start-start-radius': '0px', 'border-style': 'none',
        'border-top': '0px none rgb(0, 0, 0)', 'border-top-color': 'rgb(0, 0, 0)',
        'border-top-style': 'none', 'border-width': '0px', 'box-shadow': 'none', 'clear': 'none',
        'clip-path': 'none', 'column-count': 'auto', 'column-gap': 'normal', 'container': 'none',
        'container-name': 'none', 'container-type': 'normal', 'cursor': 'auto', 'filter': 'none', 'flex': '0 1 auto',
        'flex-basis': 'auto', 'flex-flow': 'row nowrap', 'flex-grow': '0', 'flex-shrink': '1',
        'flex-wrap': 'nowrap', 'font': '16px Times', 'gap': 'normal', 'grid-area': 'auto',
        'grid-auto-columns': 'auto', 'grid-auto-flow': 'row', 'grid-auto-rows': 'auto',
        'grid-column': 'auto', 'grid-column-end': 'auto', 'grid-column-start': 'auto',
        'grid-row': 'auto', 'grid-row-end': 'auto', 'grid-row-start': 'auto',
        'grid-template': 'none', 'grid-template-areas': 'none', 'grid-template-columns': 'none',
        'grid-template-rows': 'none', 'inset': 'auto', 'inset-block': 'auto',
        'inset-block-end': 'auto', 'inset-block-start': 'auto', 'inset-inline': 'auto',
        'inset-inline-end': 'auto', 'inset-inline-start': 'auto', 'justify-content': 'normal',
        'justify-items': 'normal', 'justify-self': 'auto', 'line-break': 'auto', 'margin': '0px',
        'margin-block': '0px', 'margin-block-end': '0px', 'margin-block-start': '0px',
        'margin-inline': '0px', 'margin-inline-end': '0px', 'margin-inline-start': '0px',
        'mask': 'none', 'mask-clip': 'border-box', 'mask-image': 'none',
        'mask-origin': 'border-box', 'mask-position': '0% 0%', 'mask-repeat': 'repeat',
        'mask-size': 'auto', 'max-height': 'none', 'max-width': 'none', 'min-height': '0px',
        'min-width': '0px', 'object-fit': 'fill', 'object-position': '50% 50%', 'order': '0',
        'overflow': 'visible', 'overflow-wrap': 'normal', 'padding': '0px', 'padding-block': '0px',
        'padding-block-end': '0px', 'padding-block-start': '0px', 'padding-inline': '0px',
        'padding-inline-end': '0px', 'padding-inline-start': '0px', 'place-content': 'normal',
        'place-items': 'normal', 'place-self': 'auto', 'rotate': 'none', 'row-gap': 'normal',
        'scale': 'none', 'text-decoration': 'none', 'text-decoration-color': 'rgb(0, 0, 0)',
        'text-decoration-line': 'none', 'text-decoration-style': 'solid', 'text-overflow': 'clip',
        'transform': 'none', 'transition': 'all', 'transition-delay': '0s',
        'transition-duration': '0s', 'transition-property': 'all',
        'transition-timing-function': 'ease', 'translate': 'none', 'vertical-align': 'baseline',
        'word-break': 'normal', 'word-spacing': '0px', 'word-wrap': 'normal'
    };
    // `hasOwn` here is the install-time helper declared above, with the event records.
    // Names a read answers but an enumeration leaves out, as Chromium does:
    // shorthands (still readable by name).
    var COMPUTED_SHORTHANDS = {};
    ('animation background border border-block border-block-end border-block-start border-bottom ' +
     'border-color border-inline border-inline-end border-inline-start border-left border-radius ' +
     'border-right border-style border-top border-width container flex flex-flow font gap grid-area ' +
     'grid-column grid-row grid-template inset inset-block inset-inline margin margin-block ' +
     'margin-inline mask -webkit-mask outline overflow padding padding-block padding-inline place-content ' +
     'place-items place-self text-decoration transition').split(' ').forEach(function (n) {
        COMPUTED_SHORTHANDS[n] = true;
    });
    // `-webkit-` names Chromium answers as the unprefixed property.
    var WEBKIT_ALIASES = {};
    ('transform transform-origin transition transition-property transition-duration ' +
     'transition-delay transition-timing-function animation animation-name animation-duration ' +
     'animation-delay animation-timing-function animation-iteration-count animation-direction ' +
     'animation-fill-mode animation-play-state box-shadow box-sizing flex flex-grow flex-shrink ' +
     'flex-basis flex-direction flex-wrap flex-flow order justify-content align-items align-self ' +
     'align-content border-radius border-top-left-radius border-top-right-radius ' +
     'border-bottom-right-radius border-bottom-left-radius opacity').split(' ').forEach(function (n) {
        WEBKIT_ALIASES['-webkit-' + n] = n;
    });
    // `m.__fill` says whether the initial values stand behind the rows:
    // looked up on read, never copied (`true`, or `'pseudo'`, which leaves
    // out what a box that was never laid out cannot answer).
    function initialOf(m, n) {
        var fill = m.__fill;
        if (!fill || !hasOwn(COMPUTED_INITIAL, n)) return undefined;
        if (fill === 'pseudo' && /^(margin|padding|inset|transform-origin)/.test(n)) return undefined;
        return COMPUTED_INITIAL[n];
    }
    function computedValue(m, n) {
        if (hasOwn(m, n)) return m[n];
        var v = initialOf(m, n);
        if (v === undefined && hasOwn(WEBKIT_ALIASES, n)) return computedValue(m, WEBKIT_ALIASES[n]);
        return v;
    }
    function markFill(m, how) {
        Object.defineProperty(m, '__fill', { value: how, enumerable: false });
    }
    // Inline text the engine is known to drop or to keep only in part: the
    // published row (or the initial value) would be a confident wrong answer,
    // where the text is what the page wrote. Listed in #866.
    function lossyInline(name, v) {
        var t = v.toLowerCase().trim();
        var listed = function () {
            var depth = 0;
            for (var i = 0; i < t.length; i++) {
                var c = t.charAt(i);
                if (c === '(') depth++;
                else if (c === ')') depth--;
                else if (c === ',' && depth === 0) return true;
            }
            return false;
        };
        if (/^(transition|animation)/.test(name) && name !== 'transition-property') return listed();
        if (name === 'box-shadow') return listed();
        if (name === 'transform') {
            return /(3d|rotate[xyz]|translatez|scalez|perspective)\s*\(/.test(t) || /[0-9.](vw|vh|vmin|vmax)\b/.test(t);
        }
        if (name === 'text-decoration') {
            return t.split(/\s+/).some(function (w) { return !/^(none|underline|overline|line-through)$/.test(w); });
        }
        if (name === 'border-color' || name === 'overflow') return /\s/.test(t.replace(/\([^)]*\)/g, ''));
        if (name === 'vertical-align') return /[0-9]/.test(t);
        if (name === 'aspect-ratio') return t.indexOf('/') < 0 && t.indexOf('.') >= 0;
        return name === 'flex-flow' || name === 'background-position-x' || name === 'background-position-y';
    }
    var LOSSY_LONGHANDS = {
        'transition': ['transition-property', 'transition-duration', 'transition-delay', 'transition-timing-function'],
        'animation': ['animation-name', 'animation-duration', 'animation-delay', 'animation-timing-function',
            'animation-iteration-count', 'animation-direction', 'animation-fill-mode', 'animation-play-state'],
        'border-color': ['border-top-color', 'border-right-color', 'border-bottom-color', 'border-left-color'],
        'text-decoration': ['text-decoration-line', 'text-decoration-style', 'text-decoration-color']
    };
    var REPLACED = { IMG: 1, VIDEO: 1, CANVAS: 1, IFRAME: 1, INPUT: 1, SELECT: 1, TEXTAREA: 1,
        BUTTON: 1, OBJECT: 1, EMBED: 1, SVG: 1 };
    // Where layout does not apply, `width`/`height` are the declared values
    // (Chromium's resolved value is the used one only for a laid-out box).
    function declaredBox(el, m, name) {
        if (name !== 'width' && name !== 'height') return false;
        if (m.display === 'none') return true;
        return m.display === 'inline' && !hasOwn(REPLACED, String(el.tagName || '').toUpperCase());
    }
    // Memoized: a page reads the same few dozen names over and over.
    var KEBAB = Object.create(null);
    function kebab(p) {
        var k = KEBAB[p];
        if (k === undefined) {
            k = KEBAB[p] = p.replace(/[A-Z]/g, function (c) { return '-' + c.toLowerCase(); });
        }
        return k;
    }
    function customVars(el) {
        for (var e = el; e; e = e.parentElement) {
            var raw = info(e, 'computed');
            if (!raw || raw.indexOf('\ncustom\t') < 0) continue;
            var vars = {};
            raw.split('\n').forEach(function (line) {
                if (line.indexOf('custom\t') !== 0) return;
                var rest = line.slice(7), j = rest.indexOf('\t');
                if (j > 0) vars[rest.slice(0, j)] = rest.slice(j + 1);
            });
            return vars;
        }
        return null;
    }
    function computedMap(el, pseudo) {
        var raw = info(el, 'computed');
        var m = {}, own = null, prefix = pseudo ? pseudo + ' ' : null;
        if (raw) {
            raw.split('\n').forEach(function (line) {
                var i = line.indexOf('\t');
                if (i <= 0) return;
                var name = line.slice(0, i);
                if (name === 'custom') return;
                // The element's rows, then its pseudo-elements' (`::before name`).
                if (name.charAt(0) !== ':') m[name] = line.slice(i + 1);
                else if (prefix && name.indexOf(prefix) === 0) {
                    var key = name.slice(prefix.length), val = line.slice(i + 1);
                    own = own || {};
                    if (key === 'custom') {
                        // The pseudo's own custom properties (`--c\tvalue`):
                        // only a pseudo that declares some publishes them.
                        var j = val.indexOf('\t');
                        if (j > 0) {
                            if (!own.__vars) Object.defineProperty(own, '__vars', { value: {}, enumerable: false });
                            own.__vars[val.slice(0, j)] = val.slice(j + 1);
                        }
                    } else own[key] = val;
                }
            });
            markFill(m, true);
        } else if (info(el, 'laidout')) {
            m.display = 'none';
            m.visibility = 'visible';
        }
        // Unset radii are 0px, the way Chrome serialises the computed
        // initial value. An empty string makes Number.parseInt return NaN.
        if (raw || info(el, 'laidout')) {
            ['border-radius', 'border-top-left-radius', 'border-top-right-radius',
             'border-bottom-right-radius', 'border-bottom-left-radius'].forEach(function (n) {
                if (!Object.prototype.hasOwnProperty.call(m, n)) m[n] = '0px';
            });
        }
        // A layout that has seen the attribute already answers for what it
        // publishes: `transform: rotate(10deg)` reads a matrix, `width: 50%`
        // pixels. The text stands in where the engine published no row (the
        // initial values stand behind the rows, they are not rows), where the
        // row is empty, where the engine is known to drop part of the value,
        // where layout does not apply, and when the write has not been laid
        // out (the forced-layout budget). `display: none` has no transform.
        // A declaration `el.style` leaves out (a bad url, CH2.E29) is not
        // here at all, so the cascaded value shows. No row cannot be read as
        // "initial" in general: the engine stores `justify-content:
        // flex-start` and `align-items: stretch` as its defaults
        // and publishes no row for them, where Chromium reads the value.
        var inline = el.style;
        if (inline) {
            var stale = !!info(el, 'stale');
            var slot = inline.length ? slotOf(el) : null;
            for (var i = 0; i < inline.length; i++) {
                var name = inline.item(i);
                var v = inline.getPropertyValue(name);
                if (!v) continue;
                // An animated property (Web Animations' layer, a CSS
                // animation or transition) reads the engine's row: the
                // animation is above the inline text in the cascade.
                if (slot && N.animated(slot.gen, slot.id, name)) continue;
                if (name === 'opacity') {
                    // #861: a plain value is seen at once, as a number in 0..1.
                    // What needs the cascade (`calc()`, `var()`, `inherit`) or
                    // is invalid reads the last layout's row.
                    if (/^[+-]?(\d*\.)?\d+(e[+-]?\d+)?%?$/i.test(v)) {
                        var n = parseFloat(v) / (v.charAt(v.length - 1) === '%' ? 100 : 1);
                        m[name] = String(Math.min(1, Math.max(0, n)));
                    }
                    continue;
                }
                if (name === 'transform' && m.display === 'none' && !stale) continue;
                var lossy = !stale && lossyInline(name, v);
                if (stale || lossy || !hasOwn(m, name) || m[name] === '' || declaredBox(el, m, name)) {
                    m[name] = v;
                }
                // The longhands of a shorthand the engine dropped part of are
                // unknown too: they read '' (as before #866), not a guess.
                if (lossy && hasOwn(LOSSY_LONGHANDS, name)) {
                    LOSSY_LONGHANDS[name].forEach(function (n) { if (!inline.getPropertyValue(n)) m[n] = ''; });
                }
            }
        }
        return pseudo ? pseudoMap(m, own, pseudo) : m;
    }
    // What a pseudo-element reads: its own cascaded rows, or, with no rule
    // that set `content`, the initial values and what it inherits.
    var INHERITED = ['color', 'font-family', 'font-size', 'font-style', 'font-weight', 'letter-spacing',
        'line-height', 'text-align', 'text-transform', 'visibility', 'white-space'];
    function pseudoMap(m, own, pseudo) {
        var isBox = pseudo === '::before' || pseudo === '::after';
        if (own) {
            if (own.content === 'normal') own.content = 'none';
            markFill(own, 'pseudo');
            return own;
        }
        if (!m.display) return {};
        var out = { display: 'inline', position: 'static', width: 'auto', height: 'auto',
            content: isBox ? 'none' : 'normal' };
        INHERITED.forEach(function (n) { if (m[n]) out[n] = m[n]; });
        return out;
    }
    // The canonical name of getComputedStyle's second argument, '' for the
    // element itself, or null for one that names no pseudo-element.
    function pseudoArgument(p) {
        if (p === undefined || p === null) return '';
        p = String(p).toLowerCase();
        if (p === '') return '';
        var named = /^(?:::?)?(before|after|first-line|first-letter)$/.exec(p);
        if (named) return '::' + named[1];
        return /^::(marker|selection|backdrop|placeholder)$/.test(p) ? p : null;
    }
    function makeComputed(el, pseudo) {
        var m = pseudo === null ? {} : computedMap(el, pseudo);
        // Listed on demand: a read of one property should not pay for the
        // list. Longhands with a value, `-webkit-` names last, as Chromium.
        // Built once per object. Only a published value can be empty; a name
        // from the initial table never is, so it is not looked at.
        var sorted = null;
        function names() {
            if (sorted) return sorted;
            var seen = Object.create(null), list = [];
            var add = function (n) {
                if (seen[n] || hasOwn(COMPUTED_SHORTHANDS, n)) return;
                seen[n] = true;
                list.push(n);
            };
            for (var n in m) {
                if (hasOwn(m, n) && m[n] !== '') add(n);
                else if (hasOwn(m, n)) seen[n] = true;
            }
            if (m.__fill) {
                for (var k in COMPUTED_INITIAL) {
                    if (!(k in seen) && initialOf(m, k) !== undefined) add(k);
                }
            }
            var webkit = [], plain = [];
            list.forEach(function (x) { (x.charCodeAt(0) === 45 ? webkit : plain).push(x); });
            sorted = plain.sort().concat(webkit.sort());
            return sorted;
        }
        var cache = Object.create(null);
        function read(n) {
            var v = cache[n];
            if (v !== undefined) return v;
            v = computedValue(m, n);
            if (v === undefined) return customValue(n);
            return (cache[n] = v);
        }
        var decl = Object.create(CSSStyleDeclaration.prototype);
        function ro() { throw new DOMException('These styles are computed, and therefore read-only.', 'NoModificationAllowedError'); }
        // A custom property not set inline: the cascaded (inherited) value.
        function customValue(n) {
            if (pseudo === null || n.indexOf('--') !== 0) return '';
            var vars = m.__vars || customVars(el);
            return vars && Object.prototype.hasOwnProperty.call(vars, n) ? vars[n] : '';
        }
        var own = {
            // A custom property name is case-sensitive (Primer's
            // `--fgColor-*`): only a standard property is kebab-cased.
            getPropertyValue: function (n) { n = String(n); if (n.indexOf('--') !== 0) n = kebab(n); return read(n); },
            getPropertyPriority: function () { return ''; },
            item: function (i) { return names()[i >>> 0] || ''; },
            setProperty: ro, removeProperty: ro,
            parentRule: null
        };
        Object.keys(own).forEach(function (k) {
            Object.defineProperty(decl, k, { value: own[k], writable: false, configurable: true, enumerable: false });
        });
        Object.defineProperty(decl, 'cssText', { configurable: true, enumerable: false,
            get: function () { return names().map(function (n) { return n + ': ' + read(n) + ';'; }).join(' '); } });
        Object.defineProperty(decl, 'length', { configurable: true, enumerable: false,
            get: function () { return names().length; } });
        return new Proxy(decl, {
            get: function (t, p, r) {
                if (typeof p !== 'string') return Reflect.get(t, p, r);
                var c0 = p.charCodeAt(0);
                if (c0 >= 48 && c0 <= 57 && /^(0|[1-9][0-9]*)$/.test(p)) return names()[+p];
                if (p in t) return Reflect.get(t, p, r);
                var n = p.indexOf('--') === 0 ? p : kebab(p).replace(/^webkit-/, '-webkit-');
                return p === 'cssFloat' ? (m['float'] || '') : read(n);
            },
            set: function (t, p) { if (typeof p === 'string' && !(p in t)) ro(); return false; }
        });
    }
    g.getComputedStyle = function getComputedStyle(el, pseudo) {
        if (!el || el.nodeType !== 1) {
            throw new TypeError("Failed to execute 'getComputedStyle' on 'Window': parameter 1 is not of type 'Element'.");
        }
        return makeComputed(el, pseudoArgument(pseudo));
    };

    // The root element's client size is the viewport's, and its scroll size
    // is the document's (the body's bottom/right edge, at least the viewport).
    function isRoot(el) { return el === document.documentElement; }
    function viewport(axis) {
        var v = axis === 'x' ? g.innerWidth : g.innerHeight;
        return typeof v === 'number' ? v : 0;
    }
    function documentExtent(axis) {
        var body = document.body, b = body ? geom(body) : null;
        var edge = b ? (axis === 'x' ? b.x + b.w + b.sw - b.cw : b.y + b.h + b.sh - b.ch) : 0;
        return Math.max(viewport(axis), edge);
    }
    getter(Element.prototype, 'clientWidth', function () {
        if (isRoot(this)) return round(viewport('x'));
        var b = geom(this, 'client'); return b ? round(b.cw) : 0;
    });
    getter(Element.prototype, 'clientHeight', function () {
        if (isRoot(this)) return round(viewport('y'));
        var b = geom(this, 'client'); return b ? round(b.ch) : 0;
    });
    getter(Element.prototype, 'clientLeft', function () { var b = geom(this, 'client'); return b ? round(b.bl) : 0; });
    getter(Element.prototype, 'clientTop', function () { var b = geom(this, 'client'); return b ? round(b.bt) : 0; });
    getter(Element.prototype, 'scrollWidth', function () {
        if (isRoot(this)) return round(documentExtent('x'));
        var b = geom(this, 'scroll_size'); return b ? round(Math.max(b.sw, b.cw)) : 0;
    });
    getter(Element.prototype, 'scrollHeight', function () {
        if (isRoot(this)) return round(documentExtent('y'));
        var b = geom(this, 'scroll_size'); return b ? round(Math.max(b.sh, b.ch)) : 0;
    });

    // style: a CSSStyleDeclaration over the style attribute, one per
    // element. Reads parse the attribute; writes reserialize it, so the
    // cascade sees them through the attribute (and the write marks Style).
    var CSSStyleDeclaration = iface('CSSStyleDeclaration');
    var OWNER = Symbol('rustkit.styleOwner');
    // The attribute is a declaration list (CH2.E27): only a top-level `;`
    // ends a declaration, not one inside a string, a ()/[]/{} block, an
    // unquoted url( token (one token to its first unescaped `)`, with no
    // string, comment or block inside: `url(a/*b)`, `url(it's)`) or a
    // comment; `\;` is text. Comments drop out. At the end a custom property
    // keeps its text as written; any other declaration gets its open url,
    // string and blocks closed. rustkit_cssparser::split_declaration_list is
    // the Rust copy of this rule; both run declaration_list_cases.json.
    function splitDecls(text) {
        var out = [], cur = '', closers = [], quote = null, openUrl = false, i = 0, c, j;
        var pairs = objCreate(null);
        pairs['('] = ')'; pairs['['] = ']'; pairs['{'] = '}';
        while (i < text.length) {
            c = text.charAt(i++);
            if (c === '\\') { cur += c + text.charAt(i++); continue; }
            if (quote) { if (c === quote) quote = null; cur += c; continue; }
            if (c === '/' && text.charAt(i) === '*') {
                var end = text.indexOf('*/', i + 1);
                i = end < 0 ? text.length : end + 2;
                cur += ' ';
                continue;
            }
            if (c === '(' && /(^|[^\w\-\\\u0080-￿])url$/i.test(cur)) {
                cur += c;
                for (j = i; j < text.length && /\s/.test(text.charAt(j)); j++) {}
                if (text.charAt(j) === '"' || text.charAt(j) === "'") { closers.push(')'); continue; }
                openUrl = true;
                while (i < text.length) {
                    c = text.charAt(i++);
                    cur += c;
                    if (c === '\\') cur += text.charAt(i++);
                    else if (c === ')') { openUrl = false; break; }
                }
                continue;
            }
            if (c === '"' || c === "'") quote = c;
            else if (pairs[c]) closers.push(pairs[c]);
            else if (closers.length && c === closers[closers.length - 1]) closers.pop();
            else if (c === ';' && !closers.length) { out.push(cur); cur = ''; continue; }
            cur += c;
        }
        if (!/^\s*--/.test(cur)) {
            if (openUrl) cur += ')';
            if (quote) cur += quote;
            while (closers.length) cur += closers.pop();
        }
        out.push(cur);
        return out;
    }
    // A value holding a bad url (CSS Syntax 3 §4.3.6: an unquoted url with a
    // quote, `(`, inner whitespace, a control character or a broken escape in
    // it) is invalid for every property, so the declaration is not in the
    // block (CH2.E29: Chromium's `el.style.length` leaves it out and the
    // earlier value stands). The same rule as `rustkit_css::mask::parse_css_url`.
    //
    // The value is read as the CSS tokenizer reads it (SEC-STYLE-URL): a
    // function name is an identifier with its escapes decoded (`\75rl(` is
    // `url(`), strings and comments are skipped, an identifier after `#` or
    // `@`, or one that starts with a digit, is not a function name, and
    // whitespace is CSS whitespace. Pinned against Chrome 148 in
    // style_value_url_tests.rs. Every built-in it uses is taken now, so page
    // script cannot change the answer.
    //
    // Only a `url(` or an escape can make a value bad, so a value with
    // neither (almost every value a page writes) is answered by one search.
    var urlLower = uncurry(String.prototype.toLowerCase), urlIndexOf = uncurry(String.prototype.indexOf);
    var urlCode = uncurry(String.prototype.charCodeAt);
    // A property name or priority as the declaration methods compare it, with the same taken built-ins.
    var cssTrim = uncurry(String.prototype.trim), toDOMString = String;
    function propertyName(name) { return urlLower(cssTrim(toDOMString(name))); }
    function cssNewline(c) { return c === 10 || c === 13 || c === 12; }
    function cssSpace(c) { return c === 32 || c === 9 || cssNewline(c); }
    // A value without its leading and trailing CSS whitespace (space, tab, newlines; not NBSP or the other
    // characters JavaScript's trim takes, which CSS reads as name characters).
    var cssSlice = uncurry(String.prototype.slice);
    function cssTrimValue(v) {
        var a = 0, b = v.length;
        while (a < b && cssSpace(urlCode(v, a))) a++;
        while (b > a && cssSpace(urlCode(v, b - 1))) b--;
        return a === 0 && b === v.length ? v : cssSlice(v, a, b);
    }
    function hexValue(c) {
        if (c >= 48 && c <= 57) return c - 48;
        if (c >= 65 && c <= 70) return c - 55;
        if (c >= 97 && c <= 102) return c - 87;
        return -1;
    }
    function identCode(c) {
        return (c >= 97 && c <= 122) || (c >= 65 && c <= 90) || (c >= 48 && c <= 57) || c === 95 || c === 45 || c >= 128;
    }
    // A backslash at `i` starts an escape unless a newline follows it.
    function validEscape(value, i) {
        return urlCode(value, i) === 92 && !(i + 1 < value.length && cssNewline(urlCode(value, i + 1)));
    }
    // The index just past the escape whose backslash is at `i` (§4.3.7: up to six hex digits and one
    // whitespace, or one other character).
    function escapeEnd(value, i) {
        var n = value.length, j = i + 1, k;
        if (j >= n) return j;
        if (hexValue(urlCode(value, j)) < 0) return j + 1;
        for (k = 0; k < 6 && j < n && hexValue(urlCode(value, j)) >= 0; k++) j++;
        if (j + 1 < n && urlCode(value, j) === 13 && urlCode(value, j + 1) === 10) return j + 2;
        if (j < n && cssSpace(urlCode(value, j))) return j + 1;
        return j;
    }
    // Characters the engine's url reader (`rustkit_css::mask::parse_css_url`: Rust's `char::is_whitespace` and
    // `is_control`) refuses in an unquoted url although CSS does not (NUL reads as U+FFFD, NBSP is a url
    // character): refused here too while the engine does, so a value kept here is one the engine reads alike.
    function engineRefusesInUrl(c) {
        return c === 0 || (c >= 0x7f && c <= 0x9f) || c === 0xa0 || c === 0x1680 || (c >= 0x2000 && c <= 0x200a) ||
            c === 0x2028 || c === 0x2029 || c === 0x202f || c === 0x205f || c === 0x3000;
    }
    function hasBadUrl(value) {
        if (urlIndexOf(urlLower(value), 'url(') < 0 && urlIndexOf(value, '\\') < 0) return false;
        var n = value.length, i = 0, c, q, h, k, cp, prev = -1;
        while (i < n) {
            c = urlCode(value, i);
            if (c === 34 || c === 39) {
                // A string: to its closing quote; a backslash takes the next character.
                for (i++; i < n; i++) {
                    q = urlCode(value, i);
                    if (q === 92) { i++; continue; }
                    if (q === c || cssNewline(q)) break;
                }
                prev = c; i++;
                continue;
            }
            if (c === 47 && i + 1 < n && urlCode(value, i + 1) === 42) {
                k = urlIndexOf(value, '*/', i + 2);
                i = k < 0 ? n : k + 2;
                prev = 32;
                continue;
            }
            if (!identCode(c) && !validEscape(value, i)) { prev = c; i++; continue; }
            // An identifier run, decoded: its length and its first three code points (plain variables, not an
            // array page script could reach through Array.prototype).
            var count = 0, c0 = -1, c1 = -1, c2 = -1;
            while (i < n) {
                c = urlCode(value, i);
                if (identCode(c)) { cp = c; i++; }
                else if (validEscape(value, i)) {
                    i++;
                    if (i >= n) { cp = 0xfffd; }
                    else if (hexValue(urlCode(value, i)) >= 0) {
                        cp = 0;
                        for (k = 0; k < 6 && i < n && (h = hexValue(urlCode(value, i))) >= 0; k++, i++) cp = cp * 16 + h;
                        if (i < n && urlCode(value, i) === 13 && i + 1 < n && urlCode(value, i + 1) === 10) i += 2;
                        else if (i < n && cssSpace(urlCode(value, i))) i++;
                        if (cp === 0 || (cp >= 0xd800 && cp <= 0xdfff) || cp > 0x10ffff) cp = 0xfffd;
                    } else { cp = urlCode(value, i); i++; }
                } else break;
                if (count === 0) c0 = cp; else if (count === 1) c1 = cp; else if (count === 2) c2 = cp;
                count++;
            }
            var isUrl = count === 3 && prev !== 35 && prev !== 64 &&
                (c0 === 117 || c0 === 85) && (c1 === 114 || c1 === 82) && (c2 === 108 || c2 === 76) &&
                i < n && urlCode(value, i) === 40;
            prev = 97;
            if (!isUrl) continue;
            // The url's contents (§4.3.6), after `(` and any whitespace.
            for (i++; i < n && cssSpace(urlCode(value, i)); i++) {}
            // A quoted url is a function with a string: the string rule above reads it.
            if (i < n && (urlCode(value, i) === 34 || urlCode(value, i) === 39)) continue;
            for (; i < n; i++) {
                c = urlCode(value, i);
                if (c === 41) break;
                if (cssSpace(c)) {
                    for (k = i; k < n && cssSpace(urlCode(value, k)); k++) {}
                    // Whitespace is allowed only before the closing `)` (or the end).
                    if (k < n && urlCode(value, k) !== 41) return true;
                    i = k - 1;
                    continue;
                }
                if (c === 34 || c === 39 || c === 40 || c <= 8 || c === 11 || (c >= 14 && c <= 31) || c === 127) return true;
                if (engineRefusesInUrl(c)) return true;
                if (c === 92) {
                    if (!validEscape(value, i)) return true;
                    i = escapeEnd(value, i) - 1;
                }
            }
            i++;
            prev = 41;
        }
        return false;
    }
    // How many times an inline style attribute's text has been parsed (`__rk.inlineStyleParses`): the
    // measure behind MotionMark's Multiply, which reads and writes thousands of inline properties a frame.
    var inlineStyleParses = 0;
    __rk.inlineStyleParses = function () { return inlineStyleParses; };
    // Each element's declarations as last parsed, with the attribute text they were parsed from: a read
    // of unchanged text needs no parse. Keyed by the text itself, so a script's setAttribute (or anything
    // else that changes the attribute) is read afresh. Readers see the kept declarations' fields; writers
    // work on copies, and a write keeps its own copy.
    //
    // This state outlives a call, so it is made only of what page script cannot reach or change: lists and
    // declarations inherit from nothing, are filled by assignment with an explicit count, and hold only
    // strings and booleans; the built-ins on these paths are taken now. A parse (the older code below, which
    // reads built-ins as it goes) is kept only while those built-ins are still the ones this script
    // started with, and only when it produced plain strings: each field of its output is read once, and
    // the value checked is the value kept. A parse that page script disturbed in a way that check does not
    // see (an index accessor on Array.prototype, say) still yields plain strings, kept until the text
    // changes.
    var parsedStyles = new WeakMap(), mapDelete = uncurry(WeakMap.prototype.delete);
    var ownKey = uncurry(Object.prototype.hasOwnProperty), strCode = uncurry(String.prototype.charCodeAt);
    var strLower = uncurry(String.prototype.toLowerCase), fromCode = String.fromCharCode;
    var ownDescriptor = Object.getOwnPropertyDescriptor;
    var PARSER_BUILTINS = [
        [Array.prototype, 'filter'], [Array.prototype, 'forEach'], [Array.prototype, 'push'], [Array.prototype, 'pop'],
        [String.prototype, 'charAt'], [String.prototype, 'substr'], [String.prototype, 'slice'],
        [String.prototype, 'indexOf'], [String.prototype, 'trim'], [String.prototype, 'toLowerCase'],
        [String.prototype, 'replace'], [RegExp.prototype, 'test'], [RegExp.prototype, 'exec']
    ];
    var PARSER_ORIGINALS = [];
    for (var pb = 0; pb < PARSER_BUILTINS.length; pb++) {
        PARSER_ORIGINALS[pb] = ownDescriptor(PARSER_BUILTINS[pb][0], PARSER_BUILTINS[pb][1]).value;
    }
    // Whether the parser's built-ins are as they were: an own data property holding the original. A
    // descriptor read runs no page code.
    function parserPristine() {
        for (var i = 0; i < PARSER_BUILTINS.length; i++) {
            var d = ownDescriptor(PARSER_BUILTINS[i][0], PARSER_BUILTINS[i][1]);
            if (!d || !ownKey(d, 'value') || d.value !== PARSER_ORIGINALS[i]) return false;
        }
        return true;
    }
    function newList() {
        var l = objCreate(null);
        l.n = 0;
        return l;
    }
    function newDecl(name, value, important) {
        var d = objCreate(null);
        d.name = name; d.value = value; d.important = important;
        return d;
    }
    function listAdd(l, d) {
        l[l.n] = d;
        l.n = l.n + 1;
    }
    // The parser's output as a private list. `clean` is false when an entry was not plain strings and a
    // boolean (the parse is then not kept).
    function fromParse(out) {
        var l = newList(), n = out.length, clean = typeof n === 'number';
        for (var i = 0; clean && i < n; i++) {
            // Each field is read once: what is checked is what is kept.
            var x = out[i], name, value, important;
            if (x !== null && typeof x === 'object') { name = x.name; value = x.value; important = x.important; }
            if (typeof name === 'string' && typeof value === 'string' && typeof important === 'boolean') {
                listAdd(l, newDecl(name, value, important));
            } else {
                clean = false;
            }
        }
        l.clean = clean;
        return l;
    }
    function copyList(src) {
        var l = newList();
        for (var i = 0; i < src.n; i++) listAdd(l, newDecl(src[i].name, src[i].value, src[i].important));
        return l;
    }
    function exactOf(l) {
        for (var i = 0; i < l.n; i++) {
            if (!nameRoundTrips(l[i].name) || !roundTrips(l[i].value)) return false;
        }
        return true;
    }
    function keep(el, text, list, exact) {
        var e = objCreate(null);
        e.text = text; e.list = list; e.exact = exact;
        mapSet(parsedStyles, el, e);
    }
    // The kept declarations, for reading only.
    function keptDecls(st) {
        var el = st[OWNER], raw = el.getAttribute('style'), text = raw == null ? '' : raw;
        // An attribute read that answers something other than a string (page script's own getAttribute) is
        // parsed as its text and nothing is kept.
        var keepable = typeof text === 'string';
        if (!keepable) text = '' + text;
        var kept = mapGet(parsedStyles, el);
        if (keepable && kept && kept.text === text) return kept.list;
        var list = fromParse(parseDecls(text));
        if (keepable && list.clean && parserPristine()) keep(el, text, list, exactOf(list));
        else mapDelete(parsedStyles, el);
        return list;
    }
    // A copy, for a caller that changes it.
    function decls(st) {
        return copyList(keptDecls(st));
    }
    function declNamed(st, name) {
        var list = keptDecls(st);
        for (var i = 0; i < list.n; i++) if (list[i].name === name) return list[i];
        return null;
    }
    function parseDecls(text) {
        var out = [];
        inlineStyleParses++;
        splitDecls(text).forEach(function (d) {
            var i = d.indexOf(':');
            if (i < 0) return;
            var name = d.slice(0, i).trim().toLowerCase(), value = cssTrimValue(d.slice(i + 1));
            var important = /!\s*important$/i.test(value);
            if (important) value = value.replace(/\s*!\s*important$/i, '');
            if (!name || !value) return;
            if (hasBadUrl(value)) return;
            out = out.filter(function (x) { return x.name !== name; });
            out.push({ name: name, value: value, important: important });
        });
        return out;
    }
    // A declaration whose serialization reads back as itself: a plain property name (an identifier or a
    // custom property), and a value with no quote, escape, `;`, `!`, brace, comment or control character
    // whose parentheses and brackets nest and close in order (`rotate(9deg)`, `hsla(…)`). Anything else
    // is parsed again on the next read, which is always right. Character codes, not patterns.
    function isLowerAlpha(c) { return c >= 97 && c <= 122; }
    function isDigit(c) { return c >= 48 && c <= 57; }
    function nameRoundTrips(name) {
        var n = name.length, i = 0;
        if (n === 0) return false;
        if (n >= 2 && strCode(name, 0) === 45 && strCode(name, 1) === 45) {
            if (n === 2) return false;
            for (i = 2; i < n; i++) {
                var k = strCode(name, i);
                if (!isLowerAlpha(k) && !isDigit(k) && k !== 45 && k !== 95) return false;
            }
            return true;
        }
        if (strCode(name, 0) === 45) i = 1;
        if (i >= n || !isLowerAlpha(strCode(name, i))) return false;
        for (i = i + 1; i < n; i++) {
            var c = strCode(name, i);
            if (!isLowerAlpha(c) && !isDigit(c) && c !== 45) return false;
        }
        return true;
    }
    function roundTrips(value) {
        // The open brackets, entries 0..depth-1.
        var open = objCreate(null), depth = 0, prev = -1;
        for (var i = 0; i < value.length; i++) {
            var c = strCode(value, i);
            // " ' \ ; ! { } and control characters
            if (c === 34 || c === 39 || c === 92 || c === 59 || c === 33 || c === 123 || c === 125 || c < 32 || c === 127) return false;
            // `/*` and `*/`
            if ((prev === 47 && c === 42) || (prev === 42 && c === 47)) return false;
            if (c === 40) open[depth++] = 41;
            else if (c === 91) open[depth++] = 93;
            else if (c === 41 || c === 93) { if (depth === 0 || open[--depth] !== c) return false; }
            prev = c;
        }
        return depth === 0;
    }
    function serializeDecls(ds) {
        var text = '';
        for (var i = 0; i < ds.n; i++) {
            var d = ds[i];
            text += (i ? ' ' : '') + d.name + ': ' + d.value + (d.important ? ' !important' : '') + ';';
        }
        return text;
    }
    // `ds` is a private list the caller made and drops: it is written, and kept under the text written when
    // every declaration in it reads back as itself, so the next read of that text needs no parse. If the
    // write itself changes the attribute again (a custom element's attributeChangedCallback), the text no
    // longer matches and the next read parses.
    function storeDecls(st, ds) {
        var el = st[OWNER], text = serializeDecls(ds), exact = exactOf(ds);
        el.setAttribute('style', text);
        if (exact) keep(el, text, ds, true);
        else mapDelete(parsedStyles, el);
    }
    // `backgroundColor` -> `background-color`, each name converted once (a null-prototype table: no
    // inherited key answers, and page script cannot reach it). The conversion reads character codes.
    var cssNames = objCreate(null), cssNameCount = 0;
    function cssName(prop) {
        if (ownKey(cssNames, prop)) return cssNames[prop];
        var name = cssNameOf(prop);
        if (cssNameCount < 4096) { cssNames[prop] = name; cssNameCount++; }
        return name;
    }
    function cssNameOf(prop) {
        if (prop === 'cssFloat') return 'float';
        var out = '', dash = false, i;
        for (i = 0; i < prop.length; i++) if (strCode(prop, i) === 45) { dash = true; break; }
        if (dash) return strLower(prop);
        for (i = 0; i < prop.length; i++) {
            var c = strCode(prop, i);
            out += c >= 65 && c <= 90 ? '-' + fromCode(c + 32) : fromCode(c);
        }
        // `webkitX` -> `-webkit-x` (and moz, ms).
        var prefixes = ['webkit-', 'moz-', 'ms-'];
        for (i = 0; i < prefixes.length; i++) {
            var pre = prefixes[i], match = out.length > pre.length;
            for (var j = 0; match && j < pre.length; j++) if (strCode(out, j) !== strCode(pre, j)) match = false;
            if (match) return '-' + out;
        }
        return out;
    }
    // NATIVE-SETPROPERTY: inline style reads and writes are one host call each (`inline_style.rs`, the
    // same results as the JavaScript copy here, which interprets ~6,800 opcodes per write). Only what
    // runs page code stays here: the arguments' string conversions, and a MutationObserver's or a
    // custom element's hook around a write that changed the attribute, made as `putPlain` makes them.
    // `__rk.styleNative(false)` puts the JavaScript copy back, for the cross-check tests.
    var styleNative = true, styleString = String;
    __rk.styleNative = function (on) { styleNative = on === true; };
    function styleRead(st, op, arg) {
        var s = slotOf(st[OWNER]), r = N.styleRead(s.gen, s.id, op, arg);
        return r === -1 ? (op === 'length' ? 0 : '') : r;
    }
    function styleWrite(el, op, name, value, priority) {
        var s = slotOf(el);
        var m = HOOKS.mutation && HOOKS.mutation.data(el, 'setAttr', 'style');
        var ar = HOOKS.attrs && HOOKS.watched.style === true && HOOKS.attrs(el, 'setAttr', 'style', true);
        var r = N.styleWrite(gen, s.gen === gen ? s.id : -1, op, name, value, priority);
        if (r === -1) throw new DOMException("Failed to execute 'setAttribute'.", 'NotFoundError');
        // `set` answers whether it wrote; `remove` the value the property had, written when it had one.
        if (r === true || (op === 'remove' && r !== '')) {
            if (m) m();
            if (ar) ar();
        }
        return r;
    }
    var styleMethods = {
        getPropertyValue: function (name) {
            if (styleNative) return styleRead(this, 'value', styleString(name));
            var d = declNamed(this, propertyName(name));
            return d ? d.value : '';
        },
        getPropertyPriority: function (name) {
            if (styleNative) return styleRead(this, 'priority', styleString(name));
            var d = declNamed(this, propertyName(name));
            return d && d.important ? 'important' : '';
        },
        setProperty: function (name, value, priority) {
            if (styleNative) {
                styleWrite(this[OWNER], 'set', styleString(name), value == null ? '' : styleString(value), styleString(priority || ''));
                return;
            }
            name = propertyName(name);
            value = value == null ? '' : cssTrimValue(toDOMString(value));
            // Plain strings from here on: what is serialised is what is kept and checked.
            if (typeof name !== 'string') name = '' + name;
            if (typeof value !== 'string') value = '' + value;
            if (value === '') { this.removeProperty(name); return; }
            // An invalid value changes nothing (the attribute is not rewritten).
            if (hasBadUrl(value)) return;
            var important = urlLower(toDOMString(priority || '')) === 'important';
            // The same declaration again changes nothing: the attribute is
            // not rewritten (Blink's set-property returns "did not change"),
            // so a geometry read after it lays nothing out (LAT7).
            var same = declNamed(this, name);
            if (same && same.value === value && same.important === important) return;
            var ds = decls(this), d = null;
            for (var i = 0; i < ds.n; i++) if (ds[i].name === name) { d = ds[i]; break; }
            if (d) { d.value = value; d.important = important; }
            else listAdd(ds, newDecl(name, value, important));
            storeDecls(this, ds);
        },
        removeProperty: function (name) {
            if (styleNative) return styleWrite(this[OWNER], 'remove', styleString(name), '', '');
            name = propertyName(name);
            if (typeof name !== 'string') name = '' + name;
            var ds = decls(this), old = this.getPropertyValue(name), kept = newList();
            for (var i = 0; i < ds.n; i++) if (ds[i].name !== name) listAdd(kept, ds[i]);
            if (kept.n !== ds.n) storeDecls(this, kept);
            return old;
        },
        item: function (i) {
            if (styleNative) return styleRead(this, 'item', i >>> 0);
            var ds = keptDecls(this); i = i >>> 0; return i < ds.n ? ds[i].name : '';
        }
    };
    Object.keys(styleMethods).forEach(function (k) { CSSStyleDeclaration.prototype[k] = styleMethods[k]; });
    getter(CSSStyleDeclaration.prototype, 'length', function () {
        return styleNative ? styleRead(this, 'length', 0) : keptDecls(this).n;
    });
    accessor(CSSStyleDeclaration.prototype, 'cssText', function () {
        if (styleNative) return styleRead(this, 'text', '');
        return this[OWNER].getAttribute('style') ? serializeDecls(keptDecls(this)) : '';
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
                // `setProperty(name, v)` with no priority, without the method call (NATIVE-SETPROPERTY).
                if (styleNative) styleWrite(t[OWNER], 'set', cssName(p), v == null ? '' : styleString(v), '');
                else t.setProperty(cssName(p), v);
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
    DocumentFragment.prototype.querySelector = queries.querySelector;
    DocumentFragment.prototype.querySelectorAll = queries.querySelectorAll;
    // NonElementParentNode on a fragment: the id table only knows the
    // document, so walk the fragment's own subtree.
    // A DOMParser document's ids are not in the table either.
    function findById(root, id) {
        id = String(id);
        function find(n) {
            for (var c = n.firstChild; c; c = c.nextSibling) {
                if (c.nodeType !== 1) continue;
                if (id !== '' && c.getAttribute('id') === id) return c;
                var hit = find(c);
                if (hit) return hit;
            }
            return null;
        }
        return find(root);
    }
    DocumentFragment.prototype.getElementById = function (id) { return findById(this, id); };
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
    // A node a document makes is that document's (`adoptTo`): one made by
    // a DOMParser document is in a document with scripting off.
    function ownedBy(node, doc) {
        var s = node && node[SLOT], d = doc && doc[SLOT];
        if (s && d && s.gen === gen && d.gen === gen && doc.nodeType === 9) N.write(gen, 'adoptTo', s.id, d.id);
        return node;
    }
    __rk.adoptTo = function (node, doc) {
        var s = node && node[SLOT], d = doc && doc[SLOT];
        if (s && d && s.gen === gen && d.gen === gen) N.write(gen, 'adoptTo', s.id, d.id);
    };
    Document.prototype.createElement = function (tag) {
        tag = String(tag);
        var el = ownedBy(create('element', tag, 'createElement'), this);
        if (HOOKS.scripts) HOOKS.scripts.created(el);
        return el;
    };
    Document.prototype.createTextNode = function (data) {
        return ownedBy(create('text', String(data), 'createTextNode'), this);
    };
    Document.prototype.createComment = function (data) {
        return ownedBy(create('comment', String(data), 'createComment'), this);
    };
    // DOM §4.5 importNode: a copy for this document. Nodes here all share
    // the one Rust Document, so importing is cloning.
    Document.prototype.importNode = function (node, deep) {
        nodeArg(node, 'importNode');
        if (node.nodeType === 9) {
            throw new DOMException("Failed to execute 'importNode' on 'Document': " +
                'The node provided is a document, which may not be imported.', 'NotSupportedError');
        }
        return ownedBy(node.cloneNode(!!deep), this);
    };
    Document.prototype.createDocumentFragment = function () {
        return ownedBy(create('fragment', '', 'createDocumentFragment'), this);
    };
    // DOM Range (DOM §5) and createContextualFragment (DOM Parsing). A new
    // range is collapsed at the start of its document. createContextualFragment
    // parses in the context of the start node (a body, when that node is the
    // document) and returns a fragment; its scripts run when it is inserted.
    var RANGE = Symbol('rustkit.range');
    function nodeLength(node) {
        return (node.nodeType === 3 || node.nodeType === 8) ? node.length : node.childNodes.length;
    }
    function childIndex(parent, node) {
        var i = 0;
        for (var c = parent.firstChild; c; c = c.nextSibling, i++) if (c === node) return i;
        return -1;
    }
    // Is boundary `a` strictly before boundary `b`?
    function pointBefore(a, b) {
        if (a.node === b.node) return a.offset < b.offset;
        var pos = a.node.compareDocumentPosition(b.node);
        if (pos & 16) {
            var child = b.node;
            while (child.parentNode && child.parentNode !== a.node) child = child.parentNode;
            return a.offset <= childIndex(a.node, child);
        }
        if (pos & 8) {
            var down = a.node;
            while (down.parentNode && down.parentNode !== b.node) down = down.parentNode;
            return childIndex(b.node, down) < b.offset;
        }
        return (pos & 4) !== 0;
    }
    function rangeBoundary(node, offset, method) {
        if (node == null || !node[SLOT]) {
            throw new TypeError("Failed to execute '" + method + "' on 'Range': parameter 1 is not of type 'Node'.");
        }
        if (node.nodeType === 10) {
            throw new DOMException("Failed to execute '" + method + "' on 'Range': the given Node has an invalid node type.", 'InvalidNodeTypeError');
        }
        offset = Number(offset);
        var len = nodeLength(node);
        if (!isFinite(offset) || offset < 0 || offset > len) {
            throw new DOMException("Failed to execute '" + method + "' on 'Range': The offset is outside the node's length.", 'IndexSizeError');
        }
        return { node: node, offset: offset };
    }
    function makeRange(doc) {
        var range = Object.create(Range.prototype);
        range[RANGE] = {
            start: { node: doc, offset: 0 },
            end: { node: doc, offset: 0 }
        };
        return range;
    }
    function Range() {
        if (!new.target) {
            throw new TypeError("Failed to construct 'Range': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        }
        var made = makeRange(g.document);
        Object.setPrototypeOf(made, Range.prototype);
        // makeRange already set the prototype via Object.create; copy the slot.
        this[RANGE] = made[RANGE];
    }
    Object.defineProperty(Range.prototype, Symbol.toStringTag, { value: 'Range' });
    g.Range = Range;
    // AbstractRange and StaticRange (DOM §5.3, §5.4): a StaticRange is four
    // plain values, never updated and never validated beyond its containers'
    // types. `InputEvent.getTargetRanges()` answers them.
    function AbstractRange() { throw new TypeError('Illegal constructor'); }
    Object.defineProperty(AbstractRange.prototype, Symbol.toStringTag, { value: 'AbstractRange', configurable: true });
    Object.setPrototypeOf(Range.prototype, AbstractRange.prototype);
    Object.setPrototypeOf(Range, AbstractRange);
    g.AbstractRange = AbstractRange;
    var STATIC = Symbol('rustkit.staticRange');
    function StaticRange(init) {
        if (!new.target) throw new TypeError("Failed to construct 'StaticRange': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
        if (arguments.length < 1) throw new TypeError("Failed to construct 'StaticRange': 1 argument required, but only 0 present.");
        if (init === null || typeof init !== 'object') throw new TypeError("Failed to construct 'StaticRange': The provided value is not of type 'StaticRangeInit'.");
        ['startContainer', 'startOffset', 'endContainer', 'endOffset'].forEach(function (k) {
            if (init[k] === undefined) {
                throw new TypeError("Failed to construct 'StaticRange': Failed to read the '" + k + "' property from 'StaticRangeInit': Required member is undefined.");
            }
        });
        [init.startContainer, init.endContainer].forEach(function (n) {
            if (!(n instanceof Node)) throw new TypeError("Failed to construct 'StaticRange': Failed to read a container from 'StaticRangeInit': it is not of type 'Node'.");
            if (n.nodeType === 10 || n.nodeType === 2) {
                throw new DOMException("Failed to construct 'StaticRange': Neither startContainer nor endContainer can be a DocumentType or Attribute node.", 'InvalidNodeTypeError');
            }
        });
        Object.defineProperty(this, STATIC, { value: {
            startContainer: init.startContainer, startOffset: init.startOffset >>> 0,
            endContainer: init.endContainer, endOffset: init.endOffset >>> 0 } });
    }
    StaticRange.prototype = Object.create(AbstractRange.prototype, { constructor: { value: StaticRange, writable: true, configurable: true } });
    Object.setPrototypeOf(StaticRange, AbstractRange);
    Object.defineProperty(StaticRange.prototype, Symbol.toStringTag, { value: 'StaticRange', configurable: true });
    ['startContainer', 'startOffset', 'endContainer', 'endOffset'].forEach(function (k) {
        getter(StaticRange.prototype, k, function () {
            var s = this[STATIC];
            if (!s) throw new TypeError('Illegal invocation');
            return s[k];
        });
    });
    getter(StaticRange.prototype, 'collapsed', function () {
        var s = this[STATIC];
        if (!s) throw new TypeError('Illegal invocation');
        return s.startContainer === s.endContainer && s.startOffset === s.endOffset;
    });
    g.StaticRange = StaticRange;
    function slots(range, method) {
        var r = range && range[RANGE];
        if (!r) {
            throw new TypeError("Failed to execute '" + method + "' on 'Range': Illegal invocation");
        }
        return r;
    }
    getter(Range.prototype, 'startContainer', function () { return slots(this, 'startContainer').start.node; });
    getter(Range.prototype, 'startOffset', function () { return slots(this, 'startContainer').start.offset; });
    getter(Range.prototype, 'endContainer', function () { return slots(this, 'endContainer').end.node; });
    getter(Range.prototype, 'endOffset', function () { return slots(this, 'endContainer').end.offset; });
    getter(Range.prototype, 'collapsed', function () {
        var r = slots(this, 'collapsed');
        return r.start.node === r.end.node && r.start.offset === r.end.offset;
    });
    getter(Range.prototype, 'commonAncestorContainer', function () {
        var r = slots(this, 'commonAncestorContainer');
        var seen = [];
        for (var n = r.start.node; n; n = n.parentNode) seen.push(n);
        for (var m = r.end.node; m; m = m.parentNode) if (seen.indexOf(m) >= 0) return m;
        return r.start.node;
    });
    Range.prototype.setStart = function (node, offset) {
        var r = slots(this, 'setStart');
        var p = rangeBoundary(node, offset, 'setStart');
        r.start = p;
        if (pointBefore(r.end, r.start)) r.end = { node: p.node, offset: p.offset };
    };
    Range.prototype.setEnd = function (node, offset) {
        var r = slots(this, 'setEnd');
        var p = rangeBoundary(node, offset, 'setEnd');
        r.end = p;
        if (pointBefore(r.end, r.start)) r.start = { node: p.node, offset: p.offset };
    };
    Range.prototype.selectNode = function (node) {
        slots(this, 'selectNode');
        if (node == null || !node[SLOT]) {
            throw new TypeError("Failed to execute 'selectNode' on 'Range': parameter 1 is not of type 'Node'.");
        }
        var parent = node.parentNode;
        if (!parent) {
            throw new DOMException("Failed to execute 'selectNode' on 'Range': the given Node has no parent.", 'InvalidNodeTypeError');
        }
        var index = childIndex(parent, node);
        this.setStart(parent, index);
        this.setEnd(parent, index + 1);
    };
    Range.prototype.selectNodeContents = function (node) {
        slots(this, 'selectNodeContents');
        this.setStart(node, 0);
        this.setEnd(node, nodeLength(node));
    };
    Range.prototype.collapse = function (toStart) {
        var r = slots(this, 'collapse');
        if (toStart || toStart === undefined) r.end = { node: r.start.node, offset: r.start.offset };
        else r.start = { node: r.end.node, offset: r.end.offset };
    };
    function contentsOf(range, cutting) {
        var r = slots(range, cutting ? 'deleteContents' : 'cloneContents');
        var frag = g.document.createDocumentFragment();
        if (r.start.node === r.end.node && r.start.offset === r.end.offset) return frag;
        var n = r.start.node;
        if (r.start.node === r.end.node) {
            if (n.nodeType === 3 || n.nodeType === 8) {
                var data = n.data.slice(r.start.offset, r.end.offset);
                // Cutting hands the removed text back too (extractContents, surroundContents).
                frag.appendChild(g.document.createTextNode(data));
                if (cutting) n.data = n.data.slice(0, r.start.offset) + n.data.slice(r.end.offset);
            } else {
                var kids = Array.prototype.slice.call(n.childNodes, r.start.offset, r.end.offset);
                kids.forEach(function (k) {
                    frag.appendChild(cutting ? k : k.cloneNode(true));
                });
            }
            if (cutting) r.end = { node: n, offset: r.start.offset };
            return frag;
        }
        // The ends sit in different nodes: clone or remove each whole node
        // strictly inside the range, and the partial character data at each end.
        function takeText(node, from, to) {
            var piece = node.data.slice(from, to);
            if (cutting) node.data = node.data.slice(0, from) + node.data.slice(to);
            else frag.appendChild(g.document.createTextNode(piece));
            if (cutting) frag.appendChild(g.document.createTextNode(piece));
        }
        if ((r.start.node.nodeType === 3 || r.start.node.nodeType === 8) && r.start.offset < r.start.node.length) {
            takeText(r.start.node, r.start.offset, r.start.node.length);
        }
        var cur = r.start.node;
        while (cur && cur !== r.end.node) {
            var next = cur.nextSibling || (cur.parentNode && cur.parentNode.nextSibling);
            if (cur !== r.start.node && cur.parentNode && pointBefore({ node: cur.parentNode, offset: childIndex(cur.parentNode, cur) }, r.end)
                && pointBefore(r.start, { node: cur.parentNode, offset: childIndex(cur.parentNode, cur) + 1 })) {
                frag.appendChild(cutting ? cur : cur.cloneNode(true));
            }
            cur = next;
            if (cur === r.start.node) break;
        }
        if ((r.end.node.nodeType === 3 || r.end.node.nodeType === 8) && r.end.offset > 0 && r.end.node !== r.start.node) {
            takeText(r.end.node, 0, r.end.offset);
        }
        if (cutting) r.end = { node: r.start.node, offset: r.start.offset };
        return frag;
    }
    Range.prototype.cloneContents = function () { return contentsOf(this, false); };
    Range.prototype.deleteContents = function () { contentsOf(this, true); };
    // DOM §5.5 extractContents: what deleteContents removes, handed back.
    Range.prototype.extractContents = function () { return contentsOf(this, true); };
    function isInclusiveAncestor(ancestor, node) {
        for (; node; node = node.parentNode) if (node === ancestor) return true;
        return false;
    }
    // DOM §5.5 insertNode: split a Text start node, insert before the reference
    // node, and (for a collapsed range) put the end after what went in.
    Range.prototype.insertNode = function (node) {
        var r = slots(this, 'insertNode');
        if (node == null || !node[SLOT]) {
            throw new TypeError("Failed to execute 'insertNode' on 'Range': parameter 1 is not of type 'Node'.");
        }
        var s = r.start.node;
        if (s.nodeType === 8 || (s.nodeType === 3 && !s.parentNode) || s === node) {
            throw new DOMException("Failed to execute 'insertNode' on 'Range': The node to be inserted cannot be placed at the range start.", 'HierarchyRequestError');
        }
        var wasCollapsed = r.start.node === r.end.node && r.start.offset === r.end.offset;
        var ref;
        var parent;
        if (s.nodeType === 3) {
            var off = r.start.offset;
            parent = s.parentNode;
            ref = g.document.createTextNode(s.data.slice(off));
            s.data = s.data.slice(0, off);
            parent.insertBefore(ref, s.nextSibling);
            // The end boundary that sat in the cut-off tail moves with it.
            if (!wasCollapsed && r.end.node === s && r.end.offset > off) {
                r.end = { node: ref, offset: r.end.offset - off };
            }
        } else {
            ref = r.start.offset < s.childNodes.length ? s.childNodes[r.start.offset] : null;
            parent = s;
        }
        if (node === ref) ref = ref.nextSibling;
        var newOffset = (ref ? childIndex(parent, ref) : nodeLength(parent)) + (node.nodeType === 11 ? node.childNodes.length : 1);
        parent.insertBefore(node, ref);
        if (wasCollapsed) r.end = { node: parent, offset: newOffset };
    };
    // DOM §5.5 surroundContents: refuse a range that cuts an element in two,
    // empty `newParent`, move the contents into it, and select it.
    Range.prototype.surroundContents = function (newParent) {
        var r = slots(this, 'surroundContents');
        if (newParent == null || !newParent[SLOT]) {
            throw new TypeError("Failed to execute 'surroundContents' on 'Range': parameter 1 is not of type 'Node'.");
        }
        function partiallyContained(node) {
            return node.nodeType !== 3 && isInclusiveAncestor(node, r.start.node) !== isInclusiveAncestor(node, r.end.node);
        }
        for (var a = r.start.node; a; a = a.parentNode) {
            if (partiallyContained(a)) {
                throw new DOMException("Failed to execute 'surroundContents' on 'Range': The Range has partially selected a non-Text node.", 'InvalidStateError');
            }
        }
        for (var b = r.end.node; b; b = b.parentNode) {
            if (partiallyContained(b)) {
                throw new DOMException("Failed to execute 'surroundContents' on 'Range': The Range has partially selected a non-Text node.", 'InvalidStateError');
            }
        }
        if (newParent.nodeType === 9 || newParent.nodeType === 10 || newParent.nodeType === 11) {
            throw new DOMException("Failed to execute 'surroundContents' on 'Range': The node provided is of an invalid type.", 'InvalidNodeTypeError');
        }
        var fragment = contentsOf(this, true);
        while (newParent.firstChild) newParent.removeChild(newParent.firstChild);
        this.insertNode(newParent);
        newParent.appendChild(fragment);
        this.selectNode(newParent);
    };
    function rangeContext(node) {
        if (node.nodeType === 1) return node;
        if (node.nodeType === 3 || node.nodeType === 8) return node.parentElement;
        return null;
    }
    Range.prototype.createContextualFragment = function (html) {
        var r = slots(this, 'createContextualFragment');
        var ctx = rangeContext(r.start.node);
        var tag = ctx && ctx.localName ? ctx.localName : 'body';
        var home = r.start.node.nodeType === 9 ? r.start.node : r.start.node.ownerDocument;
        var hs = home && home[SLOT];
        var made = N.write(gen, 'create:fragmentHTML', tag, String(html), hs && hs.gen === gen ? hs.id : -1);
        if (typeof made === 'string') {
            throw new DOMException("Failed to execute 'createContextualFragment' on 'Range'.", made);
        }
        var frag = wrap(made);
        // Mark every script in the fragment (not ones parked in a template's
        // content) so insertion prepares them. innerHTML does not.
        function markScripts(node) {
            if (node.nodeType === 1 && node.localName === 'script' && HOOKS.scripts) HOOKS.scripts.created(node);
            if (node.localName === 'template') return;
            for (var c = node.firstChild; c; c = c.nextSibling) markScripts(c);
        }
        markScripts(frag);
        return frag;
    };
    Document.prototype.createRange = function () {
        return makeRange(this.nodeType === 9 ? this : g.document);
    };
    // Range stringifier (DOM §5.5): the data of the Text nodes the range
    // contains, the partial ends included.
    function nextInTree(n) {
        if (n.firstChild) return n.firstChild;
        return nextSkipping(n);
    }
    function nextSkipping(n) {
        while (n) {
            if (n.nextSibling) return n.nextSibling;
            n = n.parentNode;
        }
        return null;
    }
    function rangeText(r) {
        var s = r.start, e = r.end;
        if (s.node === e.node && s.node.nodeType === 3) return s.node.data.slice(s.offset, e.offset);
        var out = s.node.nodeType === 3 ? s.node.data.slice(s.offset) : '';
        var n = s.node.nodeType === 3 ? nextSkipping(s.node)
              : (s.offset < s.node.childNodes.length ? s.node.childNodes[s.offset] : nextSkipping(s.node));
        while (n) {
            if (n === e.node) {
                if (n.nodeType === 3) {
                    out += n.data.slice(0, e.offset);
                    break;
                }
                n = nextInTree(n);
                continue;
            }
            var p = n.parentNode;
            if (!p || !pointBefore({ node: p, offset: childIndex(p, n) }, e)) break;
            if (n.nodeType === 3) out += n.data;
            n = nextInTree(n);
        }
        return out;
    }
    Range.prototype.toString = function () { return rangeText(slots(this, 'toString')); };
    // The rects of a range (CSSOM View §10.1): the client rects of each
    // element it selects whose parent it does not select, and of the part
    // of each text node it selects or ends in, in tree order. The text
    // rects are per line and per character (layout's `text_range_rects`).
    var DOMRectList = g.DOMRectList;
    if (typeof DOMRectList !== 'function') {
        DOMRectList = function DOMRectList() { throw new TypeError('Illegal constructor'); };
        Object.defineProperty(DOMRectList.prototype, Symbol.toStringTag, { value: 'DOMRectList' });
        DOMRectList.prototype.item = function (i) { i = i >>> 0; return i < this.length ? this[i] : null; };
        g.DOMRectList = DOMRectList;
    }
    function rectList(rects) {
        var list = Object.create(DOMRectList.prototype);
        rects.forEach(function (r, i) { Object.defineProperty(list, i, { value: r, enumerable: true }); });
        Object.defineProperty(list, 'length', { value: rects.length });
        return list;
    }
    function rangeRects(r) {
        // `out` in tree order, a text node as null until the one host call
        // below answers for all of them (`asks`).
        var out = [], asks = [], start = r.start, end = r.end;
        // The first node at or after the start, and the first one past the end.
        var node = start.node.nodeType === 1 || start.node.nodeType === 9 || start.node.nodeType === 11
            ? (start.node.childNodes[start.offset] || nextSkipping(start.node)) : start.node;
        var stop = end.node.nodeType === 1 || end.node.nodeType === 9 || end.node.nodeType === 11
            ? (end.node.childNodes[end.offset] || nextSkipping(end.node)) : nextSkipping(end.node);
        // A node is selected when the range holds all of it: neither end
        // is in it. (Every node this is asked about is one the walk below
        // visits, or the parent of one: none is wholly before the start or
        // wholly after the end.) Looked up in two sets made once, not by
        // counting siblings for each node: a range over a long list asked
        // about every item, and counted to it each time.
        var holdsAnEnd = new Set();
        [start.node, end.node].forEach(function (n) { for (; n; n = n.parentNode) holdsAnEnd.add(n); });
        function selected(n) { return !!n.parentNode && !holdsAnEnd.has(n); }
        for (; node && node !== stop; node = nextInTree(node)) {
            if (node.nodeType === 3) {
                var from = node === start.node ? start.offset : 0;
                var to = node === end.node ? end.offset : node.length;
                var s = node[SLOT];
                if (s && s.gen === gen) {
                    asks.push(s.id + ' ' + from + ' ' + to);
                    out.push(null);
                }
            } else if (node.nodeType === 1 && selected(node) && !(node.parentNode && node.parentNode.nodeType === 1 && selected(node.parentNode))) {
                var own = node.getClientRects();
                for (var k = 0; k < own.length; k++) out.push(own[k]);
            }
        }
        if (!asks.length) return out;
        var answers = JSON.parse(N.rangeTextRects(gen, asks.join(';'))), next = 0, all = [];
        var sx = scrollOffset('x'), sy = scrollOffset('y');
        out.forEach(function (item) {
            if (item !== null) { all.push(item); return; }
            var flat = answers[next++] || [];
            for (var i = 0; i + 3 < flat.length; i += 4) all.push(rect(flat[i] - sx, flat[i + 1] - sy, flat[i + 2], flat[i + 3]));
        });
        return all;
    }
    Range.prototype.getClientRects = function () {
        return rectList(rangeRects(slots(this, 'getClientRects')));
    };
    // The smallest rect around those that are not empty in both
    // directions; all zeros when there are none.
    Range.prototype.getBoundingClientRect = function () {
        var rects = rangeRects(slots(this, 'getBoundingClientRect')).filter(function (b) { return b.width !== 0 || b.height !== 0; });
        if (!rects.length) return rect(0, 0, 0, 0);
        var left = Infinity, top = Infinity, right = -Infinity, bottom = -Infinity;
        rects.forEach(function (b) {
            left = Math.min(left, b.left); top = Math.min(top, b.top);
            right = Math.max(right, b.right); bottom = Math.max(bottom, b.bottom);
        });
        return rect(left, top, right - left, bottom - top);
    };
    Range.prototype.cloneRange = function () {
        var r = slots(this, 'cloneRange');
        var made = makeRange(g.document);
        made[RANGE].start = { node: r.start.node, offset: r.start.offset };
        made[RANGE].end = { node: r.end.node, offset: r.end.offset };
        return made;
    };
    if (!Range.prototype.setStartBefore) {
        Range.prototype.setStartBefore = function (n) { this.setStart(n.parentNode, childIndex(n.parentNode, n)); };
        Range.prototype.setStartAfter = function (n) { this.setStart(n.parentNode, childIndex(n.parentNode, n) + 1); };
        Range.prototype.setEndBefore = function (n) { this.setEnd(n.parentNode, childIndex(n.parentNode, n)); };
        Range.prototype.setEndAfter = function (n) { this.setEnd(n.parentNode, childIndex(n.parentNode, n) + 1); };
    }
    Range.prototype.detach = function () {};

    // The document's selection (Selection API §2): at most one range, and a
    // direction that says which end is the anchor. The engine paints it and
    // moves it with the mouse: `__rk.setSelection` is how a drag reaches
    // script, `__rk.takeSelection` how a script's change reaches the engine.
    var SELN = { range: null, backward: false, gen: -1, sent: '', pending: false };
    function selRange() {
        if (SELN.gen !== gen) {
            SELN.range = null;
            SELN.gen = gen;
        }
        return SELN.range;
    }
    function selectionChanged() {
        if (SELN.pending) return;
        SELN.pending = true;
        Promise.resolve().then(function () {
            SELN.pending = false;
            if (!g.Event) return;
            dispatchTo(g.document, new Event('selectionchange', { bubbles: false, cancelable: false }));
        });
    }
    function setSel(anchor, focus) {
        selRange();
        if (!anchor) {
            if (!SELN.range) return;
            SELN.range = null;
            SELN.backward = false;
            selectionChanged();
            return;
        }
        var backward = pointBefore(focus, anchor);
        var range = makeRange(g.document);
        range[RANGE].start = backward ? focus : anchor;
        range[RANGE].end = backward ? anchor : focus;
        SELN.range = range;
        SELN.backward = backward;
        selectionChanged();
    }
    function selEnds() {
        var r = selRange();
        if (!r) return null;
        var s = r[RANGE];
        return SELN.backward ? { anchor: s.end, focus: s.start } : { anchor: s.start, focus: s.end };
    }
    function Selection() { throw new TypeError('Illegal constructor'); }
    var selection = Object.create(Selection.prototype);
    Object.defineProperty(Selection.prototype, Symbol.toStringTag, { value: 'Selection' });
    g.Selection = Selection;
    getter(Selection.prototype, 'anchorNode', function () { var e = selEnds(); return e ? e.anchor.node : null; });
    getter(Selection.prototype, 'anchorOffset', function () { var e = selEnds(); return e ? e.anchor.offset : 0; });
    getter(Selection.prototype, 'focusNode', function () { var e = selEnds(); return e ? e.focus.node : null; });
    getter(Selection.prototype, 'focusOffset', function () { var e = selEnds(); return e ? e.focus.offset : 0; });
    getter(Selection.prototype, 'isCollapsed', function () { var r = selRange(); return !r || r.collapsed; });
    getter(Selection.prototype, 'rangeCount', function () { return selRange() ? 1 : 0; });
    getter(Selection.prototype, 'type', function () {
        var r = selRange();
        return !r ? 'None' : r.collapsed ? 'Caret' : 'Range';
    });
    getter(Selection.prototype, 'direction', function () {
        var r = selRange();
        return !r || r.collapsed ? 'none' : SELN.backward ? 'backward' : 'forward';
    });
    Selection.prototype.getRangeAt = function (i) {
        var r = selRange();
        if (!r || Number(i) !== 0) {
            throw new DOMException("Failed to execute 'getRangeAt' on 'Selection': " + i + " is not a valid index.", 'IndexSizeError');
        }
        return r;
    };
    Selection.prototype.addRange = function (range) {
        slots(range, 'addRange');
        if (selRange()) return;
        SELN.range = range;
        SELN.backward = false;
        selectionChanged();
    };
    Selection.prototype.removeRange = function (range) {
        if (selRange() !== range) {
            throw new DOMException("Failed to execute 'removeRange' on 'Selection': The given range isn't in document.", 'NotFoundError');
        }
        setSel(null);
    };
    Selection.prototype.removeAllRanges = function () { setSel(null); };
    Selection.prototype.empty = Selection.prototype.removeAllRanges;
    Selection.prototype.collapse = function (node, offset) {
        if (node == null) return setSel(null);
        var p = rangeBoundary(node, offset || 0, 'collapse');
        setSel(p, { node: p.node, offset: p.offset });
    };
    Selection.prototype.setPosition = Selection.prototype.collapse;
    Selection.prototype.collapseToStart = function () {
        var r = selRange();
        if (!r) throw new DOMException("Failed to execute 'collapseToStart' on 'Selection': there is no selection.", 'InvalidStateError');
        var s = r[RANGE].start;
        setSel({ node: s.node, offset: s.offset }, { node: s.node, offset: s.offset });
    };
    Selection.prototype.collapseToEnd = function () {
        var r = selRange();
        if (!r) throw new DOMException("Failed to execute 'collapseToEnd' on 'Selection': there is no selection.", 'InvalidStateError');
        var e = r[RANGE].end;
        setSel({ node: e.node, offset: e.offset }, { node: e.node, offset: e.offset });
    };
    Selection.prototype.extend = function (node, offset) {
        var ends = selEnds();
        if (!ends) throw new DOMException("Failed to execute 'extend' on 'Selection': This Selection object doesn't have any Ranges.", 'InvalidStateError');
        var p = rangeBoundary(node, offset || 0, 'extend');
        setSel({ node: ends.anchor.node, offset: ends.anchor.offset }, p);
    };
    Selection.prototype.setBaseAndExtent = function (an, ao, fn, fo) {
        var a = rangeBoundary(an, ao, 'setBaseAndExtent');
        var f = rangeBoundary(fn, fo, 'setBaseAndExtent');
        setSel(a, f);
    };
    Selection.prototype.selectAllChildren = function (node) {
        var a = rangeBoundary(node, 0, 'selectAllChildren');
        setSel(a, { node: node, offset: node.childNodes.length });
    };
    Selection.prototype.deleteFromDocument = function () {
        var r = selRange();
        if (r) {
            r.deleteContents();
            selectionChanged();
        }
    };
    Selection.prototype.containsNode = function (node, partly) {
        var r = selRange();
        if (!r || !node || !node.parentNode) return false;
        var s = r[RANGE];
        var at = { node: node.parentNode, offset: childIndex(node.parentNode, node) };
        var after = { node: node.parentNode, offset: at.offset + 1 };
        if (partly) return pointBefore(at, s.end) && pointBefore(s.start, after);
        return !pointBefore(at, s.start) && !pointBefore(s.end, after);
    };
    Selection.prototype.modify = function () {};
    Selection.prototype.toString = function () {
        var r = selRange();
        return r ? rangeText(r[RANGE]) : '';
    };
    g.getSelection = function () { return selection; };
    Document.prototype.getSelection = function () { return selection; };
    // The engine moved the selection (a drag, a double-click): ids and
    // UTF-16 offsets, or null to clear it.
    __rk.setSelection = function (aId, aOff, fId, fOff) {
        var a = typeof aId === 'number' ? wrap(aId) : null;
        var f = typeof fId === 'number' ? wrap(fId) : null;
        if (!a || !f) setSel(null);
        else setSel({ node: a, offset: Math.min(aOff, nodeLength(a)) }, { node: f, offset: Math.min(fOff, nodeLength(f)) });
        SELN.sent = selKey();
    };
    function selKey() {
        var e = selEnds();
        if (!e) return '';
        var a = e.anchor.node[SLOT], f = e.focus.node[SLOT];
        if (!a || !f || a.gen !== gen || f.gen !== gen) return '';
        return a.id + ' ' + e.anchor.offset + ' ' + f.id + ' ' + e.focus.offset;
    }
    // Plain-text editing in a contenteditable host (CH2.E2): the engine
    // hands each editing key here; it edits the DOM at the selection,
    // moves the selection, and fires `input` at the host. Returns true
    // when something changed.
    function inside(host, node) {
        for (var n = node; n; n = n.parentNode) if (n === host) return true;
        return false;
    }
    function lastDeep(n) { while (n.lastChild) n = n.lastChild; return n; }
    function firstDeep(n) { while (n.firstChild) n = n.firstChild; return n; }
    function prevLeaf(host, n) {
        while (n && n !== host) {
            if (n.previousSibling) return lastDeep(n.previousSibling);
            n = n.parentNode;
        }
        return null;
    }
    function nextLeaf(host, n) {
        while (n && n !== host) {
            if (n.nextSibling) return firstDeep(n.nextSibling);
            n = n.parentNode;
        }
        return null;
    }
    function isLeaf(n) { return (n.nodeType === 3 && n.data.length > 0) || n.localName === 'br'; }
    // The text node or <br> just before / after a collapsed point, or null.
    function leafBefore(host, p) {
        var n = p.node.nodeType === 3 ? prevLeaf(host, p.node)
              : p.offset > 0 ? lastDeep(p.node.childNodes[p.offset - 1]) : prevLeaf(host, p.node);
        while (n && !isLeaf(n)) n = prevLeaf(host, n);
        return n;
    }
    function leafAfter(host, p) {
        var n = p.node.nodeType === 3 ? nextLeaf(host, p.node)
              : p.offset < p.node.childNodes.length ? firstDeep(p.node.childNodes[p.offset]) : nextLeaf(host, p.node);
        while (n && !isLeaf(n)) n = nextLeaf(host, n);
        return n;
    }
    function caretAt(node, offset) { setSel({ node: node, offset: offset }, { node: node, offset: offset }); }
    function insertText(host, p, text) {
        if (p.node.nodeType === 3) {
            p.node.data = p.node.data.slice(0, p.offset) + text + p.node.data.slice(p.offset);
            caretAt(p.node, p.offset + text.length);
            return;
        }
        var before = p.offset > 0 ? p.node.childNodes[p.offset - 1] : null;
        if (before && before.nodeType === 3) {
            before.data += text;
            caretAt(before, before.data.length);
            return;
        }
        var t = g.document.createTextNode(text);
        p.node.insertBefore(t, p.node.childNodes[p.offset] || null);
        caretAt(t, text.length);
    }
    function preformatted(host) {
        if (/^plaintext-only$/i.test(host.getAttribute('contenteditable') || '')) return true;
        var ws = g.getComputedStyle ? String(g.getComputedStyle(host).whiteSpace || '') : '';
        return /^(pre|pre-wrap|pre-line|break-spaces)$/.test(ws);
    }
    __rk.editKey = function (hostId, op, text) {
        var host = typeof hostId === 'number' ? wrap(hostId) : null;
        if (!host) return false;
        var r = selRange();
        if (!r || !inside(host, r[RANGE].start.node) || !inside(host, r[RANGE].end.node)) {
            caretAt(host, host.childNodes.length);
            r = selRange();
        }
        if (op === 'all') {
            setSel({ node: host, offset: 0 }, { node: host, offset: host.childNodes.length });
            return true;
        }
        var p = r[RANGE].start, leaf;
        if (op === 'left' || op === 'right') {
            if (!r.collapsed) {
                var end = op === 'left' ? r[RANGE].start : r[RANGE].end;
                caretAt(end.node, end.offset);
                return true;
            }
            if (op === 'left') {
                if (p.node.nodeType === 3 && p.offset > 0) { caretAt(p.node, p.offset - 1); return true; }
                leaf = leafBefore(host, p);
                if (!leaf) return false;
                if (leaf.nodeType === 3) caretAt(leaf, leaf.data.length - 1);
                else caretAt(leaf.parentNode, childIndex(leaf.parentNode, leaf));
            } else {
                if (p.node.nodeType === 3 && p.offset < p.node.data.length) { caretAt(p.node, p.offset + 1); return true; }
                leaf = leafAfter(host, p);
                if (!leaf) return false;
                if (leaf.nodeType === 3) caretAt(leaf, 1);
                else caretAt(leaf.parentNode, childIndex(leaf.parentNode, leaf) + 1);
            }
            return true;
        }
        var inputType, data = null, changed = false;
        if (!r.collapsed) {
            r.deleteContents();
            p = r[RANGE].start;
            caretAt(p.node, p.offset);
            changed = true;
        }
        if (op === 'insert') {
            inputType = 'insertText';
            data = String(text);
            // A typed space at an edge or beside another space would
            // collapse away: it goes in as a no-break space, as in Chrome.
            if (data === ' ' && !preformatted(host)) {
                var t = p.node.nodeType === 3 ? p.node.data : '';
                var prev = t.charAt(p.offset - 1), next = t.charAt(p.offset);
                if (!prev || !next || prev === ' ' || next === ' ') data = ' ';
            }
            insertText(host, p, data);
            changed = true;
        } else if (op === 'enter') {
            inputType = 'insertParagraph';
            if (preformatted(host)) {
                insertText(host, p, '\n');
            } else {
                var br = g.document.createElement('br');
                var after = null;
                if (p.node.nodeType === 3) {
                    var rest = p.node.data.slice(p.offset);
                    p.node.data = p.node.data.slice(0, p.offset);
                    p.node.parentNode.insertBefore(br, p.node.nextSibling);
                    after = g.document.createTextNode(rest);
                    br.parentNode.insertBefore(after, br.nextSibling);
                } else {
                    p.node.insertBefore(br, p.node.childNodes[p.offset] || null);
                }
                // A line break at the very end needs a second one to
                // show the new empty line.
                if (!leafAfter(host, { node: br.parentNode, offset: childIndex(br.parentNode, br) + 1 })) {
                    br.parentNode.insertBefore(g.document.createElement('br'), br.nextSibling);
                }
                if (after) caretAt(after, 0);
                else caretAt(br.parentNode, childIndex(br.parentNode, br) + 1);
            }
            changed = true;
        } else if (op === 'back' || op === 'del') {
            inputType = op === 'back' ? 'deleteContentBackward' : 'deleteContentForward';
            if (!changed) {
                if (op === 'back' && p.node.nodeType === 3 && p.offset > 0) {
                    var cut = p.offset - 1;
                    var code = p.node.data.charCodeAt(cut);
                    if (cut > 0 && code >= 0xDC00 && code <= 0xDFFF) cut--;
                    p.node.data = p.node.data.slice(0, cut) + p.node.data.slice(p.offset);
                    caretAt(p.node, cut);
                    changed = true;
                } else if (op === 'del' && p.node.nodeType === 3 && p.offset < p.node.data.length) {
                    var stop = p.offset + 1;
                    var hi = p.node.data.charCodeAt(p.offset);
                    if (hi >= 0xD800 && hi <= 0xDBFF) stop++;
                    p.node.data = p.node.data.slice(0, p.offset) + p.node.data.slice(stop);
                    caretAt(p.node, p.offset);
                    changed = true;
                } else {
                    leaf = op === 'back' ? leafBefore(host, p) : leafAfter(host, p);
                    if (leaf && leaf.nodeType === 3) {
                        if (op === 'back') {
                            leaf.data = leaf.data.slice(0, -1);
                            caretAt(leaf, leaf.data.length);
                        } else {
                            leaf.data = leaf.data.slice(1);
                            caretAt(leaf, 0);
                        }
                        changed = true;
                    } else if (leaf) {
                        var parent = leaf.parentNode, at = childIndex(parent, leaf);
                        parent.removeChild(leaf);
                        if (p.node.nodeType !== 3) caretAt(parent, at);
                        changed = true;
                    }
                }
            }
        }
        if (!changed) return false;
        var ev = make('InputEvent', 'input', { bubbles: true, composed: true, view: g, data: data, inputType: inputType });
        trust(ev);
        dispatchTo(host, ev);
        return true;
    };
    // The selection as the engine last heard it changed: null when it did
    // not, else '' (none) or "anchorId anchorOffset focusId focusOffset".
    __rk.takeSelection = function () {
        var key = selKey();
        if (key === SELN.sent) return null;
        SELN.sent = key;
        return key;
    };
    // An `<svg>` is one box in the layout. The shape of it a client point
    // is on, else the `<svg>`: found host-side, from the shapes' geometry
    // (svg_geometry.rs `shape_at`), with nothing a page can replace on the
    // way.
    var svgClientRect = Element.prototype.getBoundingClientRect;
    function svgShapeAt(svg, x, y) {
        var s = svg[SLOT];
        if (!s || s.gen !== gen) return svg;
        var r = svgClientRect.call(svg), b = geom(svg, 'client');
        var id = N.svgHit(s.gen, s.id, x - r.left - (b ? round(b.bl) : 0), y - r.top - (b ? round(b.bt) : 0), r.width, r.height);
        return id >= 0 ? (wrap(id) || svg) : svg;
    }
    function pointHit(x, y, method) {
        x = Number(x);
        y = Number(y);
        // WebIDL `double` throws on NaN and infinities. Chrome does the same
        // for elementFromPoint; the coordinates have to be finite before the call.
        if (!isFinite(x) || !isFinite(y)) {
            throw new TypeError("Failed to execute '" + method + "' on 'Document': The provided double value is non-finite.");
        }
        var w = g.innerWidth, h = g.innerHeight;
        if (typeof w === 'number' && typeof h === 'number' && (x < 0 || y < 0 || x >= w || y >= h)) return null;
        var id = N.hit(x, y);
        // A point of the viewport no box is under is the root element's
        // (Chrome for Testing 148: HTML below a short body, not BODY).
        if (id === -1) {
            var root = g.document.documentElement;
            return root && root[SLOT] ? root[SLOT].id : null;
        }
        return id == null ? null : id;
    }
    Document.prototype.elementFromPoint = function (x, y) {
        var id = pointHit(x, y, 'elementFromPoint');
        if (id == null) return null;
        var node = wrap(id);
        while (node && node.nodeType !== 1) node = node.parentElement || node.parentNode;
        // An `<svg>` is one box in the layout: the shape under the point
        // is found from its geometry (web_svg_geometry.js).
        if (node && node.localName === 'svg') node = svgShapeAt(node, Number(x), Number(y));
        return node && node.nodeType === 1 ? node : null;
    };
    Document.prototype.elementsFromPoint = function (x, y) {
        var el = pointHit(x, y, 'elementsFromPoint');
        if (el == null) return [];
        var node = wrap(el);
        while (node && node.nodeType !== 1) node = node.parentElement || node.parentNode;
        var out = [];
        for (; node && node.nodeType === 1; node = node.parentElement) out.push(node);
        return out;
    };
    // Named access on the Window object (web_named_access.js): the element
    // the id or name picks out, an HTMLCollection when several do, else
    // undefined. Looked up each time, so ids added or removed are seen.
    HOOKS.windowNamed = function (name) {
        var d = g.document;
        if (!d || typeof name !== 'string' || name === '') return undefined;
        var ids = collect(d, 'windowNamed', name);
        if (!ids) return undefined;
        if (ids === 'frame') return HOOKS.windowNamed.FRAME;
        var c = list(HTMLCollection.prototype, ids, true);
        return c.length === 0 ? undefined : c.length === 1 ? c[0] : c;
    };
    // Answered for a name only an iframe/frame carries (unsupported).
    Object.defineProperty(HOOKS.windowNamed, 'FRAME', { value: Object.freeze({}) });
    Document.prototype.getElementById = function (id) {
        var s = slotOf(this);
        if (this !== g.document) return findById(this, id);
        return s.gen === gen ? wrap(N.byId(s.gen, String(id))) : null;
    };
    // A DOMParser document answers these from its own tree.
    function childNamed(n, name) {
        for (var c = n && n.firstChild; c; c = c.nextSibling) {
            if (c.nodeType === 1 && (!name || c.localName === name)) return c;
        }
        return null;
    }
    ['documentElement', 'head', 'body'].forEach(function (k) {
        getter(Document.prototype, k, function () {
            var s = slotOf(this);
            if (this !== g.document) {
                var root = childNamed(this);
                return k === 'documentElement' ? root : childNamed(root, k);
            }
            return s.gen === gen ? wrap(N.root(s.gen, k)) : null;
        });
    });
    // `document.title` (HTML §3.1.3): the document's first HTML <title>
    // in tree order (an SVG <title> inside the page is not one). Read, it
    // is that element's own text with ASCII whitespace stripped and
    // collapsed; set, it replaces the element's children, making a <title>
    // at the end of the head when the document has none.
    // Chrome 148 goes one step past the standard: set with no <title> and
    // no <head> to hold one, the value is still read back until a <title>
    // appears. `titleless` keeps that value.
    var titleless = new WeakMap();
    function titleElement(doc) {
        var all = doc.querySelectorAll('title');
        for (var i = 0; i < all.length; i++) {
            if (all[i].namespaceURI === 'http://www.w3.org/1999/xhtml') return all[i];
        }
        return null;
    }
    function collapsed(text) {
        return text.replace(/[\t\n\f\r ]+/g, ' ').replace(/^ | $/g, '');
    }
    accessor(Document.prototype, 'title', function () {
        var t = titleElement(this);
        if (!t) return collapsed(titleless.get(this) || '');
        titleless.delete(this);
        var text = '';
        for (var c = t.firstChild; c; c = c.nextSibling) {
            if (c.nodeType === 3) text += c.data;
        }
        return collapsed(text);
    }, function (value) {
        value = String(value);
        var t = titleElement(this);
        if (!t) {
            var head = this.head;
            if (!head) { titleless.set(this, value); return; }
            t = this.createElement('title');
            head.appendChild(t);
        }
        titleless.delete(this);
        t.textContent = value;
    });

    // `document.currentScript` (HTML §3.1.1): the classic <script> element
    // being run, else null. Pages lean on it constantly: inline scripts
    // that end in `document.currentScript.remove()`, and webpack's
    // `publicPath` detection (`document.currentScript.src`). With it
    // missing, both threw "cannot convert null or undefined to object" and
    // the rest of the script never ran. The engine names the element
    // through `__rk.setCurrentScript(id)` before each script and clears it
    // after; the hook is not enumerable.
    var currentScript = null;
    getter(Document.prototype, 'currentScript', function () {
        var s = slotOf(this); return s.gen === gen ? currentScript : null;
    });
    __rk.setCurrentScript = function (id) { currentScript = typeof id === 'number' ? wrap(id) : null; };
    // The engine reports a script's outcome to its element: `load` or
    // `error`, which do not bubble (HTML "execute the script element").
    __rk.fireOn = function (id, type) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (target) dispatchTo(target, new Event(type));
    };
    // The engine reports that an `<img>`'s image failed to load (a 404, a
    // failed connection, bytes that do not decode, its budget spent): the
    // element is marked with the `src`/`srcset` that failed, which is what
    // `img.complete` reads (web_reflect.js), and hears `error`.
    var failedImages = new WeakMap();
    __rk.imageFailed = function (id) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return;
        loadedImages.delete(target);
        failedImages.set(target, [target.getAttribute('src'), target.getAttribute('srcset')]);
        var ev = make('Event', 'error');
        trust(ev);
        dispatchTo(target, ev);
        settleDecodes(target, false);
    };
    // The engine reports that an `<img>`'s image is in (E54): the element
    // is marked with the `src`/`srcset` that loaded, the image's own size
    // and the URL it came from, which `complete`, `naturalWidth`,
    // `naturalHeight` and `currentSrc` read (web_reflect.js), and then
    // hears `load`: trusted, at the element only. The mark and the event
    // are one step, so a handler never runs for an image that reads as
    // still loading.
    var loadedImages = new WeakMap();
    // MEDIA-AVF M1.2: each media element's state as the engine last pushed it, read
    // by the HTMLMediaElement getters. `__rk.mediaEvent(id, type, state)` sets it (when
    // `state` is given, as JSON) and then fires `type` (when given): trusted, at the
    // element only, so a handler reads the state as of its own event.
    var MEDIA_INITIAL = { rs: 0, ns: 0, duration: null, src: '', error: null, w: 0, h: 0, buffered: [], seekable: [] };
    var mediaStates = new WeakMap();
    __rk.mediaEvent = function (id, type, state) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return;
        if (state) mediaStates.set(target, JSON.parse(state));
        if (!type) return;
        var ev = make('Event', type);
        trust(ev);
        dispatchTo(target, ev);
    };
    __rk.imageLoaded = function (id, width, height, url) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return;
        failedImages.delete(target);
        // No URL: a `data:` image, which is its own `src`.
        loadedImages.set(target, [target.getAttribute('src'), target.getAttribute('srcset'), width, height, url || target.getAttribute('src') || '']);
        var ev = make('Event', 'load');
        trust(ev);
        dispatchTo(target, ev);
        settleDecodes(target, true);
    };
    // `decode()` promises waiting on an `<img>` (web_reflect.js): each with
    // the `src`/`srcset` it waits for. They settle with the image's event,
    // and resolve when the engine gives up on it (E54 R1: a slow image the
    // engine stopped loading hears nothing, and a decode waiting on it
    // would wait for good). A `src` change rejects them, as in Chrome.
    var pendingDecodes = new WeakMap();
    var gaveUpImages = new WeakMap();
    function decodeError() {
        var E = g.DOMException;
        return E ? new E('The source image cannot be decoded.', 'EncodingError') : new Error('The source image cannot be decoded.');
    }
    function sameSource(img, mark) {
        return !!mark && mark[0] === img.getAttribute('src') && mark[1] === img.getAttribute('srcset');
    }
    function settleDecodes(img, ok) {
        var list = pendingDecodes.get(img);
        if (!list) return;
        pendingDecodes.delete(img);
        list.forEach(function (p) { if (ok) p[2](); else p[3](decodeError()); });
    }
    __rk.decodeWait = function (img, resolve, reject) {
        if (sameSource(img, gaveUpImages.get(img))) return resolve();
        var list = pendingDecodes.get(img) || [];
        list.push([img.getAttribute('src'), img.getAttribute('srcset'), resolve, reject]);
        pendingDecodes.set(img, list);
    };
    __rk.imageGaveUp = function (id) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return;
        gaveUpImages.set(target, [target.getAttribute('src'), target.getAttribute('srcset')]);
        settleDecodes(target, true);
    };
    __rk.imageWritten = function (img) {
        // A failure is not held against the restarted load, nor is a
        // give-up; a successful image stays readable while the same
        // `src` reloads (its mark is checked against `src`).
        failedImages.delete(img);
        gaveUpImages.delete(img);
        var list = pendingDecodes.get(img);
        if (!list) return;
        var keep = list.filter(function (p) { return sameSource(img, p); });
        list.filter(function (p) { return !sameSource(img, p); }).forEach(function (p) { p[3](decodeError()); });
        if (keep.length) pendingDecodes.set(img, keep); else pendingDecodes.delete(img);
    };
    // The mark of the image an `<img>` shows now: null while its current
    // `src`/`srcset` has not loaded.
    __rk.imageLoadedFor = function (img) {
        var mark = loadedImages.get(img);
        if (!mark || mark[0] !== img.getAttribute('src') || mark[1] !== img.getAttribute('srcset')) return null;
        return mark;
    };
    __rk.imageFailedFor = function (img) { return failedImages.get(img) || null; };
    // The engine's CSS transitions report their phases (css-transitions-2
    // §4): a trusted TransitionEvent that bubbles; `transitionend` can be
    // cancelled, the others not. An element taken out of the document
    // still hears its `transitioncancel`.
    __rk.fireTransition = function (id, type, init) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return;
        init = bare(init);
        init.bubbles = true;
        init.cancelable = type === 'transitionend';
        var ev = make('TransitionEvent', type, init);
        trust(ev);
        dispatchTo(target, ev);
    };
    // Likewise the engine's `@keyframes` animations (css-animations-1 §5):
    // a trusted AnimationEvent that bubbles and cannot be cancelled.
    __rk.fireAnimation = function (id, type, init) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return;
        init = bare(init);
        init.bubbles = true;
        init.cancelable = false;
        var ev = make('AnimationEvent', type, init);
        trust(ev);
        dispatchTo(target, ev);
    };
    // The engine delivers the user's mouse input: a trusted event that
    // bubbles and can be cancelled (UI Events §3.4), except the enter and
    // leave events, which do neither and go to one element each.
    // `pointer*` and `click` are PointerEvents of the mouse (Pointer
    // Events §4, §12); `mousedown`, `mouseup` and `click` carry the click
    // count. Returns false when a listener called preventDefault().
    __rk.fireMouse = function (id, type, init) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!target) return true;
        init = bare(init);
        // The pointer is on a shape of an `<svg>`, which the layout
        // sees as one box: the event's target is the shape.
        if (target.localName === 'svg' && typeof init.clientX === 'number') {
            target = svgShapeAt(target, init.clientX, init.clientY);
        }
        // `auxclick` and `contextmenu` are PointerEvents too (Chromium 143).
        var pointer = type === 'click' || type === 'auxclick' || type === 'contextmenu' ||
            type.indexOf('pointer') === 0;
        var edge = /(enter|leave)$/.test(type);
        var press = type === 'mousedown' || type === 'mouseup' || type === 'click' || type === 'auxclick';
        init.bubbles = !edge;
        init.cancelable = !edge && type !== 'pointercancel';
        init.composed = !edge;
        init.view = g;
        init.detail = typeof init.detail === 'number' ? init.detail : (press ? 1 : 0);
        init.relatedTarget = typeof init.related === 'number' ? wrap(init.related) : null;
        if (pointer) {
            init.pointerId = 1;
            init.pointerType = 'mouse';
            init.isPrimary = true;
            init.pressure = init.buttons ? 0.5 : 0;
        }
        var ev = make(pointer && __rk.hasCtor('PointerEvent') ? 'PointerEvent' : 'MouseEvent', type, init);
        // Where the point is in the document and in the target's box.
        var at = bare({
            pageX: init.clientX + (g.scrollX || 0), pageY: init.clientY + (g.scrollY || 0),
            offsetX: init.offsetX, offsetY: init.offsetY,
            movementX: init.movementX, movementY: init.movementY
        });
        // A move or a boundary event has no button of its own: `which`
        // is the primary button while it is held.
        if (!press && !pointer) at.which = init.buttons & 1;
        extra(ev, at);
        trust(ev);
        return dispatchTo(target, ev);
    };

    // Whether a wheel at this element can be cancelled: something on its
    // way up to the window listens for `wheel` and is not passive (an
    // `onwheel` handler never is). Chrome for Testing 148 asks the same
    // before it scrolls: with only passive listeners it scrolls first and
    // tells them afterwards.
    // On the host object, like the key dispatch: not the page's to call.
    // The path is walked with the dispatch's own parent links and plain
    // loops: nothing a page can replace (an array method, a parentNode
    // getter) runs here.
    function blocks(list) {
        if (!list) return false;
        for (var i = 0; i < list.length; i++) if (!list[i].removed && !list[i].passive) return true;
        return false;
    }
    __rk.wheelBlocks = function (id) {
            var n = typeof id === 'number' ? wrap(id) : null, probe = { type: 'wheel' };
            // The legacy type is looked for only when something could hear
            // it: each handler lookup reads the node's attributes.
            var legacy = mousewheelListeners > 0 || mousewheelHandler || N.wheelAttribute();
            while (n) {
                var all = LISTENERS.get(n);
                if (all && (blocks(all.wheel) || (legacy && blocks(all.mousewheel)))) return true;
                if (typeof handlerOf(n, 'wheel') === 'function' || (legacy && typeof handlerOf(n, 'mousewheel') === 'function')) return true;
                n = eventParent(n, probe);
            }
            return false;
    };
    // The engine delivers the user's wheel (UI Events §5.5): a trusted
    // WheelEvent that bubbles, in pixels, with the legacy deltas Chrome
    // reports (120 a notch, a notch per 100px and at least one). Before
    // the scroll when it can be cancelled (`wheelBlocks`), and then it
    // returns false when a listener called preventDefault(): the engine
    // leaves the scroll alone. After the scroll, and not cancelable, when
    // only passive listeners hear it.
    __rk.fireWheel = function (id, init, cancelable) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target || !__rk.hasCtor('WheelEvent')) return true;
            init = bare(init);
            if (target.localName === 'svg') target = svgShapeAt(target, init.clientX, init.clientY);
            init.bubbles = true;
            init.cancelable = cancelable !== false;
            init.composed = true;
            init.view = g;
            init.deltaZ = 0;
            init.deltaMode = 0;
            var notches = function (d) { return d === 0 ? 0 : -120 * (d < 0 ? -1 : 1) * mathMax(1, mathRound(mathAbs(d) / 100)); };
            init.wheelDeltaX = notches(init.deltaX);
            init.wheelDeltaY = notches(init.deltaY);
            var ev = make('WheelEvent', 'wheel', init);
            // A target with no `wheel` listener or handler hears the legacy
            // `mousewheel` instead (Chrome for Testing 148; the user's
            // wheel only). Kept in a private map, read by `invoke`.
            if (mousewheelListeners > 0 || mousewheelHandler || N.wheelAttribute()) mapSet(LEGACY_TYPE, ev, 'mousewheel');
            var at = bare({
                pageX: init.clientX + (g.scrollX || 0), pageY: init.clientY + (g.scrollY || 0),
                offsetX: init.offsetX, offsetY: init.offsetY
            });
            extra(ev, at);
            trust(ev);
            return dispatchTo(target, ev);
    };

    // The engine's drag-and-drop processing model (HTML §6.11.5) delivers
    // a trusted DragEvent with the drag's DataTransfer (web_dnd.js): every
    // drag event bubbles; dragleave and dragend cannot be cancelled. `op`
    // is the current drag operation. Returns 'notCancelled:operation',
    // the operation being what the dataTransfer's effects allow after the
    // listeners ran.
    __rk.fireDrag = function (id, type, init, op) {
        var store = __rk.dragStore;
        var target = typeof id === 'number' ? wrap(id) : null;
        if (!store || !target) return '1:none';
        init = bare(init);
        if (type === 'dragstart') store.begin(target);
        var dt = store.prepare(type, op);
        var cancelable = type !== 'dragleave' && type !== 'dragend';
        init.bubbles = true;
        init.cancelable = cancelable;
        init.composed = true;
        init.view = g;
        init.detail = 0;
        init.relatedTarget = typeof init.related === 'number' ? wrap(init.related) : null;
        init.dataTransfer = dt;
        var ev = make(__rk.hasCtor('DragEvent') ? 'DragEvent' : 'MouseEvent', type, init);

        var at = bare({
            pageX: init.clientX + (g.scrollX || 0), pageY: init.clientY + (g.scrollY || 0),
            offsetX: init.offsetX, offsetY: init.offsetY, movementX: 0, movementY: 0, dataTransfer: dt
        });
        extra(ev, at);
        trust(ev);
        var notCancelled = dispatchTo(target, ev);
        // A cancelled dragstart ends the drag before it began.
        var settled = store.settle(type === 'dragstart' && !notCancelled ? 'dragend' : type);
        return (notCancelled ? '1:' : '0:') + settled;
    };

    // The engine delivers the user's key press or release to the focused
    // element: a trusted KeyboardEvent that bubbles and can be cancelled
    // (UI Events §3.7). With nothing focused in the engine (`id` null) it
    // goes to the page's active element: what script focused, else the
    // body. Returns false when a listener called preventDefault().
    // On the host object, not the document: a page can neither call it
    // (the event is trusted) nor replace it. The key arrives as an
    // argument, never in script source (`DomBindings::fire_key_event`).
    __rk.fireKey = function (id, type, init) {
            var target = typeof id === 'number' ? wrap(id) : g.document.activeElement;
            if (!target) return true;
            init = bare(init);
            init.bubbles = true;
            init.cancelable = true;
            init.composed = true;
            init.view = g;
            // The legacy codes (UI Events §7.3; Chrome for Testing 148):
            // `keydown` and `keyup` carry the key's virtual-key code and no
            // charCode; `keypress` carries the character in both.
            if (type === 'keypress') {
                init.keyCode = init.charCode = init.key === 'Enter' ? 13 : (init.key.codePointAt(0) || 0);
            } else {
                init.keyCode = virtualKey(init.key);
                init.charCode = 0;
            }
            var ev = make('KeyboardEvent', type, init);
            trust(ev);
            return dispatchTo(target, ev);
    };
    // A key's Windows virtual-key code, which is what `keyCode` is on every
    // platform, for a US layout: a letter is its capital, a shifted digit or
    // mark is the key it is on.
    var VK_NAMED = { Backspace: 8, Tab: 9, Enter: 13, Shift: 16, Control: 17, Alt: 18, Pause: 19, CapsLock: 20,
        Escape: 27, PageUp: 33, PageDown: 34, End: 35, Home: 36, ArrowLeft: 37, ArrowUp: 38, ArrowRight: 39,
        ArrowDown: 40, Insert: 45, Delete: 46, Meta: 91, ContextMenu: 93, NumLock: 144, ScrollLock: 145 };
    var VK_MARKS = { ' ': 32, ')': 48, '!': 49, '@': 50, '#': 51, '$': 52, '%': 53, '^': 54, '&': 55, '*': 56, '(': 57,
        ';': 186, ':': 186, '=': 187, '+': 187, ',': 188, '<': 188, '-': 189, '_': 189, '.': 190, '>': 190,
        '/': 191, '?': 191, '`': 192, '~': 192, '[': 219, '{': 219, '\\': 220, '|': 220, ']': 221, '}': 221,
        "'": 222, '"': 222 };
    function virtualKey(key) {
        if (hasOwn(VK_NAMED, key)) return VK_NAMED[key];
        if (hasOwn(VK_MARKS, key)) return VK_MARKS[key];
        if (/^[a-z]$/.test(key)) return key.charCodeAt(0) - 32;
        if (/^[A-Z0-9]$/.test(key)) return key.charCodeAt(0);
        var f = /^F([1-9]|1[0-9]|2[0-4])$/.exec(key);
        return f ? 111 + Number(f[1]) : 0;
    }
    // The text a key is about to insert in an editable element: a trusted
    // `textInput` TextEvent after the keypress, before the insertion and its
    // `input` (Chrome for Testing 148). Returns false when cancelled.
    // On the host object, as `fireKey` is.
    __rk.fireTextInput = function (id, data) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target || typeof __rk.newTextEvent !== 'function') return true;
            var ev = make('TextEvent', 'textInput', { bubbles: true, cancelable: true, composed: true, view: g, data: data });
            trust(ev);
            return dispatchTo(target, ev);
    };
    // What the user typed changed the control's value: `input`, which
    // bubbles and cannot be cancelled (Input Events §5.1). `data` is the
    // inserted text, null for a deletion.
    // These three are the engine's alone: on the host object (`__rk`),
    // which page script cannot reach, not on `document`.
    __rk.fireInput = function (id, data) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target) return;
            // The user edited it: `change` when it loses the focus
            // (node_apis.js).
            __rk.markEdited(target);
            var ev = make('InputEvent', 'input', {
                bubbles: true, composed: true, view: g, data: data,
                inputType: data === null ? 'deleteContentBackward' : 'insertText'
            });
            trust(ev);
            dispatchTo(target, ev);
    };
    // The user picked an option of a select in its drop-down or with a
    // key (web_forms.js's pick: selectedness, `input`, `change`). Returns
    // whether the selection changed.
    var pickOption = null;
    __rk.pickOption = function (id, index) {
        var target = typeof id === 'number' ? wrap(id) : null;
        return !!(target && pickOption && pickOption(target, index));
    };
    // Enter in a field (web_forms.js's implicit submission).
    var implicitSubmit = null;
    __rk.implicitSubmit = function (id) {
        var target = typeof id === 'number' ? wrap(id) : null;
        if (target && implicitSubmit) implicitSubmit(target);
    };

    // EventTarget (DOM §2.7) for node wrappers, document and window: a
    // JS-side listener registry, and dispatch through capture, target and
    // bubble phases along the wrapper tree (then document, then window).
    // Engine input events still dispatch Rust-side (events.rs, pin §4);
    // this is what page script registers and fires itself.
    var LISTENERS = new WeakMap();
    function thisTarget(o) { return o == null ? g : o; }
    function flag(opts, key) {
        return typeof opts === 'boolean' ? key === 'capture' && opts : !!(opts && opts[key]);
    }
    var PASSIVE_BY_DEFAULT = { wheel: true, mousewheel: true, touchstart: true, touchmove: true };
    // The event a passive listener is hearing now, if one is: it cannot
    // cancel that event. Held here, not on the event, where a page could
    // clear it.
    var passiveNow = null;
    // The engine dispatches `wheel` only once something can hear it, so a
    // page with no wheel listener scrolls without running any script.
    // How many wheel and mousewheel listeners there are, and how many of
    // them are not passive, plus whether a handler was ever set; the
    // engine is told when either answer changes (`noteWheel`).
    var wheelListeners = 0, wheelBlocking = 0, wheelHandler = false, wheelSaid = '';
    // Whether anything could hear the legacy `mousewheel`: only then does a
    // wheel carry the legacy type, so a page without one pays nothing for
    // it at each node of the path.
    var mousewheelListeners = 0, mousewheelHandler = false;
    function noteWheel() {
        var now = (wheelListeners > 0 || wheelHandler) + ':' + (wheelBlocking > 0 || wheelHandler);
        if (now === wheelSaid) return;
        wheelSaid = now;
        N.noteWheel(wheelListeners > 0 || wheelHandler, wheelBlocking > 0 || wheelHandler);
    }
    function wheelType(type) { return type === 'wheel' || type === 'mousewheel'; }
    // The frame tree's, as the type map is: the window that dispatches an
    // event may not be the one that made it.
    var LEGACY_TYPE = SHARED.legacyType || (SHARED.legacyType = new WeakMap());
    EventTarget.prototype.addEventListener = function (type, cb, opts) {
        if (typeof cb !== 'function' && !(cb && typeof cb.handleEvent === 'function')) return;
        var t = thisTarget(this), all = LISTENERS.get(t);
        if (!all) { all = {}; LISTENERS.set(t, all); }
        type = String(type);
        var list = all[type] || (all[type] = []), capture = flag(opts, 'capture');
        // The `signal` option: an aborted signal adds nothing, and a later
        // abort removes the listener.
        var signal = opts && typeof opts === 'object' ? opts.signal : undefined;
        if (signal != null && signal.aborted) return;
        for (var i = 0; i < list.length; i++) {
            if (list[i].cb === cb && list[i].capture === capture) return;
        }
        // `passive` (DOM §2.7): such a listener cannot cancel the event.
        // Left out, it is on for `wheel`, `mousewheel` and the touch moves
        // on the window, the document, its root and its body, as in Chrome.
        var passive = opts && typeof opts === 'object' && opts.passive !== undefined ? !!opts.passive
            : PASSIVE_BY_DEFAULT[type] === true && (t === g || t === doc || t === doc.documentElement || t === doc.body);
        list.push({ cb: cb, capture: capture, once: flag(opts, 'once'), passive: passive, removed: false });
        if (wheelType(type)) { wheelListeners++; if (!passive) wheelBlocking++; if (type === 'mousewheel') mousewheelListeners++; noteWheel(); }
        if (signal != null && typeof signal.addEventListener === 'function') {
            signal.addEventListener('abort', function () { t.removeEventListener(type, cb, capture); }, { once: true });
        }
    };
    // Does `target` have a listener for `type` right now? For the installs
    // that keep a target alive only while something can hear it
    // (web_match_media.js prunes its document list of media query lists).
    HOOKS.hasListener = function (target, type) {
        var all = LISTENERS.get(thisTarget(target)), list = all && all[String(type)];
        return !!(list && list.length > 0);
    };
    EventTarget.prototype.removeEventListener = function (type, cb, opts) {
        var all = LISTENERS.get(thisTarget(this)), list = all && all[String(type)];
        if (!list) return;
        var capture = flag(opts, 'capture');
        for (var i = 0; i < list.length; i++) {
            if (list[i].cb === cb && list[i].capture === capture) {
                list[i].removed = true;
                if (wheelType(String(type))) { wheelListeners--; if (!list[i].passive) wheelBlocking--; if (String(type) === 'mousewheel') mousewheelListeners--; noteWheel(); }
                list.splice(i, 1);
                return;
            }
        }
    };
    // A listener's exception goes to the engine's error log, not to the
    // dispatcher (as in a browser, where it reaches window.onerror).
    var callNow = uncurry(Function.prototype.call);
    function callListener(t, cb, event) {
        try {
            // The call of install time: a page that replaces
            // Function.prototype.call does not stand between the engine's
            // dispatch and a listener.
            return typeof cb === 'function' ? callNow(cb, t, event) : cb.handleEvent(event);
        } catch (e) {
            try { __rk.reportError(e); } catch (_) {}
        }
    }
    function callOnError(t, h, event) {
        try {
            return callNow(h, t, event.message, event.filename, event.lineno, event.colno, event.error);
        } catch (e) {
            try { __rk.reportError(e); } catch (_) {}
        }
    }
    // An element's on<type> handler (HTML §8.1.8): the value assigned through
    // its `on<type>` IDL attribute (`null` included, which turns the handler
    // off), else its `on<type>` content attribute. The attribute's text is
    // the body of `function (event)`, with the element and its document in
    // scope, compiled on first use and again when the text changes. A body
    // that does not compile is logged once and is no handler. Setting or
    // removing the attribute drops the assigned value: the attribute is the
    // handler again (§8.1.8.1 "event handler content attributes").
    var INLINE = new WeakMap(), ASSIGNED = new WeakMap();
    // `src` as the body of `function (event)`, with `el`'s document and
    // then `el` in scope (§8.1.8.1 "getting the current value of the event
    // handler", the scope chain). `this` is whatever the handler is called
    // on: the element, or the Window for a body handler the Window owns.
    //
    // Known deviation (L2): for a handler the Window owns (`<body onload>`,
    // `<frameset onresize>`), the spec's scope chain is the global only: its
    // "element" is null, so neither the document nor the body is put in
    // scope (§8.1.8.1 steps 10 and 12 apply to an element's handler). This
    // compiles every attribute the same way, with the document and the
    // element in scope, so `<body onload="getElementById(...)">` resolves
    // where the spec would throw. Kept deliberately: nothing on the web
    // depends on the throw, and one compile path is one place to get right.
    function compileHandler(src, el) {
        try {
            // A handler attribute is the document's code, whoever set it:
            // its text first, then the one step that compiles it.
            var body = 'return function (event) { with (__rk_doc) { with (__rk_el) {\n' + src + '\n} } };';
            return __rk.compileDocumentFunction('__rk_doc,__rk_el', body)(el.ownerDocument || {}, el);
        } catch (e) {
            try { __rk.reportError(e); } catch (_) {}
            return null;
        }
    }
    function inlineHandler(t, type) {
        var src = t.getAttribute('on' + type);
        return src === null ? null : inlineFrom(t, type, src);
    }
    // The handler `src` is as `t`'s `on<type>` attribute, compiled once.
    function inlineFrom(t, type, src) {
        var all = INLINE.get(t);
        if (!all) { all = {}; INLINE.set(t, all); }
        var c = all[type];
        if (!c || c.src !== src) c = all[type] = { src: src, fn: compileHandler(src, t) };
        return c.fn;
    }
    // HTML §8.1.8.1 "determine the target of an event handler", for a
    // Window type (WINDOW_HANDLER_TYPES) on an element:
    //   1. not a body or frameset element: the element itself;
    //   3. a body/frameset whose node document is not the active document
    //      (a DOMParser body: no Window): null, so the getter answers null
    //      and the setter does nothing;
    //   4. else the Window: the body's or frameset's node document is the
    //      global document, attached to it or not (`createElement('body')`
    //      forwards too; the spec never asks whether it is THE body element).
    function isBodyOrFrameset(t) {
        if (t.nodeType !== 1) return false;
        var n = t.localName;
        return n === 'body' || n === 'frameset';
    }
    function windowHandlerTarget(t) {
        return isBodyOrFrameset(t) ? (t.ownerDocument === doc ? g : null) : t;
    }
    function handlerOf(t, type) {
        if (PLAIN.has(t)) return undefined;
        // A body or frameset has no handler of its own for a Window type,
        // assigned or inline: the target is the Window (its handler runs
        // when the event reaches the Window) or nothing.
        if (WINDOW_HANDLER_TYPES[type] && isBodyOrFrameset(t)) return undefined;
        var a = ASSIGNED.get(t);
        if (a && type in a) return a[type];
        if (!t[SLOT] || t.nodeType !== 1) return t['on' + type];
        // A type with no accessor (a custom event) is still a data property.
        if (!HANDLER_TYPES[type]) {
            var h = t['on' + type];
            if (h !== undefined) return h;
        }
        return inlineHandler(t, type);
    }
    function attributeChanged(t, name) {
        // The native setAttr/removeAttr lowercases the name for HTML elements
        // (`OnClick` is the `onclick` attribute), so compare the lowercased
        // name here too. Lowercasing unconditionally is fine: SVG's on<type>
        // attributes are lowercase already, so a case-sensitive SVG name
        // either is lowercase or names no handler attribute at all.
        name = String(name);
        // `on` in either case, asked before lowering the name (most names are not handlers).
        var n0 = name[0], n1 = name[1];
        if (name.length > 2 && (n0 === 'o' || n0 === 'O') && (n1 === 'n' || n1 === 'N')) {
            name = name.toLowerCase();
            var type = name.slice(2), a = ASSIGNED.get(t);
            if (a) delete a[type];
            // The body's attribute for a Window type is the Window's
            // handler: it replaces what was assigned to `window.on<type>`.
            // Only the body element's attribute (L1): the Window's getter
            // reads `doc.body`'s attribute lazily, so a detached body's
            // attribute, which the spec would also compile into the
            // Window's handler, is left alone rather than dropping the
            // assigned handler for nothing.
            if (WINDOW_HANDLER_TYPES[type] && t === doc.body && windowHandlerTarget(t) === g) {
                delete WINDOW_ASSIGNED[type];
            }
        }
    }
    // HTML §8.1.8.2 GlobalEventHandlers: `on<type>` is an accessor on
    // HTMLElement.prototype (and SVGElement.prototype), not a data property
    // of each element. The getter answers the assigned value, else the
    // compiled content attribute, else null; the setter keeps a function or
    // object and makes anything else null. Script that wraps the native
    // accessor needs the descriptor to exist: a script-retry shim takes
    // `Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'onerror')` and
    // calls `o.get.call(this)` / `o.set.call(this, fn)` from its own, and
    // without it every later `onerror` read and write on any element threw,
    // which stopped Next's bootstrap and webpack's chunk loader at the first
    // `<script src>` they inserted.
    var HANDLER_TYPES = Object.create(null);
    ('abort animationcancel animationend animationiteration animationstart auxclick beforeinput ' +
     'beforematch beforetoggle blur cancel canplay canplaythrough change click close contextlost ' +
     'contextmenu contextrestored copy cuechange cut dblclick drag dragend dragenter dragleave ' +
     'dragover dragstart drop durationchange emptied ended error focus formdata gotpointercapture ' +
     'input invalid keydown keypress keyup load loadeddata loadedmetadata loadstart ' +
     'lostpointercapture mousedown mouseenter mouseleave mousemove mouseout mouseover mouseup ' +
     'paste pause play playing pointercancel pointerdown pointerenter pointerleave pointermove ' +
     'pointerout pointerover pointerrawupdate pointerup progress ratechange reset resize scroll ' +
     'scrollend securitypolicyviolation seeked seeking select selectionchange selectstart ' +
     'slotchange stalled submit suspend timeupdate toggle touchcancel touchend touchmove ' +
     'touchstart transitioncancel transitionend transitionrun transitionstart volumechange ' +
     'waiting wheel').split(' ').forEach(function (type) {
        HANDLER_TYPES[type] = true;
        var desc = {
            configurable: true, enumerable: true,
            get: function () { return elementHandlerGet(this, type); },
            set: function (v) { elementHandlerSet(this, type, v); }
        };
        Object.defineProperty(HTMLElement.prototype, 'on' + type, desc);
        Object.defineProperty(g.SVGElement.prototype, 'on' + type, desc);
    });
    function elementHandlerGet(t, type) {
        var a = ASSIGNED.get(t);
        if (a && type in a) return a[type];
        return t[SLOT] && t.nodeType === 1 ? inlineHandler(t, type) : null;
    }
    function handlerValue(v) {
        return typeof v === 'function' || (typeof v === 'object' && v !== null) ? v : null;
    }
    function elementHandlerSet(t, type, v) {
        if (wheelType(type) && typeof v === 'function') { wheelHandler = true; if (type === 'mousewheel') mousewheelHandler = true; noteWheel(); }
        var a = ASSIGNED.get(t);
        if (!a) { a = Object.create(null); ASSIGNED.set(t, a); }
        a[type] = handlerValue(v);
    }
    // HTML §8.1.8.2.1, event handlers on body and frameset elements: the
    // WindowEventHandlers set (onhashchange, onpopstate, onstorage, ...) and
    // the Window-reflecting set (onload, onresize, onscroll, onerror,
    // onfocus, onblur) on the body element are the Window's handlers. The
    // IDL attributes on HTMLBodyElement.prototype (and the frameset's)
    // forward to `window.on<type>` for any body or frameset of the global
    // document (windowHandlerTarget), and `<body onload="...">` is the
    // Window's `load` handler: compiled from the attribute with the
    // document and the body in scope, the Window as `this`, and run by the
    // Window's `load` dispatch (which is where `load` fires; older pages
    // put their start-up there). A frameset document's body element is its
    // frameset (§3.1.3), so `<frameset onload>` is the same. A body in
    // another document has no Window to forward to: null, no assignment.
    //
    // `window.on<type>` for these types is an accessor on the Window over
    // one slot: the value assigned (through the Window or the body), else
    // the body element's compiled content attribute, else null. Setting or
    // removing the body element's attribute drops the assigned value, as
    // on elements. The Window's dispatch reads `window['on' + type]`, so it
    // sees the slot. The other on<type> on the Window stay data properties.
    var WINDOW_HANDLER_TYPES = Object.create(null), WINDOW_ASSIGNED = Object.create(null);
    ('afterprint beforeprint beforeunload blur error focus hashchange languagechange load message ' +
     'messageerror offline online pagehide pagereveal pageshow pageswap popstate rejectionhandled ' +
     'resize scroll storage unhandledrejection unload').split(' ').forEach(function (type) {
        WINDOW_HANDLER_TYPES[type] = true;
        var name = 'on' + type;
        Object.defineProperty(g, name, {
            configurable: true, enumerable: true,
            get: function () {
                if (type in WINDOW_ASSIGNED) return WINDOW_ASSIGNED[type];
                var body = doc.body;
                return body ? inlineHandler(body, type) : null;
            },
            set: function (v) { WINDOW_ASSIGNED[type] = handlerValue(v); }
        });
        // §8.1.8.2 getter step 2 / setter step 2: a null target answers
        // null and takes no assignment.
        var desc = {
            configurable: true, enumerable: true,
            get: function () {
                var target = windowHandlerTarget(this);
                return target === g ? g[name] : target === null ? null : elementHandlerGet(this, type);
            },
            set: function (v) {
                var target = windowHandlerTarget(this);
                if (target === g) g[name] = v; else if (target !== null) elementHandlerSet(this, type, v);
            }
        };
        Object.defineProperty(g.HTMLBodyElement.prototype, name, desc);
        Object.defineProperty(g.HTMLFrameSetElement.prototype, name, desc);
    });
    function invoke(t, event, phase, capture) {
        mapSet(EV_CURRENT, event, t);
        mapSet(EV_PHASE, event, phase);
        // A listener sees the target as its own tree sees it: a node inside a
        // shadow tree is the host to a listener outside.
        if (HOOKS.retarget && event[ORIGIN]) mapSet(EV_TARGET, event, HOOKS.retarget(event[ORIGIN], t));
        var type = evType(event);
        var all = LISTENERS.get(t), list = all && all[type];
        // A replayed `load` (below) reaches only what its record holds.
        var only = ONLY !== null && ONLY.event === event ? ONLY : null;
        // The legacy type, at a target with no listener or handler for the
        // event's own (the user's wheel as `mousewheel`). The engine's
        // event changes type in the type map only, never by assignment.
        var legacy = mapGet(LEGACY_TYPE, event);
        if (legacy && mapHas(EVTYPE, event) && !(list && list.length) && typeof handlerOf(t, type) !== 'function' &&
            ((all && all[legacy] && all[legacy].length) || typeof handlerOf(t, legacy) === 'function')) {
            mapSet(EVTYPE, event, legacy);
            try { invoke(t, event, phase, capture); } finally { mapSet(EVTYPE, event, type); }
            return;
        }
        if (list) {
            list = list.slice();
            for (var i = 0; i < list.length && !event[STOP_NOW]; i++) {
                var l = list[i];
                if (l.removed || l.capture !== capture) continue;
                if (only && !setHas(only.entries, l)) continue;
                if (l.once) t.removeEventListener(type, l.cb, { capture: l.capture });
                var passiveBefore = passiveNow;
                passiveNow = l.passive === true ? event : null;
                try { callListener(t, l.cb, event); } finally { passiveNow = passiveBefore; }
            }
        }
        // The on<type> handler runs with the non-capture listeners.
        var h = capture || event[STOP_NOW] ? null
            : only ? (t === only.target ? only.handler : null) : handlerOf(t, type);
        if (typeof h !== 'function') return;
        // `window.onerror` (HTML §8.1.8.2, the OnErrorEventHandler): for an
        // ErrorEvent at the Window it is called with the event's message,
        // filename, lineno, colno and error, and `true` back cancels.
        var special = t === g && type === 'error' && PROTOS.ErrorEvent !== undefined && isProto(PROTOS.ErrorEvent, event);
        var back = special ? callOnError(t, h, event) : callListener(t, h, event);
        if (back === (special ? true : false) && setHas(CANCELABLE, event)) {
            if (isRealEvent(event)) setAdd(CANCELED, event); else event.defaultPrevented = true;
        }
    }
    // A node's parent, then document's is window (except for `load`, which
    // never reaches window from a node).
    function eventParent(n, event) {
        if (n === g) return null;
        if (n === doc) return evType(event) === 'load' ? null : g;
        if (!n[SLOT]) return null;
        var p = n.parentNode;
        // A composed event goes on from a shadow root to its host.
        return p || (HOOKS.shadowParent ? HOOKS.shadowParent(n, event) : null);
    }
    // What a `load` dispatched at `target` right now would call: its own
    // listeners and handler, and the capturing listeners above it (`load`
    // does not bubble and stops at the document). `null` when nothing would
    // hear it. For web_frames.js: Chrome fires a blank frame's `load` inside
    // the frame's insertion; the engine's comes later and is replayed to
    // this record, so what was added, or put in a handler's place, after
    // the insertion does not hear it. `parsed`: the frame was the parser's,
    // and at its insertion only its `onload` attribute could be on it.
    //
    // Read from the engine's own records: the handler as assigned or as the
    // `onload` attribute in the node store, the listener registry, and the
    // node store's parent, in a walk with an end.
    var ONLY = null, LOAD_WALK_MAX = 65536;
    HOOKS.loadSnapshot = function (target, parsed) {
        var t = thisTarget(target), s = t[SLOT], a = parsed ? undefined : ASSIGNED.get(t);
        var snap = objCreate(null), any = false;
        snap.target = t; snap.event = null; snap.handler = null; snap.src = null; snap.entries = new WeakSetNow();
        if (a && 'load' in a) { if (typeof a.load === 'function') snap.handler = a.load; }
        else if (s && s.gen === gen) {
            var src = N.attr(s.gen, s.id, 'onload');
            // Compiled when the `load` is replayed, as a dispatch does.
            if (src !== null) snap.src = src;
        }
        for (var n = t, steps = 0; !parsed && n && n !== g && steps < LOAD_WALK_MAX; steps++) {
            var all = LISTENERS.get(n), list = all && all.load;
            for (var i = 0; list && i < list.length; i++) {
                if (n === t || list[i].capture) { setAdd(snap.entries, list[i]); any = true; }
            }
            n = n === doc || !n[SLOT] ? null : related(n, 'parent');
        }
        return any || snap.src !== null || typeof snap.handler === 'function' ? snap : null;
    };
    // Dispatch `event` at the record's target to what the record holds (a
    // listener removed since is not called).
    HOOKS.loadReplay = function (snap, event) {
        var was = ONLY;
        if (snap.src !== null) { snap.handler = inlineFrom(snap.target, 'load', snap.src); snap.src = null; }
        snap.event = event; ONLY = snap;
        try { return dispatchTo(snap.target, event); } finally { ONLY = was; snap.event = null; }
    };
    // Activation behaviour (DOM §2.9 "dispatch", steps 5 and 11): a click
    // (the user's, `el.click()`, or a MouseEvent script dispatches) checks a
    // checkbox before its listeners run and clicks a label's control after.
    // `activation(target)` is web_forms.js's; it answers the function to
    // call with "was not cancelled" once the listeners have run, or null.
    var activation = null, SYNTHETIC_CLICK = Symbol('rustkit.click');
    function activationFor(t, event) {
        if (!activation || evType(event) !== 'click' || !t[SLOT]) return null;
        if (!event[SYNTHETIC_CLICK] && !isMouseClick(event)) return null;
        return activation(t, event);
    }
    EventTarget.prototype.dispatchEvent = function (event) {
        // An Event: made by one of the Event constructors or by the engine
        // (both stamp it). Anything else, a look-alike or a Proxy of an
        // event included, is a TypeError, as in Chrome.
        if (event == null || typeof event !== 'object' || !setHas(STAMPED, event)) {
            throw new TypeError("Failed to execute 'dispatchEvent' on 'EventTarget': " +
                "parameter 1 is not of type 'Event'.");
        }
        // An event already being dispatched cannot be dispatched again.
        if (setHas(DISPATCHING, event)) {
            throw new DOMException("Failed to execute 'dispatchEvent' on 'EventTarget': The event is already being dispatched.", 'InvalidStateError');
        }
        // Only the engine's own event, on its one dispatch, is trusted.
        stamp(event);
        if (setHas(UNDISPATCHED, event)) setDrop(UNDISPATCHED, event); else setDrop(TRUSTED, event);
        var t = thisTarget(this), path = [], i;
        for (var n = t; n; n = eventParent(n, event)) path.push(n);
        mapSet(EV_TARGET, event, t);
        event[ORIGIN] = t;
        event[PATH] = path;
        var activated = activationFor(t, event);
        setAdd(DISPATCHING, event);
        try {
            for (i = path.length - 1; i > 0 && !event[STOP]; i--) invoke(path[i], event, 1, true);
            if (!event[STOP]) invoke(t, event, 2, true);
            if (!event[STOP]) invoke(t, event, 2, false);
            if (event.bubbles) {
                for (i = 1; i < path.length && !event[STOP]; i++) invoke(path[i], event, 3, false);
            }
        } finally {
            setDrop(DISPATCHING, event);
            event[STOP] = event[STOP_NOW] = false;
            mapSet(EV_CURRENT, event, null);
            mapSet(EV_PHASE, event, 0);
            // After dispatch the target is the outermost host (the node
            // itself when it is in the document), and there is no path.
            mapSet(EV_TARGET, event, HOOKS.retarget ? HOOKS.retarget(t, g) : t);
            event[ORIGIN] = null;
            event[PATH] = null;
        }
        // The canceled flag, not a property a listener can redefine; an
        // object that is not an Event keeps its own.
        var canceled = isRealEvent(event) ? setHas(CANCELED, event) : event.defaultPrevented === true;
        if (activated) activated(!canceled);
        return !canceled;
    };
    // The engine's own dispatches go through this, whatever a page has put
    // in place of `dispatchEvent` since.
    var dispatchTo = uncurry(EventTarget.prototype.dispatchEvent);
    __rk.dispatchTo = dispatchTo;
    // Promise rejection events (HTML §8.1.7.4 "unhandled promise
    // rejections"). The runtime calls this when a turn's microtasks have
    // run (`JsRuntime::set_rejection_events`): once for each promise
    // rejected with no handler (`unhandledrejection`, cancelable), and once
    // more for one that got its handler after that (`rejectionhandled`).
    // The event is the engine's own: trusted, at the Window, built and
    // dispatched with nothing a page can replace. `false` back means a
    // listener canceled it, which keeps the rejection out of the console.
    __rk.fireRejection = function (type, promise, reason) {
        var init = objCreate(null);
        init.cancelable = type === 'unhandledrejection';
        init.promise = promise;
        init.reason = reason;
        var e = make('PromiseRejectionEvent', type, init);
        trust(e);
        return dispatchTo(g, e);
    };

    // The `error` event for an uncaught exception (HTML §8.1.4.6 "report
    // an exception"). The runtime calls this with the exception before it
    // is reported any further (`JsRuntime::set_error_events`), and with
    // what the event is to say, which the runtime decides from the scripts
    // the engine registered (`JsRuntime::register_script`): the thrown
    // value, the message, the file, line and column. Script errors from
    // another origin are reported without detail, as the HTML standard's
    // muted errors: the value is null, the message "Script error.", no
    // file, no position. Nothing here reads a URL or decides that.
    // The event is the engine's own: trusted, cancelable, at the Window,
    // built and dispatched with nothing a page can replace. `false` back
    // means a listener canceled it, which keeps the exception out of the
    // console.
    __rk.fireError = function (value, message, file, line, column) {
        var init = objCreate(null);
        init.cancelable = true;
        init.message = message;
        init.filename = file;
        init.lineno = line;
        init.colno = column;
        init.error = value;
        var e = make('ErrorEvent', 'error', init);
        trust(e);
        return dispatchTo(g, e);
    };
    // `reportError(e)` (HTML §8.1.4.6): `e` is reported as an uncaught
    // exception is, the `error` event first.
    defineOwn(g, 'reportError', {
        value: function reportError(e) {
            if (arguments.length < 1) {
                throw new TypeError("Failed to execute 'reportError' on 'Window': 1 argument required, but only 0 present.");
            }
            // `true`: reported for the page, by the code that called this.
            __rk.reportError(e, true);
        },
        writable: true, enumerable: true, configurable: true
    });

    function Event(type, init) {
        if (!(this instanceof Event)) {
            throw new TypeError("Failed to construct 'Event': Please use the 'new' operator.");
        }
        if (arguments.length < 1) {
            throw new TypeError("Failed to construct 'Event': 1 argument required, but only 0 present.");
        }
        init = init || {};
        // Own properties, defined (not assigned): what a page puts on the
        // prototypes is never run for them. The type is kept here too, and
        // `cancelable` in a private set (WHL3).
        mapSet(EVTYPE, this, String(type));
        defineOwn(this, 'type', getterDesc(TYPE_GET));
        own(this, 'bubbles', !!init.bubbles);
        setCancelable(this, !!init.cancelable);
        own(this, 'composed', !!init.composed);
        stamp(this);
        // On performance.now()'s clock (ms from the time origin), as Chrome's DOMHighResTimeStamp.
        own(this, 'timeStamp', typeof __rk.perfNow === 'function' ? __rk.perfNow() : Date.now());
    }
    var TYPE_GET = Object.getOwnPropertyDescriptor({ get type() { return mapGet(EVTYPE, this); } }, 'type').get;
    function own(o, k, v) { defineOwn(o, k, { value: v, writable: true, enumerable: true, configurable: true }); }
    // The legacy initializers; they do nothing during dispatch.
    // Re-initialising clears the canceled flag (DOM §2.2 "initialize");
    // during dispatch it does nothing.
    Event.prototype.initEvent = function (type, bubbles, cancelable) {
        if (setHas(DISPATCHING, this)) return;
        if (mapHas(EVTYPE, this)) mapSet(EVTYPE, this, String(type)); else this.type = String(type);
        own(this, 'bubbles', !!bubbles);
        setCancelable(this, !!cancelable);
        setDrop(CANCELED, this);
    };
    function cancel(ev) { if (setHas(CANCELABLE, ev) && passiveNow !== ev) setAdd(CANCELED, ev); }
    // `cancelable` is read-only, as in Chrome for Testing 148 (assignment
    // does nothing; strict code throws).
    Object.defineProperty(Event.prototype, 'cancelable', {
        get: function () { return setHas(CANCELABLE, this); }, enumerable: true, configurable: true
    });
    Event.prototype.preventDefault = function () { cancel(this); };
    // Read-only (no setter: an assignment does nothing, and throws in
    // strict code); the canceled flag above (Chrome for Testing 148).
    Object.defineProperty(Event.prototype, 'defaultPrevented', {
        get: function () { return setHas(CANCELED, this); }, enumerable: true, configurable: true
    });
    // The legacy alias (DOM §2.2): false cancels, as preventDefault() does.
    Object.defineProperty(Event.prototype, 'returnValue', {
        get: function () { return !setHas(CANCELED, this); },
        set: function (v) { if (!v) cancel(this); },
        enumerable: true, configurable: true
    });
    // The nodes the event is going through, target first; empty outside dispatch.
    Event.prototype.composedPath = function () { return this[PATH] ? this[PATH].slice() : []; };
    Event.prototype.stopPropagation = function () { this[STOP] = true; };
    // DOM §2.2: the legacy alias. Reading it is the stop-propagation flag;
    // setting it true is stopPropagation(), setting it false does nothing.
    Object.defineProperty(Event.prototype, 'cancelBubble', {
        get: function () { return this[STOP] === true; },
        set: function (v) { if (v) this[STOP] = true; },
        enumerable: true, configurable: true
    });
    Event.prototype.stopImmediatePropagation = function () { this[STOP] = this[STOP_NOW] = true; };
    getter(Event.prototype, 'srcElement', function () { return this.target; });
    getter(Event.prototype, 'target', function () { return mapHas(EV_TARGET, this) ? mapGet(EV_TARGET, this) : null; });
    getter(Event.prototype, 'currentTarget', function () { return mapHas(EV_CURRENT, this) ? mapGet(EV_CURRENT, this) : null; });
    getter(Event.prototype, 'eventPhase', function () { return mapHas(EV_PHASE, this) ? mapGet(EV_PHASE, this) : 0; });
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
    CustomEvent.prototype.initCustomEvent = function (type, bubbles, cancelable, detail) {
        if (setHas(DISPATCHING, this)) return;
        this.initEvent(type, bubbles, cancelable);
        this.detail = detail === undefined ? null : detail;
    };
    g.Event = Event;
    // Defined by index: no array method of another window's realm runs.
    fixed(EVENT_PROTOS, EVENT_PROTOS.length, Event.prototype);
    g.CustomEvent = CustomEvent;
    __rk.keepCtor('Event', Event);
    __rk.keepCtor('CustomEvent', CustomEvent);

    // DOMParser (HTML §8.5.1), for 'text/html' only: the XML types need an
    // XML parser, which RustKit does not have yet.
    var DOMParser = function DOMParser() {
        if (!(this instanceof DOMParser)) {
            throw new TypeError("Failed to construct 'DOMParser': Please use the 'new' operator.");
        }
    };
    DOMParser.prototype.parseFromString = function (str, type) {
        type = String(type);
        if (type === 'text/xml' || type === 'application/xml' ||
            type === 'application/xhtml+xml' || type === 'image/svg+xml') {
            __rk.unsupported('DOMParser', 'parseFromString(XML)');
            throw new DOMException("Failed to execute 'parseFromString' on 'DOMParser': " +
                "XML documents are not supported yet.", 'NotSupportedError');
        }
        if (type !== 'text/html') {
            throw new TypeError("Failed to execute 'parseFromString' on 'DOMParser': The provided value '" +
                type + "' is not a valid enum value of type DOMParserSupportedType.");
        }
        return create('document', String(str), 'parseFromString');
    };
    Object.defineProperty(DOMParser.prototype, Symbol.toStringTag, { value: 'DOMParser' });
    g.DOMParser = DOMParser;
    // HTML §6.4.4 "click()": nothing on a disabled form control; otherwise a
    // synthetic pointer event, which in Chrome for Testing 148 is an
    // untrusted, composed PointerEvent with detail 0, button 0, pointerId -1
    // and an empty pointerType, at 0,0, with the window as its view.
    // `clickElement` is that click: script's `click()`, and the click a
    // label's activation gives its control (trusted when the label's was).
    function clickElement(el, trusted) {
        if (el.disabled === true && /^(button|input|select|textarea|fieldset|optgroup|option)$/.test(el.localName)) return;
        var init = { bubbles: true, cancelable: true, composed: true, view: g, detail: 0, button: 0, buttons: 0,
                     pointerId: -1, pointerType: '' };
        var e = make(__rk.hasCtor('PointerEvent') ? 'PointerEvent' : 'MouseEvent', 'click', init);
        // An own property, defined: no setter on a prototype sees the event.
        fixed(e, SYNTHETIC_CLICK, true);
        if (trusted) trust(e);
        // A link's activation behaviour (HTML §4.6.4): the user's click is
        // followed by the engine, script's own click() here.
        if (dispatchTo(el, e) && __rk.followLink) __rk.followLink(el);
    }
    HTMLElement.prototype.click = function click() { clickElement(this, false); };

    // The global `document` becomes the Document wrapper.
    var doc = g.document;
    Object.setPrototypeOf(doc, Document.prototype);
    // The stub's own factories would shadow the Rust-backed ones.
    delete doc.createElement;
    delete doc.createTextNode;
    delete doc.createDocumentFragment;
    // Document and window trade the lifecycle's per-object listener lists
    // for the shared EventTarget, so element events bubble up to them.
    ['addEventListener', 'removeEventListener', 'dispatchEvent'].forEach(function (k) {
        delete doc[k];
        g[k] = EventTarget.prototype[k];
    });

    // For web_forms.js, which deletes it: the engine is told of a
    // checkedness change, and dispatch is given the activation behaviours.
    g.__rkFormInternals = {
        noteChecked: function (el, v) {
            var s = el != null ? el[SLOT] : undefined;
            if (s && s.gen === gen) N.checked(s.gen, s.id, v);
        },
        noteSubmit: function (form, submitter) {
            var s = form != null ? form[SLOT] : undefined, b = submitter != null ? submitter[SLOT] : undefined;
            if (s && s.gen === gen) N.submit(s.gen, s.id, b && b.gen === gen ? b.id : null);
        },
        setActivation: function (f) { activation = f; },
        // The engine's own click, reset and dispatch, for the activation
        // behaviours: never the page-visible methods of those names.
        click: clickElement,
        reset: resetForm,
        dispatch: function (target, event) { return dispatchTo(target, event); },
        setImplicitSubmit: function (f) { implicitSubmit = f; },
        setPickOption: function (f) { pickOption = f; }
    };
    // For web_history.js, which deletes it: script asks for a navigation.
    g.__rkNavInternals = { navigate: function (url, replace) { N.navigate(String(url), !!replace); } };
    // For web_scripts.js, which deletes it: a script element to fetch and run.
    g.__rkScriptInternals = {
        fetch: function (el, url, inOrder) {
            var s = el[SLOT];
            if (s && s.gen === gen) N.insertedScript(s.id, String(url), !!inOrder);
        },
        // `document.currentScript` while an inserted inline script runs;
        // answers what it was.
        current: function (el) { var was = currentScript; currentScript = el; return was; }
    };
    // web_cssom.js publishes constructed-sheet text through this. The
    // wrapper's slot is private to this closure.
    __rk.setAdoptedStyles = function (node, json) {
        var s = node != null ? node[SLOT] : undefined;
        if (!s || s.gen !== gen) return;
        N.setAdopted(s.gen, s.id, String(json));
    };
    // web_cssom.js: CSS.registerProperty(); '' or the DOMException name.
    __rk.registerProperty = function (name, syntax, inherits, initial) {
        return N.registerProperty(name, syntax, inherits, initial);
    };
    // web_cssom.js: a <style>'s sheet.disabled, for the sheet it owns now.
    __rk.setSheetDisabled = function (node, on) {
        var s = node != null ? node[SLOT] : undefined;
        if (!s || s.gen !== gen) return;
        N.setSheetDisabled(s.gen, s.id, !!on);
    };
    // web_cssom.js: which sheet a <style> owns now (a number that changes
    // when its children change or it moves); -1 for a node of another
    // document.
    __rk.sheetGeneration = function (node) {
        var s = node != null ? node[SLOT] : undefined;
        if (!s || s.gen !== gen) return -1;
        return N.info(s.gen, s.id, 'sheetGen');
    };

    // For dom/editing.js, which deletes it.
    g.__rkEditInternals = {
        wrap: wrap, slotOf: slotOf, gen: function () { return gen; },
        selectionOf: function (el) { return selectionOf(el); },
        setEngineSelection: function (f) { engineSelection = f; },
        setSelection: function (el, start, end, dir) {
            var s = selectionOf(el);
            s.start = start; s.end = end; s.dir = dir;
        }
    };

    // For node_apis.js, which runs next and deletes it.
    g.__rkNodeInternals = {
        wrap: wrap, slotOf: slotOf, gen: function () { return gen; },
        local: function (id) { return N.info(gen, id, 'local'); },
        setFragmentProto: function (f) { fragmentProto = f; },
        extendElementProto: function (f) {
            var base = elementProto;
            elementProto = function (id) { return f(id, base); };
        }
    };

    __rk.domReset = function (newGen) {
        gen = newGen;
        cache = new Map();
        var root = N.root(gen, 'document');
        Object.defineProperty(doc, SLOT, { value: slotRecord(root), configurable: true });
        if (typeof root === 'number') cache.set(root, doc);
    };
    __rk.domReset(0);
})(globalThis);
"#;

#[cfg(test)]
mod tests {
    use crate::{DomBindings, DomDirty, MutationKind};
    use rustkit_dom::Document;
    use rustkit_js::{JsRuntime, JsValue};
    use std::rc::Rc;

    const PAGE: &str = r#"<!DOCTYPE html><html><head><title>T</title></head>
<body><div id="main" class="box"><p class="x">Hello, <b>world</b>!</p><!--c--><p class="x">Two</p></div>
<p id="outside" class="x">Out</p></body></html>"#;

    fn bound(html: &str) -> (DomBindings, Rc<Document>) {
        let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
        let document = Rc::new(Document::parse_html(html).unwrap());
        bindings.set_document(document.clone()).unwrap();
        (bindings, document)
    }

    fn eval_string(bindings: &DomBindings, script: &str) -> String {
        match bindings.evaluate(script).unwrap() {
            JsValue::String(s) => s,
            other => panic!("{script} evaluated to {other:?}"),
        }
    }

    #[test]
    fn clone_node_copies_into_fresh_detached_nodes() {
        let (b, doc) = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'), s = m.cloneNode(), d = m.cloneNode(true); \
                 var t = m.firstChild.firstChild.cloneNode(); \
                 [s.parentNode === null, s.childNodes.length, s.className, s.id, \
                  d.childNodes.length, d.textContent, d.firstChild !== m.firstChild, \
                  d.firstChild.className, d.innerHTML === m.innerHTML, \
                  t.data, t.parentNode === null, document.getElementById('main') === m, \
                  document.querySelectorAll('.box').length].join('|')"
            ),
            "true|0|box|main|3|Hello, world!Two|true|x|true|Hello, |true|true|1"
        );
        assert_eq!(
            b.take_dirty(),
            DomDirty::Clean,
            "a detached clone marks nothing"
        );
        assert_eq!(
            eval_string(
                &b,
                "document.body.appendChild(d); \
                 var r = [document.querySelectorAll('.box').length]; \
                 try { document.cloneNode(); } catch (e) { r.push(e.name); } r.join('|')"
            ),
            "2|NotSupportedError"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
        // The clone is in the Rust tree the cascade reads.
        let body = doc.body().unwrap();
        assert_eq!(
            body.last_child().unwrap().text_content(),
            "Hello, world!Two"
        );
    }

    #[test]
    fn attribute_names_and_node_equality() {
        let (b, _) = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'), r = []; m.setAttribute('data-z', '1'); \
                 var i = document.createElement('i'); \
                 r.push(m.getAttributeNames().join(','), i.getAttributeNames().length, \
                        i.hasAttributes(), m.hasAttributes()); \
                 var ps = document.querySelectorAll('.x'), c = m.cloneNode(true); \
                 r.push(ps[0].isEqualNode(ps[1]), c.isEqualNode(m), m.isEqualNode(null), \
                        m.isSameNode(m), c.isSameNode(m)); \
                 c.lastChild.textContent = 'Changed'; r.push(c.isEqualNode(m)); \
                 var x = document.createElement('a'), y = document.createElement('a'); \
                 x.setAttribute('p', '1'); x.setAttribute('q', '2'); \
                 y.setAttribute('q', '2'); y.setAttribute('p', '1'); r.push(x.isEqualNode(y)); \
                 y.setAttribute('p', '3'); r.push(x.isEqualNode(y)); \
                 try { m.isEqualNode({}); } catch (e) { r.push(e.name); } \
                 r.join('|')"
            ),
            "id,class,data-z|0|false|true|false|true|false|true|false|false|true|false|TypeError"
        );
    }

    #[test]
    fn compare_document_position_orders_nodes() {
        let (b, _) = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'), o = document.getElementById('outside'), \
                     w = document.querySelector('b'); \
                 [m.compareDocumentPosition(o), o.compareDocumentPosition(m), \
                  m.compareDocumentPosition(w), w.compareDocumentPosition(m), \
                  m.compareDocumentPosition(m), document.compareDocumentPosition(w), \
                  Node.DOCUMENT_POSITION_FOLLOWING, m.DOCUMENT_POSITION_CONTAINED_BY].join('|')"
            ),
            "4|2|20|10|0|20|4|16"
        );
        // Disconnected nodes: flagged, and ordered one way consistently.
        assert_eq!(
            eval_string(
                &b,
                "var x = document.createElement('x'), p = m.compareDocumentPosition(x), \
                     q = x.compareDocumentPosition(m); \
                 [p & 33, q & 33, (p & 6) + (q & 6)].join('|')"
            ),
            "33|33|6"
        );
    }

    #[test]
    fn normalize_merges_adjacent_text_and_drops_empty_text() {
        let (b, _) = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'); m.textContent = ''; \
                 ['a', '', 'b'].forEach(function (s) { m.appendChild(document.createTextNode(s)); }); \
                 var i = document.createElement('i'); i.appendChild(document.createTextNode('')); \
                 m.appendChild(i); m.appendChild(document.createTextNode('c')); \
                 var first = m.firstChild; m.normalize(); \
                 [m.childNodes.length, m.firstChild === first, first.data, \
                  i.childNodes.length, m.lastChild.data].join('|')"
            ),
            "3|true|ab|0|c"
        );
    }

    #[test]
    fn insert_adjacent_html_element_and_text() {
        let (b, doc) = bound(PAGE);
        assert_eq!(
            eval_string(
                &b,
                "var m = document.getElementById('main'), r = []; \
                 m.insertAdjacentHTML('beforebegin', '<h1 id=h>T</h1>'); \
                 m.insertAdjacentHTML('afterBegin', '<span>s1</span><span>s2</span>'); \
                 m.insertAdjacentHTML('beforeend', 'end<em>!</em>'); \
                 m.insertAdjacentHTML('afterend', '<hr id=r>'); \
                 r.push(m.previousSibling.id, m.firstChild.textContent, \
                        m.firstChild.nextSibling.textContent, m.lastChild.tagName, \
                        m.lastChild.previousSibling.data, m.nextSibling.id); \
                 var e = document.createElement('u'), d = document.createElement('div'); \
                 r.push(m.insertAdjacentElement('afterend', e) === e, m.nextSibling === e, \
                        d.insertAdjacentElement('beforebegin', e) === null, \
                        e.parentNode === document.body); \
                 m.insertAdjacentText('afterbegin', 'txt'); r.push(m.firstChild.data); \
                 try { m.insertAdjacentHTML('middle', 'x'); } catch (x) { r.push(x.name); } \
                 try { document.documentElement.insertAdjacentHTML('beforebegin', 'x'); } \
                 catch (x) { r.push(x.name); } \
                 try { d.insertAdjacentHTML('afterend', 'x'); } catch (x) { r.push(x.name); } \
                 try { m.insertAdjacentElement('beforeend', 'str'); } catch (x) { r.push(x.name); } \
                 document.documentElement.insertAdjacentHTML('beforeend', '<p id=late>L</p>'); \
                 r.push(document.getElementById('late').parentNode === document.documentElement); \
                 r.join('|')"
            ),
            "h|s1|s2|EM|end|r|true|true|true|true|txt|\
             SyntaxError|NoModificationAllowedError|NoModificationAllowedError|TypeError|true"
        );
        assert_eq!(b.take_dirty(), DomDirty::Style);
        assert_eq!(
            doc.get_element_by_id("main").unwrap().text_content(),
            "txts1s2Hello, world!Twoend!"
        );
    }

    const FORM: &str = r#"<!DOCTYPE html><html><body><form id="f">
<input id="i" value="ab"><textarea id="t">hi</textarea><input id="c" type="checkbox">
<input id="h" type="HIDDEN" value="hv"><button id="b">Go</button></form></body></html>"#;

    #[test]
    fn text_control_value_is_a_dirty_value_the_engine_is_told_about() {
        let (b, doc) = bound(FORM);
        let raw = |id: &str| doc.get_element_by_id(id).unwrap().id.raw();
        assert_eq!(
            eval_string(
                &b,
                "var i = document.getElementById('i'), t = document.getElementById('t'), r = []; \
                 r.push(i instanceof HTMLInputElement, t instanceof HTMLTextAreaElement, \
                        document.getElementById('f') instanceof HTMLFormElement, \
                        i.value, i.defaultValue, i.type, t.value, t.defaultValue, t.type); \
                 i.value = 'x\\ny'; t.value = 'a\\r\\nb'; \
                 r.push(i.value, i.getAttribute('value'), t.value, t.textLength, t.textContent); \
                 r.join('|')"
            ),
            "true|true|true|ab|ab|text|hi|hi|textarea|xy|ab|a\nb|3|hi"
        );
        assert_eq!(b.take_dirty(), DomDirty::Layout, "a value write repaints");
        assert_eq!(
            b.take_value_writes(),
            vec![(raw("i"), "xy".to_string()), (raw("t"), "a\nb".to_string())]
        );
        assert!(b.take_value_writes().is_empty());
        assert_eq!(
            doc.get_element_by_id("i").unwrap().get_attribute("value"),
            Some("ab"),
            "the value attribute is the default, not the value"
        );
    }

    #[test]
    fn other_input_modes_and_reflected_control_attributes() {
        let (b, _doc) = bound(FORM);
        assert_eq!(
            eval_string(
                &b,
                "var c = document.getElementById('c'), h = document.getElementById('h'), r = []; \
                 r.push(c.value, h.type, h.value); c.value = 'yes'; h.value = 'hw'; \
                 r.push(c.getAttribute('value'), h.getAttribute('value')); \
                 var i = document.getElementById('i'); i.disabled = true; i.readOnly = true; \
                 i.name = 'q'; i.placeholder = 'p'; \
                 r.push(i.getAttribute('disabled'), i.hasAttribute('readonly'), i.name, \
                        i.placeholder, i.required, i.form.id, document.body.value); \
                 var f = document.getElementById('f'); \
                 r.push(f.elements.length, f.elements[4].id, f.method); \
                 i.focus(); i.blur(); r.join('|')"
            ),
            "on|hidden|hv|yes|hw||true|q|p|false|f||5|b|get"
        );
        assert!(
            b.take_value_writes().is_empty(),
            "attribute modes queue no value"
        );
    }

    #[test]
    fn created_controls_are_rust_backed() {
        let (b, doc) = bound(FORM);
        assert_eq!(
            eval_string(
                &b,
                "var i = document.createElement('INPUT'), t = document.createElement('textarea'); \
                 var f = document.createElement('form'); \
                 f.appendChild(i); f.appendChild(t); document.body.appendChild(f); \
                 i.id = 'made'; i.value = 'v'; \
                 [i instanceof HTMLInputElement, t instanceof HTMLTextAreaElement, \
                  f instanceof HTMLFormElement, i.parentNode === f, i.value, \
                  document.querySelector('#made') === i].join('|')"
            ),
            "true|true|true|true|v|true"
        );
        assert!(doc.get_element_by_id("made").is_some());
    }

    #[test]
    fn typed_text_reads_back_and_reset_or_navigate_drop_it() {
        let (b, doc) = bound(FORM);
        let i = doc.get_element_by_id("i").unwrap().id.raw();
        b.sync_control_value(i, "typed".to_string());
        assert_eq!(
            eval_string(&b, "document.getElementById('i').value"),
            "typed"
        );
        assert_eq!(
            b.take_dirty(),
            DomDirty::Clean,
            "a read or a sync marks nothing"
        );
        assert_eq!(
            eval_string(
                &b,
                "var f = document.getElementById('f'), n = 0; \
                 f.addEventListener('reset', function () { n++; }); \
                 document.getElementById('t').value = 'edited'; f.reset(); \
                 [n, document.getElementById('i').value, document.getElementById('t').value].join('|')"
            ),
            "1|ab|hi"
        );
        assert_eq!(b.take_dirty(), DomDirty::Layout);
        let t = doc.get_element_by_id("t").unwrap().id.raw();
        assert_eq!(
            b.take_value_writes(),
            vec![
                (t, "edited".to_string()),
                (i, "ab".to_string()),
                (t, "hi".to_string())
            ],
            "reset repaints each text control's default, and only those"
        );

        // A new document starts with no dirty values, even at a reused NodeId.
        b.sync_control_value(i, "stale".to_string());
        b.set_document(Rc::new(Document::parse_html(FORM).unwrap()))
            .unwrap();
        assert_eq!(eval_string(&b, "document.getElementById('i').value"), "ab");
        assert!(b.take_value_writes().is_empty());
    }

    // The relayout cause log: each dirtying write is counted by kind and
    // the first one names its node; a write that changes nothing is not.
    #[test]
    fn dirtying_writes_are_tallied_by_kind_with_the_first_node() {
        let (b, _doc) = bound(PAGE);
        assert!(b.take_script_mutations().is_empty());
        b.evaluate(
            "var m = document.getElementById('main'); \
             m.className = 'box';",
        )
        .unwrap();
        assert!(
            b.take_script_mutations().is_empty(),
            "an attribute set to its value is not a write"
        );
        b.evaluate(
            "var m = document.getElementById('main'); \
             m.setAttribute('data-x', '1'); \
             m.style.color = 'red'; \
             m.appendChild(document.createElement('span')); \
             document.getElementById('outside').textContent = 'changed'; \
             m.setAttribute('data-y', '2');",
        )
        .unwrap();
        let tally = b.take_script_mutations();
        assert_eq!(
            tally.first,
            Some((MutationKind::Attribute, "div#main".to_string()))
        );
        assert_eq!(tally.count(MutationKind::Attribute), 2);
        assert_eq!(tally.count(MutationKind::Style), 1);
        assert_eq!(tally.count(MutationKind::ChildList), 1);
        assert_eq!(tally.count(MutationKind::Text), 1);
        assert_eq!(tally.count(MutationKind::FormValue), 0);
        assert_eq!(tally.total(), 5);
        assert!(b.take_script_mutations().is_empty(), "taking resets it");
        // The dirty bucket is unchanged by the tally.
        assert_eq!(b.take_dirty(), DomDirty::Style);
    }

    /// #655 FormValue path: `.value` / `.checked` writes must tally as
    /// `form_value` with the control's label, or W7-C mis-attributes Input
    /// vs script_mutation. Reads must not increment the tally.
    #[test]
    fn form_value_writes_are_tallied_with_the_control_label() {
        let (b, _doc) = bound(FORM);
        assert!(b.take_script_mutations().is_empty());
        b.evaluate("var _ = document.getElementById('i').value;")
            .unwrap();
        assert!(
            b.take_script_mutations().is_empty(),
            "a value read is not a FormValue write"
        );

        b.evaluate("document.getElementById('i').value = 'typed';")
            .unwrap();
        let tally = b.take_script_mutations();
        assert_eq!(
            tally.first,
            Some((MutationKind::FormValue, "input#i".to_string()))
        );
        assert_eq!(tally.count(MutationKind::FormValue), 1);
        assert_eq!(tally.total(), 1);
        assert_eq!(b.take_dirty(), DomDirty::Layout);

        b.evaluate(
            "document.getElementById('c').checked = true; \
             document.getElementById('t').value = 'more';",
        )
        .unwrap();
        let tally = b.take_script_mutations();
        assert_eq!(
            tally.first,
            Some((MutationKind::FormValue, "input#c".to_string()))
        );
        assert_eq!(tally.count(MutationKind::FormValue), 2);
        assert_eq!(tally.total(), 2);
        assert!(b.take_script_mutations().is_empty(), "taking resets it");
    }

    #[test]
    fn first_write_names_a_node_without_an_id_by_tag() {
        let (b, _doc) = bound(PAGE);
        b.evaluate("document.getElementById('outside').firstChild.data = 'Bye';")
            .unwrap();
        let tally = b.take_script_mutations();
        // A text node's own write has no element to name.
        assert_eq!(tally.first, Some((MutationKind::Text, String::new())));
        b.evaluate("document.body.innerHTML = '<i>x</i>';").unwrap();
        assert_eq!(
            b.take_script_mutations().first,
            Some((MutationKind::ChildList, "body".to_string()))
        );
    }

    #[test]
    fn a_new_document_drops_the_previous_tally() {
        let (b, _doc) = bound(PAGE);
        b.evaluate("document.getElementById('main').setAttribute('data-x', '1');")
            .unwrap();
        b.set_document(Rc::new(Document::parse_html("<p>x</p>").unwrap()))
            .unwrap();
        assert!(b.take_script_mutations().is_empty());
    }

    // The page picks its ids: the label the relayout log prints is clipped
    // and carries no control characters, so a page cannot flood or forge it.
    #[test]
    fn a_long_or_multiline_id_is_clipped_in_the_label() {
        let (b, _doc) = bound(PAGE);
        b.evaluate(
            "var m = document.getElementById('main'); \
             m.id = 'x\\nINFO forged ' + 'a'.repeat(100000);",
        )
        .unwrap();
        b.take_script_mutations();
        b.evaluate("m.setAttribute('data-n', '1');").unwrap();
        let (_, label) = b.take_script_mutations().first.expect("a write");
        assert!(!label.chars().any(char::is_control), "{label:?}");
        assert!(label.starts_with("div#x?INFO forged aaa"), "{label:?}");
        assert!(label.ends_with('…'), "{label:?}");
        assert_eq!(label.chars().count(), "div#".len() + 48 + 1);
    }

    // The incremental-restyle mutation log: one record per dirtying write,
    // naming its node, the attribute, and the old class or id.
    #[test]
    fn mutation_log_records_each_write_per_node() {
        let (b, doc) = bound(&PAGE.replace("<body>", "<body><input id=\"in\" type=\"checkbox\">"));
        let raw = |id: &str| doc.get_element_by_id(id).unwrap().id.raw();
        let (main, outside, input) = (raw("main"), raw("outside"), raw("in"));
        assert!(b.take_mutation_log().is_empty());
        b.evaluate(
            "var m = document.getElementById('main'); \
             m.className = 'box'; \
             m.className = 'box wide'; \
             m.id = 'main2'; \
             m.setAttribute('data-x', '1'); \
             m.style.color = 'red'; \
             var o = document.getElementById('outside'); \
             o.firstChild.data = 'Bye'; \
             m.appendChild(o); \
             m.removeChild(o); \
             m.innerHTML = '<i>x</i>'; \
             var f = document.createDocumentFragment(); \
             f.appendChild(document.createElement('a')); \
             f.appendChild(document.createElement('b')); \
             document.body.appendChild(f); \
             document.getElementById('in').checked = true;",
        )
        .unwrap();
        let log = b.take_mutation_log();
        assert!(!log.full);
        type Row<'a> = (usize, MutationKind, Option<&'a str>, Option<&'a str>, bool);
        let summary: Vec<Row> = log
            .records
            .iter()
            .map(|r| (r.node, r.kind, r.attr.as_deref(), r.old.as_deref(), r.child.is_some()))
            .collect();
        let body = doc.body().unwrap().id.raw();
        let fragment = log.records[9].node;
        assert_ne!(fragment, body);
        let text = doc.get_element_by_id("outside").unwrap().first_child().unwrap().id.raw();
        assert_eq!(
            summary,
            vec![
                // Setting `class` to its value is no write.
                (main, MutationKind::Attribute, Some("class"), Some("box"), false),
                (main, MutationKind::Attribute, Some("id"), Some("main"), false),
                (main, MutationKind::Attribute, Some("data-x"), None, false),
                (main, MutationKind::Style, Some("style"), None, false),
                (text, MutationKind::Text, None, None, false),
                // A move records the parent it left and the one it joined.
                (body, MutationKind::ChildList, None, None, true),
                (main, MutationKind::ChildList, None, None, true),
                (main, MutationKind::ChildList, None, None, true),
                // innerHTML replaces every child.
                (main, MutationKind::ChildList, None, None, false),
                // Filling a detached fragment is recorded too (the engine
                // skips what is not connected); inserting it records its
                // children.
                (fragment, MutationKind::ChildList, None, None, true),
                (fragment, MutationKind::ChildList, None, None, true),
                (body, MutationKind::ChildList, None, None, true),
                (body, MutationKind::ChildList, None, None, true),
                (input, MutationKind::FormValue, Some("checked"), None, false),
            ]
        );
        assert_eq!(log.records[5].child, Some(outside));
        assert!(b.take_mutation_log().is_empty(), "taking resets it");
        assert_eq!(b.take_dirty(), DomDirty::Style, "the bucket is unchanged by the log");
    }

    #[test]
    fn mutation_log_overflows_into_full() {
        let (b, _doc) = bound(PAGE);
        b.evaluate(&format!(
            "var m = document.getElementById('main'); \
             for (var i = 0; i < {}; i++) m.setAttribute('data-n', String(i));",
            crate::MUTATION_LOG_CAP + 1
        ))
        .unwrap();
        let log = b.take_mutation_log();
        assert!(log.full && log.records.is_empty());
        assert!(!log.is_empty());
        assert!(b.take_mutation_log().is_empty());
    }

    /// Exact `LABEL_PART_MAX` (48) keeps the id; one more character adds `…`.
    #[test]
    fn label_part_boundary_keeps_exactly_forty_eight_chars() {
        let (b, _doc) = bound(PAGE);
        let id48 = "a".repeat(48);
        b.evaluate(&format!(
            "var m = document.getElementById('main'); m.id = '{id48}';"
        ))
        .unwrap();
        b.take_script_mutations();
        b.evaluate("m.setAttribute('data-n', '1');").unwrap();
        let (_, label) = b.take_script_mutations().first.expect("a write");
        assert_eq!(label, format!("div#{id48}"));
        assert!(!label.ends_with('…'), "{label:?}");

        let id49 = "b".repeat(49);
        b.evaluate(&format!("m.id = '{id49}';")).unwrap();
        b.take_script_mutations();
        b.evaluate("m.setAttribute('data-n', '2');").unwrap();
        let (_, label) = b.take_script_mutations().first.expect("a write");
        assert_eq!(label, format!("div#{}…", "b".repeat(48)));
    }
}

/// LAT7c: the reads a Web Animations layer write of `props` reaches, or
/// `None` when one of them is one layout reads (the boxes may move). The
/// paint properties are the engine's paint-only animation frame's
/// (`animations::is_paint_prop`).
fn layer_reads<'a>(props: impl Iterator<Item = &'a str>) -> Option<u8> {
    let mut reads = 0;
    for prop in props {
        reads |= match prop {
            "opacity" | "color" | "background-color" | "border-top-color" | "border-right-color"
            | "border-bottom-color" | "border-left-color" => crate::LAYER_PAINT,
            "transform" | "translate" | "rotate" | "scale" => crate::LAYER_PAINT | crate::LAYER_TRANSFORM,
            _ => return None,
        };
    }
    Some(reads)
}
