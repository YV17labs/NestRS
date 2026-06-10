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
    pub origins: Vec<String>,
    pub methods: Vec<String>,
    pub headers: Vec<String>,
    pub exposed_headers: Vec<String>,
    pub credentials: bool,
    pub max_age: Option<Duration>,
}

impl CorsConfig {
    /// Build a [`CorsConfig`] from the `NESTRS_HTTP__CORS_*` keys. Returns
    /// `Ok(None)` when `NESTRS_HTTP__CORS_ORIGINS` is unset (CORS off).
    pub fn from_env(env: &ConfigService) -> Result<Option<Self>> {
        let origins = env.list("CORS_ORIGINS");
        if origins.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            origins,
            methods: env.list("CORS_METHODS"),
            headers: env.list("CORS_HEADERS"),
            exposed_headers: env.list("CORS_EXPOSED"),
            credentials: env
                .flag("CORS_CREDENTIALS", false)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?,
            max_age: env
                .parse::<u64>("CORS_MAX_AGE")
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .map(Duration::from_secs),
        }))
    }

    /// Translate to poem's middleware. `origins: ["*"]` becomes the
    /// wildcard; explicit origins map one-to-one.
    pub fn into_middleware(self) -> Result<Cors> {
        if self.credentials && self.origins.iter().any(|origin| origin == "*") {
            anyhow::bail!("invalid CORS config: wildcard origin with credentials is not permitted");
        }
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

    // `from_env` mutates real process env; serialize so two tests don't race.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // FIXME: env mutation is unsafe; serialized within this binary by the
        // mutex above.
        for (k, v) in vars {
            match v {
                Some(value) => unsafe { std::env::set_var(k, value) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        // Wipe after — never leak set vars to neighbouring tests.
        for (k, _) in vars {
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    fn http_env() -> nest_rs_config::ConfigService {
        nest_rs_config::ConfigService::for_namespace("http")
    }

    #[test]
    fn from_env_returns_none_when_origins_unset() {
        with_env(&[("NESTRS_HTTP__CORS_ORIGINS", None)], || {
            let cfg = CorsConfig::from_env(&http_env()).expect("no error");
            assert!(cfg.is_none(), "unset origins ⇒ CORS off");
        });
    }

    #[test]
    fn from_env_reads_origins_methods_headers_when_set() {
        with_env(
            &[
                (
                    "NESTRS_HTTP__CORS_ORIGINS",
                    Some("https://a.example,https://b.example"),
                ),
                ("NESTRS_HTTP__CORS_METHODS", Some("GET,POST")),
                ("NESTRS_HTTP__CORS_HEADERS", Some("content-type")),
            ],
            || {
                let cfg = CorsConfig::from_env(&http_env())
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
            },
        );
    }

    #[test]
    fn from_env_reads_credentials_flag_and_max_age() {
        with_env(
            &[
                ("NESTRS_HTTP__CORS_ORIGINS", Some("*")),
                ("NESTRS_HTTP__CORS_CREDENTIALS", Some("true")),
                ("NESTRS_HTTP__CORS_MAX_AGE", Some("600")),
            ],
            || {
                let cfg = CorsConfig::from_env(&http_env())
                    .expect("no error")
                    .expect("Some");
                assert!(cfg.credentials);
                assert_eq!(cfg.max_age, Some(Duration::from_secs(600)));
            },
        );
    }
}
