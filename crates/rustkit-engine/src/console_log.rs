//! Optional mirror of page console output and script errors into the tracing
//! log. Off unless `HIWAVE_CONSOLE_LOG=1`.
//!
//! When it is off, every function here returns before taking a lock, reading
//! an error report, or writing a line. The Inspector is unchanged: it stays
//! off unless something else turns it on; this mirror shares only its pure
//! URL scrub ([`crate::inspector::scrub_url`]): [`brief_url`] drops the
//! query, fragment, `;params` and userinfo of every URL a line names as a
//! URL: requests, locations, script places (console and error frames),
//! inserted scripts and dynamic imports (T7 C1). Page text a line quotes (a console message, an error
//! message, a source snippet) is not parsed for URLs. Script names such as `inserted#N` are recorded here, only while
//! the switch is on, so a line can show the script's URL. Lines for live
//! script requests are also written on the `rustkit-net` runtime (T7); its
//! tasks carry the page's nav id, and in tests the switch and sink, from the
//! page thread.

use std::collections::VecDeque;
use std::sync::Mutex;
#[cfg(not(test))]
use std::sync::OnceLock;

use rustkit_js::{ErrorReport, LogLevel, ScriptPosition};

use crate::inspector::{cap, MAX_MESSAGE_CHARS, MAX_STACK_CHARS};

const ALIAS_CAP: usize = 1024;
/// Characters of script source kept on each side of a throw's column.
const CONTEXT_CHARS: usize = 160;
/// The quoted source, including the ellipsis, never longer than this.
const SNIPPET_CAP: usize = 400;
/// A callee expression longer than this keeps its tail (the property).
const CALLEE_CAP: usize = 120;

struct AliasMap {
    entries: VecDeque<(String, String)>,
}

/// Run `f` on the label map. The process shares one; each test thread has
/// its own, emptied by [`force_for_test`], so parallel tests that both
/// record `inserted#1` never read each other's URL (CA1 item 5).
#[cfg(not(test))]
fn with_aliases<R>(f: impl FnOnce(&mut AliasMap) -> R) -> Option<R> {
    static ALIASES: OnceLock<Mutex<AliasMap>> = OnceLock::new();
    let mut map = ALIASES.get_or_init(|| Mutex::new(AliasMap { entries: VecDeque::new() })).lock().ok()?;
    Some(f(&mut map))
}

#[cfg(test)]
fn with_aliases<R>(f: impl FnOnce(&mut AliasMap) -> R) -> Option<R> {
    TEST_ALIASES.with(|map| Some(f(&mut map.borrow_mut())))
}

#[cfg(test)]
thread_local! {
    static TEST_ALIASES: std::cell::RefCell<AliasMap> =
        const { std::cell::RefCell::new(AliasMap { entries: VecDeque::new() }) };
    static FORCE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
    static SINK: std::cell::RefCell<Option<std::sync::Arc<Mutex<Vec<String>>>>> =
        const { std::cell::RefCell::new(None) };
}

/// `HIWAVE_CONSOLE_LOG=1`. Empty, `0`, and anything else are off.
pub(crate) fn enabled() -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCE.with(std::cell::Cell::get) {
        return forced;
    }
    std::env::var("HIWAVE_CONSOLE_LOG").ok().as_deref() == Some("1")
}

#[cfg(test)]
pub(crate) fn force_for_test(on: Option<bool>) {
    FORCE.with(|cell| cell.set(on));
    TEST_ALIASES.with(|map| map.borrow_mut().entries.clear());
    sync_diag_armed();
}

#[cfg(test)]
pub(crate) fn install_sink(sink: Option<std::sync::Arc<Mutex<Vec<String>>>>) {
    SINK.with(|slot| *slot.borrow_mut() = sink);
}

/// This thread's forced switch and sink, carried to a network-runtime task.
#[cfg(test)]
pub(crate) type TestHooks = (Option<bool>, Option<std::sync::Arc<Mutex<Vec<String>>>>);

#[cfg(test)]
pub(crate) fn test_hooks() -> TestHooks {
    (FORCE.with(std::cell::Cell::get), SINK.with(|slot| slot.borrow().clone()))
}

/// Install `hooks` on this thread; returns the ones they replace.
#[cfg(test)]
pub(crate) fn swap_test_hooks(hooks: TestHooks) -> TestHooks {
    let before = test_hooks();
    FORCE.with(|cell| cell.set(hooks.0));
    SINK.with(|slot| *slot.borrow_mut() = hooks.1);
    before
}

/// `name` (an engine script label) was loaded from `url`. Ignored when the
/// switch is off.
pub(crate) fn alias(name: &str, url: &str) {
    if !enabled() {
        return;
    }
    with_aliases(|map| {
        if map.entries.len() >= ALIAS_CAP {
            map.entries.pop_front();
        }
        map.entries.push_back((name.to_string(), url.to_string()));
    });
}

/// The URL an engine script label stands for, or `path` itself; either
/// way through [`brief_place`].
fn resolve(path: &str) -> String {
    let url = with_aliases(|map| map.entries.iter().rev().find(|(name, _)| name == path).map(|(_, url)| url.clone()));
    brief_place(url.flatten().as_deref().unwrap_or(path))
}

fn emit(line: String) {
    #[cfg(test)]
    SINK.with(|slot| {
        if let Some(sink) = slot.borrow().clone() {
            if let Ok(mut lines) = sink.lock() {
                lines.push(line.clone());
            }
        }
    });
    tracing::info!(target: "hiwave::console", "{line}");
}

pub(crate) fn console_message(level: LogLevel, message: &str, at: Option<&ScriptPosition>) {
    if !enabled() {
        return;
    }
    let level = match level {
        LogLevel::Log => "log",
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
        LogLevel::Debug => "debug",
    };
    let (message, _) = cap(message, MAX_MESSAGE_CHARS);
    let where_at = match at {
        Some(at) => format!("{}:{}:{}", resolve(&at.path), at.line, at.column),
        None => "-".to_string(),
    };
    emit(format!("console {level} {where_at} {message}"));
}

/// Source around a throw, and the callee expression when the column sits
/// on a call's `(`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceWindow {
    pub snippet: String,
    pub callee: Option<String>,
}

/// About [`CONTEXT_CHARS`] characters either side of `line`:`column` in
/// `source`, flattened to one line and capped. `column` is 1-based. The
/// callee is the expression immediately before a `(` at that column
/// (`box.missing(`), which is the name Boa places a call on.
pub(crate) fn excerpt(source: &str, line: u32, column: u32) -> Option<SourceWindow> {
    if line == 0 || column == 0 {
        return None;
    }
    let body = source.split_inclusive('\n').nth(line as usize - 1)?;
    let body = body.trim_end_matches(['\n', '\r']);
    let chars: Vec<char> = body.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let col = (column as usize - 1).min(chars.len().saturating_sub(1));
    let start = col.saturating_sub(CONTEXT_CHARS);
    let end = (col + CONTEXT_CHARS).min(chars.len()).max(col + 1).min(chars.len());
    let mut snippet: String = chars[start..end].iter().collect();
    snippet = flatten(&snippet);
    let mut shown = String::new();
    if start > 0 {
        shown.push('…');
    }
    shown.push_str(&snippet);
    if end < chars.len() {
        shown.push('…');
    }
    let snippet = cap_middle(&shown, SNIPPET_CAP);
    let callee = callee_before(&chars, col);
    Some(SourceWindow { snippet, callee })
}

fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '\n' || ch == '\r' || ch == '\t' || ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

fn cap_middle(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(1);
    let skip = count.saturating_sub(keep) / 2;
    let piece: String = text.chars().skip(skip).take(keep).collect();
    format!("…{piece}")
}

fn callee_before(chars: &[char], col: usize) -> Option<String> {
    if chars.get(col) != Some(&'(') {
        return None;
    }
    let mut end = col;
    while end > 0 && chars[end - 1].is_whitespace() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && is_callee_char(chars[start - 1]) {
        start -= 1;
    }
    let name: String = chars[start..end].iter().collect();
    let name = name.trim_matches(|c| c == '.' || c == '?');
    if name.is_empty() {
        return None;
    }
    let count = name.chars().count();
    if count <= CALLEE_CAP {
        return Some(name.to_string());
    }
    let tail: String = name.chars().skip(count - CALLEE_CAP).collect();
    Some(format!("…{tail}"))
}

fn is_callee_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '$' || ch == '.' || ch == '?'
}

pub(crate) fn page_error(report: &ErrorReport, script: &str, window: Option<SourceWindow>) {
    if !enabled() {
        return;
    }
    let kind = if report.rejection { "unhandled rejection" } else { "uncaught" };
    let name = report.name.unwrap_or("Error");
    let (message, _) = cap(report.message.as_deref().unwrap_or(""), MAX_MESSAGE_CHARS);
    let (url, line, column) = report.frames.first().map(|frame| (resolve(&frame.path), frame.line, frame.column)).unwrap_or_else(
        || (resolve(report.module.as_deref().unwrap_or(script)), 0, 0),
    );
    let around = match window {
        Some(window) => {
            let callee = window.callee.map(|name| format!(" callee {name}")).unwrap_or_default();
            format!("{callee} source {}", window.snippet)
        }
        None => String::new(),
    };
    let stack = if report.frames.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> = report
            .frames
            .iter()
            .map(|frame| format!("    at {}:{}:{}", resolve(&frame.path), frame.line, frame.column))
            .collect();
        let (stack, _) = cap(&lines.join("\n"), MAX_STACK_CHARS);
        format!("\n{stack}")
    };
    emit(format!("{kind} {name}: {message} at {url}:{line}:{column}{around}{stack}"));
}

pub(crate) fn inserted_script(url: &str, event: &str) {
    if !enabled() {
        return;
    }
    emit(format!("inserted script {event} {}", brief_place(url)));
}

/// A dynamic `import()` whose promise rejected. `url` is empty when the
/// specifier did not resolve. The reason is the fetch, parse, link, or
/// evaluation failure, on one line.
/// Install the process-wide hooks and arm the walks for this thread when
/// the switch is on. Safe to call from every `Engine::new`: the hooks are
/// one static function, and a disarmed walk does not run.
pub(crate) fn install_console_diag() {
    rustkit_js::set_interrupt_diag(Some(on_interrupt));
    rustkit_js::set_host_stack_diag(Some(on_host_stack));
    rustkit_net::set_fetch_diag(Some(on_fetch));
    rustkit_dom::set_removal_hook(Some(on_removal));
    rustkit_bindings::set_location_hook(Some(on_location));
    sync_diag_armed();
}

pub(crate) fn sync_diag_armed() {
    let on = enabled();
    rustkit_js::set_host_stack_armed(on);
    rustkit_dom::set_removal_armed(on);
    rustkit_bindings::set_location_armed(on);
    if on {
        rustkit_net::set_fetch_diag_armed(true);
    }
}

/// Origin and path only, capped at 180 characters. The query, fragment,
/// `;params` and `user:pass@` are dropped, as everywhere else devtools
/// writes a URL (Pete 10-23, shapes only; T7 C1): they carry tokens, and
/// since T7 these lines are written on the network runtime too.
///
/// An absolute string is parsed by `url` and scrubbed by the Inspector's
/// [`crate::inspector::scrub_url`], so the parser's reading of the string
/// (`http:u:pw@host`, `https:\\u:pw@host`, a tab in the scheme, leading
/// space) is what is cut, not this module's; it also reduces `data:` to a
/// known MIME type, `blob:` to its origin and `javascript:` to its scheme,
/// and lowercases scheme and host (T7a R1 HOLD 5682590983). A string the
/// parser rejects after reading a scheme prints no part of itself. Only a
/// string the parser found no scheme in is cut by hand, as a relative
/// string: a `://` in its query is not a scheme.
pub(crate) fn brief_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(parsed) => cap_url(crate::inspector::scrub_url(&parsed, false)),
        // The parser's verdict on whether there is a scheme is the only one
        // used here. `Url::parse` trims leading and trailing C0 controls
        // and spaces and strips tabs, CRs and LFs before it reads the
        // scheme, so a raw-string check (`^[A-Za-z][A-Za-z0-9+.-]*:`)
        // disagrees with it on ` https://u:pw@h` or `ht\ttps://u:pw@h`, and
        // would hand those to the relative cut, which prints the user and
        // part of the password. In url 2.5.7 `RelativeUrlWithoutBase` is
        // the only error returned before a scheme is read (R1 B1).
        Err(url::ParseError::RelativeUrlWithoutBase) => brief_relative(url),
        // Every other error comes after a scheme was read (a bad port, host
        // or IPv6 literal); whatever follows it may be a credential cut in
        // the wrong place, so none of it is printed.
        Err(_) => "…".to_string(),
    }
}

/// The base a relative string is joined onto to learn whether the parser
/// reads it as scheme-relative. Reserved under RFC 2606; no page has it.
const RELATIVE_BASE: &str = "https://relative.invalid/";
const RELATIVE_HOST: &str = "relative.invalid";

/// A relative string, cut as the parser reads it joined onto a fixed base
/// (R1 B2). When the join lands on another host, or carries userinfo, the
/// string was scheme-relative in disguise (`\\u:pw@h/x`, `/\u:pw@h/x`,
/// `\/u:pw@h/x`, a leading space or tab, a tab inside the slashes: with a
/// special base the parser reads each as `user=u, password=pw, host=h`),
/// and only `//`, the host[:port] and the path without its `;params` are
/// printed. A string the join rejects prints no part of itself. Any other
/// string is cut by hand, so `./`, `../` and bare specifiers stay as the
/// page wrote them: at `?` or `#`, and without the path's `;params`
/// (`../a;jsessionid=S/b` is `../a/b`, as on the absolute side).
fn brief_relative(url: &str) -> String {
    let base = url::Url::parse(RELATIVE_BASE).expect("fixed base parses");
    let Ok(joined) = base.join(url) else {
        return "…".to_string();
    };
    let host = joined.host_str().unwrap_or("");
    if host != RELATIVE_HOST || !joined.username().is_empty() || joined.password().is_some() {
        let port = joined.port().map(|p| format!(":{p}")).unwrap_or_default();
        let path = crate::inspector::drop_path_params(joined.path());
        return cap_url(format!("//{host}{port}{path}"));
    }
    let (slashes, rest) = url.strip_prefix("//").map_or(("", url), |rest| ("//", rest));
    // Userinfo and the authority only after `//`; `;params` only after it.
    let (host, path) = if slashes.is_empty() {
        ("", rest)
    } else {
        let authority = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let (authority, path) = rest.split_at(authority);
        (authority.rfind('@').map_or(authority, |at| &authority[at + 1..]), path)
    };
    let end = path.find(['?', '#']).unwrap_or(path.len());
    let path = &path[..end];
    // A not-quite-scheme (`ht tp://u:pw@h`, `1http://...`) or an `@` before
    // the first `/` (`a b:pw@h/x`) is a credential the parser did not read
    // as one; none of it is printed (T8, carried from T7a).
    let first_slash = path.find('/').unwrap_or(path.len());
    if slashes.is_empty() && (path.contains("://") || path.find('@').is_some_and(|at| at < first_slash)) {
        return "…".to_string();
    }
    // Control characters become spaces, so a specifier cannot start a
    // forged mirror line.
    let brief = format!("{slashes}{host}{}", crate::inspector::drop_path_params(path));
    cap_url(brief.chars().map(|c| if c.is_control() { ' ' } else { c }).collect())
}

/// At most 180 characters, the last one `…` when cut.
fn cap_url(mut brief: String) -> String {
    if brief.chars().count() > 180 {
        brief = brief.chars().take(179).collect();
        brief.push('…');
    }
    brief
}

/// A script's place as a line shows it: an engine label (`inline#1`,
/// `inserted#4`: a word, `#`, digits only) as it is, anything else (a URL,
/// a relative specifier, `app#token`) through [`brief_url`].
fn brief_place(place: &str) -> String {
    let label = place.split_once('#').is_some_and(|(kind, n)| {
        !kind.is_empty()
            && kind.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            && !n.is_empty()
            && n.chars().all(|c| c.is_ascii_digit())
    });
    if label { place.to_string() } else { brief_url(place) }
}

fn dash(text: &str) -> &str {
    if text.is_empty() { "-" } else { text }
}

/// `net {phase} {method} {url} status={status} {disposition} nav={nav}`.
pub(crate) fn net(phase: &str, method: &str, url: &str, status: &str, disposition: &str, nav: u64) {
    if !enabled() {
        return;
    }
    let method = if method.is_empty() { "-" } else { method };
    emit(format!(
        "net {phase} {method} {} status={status} {disposition} nav={nav}",
        brief_url(url)
    ));
}

/// `doc {kind} turn=… job=… nav=… {detail}`.
pub(crate) fn doc(kind: &str, detail: &str) {
    if !enabled() {
        return;
    }
    emit(format!(
        "doc {kind} turn={} job={} nav={} {detail}",
        dash(rustkit_js::diag_turn()),
        dash(rustkit_js::diag_job()),
        rustkit_js::diag_nav()
    ));
}

fn on_host_stack(depth: usize, histogram: &str, frames: &str) {
    if !enabled() {
        return;
    }
    emit(format!(
        "host stack depth={depth} histogram=\"{histogram}\" frames=\"{frames}\""
    ));
}

fn on_interrupt(site: &'static str, message: &'static str, limit: &'static str) {
    if !enabled() {
        return;
    }
    emit(format!(
        "interrupt site={site} message={message} limit={limit} {} {} turn={} job={} nav={}",
        rustkit_js::interrupt_attribution(),
        super::script_budget_attribution(),
        dash(rustkit_js::diag_turn()),
        dash(rustkit_js::diag_job()),
        rustkit_js::diag_nav(),
    ));
}

fn on_fetch(method: &str, url: &str, status: &str, disposition: &str) {
    let phase = if disposition == "issued" { "start" } else { "end" };
    net(phase, method, url, status, disposition, rustkit_js::diag_nav());
}

fn on_removal(tag: &str, id: &str, nodes: usize) {
    if !enabled() {
        return;
    }
    let id = if id.is_empty() { "-" } else { id };
    doc("dom-remove", &format!("parent={tag}#{id} nodes={nodes}"));
}

fn on_location(url: &str, replace: bool) {
    if !enabled() {
        return;
    }
    let how = if replace { "replace" } else { "assign" };
    doc("location", &format!("{how} {}", brief_url(url)));
}

pub(crate) fn dynamic_import_failed(specifier: &str, url: &str, reason: &str) {
    if !enabled() {
        return;
    }
    let url = if url.is_empty() { "-".to_string() } else { brief_place(url) };
    let (reason, _) = cap(reason, MAX_MESSAGE_CHARS);
    let reason = reason.replace(['\n', '\r'], " ");
    emit(format!("dynamic import {} -> {url}: {reason}", brief_place(specifier)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            force_for_test(None);
            install_sink(None);
        }
    }

    /// CA1 item 5: test threads ran with one process-wide alias map, so a
    /// label another test recorded (`inserted#1` from a different page) won
    /// the reverse lookup and a parallel run read its URL.
    #[test]
    fn a_label_recorded_on_another_test_thread_does_not_resolve_here() {
        let _reset = Reset;
        force_for_test(Some(true));
        alias("inserted#90001", "https://mine.example/a.js");
        std::thread::spawn(|| {
            force_for_test(Some(true));
            alias("inserted#90001", "https://theirs.example/b.js");
            force_for_test(None);
        })
        .join()
        .unwrap();
        assert_eq!(resolve("inserted#90001"), "https://mine.example/a.js");
    }

    #[test]
    fn forcing_the_switch_starts_with_no_labels() {
        let _reset = Reset;
        force_for_test(Some(true));
        alias("inserted#90002", "https://old.example/a.js");
        force_for_test(Some(true));
        assert_eq!(resolve("inserted#90002"), "inserted#90002");
    }

    #[test]
    fn a_line_is_written_only_while_the_switch_is_forced_on() {
        let _reset = Reset;
        force_for_test(Some(true));
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        alias("chunk#console-log-unit", "https://cdn.example/ok.js");
        alias("inline#console-log-unit", "https://page.example/");
        console_message(
            LogLevel::Warn,
            "hello-console",
            Some(&ScriptPosition { path: "inline#console-log-unit".into(), line: 2, column: 5 }),
        );
        console_message(
            LogLevel::Log,
            "from-chunk",
            Some(&ScriptPosition { path: "chunk#console-log-unit".into(), line: 1, column: 1 }),
        );
        page_error(
            &ErrorReport {
                value: "error",
                name: Some("Error"),
                message: Some("boom-inline".into()),
                message_len: Some(11),
                frames: vec![ScriptPosition { path: "inline#console-log-unit".into(), line: 4, column: 1 }],
                functions: Vec::new(),
                located: true,
                rejection: false,
                module: None,
            },
            "inline#console-log-unit",
            None,
        );
        page_error(
            &ErrorReport {
                value: "error",
                name: Some("Error"),
                message: Some("boom-reject".into()),
                message_len: Some(11),
                frames: vec![ScriptPosition { path: "inline#console-log-unit".into(), line: 3, column: 1 }],
                functions: Vec::new(),
                located: true,
                rejection: true,
                module: None,
            },
            "inline#console-log-unit",
            None,
        );
        let long = format!("{}; box.missing({});", "a".repeat(400), "b".repeat(400));
        let open = long.find('(').unwrap() as u32 + 1;
        let window = excerpt(&long, 1, open).expect("window");
        assert_eq!(window.callee.as_deref(), Some("box.missing"));
        assert!(window.snippet.contains("box.missing("), "{}", window.snippet);
        assert!(window.snippet.starts_with('…') && window.snippet.ends_with('…'), "{}", window.snippet);
        assert!(window.snippet.chars().count() <= 400, "{}", window.snippet.chars().count());
        assert!(!window.snippet.contains('\n'), "{}", window.snippet);
        page_error(
            &ErrorReport {
                value: "error",
                name: Some("TypeError"),
                message: Some("not a callable function".into()),
                message_len: Some(22),
                frames: vec![ScriptPosition { path: "inline#console-log-unit".into(), line: 1, column: open }],
                functions: Vec::new(),
                located: true,
                rejection: false,
                module: None,
            },
            "inline#console-log-unit",
            Some(window),
        );
        inserted_script("https://cdn.example/ok.js", "load");
        inserted_script("https://cdn.example/missing.js", "error");
        dynamic_import_failed("./gone.js", "https://cdn.example/gone.js", "not found");
        dynamic_import_failed("bare", "", "Failed to resolve module specifier 'bare'");
        let lines = sink.lock().unwrap().clone();
        let text = lines.join("\n");
        assert!(text.contains("console warn https://page.example/:2:5 hello-console"), "{text}");
        assert!(text.contains("console log https://cdn.example/ok.js:1:1 from-chunk"), "{text}");
        assert!(text.contains("uncaught Error: boom-inline at https://page.example/:4:1"), "{text}");
        assert!(text.contains("    at https://page.example/:4:1"), "{text}");
        assert!(text.contains("unhandled rejection Error: boom-reject at https://page.example/:3:1"), "{text}");
        let call = lines.iter().find(|line| line.contains("not a callable")).expect("call");
        let head = call.lines().next().expect("head");
        assert!(head.contains("callee box.missing"), "{head}");
        assert!(head.contains("source ") && head.contains("box.missing("), "{head}");
        assert!(!head[head.find("source ").unwrap()..].contains('\n'));
        assert!(call.lines().nth(1).is_some_and(|line| line.contains("at https://page.example/:1:414")), "{call}");
        assert!(text.contains("inserted script load https://cdn.example/ok.js"), "{text}");
        assert!(text.contains("inserted script error https://cdn.example/missing.js"), "{text}");
        assert!(text.contains("dynamic import ./gone.js -> https://cdn.example/gone.js: not found"), "{text}");
        assert!(text.contains("dynamic import bare -> -: Failed to resolve module specifier 'bare'"), "{text}");
        assert!(!text.contains("chunk#console-log-unit"), "{text}");

        force_for_test(Some(false));
        let before = sink.lock().unwrap().len();
        console_message(LogLevel::Log, "quiet", None);
        page_error(
            &ErrorReport {
                value: "error",
                name: Some("Error"),
                message: Some("nope".into()),
                message_len: Some(4),
                frames: Vec::new(),
                functions: Vec::new(),
                located: false,
                rejection: false,
                module: None,
            },
            "inline#1",
            None,
        );
        inserted_script("https://cdn.example/ok.js", "load");
        dynamic_import_failed("./gone.js", "https://cdn.example/gone.js", "not found");
        alias("chunk#console-log-unit", "https://cdn.example/other.js");
        assert_eq!(sink.lock().unwrap().len(), before);
    }

    #[test]
    fn brief_url_drops_the_query_and_fragment() {
        // T7 C1: shapes only, URL queries too.
        assert_eq!(brief_url("https://www.reddit.com/svc/shreddit"), "https://www.reddit.com/svc/shreddit");
        let long = format!("https://www.reddit.com/svc/shreddit/gql?{}", "q".repeat(80));
        assert_eq!(brief_url(&long), "https://www.reddit.com/svc/shreddit/gql");
        assert_eq!(brief_url("https://a.test/p?token=SECRET#frag"), "https://a.test/p");
        assert_eq!(brief_url("https://a.test/p#SECRET?x"), "https://a.test/p");
        assert!(brief_url(&format!("https://example.test/{}", "p".repeat(300))).chars().count() <= 180);
        // `;params` and userinfo carry tokens too (T7a review).
        assert_eq!(brief_url("https://a.test/p;jsessionid=SECRET?x"), "https://a.test/p");
        assert_eq!(brief_url("https://user:SECRET@a.test/p"), "https://a.test/p");
        assert_eq!(brief_url("https://a.test/p@q"), "https://a.test/p@q");
    }

    #[test]
    fn brief_url_takes_a_scheme_only_at_the_start() {
        // T7a R1 delta (Atlas 1, Prometheus 7): a `://` later in the URL
        // is not the scheme, so a relative URL keeps its query cut too.
        assert_eq!(brief_url("/api?token=SECRET&next=https://x.test/p"), "/api");
        assert_eq!(brief_url("./m.js?a=1&u=http://e.test/"), "./m.js");
        assert_eq!(brief_url("m.js#SECRET://x"), "m.js");
        // `;` is cut after the authority only.
        assert_eq!(brief_url("https://a.test/p;s=SECRET"), "https://a.test/p");
    }

    #[test]
    fn brief_url_strips_userinfo_after_a_leading_double_slash() {
        // T7a R1 delta (Atlas 2).
        assert_eq!(brief_url("//u:SECRET@host.test/x"), "//host.test/x");
        assert_eq!(brief_url("//host.test/x?SECRET"), "//host.test/x");
    }

    #[test]
    fn brief_url_reduces_a_data_url_to_its_mime_type() {
        // T7a R1 delta (Atlas 3): the payload is page content.
        assert_eq!(brief_url("data:text/javascript;base64,U0VDUkVU"), "data:text/javascript,…");
        assert_eq!(brief_url("data:text/plain,SECRET?x#y"), "data:text/plain,…");
        assert_eq!(brief_url("DATA:,SECRET"), "data:,…");
        // An unknown MIME type is page text; a query in it is still a query.
        assert_eq!(brief_url("data:SECRETTOKEN,x"), "data:,…");
    }

    // T7a R1 HOLD 5682590983: absolute strings go through `Url::parse` and
    // the Inspector's `scrub_url`, so what the parser reads as credentials
    // is what is cut. One test per leak row.

    #[test]
    fn brief_url_http_with_no_slashes_drops_the_userinfo() {
        assert_eq!(brief_url("http:u:SECRET@host/x"), "http://host/x");
    }

    #[test]
    fn brief_url_https_with_one_slash_drops_the_userinfo() {
        assert_eq!(brief_url("https:/u:SECRET@host"), "https://host/");
    }

    #[test]
    fn brief_url_https_with_backslashes_drops_the_userinfo_and_query() {
        assert_eq!(brief_url(r"https:\\u:SECRET@host\x?q=1"), "https://host/x");
    }

    #[test]
    fn brief_url_https_with_a_slash_and_backslash_drops_the_userinfo() {
        assert_eq!(brief_url(r"https:/\u:SECRET@host/x"), "https://host/x");
    }

    #[test]
    fn brief_url_with_a_leading_space_is_still_absolute() {
        assert_eq!(brief_url(" https://u:SECRET@h/x?q"), "https://h/x");
    }

    #[test]
    fn brief_url_with_a_tab_in_the_scheme_is_still_absolute() {
        assert_eq!(brief_url("ht\ttp://u:SECRET@h/x"), "http://h/x");
    }

    #[test]
    fn brief_url_data_url_with_a_query_in_its_mime_prints_no_query() {
        let brief = brief_url("data:text/plain?tok=SECRET#f");
        assert_eq!(brief, "data:,…");
        assert!(!brief.contains("SECRET"), "{brief}");
    }

    #[test]
    fn brief_url_javascript_url_keeps_only_its_scheme() {
        let brief = brief_url("javascript:alert('SECRET')");
        assert_eq!(brief, "javascript:");
        assert!(!brief.contains("SECRET"), "{brief}");
    }

    #[test]
    fn brief_url_blob_url_keeps_only_its_origin() {
        let brief = brief_url("blob:https://u:SECRET@a/b?x");
        assert_eq!(brief, "blob:https://a");
        assert!(!brief.contains("SECRET"), "{brief}");
    }

    #[test]
    fn brief_url_prints_nothing_of_a_schemed_string_that_does_not_parse() {
        // `p?w` reads as a port, so the parser rejects it; a hand cut at
        // `?` would print the user and part of the password.
        let brief = brief_url("https://user:SECRET?w@host/x");
        assert_eq!(brief, "…");
        assert!(!brief.contains("user") && !brief.contains("SECRET"), "{brief}");
        assert_eq!(brief_url("http://[::1"), "…");
    }

    /// R1 B1 @ 9ceed2a5: the parser trims leading space and strips a tab
    /// before reading the scheme, so these are schemed strings that do not
    /// parse, not relative ones; a raw-string scheme check would have sent
    /// them to the hand cut, which prints the user and part of the password.
    #[test]
    fn brief_url_prints_nothing_of_a_schemed_string_the_parser_had_to_trim_first() {
        for u in [" https://user:SECRET?w@host/x", "ht\ttps://user:SECRET?w@host/x", " http://u:SECRET@host:bad/x"] {
            let brief = brief_url(u);
            assert_eq!(brief, "…", "{u:?}");
            assert!(!brief.contains("SECRET"), "{u:?} -> {brief}");
        }
    }

    /// R1 M1 @ 9ceed2a5: a `javascript:` URL with an authority is a base
    /// URL to the parser, so the `cannot_be_a_base` reduction missed it.
    #[test]
    fn brief_url_javascript_url_with_an_authority_keeps_only_its_scheme() {
        for u in ["javascript://h/alert(SECRET)", "javascript://alert(1)"] {
            let brief = brief_url(u);
            assert_eq!(brief, "javascript:", "{u:?}");
            assert!(!brief.contains('(') && !brief.contains("SECRET"), "{u:?} -> {brief}");
        }
    }

    #[test]
    fn brief_url_lowercases_the_scheme_and_host_only() {
        assert_eq!(brief_url("HTTPS://USER:SECRET@HOST.TEST/X?TOKEN=S#F"), "https://host.test/X");
        assert_eq!(brief_url("http://[::1]:8080/x?q=1"), "http://[::1]:8080/x");
        assert_eq!(brief_url("wss://u:SECRET@h/x;s=1"), "wss://h/x");
    }

    #[test]
    fn brief_url_cuts_semicolon_params_in_the_path_and_keeps_an_authority_semicolon() {
        // The authority's `;` is the parser's to read; only the path's
        // `;params` are cut (T7a Atlas pin).
        assert_eq!(brief_url("https://h/p@q;r?s"), "https://h/p@q");
        let brief = brief_url("https://h;x/p");
        assert!(!brief.contains('?'), "{brief}");
        assert!(brief == "…" || brief.starts_with("https://h;x/"), "{brief}");
        assert_eq!(brief_url("https://h/a;jsessionid=SECRET/b"), "https://h/a/b");
    }

    // T7a R1 HOLD 5685085123 (B2): a relative string is joined onto a fixed
    // base, so a scheme-relative string in disguise is cut as the parser
    // reads it. One test per leak row; none may print the user or password.

    /// `brief_url(input)`, checked to print neither the user nor the password.
    fn brief_without_userinfo(input: &str) -> String {
        let brief = brief_url(input);
        assert!(!brief.contains("SECRET"), "{input:?} printed {brief:?}");
        assert!(!brief.contains("u:"), "{input:?} printed {brief:?}");
        assert!(!brief.contains('@'), "{input:?} printed {brief:?}");
        brief
    }

    #[test]
    fn brief_relative_two_backslashes_drop_the_userinfo() {
        assert_eq!(brief_without_userinfo(r"\\u:SECRET@h/x"), "//h/x");
    }

    #[test]
    fn brief_relative_slash_backslash_drops_the_userinfo() {
        assert_eq!(brief_without_userinfo(r"/\u:SECRET@h/x"), "//h/x");
    }

    #[test]
    fn brief_relative_backslash_slash_drops_the_userinfo() {
        assert_eq!(brief_without_userinfo(r"\/u:SECRET@h/x"), "//h/x");
    }

    #[test]
    fn brief_relative_with_a_leading_space_drops_the_userinfo() {
        assert_eq!(brief_without_userinfo(" //u:SECRET@h/x"), "//h/x");
    }

    #[test]
    fn brief_relative_with_a_leading_tab_drops_the_userinfo() {
        assert_eq!(brief_without_userinfo("\t//u:SECRET@h/x"), "//h/x");
    }

    #[test]
    fn brief_relative_with_a_tab_between_the_slashes_drops_the_userinfo() {
        assert_eq!(brief_without_userinfo("/\t/u:SECRET@h/x"), "//h/x");
    }

    #[test]
    fn brief_relative_scheme_relative_keeps_the_port_and_cuts_the_rest() {
        assert_eq!(brief_without_userinfo(r"\\u:SECRET@h:8443/a;jsessionid=SECRET/b?t=SECRET#SECRET"), "//h:8443/a/b");
    }

    #[test]
    fn brief_relative_plain_paths_stay_as_the_page_wrote_them() {
        assert_eq!(brief_url("../a/b?x"), "../a/b");
        assert_eq!(brief_url("/api?token=SECRET"), "/api");
        assert_eq!(brief_url("./m.js?a=1&u=http://e.test/"), "./m.js");
        assert_eq!(brief_url("/a;jsessionid=SECRET/b"), "/a/b");
        assert_eq!(brief_url("m.js"), "m.js");
    }

    // T8 (T7a nits carried, Atlas 75f82d34 residue + Prometheus 295816ff).

    #[test]
    fn brief_relative_prints_nothing_of_a_not_quite_scheme_with_userinfo() {
        // The parser finds no scheme in these, so they reach the hand cut,
        // which printed them whole.
        for u in ["ht tp://u:SECRET@h/x", "1http://u:SECRET@h/x", "-http://u:SECRET@h/x", "h ttps://u:SECRET@h/x?q"] {
            let brief = brief_without_userinfo(u);
            assert_eq!(brief, "…", "{u:?}");
        }
    }

    #[test]
    fn brief_relative_prints_nothing_of_a_userinfo_before_the_first_slash() {
        for u in ["a b:SECRET@h/x", "u%3ASECRET@h/x"] {
            assert_eq!(brief_without_userinfo(u), "…", "{u:?}");
        }
        // An `@` in the path, after the first `/`, is path text.
        assert_eq!(brief_url("./a/b@c"), "./a/b@c");
    }

    #[test]
    fn scrub_url_keeps_a_hostless_path_a_path_when_its_params_are_cut() {
        // `foo:/;x/a@b`: cutting `;x` leaves `//a@b`, which url's `set_path`
        // serialised as `foo://a@b`, so the record re-read as user `a`,
        // host `b`.
        let brief = brief_url("foo:/;x/a@b");
        assert_eq!(brief, "foo:/.//a@b");
        let reread = url::Url::parse(&brief).unwrap();
        assert!(reread.host().is_none() && reread.username().is_empty(), "{brief}");
    }

    #[test]
    fn brief_relative_maps_control_characters_so_a_line_cannot_be_forged() {
        let brief = brief_url("./x\nnet response GET https://evil status=200");
        assert!(!brief.contains('\n'), "{brief:?}");
        let brief = brief_url("/a\tb?S");
        assert!(!brief.contains('\t') && !brief.contains('S'), "{brief:?}");
    }

    #[test]
    fn brief_place_keeps_only_numbered_engine_labels() {
        // T7a R1 delta (Atlas 5): `word#` is a label only when digits follow.
        assert_eq!(brief_place("inline#1"), "inline#1");
        assert_eq!(brief_place("unparseable#12"), "unparseable#12");
        assert_eq!(brief_place("app#SECRET"), "app");
        assert_eq!(brief_place("app#"), "app");
    }

    #[test]
    fn script_places_inserted_scripts_and_imports_drop_the_query_too() {
        // T7a review: the header says every URL a line names; these were
        // printed raw.
        let _reset = Reset;
        force_for_test(Some(true));
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        alias("chunk#q-unit", "https://cdn.example/c.js?token=SECRET");
        let at = ScriptPosition { path: "chunk#q-unit".into(), line: 2, column: 3 };
        console_message(LogLevel::Log, "hi", Some(&at));
        let inline = ScriptPosition { path: "inline#1".into(), line: 1, column: 1 };
        console_message(LogLevel::Log, "hi", Some(&inline));
        inserted_script("https://cdn.example/i.js?token=SECRET#f", "load");
        dynamic_import_failed("./m.js?token=SECRET", "https://u:SECRET@cdn.example/m.js;s=SECRET", "not found");
        let text = sink.lock().unwrap().join("\n");
        assert!(text.contains("console log https://cdn.example/c.js:2:3 hi"), "{text}");
        assert!(text.contains("console log inline#1:1:1 hi"), "{text}");
        assert!(text.contains("inserted script load https://cdn.example/i.js"), "{text}");
        assert!(text.contains("dynamic import ./m.js -> https://cdn.example/m.js: not found"), "{text}");
        assert!(!text.contains("SECRET"), "{text}");
    }

    #[test]
    fn an_interrupt_line_names_the_site_and_turn_only_while_forced_on() {
        let _reset = Reset;
        force_for_test(None);
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        rustkit_js::set_diag_turn("load");
        rustkit_js::set_diag_job("microtask");
        rustkit_js::set_diag_nav(7);
        on_interrupt("await.resume.fulfill", "async function was interrupted", "LoopIteration");
        assert!(sink.lock().unwrap().is_empty());
        force_for_test(Some(true));
        on_interrupt("await.resume.reject", "async function was interrupted", "LoopIteration");
        let lines = sink.lock().unwrap().clone();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let line = &lines[0];
        assert!(line.contains("interrupt site=await.resume.reject"), "{line}");
        assert!(line.contains("message=async function was interrupted"), "{line}");
        assert!(line.contains("limit=LoopIteration"), "{line}");
        assert!(line.contains("word="), "{line}");
        assert!(line.contains("setter="), "{line}");
        assert!(line.contains("depth="), "{line}");
        assert!(line.contains("recursion_limit="), "{line}");
        assert!(line.contains("host_limit="), "{line}");
        assert!(line.contains("exec_us="), "{line}");
        assert!(line.contains("wall_us="), "{line}");
        assert!(line.contains("turn=load"), "{line}");
        assert!(line.contains("job=microtask"), "{line}");
        assert!(line.contains("nav=7"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        rustkit_js::set_diag_turn("");
        rustkit_js::set_diag_job("");
        rustkit_js::set_diag_nav(0);
    }

    #[test]
    fn a_host_stack_line_names_the_depth_only_while_forced_on() {
        let _reset = Reset;
        force_for_test(None);
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        on_host_stack(256, "set x=256", "set x > set x");
        assert!(sink.lock().unwrap().is_empty());
        force_for_test(Some(true));
        on_host_stack(512, "set x=512", "set x > set x");
        let lines = sink.lock().unwrap().clone();
        assert_eq!(lines.len(), 1, "{lines:?}");
        let line = &lines[0];
        assert!(line.contains("host stack depth=512"), "{line}");
        assert!(line.contains("histogram=\"set x=512\""), "{line}");
        assert!(line.contains("frames=\"set x > set x\""), "{line}");
        assert!(!line.contains('\n'), "{line}");
    }
}

#[cfg(all(test, feature = "headless"))]
mod engine_tests {
    use super::*;
    use crate::error_page_script_tests::serve;
    use crate::{Bounds, Engine, EngineConfig};
    use std::sync::{Arc, Mutex};

    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            force_for_test(None);
            install_sink(None);
        }
    }

    const PAGE: &str = r#"<!doctype html><html><head></head><body>
<script>
console.warn('hello-console');
var ok = document.createElement('script');
ok.src = '/ok.js';
document.head.appendChild(ok);
var missing = document.createElement('script');
missing.src = '/missing.js';
document.head.appendChild(missing);
Promise.reject(new Error('boom-reject'));
setTimeout(function () { throw new Error('boom-timer'); }, 10);
var box = {};
box.missing(1);
</script>
<script>
throw new Error('boom-inline');
</script>
</body></html>"#;

    fn site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/ok.js" => (200, "text/javascript", vec![], "console.log('from-chunk');".into()),
            "/missing.js" => (404, "text/plain", vec![], "missing".into()),
            _ => (200, "text/html", vec![], PAGE.into()),
        }
    }

    #[test]
    fn mirrors_console_exceptions_rejections_and_inserted_scripts() {
        let _reset = Reset;
        force_for_test(Some(true));
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        let server = serve(site);
        let page = server.url("/").to_string();
        let mut engine = Engine::new(EngineConfig { timer_horizon_ms: 0, ..EngineConfig::default() }).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 800 })
            .expect("view");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
        let started = std::time::Instant::now();
        let text = loop {
            rt.block_on(engine.pump_live(view, 50));
            let text = sink.lock().unwrap().join("\n");
            if text.contains("boom-timer") && text.contains("from-chunk") && text.contains("inserted script error") {
                break text;
            }
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "console log missing events:\n{text}"
            );
        };
        let warn = text.lines().find(|line| line.contains("hello-console")).unwrap_or("");
        assert!(warn.contains(&page), "console warn should name the page\n{warn}\n{text}");
        assert!(text.contains("uncaught") && text.contains("boom-inline"), "{text}");
        assert!(text.contains("unhandled rejection") && text.contains("boom-reject"), "{text}");
        let inline = text.lines().find(|line| line.contains("boom-inline")).unwrap_or("");
        assert!(inline.contains(&page), "uncaught stack should name the page\n{inline}");
        assert!(inline.contains("at "), "{inline}");
        assert!(inline.contains("source ") && inline.contains("throw new Error"), "{inline}");
        assert!(!inline.contains('\n'), "{inline}");
        assert!(text.contains("inserted script load") && text.contains("/ok.js"), "{text}");
        assert!(text.contains("inserted script error") && text.contains("/missing.js"), "{text}");
        let chunk = text.lines().find(|line| line.contains("from-chunk")).unwrap_or("");
        assert!(chunk.contains("/ok.js"), "{chunk}");
        assert!(!chunk.contains("inserted#"), "{chunk}");
        assert!(text.contains("boom-timer"), "{text}");
        let call = text.lines().find(|line| line.contains("not a callable")).unwrap_or("");
        assert!(call.contains("callee box.missing"), "{call}\n{text}");
        assert!(call.contains("source ") && call.contains("box.missing("), "{call}");
        assert_eq!(call.lines().count(), 1, "{call}");
    }

    #[test]
    fn stays_quiet_when_the_switch_is_unset() {
        if std::env::var("HIWAVE_CONSOLE_LOG").ok().as_deref() == Some("1") {
            return;
        }
        let _reset = Reset;
        force_for_test(None);
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        let mut engine = Engine::new(EngineConfig::default()).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 300, height: 200 })
            .expect("view");
        engine
            .load_html(
                view,
                "<script>console.log('should-stay-quiet'); throw new Error('nope');</script>",
            )
            .expect("load");
        let lines = sink.lock().unwrap().clone();
        assert!(lines.is_empty(), "{lines:?}");
    }

    const IMPORTS: &str = r#"<!doctype html><html><body><script>
import('/ok.js').then(function () {});
import('/missing.js').catch(function () {});
import('bare-specifier').catch(function () {});
import('/throws.js').catch(function () {});
import('/syntax.js').catch(function () {});
</script></body></html>"#;

    fn import_site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/ok.js" => (200, "text/javascript", vec![], "export const name = 'ok';".into()),
            "/throws.js" => (200, "text/javascript", vec![], "throw new Error('eval-boom');".into()),
            "/syntax.js" => (200, "text/javascript", vec![], "let = = 1;".into()),
            "/missing.js" => (404, "text/plain", vec![], "missing".into()),
            _ => (200, "text/html", vec![], IMPORTS.into()),
        }
    }

    fn dynamic_lines(sink: &Mutex<Vec<String>>) -> Vec<String> {
        sink.lock()
            .unwrap()
            .iter()
            .filter(|line| line.starts_with("dynamic import "))
            .cloned()
            .collect()
    }

    fn load_imports() -> Vec<String> {
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        let server = serve(import_site);
        let mut engine = Engine::new(EngineConfig { timer_horizon_ms: 0, ..EngineConfig::default() }).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
            .expect("view");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
        dynamic_lines(&sink)
    }

    #[test]
    fn logs_the_specifier_url_and_reason_of_a_failed_dynamic_import() {
        let _reset = Reset;
        force_for_test(Some(true));
        let lines = load_imports();
        let one = |specifier: &str| -> String {
            let found: Vec<_> = lines.iter().filter(|line| line.starts_with(&format!("dynamic import {specifier} ->"))).cloned().collect();
            assert_eq!(found.len(), 1, "{specifier} in {lines:#?}");
            found[0].clone()
        };
        let missing = one("/missing.js");
        assert!(missing.contains("-> http://") && missing.contains("/missing.js:"), "{missing}");
        let bare = one("bare-specifier");
        assert!(bare.starts_with("dynamic import bare-specifier -> -: "), "{bare}");
        let thrown = one("/throws.js");
        assert!(thrown.contains("/throws.js:") && thrown.contains("eval-boom"), "{thrown}");
        let syntax = one("/syntax.js");
        assert!(syntax.contains("/syntax.js: ") && syntax.split(": ").nth(1).is_some_and(|reason| !reason.is_empty()), "{syntax}");
        assert!(lines.iter().all(|line| !line.contains("/ok.js")), "{lines:#?}");
    }

    #[test]
    fn failed_dynamic_imports_stay_quiet_when_the_switch_is_unset() {
        if std::env::var("HIWAVE_CONSOLE_LOG").ok().as_deref() == Some("1") {
            return;
        }
        let _reset = Reset;
        force_for_test(None);
        let lines = load_imports();
        assert!(lines.is_empty(), "{lines:#?}");
    }

    fn diag_page() -> String {
        let mut spans = String::new();
        for i in 0..60 {
            spans.push_str(&format!("<span id=\"s{i}\"></span>"));
        }
        format!(
            "<!doctype html><html><body><div id=\"feed\">{spans}</div><script>\
fetch('/gql?' + '{}');\
document.getElementById('feed').innerHTML = '';\
location.assign('/next');\
</script></body></html>",
            "q".repeat(80)
        )
    }

    fn diag_site(path: &str, _: &str) -> (u16, &'static str, Vec<(&'static str, &'static str)>, String) {
        match path {
            "/gql" => (200, "application/json", vec![], "{\"ok\":true}".into()),
            _ => (200, "text/html", vec![], diag_page()),
        }
    }

    #[test]
    fn logs_navigation_fetches_and_a_large_removal() {
        let _reset = Reset;
        force_for_test(Some(true));
        let sink = Arc::new(Mutex::new(Vec::new()));
        install_sink(Some(Arc::clone(&sink)));
        let server = serve(diag_site);
        let mut engine = Engine::new(EngineConfig { timer_horizon_ms: 0, ..EngineConfig::default() }).expect("engine");
        let view = engine
            .create_headless_view(Bounds { x: 0, y: 0, width: 400, height: 300 })
            .expect("view");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(engine.load_url(view, server.url("/"))).expect("load");
        let lines = sink.lock().unwrap().clone();
        let text = lines.join("\n");
        assert!(text.contains("doc navigation-start") && text.contains("turn=load"), "{text}");
        assert!(text.contains("doc navigation-commit") && text.contains("status=200"), "{text}");
        assert!(text.contains("doc document-replace"), "{text}");
        assert!(text.contains("doc dom-remove") && text.contains("parent=div#feed") && text.contains("nodes="), "{text}");
        assert!(text.contains("doc location") && text.contains("assign") && text.contains("/next"), "{text}");
        let gql: Vec<_> = lines.iter().filter(|line| line.contains("/gql")).cloned().collect();
        assert!(gql.iter().any(|line| line.contains("net start") && line.contains("GET")), "{gql:#?}\n{text}");
        assert!(gql.iter().any(|line| line.contains("status=200") && line.contains("response")), "{gql:#?}");
        assert!(gql.iter().any(|line| line.contains("delivered") || line.contains("nav=")), "{gql:#?}");
        // T7 C1: the query is dropped, not truncated.
        assert!(gql.iter().all(|line| !line.contains("/gql?") && !line.contains("qqq")), "a query reached the mirror: {gql:#?}");
    }
}
