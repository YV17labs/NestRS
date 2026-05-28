//! Configuration loading for nestrs — the `@nestjs/config` analog.
//!
//! Kept out of `nestrs-core` so the kernel (container, modules, lifecycle) does
//! not drag `figment` into every crate that depends on it; an app or framework
//! crate that needs configuration depends on `nestrs-config` directly.

use std::env;

use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::de::DeserializeOwned;
use thiserror::Error;
use validator::Validate;

/// A configuration failure: a load/parse error from figment, or a violation of a
/// config type's declarative `validator` rules ([`load_validated`]).
// `figment::Error` is ~208 bytes; boxing it keeps every `Result<_, ConfigError>`
// small, satisfying `clippy::result_large_err` without bloating the hot path.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration error: {0}")]
    Source(Box<figment::Error>),
    #[error("configuration validation failed: {0}")]
    Validation(#[from] validator::ValidationErrors),
}

impl From<figment::Error> for ConfigError {
    fn from(value: figment::Error) -> Self {
        Self::Source(Box::new(value))
    }
}

pub type Result<T> = std::result::Result<T, ConfigError>;

/// Framework-wide environment-variable scheme.
///
/// **Rule:** `NESTRS_<DOMAIN>__<KEY>`. One prefix (`NESTRS_`), one domain
/// segment, then the leaf key. Domain boundaries use **double underscore**;
/// the leaf key itself stays snake_case (single underscores allowed inside
/// it). Nothing outside this prefix is read by the framework — no
/// `OTEL_*`/`RUST_LOG` aliasing.
///
/// Why double-underscore: it lets [`load`] feed any `serde`-deserializable
/// struct directly via figment's `Env::prefixed("NESTRS_").split("__")`, so
/// `NESTRS_SERVICE__INSTANCE_ID` populates `service.instance_id` without
/// ambiguity vs. struct fields whose own names contain underscores.
///
/// Domains in use today (extend the table as crates land):
///
/// | Domain      | Owner                | Example variable                   |
/// |-------------|----------------------|------------------------------------|
/// | `log`       | `nestrs-telemetry`   | `NESTRS_LOG__LEVEL`                |
/// | `service`   | `nestrs-telemetry`   | `NESTRS_SERVICE__NAME`             |
/// | `telemetry` | `nestrs-telemetry`   | `NESTRS_TELEMETRY__OTLP_ENDPOINT`  |
/// | `http`      | `nestrs-telemetry`   | `NESTRS_HTTP__ACCESS_LOG` (via `OtelHttp`) |
///
/// Each crate that owns a domain documents its full key list on the relevant
/// config type. Crates **must not** read env vars under another crate's
/// domain — that is the contract that keeps the namespace coherent.
///
/// [`load`] is the bulk loader for apps that prefer a TOML file overlaid
/// with env vars; individual framework crates expose `from_env()` shortcuts
/// that read the same names directly via [`env_var`].
pub fn load<T: DeserializeOwned>(toml_path: Option<&str>) -> Result<T> {
    let mut figment = Figment::new();
    if let Some(path) = toml_path {
        figment = figment.merge(Toml::file(path));
    }
    figment = figment.merge(Env::prefixed("NESTRS_").split("__"));
    Ok(figment.extract()?)
}

/// Like [`load`], then run the config type's declarative `validator`
/// `#[validate(...)]` rules — the `@nestjs/config` + class-validator combo. Fails
/// with [`ConfigError::Validation`] when a rule is violated, so a malformed
/// environment is caught at startup rather than at first use. Seed the result
/// into the DI graph (`App::builder().provide(cfg)`) to inject it as `Arc<T>`.
pub fn load_validated<T: DeserializeOwned + Validate>(toml_path: Option<&str>) -> Result<T> {
    let value: T = load(toml_path)?;
    value.validate()?;
    Ok(value)
}

/// Read a single env var, treating empty strings as unset. Use this from
/// per-crate `from_env()` shortcuts that read individual `NESTRS_*` keys —
/// the empty-as-unset rule prevents `FOO=` in a `.env` file from blanking
/// out an in-code default.
pub fn env_var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq)]
    struct AppConfig {
        port: u16,
        name: String,
    }

    // `figment::Jail::expect_with` requires a closure returning the bare
    // `Result<(), figment::Error>` — its `Err` is ~208 bytes, but the
    // signature is fixed by figment so the lint cannot be honored here.
    #[test]
    #[allow(clippy::result_large_err)]
    fn load_from_env_overrides_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_PORT", "4242");
            jail.set_env("NESTRS_NAME", "demo");
            let cfg: AppConfig = load(None).expect("config should load");
            assert_eq!(
                cfg,
                AppConfig {
                    port: 4242,
                    name: "demo".into()
                }
            );
            Ok(())
        });
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct ServiceConfig {
        instance_id: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct NestedConfig {
        service: ServiceConfig,
    }

    /// Locks the `NESTRS_<DOMAIN>__<KEY>` mapping so a future change to the
    /// figment splitter wouldn't silently break the framework scheme.
    #[test]
    #[allow(clippy::result_large_err)]
    fn double_underscore_separates_domain_from_snake_case_key() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_SERVICE__INSTANCE_ID", "abc-123");
            let cfg: NestedConfig = load(None).expect("config should load");
            assert_eq!(cfg.service.instance_id, "abc-123");
            Ok(())
        });
    }

    #[derive(Debug, Deserialize, Validate)]
    struct ValidatedConfig {
        #[validate(range(min = 1))]
        port: u16,
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn load_validated_rejects_a_value_that_breaks_a_rule() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_PORT", "0");
            let err = load_validated::<ValidatedConfig>(None).expect_err("port 0 violates min = 1");
            assert!(matches!(err, ConfigError::Validation(_)));
            Ok(())
        });
    }

    #[test]
    #[allow(clippy::result_large_err)]
    fn load_validated_accepts_a_valid_value() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_PORT", "8080");
            let cfg = load_validated::<ValidatedConfig>(None).expect("port 8080 is valid");
            assert_eq!(cfg.port, 8080);
            Ok(())
        });
    }
}
