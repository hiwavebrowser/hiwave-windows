//! Optional copy of the page's console into a file of its own. Off unless
//! `HIWAVE_CONSOLE_LOG=<path>` (or `parity-capture --console-log <path>`).
//!
//! Each console call (`log`, `info`, `warn`, `error`, `debug`), uncaught
//! exception and unhandled rejection is one JSON line: wall time, view,
//! the page's URL, level, the message text, and the script's URL, line and
//! column. It is the feed the Inspector's console reads (`inspector_glue`),
//! with the page's text kept.
//!
//! **The file holds what the page said**: cookies, form values, tokens,
//! whatever a page logs or throws. That is why it is a separate file,
//! created for its owner only (0600), and why nothing here writes a line
//! to the tracing log, which carries shapes only. URLs lose their query,
//! fragment, `;params` and userinfo, as everywhere else devtools writes
//! one ([`crate::console_log::brief_url`]).
//!
//! `HIWAVE_CONSOLE_LOG=1` is a different, older switch: the tracing mirror
//! in `console_log`. A value names a file here only when it has a `/` in
//! it ([`setting`]), so `1`, `0` and every other bare word mean what they
//! did before.
//!
//! When it is off, every function here returns before formatting a line,
//! and no file is made.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::OnceLock;

use rustkit_js::{ErrorReport, LogLevel};
use serde::Serialize;

/// The file stops growing here, with one last `truncated` line.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;
/// A message longer than this many bytes is cut (at a character).
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// Room kept for the `truncated` line, so the file never passes its cap.
const LAST_LINE_BYTES: u64 = 96;

/// The path `set_path` gave, read once when the file is first wanted.
static CHOSEN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Write the page console to `path` for the rest of the process (the
/// command-line form of `HIWAVE_CONSOLE_LOG=<path>`; it wins over the
/// variable). Call it before the first engine is made: the file is opened
/// once.
pub fn set_path(path: impl Into<PathBuf>) {
    if let Ok(mut chosen) = CHOSEN.lock() {
        *chosen = Some(path.into());
    }
}

/// What a `HIWAVE_CONSOLE_LOG` value asks of this module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Setting {
    /// Nothing: unset, or the tracing mirror's own values.
    Off,
    /// A value with a `/` in it names the file (`/tmp/console.jsonl`,
    /// `./console.jsonl`).
    File(PathBuf),
    /// Neither: a mistake worth saying once.
    NotAPath,
}

pub(crate) fn setting(value: &std::ffi::OsStr) -> Setting {
    let bytes = value.as_encoded_bytes();
    if bytes.contains(&b'/') {
        return Setting::File(PathBuf::from(value));
    }
    // `1`, `0`, and the bare words people set a switch to (`true`, `on`).
    if bytes.iter().all(u8::is_ascii_alphanumeric) {
        return Setting::Off;
    }
    Setting::NotAPath
}

/// The file to write, opened: `chosen` (what `set_path` was given) wins
/// over `env` (the variable's value). `None` when neither names a file or
/// the file cannot be opened; the reason is logged as a shape (no path, no
/// value), by the one caller that runs once a process.
fn open_configured(chosen: Option<PathBuf>, env: Option<std::ffi::OsString>, limit: u64) -> Option<Sink> {
    let path = match chosen {
        Some(path) => path,
        None => match setting(&env?) {
            Setting::File(path) => path,
            Setting::Off => return None,
            Setting::NotAPath => {
                // The rule, never the value.
                tracing::warn!(
                    "HIWAVE_CONSOLE_LOG is set to something that is neither `1` (the tracing mirror) nor a path: \
                     the console log file is written only to a value with a `/` in it (`./name` for the current directory); \
                     nothing will be written to it"
                );
                return None;
            }
        },
    };
    match Sink::open(&path, limit) {
        Ok(sink) => Some(sink),
        Err(error) => {
            // Only with the switch on, and only the error's kind.
            tracing::warn!(kind = ?error.kind(), "the console log file could not be opened; nothing will be written to it");
            None
        }
    }
}

struct Sink {
    file: File,
    written: u64,
    limit: u64,
    full: bool,
}

impl Sink {
    /// Open `path` for appending, for its owner only: a new file is made
    /// 0600 and one that exists is narrowed to it. File open hardening:
    /// the path itself is opened, never waited on, and only a regular file
    /// this user owns is kept.
    fn open(path: &Path, limit: u64) -> std::io::Result<Self> {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let file = options.open(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            // The handle's own metadata (fstat), not the path's.
            let opened = file.metadata()?;
            // SAFETY: `geteuid` takes nothing, touches no memory and cannot fail.
            let me = unsafe { libc::geteuid() };
            if !opened.file_type().is_file() || opened.uid() != me {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "not a regular file owned by this user",
                ));
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        let written = file.metadata()?.len();
        // A file that filled in an earlier run stays as it ended.
        let full = written + LAST_LINE_BYTES >= limit || ends_truncated(path, written);
        Ok(Self { file, written, limit, full })
    }

    /// Append `line` and a newline in one write, or, when that would pass
    /// the cap, the `truncated` line, once, and nothing after it. Write
    /// errors are swallowed: diagnostics never fail a page.
    fn write(&mut self, line: &str) {
        if self.full {
            return;
        }
        let mut bytes = Vec::with_capacity(line.len() + 1);
        if self.written + line.len() as u64 + 1 + LAST_LINE_BYTES > self.limit {
            self.full = true;
            let last = format!("{{\"t\":\"{}\",\"truncated\":true,\"limit_bytes\":{}}}", wall_time(), self.limit);
            if self.written + last.len() as u64 + 1 > self.limit {
                return;
            }
            bytes.extend_from_slice(last.as_bytes());
        } else {
            bytes.extend_from_slice(line.as_bytes());
        }
        bytes.push(b'\n');
        if self.file.write_all(&bytes).is_ok() {
            self.written += bytes.len() as u64;
        }
    }
}

/// Whether the file's last line is a `truncated` line. A message cannot
/// forge one: its quotes are escaped in the JSON.
fn ends_truncated(path: &Path, len: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let mut tail = Vec::new();
    let read = File::open(path).and_then(|mut file| {
        file.seek(SeekFrom::Start(len.saturating_sub(2 * LAST_LINE_BYTES)))?;
        file.read_to_end(&mut tail)
    });
    if read.is_err() {
        return false;
    }
    let tail = String::from_utf8_lossy(&tail);
    let last = tail.trim_end_matches('\n').rsplit('\n').next().unwrap_or("");
    last.starts_with("{\"t\":\"") && last.contains("\",\"truncated\":true,\"limit_bytes\":")
}

/// The process's file, opened the first time it is wanted, from what
/// `set_path` was given or else the variable.
fn process_sink() -> Option<&'static Mutex<Sink>> {
    static SINK: OnceLock<Option<Mutex<Sink>>> = OnceLock::new();
    SINK.get_or_init(|| {
        let chosen = CHOSEN.lock().ok().and_then(|chosen| chosen.clone());
        open_configured(chosen, std::env::var_os("HIWAVE_CONSOLE_LOG"), MAX_FILE_BYTES).map(Mutex::new)
    })
    .as_ref()
}

/// Run `f` on the open file; `None` when the switch is off or the file
/// could not be opened. Each test thread forces its own.
#[cfg(not(test))]
fn with_sink<R>(f: impl FnOnce(&mut Sink) -> R) -> Option<R> {
    Some(f(&mut *process_sink()?.lock().ok()?))
}

#[cfg(test)]
fn with_sink<R>(f: impl FnOnce(&mut Sink) -> R) -> Option<R> {
    TEST_SINK.with(|sink| match &mut *sink.borrow_mut() {
        TestSink::Off => None,
        TestSink::Own(sink) => Some(f(sink)),
        TestSink::Process => Some(f(&mut *process_sink()?.lock().ok()?)),
    })
}

#[cfg(test)]
enum TestSink {
    Off,
    Own(Sink),
    Process,
}

#[cfg(test)]
thread_local! {
    static TEST_SINK: std::cell::RefCell<TestSink> = const { std::cell::RefCell::new(TestSink::Off) };
}

/// What a test forces on its own thread.
#[cfg(test)]
pub(crate) enum Forced {
    Off,
    /// As `set_path` with this file, stopping at this many bytes.
    On(PathBuf, u64),
    /// As a process whose `set_path` and `HIWAVE_CONSOLE_LOG` are these.
    Configured { set_path: Option<PathBuf>, env: Option<std::ffi::OsString> },
    /// The process's own file: the real `set_path` and environment, read
    /// once. Only for a test that is alone in its process.
    Process,
}

/// Tests never read the environment unless they ask for `Process`: `None`
/// and `Off` are both off; `On` and `Configured` open the file the way a
/// new process does (a file that cannot be opened is off, with a warning).
#[cfg(test)]
pub(crate) fn force_for_test(forced: Option<Forced>) {
    let sink = match forced {
        Some(Forced::On(path, limit)) => open_configured(Some(path), None, limit),
        Some(Forced::Configured { set_path, env }) => open_configured(set_path, env, MAX_FILE_BYTES),
        Some(Forced::Process) => {
            TEST_SINK.with(|slot| *slot.borrow_mut() = TestSink::Process);
            return;
        }
        Some(Forced::Off) | None => None,
    };
    TEST_SINK.with(|slot| *slot.borrow_mut() = sink.map_or(TestSink::Off, TestSink::Own));
}

/// Whether console lines are being written to a file.
pub(crate) fn enabled() -> bool {
    with_sink(|_| ()).is_some()
}

/// The view and document a line is about.
#[derive(Debug, Clone)]
pub(crate) struct Page {
    view: u64,
    url: String,
}

impl Page {
    /// `url` is the document's; it is cut to origin and path here.
    pub(crate) fn new(view: u64, url: &str) -> Self {
        Self { view, url: crate::console_log::brief_url(url) }
    }
}

#[derive(Serialize)]
struct Line<'a> {
    /// Wall time, UTC, to the millisecond.
    t: String,
    view: u64,
    page: &'a str,
    /// `console`, `uncaught` or `unhandled_rejection`.
    kind: &'a str,
    level: &'a str,
    message: &'a str,
    /// The message was longer than [`MAX_MESSAGE_BYTES`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    cut: bool,
    source: Option<String>,
    line: Option<u32>,
    column: Option<u32>,
}

/// A script position as a line shows it: `path` is an engine label or a
/// URL (see [`crate::console_log::resolve`]).
pub(crate) type At<'a> = (&'a str, u32, u32);

fn write(page: &Page, kind: &str, level: &str, message: &str, at: Option<At<'_>>) {
    let (message, cut) = cap_bytes(message, MAX_MESSAGE_BYTES);
    let line = Line {
        t: wall_time(),
        view: page.view,
        page: &page.url,
        kind,
        level,
        message,
        cut,
        source: at.map(|(path, _, _)| source(page.view, path)),
        line: at.map(|(_, line, _)| line),
        column: at.map(|(_, _, column)| column),
    };
    if let Ok(line) = serde_json::to_string(&line) {
        with_sink(|sink| sink.write(&line));
    }
}

/// A script of `view`'s place as the Inspector's console shows it: an inline
/// script's label (`inline#` and digits, nothing else) as it is (the document is `page`, and the line counts
/// from the script's own start), anything else as the URL it was loaded
/// from, cut like every devtools URL.
fn source(view: u64, path: &str) -> String {
    let inline = path.strip_prefix("inline#").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
    if inline {
        return path.to_string();
    }
    crate::console_log::resolve(view, path)
}

/// One console call, with the text the page logged.
pub(crate) fn console(page: &Page, level: LogLevel, message: &str, at: Option<At<'_>>) {
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
    write(page, "console", level, message, at);
}

/// One uncaught exception or unhandled rejection, worded as a browser's
/// console words it (`Uncaught TypeError: x`, `Uncaught (in promise) x`).
/// The place is the throw's, when the runtime knows it (`located`); a
/// module's late error names the module.
pub(crate) fn page_error(page: &Page, report: &ErrorReport) {
    if !enabled() {
        return;
    }
    let (kind, lead) =
        if report.rejection { ("unhandled_rejection", "Uncaught (in promise)") } else { ("uncaught", "Uncaught") };
    let message = match (report.name, report.message.as_deref()) {
        (Some(name), Some(text)) if !text.is_empty() => format!("{lead} {name}: {text}"),
        (Some(name), _) => format!("{lead} {name}"),
        (None, Some(text)) => format!("{lead} {text}"),
        (None, None) => format!("{lead} {}", report.value),
    };
    let at = report
        .frames
        .first()
        .filter(|_| report.located)
        .map(|frame| (frame.path.as_str(), frame.line, frame.column))
        .or_else(|| report.module.as_deref().map(|module| (module, 0, 0)));
    write(page, kind, "error", &message, at);
}

/// `text`, or its first `max` bytes back to a whole character.
fn cap_bytes(text: &str, max: usize) -> (&str, bool) {
    if text.len() <= max {
        return (text, false);
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

/// Now, as `2026-11-13T21:40:01.123Z`.
fn wall_time() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    utc(now.as_secs(), now.subsec_millis())
}

/// Seconds since 1970 as a UTC date and time (the proleptic Gregorian
/// calendar; Howard Hinnant's `civil_from_days`).
fn utc(secs: u64, millis: u32) -> String {
    let days = (secs / 86_400) as i64;
    let of_day = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_is_the_calendar_date_and_time() {
        assert_eq!(utc(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(utc(951_782_400, 7), "2000-02-29T00:00:00.007Z");
        assert_eq!(utc(1_794_606_001, 123), "2026-11-13T21:40:01.123Z");
        assert_eq!(utc(4_102_444_799, 999), "2099-12-31T23:59:59.999Z");
    }

    /// R1 nit 4: only `inline#` and digits is a label; anything else that
    /// starts that way is a URL or a specifier, and is cut like one.
    #[test]
    fn only_a_strict_inline_label_is_written_as_it_is() {
        assert_eq!(source(0, "inline#1"), "inline#1");
        assert_eq!(source(0, "inline#204"), "inline#204");
        for not_a_label in ["inline#", "inline#1?token=secret", "inline#x#secret", "inline#1/../u:secret@h/x?secret"] {
            let shown = source(0, not_a_label);
            assert!(!shown.contains("secret"), "{not_a_label:?} was written as {shown:?}");
        }
    }

    #[test]
    fn a_cut_lands_on_a_character() {
        assert_eq!(cap_bytes("abc", 3), ("abc", false));
        assert_eq!(cap_bytes("abcd", 3), ("abc", true));
        assert_eq!(cap_bytes("a\u{e9}\u{e9}", 2), ("a", true));
        assert_eq!(cap_bytes("\u{1f600}", 3), ("", true));
    }
}
