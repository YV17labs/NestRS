//! CORS settings for the HTTP transport, settable both via `NESTRS_HTTP__CORS_*`
//! env vars and pinned in code as `HttpConfig.cors`. The [`HttpModule`](crate::HttpModule)
//! translates a [`CorsConfig`] into poem's [`Cors`](poem::middleware::Cors)
//! middleware at boot.

use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, Result};
use nestrs_config::ConfigService;
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
