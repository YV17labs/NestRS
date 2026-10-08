//! Default security response headers, on by default, each overridable via
//! `<PREFIX>_HTTP__*` or the pinned struct. The family is
//! [OWASP's list](https://owasp.org/www-project-secure-headers/).
//!
//! Emitted by default:
//!
//! - `X-Content-Type-Options: nosniff`
//! - `X-Frame-Options: DENY`
//! - `Referrer-Policy: strict-origin-when-cross-origin`
//! - `Cross-Origin-Opener-Policy: same-origin`
//! - `Cross-Origin-Resource-Policy: same-origin` — binds `no-cors` loads only,
//!   so CORS requests are untouched; set `cross-origin` to be embedded.
//!
//! Configurable, off by default:
//!
//! - `Strict-Transport-Security` — carries a default value, applied only under TLS.
//! - `Content-Security-Policy`, `Cross-Origin-Embedder-Policy` and
//!   `Permissions-Policy` — they govern the app's own documents.

use nest_rs_config::{ConfigError, ConfigService, Result};
use poem::http::{HeaderName, HeaderValue, header};

/// HSTS default: one year, include subdomains. `preload` is an explicit opt-in.
const DEFAULT_HSTS: &str = "max-age=31536000; includeSubDomains";
const DEFAULT_FRAME_OPTIONS: &str = "DENY";
/// The referrer policy OWASP recommends: the origin travels cross-origin, the
/// path and query never do.
const DEFAULT_REFERRER_POLICY: &str = "strict-origin-when-cross-origin";
const DEFAULT_COOP: &str = "same-origin";
const DEFAULT_CORP: &str = "same-origin";

const KEY_FRAME_OPTIONS: &str = "FRAME_OPTIONS";
const KEY_HSTS: &str = "HSTS";
const KEY_REFERRER_POLICY: &str = "REFERRER_POLICY";
const KEY_COOP: &str = "CROSS_ORIGIN_OPENER_POLICY";
const KEY_CORP: &str = "CROSS_ORIGIN_RESOURCE_POLICY";
const KEY_COEP: &str = "CROSS_ORIGIN_EMBEDDER_POLICY";
const KEY_PERMISSIONS_POLICY: &str = "PERMISSIONS_POLICY";
const KEY_CSP: &str = "CONTENT_SECURITY_POLICY";

/// Default-on security headers. Disable the whole set with `enabled = false`;
/// drop an individual header with `None` in code, or `off` in the environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpSecurityHeaders {
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
    /// `Cross-Origin-Embedder-Policy` value; `None` (the default) ⇒ omitted.
    pub cross_origin_embedder_policy: Option<String>,
    /// `Permissions-Policy` value; `None` (the default) ⇒ omitted.
    pub permissions_policy: Option<String>,
    /// `Content-Security-Policy` value; `None` (the default) ⇒ omitted.
    pub content_security_policy: Option<String>,
}

impl Default for HttpSecurityHeaders {
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

impl HttpSecurityHeaders {
    /// Read `<PREFIX>_HTTP__SECURITY_HEADERS` (master) plus one key per header,
    /// overlaid onto `base`. Absent vars keep `base`'s value (the safe defaults
    /// unless the call site pinned something else); `off` drops that one
    /// header, and a blank value is refused.
    pub fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let resolved = Self {
            enabled: env.flag("SECURITY_HEADERS", base.enabled)?,
            content_type_options: env.flag("CONTENT_TYPE_OPTIONS", base.content_type_options)?,
            frame_options: override_header(env, KEY_FRAME_OPTIONS, base.frame_options)?,
            hsts: override_header(env, KEY_HSTS, base.hsts)?,
            referrer_policy: override_header(env, KEY_REFERRER_POLICY, base.referrer_policy)?,
            cross_origin_opener_policy: override_header(
                env,
                KEY_COOP,
                base.cross_origin_opener_policy,
            )?,
            cross_origin_resource_policy: override_header(
                env,
                KEY_CORP,
                base.cross_origin_resource_policy,
            )?,
            cross_origin_embedder_policy: override_header(
                env,
                KEY_COEP,
                base.cross_origin_embedder_policy,
            )?,
            permissions_policy: override_header(
                env,
                KEY_PERMISSIONS_POLICY,
                base.permissions_policy,
            )?,
            content_security_policy: override_header(env, KEY_CSP, base.content_security_policy)?,
        };
        // The response layer would silently drop an invalid value: a stray char
        // in `<PREFIX>_HTTP__HSTS` would remove HSTS in production.
        for entry in resolved.value_headers() {
            validate_header_value(env, entry.key, entry.value)?;
        }
        Ok(resolved)
    }

    /// The value-carrying headers, in emission order, read by both the boot
    /// validation and the emitted list.
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
/// value, naming the offending env var. Only a value pinned in code reaches
/// this refusal, so quoting it leaks nothing the environment held.
#[expect(
    clippy::map_err_ignore,
    reason = "InvalidHeaderValue carries nothing; the refusal names the variable"
)]
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

/// The value a header's variable takes to drop that header.
const OFF: &str = "off";

/// A header's value from the environment over `default`: absent keeps the
/// default, `off` (whatever its case) drops the header, and a blank value is
/// refused.
fn override_header(
    env: &ConfigService,
    key: &str,
    default: Option<String>,
) -> Result<Option<String>> {
    let Some(setting) = env.setting(key)? else {
        return Ok(default);
    };
    let value = setting.value.trim();
    if value.eq_ignore_ascii_case(OFF) {
        return Ok(None);
    }
    if value.is_empty() {
        return Err(setting.refuse(format_args!(
            "is blank — set it to `{OFF}` to drop the header"
        )));
    }
    if HeaderValue::from_str(value).is_err() {
        return Err(setting.refuse(format_args!(
            "`{}` is not a valid HTTP header value",
            setting.shown()
        )));
    }
    Ok(Some(setting.value))
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
        let plain = HttpSecurityHeaders::default().headers(false);
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

    #[test]
    fn the_cross_origin_and_referrer_defaults_are_emitted() {
        let plain = HttpSecurityHeaders::default().headers(false);
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

    #[test]
    fn the_argued_off_members_are_absent_by_default_and_settable() {
        let plain = HttpSecurityHeaders::default().headers(false);
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

        let pinned = HttpSecurityHeaders {
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
                HttpSecurityHeaders::from_env(&env, Default::default()).expect("value loads");
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
        let d = HttpSecurityHeaders::default();
        assert!(
            value_of(&d.headers(true), &header::STRICT_TRANSPORT_SECURITY).is_some(),
            "HSTS must be emitted under TLS",
        );
    }

    #[test]
    fn disabled_emits_nothing() {
        let cfg = HttpSecurityHeaders {
            enabled: false,
            ..Default::default()
        };
        assert!(cfg.headers(true).is_empty());
    }

    #[test]
    fn an_invalid_header_value_fails_boot_naming_the_var() {
        let cfg = ConfigService::with_vars("http", [(KEY_FRAME_OPTIONS, "bad\nvalue")]);
        let err = HttpSecurityHeaders::from_env(&cfg, Default::default()).unwrap_err();
        assert!(
            matches!(err, ConfigError::Parse { ref var, .. }
                if *var == nest_rs_config::var_name("http", KEY_FRAME_OPTIONS)),
            "expected a Parse error naming FRAME_OPTIONS, got {err:?}",
        );
    }

    #[test]
    fn an_invalid_value_on_a_newer_header_fails_boot_too() {
        let cfg =
            ConfigService::with_vars("http", [(KEY_CSP, "default-src 'none'\nX-Injected: y")]);
        let err = HttpSecurityHeaders::from_env(&cfg, Default::default()).unwrap_err();
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
            HttpSecurityHeaders::from_env(&cfg, Default::default()).expect("valid value loads");
        assert_eq!(loaded.frame_options.as_deref(), Some("SAMEORIGIN"));
    }

    #[test]
    fn off_in_the_environment_drops_one_header_and_leaves_the_others() {
        let env = ConfigService::with_vars(
            "http",
            [
                ("FRAME_OPTIONS", "off"),
                ("HSTS", " OFF "),
                ("REFERRER_POLICY", "no-referrer"),
            ],
        );
        let cfg = HttpSecurityHeaders::from_env(&env, HttpSecurityHeaders::default())
            .expect("`off` is a value the environment may set");
        let emitted = cfg.headers(true);
        assert!(
            value_of(&emitted, &header::X_FRAME_OPTIONS).is_none(),
            "`off` drops the header",
        );
        assert!(
            value_of(&emitted, &header::STRICT_TRANSPORT_SECURITY).is_none(),
            "whatever its case and surrounding space",
        );
        assert_eq!(
            value_of(&emitted, &header::REFERRER_POLICY).as_deref(),
            Some("no-referrer"),
            "any other value still overrides",
        );
        assert!(
            value_of(&emitted, &header::X_CONTENT_TYPE_OPTIONS).is_some(),
            "dropping one header leaves the others",
        );
    }

    #[test]
    fn a_blank_header_value_is_refused_naming_off() {
        let env = ConfigService::with_vars("http", [("HSTS", "   ")]);
        let err = HttpSecurityHeaders::from_env(&env, HttpSecurityHeaders::default())
            .expect_err("a blank header value is refused")
            .to_string();
        assert!(
            err.contains(&nest_rs_config::var_name("http", "HSTS")),
            "names the variable: {err}"
        );
        assert!(err.contains("`off`"), "and how to drop the header: {err}");
    }
}
