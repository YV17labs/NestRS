//! [`ConfigSource`] — pluggable backing store for [`ConfigService`].
//!
//! [`EnvSource`] (default) reads from the process environment after the `.env`
//! cascade has been merged. A third-party crate can ship an alternative
//! (Vault, K8s ConfigMap, AWS Parameter Store) by implementing [`ConfigSource`]
//! and constructing [`ConfigService::with_source`].
//!
//! Sync on purpose: `Config::from_env` runs sync at boot. A remote source
//! pre-fetches into an in-memory map and serves `get` from that map.
//!
//! [`ConfigService`]: crate::ConfigService
//! [`ConfigService::with_source`]: crate::ConfigService::with_source

use std::env;

use crate::dotenv;

/// Empty strings count as unset, so `FOO=` in a `.env` does not blank an
/// in-code default.
pub fn env_var(name: &str) -> Option<String> {
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

/// Default [`ConfigSource`] — reads from the process environment after the
/// `.env` cascade has been merged. The merge runs on the **first** call to
/// [`EnvSource::get`] (guarded by a `Once`), so a
/// [`ConfigService`](crate::ConfigService) built on a custom [`ConfigSource`]
/// never triggers it and the process env stays untouched.
#[derive(Default)]
pub struct EnvSource;

impl ConfigSource for EnvSource {
    fn get(&self, var: &str) -> Option<String> {
        dotenv::ensure_env_loaded();
        env_var(var)
    }
}
