//! Default security response headers. Fail-secure posture: on by default, so a
//! freshly-scaffolded app ships safe headers without having to remember them;
//! every value is overridable via `NESTRS_HTTP__*` (the framework-wide dual-path
//! config rule) or the pinned struct.
//!
//! # The family is [OWASP's list](https://owasp.org/www-project-secure-headers/),
//! and every member is answered
//!
//! A header the framework knows about and does not mention is the silence these
//! rules forbid, so each is either **emitted by default**, or **configurable
//! with its default argued here**. There is no third state.
//!
//! Emitted by default — every one of them safe for a JSON API that also serves
//! the framework's own HTML surfaces (the Swagger UI at `GET /api`, an OAuth
//! redirect landing back on this origin):
//!
//! - `X-Content-Type-Options: nosniff` — defeats MIME sniffing of a body.
//! - `X-Frame-Options: DENY` — no framing (clickjacking) by default.
//! - `Referrer-Policy: strict-origin-when-cross-origin` — a cross-origin
//!   request carries the origin and never the path or query. This one is not
//!   theoretical here: `nest-rs-social` performs OAuth redirects whose URLs
//!   carry `state`, and the Swagger UI is a real page whose outbound links would
//!   otherwise leak the API paths a reader was browsing. It matches what current
//!   browsers already default to, so it costs nothing and pins the behaviour for
//!   the ones that do not.
//! - `Cross-Origin-Opener-Policy: same-origin` — a document this server serves
//!   gets its own browsing-context group, so a cross-origin opener keeps no
//!   `window` reference to it.
//! - `Cross-Origin-Resource-Policy: same-origin` — a *no-cors* cross-origin load
//!   of a response from this server is blocked (Spectre-style side channels,
//!   and embedding an API response as a subresource). It does not touch CORS
//!   requests, which is why an API can carry it: the check applies to `no-cors`
//!   fetches, and a browser doing CORS never reaches it. An app that serves
//!   images or downloads meant to be embedded cross-origin sets
//!   `cross-origin` here.
//!
//! Configurable, off by default, each for a stated reason:
//!
//! - `Strict-Transport-Security` — carries a value by default but is applied
//!   **only when TLS is active**, since HSTS over plain HTTP is meaningless and
//!   a foot-gun on localhost.
//! - `Content-Security-Policy` — an API framework cannot know a page's sources.
//!   A default restrictive enough to be worth having (`default-src 'none'`)
//!   breaks the Swagger UI and every HTML surface an app serves; one loose
//!   enough to be safe to ship (`default-src 'self' 'unsafe-inline'`) states
//!   nothing a reader should rely on. The header a policy belongs in is here and
//!   settable both ways; the policy itself is the app's.
//! - `Cross-Origin-Embedder-Policy` — `require-corp` is a property a *page*
//!   opts into in order to become cross-origin isolated, and it breaks every
//!   cross-origin subresource that does not itself carry CORP. Turning that on
//!   for an app that never asked would break embeds to buy an isolation nothing
//!   here uses.
//! - `Permissions-Policy` — it governs powerful features (camera, geolocation,
//!   payment) in a *document*, so a default would be a guess about pages the
//!   framework does not serve, and a restrictive guess silently disables a
//!   feature the app's own front end asked for. An app that serves documents
//!   states its own.

use nest_rs_config::{ConfigError, ConfigService, Result};
use poem::http::{HeaderName, HeaderValue, header};

/// HSTS default: one year, include subdomains. No `preload` (that is an explicit
/// opt-in with real consequences — a developer who wants it sets it).
const DEFAULT_HSTS: &str = "max-age=31536000; includeSubDomains";
const DEFAULT_FRAME_OPTIONS: &str = "DENY";
/// The referrer policy OWASP recommends and current browsers already default
/// to: the origin travels cross-origin, the path and query never do.
const DEFAULT_REFERRER_POLICY: &str = "strict-origin-when-cross-origin";
const DEFAULT_COOP: &str = "same-origin";
const DEFAULT_CORP: &str = "same-origin";

// Each env key is spelled once, here, and read twice — by the overlay that
// resolves the value and by the table that validates and emits it.
const KEY_FRAME_OPTIONS: &str = "FRAME_OPTIONS";
const KEY_HSTS: &str = "HSTS";
const KEY_REFERRER_POLICY: &str = "REFERRER_POLICY";
const KEY_COOP: &str = "CROSS_ORIGIN_OPENER_POLICY";
const KEY_CORP: &str = "CROSS_ORIGIN_RESOURCE_POLICY";
const KEY_COEP: &str = "CROSS_ORIGIN_EMBEDDER_POLICY";
const KEY_PERMISSIONS_POLICY: &str = "PERMISSIONS_POLICY";
const KEY_CSP: &str = "CONTENT_SECURITY_POLICY";

/// Default-on security headers. Disable the whole set with `enabled = false`;
/// drop an individual header by setting its value to an empty string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityHeadersConfig {
    /// Master switch. `false` ⇒ emit no security headers at all.
    pub enabled: bool,
    /// Emit `X-Content-Type-Options: nosniff` (default `true`).
    pub content_type_options: bool,
    /// `X-Frame-Options` value; `None`/empty ⇒ header omitted. Default `DENY`.
    pub frame_options: Option<String>,
    /// `Strict-Transport-Security` value, emitted only under TLS; `None`/empty ⇒
    /// omitted. Default one year + `includeSubDomains`.
    pub hsts: Option<String>,
    /// `Referrer-Policy` value; `None`/empty ⇒ omitted. Default
    /// `strict-origin-when-cross-origin`.
    pub referrer_policy: Option<String>,
    /// `Cross-Origin-Opener-Policy` value; `None`/empty ⇒ omitted. Default
    /// `same-origin`.
    pub cross_origin_opener_policy: Option<String>,
    /// `Cross-Origin-Resource-Policy` value; `None`/empty ⇒ omitted. Default
    /// `same-origin`; set `cross-origin` on a server whose responses are meant
    /// to be embedded by other origins.
    pub cross_origin_resource_policy: Option<String>,
    /// `Cross-Origin-Embedder-Policy` value; `None` (the default) ⇒ omitted —
    /// see the module docs for why cross-origin isolation is not turned on for
    /// an app that did not ask for it.
    pub cross_origin_embedder_policy: Option<String>,
    /// `Permissions-Policy` value; `None` (the default) ⇒ omitted — the
    /// framework serves no document whose feature set it could speak for.
    pub permissions_policy: Option<String>,
    /// `Content-Security-Policy` value; `None` (the default) ⇒ omitted — the
    /// sources of an app's pages are the app's to declare.
    pub content_security_policy: Option<String>,
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            content_type_options: true,
            frame_options: Some(DEFAULT_FRAME_OPTIONS.to_owned()),
            hsts: Some(DEFAULT_HSTS.to_owned()),
            referrer_policy: Some(DEFAULT_REFERRER_POLICY.to_owned()),
            cross_origin_opener_policy: Some(DEFAULT_COOP.to_owned()),
            cross_origin_resource_policy: Some(DEFAULT_CORP.to_owned()),
            cross_origin_embedder_policy: None,
            permissions_policy: None,
            content_security_policy: None,
        }
    }
}

/// One value-carrying header: the env key it is read from, the response header
/// it renders into, whether TLS gates it, and the resolved value.
struct ValueHeader<'a> {
    key: &'static str,
    name: HeaderName,
    tls_only: bool,
    value: &'a Option<String>,
}

impl SecurityHeadersConfig {
    /// Read `NESTRS_HTTP__SECURITY_HEADERS` (master) plus one key per header,
    /// overlaid onto `base`. Absent vars keep `base`'s value (the safe defaults
    /// unless the call site pinned something else); an explicit empty string
    /// drops that one header.
    pub fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let resolved = Self {
            enabled: env.flag("SECURITY_HEADERS", base.enabled)?,
            content_type_options: env.flag("CONTENT_TYPE_OPTIONS", base.content_type_options)?,
            frame_options: override_header(env.get(KEY_FRAME_OPTIONS), base.frame_options),
            hsts: override_header(env.get(KEY_HSTS), base.hsts),
            referrer_policy: override_header(env.get(KEY_REFERRER_POLICY), base.referrer_policy),
            cross_origin_opener_policy: override_header(
                env.get(KEY_COOP),
                base.cross_origin_opener_policy,
            ),
            cross_origin_resource_policy: override_header(
                env.get(KEY_CORP),
                base.cross_origin_resource_policy,
            ),
            cross_origin_embedder_policy: override_header(
                env.get(KEY_COEP),
                base.cross_origin_embedder_policy,
            ),
            permissions_policy: override_header(
                env.get(KEY_PERMISSIONS_POLICY),
                base.permissions_policy,
            ),
            content_security_policy: override_header(
                env.get(KEY_CSP),
                base.content_security_policy,
            ),
        };
        // Reject a set-but-invalid header value at boot, naming the env var
        // (HTTP-S4) — otherwise the response layer silently drops it and a
        // stray char in `NESTRS_HTTP__HSTS` quietly removes HSTS in prod. One
        // walk over the table, so a header added to it is validated by
        // arriving rather than by someone remembering the second call site.
        for entry in resolved.value_headers() {
            validate_header_value(env, entry.key, entry.value)?;
        }
        Ok(resolved)
    }

    /// The value-carrying headers, in emission order. One table: the boot
    /// validation and the emitted list both read it, so a member cannot arrive
    /// validated but unemitted (or the reverse).
    ///
    /// The names come from the `http` crate's constants wherever it has one;
    /// the three `Cross-Origin-*` headers and `Permissions-Policy` are not in
    /// its table, so they are `from_static` literals here and nowhere else.
    fn value_headers(&self) -> [ValueHeader<'_>; 8] {
        [
            ValueHeader {
                key: KEY_FRAME_OPTIONS,
                name: header::X_FRAME_OPTIONS,
                tls_only: false,
                value: &self.frame_options,
            },
            ValueHeader {
                key: KEY_REFERRER_POLICY,
                name: header::REFERRER_POLICY,
                tls_only: false,
                value: &self.referrer_policy,
            },
            ValueHeader {
                key: KEY_COOP,
                name: HeaderName::from_static("cross-origin-opener-policy"),
                tls_only: false,
                value: &self.cross_origin_opener_policy,
            },
            ValueHeader {
                key: KEY_CORP,
                name: HeaderName::from_static("cross-origin-resource-policy"),
                tls_only: false,
                value: &self.cross_origin_resource_policy,
            },
            ValueHeader {
                key: KEY_COEP,
                name: HeaderName::from_static("cross-origin-embedder-policy"),
                tls_only: false,
                value: &self.cross_origin_embedder_policy,
            },
            ValueHeader {
                key: KEY_PERMISSIONS_POLICY,
                name: HeaderName::from_static("permissions-policy"),
                tls_only: false,
                value: &self.permissions_policy,
            },
            ValueHeader {
                key: KEY_CSP,
                name: header::CONTENT_SECURITY_POLICY,
                tls_only: false,
                value: &self.content_security_policy,
            },
            // HSTS last, and the one entry `tls_only` exists for.
            ValueHeader {
                key: KEY_HSTS,
                name: header::STRICT_TRANSPORT_SECURITY,
                tls_only: true,
                value: &self.hsts,
            },
        ]
    }

    /// The `(name, value)` headers to set, given whether TLS is active. HSTS is
    /// included only under TLS. Returns an empty list when disabled.
    pub fn headers(&self, tls_active: bool) -> Vec<(HeaderName, String)> {
        if !self.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.content_type_options {
            out.push((header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_owned()));
        }
        for entry in self.value_headers() {
            if entry.tls_only && !tls_active {
                continue;
            }
            if let Some(value) = non_empty(entry.value) {
                out.push((entry.name, value));
            }
        }
        out
    }
}

/// Boot-fatal check that a non-empty header value parses as an HTTP header
/// value, naming the offending env var so the misconfig is obvious (HTTP-S4).
fn validate_header_value(env: &ConfigService, key: &str, value: &Option<String>) -> Result<()> {
    if let Some(v) = non_empty(value) {
        HeaderValue::from_str(&v).map_err(|_| {
            ConfigError::parse(
                env.var_name(key),
                format!("`{v}` is not a valid HTTP header value"),
            )
        })?;
    }
    Ok(())
}

/// An env value present (even empty) overrides the default; absent keeps it.
fn override_header(env_value: Option<String>, default: Option<String>) -> Option<String> {
    match env_value {
        Some(v) if v.trim().is_empty() => None,
        Some(v) => Some(v),
        None => default,
    }
}

fn non_empty(value: &Option<String>) -> Option<String> {
    value
        .as_ref()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value_of(headers: &[(HeaderName, String)], name: &HeaderName) -> Option<String> {
        headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
    }

    #[test]
    fn defaults_are_on_with_safe_values() {
        let plain = SecurityHeadersConfig::default().headers(false);
        assert_eq!(
            value_of(&plain, &header::X_CONTENT_TYPE_OPTIONS).as_deref(),
            Some("nosniff"),
        );
        assert_eq!(
            value_of(&plain, &header::X_FRAME_OPTIONS).as_deref(),
            Some("DENY"),
        );
        assert!(
            value_of(&plain, &header::STRICT_TRANSPORT_SECURITY).is_none(),
            "HSTS must not be emitted over plain HTTP",
        );
    }

    // The three members added with a default: absent, each is a header nobody
    // would notice missing until an incident.
    #[test]
    fn the_cross_origin_and_referrer_defaults_are_emitted() {
        let plain = SecurityHeadersConfig::default().headers(false);
        assert_eq!(
            value_of(&plain, &header::REFERRER_POLICY).as_deref(),
            Some("strict-origin-when-cross-origin"),
        );
        assert_eq!(
            value_of(
                &plain,
                &HeaderName::from_static("cross-origin-opener-policy")
            )
            .as_deref(),
            Some("same-origin"),
        );
        assert_eq!(
            value_of(
                &plain,
                &HeaderName::from_static("cross-origin-resource-policy")
            )
            .as_deref(),
            Some("same-origin"),
        );
    }

    // The three members deliberately left off: configurable, and silent until
    // configured. A default here would be the framework guessing about a
    // document it does not serve.
    #[test]
    fn the_argued_off_members_are_absent_by_default_and_settable() {
        let plain = SecurityHeadersConfig::default().headers(false);
        for name in [
            HeaderName::from_static("cross-origin-embedder-policy"),
            HeaderName::from_static("permissions-policy"),
            header::CONTENT_SECURITY_POLICY,
        ] {
            assert!(
                value_of(&plain, &name).is_none(),
                "{name} carries no default",
            );
        }

        let pinned = SecurityHeadersConfig {
            cross_origin_embedder_policy: Some("require-corp".into()),
            permissions_policy: Some("geolocation=()".into()),
            content_security_policy: Some("default-src 'none'".into()),
            ..Default::default()
        }
        .headers(false);
        assert_eq!(
            value_of(
                &pinned,
                &HeaderName::from_static("cross-origin-embedder-policy")
            )
            .as_deref(),
            Some("require-corp"),
        );
        assert_eq!(
            value_of(&pinned, &HeaderName::from_static("permissions-policy")).as_deref(),
            Some("geolocation=()"),
        );
        assert_eq!(
            value_of(&pinned, &header::CONTENT_SECURITY_POLICY).as_deref(),
            Some("default-src 'none'"),
        );
    }

    // Every member is settable from the environment too — the dual-path rule,
    // asserted over the whole table rather than at whichever key was added last.
    #[test]
    fn every_value_header_is_settable_from_the_environment() {
        for key in [
            KEY_FRAME_OPTIONS,
            KEY_HSTS,
            KEY_REFERRER_POLICY,
            KEY_COOP,
            KEY_CORP,
            KEY_COEP,
            KEY_PERMISSIONS_POLICY,
            KEY_CSP,
        ] {
            let env = ConfigService::with_vars("http", [(key, "no-referrer")]);
            let loaded =
                SecurityHeadersConfig::from_env(&env, Default::default()).expect("value loads");
            let emitted = loaded.headers(true);
            let entry = loaded
                .value_headers()
                .into_iter()
                .find(|entry| entry.key == key)
                .map(|entry| entry.name)
                .expect("every env key names a header in the table");
            assert_eq!(
                value_of(&emitted, &entry).as_deref(),
                Some("no-referrer"),
                "{key} must reach the wire",
            );
        }
    }

    #[test]
    fn hsts_only_under_tls() {
        let d = SecurityHeadersConfig::default();
        assert!(
            value_of(&d.headers(true), &header::STRICT_TRANSPORT_SECURITY).is_some(),
            "HSTS must be emitted under TLS",
        );
    }

    #[test]
    fn disabled_emits_nothing() {
        let cfg = SecurityHeadersConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(cfg.headers(true).is_empty());
    }

    #[test]
    fn an_invalid_header_value_fails_boot_naming_the_var() {
        let cfg = ConfigService::with_vars("http", [(KEY_FRAME_OPTIONS, "bad\nvalue")]);
        let err = SecurityHeadersConfig::from_env(&cfg, Default::default()).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse { ref var, .. }
                if *var == nest_rs_config::var_name("http", KEY_FRAME_OPTIONS)),
            "expected a Parse error naming FRAME_OPTIONS, got {err:?}",
        );
    }

    // The same refusal, at the member added last — the validation walks the
    // table, so it binds every header rather than the two it was written for.
    #[test]
    fn an_invalid_value_on_a_newer_header_fails_boot_too() {
        let cfg =
            ConfigService::with_vars("http", [(KEY_CSP, "default-src 'none'\nX-Injected: y")]);
        let err = SecurityHeadersConfig::from_env(&cfg, Default::default()).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse { ref var, .. }
                if *var == nest_rs_config::var_name("http", KEY_CSP)),
            "expected a Parse error naming CONTENT_SECURITY_POLICY, got {err:?}",
        );
    }

    #[test]
    fn a_valid_override_still_loads() {
        let cfg = ConfigService::with_vars("http", [(KEY_FRAME_OPTIONS, "SAMEORIGIN")]);
        let loaded =
            SecurityHeadersConfig::from_env(&cfg, Default::default()).expect("valid value loads");
        assert_eq!(loaded.frame_options.as_deref(), Some("SAMEORIGIN"));
    }

    #[test]
    fn an_empty_override_drops_one_header() {
        let cfg = SecurityHeadersConfig {
            frame_options: override_header(Some(String::new()), Some("DENY".into())),
            ..Default::default()
        };
        let emitted = cfg.headers(false);
        assert!(value_of(&emitted, &header::X_FRAME_OPTIONS).is_none());
        assert!(
            value_of(&emitted, &header::X_CONTENT_TYPE_OPTIONS).is_some(),
            "dropping one header leaves the others",
        );
    }
}
