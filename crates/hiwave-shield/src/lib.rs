//! Ad blocking and privacy protection for HiWave
//!
//! Uses Brave's adblock-rust engine for high-performance ad and tracker blocking.

pub mod filter_lists;

use adblock::lists::ParseOptions;
use adblock::Engine;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use url::Url;
use hiwave_core::HiWaveResult;

pub use filter_lists::{FilterListManager, FilterListSource, FILTER_LISTS};

/// Which list, and which of its rules, blocked a request (dev-tools T2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockMatch {
    /// The list's name (`EasyList`, `EasyPrivacy`), `HiWave defaults` for
    /// the starter rules, `custom` for rules handed to `with_rules`, or
    /// [`UNATTRIBUTED`] when the engine cannot say which list it was.
    pub list: String,
    /// The rule as the list writes it; `None` when the engine was built
    /// without rule names. Rules the engine joined into one are written
    /// as the list writes each, separated by ` <+> `: one of them matched.
    pub rule: Option<String>,
}

/// The list named for a match that rules from more than one list could
/// have made: the engine joined rules from different lists into the one
/// that matched (` <+> `), or was built without rule names.
pub const UNATTRIBUTED: &str = "unattributed";

/// What adblock-rust writes between the rules it joined into one.
const JOINED: &str = " <+> ";

/// Ad blocker powered by Brave's adblock-rust engine
pub struct AdBlocker {
    engine: Engine,
    enabled: bool,
    requests_blocked: AtomicU64,
    trackers_blocked: AtomicU64,
    /// The list each rule came from, when rules came from more than one.
    rule_lists: HashMap<String, &'static str>,
    /// The list every rule came from, when rules came from one list.
    default_list: &'static str,
    /// Rules came from more than one list. Without rule names a match then
    /// cannot be traced to its list.
    several_lists: bool,
    /// The engine keeps each rule's text, so a match names its rule. About
    /// 9 MB more for the full lists, so `with_filter_lists` leaves it off
    /// (`with_filter_lists_naming_rules` turns it on).
    names_rules: bool,
}

impl AdBlocker {
    /// The name a match by a built-in rule gives for its list.
    const DEFAULTS_LIST: &'static str = "HiWave defaults";

    /// The built-in starter rules, audited against EasyList/EasyPrivacy (SH1): every host-wide
    /// ad or tracker rule here is one the lists block or leave to their site-specific
    /// exceptions. Two were removed because the lists allow what they block on purpose: the
    /// reCAPTCHA `aframe` (EasyList has `@@||google.com/recaptcha/api2/`, and blocking it breaks
    /// captchas) and the whole `connect.facebook.net` host (it serves the SDK that logins and
    /// like buttons need; EasyList blocks only `/signals/`, `fbds.js` and `pcm.js`, third-party).
    const DEFAULT_RULES: &'static [&'static str] = &[
        // SH2: every rule here is `$third-party`: it protects a page from the tracker, and must not
        // stop a user browsing the tracker's own site (mixpanel.com blocked its own scripts).
        // Google advertising
        "||doubleclick.net^$third-party",
        "||googlesyndication.com^$third-party",
        "||googleadservices.com^$third-party",
        "||adtrafficquality.google^$third-party",
        // Social media ads/tracking
        "||ads.twitter.com^$third-party",
        "||facebook.com/tr^$third-party",
        "||tr.snapchat.com^$third-party",
        // Amazon
        "||amazon-adsystem.com^$third-party",
        // Criteo
        "||criteo.com/sync^$third-party",
        "||criteo.com/syncframe^$third-party",
        // Common ad networks
        "||adnxs.com^$third-party",
        "||adsrvr.org^$third-party",
        "||adroll.com^$third-party",
        "||taboola.com^$third-party",
        "||outbrain.com^$third-party",
        "||zedo.com^$third-party",
        "||bidswitch.net^$third-party",
        "||rubiconproject.com^$third-party",
        "||openx.net^$third-party",
        "||pubmatic.com^$third-party",
        "||casalemedia.com^$third-party",
        "||mediavine.com^$third-party",
        // Tracking pixels and analytics
        "||pixel.facebook.com^$third-party",
        "||bat.bing.com^$third-party",
        "||scorecardresearch.com^$third-party",
        "||chartbeat.com^$third-party",
        "||segment.io^$third-party",
        "||segment.com^$third-party",
        "||mixpanel.com^$third-party",
        "||hotjar.com^$third-party",
        "||fullstory.com^$third-party",
        "||mouseflow.com^$third-party",
        "||luckyorange.com^$third-party",
        // Service worker iframes (block as popups)
        "||googletagmanager.com/static/service_worker^",
    ];

    /// Creates a new ad blocker with starter rules
    pub fn new() -> Self {
        log::info!("Initializing ad blocker");

        Self::from_parts(Self::DEFAULT_RULES, HashMap::new(), Self::DEFAULTS_LIST, false, true)
    }

    /// Creates ad blocker with the given filter rules. A match names its
    /// rule (the engine keeps every rule's text).
    pub fn with_rules(rules: &[&str]) -> Self {
        log::info!("Initializing ad blocker with {} rules", rules.len());
        Self::from_parts(rules, HashMap::new(), "custom", false, true)
    }

    fn from_parts(
        rules: &[&str],
        rule_lists: HashMap<String, &'static str>,
        default_list: &'static str,
        several_lists: bool,
        names_rules: bool,
    ) -> Self {
        let engine =
            Engine::from_rules_parametrised(rules.iter().copied(), ParseOptions::default(), names_rules, true);
        Self {
            engine,
            enabled: true,
            requests_blocked: AtomicU64::new(0),
            trackers_blocked: AtomicU64::new(0),
            rule_lists,
            default_list,
            several_lists,
            names_rules,
        }
    }

    /// Creates ad blocker from several named lists: a match names the list
    /// its rule came from (the first, for a rule two lists share).
    pub fn from_named_lists(lists: &[(&'static str, &str)]) -> Self {
        Self::from_lists(lists, true)
    }

    fn from_lists(lists: &[(&'static str, &str)], names_rules: bool) -> Self {
        // One list: every match is from it, with or without rule names.
        // Several: only a match's rule text can name its list, so without
        // rule names the map would never be read (one key per list line).
        let several_lists = lists.len() > 1;
        let map = several_lists && names_rules;
        let mut rules = Vec::new();
        let mut rule_lists = HashMap::new();
        for (name, content) in lists {
            for line in content.lines() {
                if map {
                    rule_lists.entry(line.trim().to_string()).or_insert(*name);
                }
                rules.push(line);
            }
        }
        let default_list = lists.first().map(|(name, _)| *name).unwrap_or("custom");
        Self::from_parts(&rules, rule_lists, default_list, several_lists, names_rules)
    }

    /// Creates ad blocker from filter list content (e.g., EasyList)
    pub fn from_filter_list(list_content: &str) -> Self {
        log::info!("Initializing ad blocker from filter list");

        let rules: Vec<&str> = list_content.lines().collect();
        Self::with_rules(&rules)
    }

    /// Creates ad blocker with full EasyList/EasyPrivacy filter lists
    /// Falls back to default rules if filter lists can't be loaded
    pub fn with_filter_lists() -> Self {
        Self::filter_lists(false)
    }

    /// [`with_filter_lists`](Self::with_filter_lists), with a match naming
    /// its rule and list (dev-tools T2: the event log's `blocked_by`). The
    /// rule text costs about 9 MB, so only a tool that reports it asks.
    pub fn with_filter_lists_naming_rules() -> Self {
        Self::filter_lists(true)
    }

    fn filter_lists(names_rules: bool) -> Self {
        log::info!("Initializing ad blocker with EasyList/EasyPrivacy");

        match FilterListManager::new() {
            Some(manager) => {
                // Each list kept apart, so a match can name its list.
                let lists: Vec<(&'static str, String)> = FILTER_LISTS
                    .iter()
                    .filter_map(|source| manager.get_filter_list(source).map(|content| (source.name, content)))
                    .collect();
                if lists.is_empty() {
                    log::warn!("No filter lists available, using default rules");
                    Self::new()
                } else {
                    let rule_count = lists
                        .iter()
                        .flat_map(|(_, content)| content.lines())
                        .filter(|l| !l.trim().is_empty() && !l.starts_with('!'))
                        .count();
                    log::info!(
                        "Loaded {} filter rules from EasyList/EasyPrivacy",
                        rule_count
                    );
                    let named: Vec<(&'static str, &str)> =
                        lists.iter().map(|(name, content)| (*name, content.as_str())).collect();
                    Self::filter_engine(&named, names_rules)
                }
            }
            None => {
                log::warn!("Failed to initialize filter list manager, using default rules");
                Self::new()
            }
        }
    }

    /// The engine the app runs once the downloaded lists are in: what [`with_filter_lists`]
    /// (Self::with_filter_lists) builds from them. A seam, so a test can hand it lists without a
    /// download.
    ///
    /// SH1: the lists ADD to the built-in rules, they do not replace them. A host the starter
    /// rules block (and the app's pending floor blocks until the engine is ready) stays blocked
    /// after, even when EasyList only names its subdomains (`doubleclick.net` bare). One engine,
    /// so a list's exception (`@@||cdn.segment.com/analytics-next/`) still beats a built-in rule.
    /// The built-in rules come last, so a rule both have is named for the list.
    ///
    /// POLICY (Atlas, #1058): list exceptions WIN over built-in blocks. EasyList/EasyPrivacy `@@`
    /// rules are curated site-unbreak fixes and honouring them is what every mainstream blocker
    /// does; the built-in rules fill gaps the lists leave, they are not an override. A future
    /// list that excepts a built-in host therefore unblocks it. The control is the real-list walk
    /// (`the_cached_real_lists_leave_every_built_in_rule_blocking`), which fails if a list ever does.
    fn filter_engine(lists: &[(&'static str, &str)], names_rules: bool) -> Self {
        let defaults = Self::DEFAULT_RULES.join("
");
        let mut all: Vec<(&'static str, &str)> = lists.to_vec();
        all.push((Self::DEFAULTS_LIST, defaults.as_str()));
        Self::from_lists(&all, names_rules)
    }

    /// Reload rules - creates a new engine (immutable design). A match names
    /// its rule from now on, and its list is `custom`.
    pub fn load_rules(&mut self, rules: &[String]) -> HiWaveResult<()> {
        log::info!("Reloading ad blocker with {} rules", rules.len());

        let rules_refs: Vec<&str> = rules.iter().map(|s| s.as_str()).collect();
        self.engine = Engine::from_rules_debug(rules_refs, ParseOptions::default());
        self.rule_lists.clear();
        self.default_list = "custom";
        self.several_lists = false;
        self.names_rules = true;
        self.requests_blocked.store(0, Ordering::Relaxed);
        self.trackers_blocked.store(0, Ordering::Relaxed);

        Ok(())
    }

    /// Check if a request should be blocked
    pub fn should_block(&self, url: &Url, source_url: &Url, resource_type: ResourceType) -> bool {
        self.check(url, source_url, resource_type).is_some()
    }

    /// The list and rule that block a request, or `None` when none does.
    pub fn check(&self, url: &Url, source_url: &Url, resource_type: ResourceType) -> Option<BlockMatch> {
        if !self.enabled {
            return None;
        }
        // Only network requests are filtered. A `data:` (or `blob:`) URL
        // carries its own content: nothing is requested, and adblock cannot
        // build a request from it.
        if !matches!(url.scheme(), "http" | "https" | "ws" | "wss") {
            return None;
        }

        let request = adblock::request::Request::new(
            url.as_str(),
            source_url.as_str(),
            resource_type.as_str(),
        );

        match request {
            Ok(req) => {
                let result = self.engine.check_network_request(&req);

                if result.matched {
                    // The host only: not the path or query (signed URLs, tokens) and not the page it
                    // came from (browsing history). Logs carry shapes, never bodies.
                    log::debug!("Blocked: {}", url.host_str().unwrap_or("-"));
                    self.requests_blocked.fetch_add(1, Ordering::Relaxed);
                    self.trackers_blocked.fetch_add(1, Ordering::Relaxed);
                    // Without rule names the engine writes "NetworkFilter".
                    let rule = result.filter.filter(|_| self.names_rules);
                    let list = self.list_of(rule.as_deref());
                    Some(BlockMatch { list: list.to_string(), rule })
                } else {
                    None
                }
            }
            Err(e) => {
                // Host only, and not the error's text, which may quote the URL.
                let _ = e;
                log::warn!("Failed to create request for {}", url.host_str().unwrap_or("-"));
                None
            }
        }
    }

    /// The list a matched rule came from. A rule the engine joined from
    /// several (` <+> `) is looked up part by part, and named only when
    /// every part is from the same list: any of them may have matched.
    fn list_of(&self, rule: Option<&str>) -> &'static str {
        if !self.several_lists {
            return self.default_list;
        }
        if !self.names_rules {
            return UNATTRIBUTED;
        }
        let Some(rule) = rule else { return UNATTRIBUTED };
        let mut lists = rule.split(JOINED).map(|part| self.rule_lists.get(part.trim()).copied());
        match lists.next().flatten() {
            Some(list) if lists.all(|other| other == Some(list)) => list,
            _ => UNATTRIBUTED,
        }
    }

    /// Get cosmetic filters (CSS selectors to hide elements)
    pub fn get_cosmetic_filters(&self, url: &str) -> Vec<String> {
        let resources = self.engine.url_cosmetic_resources(url);
        resources.hide_selectors.into_iter().collect()
    }

    /// Enable or disable ad blocking
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        log::info!(
            "Ad blocking {}",
            if enabled { "enabled" } else { "disabled" }
        );
    }

    /// Check if ad blocking is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get blocking statistics
    pub fn get_stats(&self) -> BlockingStats {
        BlockingStats {
            requests_blocked: self.requests_blocked.load(Ordering::Relaxed),
            bytes_saved: 0,
            trackers_blocked: self.trackers_blocked.load(Ordering::Relaxed),
        }
    }

    /// Manually increment blocking counters (for user blocklist, flood protection, etc.)
    pub fn increment_block_count(&self) {
        self.requests_blocked.fetch_add(1, Ordering::Relaxed);
        self.trackers_blocked.fetch_add(1, Ordering::Relaxed);
    }
}

impl Default for AdBlocker {
    fn default() -> Self {
        Self::new()
    }
}

/// Type of resource being requested
#[derive(Debug, Clone, Copy)]
pub enum ResourceType {
    Document,
    Script,
    Image,
    Stylesheet,
    Font,
    Xhr,
    WebSocket,
    Media,
    Other,
}

impl ResourceType {
    fn as_str(&self) -> &'static str {
        match self {
            ResourceType::Document => "document",
            ResourceType::Script => "script",
            ResourceType::Image => "image",
            ResourceType::Stylesheet => "stylesheet",
            ResourceType::Font => "font",
            ResourceType::Xhr => "xmlhttprequest",
            ResourceType::WebSocket => "websocket",
            ResourceType::Media => "media",
            ResourceType::Other => "other",
        }
    }
}

/// Statistics about blocked requests
#[derive(Debug, Clone, Default)]
pub struct BlockingStats {
    pub requests_blocked: u64,
    pub bytes_saved: u64,
    pub trackers_blocked: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Capture(std::sync::Mutex<Vec<String>>);
    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata) -> bool {
            true
        }
        fn log(&self, record: &log::Record) {
            self.0.lock().unwrap().push(format!("{}", record.args()));
        }
        fn flush(&self) {}
    }
    static CAPTURE: Capture = Capture(std::sync::Mutex::new(Vec::new()));

    /// Logs carry shapes, never bodies (the standing rule): a blocked request is logged by its host.
    /// Its path, query and fragment and the page it came from (history, signed URLs, tokens) are not.
    /// The test's own host marks its lines, so a parallel test's output cannot fail it.
    #[test]
    fn a_blocked_request_is_logged_by_host_only() {
        let _ = log::set_logger(&CAPTURE);
        log::set_max_level(log::LevelFilter::Trace);
        let blocker = AdBlocker::with_rules(&["||shape-probe-host.example^"]);
        let url = Url::parse("https://shape-probe-host.example/pixel/u123abc?session=s3cr3t#frag9").unwrap();
        let source = Url::parse("https://inbox.example/mail/thread42?token=t0k3n").unwrap();
        assert!(blocker.should_block(&url, &source, ResourceType::Script));
        let lines: Vec<String> = CAPTURE
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.contains("shape-probe-host.example"))
            .cloned()
            .collect();
        assert!(!lines.is_empty(), "the block is logged, by host");
        let text = lines.join("
");
        for secret in ["pixel", "u123abc", "session", "s3cr3t", "frag9", "inbox", "thread42", "t0k3n"] {
            assert!(!text.contains(secret), "{secret} reached a log line: {text}");
        }
    }

    /// A small stand-in for EasyList/EasyPrivacy: third-party-only rules for some hosts, the
    /// captcha allow EasyList carries (`@@||google.com/recaptcha/api2/`), its allow for the
    /// segment analytics bundle, and its three narrow Facebook rules. It names none of the
    /// built-in floor hosts below.
    const LIST: &str = concat!(
        "! stand-in
",
        "||ads.example.com^$third-party
",
        "@@||google.com/recaptcha/api2/
",
        "@@||cdn.segment.com/analytics-next/
",
        "||connect.facebook.net/signals/$third-party
",
        "||connect.facebook.net^*/fbds.js$third-party
",
        "||connect.facebook.net^*/pcm.js$third-party
",
    );

    fn page() -> Url {
        Url::parse("https://news.example/").unwrap()
    }

    /// A URL the built-in rule `||host[/path]^` blocks.
    fn url_for_rule(rule: &str) -> Url {
        let rest = rule.trim_start_matches("||").split('$').next().unwrap().trim_end_matches('^');
        let url = if rest.contains('/') { format!("https://{rest}?x=1") } else { format!("https://{rest}/x.js") };
        Url::parse(&url).unwrap()
    }

    /// SH1: once the downloaded lists are loaded the built-in rules still apply (the lists add to
    /// them, they do not replace them), so a floor host is blocked both before the engine is ready
    /// (the starter rules) and after.
    #[test]
    fn every_built_in_rule_still_blocks_once_the_lists_are_loaded() {
        let before = AdBlocker::new();
        let after = AdBlocker::filter_engine(&[("EasyList", LIST)], false);
        for rule in AdBlocker::DEFAULT_RULES {
            let url = url_for_rule(rule);
            assert!(before.should_block(&url, &page(), ResourceType::Script), "{rule}: blocked before ready ({url})");
            assert!(after.should_block(&url, &page(), ResourceType::Script), "{rule}: blocked after ready ({url})");
        }
    }

    /// What a captcha or a login needs stays allowed before and after: the reCAPTCHA frame
    /// (EasyList allows `google.com/recaptcha/api2/` on purpose), the Facebook SDK (EasyList blocks
    /// only `/signals/`, `fbds.js` and `pcm.js`), and a first-party script.
    #[test]
    fn a_captcha_frame_the_facebook_sdk_and_a_first_party_script_stay_allowed() {
        let before = AdBlocker::new();
        let after = AdBlocker::filter_engine(&[("EasyList", LIST)], false);
        let allowed = [
            ("https://www.google.com/recaptcha/api2/aframe", ResourceType::Other),
            ("https://www.google.com/recaptcha/api2/anchor?k=1", ResourceType::Other),
            ("https://hcaptcha.com/1/api.js", ResourceType::Script),
            ("https://newassets.hcaptcha.com/captcha/v1/abc/static/hcaptcha.html", ResourceType::Other),
            ("https://connect.facebook.net/en_US/sdk.js", ResourceType::Script),
            ("https://news.example/app.js", ResourceType::Script),
        ];
        for (url, kind) in allowed {
            let url = Url::parse(url).unwrap();
            assert!(!before.should_block(&url, &page(), kind), "allowed before ready: {url}");
            assert!(!after.should_block(&url, &page(), kind), "allowed after ready: {url}");
        }
    }

    /// Dropping the `connect.facebook.net` default left the lists' three narrow rules to block
    /// its tracking scripts; they must still block once the engine is ready.
    #[test]
    fn the_facebook_tracking_scripts_still_block_and_the_sdk_does_not() {
        let after = AdBlocker::filter_engine(&[("EasyList", LIST)], false);
        for blocked in [
            "https://connect.facebook.net/signals/config/1",
            "https://connect.facebook.net/en_US/fbds.js",
            "https://connect.facebook.net/en_US/pcm.js",
        ] {
            assert!(after.should_block(&Url::parse(blocked).unwrap(), &page(), ResourceType::Script), "{blocked}");
        }
    }

    /// The lists as the app has them on disk, read WITHOUT downloading (`None` when nothing is
    /// cached, which is the case on a fresh checkout: the test then says so and does nothing).
    fn cached_lists() -> Option<Vec<(&'static str, String)>> {
        let dir = dirs::data_local_dir()?.join("hiwave").join("filter-lists");
        let lists: Vec<(&'static str, String)> = FILTER_LISTS
            .iter()
            .filter_map(|source| {
                std::fs::read_to_string(dir.join(source.filename)).ok().map(|content| (source.name, content))
            })
            .filter(|(_, content)| content.lines().count() > 1000)
            .collect();
        (!lists.is_empty()).then_some(lists)
    }

    /// SH1 against the REAL lists (Prometheus #1058 nit 1, Atlas policy: exceptions win): every
    /// remaining built-in host is still blocked with EasyList and EasyPrivacy loaded, so no real
    /// list exception unblocks one today; a captcha frame stays allowed; the Facebook tracking
    /// scripts block. `track.adform.net` is blocked by EasyList and by no built-in rule: if it is
    /// not blocked here, no list was loaded and the rest of the test would be proving the
    /// defaults alone.
    #[test]
    fn the_cached_real_lists_leave_every_built_in_rule_blocking() {
        let Some(lists) = cached_lists() else {
            eprintln!("SKIP: no cached EasyList/EasyPrivacy on this machine (run the app once)");
            return;
        };
        let named: Vec<(&'static str, &str)> = lists.iter().map(|(n, c)| (*n, c.as_str())).collect();
        let after = AdBlocker::filter_engine(&named, false);
        let adform = Url::parse("https://track.adform.net/x.js").unwrap();
        assert!(!AdBlocker::new().should_block(&adform, &page(), ResourceType::Script), "no built-in rule covers adform");
        assert!(after.should_block(&adform, &page(), ResourceType::Script), "a list really loaded");
        for rule in AdBlocker::DEFAULT_RULES {
            let url = url_for_rule(rule);
            for kind in [ResourceType::Script, ResourceType::Xhr, ResourceType::Image] {
                assert!(after.should_block(&url, &page(), kind), "{rule}: a real list unblocks {url} ({kind:?})");
            }
        }
        for allowed in [
            "https://hcaptcha.com/1/api.js",
            "https://www.google.com/recaptcha/api2/anchor?k=1",
            "https://connect.facebook.net/en_US/sdk.js",
            "https://cdn.segment.com/analytics-next/bundle.js",
        ] {
            assert!(!after.should_block(&Url::parse(allowed).unwrap(), &page(), ResourceType::Script), "{allowed}");
        }
        for blocked in ["https://connect.facebook.net/signals/config/1", "https://connect.facebook.net/en_US/fbds.js"] {
            assert!(after.should_block(&Url::parse(blocked).unwrap(), &page(), ResourceType::Script), "{blocked}");
        }
    }

    /// A list's own exception still beats a built-in rule (they are one engine): segment's
    /// analytics bundle is allowed by EasyPrivacy, the rest of segment.io stays blocked.
    #[test]
    fn a_list_exception_still_beats_a_built_in_rule() {
        let after = AdBlocker::filter_engine(&[("EasyList", LIST)], false);
        let bundle = Url::parse("https://cdn.segment.com/analytics-next/bundle.js").unwrap();
        let track = Url::parse("https://api.segment.io/v1/t").unwrap();
        assert!(!after.should_block(&bundle, &page(), ResourceType::Script), "the allowed bundle");
        assert!(after.should_block(&track, &page(), ResourceType::Xhr), "the tracking call");
    }

    /// A block by a built-in rule names the built-in list, and a block by the lists names theirs.
    #[test]
    fn a_block_by_a_built_in_rule_names_the_built_in_list() {
        let after = AdBlocker::filter_engine(&[("EasyList", LIST)], true);
        let floor = Url::parse("https://adsrvr.org/x.js").unwrap();
        let listed = Url::parse("https://ads.example.com/b.js").unwrap();
        assert_eq!(after.check(&floor, &page(), ResourceType::Script).map(|m| m.list), Some("HiWave defaults".into()));
        assert_eq!(after.check(&listed, &page(), ResourceType::Script).map(|m| m.list), Some("EasyList".into()));
    }

    #[test]
    fn test_create_blocker() {
        let blocker = AdBlocker::new();
        assert!(blocker.is_enabled());
    }

    #[test]
    fn test_enable_disable() {
        let mut blocker = AdBlocker::new();

        blocker.set_enabled(false);
        assert!(!blocker.is_enabled());

        blocker.set_enabled(true);
        assert!(blocker.is_enabled());
    }

    #[test]
    fn test_with_rules() {
        let rules = vec!["||ads.example.com^", "||tracking.example.org^"];
        let blocker = AdBlocker::with_rules(&rules);
        assert!(blocker.is_enabled());
    }

    #[test]
    fn test_blocking() {
        let rules = vec!["||ads.example.com^"];
        let blocker = AdBlocker::with_rules(&rules);

        let url = Url::parse("https://ads.example.com/banner.js").unwrap();
        let source = Url::parse("https://example.com/").unwrap();

        let blocked = blocker.should_block(&url, &source, ResourceType::Script);
        assert!(blocked);
    }

    /// Dev-tools T2: a block names the list its rule came from and the rule
    /// as the list writes it; an allowed request names nothing.
    #[test]
    fn a_block_names_its_list_and_rule() {
        let blocker = AdBlocker::from_named_lists(&[
            ("EasyList", "! comment\n||ads.example.com^\n"),
            ("EasyPrivacy", "||pixel.example.org^$image\n"),
        ]);
        let source = Url::parse("https://example.com/").unwrap();
        let ad = Url::parse("https://ads.example.com/banner.js").unwrap();
        let pixel = Url::parse("https://pixel.example.org/p.gif").unwrap();
        let fine = Url::parse("https://example.com/app.js").unwrap();
        assert_eq!(
            blocker.check(&ad, &source, ResourceType::Script),
            Some(BlockMatch { list: "EasyList".into(), rule: Some("||ads.example.com^".into()) })
        );
        assert_eq!(
            blocker.check(&pixel, &source, ResourceType::Image),
            Some(BlockMatch { list: "EasyPrivacy".into(), rule: Some("||pixel.example.org^$image".into()) })
        );
        assert_eq!(blocker.check(&fine, &source, ResourceType::Script), None);
        assert_eq!(
            AdBlocker::new().check(&Url::parse("https://doubleclick.net/x").unwrap(), &source, ResourceType::Script),
            Some(BlockMatch { list: "HiWave defaults".into(), rule: Some("||doubleclick.net^$third-party".into()) })
        );
    }

    /// Prometheus R1 on #792: adblock-rust joins plain pattern rules with
    /// the same options into one (` <+> `), which no list writes. Such a
    /// match is named for a list only when every joined rule is from it.
    #[test]
    fn a_joined_rule_names_a_list_only_when_all_its_parts_are_from_it() {
        let source = Url::parse("https://example.com/").unwrap();
        let ad = Url::parse("https://example.com/adbanner.js").unwrap();

        // The rule that matched is EasyPrivacy's; it is joined with one of
        // EasyList's (same options, same token), so either could have matched.
        let mixed = AdBlocker::from_named_lists(&[("EasyList", "/adbanner.gif\n"), ("EasyPrivacy", "/adbanner.js\n")]);
        let found = mixed.check(&ad, &source, ResourceType::Script).expect("blocked");
        assert!(found.rule.as_deref().is_some_and(|r| r.contains(JOINED)), "the engine joined the rules: {found:?}");
        assert_eq!(found.list, UNATTRIBUTED, "{found:?}");

        // Both joined rules are EasyPrivacy's.
        let same = AdBlocker::from_named_lists(&[
            ("EasyList", "||ads.example.org^\n"),
            ("EasyPrivacy", "/adbanner.js\n/adbanner.gif\n"),
        ]);
        let found = same.check(&ad, &source, ResourceType::Script).expect("blocked");
        assert!(found.rule.as_deref().is_some_and(|r| r.contains(JOINED)), "the engine joined the rules: {found:?}");
        assert_eq!(found.list, "EasyPrivacy", "{found:?}");
    }

    /// Prometheus R1 on #792: the rule text costs every user about 9 MB, so
    /// the full lists keep it only when asked. Without it a match names no
    /// rule, and no list when several were loaded.
    #[test]
    fn without_rule_names_a_match_names_no_rule() {
        let source = Url::parse("https://example.com/").unwrap();
        let ad = Url::parse("https://ads.example.com/banner.js").unwrap();
        let lists = [("EasyList", "||ads.example.com^\n"), ("EasyPrivacy", "||pixel.example.org^\n")];
        assert_eq!(
            AdBlocker::from_lists(&lists, false).check(&ad, &source, ResourceType::Script),
            Some(BlockMatch { list: UNATTRIBUTED.into(), rule: None })
        );
        assert_eq!(
            AdBlocker::from_lists(&lists[..1], false).check(&ad, &source, ResourceType::Script),
            Some(BlockMatch { list: "EasyList".into(), rule: None })
        );
        assert_eq!(
            AdBlocker::from_lists(&lists, true).check(&ad, &source, ResourceType::Script),
            Some(BlockMatch { list: "EasyList".into(), rule: Some("||ads.example.com^".into()) })
        );
    }

    /// #792 independent review: the app's constructor (`with_filter_lists`:
    /// two lists, no rule names) built a rule-to-list map it never read, one
    /// key per list line, in each of the app's two engines.
    #[test]
    fn without_rule_names_two_lists_build_no_rule_map() {
        let source = Url::parse("https://example.com/").unwrap();
        let ad = Url::parse("https://ads.example.com/banner.js").unwrap();
        let lists = [("EasyList", "||ads.example.com^\n"), ("EasyPrivacy", "||pixel.example.org^\n")];
        let app = AdBlocker::from_lists(&lists, false);
        assert!(app.rule_lists.is_empty(), "{} keys", app.rule_lists.len());
        assert_eq!(
            app.check(&ad, &source, ResourceType::Script),
            Some(BlockMatch { list: UNATTRIBUTED.into(), rule: None })
        );
        assert_eq!(AdBlocker::from_lists(&lists, true).rule_lists.len(), 2);
    }

    /// A rule in both lists is credited to the first (`or_insert`).
    #[test]
    fn a_rule_in_both_lists_names_the_first() {
        let source = Url::parse("https://example.com/").unwrap();
        let ad = Url::parse("https://ads.example.com/banner.js").unwrap();
        let blocker = AdBlocker::from_named_lists(&[
            ("EasyList", "||ads.example.com^\n"),
            ("EasyPrivacy", "||ads.example.com^\n||pixel.example.org^\n"),
        ]);
        assert_eq!(blocker.check(&ad, &source, ResourceType::Script).map(|m| m.list), Some("EasyList".into()));
    }

    #[test]
    fn a_data_url_is_never_blocked() {
        // Even a rule that names it: a `data:` URL is not a request.
        let blocker = AdBlocker::with_rules(&["STICKY_CANARY", "*$script"]);
        let url = Url::parse("data:text/javascript,window.STICKY_CANARY=1").unwrap();
        let source = Url::parse("https://www.reddit.com/").unwrap();
        assert!(!blocker.should_block(&url, &source, ResourceType::Script));
        assert_eq!(blocker.requests_blocked.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn type_option_blocks_only_matching_resource_types() {
        // Privacy pin #364 tags every fetch with a destination so EasyList
        // `$script` / `$image` options apply. A mis-mapped type would either
        // leak tracker scripts or break first-party images.
        let blocker = AdBlocker::with_rules(&["||ads.example.com^$script"]);
        let url = Url::parse("https://ads.example.com/x").unwrap();
        let source = Url::parse("https://news.example/").unwrap();

        assert!(blocker.should_block(&url, &source, ResourceType::Script));
        assert!(
            !blocker.should_block(&url, &source, ResourceType::Image),
            "image must not match a $script rule"
        );
        assert!(
            !blocker.should_block(&url, &source, ResourceType::Document),
            "document must not match a $script rule"
        );
        assert!(!blocker.should_block(&url, &source, ResourceType::Stylesheet));
        assert!(!blocker.should_block(&url, &source, ResourceType::Font));
    }

    #[test]
    fn third_party_option_spares_first_party_requests() {
        let blocker = AdBlocker::with_rules(&["||tracker.example^$third-party"]);
        let url = Url::parse("https://tracker.example/pixel").unwrap();
        let first_party = Url::parse("https://tracker.example/page").unwrap();
        let third_party = Url::parse("https://news.example/").unwrap();

        assert!(
            !blocker.should_block(&url, &first_party, ResourceType::Image),
            "same-site request must not match $third-party"
        );
        assert!(blocker.should_block(&url, &third_party, ResourceType::Image));
    }

    #[test]
    fn default_rules_block_known_trackers_and_spare_benign_hosts() {
        let blocker = AdBlocker::new();
        let source = Url::parse("https://example.com/").unwrap();

        let tracker = Url::parse("https://doubleclick.net/pagead/js").unwrap();
        assert!(blocker.should_block(&tracker, &source, ResourceType::Script));

        let benign = Url::parse("https://example.com/app.js").unwrap();
        assert!(!blocker.should_block(&benign, &source, ResourceType::Script));
    }

    #[test]
    fn disabled_blocker_never_matches() {
        let mut blocker = AdBlocker::with_rules(&["||ads.example.com^"]);
        blocker.set_enabled(false);
        let url = Url::parse("https://ads.example.com/banner.js").unwrap();
        let source = Url::parse("https://example.com/").unwrap();
        assert!(!blocker.should_block(&url, &source, ResourceType::Script));
        assert_eq!(blocker.get_stats().requests_blocked, 0);
    }

    #[test]
    fn a_match_increments_blocking_stats() {
        let blocker = AdBlocker::with_rules(&["||ads.example.com^"]);
        let url = Url::parse("https://ads.example.com/banner.js").unwrap();
        let source = Url::parse("https://example.com/").unwrap();
        assert!(blocker.should_block(&url, &source, ResourceType::Script));
        let stats = blocker.get_stats();
        assert_eq!(stats.requests_blocked, 1);
        assert_eq!(stats.trackers_blocked, 1);
    }

    /// SH2: the host-wide defaults are third-party rules. Browsing mixpanel.com must not block
    /// mixpanel.com's own scripts; embedding it on another site still does.
    #[test]
    fn a_host_wide_default_does_not_block_the_site_s_own_subresources() {
        let blocker = AdBlocker::new();
        let script = |url: &str, page: &str| {
            blocker.check(&Url::parse(url).unwrap(), &Url::parse(page).unwrap(), ResourceType::Script)
        };
        for host in ["mixpanel.com", "segment.com", "hotjar.com", "taboola.com"] {
            let own = format!("https://{host}/lib.js");
            let page = format!("https://{host}/");
            assert!(script(&own, &page).is_none(), "{host} blocked on its own site");
            assert!(script(&own, "https://news.example/").is_some(), "{host} not blocked as a third party");
        }
    }
}
