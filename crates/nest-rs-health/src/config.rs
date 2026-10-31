//! [`HealthConfig`] — the two ceilings that bound a probe response.
//!
//! Milliseconds, unlike the framework's `*_SECS` keys: the kubelet's
//! `timeoutSeconds` defaults to 1, so the only interval that matters is inside
//! a second. Each is a `Duration`, so `0` is refused rather than read as
//! *off*: an unbounded probe outlives the kubelet's deadline, a zero one fails
//! every poll.

use std::time::Duration;

use nest_rs_config::{Bound, DurationBounds, Floor, config};

/// Per-indicator ceiling: under the probe deadline, so a slow indicator is
/// reported by name before the probe-wide deadline sweeps it up.
const DEFAULT_INDICATOR_TIMEOUT: Duration = Duration::from_millis(750);

/// Probe-wide ceiling: inside Kubernetes' 1 s `timeoutSeconds` default, the
/// remainder left to the connection and the response.
const DEFAULT_PROBE_DEADLINE: Duration = Duration::from_millis(900);

const NOT_ZERO: &str = "an unbounded probe outlives the kubelet's deadline and a zero one fails \
                        every probe on the first poll, so neither is a ceiling";

const PAST_A_MINUTE: Bound = Bound::new(
    60_000,
    "a probe answered later than a minute has been scored a failure already — the kubelet \
     sends the next one every `periodSeconds`, ten by default — so a ceiling that long \
     bounds nothing an orchestrator waits for",
);

const INDICATOR_TIMEOUT: DurationBounds =
    DurationBounds::millis(Floor::Units(Bound::new(1, NOT_ZERO)), PAST_A_MINUTE);

const PROBE_DEADLINE: DurationBounds =
    DurationBounds::millis(Floor::Units(Bound::new(1, NOT_ZERO)), PAST_A_MINUTE);

/// Health probe options resolved at boot (namespace `health`).
#[config(namespace = "health")]
#[derive(Clone, Debug)]
pub struct HealthConfig {
    /// Wall-clock ceiling on **one** indicator, past which it reports `down`.
    /// Read from `<PREFIX>_HEALTH__INDICATOR_TIMEOUT_MS`, from 1 to 60000; defaults
    /// to 750 ms. At or above [`probe_deadline`](Self::probe_deadline) it
    /// never fires, and the per-indicator diagnostic is lost.
    #[config(bounds = INDICATOR_TIMEOUT)]
    pub indicator_timeout: Duration,
    /// Wall-clock ceiling on the **whole** probe response; indicators that have
    /// not answered by then are `down`. Read from
    /// `<PREFIX>_HEALTH__PROBE_DEADLINE_MS`, from 1 to 60000; defaults to 900 ms.
    #[config(bounds = PROBE_DEADLINE)]
    pub probe_deadline: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            indicator_timeout: DEFAULT_INDICATOR_TIMEOUT,
            probe_deadline: DEFAULT_PROBE_DEADLINE,
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::{Config, ConfigService};

    use super::*;

    #[test]
    fn defaults_fit_inside_the_kubelet_default_deadline() {
        let cfg = HealthConfig::default();
        assert!(cfg.probe_deadline < Duration::from_secs(1));
        assert!(
            cfg.indicator_timeout < cfg.probe_deadline,
            "the per-indicator ceiling fires first, so the warn names which one hung",
        );
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = HealthConfig {
            indicator_timeout: Duration::from_millis(120),
            probe_deadline: Duration::from_millis(300),
        };
        let cfg = HealthConfig::from_env(
            &ConfigService::with_vars("health", [("PROBE_DEADLINE_MS", "450")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.probe_deadline, Duration::from_millis(450));
        assert_eq!(
            cfg.indicator_timeout,
            Duration::from_millis(120),
            "the field the env is silent about keeps the pin",
        );
    }

    #[test]
    fn from_env_falls_back_to_the_defaults_when_unset() {
        let cfg =
            HealthConfig::from_env(&ConfigService::with_vars("health", []), Default::default())
                .expect("ok");
        let defaults = HealthConfig::default();
        assert_eq!(cfg.indicator_timeout, defaults.indicator_timeout);
        assert_eq!(cfg.probe_deadline, defaults.probe_deadline);
    }

    #[test]
    fn from_env_rejects_an_unparseable_ceiling() {
        assert!(
            HealthConfig::from_env(
                &ConfigService::with_vars("health", [("INDICATOR_TIMEOUT_MS", "soon")]),
                Default::default()
            )
            .is_err(),
            "non-numeric must surface as a boot error — no silent default",
        );
    }

    #[test]
    fn zero_is_refused_on_both_ceilings() {
        for (zeroed, key) in [
            (
                HealthConfig {
                    indicator_timeout: Duration::ZERO,
                    ..Default::default()
                },
                "INDICATOR_TIMEOUT_MS",
            ),
            (
                HealthConfig {
                    probe_deadline: Duration::ZERO,
                    ..Default::default()
                },
                "PROBE_DEADLINE_MS",
            ),
        ] {
            let pinned = HealthConfig::from_env(&ConfigService::with_vars("health", []), zeroed)
                .expect_err("a pinned zero fails the boot")
                .to_string();
            assert!(
                pinned.contains(&nest_rs_config::var_name("health", key))
                    && pinned.contains("set in code is 0ns"),
                "{pinned}"
            );
            let from_env = HealthConfig::from_env(
                &ConfigService::with_vars("health", [(key, "0")]),
                HealthConfig::default(),
            )
            .expect_err("a zero from the environment fails the boot")
            .to_string();
            assert!(
                from_env.contains("must be at least 1 millisecond"),
                "{from_env}"
            );
        }
    }

    #[test]
    fn a_ceiling_past_a_minute_is_refused_on_both() {
        for key in ["INDICATOR_TIMEOUT_MS", "PROBE_DEADLINE_MS"] {
            let from_env = HealthConfig::from_env(
                &ConfigService::with_vars("health", [(key, "60001")]),
                HealthConfig::default(),
            )
            .expect_err("past a minute")
            .to_string();
            assert!(
                from_env.contains(&nest_rs_config::var_name("health", key))
                    && from_env.contains("must be at most 60000 milliseconds"),
                "{from_env}"
            );
        }
        let pinned = HealthConfig::from_env(
            &ConfigService::with_vars("health", []),
            HealthConfig {
                probe_deadline: Duration::MAX,
                ..HealthConfig::default()
            },
        )
        .expect_err("a pinned deadline past a minute")
        .to_string();
        assert!(
            pinned.contains("above the 60s it must be at most"),
            "{pinned}"
        );
    }
}
