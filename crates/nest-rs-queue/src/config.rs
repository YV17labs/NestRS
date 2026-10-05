//! [`QueueConfig`] — the settings of the port's own transport, the
//! [`QueueWorker`](crate::QueueWorker). Namespace `queue`: the port runs the
//! consume loop on every backend, so its drain window survives a backend swap
//! (`.claude/decisions/port-config-namespace.md`).

use std::time::Duration;

use nest_rs_config::{Bound, Config, ConfigService, DurationBounds, Floor, Result, config};

/// Default drain window on shutdown: 20s, the HTTP transport's, for the same
/// arithmetic. The way down is the transports' windows, then the shutdown hooks'
/// five seconds, then the telemetry flush's three, and it fits the 30 seconds
/// Kubernetes gives a pod between `SIGTERM` and `SIGKILL` by default;
/// `nest-rs-testing` pins the sum.
const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 20;

/// The drain window's bounds, the variable that sets it, and why.
const SHUTDOWN_TIMEOUT: DurationBounds = DurationBounds::secs(
    "SHUTDOWN_TIMEOUT_SECS",
    "QueueConfig::shutdown_timeout",
    Floor::Units(Bound {
        count: 1,
        why: "the drain keeps up to half its window to hand interrupted jobs back, and with \
              none a job running at shutdown waits out its lease before another worker runs it",
    }),
    Bound {
        count: 60 * 60,
        why: "a stopping replica holds its rollout for as long as it drains, and a job still \
              running an hour into a shutdown is one to hand back and resume from its \
              checkpoint, not to wait out",
    },
);

/// The queue worker's settings, set through `<PREFIX>_QUEUE__*` or pinned
/// through [`QueueModule::for_root`](crate::QueueModule::for_root).
#[config(namespace = "queue")]
#[derive(Clone, Debug)]
pub struct QueueConfig {
    /// How long the worker lets running attempts finish after a shutdown
    /// signal. An attempt still running when the window closes, less a reserve
    /// — the backend's answer bound, or five seconds when that is shorter, at
    /// most half the window — is cut and its job handed back to the queue for
    /// another replica inside the reserve. Read from
    /// `<PREFIX>_QUEUE__SHUTDOWN_TIMEOUT_SECS`, at least 1 and at most 3600;
    /// defaults to 20s.
    pub shutdown_timeout: Duration,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            shutdown_timeout: Duration::from_secs(DEFAULT_SHUTDOWN_TIMEOUT_SECS),
        }
    }
}

impl Config for QueueConfig {
    fn from_env(env: &ConfigService, base: Self) -> Result<Self> {
        Ok(Self {
            shutdown_timeout: SHUTDOWN_TIMEOUT.read(env, base.shutdown_timeout)?.value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_config::Namespaced;

    fn read(vars: &[(&str, &str)]) -> Result<QueueConfig> {
        QueueConfig::from_env(
            &ConfigService::with_vars("queue", vars.iter().copied()),
            QueueConfig::default(),
        )
    }

    #[test]
    fn the_namespace_is_the_ports_word() {
        assert_eq!(QueueConfig::NAMESPACE, "queue");
    }

    #[test]
    fn the_drain_window_defaults_to_20s_and_reads_the_env() {
        assert_eq!(
            read(&[]).expect("defaults").shutdown_timeout,
            Duration::from_secs(20)
        );
        assert_eq!(
            read(&[("SHUTDOWN_TIMEOUT_SECS", "45")])
                .expect("read")
                .shutdown_timeout,
            Duration::from_secs(45),
        );
    }

    #[test]
    fn a_window_outside_its_bounds_is_refused_naming_the_variable() {
        for refused in ["0", "3601"] {
            let error = read(&[("SHUTDOWN_TIMEOUT_SECS", refused)])
                .expect_err("out of bounds")
                .to_string();
            assert!(
                error.contains(&nest_rs_config::var_name("queue", "SHUTDOWN_TIMEOUT_SECS")),
                "{error}"
            );
        }
    }
}
