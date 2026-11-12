//! What the framework reads off a configured endpoint: [`endpoint_shown`], a
//! URL as a sentence may show it, and whether its host is this machine's
//! loopback.

/// `url` as a sentence may show it: scheme, host, port and path — never
/// userinfo, query or fragment, where a credential travels; a value that does
/// not parse is shown as "an unparsable URL".
///
/// ```
/// use nest_rs_config::endpoint_shown;
///
/// assert_eq!(
///     endpoint_shown("https://user:s3cr3t@idp.example:8443/token?key=k#f"),
///     "https://idp.example:8443/token",
/// );
/// assert_eq!(endpoint_shown("http://user:s3cr3t@[::1"), "an unparsable URL");
/// ```
pub fn endpoint_shown(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return "an unparsable URL".to_owned();
    };
    // `mailto:user@host`: the whole address is the path.
    if parsed.cannot_be_a_base() {
        return format!("a {}: URL", parsed.scheme());
    }
    let (scheme, path) = (parsed.scheme(), parsed.path());
    let host = parsed.host_str().unwrap_or_default();
    match parsed.port() {
        Some(port) => format!("{scheme}://{host}:{port}{path}"),
        None => format!("{scheme}://{host}{path}"),
    }
}

/// Whether `host`, a URL's host, names this machine's loopback: `localhost`
/// in any case, or a loopback address, an IPv6 literal with or without its
/// brackets.
pub fn is_loopback(host: &str) -> bool {
    let host = host.strip_circumfix('[', ']').unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "s3cr3t";

    #[test]
    fn userinfo_query_and_fragment_never_appear() {
        for (url, shown) in [
            (
                "https://user:s3cr3t@idp.example/oauth/token?client_secret=s3cr3t#s3cr3t",
                "https://idp.example/oauth/token",
            ),
            ("http://s3cr3t@collector:4318", "http://collector:4318/"),
            (
                "https://[::1]:8443/v1/traces?s3cr3t",
                "https://[::1]:8443/v1/traces",
            ),
            (
                "  https://idp.example:443/jwks  ",
                "https://idp.example/jwks",
            ),
            ("redis://:s3cr3t@cache:6379/0", "redis://cache:6379/0"),
            (
                "file:///run/secrets/collector",
                "file:///run/secrets/collector",
            ),
        ] {
            assert_eq!(endpoint_shown(url), shown, "{url}");
            assert!(!endpoint_shown(url).contains(SECRET), "{url}");
        }
    }

    #[test]
    fn a_value_that_does_not_parse_is_not_echoed() {
        for url in [
            "https://u:s3cr3t@[::1",
            "s3cr3t",
            "https://u:s3cr3t@idp:99999/x",
            "",
        ] {
            assert_eq!(endpoint_shown(url), "an unparsable URL", "{url}");
        }
    }

    #[test]
    fn an_address_with_no_hierarchy_is_named_by_its_scheme_alone() {
        assert_eq!(endpoint_shown("mailto:s3cr3t@example.com"), "a mailto: URL");
    }

    #[test]
    fn loopback_is_localhost_or_a_loopback_address_on_every_spelling() {
        for host in [
            "localhost",
            "LOCALHOST",
            "127.0.0.1",
            "127.3.2.1",
            "::1",
            "[::1]",
            "[0:0:0:0:0:0:0:1]",
        ] {
            assert!(is_loopback(host), "{host}");
        }
        for host in [
            "localhost.example",
            "collector.internal",
            "10.0.0.7",
            "2001:db8::1",
            "[2001:db8::1]",
            "[::1",
            "",
        ] {
            assert!(!is_loopback(host), "{host}");
        }
    }
}
