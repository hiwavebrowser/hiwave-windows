//! Resident memory of the full filter lists with and without rule names
//! (dev-tools T2, Prometheus R1 on #792). One engine per process:
//! `cargo run -p hiwave-shield --example rule_names_rss -- plain|names`.

use hiwave_shield::{AdBlocker, ResourceType};
use url::Url;

fn rss_kib() -> u64 {
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse().expect("rss")
}

fn main() {
    let names = match std::env::args().nth(1).as_deref() {
        Some("names") => true,
        Some("plain") => false,
        _ => panic!("usage: rule_names_rss plain|names"),
    };
    let before = rss_kib();
    let blocker = if names { AdBlocker::with_filter_lists_naming_rules() } else { AdBlocker::with_filter_lists() };
    let after = rss_kib();
    let url = Url::parse("https://pagead2.googlesyndication.com/pagead/js/adsbygoogle.js").unwrap();
    let source = Url::parse("https://example.com/").unwrap();
    let found = blocker.check(&url, &source, ResourceType::Script);
    println!(
        "{} rss_before_kib={before} rss_after_kib={after} engine_kib={} match={found:?}",
        if names { "names" } else { "plain" },
        after.saturating_sub(before)
    );
}
