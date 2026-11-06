//! The module host (Z phase, package C0): Boa's `ModuleLoader` wired to a
//! document.
//!
//! Boa asks its loader for each import and expects the answer through a
//! callback it may be handed later. This host uses that to stay out of the
//! network entirely: an import the graph asks for is **recorded**, not
//! fetched. The embedder (the engine) takes the recorded URLs, fetches them
//! under its own policy, and supplies each source back; Boa then carries on
//! loading the graph. Nothing here blocks, nothing here opens a connection.
//!
//! Identity is the URL. The module map is keyed by the URL a module was
//! *finally* served from, so two roots (or a redirect) that reach the same
//! file share one module and it is evaluated once. A specifier is resolved
//! against the importing module's URL; a bare specifier (`react`) is refused
//! as in a browser with no import map.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use boa_engine::builtins::promise::PromiseState;
use boa_engine::module::{ModuleLoader, ModuleRequest, Referrer};
use boa_engine::object::builtins::JsPromise;
use boa_engine::{js_string, Context, JsNativeError, JsObject, JsResult, JsValue, Module, Source};
use url::Url;

use crate::JsRuntime;

struct Waiter {
    url: String,
    specifier: String,
    /// This wait came from a dynamic `import()`, not a static dependency.
    dynamic: bool,
    sender: tokio::sync::oneshot::Sender<JsResult<Module>>,
}

/// Boa's `ModuleLoader` for a document. Interior-mutable and `Rc`-shared:
/// the context owns one reference, the runtime the other.
#[derive(Default)]
pub(crate) struct HostModuleLoader {
    /// The document's URL, the base for a root module's own imports when it
    /// has no path of its own.
    base: RefCell<Option<Url>>,
    /// Every module parsed so far, by the URL it was served from.
    modules: RefCell<HashMap<String, Module>>,
    /// Imports waiting for their source.
    waiting: RefCell<Vec<Waiter>>,
    /// URLs the graph asked for that the host has not been told about yet.
    requested: RefCell<Vec<String>>,
    /// URLs ever handed to the host (a URL is asked for once).
    asked: RefCell<HashSet<String>>,
    /// The page's import maps, merged.
    import_map: RefCell<crate::import_map::ImportMap>,
}

impl HostModuleLoader {
    fn resolve(&self, specifier: &str, referrer: Option<&str>) -> Result<Url, String> {
        let document = self.base.borrow().clone();
        let referrer_url = referrer.and_then(|r| Url::parse(r).ok()).or_else(|| document.clone());
        let Some(referrer_url) = referrer_url else {
            // No base at all: only an absolute URL can be resolved.
            return Url::parse(specifier)
                .map_err(|_| format!("Failed to resolve module specifier '{specifier}': no base URL"));
        };
        // A relative specifier resolves against the importing module; the
        // import map (if any) decides everything else.
        self.import_map.borrow().resolve(specifier, &referrer_url, &referrer_url)
    }

    pub fn register_module(&self, specifier: &str, module: Module) {
        self.modules.borrow_mut().insert(specifier.to_string(), module);
    }

    pub fn get_module(&self, specifier: &str) -> Option<Module> {
        self.modules.borrow().get(specifier).cloned()
    }
}

fn path_string(path: Option<&Path>) -> Option<String> {
    path.map(|p| p.to_string_lossy().into_owned())
}

impl ModuleLoader for HostModuleLoader {
    async fn load_imported_module(
        self: Rc<Self>,
        referrer: Referrer,
        request: ModuleRequest,
        _context: &RefCell<&mut Context>,
    ) -> JsResult<Module> {
        let specifier = request.specifier().to_std_string_escaped();
        let referrer_url = path_string(referrer.path());
        let dynamic = boa_engine::module::in_dynamic_import();
        let url = match self.resolve(&specifier, referrer_url.as_deref()) {
            Ok(url) => url.to_string(),
            Err(message) => {
                if dynamic {
                    boa_engine::module::record_dynamic_import_failure(&specifier, "", &message);
                }
                return Err(JsNativeError::typ().with_message(message).into());
            }
        };
        // Already parsed: the same module, every time.
        let known = self.modules.borrow().get(&url).cloned();
        if let Some(module) = known {
            return Ok(module);
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.waiting.borrow_mut().push(Waiter {
            url: url.clone(),
            specifier: specifier.clone(),
            dynamic,
            sender: tx,
        });
        if self.asked.borrow_mut().insert(url.clone()) {
            self.requested.borrow_mut().push(url.clone());
        }
        match rx.await {
            Ok(res) => res,
            Err(_) => {
                if dynamic {
                    boa_engine::module::record_dynamic_import_failure(&specifier, &url, "Module fetch cancelled");
                }
                Err(JsNativeError::typ()
                    .with_message("Module fetch cancelled")
                    .into())
            }
        }
    }

    fn init_import_meta(self: Rc<Self>, import_meta: &JsObject, module: &Module, context: &mut Context) {
        if let Some(url) = path_string(module.path()) {
            let _ = import_meta.set(js_string!("url"), js_string!(url.as_str()), false, context);
        }
    }
}

/// A dynamic `import()` that rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicImportFailure {
    pub specifier: String,
    /// Resolved URL. Empty when the specifier itself could not be resolved.
    pub url: String,
    pub reason: String,
}

/// A module the embedder started (`JsRuntime::begin_module`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModuleHandle(String);

/// Where a started module is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModuleState {
    /// Waiting for imports the host has not supplied yet.
    Loading,
    /// Loaded and linked, evaluation not finished (a top-level `await`).
    Evaluating,
    /// Evaluated.
    Done,
    /// The graph could not be loaded or linked: a missing or malformed
    /// dependency, a bare specifier, a link error. No module code ran.
    Failed(String),
    /// The graph loaded and ran, and evaluation threw.
    Threw(String),
}

/// A module's source as the host fetched it.
#[derive(Debug, Clone)]
pub struct FetchedModule {
    /// Where it was finally served from, after redirects: its identity.
    pub final_url: String,
    pub source: String,
}

pub(crate) struct ModuleEntry {
    module: Module,
    load: JsPromise,
    evaluation: Option<JsPromise>,
    failed: Option<String>,
    /// A poll answered `Evaluating` (a top-level `await`) and the outcome
    /// has not been taken since (see [`JsRuntime::take_error_reports`]).
    awaiting: bool,
    /// Its evaluation's outcome has been reported.
    settled: bool,
}

#[derive(Default)]
pub(crate) struct ModuleHost {
    pub(crate) loader: Rc<HostModuleLoader>,
    entries: HashMap<String, ModuleEntry>,
}

impl ModuleHost {
    pub(crate) fn loader(&self) -> Rc<HostModuleLoader> {
        self.loader.clone()
    }
}

fn describe(error: &boa_engine::JsError, context: &mut Context) -> String {
    match error.try_native(context) {
        Ok(native) => native.to_string(),
        Err(_) => crate::describe::error_text(error, context),
    }
}

fn describe_value(value: &JsValue, context: &mut Context) -> String {
    let error = boa_engine::JsError::from_opaque(value.clone());
    describe(&error, context)
}

impl JsRuntime {
    /// The document's URL: the base for resolving a root module's imports.
    pub fn set_module_base(&mut self, url: &str) {
        *self.modules.loader.base.borrow_mut() = Url::parse(url).ok();
    }

    /// Register an import map (`<script type=importmap>`'s text). Returns the
    /// per-entry warnings; an unusable document is an error and changes
    /// nothing. Must come before the modules that need it are started: what is
    /// already resolved stays resolved.
    pub fn add_import_map(&mut self, text: &str) -> Result<Vec<String>, String> {
        let base = self
            .modules
            .loader
            .base
            .borrow()
            .clone()
            .ok_or_else(|| "no document URL to resolve the import map against".to_string())?;
        let (map, warnings) = crate::import_map::ImportMap::parse(text, &base)?;
        self.modules.loader.import_map.borrow_mut().merge(map);
        Ok(warnings)
    }

    /// Parse `source` as the module served from `url` and start loading its
    /// graph. A syntax error is returned here, before any import is asked
    /// for. Starting a URL that is already started returns its handle: the
    /// module map has one entry per URL, so a module is never evaluated twice.
    pub fn begin_module(&mut self, url: &str, source: &str) -> Result<ModuleHandle, String> {
        if self.modules.entries.contains_key(url) {
            return Ok(ModuleHandle(url.to_string()));
        }
        let existing = self.modules.loader.modules.borrow().get(url).cloned();
        let module = match existing {
            Some(module) => module,
            None => {
                self.diag.know_path(url);
                self.sources.remember(url, source);
                let path = Path::new(url);
                let parsed = Module::parse(Source::from_bytes(source).with_path(path), None, &mut self.context);
                match parsed {
                    Ok(module) => module,
                    Err(e) => {
                        self.report_error(&e);
                        return Err(describe(&e, &mut self.context));
                    }
                }
            }
        };
        self.modules
            .loader
            .modules
            .borrow_mut()
            .insert(url.to_string(), module.clone());
        let load = module.load(&mut self.context);
        // The engine's promise, not the page's: a failure is the module's
        // error, never an unhandled rejection.
        self.rejections.forget(&JsObject::from(load.clone()));
        let _ = self.context.run_jobs();
        self.consume_interrupt();
        self.modules.entries.insert(
            url.to_string(),
            ModuleEntry {
                module,
                load,
                evaluation: None,
                failed: None,
                awaiting: false,
                settled: false,
            },
        );
        Ok(ModuleHandle(url.to_string()))
    }

    /// Dynamic `import()` calls that rejected since the last call: the
    /// specifier, the resolved URL (empty when it did not resolve), and why.
    pub fn take_dynamic_import_failures(&mut self) -> Vec<DynamicImportFailure> {
        boa_engine::module::take_dynamic_import_failures()
            .into_iter()
            .map(|failure| DynamicImportFailure {
                specifier: failure.specifier,
                url: failure.url,
                reason: failure.reason,
            })
            .collect()
    }

    /// The URLs the graph has asked for since the last call, each at most
    /// once, in the order Boa asked.
    pub fn take_module_requests(&mut self) -> Vec<String> {
        std::mem::take(&mut *self.modules.loader.requested.borrow_mut())
    }

    /// Give the host's answer for `requested`: its source, or why there is
    /// none. Every import waiting for that URL continues (or fails).
    pub fn supply_module(&mut self, requested: &str, outcome: Result<FetchedModule, String>) {
        let loader = self.modules.loader.clone();
        let module: Result<Module, String> = match outcome {
            Err(message) => Err(message),
            Ok(fetched) => {
                let known = loader.modules.borrow().get(&fetched.final_url).cloned();
                match known {
                    Some(module) => Ok(module),
                    None => {
                        self.diag.know_path(&fetched.final_url);
                        self.sources.remember(&fetched.final_url, &fetched.source);
                        if fetched.final_url != requested {
                            self.sources.remember(requested, &fetched.source);
                        }
                        let path = Path::new(fetched.final_url.as_str());
                        match Module::parse(
                            Source::from_bytes(fetched.source.as_str()).with_path(path),
                            None,
                            &mut self.context,
                        ) {
                            Ok(module) => {
                                loader
                                    .modules
                                    .borrow_mut()
                                    .insert(fetched.final_url.clone(), module.clone());
                                Ok(module)
                            }
                            Err(e) => Err(describe(&e, &mut self.context)),
                        }
                    }
                }
            }
        };
        if let Ok(module) = &module {
            // Both the asked-for URL and (above) the final one name it.
            loader.modules.borrow_mut().insert(requested.to_string(), module.clone());
        }
        // Wake everything waiting for it, in the order they asked. A module
        // reached by a redirect also answers imports of its final URL.
        let waiters: Vec<Waiter> = {
            let mut waiting = loader.waiting.borrow_mut();
            let (ready, rest): (Vec<Waiter>, Vec<Waiter>) = std::mem::take(&mut *waiting)
                .into_iter()
                .partition(|w| w.url == requested || module.as_ref().is_ok_and(|m| loader.modules.borrow().get(&w.url).is_some_and(|k| k == m)));
            *waiting = rest;
            ready
        };
        for waiter in waiters {
            match &module {
                Ok(module) => {
                    let _ = waiter.sender.send(Ok(module.clone()));
                }
                Err(message) => {
                    if waiter.dynamic {
                        boa_engine::module::record_dynamic_import_failure(&waiter.specifier, &waiter.url, message);
                    }
                    let _ = waiter.sender.send(Err(JsNativeError::typ()
                        .with_message(format!("Failed to fetch dynamically imported module: {message}"))
                        .into()));
                }
            }
        }
        let _ = self.context.run_jobs();
    }

    /// Advance a started module as far as it can go and say where it is:
    /// once its graph is loaded it is linked and evaluated.
    pub fn poll_module(&mut self, handle: &ModuleHandle) -> ModuleState {
        let _ = self.context.run_jobs();
        self.consume_interrupt();
        let Some(entry) = self.modules.entries.get_mut(&handle.0) else {
            return ModuleState::Failed("unknown module".into());
        };
        if let Some(message) = &entry.failed {
            return ModuleState::Failed(message.clone());
        }
        if entry.evaluation.is_none() {
            match entry.load.state() {
                PromiseState::Pending => return ModuleState::Loading,
                PromiseState::Rejected(reason) => {
                    self.report_value(&reason);
                    let message = describe_value(&reason, &mut self.context);
                    if let Some(entry) = self.modules.entries.get_mut(&handle.0) {
                        entry.failed = Some(message.clone());
                    }
                    return ModuleState::Failed(message);
                }
                PromiseState::Fulfilled(_) => {}
            }
            let module = entry.module.clone();
            if let Err(e) = module.link(&mut self.context) {
                self.report_error(&e);
                let message = describe(&e, &mut self.context);
                if let Some(entry) = self.modules.entries.get_mut(&handle.0) {
                    entry.failed = Some(message.clone());
                }
                return ModuleState::Failed(message);
            }
            self.context.reset_recursion_max();
            let evaluation = match module.evaluate(&mut self.context) {
                Ok(p) => {
                    self.rejections.forget(&JsObject::from(p.clone()));
                    p
                }
                Err(e) => {
                    self.report_error(&e);
                    let message = describe(&e, &mut self.context);
                    if let Some(entry) = self.modules.entries.get_mut(&handle.0) {
                        entry.failed = Some(message.clone());
                    }
                    return ModuleState::Failed(message);
                }
            };
            let _ = self.context.run_jobs();
            self.consume_interrupt();
            if let Some(entry) = self.modules.entries.get_mut(&handle.0) {
                entry.evaluation = Some(evaluation);
            }
        }
        let evaluation = match self.modules.entries.get(&handle.0).and_then(|e| e.evaluation.clone()) {
            Some(p) => p,
            None => return ModuleState::Loading,
        };
        let state = evaluation.state();
        // The outcome is reported once, here or as a late module error.
        let pending = matches!(state, PromiseState::Pending);
        let mut first_outcome = false;
        if let Some(entry) = self.modules.entries.get_mut(&handle.0) {
            entry.awaiting = pending;
            first_outcome = !pending && !entry.settled;
            entry.settled |= !pending;
        }
        match state {
            PromiseState::Pending => ModuleState::Evaluating,
            PromiseState::Fulfilled(_) => ModuleState::Done,
            PromiseState::Rejected(reason) => {
                if first_outcome {
                    self.report_value(&reason);
                }
                ModuleState::Threw(self.thrown_text(&reason))
            }
        }
    }

    /// A module's error, reported once (with reports on).
    fn report_error(&mut self, error: &boa_engine::JsError) {
        if self.diag.on.get() {
            let report = self.diag.describe_error(error, &mut self.context);
            self.diag.push(report);
        }
    }

    fn report_value(&mut self, value: &JsValue) {
        if self.diag.on.get() {
            let report = self.diag.describe(value, None, false, &mut self.context);
            self.diag.push(report);
        }
    }

    /// A module evaluation's rejection as the module's error text, the
    /// engine's own record of the stack after the message, as Boa's text
    /// for a classic script's error has it (read through the realm's
    /// getter: no page code runs).
    fn thrown_text(&mut self, reason: &JsValue) -> String {
        let mut message = describe_value(reason, &mut self.context);
        let getter = self.diag.stack_getter.borrow().clone();
        if let (_, Some(stack)) = crate::describe::error_report(reason, getter.as_ref(), &mut self.context) {
            message.push('\n');
            message.push_str(stack.trim_end());
        }
        message
    }

    /// The modules a poll left `Evaluating` (a top-level `await` still
    /// waiting) whose evaluation has since thrown: `(module URL, reason)`,
    /// each once, by URL. A browser reports such an error as the module's
    /// uncaught error. Modules that finished cleanly stop being watched.
    pub(crate) fn take_late_module_rejections(&mut self) -> Vec<(String, JsValue)> {
        let mut settled = Vec::new();
        for (url, entry) in self.modules.entries.iter_mut() {
            if !entry.awaiting {
                continue;
            }
            match entry.evaluation.as_ref().map(JsPromise::state) {
                Some(PromiseState::Pending) => {}
                Some(PromiseState::Rejected(reason)) => {
                    entry.awaiting = false;
                    entry.settled = true;
                    settled.push((url.clone(), reason));
                }
                _ => {
                    entry.awaiting = false;
                    entry.settled = true;
                }
            }
        }
        settled.sort_by(|a, b| a.0.cmp(&b.0));
        settled
    }
}
