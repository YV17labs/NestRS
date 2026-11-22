//! [`ResilienceConfig`] — a policy's settings, read per instance from
//! `<PREFIX>_RESILIENCE__*`.

use std::ops::RangeInclusive;
use std::time::Duration;

use nest_rs_config::{Bound, CountBounds, DurationBounds, Floor, config};

const TIMEOUT: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        1,
        "a zero timeout gives every attempt up before its call is sent",
    )),
    Bound::new(
        10 * 60 * 1000,
        "one attempt past ten minutes outwaits every edge's deadline many times over, and a \
         value that long is a unit slip more often than a choice (`600` meant as seconds)",
    ),
);

const RETRIES: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "no retry is `0`, which turns retries off rather than counting one",
    ),
    Bound::new(
        10,
        "past ten, one caller sends a failing dependency eleven calls, the hammering a \
         breaker exists to stop",
    ),
);

const BACKOFF: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        1,
        "a zero wait sends the retry at once, into the failure the last attempt met",
    )),
    Bound::new(
        5 * 60 * 1000,
        "a retry waiting past five minutes outwaits every caller of one call; work that must \
         wait longer is a queue job, whose own retries wait as long",
    ),
);

const BREAKER_WINDOW: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a window of no call never sees a failure; `0` turns the breaker off",
    ),
    Bound::new(
        10_000,
        "past ten thousand calls the window still counts failures from long before the \
         outage, and holds a byte per call on every replica, and the instant it answered once \
         outcomes have an age",
    ),
);

const BREAKER_FAILURES: CountBounds = CountBounds::new(
    Bound::new(1, "a breaker opening on no failure refuses every call"),
    Bound::new(
        10_000,
        "a breaker never opens on more failures than its window holds, and the window holds \
         ten thousand at most",
    ),
);

const BREAKER_OPEN_FLOOR: Bound = Bound::new(
    1,
    "a breaker open for no time lets every call through to the failing dependency",
);

const BREAKER_OPEN: DurationBounds = DurationBounds::secs(
    Floor::Units(BREAKER_OPEN_FLOOR),
    Bound::new(
        60 * 60,
        "a breaker open past an hour still refuses a dependency long since back",
    ),
);

/// The open times [`ResilienceConfig::breaker_open`] takes, which a manual
/// opening is held to.
pub(crate) const BREAKER_OPEN_TIMES: RangeInclusive<Duration> = RangeInclusive::new(
    BREAKER_OPEN.unit().duration(BREAKER_OPEN_FLOOR.count),
    BREAKER_OPEN.unit().duration(BREAKER_OPEN.most().count),
);

/// The longest an attempt runs: the ceiling of [`ResilienceConfig::timeout`],
/// which a `#[resilient]` method's own timeout is held to as well.
pub(crate) const LONGEST_ATTEMPT: Duration = TIMEOUT.unit().duration(TIMEOUT.most().count);

const BREAKER_AGE: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "an age of no time forgets each outcome before the next is counted, so the window never \
         holds two; `0` turns the age off",
    )),
    Bound::new(
        24 * 60 * 60,
        "past a day the window still counts failures from long before the outage, which the age \
         is there to forget",
    ),
);

const BREAKER_PROBES: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a breaker closing on no probe lets every call through to a dependency it never tried",
    ),
    Bound::new(
        100,
        "past a hundred, a dependency back is still refused to every caller but one for a hundred \
         answers in a row",
    ),
);

const CONCURRENCY: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a bulkhead of no slot refuses every call; `0` turns it off",
    ),
    Bound::new(
        65_536,
        "past 65536 calls at once a replica runs out of sockets before the bulkhead refuses \
         one",
    ),
);

const CONCURRENCY_WAIT: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(0, "a wait of none refuses at once")),
    Bound::new(
        60 * 1000,
        "a call waiting past a minute for a slot outwaits the attempt it waits to make",
    ),
);

const CONCURRENCY_WAITERS: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a bound of no waiter refuses every call finding no free slot, as a wait of none does; \
         `0` turns the bound off",
    ),
    Bound::new(
        65_536,
        "past 65536 calls waiting at once, a replica holds more callers in memory, each for up to \
         a minute, than the most calls a bulkhead runs at once; a backlog that long is a queue's, \
         whose jobs wait in its backend rather than in memory",
    ),
);

const RATE_LIMIT: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a limit of no call refuses every call; `0` turns the rate limit off",
    ),
    Bound::new(
        1_000_000_000,
        "past a billion calls in a window, over eleven thousand a second for the longest window \
         there is, no dependency publishes a rate, so a number that large is a slip that turns \
         the limit off in all but name",
    ),
);

const RATE_WINDOW: DurationBounds = DurationBounds::secs(
    Floor::Units(Bound::new(
        1,
        "a zero window resets on every call, so the count never passes one and every call is \
         sent at any limit",
    )),
    Bound::new(
        24 * 60 * 60,
        "a window past a day is a quota rather than a rate, and a count each replica keeps in \
         memory, forgotten at its restart, holds no quota",
    ),
);

const RATE_WAIT: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(0, "a wait of none refuses at once")),
    Bound::new(
        60 * 1000,
        "a call waiting past a minute for the window's reset outwaits a request's default \
         deadline three times over, holding its slot of the bulkhead all along; one that must \
         wait longer is refused with the `Retry-After` its client or its queue's retry waits",
    ),
);

/// One policy's settings, read by
/// [`ResilienceModule`](crate::ResilienceModule) from `<PREFIX>_RESILIENCE__*`
/// for the default instance and `<PREFIX>_RESILIENCE__<INSTANCE>__*` for a
/// named one, over a base
/// [`ResilienceModule::for_root`](crate::ResilienceModule::for_root) may pin.
///
/// Every wait is bounded: an attempt by [`timeout`](Self::timeout), the wait
/// before a retry by [`backoff_ceiling`](Self::backoff_ceiling), a slot by
/// [`concurrency_wait`](Self::concurrency_wait), the window's reset by
/// [`rate_wait`](Self::rate_wait), and the number of attempts by
/// [`retries`](Self::retries); the calls waiting for a slot at once, by
/// [`concurrency_waiters`](Self::concurrency_waiters).
#[config(namespace = "resilience")]
#[derive(Clone, Debug)]
pub struct ResilienceConfig {
    /// How long one attempt waits for its call's answer before giving it up,
    /// `TIMEOUT_MS`, from 1 ms to ten minutes; defaults to 5 seconds. The
    /// unit's deadline, when it comes first, ends the attempt sooner, and a
    /// `#[resilient]` method may pin its own, in the same range.
    #[config(bounds = TIMEOUT)]
    pub timeout: Duration,
    /// How many times an idempotent call is attempted again after its attempt
    /// failed, `RETRIES`, from 1 to 10, `0` for none; defaults to 2. A call
    /// run without declaring itself idempotent is never attempted again, and an
    /// idempotent `#[resilient]` method may pin its own, in the same range.
    #[config(bounds = RETRIES)]
    pub retries: Option<usize>,
    /// About how long the first retry waits, `BACKOFF_FIRST_MS`; each one after
    /// waits twice the last, up to [`backoff_ceiling`](Self::backoff_ceiling),
    /// each jittered by ±20 %. From 1 ms to five minutes, at most the ceiling;
    /// defaults to 200 ms.
    #[config(bounds = BACKOFF)]
    pub backoff_first: Duration,
    /// The longest a retry waits, `BACKOFF_CEILING_MS`, from 1 ms to five
    /// minutes; defaults to 2 seconds. A dependency asking a longer wait — a
    /// `Retry-After` the reqwest middleware reads — is not attempted again.
    #[config(bounds = BACKOFF)]
    pub backoff_ceiling: Duration,
    /// How many of the latest attempts the circuit breaker counts,
    /// `BREAKER_WINDOW`, from 1 to 10000, `0` for no breaker; defaults to 20.
    #[config(bounds = BREAKER_WINDOW)]
    pub breaker_window: Option<usize>,
    /// How many failed attempts in the window open the breaker,
    /// `BREAKER_FAILURES`, from 1 to the window; defaults to 10. With a
    /// [`breaker_age`](Self::breaker_age) under a
    /// [`rate_limit`](Self::rate_limit), it is at most the failures that rate
    /// lets answer within the age of one another, an attempt running ten
    /// minutes at most: a breaker opening on more never opens, and is refused
    /// at boot.
    #[config(bounds = BREAKER_FAILURES)]
    pub breaker_failures: usize,
    /// How long an open breaker refuses every call before one attempt probes
    /// the dependency, `BREAKER_OPEN_SECS`, from 1 second to an hour; defaults
    /// to 30 seconds.
    #[config(bounds = BREAKER_OPEN)]
    pub breaker_open: Duration,
    /// How old an attempt's outcome grows before it no longer counts in the
    /// window, `BREAKER_AGE_SECS`, from 1 second to a day, `0` for no age;
    /// defaults to none, the window counting its latest attempts however old.
    /// An outcome's age runs from its call's answer, and the window forgets
    /// what is past it each time it counts one, so failures one short of
    /// opening the breaker, aged past it by a quiet spell, do not open it with
    /// the next. Under a rate limit it leaves room for
    /// [`breaker_failures`](Self::breaker_failures) failures to answer within
    /// it, or the boot is refused.
    #[config(bounds = BREAKER_AGE)]
    pub breaker_age: Option<Duration>,
    /// How many probes in a row must succeed before an open breaker closes,
    /// `BREAKER_PROBES`, from 1 to 100; defaults to 1. One probe runs at a
    /// time, and a failed one opens the breaker again for its whole open time,
    /// the series starting over: a probe whose call is cut at its timeout or
    /// its unit's deadline has failed. A probe whose call is never sent —
    /// refused by the rate, or its unit out of time before it — or whose
    /// caller drops it before it answers counts as neither a success nor a
    /// failure, and the series keeps its count.
    #[config(bounds = BREAKER_PROBES)]
    pub breaker_probes: usize,
    /// How many calls run inside the policy at once, the bulkhead,
    /// `CONCURRENCY`, from 1 to 65536, `0` for no bound; defaults to none.
    #[config(bounds = CONCURRENCY)]
    pub concurrency: Option<usize>,
    /// How long a call waits for a slot of the bulkhead before it is refused,
    /// `CONCURRENCY_WAIT_MS`, from none to a minute; defaults to none, a full
    /// bulkhead refusing at once.
    #[config(bounds = CONCURRENCY_WAIT)]
    pub concurrency_wait: Duration,
    /// How many calls wait for a slot of the bulkhead at once,
    /// `CONCURRENCY_WAITERS`, from 1 to 65536, `0` for no bound; defaults to
    /// none. A call finding every slot taken while as many calls wait is
    /// refused at once, and a waiting call gives its place up as it takes a
    /// slot, runs out of its wait or is dropped. It bounds nothing without
    /// [`concurrency`](Self::concurrency), or beside a
    /// [`concurrency_wait`](Self::concurrency_wait) of none, where no call
    /// waits.
    #[config(bounds = CONCURRENCY_WAITERS)]
    pub concurrency_waiters: Option<usize>,
    /// How many attempts the policy sends its dependency per
    /// [`rate_window`](Self::rate_window), a retry included, `RATE_LIMIT`,
    /// from 1 to a billion, `0` for none; defaults to none. An attempt past it
    /// is not sent: the run ends refused, told the time to the window's reset,
    /// unless [`rate_wait`](Self::rate_wait) reaches it. Each replica counts
    /// its own attempts, so a limit the dependency sets for the whole app is
    /// divided by the replicas sharing it.
    ///
    /// It counts attempts, not the requests one makes: a redirect the call's
    /// client follows, or a second request in one body, goes out inside the
    /// attempt that sent it. Where the dependency's own limit counts every
    /// request it is sent, each attempt sends one — a reqwest client built
    /// with `redirect::Policy::none()` answers a redirect as it came.
    #[config(bounds = RATE_LIMIT)]
    pub rate_limit: Option<usize>,
    /// The window the rate limit counts over, `RATE_WINDOW_SECS`, from 1
    /// second to a day; defaults to a minute. It is fixed: the first attempt
    /// counted opens it, and the count starts again once it has passed.
    #[config(bounds = RATE_WINDOW)]
    pub rate_window: Duration,
    /// How long one attempt waits in all for the window's reset before it is
    /// refused, `RATE_WAIT_MS`, from none to a minute; defaults to none, an
    /// attempt past the limit refused at once. A reset this wait or the unit's
    /// deadline cannot reach is not waited for, and the wait is not the
    /// attempt's: its [`timeout`](Self::timeout) starts when its call is sent.
    /// When the wait ends the circuit breaker is asked again, so one that
    /// opened meanwhile refuses the attempt unsent.
    #[config(bounds = RATE_WAIT)]
    pub rate_wait: Duration,
}

impl Default for ResilienceConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
            retries: Some(2),
            backoff_first: Duration::from_millis(200),
            backoff_ceiling: Duration::from_secs(2),
            breaker_window: Some(20),
            breaker_failures: 10,
            breaker_open: Duration::from_secs(30),
            breaker_age: None,
            breaker_probes: 1,
            concurrency: None,
            concurrency_wait: Duration::ZERO,
            concurrency_waiters: None,
            rate_limit: None,
            rate_window: Duration::from_secs(60),
            rate_wait: Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_config::{Config, ConfigService};

    use super::*;

    fn read(vars: &[(&str, &str)]) -> nest_rs_config::Result<ResilienceConfig> {
        ResilienceConfig::from_env(
            &ConfigService::with_vars("resilience", vars.iter().copied()),
            ResilienceConfig::default(),
        )
    }

    #[test]
    fn every_key_is_its_fields_name() {
        assert_eq!(
            [
                ResilienceConfig::TIMEOUT.key(),
                ResilienceConfig::RETRIES.key(),
                ResilienceConfig::BACKOFF_FIRST.key(),
                ResilienceConfig::BACKOFF_CEILING.key(),
                ResilienceConfig::BREAKER_WINDOW.key(),
                ResilienceConfig::BREAKER_FAILURES.key(),
                ResilienceConfig::BREAKER_OPEN.key(),
                ResilienceConfig::BREAKER_AGE.key(),
                ResilienceConfig::BREAKER_PROBES.key(),
                ResilienceConfig::CONCURRENCY.key(),
                ResilienceConfig::CONCURRENCY_WAIT.key(),
                ResilienceConfig::CONCURRENCY_WAITERS.key(),
                ResilienceConfig::RATE_LIMIT.key(),
                ResilienceConfig::RATE_WINDOW.key(),
                ResilienceConfig::RATE_WAIT.key(),
            ],
            [
                "TIMEOUT_MS",
                "RETRIES",
                "BACKOFF_FIRST_MS",
                "BACKOFF_CEILING_MS",
                "BREAKER_WINDOW",
                "BREAKER_FAILURES",
                "BREAKER_OPEN_SECS",
                "BREAKER_AGE_SECS",
                "BREAKER_PROBES",
                "CONCURRENCY",
                "CONCURRENCY_WAIT_MS",
                "CONCURRENCY_WAITERS",
                "RATE_LIMIT",
                "RATE_WINDOW_SECS",
                "RATE_WAIT_MS",
            ]
        );
    }

    #[test]
    fn the_environment_overlays_each_field_and_zero_turns_off_what_has_an_off() {
        let read = read(&[
            ("TIMEOUT_MS", "750"),
            ("RETRIES", "0"),
            ("BACKOFF_FIRST_MS", "50"),
            ("BACKOFF_CEILING_MS", "400"),
            ("BREAKER_WINDOW", "0"),
            ("BREAKER_FAILURES", "3"),
            ("BREAKER_OPEN_SECS", "5"),
            ("BREAKER_AGE_SECS", "120"),
            ("BREAKER_PROBES", "3"),
            ("CONCURRENCY", "8"),
            ("CONCURRENCY_WAIT_MS", "0"),
            ("CONCURRENCY_WAITERS", "16"),
            ("RATE_LIMIT", "100"),
            ("RATE_WINDOW_SECS", "1"),
            ("RATE_WAIT_MS", "250"),
        ])
        .expect("every value is in range");
        assert_eq!(read.timeout, Duration::from_millis(750));
        assert_eq!(read.retries, None, "`0` turns retries off");
        assert_eq!(
            (read.backoff_first, read.backoff_ceiling),
            (Duration::from_millis(50), Duration::from_millis(400))
        );
        assert_eq!(read.breaker_window, None, "`0` turns the breaker off");
        assert_eq!(read.breaker_failures, 3);
        assert_eq!(read.breaker_open, Duration::from_secs(5));
        assert_eq!(read.breaker_age, Some(Duration::from_secs(120)));
        assert_eq!(read.breaker_probes, 3);
        assert_eq!(read.concurrency, Some(8));
        assert_eq!(read.concurrency_wait, Duration::ZERO);
        assert_eq!(read.concurrency_waiters, Some(16));
        assert_eq!(read.rate_limit, Some(100));
        assert_eq!(
            (read.rate_window, read.rate_wait),
            (Duration::from_secs(1), Duration::from_millis(250))
        );

        let off = ResilienceConfig::from_env(
            &ConfigService::with_vars("resilience", [("RATE_LIMIT", "0")]),
            ResilienceConfig {
                rate_limit: Some(10),
                ..ResilienceConfig::default()
            },
        )
        .expect("`0` is the rate limit's off");
        assert_eq!(off.rate_limit, None, "`0` turns the rate limit off");

        let ageless = ResilienceConfig::from_env(
            &ConfigService::with_vars("resilience", [("BREAKER_AGE_SECS", "0")]),
            ResilienceConfig {
                breaker_age: Some(Duration::from_secs(60)),
                ..ResilienceConfig::default()
            },
        )
        .expect("`0` is the age's off");
        assert_eq!(ageless.breaker_age, None, "`0` turns the age off");

        let unbounded = ResilienceConfig::from_env(
            &ConfigService::with_vars("resilience", [("CONCURRENCY_WAITERS", "0")]),
            ResilienceConfig {
                concurrency_waiters: Some(4),
                ..ResilienceConfig::default()
            },
        )
        .expect("`0` is the waiters' off");
        assert_eq!(
            unbounded.concurrency_waiters, None,
            "`0` turns the bound on the waiters off"
        );
    }

    #[test]
    fn unset_reads_the_defaults() {
        let read = read(&[]).expect("the defaults are in range");
        let default = ResilienceConfig::default();
        assert_eq!(
            (
                read.timeout,
                read.retries,
                read.breaker_window,
                read.concurrency
            ),
            (
                default.timeout,
                default.retries,
                default.breaker_window,
                default.concurrency
            )
        );
        assert_eq!(
            (read.rate_limit, read.rate_window, read.rate_wait),
            (None, Duration::from_secs(60), Duration::ZERO),
            "no rate limit, over a minute's window, refusing at once once set"
        );
        assert_eq!(
            (read.breaker_age, read.breaker_probes),
            (None, 1),
            "no age, and one probe closing the breaker, as before either existed"
        );
        assert_eq!(
            read.concurrency_waiters, None,
            "no bound on the calls waiting for a slot, as before it existed"
        );
    }

    /// Each refusal names its variable and its range, never the value it was
    /// given.
    #[test]
    fn a_value_outside_its_range_is_refused_naming_its_variable() {
        for (key, raw, said) in [
            ("TIMEOUT_MS", "0", "must be at least 1 millisecond"),
            (
                "TIMEOUT_MS",
                "600001",
                "must be at most 600000 milliseconds",
            ),
            ("RETRIES", "11", "must be at most 10"),
            ("BACKOFF_FIRST_MS", "0", "must be at least 1 millisecond"),
            ("BREAKER_WINDOW", "10001", "must be at most 10000"),
            ("BREAKER_FAILURES", "0", "must be at least 1"),
            ("BREAKER_OPEN_SECS", "3601", "must be at most 3600 seconds"),
            (
                "BREAKER_AGE_SECS",
                "86401",
                "must be at most 86400 seconds, or 0 to turn it off",
            ),
            ("BREAKER_PROBES", "0", "must be at least 1"),
            ("BREAKER_PROBES", "101", "must be at most 100"),
            ("CONCURRENCY", "65537", "must be at most 65536"),
            (
                "CONCURRENCY_WAIT_MS",
                "60001",
                "must be at most 60000 milliseconds",
            ),
            (
                "CONCURRENCY_WAITERS",
                "65537",
                "must be at most 65536, or 0 to turn it off",
            ),
            ("RATE_LIMIT", "1000000001", "must be at most 1000000000"),
            ("RATE_WINDOW_SECS", "0", "must be at least 1 second"),
            ("RATE_WINDOW_SECS", "86401", "must be at most 86400 seconds"),
            (
                "RATE_WAIT_MS",
                "60001",
                "must be at most 60000 milliseconds",
            ),
        ] {
            let refused = read(&[(key, raw)])
                .expect_err("outside the range")
                .to_string();
            let var = nest_rs_config::var_name("resilience", key);
            assert!(
                refused.contains(&var) && refused.contains(said),
                "{refused}"
            );
            assert!(!refused.contains(&format!(" {raw} ")), "{refused}");
        }
    }

    #[test]
    fn a_pinned_zero_where_zero_is_off_is_refused() {
        for (pinned, field) in [
            (
                ResilienceConfig {
                    retries: Some(0),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::retries` set in code",
            ),
            (
                ResilienceConfig {
                    rate_limit: Some(0),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::rate_limit` set in code",
            ),
            (
                ResilienceConfig {
                    breaker_age: Some(Duration::ZERO),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::breaker_age` set in code",
            ),
            (
                ResilienceConfig {
                    concurrency_waiters: Some(0),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::concurrency_waiters` set in code",
            ),
        ] {
            let refused =
                ResilienceConfig::from_env(&ConfigService::with_vars("resilience", []), pinned)
                    .expect_err("off in code is `None`")
                    .to_string();
            assert!(refused.contains(field), "{refused}");
        }
    }

    /// A window or a wait pinned past its range is refused as its variable
    /// would be, a window shorter than a second included.
    #[test]
    fn a_rate_window_or_wait_pinned_outside_its_range_is_refused() {
        for (pinned, var) in [
            (
                ResilienceConfig {
                    rate_window: Duration::from_millis(500),
                    ..ResilienceConfig::default()
                },
                "RATE_WINDOW_SECS",
            ),
            (
                ResilienceConfig {
                    rate_wait: Duration::from_secs(61),
                    ..ResilienceConfig::default()
                },
                "RATE_WAIT_MS",
            ),
        ] {
            let refused =
                ResilienceConfig::from_env(&ConfigService::with_vars("resilience", []), pinned)
                    .expect_err("a pin outside the range")
                    .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("resilience", var)),
                "{refused}"
            );
        }
    }

    /// An age or a count of probes pinned past its range is refused as its
    /// variable would be, an age shorter than a second included.
    #[test]
    fn a_breaker_age_or_probes_pinned_outside_its_range_is_refused() {
        for (pinned, var) in [
            (
                ResilienceConfig {
                    breaker_age: Some(Duration::from_millis(500)),
                    ..ResilienceConfig::default()
                },
                "BREAKER_AGE_SECS",
            ),
            (
                ResilienceConfig {
                    breaker_age: Some(Duration::from_secs(24 * 60 * 60 + 1)),
                    ..ResilienceConfig::default()
                },
                "BREAKER_AGE_SECS",
            ),
            (
                ResilienceConfig {
                    breaker_probes: 0,
                    ..ResilienceConfig::default()
                },
                "BREAKER_PROBES",
            ),
            (
                ResilienceConfig {
                    breaker_probes: 101,
                    ..ResilienceConfig::default()
                },
                "BREAKER_PROBES",
            ),
        ] {
            let refused =
                ResilienceConfig::from_env(&ConfigService::with_vars("resilience", []), pinned)
                    .expect_err("a pin outside the range")
                    .to_string();
            assert!(
                refused.contains(&nest_rs_config::var_name("resilience", var)),
                "{refused}"
            );
        }
    }

    /// The bound on the waiters takes one to its ceiling, read or pinned, and
    /// a value past either end is refused with that end's reason, naming the
    /// variable. The ceiling's reason is what a waiter holds, its caller in
    /// memory for up to the wait's ceiling, more of them than the slots'
    /// ceiling runs at once; a waiter has sent nothing, so holds no socket.
    #[test]
    fn the_waiters_bound_takes_one_to_its_ceiling_and_says_why_past_either_end() {
        let (floor, ceiling) = (CONCURRENCY_WAITERS.least(), CONCURRENCY_WAITERS.most());
        let pinned = |count: u64| {
            ResilienceConfig::from_env(
                &ConfigService::with_vars("resilience", []),
                ResilienceConfig {
                    concurrency_waiters: Some(usize::try_from(count).expect("a count")),
                    ..ResilienceConfig::default()
                },
            )
        };
        for end in [floor.count, ceiling.count] {
            let raw = end.to_string();
            let read = read(&[("CONCURRENCY_WAITERS", raw.as_str())]).expect("an end is taken");
            assert_eq!(read.concurrency_waiters, usize::try_from(end).ok());
            assert!(pinned(end).is_ok(), "{end} pinned in code");
        }
        assert_eq!(
            read(&[("CONCURRENCY_WAITERS", "1")])
                .expect("one waiter is a bound")
                .concurrency_waiters,
            Some(1),
            "the least bound that lets a call wait"
        );

        let var = nest_rs_config::var_name("resilience", "CONCURRENCY_WAITERS");
        let past = (ceiling.count + 1).to_string();
        let why = "past 65536 calls waiting at once, a replica holds more callers in memory, each \
                   for up to a minute, than the most calls a bulkhead runs at once; a backlog that \
                   long is a queue's, whose jobs wait in its backend rather than in memory";
        for refused in [
            read(&[("CONCURRENCY_WAITERS", past.as_str())]),
            pinned(ceiling.count + 1),
        ] {
            let refused = refused.expect_err("past the ceiling").to_string();
            assert!(refused.contains(&var) && refused.contains(why), "{refused}");
        }
        assert_eq!(
            (
                ceiling.count,
                CONCURRENCY_WAIT
                    .unit()
                    .duration(CONCURRENCY_WAIT.most().count)
            ),
            (CONCURRENCY.most().count, Duration::from_secs(60)),
            "the most calls a bulkhead runs at once, and the longest a waiter waits"
        );
        let none = pinned(0).expect_err("off in code is `None`").to_string();
        assert!(none.contains(&var) && none.contains(floor.why), "{none}");
    }

    /// A manual opening is held to the open times `breaker_open` takes,
    /// both ends included, as a value pinned in code is: the range is the
    /// config's own, read off its bounds, never a second literal.
    #[test]
    fn the_open_times_a_manual_opening_takes_are_breaker_opens() {
        let (least, most) = (*BREAKER_OPEN_TIMES.start(), *BREAKER_OPEN_TIMES.end());
        let tick = Duration::from_millis(1);
        for duration in [
            Duration::ZERO,
            least - tick,
            least,
            least + tick,
            most,
            most + tick,
        ] {
            assert_eq!(
                BREAKER_OPEN_TIMES.contains(&duration),
                ResilienceConfig::BREAKER_OPEN
                    .check("resilience", duration)
                    .is_ok(),
                "{duration:?}"
            );
        }
    }

    /// The longest attempt the policy's checks count on is the timeout's
    /// ceiling, the longest a policy's or a method's timeout takes.
    #[test]
    fn the_longest_attempt_is_the_timeouts_ceiling() {
        let tick = Duration::from_millis(1);
        assert!(
            ResilienceConfig::TIMEOUT
                .check("resilience", LONGEST_ATTEMPT)
                .is_ok()
        );
        assert!(
            ResilienceConfig::TIMEOUT
                .check("resilience", LONGEST_ATTEMPT + tick)
                .is_err()
        );
    }
}
