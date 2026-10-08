//! [`WsConfig`] — WebSocket transport options resolved at boot.
//!
//! [`max_connection`](WsConfig::max_connection) is a **security** control: a
//! connection captures its principal and ability once at the upgrade, where `exp`
//! is checked, so the ceiling bounds how long a revoked or expired credential
//! keeps a live socket's privileges.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, DurationBounds, Result, config};

/// Default socket-lifetime ceiling: 4 hours.
const DEFAULT_MAX_CONNECTION_SECS: u64 = 4 * 60 * 60;

/// The socket-lifetime ceiling's range and the variable that sets it.
const MAX_CONNECTION: DurationBounds = DurationBounds::secs(
    "MAX_CONNECTION_SECS",
    "WsConfig::max_connection",
    nest_rs_http::MAX_CONNECTION_FLOOR,
    nest_rs_http::MAX_CONNECTION_CEILING,
);

/// Default per-message byte cap: 64 KiB, against tungstenite's own 64 MiB.
pub(crate) const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// WebSocket transport options resolved at boot (namespace `ws`).
#[config(namespace = "ws")]
#[derive(Clone, Debug)]
pub struct WsConfig {
    /// Maximum lifetime of a single WebSocket connection; the peer must then
    /// re-upgrade, re-running authn/authz. `None` ⇒ unlimited. Read from
    /// `<PREFIX>_WS__MAX_CONNECTION_SECS`, whole seconds from 1 to 86400 or `0`
    /// for unlimited; defaults to 4 hours.
    pub max_connection: Option<Duration>,
    /// Maximum bytes accepted for a single inbound message, enforced at the
    /// protocol layer so a giant frame is refused before it is buffered. Read
    /// from `<PREFIX>_WS__MAX_MESSAGE_BYTES`; defaults to 64 KiB.
    #[validate(range(min = 1, message = "must be at least 1 byte"))]
    pub max_message_bytes: usize,
}

impl Default for WsConfig {
    fn default() -> Self {
        Self {
            max_connection: Some(Duration::from_secs(DEFAULT_MAX_CONNECTION_SECS)),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

impl WsConfig {
    /// Pin the socket-lifetime ceiling in code.
    pub fn with_max_connection(mut self, ttl: Duration) -> Self {
        self.max_connection = Some(ttl);
        self
    }
}

impl Config for WsConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        let max_connection = MAX_CONNECTION
            .read_optional(env, base.max_connection)?
            .map(|read| read.value);
        let max_message_bytes = env
            .parse::<usize>("MAX_MESSAGE_BYTES")?
            .unwrap_or(base.max_message_bytes);
        Ok(Self {
            max_connection,
            max_message_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = WsConfig {
            max_connection: Some(Duration::from_secs(60)),
            max_message_bytes: 999,
        };
        let cfg = WsConfig::from_env(
            &ConfigService::with_vars("ws", [("MAX_MESSAGE_BYTES", "2048")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.max_message_bytes, 2048, "the env outranks the pin");
        assert_eq!(
            cfg.max_connection,
            Some(Duration::from_secs(60)),
            "and the field the env is silent about keeps the pin",
        );
    }

    #[test]
    fn default_bounds_the_socket_to_four_hours() {
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
    fn from_env_falls_back_to_the_default_when_unset() {
        let cfg = WsConfig::from_env(&ConfigService::with_vars("ws", []), Default::default())
            .expect("ok");
        assert_eq!(cfg.max_connection, Some(Duration::from_secs(4 * 60 * 60)));
    }

    #[test]
    fn from_env_reads_a_custom_ceiling_in_seconds() {
        let cfg = WsConfig::from_env(
            &ConfigService::with_vars("ws", [("MAX_CONNECTION_SECS", "900")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.max_connection, Some(Duration::from_secs(900)));
    }

    #[test]
    fn from_env_treats_zero_as_unlimited() {
        let cfg = WsConfig::from_env(
            &ConfigService::with_vars("ws", [("MAX_CONNECTION_SECS", "0")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.max_connection, None, "0 is the unlimited sentinel");
    }

    #[test]
    fn default_message_cap_is_64_kib() {
        assert_eq!(WsConfig::default().max_message_bytes, 64 * 1024);
    }

    #[test]
    fn from_env_reads_a_custom_message_cap() {
        let cfg = WsConfig::from_env(
            &ConfigService::with_vars("ws", [("MAX_MESSAGE_BYTES", "1048576")]),
            Default::default(),
        )
        .expect("ok");
        assert_eq!(cfg.max_message_bytes, 1_048_576);
    }

    #[test]
    fn from_env_rejects_an_unparseable_ceiling() {
        assert!(
            WsConfig::from_env(
                &ConfigService::with_vars("ws", [("MAX_CONNECTION_SECS", "forever")]),
                Default::default()
            )
            .is_err(),
            "non-numeric must surface as a boot error — no silent default",
        );
    }

    #[test]
    fn a_ceiling_outside_its_range_is_refused_from_either_side() {
        let var = nest_rs_config::var_name("ws", "MAX_CONNECTION_SECS");
        let from_env = WsConfig::from_env(
            &ConfigService::with_vars("ws", [("MAX_CONNECTION_SECS", "86401")]),
            Default::default(),
        )
        .expect_err("past a day")
        .to_string();
        assert!(
            from_env.contains(&var)
                && from_env.contains("must be at most 86400 seconds, or 0 to turn it off"),
            "{from_env}"
        );
        for pinned in [Duration::MAX, Duration::ZERO] {
            let refused = WsConfig::from_env(
                &ConfigService::with_vars("ws", []),
                WsConfig::default().with_max_connection(pinned),
            )
            .expect_err("a pinned ceiling outside the range")
            .to_string();
            assert!(
                refused.contains(&var) && refused.contains("`None` turns it off"),
                "{refused}"
            );
        }
    }
}
