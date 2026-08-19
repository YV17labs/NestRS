//! CORS settings for the HTTP transport, settable both via `NESTRS_HTTP__CORS_*`
//! env vars and pinned in code as `HttpConfig.cors`. The [`HttpModule`](crate::HttpModule)
//! translates a [`CorsConfig`] into poem's [`Cors`](poem::middleware::Cors)
//! middleware at boot.

use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use nest_rs_config::ConfigService;
use poem::http::{HeaderName, Method};
use poem::middleware::Cors;

/// Cross-Origin Resource Sharing policy. `origins` empty ⇒ no CORS layer
/// installed (the default). Lists are comma-separated in env vars.
#[derive(Clone, Debug, Default)]
pub struct CorsConfig {
    /// Allowed origins; empty ⇒ no CORS layer installed.
    pub origins: Vec<String>,
    /// Allowed request methods (`Access-Control-Allow-Methods`).
    pub methods: Vec<String>,
    /// Allowed request headers (`Access-Control-Allow-Headers`).
    pub headers: Vec<String>,
    /// Response headers exposed to the browser (`Access-Control-Expose-Headers`).
    pub exposed_headers: Vec<String>,
    /// Whether to allow credentialed requests (`Access-Control-Allow-Credentials`).
    pub credentials: bool,
    /// Preflight cache lifetime (`Access-Control-Max-Age`); `None` omits it.
    pub max_age: Option<Duration>,
}

/// The wildcard, as the four lists spell it.
const WILDCARD: &str = "*";

/// The four lists a `*` may appear in, paired with the response header each one
/// renders into. One table, so the refusal below is worded once and a fifth list
/// joins it by growing this array rather than by copying a check.
type WildcardList<'a> = (&'a str, &'a str, &'a [String]);

impl CorsConfig {
    /// Every list where `*` is a literal once the request's credentials mode is
    /// `include` — WHATWG Fetch, *CORS protocol*.
    ///
    /// The rule is one rule and it binds four headers, not one:
    /// `Access-Control-Allow-Origin`, `-Headers`, `-Methods` and
    /// `-Expose-Headers` all lose their wildcard meaning for a credentialed
    /// request, and `*` is read as the literal origin / header name / method
    /// named `*`. `*` is a valid `tchar`, so nothing refuses it: the header is
    /// emitted, every browser drops the response, and there is nothing
    /// server-side to point at. Only the origin list was checked here, and the
    /// other three were accepted in silence.
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

    /// One check, one sentence, four lists. A per-key refusal multiplies with
    /// the table and what multiplies is what gets skipped — which is how three
    /// of these four came to be unchecked.
    fn reject_wildcards_with_credentials(&self) -> Result<()> {
        if !self.credentials {
            return Ok(());
        }
        for (field, header, values) in self.wildcard_lists() {
            if values.iter().any(|value| value == WILDCARD) {
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

    /// Overlay the `NESTRS_HTTP__CORS_*` keys onto `base` (the policy pinned in
    /// code, if any). Returns `Ok(None)` when neither the environment nor `base`
    /// supplies an origin — no origins means no CORS layer.
    ///
    /// Every sub-key overlays independently, so a deployment can widen the
    /// origins of a policy pinned in code without restating its methods and
    /// headers.
    pub fn from_env(env: &ConfigService, base: Option<Self>) -> Result<Option<Self>> {
        let base = base.unwrap_or_default();
        let origins = env.list("CORS_ORIGINS", base.origins);
        if origins.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            origins,
            methods: env.list("CORS_METHODS", base.methods),
            headers: env.list("CORS_HEADERS", base.headers),
            exposed_headers: env.list("CORS_EXPOSED", base.exposed_headers),
            credentials: env
                .flag("CORS_CREDENTIALS", base.credentials)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?,
            max_age: env
                .parse::<u64>("CORS_MAX_AGE")
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .map(Duration::from_secs)
                .or(base.max_age),
        }))
    }

    /// Translate to poem's middleware. `origins: ["*"]` becomes the
    /// wildcard; explicit origins map one-to-one.
    pub fn into_middleware(self) -> Result<Cors> {
        self.reject_wildcards_with_credentials()?;
        let mut cors = Cors::new();
        for origin in &self.origins {
            cors = cors.allow_origin(origin);
        }
        for m in &self.methods {
            let method = Method::from_bytes(m.as_bytes())
                .with_context(|| format!("invalid HTTP method in CORS config: `{m}`"))?;
            cors = cors.allow_method(method);
        }
        for h in &self.headers {
            let header = HeaderName::from_str(h)
                .with_context(|| format!("invalid header name in CORS allow-list: `{h}`"))?;
            cors = cors.allow_header(header);
        }
        for h in &self.exposed_headers {
            let header = HeaderName::from_str(h)
                .with_context(|| format!("invalid header name in CORS expose-list: `{h}`"))?;
            cors = cors.expose_header(header);
        }
        if self.credentials {
            cors = cors.allow_credentials(true);
        }
        if let Some(age) = self.max_age {
            let secs: i32 = age
                .as_secs()
                .try_into()
                .context("CORS max_age overflows i32 seconds (~68 years); pick a smaller value")?;
            cors = cors.max_age(secs);
        }
        Ok(cors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(origins: &[&str]) -> CorsConfig {
        CorsConfig {
            origins: origins.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn into_middleware_accepts_an_empty_config() {
        cfg(&[]).into_middleware().expect("empty config builds");
    }

    #[test]
    fn into_middleware_accepts_a_basic_origin_list() {
        cfg(&["https://app.example.com"])
            .into_middleware()
            .expect("valid config");
    }

    fn err_string(result: Result<Cors>) -> String {
        match result {
            Ok(_) => panic!("expected an error"),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn into_middleware_rejects_an_invalid_method() {
        // Spaces aren't token characters in RFC 9110 §9 — `Method::from_bytes`
        // refuses them.
        let cfg = CorsConfig {
            origins: vec!["*".into()],
            methods: vec!["BAD METHOD".into()],
            ..Default::default()
        };
        let err = err_string(cfg.into_middleware());
        assert!(err.contains("invalid HTTP method"), "got: {err}");
    }

    #[test]
    fn into_middleware_rejects_an_invalid_header_name() {
        let cfg = CorsConfig {
            origins: vec!["*".into()],
            headers: vec!["bad header!".into()],
            ..Default::default()
        };
        let err = err_string(cfg.into_middleware());
        assert!(err.contains("invalid header name"), "got: {err}");
    }

    #[test]
    fn into_middleware_rejects_a_max_age_that_overflows_i32_seconds() {
        let cfg = CorsConfig {
            origins: vec!["*".into()],
            max_age: Some(Duration::from_secs(u64::MAX)),
            ..Default::default()
        };
        let err = err_string(cfg.into_middleware());
        assert!(err.contains("max_age overflows"), "got: {err}");
    }

    #[test]
    fn into_middleware_accepts_credentials_and_max_age_and_exposed_headers() {
        let cfg = CorsConfig {
            origins: vec!["https://app.example.com".into()],
            methods: vec!["GET".into(), "POST".into()],
            headers: vec!["content-type".into(), "x-trace-id".into()],
            exposed_headers: vec!["x-trace-id".into()],
            credentials: true,
            max_age: Some(Duration::from_secs(60 * 60)),
        };
        cfg.into_middleware()
            .expect("a fully-specified config builds");
    }

    #[test]
    fn into_middleware_rejects_an_invalid_exposed_header() {
        let cfg = CorsConfig {
            origins: vec!["*".into()],
            exposed_headers: vec!["bad header!".into()],
            ..Default::default()
        };
        let err = err_string(cfg.into_middleware());
        assert!(err.contains("invalid header name"), "got: {err}");
        assert!(err.contains("expose-list"), "must name the list: {err}");
    }

    // WHATWG Fetch's credentials rule binds four lists, not one. Each is
    // asserted on its own: a shared check that silently stopped covering one of
    // them would still pass a test that only ever looked at `origins`.
    #[test]
    fn credentials_refuse_a_wildcard_in_every_list_the_rule_binds() {
        /// One case: the field a `*` is planted in, the header it renders
        /// into, and the plant itself.
        type Case = (&'static str, &'static str, fn(&mut CorsConfig));
        let cases: [Case; 4] = [
            ("origins", "Access-Control-Allow-Origin", |c| {
                c.origins = vec![WILDCARD.into()]
            }),
            ("headers", "Access-Control-Allow-Headers", |c| {
                c.headers = vec![WILDCARD.into()]
            }),
            ("methods", "Access-Control-Allow-Methods", |c| {
                c.methods = vec![WILDCARD.into()]
            }),
            ("exposed_headers", "Access-Control-Expose-Headers", |c| {
                c.exposed_headers = vec![WILDCARD.into()]
            }),
        ];
        for (field, header, wildcard) in cases {
            let mut cfg = CorsConfig {
                origins: vec!["https://app.example.com".into()],
                credentials: true,
                ..Default::default()
            };
            wildcard(&mut cfg);
            let err = err_string(cfg.into_middleware());
            assert!(err.contains(field), "must name the list: {err}");
            assert!(err.contains(header), "must name the header: {err}");
        }
    }

    // The refusal is about the *pair*: a wildcard without credentials is the
    // ordinary public-API policy and must keep building.
    #[test]
    fn a_wildcard_without_credentials_still_builds() {
        CorsConfig {
            origins: vec![WILDCARD.into()],
            headers: vec![WILDCARD.into()],
            methods: vec![WILDCARD.into()],
            exposed_headers: vec![WILDCARD.into()],
            credentials: false,
            ..Default::default()
        }
        .into_middleware()
        .expect("a wildcard policy with no credentials is legal");
    }

    #[test]
    fn from_env_returns_none_when_origins_unset() {
        let cfg =
            CorsConfig::from_env(&ConfigService::with_vars("http", []), None).expect("no error");
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
        let cfg = CorsConfig::from_env(&service, None)
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
        let cfg = CorsConfig::from_env(&service, None)
            .expect("no error")
            .expect("Some");
        assert!(cfg.credentials);
        assert_eq!(cfg.max_age, Some(Duration::from_secs(600)));
    }
}
