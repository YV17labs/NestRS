//! CORS settings for the HTTP transport, read from `<PREFIX>_HTTP__CORS_*` or
//! pinned as `HttpConfig.cors`, and checked at boot by the transport, which
//! answers the policy around every route.

use std::time::Duration;

use anyhow::Result;
use nest_rs_config::ConfigService;

/// Cross-Origin Resource Sharing policy. `origins` empty ⇒ no CORS layer
/// installed (the default). Lists are comma-separated in env vars.
///
/// The same `origins` decide which browser origins open a WebSocket gateway's
/// socket or a graphql-ws socket and call `/mcp`, which a browser never asks
/// CORS about; the CORS layer answers first and admits only what they list,
/// the server's own origin included.
#[derive(Clone, Debug, Default)]
pub struct HttpCors {
    /// Allowed origins; empty ⇒ no CORS layer installed. Also the browser
    /// origins the socket upgrades and `/mcp` admit.
    pub origins: Vec<String>,
    /// Allowed request methods (`Access-Control-Allow-Methods`).
    pub methods: Vec<String>,
    /// Allowed request headers (`Access-Control-Allow-Headers`).
    pub headers: Vec<String>,
    /// Response headers exposed to the browser (`Access-Control-Expose-Headers`),
    /// beside the ones the framework itself sends, which are always exposed.
    pub exposed_headers: Vec<String>,
    /// Whether to allow credentialed requests (`Access-Control-Allow-Credentials`).
    pub credentials: bool,
    /// Preflight cache lifetime (`Access-Control-Max-Age`); `None` omits it.
    pub max_age: Option<Duration>,
}

/// The wildcard, as the four lists spell it.
pub(crate) const WILDCARD: &str = "*";

/// The response headers the framework sends on a route's behalf. None is
/// CORS-safelisted, so a cross-origin client reads them only when exposed.
pub(crate) const FRAMEWORK_SENT: [&str; 3] = ["link", "location", "retry-after"];

/// Whether a list holds the wildcard, which admits every value.
pub(crate) fn holds_wildcard(values: &[String]) -> bool {
    values.iter().any(|value| value == WILDCARD)
}

/// The values a list admits one by one: none when it holds the wildcard, which
/// admits them all.
pub(crate) fn listed(values: &[String]) -> &[String] {
    if holds_wildcard(values) { &[] } else { values }
}

/// A list a `*` may appear in, paired with the response header it renders into.
type WildcardList<'a> = (&'a str, &'a str, &'a [String]);

impl HttpCors {
    /// Every list where `*` is a literal once the request's credentials mode is
    /// `include` — WHATWG Fetch, *CORS protocol*. `*` is a valid `tchar`, so
    /// nothing else refuses it: the header is emitted and every browser drops it.
    fn wildcard_lists(&self) -> [WildcardList<'_>; 4] {
        [
            ("origins", "Access-Control-Allow-Origin", &self.origins),
            ("headers", "Access-Control-Allow-Headers", &self.headers),
            ("methods", "Access-Control-Allow-Methods", &self.methods),
            (
                "exposed_headers",
                "Access-Control-Expose-Headers",
                &self.exposed_headers,
            ),
        ]
    }

    /// Refuse a `*` that credentials turn into a literal no browser honours.
    pub(crate) fn reject_wildcards_with_credentials(&self) -> Result<()> {
        if !self.credentials {
            return Ok(());
        }
        for (field, header, values) in self.wildcard_lists() {
            if holds_wildcard(values) {
                anyhow::bail!(
                    "invalid CORS config: `{WILDCARD}` in `{field}` with `credentials` — for a \
                     request whose credentials mode is `include`, WHATWG Fetch reads \
                     `{WILDCARD}` in `{header}` as that literal value rather than as a \
                     wildcard, so the response is emitted and no browser honours it; list the \
                     values explicitly or turn `credentials` off",
                );
            }
        }
        Ok(())
    }

    /// Overlay the `<PREFIX>_HTTP__CORS_*` keys onto `base` (the policy pinned in
    /// code, if any). Returns `Ok(None)` when neither the environment nor `base`
    /// supplies an origin — no origins means no CORS layer. Each sub-key
    /// overlays independently.
    pub fn from_env(
        env: &ConfigService,
        base: Option<Self>,
    ) -> nest_rs_config::Result<Option<Self>> {
        let base = base.unwrap_or_default();
        let origins = env.list("CORS_ORIGINS", base.origins)?;
        if origins.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            origins,
            methods: env.list("CORS_METHODS", base.methods)?,
            headers: env.list("CORS_HEADERS", base.headers)?,
            exposed_headers: env.list("CORS_EXPOSED", base.exposed_headers)?,
            credentials: env.flag("CORS_CREDENTIALS", base.credentials)?,
            max_age: env
                .parse::<u64>("CORS_MAX_AGE")?
                .map(Duration::from_secs)
                .or(base.max_age),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_env_returns_none_when_origins_unset() {
        let cfg =
            HttpCors::from_env(&ConfigService::with_vars("http", []), None).expect("no error");
        assert!(cfg.is_none(), "unset origins ⇒ CORS off");
    }

    #[test]
    fn from_env_reads_origins_methods_headers_when_set() {
        let service = ConfigService::with_vars(
            "http",
            [
                ("CORS_ORIGINS", "https://a.example,https://b.example"),
                ("CORS_METHODS", "GET,POST"),
                ("CORS_HEADERS", "content-type"),
            ],
        );
        let cfg = HttpCors::from_env(&service, None)
            .expect("no error")
            .expect("Some when origins set");
        assert_eq!(
            cfg.origins,
            vec!["https://a.example".to_string(), "https://b.example".into()]
        );
        assert_eq!(cfg.methods, vec!["GET".to_string(), "POST".into()]);
        assert_eq!(cfg.headers, vec!["content-type".to_string()]);
        assert!(!cfg.credentials, "off by default");
        assert!(cfg.max_age.is_none(), "off by default");
    }

    #[test]
    fn from_env_reads_credentials_flag_and_max_age() {
        let service = ConfigService::with_vars(
            "http",
            [
                ("CORS_ORIGINS", "*"),
                ("CORS_CREDENTIALS", "true"),
                ("CORS_MAX_AGE", "600"),
            ],
        );
        let cfg = HttpCors::from_env(&service, None)
            .expect("no error")
            .expect("Some");
        assert!(cfg.credentials);
        assert_eq!(cfg.max_age, Some(Duration::from_secs(600)));
    }
}
