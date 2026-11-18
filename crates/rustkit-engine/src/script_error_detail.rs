//! Which scripts the page is told about in full when they throw.
//!
//! Script errors from another origin are reported without detail, as the
//! HTML standard's muted errors: the `error` event says "Script error." and
//! carries no file, position or value. That is decided here, once, when a
//! script is fetched, from the document's URL and where the script came
//! from in the end, and handed to the runtime with the script
//! (`DomBindings::register_script`). The runtime reports a script it was
//! never told about the same way.
//!
//! A script is reported in full only when the document has an origin of its
//! own and every URL of the script's fetch (each redirect and the final
//! response) is of that origin. There is no `crossorigin` (CORS) case yet:
//! such a script is reported without detail too.

use url::Url;

/// Whether the errors of a script fetched for the document at `document`
/// are reported without detail: `redirects` are the URLs that redirected on
/// the way, `landed` the URL of the response that was read.
pub(crate) fn without_detail<'a>(document: Option<&Url>, redirects: impl IntoIterator<Item = &'a Url>, landed: &'a Url) -> bool {
    rustkit_bindings::script_without_detail(document, redirects, landed)
}

/// [`without_detail`] for a response.
pub(crate) fn of_response(document: Option<&Url>, response: &rustkit_net::Response) -> bool {
    without_detail(document, response.redirect_chain.iter().map(|(url, _)| url), &response.url)
}

#[cfg(test)]
mod tests {
    use super::without_detail;
    use url::Url;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn decided(document: Option<&str>, redirects: &[&str], landed: &str) -> bool {
        let document = document.map(u);
        let redirects: Vec<Url> = redirects.iter().map(|s| u(s)).collect();
        without_detail(document.as_ref(), redirects.iter(), &u(landed))
    }

    /// The page's own origin, directly and after a redirect that stays in it: in full.
    #[test]
    fn a_script_of_the_documents_origin_is_reported_in_full() {
        assert!(!decided(Some("https://a.test/page"), &[], "https://a.test/s.js"));
        assert!(!decided(Some("https://a.test/page"), &["https://a.test/old.js"], "https://a.test/new.js"));
        assert!(!decided(Some("http://127.0.0.1:8000/"), &[], "http://127.0.0.1:8000/s.js"));
    }

    /// Another host, scheme or port; a same-origin URL that redirects away; one that leaves and
    /// comes back: without detail.
    #[test]
    fn a_script_that_is_or_was_another_origins_is_reported_without_detail() {
        assert!(decided(Some("https://a.test/page"), &[], "https://b.test/s.js"));
        assert!(decided(Some("https://a.test/page"), &[], "http://a.test/s.js"));
        assert!(decided(Some("http://127.0.0.1:8000/"), &[], "http://127.0.0.1:8001/s.js"));
        assert!(decided(Some("https://a.test/page"), &["https://a.test/s.js"], "https://b.test/s.js"));
        assert!(decided(Some("https://a.test/page"), &["https://a.test/s.js", "https://b.test/hop"], "https://a.test/back.js"));
    }

    /// A document with no origin of its own to compare, or no document: every fetched script.
    #[test]
    fn a_document_without_an_origin_gets_no_detail_of_any_fetched_script() {
        assert!(decided(None, &[], "https://a.test/s.js"));
        assert!(decided(Some("about:blank"), &[], "https://a.test/s.js"));
        assert!(decided(Some("about:srcdoc"), &[], "https://a.test/s.js"));
        assert!(decided(Some("data:text/html,x"), &[], "https://a.test/s.js"));
        assert!(decided(Some("blob:null/0b9f"), &[], "https://a.test/s.js"));
        // A blob document made by a page of another origin than the script's.
        assert!(decided(Some("blob:https://b.test/0b9f"), &[], "https://a.test/s.js"));
    }
}
