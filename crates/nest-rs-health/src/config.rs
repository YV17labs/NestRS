//! [`HealthConfig`] — the two ceilings that bound a probe response.
//!
//! **The unit is milliseconds, and that is the kubelet's doing rather than a
//! deviation for its own sake.** Every other duration this framework configures
//! is a whole-second `*_SECS` key, because every other one bounds a connection
//! that lives for hours. A probe is bounded by `timeoutSeconds`, whose
//! Kubernetes default is **1** — the kubelet scores a probe that has not
//! answered by then as a *failure*, and on a liveness probe a failure repeated
//! `failureThreshold` times restarts the container. A grammar that cannot
//! express a value **inside** one second therefore cannot express the only
//! interval that matters here, so both ceilings are read in milliseconds.
//!
//! **`0` is not the unlimited sentinel here, and the refusal is the point.**
//! A connection ceiling reads `0` as *off*
//! ([`Floor::UnitsOrOff`]), which is right
//! where the ceiling bounds how long a connection may replay privileges it
//! authenticated with once: turning it off restores the pre-ceiling behaviour,
//! and the deployment that asks for that has asked for something coherent. Here *off* is the defect — an
//! unbounded probe outliving the kubelet's deadline is exactly what these two
//! fields exist to prevent — so a `0` cannot mean "no ceiling" without meaning
//! "restart loop", and it cannot mean "zero milliseconds" either, which would
//! fail every probe on the first poll. Both are read through
//! [`DurationBounds`] from one millisecond to a
//! minute, so `0` is a boot error naming the variable — from the environment or
//! pinned in code alike, in the sentence every bounded duration of the
//! framework is refused in.
//!
//! Dual-path like every `nest-rs-*` config: settable via `<PREFIX>_HEALTH__*` env
//! vars **and** via the pinned struct passed to
//! [`HealthModule::for_root`](crate::HealthModule::for_root), composing per
//! field.

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};

/// Per-indicator ceiling: 750 ms. Under the probe deadline by a margin, so the
/// common single-slow-indicator case is reported **by name** (`health indicator
/// timed out` carries which one) instead of being swept up by the probe-wide
/// deadline, which can only say how many did not answer.
const DEFAULT_INDICATOR_TIMEOUT_MS: u64 = 750;

/// Probe-wide ceiling: 900 ms — inside Kubernetes' own `timeoutSeconds` default
/// of 1 s, with the remainder left to the connection and the response. A
/// deployment that raised `timeoutSeconds` raises this to match; one that did
/// not gets a `503` it can read and log, rather than a kubelet timeout it
/// cannot.
const DEFAULT_PROBE_DEADLINE_MS: u64 = 900;

/// Why neither ceiling may be zero — the reason both refusals give.
const NOT_ZERO: &str = "an unbounded probe outlives the kubelet's deadline and a zero one fails \
                        every probe on the first poll, so neither is a ceiling";

/// Why neither ceiling may pass a minute — the reason both refusals give.
const PAST_A_MINUTE: Bound = Bound {
    count: 60_000,
    why: "a probe answered later than a minute has been scored a failure already — the kubelet \
          sends the next one every `periodSeconds`, ten by default — so a ceiling that long \
          bounds nothing an orchestrator waits for",
};

/// The per-indicator ceiling's range, the variable that sets it, and why.
const INDICATOR_TIMEOUT: DurationBounds = DurationBounds::millis(
    "INDICATOR_TIMEOUT_MS",
    "HealthConfig::indicator_timeout_ms",
    Floor::Units(Bound {
        count: 1,
        why: NOT_ZERO,
    }),
    PAST_A_MINUTE,
);

/// The probe deadline's range, the variable that sets it, and why.
const PROBE_DEADLINE: DurationBounds = DurationBounds::millis(
    "PROBE_DEADLINE_MS",
    "HealthConfig::probe_deadline_ms",
    Floor::Units(Bound {
        count: 1,
        why: NOT_ZERO,
    }),
    PAST_A_MINUTE,
);

/// Health probe options resolved at boot (namespace `health`). See the module
/// docs for why the unit is milliseconds and why `0` is refused.
#[config(namespace = "health")]
#[derive(Clone, Debug)]
pub struct HealthConfig {
    /// Wall-clock ceiling on **one** indicator. An indicator probing a dead
    /// peer (a hung TCP connect, a stalled query) reports `down` with an opaque
    /// reason at this point, and a `warn` on `nest_rs::health` names it.
    /// Read from `<PREFIX>_HEALTH__INDICATOR_TIMEOUT_MS`, from 1 to 60000; defaults
    /// to 750 ms.
    ///
    /// Indicators run concurrently, so this bounds the slowest one rather than
    /// the sum. Setting it at or above
    /// [`probe_deadline_ms`](Self::probe_deadline_ms) is legal and costs the
    /// per-indicator diagnostic: the probe deadline then always fires first.
    pub indicator_timeout_ms: u64,
    /// Wall-clock ceiling on the **whole** probe response, whatever the
    /// indicator count. Indicators that have answered by then are reported as
    /// they answered; the rest are `down` with an opaque reason, and the probe
    /// is `down` overall. Read from `<PREFIX>_HEALTH__PROBE_DEADLINE_MS`, from 1 to
    /// 60000; defaults to 900 ms.
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

/// Whole milliseconds, saturating. A `Duration` of ~584 million years is the
/// only input that clamps, and clamping it is still a ceiling — the alternative
/// on a hot path is a panic on a value nobody can type by accident.
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
        // Kubernetes' `timeoutSeconds` defaults to 1 and a timed-out httpGet
        // probe is a *failure*, so both ceilings must land under a second — the
        // probe deadline with room left for the connection and the body.
        let cfg = HealthConfig::default();
        assert!(cfg.probe_deadline() < Duration::from_secs(1));
        assert!(
            cfg.indicator_timeout() < cfg.probe_deadline(),
            "the per-indicator ceiling fires first, so the warn names which one hung",
        );
    }

    // The dual-path rule is framework-wide: a pinned `HealthConfig` still takes
    // its overrides per field from `<PREFIX>_HEALTH__*`.
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

    /// `0` is refused rather than read as the framework's usual unlimited
    /// sentinel: an unbounded probe is the defect these fields exist to
    /// prevent, and a zero-millisecond one fails every probe on the first poll.
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

    /// Every duration a deployment sets has a ceiling: past a minute either
    /// ceiling is refused naming its variable, from either side.
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
