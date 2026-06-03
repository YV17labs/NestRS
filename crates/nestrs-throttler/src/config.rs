//! [`ThrottlerConfig`] — rate-limit settings populated from `NESTRS_THROTTLER__*`.

use nestrs_config::{config, Config, ConfigService, Result};
use validator::Validate;

#[config(namespace = "throttler")]
#[derive(Clone, Debug, Default, Validate)]
pub struct ThrottlerConfig {
    /// Requests allowed per window. Unset ⇒ module default (60).
    pub limit: Option<u32>,
    /// Window size in whole seconds. Unset ⇒ module default (60).
    pub window_secs: Option<u64>,
    /// Proxies whose `X-Forwarded-For` chain may supply the client IP. An
    /// unparseable IP aborts the boot.
    pub trusted_proxies: Vec<String>,
}

impl Config for ThrottlerConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        Ok(Self {
            limit: env.parse("LIMIT")?,
            window_secs: env.parse("WINDOW_SECS")?,
            trusted_proxies: env.list("TRUSTED_PROXIES"),
        })
    }
}
