//! What the style and layout build reads besides the view and the
//! document, behind one `Rc` (forced layout, Pete's option 2, 2026-10-26).
//!
//! A script's geometry read inside a task has to lay the page out again
//! (forced layout). Script runs with the view's bindings borrowed out of
//! the engine, so the read cannot reach `&mut Engine`. The build's state
//! therefore moves out of `Engine` into a context the bindings' closures
//! can hold, the way they hold the hit tester and the selector matcher.
//! This is the first step: the engine-local build state. The engine keeps
//! one context and reads it through `Engine::lctx`; nothing else changes.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use url::Url;

use rustkit_image::ImageManager;
use rustkit_layout::FontLoader;

use crate::{EngineViewId, StyleRecord};

pub(crate) struct LayoutCtx {
    /// Decoded images: the build reads natural sizes from it (and decodes a
    /// `data:` image in place).
    pub(crate) image_manager: Arc<ImageManager>,
    /// Webfont loader. PER-ENGINE AND Arc-SHARED per the 2026-08-08 design
    /// pin: one loader for the whole engine, with cross-site isolation
    /// provided by the partition key on every operation rather than by
    /// handing each view its own object. Per-view loaders would refetch the
    /// same face for every tab; an unpartitioned shared one would be a
    /// cross-site timing oracle. The partitioned shared loader is the only
    /// shape that is both.
    pub(crate) font_loader: Arc<FontLoader>,
    /// Cascade provenance, recorded by the REAL cascade when armed.
    ///
    /// `None` = not recording, which is the default and costs nothing. See
    /// [`crate::Engine::set_style_recording`] for why this is a side table
    /// rather than a field on `ComputedStyle`.
    pub(crate) style_trace: RefCell<Option<Vec<StyleRecord>>>,
    /// The view currently being laid out, so the view-agnostic recursive
    /// builder can read that view's edit states, focus and viewport (and
    /// only that view's). Set by `build_layout_for_view` around the build;
    /// `None` outside one.
    pub(crate) building: RefCell<Option<Rc<BuildView>>>,
    /// Parsed SVG documents keyed by URL. SVGs referenced from <img> are
    /// vector content — they bypass ImageManager's raster decode (which
    /// rejects them as "Unknown image format") and are spliced into the
    /// display list as vector commands at build time.
    pub(crate) svg_cache: RefCell<HashMap<String, rustkit_svg::SvgDocument>>,
    /// The SVG images of `svg_cache` (fetched or data: URLs, not inline
    /// `<svg>`), shared with the canvas image loader: a canvas
    /// `drawImage` of an SVG `<img>` rasterizes its path list (STR-7
    /// slice 3).
    pub(crate) canvas_svgs: crate::canvas_paint::CanvasSvgs,
    /// Moves whenever an SVG image (fetched or a `data:` URL) enters `svg_cache`: an `<img>`'s
    /// natural size can come from there, so a kept box tree built before it is stale
    /// ([`Self::image_generation`]).
    pub(crate) svg_images: std::cell::Cell<u64>,
    /// The SVG paint declarations (`fill`, `stroke`, `stop-color`, ...)
    /// author rules give each element inside an inline `<svg>`, by raw
    /// NodeId. Filled by `cache_svg_paint_styles` before each build (a
    /// relayout's and a forced one's); the serializer writes them into the
    /// element's `style` so rustkit-svg, which sees only the markup, paints
    /// with them. One map per context, so between builds it is the map of
    /// the view built last: `export_layout_json`, which reads it outside a
    /// build, recomputes it for the view it exports.
    pub(crate) svg_paint_styles: RefCell<Rc<HashMap<usize, crate::SvgPaintStyle>>>,
    /// What the inline svg pre-pass keeps between builds: the id index,
    /// each svg's markup key and the paint maps (`svg_prepass`, LAT4b).
    pub(crate) svg_prepass: RefCell<crate::svg_prepass::SvgPrepass>,
    /// `EngineConfig::nested_contexts`, for the build: an `<iframe>` is a
    /// replaced box with a 2px border only with it on.
    pub(crate) nested_contexts: std::cell::Cell<bool>,
}

impl Default for LayoutCtx {
    fn default() -> Self {
        Self {
            image_manager: Arc::new(ImageManager::new()),
            font_loader: Arc::new(FontLoader::new()),
            style_trace: RefCell::default(),
            building: RefCell::default(),
            svg_cache: RefCell::default(),
            canvas_svgs: Default::default(),
            svg_images: std::cell::Cell::new(0),
            svg_paint_styles: RefCell::default(),
            svg_prepass: RefCell::default(),
            nested_contexts: std::cell::Cell::new(false),
        }
    }
}

impl LayoutCtx {
    /// What an `<img>`'s natural size can come from has changed when this moves: a raster image
    /// entered or left the image cache, or an SVG image entered the SVG cache. The kept box tree
    /// records it at its build and is refused when it differs (#1304; the SVG half after batch 108).
    pub(crate) fn image_generation(&self) -> u64 {
        self.image_manager.cache_generation().wrapping_add(self.svg_images.get())
    }
}

/// What a build reads of the view it is for, taken by the engine when the
/// build starts.
///
/// `Engine::build_view` is the one place a `BuildView` is made from the
/// engine's view; a build that starts later from one already taken (the
/// forced read, #1057) goes through [`BuildView::for_build`], so a field
/// added here is carried along without touching those call sites.
#[derive(Clone)]
pub(crate) struct BuildView {
    pub(crate) id: EngineViewId,
    /// `None` when the view is not (or no longer) in the engine.
    pub(crate) lay: Option<Rc<ViewLayout>>,
    /// The document's URL: relative resource URLs and the font partition.
    pub(crate) url: Option<Url>,
    /// The viewport in CSS px: the headless bounds, else the host surface.
    pub(crate) viewport: Option<(f32, f32)>,
    /// The device pixel ratio an `<img>`'s `srcset` is selected for (E54b).
    pub(crate) dpr: f32,
    /// Focus is per-VIEW but the recursive layout builder is view-agnostic,
    /// so the alternative was checking "is this node focused in ANY view",
    /// which would mark a node focused in one view while building another.
    pub(crate) focus: Option<rustkit_dom::NodeId>,
    /// The view's `<link>` sheets: the rule-index fingerprint vouches for
    /// them only when the build was handed this very list.
    pub(crate) external_sheets: Rc<Vec<rustkit_css::Stylesheet>>,
    /// The `<link>` that owns each of `external_sheets`, for its place
    /// among the `<style>` sheets (`sheet_document_order`).
    pub(crate) external_sheet_owners: Rc<Vec<rustkit_dom::NodeId>>,
}

impl BuildView {
    /// This view for a build that starts now: every per-build field copied,
    /// with what can change between builds (the focused node) read afresh
    /// from the view's layout state.
    pub(crate) fn for_build(&self) -> BuildView {
        BuildView {
            focus: self.lay.as_ref().and_then(|lay| lay.focused_node.get()),
            ..self.clone()
        }
    }
}

impl LayoutCtx {
    /// Runs `build` with `view` as the view being built. What was there
    /// before is put back when `build` returns or unwinds, so a build that
    /// panics leaves no view (and no `ViewLayout`) held by the context.
    ///
    /// `build` must be a complete build of the view: the transitions of
    /// every element it did not reach are cancelled when it returns, so a
    /// build cut short would cancel what it never got to. Two builds are
    /// known not to be complete and are not swept: one that took the kept
    /// box tree, and a pointer restyle's partial build
    /// (`crate::restyle_build_active`), which skips everything that is not
    /// under a changed element.
    pub(crate) fn with_building<R>(&self, view: BuildView, build: impl FnOnce() -> R) -> R {
        struct Restore<'a> {
            ctx: &'a LayoutCtx,
            prev: Option<Rc<BuildView>>,
        }
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                *self.ctx.building.borrow_mut() = self.prev.take();
            }
        }
        // A selector walk that reads an ancestor's earlier siblings from the
        // DOM reads their checkedness from here (`crate::WalkCheckedScope`).
        let checked = view.lay.as_ref().map(|lay| lay.checked_states.borrow().clone()).unwrap_or_default();
        let pointer = view
            .lay
            .as_ref()
            .map(|lay| (lay.hover_chain.borrow().clone(), lay.active_chain.borrow().clone()))
            .unwrap_or_default();
        let focus = view
            .lay
            .as_ref()
            .map(|lay| (lay.focus_chain.borrow().clone(), lay.focus_visible.get()))
            .unwrap_or_default();
        let _walk_checked = crate::WalkCheckedScope::enter(Some(checked), Some(pointer));
        let _walk_focus = crate::WalkFocusScope::enter(focus.0, focus.1);
        let lay = view.lay.clone();
        if let Some(lay) = lay.as_ref() {
            // Script's pause/play/cancel/finish of CSS animations, before
            // the build reads them: here rather than in the relayout
            // entry, so a forced layout (`getComputedStyle` right after
            // `a.pause()` in the same task) reads the controlled state.
            let controls = std::mem::take(&mut lay.animated.borrow_mut().controls);
            let mut anims = lay.animations.borrow_mut();
            for control in &controls {
                anims.control(control);
            }
            anims.last_build_clock = Some(anims.clock_ms);
            anims.begin_build();
        }
        // A pointer restyle sets this before its builds and takes it
        // after them: read before `build`, while it is certainly set.
        let partial_build = crate::restyle_build_active();
        crate::relayout_cause::set_last_build_partial(false);
        let prev = self.building.replace(Some(Rc::new(view)));
        let _restore = Restore { ctx: self, prev };
        let built = build();
        let partial_build = build_was_partial(partial_build);
        // What the build did not visit has no computed style: its
        // transitions end here. Not after a build that took the kept box
        // tree (it visited nothing) or a partial restyle build (it skipped
        // what is not under a changed element): every element is still
        // there with its style.
        if let Some(lay) = lay.as_ref() {
            let reused_tree = crate::relayout_cause::last_build_reused_tree();
            lay.animations.borrow_mut().end_build(reused_tree, partial_build);
        }
        built
    }

    /// The transitions' and animations' hand-off for one element of the
    /// view being built (`Animations::on_style`); nothing outside a build.
    /// A `@keyframes` an animation names is resolved here: each block's
    /// declarations applied to the element's cascaded style, and the values
    /// of the properties they set read back. True when the element animates or
    /// may (it names an animation or a transition, script animates it, or it
    /// has running records): its style is then more than its cascade.
    pub(crate) fn animate_style(
        &self,
        node: usize,
        style: &mut crate::ComputedStyle,
        stylesheets: &[rustkit_css::Stylesheet],
    ) -> bool {
        let lay = self
            .building
            .borrow()
            .as_ref()
            .and_then(|view| view.lay.clone());
        let Some(lay) = lay else {
            return false;
        };
        let resolve =
            |name: &str, base: &crate::ComputedStyle| -> Option<Vec<crate::animations::Frame>> {
                let rule = stylesheets
                    .iter()
                    .rev()
                    .flat_map(|sheet| sheet.keyframes.iter().rev())
                    .find(|rule| rule.name == name)?;
                let mut frames = Vec::new();
                for block in &rule.frames {
                    let mut probe = base.clone();
                    let mut timing = None;
                    let mut props = Vec::new();
                    for declaration in &block.declarations {
                        let property = declaration.property.to_ascii_lowercase();
                        if property == "animation-timing-function" {
                            timing = Some(crate::parse_timing_function(&declaration.value));
                            continue;
                        }
                        self.apply_style_property(&mut probe, &property, &declaration.value);
                        props.extend(crate::animations::prop_indices(&property));
                    }
                    props.sort_unstable();
                    props.dedup();
                    let values: Vec<(usize, crate::animations::AnimValue)> = props
                        .iter()
                        .map(|&i| (i, crate::animations::prop_value(&probe, i)))
                        .collect();
                    for &offset in &block.offsets {
                        frames.push(crate::animations::Frame {
                            offset,
                            values: values.clone(),
                            timing: timing.clone(),
                        });
                    }
                }
                Some(frames)
            };
        // LAT2: an element that animates, or may (it names an animation
        // or a transition, or script animates it), keeps its cascaded
        // style for the frames painted without a build.
        let entries = lay.animated.borrow().script.get(&node).cloned();
        let named = style
            .animations
            .iter()
            .any(|item| !item.name.trim().is_empty() && !item.name.trim().eq_ignore_ascii_case("none"));
        let may_animate = named || !style.transitions.is_empty() || entries.is_some() || {
            lay.animations.borrow().has_records(node)
        };
        let cascaded = may_animate.then(|| style.clone());
        lay.animations.borrow_mut().on_style(node, style, &resolve);
        // Script's animated values, above the CSS ones: each entry parsed
        // against the style as it stands and composed on it.
        self.apply_script_layer(style, entries.as_deref());
        let mut anims = lay.animations.borrow_mut();
        match cascaded {
            Some(cascaded) if entries.is_some() || anims.has_records(node) => {
                anims.captured.insert(
                    node,
                    crate::animations::Captured {
                        cascaded,
                        values: crate::animations::all_values(style),
                        settled_opacity: style.animation_settled_opacity,
                        script: entries,
                    },
                );
            }
            _ => {
                anims.captured.remove(&node);
            }
        }
        may_animate
    }

    /// Script's animated-style entries for one element (Web Animations),
    /// composed on `style` in order.
    fn apply_script_layer(&self, style: &mut crate::ComputedStyle, entries: Option<&[(String, String, String)]>) {
        let Some(entries) = entries else {
            return;
        };
        for (property, value, composite) in entries {
            let property = property.to_ascii_lowercase();
            let indices = crate::animations::prop_indices(&property);
            if indices.is_empty() {
                self.apply_style_property(style, &property, value);
                continue;
            }
            let mut probe = style.clone();
            self.apply_style_property(&mut probe, &property, value);
            for i in indices {
                let under = crate::animations::prop_value(style, i);
                let new = crate::animations::prop_value(&probe, i);
                let composed = crate::animations::compose(&under, &new, composite);
                crate::animations::set_prop_value(style, i, &composed);
            }
        }
        // Paint's `group_opacity` takes the settled frame for a box
        // that names a CSS animation (the parity settle path, set in
        // `Animations::on_style` above). The script layer composed
        // over it just now, so the settled value is restated here or
        // paint would show the CSS-only opacity under a script fade.
        if !style.animation_name.trim().is_empty() && !style.animation_name.eq_ignore_ascii_case("none") {
            style.animation_settled_opacity = Some(style.opacity);
        }
    }

    /// LAT2: one animation frame of `view` without a build. Each captured
    /// element is restyled from its cascaded style at the animation clock
    /// (transitions, animations and script's layer, as a build would), and
    /// its values are compared with the ones painted last. `Ok` lists what
    /// moved when all of it is paint; `Err` says why the frame needs a
    /// build and a layout. Either way the animations' state (phases,
    /// events) has moved to the clock, as a build would have moved it.
    pub(crate) fn animation_frame(&self, view: BuildView) -> Result<Vec<FrameChange>, String> {
        let Some(lay) = view.lay.clone() else {
            return Err("no view".into());
        };
        // Script's pause/play/cancel/finish, as a build takes them.
        let controls = std::mem::take(&mut lay.animated.borrow_mut().controls);
        let script: std::collections::HashMap<usize, Vec<(String, String, String)>> =
            lay.animated.borrow().script.clone();
        let nodes: Vec<usize> = {
            let mut anims = lay.animations.borrow_mut();
            for control in &controls {
                anims.control(control);
            }
            // A record's element was captured by the build that made the
            // record. A script-layer entry's element is captured by the
            // next build that visits it: one that no build has visited
            // (detached, or under `display: none`) paints nothing, and
            // the frame has nothing of it to place (LAT14: until
            // 2026-11-08 such an entry laid out every frame of the page).
            if let Some(node) = anims
                .record_nodes()
                .into_iter()
                .chain(script.keys().copied().filter(|node| anims.visited(*node)))
                .find(|node| !anims.captured.contains_key(node))
            {
                return Err(format!("element {node} animates with no captured style"));
            }
            let mut nodes: Vec<usize> = anims.captured.keys().copied().collect();
            nodes.sort_unstable();
            nodes
        };
        // Values parsed from script's layer read the view (viewport units).
        let prev = self.building.replace(Some(Rc::new(view)));
        let result = (|| {
            let mut changes = Vec::new();
            for node in nodes {
                let captured = lay.animations.borrow().captured.get(&node).cloned();
                let Some(captured) = captured else { continue };
                let mut style = captured.cascaded.clone();
                // A `@keyframes` is resolved when its record is made, at a
                // build; a name with none (no rule) resolves to nothing at
                // every build, so nothing here either.
                lay.animations.borrow_mut().on_style(node, &mut style, &|_, _| None);
                let entries = script.get(&node).cloned();
                if entries != captured.script {
                    // An entry of a property the engine does not
                    // interpolate is applied whole: what it sets is not
                    // known to be paint.
                    let opaque = |e: &Option<Vec<(String, String, String)>>| -> Vec<(String, String)> {
                        e.iter()
                            .flatten()
                            .filter(|(p, _, _)| crate::animations::prop_indices(p).is_empty())
                            .map(|(p, v, _)| (p.to_ascii_lowercase(), v.clone()))
                            .collect()
                    };
                    if opaque(&entries) != opaque(&captured.script) {
                        return Err(format!("script animates a property of element {node} the frame cannot place"));
                    }
                }
                self.apply_script_layer(&mut style, entries.as_deref());
                let values = crate::animations::all_values(&style);
                let mut props = Vec::new();
                for (index, (was, is)) in captured.values.iter().zip(&values).enumerate() {
                    if was == is {
                        continue;
                    }
                    if !crate::animations::is_paint_prop(index) {
                        return Err(format!("`{}` animates", crate::animations::prop_name(index)));
                    }
                    props.push((index, was.clone(), is.clone()));
                }
                let settled = (captured.settled_opacity != style.animation_settled_opacity)
                    .then_some((captured.settled_opacity, style.animation_settled_opacity));
                if let Some(c) = lay.animations.borrow_mut().captured.get_mut(&node) {
                    c.values = values;
                    c.settled_opacity = style.animation_settled_opacity;
                    c.script = entries;
                }
                if !props.is_empty() || settled.is_some() {
                    changes.push(FrameChange { node, props, settled });
                }
            }
            Ok(changes)
        })();
        *self.building.borrow_mut() = prev;
        let mut anims = lay.animations.borrow_mut();
        anims.last_build_clock = Some(anims.clock_ms);
        result
    }

    /// What a forced layout of `lay`'s view would find borrowed, `None`
    /// when nothing is: a build in progress, or build state held by the
    /// engine code that called the script now reading geometry.
    pub(crate) fn busy_for_forced_layout(&self, lay: &ViewLayout) -> Option<&'static str> {
        let free = [
            (
                "building",
                self.building.try_borrow().is_ok_and(|b| b.is_none()),
            ),
            ("svg_cache", self.svg_cache.try_borrow_mut().is_ok()),
            (
                "svg_paint_styles",
                self.svg_paint_styles.try_borrow_mut().is_ok(),
            ),
            ("svg_prepass", self.svg_prepass.try_borrow_mut().is_ok()),
            ("style_trace", self.style_trace.try_borrow_mut().is_ok()),
            ("hover_chain", lay.hover_chain.try_borrow_mut().is_ok()),
            ("active_chain", lay.active_chain.try_borrow_mut().is_ok()),
            ("edit_states", lay.edit_states.try_borrow_mut().is_ok()),
            (
                "checked_states",
                lay.checked_states.try_borrow_mut().is_ok(),
            ),
            ("sheet_cache", lay.sheet_cache.try_borrow_mut().is_ok()),
            ("prepared_sheets", lay.prepared_sheets.try_borrow_mut().is_ok()),
            ("rule_reads", lay.rule_reads.try_borrow_mut().is_ok()),
            (
                "boxless_styles",
                lay.boxless_styles.try_borrow_mut().is_ok(),
            ),
            ("pseudo_styles", lay.pseudo_styles.try_borrow_mut().is_ok()),
        ];
        free.into_iter()
            .find(|(_, free)| !free)
            .map(|(name, _)| name)
    }

    pub(crate) fn building_id(&self) -> Option<EngineViewId> {
        self.building.borrow().as_ref().map(|b| b.id)
    }

    pub(crate) fn building_lay(&self) -> Option<Rc<ViewLayout>> {
        self.building.borrow().as_ref().and_then(|b| b.lay.clone())
    }

    /// `f` over the `ViewLayout` of the view being built, without the
    /// `Rc` clone of `building_lay`: for the lookups made once per node.
    pub(crate) fn with_building_lay<R>(&self, f: impl FnOnce(&ViewLayout) -> R) -> Option<R> {
        let building = self.building.borrow();
        building.as_ref()?.lay.as_deref().map(f)
    }

    pub(crate) fn building_focus(&self) -> Option<rustkit_dom::NodeId> {
        self.building.borrow().as_ref().and_then(|b| b.focus)
    }

    pub(crate) fn building_dpr(&self) -> f32 {
        self.building.borrow().as_ref().map_or(1.0, |b| b.dpr)
    }

    pub(crate) fn building_viewport(&self) -> Option<(f32, f32)> {
        self.building.borrow().as_ref().and_then(|b| b.viewport)
    }

    /// The owners of the building view's external sheets; empty outside
    /// a view build.
    pub(crate) fn building_external_sheet_owners(&self) -> Rc<Vec<rustkit_dom::NodeId>> {
        self.building.borrow().as_ref().map(|b| b.external_sheet_owners.clone()).unwrap_or_default()
    }

    pub(crate) fn building_url(&self) -> Option<Url> {
        self.building.borrow().as_ref().and_then(|b| b.url.clone())
    }
}

/// What the build reads and writes of one view, shared the same way
/// (`ViewState::lay`). The engine owns the writes outside a build; a build
/// only reads, apart from the three outputs at the end.
#[derive(Default)]
pub(crate) struct ViewLayout {
    /// Currently focused DOM node.
    pub(crate) focused_node: Cell<Option<rustkit_dom::NodeId>>,
    /// The element the pointer is over and the elements above it (raw
    /// NodeIds): what `:hover` matches in this view's next build.
    pub(crate) hover_chain: RefCell<Vec<usize>>,
    /// The element the press in progress landed on and the elements above
    /// it: what `:active` matches, from the press to the release.
    pub(crate) active_chain: RefCell<Vec<usize>>,
    /// CH2.E22: the page's focused element and the elements above it (a
    /// shadow root's host included), innermost first, raw NodeIds: what
    /// `:focus` (the first) and `:focus-within` (all) match in this view's
    /// next build. Empty when nothing is focused. The page's focus, so a
    /// focused button is here while `focused_node` (the control that takes
    /// the keys) is not.
    pub(crate) focus_chain: RefCell<Vec<usize>>,
    /// CH2.E22: whether the focused element matches `:focus-visible`.
    pub(crate) focus_visible: Cell<bool>,
    /// Live text-editing state for this view's form controls, keyed by raw
    /// `NodeId`.
    ///
    /// Per-VIEW, not engine-global: `NodeId` is per-DOCUMENT (every
    /// `Document` restarts its counter at 1), so a global map lets one
    /// document's node 4 collide with another's. Living on the view also
    /// makes the lifetime obvious — drop the view, drop the map — and gives
    /// document replacement one clear place to clear.
    ///
    /// This is the IDL-value side table browsers keep separate from the
    /// content attribute: `getAttribute("value")` is the authored default,
    /// this is the live value. Mutating the DOM instead would conflate the
    /// two and break form-reset semantics.
    ///
    /// Shared with the page's bindings (`DomBindings::share_edit_states`):
    /// `execCommand` and the user's edit commands run on these states, so
    /// script's undo undoes the user's typing.
    pub(crate) edit_states: rustkit_bindings::SharedEditStates,
    /// Checkedness of the checkboxes and radio buttons a click or script
    /// has set, keyed by raw NodeId: the other half of the same side table.
    /// A control missing here is checked when it has the `checked`
    /// attribute. Script owns the state (`input.checked`); style
    /// (`:checked`), paint and form submission read it here.
    pub(crate) checked_states: RefCell<HashMap<usize, bool>>,
    /// The `<select>` drop-down open in this view, if one is
    /// (`select_popup`). Its commands follow the page's in the display list.
    pub(crate) select_popup: RefCell<Option<crate::select_popup::SelectPopup>>,
    /// The press that closed a drop-down: its release is the popup's too,
    /// and goes nowhere on the page.
    pub(crate) select_popup_release: Cell<bool>,
    /// The press that opened the drop-down is still down: its release on
    /// an option picks it (press, drag, release), anywhere else it is the
    /// page's and the popup stays open.
    pub(crate) select_popup_opening_press: Cell<bool>,
    /// How many commands at the end of the view's display list are the
    /// popup's (0 when none).
    pub(crate) select_popup_painted: Cell<usize>,
    /// Type-to-select on a focused, closed drop-down: the select (raw
    /// NodeId) and what has been typed at it.
    pub(crate) select_typeahead: RefCell<(usize, crate::select_popup::TypeAhead)>,
    /// Where script scrolled this view's elements (shared with the bindings).
    pub(crate) element_scrolls: rustkit_bindings::SharedElementScrolls,
    /// The parsed `<style>` sheets of this view's document, by text.
    pub(crate) sheet_cache: RefCell<crate::SheetCache>,
    /// The sheets this view's last full build cascaded with, prepared
    /// (kept for the viewport, ranked by `@layer`) with their custom
    /// properties, under the identity of what they were made from. The
    /// next build takes them unless a sheet or the viewport changed.
    pub(crate) prepared_sheets: RefCell<Option<crate::PreparedSheets>>,
    /// The rule index this view was last built with: its `reads` say
    /// which elements a `:hover` or `:active` rule can match, so which
    /// changes of the two chains are a restyle.
    pub(crate) rule_reads: RefCell<Option<std::rc::Rc<crate::RuleIndex>>>,
    /// Whether `html` had a non-visible `overflow` at the last build. The
    /// viewport then takes html's, and a body with its own is a scroller
    /// that clips what it holds (see `scrollable_bottom`).
    pub(crate) html_clips: Cell<bool>,
    /// The computed styles of this view's elements that have no layout
    /// box: `display: contents` (hoisted out of the tree), `display: none`,
    /// an empty block or inline the box tree dropped, and the root, which
    /// is styled but not walked. Serialised (`computed_style_lines`) on
    /// the first script read, not at build time. Keyed by raw NodeId, so
    /// per view like `edit_states`: ids restart in every document, and a
    /// second view's build must not replace them (R1 M1 on #786). A full
    /// build replaces the map; a restyle keeps the entries it does not
    /// walk.
    pub(crate) boxless_styles: RefCell<HashMap<usize, BoxlessStyle>>,
    /// The cascaded `::before`/`::after` styles of this view's elements, by
    /// raw NodeId and pseudo name, for `getComputedStyle(el, pseudo)`. Kept
    /// like `boxless_styles`.
    pub(crate) pseudo_styles: RefCell<HashMap<(usize, &'static str), crate::ComputedStyle>>,
    /// The document and author svg paint map this view's last build
    /// styled its inline svgs with, for `getComputedStyle(fill / stroke)`
    /// on svg content (`svg_computed`, A2.1).
    pub(crate) svg_paint: RefCell<Option<Rc<crate::svg_computed::SvgPaintSnapshot>>>,
    /// The keys of the engine's `svg_cache` that this view's inline svg
    /// passes parsed for an svg holding a moving svg paint value (A2.2):
    /// one per frame, evicted by this view's next pass that no longer uses
    /// them, so a long transition keeps the cache flat. Per view: the cache
    /// is shared, and another view's pass must not evict what this one
    /// paints.
    pub(crate) svg_transient_keys: RefCell<Vec<String>>,
    /// MEDIA-AVF: each video's display size once its metadata is in, by raw NodeId:
    /// what its replaced box is built at (`media.rs`).
    pub(crate) media_sizes: RefCell<HashMap<usize, (u32, u32)>>,
    /// This view's running CSS transitions and the values they compare
    /// against, read and advanced by every build (`animations.rs`).
    pub(crate) animations: RefCell<crate::animations::Animations>,
    /// The animated-style layer script's Web Animations write (shared
    /// with the bindings), overlaid above the CSS animations, and the
    /// engine's CSS animations as script sees them.
    pub(crate) animated: rustkit_bindings::SharedAnimatedStyles,
    /// CH2.E7: the `overflow: auto` scrollers the last layout gave a styled
    /// bar (raw NodeId → vertical, horizontal), and whether the viewport
    /// had one. The next layout starts from them, so a relayout does not
    /// pay the overflow pass again, and a bar stays once shown, as in
    /// Chrome. Cleared with the document.
    pub(crate) scrollbar_marks: RefCell<HashMap<usize, (bool, bool)>>,
    pub(crate) root_scrollbar: Cell<bool>,
    /// CQ1: the content-box size of every size container in this view's
    /// last layout (raw NodeId → inline, block). The next build answers
    /// `@container` queries with them (`container_query`), and
    /// `lay_out_document` lays out again while they move. Cleared with the
    /// document.
    pub(crate) container_sizes: RefCell<crate::container_query::MeasuredContainers>,
    /// Whether the last build's sheets had any `@container` rule.
    pub(crate) container_rules: Cell<bool>,
    /// The extra build-and-layout passes container sizes have cost this
    /// view, and the layouts that hit the pass cap still moving (#1070 R1).
    /// What the last walk left: the size containers `container_sizes`
    /// measures, and what the build asked of them
    /// (`container_query::take_build_facts`).
    pub(crate) container_facts: RefCell<crate::container_query::BuildFacts>,
    pub(crate) container_passes: Cell<u32>,
    /// LAT22 probe: why reads built the tree instead of being answered
    /// from an animation frame, by reason.
    pub(crate) forced_build_reasons: RefCell<std::collections::BTreeMap<String, u32>>,
    /// LAT22 probe: the shadow trees' sheet cost in the view's last build.
    pub(crate) last_shadow_build: Cell<crate::shadow_tree::ShadowBuildStats>,
    /// FORCED-READ: the elements the view's last build walked (styled and given an identity).
    pub(crate) last_build_elements: Cell<u32>,
    /// FORCED-READ: the document's forced builds so far, the time in their build stage and in
    /// their layout stage, in ms (the relayout summary takes them).
    pub(crate) forced_stage_ms: Cell<(f64, f64)>,
    /// Element cascades of the view's last build that had no rule index.
    pub(crate) last_unindexed_cascades: Cell<u32>,
    /// LAT22: the document's shadow tree styles across builds.
    pub(crate) tree_styles: Rc<RefCell<crate::shadow_tree::ViewTreeStyles>>,
    pub(crate) container_unsettled: Cell<u32>,
    /// Builds of this view that told the walk they replay memoized styles
    /// (`container_query::note_replaying`); read by the wiring test.
    pub(crate) container_replays: Cell<u32>,
    /// How many layout passes this view's builds have run in all (one per
    /// build, or more while styled bars appear or go), and how many builds
    /// ran them. Read by tests to pin that a relayout takes one pass.
    pub(crate) layout_passes: Cell<u32>,
    pub(crate) layout_builds: Cell<u32>,
    /// INC5: this view's last layout, kept for the incremental path while
    /// `RUSTKIT_INCREMENTAL_LAYOUT` is not `off`, and what that path did.
    pub(crate) incremental_layout: RefCell<Option<crate::incremental_layout::Retained>>,
    pub(crate) incremental_stats: RefCell<crate::incremental_layout::Stats>,
    /// INC5 stage 2: the coming relayout's dirty set changed no style and no
    /// box, so its build may be skipped (`Engine::relayout` sets it, the
    /// next build takes it).
    pub(crate) incremental_skip_build: Cell<bool>,
    /// LPF1: the view's last full build as built (before layout), kept while script writes
    /// change only paint (`want_script_unlaid`), so a relayout for such writes builds only the
    /// written elements and compares them with their boxes here; patched along with the laid-out
    /// tree. `None` after any full build that was not asked to keep it.
    pub(crate) script_unlaid: RefCell<Option<rustkit_layout::LayoutBox>>,
    pub(crate) want_script_unlaid: Cell<bool>,
    /// TREE-REUSE (PR 3): the elements this relayout's script writes reached, when its build may
    /// build them alone and splice them into `script_unlaid` (`Engine::relayout` sets it while
    /// `RUSTKIT_RETAINED_STYLES=boxes`; the relayout's own build takes it).
    pub(crate) splice_roots: RefCell<Option<Vec<usize>>>,
    /// A transition or animation was moving when the kept tree (`script_unlaid`) was built, so
    /// boxes no later write reaches may hold a value it has moved on from, or that went when it
    /// ended: the kept tree is not spliced into (#1247 R1 item 2).
    pub(crate) script_unlaid_animated: Cell<bool>,
    /// The script animation layer's generation (`animated.generation`) when the kept tree was
    /// built. Script animations (`element.animate`) are applied in the build from that layer and
    /// can start, move or fill in paint-only frames no build sees; when the layer has been written
    /// since, the kept tree is not spliced into (#1247 delta R1).
    pub(crate) script_unlaid_generation: Cell<u64>,
    /// The image cache's generation (`ImageManager::cache_generation`) when the kept tree was built.
    /// An image that arrives later is laid out by its own relayout (`ImageLoaded`, a full build), but
    /// a script relayout can run before it; the kept tree holds the image's natural size as of its
    /// build, so once any image has arrived since, the kept tree is not spliced into (BOXES-DIFF).
    pub(crate) script_unlaid_images: Cell<u64>,
    /// TREE-REUSE step (1): set by a forced layout (a geometry read after a write) for its own
    /// build while `RUSTKIT_RETAINED_STYLES=boxes`: that build keeps its tree, built from the DOM as
    /// it was at the read, so the end-of-task relayout splices the task's writes into it instead of
    /// building the whole document a second time. Its dirty set holds every write of the task, those
    /// before the read too, so building them again over the forced tree is exact.
    pub(crate) keep_forced_tree_now: Cell<bool>,
    /// The kept tree is a forced layout's, not the one the view laid out and published. LPF1's short
    /// path compares written boxes with the kept tree and patches the published tree to match, which
    /// is right only when the two are the same tree: it does not take a forced read's.
    pub(crate) script_unlaid_unpublished: Cell<bool>,
    /// Set by `Engine::relayout_uncounted` for its own build only: the tree a relayout builds and
    /// publishes is the one kept. Any other build (a forced layout for a geometry read) drops the
    /// kept tree, since the published boxes were not laid out from it, except a forced layout's
    /// in `boxes` mode (`keep_forced_tree_now`).
    pub(crate) retain_script_unlaid_now: Cell<bool>,
    /// After a script write that changed more than paint, the relayouts the short path sits out
    /// (no kept tree, no attempt) before it tries again: a page that keeps changing layout does
    /// not pay for a kept copy of its tree every frame.
    pub(crate) script_cooldown: Cell<u32>,
    /// LPF3: the cooldown the short path's last miss set, while its run of misses goes on; `None`
    /// once it has ended (a hit, `SCRIPT_RESTYLE_IDLE` relayouts with no script paint write, a
    /// navigation). A miss with a run going is one in a row: it sits out twice the last cooldown
    /// (at least 1, at most `SCRIPT_RESTYLE_COOLDOWN`); a one-off miss sits out none. A relayout
    /// the path refuses leaves the run as it is (R1: a scroll between two misses ended it).
    pub(crate) script_last_cooldown: Cell<Option<u32>>,
    /// A window or element scroll happened in the task that dirtied the page: the short path,
    /// which re-stamps no scroll offset, is not taken for the relayout (R1). Cleared by it.
    pub(crate) scrolled_in_task: Cell<bool>,
    /// Relayouts in a row that no script paint write asked for; at 8 the kept tree is let go.
    pub(crate) script_idle: Cell<u32>,
    /// The written elements (raw NodeIds) the coming relayout may restyle alone
    /// (`Engine::relayout` sets it, `relayout_uncounted` takes it), and the child nodes the last
    /// such restyle's build went into.
    pub(crate) script_roots: RefCell<Option<Vec<usize>>>,
    pub(crate) script_walked: Cell<Option<usize>>,
    /// LAT9: the URL each `<img>` (raw NodeId) last painted with a loaded
    /// image. When script swaps its `src`, the box keeps painting this one
    /// while the new image is in flight, so a card does not go blank
    /// between the two. Once the new one has settled as failed
    /// ([`Self::failed_images`]) the old image goes, as in Chrome. Per view
    /// and cleared with the document, like the other NodeId tables.
    pub(crate) shown_images: RefCell<HashMap<usize, ImgSource>>,
    /// LAT9-r1: the image URLs of this document whose load has settled as
    /// failed, and which `<img>` elements have heard `error` for theirs.
    pub(crate) failed_images: RefCell<FailedImages>,
    /// E54: which `<img>` elements have heard `load`, and for which image.
    pub(crate) loaded_images: RefCell<LoadedImages>,
}

impl ViewLayout {
    /// A navigation committed: the script restyle path starts the new document afresh. No kept
    /// tree (its node ids were the old document's), no cooldown, and no run of misses (LPF3 R1).
    pub(crate) fn forget_script_restyle(&self) {
        self.script_unlaid.borrow_mut().take();
        self.script_unlaid_animated.set(false);
        self.script_unlaid_unpublished.set(false);
        self.want_script_unlaid.set(false);
        self.script_cooldown.set(0);
        self.script_last_cooldown.set(None);
        self.script_idle.set(0);
    }
}

/// The current document's loaded images, as its script has been told of
/// them ([`ViewLayout::loaded_images`]). An `<img>` whose image is in
/// hears `load` once for that image; script reads `complete`,
/// `naturalWidth`/`naturalHeight` and `currentSrc` from what the event
/// brought, so an image reads as in flight until its `load`.
#[derive(Default)]
pub(crate) struct LoadedImages {
    /// An image arrived, an `<img>` was found showing a loaded image it has
    /// not heard of, or script wrote a `src`: the next live turn (or the
    /// load, before the window's `load`) has events to send.
    pub(crate) due: bool,
    /// The URL each `<img>` (raw NodeId) has heard `load` for, as its
    /// [`Self::key`]. An element whose `src` moves on, or is written again,
    /// is dropped, so it hears the next one.
    pub(crate) heard: HashMap<usize, u64>,
    /// The `<img>` elements (raw NodeId) script gave a `src` that are in
    /// no document tree (`new Image()`, a created element not inserted): a
    /// preload or a pixel. Each is fetched and hears `load` or `error`
    /// like one in the document; it leaves the list once it has heard, and
    /// comes back if its `src` is written again.
    pub(crate) detached: Vec<usize>,
    /// The URLs the engine gave up on (an image's budget ran out, or it got
    /// no slot within its pass's cap) and which `<img>` elements were told
    /// so, by [`Self::key`]. Not a failure: no event fires, and the element
    /// reads as loading, but a `decode()` waiting on it resolves (E54 R1).
    /// A pass that asks for such a URL again takes it out.
    pub(crate) gave_up: std::collections::HashSet<String>,
    pub(crate) gave_up_heard: HashMap<usize, u64>,
}

impl LoadedImages {
    /// What [`Self::heard`] keeps of a URL: a hash. A `data:` URL can be
    /// megabytes, and a page can show hundreds of them, each looked at on
    /// every planning pass (R1 nit 4), so a long URL is hashed by its length
    /// and its first and last 256 bytes, not all of it.
    pub(crate) fn key(url: &str) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let bytes = url.as_bytes();
        if bytes.len() <= 1024 {
            bytes.hash(&mut hasher);
        } else {
            bytes.len().hash(&mut hasher);
            bytes[..256].hash(&mut hasher);
            bytes[bytes.len() - 256..].hash(&mut hasher);
        }
        hasher.finish()
    }
}

/// The current document's failed image loads ([`ViewLayout::failed_images`]).
///
/// A URL is in `urls` only once its load has failed: an error status, a
/// failed connection, bytes that do not decode (`FetchedImages::failed`).
/// A request that is still out is not in here, however long it has been
/// out, nor is one the load stopped waiting for (that one goes on as live
/// work). Nor is one the engine gave up on, its own budget spent or no slot
/// within the pass's cap (`FetchedImages::gave_up`): that image was slow,
/// not broken, Chrome would still load it, and its element keeps what it
/// paints and hears nothing. A pass that asks for a failed URL again takes
/// it out when it starts the request.
///
/// `error` goes to the `<img>` elements in the document and to those
/// script gave a `src` outside it ([`LoadedImages::detached`]).
#[derive(Default)]
pub(crate) struct FailedImages {
    /// Absolute URLs.
    pub(crate) urls: std::collections::HashSet<String>,
    /// A failure arrived, or the document's images were looked over again,
    /// since `error` was last sent: the next live turn has events to send.
    pub(crate) due: bool,
    /// The failed URL each `<img>` (raw NodeId) has heard `error` for. An
    /// element whose `src` moves on is dropped, so it hears the next one.
    pub(crate) heard: HashMap<usize, String>,
}

/// An `<img>`'s `src` and the image it painted last ([`ViewLayout::shown_images`]).
pub(crate) struct ImgSource {
    /// The absolute URL its `src` (or `srcset` pick) names now.
    pub(crate) wanted: String,
    /// The density (image pixels per CSS pixel) of the pick `wanted` names (E54b): `srcset` picks
    /// a 2x image for a 1x box, and its natural size is the decoded size over that density.
    pub(crate) wanted_density: f32,
    /// The last loaded image it painted.
    pub(crate) shown: Option<String>,
    /// The density of the pick `shown` came from. During a pending swap the box paints `shown`,
    /// so its natural size divides by THIS, never by the new pick's `wanted_density`.
    pub(crate) shown_density: f32,
}

/// An image with no pick yet is 1x: a derived `Default` would be 0.0, which divides a natural
/// size by zero.
impl Default for ImgSource {
    fn default() -> Self {
        Self { wanted: String::new(), wanted_density: 1.0, shown: None, shown_density: 1.0 }
    }
}

/// What `getComputedStyle` reads for an element with no layout box
/// (`ViewLayout::boxless_styles`).
pub(crate) struct BoxlessStyle {
    /// A box holding the element's computed style and no geometry; its
    /// `node_id` is the element's.
    pub(crate) box_: crate::LayoutBox,
    /// Whether the element's custom-property set is its own rather than
    /// its parent's (`Arc`), so the read publishes the set with it.
    pub(crate) own_vars: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_img_source_with_no_pick_yet_is_1x() {
        // A derived Default gave 0.0, which divides a natural size by zero if either density is
        // read before a pick records one (Prometheus LOW on #1039).
        let source = ImgSource::default();
        assert_eq!(source.wanted_density, 1.0);
        assert_eq!(source.shown_density, 1.0);
    }

    fn build_view(lay: &Rc<ViewLayout>) -> BuildView {
        BuildView {
            id: EngineViewId::new(),
            lay: Some(lay.clone()),
            url: None,
            dpr: 1.0,
            viewport: None,
            focus: None,
            external_sheets: Rc::default(),
            external_sheet_owners: Rc::default(),
        }
    }

    #[test]
    fn a_view_for_a_later_build_keeps_the_per_build_fields_and_rereads_focus() {
        let lay = Rc::new(ViewLayout::default());
        let mut taken = build_view(&lay);
        taken.dpr = 2.0;
        taken.viewport = Some((800.0, 600.0));
        taken.url = Url::parse("https://example.com/a").ok();
        let focused = rustkit_dom::NodeId::new(7);
        lay.focused_node.set(Some(focused));
        let later = taken.for_build();
        assert_eq!(later.id, taken.id);
        assert!(Rc::ptr_eq(later.lay.as_ref().unwrap(), &lay));
        assert_eq!(later.url, taken.url);
        assert_eq!(later.viewport, Some((800.0, 600.0)));
        assert_eq!(later.dpr, 2.0);
        assert_eq!(later.focus, Some(focused));
        assert!(Rc::ptr_eq(&later.external_sheets, &taken.external_sheets));
        assert!(Rc::ptr_eq(&later.external_sheet_owners, &taken.external_sheet_owners));
    }

    #[test]
    fn a_build_that_panics_leaves_no_view_held() {
        let ctx = LayoutCtx::default();
        let lay = Rc::new(ViewLayout::default());
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ctx.with_building(build_view(&lay), || {
                assert!(ctx.building_id().is_some());
                panic!("a build that fails");
            })
        }));
        assert!(unwound.is_err());
        assert!(ctx.building.borrow().is_none());
        assert_eq!(Rc::strong_count(&lay), 1, "the ViewLayout is released");
    }

    #[test]
    fn a_build_inside_a_build_puts_the_outer_view_back() {
        let ctx = LayoutCtx::default();
        let lay = Rc::new(ViewLayout::default());
        let outer = build_view(&lay);
        let outer_id = outer.id;
        ctx.with_building(outer, || {
            ctx.with_building(build_view(&lay), || {
                assert_ne!(ctx.building_id(), Some(outer_id))
            });
            assert_eq!(ctx.building_id(), Some(outer_id));
        });
        assert!(ctx.building.borrow().is_none());
        assert_eq!(Rc::strong_count(&lay), 1);
    }

    /// #812: a forced layout must decline while a build holds the ctx, so a
    /// geometry read nested inside layout answers from the last published
    /// frame instead of re-entering.
    #[test]
    fn busy_for_forced_layout_names_a_held_build() {
        let ctx = LayoutCtx::default();
        let lay = Rc::new(ViewLayout::default());
        assert_eq!(ctx.busy_for_forced_layout(&lay), None);
        ctx.with_building(build_view(&lay), || {
            assert_eq!(ctx.busy_for_forced_layout(&lay), Some("building"));
        });
        assert_eq!(ctx.busy_for_forced_layout(&lay), None);
    }
}

/// What an animation frame moved on one element (`LayoutCtx::animation_frame`).
#[derive(Clone, Debug)]
pub(crate) struct FrameChange {
    pub(crate) node: usize,
    /// Paint properties, by animatable index: the value painted last, and
    /// the value now.
    pub(crate) props: Vec<(usize, crate::animations::AnimValue, crate::animations::AnimValue)>,
    /// `animation_settled_opacity`, painted last and now.
    pub(crate) settled: Option<(Option<f32>, Option<f32>)>,
}

/// Whether the build `with_building` just ran visited only part of the document, so the animation
/// state keeps what it did not visit (`Animations::end_build`): `started_partial` (a pointer
/// restyle set its restyle build before the build), or the last build ran under a restyle build. A
/// kept-tree splice (TREE-REUSE) starts its restyle build inside the build, so only the second
/// sees it; a splice that missed after its partial build, and `boxes-verify`, build in full
/// afterwards, which says otherwise. `boxes-verify` asks it too, for what a splice's sweep would
/// drop (`Animations::would_sweep`).
pub(crate) fn build_was_partial(started_partial: bool) -> bool {
    #[cfg(test)]
    if IGNORE_LAST_BUILD_PARTIAL.with(|c| c.get()) {
        return started_partial;
    }
    started_partial || crate::relayout_cause::last_build_partial()
}

#[cfg(test)]
thread_local! {
    /// Test hook: `build_was_partial` as before #1257's fix (a splice swept like a full build), so a
    /// test can show that `boxes-verify` sees that class of bug.
    pub(crate) static IGNORE_LAST_BUILD_PARTIAL: Cell<bool> = const { Cell::new(false) };
}
