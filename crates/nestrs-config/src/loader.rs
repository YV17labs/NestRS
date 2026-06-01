//! The configuration loaders and the framework-wide environment-variable scheme.

use std::env;

use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use serde::de::DeserializeOwned;
use validator::Validate;

use crate::error::Result;

/// Framework-wide environment-variable scheme.
///
/// **Rule:** `NESTRS_<DOMAIN>__<KEY>`. One prefix (`NESTRS_`), one domain
/// segment, then the leaf key. Domain boundaries use **double underscore**;
/// the leaf key itself stays snake_case (single underscores allowed inside
/// it). Nothing outside this prefix is read by the framework — no
/// `OTEL_*`/`RUST_LOG` aliasing.
///
/// **The domain is the owning crate's name** with the `nestrs-` prefix stripped
/// (`nestrs-database` → `database`, `nestrs-telemetry` → `telemetry`). A domain
/// and its crate always share a name; if they diverge, one of them is misnamed.
///
/// Why double-underscore: it lets [`load`] feed any `serde`-deserializable
/// struct directly via figment's `Env::prefixed("NESTRS_").split("__")`, so
/// `NESTRS_DATABASE__MAX_CONNECTIONS` populates `database.max_connections`
/// without ambiguity vs. leaf keys whose own names contain underscores.
///
/// Domains in use today (extend the table as crates land):
///
/// | Domain      | Owner (crate)        | Example variable                   |
/// |-------------|----------------------|------------------------------------|
/// | `database`  | `nestrs-database`    | `NESTRS_DATABASE__URL`             |
/// | `queue`     | `nestrs-queue`       | `NESTRS_QUEUE__URL`                |
/// | `authn`     | `nestrs-authn`       | `NESTRS_AUTHN__PUBLIC_KEY`, `NESTRS_AUTHN__OAUTH_CLIENT_ID` |
/// | `telemetry` | `nestrs-telemetry`   | `NESTRS_TELEMETRY__LOG_LEVEL`, `NESTRS_TELEMETRY__SERVICE_NAME` |
/// | `http`      | `nestrs-http`        | `NESTRS_HTTP__TLS_KEY_FILE`, `NESTRS_HTTP__ACCESS_LOG` |
///
/// Each crate that owns a domain documents its full key list on the relevant
/// config type. Crates **must not** read env vars under another crate's
/// domain — that is the contract that keeps the namespace coherent.
///
/// [`load`] is the bulk loader for apps that prefer a TOML file overlaid
/// with env vars; individual framework crates expose `from_env()` shortcuts
/// that read the same names directly via [`env_var`].
pub fn load<T: DeserializeOwned>(toml_path: Option<&str>) -> Result<T> {
    crate::dotenv::ensure_env_loaded();
    let mut figment = Figment::new();
    if let Some(path) = toml_path {
        figment = figment.merge(Toml::file(path));
    }
    figment = figment.merge(Env::prefixed("NESTRS_").split("__"));
    Ok(figment.extract()?)
}

/// Like [`load`], then run the config type's declarative `validator`
/// `#[validate(...)]` rules — the `@nestjs/config` + class-validator combo. Fails
/// with [`ConfigError::Validation`](crate::ConfigError::Validation) when a rule is
/// violated, so a malformed environment is caught at startup rather than at first
/// use. Seed the result into the DI graph (`App::builder().provide(cfg)`) to inject
/// it as `Arc<T>`.
pub fn load_validated<T: DeserializeOwned + Validate>(toml_path: Option<&str>) -> Result<T> {
    let value: T = load(toml_path)?;
    value.validate()?;
    Ok(value)
}

/// Load a **namespaced** config from `NESTRS_<NAMESPACE>__*`, then validate it —
/// the loader behind [`Config::load`](crate::Config::load) (the `registerAs`
/// analog). Unlike [`load`], the prefix already carries the domain, so the
/// remaining `__`-split keys map onto the struct's fields directly:
/// `NESTRS_DATABASE__MAX_CONNECTIONS` populates `max_connections` of a
/// `namespace = "database"` config. Env-only by design — a namespaced config is
/// environment-driven; an app that wants a TOML overlay uses [`load`] on a
/// whole-app struct instead.
///
/// Fails with [`ConfigError`](crate::ConfigError) on a malformed value or a
/// violated `#[validate(...)]` rule, so a bad environment is caught at boot
/// rather than at first use.
pub fn load_namespaced<T: DeserializeOwned + Validate>(namespace: &str) -> Result<T> {
    crate::dotenv::ensure_env_loaded();
    let value: T = Figment::new().merge(namespaced_env(namespace)).extract()?;
    value.validate()?;
    Ok(value)
}

/// The `NESTRS_<NAMESPACE>__*` env layer: the single place the domain prefix is
/// built (used by [`load_namespaced`]).
fn namespaced_env(namespace: &str) -> Env {
    Env::prefixed(&format!("NESTRS_{}__", namespace.to_ascii_uppercase())).split("__")
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
    use crate::ConfigError;
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
    struct DomainConfig {
        max_connections: u32,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct NestedConfig {
        database: DomainConfig,
    }

    /// Locks the `NESTRS_<DOMAIN>__<KEY>` mapping so a future change to the
    /// figment splitter wouldn't silently break the framework scheme: the
    /// double underscore separates the domain from a snake_case leaf key.
    #[test]
    #[allow(clippy::result_large_err)]
    fn double_underscore_separates_domain_from_snake_case_key() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_DATABASE__MAX_CONNECTIONS", "16");
            let cfg: NestedConfig = load(None).expect("config should load");
            assert_eq!(cfg.database.max_connections, 16);
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
