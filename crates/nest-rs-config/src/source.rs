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
                target: crate::TARGET,
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

/// Read `name` from the **deployment** — the real process environment minus
/// anything `Environment::init` merged in from a cascade file.
///
/// `init` publishes the cascade set-if-absent so raw `std::env::var` consumers
/// (`NESTRS_LOG*`, a `migrate` binary) behave as the cascade says. That merge
/// would otherwise promote every committed `.env` value into the deployment
/// tier and silently outrank a `for_root` pin — the tier the docs place
/// *above* every file. Subtracting the published names keeps the pin reachable
/// in a scaffolded app.
pub(crate) fn deployment_env_var(name: &str) -> Option<String> {
    if crate::dotenv::published_from_cascade(name) {
        return None;
    }
    real_env_var(name)
}

/// Where a [`ConfigService`](crate::ConfigService) reads raw values from. The
/// default is [`EnvSource`] (process env + `.env` cascade); a third-party
/// crate can ship an alternative (Vault, K8s ConfigMap, AWS Parameter Store)
/// by implementing this trait and passing an instance to
/// [`ConfigService::with_source`](crate::ConfigService::with_source).
pub trait ConfigSource: Send + Sync + 'static {
    /// Return the raw value for the fully-qualified variable name (e.g.
    /// `"NESTRS_SEAORM__URL"`). Empty strings should be treated as unset.
    fn get(&self, var: &str) -> Option<String>;

    /// The subset of [`get`](Self::get) that comes from the **deployment** —
    /// the tier that outranks a value pinned in code by
    /// `Module::for_root(cfg)`. Everything else (a `.env` file committed beside
    /// the code) loses to the pin instead, which is what makes
    /// `real env > pinned > .env cascade > defaults` a per-field rule rather
    /// than a whole-struct one.
    ///
    /// Defaults to [`get`](Self::get): a custom source is deployment-supplied
    /// unless it says otherwise, so a Vault or ConfigMap value is never
    /// shadowed by a pinned struct. Only [`EnvSource`] narrows it, and only
    /// because it is the one source serving two tiers at once.
    fn get_from_deployment(&self, var: &str) -> Option<String> {
        self.get(var)
    }
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

    /// The real process environment only. A `.env` file is checked into the
    /// repository next to the code that pins the value, so it reads as another
    /// in-code default and loses to the pin; an actual deployment variable
    /// wins. Values `Environment::init` merged **from** the cascade are
    /// subtracted, so publishing does not promote a committed file into the
    /// deployment tier.
    fn get_from_deployment(&self, var: &str) -> Option<String> {
        deployment_env_var(var)
    }
}

/// A [`ConfigSource`] backed by an in-memory map — resolves each variable from
/// the map and touches **neither** the process environment nor the `.env`
/// cascade. Pair it with
/// [`ConfigService::with_source`](crate::ConfigService::with_source) to exercise
/// config parsing **hermetically** (tests, fixtures) without mutating global
/// process env — so the tests need no `unsafe { std::env::set_var }` and stay
/// parallel-safe. Keys are the fully-qualified `<PREFIX>_<DOMAIN>__<KEY>` names.
///
/// ```
/// use std::sync::Arc;
/// use nest_rs_config::{ConfigService, MapSource, var_name};
///
/// // Built, never spelled: a fixture keyed on a literal reads nothing under a
/// // deployment that renamed the prefix, and reads it as "unset".
/// let port = var_name("app", "PORT");
/// let source = MapSource::from_iter([(port.as_str(), "8080")]);
/// let cfg = ConfigService::with_source("app", Arc::new(source));
/// assert_eq!(cfg.get("PORT").as_deref(), Some("8080"));
/// assert_eq!(cfg.get("MISSING"), None); // absent ⇒ falls back to in-code defaults
/// ```
#[derive(Clone, Debug, Default)]
pub struct MapSource(HashMap<String, String>);

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
            jail.set_env("FIXTURE_PREC__X", "from_real");
            let map = HashMap::from([("FIXTURE_PREC__X".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(
                env_var_from("FIXTURE_PREC__X", &map).as_deref(),
                Some("from_real"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_falls_back_to_dotenv_when_real_env_absent() {
        figment::Jail::expect_with(|_| {
            let map = HashMap::from([("FIXTURE_PREC__Y".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(
                env_var_from("FIXTURE_PREC__Y", &map).as_deref(),
                Some("from_dotenv"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_present_but_empty_real_env_suppresses_dotenv_fallback() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("FIXTURE_PREC__Z", "");
            let map = HashMap::from([("FIXTURE_PREC__Z".to_owned(), "from_dotenv".to_owned())]);
            assert_eq!(env_var_from("FIXTURE_PREC__Z", &map), None);
            Ok(())
        });
    }

    // A6: `Environment::init` merges the cascade into `std::env`, which used to
    // make every committed `.env` value indistinguishable from a deployment
    // one — so a `for_root` pin lost to `.env`, inverting the documented tier
    // in every scaffolded app. `get_from_deployment` must subtract what the
    // cascade published.
    #[test]
    fn deployment_env_var_ignores_values_published_from_the_cascade() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "FIXTURE_DEPLOY__FROM_FILE=file\nFIXTURE_DEPLOY__FROM_REAL=file",
            )?;
            jail.set_env("FIXTURE_DEPLOY__FROM_REAL", "real");
            crate::dotenv::load_cascade(std::path::Path::new("."), crate::Environment::Development);

            assert_eq!(
                deployment_env_var("FIXTURE_DEPLOY__FROM_FILE"),
                None,
                "a committed `.env` must lose to a value pinned in `for_root`",
            );
            assert_eq!(
                deployment_env_var("FIXTURE_DEPLOY__FROM_REAL").as_deref(),
                Some("real"),
                "a real deployment variable still outranks the pin",
            );
            // The plain read is unaffected — both tiers still resolve there.
            assert_eq!(
                EnvSource.get("FIXTURE_DEPLOY__FROM_FILE").as_deref(),
                Some("file"),
            );
            Ok(())
        });
    }

    #[test]
    fn env_var_read_never_writes_the_dotenv_value_into_the_process_env() {
        figment::Jail::expect_with(|_| {
            let map = HashMap::from([("FIXTURE_PREC__ONLY_IN_MAP".to_owned(), "v".to_owned())]);
            assert_eq!(
                env_var_from("FIXTURE_PREC__ONLY_IN_MAP", &map).as_deref(),
                Some("v"),
            );
            // The read resolved from the map, not by merging into the env.
            assert!(std::env::var("FIXTURE_PREC__ONLY_IN_MAP").is_err());
            Ok(())
        });
    }
}
