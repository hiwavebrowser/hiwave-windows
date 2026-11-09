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
use std::collections::HashMap;
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

/// Control writes script made that the engine has not taken yet. A forced
/// layout copies them into the engine's edit state before it builds, and
/// drains what it applied (`ForcedLayout`).
#[derive(Default)]
pub struct ControlWrites {
    pub values: Vec<(usize, String)>,
    pub checked: Vec<(usize, Option<bool>)>,
}

/// A layout of the DOM as it is now: what a forced layout publishes.
pub struct ForcedGeometry {
    /// Builds the geometry and computed-style maps of this layout, on the
    /// first read that needs them (as `DomBindings::set_geometry_source`).
    pub source: GeometrySource,
    pub hit_tester: Option<HitTester>,
    /// Formats one element's computed style of this layout on its first
    /// read (LAT8); `None` keeps whatever `source` returns.
    pub computed_lines: Option<ComputedLines>,
    /// The furthest the view can scroll over this layout, as
    /// `DomBindings::set_scroll_state` takes it.
    pub max_scroll: (f32, f32),
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
}

pub(crate) type SharedDomHost = Rc<RefCell<DomHost>>;

impl DomHost {
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
        // Only a paint-layer write (LAT7c) is pending: Web Animations'.
        let cause = match dirty.written_since_layout() {
            true => dirty.cause().unwrap_or("other"),
            false => "animation",
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
        // The document may have grown or shrunk: scrollTo clamps to the new
        // end, and an offset past it comes back to it.
        let scroll = h.scroll_state.clone();
        let mut s = scroll.borrow_mut();
        (s.max_x, s.max_y) = laid.max_scroll;
        s.x = s.x.min(s.max_x.max(0.0));
        s.y = s.y.min(s.max_y.max(0.0));
        h.scroll = (s.x, s.y);
        h.forced_layouts += 1;
        h.forced_in_task += 1;
        h.forced_report.note(cause, read, ms);
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

    /// Bind `document`, returning the new generation.
    pub(crate) fn bind(&mut self, document: Rc<Document>) -> u32 {
        let templates = self.templates.get_mut();
        templates.clear();
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
fn adopt_template_contents(
    document: &Document,
    root: &Rc<Node>,
    templates: &mut HashMap<usize, Rc<Node>>,
) {
    let mut pending = vec![root.clone()];
    while let Some(node) = pending.pop() {
        if is_template(&node) {
            let content = templates
                .entry(node.id.raw())
                .or_insert_with(|| document.create_node(NodeType::DocumentFragment))
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
            let nodes = document.parse_fragment(html, context).map_err(|_| "SyntaxError")?;
            let fragment = document.create_node(NodeType::DocumentFragment);
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
            let templates = &mut host.templates.borrow_mut();
            for child in parsed.root().children() {
                // The parsed tree's NodeIds are its own: clone with an
                // empty template table, then give its templates content.
                doc.append_child(clone_node(document, &child, true, &mut HashMap::new()));
            }
            // An HTML document always has a body, but rustkit-html drops
            // it (and the text) for text-only input; supply an empty one.
            if let Some(html) = doc
                .children()
                .into_iter()
                .find(|n| n.tag_name() == Some("html"))
            {
                if !html.children().iter().any(|n| n.tag_name() == Some("body")) {
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
            Ok((
                node_id(Some(clone_node(document, &node, deep, templates))),
                DomDirty::Clean,
            ))
        }
        // HTML §8.5 innerHTML setter: parse as the element's contents (the
        // fragment parsing algorithm), then replace all children with it.
        // A template's innerHTML is its content's (HTML §4.12.3).
        ("setHTML", NodeType::Element { tag_name, .. }) => {
            let html = string_arg(args, 3).unwrap_or("");
            let nodes = document
                .parse_fragment_in(html, tag_name, is_in_html_content(&node), node.holds_svg_content())
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
}

impl Reach {
    fn mark(self, dirty: &crate::DirtyMark, bucket: DomDirty, cause: &'static str) {
        match self {
            Reach::Seen => dirty.mark(bucket, cause),
            Reach::Unseen => dirty.mark_unseen(bucket),
        }
    }
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
    if let Some(content) = templates.get(&node.id.raw()).cloned() {
        let content = clone_node(document, &content, deep, templates);
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

/// Elements whose Text children serialize unescaped.
const RAW_TEXT_ELEMENTS: &[&str] = &[
    "style", "script", "xmp", "iframe", "noembed", "noframes", "plaintext",
];

/// HTML §13.3 "serializing HTML fragments" for one node (`outerHTML`).
/// Attributes come out sorted by name: rustkit-dom keeps them in a
/// HashMap, so source order is gone.
fn serialize_node(node: &Rc<Node>, out: &mut String, templates: &HashMap<usize, Rc<Node>>) {
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
                serialize_children(node, out, templates);
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
        NodeType::Document | NodeType::DocumentFragment => serialize_children(node, out, templates),
    }
}

/// The children of `node`, serialized (`innerHTML`); a template's are
/// its content's children.
fn serialize_children(node: &Rc<Node>, out: &mut String, templates: &HashMap<usize, Rc<Node>>) {
    let source = templates.get(&node.id.raw()).unwrap_or(node);
    for child in source.children() {
        serialize_node(&child, out, templates);
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
fn node_info(node: &Rc<Node>, field: &str, templates: &HashMap<usize, Rc<Node>>) -> JsValue {
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
        // Space-separated (a name never holds whitespace) and sorted:
        // rustkit-dom keeps attributes in a HashMap, so source order is gone.
        "attrNames" => match &node.node_type {
            NodeType::Element { attributes, .. } => {
                let mut names: Vec<&str> = attributes.keys().map(String::as_str).collect();
                names.sort_unstable();
                JsValue::String(names.join(" "))
            }
            _ => JsValue::Null,
        },
        "innerHTML" => {
            let mut out = String::new();
            serialize_children(node, &mut out, templates);
            JsValue::String(out)
        }
        "outerHTML" => {
            let mut out = String::new();
            serialize_node(node, &mut out, templates);
            JsValue::String(out)
        }
        "content" => node_id(templates.get(&node.id.raw()).cloned()),
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
    runtime.register_host_function(
        "__rustkit_dom_info",
        3,
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
                DomHost::ensure_fresh_geometry(&h, &d, read);
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
                (Some(node), Some(field)) => node_info(&node, field, &host.templates.borrow()),
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
            d.mark(crate::DomDirty::Layout, "animation");
            JsValue::Undefined
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
            if let Some(tester) = host.hit_tester.clone() {
                return tester(x, y, host.window_scroll())
                    .map_or(JsValue::Null, |id| JsValue::Number(id as f64));
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
                reach.mark(&d, DomDirty::Style, cause);
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
    runtime.register_host_function(
        "__rustkit_dom_write",
        5,
        Box::new(move |args| {
            let record = write_record(&h.borrow(), args);
            let (reach, cause) = write_reach(&h.borrow(), args, &d);
            match write(&h.borrow(), args) {
            Ok((result, bucket)) => {
                if let Some(record) = record.filter(|_| bucket != DomDirty::Clean) {
                    l.borrow_mut().push(record);
                }
                trace_dirty(&h.borrow(), args, bucket);
                note_mutation(&m, bucket, write_kind(args), || {
                    node_label(&h.borrow(), args, 2)
                });
                reach.mark(&d, bucket, cause);
                result
            }
            Err(name) => JsValue::String(name.to_string()),
            }
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
            if let Some(node) = h.borrow().node(args).filter(|_| bucket != DomDirty::Clean) {
                l.borrow_mut().push(control_record(node.id.raw(), None));
            }
            note_mutation(&m, bucket, MutationKind::FormValue, || {
                node_label(&h.borrow(), args, 1)
            });
            d.mark(bucket, "form");
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
            if let Some(node) = h.borrow().node(args).filter(|_| bucket != DomDirty::Clean) {
                l.borrow_mut().push(control_record(node.id.raw(), Some("checked")));
            }
            note_mutation(&m, bucket, MutationKind::FormValue, || {
                node_label(&h.borrow(), args, 1)
            });
            d.mark(bucket, "form");
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
            d.mark(DomDirty::Style, "sheet");
            JsValue::Null
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
            d.mark(DomDirty::Style, "sheet");
            JsValue::Null
        }),
    )?;

    // Its listener and handler errors go to the host natives (`__rk`).
    runtime.evaluate_with_host(WRAPPERS_JS)?;
    shadow::install(runtime, host, dirty)?;
    node_apis::install(runtime, host)
}

/// Wrapper prototypes, the identity cache, and the `document` read
/// surface. `window.__rustkit_dom_reset(gen)` rebinds it to a new document.
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
        hit: __rustkit_dom_hit, setAdopted: __rustkit_dom_set_adopted,
        setSheetDisabled: __rustkit_dom_set_sheet_disabled,
        scrollElement: __rustkit_dom_scroll_element,
        animate: __rustkit_dom_animate, cssAnimations: __rustkit_dom_css_animations,
        cssControl: __rustkit_dom_css_control, animated: __rustkit_dom_animated
    };
    ['root', 'by_id', 'collect', 'info', 'attr', 'mutate', 'write', 'matches', 'value', 'resolve', 'checked', 'submit', 'navigate',
        'inserted_script', 'hit', 'set_adopted', 'set_sheet_disabled', 'scroll_element', 'animate', 'css_animations', 'css_control', 'animated'].forEach(function (n) {
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
    var HOOKS = {};
    Object.defineProperty(g, '__rkDomHooks', { value: HOOKS, configurable: true, enumerable: false });
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
        // There is no media playback here, and a page must be able to find
        // that out the way it asks: a media element is one with nothing to
        // play. Without the methods the question itself threw (github.com's
        // `video.load()` in an effect). Canvas contexts are web_canvas.js's,
        // which takes this way to a canvas's node and deletes it.
        Object.defineProperty(g, '__rustkit_canvas_slot', {
            value: function (el) { var s = slotOf(el); return [s.gen, s.id]; },
            configurable: true, writable: true, enumerable: false
        });
        var media = HTMLMediaElement.prototype;
        ['HAVE_NOTHING', 'HAVE_METADATA', 'HAVE_CURRENT_DATA', 'HAVE_FUTURE_DATA', 'HAVE_ENOUGH_DATA'].forEach(function (k, i) {
            HTMLMediaElement[k] = media[k] = i;
        });
        media.load = function load() {};
        media.pause = function pause() {};
        media.canPlayType = function canPlayType() { return ''; };
        media.play = function play() {
            return Promise.reject(new g.DOMException('The element has no supported sources.', 'NotSupportedError'));
        };
        [['paused', true], ['ended', false], ['readyState', 0], ['currentTime', 0], ['duration', NaN], ['error', null]].forEach(function (p) {
            Object.defineProperty(media, p[0], { value: p[1], writable: true, enumerable: true, configurable: true });
        });
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
        var m = HOOKS.mutation && HOOKS.mutation.data(o, op, a);
        // Removing an absent attribute runs no attribute-change steps (DOM
        // "remove an attribute by name"), so it must not drop an assigned
        // on<type> handler. N.attr matches the name as the write does.
        var changed = op === 'setAttr' ||
            (op === 'removeAttr' && s.gen === gen && N.attr(s.gen, s.id, a) !== null);
        var r = N.write(gen, op, s.gen === gen ? s.id : -1, a, b);
        if (typeof r === 'string') {
            throw new DOMException("Failed to execute '" + method + "'.", r);
        }
        if (m) m();
        if (changed) attributeChanged(o, a);
        if (HOOKS.scripts) HOOKS.scripts.changed(o, op, a);
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
    // A node in a DOMParser document is owned by it; every other node by
    // the global document.
    getter(Node.prototype, 'ownerDocument', function () {
        if (info(this, 'type') === 9) return null;
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
            var names = a.getAttributeNames();
            if (names.join(' ') !== b.getAttributeNames().join(' ')) return false;
            for (var k = 0; k < names.length; k++) {
                if (a.getAttribute(names[k]) !== b.getAttribute(names[k])) return false;
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
    getter(Element.prototype, 'outerHTML', function () { return info(this, 'outerHTML'); });
    getter(Element.prototype, 'localName', function () { return info(this, 'local'); });
    accessor(HTMLElement.prototype, 'innerText', function () { return info(this, 'innerText'); },
        function (v) { setData(this, 'setInnerText', text(v), null, 'innerText'); });
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
    [Element, Document, DocumentFragment].forEach(function (C) {
        getter(C.prototype, 'firstElementChild', function () { return elementChildren(this)[0] || null; });
        getter(C.prototype, 'lastElementChild', function () {
            var c = elementChildren(this); return c[c.length - 1] || null;
        });
        getter(C.prototype, 'childElementCount', function () { return elementChildren(this).length; });
    });
    [Document, DocumentFragment].forEach(function (C) {
        getter(C.prototype, 'children', function () {
            var s = slotOf(this);
            return list(HTMLCollection.prototype, s.gen === gen ? N.info(s.gen, s.id, 'children') : '', true);
        });
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
    HTMLFormElement.prototype.reset = function () {
        if (!this.dispatchEvent(new Event('reset', { bubbles: true, cancelable: true }))) return;
        controls(this, /^(input|textarea)$/).forEach(function (el) {
            var s = slotOf(el);
            if (s.gen === gen && (el.localName === 'textarea' || inputMode(el) === 'value')) {
                N.value(s.gen, s.id, null);
            }
        });
    };
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
        holder.innerHTML = String(html);
        Array.prototype.slice.call(holder.childNodes).forEach(function (n) {
            spot[0].insertBefore(n, spot[1]);
        });
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
            // The indexed getter (`classList[0]`) answers like item().
            l = new Proxy(l, {
                get: function (t, p, r) {
                    return typeof p === 'string' && /^(0|[1-9][0-9]*)$/.test(p)
                        ? tokens(t)[Number(p)] : Reflect.get(t, p, r);
                }
            });
            classLists.set(this, l);
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
    // with; it takes this and deletes it. write: the element's entries,
    // `[[property, value, composite], ...]`, or null for none. css: the
    // engine's CSS animations and transitions as of the last layout.
    // control: pause/play/cancel/finish one of them.
    g.__rustkit_animated_style = {
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
    var hasOwn = function (o, n) { return Object.prototype.hasOwnProperty.call(o, n); };
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
                if (c0 >= 48 && c0 <= 57 && /^(0|[1-9][0-9]*)$/.test(p)) return names()[Number(p)];
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
        var pairs = { '(': ')', '[': ']', '{': '}' };
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
    // quote, `(`, inner whitespace or a control character in it) is invalid
    // for every property, so the declaration is not in the block (CH2.E29:
    // Chromium's `el.style.length` leaves it out and the earlier value
    // stands). The same rule as `rustkit_css::mask::parse_css_url`.
    function hasBadUrl(value) {
        var quote = null, i, j, k, c;
        for (i = 0; i < value.length; i++) {
            c = value.charAt(i);
            if (quote) { if (c === '\\') i++; else if (c === quote) quote = null; continue; }
            if (c === '"' || c === "'") { quote = c; continue; }
            if (c === '\\') { i++; continue; }
            // `url(` as its own function name, not the tail of `foourl(`.
            if (value.substr(i, 4).toLowerCase() !== 'url(' ||
                (i > 0 && /[\w\-\\\u0080-￿]/.test(value.charAt(i - 1)))) continue;
            for (j = i + 4; j < value.length && /\s/.test(value.charAt(j)); j++) {}
            // A quoted url is a function with a string: the strings rule above reads it.
            if (value.charAt(j) === '"' || value.charAt(j) === "'") { i = j - 1; continue; }
            for (; j < value.length; j++) {
                c = value.charAt(j);
                if (c === '\\') { j++; continue; }
                if (c === ')') break;
                if (c === '"' || c === "'" || c === '(' || /[\x00-\x08\x0b\x0e-\x1f\x7f]/.test(c)) return true;
                if (/\s/.test(c)) {
                    for (k = j; k < value.length && /\s/.test(value.charAt(k)); k++) {}
                    // Whitespace is allowed only before the closing `)`.
                    if (k < value.length && value.charAt(k) !== ')') return true;
                    j = k - 1;
                }
            }
            i = j;
        }
        return false;
    }
    function decls(st) {
        var out = [];
        splitDecls(st[OWNER].getAttribute('style') || '').forEach(function (d) {
            var i = d.indexOf(':');
            if (i < 0) return;
            var name = d.slice(0, i).trim().toLowerCase(), value = d.slice(i + 1).trim();
            var important = /!\s*important$/i.test(value);
            if (important) value = value.replace(/\s*!\s*important$/i, '');
            if (!name || !value) return;
            if (hasBadUrl(value)) return;
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
            // An invalid value changes nothing (the attribute is not rewritten).
            if (hasBadUrl(value)) return;
            var ds = decls(this), d = ds.filter(function (x) { return x.name === name; })[0];
            var important = String(priority || '').toLowerCase() === 'important';
            // The same declaration again changes nothing: the attribute is
            // not rewritten (Blink's set-property returns "did not change"),
            // so a geometry read after it lays nothing out (LAT7).
            if (d && d.value === value && d.important === important) return;
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
    Document.prototype.createElement = function (tag) {
        tag = String(tag);
        var el = create('element', tag, 'createElement');
        if (HOOKS.scripts) HOOKS.scripts.created(el);
        return el;
    };
    Document.prototype.createTextNode = function (data) {
        return create('text', String(data), 'createTextNode');
    };
    Document.prototype.createComment = function (data) {
        return create('comment', String(data), 'createComment');
    };
    // DOM §4.5 importNode: a copy for this document. Nodes here all share
    // the one Rust Document, so importing is cloning.
    Document.prototype.importNode = function (node, deep) {
        nodeArg(node, 'importNode');
        if (node.nodeType === 9) {
            throw new DOMException("Failed to execute 'importNode' on 'Document': " +
                'The node provided is a document, which may not be imported.', 'NotSupportedError');
        }
        return node.cloneNode(!!deep);
    };
    Document.prototype.createDocumentFragment = function () {
        return create('fragment', '', 'createDocumentFragment');
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
                if (!cutting) frag.appendChild(g.document.createTextNode(data));
                else n.data = n.data.slice(0, r.start.offset) + n.data.slice(r.end.offset);
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
    function rangeContext(node) {
        if (node.nodeType === 1) return node;
        if (node.nodeType === 3 || node.nodeType === 8) return node.parentElement;
        return null;
    }
    Range.prototype.createContextualFragment = function (html) {
        var r = slots(this, 'createContextualFragment');
        var ctx = rangeContext(r.start.node);
        var tag = ctx && ctx.localName ? ctx.localName : 'body';
        var made = N.write(gen, 'create:fragmentHTML', tag, String(html));
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
    // The rect of a range: the union of the boxes of the elements its ends
    // sit in (no per-character geometry reaches script yet).
    Range.prototype.getBoundingClientRect = function () {
        var r = slots(this, 'getBoundingClientRect');
        function box(node) {
            var el = node.nodeType === 1 ? node : node.parentElement;
            return el && el.getBoundingClientRect ? el.getBoundingClientRect() : null;
        }
        var a = box(r.start.node), b = r.end.node === r.start.node ? a : box(r.end.node);
        if (!a) a = b;
        if (!b) b = a;
        if (!a) return g.DOMRect ? new g.DOMRect(0, 0, 0, 0) : { x: 0, y: 0, width: 0, height: 0, top: 0, left: 0, right: 0, bottom: 0 };
        var left = Math.min(a.left, b.left), top = Math.min(a.top, b.top);
        var right = Math.max(a.right, b.right), bottom = Math.max(a.bottom, b.bottom);
        return g.DOMRect ? new g.DOMRect(left, top, right - left, bottom - top)
            : { x: left, y: top, width: right - left, height: bottom - top, top: top, left: left, right: right, bottom: bottom };
    };
    Range.prototype.getClientRects = function () { return [this.getBoundingClientRect()]; };
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
    // moves it with the mouse: `__rkSetSelection` is how a drag reaches
    // script, `__rkTakeSelection` how a script's change reaches the engine.
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
            g.document.dispatchEvent(new g.Event('selectionchange', { bubbles: false, cancelable: false }));
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
    Object.defineProperty(Document.prototype, '__rkSetSelection', {
        value: function (aId, aOff, fId, fOff) {
            var a = typeof aId === 'number' ? wrap(aId) : null;
            var f = typeof fId === 'number' ? wrap(fId) : null;
            if (!a || !f) setSel(null);
            else setSel({ node: a, offset: Math.min(aOff, nodeLength(a)) }, { node: f, offset: Math.min(fOff, nodeLength(f)) });
            SELN.sent = selKey();
        },
        configurable: true, enumerable: false, writable: true
    });
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
    Object.defineProperty(Document.prototype, '__rkEditKey', {
        value: function (hostId, op, text) {
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
            var ev = new g.InputEvent('input', { bubbles: true, composed: true, view: g, data: data, inputType: inputType });
            ev.isTrusted = true;
            host.dispatchEvent(ev);
            return true;
        },
        configurable: true, enumerable: false, writable: true
    });
    // The selection as the engine last heard it changed: null when it did
    // not, else '' (none) or "anchorId anchorOffset focusId focusOffset".
    Object.defineProperty(Document.prototype, '__rkTakeSelection', {
        value: function () {
            var key = selKey();
            if (key === SELN.sent) return null;
            SELN.sent = key;
            return key;
        },
        configurable: true, enumerable: false, writable: true
    });
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
        return id == null ? null : id;
    }
    Document.prototype.elementFromPoint = function (x, y) {
        var id = pointHit(x, y, 'elementFromPoint');
        if (id == null) return null;
        var node = wrap(id);
        while (node && node.nodeType !== 1) node = node.parentElement || node.parentNode;
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
    // The global document's title is an own property the engine sets; a
    // DOMParser document reads its first <title> (HTML §3.1.3).
    getter(Document.prototype, 'title', function () {
        var t = this.querySelector('title');
        return t ? t.textContent.replace(/[\t\n\f\r ]+/g, ' ').trim() : '';
    });

    // `document.currentScript` (HTML §3.1.1): the classic <script> element
    // being run, else null. Pages lean on it constantly: inline scripts
    // that end in `document.currentScript.remove()`, and webpack's
    // `publicPath` detection (`document.currentScript.src`). With it
    // missing, both threw "cannot convert null or undefined to object" and
    // the rest of the script never ran. The engine names the element
    // through `__rkSetCurrentScript(id)` before each script and clears it
    // after; the hook is not enumerable.
    var currentScript = null;
    getter(Document.prototype, 'currentScript', function () {
        var s = slotOf(this); return s.gen === gen ? currentScript : null;
    });
    Object.defineProperty(Document.prototype, '__rkSetCurrentScript', {
        value: function (id) { currentScript = typeof id === 'number' ? wrap(id) : null; },
        configurable: true, enumerable: false, writable: true
    });
    // The engine reports a script's outcome to its element: `load` or
    // `error`, which do not bubble (HTML "execute the script element").
    Object.defineProperty(Document.prototype, '__rkFireOn', {
        value: function (id, type) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (target) target.dispatchEvent(new Event(type));
        },
        configurable: true, enumerable: false, writable: true
    });
    // The engine's CSS transitions report their phases (css-transitions-2
    // §4): a trusted TransitionEvent that bubbles; `transitionend` can be
    // cancelled, the others not. An element taken out of the document
    // still hears its `transitioncancel`.
    Object.defineProperty(Document.prototype, '__rkFireTransition', {
        value: function (id, type, init) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target) return;
            init.bubbles = true;
            init.cancelable = type === 'transitionend';
            var ev = new (typeof g.TransitionEvent === 'function' ? g.TransitionEvent : g.Event)(type, init);
            try { ev.isTrusted = true; } catch (_) {}
            target.dispatchEvent(ev);
        },
        configurable: true, enumerable: false, writable: true
    });
    // Likewise the engine's `@keyframes` animations (css-animations-1 §5):
    // a trusted AnimationEvent that bubbles and cannot be cancelled.
    Object.defineProperty(Document.prototype, '__rkFireAnimation', {
        value: function (id, type, init) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target) return;
            init.bubbles = true;
            init.cancelable = false;
            var ev = new (typeof g.AnimationEvent === 'function' ? g.AnimationEvent : g.Event)(type, init);
            try { ev.isTrusted = true; } catch (_) {}
            target.dispatchEvent(ev);
        },
        configurable: true, enumerable: false, writable: true
    });
    // The engine delivers the user's mouse input: a trusted event that
    // bubbles and can be cancelled (UI Events §3.4), except the enter and
    // leave events, which do neither and go to one element each.
    // `pointer*` and `click` are PointerEvents of the mouse (Pointer
    // Events §4, §12); `mousedown`, `mouseup` and `click` carry the click
    // count. Returns false when a listener called preventDefault().
    Object.defineProperty(Document.prototype, '__rkFireMouse', {
        value: function (id, type, init) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target) return true;
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
            var ev = new (pointer && g.PointerEvent || g.MouseEvent)(type, init);
            ev.isTrusted = true;
            // Where the point is in the document and in the target's box.
            var at = {
                pageX: init.clientX + (g.scrollX || 0), pageY: init.clientY + (g.scrollY || 0),
                offsetX: init.offsetX, offsetY: init.offsetY,
                movementX: init.movementX, movementY: init.movementY
            };
            // A move or a boundary event has no button of its own: `which`
            // is the primary button while it is held.
            if (!press && !pointer) at.which = init.buttons & 1;
            Object.keys(at).forEach(function (k) {
                Object.defineProperty(ev, k, { value: at[k], configurable: true, enumerable: true });
            });
            return target.dispatchEvent(ev);
        },
        configurable: true, enumerable: false, writable: true
    });

    // The engine's drag-and-drop processing model (HTML §6.11.5) delivers
    // a trusted DragEvent with the drag's DataTransfer (web_dnd.js): every
    // drag event bubbles; dragleave and dragend cannot be cancelled. `op`
    // is the current drag operation. Returns 'notCancelled:operation',
    // the operation being what the dataTransfer's effects allow after the
    // listeners ran.
    Object.defineProperty(Document.prototype, '__rkFireDrag', {
        value: function (id, type, init, op) {
            var store = g.__rkDragStore;
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!store || !target) return '1:none';
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
            var ev = new (g.DragEvent || g.MouseEvent)(type, init);
            if (!(ev.dataTransfer instanceof Object)) {
                Object.defineProperty(ev, 'dataTransfer', { value: dt, configurable: true, enumerable: true });
            }
            ev.isTrusted = true;
            var at = {
                pageX: init.clientX + (g.scrollX || 0), pageY: init.clientY + (g.scrollY || 0),
                offsetX: init.offsetX, offsetY: init.offsetY, movementX: 0, movementY: 0
            };
            Object.keys(at).forEach(function (k) {
                Object.defineProperty(ev, k, { value: at[k], configurable: true, enumerable: true });
            });
            var notCancelled = target.dispatchEvent(ev);
            // A cancelled dragstart ends the drag before it began.
            var settled = store.settle(type === 'dragstart' && !notCancelled ? 'dragend' : type);
            return (notCancelled ? '1:' : '0:') + settled;
        },
        configurable: true, enumerable: false, writable: true
    });

    // The engine delivers the user's key press or release to the focused
    // element: a trusted KeyboardEvent that bubbles and can be cancelled
    // (UI Events §3.7). With nothing focused in the engine (`id` null) it
    // goes to the page's active element: what script focused, else the
    // body. Returns false when a listener called preventDefault().
    Object.defineProperty(Document.prototype, '__rkFireKey', {
        value: function (id, type, init) {
            var target = typeof id === 'number' ? wrap(id) : g.document.activeElement;
            if (!target) return true;
            init.bubbles = true;
            init.cancelable = true;
            init.composed = true;
            init.view = g;
            var ev = new g.KeyboardEvent(type, init);
            ev.isTrusted = true;
            return target.dispatchEvent(ev);
        },
        configurable: true, enumerable: false, writable: true
    });
    // What the user typed changed the control's value: `input`, which
    // bubbles and cannot be cancelled (Input Events §5.1). `data` is the
    // inserted text, null for a deletion.
    Object.defineProperty(Document.prototype, '__rkFireInput', {
        value: function (id, data) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (!target) return;
            // The user edited it: `change` when it loses the focus
            // (node_apis.js).
            Object.defineProperty(target, '__rkEdited', {
                value: true, configurable: true, enumerable: false, writable: true
            });
            var ev = new g.InputEvent('input', {
                bubbles: true, composed: true, view: g, data: data,
                inputType: data === null ? 'deleteContentBackward' : 'insertText'
            });
            ev.isTrusted = true;
            target.dispatchEvent(ev);
        },
        configurable: true, enumerable: false, writable: true
    });
    // The user picked an option of a select in its drop-down or with a
    // key (web_forms.js's pick: selectedness, `input`, `change`). Returns
    // whether the selection changed.
    var pickOption = null;
    Object.defineProperty(Document.prototype, '__rkPickOption', {
        value: function (id, index) {
            var target = typeof id === 'number' ? wrap(id) : null;
            return !!(target && pickOption && pickOption(target, index));
        },
        configurable: true, enumerable: false, writable: true
    });
    // Enter in a field (web_forms.js's implicit submission).
    var implicitSubmit = null;
    Object.defineProperty(Document.prototype, '__rkImplicitSubmit', {
        value: function (id) {
            var target = typeof id === 'number' ? wrap(id) : null;
            if (target && implicitSubmit) implicitSubmit(target);
        },
        configurable: true, enumerable: false, writable: true
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
        // The `signal` option: an aborted signal adds nothing, and a later
        // abort removes the listener.
        var signal = opts && typeof opts === 'object' ? opts.signal : undefined;
        if (signal != null && signal.aborted) return;
        for (var i = 0; i < list.length; i++) {
            if (list[i].cb === cb && list[i].capture === capture) return;
        }
        list.push({ cb: cb, capture: capture, once: flag(opts, 'once'), removed: false });
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
            return new Function('__rk_doc', '__rk_el',
                'return function (event) { with (__rk_doc) { with (__rk_el) {\n' + src + '\n} } };'
            )(el.ownerDocument || {}, el);
        } catch (e) {
            try { __rk.reportError(e); } catch (_) {}
            return null;
        }
    }
    function inlineHandler(t, type) {
        var src = t.getAttribute('on' + type);
        if (src === null) return null;
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
        name = String(name).toLowerCase();
        if (name.length > 2 && name.charCodeAt(0) === 111 && name.charCodeAt(1) === 110) {
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
    var ORIGIN = Symbol('rustkit.origin'), PATH = Symbol('rustkit.path');
    function invoke(t, event, phase, capture) {
        event.currentTarget = t;
        event.eventPhase = phase;
        // A listener sees the target as its own tree sees it: a node inside a
        // shadow tree is the host to a listener outside.
        if (HOOKS.retarget && event[ORIGIN]) event.target = HOOKS.retarget(event[ORIGIN], t);
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
        var h = capture || event[STOP_NOW] ? null : handlerOf(t, event.type);
        if (typeof h === 'function' && callListener(t, h, event) === false && event.cancelable) {
            event.defaultPrevented = true;
        }
    }
    // A node's parent, then document's is window (except for `load`, which
    // never reaches window from a node).
    function eventParent(n, event) {
        if (n === g) return null;
        if (n === doc) return event.type === 'load' ? null : g;
        if (!n[SLOT]) return null;
        var p = n.parentNode;
        // A composed event goes on from a shadow root to its host.
        return p || (HOOKS.shadowParent ? HOOKS.shadowParent(n, event) : null);
    }
    // Activation behaviour (DOM §2.9 "dispatch", steps 5 and 11): a click
    // (the user's, `el.click()`, or a MouseEvent script dispatches) checks a
    // checkbox before its listeners run and clicks a label's control after.
    // `activation(target)` is web_forms.js's; it answers the function to
    // call with "was not cancelled" once the listeners have run, or null.
    var activation = null, SYNTHETIC_CLICK = Symbol('rustkit.click');
    function activationFor(t, event) {
        if (!activation || event.type !== 'click' || !t[SLOT]) return null;
        if (!event[SYNTHETIC_CLICK] && !(typeof g.MouseEvent === 'function' && event instanceof g.MouseEvent)) return null;
        return activation(t);
    }
    EventTarget.prototype.dispatchEvent = function (event) {
        if (event == null || typeof event !== 'object' || event.type === undefined) {
            throw new TypeError("Failed to execute 'dispatchEvent' on 'EventTarget': " +
                "parameter 1 is not of type 'Event'.");
        }
        var t = thisTarget(this), path = [], i;
        for (var n = t; n; n = eventParent(n, event)) path.push(n);
        event.target = t;
        event[ORIGIN] = t;
        event[PATH] = path;
        var activated = activationFor(t, event);
        for (i = path.length - 1; i > 0 && !event[STOP]; i--) invoke(path[i], event, 1, true);
        if (!event[STOP]) invoke(t, event, 2, true);
        if (!event[STOP]) invoke(t, event, 2, false);
        if (event.bubbles) {
            for (i = 1; i < path.length && !event[STOP]; i++) invoke(path[i], event, 3, false);
        }
        event[STOP] = event[STOP_NOW] = false;
        event.currentTarget = null;
        event.eventPhase = 0;
        // After dispatch the target is the outermost host (the node itself
        // when it is in the document), and there is no path.
        event.target = HOOKS.retarget ? HOOKS.retarget(t, g) : t;
        event[ORIGIN] = null;
        event[PATH] = null;
        if (activated) activated(!event.defaultPrevented);
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
    // The legacy initializers; they do nothing during dispatch.
    Event.prototype.initEvent = function (type, bubbles, cancelable) {
        if (this.eventPhase !== 0) return;
        this.type = String(type);
        this.bubbles = !!bubbles;
        this.cancelable = !!cancelable;
    };
    Event.prototype.preventDefault = function () { if (this.cancelable) this.defaultPrevented = true; };
    // The nodes the event is going through, target first; empty outside dispatch.
    Event.prototype.composedPath = function () { return this[PATH] ? this[PATH].slice() : []; };
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
    CustomEvent.prototype.initCustomEvent = function (type, bubbles, cancelable, detail) {
        if (this.eventPhase !== 0) return;
        this.initEvent(type, bubbles, cancelable);
        this.detail = detail === undefined ? null : detail;
    };
    g.Event = Event;
    g.CustomEvent = CustomEvent;

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
    HTMLElement.prototype.click = function () {
        var e = new Event('click', { bubbles: true, cancelable: true });
        e[SYNTHETIC_CLICK] = true;
        // A link's activation behaviour (HTML §4.6.4): the user's click is
        // followed by the engine, script's own click() here.
        if (this.dispatchEvent(e) && g.__rustkit_follow_link) g.__rustkit_follow_link(this);
    };

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
    g.__rkSetAdoptedStyles = function (node, json) {
        var s = node != null ? node[SLOT] : undefined;
        if (!s || s.gen !== gen) return;
        N.setAdopted(s.gen, s.id, String(json));
    };
    // web_cssom.js: a <style>'s sheet.disabled, for the sheet it owns now.
    g.__rkSetSheetDisabled = function (node, on) {
        var s = node != null ? node[SLOT] : undefined;
        if (!s || s.gen !== gen) return;
        N.setSheetDisabled(s.gen, s.id, !!on);
    };
    // web_cssom.js: which sheet a <style> owns now (a number that changes
    // when its children change or it moves); -1 for a node of another
    // document.
    g.__rkSheetGeneration = function (node) {
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
        setFragmentProto: function (f) { fragmentProto = f; },
        extendElementProto: function (f) {
            var base = elementProto;
            elementProto = function (id) { return f(id, base); };
        }
    };

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
            "class,data-z,id|0|false|true|false|true|false|true|false|false|true|false|TypeError"
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
