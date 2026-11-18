//! A response body reaches script in time proportional to its size.
//!
//! Measured 2026-11-11 on develop 77610d63 (release, loopback): `fetch()` of
//! 64 KB took 357 ms, 256 KB 3119 ms, and 1 MB was still running at the
//! 15 s script budget; Chrome 148 answers 8 MB in 20 ms. Every byte crossed
//! into script as base64 inside the delivery's source text and was then
//! taken apart by script loops (`atob`, a `charCodeAt` loop, a `TextDecoder`
//! that builds its string one `+=` at a time), and a string built by `+=`
//! is copied on every step.
//!
//! The pin has two halves, per size and per surface:
//! - no script loop runs once per byte: the runtime's loop limit is set far
//!   below the body's length, so a per-byte loop anywhere on the path stops
//!   the script (this half does not depend on the machine's load);
//! - a wall-clock bound, generous enough for a loaded machine.
//!
//! Against a real loopback server and the real policy, as `script_net_tests`.

use crate::script_net::pump;
use crate::script_net_tests::serve_byte_routes;
use rustkit_bindings::DomBindings;
use rustkit_js::{JsRuntime, JsValue};
use rustkit_net::policy::FetchPolicy;
use rustkit_net::{LoaderConfig, ResourceLoader};
use std::time::{Duration, Instant};
use url::Url;

const KB64: usize = 64 * 1024;
const MB1: usize = 1024 * 1024;
const MB8: usize = 8 * 1024 * 1024;

/// No loop on the delivery path may turn this many times: far below the
/// smallest body's length, far above anything per-request.
const LOOP_LIMIT: u64 = 5_000;

/// `/text-N`: N bytes of UTF-8 (ASCII with a two-byte and a three-byte
/// character at fixed places, so a decoder that drops or doubles a byte
/// shows). `/json-N`: a JSON object N bytes long. `/bytes-N`: N bytes that
/// are not UTF-8.
fn text_body(n: usize) -> Vec<u8> {
    let mut body = vec![b'a'; n];
    body[0] = b'<';
    body[10..12].copy_from_slice("é".as_bytes());
    body[n / 2..n / 2 + 3].copy_from_slice("€".as_bytes());
    body[n - 1] = b'>';
    body
}

fn json_body(n: usize) -> Vec<u8> {
    let head = br#"{"n":7,"s":""#;
    let tail = br#""}"#;
    let mut body = Vec::with_capacity(n);
    body.extend_from_slice(head);
    body.resize(n - tail.len(), b'j');
    body.extend_from_slice(tail);
    body
}

fn byte_body(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8 ^ 0x80).collect()
}

struct Rig {
    bindings: DomBindings,
    policy: FetchPolicy,
    loader: ResourceLoader,
    rt: tokio::runtime::Runtime,
    port: u16,
}

fn rig() -> Rig {
    let mut routes: Vec<(&'static str, &'static str, Vec<u8>)> = Vec::new();
    for (n, text, json, bytes) in [
        (KB64, "/text-64k", "/json-64k", "/bytes-64k"),
        (MB1, "/text-1m", "/json-1m", "/bytes-1m"),
        (MB8, "/text-8m", "/json-8m", "/bytes-8m"),
    ] {
        routes.push((text, "text/plain; charset=utf-8", text_body(n)));
        routes.push((json, "application/json", json_body(n)));
        routes.push((bytes, "application/octet-stream", byte_body(n)));
    }
    rig_with(routes)
}

fn rig_with(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> Rig {
    let server = serve_byte_routes(routes);
    let bindings = DomBindings::new(JsRuntime::new().unwrap()).unwrap();
    bindings.enable_net_bridge().unwrap();
    bindings.set_loop_iteration_limit(LOOP_LIMIT);
    Rig {
        policy: FetchPolicy::for_page(Url::parse(&format!("http://127.0.0.1:{}/", server.port)).unwrap(), None),
        loader: ResourceLoader::new(LoaderConfig::default()).unwrap(),
        rt: tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap(),
        bindings,
        port: server.port,
    }
}

impl Rig {
    /// Run `script` (which sets `window.out` when its request is answered),
    /// pump the network until it has, and return `out` and how long the
    /// whole exchange took.
    fn exchange(&self, script: &str) -> (String, Duration) {
        let started = Instant::now();
        let script = script.replace("ORIGIN", &format!("http://127.0.0.1:{}", self.port));
        self.bindings
            .evaluate(&format!("window.out = 'no answer'; {script}"))
            .unwrap_or_else(|e| panic!("the request script failed: {e:?}"));
        for _ in 0..8 {
            let _ = self.rt.block_on(pump(
                &self.bindings,
                &self.policy,
                &self.loader,
                tokio::time::Instant::now() + Duration::from_secs(30),
                8,
                None,
                &crate::inspector::Inspector::new(&Default::default()),
                false,
            ));
            if self.out() != "no answer" {
                break;
            }
        }
        (self.out(), started.elapsed())
    }

    fn out(&self) -> String {
        match self.bindings.evaluate("String(window.out)") {
            Ok(JsValue::String(s)) => s,
            other => format!("{other:?}"),
        }
    }
}

/// What script should see of `/text-N`: its length in UTF-16 units (the two
/// multi-byte characters are one unit each) and the characters at the
/// places `text_body` marked.
fn text_seen(n: usize) -> String {
    format!("{}:<:é:€:>", n - 3)
}

const TEXT_PROBE: &str = "t.length + ':' + t[0] + ':' + t[10] + ':' + t[BYTES / 2 - 1] + ':' + t[t.length - 1]";
const BYTES_PROBE: &str = "b.byteLength + ':' + v[0] + ':' + v[250] + ':' + v[251] + ':' + v[b.byteLength - 1]";

fn bytes_seen(n: usize) -> String {
    let at = |i: usize| (i % 251) as u8 ^ 0x80;
    format!("{n}:{}:{}:{}:{}", at(0), at(250), at(251), at(n - 1))
}

fn check(label: &str, size: usize, got: (String, Duration), want: String, within: Duration) {
    let (out, took) = got;
    eprintln!("{label} {size} bytes: {} ms", took.as_millis());
    assert_eq!(out, want, "{label}, {size} bytes");
    assert!(took <= within, "{label}, {size} bytes took {took:?} (limit {within:?})");
}

fn all_surfaces(suffix: &str, size: usize, within: Duration) {
    let rig = rig();
    let fail = ".catch(function (e) { window.out = 'failed: ' + e; })";

    let text = format!(
        "fetch('ORIGIN/text-{suffix}').then(function (r) {{ return r.text(); }}).then(function (t) {{ window.out = {}; }}){fail};",
        TEXT_PROBE.replace("BYTES", &size.to_string())
    );
    check("fetch text()", size, rig.exchange(&text), text_seen(size), within);

    let buffer = format!(
        "fetch('ORIGIN/bytes-{suffix}').then(function (r) {{ return r.arrayBuffer(); }}).then(function (b) {{ var v = new Uint8Array(b); window.out = {BYTES_PROBE}; }}){fail};"
    );
    check("fetch arrayBuffer()", size, rig.exchange(&buffer), bytes_seen(size), within);

    let json = format!(
        "fetch('ORIGIN/json-{suffix}').then(function (r) {{ return r.json(); }}).then(function (j) {{ window.out = j.n + ':' + j.s.length + ':' + j.s[0] + j.s[j.s.length - 1]; }}){fail};"
    );
    check("fetch json()", size, rig.exchange(&json), format!("7:{}:jj", size - 14), within);

    let xhr_text = format!(
        "var x = new XMLHttpRequest(); x.open('GET', 'ORIGIN/text-{suffix}'); \
         x.onload = function () {{ var t = x.responseText; window.out = {}; }}; \
         x.onerror = function () {{ window.out = 'failed: error event'; }}; x.send();",
        TEXT_PROBE.replace("BYTES", &size.to_string())
    );
    check("XHR responseText", size, rig.exchange(&xhr_text), text_seen(size), within);

    let xhr_buffer = format!(
        "var y = new XMLHttpRequest(); y.open('GET', 'ORIGIN/bytes-{suffix}'); y.responseType = 'arraybuffer'; \
         y.onload = function () {{ var b = y.response, v = new Uint8Array(b); window.out = {BYTES_PROBE}; }}; \
         y.onerror = function () {{ window.out = 'failed: error event'; }}; y.send();"
    );
    check("XHR arraybuffer", size, rig.exchange(&xhr_buffer), bytes_seen(size), within);

    let xhr_json = format!(
        "var z = new XMLHttpRequest(); z.open('GET', 'ORIGIN/json-{suffix}'); z.responseType = 'json'; \
         z.onload = function () {{ var j = z.response; window.out = j.n + ':' + j.s.length + ':' + j.s[0] + j.s[j.s.length - 1]; }}; \
         z.onerror = function () {{ window.out = 'failed: error event'; }}; z.send();"
    );
    check("XHR json", size, rig.exchange(&xhr_json), format!("7:{}:jj", size - 14), within);
}

#[test]
fn a_64_kb_body_reaches_script_without_a_per_byte_loop() {
    all_surfaces("64k", KB64, Duration::from_millis(500));
}

#[test]
fn a_1_mb_body_reaches_script_without_a_per_byte_loop() {
    all_surfaces("1m", MB1, Duration::from_millis(1000));
}

#[test]
fn an_8_mb_body_reaches_script_without_a_per_byte_loop() {
    all_surfaces("8m", MB8, Duration::from_millis(2500));
}

/// The encoders and decoders the bodies go through, as a page calls them:
/// megabytes at a time, no per-byte script loop, the same answers.
#[test]
fn text_and_base64_codecs_take_megabytes_without_a_per_byte_loop() {
    let rig = rig();
    let started = Instant::now();
    let out = rig
        .bindings
        .evaluate(
            "var big = function (n, c) { return new TextDecoder().decode(new Uint8Array(n).fill(c)); };\
             var s = 'é€😀' + big(2 * 1024 * 1024, 97) + '\\ud800';\
             var bytes = new TextEncoder().encode(s);\
             var back = new TextDecoder().decode(bytes);\
             var b64 = btoa('\\u00ff' + big(1024 * 1024, 98));\
             var bin = atob(b64);\
             [bytes.length, bytes[0], bytes[1], bytes[bytes.length - 1], back.length, back.charCodeAt(back.length - 1),\
              back.slice(0, 4) === 'é€😀', b64.length, b64.slice(0, 4), bin.length, bin.charCodeAt(0), bin[bin.length - 1]].join()",
        )
        .unwrap_or_else(|e| panic!("the codec script failed: {e:?}"));
    // 2 + 3 + 4 bytes, 2 MiB of 'a', and a lone surrogate as U+FFFD (3 bytes).
    let n = 2 * 1024 * 1024 + 12;
    let want = format!("{n},195,169,189,{},65533,true,{},/2Ji,{},255,b", 2 * 1024 * 1024 + 5, (1024 * 1024 + 1 + 2) / 3 * 4, 1024 * 1024 + 1);
    assert_eq!(format!("{out:?}"), format!("{:?}", JsValue::String(want)));
    assert!(started.elapsed() <= Duration::from_millis(1500), "took {:?}", started.elapsed());
}

// ── The response cap (row 12) ──────────────────────────────────────────────
//
// `FetchLimits::max_response_bytes` was 10 MiB for everything the policy
// fetches for script. JetStream 2.2 preloads three files of 11.9 to 12.2 MB
// with `fetch()`, and ordinary sites ship bundles that size. Atlas,
// 2026-11-11: 64 MiB for script-initiated fetch/XHR and module scripts.

const MB12: usize = 12 * 1024 * 1024;
const CAP: usize = 64 * 1024 * 1024;

#[test]
fn a_12_mb_body_is_under_the_response_cap() {
    let rig = rig_with(vec![("/bytes-12m", "application/octet-stream", byte_body(MB12))]);
    let script = format!(
        "fetch('ORIGIN/bytes-12m').then(function (r) {{ return r.arrayBuffer(); }}).then(function (b) {{ var v = new Uint8Array(b); window.out = {BYTES_PROBE}; }}).catch(function (e) {{ window.out = 'failed: ' + e; }});"
    );
    check("fetch arrayBuffer()", MB12, rig.exchange(&script), bytes_seen(MB12), Duration::from_millis(3000));
}

#[test]
fn a_body_one_byte_over_the_response_cap_is_refused_and_one_at_it_is_not() {
    assert_eq!(rustkit_net::policy::FetchLimits::default().max_response_bytes, CAP);
    let rig = rig_with(vec![
        ("/at-cap", "application/octet-stream", vec![7u8; CAP]),
        ("/over-cap", "application/octet-stream", vec![7u8; CAP + 1]),
    ]);
    let ask = |path: &str| {
        format!(
            "fetch('ORIGIN{path}').then(function (r) {{ return r.arrayBuffer(); }}).then(function (b) {{ window.out = 'bytes ' + b.byteLength; }}).catch(function (e) {{ window.out = 'failed: ' + e; }});"
        )
    };
    assert_eq!(rig.exchange(&ask("/at-cap")).0, format!("bytes {CAP}"));
    // The page learns only that it failed, as for every refusal.
    assert_eq!(rig.exchange(&ask("/over-cap")).0, "failed: TypeError: Failed to fetch");
}

// ── Blob URLs (row 13) ─────────────────────────────────────────────────────

/// `URL.createObjectURL(blob)` names bytes the page already holds: fetching
/// it answers from them, with no request, until it is revoked. (It returned
/// a URL nothing stood behind.) Chrome 148: the text, then a TypeError.
#[test]
fn fetching_a_blob_url_answers_from_the_blob_until_it_is_revoked() {
    let rig = rig();
    let made = "var u = URL.createObjectURL(new Blob(['from ', 'a blob'], { type: 'text/plain' }));";
    let ask = "fetch(u).then(function (r) { return r.text().then(function (t) { window.out = r.status + ':' + r.headers.get('content-type') + ':' + t; }); }).catch(function (e) { window.out = 'failed: ' + e.name; });";
    assert_eq!(rig.exchange(&format!("{made} {ask}")).0, "200:text/plain:from a blob");
    assert_eq!(rig.exchange(&format!("URL.revokeObjectURL(u); {ask}")).0, "failed: TypeError");
}
