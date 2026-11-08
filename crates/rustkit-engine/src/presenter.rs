//! The presentation half of the engine: the view host, the compositor
//! (surfaces and headless textures) and the renderer.
//!
//! Threads plan M3, first PR (the pure move). Before this module the
//! `Engine` held `viewhost`, `compositor` and `renderer` beside the page
//! state (documents, bindings, layout, display lists), and `render`
//! reached into both. Now everything that touches a window, a GPU surface
//! or the renderer lives here, and the page side hands it what to draw:
//! a display list, a scroll offset and a frame generation. The engine
//! still calls it on the same thread, in the same order, with the same
//! arguments as before; nothing about a frame changes. The second PR
//! makes the hand-over a `FrameCommit` and gives the presenter its own
//! latest-commit-per-view store, which is what a page thread (M4) needs.
//!
//! Bodies are moved from `lib.rs` as they were, so the diff of the first
//! PR is read as a move. Where a body read a `ViewState` field, the field
//! is now an argument.
//!
//! Second PR: the hand-over is a [`FrameCommit`], an immutable record the
//! page side builds when its frame changes (a new display list, a new
//! scroll offset) and the presenter keeps as the newest commit per view
//! (`THREAD_OWNERSHIP_2026-10.md` §3 option A, "How a frame gets to the
//! screen"; threads plan invariant 4). `present` draws a view's latest
//! commit and nothing else: it never reads the page's `ViewState`, so at
//! M4 the commit can arrive over a channel from a page thread and the
//! window thread keeps drawing the last frame it was given while the
//! page is busy. Everything a commit holds is `Send + Sync`
//! (`send_bounds_tests`). The W-gate log (`wgate.rs`) times the
//! presenter's own stages and reports input-to-present latency here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use rustkit_compositor::Compositor;
use rustkit_image::ImageManager;
use rustkit_layout::DisplayList;
use rustkit_renderer::{RenderStats, Renderer};
use rustkit_viewhost::{Bounds, ViewHost, ViewHostTrait, ViewId};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use rustkit_viewhost::WindowHandle;
use tracing::{debug, info, trace, warn};
use url::Url;

#[cfg(windows)]
use windows::Win32::Foundation::HWND;

use super::wgate::{self, HoldStage, InputLatency, InputMark};
use super::{EngineError, EngineViewId, PresentedFrame};

/// One frame, as the page side hands it over: everything the presenter
/// needs to draw it, and nothing the page side can change afterwards.
/// The display list is shared by `Arc` (the page keeps its own handle for
/// exports and hit tests); a commit holds it alive after the page has
/// moved on, so the window can keep drawing the last frame of a page
/// that is mid-relayout or replaced (invariant 11 in part: images, glyph
/// runs and SVGs are the R1 audit's).
#[derive(Debug, Clone)]
pub struct FrameCommit {
    /// `ViewState::frame_generation` of `list`: counts the display lists
    /// the view has been given.
    pub generation: u64,
    /// `ViewState::nav_id` (M3b) of the document `list` was built from: a
    /// commit from a navigation the page has since left is still drawable,
    /// and still named.
    pub nav_generation: u64,
    /// `None` before the view's first layout: the background is drawn.
    pub list: Option<Arc<DisplayList>>,
    /// The scroll offset the frame is drawn at, in CSS pixels.
    pub scroll_offset: (f32, f32),
    /// The largest offset the page allows (`ViewState::max_scroll_offset`),
    /// so a presenter-side scroll (M4b) can clamp without asking the page.
    pub max_scroll_offset: (f32, f32),
    /// The viewport the list was laid out for, in CSS pixels (0 x 0 before
    /// the first layout).
    pub laid_out_for: (u32, u32),
    /// The earliest input the page took since its last commit, for the
    /// input-to-present log.
    pub input: Option<InputMark>,
    /// The page's canvas colour (CSS 2.1 §14.2), as the window should
    /// clear a surface that is larger than `laid_out_for`. Opaque; a
    /// transparent canvas is white.
    pub canvas: [f64; 4],
    /// A `<select>` drop-down is open in this frame (CH2.E6b). Like the
    /// platform popup it stands for, it takes every press while open: a
    /// press outside it closes it and goes nowhere else, so a view with a
    /// region (the RustKit chrome over the page) is hit everywhere.
    pub modal_popup: bool,
}

/// The window thread's half of the engine: every NSView/HWND, every GPU
/// surface and the one renderer. See the module doc.
pub(crate) struct Presenter {
    viewhost: ViewHost,
    compositor: Compositor,
    renderer: Option<Renderer>,
    /// The newest commit of each view: what `present` draws.
    commits: HashMap<EngineViewId, FrameCommit>,
    /// Input-to-present samples (W-gate).
    latency: InputLatency,
    /// Views whose most recent render attempt failed. A view entering this
    /// set logs one `warn!`; leaving it logs recovery. Without this, a
    /// persistently failing render (e.g. a wedged surface) freezes the
    /// screen while the log stays silent.
    render_failing: HashSet<EngineViewId>,
    /// `EngineConfig::background_color`: what a view shows before its
    /// first frame.
    background_color: [f64; 4],
    /// Host views drawn as transparent surfaces (CH3.A3c).
    transparent: HashSet<ViewId>,
    /// Images handed to the renderer so far (tests: one upload per image,
    /// however often the frame is drawn).
    #[cfg(test)]
    pub(crate) images_uploaded: usize,
}

impl Presenter {
    /// The view host, the compositor and the renderer, as `Engine::new`
    /// built them.
    pub(crate) fn new(background_color: [f64; 4]) -> Result<Self, EngineError> {
        // Initialize ViewHost
        let viewhost = ViewHost::new();

        // Initialize Compositor (unit tests serialise device creation; see
        // `test_compositor`)
        #[cfg(test)]
        let compositor = super::test_compositor();
        #[cfg(not(test))]
        let compositor = Compositor::new();
        let compositor = compositor.map_err(|e| EngineError::RenderError(e.to_string()))?;

        // Initialize Renderer
        let renderer = Renderer::new(
            compositor.device_arc(),
            compositor.queue_arc(),
            compositor.surface_format(),
        )
        .map_err(|e| EngineError::RenderError(e.to_string()))?;

        info!(
            adapter = ?compositor.adapter_info().name,
            "Engine initialized with GPU renderer"
        );

        Ok(Self {
            viewhost,
            compositor,
            renderer: Some(renderer),
            commits: HashMap::new(),
            latency: InputLatency::default(),
            render_failing: HashSet::new(),
            background_color,
            transparent: HashSet::new(),
            #[cfg(test)]
            images_uploaded: 0,
        })
    }

    /// A presenter with a compositor and no renderer: what the layout-only
    /// test engines built by hand (`renderer: None`).
    #[cfg(test)]
    pub(crate) fn layout_only(compositor: Compositor) -> Self {
        Self {
            viewhost: ViewHost::new(),
            compositor,
            renderer: None,
            commits: HashMap::new(),
            latency: InputLatency::default(),
            render_failing: HashSet::new(),
            background_color: super::EngineConfig::default().background_color,
            transparent: HashSet::new(),
            images_uploaded: 0,
        }
    }

    // ----- commits -------------------------------------------------------

    /// Take `commit` as the newest frame of view `id`; the one before it
    /// is dropped (with the last `Arc` to its list, when the page side has
    /// moved on too).
    pub(crate) fn commit(&mut self, id: EngineViewId, commit: FrameCommit) {
        self.commits.insert(id, commit);
    }

    /// The newest commit of view `id`, if it has had one.
    pub(crate) fn latest(&self, id: EngineViewId) -> Option<&FrameCommit> {
        self.commits.get(&id)
    }

    /// Input-to-present samples recorded so far (tests).
    #[cfg(test)]
    pub(crate) fn latency_samples(&self) -> u64 {
        self.latency.samples
    }

    // ----- views and surfaces -------------------------------------------

    /// A host view (NSView / HWND child) in `parent`.
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    pub(crate) fn create_host_view(
        &self,
        parent: WindowHandle,
        bounds: Bounds,
    ) -> Result<ViewId, EngineError> {
        <ViewHost as ViewHostTrait>::create_view(&self.viewhost, parent, bounds)
            .map_err(|e| EngineError::ViewError(e.to_string()))
    }

    /// The GPU surface of a host view.
    #[cfg(target_os = "windows")]
    pub(crate) fn attach_surface(&mut self, viewhost_id: ViewId, bounds: Bounds) -> Result<(), EngineError> {
        // Create compositor surface
        let hwnd = <ViewHost as ViewHostTrait>::get_hwnd(&self.viewhost, viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))?;

        unsafe {
            self.compositor
                .create_surface_for_hwnd(viewhost_id, hwnd, bounds.width, bounds.height)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;
        }
        Ok(())
    }

    /// The GPU surface of a host view.
    #[cfg(target_os = "macos")]
    pub(crate) fn attach_surface(&mut self, viewhost_id: ViewId, bounds: Bounds) -> Result<(), EngineError> {
        // Get raw window handle for compositor
        let raw_handle =
            <ViewHost as ViewHostTrait>::get_raw_window_handle(&self.viewhost, viewhost_id)
                .map_err(|e| EngineError::ViewError(e.to_string()))?;

        // Create compositor surface
        unsafe {
            self.compositor
                .create_surface_for_raw_handle(viewhost_id, raw_handle, bounds.width, bounds.height)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;
        }
        Ok(())
    }

    /// A headless texture in place of a surface.
    #[cfg(any(test, feature = "headless"))]
    pub(crate) fn create_headless_texture(&mut self, viewhost_id: ViewId, bounds: Bounds) -> Result<(), EngineError> {
        self.compositor
            .create_headless_texture(viewhost_id, bounds.width, bounds.height)
            .map_err(|e| EngineError::RenderError(e.to_string()))
    }

    /// The background colour, before the view's first frame.
    pub(crate) fn clear_to_background(&self, viewhost_id: ViewId) -> Result<(), EngineError> {
        self.compositor
            .render_solid_color(viewhost_id, self.background_color)
            .map_err(|e| EngineError::RenderError(e.to_string()))
    }

    /// Whether `viewhost_id` is drawn as a transparent surface (CH3.A3c,
    /// the RustKit chrome over the page): its frame is cleared to
    /// (0, 0, 0, 0) instead of its canvas, so what it does not paint shows
    /// the views below; the surface is premultiplied and not opaque.
    pub(crate) fn set_transparent(&mut self, viewhost_id: ViewId, transparent: bool) {
        if transparent {
            self.transparent.insert(viewhost_id);
        } else {
            self.transparent.remove(&viewhost_id);
        }
        match self.compositor.set_surface_transparent(viewhost_id, transparent) {
            Ok(_) => {}
            Err(error) => warn!(%error, "surface alpha mode not changed"),
        }
    }

    /// The colour a view shows before its first frame: its background, or
    /// nothing for a transparent view.
    pub(crate) fn clear_to_empty(&self, viewhost_id: ViewId) -> Result<(), EngineError> {
        let colour = if self.transparent.contains(&viewhost_id) { [0.0; 4] } else { self.background_color };
        self.compositor
            .render_solid_color(viewhost_id, colour)
            .map_err(|e| EngineError::RenderError(e.to_string()))
    }

    /// A headless view's texture as RGBA, alpha kept (tests).
    #[cfg(test)]
    pub(crate) fn read_headless_rgba(&self, viewhost_id: ViewId) -> Result<(u32, u32, Vec<u8>), EngineError> {
        self.compositor
            .read_headless_rgba(viewhost_id)
            .map_err(|e| EngineError::RenderError(e.to_string()))
    }

    /// Drop a view's surface and host view, and forget its render state.
    pub(crate) fn destroy_view(&mut self, id: EngineViewId, viewhost_id: ViewId) {
        self.render_failing.remove(&id);
        self.commits.remove(&id);

        // Destroy compositor surface
        let _ = self.compositor.destroy_surface(viewhost_id);

        // Destroy viewhost view
        let _ = <ViewHost as ViewHostTrait>::destroy_view(&self.viewhost, viewhost_id);
    }

    /// The surface half of a resize: the viewhost's bounds and the GPU
    /// surface (or the headless texture) take the new size.
    pub(crate) fn apply_bounds(
        &mut self,
        viewhost_id: ViewId,
        is_headless: bool,
        bounds: Bounds,
    ) -> Result<(), EngineError> {
        if is_headless {
            // Headless view: recreate headless texture with new size
            // First destroy old texture
            self.compositor.destroy_headless_texture(viewhost_id).ok(); // Ignore errors if it doesn't exist

            // Create new texture with new size
            self.compositor
                .create_headless_texture(viewhost_id, bounds.width, bounds.height)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;
        } else {
            // Regular view: resize viewhost and surface
            self.viewhost
                .set_bounds(viewhost_id, bounds)
                .map_err(|e| EngineError::ViewError(e.to_string()))?;

            self.compositor
                .resize_surface(viewhost_id, bounds.width, bounds.height)
                .map_err(|e| EngineError::RenderError(e.to_string()))?;
        }
        Ok(())
    }

    /// The host view's bounds (a headless view has none here; the page
    /// side keeps those).
    /// The platform view pointer (`NSView` on macOS). 0 when the view is gone.
    pub(crate) fn raw_view(&self, viewhost_id: ViewId) -> isize {
        self.viewhost.raw_view(viewhost_id)
    }

    pub(crate) fn host_bounds(&self, viewhost_id: ViewId) -> Result<Bounds, EngineError> {
        self.viewhost
            .get_bounds(viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))
    }

    /// The surface's (or headless texture's) size in device pixels.
    pub(crate) fn surface_size(&self, viewhost_id: ViewId) -> Result<(u32, u32), EngineError> {
        self.compositor
            .get_surface_size(viewhost_id)
            .map_err(|e| EngineError::RenderError(e.to_string()))
    }

    /// Give the host view keyboard focus (the window system's half of
    /// focus; see `Engine::grab_keyboard`).
    pub(crate) fn grab_keyboard(&self, viewhost_id: ViewId) {
        let _ = <ViewHost as ViewHostTrait>::focus_view(&self.viewhost, viewhost_id);
    }

    pub(crate) fn focus(&self, viewhost_id: ViewId) -> Result<(), EngineError> {
        self.viewhost
            .focus(viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))
    }

    pub(crate) fn set_visible(&self, viewhost_id: ViewId, visible: bool) -> Result<(), EngineError> {
        self.viewhost
            .set_visible(viewhost_id, visible)
            .map_err(|e| EngineError::ViewError(e.to_string()))
    }

    /// Get the native window handle (HWND) for a view.
    #[cfg(windows)]
    pub(crate) fn hwnd(&self, viewhost_id: ViewId) -> Result<HWND, EngineError> {
        self.viewhost
            .get_hwnd(viewhost_id)
            .map_err(|e| EngineError::ViewError(e.to_string()))
    }

    /// Get GPU info.
    pub(crate) fn gpu_info(&self) -> String {
        format!("{:?}", self.compositor.adapter_info())
    }

    /// Batch sizes and stack depths of the last frame (shell diagnostics /
    /// screenshot harness).
    pub(crate) fn render_stats(&self) -> RenderStats {
        self.renderer
            .as_ref()
            .map(|r| r.get_render_stats())
            .unwrap_or_default()
    }

    // ----- drawing -------------------------------------------------------

    /// Bookkeeping around one `present`: warn once per failure episode,
    /// not per frame (a wedged surface renders at event rate and would
    /// flood the log, but total silence is how a frozen screen goes
    /// undiagnosed for a whole session), and log the recovery.
    pub(crate) fn note_render_result(&mut self, id: EngineViewId, result: &Result<(), EngineError>) {
        match result {
            Ok(()) => {
                if self.render_failing.remove(&id) {
                    info!(?id, "View render recovered");
                }
            }
            Err(e) => {
                if self.render_failing.insert(id) {
                    warn!(?id, error = %e, "View render failing; frames are NOT being presented (will log again on recovery)");
                } else {
                    trace!(?id, error = %e, "View render still failing");
                }
            }
        }
    }

    /// Draw view `id`'s newest commit: upload the images its display list
    /// names, execute the list (scrolled, for a window view) and present.
    /// Returns what the frame was drawn from, for `Engine::frame_is_current`;
    /// `None` only from the no-renderer fallback of a window view, which
    /// drew the background and recorded nothing before this module existed
    /// (there is always a renderer, so that arm is dead; kept as it was).
    /// A view with no commit yet draws its background.
    ///
    /// `viewhost_id` and `is_headless` are the view's surface, which the
    /// presenter owns; nothing of the page's state is read here.
    ///
    /// Was the body of `Engine::render` after its view reads.
    pub(crate) fn present(
        &mut self,
        id: EngineViewId,
        viewhost_id: ViewId,
        is_headless: bool,
        images: &ImageManager,
    ) -> Result<Option<PresentedFrame>, EngineError> {
        // The commit is the presenter's own; it is cloned out (an `Arc`
        // and a few words) so the draw can borrow the renderer mutably.
        let commit = self.commits.get(&id).cloned().unwrap_or(FrameCommit {
            generation: 0,
            nav_generation: 0,
            list: None,
            scroll_offset: (0.0, 0.0),
            max_scroll_offset: (0.0, 0.0),
            laid_out_for: (0, 0),
            input: None,
            canvas: self.background_color,
            modal_popup: false,
        });
        // A transparent view clears to nothing: what its document does not
        // paint shows the views below (CH3.A3c).
        let transparent = self.transparent.contains(&viewhost_id);
        let canvas = if transparent { [0.0; 4] } else { commit.canvas };
        let display_list: Option<&DisplayList> = commit.list.as_deref();
        let generation = commit.generation;
        let scroll_offset = commit.scroll_offset;
        let cmd_count = display_list.map(|dl| dl.commands.len()).unwrap_or(0);

        trace!(
            ?id,
            has_display_list = display_list.is_some(),
            cmd_count,
            is_headless,
            "Rendering view"
        );

        // Get surface size and update renderer viewport before rendering
        let (surface_width, surface_height) = {
            let _surface_span = tracing::debug_span!("get_surface_size").entered();
            self.compositor
                .get_surface_size(viewhost_id)
                .map_err(|e| EngineError::RenderError(e.to_string()))?
        };

        if let Some(renderer) = &mut self.renderer {
            renderer.set_viewport_size(surface_width, surface_height);
        }

        // What this frame is drawn from, kept when it has been presented.
        let mut missing_images = Vec::new();
        let upload_time = wgate::time_stage(HoldStage::Execute);

        // Upload images from cache to renderer before drawing
        if let Some(display_list) = display_list {
            missing_images = self.upload_display_list_images(images, &display_list.commands);
        }
        let presented = PresentedFrame {
            generation,
            scroll_offset,
            surface: (surface_width, surface_height),
            missing_images,
            at: std::time::Instant::now(),
        };
        drop(upload_time);

        // Render based on whether view is headless or not
        if is_headless {
            // Headless rendering path - no surface, no present
            let texture_view = {
                let _texture_span = tracing::debug_span!("get_headless_texture_view").entered();
                self.compositor
                    .get_headless_texture_view(viewhost_id)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?
            };
            let _execute_time = wgate::time_stage(HoldStage::Execute);

            let _execute_span = tracing::info_span!("renderer_execute", cmd_count).entered();
            if let Some(renderer) = &mut self.renderer {
                // A headless view is drawn unscrolled, as it always was.
                Self::execute(renderer, display_list, (0.0, 0.0), &texture_view, canvas, transparent)?;
            } else {
                // Fallback to compositor solid color
                self.compositor
                    .render_solid_color(viewhost_id, self.background_color)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
            }

            // No present() needed for headless - texture is already updated
        } else {
            // Regular surface rendering path
            let acquire_time = wgate::time_stage(HoldStage::Acquire);
            let (output, texture_view) = {
                let _texture_span = tracing::debug_span!("get_surface_texture").entered();
                self.compositor
                    .get_surface_texture(viewhost_id)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?
            };
            drop(acquire_time);

            // Scroll is applied at render time as a whole-page translate:
            // the display list stays in document coordinates and the
            // renderer shifts it by the clamped offset scroll_view()
            // maintains (`Renderer::set_scroll_offset`), so a scrolled
            // frame draws the page's own list, not a copy of it.

            // Render using display list if available, otherwise just clear to background
            let execute_time = wgate::time_stage(HoldStage::Execute);
            {
                let _execute_span = tracing::info_span!("renderer_execute", cmd_count).entered();
                if let Some(renderer) = &mut self.renderer {
                    Self::execute(renderer, display_list, scroll_offset, &texture_view, canvas, transparent)?;
                } else {
                    // Fallback to compositor solid color (shouldn't normally happen)
                    drop(output); // Release the texture
                    self.compositor
                        .render_solid_color(viewhost_id, self.background_color)
                        .map_err(|e| EngineError::RenderError(e.to_string()))?;
                    return Ok(None);
                }
            }

            drop(execute_time);

            // Present surface texture
            let _present_time = wgate::time_stage(HoldStage::Present);
            self.compositor.present(output);
        }

        // The W-gate's second figure: from the input's arrival at the
        // app's queue to this present. A headless view presents nothing,
        // but its draw is the same work, so it is measured the same way.
        if let Some(input) = commit.input {
            let ms = input.at.elapsed().as_millis().min(u32::MAX as u128) as u32;
            debug!(ms, seq = input.seq, "Input to present");
            if let Some(summary) = self.latency.record(ms) {
                info!("{summary}");
            }
            // Once: a commit drawn again (a new surface size, the one-second
            // refresh) is not that input's frame a second time.
            if let Some(stored) = self.commits.get_mut(&id) {
                stored.input = None;
            }
        }

        Ok(Some(presented))
    }

    /// Execute `display_list` (or nothing, which clears to the background)
    /// at `scroll_offset` into `target`: the one draw both kinds of view
    /// share. The list is borrowed, never copied.
    fn execute(
        renderer: &mut Renderer,
        display_list: Option<&DisplayList>,
        scroll_offset: (f32, f32),
        target: &wgpu::TextureView,
        canvas: [f64; 4],
        transparent: bool,
    ) -> Result<(), EngineError> {
        let commands = display_list.map(|dl| dl.commands.as_slice()).unwrap_or(&[]);
        renderer.set_scroll_offset(scroll_offset);
        renderer.set_clear_color(canvas);
        renderer.set_transparent(transparent);
        let drawn = renderer
            .execute(commands, target)
            .map_err(|e| EngineError::RenderError(e.to_string()));
        renderer.set_scroll_offset((0.0, 0.0));
        drawn
    }

    /// Draw `display_list` at `scroll_offset` into a headless view's
    /// texture through the window path's draw (`execute`), and write it to
    /// `path` as PPM when one is given. A headless view's own frames are
    /// drawn unscrolled; this is how a test sees a scrolled window frame.
    #[cfg(test)]
    pub(crate) fn draw_scrolled(
        &mut self,
        viewhost_id: ViewId,
        display_list: &DisplayList,
        scroll_offset: (f32, f32),
        path: Option<&str>,
    ) -> Result<(), EngineError> {
        let (width, height) = self.surface_size(viewhost_id)?;
        let renderer = self.renderer.as_mut().expect("renderer");
        renderer.set_viewport_size(width, height);
        match path {
            None => {
                let target = self
                    .compositor
                    .get_headless_texture_view(viewhost_id)
                    .map_err(|e| EngineError::RenderError(e.to_string()))?;
                Self::execute(
                    renderer,
                    Some(display_list),
                    scroll_offset,
                    &target,
                    [1.0, 1.0, 1.0, 1.0],
                    false,
                )
            }
            Some(path) => {
                renderer.set_scroll_offset(scroll_offset);
                let captured = self
                    .compositor
                    .capture_frame_with_renderer(viewhost_id, path, renderer, &display_list.commands)
                    .map_err(|e| EngineError::RenderError(e.to_string()));
                renderer.set_scroll_offset((0.0, 0.0));
                captured
            }
        }
    }

    /// The renderer, for tests that read its caches.
    #[cfg(test)]
    pub(crate) fn renderer_mut(&mut self) -> Option<&mut Renderer> {
        self.renderer.as_mut()
    }

    /// Draw `commands` at `viewport` into a headless view's texture and
    /// write it to `path` as PPM: what the paint tests did by reaching
    /// into the renderer and the compositor.
    #[cfg(test)]
    pub(crate) fn capture_commands(
        &mut self,
        viewhost_id: ViewId,
        viewport: (u32, u32),
        path: &str,
        commands: &[rustkit_layout::DisplayCommand],
    ) -> Result<(), EngineError> {
        let renderer = self.renderer.as_mut().expect("renderer");
        renderer.set_viewport_size(viewport.0, viewport.1);
        self.compositor
            .capture_frame_with_renderer(viewhost_id, path, renderer, commands)
            .map_err(|e| EngineError::RenderError(e.to_string()))
    }

    /// Upload images referenced in display commands to the renderer's texture cache.
    ///
    /// This scans the display list for BackgroundImage and Image commands and ensures
    /// any cached images are uploaded to the GPU before rendering.
    /// For data: URLs, images are loaded synchronously on-demand.
    ///
    /// Returns the network images the list names that are not in the cache
    /// yet: the frame is drawn without them.
    fn upload_display_list_images(
        &mut self,
        images: &ImageManager,
        commands: &[rustkit_layout::DisplayCommand],
    ) -> Vec<Url> {
        use std::time::Duration;

        let mut missing = Vec::new();
        // Early exit if no renderer
        let Some(renderer) = &mut self.renderer else {
            return missing;
        };

        // Collect unique image URLs from display list
        // (url, image, whether it is uploaded as a mask texture)
        let mut urls_to_upload: Vec<(String, std::sync::Arc<rustkit_image::LoadedImage>, bool)> =
            Vec::new();
        let mut urls_seen = HashSet::new();

        for cmd in commands {
            // Extract URL from BackgroundImage, Image and MaskedColor commands
            let (url, mask) = match cmd {
                rustkit_layout::DisplayCommand::BackgroundImage { url, .. } => (url, false),
                rustkit_layout::DisplayCommand::Image { url, .. } => (url, false),
                rustkit_layout::DisplayCommand::MaskedColor { url, .. } => (url, true),
                _ => continue,
            };

            if !urls_seen.insert((url.clone(), mask)) {
                continue; // Already processed
            }

            // Skip if already in renderer
            if if mask { renderer.has_mask_image(url) } else { renderer.has_image(url) } {
                continue;
            }

            // Try to parse as URL
            let Ok(parsed_url) = url::Url::parse(url) else {
                tracing::warn!(url = %rustkit_net::str_url_for_log(url), "Invalid URL for image");
                continue;
            };

            // Try to get from cache or load data: URLs synchronously
            let image = if let Some(cached) = images.get_cached(&parsed_url) {
                Some(cached)
            } else if parsed_url.scheme() == "data" {
                // For data: URLs, load synchronously since they don't require network
                match images.load_blocking(parsed_url.clone()) {
                    Ok(img) => Some(img),
                    Err(e) => {
                        tracing::warn!(?e, url = %rustkit_net::url_for_log(&parsed_url), "Failed to decode data URL image");
                        None
                    }
                }
            } else {
                // Image not cached and not a data: URL - it will render when loaded
                missing.push(parsed_url.clone());
                None
            };

            if let Some(img) = image {
                urls_to_upload.push((url.clone(), img, mask));
            }
        }

        // Now upload all collected images
        let mut uploaded_count = 0;
        for (url_str, image, mask) in urls_to_upload {
            let frame = image.current_frame(Duration::ZERO);
            let uploaded = if mask {
                renderer.upload_mask_image(&url_str, frame.width(), frame.height(), frame.data())
            } else {
                renderer.upload_image(&url_str, frame.width(), frame.height(), frame.data())
            };
            if let Err(e) = uploaded {
                tracing::warn!(?e, url = %rustkit_net::str_url_for_log(&url_str), "Failed to upload image to renderer");
            } else {
                uploaded_count += 1;
                tracing::debug!(%url_str, "Uploaded image to renderer");
            }
        }
        #[cfg(test)]
        {
            self.images_uploaded += uploaded_count;
        }
        let _ = uploaded_count;
        missing
    }

    /// Capture a frame from a view to a PPM file.
    ///
    /// This renders the current display list to an offscreen texture and saves it.
    /// This is useful for deterministic testing and visual debugging.
    /// The output is a PPM file (simple portable format).
    pub(crate) fn capture_frame(
        &mut self,
        viewhost_id: ViewId,
        display_list: Option<&DisplayList>,
        path: &str,
    ) -> Result<(), EngineError> {
        // Get surface size
        let (width, height) = self
            .compositor
            .get_surface_size(viewhost_id)
            .map_err(|e| EngineError::RenderError(e.to_string()))?;

        if width == 0 || height == 0 {
            return Err(EngineError::RenderError(
                "Cannot capture zero-size frame".into(),
            ));
        }

        // If we have a display list and renderer, render to offscreen texture
        match (display_list, &mut self.renderer) {
            (Some(display_list), Some(renderer)) => {
                // Update viewport size for correct coordinate transforms
                renderer.set_viewport_size(width, height);

                // Capture with actual display list rendering
                self.compositor
                    .capture_frame_with_renderer(
                        viewhost_id,
                        path,
                        renderer,
                        &display_list.commands,
                    )
                    .map_err(|e| EngineError::RenderError(e.to_string()))
            }
            _ => {
                // Fallback to magenta test pattern if no display list
                self.compositor
                    .capture_frame_to_file(viewhost_id, path)
                    .map_err(|e| EngineError::RenderError(e.to_string()))
            }
        }
    }

    /// Render a view's current display list to `output_path` as PNG (plus a
    /// JSON sidecar) and return the capture metadata.
    #[cfg(windows)]
    pub(crate) fn capture_screenshot(
        &mut self,
        bounds: Bounds,
        display_list: Option<&DisplayList>,
        output_path: &std::path::Path,
    ) -> Result<super::ScreenshotMetadata, EngineError> {
        if bounds.width == 0 || bounds.height == 0 {
            return Err(EngineError::RenderError(format!(
                "Cannot capture screenshot of zero-sized view: {}x{}",
                bounds.width, bounds.height
            )));
        }

        if let Some(renderer) = &mut self.renderer {
            renderer.set_viewport_size(bounds.width, bounds.height);
            let commands = display_list
                .map(|dl| dl.commands.as_slice())
                .unwrap_or(&[]);
            renderer
                .execute_and_capture(commands, output_path)
                .map_err(|e| EngineError::RenderError(e.to_string()))
        } else {
            Err(EngineError::RenderError("No renderer available".to_string()))
        }
    }
}

/// What of a [`FrameCommit`] is already on screen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShownFrame {
    pub nav_generation: u64,
    pub generation: u64,
    pub scroll_offset: (f32, f32),
}

/// Whether `commit` should be drawn over `shown`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PresentChoice {
    /// No display list yet. Drawing this is the background, and it must not
    /// count as the frame on screen or the laid-out frame is dropped.
    WaitForLayout,
    AlreadyShown,
    Draw,
}

/// A new navigation is a different frame even when its generation and
/// scroll match the one on screen. A commit with no display list is not
/// presented.
pub(crate) fn present_choice(shown: Option<ShownFrame>, commit: &FrameCommit) -> PresentChoice {
    if commit.list.is_none() {
        return PresentChoice::WaitForLayout;
    }
    let key = ShownFrame {
        nav_generation: commit.nav_generation,
        generation: commit.generation,
        scroll_offset: commit.scroll_offset,
    };
    if shown == Some(key) {
        PresentChoice::AlreadyShown
    } else {
        PresentChoice::Draw
    }
}

/// The window thread's surface for a page that lives on the content thread.
///
/// The NSView, the GPU surface and the renderer are created here, on the
/// thread that calls [`WindowSurface::attach`]. The page thread never sees
/// them. It publishes [`FrameCommit`]s; this draws the newest one, including
/// a commit whose only change is the scroll offset.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub struct WindowSurface {
    presenter: Presenter,
    viewhost_id: ViewId,
    id: super::EngineViewId,
    shown: Option<ShownFrame>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl WindowSurface {
    /// A host view and its GPU surface in `parent`. Fails before any page
    /// state exists.
    pub fn attach(parent: WindowHandle, bounds: Bounds) -> Result<Self, EngineError> {
        let mut presenter = Presenter::new(super::EngineConfig::default().background_color)?;
        let viewhost_id = presenter.create_host_view(parent, bounds)?;
        presenter.attach_surface(viewhost_id, bounds)?;
        presenter.clear_to_background(viewhost_id)?;
        Ok(Self {
            presenter,
            viewhost_id,
            id: super::EngineViewId::new(),
            shown: None,
        })
    }

    pub fn raw_view(&self) -> isize {
        self.presenter.raw_view(self.viewhost_id)
    }

    /// The host view's id, which tags the input its NSView records.
    pub fn viewhost_id(&self) -> ViewId {
        self.viewhost_id
    }

    /// The NSView and the GPU surface take `bounds` now. The page lays out
    /// when it receives the same bounds as a message. The surface is new,
    /// so the caller draws the commit already published again: anchored at
    /// the top left, with the page's canvas colour in the area the old
    /// layout does not cover.
    pub fn resize(&mut self, bounds: Bounds) -> Result<(), EngineError> {
        self.shown = None;
        self.presenter.apply_bounds(self.viewhost_id, false, bounds)
    }

    pub fn grab_keyboard(&self) {
        self.presenter.grab_keyboard(self.viewhost_id)
    }

    /// Draw this surface transparent (CH3.A3c, the RustKit chrome over the
    /// page): the layer is not opaque, frames clear to (0, 0, 0, 0) and hold
    /// premultiplied colour. The frame on screen is cleared now and the
    /// next commit is drawn again, whatever it is.
    pub fn set_transparent(&mut self, transparent: bool) -> Result<(), EngineError> {
        self.presenter.set_transparent(self.viewhost_id, transparent);
        self.shown = None;
        self.presenter.clear_to_empty(self.viewhost_id)
    }

    /// Draw `commit` when its navigation, generation or scroll offset
    /// differs from the frame on screen. A commit that has not been laid
    /// out is not drawn and does not become the frame on screen.
    pub fn present_commit(&mut self, commit: &FrameCommit, images: &ImageManager) -> Result<(), EngineError> {
        if present_choice(self.shown, commit) != PresentChoice::Draw {
            return Ok(());
        }
        let key = ShownFrame {
            nav_generation: commit.nav_generation,
            generation: commit.generation,
            scroll_offset: commit.scroll_offset,
        };
        self.presenter.commit(self.id, commit.clone());
        let drawn = self.presenter.present(self.id, self.viewhost_id, false, images);
        let noted = match &drawn {
            Ok(_) => Ok(()),
            Err(error) => Err(EngineError::RenderError(error.to_string())),
        };
        self.presenter.note_render_result(self.id, &noted);
        if drawn.is_ok() {
            self.shown = Some(key);
        }
        drawn.map(|_| ())
    }
}

#[cfg(test)]
mod present_choice_tests {
    use super::*;

    fn commit(nav: u64, generation: u64, laid_out: bool) -> FrameCommit {
        FrameCommit {
            generation,
            nav_generation: nav,
            list: laid_out.then(|| Arc::new(DisplayList::new())),
            scroll_offset: (0.0, 0.0),
            max_scroll_offset: (0.0, 0.0),
            laid_out_for: if laid_out { (800, 600) } else { (0, 0) },
            input: None,
            canvas: [1.0, 1.0, 1.0, 1.0],
            modal_popup: false,
        }
    }

    #[test]
    fn a_commit_before_layout_is_not_the_frame_on_screen() {
        assert_eq!(
            present_choice(None, &commit(1, 0, false)),
            PresentChoice::WaitForLayout
        );
        // Recording that empty frame would hide the laid-out one when the
        // generation matches.
        let laid_out = commit(1, 1, true);
        assert_eq!(present_choice(None, &laid_out), PresentChoice::Draw);
    }

    #[test]
    fn a_new_navigation_is_drawn_when_the_generation_matches() {
        let shown = ShownFrame {
            nav_generation: 1,
            generation: 1,
            scroll_offset: (0.0, 0.0),
        };
        assert_eq!(
            present_choice(Some(shown), &commit(1, 1, true)),
            PresentChoice::AlreadyShown
        );
        assert_eq!(
            present_choice(Some(shown), &commit(2, 1, true)),
            PresentChoice::Draw
        );
    }
}

