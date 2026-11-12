//! [`OutboxConfig`] — the settings of the port's [`OutboxRelay`](crate::OutboxRelay).
//! Namespace `queue__outbox`: the relay is the port's on every backend, so its
//! settings survive a backend or a data driver swap
//! (`.claude/decisions/port-config-namespace.md`).

use std::time::Duration;

use nest_rs_config::{Bound, CountBounds, DurationBounds, Floor, config};

use crate::ENQUEUE_BATCH;

/// How often the relay looks for due records nothing woke it for, its bounds,
/// and why.
const SWEEP_INTERVAL: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        100,
        "a shorter sweep queries the store several times a second for records a commit \
         already wakes the relay for",
    )),
    Bound::new(
        60_000,
        "a record a commit's wake-up missed, or one a failed pass rescheduled, waits a \
         whole sweep to be filed",
    ),
);

/// When records waiting unfiled are an outage the relay says at `error`, its
/// bounds, and why.
const STALL_WARNING: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a record waits at least one pass, so a shorter warning fires on every write",
    )),
    Bound::new(
        24 * 60 * 60,
        "a backend refusing every job for a day has stopped serving; the table grows \
         until it is said",
    ),
);

/// How many records one pass claims: at most what one call to the backend
/// files.
const BATCH_SIZE: CountBounds = CountBounds::new(
    Bound::new(1, "a pass that claims nothing files nothing"),
    Bound::new(
        ENQUEUE_BATCH as u64,
        "a pass claims what one call to the backend files, holding it locked until it is \
         filed, and a claim the port could not hand the backend in one call holds its locks \
         for several",
    ),
);

/// The relay's settings, set through `<PREFIX>_QUEUE__OUTBOX__*` or pinned
/// through [`OutboxModule::for_root`](crate::OutboxModule::for_root).
#[config(namespace = "queue__outbox")]
#[derive(Clone, Debug)]
pub struct OutboxConfig {
    /// How long the relay waits for a commit to wake it before it looks for
    /// due records anyway — one a failed pass rescheduled, one another
    /// replica's unit wrote. Read from
    /// `<PREFIX>_QUEUE__OUTBOX__SWEEP_INTERVAL_MS`, at least 100 and at most
    /// 60000; defaults to 1s.
    #[config(bounds = SWEEP_INTERVAL)]
    pub sweep_interval: Duration,
    /// How many records one pass claims, holding them locked while it files
    /// them. Read from `<PREFIX>_QUEUE__OUTBOX__BATCH_SIZE`, at least 1 and at
    /// most [`ENQUEUE_BATCH`]; defaults to 100.
    #[config(bounds = BATCH_SIZE)]
    pub batch_size: usize,
    /// How long records may wait unfiled before the relay says so at `error`,
    /// once per outage. Read from
    /// `<PREFIX>_QUEUE__OUTBOX__STALL_WARNING_SECS`, at least 1 and at most
    /// 86400; defaults to 5 minutes.
    #[config(bounds = STALL_WARNING)]
    pub stall_warning: Duration,
}

impl Default for OutboxConfig {
    fn default() -> Self {
        Self {
            sweep_interval: Duration::from_secs(1),
            batch_size: ENQUEUE_BATCH,
            stall_warning: Duration::from_secs(5 * 60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nest_rs_config::{Config, ConfigService, Namespaced, Result};

    fn read(vars: &[(&str, &str)]) -> Result<OutboxConfig> {
        read_over(vars, OutboxConfig::default())
    }

    fn read_over(vars: &[(&str, &str)], base: OutboxConfig) -> Result<OutboxConfig> {
        OutboxConfig::from_env(
            &ConfigService::with_vars("queue__outbox", vars.iter().copied()),
            base,
        )
    }

    #[test]
    fn the_namespace_is_the_ports_word_then_the_outboxs() {
        assert_eq!(OutboxConfig::NAMESPACE, "queue__outbox");
    }

    #[test]
    fn the_defaults_sweep_each_second_claim_a_call_and_warn_after_five_minutes() {
        let defaults = read(&[]).expect("the defaults are in range");
        assert_eq!(defaults.sweep_interval, Duration::from_secs(1));
        assert_eq!(defaults.batch_size, ENQUEUE_BATCH);
        assert_eq!(defaults.stall_warning, Duration::from_secs(300));
    }

    #[test]
    fn every_field_reads_its_variable_over_the_pinned_base() {
        let pinned = OutboxConfig {
            sweep_interval: Duration::from_millis(250),
            batch_size: 10,
            stall_warning: Duration::from_secs(60),
        };
        let kept = read_over(&[], pinned.clone()).expect("a pin in range");
        assert_eq!(kept.sweep_interval, pinned.sweep_interval);
        assert_eq!(kept.batch_size, 10);
        assert_eq!(kept.stall_warning, pinned.stall_warning);

        let read = read_over(
            &[
                ("SWEEP_INTERVAL_MS", "500"),
                ("BATCH_SIZE", "20"),
                ("STALL_WARNING_SECS", "30"),
            ],
            pinned,
        )
        .expect("in range");
        assert_eq!(read.sweep_interval, Duration::from_millis(500));
        assert_eq!(read.batch_size, 20);
        assert_eq!(read.stall_warning, Duration::from_secs(30));
    }

    #[test]
    fn a_value_outside_its_range_is_refused_naming_its_variable() {
        for (key, refused) in [
            ("SWEEP_INTERVAL_MS", "99"),
            ("SWEEP_INTERVAL_MS", "60001"),
            ("BATCH_SIZE", "0"),
            ("BATCH_SIZE", "101"),
            ("STALL_WARNING_SECS", "0"),
            ("STALL_WARNING_SECS", "86401"),
        ] {
            let error = read(&[(key, refused)])
                .expect_err("out of range")
                .to_string();
            assert!(
                error.contains(&nest_rs_config::var_name("queue__outbox", key)),
                "{key}={refused}: {error}"
            );
        }
        let pinned = OutboxConfig {
            batch_size: ENQUEUE_BATCH + 1,
            ..OutboxConfig::default()
        };
        let error = read_over(&[], pinned).expect_err("a pin is held to the same range");
        assert!(
            error
                .to_string()
                .contains(&nest_rs_config::var_name("queue__outbox", "BATCH_SIZE")),
            "{error}"
        );
    }
}
