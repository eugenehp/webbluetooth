//! What counts as "the site", and whether it may ask for a radio.
//!
//! Shared by the commands and the toolbar so that the padlock in the address
//! bar and the answer `requestDevice` gives can never disagree.

/// The origin of a URL, as `https://example.com`.
///
/// Not `Url::origin().ascii_serialization()` on its own. That returns an
/// *opaque* origin for any scheme the URL Standard does not call special, and
/// `tauri:` — the scheme this browser serves its own pages on — is not
/// special. Taking that answer at face value made every command on the start
/// page refuse itself with "not available on an opaque origin", which is
/// exactly the failure this function exists to avoid.
///
/// A URL with no host still has no origin worth remembering a permission
/// against: `data:`, `blob:` and `about:blank` return `None`.
pub fn of(url: &url::Url) -> Option<String> {
    if let url::Origin::Tuple(..) = url.origin() {
        return Some(url.origin().ascii_serialization());
    }
    let host = url.host_str()?;
    let scheme = url.scheme();
    Some(match url.port() {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Whether Web Bluetooth is offered here at all.
///
/// The Secure Contexts rule, as far as it goes in a browser this size: TLS,
/// loopback, or a page this application shipped.
///
/// `file:` is deliberately absent. It is not potentially trustworthy, no
/// browser exposes Web Bluetooth there, and a downloaded HTML file is exactly
/// the thing that should not reach the radio. Serve local test pages over
/// `http://localhost` instead.
pub fn is_secure(url: &url::Url) -> bool {
    match url.scheme() {
        "https" | "wss" => true,
        // The browser's own bundled pages. Web content cannot navigate into
        // this scheme, and on Windows the same pages arrive as
        // `http://tauri.localhost`, which the loopback arm below covers.
        "tauri" => true,
        "http" | "ws" => url.host_str().is_some_and(is_loopback),
        _ => false,
    }
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host == "127.0.0.1"
        || host == "[::1]"
        || host == "::1"
        || host.ends_with(".localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).expect("test URL")
    }

    #[test]
    fn the_browsers_own_scheme_has_an_origin() {
        // The regression this module exists for: `Url::origin()` calls this
        // opaque, and every command keyed on it refused the start page.
        assert_eq!(
            of(&url("tauri://localhost/home.html")).as_deref(),
            Some("tauri://localhost")
        );
        assert!(is_secure(&url("tauri://localhost/home.html")));
    }

    #[test]
    fn ordinary_origins_are_unchanged() {
        assert_eq!(
            of(&url("https://example.com/a/b?c")).as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            of(&url("https://example.com:8443/")).as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(
            of(&url("http://a.example.com/")).as_deref(),
            Some("http://a.example.com")
        );
    }

    #[test]
    fn hostless_urls_have_no_origin() {
        assert_eq!(of(&url("data:text/html,hi")), None);
        assert_eq!(of(&url("about:blank")), None);
    }

    #[test]
    fn secure_contexts_only() {
        assert!(is_secure(&url("https://example.com")));
        assert!(is_secure(&url("http://localhost:8000")));
        assert!(is_secure(&url("http://127.0.0.1:8000")));
        assert!(is_secure(&url("http://dev.localhost/")));
        assert!(!is_secure(&url("http://example.com")));
        assert!(!is_secure(&url("file:///tmp/page.html")));
    }
}
