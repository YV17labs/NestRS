use nestrs_config::{Config, ConfigService, Result, config};
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
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            tls: None,
            cors: None,
            server_header: false,
        }
    }
}

impl Config for HttpConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        Ok(Self {
            host: env.get("HOST").unwrap_or_else(|| DEFAULT_HOST.to_string()),
            port: env.parse("PORT")?.unwrap_or(DEFAULT_PORT),
            tls: TlsConfig::from_env().map_err(|e| nestrs_config::ConfigError::Parse {
                var: "NESTRS_HTTP__TLS_*".into(),
                message: e.to_string(),
            })?,
            cors: CorsConfig::from_env(env).map_err(|e| nestrs_config::ConfigError::Parse {
                var: "NESTRS_HTTP__CORS_*".into(),
                message: e.to_string(),
            })?,
            server_header: env.flag("SERVER_HEADER", false)?,
        })
    }
}

#[cfg(test)]
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
    }

    #[test]
    fn default_constants_do_not_drift() {
        // App ops read `DEFAULT_PORT` indirectly via `HttpConfig::default()` —
        // a rename or value change is a deployment surprise. Pin them.
        assert_eq!(DEFAULT_HOST, "0.0.0.0");
        assert_eq!(DEFAULT_PORT, 3000);
    }
}
