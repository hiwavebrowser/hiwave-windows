//! test262 runner for the vendored JS engine (tc39/test262, fetched by `scripts/test262_sync.sh`).
//!
//! Runs each listed test as test262's INTERPRETING.md says: a new context per run, `assert.js` and
//! `sta.js` first (unless `raw`), the test's `includes`, then the test, once in sloppy mode and once
//! in strict mode unless its flags say otherwise; `module` tests as modules; `negative` tests must
//! fail in the named phase with the named error; `async` tests must print the completion line.
//!
//!   cargo run --release -p rustkit-js --example test262 -- --root third_party/test262 --list files.txt
//!
//! Output, one line per run, tab separated: `RESULT <path> <mode> <PASS|FAIL|TIMEOUT|PANIC> <why>`,
//! after a `START <path>` line, so a driver can tell which test a crash of this process belongs to.
//! `scripts/test262.py` is that driver: it splits the list, runs chunks in parallel, restarts after a
//! crash, and compares the outcome with the checked-in expectations.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use boa_engine::builtins::array_buffer::ArrayBuffer;
use boa_engine::builtins::promise::PromiseState;
use boa_engine::module::{Module, SimpleModuleLoader};
use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::realm::Realm;
use boa_engine::{js_string, Context, JsError, JsNativeError, JsResult, JsValue, NativeFunction, Script, Source};

const ALL: u64 = u64::MAX;

/// What a test's front matter (`/*--- ... ---*/`, YAML) says. Only the keys a runner acts on.
#[derive(Default, Debug)]
struct Meta {
    includes: Vec<String>,
    flags: Vec<String>,
    negative: Option<(String, String)>, // (phase, type)
}

impl Meta {
    fn has(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

/// A list value: `[a, b]` on the key's line, or `- a` lines under it.
fn yaml_list(lines: &[&str], at: usize, rest: &str) -> Vec<String> {
    let rest = rest.trim();
    let mut out = Vec::new();
    if rest.starts_with('[') {
        // A flow list may run over several lines.
        let mut text = rest.to_string();
        let mut i = at + 1;
        while !text.contains(']') && i < lines.len() {
            text.push_str(lines[i]);
            i += 1;
        }
        let inner = text.trim_start_matches('[').split(']').next().unwrap_or("");
        for item in inner.split(',') {
            let item = item.trim().trim_matches(|c| c == '"' || c == '\'');
            if !item.is_empty() {
                out.push(item.to_string());
            }
        }
    } else {
        for line in &lines[at + 1..] {
            let t = line.trim();
            if let Some(item) = t.strip_prefix("- ") {
                out.push(item.trim().trim_matches(|c| c == '"' || c == '\'').to_string());
            } else if !t.is_empty() {
                break;
            }
        }
    }
    out
}

fn parse_meta(source: &str) -> Meta {
    let mut meta = Meta::default();
    let Some(start) = source.find("/*---") else { return meta };
    let Some(len) = source[start..].find("---*/") else { return meta };
    let block = &source[start + 5..start + len];
    let lines: Vec<&str> = block.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        // Top-level keys only: no indentation.
        if line.starts_with(' ') || line.starts_with('\t') {
            continue;
        }
        if let Some(rest) = line.strip_prefix("includes:") {
            meta.includes = yaml_list(&lines, i, rest);
        } else if let Some(rest) = line.strip_prefix("flags:") {
            meta.flags = yaml_list(&lines, i, rest);
        } else if line.starts_with("negative:") {
            let (mut phase, mut kind) = (String::new(), String::new());
            for sub in &lines[i + 1..] {
                if !(sub.starts_with(' ') || sub.starts_with('\t')) && !sub.trim().is_empty() {
                    break;
                }
                let t = sub.trim();
                if let Some(v) = t.strip_prefix("phase:") {
                    phase = v.trim().to_string();
                } else if let Some(v) = t.strip_prefix("type:") {
                    kind = v.trim().to_string();
                }
            }
            meta.negative = Some((phase, kind));
        }
    }
    meta
}

/// What `print` was called with, for `async` tests.
#[derive(Default)]
struct Printed {
    complete: bool,
    failure: Option<String>,
}

thread_local! {
    static PRINTED: RefCell<Printed> = RefCell::new(Printed::default());
}

fn print(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(context)?
        .to_std_string_escaped();
    PRINTED.with(|p| {
        let mut p = p.borrow_mut();
        if text == "Test262:AsyncTestComplete" {
            p.complete = true;
        } else if let Some(why) = text.strip_prefix("Test262:AsyncTestFailure:") {
            p.failure.get_or_insert_with(|| why.to_string());
        }
    });
    Ok(JsValue::undefined())
}

/// Defines `print` and `$262` in `realm`. `$262.createRealm()` does the same in a new realm.
fn install_host(realm: &Realm, context: &mut Context) -> JsResult<JsValue> {
    let outer = context.enter_realm(realm.clone());
    let result = (|| {
        context.register_global_builtin_callable(js_string!("print"), 1, NativeFunction::from_fn_ptr(print))?;
        let global = context.global_object();
        let create_realm = NativeFunction::from_fn_ptr(|_, _, context| {
            let realm = context.create_realm()?;
            install_host(&realm, context)
        });
        let eval_script = NativeFunction::from_copy_closure_with_captures(
            |_, args, realm: &Realm, context| {
                let source = args.first().cloned().unwrap_or_default().to_string(context)?.to_std_string_escaped();
                let outer = context.enter_realm(realm.clone());
                let result = Script::parse(Source::from_bytes(source.as_bytes()), Some(realm.clone()), context)
                    .and_then(|script| script.evaluate(context));
                context.enter_realm(outer);
                result
            },
            realm.clone(),
        );
        let detach = NativeFunction::from_fn_ptr(|_, args, _| {
            let object = args.first().and_then(JsValue::as_object);
            let Some(object) = object else {
                return Err(JsNativeError::typ().with_message("detachArrayBuffer: not an object").into());
            };
            let Some(mut buffer) = object.downcast_mut::<ArrayBuffer>() else {
                return Err(JsNativeError::typ().with_message("detachArrayBuffer: not an ArrayBuffer").into());
            };
            buffer.detach(&JsValue::undefined())?;
            Ok(JsValue::null())
        });
        let gc = NativeFunction::from_fn_ptr(|_, _, _| {
            boa_engine::gc::force_collect();
            Ok(JsValue::undefined())
        });
        let host = ObjectInitializer::new(context)
            .function(create_realm, js_string!("createRealm"), 0)
            .function(eval_script, js_string!("evalScript"), 1)
            .function(detach, js_string!("detachArrayBuffer"), 1)
            .function(gc, js_string!("gc"), 0)
            .property(js_string!("global"), global, Attribute::WRITABLE | Attribute::CONFIGURABLE)
            .build();
        context.register_global_property(
            js_string!("$262"),
            host.clone(),
            Attribute::WRITABLE | Attribute::CONFIGURABLE,
        )?;
        Ok(JsValue::from(host))
    })();
    context.enter_realm(outer);
    result
}

/// The name of the error a script threw: its constructor's `name`, as test262's `negative.type`.
fn error_name(err: &JsError, context: &mut Context) -> String {
    if let Ok(native) = err.try_native(context) {
        let text = native.to_string();
        if let Some(name) = text.split(':').next() {
            return name.trim().to_string();
        }
    }
    let Ok(value) = err.clone().into_opaque(context) else {
        return "(error could not be made a value)".to_string();
    };
    let name = value
        .as_object()
        .and_then(|o| o.get(js_string!("constructor"), context).ok())
        .and_then(|c| c.as_object().and_then(|c| c.get(js_string!("name"), context).ok()))
        .and_then(|n| n.as_string().map(|s| s.to_std_string_escaped()));
    name.unwrap_or_else(|| "(not an error object)".to_string())
}

fn short(text: &str) -> String {
    let one_line: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(160).collect();
    one_line
}

fn describe(err: &JsError, context: &mut Context) -> String {
    let name = error_name(err, context);
    let text = match err.try_native(context) {
        Ok(native) => native.to_string(),
        Err(_) => err
            .clone()
            .into_opaque(context)
            .and_then(|value| value.to_string(context))
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default(),
    };
    short(&format!("{name}: {text}"))
}

struct Runner {
    root: PathBuf,
    harness: HashMap<String, String>,
    word: Arc<AtomicU64>,
    deadline: Arc<AtomicU64>, // milliseconds since `epoch` at which the watchdog stops the run; 0 = none
    epoch: Instant,
    timeout: Duration,
}

enum Outcome {
    Pass,
    Fail(String),
    Timeout,
}

impl Runner {
    fn harness_file(&mut self, name: &str) -> Result<String, String> {
        if let Some(text) = self.harness.get(name) {
            return Ok(text.clone());
        }
        let path = self.root.join("harness").join(name);
        let text = std::fs::read_to_string(&path).map_err(|e| format!("harness {name}: {e}"))?;
        self.harness.insert(name.to_string(), text.clone());
        Ok(text)
    }

    fn interrupted(&self) -> bool {
        self.word.load(Ordering::Relaxed) == ALL
    }

    /// One run of one test in one mode.
    fn run_one(&mut self, path: &Path, source: &str, meta: &Meta, strict: bool) -> Outcome {
        PRINTED.with(|p| *p.borrow_mut() = Printed::default());
        self.word.store(0, Ordering::Relaxed);
        let stop_at = self.epoch.elapsed() + self.timeout;
        self.deadline.store(stop_at.as_millis() as u64, Ordering::Relaxed);
        let outcome = self.run_inner(path, source, meta, strict);
        self.deadline.store(0, Ordering::Relaxed);
        if self.interrupted() {
            return Outcome::Timeout;
        }
        outcome
    }

    fn run_inner(&mut self, path: &Path, source: &str, meta: &Meta, strict: bool) -> Outcome {
        let module = meta.has("module");
        let raw = meta.has("raw");
        let is_async = meta.has("async");
        let dir = path.parent().unwrap_or(Path::new("."));
        let loader = match SimpleModuleLoader::new(dir) {
            Ok(loader) => Rc::new(loader),
            Err(e) => return Outcome::Fail(format!("module loader: {e}")),
        };
        let mut context = match Context::builder().module_loader(loader.clone()).build() {
            Ok(context) => context,
            Err(e) => return Outcome::Fail(format!("context: {e}")),
        };
        let context = &mut context;
        context.set_interrupt_word(Some(self.word.clone()));
        context.set_interrupt_turn(ALL);
        let realm = context.realm().clone();
        if let Err(e) = install_host(&realm, context) {
            return Outcome::Fail(format!("host: {}", describe(&e, context)));
        }
        if !raw {
            let mut files = vec!["assert.js".to_string(), "sta.js".to_string()];
            if is_async {
                files.push("doneprintHandle.js".to_string());
            }
            files.extend(meta.includes.iter().cloned());
            for name in files {
                let text = match self.harness_file(&name) {
                    Ok(text) => text,
                    Err(e) => return Outcome::Fail(e),
                };
                if let Err(e) = context.eval(Source::from_bytes(text.as_bytes())) {
                    return Outcome::Fail(format!("harness {name} threw {}", describe(&e, context)));
                }
            }
        }
        let text = if strict && !raw && !module {
            format!("\"use strict\";\n{source}")
        } else {
            source.to_string()
        };
        let negative = meta.negative.as_ref();
        // Parse.
        enum Parsed {
            Script(Script),
            Module(Module),
        }
        let parsed = if module {
            Module::parse(Source::from_reader(text.as_bytes(), Some(path)), None, context).map(Parsed::Module)
        } else {
            Script::parse(Source::from_reader(text.as_bytes(), Some(path)), None, context).map(Parsed::Script)
        };
        let parsed = match parsed {
            Ok(parsed) => parsed,
            Err(e) => {
                let name = error_name(&e, context);
                return match negative {
                    Some((phase, kind)) if phase == "parse" && *kind == name => Outcome::Pass,
                    Some((phase, kind)) => Outcome::Fail(format!(
                        "expected {kind} in phase {phase}, parse failed with {}",
                        describe(&e, context)
                    )),
                    None => Outcome::Fail(format!("parse: {}", describe(&e, context))),
                };
            }
        };
        if let Some((phase, kind)) = negative.filter(|(phase, _)| *phase == "parse") {
            // An early error may be reported when the code is compiled, not only when it is parsed.
            // The test must not run; compiling is as far as a parse-phase test goes.
            return match parsed {
                Parsed::Script(script) => match script.codeblock(context).err() {
                    Some(e) if error_name(&e, context) == *kind => Outcome::Pass,
                    Some(e) => Outcome::Fail(format!("expected {kind} at parse, got {}", describe(&e, context))),
                    None => Outcome::Fail(format!("expected {kind} at parse, parsed without error")),
                },
                Parsed::Module(module) => self.module_negative(module, phase, kind, context),
            };
        }
        // Evaluate.
        let thrown: Option<JsError> = match parsed {
            Parsed::Script(script) => match script.evaluate(context) {
                Ok(_) => context.run_jobs().err(),
                Err(e) => Some(e),
            },
            Parsed::Module(module) => {
                let promise = module.load_link_evaluate(context);
                match context.run_jobs() {
                    Err(e) => Some(e),
                    Ok(()) => match promise.state() {
                        PromiseState::Fulfilled(_) => None,
                        PromiseState::Rejected(reason) => Some(JsError::from_opaque(reason)),
                        PromiseState::Pending => {
                            return Outcome::Fail("the module's evaluation promise never settled".to_string());
                        }
                    },
                }
            }
        };
        if self.interrupted() {
            return Outcome::Timeout;
        }
        match (negative, thrown) {
            (Some((phase, kind)), Some(e)) => {
                let name = error_name(&e, context);
                if name == *kind {
                    Outcome::Pass
                } else {
                    Outcome::Fail(format!("expected {kind} in phase {phase}, got {}", describe(&e, context)))
                }
            }
            (Some((phase, kind)), None) => Outcome::Fail(format!("expected {kind} in phase {phase}, no error")),
            (None, Some(e)) => Outcome::Fail(describe(&e, context)),
            (None, None) => {
                if !is_async {
                    return Outcome::Pass;
                }
                PRINTED.with(|p| {
                    let p = p.borrow();
                    if let Some(why) = &p.failure {
                        Outcome::Fail(short(&format!("async failure: {why}")))
                    } else if p.complete {
                        Outcome::Pass
                    } else {
                        Outcome::Fail("async test did not print its completion line".to_string())
                    }
                })
            }
        }
    }

    /// A module whose `negative.phase` is `parse` but whose own text parsed: the error is in what
    /// loading and linking reach.
    fn module_negative(&mut self, module: Module, phase: &str, kind: &str, context: &mut Context) -> Outcome {
        let promise = module.load_link_evaluate(context);
        let thrown = match context.run_jobs() {
            Err(e) => Some(e),
            Ok(()) => match promise.state() {
                PromiseState::Rejected(reason) => Some(JsError::from_opaque(reason)),
                _ => None,
            },
        };
        match thrown {
            Some(e) if error_name(&e, context) == kind => Outcome::Pass,
            Some(e) => Outcome::Fail(format!("expected {kind} in phase {phase}, got {}", describe(&e, context))),
            None => Outcome::Fail(format!("expected {kind} in phase {phase}, no error")),
        }
    }
}

fn main() {
    let mut root = PathBuf::from("third_party/test262");
    let mut list = None;
    let mut timeout = Duration::from_secs(10);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = PathBuf::from(args.next().expect("--root DIR")),
            "--list" => list = Some(args.next().expect("--list FILE")),
            "--timeout-ms" => {
                timeout = Duration::from_millis(args.next().and_then(|n| n.parse().ok()).expect("--timeout-ms N"));
            }
            other => panic!("unknown argument {other}"),
        }
    }
    let list = std::fs::read_to_string(list.expect("--list FILE")).expect("the list file");
    let files: Vec<String> = list.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();

    let word = Arc::new(AtomicU64::new(0));
    let deadline = Arc::new(AtomicU64::new(0));
    let epoch = Instant::now();
    {
        // The watchdog: sets the interrupt word when a run passes its deadline.
        let (word, deadline) = (word.clone(), deadline.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(50));
            let at = deadline.load(Ordering::Relaxed);
            if at != 0 && epoch.elapsed().as_millis() as u64 >= at {
                word.store(ALL, Ordering::Relaxed);
            }
        });
    }

    // Tests recurse deeply and the engine's limits are its own: give the thread a large stack.
    let worker = std::thread::Builder::new().stack_size(256 * 1024 * 1024).spawn(move || {
        let mut runner = Runner { root, harness: HashMap::new(), word, deadline, epoch, timeout };
        let out = std::io::stdout();
        for file in files {
            {
                let mut out = out.lock();
                writeln!(out, "START\t{file}").ok();
                out.flush().ok();
            }
            let path = runner.root.join(&file);
            let source = match std::fs::read_to_string(&path) {
                Ok(source) => source,
                Err(e) => {
                    println!("RESULT\t{file}\t-\tFAIL\tcannot read: {e}");
                    continue;
                }
            };
            let meta = parse_meta(&source);
            let modes: &[(&str, bool)] = if meta.has("raw") {
                &[("raw", false)]
            } else if meta.has("module") {
                &[("module", true)]
            } else if meta.has("onlyStrict") {
                &[("strict", true)]
            } else if meta.has("noStrict") {
                &[("sloppy", false)]
            } else {
                &[("sloppy", false), ("strict", true)]
            };
            for (mode, strict) in modes {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    runner.run_one(&path, &source, &meta, *strict)
                }));
                let (status, why) = match outcome {
                    Ok(Outcome::Pass) => ("PASS", String::new()),
                    Ok(Outcome::Fail(why)) => ("FAIL", why),
                    Ok(Outcome::Timeout) => ("TIMEOUT", String::new()),
                    Err(_) => ("PANIC", String::new()),
                };
                let mut out = out.lock();
                writeln!(out, "RESULT\t{file}\t{mode}\t{status}\t{why}").ok();
                out.flush().ok();
            }
        }
    });
    worker.expect("spawn").join().expect("the worker thread");
}
