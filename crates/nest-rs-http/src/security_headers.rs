//! Default security response headers. Fail-secure posture: on by default, so a
//! freshly-scaffolded app ships safe headers without having to remember them;
//! every value is overridable via `NESTRS_HTTP__*` (the framework-wide dual-path
//! config rule) or the pinned struct.
//!
//! - `X-Content-Type-Options: nosniff` — defeats MIME sniffing of a body.
//! - `X-Frame-Options: DENY` — no framing (clickjacking) by default.
//! - `Strict-Transport-Security` — applied **only when TLS is active**, since
//!   HSTS over plain HTTP is meaningless and a foot-gun on localhost.

use nest_rs_config::{ConfigService, Result};

/// HSTS default: one year, include subdomains. No `preload` (that is an explicit
/// opt-in with real consequences — a developer who wants it sets it).
const DEFAULT_HSTS: &str = "max-age=31536000; includeSubDomains";
const DEFAULT_FRAME_OPTIONS: &str = "DENY";

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
}

impl Default for SecurityHeadersConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            content_type_options: true,
            frame_options: Some(DEFAULT_FRAME_OPTIONS.to_owned()),
            hsts: Some(DEFAULT_HSTS.to_owned()),
        }
    }
}

impl SecurityHeadersConfig {
    /// Read `NESTRS_HTTP__SECURITY_HEADERS` (master), `__FRAME_OPTIONS`, `__HSTS`,
    /// `__CONTENT_TYPE_OPTIONS`. Absent vars keep the safe defaults; an explicit
    /// empty string drops that one header.
    pub fn from_env(env: &ConfigService) -> Result<Self> {
        let d = Self::default();
        Ok(Self {
            enabled: env.flag("SECURITY_HEADERS", d.enabled)?,
            content_type_options: env.flag("CONTENT_TYPE_OPTIONS", d.content_type_options)?,
            frame_options: override_header(env.get("FRAME_OPTIONS"), d.frame_options),
            hsts: override_header(env.get("HSTS"), d.hsts),
        })
    }

    /// The `(name, value)` headers to set, given whether TLS is active. HSTS is
    /// included only under TLS. Returns an empty list when disabled.
    pub fn headers(&self, tls_active: bool) -> Vec<(&'static str, String)> {
        if !self.enabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.content_type_options {
            out.push(("x-content-type-options", "nosniff".to_owned()));
        }
        if let Some(v) = non_empty(&self.frame_options) {
            out.push(("x-frame-options", v));
        }
        if tls_active && let Some(v) = non_empty(&self.hsts) {
            out.push(("strict-transport-security", v));
        }
        out
    }
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

    #[test]
    fn defaults_are_on_with_safe_values() {
        let d = SecurityHeadersConfig::default();
        let plain = d.headers(false);
        assert!(plain.contains(&("x-content-type-options", "nosniff".to_owned())));
        assert!(plain.contains(&("x-frame-options", "DENY".to_owned())));
        assert!(
            !plain.iter().any(|(k, _)| *k == "strict-transport-security"),
            "HSTS must not be emitted over plain HTTP",
        );
    }

    #[test]
    fn hsts_only_under_tls() {
        let d = SecurityHeadersConfig::default();
        let tls = d.headers(true);
        assert!(
            tls.iter().any(|(k, _)| *k == "strict-transport-security"),
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
    fn an_empty_override_drops_one_header() {
        let cfg = SecurityHeadersConfig {
            frame_options: override_header(Some(String::new()), Some("DENY".into())),
            ..Default::default()
        };
        assert!(!cfg.headers(false).iter().any(|(k, _)| *k == "x-frame-options"));
        assert!(
            cfg.headers(false)
                .iter()
                .any(|(k, _)| *k == "x-content-type-options"),
            "dropping one header leaves the others",
        );
    }
}
