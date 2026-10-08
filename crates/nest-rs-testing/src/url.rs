//! The URL a suite reads, with one RFC 3986 component replaced — a test's own
//! database, its own user, or the address of a proxy it put in front of the
//! server — and everything else the deployment set kept: the scheme, the
//! credentials, the query a TLS or protocol setting rides in, the fragment.

use std::fmt::Display;

/// `url` on `database`: its path replaced by `/<database>`, percent-encoded as
/// one segment, and every other component kept. The path is what follows the
/// authority — introduced by `//`, ended by the first `/`, `?` or `#` (RFC 3986
/// §3.2) — up to the first `?` or `#`.
///
/// ```
/// use nest_rs_testing::url_on;
///
/// assert_eq!(url_on("redis://cache:6379/1?protocol=resp3", 4), "redis://cache:6379/4?protocol=resp3");
/// assert_eq!(url_on("postgres://app:pw@db:5432", "app_test"), "postgres://app:pw@db:5432/app_test");
/// ```
pub fn url_on(url: &str, database: impl Display) -> String {
    let parts = Parts::of(url);
    format!(
        "{}/{}{}",
        &url[..parts.path],
        encoded(&database.to_string()),
        &url[parts.after_path..]
    )
}

/// `url` as `user` with `password`: any credentials it carried replaced, both
/// percent-encoded, and every other component kept. The userinfo is the
/// authority up to its last `@` (RFC 3986 §3.2.1).
///
/// ```
/// use nest_rs_testing::url_as;
///
/// assert_eq!(url_as("redis://default:old@cache:6379/2", "app", "s3cret"), "redis://app:s3cret@cache:6379/2");
/// ```
pub fn url_as(url: &str, user: &str, password: &str) -> String {
    let parts = Parts::of(url);
    format!(
        "{}{}:{}@{}",
        &url[..parts.authority],
        encoded(user),
        encoded(password),
        &url[parts.host..]
    )
}

/// `url` at `address`: its host and port replaced by the `host:port` a proxy in
/// front of the suite's server listens on, and every other component kept. The
/// host is the authority past its userinfo (RFC 3986 §3.2.2), the port with it.
///
/// ```
/// use nest_rs_testing::url_at;
///
/// let proxy: std::net::SocketAddr = "127.0.0.1:40123".parse()?;
/// assert_eq!(url_at("redis://app:pw@cache:6379/2?protocol=resp3", proxy), "redis://app:pw@127.0.0.1:40123/2?protocol=resp3");
/// # Ok::<(), std::net::AddrParseError>(())
/// ```
pub fn url_at(url: &str, address: impl Display) -> String {
    let parts = Parts::of(url);
    format!("{}{address}{}", &url[..parts.host], &url[parts.path..])
}

/// Where each component `url_on`, `url_as` and `url_at` replace starts, as
/// byte offsets into the URL.
struct Parts {
    authority: usize,
    host: usize,
    path: usize,
    after_path: usize,
}

impl Parts {
    fn of(url: &str) -> Self {
        // The URL may carry a password, so the refusal never quotes it.
        let authority = url
            .find("://")
            .map(|at| at + 3)
            .unwrap_or_else(|| panic!("a URL with an authority (`scheme://host`) is expected"));
        let path = url[authority..]
            .find(['/', '?', '#'])
            .map_or(url.len(), |at| authority + at);
        let host = url[authority..path]
            .rfind('@')
            .map_or(authority, |at| authority + at + 1);
        let after_path = url[path..]
            .find(['?', '#'])
            .map_or(url.len(), |at| path + at);
        Self {
            authority,
            host,
            path,
            after_path,
        }
    }
}

/// `component` with every byte but RFC 3986's unreserved ones (§2.3)
/// percent-encoded, so it holds whatever it is given and parses back to it.
fn encoded(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for byte in component.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_replaces_the_path_and_keeps_every_other_component() {
        for (url, on) in [
            ("redis://redis:6379", "redis://redis:6379/9"),
            ("redis://redis:6379/", "redis://redis:6379/9"),
            ("redis://redis:6379/1", "redis://redis:6379/9"),
            (
                "redis://redis:6379?protocol=resp3",
                "redis://redis:6379/9?protocol=resp3",
            ),
            (
                "rediss://:secret@cache:6380/1/?protocol=resp3",
                "rediss://:secret@cache:6380/9?protocol=resp3",
            ),
            (
                "redis://redis:6379/1#primary",
                "redis://redis:6379/9#primary",
            ),
            (
                "postgres://u:p@host:5432/postgres",
                "postgres://u:p@host:5432/9",
            ),
            // No path: the last `/` is the second of `//`.
            ("postgres://host:5432", "postgres://host:5432/9"),
            (
                "postgres://host/postgres?sslmode=require",
                "postgres://host/9?sslmode=require",
            ),
        ] {
            assert_eq!(url_on(url, 9), on, "{url}");
        }
    }

    #[test]
    fn a_database_name_is_one_segment() {
        assert_eq!(
            url_on("postgres://host/app", "a/b c"),
            "postgres://host/a%2Fb%20c"
        );
    }

    #[test]
    fn a_user_replaces_the_credentials_and_keeps_every_other_component() {
        for (url, user, password, the_user) in [
            (
                "redis://redis:6379",
                "app",
                "pw",
                "redis://app:pw@redis:6379",
            ),
            ("redis://redis:6379", "", "pw", "redis://:pw@redis:6379"),
            (
                "rediss://:secret@cache:6380/1?protocol=resp3",
                "app",
                "pw",
                "rediss://app:pw@cache:6380/1?protocol=resp3",
            ),
            (
                "redis://default:secret@redis:6379",
                "app",
                "pw",
                "redis://app:pw@redis:6379",
            ),
            // An `@` past the authority is not userinfo.
            (
                "redis://redis:6379/1#replica@east",
                "app",
                "pw",
                "redis://app:pw@redis:6379/1#replica@east",
            ),
        ] {
            assert_eq!(url_as(url, user, password), the_user, "{url}");
        }
    }

    #[test]
    fn credentials_are_percent_encoded() {
        assert_eq!(
            url_as("redis://redis:6379", "a user", "p@ss:w/rd"),
            "redis://a%20user:p%40ss%3Aw%2Frd@redis:6379"
        );
    }

    #[test]
    fn an_address_replaces_the_host_and_port_and_keeps_every_other_component() {
        let proxy: std::net::SocketAddr = "127.0.0.1:40123".parse().expect("an address");
        for (url, at) in [
            ("redis://redis:6379", "redis://127.0.0.1:40123"),
            ("redis://redis", "redis://127.0.0.1:40123"),
            ("redis://redis:6379/", "redis://127.0.0.1:40123/"),
            (
                "redis://app:p%40ss@redis:6379/1?protocol=resp3#primary",
                "redis://app:p%40ss@127.0.0.1:40123/1?protocol=resp3#primary",
            ),
            (
                "redis://:secret@[::1]:6379?protocol=resp3",
                "redis://:secret@127.0.0.1:40123?protocol=resp3",
            ),
            // An `@` past the authority is not userinfo.
            (
                "redis://redis:6379/1#replica@east",
                "redis://127.0.0.1:40123/1#replica@east",
            ),
        ] {
            assert_eq!(url_at(url, proxy), at, "{url}");
        }
    }

    #[test]
    fn an_ipv6_address_is_written_in_brackets() {
        let proxy: std::net::SocketAddr = "[::1]:40123".parse().expect("an address");
        assert_eq!(
            url_at("redis://redis:6379/2", proxy),
            "redis://[::1]:40123/2"
        );
    }

    #[test]
    #[should_panic(expected = "a URL with an authority")]
    fn a_url_without_an_authority_is_refused_without_quoting_it() {
        let _ = url_as("redis:secret", "app", "pw");
    }
}
