//! [`ResiliencePolicy`] — one named policy around the calls to one dependency:
//! a timeout per attempt, retries for an idempotent call, a circuit breaker, a
//! bulkhead and an outbound rate limit, every one of them shared by every
//! caller in the process.

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use nest_rs_config::{ConfigError, Namespaced, instance_namespace};
use nest_rs_core::{Instance, Unnamed};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::breaker::{Breaker, BreakerState, BreakerStats};
use crate::bulkhead::Bulkhead;
use crate::call::{Again, Cancellable, MakeAgain, MakeOnce, Once, Uncancelled};
use crate::config::{ResilienceConfig, is_open_time};
use crate::error::{BreakerError, ResilienceError};
use crate::events::ResilienceEvents;
use crate::mechanisms::Mechanisms;
use crate::resilient::{Judge, ResilientAnswer, ResilientMethod, judging_by_default};
use crate::run::{Run, Running};

/// The calls to one dependency — a payment provider, a carrier's API, a
/// language model — run under one policy, by every caller in the process.
///
/// Injected as `Arc<ResiliencePolicy<I>>` once a module imports
/// [`ResilienceModule<I>`](crate::ResilienceModule), which builds it from
/// `<PREFIX>_RESILIENCE__<INSTANCE>__*` ([`ResilienceConfig`]); the default
/// instance, `ResiliencePolicy`, reads `<PREFIX>_RESILIENCE__*`. A method runs
/// its body under it through [`#[resilient]`](macro@crate::resilient), naming
/// the field that holds it, and reads its attempt's [`CancellationToken`]
/// under the name its `cancel` key gives; [`run`](Self::run) and
/// [`run_idempotent`](Self::run_idempotent) take a call that is part of a
/// method, as a closure of no argument, and
/// [`run_cancellable`](Self::run_cancellable) and
/// [`run_idempotent_cancellable`](Self::run_idempotent_cancellable) hand it that
/// token.
///
/// Each attempt is given up past the policy's timeout or the deadline of the
/// unit it runs in (`nest_rs_core::Deadline`), whichever comes first, and its
/// [`CancellationToken`], when its call takes one, cancelled, so work the call
/// handed to another task stops with it; no retry starts whose wait ends past
/// that deadline. A call under one policy made from an attempt of another — a
/// decorated method calling a second dependency's — is given up with that
/// attempt, when that one's bound ends first: its own attempt, in flight
/// there, counts as timed out in its own breaker, so a dependency that never
/// answers opens the breaker naming it however short the timeout its caller
/// runs under, and what its run held unsent — a slot, a token, a probe's turn
/// — is given back, counted for nothing. A call is attempted again only when
/// it is declared idempotent, through [`run_idempotent`](Self::run_idempotent):
/// replaying one that is not would charge a card twice. An attempt's success
/// or failure counts in the circuit breaker, which refuses every call at once
/// while it is open, the bulkhead bounds how many calls run inside the policy
/// together, and the rate limit how many attempts it sends per window, a retry
/// included.
///
/// Inside a unit of work holding a transaction, each attempt of a call
/// declared idempotent, and of a decorated method declaring a `fallback`,
/// sent once or again, runs in a savepoint of it, sent with the attempt's
/// first statement and settled before the next attempt and before the run
/// answers: kept when the run answers the attempt's `Ok`, rolled back when the
/// attempt failed, was given up at its bound or panicked. A failed attempt so
/// leaves no write for the next to repeat, and a run ending on an error leaves
/// its unit what it wrote before the call — whatever retries the policy has,
/// none included: what a call leaves is read off how it is declared, never off
/// a setting of the deployment's. Any other call sent once, and a run on the
/// pool, open none: an `Err` there leaves what its body wrote, as it does with
/// no policy.
///
/// Every `Err` a call run through [`run`](Self::run) or
/// [`run_idempotent`](Self::run_idempotent) answers counts as the dependency
/// failing, and is sent again when the call is idempotent: an answer the
/// dependency gave — a declined card, a parcel not found — belongs in such a
/// call's `Ok`, so it is neither retried nor counted a failure. A decorated
/// method says otherwise of its own answers, key by key — `retry_if`,
/// `retry_ok_if` and `record_if`, an `Err` `record_if` refuses counting in the
/// breaker as neither a failure nor a success — and names the `fallback`
/// answering in place of the error its run ends on. The error is a
/// [`std::error::Error`]: its chain is the `error` of the line a retry files,
/// at `debug`, and of the `warn` the breaker files when that failure opens it,
/// where an operator reads what the dependency answered once every caller is
/// told the breaker is open. A URL in that chain — whatever follows a `://`,
/// up to the next whitespace, control character, double quote or angle
/// bracket — is told by its scheme, its host and a port of digits, and by its
/// scheme alone where an `@` past its authority's end leaves its host
/// impossible to tell from a userinfo: no byte of the userinfo, the path, the
/// query or the fragment a request carried. A status reqwest's own sentence
/// says is told by its class and its code, and the chain no further: the
/// phrase reqwest writes after the code is the dependency's. A value an error
/// quotes otherwise is the error's to keep out, a URL holding one of those
/// four unencoded included, which is read as ending there.
///
/// A call made inside an attempt of its own policy is a call that attempt
/// makes, sharing what the attempt holds and counting in it as [a call inside
/// an attempt of its own
/// policy](https://nestrs.dev/fundamentals/failures-and-fallbacks/#a-call-inside-an-attempt-of-its-own-policy)
/// does.
///
/// The breaker's state is read through [`breaker_state`](Self::breaker_state),
/// and switched by hand — an operator taking a dependency out for its
/// maintenance window — through [`open_breaker`](Self::open_breaker) and
/// [`close_breaker`](Self::close_breaker). What the policy holds now is read
/// beside it — [`breaker_stats`](Self::breaker_stats),
/// [`concurrency_running`](Self::concurrency_running),
/// [`concurrency_waiting`](Self::concurrency_waiting),
/// [`rate_available`](Self::rate_available) — and what it does is told as it
/// happens to each subscription [`events`](Self::events) opens.
///
/// ```
/// use std::sync::Arc;
///
/// use nest_rs_core::{App, injectable, instance, module};
/// use nest_rs_resilience::{ResilienceError, ResilienceModule, ResiliencePolicy};
///
/// #[instance]
/// pub struct Upstream;
///
/// #[injectable]
/// pub struct PreviewService {
///     #[inject]
///     upstream: Arc<ResiliencePolicy<Upstream>>,
/// }
///
/// impl PreviewService {
///     pub async fn preview(&self, url: &str) -> Result<String, ResilienceError<std::io::Error>> {
///         self.upstream
///             .run_idempotent(|| async move { Ok(format!("a preview of {url}")) })
///             .await
///     }
/// }
///
/// #[module(imports = [ResilienceModule<Upstream>], providers = [PreviewService])]
/// pub struct PostsModule;
///
/// # #[nest_rs_core::main]
/// # async fn main() -> anyhow::Result<()> {
/// let app = App::builder().module::<PostsModule>().build().await?;
/// let service = app.container().get::<PreviewService>().expect("provided");
/// assert_eq!(service.preview("https://example.com").await?, "a preview of https://example.com");
/// # Ok(())
/// # }
/// ```
pub struct ResiliencePolicy<I = Unnamed> {
    pub(crate) mechanisms: Arc<Mechanisms>,
    _instance: PhantomData<fn() -> I>,
}

impl<I: Instance> ResiliencePolicy<I> {
    /// The policy `config` describes, refusing a combination of its settings
    /// that cannot work, named as the instance's variables
    /// ([`Mechanisms::new`]).
    pub(crate) fn new(config: &ResilienceConfig) -> Result<Self, ConfigError> {
        Ok(Self {
            mechanisms: Mechanisms::new::<I>(config)?,
            _instance: PhantomData,
        })
    }

    /// Run `call` once, under the policy's timeout, breaker, bulkhead and rate
    /// limit, and never again: a call this runs may change what it calls, so a
    /// retry could do it twice.
    ///
    /// Its error is a [`std::error::Error`], whose chain the breaker's opening
    /// tells when that failure opens it. An attempt given up is dropped where it
    /// waits; a call handing work beyond its future — a spawned task, a request
    /// another client sends — takes the attempt's token through
    /// [`run_cancellable`](Self::run_cancellable).
    pub fn run<T, E, F, Fut>(&self, call: F) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_once::<Result<T, ResilienceError<E>>, _, Uncancelled, _>(
            None,
            judging_by_default(),
            call,
        )
    }

    /// [`run`](Self::run), `call` handed the attempt's [`CancellationToken`],
    /// cancelled when the attempt is given up: the one a call needs for work
    /// that outlives its future, made for this call alone.
    pub fn run_cancellable<T, E, F, Fut>(
        &self,
        call: F,
    ) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnOnce(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_once::<Result<T, ResilienceError<E>>, _, Cancellable, _>(
            None,
            judging_by_default(),
            call,
        )
    }

    /// Run the idempotent `call`, attempting it again after a failed or timed
    /// out attempt, up to the policy's retries, each retry waiting the
    /// policy's backoff first — or longer, the `Retry-After` of a
    /// [`Problem`](nest_rs_core::Problem) its error's chain carries. An attempt
    /// the circuit breaker refuses ends the run at once, as does one finding no
    /// token of the rate limit free that cannot wait for the next.
    ///
    /// Idempotent means one call or several leave the dependency as one does:
    /// a read, a `PUT`, a request carrying an idempotency key the dependency
    /// honours. Its error is a [`std::error::Error`], whose chain each retry's
    /// line tells, and the breaker's opening when that failure opens it; an
    /// attempt given up is dropped where it waits, and a call handing work
    /// beyond its future takes each attempt's token through
    /// [`run_idempotent_cancellable`](Self::run_idempotent_cancellable).
    ///
    /// Inside a unit of work holding a transaction, each attempt runs in a
    /// savepoint of that transaction, whether or not the policy has retries:
    /// what `call` writes through the data layer is rolled back unless the run
    /// answers that attempt's `Ok`. Once an attempt sent a statement, its
    /// savepoint holds the unit until it settles, so another statement of that
    /// unit, issued beside the run, waits for it; an attempt given up while one
    /// of its statements runs is rolled back once that statement ends, which
    /// the next attempt and the run's answer wait for, and one given up before
    /// its savepoint was answered — its unit waiting for a connection, or for
    /// that answer — leaves the unit as it was; and a run dropped mid-attempt,
    /// once its savepoint was answered, leaves what holds that savepoint
    /// unable to commit, as an atomic block dropped does: its unit — or, for a
    /// run nested in another run's attempt or in an atomic block, that
    /// attempt's savepoint or that block alone, which is rolled back while
    /// the unit goes on to commit. A savepoint the database cannot settle
    /// ends the run on [`ResilienceError::Unsettled`].
    ///
    /// An attempt's wait for its unit counts against its bound: of two such
    /// calls joined in one transaction, the second's first statement waits
    /// for the first's whole attempt, and may be given up at its timeout and
    /// counted as its dependency's failure — send them one after the other,
    /// or outside the transaction.
    pub fn run_idempotent<T, E, F, Fut>(
        &self,
        call: F,
    ) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_again::<Result<T, ResilienceError<E>>, _, Uncancelled, _>(
            None,
            judging_by_default(),
            call,
        )
    }

    /// [`run_idempotent`](Self::run_idempotent), `call` handed each attempt's
    /// own [`CancellationToken`], cancelled when that attempt is given up.
    pub fn run_idempotent_cancellable<T, E, F, Fut>(
        &self,
        call: F,
    ) -> impl Future<Output = Result<T, ResilienceError<E>>>
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = Result<T, E>>,
        E: Error + 'static,
    {
        self.run_again::<Result<T, ResilienceError<E>>, _, Cancellable, _>(
            None,
            judging_by_default(),
            call,
        )
    }

    /// `call` run once, for `method` when a decorated method runs it, its
    /// future made as `K` makes it — handed a token or none — its answer read
    /// as `R` reads a body's and answered as `R`, an `Err` a failure in the
    /// breaker when `judging` records it and counted in neither direction
    /// otherwise: what [`run`](Self::run) and
    /// [`run_cancellable`](Self::run_cancellable) run, with no method and every
    /// `Err` a failure.
    pub(crate) fn run_once<R, J, K, F>(
        &self,
        method: Option<&'static ResilientMethod>,
        judging: J,
        call: F,
    ) -> Running<'_, R, Once<F, K>, J>
    where
        R: ResilientAnswer,
        R::Err: Error + 'static,
        J: Judge<R>,
        K: MakeOnce<F>,
        K::Made: Future<Output = R::Attempt>,
    {
        let run = Run::new(&self.mechanisms, method, false);
        Running::new(Once::new(call), judging, run)
    }

    /// The idempotent `call` run, for `method` when a decorated method runs
    /// it, each attempt's future made as `K` makes it — handed a token or
    /// none — each answer read as `R` reads a body's and the last answered as
    /// `R`, each counted as `judging` says — an `Err` or an `Ok` sent again
    /// when it retries it, an `Err` a failure in the breaker when it records
    /// it and counted in neither direction otherwise: what
    /// [`run_idempotent`](Self::run_idempotent) and
    /// [`run_idempotent_cancellable`](Self::run_idempotent_cancellable) run,
    /// with no method and every `Err` a failure sent again.
    pub(crate) fn run_again<R, J, K, F>(
        &self,
        method: Option<&'static ResilientMethod>,
        judging: J,
        call: F,
    ) -> Running<'_, R, Again<F, K>, J>
    where
        R: ResilientAnswer,
        R::Err: Error + 'static,
        J: Judge<R>,
        K: MakeAgain<F>,
        K::Made: Future<Output = R::Attempt>,
    {
        let run = Run::new(&self.mechanisms, method, true);
        Running::new(Again::new(call), judging, run)
    }

    /// The circuit breaker's state, as a call arriving now meets it, or `None`
    /// for a policy with no breaker (`BREAKER_WINDOW` set to `0`).
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use nest_rs_resilience::{BreakerState, ResiliencePolicy};
    ///
    /// fn retry_after(policy: &ResiliencePolicy) -> Option<Duration> {
    ///     match policy.breaker_state()? {
    ///         BreakerState::Open { retry_after, .. } => Some(retry_after),
    ///         _ => None,
    ///     }
    /// }
    /// ```
    pub fn breaker_state(&self) -> Option<BreakerState> {
        self.mechanisms.breaker.as_ref().map(Breaker::read)
    }

    /// What the circuit breaker's window counts now — its outcomes, and the
    /// failures among them — as a call arriving now meets it, or `None` for a
    /// policy with no breaker (`BREAKER_WINDOW` set to `0`).
    ///
    /// An outcome past [`breaker_age`](ResilienceConfig::breaker_age) counts
    /// no more, and the window is forgotten as the breaker opens: open or
    /// half-open it reads none, and counts afresh once closed.
    pub fn breaker_stats(&self) -> Option<BreakerStats> {
        self.mechanisms.breaker.as_ref().map(Breaker::stats)
    }

    /// How many calls hold a slot of the bulkhead now — each for its whole
    /// run, its retries and their waits included, and through
    /// [`ResilienceMiddleware`](crate::ResilienceMiddleware) until its
    /// response's body ends — or `None` for a policy with no bulkhead
    /// (`CONCURRENCY` unset). A call is counted from the moment its own
    /// future holds its slot: one a freed slot is handed to is read as
    /// [waiting](Self::concurrency_waiting) until it is polled, so at every
    /// instant a call is in one of the two reads and never in both.
    pub fn concurrency_running(&self) -> Option<usize> {
        self.mechanisms.bulkhead.as_ref().map(Bulkhead::running)
    }

    /// How many calls wait for a slot of the bulkhead now, or `None` for a
    /// policy with no bulkhead (`CONCURRENCY` unset): none ever, where a call
    /// finding no free slot does not wait (`CONCURRENCY_WAIT_MS` unset).
    pub fn concurrency_waiting(&self) -> Option<usize> {
        self.mechanisms.bulkhead.as_ref().map(Bulkhead::waiting)
    }

    /// How many tokens of the rate limit are free now — what as many attempts
    /// arriving together would take, none while an attempt waits for one — or
    /// `None` for a policy with no rate limit (`RATE_LIMIT` unset).
    pub fn rate_available(&self) -> Option<usize> {
        self.mechanisms
            .rate
            .as_ref()
            .map(|rate| rate.available(Instant::now()))
    }

    /// A subscription to what the policy does from now on: each retry, each
    /// attempt given up at its bound, each call refused unsent, each
    /// transition of its breaker and each fallback answering for a run, as a
    /// [`ResilienceEvent`](crate::ResilienceEvent). Every subscription is told
    /// every event; one opened later is told nothing that came before it.
    ///
    /// With no subscription open, the policy builds no event. A subscription
    /// is this process's, as the policy's state is, and ends once the policy
    /// is dropped.
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use nest_rs_resilience::{BreakerPhase, ResilienceEvent, ResiliencePolicy};
    ///
    /// fn watch(policy: &Arc<ResiliencePolicy>) {
    ///     // The task holds the subscription and not the policy, so it ends
    ///     // with the app.
    ///     let mut events = policy.events();
    ///     tokio::spawn(async move {
    ///         while let Some(event) = events.next().await {
    ///             if let ResilienceEvent::Transition { to, manual: false, .. } = event
    ///                 && to == BreakerPhase::Open
    ///             {
    ///                 // The dependency is failing: page whoever is on call.
    ///             }
    ///         }
    ///     });
    /// }
    /// ```
    pub fn events(&self) -> ResilienceEvents {
        self.mechanisms.teller.subscribe()
    }

    /// Open the circuit breaker by hand for `duration`, an open time
    /// [`breaker_open`](ResilienceConfig::breaker_open) takes in code, above
    /// zero and up to an hour: every call is refused unsent, as by an open
    /// breaker, told what is left of it — one an attempt in flight makes of
    /// this policy included — then the breaker probes the dependency as after
    /// any opening.
    ///
    /// Whatever the breaker was in, `duration` counts from now and replaces
    /// any open time it had, shorter or longer, and its window is forgotten:
    /// a call let through before it changes nothing once it answers — a probe
    /// answering a success closes nothing — and a probe waiting for its token
    /// of the rate loses its turn. When its wait ends it meets the breaker as
    /// a call
    /// arriving then does: refused while the opening lasts, and taking the
    /// next probe's turn once it has passed. Each call's transition is filed as a
    /// `warn`, marked `manual` and carrying the open time it was given, and
    /// counts on the transitions' counter marked manual, whether or not the
    /// breaker was open already.
    ///
    /// The switch is this process's, as the breaker is: each replica keeps its
    /// own, so a dependency taken out for the whole app is switched on every
    /// replica.
    ///
    /// # Errors
    ///
    /// [`BreakerError::Off`] for a policy with no breaker, and
    /// [`BreakerError::OutOfRange`] for a `duration` `breaker_open` would not
    /// take in code, none or one past an hour; either leaves the breaker as it
    /// was.
    pub fn open_breaker(&self, duration: Duration) -> Result<(), BreakerError> {
        let breaker = self.mechanisms.breaker.as_ref().ok_or(BreakerError::Off)?;
        if !is_open_time(duration) {
            return Err(BreakerError::out_of_range());
        }
        breaker.open_manually(duration);
        Ok(())
    }

    /// Close the circuit breaker by hand, forgetting its window: every call
    /// is sent again, and counted afresh.
    ///
    /// Whatever the breaker was in, a call let through before it changes
    /// nothing once it answers: a probe answering a failure opens nothing,
    /// and no such answer counts in the window that follows. A breaker its
    /// probes closed counts the same way: the answer of a call sent before
    /// it opened counts nowhere. Each call's transition is filed as a `warn`,
    /// marked `manual`, and counts on the transitions' counter marked
    /// manual, whether or not the breaker was closed already.
    /// Like [`open_breaker`](Self::open_breaker), it switches this replica's
    /// breaker alone.
    ///
    /// # Errors
    ///
    /// [`BreakerError::Off`] for a policy with no breaker.
    pub fn close_breaker(&self) -> Result<(), BreakerError> {
        self.mechanisms
            .breaker
            .as_ref()
            .ok_or(BreakerError::Off)?
            .close_manually();
        Ok(())
    }
}

impl<I: Instance> nest_rs_core::CarriesRemedy for ResiliencePolicy<I> {
    fn remedy() -> Cow<'static, str> {
        let import =
            nest_rs_core::__private::typed_import::<I>("nest_rs::resilience::ResilienceModule");
        let variables =
            nest_rs_config::var_name(&instance_namespace::<I>(ResilienceConfig::NAMESPACE), "*");
        format!("Import `{import}`, which builds the policy from {variables}.").into()
    }
}

impl<I> fmt::Debug for ResiliencePolicy<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mechanisms = &*self.mechanisms;
        f.debug_struct("ResiliencePolicy")
            .field("instance", &mechanisms.teller.instance())
            .field("timeout", &mechanisms.timeout)
            .field("retries", &mechanisms.retries)
            .field("breaker", &mechanisms.breaker.is_some())
            .field("bulkhead", &mechanisms.bulkhead.is_some())
            .field("rate", &mechanisms.rate.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use nest_rs_core::{CarriesRemedy, instance};

    use super::*;
    use crate::TARGET;

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

    #[test]
    fn an_unmet_policy_names_its_instances_import() {
        assert_eq!(
            ResiliencePolicy::<Upstream>::remedy(),
            format!(
                "Import `nest_rs::resilience::ResilienceModule<Upstream>`, which builds the \
                 policy from {}.",
                nest_rs_config::var_name("resilience__upstream", "*")
            )
        );
        assert_eq!(
            ResiliencePolicy::<Unnamed>::remedy(),
            format!(
                "Import `nest_rs::resilience::ResilienceModule`, which builds the policy from {}.",
                nest_rs_config::var_name("resilience", "*")
            )
        );
    }

    /// A policy with no breaker says so by type, never as a breaker closed,
    /// and refuses either switch; an age and probes mean nothing without a
    /// breaker, as a slot's wait means nothing without slots, and build none.
    #[tokio::test(start_paused = true)]
    async fn a_policy_with_no_breaker_says_so_by_type_and_refuses_either_switch() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig {
            breaker_window: None,
            breaker_age: Some(Duration::from_secs(60)),
            breaker_probes: 3,
            ..ResilienceConfig::default()
        });
        assert!(policy.mechanisms.breaker.is_none());
        assert_eq!(policy.breaker_state(), None);
        assert_eq!(
            policy.open_breaker(Duration::from_secs(10)),
            Err(BreakerError::Off)
        );
        assert_eq!(
            policy.open_breaker(Duration::ZERO),
            Err(BreakerError::Off),
            "no breaker before a duration it would not take"
        );
        assert_eq!(policy.close_breaker(), Err(BreakerError::Off));
        assert_eq!(
            BreakerError::Off.to_string(),
            "the policy has no circuit breaker to switch"
        );
        assert!(policy.run(answering).await.is_ok());
        assert!(lines.find(TARGET, crate::teller::MOVED).is_empty());
    }

    /// A manual opening for a duration `breaker_open` would not take in code
    /// — none, or past its ceiling — is refused, never clamped: the breaker
    /// stays as it was and files nothing, and the refusal names the open times
    /// it takes and the field taking them, never the duration given. Anything
    /// above none up to the ceiling is taken, a time under a second included.
    #[tokio::test(start_paused = true)]
    async fn a_manual_opening_outside_the_open_times_breaker_open_takes_is_refused_unclamped() {
        let lines = nest_rs_testing::LogCapture::install();
        let policy = policy(ResilienceConfig::default());
        let bounds = ResilienceConfig::BREAKER_OPEN.bounds();
        assert!(
            matches!(bounds.least(), nest_rs_config::Floor::AboveZero(_)),
            "an open time is anything above none"
        );
        let most = bounds.unit().duration(bounds.most().count);
        let tick = Duration::from_nanos(1);
        for outside in [Duration::ZERO, most + tick, Duration::MAX] {
            let refused = policy
                .open_breaker(outside)
                .expect_err("outside the open times");
            assert_eq!(refused, BreakerError::OutOfRange { most });
            let said = refused.to_string();
            assert_eq!(
                said,
                format!(
                    "a breaker opened by hand stays open for a time above zero, up to {most:?}, \
                     the open times `{}` takes in code",
                    ResilienceConfig::BREAKER_OPEN.field().path()
                )
            );
            assert!(!said.contains(&format!("{outside:?}")), "{said}");
        }
        assert_eq!(policy.breaker_state(), Some(BreakerState::Closed));
        assert!(lines.find(TARGET, crate::teller::MOVED).is_empty());
        for taken in [tick, Duration::from_millis(500), most] {
            assert_eq!(policy.open_breaker(taken), Ok(()), "{taken:?}");
            assert_eq!(
                policy.breaker_state(),
                Some(BreakerState::Open { retry_after: taken })
            );
        }
    }
}
