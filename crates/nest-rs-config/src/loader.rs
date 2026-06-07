//! Framework env-var scheme `NESTRS_<DOMAIN>__<KEY>` and the typed
//! [`ConfigService`] reader handed to a config's `from_env`.
//!
//! Domain = owning crate's name with the `nestrs-` prefix stripped. A crate
//! maps **its own** domain; sibling vars may only be borrowed via an
//! **explicit fallback** in a `from_env` (own > borrowed > code default), since
//! the `.env` cascade is merged once before any `from_env` runs.
//!
//! The reader's backing store is a [`ConfigSource`] — [`EnvSource`] is the
//! default (process env + `.env` cascade). A third-party crate can ship an
//! alternative source (Vault, K8s ConfigMap, AWS Parameter Store, …) by
//! implementing [`ConfigSource`] and constructing
//! [`ConfigService::with_source`].

use std::env;
use std::str::FromStr;
use std::sync::Arc;

use crate::dotenv;
use crate::error::ConfigError;

const PREFIX: &str = "NESTRS_";

/// Empty strings count as unset, so `FOO=` in a `.env` does not blank an
/// in-code default.
pub fn env_var(name: &str) -> Option<String> {
    match env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Where a [`ConfigService`] reads raw values from. The default is
/// [`EnvSource`] (process env + `.env` cascade); a third-party crate can ship
/// an alternative (Vault, K8s ConfigMap, AWS Parameter Store) by implementing
/// this trait and passing an instance to [`ConfigService::with_source`].
///
/// Sync on purpose: `Config::from_env` runs sync at boot. A remote source
/// pre-fetches into an in-memory map and serves `get` from that map.
pub trait ConfigSource: Send + Sync + 'static {
    /// Return the raw value for the fully-qualified variable name (e.g.
    /// `"NESTRS_DATABASE__URL"`). Empty strings should be treated as unset.
    fn get(&self, var: &str) -> Option<String>;
}

/// Default [`ConfigSource`] — reads from the process environment after the
/// `.env` cascade has been merged. The merge runs on the **first** call to
/// [`EnvSource::get`] (guarded by a `Once`), so a [`ConfigService`] built on a
/// custom [`ConfigSource`] never triggers it and the process env stays
/// untouched.
#[derive(Default)]
pub struct EnvSource;

impl ConfigSource for EnvSource {
    fn get(&self, var: &str) -> Option<String> {
        dotenv::ensure_env_loaded();
        env_var(var)
    }
}

/// Typed reader bound to one namespace; resolves `NESTRS_<NAMESPACE>__<KEY>`.
pub struct ConfigService {
    namespace: String,
    source: Arc<dyn ConfigSource>,
}

impl ConfigService {
    pub fn for_namespace(namespace: &str) -> Self {
        Self::with_source(namespace, Arc::new(EnvSource))
    }

    /// Build a reader backed by a custom [`ConfigSource`]. The `.env` cascade
    /// is **not** merged — the source is the sole authority for resolution,
    /// and the process env stays untouched (no global side effect from
    /// constructing this reader).
    pub fn with_source(namespace: &str, source: Arc<dyn ConfigSource>) -> Self {
        Self {
            namespace: namespace.to_ascii_uppercase(),
            source,
        }
    }

    pub fn var(&self, key: &str) -> String {
        format!("{PREFIX}{}__{}", self.namespace, key.to_ascii_uppercase())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.source.get(&self.var(key))
    }

    /// `Err` (naming the variable) when set-but-unparseable — boot-fatal, no
    /// silent fallback.
    pub fn parse<T>(&self, key: &str) -> Result<Option<T>, ConfigError>
    where
        T: FromStr,
        T::Err: std::fmt::Display,
    {
        match self.get(key) {
            None => Ok(None),
            Some(raw) => raw
                .parse::<T>()
                .map(Some)
                .map_err(|e| ConfigError::parse(self.var(key), e.to_string())),
        }
    }

    /// `1`/`true`/`yes`/`on` and their negatives, case-insensitive.
    pub fn flag(&self, key: &str, default: bool) -> Result<bool, ConfigError> {
        match self.get(key) {
            None => Ok(default),
            Some(raw) => match raw.trim().to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => Ok(true),
                "0" | "false" | "no" | "off" => Ok(false),
                other => Err(ConfigError::parse(
                    self.var(key),
                    format!("expected a boolean, got `{other}`"),
                )),
            },
        }
    }

    /// Comma-separated, trimmed, empties dropped.
    pub fn list(&self, key: &str) -> Vec<String> {
        self.get(key)
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
// figment::Jail's fixed closure signature triggers this lint unactionably.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    #[test]
    fn var_builds_the_namespaced_name() {
        let env = ConfigService::for_namespace("database");
        assert_eq!(env.var("URL"), "NESTRS_DATABASE__URL");
        assert_eq!(
            env.var("max_connections"),
            "NESTRS_DATABASE__MAX_CONNECTIONS"
        );
    }

    #[test]
    fn parse_reports_the_variable_on_failure() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTDB__MAX", "not-a-number");
            let env = ConfigService::for_namespace("testdb");
            let err = env.parse::<u32>("MAX").expect_err("non-numeric must fail");
            assert!(
                matches!(err, ConfigError::Parse { ref var, .. } if var == "NESTRS_TESTDB__MAX")
            );
            Ok(())
        });
    }

    #[test]
    fn parse_is_none_when_unset() {
        figment::Jail::expect_with(|_| {
            let env = ConfigService::for_namespace("testdb");
            assert!(
                env.parse::<u32>("UNSET_KEY")
                    .expect("unset is Ok(None)")
                    .is_none()
            );
            Ok(())
        });
    }

    #[test]
    fn flag_reads_common_spellings() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTF__ON", "yes");
            jail.set_env("NESTRS_TESTF__OFF", "false");
            let env = ConfigService::for_namespace("testf");
            assert!(env.flag("ON", false).unwrap());
            assert!(!env.flag("OFF", true).unwrap());
            assert!(env.flag("MISSING", true).unwrap());
            Ok(())
        });
    }

    #[test]
    fn list_splits_on_commas() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("NESTRS_TESTL__SCOPES", "read:user, write , ,admin");
            let env = ConfigService::for_namespace("testl");
            assert_eq!(env.list("SCOPES"), vec!["read:user", "write", "admin"]);
            Ok(())
        });
    }

    // A `with_source` reader bypasses the env entirely — pin that the source
    // is the sole authority so a third-party Vault/ConfigMap impl is not
    // shadowed by stale process env.
    #[test]
    fn with_source_reads_from_the_custom_source_only() {
        use std::collections::HashMap;
        struct Map(HashMap<&'static str, &'static str>);
        impl ConfigSource for Map {
            fn get(&self, var: &str) -> Option<String> {
                self.0.get(var).map(|s| (*s).to_owned())
            }
        }
        let source = Arc::new(Map(HashMap::from([(
            "NESTRS_CUSTOM__URL",
            "value-from-map",
        )])));
        let env = ConfigService::with_source("custom", source);
        assert_eq!(env.get("URL").as_deref(), Some("value-from-map"));
        assert!(env.get("MISSING").is_none());
    }

    // The dotenv cascade used to fire from `for_namespace`, which meant any
    // `ConfigService` — including one built on a custom source — would
    // permanently merge `.env` into the process env. Pin that a non-env
    // source never triggers the merge: `.env` exists in the jail with a
    // marker, and after a `with_source` read, that marker must still be
    // unset in `std::env`.
    #[test]
    fn with_source_does_not_load_dotenv_into_process_env() {
        struct Empty;
        impl ConfigSource for Empty {
            fn get(&self, _var: &str) -> Option<String> {
                None
            }
        }
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                ".env",
                "NESTRS_LEAK_GUARD__SHOULD_STAY_UNSET=loaded-from-dotenv",
            )?;
            // Build + use the custom-source reader. If dotenv leaked here it
            // would set the marker in the jailed process env.
            let env = ConfigService::with_source("leakguard", Arc::new(Empty));
            assert!(env.get("ANYTHING").is_none());
            assert!(
                std::env::var("NESTRS_LEAK_GUARD__SHOULD_STAY_UNSET").is_err(),
                "custom-source path must not merge .env into the process env",
            );
            Ok(())
        });
    }
}
