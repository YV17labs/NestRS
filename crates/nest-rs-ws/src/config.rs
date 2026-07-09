//! [`WsConfig`] — WebSocket transport options resolved at boot.
//!
//! Today a single field, [`max_connection`](WsConfig::max_connection): a ceiling
//! on how long one socket may stay open. It is a **security** control, not a
//! resource knob. A WS connection captures its principal/ability **once** at the
//! upgrade and replays them for every message; `exp` is checked only at that
//! upgrade. Without a ceiling a socket therefore keeps its privileges after the
//! bearer token has expired, the user has logged out, or the grant was revoked —
//! until the peer happens to disconnect. The ceiling bounds that stale-privilege
//! window: the server closes the socket when it elapses, forcing a fresh upgrade
//! (and with it a fresh authn/authz check).
//!
//! Dual-path like every `nest-rs-*` config: settable via `NESTRS_WS__*` env vars
//! (`NESTRS_WS__MAX_CONNECTION_SECS`) **and** the pinned struct passed to
//! [`WsModule::for_root`](crate::WsModule::for_root). `0` (env) / `None` (struct)
//! means **unlimited** — the pre-ceiling behaviour, kept opt-in preservable.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, Result, config};
use validator::Validate;

/// Default socket-lifetime ceiling: 4 hours. Long enough not to disrupt a normal
/// interactive session, short enough to bound how long a revoked or expired
/// credential keeps a live socket's privileges.
const DEFAULT_MAX_CONNECTION_SECS: u64 = 4 * 60 * 60;

#[config(namespace = "ws")]
#[derive(Clone, Debug, Validate)]
pub struct WsConfig {
    /// Maximum lifetime of a single WebSocket connection. When it elapses the
    /// server closes the socket through the normal disconnect path, so the peer
    /// must re-upgrade — re-running authn/authz and re-checking token `exp`.
    /// `None` ⇒ unlimited (the pre-ceiling behaviour). Read from
    /// `NESTRS_WS__MAX_CONNECTION_SECS` (whole seconds; `0` ⇒ unlimited);
    /// defaults to 4 hours.
    pub max_connection: Option<Duration>,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            max_connection: Some(Duration::from_secs(DEFAULT_MAX_CONNECTION_SECS)),
        }
    }
}

impl WsConfig {
    /// Pin the socket-lifetime ceiling in code.
    pub fn with_max_connection(mut self, ttl: Duration) -> Self {
        self.max_connection = Some(ttl);
        self
    }

    /// Remove the socket-lifetime ceiling (unlimited) — restores the
    /// pre-ceiling behaviour explicitly.
    pub fn unlimited(mut self) -> Self {
        self.max_connection = None;
        self
    }
}

impl Config for WsConfig {
    fn from_env(env: &ConfigService) -> Result<Self> {
        // `0` is the "unlimited" sentinel; unset falls back to the default
        // ceiling; a set-but-unparseable value surfaces as a boot error.
        let max_connection = match env.parse::<u64>("MAX_CONNECTION_SECS")? {
            None => Some(Duration::from_secs(DEFAULT_MAX_CONNECTION_SECS)),
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
        };
        Ok(Self { max_connection })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bounds_the_socket_to_four_hours() {
        // A bounded default is deliberate: the stale-privilege window is capped
        // unless an operator opts back into unlimited.
        assert_eq!(
            WsConfig::default().max_connection,
            Some(Duration::from_secs(4 * 60 * 60)),
        );
    }

    #[test]
    fn with_max_connection_pins_the_ceiling_in_code() {
        let cfg = WsConfig::default().with_max_connection(Duration::from_secs(600));
        assert_eq!(cfg.max_connection, Some(Duration::from_secs(600)));
    }

    #[test]
    fn unlimited_clears_the_ceiling() {
        assert_eq!(WsConfig::default().unlimited().max_connection, None);
    }

    #[test]
    fn from_env_falls_back_to_the_default_when_unset() {
        let cfg = WsConfig::from_env(&ConfigService::with_vars("ws", [])).expect("ok");
        assert_eq!(cfg.max_connection, Some(Duration::from_secs(4 * 60 * 60)));
    }

    #[test]
    fn from_env_reads_a_custom_ceiling_in_seconds() {
        let cfg = WsConfig::from_env(&ConfigService::with_vars(
            "ws",
            [("NESTRS_WS__MAX_CONNECTION_SECS", "900")],
        ))
        .expect("ok");
        assert_eq!(cfg.max_connection, Some(Duration::from_secs(900)));
    }

    #[test]
    fn from_env_treats_zero_as_unlimited() {
        let cfg = WsConfig::from_env(&ConfigService::with_vars(
            "ws",
            [("NESTRS_WS__MAX_CONNECTION_SECS", "0")],
        ))
        .expect("ok");
        assert_eq!(cfg.max_connection, None, "0 is the unlimited sentinel");
    }

    #[test]
    fn from_env_rejects_an_unparseable_ceiling() {
        assert!(
            WsConfig::from_env(&ConfigService::with_vars(
                "ws",
                [("NESTRS_WS__MAX_CONNECTION_SECS", "forever")]
            ))
            .is_err(),
            "non-numeric must surface as a boot error — no silent default",
        );
    }
}
