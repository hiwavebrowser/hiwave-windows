//! # Security Tests
//!
//! Tests for the security system including origin model, CSP, CORS, etc.

use rustkit_net::security::{
    check_mixed_content, ContentSecurityPolicy, CookieAttributes, CorsChecker, CorsResult,
    CspDirective, CspSource, MixedContentResult, MixedContentType, Origin, ReferrerPolicy,
    SameSite, SandboxFlags, SecurityContext,
};
use url::Url;

// ==================== Origin Tests ====================

/// Test origin extraction from URLs.
#[test]
fn test_origin_from_https_url() {
    let url = Url::parse("https://example.com:443/path?query=1").unwrap();
    let origin = Origin::from_url(&url);

    match origin {
        Origin::Tuple { scheme, host, port } => {
            assert_eq!(scheme, "https");
            assert_eq!(host, "example.com");
            assert_eq!(port, Some(443));
        }
        _ => panic!("Expected tuple origin"),
    }
}

/// Test origin with non-standard port.
#[test]
fn test_origin_with_port() {
    let url = Url::parse("https://example.com:8443/").unwrap();
    let origin = Origin::from_url(&url);

    match origin {
        Origin::Tuple { scheme, host, port } => {
            assert_eq!(scheme, "https");
            assert_eq!(host, "example.com");
            assert_eq!(port, Some(8443));
        }
        _ => panic!("Expected tuple origin"),
    }
}

/// Test same-origin check.
#[test]
fn test_same_origin() {
    let url1 = Url::parse("https://example.com/page1").unwrap();
    let url2 = Url::parse("https://example.com/page2").unwrap();

    let origin1 = Origin::from_url(&url1);
    let origin2 = Origin::from_url(&url2);

    assert!(origin1.same_origin(&origin2));
}

/// Test cross-origin - different host.
#[test]
fn test_cross_origin_different_host() {
    let url1 = Url::parse("https://example.com/").unwrap();
    let url2 = Url::parse("https://other.com/").unwrap();

    let origin1 = Origin::from_url(&url1);
    let origin2 = Origin::from_url(&url2);

    assert!(!origin1.same_origin(&origin2));
}

/// Test cross-origin - different scheme.
#[test]
fn test_cross_origin_different_scheme() {
    let url1 = Url::parse("https://example.com/").unwrap();
    let url2 = Url::parse("http://example.com/").unwrap();

    let origin1 = Origin::from_url(&url1);
    let origin2 = Origin::from_url(&url2);

    assert!(!origin1.same_origin(&origin2));
}

/// Test cross-origin - different port.
#[test]
fn test_cross_origin_different_port() {
    let url1 = Url::parse("https://example.com:443/").unwrap();
    let url2 = Url::parse("https://example.com:8443/").unwrap();

    let origin1 = Origin::from_url(&url1);
    let origin2 = Origin::from_url(&url2);

    assert!(!origin1.same_origin(&origin2));
}

/// Test opaque origins.
#[test]
fn test_opaque_origins() {
    // data: URLs have opaque origins
    let data_url = Url::parse("data:text/html,<h1>Hi</h1>").unwrap();
    let origin = Origin::from_url(&data_url);
    assert!(origin.is_opaque());

    // file: URLs have opaque origins
    let file_url = Url::parse("file:///path/to/file.html").unwrap();
    let origin = Origin::from_url(&file_url);
    assert!(origin.is_opaque());

    // Opaque origins are never same-origin
    assert!(!origin.same_origin(&origin));
}

/// Test secure origin detection.
#[test]
fn test_secure_origin() {
    // HTTPS is secure
    let https = Url::parse("https://example.com/").unwrap();
    assert!(Origin::from_url(&https).is_secure());

    // HTTP is not secure
    let http = Url::parse("http://example.com/").unwrap();
    assert!(!Origin::from_url(&http).is_secure());

    // localhost is secure (even over HTTP)
    let localhost = Url::parse("http://localhost:3000/").unwrap();
    assert!(Origin::from_url(&localhost).is_secure());

    let localhost_ip = Url::parse("http://127.0.0.1:8080/").unwrap();
    assert!(Origin::from_url(&localhost_ip).is_secure());
}

/// Test origin serialization.
#[test]
fn test_origin_serialize() {
    let url = Url::parse("https://example.com/path").unwrap();
    let origin = Origin::from_url(&url);
    assert_eq!(origin.serialize(), "https://example.com");

    let url_port = Url::parse("https://example.com:8443/path").unwrap();
    let origin_port = Origin::from_url(&url_port);
    assert_eq!(origin_port.serialize(), "https://example.com:8443");
}

// ==================== CSP Tests ====================

/// Test CSP parsing.
#[test]
fn test_csp_parse_basic() {
    let csp = ContentSecurityPolicy::parse("default-src 'self'").unwrap();
    assert!(csp.directives.contains_key(&CspDirective::DefaultSrc));
}

/// Test CSP with multiple directives.
#[test]
fn test_csp_parse_multiple() {
    let csp = ContentSecurityPolicy::parse(
        "default-src 'self'; script-src 'self' https://cdn.example.com; img-src *"
    ).unwrap();

    assert!(csp.directives.contains_key(&CspDirective::DefaultSrc));
    assert!(csp.directives.contains_key(&CspDirective::ScriptSrc));
    assert!(csp.directives.contains_key(&CspDirective::ImgSrc));
}

/// Test CSP source parsing.
#[test]
fn test_csp_source_self() {
    let source = CspSource::parse("'self'").unwrap();
    assert_eq!(source, CspSource::Self_);
}

/// Test CSP source parsing - none.
#[test]
fn test_csp_source_none() {
    let source = CspSource::parse("'none'").unwrap();
    assert_eq!(source, CspSource::None);
}

/// Test CSP source parsing - unsafe-inline.
#[test]
fn test_csp_source_unsafe_inline() {
    let source = CspSource::parse("'unsafe-inline'").unwrap();
    assert_eq!(source, CspSource::UnsafeInline);
}

/// Test CSP source parsing - unsafe-eval.
#[test]
fn test_csp_source_unsafe_eval() {
    let source = CspSource::parse("'unsafe-eval'").unwrap();
    assert_eq!(source, CspSource::UnsafeEval);
}

/// Test CSP source parsing - nonce.
#[test]
fn test_csp_source_nonce() {
    let source = CspSource::parse("'nonce-abc123XYZ='").unwrap();
    if let CspSource::Nonce(n) = source {
        assert_eq!(n, "abc123XYZ=");
    } else {
        panic!("Expected nonce source");
    }
}

/// Test CSP source parsing - hash.
#[test]
fn test_csp_source_hash() {
    let source = CspSource::parse("'sha256-abcdef123456'").unwrap();
    if let CspSource::Hash(algo, hash) = source {
        assert_eq!(algo, rustkit_net::security::HashAlgorithm::Sha256);
        assert_eq!(hash, "abcdef123456");
    } else {
        panic!("Expected hash source");
    }
}

/// Test CSP allows eval check.
#[test]
fn test_csp_allows_eval() {
    let csp_no_eval = ContentSecurityPolicy::parse("script-src 'self'").unwrap();
    assert!(!csp_no_eval.allows_eval());

    let csp_eval = ContentSecurityPolicy::parse("script-src 'self' 'unsafe-eval'").unwrap();
    assert!(csp_eval.allows_eval());
}

/// Test CSP allows inline script.
#[test]
fn test_csp_allows_inline_script() {
    let csp = ContentSecurityPolicy::parse("script-src 'self'").unwrap();
    assert!(!csp.allows_script(None, true, None, None));

    let csp_inline = ContentSecurityPolicy::parse("script-src 'self' 'unsafe-inline'").unwrap();
    assert!(csp_inline.allows_script(None, true, None, None));
}

/// Test CSP allows script with nonce.
#[test]
fn test_csp_allows_script_nonce() {
    let csp = ContentSecurityPolicy::parse("script-src 'nonce-abc123'").unwrap();
    
    // Correct nonce - allowed
    assert!(csp.allows_script(None, true, Some("abc123"), None));
    
    // Wrong nonce - denied
    assert!(!csp.allows_script(None, true, Some("wrong"), None));
}

// ==================== CORS Tests ====================

/// Test CORS simple request detection.
#[test]
fn test_cors_simple_request() {
    // GET is simple
    assert!(CorsChecker::is_simple_request("GET", &[]));
    
    // HEAD is simple
    assert!(CorsChecker::is_simple_request("HEAD", &[]));
    
    // POST with text/plain is simple
    assert!(CorsChecker::is_simple_request("POST", &[("content-type", "text/plain")]));
    
    // PUT is not simple
    assert!(!CorsChecker::is_simple_request("PUT", &[]));
    
    // DELETE is not simple
    assert!(!CorsChecker::is_simple_request("DELETE", &[]));
    
    // POST with application/json is not simple
    assert!(!CorsChecker::is_simple_request("POST", &[("content-type", "application/json")]));
}

/// Test CORS response check - wildcard.
#[test]
fn test_cors_wildcard() {
    let checker = CorsChecker::new();
    
    let result = checker.check_response("https://example.com", Some("*"), None, false);
    assert!(matches!(result, CorsResult::Allowed));
}

/// Test CORS response check - wildcard with credentials denied.
#[test]
fn test_cors_wildcard_with_credentials() {
    let checker = CorsChecker::new();
    
    let result = checker.check_response("https://example.com", Some("*"), None, true);
    assert!(matches!(result, CorsResult::Denied(_)));
}

/// Test CORS response check - exact match.
#[test]
fn test_cors_exact_match() {
    let checker = CorsChecker::new();
    
    let result = checker.check_response(
        "https://example.com",
        Some("https://example.com"),
        None,
        false
    );
    assert!(matches!(result, CorsResult::Allowed));
}

/// Test CORS response check - no match.
#[test]
fn test_cors_no_match() {
    let checker = CorsChecker::new();
    
    let result = checker.check_response(
        "https://example.com",
        Some("https://other.com"),
        None,
        false
    );
    assert!(matches!(result, CorsResult::Denied(_)));
}

/// Test CORS response check - missing header.
#[test]
fn test_cors_missing_header() {
    let checker = CorsChecker::new();
    
    let result = checker.check_response("https://example.com", None, None, false);
    assert!(matches!(result, CorsResult::Denied(_)));
}

/// Test CORS with credentials.
#[test]
fn test_cors_with_credentials() {
    let checker = CorsChecker::new();
    
    // With credentials and allow-credentials: true
    let result = checker.check_response(
        "https://example.com",
        Some("https://example.com"),
        Some("true"),
        true
    );
    assert!(matches!(result, CorsResult::Allowed));
    
    // With credentials but no allow-credentials header
    let result = checker.check_response(
        "https://example.com",
        Some("https://example.com"),
        None,
        true
    );
    assert!(matches!(result, CorsResult::Denied(_)));
}

// ==================== Referrer Policy Tests ====================

/// Test referrer policy - no-referrer.
#[test]
fn test_referrer_policy_no_referrer() {
    let referrer = Url::parse("https://example.com/page?secret=123").unwrap();
    let target = Url::parse("https://other.com/").unwrap();
    
    let policy = ReferrerPolicy::NoReferrer;
    assert_eq!(policy.compute_referrer(&referrer, &target), None);
}

/// Test referrer policy - origin.
#[test]
fn test_referrer_policy_origin() {
    let referrer = Url::parse("https://example.com/page?secret=123").unwrap();
    let target = Url::parse("https://other.com/").unwrap();
    
    let policy = ReferrerPolicy::Origin;
    assert_eq!(
        policy.compute_referrer(&referrer, &target),
        Some("https://example.com/".to_string())
    );
}

/// Test referrer policy - same-origin.
#[test]
fn test_referrer_policy_same_origin() {
    let referrer = Url::parse("https://example.com/page").unwrap();
    let target_same = Url::parse("https://example.com/other").unwrap();
    let target_cross = Url::parse("https://other.com/").unwrap();
    
    let policy = ReferrerPolicy::SameOrigin;
    
    // Same-origin - full referrer
    assert!(policy.compute_referrer(&referrer, &target_same).is_some());
    
    // Cross-origin - no referrer
    assert_eq!(policy.compute_referrer(&referrer, &target_cross), None);
}

/// Test referrer policy - downgrade protection.
#[test]
fn test_referrer_policy_downgrade() {
    let referrer = Url::parse("https://example.com/page").unwrap();
    let target_https = Url::parse("https://other.com/").unwrap();
    let target_http = Url::parse("http://other.com/").unwrap();
    
    let policy = ReferrerPolicy::StrictOrigin;
    
    // HTTPS to HTTPS - origin sent
    assert!(policy.compute_referrer(&referrer, &target_https).is_some());
    
    // HTTPS to HTTP (downgrade) - no referrer
    assert_eq!(policy.compute_referrer(&referrer, &target_http), None);
}

// ==================== Cookie Tests ====================

/// Test cookie SameSite=Strict.
#[test]
fn test_cookie_same_site_strict() {
    let url = Url::parse("https://example.com/page").unwrap();
    
    let cookie = CookieAttributes {
        name: "session".into(),
        value: "abc123".into(),
        same_site: SameSite::Strict,
        secure: true,
        ..Default::default()
    };
    
    // Same-site - allowed
    assert!(cookie.should_send(&url, true, false));
    
    // Cross-site - denied
    assert!(!cookie.should_send(&url, false, false));
    
    // Cross-site top-level navigation - still denied for Strict
    assert!(!cookie.should_send(&url, false, true));
}

/// Test cookie SameSite=Lax.
#[test]
fn test_cookie_same_site_lax() {
    let url = Url::parse("https://example.com/page").unwrap();
    
    let cookie = CookieAttributes {
        name: "session".into(),
        value: "abc123".into(),
        same_site: SameSite::Lax,
        secure: true,
        ..Default::default()
    };
    
    // Same-site - allowed
    assert!(cookie.should_send(&url, true, false));
    
    // Cross-site subresource - denied
    assert!(!cookie.should_send(&url, false, false));
    
    // Cross-site top-level navigation - allowed for Lax
    assert!(cookie.should_send(&url, false, true));
}

/// Test cookie Secure flag.
#[test]
fn test_cookie_secure() {
    let https_url = Url::parse("https://example.com/").unwrap();
    let http_url = Url::parse("http://example.com/").unwrap();
    
    let secure_cookie = CookieAttributes {
        name: "secure".into(),
        value: "value".into(),
        secure: true,
        ..Default::default()
    };
    
    // HTTPS - allowed
    assert!(secure_cookie.should_send(&https_url, true, false));
    
    // HTTP - denied
    assert!(!secure_cookie.should_send(&http_url, true, false));
}

/// Test cookie SameSite=None requires Secure.
#[test]
fn test_cookie_same_site_none_requires_secure() {
    let url = Url::parse("https://example.com/").unwrap();
    
    // SameSite=None with Secure - allowed
    let valid = CookieAttributes {
        name: "tracking".into(),
        value: "value".into(),
        same_site: SameSite::None,
        secure: true,
        ..Default::default()
    };
    assert!(valid.should_send(&url, false, false));
    
    // SameSite=None without Secure - denied
    let invalid = CookieAttributes {
        name: "tracking".into(),
        value: "value".into(),
        same_site: SameSite::None,
        secure: false,
        ..Default::default()
    };
    assert!(!invalid.should_send(&url, false, false));
}

// ==================== Mixed Content Tests ====================

/// Test mixed content - HTTPS resource on HTTPS page.
#[test]
fn test_mixed_content_https_on_https() {
    let page = Url::parse("https://example.com/").unwrap();
    let resource = Url::parse("https://cdn.example.com/script.js").unwrap();
    
    let result = check_mixed_content(&page, &resource, MixedContentType::Script);
    assert_eq!(result, MixedContentResult::Allowed);
}

/// Test mixed content - HTTP script on HTTPS page (blockable).
#[test]
fn test_mixed_content_http_script() {
    let page = Url::parse("https://example.com/").unwrap();
    let resource = Url::parse("http://example.com/script.js").unwrap();
    
    let result = check_mixed_content(&page, &resource, MixedContentType::Script);
    assert_eq!(result, MixedContentResult::Blockable);
}

/// Test mixed content - HTTP image on HTTPS page (optionally blockable).
#[test]
fn test_mixed_content_http_image() {
    let page = Url::parse("https://example.com/").unwrap();
    let resource = Url::parse("http://example.com/image.png").unwrap();
    
    let result = check_mixed_content(&page, &resource, MixedContentType::Image);
    assert_eq!(result, MixedContentResult::OptionallyBlockable);
}

/// Test mixed content - data URL is allowed.
#[test]
fn test_mixed_content_data_url() {
    let page = Url::parse("https://example.com/").unwrap();
    let resource = Url::parse("data:image/png;base64,abc123").unwrap();
    
    let result = check_mixed_content(&page, &resource, MixedContentType::Image);
    assert_eq!(result, MixedContentResult::Allowed);
}

/// Test mixed content - HTTP page doesn't trigger.
#[test]
fn test_mixed_content_http_page() {
    let page = Url::parse("http://example.com/").unwrap();
    let resource = Url::parse("http://example.com/script.js").unwrap();
    
    let result = check_mixed_content(&page, &resource, MixedContentType::Script);
    assert_eq!(result, MixedContentResult::Allowed);
}

// ==================== Sandbox Tests ====================

/// Test sandbox flag parsing.
#[test]
fn test_sandbox_parse() {
    let flags = SandboxFlags::parse("allow-scripts allow-forms allow-popups");
    
    assert!(flags.allow_scripts);
    assert!(flags.allow_forms);
    assert!(flags.allow_popups);
    assert!(!flags.allow_same_origin);
    assert!(!flags.allow_top_navigation);
}

/// Test empty sandbox (most restrictive).
#[test]
fn test_sandbox_empty() {
    let flags = SandboxFlags::parse("");
    
    assert!(!flags.allow_scripts);
    assert!(!flags.allow_forms);
    assert!(!flags.allow_popups);
    assert!(!flags.allow_same_origin);
}

// ==================== Security Context Tests ====================

/// Test security context creation.
#[test]
fn test_security_context() {
    let url = Url::parse("https://example.com/page").unwrap();
    let ctx = SecurityContext::from_url(&url);
    
    assert!(ctx.is_secure_context);
    assert!(!ctx.sandboxed);
}

/// Test security context same-origin check.
#[test]
fn test_security_context_same_origin() {
    let url = Url::parse("https://example.com/page").unwrap();
    let ctx = SecurityContext::from_url(&url);
    
    assert!(ctx.is_same_origin(&Url::parse("https://example.com/other").unwrap()));
    assert!(!ctx.is_same_origin(&Url::parse("https://other.com/").unwrap()));
}

/// Test security context script permission.
#[test]
fn test_security_context_allows_script() {
    let url = Url::parse("https://example.com/").unwrap();
    let mut ctx = SecurityContext::from_url(&url);
    
    // Without CSP, scripts allowed
    assert!(ctx.allows_script(None, false, None));
    
    // With restrictive CSP
    ctx.csp = Some(ContentSecurityPolicy::parse("script-src 'none'").unwrap());
    assert!(!ctx.allows_script(None, false, None));
}

