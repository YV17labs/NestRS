//! [`ResilienceConfig`] — a policy's settings, read per instance from
//! `<PREFIX>_RESILIENCE__*`.

use std::time::Duration;

use nest_rs_config::{Bound, CountBounds, DurationBounds, Floor, config};

const TIMEOUT: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        1,
        "a zero timeout gives every attempt up before its call is sent, and one above zero but \
         under a millisecond ends at the next tick of tokio's timer, which counts whole \
         milliseconds and cannot honour less",
    )),
    Bound::new(
        10 * 60 * 1000,
        "one attempt past ten minutes outlasts a worker job's default deadline, the longest the \
         framework gives a unit; a call that long is a job's work, started and then polled, \
         rather than one attempt",
    ),
);

const RETRIES: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "no retry is `0`, which turns retries off rather than counting one",
    ),
    Bound::new(
        10,
        "past ten, the retries let one call send its failing dependency twelve attempts or more, \
         the hammering a breaker exists to stop",
    ),
);

const BACKOFF: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        1,
        "a zero wait lets a retry follow its failed attempt at once, into the failure that \
         attempt met, and one above zero but under a millisecond ends at the next tick of tokio's \
         timer, which counts whole milliseconds and cannot honour less",
    )),
    Bound::new(
        5 * 60 * 1000,
        "a retry waiting past five minutes outwaits a request's default deadline fifteen times \
         over, holding its run's slot of the bulkhead, where there is one, all along; work that \
         must wait longer is a queue job, whose retry honours the wait its failure asks, up to an \
         hour",
    ),
);

const BREAKER_WINDOW: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a window of no call never sees a failure; `0` turns the breaker off",
    ),
    Bound::new(
        10_000,
        "past ten thousand calls the window holds a byte per call on every replica, and the \
         instant each answered once outcomes have an age; for a dependency called seldom, it \
         lets failures from long before the outage count, unless an age forgets them",
    ),
);

const BREAKER_FAILURES: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a breaker opening on no failure opens on any answer its window counts, a success \
         included",
    ),
    Bound::new(
        10_000,
        "a breaker never opens on more failures than its window holds, and the window holds \
         ten thousand at most",
    ),
);

const BREAKER_OPEN: DurationBounds = DurationBounds::secs(
    Floor::AboveZero(
        "a breaker open for no time lets the next call through at once, as its probe, never \
         resting the failing dependency",
    ),
    Bound::new(
        60 * 60,
        "a breaker open past an hour still refuses a dependency long since back",
    ),
);

/// The longest a breaker stays open: the ceiling of
/// [`ResilienceConfig::breaker_open`], which a manual opening is held to as
/// well.
pub(crate) const LONGEST_OPEN: Duration = BREAKER_OPEN.unit().duration(BREAKER_OPEN.most().count);

/// Whether a manual opening takes `duration`: an open time
/// [`ResilienceConfig::breaker_open`] takes in code, above none and up to
/// [`LONGEST_OPEN`].
pub(crate) fn is_open_time(duration: Duration) -> bool {
    !duration.is_zero() && duration <= LONGEST_OPEN
}

/// The longest an attempt runs: the ceiling of [`ResilienceConfig::timeout`],
/// which a `#[resilient]` method's own timeout is held to as well.
pub(crate) const LONGEST_ATTEMPT: Duration = TIMEOUT.unit().duration(TIMEOUT.most().count);

const BREAKER_AGE: DurationBounds = DurationBounds::secs(
    Floor::AboveZero(
        "an age of no time forgets an outcome once the clock has moved past its answer, so the \
         window counts only the outcomes answered at one instant; `0` turns the age off",
    ),
    Bound::new(
        24 * 60 * 60,
        "past a day the age lets the window count failures from long before the outage, which \
         the age is there to forget",
    ),
);

const BREAKER_PROBES: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a breaker closing on no probe lets every call through to a dependency it never tried",
    ),
    Bound::new(
        100,
        "past a hundred, a dependency back is sent one probe at a time, every call beside it \
         refused, for over a hundred answers in a row",
    ),
);

const CONCURRENCY: CountBounds = CountBounds::new(
    Bound::new(
        1,
        "a bulkhead of no slot refuses every call; `0` turns it off",
    ),
    Bound::new(
        65_536,
        "past 65536 calls at once, a client opening a TCP connection per call between one address \
         of the replica and one of the dependency runs out of source ports, TCP's being sixteen \
         bits, before the bulkhead refuses one; for any other client, such as one spreading its \
         calls over several addresses or multiplexing them over one connection, a bulkhead that \
         wide is a slip more often than a choice",
    ),
);

const CONCURRENCY_WAIT: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        1,
        "a wait of none refuses at once, and one above none but under a millisecond ends at the \
         next tick of tokio's timer, which counts whole milliseconds and cannot honour less; `0` \
         turns the wait off",
    )),
    Bound::new(
        60 * 1000,
        "a call waiting past a minute for a slot outwaits a request's default deadline three \
         times over; a backlog waiting longer is a queue's, whose jobs wait in its backend rather \
         than in a replica's memory",
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
    Floor::AboveZero(
        "a zero window refills the bucket at every instant, so no limit refuses a call",
    ),
    Bound::new(
        24 * 60 * 60,
        "a window past a day is a quota rather than a rate, and a bucket each replica keeps in \
         memory, full again at its restart, holds no quota",
    ),
);

const RATE_WAIT: DurationBounds = DurationBounds::millis(
    Floor::Units(Bound::new(
        1,
        "a wait of none refuses at once, and one above none but under a millisecond ends at the \
         next tick of tokio's timer, which counts whole milliseconds and cannot honour less; `0` \
         turns the wait off",
    )),
    Bound::new(
        60 * 1000,
        "a call waiting past a minute for its token outwaits a request's default deadline three \
         times over, holding its slot of the bulkhead, where there is one, all along; one that \
         must wait longer is refused with a `Retry-After` naming the time to that token",
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
/// [`concurrency_wait`](Self::concurrency_wait), a token of the rate limit by
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
    /// The longest a retry's wait grows to, before its jitter,
    /// `BACKOFF_CEILING_MS`, from 1 ms to five minutes; defaults to 2 seconds.
    /// A dependency asking a longer wait — a `Retry-After` the reqwest
    /// middleware reads — is not attempted again.
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
    /// the dependency, `BREAKER_OPEN_SECS`, whole seconds from 1 to an hour,
    /// and in code anything above zero up to an hour; defaults to 30 seconds.
    #[config(bounds = BREAKER_OPEN)]
    pub breaker_open: Duration,
    /// How old an attempt's outcome grows before it no longer counts in the
    /// window, `BREAKER_AGE_SECS`, whole seconds from 1 to a day, `0` for no
    /// age, and in code anything above zero up to a day, `None` for no age;
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
    /// its unit's deadline has failed, whether the policy or the unit's edge
    /// cut it there. A probe whose call is never sent — refused by the rate,
    /// or its unit out of time before it — whose caller drops it unanswered
    /// before then, or which answers an `Err` its method's `record_if`
    /// refuses, counts as neither a success nor a failure, and the series
    /// keeps its count.
    #[config(bounds = BREAKER_PROBES)]
    pub breaker_probes: usize,
    /// How many calls run inside the policy at once, the bulkhead,
    /// `CONCURRENCY`, from 1 to 65536, `0` for no bound; defaults to none.
    #[config(bounds = CONCURRENCY)]
    pub concurrency: Option<usize>,
    /// How long a call waits for a slot of the bulkhead before it is refused,
    /// `CONCURRENCY_WAIT_MS`, from 1 ms to a minute, `0` for no wait; defaults
    /// to none, a full bulkhead refusing at once. Beside a
    /// [`rate_limit`](Self::rate_limit), a call finding no free slot is
    /// refused by the rate at once, rather than after this wait, only when no
    /// wait it may make reaches its token: the one due it behind the attempts
    /// already waiting for theirs, past this wait and
    /// [`rate_wait`](Self::rate_wait) together, or past its unit's deadline.
    /// Any other waits for its slot and meets the rate as it stands once it
    /// holds one, so it may wait for a slot and be refused by the rate after.
    #[config(bounds = CONCURRENCY_WAIT)]
    pub concurrency_wait: Option<Duration>,
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
    /// from 1 to a billion, `0` for none; defaults to none. It is a token
    /// bucket: it holds that many tokens, starts full, and gains one every
    /// `RATE_LIMIT`th of the window until it is full again. An attempt takes
    /// a token to be sent, and one finding none free is not sent: the run
    /// ends refused, told the time to the next token, unless
    /// [`rate_wait`](Self::rate_wait) reaches it. Each replica keeps its own
    /// bucket, so a limit the dependency sets for the whole app is divided by
    /// the replicas sharing it. A bucket left idle for a window is full, and
    /// sends `RATE_LIMIT` attempts at once, then `RATE_LIMIT` per window: up
    /// to twice the limit goes out within one window's length after an idle
    /// spell, so a dependency that counts every request of the last window is
    /// given half the limit it allows.
    ///
    /// It counts attempts, not the requests one makes: a redirect the call's
    /// client follows, or a second request in one body, goes out inside the
    /// attempt that sent it. Where the dependency's own limit counts every
    /// request it is sent, each attempt sends one — a reqwest client built
    /// with `redirect::Policy::none()` answers a redirect as it came.
    #[config(bounds = RATE_LIMIT)]
    pub rate_limit: Option<usize>,
    /// The window the rate limit counts over, the time an empty bucket takes
    /// to fill, `RATE_WINDOW_SECS`, whole seconds from 1 to a day, and in code
    /// anything above zero up to a day; defaults to a minute. No window opens
    /// or resets: a token is back every [`rate_limit`](Self::rate_limit)th of
    /// it, from the first one a full bucket gives.
    #[config(bounds = RATE_WINDOW)]
    pub rate_window: Duration,
    /// The longest one attempt waits for a token, `RATE_WAIT_MS`, from 1 ms to
    /// a minute, `0` for no wait; defaults to none, an attempt finding no
    /// token free refused at once. Attempts waiting are served in the order
    /// they arrived: each reserves the token after the last one's, and one
    /// arriving stands behind them all. A token this wait or the unit's
    /// deadline cannot reach is not waited for, the attempt refused at once,
    /// so no more attempts wait at once than tokens come within the wait. An
    /// attempt waits once, the time it is told as it reserves its token, or
    /// less: a call dropped while it waits, refused by the circuit breaker
    /// once its wait ends or out of its unit's time by then gives its token
    /// to the attempt behind it. A token keeps its instant however late the
    /// runtime wakes the attempt before it, so two attempts sent after a wait
    /// may go out closer than a token's interval by that lag. The wait is not
    /// the attempt's: its [`timeout`](Self::timeout) starts when its call is
    /// sent. When the wait ends the circuit breaker is asked again, so one
    /// that opened meanwhile refuses the attempt unsent.
    #[config(bounds = RATE_WAIT)]
    pub rate_wait: Option<Duration>,
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
            concurrency_wait: None,
            concurrency_waiters: None,
            rate_limit: None,
            rate_window: Duration::from_secs(60),
            rate_wait: None,
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

    /// `pinned`, set in code with no variable over it, as the module reads it.
    fn read_pinned(pinned: ResilienceConfig) -> nest_rs_config::Result<ResilienceConfig> {
        ResilienceConfig::from_env(&ConfigService::with_vars("resilience", []), pinned)
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
        assert_eq!(read.concurrency_wait, None, "`0` turns the wait off");
        assert_eq!(read.concurrency_waiters, Some(16));
        assert_eq!(read.rate_limit, Some(100));
        assert_eq!(
            (read.rate_window, read.rate_wait),
            (Duration::from_secs(1), Some(Duration::from_millis(250)))
        );

        let off = ResilienceConfig::from_env(
            &ConfigService::with_vars(
                "resilience",
                [
                    ("CONCURRENCY", "0"),
                    ("RATE_LIMIT", "0"),
                    ("RATE_WAIT_MS", "0"),
                ],
            ),
            ResilienceConfig {
                concurrency: Some(8),
                rate_limit: Some(10),
                rate_wait: Some(Duration::from_millis(250)),
                ..ResilienceConfig::default()
            },
        )
        .expect("`0` is the bulkhead's off, the rate limit's and its wait's");
        assert_eq!(off.concurrency, None, "`0` turns the bulkhead off");
        assert_eq!(off.rate_limit, None, "`0` turns the rate limit off");
        assert_eq!(off.rate_wait, None, "`0` turns the wait off");

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
            (None, Duration::from_secs(60), None),
            "no rate limit, over a minute's window, refusing at once once set"
        );
        assert_eq!(
            read.concurrency_wait, None,
            "a full bulkhead refusing at once"
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

    /// Every setting whose zero is its off, in the config's order: off in code
    /// is `None`, so its zero pinned there is refused naming its field.
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
                    breaker_window: Some(0),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::breaker_window` set in code",
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
                    concurrency: Some(0),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::concurrency` set in code",
            ),
            (
                ResilienceConfig {
                    concurrency_wait: Some(Duration::ZERO),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::concurrency_wait` set in code",
            ),
            (
                ResilienceConfig {
                    concurrency_waiters: Some(0),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::concurrency_waiters` set in code",
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
                    rate_wait: Some(Duration::ZERO),
                    ..ResilienceConfig::default()
                },
                "`ResilienceConfig::rate_wait` set in code",
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
    /// would be, a window of no time included; a window under a second, which
    /// its variable cannot say, is inside it.
    #[test]
    fn a_rate_window_or_wait_pinned_outside_its_range_is_refused() {
        let sub_second = Duration::from_millis(500);
        let taken = read_pinned(ResilienceConfig {
            rate_window: sub_second,
            ..ResilienceConfig::default()
        })
        .expect("a window under a second, set in code");
        assert_eq!(taken.rate_window, sub_second);
        for (pinned, var) in [
            (
                ResilienceConfig {
                    rate_window: Duration::ZERO,
                    ..ResilienceConfig::default()
                },
                "RATE_WINDOW_SECS",
            ),
            (
                ResilienceConfig {
                    rate_wait: Some(Duration::from_secs(61)),
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
    /// variable would be, an age of no time included; an age under a second,
    /// which its variable cannot say, is inside it.
    #[test]
    fn a_breaker_age_or_probes_pinned_outside_its_range_is_refused() {
        let sub_second = Some(Duration::from_millis(500));
        let taken = read_pinned(ResilienceConfig {
            breaker_age: sub_second,
            ..ResilienceConfig::default()
        })
        .expect("an age under a second, set in code");
        assert_eq!(taken.breaker_age, sub_second);
        for (pinned, var) in [
            (
                ResilienceConfig {
                    breaker_age: Some(Duration::ZERO),
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

    /// A value past each ceiling is refused naming its variable, with a reason
    /// that holds whatever the policy's other settings take and whichever
    /// client the policy wraps, written out whole here; the waiters' is the
    /// test's above. Every number a reason states is held to the constant it
    /// reads, its own ceiling or another's, where this test reaches it: the
    /// hour a queue's retry honours is held by that retry, in the suite's
    /// `error::queue`.
    #[test]
    fn each_ceiling_says_why_in_words_its_siblings_leave_true() {
        let past = |most: u64| (most + 1).to_string();
        let backoff = "a retry waiting past five minutes outwaits a request's default deadline \
                       fifteen times over, holding its run's slot of the bulkhead, where there is \
                       one, all along; work that must wait longer is a queue job, whose retry \
                       honours the wait its failure asks, up to an hour";
        for (key, raw, why) in [
            (
                "TIMEOUT_MS",
                past(TIMEOUT.most().count),
                "one attempt past ten minutes outlasts a worker job's default deadline, the \
                 longest the framework gives a unit; a call that long is a job's work, started \
                 and then polled, rather than one attempt",
            ),
            (
                "RETRIES",
                past(RETRIES.most().count),
                "past ten, the retries let one call send its failing dependency twelve attempts \
                 or more, the hammering a breaker exists to stop",
            ),
            ("BACKOFF_FIRST_MS", past(BACKOFF.most().count), backoff),
            ("BACKOFF_CEILING_MS", past(BACKOFF.most().count), backoff),
            (
                "BREAKER_WINDOW",
                past(BREAKER_WINDOW.most().count),
                "past ten thousand calls the window holds a byte per call on every replica, and \
                 the instant each answered once outcomes have an age; for a dependency called \
                 seldom, it lets failures from long before the outage count, unless an age \
                 forgets them",
            ),
            (
                "BREAKER_FAILURES",
                past(BREAKER_FAILURES.most().count),
                "a breaker never opens on more failures than its window holds, and the window \
                 holds ten thousand at most",
            ),
            (
                "BREAKER_OPEN_SECS",
                past(BREAKER_OPEN.most().count),
                "a breaker open past an hour still refuses a dependency long since back",
            ),
            (
                "BREAKER_AGE_SECS",
                past(BREAKER_AGE.most().count),
                "past a day the age lets the window count failures from long before the outage, \
                 which the age is there to forget",
            ),
            (
                "BREAKER_PROBES",
                past(BREAKER_PROBES.most().count),
                "past a hundred, a dependency back is sent one probe at a time, every call beside \
                 it refused, for over a hundred answers in a row",
            ),
            (
                "CONCURRENCY",
                past(CONCURRENCY.most().count),
                "past 65536 calls at once, a client opening a TCP connection per call between one \
                 address of the replica and one of the dependency runs out of source ports, \
                 TCP's being sixteen bits, before the bulkhead refuses one; for any other client, \
                 such as one spreading its calls over several addresses or multiplexing them over \
                 one connection, a bulkhead that wide is a slip more often than a choice",
            ),
            (
                "CONCURRENCY_WAIT_MS",
                past(CONCURRENCY_WAIT.most().count),
                "a call waiting past a minute for a slot outwaits a request's default deadline \
                 three times over; a backlog waiting longer is a queue's, whose jobs wait in its \
                 backend rather than in a replica's memory",
            ),
            (
                "RATE_LIMIT",
                past(RATE_LIMIT.most().count),
                "past a billion calls in a window, over eleven thousand a second for the longest \
                 window there is, no dependency publishes a rate, so a number that large is a \
                 slip that turns the limit off in all but name",
            ),
            (
                "RATE_WINDOW_SECS",
                past(RATE_WINDOW.most().count),
                "a window past a day is a quota rather than a rate, and a bucket each replica \
                 keeps in memory, full again at its restart, holds no quota",
            ),
            (
                "RATE_WAIT_MS",
                past(RATE_WAIT.most().count),
                "a call waiting past a minute for its token outwaits a request's default \
                 deadline three times over, holding its slot of the bulkhead, where there is one, \
                 all along; one that must wait longer is refused with a `Retry-After` naming the \
                 time to that token",
            ),
        ] {
            let refused = read(&[(key, raw.as_str())])
                .expect_err("past the ceiling")
                .to_string();
            let var = nest_rs_config::var_name("resilience", key);
            assert!(refused.contains(&var) && refused.contains(why), "{refused}");
        }

        let duration = |bounds: DurationBounds| bounds.unit().duration(bounds.most().count);
        let (minute, day) = (Duration::from_secs(60), Duration::from_secs(24 * 60 * 60));
        for (bounds, said) in [
            (TIMEOUT, minute * 10),
            (BACKOFF, minute * 5),
            (BREAKER_OPEN, minute * 60),
            (BREAKER_AGE, day),
            (CONCURRENCY_WAIT, minute),
            (RATE_WINDOW, day),
            (RATE_WAIT, minute),
        ] {
            assert_eq!(
                duration(bounds),
                said,
                "the ceiling its reason says: {bounds:?}"
            );
        }
        for (bounds, said) in [
            (RETRIES, 10),
            (BREAKER_WINDOW, 10_000),
            (BREAKER_FAILURES, 10_000),
            (BREAKER_PROBES, 100),
            (CONCURRENCY, 65_536),
            (RATE_LIMIT, 1_000_000_000),
        ] {
            assert_eq!(
                bounds.most().count,
                said,
                "the ceiling its reason says: {bounds:?}"
            );
        }

        let request = nest_rs_http::DEFAULT_UNIT_TIMEOUT;
        assert_eq!(
            duration(TIMEOUT),
            nest_rs_core::JOB_TIMEOUT,
            "a worker job's default deadline"
        );
        assert!(
            nest_rs_core::JOB_TIMEOUT > request
                && nest_rs_core::JOB_TIMEOUT > nest_rs_core::SHUTDOWN_HOOKS_TIMEOUT,
            "the longest deadline the framework gives a unit, an init hook given none"
        );
        assert_eq!(
            RETRIES.most().count + 2,
            12,
            "a call and one retry past ten"
        );
        assert_eq!(duration(BACKOFF), request * 15, "fifteen times over");
        assert_eq!(
            (duration(CONCURRENCY_WAIT), duration(RATE_WAIT)),
            (request * 3, request * 3),
            "three times over"
        );
        assert_eq!(
            CONCURRENCY.most().count,
            u64::from(u16::MAX) + 1,
            "TCP's sixteen-bit ports"
        );
        assert_eq!(
            BREAKER_FAILURES.most().count,
            BREAKER_WINDOW.most().count,
            "the window holds ten thousand at most"
        );
        let per_second = RATE_LIMIT.most().count / duration(RATE_WINDOW).as_secs();
        assert!(
            (11_000..12_000).contains(&per_second),
            "over eleven thousand a second for the longest window there is: {per_second}"
        );
    }

    /// A value below each floor is refused naming its variable, with a reason
    /// that holds whatever the policy's other settings take, written out whole
    /// here: `0` read from a variable that has no off, and pinned in code where
    /// `0` is the variable's off. A duration whose one failure is none is
    /// refused none in code too, in words saying so, since anything above it
    /// is taken there; a duration the policy waits out on tokio's timer is
    /// held to a millisecond in code as from its variable, and refused none
    /// there in the floor's words, a wait's with its `None`, which is no wait.
    #[test]
    fn each_floor_says_why_in_words_its_siblings_leave_true() {
        let timeout = "a zero timeout gives every attempt up before its call is sent, and one \
                       above zero but under a millisecond ends at the next tick of tokio's timer, \
                       which counts whole milliseconds and cannot honour less";
        let backoff = "a zero wait lets a retry follow its failed attempt at once, into the \
                       failure that attempt met, and one above zero but under a millisecond ends \
                       at the next tick of tokio's timer, which counts whole milliseconds and \
                       cannot honour less";
        let open = "a breaker open for no time lets the next call through at once, as its probe, \
                    never resting the failing dependency";
        let age = "an age of no time forgets an outcome once the clock has moved past its answer, \
                   so the window counts only the outcomes answered at one instant; `0` turns the \
                   age off";
        let window =
            "a zero window refills the bucket at every instant, so no limit refuses a call";
        for (key, why) in [
            ("TIMEOUT_MS", timeout),
            ("BACKOFF_FIRST_MS", backoff),
            ("BACKOFF_CEILING_MS", backoff),
            (
                "BREAKER_FAILURES",
                "a breaker opening on no failure opens on any answer its window counts, a \
                 success included",
            ),
            ("BREAKER_OPEN_SECS", open),
            (
                "BREAKER_PROBES",
                "a breaker closing on no probe lets every call through to a dependency it never \
                 tried",
            ),
            ("RATE_WINDOW_SECS", window),
        ] {
            let refused = read(&[(key, "0")])
                .expect_err("below the floor")
                .to_string();
            let var = nest_rs_config::var_name("resilience", key);
            assert!(refused.contains(&var) && refused.contains(why), "{refused}");
        }

        let default = ResilienceConfig::default;
        for (key, pinned, why) in [
            (
                "TIMEOUT_MS",
                ResilienceConfig {
                    timeout: Duration::ZERO,
                    ..default()
                },
                timeout,
            ),
            (
                "BACKOFF_FIRST_MS",
                ResilienceConfig {
                    backoff_first: Duration::ZERO,
                    ..default()
                },
                backoff,
            ),
            (
                "BACKOFF_CEILING_MS",
                ResilienceConfig {
                    backoff_ceiling: Duration::ZERO,
                    ..default()
                },
                backoff,
            ),
        ] {
            let refused = read_pinned(pinned).expect_err("none, in code").to_string();
            let var = nest_rs_config::var_name("resilience", key);
            let said = format!("set in code is 0ns, below the 1ms it must be at least — {why}");
            assert!(
                refused.contains(&var) && refused.contains(&said),
                "{refused}"
            );
        }

        for (key, pinned, why) in [
            (
                "BREAKER_OPEN_SECS",
                ResilienceConfig {
                    breaker_open: Duration::ZERO,
                    ..default()
                },
                open,
            ),
            (
                "BREAKER_AGE_SECS",
                ResilienceConfig {
                    breaker_age: Some(Duration::ZERO),
                    ..default()
                },
                age,
            ),
            (
                "RATE_WINDOW_SECS",
                ResilienceConfig {
                    rate_window: Duration::ZERO,
                    ..default()
                },
                window,
            ),
        ] {
            let refused = read_pinned(pinned).expect_err("none, in code").to_string();
            let var = nest_rs_config::var_name("resilience", key);
            let said = format!("set in code is 0ns, and it must be above zero — {why}");
            assert!(
                refused.contains(&var) && refused.contains(&said),
                "{refused}"
            );
        }

        for (key, pinned, why) in [
            (
                "RETRIES",
                ResilienceConfig {
                    retries: Some(0),
                    ..default()
                },
                "no retry is `0`, which turns retries off rather than counting one",
            ),
            (
                "BREAKER_WINDOW",
                ResilienceConfig {
                    breaker_window: Some(0),
                    ..default()
                },
                "a window of no call never sees a failure; `0` turns the breaker off",
            ),
            (
                "CONCURRENCY",
                ResilienceConfig {
                    concurrency: Some(0),
                    ..default()
                },
                "a bulkhead of no slot refuses every call; `0` turns it off",
            ),
            (
                "CONCURRENCY_WAITERS",
                ResilienceConfig {
                    concurrency_waiters: Some(0),
                    ..default()
                },
                "a bound of no waiter refuses every call finding no free slot, as a wait of none \
                 does; `0` turns the bound off",
            ),
            (
                "RATE_LIMIT",
                ResilienceConfig {
                    rate_limit: Some(0),
                    ..default()
                },
                "a limit of no call refuses every call; `0` turns the rate limit off",
            ),
        ] {
            let refused = read_pinned(pinned)
                .expect_err("below the floor, in code")
                .to_string();
            let var = nest_rs_config::var_name("resilience", key);
            assert!(refused.contains(&var) && refused.contains(why), "{refused}");
        }

        let wait = "a wait of none refuses at once, and one above none but under a millisecond \
                    ends at the next tick of tokio's timer, which counts whole milliseconds and \
                    cannot honour less; `0` turns the wait off";
        for (key, pinned) in [
            (
                "CONCURRENCY_WAIT_MS",
                ResilienceConfig {
                    concurrency_wait: Some(Duration::ZERO),
                    ..default()
                },
            ),
            (
                "RATE_WAIT_MS",
                ResilienceConfig {
                    rate_wait: Some(Duration::ZERO),
                    ..default()
                },
            ),
        ] {
            let refused = read_pinned(pinned).expect_err("none, in code").to_string();
            let var = nest_rs_config::var_name("resilience", key);
            let said = format!(
                "set in code is 0ns, below the 1ms it must be at least — {wait}; `None` turns it off"
            );
            assert!(
                refused.contains(&var) && refused.contains(&said),
                "{refused}"
            );
        }
    }

    /// A manual opening is held to the open times `breaker_open` takes in
    /// code: anything above none, a time under a second included, up to its
    /// ceiling. The range is the config's own, read off its check and its
    /// bounds, never a second literal.
    #[test]
    fn the_open_times_a_manual_opening_takes_are_breaker_opens() {
        let most = BREAKER_OPEN.unit().duration(BREAKER_OPEN.most().count);
        let tick = Duration::from_nanos(1);
        let sub_second = Duration::from_millis(500);
        for duration in [
            Duration::ZERO,
            tick,
            sub_second,
            Duration::from_secs(1),
            most - tick,
            most,
            most + tick,
            Duration::MAX,
        ] {
            assert_eq!(
                is_open_time(duration),
                ResilienceConfig::BREAKER_OPEN
                    .check("resilience", duration)
                    .is_ok(),
                "{duration:?}"
            );
        }
        assert!(!is_open_time(Duration::ZERO), "an open time of none");
        assert!(is_open_time(sub_second), "an open time under a second");
        assert_eq!(LONGEST_OPEN, most, "the ceiling of `breaker_open`");
    }

    /// A timeout and a backoff pinned under a millisecond are refused as their
    /// variables refuse `0`, in words true of both: the timer they arm counts
    /// whole milliseconds. A millisecond is taken.
    #[test]
    fn a_timeout_or_backoff_pinned_under_a_millisecond_is_refused() {
        let timeout = "a zero timeout gives every attempt up before its call is sent, and one \
                       above zero but under a millisecond ends at the next tick of tokio's timer, \
                       which counts whole milliseconds and cannot honour less";
        let backoff = "a zero wait lets a retry follow its failed attempt at once, into the \
                       failure that attempt met, and one above zero but under a millisecond ends \
                       at the next tick of tokio's timer, which counts whole milliseconds and \
                       cannot honour less";
        let sub_milli = Duration::from_micros(500);
        let default = ResilienceConfig::default;
        for (pinned, field, why) in [
            (
                ResilienceConfig {
                    timeout: sub_milli,
                    ..default()
                },
                ResilienceConfig::TIMEOUT,
                timeout,
            ),
            (
                ResilienceConfig {
                    backoff_first: sub_milli,
                    ..default()
                },
                ResilienceConfig::BACKOFF_FIRST,
                backoff,
            ),
            (
                ResilienceConfig {
                    backoff_ceiling: sub_milli,
                    ..default()
                },
                ResilienceConfig::BACKOFF_CEILING,
                backoff,
            ),
        ] {
            let refused = read_pinned(pinned)
                .expect_err("under a millisecond, in code")
                .to_string();
            assert_eq!(
                refused,
                format!(
                    "invalid value for {}: is not set, and `{}` set in code is 500µs, below the \
                     1ms it must be at least — {why}",
                    field.var("resilience"),
                    field.field().path(),
                )
            );
        }
        let milli = Duration::from_millis(1);
        let taken = read_pinned(ResilienceConfig {
            timeout: milli,
            backoff_first: milli,
            backoff_ceiling: milli,
            ..default()
        })
        .expect("a millisecond, set in code");
        assert_eq!(
            (taken.timeout, taken.backoff_first, taken.backoff_ceiling),
            (milli, milli, milli)
        );
    }

    /// A wait for a slot or for the rate's token pinned under a millisecond is
    /// refused in words true of it, the timer its wait arms counting whole
    /// milliseconds; a millisecond is taken, and no wait is `None`, `0` from
    /// either variable, over a pin too.
    #[test]
    fn a_wait_pinned_under_a_millisecond_is_refused_and_no_wait_is_none() {
        let why = "a wait of none refuses at once, and one above none but under a millisecond \
                   ends at the next tick of tokio's timer, which counts whole milliseconds and \
                   cannot honour less; `0` turns the wait off";
        let under = Some(Duration::from_micros(500));
        let default = ResilienceConfig::default;
        for (pinned, field) in [
            (
                ResilienceConfig {
                    concurrency_wait: under,
                    ..default()
                },
                ResilienceConfig::CONCURRENCY_WAIT,
            ),
            (
                ResilienceConfig {
                    rate_wait: under,
                    ..default()
                },
                ResilienceConfig::RATE_WAIT,
            ),
        ] {
            let refused = read_pinned(pinned)
                .expect_err("under a millisecond, in code")
                .to_string();
            assert_eq!(
                refused,
                format!(
                    "invalid value for {}: is not set, and `{}` set in code is 500µs, below the \
                     1ms it must be at least — {why}; `None` turns it off",
                    field.var("resilience"),
                    field.field().path(),
                )
            );
        }
        let milli = Some(Duration::from_millis(1));
        let waiting = ResilienceConfig {
            concurrency_wait: milli,
            rate_wait: milli,
            ..default()
        };
        let taken = read_pinned(waiting.clone()).expect("a millisecond, set in code");
        assert_eq!((taken.concurrency_wait, taken.rate_wait), (milli, milli));
        let off = ResilienceConfig::from_env(
            &ConfigService::with_vars(
                "resilience",
                [("CONCURRENCY_WAIT_MS", "0"), ("RATE_WAIT_MS", "0")],
            ),
            waiting,
        )
        .expect("`0` is no wait");
        assert_eq!((off.concurrency_wait, off.rate_wait), (None, None));
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
