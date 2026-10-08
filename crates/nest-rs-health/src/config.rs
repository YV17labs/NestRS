//! [`HealthConfig`] — the two ceilings that bound a probe response.
//!
//! Milliseconds, unlike the framework's `*_SECS` keys: the kubelet's
//! `timeoutSeconds` defaults to 1, so the only interval that matters is inside
//! a second. `0` is refused rather than read as *off* ([`Floor::UnitsOrOff`]):
//! an unbounded probe outlives the kubelet's deadline, a zero one fails every
//! poll.

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};

/// Per-indicator ceiling: under the probe deadline, so a slow indicator is
/// reported by name before the probe-wide deadline sweeps it up.
const DEFAULT_INDICATOR_TIMEOUT_MS: u64 = 750;

/// Probe-wide ceiling: inside Kubernetes' 1 s `timeoutSeconds` default, the
/// remainder left to the connection and the response.
const DEFAULT_PROBE_DEADLINE_MS: u64 = 900;

const NOT_ZERO: &str = "an unbounded probe outlives the kubelet's deadline and a zero one fails \
                        every probe on the first poll, so neither is a ceiling";

const PAST_A_MINUTE: Bound = Bound {
    count: 60_000,
    why: "a probe answered later than a minute has been scored a failure already — the kubelet \
          sends the next one every `periodSeconds`, ten by default — so a ceiling that long \
          bounds nothing an orchestrator waits for",
};

const INDICATOR_TIMEOUT: DurationBounds = DurationBounds::millis(
    "INDICATOR_TIMEOUT_MS",
    "HealthConfig::indicator_timeout_ms",
    Floor::Units(Bound {
        count: 1,
        why: NOT_ZERO,
    }),
    PAST_A_MINUTE,
);

const PROBE_DEADLINE: DurationBounds = DurationBounds::millis(
    "PROBE_DEADLINE_MS",
    "HealthConfig::probe_deadline_ms",
    Floor::Units(Bound {
        count: 1,
        why: NOT_ZERO,
    }),
    PAST_A_MINUTE,
);

/// Health probe options resolved at boot (namespace `health`).
#[config(namespace = "health")]
#[derive(Clone, Debug)]
pub struct HealthConfig {
    /// Wall-clock ceiling on **one** indicator, past which it reports `down`.
    /// Read from `<PREFIX>_HEALTH__INDICATOR_TIMEOUT_MS`, from 1 to 60000; defaults
    /// to 750 ms. At or above [`probe_deadline_ms`](Self::probe_deadline_ms) it
    /// never fires, and the per-indicator diagnostic is lost.
    pub indicator_timeout_ms: u64,
    /// Wall-clock ceiling on the **whole** probe response; indicators that have
    /// not answered by then are `down`. Read from
    /// `<PREFIX>_HEALTH__PROBE_DEADLINE_MS`, from 1 to 60000; defaults to 900 ms.
    pub probe_deadline_ms: u64,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            indicator_timeout_ms: DEFAULT_INDICATOR_TIMEOUT_MS,
            probe_deadline_ms: DEFAULT_PROBE_DEADLINE_MS,
        }
    }
}

impl HealthConfig {
    /// Pin the per-indicator ceiling in code.
    pub fn with_indicator_timeout(mut self, ceiling: Duration) -> Self {
        self.indicator_timeout_ms = as_millis(ceiling);
        self
    }

    /// Pin the probe-wide deadline in code.
    pub fn with_probe_deadline(mut self, deadline: Duration) -> Self {
        self.probe_deadline_ms = as_millis(deadline);
        self
    }

    /// [`indicator_timeout_ms`](Self::indicator_timeout_ms) as a [`Duration`].
    pub fn indicator_timeout(&self) -> Duration {
        Duration::from_millis(self.indicator_timeout_ms)
    }

    /// [`probe_deadline_ms`](Self::probe_deadline_ms) as a [`Duration`].
    pub fn probe_deadline(&self) -> Duration {
        Duration::from_millis(self.probe_deadline_ms)
    }
}

/// Whole milliseconds, saturating.
fn as_millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

impl Config for HealthConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            indicator_timeout_ms: as_millis(
                INDICATOR_TIMEOUT.read(env, base.indicator_timeout())?.value,
            ),
            probe_deadline_ms: as_millis(PROBE_DEADLINE.read(env, base.probe_deadline())?.value),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_fit_inside_the_kubelet_default_deadline() {
        let cfg = HealthConfig::default();
        assert!(cfg.probe_deadline() < Duration::from_secs(1));
        assert!(
            cfg.indicator_timeout() < cfg.probe_deadline(),
            "the per-indicator ceiling fires first, so the warn names which one hung",
        );
    }

    #[test]
    fn env_overrides_each_field_of_a_pinned_config() {
        let pinned = HealthConfig::default()
            .with_indicator_timeout(Duration::from_millis(120))
            .with_probe_deadline(Duration::from_millis(300));
        let cfg = HealthConfig::from_env(
            &ConfigService::with_vars("health", [("PROBE_DEADLINE_MS", "450")]),
            pinned,
        )
        .expect("the overlay resolves");
        assert_eq!(cfg.probe_deadline(), Duration::from_millis(450));
        assert_eq!(
            cfg.indicator_timeout(),
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
        assert_eq!(cfg.indicator_timeout_ms, defaults.indicator_timeout_ms);
        assert_eq!(cfg.probe_deadline_ms, defaults.probe_deadline_ms);
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
                    indicator_timeout_ms: 0,
                    ..Default::default()
                },
                "INDICATOR_TIMEOUT_MS",
            ),
            (
                HealthConfig {
                    probe_deadline_ms: 0,
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
            HealthConfig::default().with_probe_deadline(Duration::MAX),
        )
        .expect_err("a pinned deadline past a minute")
        .to_string();
        assert!(
            pinned.contains("above the 60s it must be at most"),
            "{pinned}"
        );
    }
}
