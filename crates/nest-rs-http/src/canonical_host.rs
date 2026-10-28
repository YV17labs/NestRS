//! [`CanonicalHost`] — the host a deployment's canonical URI names.

use http::Uri;

/// The host a deployment names itself by in its canonical URI — the RFC 9728
/// resource identifier: provided by the module that reads that URI, and
/// admitted by an endpoint checking the `Host` header (the MCP allowlist), so
/// a deployment writes its host once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CanonicalHost(String);

impl CanonicalHost {
    /// The host `uri`'s authority names, without its port; `None` when it
    /// names none.
    pub fn of(uri: &str) -> Option<Self> {
        let host = uri.parse::<Uri>().ok()?.host()?.to_owned();
        (!host.is_empty()).then_some(Self(host))
    }

    /// The host, an IP literal in its brackets.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_is_read_without_its_port_path_or_query() {
        for (uri, host) in [
            ("https://mcp.example.com", "mcp.example.com"),
            (
                "https://mcp.example.com:8443/mcp?tenant=a",
                "mcp.example.com",
            ),
            ("http://localhost:3003", "localhost"),
            ("http://[::1]:3003", "[::1]"),
        ] {
            assert_eq!(
                CanonicalHost::of(uri).as_ref().map(CanonicalHost::as_str),
                Some(host),
                "{uri}"
            );
        }
    }

    #[test]
    fn a_uri_naming_no_host_has_none() {
        for uri in ["/mcp", "not a uri", ""] {
            assert_eq!(CanonicalHost::of(uri), None, "{uri}");
        }
    }
}
