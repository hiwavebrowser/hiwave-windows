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
}

impl Default for LayoutCtx {
    fn default() -> Self {
        Self {
            image_manager: Arc::new(ImageManager::new()),
            font_loader: Arc::new(FontLoader::new()),
            style_trace: RefCell::default(),
            building: RefCell::default(),
            svg_cache: RefCell::default(),
            svg_paint_styles: RefCell::default(),
            svg_prepass: RefCell::default(),
        }
    }
}

/// What a build reads of the view it is for, taken by the engine when the
/// build starts.
pub(crate) struct BuildView {
    pub(crate) id: EngineViewId,
    /// `None` when the view is not (or no longer) in the engine.
    pub(crate) lay: Option<Rc<ViewLayout>>,
    /// The document's URL: relative resource URLs and the font partition.
    pub(crate) url: Option<Url>,
    /// The viewport in CSS px: the headless bounds, else the host surface.
    pub(crate) viewport: Option<(f32, f32)>,
    /// Focus is per-VIEW but the recursive layout builder is view-agnostic,
    /// so the alternative was checking "is this node focused in ANY view",
    /// which would mark a node focused in one view while building another.
    pub(crate) focus: Option<rustkit_dom::NodeId>,
    /// The view's `<link>` sheets: the rule-index fingerprint vouches for
    /// them only when the build was handed this very list.
    pub(crate) external_sheets: Rc<Vec<rustkit_css::Stylesheet>>,
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
        let _walk_checked = crate::WalkCheckedScope::enter(Some(checked), Some(pointer));
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
        let prev = self.building.replace(Some(Rc::new(view)));
        let _restore = Restore { ctx: self, prev };
        let built = build();
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
    /// of the properties they set read back.
    pub(crate) fn animate_style(
        &self,
        node: usize,
        style: &mut crate::ComputedStyle,
        stylesheets: &[rustkit_css::Stylesheet],
    ) {
        let lay = self
            .building
            .borrow()
            .as_ref()
            .and_then(|view| view.lay.clone());
        let Some(lay) = lay else {
            return;
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
        let cascaded = (named || !style.transitions.is_empty() || entries.is_some() || {
            lay.animations.borrow().has_records(node)
        })
        .then(|| style.clone());
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
            if let Some(node) = anims
                .record_nodes()
                .into_iter()
                .chain(script.keys().copied())
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

    pub(crate) fn building_viewport(&self) -> Option<(f32, f32)> {
        self.building.borrow().as_ref().and_then(|b| b.viewport)
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
    /// How many layout passes this view's builds have run in all (one per
    /// build, or more while styled bars appear or go), and how many builds
    /// ran them. Read by tests to pin that a relayout takes one pass.
    pub(crate) layout_passes: Cell<u32>,
    pub(crate) layout_builds: Cell<u32>,
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

    fn build_view(lay: &Rc<ViewLayout>) -> BuildView {
        BuildView {
            id: EngineViewId::new(),
            lay: Some(lay.clone()),
            url: None,
            viewport: None,
            focus: None,
            external_sheets: Rc::default(),
        }
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
