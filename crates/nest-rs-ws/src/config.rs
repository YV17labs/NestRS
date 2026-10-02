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
//! means **unlimited** — the pre-ceiling behaviour, kept opt-in preservable —
//! and a value is held to a second at least and a day at most, the range every
//! long-lived connection's ceiling shares.

use std::time::Duration;

use nest_rs_config::{Config, ConfigService, DurationBounds, DurationUnit, Result, config};

/// Default socket-lifetime ceiling: 4 hours. Long enough not to disrupt a normal
/// interactive session, short enough to bound how long a revoked or expired
/// credential keeps a live socket's privileges.
const DEFAULT_MAX_CONNECTION_SECS: u64 = 4 * 60 * 60;

/// The socket-lifetime ceiling's range — the one every long-lived connection's
/// is held to, [`MAX_CONNECTION_FLOOR`](nest_rs_http::MAX_CONNECTION_FLOOR) and
/// [`MAX_CONNECTION_CEILING`](nest_rs_http::MAX_CONNECTION_CEILING) — and the
/// variable that sets it.
const MAX_CONNECTION: DurationBounds = DurationBounds {
    key: "MAX_CONNECTION_SECS",
    field: "WsConfig::max_connection",
    unit: DurationUnit::Seconds,
    least: nest_rs_http::MAX_CONNECTION_FLOOR,
    most: nest_rs_http::MAX_CONNECTION_CEILING,
};

/// Default per-message byte cap: 64 KiB. Applied at the WebSocket *protocol*
/// layer so an oversize frame is refused while reading rather than after
/// tungstenite buffers it whole (its own default is 64 MiB — a ~1000×
/// amplification, WS-I1).
pub(crate) const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// WebSocket transport options resolved at boot (namespace `ws`). See the
/// module docs for why the socket-lifetime ceiling is a security control.
#[config(namespace = "ws")]
#[derive(Clone, Debug)]
pub struct WsConfig {
    /// Maximum lifetime of a single WebSocket connection. When it elapses the
    /// server closes the socket through the normal disconnect path, so the peer
    /// must re-upgrade — re-running authn/authz and re-checking token `exp`.
    /// `None` ⇒ unlimited (the pre-ceiling behaviour). Read from
    /// `NESTRS_WS__MAX_CONNECTION_SECS`, whole seconds from 1 to 86400 (a day)
    /// or `0` for unlimited — refused outside, from the environment and from the
    /// pinned struct alike; defaults to 4 hours.
    pub max_connection: Option<Duration>,
    /// Maximum bytes accepted for a single inbound message, enforced at the
    /// WebSocket protocol layer (both `max_message_size` and `max_frame_size`)
    /// so buffering is bounded *before* a giant frame is fully read (WS-I1).
    /// Read from `NESTRS_WS__MAX_MESSAGE_BYTES`; defaults to 64 KiB.
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

    // The dual-path rule is framework-wide, not an HTTP special case: a pinned
    // `WsConfig` still takes its overrides per field from `NESTRS_WS__*`.
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

    /// The ceiling has a range like every duration a deployment sets: past a
    /// day, or a pinned zero — which would close every socket as it opens — is
    /// refused naming the variable; `0` from the environment and `None` in code
    /// stay the off switch.
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
