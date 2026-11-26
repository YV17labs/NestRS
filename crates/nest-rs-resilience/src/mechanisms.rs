//! [`Mechanisms`] — what one policy holds, with no instance in its type: its
//! settings, its breaker, its bulkhead and its rate, and what tells what they
//! do. A run reads them alone, so it names no instance.

use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

use nest_rs_config::{ConfigError, Namespaced, instance_namespace};
use nest_rs_core::{Backoff, Instance};

use crate::breaker::{Breaker, BreakerSettings};
use crate::bulkhead::Bulkhead;
use crate::config::{LONGEST_ATTEMPT, ResilienceConfig};
use crate::observed::Observed;
use crate::rate::Rate;
use crate::teller::Teller;

/// One policy's mechanisms, shared by every caller in the process.
pub(crate) struct Mechanisms {
    pub(crate) timeout: Duration,
    pub(crate) retries: u32,
    pub(crate) backoff: Backoff,
    pub(crate) breaker: Option<Breaker>,
    pub(crate) bulkhead: Option<Bulkhead>,
    pub(crate) rate: Option<Rate>,
    pub(crate) teller: Arc<Teller>,
    pub(crate) observed: Observed,
    /// Sets each run's jitter apart from the others'.
    pub(crate) runs: AtomicU64,
}

impl Mechanisms {
    /// The mechanisms `config` describes for the instance `I`, refusing a
    /// combination of its settings that cannot work, named as the instance's
    /// variables: a first backoff above the ceiling, and a breaker opening on
    /// more failures than its window holds or, under an age and a rate, than
    /// the policy's own attempts can count within the age of one another —
    /// each counted within the longest attempt of taking its token. That last
    /// check holds for what the policy bounds: the body of a response
    /// [`ResilienceMiddleware`](crate::ResilienceMiddleware) hands its caller
    /// counts when it ends, however long it is held, and is outside it.
    pub(crate) fn new<I: Instance>(config: &ResilienceConfig) -> Result<Arc<Self>, ConfigError> {
        let namespace = instance_namespace::<I>(ResilienceConfig::NAMESPACE);
        if config.backoff_first > config.backoff_ceiling {
            return Err(ConfigError::parse(
                ResilienceConfig::BACKOFF_FIRST.var(&namespace),
                format!(
                    "must be at most {}: a first wait above the longest one is never waited",
                    ResilienceConfig::BACKOFF_CEILING.var_or_field(&namespace),
                ),
            ));
        }
        if let Some(window) = config.breaker_window
            && config.breaker_failures > window
        {
            return Err(ConfigError::parse(
                ResilienceConfig::BREAKER_FAILURES.var(&namespace),
                format!(
                    "must be at most {}: a breaker opening on more failures than its window \
                     holds never opens",
                    ResilienceConfig::BREAKER_WINDOW.var_or_field(&namespace),
                ),
            ));
        }
        let rate = config
            .rate_limit
            .map(|limit| Rate::new(limit, config.rate_window, config.rate_wait));
        // An attempt's outcome ages from its answer, at most the longest attempt after its
        // token. A body `ResilienceMiddleware` holds counts when it ends, tied to no token.
        if let (Some(_), Some(age), Some(rate)) = (config.breaker_window, config.breaker_age, &rate)
            && config.breaker_failures > rate.most_sent_within(age + LONGEST_ATTEMPT)
        {
            return Err(ConfigError::parse(
                ResilienceConfig::BREAKER_FAILURES.var(&namespace),
                format!(
                    "must be at most the failures the policy's attempts can count within the age \
                     of one another under the rate limit, set by {}; {}; and {}: each attempt \
                     counts within its timeout of taking its token, so a breaker opening on more \
                     is opened by none of them; the body of a response ResilienceMiddleware's \
                     caller holds counts when it ends, outside this bound",
                    ResilienceConfig::BREAKER_AGE.var_or_field(&namespace),
                    ResilienceConfig::RATE_LIMIT.var_or_field(&namespace),
                    ResilienceConfig::RATE_WINDOW.var_or_field(&namespace),
                ),
            ));
        }
        let retries = config
            .retries
            .map_or(0, |retries| u32::try_from(retries).unwrap_or(u32::MAX));
        let teller = Arc::new(Teller::new(I::NAME));
        let breaker = config.breaker_window.map(|window| {
            let settings = BreakerSettings {
                window,
                failures: config.breaker_failures,
                open_for: config.breaker_open,
                age: config.breaker_age,
                probes: config.breaker_probes,
            };
            Breaker::new(settings, Arc::clone(&teller))
        });
        let bulkhead = config
            .concurrency
            .map(|slots| Bulkhead::new(slots, config.concurrency_wait, config.concurrency_waiters));
        // The instruments hold the mechanisms weakly, so one left on a meter never keeps a
        // policy past its app.
        Ok(Arc::new_cyclic(|mechanisms| Self {
            timeout: config.timeout,
            retries,
            backoff: Backoff::new(config.backoff_first, config.backoff_ceiling),
            observed: Observed::new(
                I::NAME,
                mechanisms,
                breaker.is_some(),
                bulkhead.is_some(),
                rate.is_some(),
            ),
            breaker,
            bulkhead,
            rate,
            teller,
            runs: AtomicU64::new(0),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use nest_rs_config::{Namespaced, instance_namespace};
    use nest_rs_core::{Unnamed, instance};

    use super::*;
    use crate::{BreakerState, ResilienceError, ResiliencePolicy, ResilienceRefusal};

    #[instance]
    struct Upstream;

    #[derive(Debug, PartialEq, Eq, thiserror::Error)]
    #[error("the upstream refused")]
    struct Refused;

    fn policy(config: ResilienceConfig) -> ResiliencePolicy<Upstream> {
        ResiliencePolicy::new(&config).expect("a valid policy")
    }

    async fn answering() -> Result<(), Refused> {
        Ok(())
    }

    /// No retry and no breaker, a bucket of `limit` tokens refilled over
    /// `window`, each attempt finding none free waiting up to `wait` for the
    /// next, or refused at once with none: the subject is the rate.
    fn rated(limit: usize, window: Duration, wait: Option<Duration>) -> ResilienceConfig {
        ResilienceConfig {
            retries: None,
            breaker_window: None,
            rate_limit: Some(limit),
            rate_window: window,
            rate_wait: wait,
            ..ResilienceConfig::default()
        }
    }

    #[test]
    fn a_first_wait_above_the_ceiling_is_refused_naming_both_variables() {
        let refused = ResiliencePolicy::<Upstream>::new(&ResilienceConfig {
            backoff_first: Duration::from_secs(3),
            ..ResilienceConfig::default()
        })
        .expect_err("a first wait above the longest")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name(
                "resilience__upstream",
                "BACKOFF_FIRST_MS"
            )) && refused.contains(&nest_rs_config::var_name(
                "resilience__upstream",
                "BACKOFF_CEILING_MS"
            )),
            "{refused}"
        );
    }

    #[test]
    fn more_failures_than_the_window_holds_is_refused_naming_both_variables() {
        let refused = ResiliencePolicy::<Unnamed>::new(&ResilienceConfig {
            breaker_window: Some(5),
            breaker_failures: 6,
            ..ResilienceConfig::default()
        })
        .expect_err("a breaker that never opens")
        .to_string();
        assert!(
            refused.contains(&nest_rs_config::var_name("resilience", "BREAKER_FAILURES"))
                && refused.contains(&nest_rs_config::var_name("resilience", "BREAKER_WINDOW")),
            "{refused}"
        );
    }

    /// Under a rate limit, a breaker opening on more failures than the
    /// policy's attempts can count within its age of one another is opened by
    /// none of them, and is refused naming the settings that decide it,
    /// quoting none of their values, and the one outcome the check does not
    /// bound, a response body the middleware's caller holds. Three attempts
    /// an hour let three answer within a minute at most — a full bucket's, the
    /// next token twenty minutes off, past that minute and the ten an attempt
    /// takes at most — and a breaker opening on three is built, and opens on
    /// them.
    #[tokio::test(start_paused = true)]
    async fn more_failures_than_the_rate_lets_answer_within_the_age_is_refused() {
        let config = |failures| ResilienceConfig {
            retries: None,
            breaker_window: Some(20),
            breaker_failures: failures,
            breaker_age: Some(Duration::from_secs(60)),
            rate_limit: Some(3),
            rate_window: Duration::from_secs(3600),
            ..ResilienceConfig::default()
        };
        let refused = ResiliencePolicy::<Upstream>::new(&config(4))
            .expect_err("a breaker the policy's attempts never open")
            .to_string();
        assert!(
            refused.contains("the policy's attempts") && refused.contains("ResilienceMiddleware"),
            "what the check bounds, and what it does not: {refused}"
        );
        let mut unnamed = refused.clone();
        for key in [
            "BREAKER_FAILURES",
            "BREAKER_AGE_SECS",
            "RATE_LIMIT",
            "RATE_WINDOW_SECS",
        ] {
            let var = nest_rs_config::var_name("resilience__upstream", key);
            assert!(unnamed.contains(&var), "{refused}");
            unnamed = unnamed.replace(&var, "");
        }
        assert!(
            !unnamed.contains(|c: char| c.is_ascii_digit()),
            "no value is quoted: {refused}"
        );

        let policy = policy(config(3));
        for _ in 0..3 {
            let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "three failures at once, from a bucket left to fill"
        );
    }

    /// The most failures that answer within the age of one another are a full
    /// bucket's and every token back while the age and the longest attempt
    /// last. Three attempts every ten minutes, with a ten minutes' age, let
    /// nine: three cut at the ten minutes an attempt takes at most, three sent
    /// as they are cut, the bucket full again, and three once it has filled
    /// anew, the age later. A breaker opening on nine is built and opens on
    /// them; one opening on ten is refused.
    #[tokio::test(start_paused = true)]
    async fn more_failures_than_a_full_bucket_and_the_tokens_back_in_the_span_send_is_refused() {
        let window = Duration::from_secs(600);
        let config = |failures| ResilienceConfig {
            timeout: LONGEST_ATTEMPT,
            retries: None,
            breaker_window: Some(20),
            breaker_failures: failures,
            breaker_age: Some(window),
            rate_limit: Some(3),
            rate_window: window,
            ..ResilienceConfig::default()
        };
        assert!(
            ResiliencePolicy::<Upstream>::new(&config(10)).is_err(),
            "a breaker the policy's attempts never open"
        );

        let policy = Arc::new(policy(config(9)));
        let cut: Vec<_> = (0..3)
            .map(|_| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    policy
                        .run(|_| std::future::pending::<Result<(), Refused>>())
                        .await
                })
            })
            .collect();
        for cut in cut {
            let cut = cut.await.expect("the call's task");
            assert!(
                matches!(
                    cut,
                    Err(ResilienceError::Refused {
                        refusal: ResilienceRefusal::TimedOut { .. },
                        ..
                    })
                ),
                "{cut:?}"
            );
        }
        let failed = || async {
            let failed = policy.run(|_| async { Err::<(), _>(Refused) }).await;
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        };
        for _ in 0..3 {
            failed().await;
        }
        tokio::time::advance(window).await;
        for _ in 0..3 {
            failed().await;
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "nine failures, the first and the last the age apart"
        );
    }

    /// The failures counted together may come from attempts sent long before
    /// they answer: one attempt a minute, each running up to the ten minutes
    /// an attempt takes at most, lets twelve answer within a minute of one
    /// another. A breaker opening on twelve is built, one on thirteen refused,
    /// and eleven sent a minute apart open it as they answer.
    #[tokio::test(start_paused = true)]
    async fn a_breaker_its_rate_paces_opens_on_failures_sent_far_apart_that_answer_together() {
        let config = |failures| ResilienceConfig {
            timeout: LONGEST_ATTEMPT,
            retries: None,
            breaker_window: Some(20),
            breaker_failures: failures,
            breaker_age: Some(Duration::from_secs(60)),
            rate_limit: Some(1),
            rate_window: Duration::from_secs(60),
            ..ResilienceConfig::default()
        };
        assert!(ResiliencePolicy::<Upstream>::new(&config(13)).is_err());
        assert!(ResiliencePolicy::<Upstream>::new(&config(12)).is_ok());

        let policy = Arc::new(policy(config(11)));
        let answers = tokio::time::Instant::now() + Duration::from_secs(600);
        let sent: Vec<_> = (1..=11)
            .map(|nth| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60) * nth).await;
                    policy
                        .run(|_| async move {
                            tokio::time::sleep_until(answers).await;
                            Err::<(), _>(Refused)
                        })
                        .await
                })
            })
            .collect();
        for sent in sent {
            let failed = sent.await.expect("the call's task");
            assert!(
                matches!(failed, Err(ResilienceError::Failed(Refused))),
                "{failed:?}"
            );
        }
        assert!(
            matches!(policy.breaker_state(), Some(BreakerState::Open { .. })),
            "ten failures at 600 s and the last at 660 s, within the minute"
        );
    }

    /// Each combination the policy refuses names every setting beside the one
    /// refused both ways it is set, its variable and its field in code, so a
    /// policy pinned through `ResilienceModule::for_root` is told what it set.
    #[test]
    fn a_refused_combination_names_each_setting_beside_it_by_its_variable_and_its_field() {
        let namespace = instance_namespace::<Upstream>(ResilienceConfig::NAMESPACE);
        let refused = |config: ResilienceConfig| {
            ResiliencePolicy::<Upstream>::new(&config)
                .expect_err("a combination that cannot work")
                .to_string()
        };
        for (said, beside) in [
            (
                refused(ResilienceConfig {
                    backoff_first: Duration::from_secs(3),
                    ..ResilienceConfig::default()
                }),
                vec![ResilienceConfig::BACKOFF_CEILING.var_or_field(&namespace)],
            ),
            (
                refused(ResilienceConfig {
                    breaker_window: Some(5),
                    breaker_failures: 6,
                    ..ResilienceConfig::default()
                }),
                vec![ResilienceConfig::BREAKER_WINDOW.var_or_field(&namespace)],
            ),
            (
                refused(ResilienceConfig {
                    breaker_age: Some(Duration::from_secs(60)),
                    rate_limit: Some(3),
                    rate_window: Duration::from_secs(3600),
                    ..ResilienceConfig::default()
                }),
                vec![
                    ResilienceConfig::BREAKER_AGE.var_or_field(&namespace),
                    ResilienceConfig::RATE_LIMIT.var_or_field(&namespace),
                    ResilienceConfig::RATE_WINDOW.var_or_field(&namespace),
                ],
            ),
        ] {
            for setting in beside {
                assert!(said.contains(&setting), "{setting} in: {said}");
            }
        }
    }

    /// An age and a rate that leave a breaker no room to open mean nothing
    /// without a breaker, as a slot's wait means nothing without slots: the
    /// settings refused beside the default breaker build a policy with none.
    #[test]
    fn an_age_a_rate_leaves_no_room_for_refuses_nothing_without_a_breaker() {
        let config = ResilienceConfig {
            breaker_age: Some(Duration::from_secs(60)),
            rate_limit: Some(3),
            rate_window: Duration::from_secs(3600),
            ..ResilienceConfig::default()
        };
        assert!(
            ResiliencePolicy::<Upstream>::new(&config).is_err(),
            "ten failures under three attempts an hour, with the default breaker"
        );
        let policy = ResiliencePolicy::<Upstream>::new(&ResilienceConfig {
            breaker_window: None,
            ..config
        })
        .expect("no breaker for the age to leave no room in");
        assert!(policy.mechanisms.breaker.is_none() && policy.mechanisms.rate.is_some());
    }

    /// A wait and a bound on the waiters mean nothing without slots, as a
    /// window and a wait mean nothing without a rate limit: the policy builds
    /// no bulkhead, and sends every call at once.
    #[tokio::test(start_paused = true)]
    async fn a_policy_with_no_slots_builds_no_bulkhead_whatever_its_wait_and_waiters() {
        let policy = Arc::new(policy(ResilienceConfig {
            retries: None,
            breaker_window: None,
            concurrency: None,
            concurrency_wait: Some(Duration::from_secs(60)),
            concurrency_waiters: Some(1),
            ..ResilienceConfig::default()
        }));
        assert!(policy.mechanisms.bulkhead.is_none());
        assert!(
            format!("{policy:?}").contains("bulkhead: false"),
            "{policy:?}"
        );
        let started = tokio::time::Instant::now();
        let calls: Vec<_> = (0..10)
            .map(|_| {
                let policy = Arc::clone(&policy);
                tokio::spawn(async move {
                    policy
                        .run(|_| async {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            Ok::<_, Refused>(())
                        })
                        .await
                })
            })
            .collect();
        for call in calls {
            let answered = call.await.expect("the call's task");
            assert!(answered.is_ok(), "{answered:?}");
        }
        assert_eq!(started.elapsed(), Duration::from_secs(1), "all at once");
    }

    /// A window and a wait mean nothing without a limit: the policy builds no
    /// rate, as a slot's wait builds no bulkhead without its slots, and sends
    /// every call with no lock taken and no timer armed for it.
    #[tokio::test(start_paused = true)]
    async fn a_policy_with_no_rate_limit_builds_no_rate_whatever_its_window_and_wait() {
        let policy = policy(ResilienceConfig {
            rate_limit: None,
            ..rated(1, Duration::from_secs(1), Some(Duration::from_secs(60)))
        });
        assert!(policy.mechanisms.rate.is_none());
        assert!(format!("{policy:?}").contains("rate: false"), "{policy:?}");
        let calls = AtomicU32::new(0);
        let started = tokio::time::Instant::now();
        for _ in 0..1_000 {
            let answered = policy
                .run(|_| async {
                    calls.fetch_add(1, Ordering::Relaxed);
                    Ok::<_, Refused>(())
                })
                .await;
            assert!(answered.is_ok(), "{answered:?}");
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1_000);
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    /// The policy's breaker forgets an outcome past `breaker_age`, and closes on
    /// `breaker_probes` probes in a row.
    #[tokio::test(start_paused = true)]
    async fn a_policy_builds_its_breaker_with_the_age_and_the_probes_it_reads() {
        let policy = policy(ResilienceConfig {
            retries: None,
            breaker_window: Some(4),
            breaker_failures: 2,
            breaker_open: Duration::from_secs(10),
            breaker_age: Some(Duration::from_secs(60)),
            breaker_probes: 2,
            ..ResilienceConfig::default()
        });
        let failing = || async { Err::<(), _>(Refused) };
        assert!(policy.run(|_| failing()).await.is_err());
        tokio::time::advance(Duration::from_secs(61)).await;
        assert!(policy.run(|_| failing()).await.is_err());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::Closed),
            "the first failure is past the age"
        );
        assert!(policy.run(|_| failing()).await.is_err());
        assert!(matches!(
            policy.breaker_state(),
            Some(BreakerState::Open { .. })
        ));
        tokio::time::advance(Duration::from_secs(10)).await;
        assert!(policy.run(|_| answering()).await.is_ok());
        assert_eq!(
            policy.breaker_state(),
            Some(BreakerState::HalfOpen),
            "one probe of two"
        );
        assert!(policy.run(|_| answering()).await.is_ok());
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
    }
}
