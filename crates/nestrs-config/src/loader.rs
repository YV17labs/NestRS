//! Framework env-var scheme `NESTRS_<DOMAIN>__<KEY>` and the typed
//! [`ConfigService`] reader handed to a config's `from_env`.
//!
//! Domain = owning crate's name with the `nestrs-` prefix stripped. A crate
//! maps **its own** domain; sibling vars may only be borrowed via an
//! **explicit fallback** in a `from_env` (own > borrowed > code default), since
//! the `.env` cascade is merged once before any `from_env` runs.

use std::env;
use std::str::FromStr;

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

/// Typed reader bound to one namespace; resolves `NESTRS_<NAMESPACE>__<KEY>`.
pub struct ConfigService {
    namespace: String,
}

impl ConfigService {
    pub fn for_namespace(namespace: &str) -> Self {
        dotenv::ensure_env_loaded();
        Self {
            namespace: namespace.to_ascii_uppercase(),
        }
    }

    pub fn var(&self, key: &str) -> String {
        format!("{PREFIX}{}__{}", self.namespace, key.to_ascii_uppercase())
    }

    pub fn get(&self, key: &str) -> Option<String> {
        env_var(&self.var(key))
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
}
