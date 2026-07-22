//! [`ConfigSource`] — pluggable backing store for [`ConfigService`].
//!
//! [`EnvSource`] (default) resolves each variable from the real process
//! environment, falling back to the parsed `.env` cascade
//! (`crate::dotenv::dotenv_values`) when the real env leaves it unset — the real
//! env always wins and the process environment is **never** mutated. A
//! third-party crate can ship an alternative (Vault, K8s ConfigMap, AWS
//! Parameter Store) by implementing [`ConfigSource`] and constructing
//! [`ConfigService::with_source`].
//!
//! Sync on purpose: `Config::from_env` runs sync at boot. A remote source
//! pre-fetches into an in-memory map and serves `get` from that map.
//!
//! [`ConfigService`]: crate::ConfigService
//! [`ConfigService::with_source`]: crate::ConfigService::with_source

use std::collections::HashMap;
use std::env;

use crate::dotenv::dotenv_values;

/// Resolve `name` from the real process environment, falling back to the parsed
/// `.env` cascade. The real env always wins; a value **present but empty** in
/// the real env counts as unset (so `FOO=` does not blank an in-code default)
/// **and** suppresses the dotenv fallback. Dotenv values are read from an
/// in-crate map and never written back, so this is side-effect-free and safe to
/// call from any thread — nothing here mutates the process environment.
pub fn env_var(name: &str) -> Option<String> {
    env_var_from(name, dotenv_values())
}

/// Core of [`env_var`], with the dotenv map supplied — factored out so the
/// real-env-vs-dotenv precedence is unit-testable without the process-wide
/// `OnceLock`.
fn env_var_from(name: &str, dotenv: &HashMap<String, String>) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        // Present but empty in the real env: treat as unset, and do not fall
        // back. An explicit real-env entry shadows the cascade, matching the
        // set-if-absent semantics of `load_cascade`.
        Ok(_) => None,
        // Same shadowing semantics, but a non-UTF-8 value is far more likely
        // a mistake than a deliberate unset — never swallow it silently.
        Err(env::VarError::NotUnicode(_)) => {
            tracing::warn!(
                target: "nest_rs::config",
                name,
                "environment variable is not valid UTF-8 — treated as unset, cascade suppressed",
            );
            None
        }
        Err(env::VarError::NotPresent) => dotenv.get(name).filter(|v| !v.is_empty()).cloned(),
    }
}

/// Read `name` from the **real** process environment only — no `.env` fallback.
/// Empty counts as unset. Used where the value must come from the real env by
/// contract: `NESTRS_ENV` selects which cascade files load, so it cannot itself
/// be sourced from the cascade (and reading it via [`env_var`] would recurse
/// into `dotenv_values`, which reads `NESTRS_ENV`).
pub(crate) fn real_env_var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Where a [`ConfigService`](crate::ConfigService) reads raw values from. The
/// default is [`EnvSource`] (process env + `.env` cascade); a third-party
/// crate can ship an alternative (Vault, K8s ConfigMap, AWS Parameter Store)
/// by implementing this trait and passing an instance to
/// [`ConfigService::with_source`](crate::ConfigService::with_source).
pub trait ConfigSource: Send + Sync + 'static {
    /// Return the raw value for the fully-qualified variable name (e.g.
    /// `"NESTRS_DATABASE__URL"`). Empty strings should be treated as unset.
    fn get(&self, var: &str) -> Option<String>;
}

/// Default [`ConfigSource`] — resolves from the real process environment with a
/// parsed `.env` cascade fallback (real env wins). Reading a value **never**
/// mutates the process environment; the cascade is parsed lazily into an
/// in-crate map on first use. A [`ConfigService`](crate::ConfigService) built on
/// a custom [`ConfigSource`] shares none of this — it touches neither the
/// cascade nor the process env.
#[derive(Default)]
pub struct EnvSource;

impl ConfigSource for EnvSource {
    fn get(&self, var: &str) -> Option<String> {
        env_var(var)
    }
}

/// A [`ConfigSource`] backed by an in-memory map — resolves each variable from
/// the map and touches **neither** the process environment nor the `.env`
/// cascade. Pair it with
/// [`ConfigService::with_source`](crate::ConfigService::with_source) to exercise
/// config parsing **hermetically** (tests, fixtures) without mutating global
/// process env — so the tests need no `unsafe { std::env::set_var }` and stay
/// parallel-safe. Keys are the fully-qualified `NESTRS_<NS>__<KEY>` names.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_config::{ConfigService, MapSource};
///
/// let source = MapSource::from_iter([("NESTRS_APP__PORT", "8080")]);
/// let cfg = ConfigService::with_source("app", Arc::new(source));
/// assert_eq!(cfg.get("PORT").as_deref(), Some("8080"));
/// assert_eq!(cfg.get("MISSING"), None); // absent ⇒ falls back to in-code defaults
/// ```
#[derive(Clone, Debug, Default)]
pub struct MapSource(HashMap<String, String>);

impl MapSource {
    /// An empty source — every lookup returns `None`, so a config falls back to
    /// its in-code defaults. Build a populated one with
    /// [`FromIterator`]/[`MapSource::from_iter`] (or, more ergonomically,
    /// [`ConfigService::with_vars`](crate::ConfigService::with_vars)).
    pub fn new() -> Self {
        Self::default()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for MapSource {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self(
            iter.into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        )
    }
}

impl ConfigSource for MapSource {
    fn get(&self, var: &str) -> Option<String> {
        // Empty counts as unset, mirroring `EnvSource`.
        self.0.get(var).filter(|v| !v.is_empty()).cloned()
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    // `env_var` resolves the real env first, then the dotenv map, and mutates
    // nothing. Pin the precedence directly on the pure core so the tests don't
    // fight the process-wide `dotenv_values` `OnceLock` (which caches the first
    // environment it sees for the whole process).

    #[test]
    fn env_var_prefers_real_env_over_dotenv() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_PREC__X", "from_real");
            let map = HashMap::from([("NESTRS_PREC__X".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(
                env_var_from("NESTRS_PREC__X", &map).as_deref(),
                Some("from_real"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_falls_back_to_dotenv_when_real_env_absent() {
        figment::Jail::expect_with(|_| {
            let map = HashMap::from([("NESTRS_PREC__Y".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(
                env_var_from("NESTRS_PREC__Y", &map).as_deref(),
                Some("from_dotenv"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_present_but_empty_real_env_suppresses_dotenv_fallback() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_PREC__Z", "");
            let map = HashMap::from([("NESTRS_PREC__Z".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(env_var_from("NESTRS_PREC__Z", &map), None);
            Ok(())
        });
    }

    #[test]
    fn env_var_read_never_writes_the_dotenv_value_into_the_process_env() {
        figment::Jail::expect_with(|_| {
            let map = HashMap::from([("NESTRS_PREC__ONLY_IN_MAP".to_owned(), "v".to_owned())]);
            assert_eq!(
                env_var_from("NESTRS_PREC__ONLY_IN_MAP", &map).as_deref(),
                Some("v"),
            );
            // The read resolved from the map, not by merging into the env.
            assert!(std::env::var("NESTRS_PREC__ONLY_IN_MAP").is_err());
            Ok(())
        });
    }
}
