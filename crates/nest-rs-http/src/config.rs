use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

use crate::cors::CorsConfig;
use crate::tls::TlsConfig;

const DEFAULT_HOST: &str = "0.0.0.0";
const DEFAULT_PORT: u16 = 3000;

/// HTTP transport options resolved at boot. Every field is settable both via
/// `NESTRS_HTTP__*` env vars (read by [`Config::from_env`]) and via the pinned
/// struct (passed to [`HttpModule::for_root`](crate::HttpModule::for_root)).
#[config(namespace = "http")]
#[derive(Clone, Debug, Validate)]
pub struct HttpConfig {
    pub host: String,
    pub port: u16,
    /// PEM cert + key for HTTPS. `None` ⇒ plain HTTP. Picked up from
    /// `NESTRS_HTTP__TLS_CERT[_FILE]` + `NESTRS_HTTP__TLS_KEY[_FILE]`.
    pub tls: Option<TlsConfig>,
    /// CORS policy. `None` ⇒ no CORS layer. Populated when
    /// `NESTRS_HTTP__CORS_ORIGINS` is set (see [`CorsConfig`]).
    pub cors: Option<CorsConfig>,
    /// `true` ⇒ emit `Server: nestrs/<version>` on every response.
    /// Defaults to `false` (production-safe — no framework fingerprint).
    /// Flip to `true` in `.env.development` to expose the version locally.
    pub server_header: bool,
    /// Mount every controller under a shared path prefix (e.g. `/api`). `None`
    /// ⇒ no prefix. Read from `NESTRS_HTTP__GLOBAL_PREFIX`; normalization
    /// (trim, drop empty/`"/"`, ensure leading `/`, strip trailing `/`) lives
    /// in [`HttpTransport::global_prefix`](crate::HttpTransport::global_prefix).
    pub global_prefix: Option<String>,
    /// Cap on the request body size accepted by
    /// [`RawBody`](crate::RawBody). `None` ⇒ the extractor's built-in default
    /// (2 MiB). Read from `NESTRS_HTTP__MAX_BODY_BYTES`. Per-route overrides
    /// keep using [`RawBody::extract_with_limit`](crate::RawBody::extract_with_limit).
    pub max_body_bytes: Option<usize>,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            tls: None,
            cors: None,
            server_header: false,
            global_prefix: None,
            max_body_bytes: None,
        }
    }
}

impl HttpConfig {
    /// Pin the global prefix in code. Empty / `"/"` collapse to `None` via the
    /// transport's normalization, so callers can pass user-provided strings
    /// without sanitizing first.
    pub fn with_global_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.global_prefix = Some(prefix.into());
        self
    }

    /// Pin the [`RawBody`](crate::RawBody) byte cap in code. Applies to every
    /// extractor invocation that does not pick its own limit via
    /// [`RawBody::extract_with_limit`](crate::RawBody::extract_with_limit).
    pub fn with_max_body_bytes(mut self, n: usize) -> Self {
        self.max_body_bytes = Some(n);
        self
    }
}

impl Config for HttpConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        let global_prefix = env.get("GLOBAL_PREFIX").and_then(|raw| {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            }
        });
        Ok(Self {
            host: env.get("HOST").unwrap_or_else(|| DEFAULT_HOST.to_string()),
            port: env.parse("PORT")?.unwrap_or(DEFAULT_PORT),
            tls: TlsConfig::from_env().map_err(|e| nest_rs_config::ConfigError::Parse {
                var: "NESTRS_HTTP__TLS_*".into(),
                message: e.to_string(),
            })?,
            cors: CorsConfig::from_env(env).map_err(|e| nest_rs_config::ConfigError::Parse {
                var: "NESTRS_HTTP__CORS_*".into(),
                message: e.to_string(),
            })?,
            server_header: env.flag("SERVER_HEADER", false)?,
            global_prefix,
            max_body_bytes: env.parse("MAX_BODY_BYTES")?,
        })
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    #[test]
    fn defaults_bind_all_interfaces_on_3000_with_no_tls_no_cors() {
        let d = HttpConfig::default();
        assert_eq!(d.host, "0.0.0.0");
        assert_eq!(d.port, 3000);
        assert!(d.tls.is_none(), "default must serve plain HTTP");
        assert!(d.cors.is_none(), "no CORS layer by default");
        assert!(
            !d.server_header,
            "Server header opt-out by default — no framework fingerprint in prod",
        );
        assert!(d.global_prefix.is_none(), "no global prefix by default");
        assert!(
            d.max_body_bytes.is_none(),
            "no body cap override — RawBody::DEFAULT_LIMIT applies",
        );
    }

    #[test]
    fn default_constants_do_not_drift() {
        // App ops read `DEFAULT_PORT` indirectly via `HttpConfig::default()` —
        // a rename or value change is a deployment surprise. Pin them.
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
        assert_eq!(DEFAULT_PORT, 3000);
    }

    #[test]
    fn with_global_prefix_pins_in_code() {
        let cfg = HttpConfig::default().with_global_prefix("/api");
        assert_eq!(cfg.global_prefix.as_deref(), Some("/api"));
    }

    #[test]
    fn with_max_body_bytes_pins_in_code() {
        let cfg = HttpConfig::default().with_max_body_bytes(64);
        assert_eq!(cfg.max_body_bytes, Some(64));
    }

    #[test]
    fn from_env_reads_global_prefix() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__GLOBAL_PREFIX", "/api");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env).expect("env parses");
            assert_eq!(cfg.global_prefix.as_deref(), Some("/api"));
            Ok(())
        });
    }

    #[test]
    fn from_env_treats_blank_global_prefix_as_unset() {
        // `NESTRS_HTTP__GLOBAL_PREFIX=` (or whitespace) must not pin an empty
        // prefix that the transport would still try to nest under.
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__GLOBAL_PREFIX", "   ");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env).expect("env parses");
            assert!(cfg.global_prefix.is_none());
            Ok(())
        });
    }

    #[test]
    fn from_env_reads_max_body_bytes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__MAX_BODY_BYTES", "1024");
            let env = ConfigService::for_namespace("http");
            let cfg = HttpConfig::from_env(&env).expect("env parses");
            assert_eq!(cfg.max_body_bytes, Some(1024));
            Ok(())
        });
    }

    #[test]
    fn from_env_rejects_unparseable_max_body_bytes() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_HTTP__MAX_BODY_BYTES", "huge");
            let env = ConfigService::for_namespace("http");
            assert!(
                HttpConfig::from_env(&env).is_err(),
                "non-numeric must surface as ConfigError — no silent default",
            );
            Ok(())
        });
    }
}
