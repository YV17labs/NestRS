//! Link-time registry of `#[scheduled]` method jobs, one [`ScheduledMethod`]
//! per method, plus the [`CronJobMeta`] the [`Scheduler`](crate::Scheduler)
//! builds from each entry.
//!
//! The scheduler gates the registry on
//! [`ReachableProviders`](::nest_rs_core::ReachableProviders), and merges it with
//! the metadata attached through `attach_meta::<…, CronJobMeta>`.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use nest_rs_core::__private::Site;
use nest_rs_core::Container;
use nest_rs_database::TransactionPolicy;

use crate::{Replicas, Trigger};

/// The async closure a [`ScheduledMethod`] / [`CronJobMeta`] dispatches:
/// resolves the provider and runs the method.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// The metadata one running job carries, attached directly through
/// `attach_meta::<…, CronJobMeta>` or built from each [`ScheduledMethod`].
///
/// A job attached by hand declares no site of its own: it binds no layer and
/// no `#[meta]`, while the app's interceptors and exception filters run around
/// it as around any tick, and its [`ScheduleView`](crate::ScheduleView) names
/// it.
///
/// [`new`](Self::new) builds one at the defaults `#[every]`, `#[cron]` and
/// `#[after]` take with no key; each `with_*` sets what one key would:
///
/// ```
/// use std::time::Duration;
///
/// use nest_rs_schedule::{CronJobMeta, Replicas, RunFn, Trigger};
///
/// const PURGE: RunFn = |_container| Box::pin(async { Ok(()) });
///
/// let meta = CronJobMeta::new(
///     module_path!(),
///     "ArchiveTasks",
///     "purge",
///     Trigger::Interval(Duration::from_secs(60)),
///     PURGE,
/// )
/// .with_timeout(Duration::from_secs(5))
/// .with_replicas(Replicas::One);
/// assert_eq!(meta.timeout(), Duration::from_secs(5));
/// assert_eq!(meta.replicas(), Replicas::One);
/// assert_eq!(meta.key(), None);
/// ```
pub struct CronJobMeta {
    pub(crate) origin: &'static str,
    pub(crate) host: &'static str,
    pub(crate) method: &'static str,
    pub(crate) trigger: Trigger,
    pub(crate) run: RunFn,
    pub(crate) timeout: Duration,
    pub(crate) transaction: TransactionPolicy,
    pub(crate) replicas: Replicas,
    pub(crate) key: Option<&'static str>,
}

impl CronJobMeta {
    /// The job `run` that `host`'s `method`, declared in `origin`
    /// (`module_path!()`), fires on `trigger`: cut after
    /// [`JOB_TIMEOUT`](nest_rs_core::JOB_TIMEOUT), its data-layer work one
    /// transaction per run, fired on every replica, its identity derived from
    /// its crate, host and method.
    pub const fn new(
        origin: &'static str,
        host: &'static str,
        method: &'static str,
        trigger: Trigger,
        run: RunFn,
    ) -> Self {
        Self {
            origin,
            host,
            method,
            trigger,
            run,
            timeout: nest_rs_core::JOB_TIMEOUT,
            transaction: TransactionPolicy::PerUnit,
            replicas: Replicas::Each,
            key: None,
        }
    }

    /// The job cut after `timeout`, as a `timeout` key declares it.
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The job's data-layer work settled under `transaction`, as a
    /// `transactional` key declares it.
    pub const fn with_transaction(mut self, transaction: TransactionPolicy) -> Self {
        self.transaction = transaction;
        self
    }

    /// The job fired on `replicas`, as a `replicas` key declares it.
    pub const fn with_replicas(mut self, replicas: Replicas) -> Self {
        self.replicas = replicas;
        self
    }

    /// The job's occurrences claimed under `key`, as a `key` key pins it
    /// (`"billing::InvoiceTasks::close_day"`).
    pub const fn with_key(mut self, key: &'static str) -> Self {
        self.key = Some(key);
        self
    }

    /// `module_path!()` of the code declaring the job; its crate opens the
    /// job's identity unless [`key`](Self::key) pins one.
    pub const fn origin(&self) -> &'static str {
        self.origin
    }

    /// The host struct, e.g. `"AudioTasks"`.
    pub const fn host(&self) -> &'static str {
        self.host
    }

    /// The scheduled method, e.g. `"heartbeat"`.
    pub const fn method(&self) -> &'static str {
        self.method
    }

    /// When this job fires.
    pub const fn trigger(&self) -> Trigger {
        self.trigger
    }

    /// The closure the scheduler invokes on each tick.
    pub const fn run(&self) -> RunFn {
        self.run
    }

    /// How long one run lasts before it is cut and reported failed.
    pub const fn timeout(&self) -> Duration {
        self.timeout
    }

    /// How this job's data-layer work is settled.
    pub const fn transaction(&self) -> TransactionPolicy {
        self.transaction
    }

    /// How many replicas fire each occurrence.
    pub const fn replicas(&self) -> Replicas {
        self.replicas
    }

    /// The identity a job firing once claims its occurrences under, when one
    /// is pinned; `None` derives it from the crate, the host struct and the
    /// method.
    pub const fn key(&self) -> Option<&'static str> {
        self.key
    }
}

/// Link-time inventory entry submitted by `#[scheduled]` per `#[every]` /
/// `#[cron]` / `#[after]`-tagged method: what a test or a package reads of a
/// scheduled method, and what the [`Scheduler`](crate::Scheduler) runs.
pub struct ScheduledMethod {
    parts: ScheduledMethodParts,
}

/// The facts `#[scheduled]` reads off one method, each the value of the
/// [`ScheduledMethod`] accessor of the same name.
#[expect(
    missing_docs,
    reason = "each field is documented on the `ScheduledMethod` accessor of its name"
)]
pub struct ScheduledMethodParts {
    pub origin: &'static str,
    pub host: &'static str,
    pub method: &'static str,
    pub host_type_id: fn() -> TypeId,
    pub trigger: Trigger,
    pub run: RunFn,
    pub timeout: Duration,
    pub transaction: TransactionPolicy,
    pub replicas: Replicas,
    pub key: Option<&'static str>,
    pub site: &'static Site,
}

/// The entry `#[scheduled]` submits for one method out of what it read; a
/// hand-built entry is a test's, never an app's.
pub const fn scheduled_method(parts: ScheduledMethodParts) -> ScheduledMethod {
    ScheduledMethod { parts }
}

impl ScheduledMethod {
    /// `module_path!()` of the module that declared it; its crate opens the
    /// job's identity unless [`key`](Self::key) pins one.
    pub const fn origin(&self) -> &'static str {
        self.parts.origin
    }

    /// The host struct, e.g. `"AudioTasks"`.
    pub const fn host(&self) -> &'static str {
        self.parts.host
    }

    /// The scheduled method, e.g. `"heartbeat"`.
    pub const fn method(&self) -> &'static str {
        self.parts.method
    }

    /// `TypeId::of::<Provider>()`, which a reader gates on
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders) as the
    /// scheduler does.
    pub fn host_type_id(&self) -> TypeId {
        (self.parts.host_type_id)()
    }

    /// When this job fires.
    pub const fn trigger(&self) -> Trigger {
        self.parts.trigger
    }

    /// The closure the scheduler invokes on each tick.
    pub(crate) const fn run(&self) -> RunFn {
        self.parts.run
    }

    /// How long one run lasts before it is cut — the `timeout` key on its
    /// `#[every]` / `#[cron]` / `#[after]`, else
    /// [`JOB_TIMEOUT`](nest_rs_core::JOB_TIMEOUT).
    pub const fn timeout(&self) -> Duration {
        self.parts.timeout
    }

    /// How this job's data-layer work is settled — the `transactional` key on
    /// its `#[every]` / `#[cron]` / `#[after]`, else one transaction per run.
    pub const fn transaction(&self) -> TransactionPolicy {
        self.parts.transaction
    }

    /// How many replicas fire each occurrence — the `replicas` key on its
    /// `#[every]` / `#[cron]`, else every replica.
    pub const fn replicas(&self) -> Replicas {
        self.parts.replicas
    }

    /// The identity pinned by the `key` on its `#[every]` / `#[cron]`, if any.
    pub const fn key(&self) -> Option<&'static str> {
        self.parts.key
    }

    /// The dispatch site its decorator declared, with the layers its method
    /// and its block bind.
    pub(crate) const fn site(&self) -> &'static Site {
        self.parts.site
    }
}

::nest_rs_core::inventory::collect!(ScheduledMethod);
