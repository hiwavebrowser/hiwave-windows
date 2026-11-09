//! Portable S1 (#875) fragment / last-hop URL pins.
//!
//! The headless `redirect_base_tests` exercise a real fetch. These pin the
//! pure HTML+Fetch rule Linux `--lib` otherwise never sees: last hop wins,
//! and the request fragment is kept when the last hop has none.

use super::url_after_redirects;
use url::Url;

#[test]
fn a_redirect_takes_the_last_hop_and_keeps_the_request_fragment() {
    let requested = Url::parse("https://www.gmail.com/signin#inbox").unwrap();
    let last = Url::parse("https://accounts.google.com/ServiceLogin").unwrap();
    let got = url_after_redirects(&requested, &last, true);
    assert_eq!(got.as_str(), "https://accounts.google.com/ServiceLogin#inbox");
}

#[test]
fn a_last_hop_fragment_wins_over_the_request() {
    let requested = Url::parse("https://a.example/start#req").unwrap();
    let last = Url::parse("https://b.example/end#hop").unwrap();
    let got = url_after_redirects(&requested, &last, true);
    assert_eq!(got.as_str(), "https://b.example/end#hop");
}

#[test]
fn no_redirect_leaves_the_request_url_alone() {
    let requested = Url::parse("https://a.example/page#frag").unwrap();
    // Same URL, empty chain: still the request.
    assert_eq!(
        url_after_redirects(&requested, &requested, false).as_str(),
        "https://a.example/page#frag"
    );
    // A different response URL without a redirect chain must not rewrite
    // (the engine only rebinds when the chain is non-empty).
    let other = Url::parse("https://b.example/other").unwrap();
    assert_eq!(
        url_after_redirects(&requested, &other, false).as_str(),
        "https://a.example/page#frag"
    );
}
