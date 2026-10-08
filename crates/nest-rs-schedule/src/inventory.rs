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

use nest_rs_core::Container;
use nest_rs_worker::JobTransaction;

use crate::{Replicas, Trigger};

/// The async closure a [`ScheduledMethod`] / [`CronJobMeta`] dispatches:
/// resolves the provider and runs the method.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// The metadata one running job carries, attached directly through
/// `attach_meta::<…, CronJobMeta>` or built from each [`ScheduledMethod`].
pub struct CronJobMeta {
    /// `module_path!()` of the code declaring the job; its crate opens the job's
    /// identity unless `key` pins one.
    pub origin: &'static str,
    /// The host struct, e.g. `"AudioTasks"`.
    pub provider: &'static str,
    /// The scheduled method, e.g. `"heartbeat"`.
    pub method: &'static str,
    /// When this job fires.
    pub trigger: Trigger,
    /// The closure the scheduler invokes on each tick.
    pub run: RunFn,
    /// How long one run lasts before it is cut and reported failed — from the
    /// `timeout` key on its `#[every]` / `#[cron]` / `#[after]`, defaulting to
    /// [`JOB_TIMEOUT`](nest_rs_worker::JOB_TIMEOUT).
    pub timeout: Duration,
    /// How this job's data-layer work is settled — from the `transactional`
    /// key on its `#[every]` / `#[cron]` / `#[after]`, defaulting to one
    /// transaction per attempt.
    pub transaction: JobTransaction,
    /// How many replicas fire each occurrence — from the `replicas` key on its
    /// `#[every]` / `#[cron]`, defaulting to every replica.
    pub replicas: Replicas,
    /// The identity a job firing once claims its occurrences under, pinned by
    /// the `key` on its `#[every]` / `#[cron]` (`"billing::InvoiceTasks::close_day"`);
    /// `None` derives it from the crate, the host struct and the method.
    pub key: Option<&'static str>,
}

/// Link-time inventory entry submitted by `#[scheduled]` per `#[every]` /
/// `#[cron]` / `#[after]`-tagged method.
pub struct ScheduledMethod {
    /// `module_path!()` of the module that declared it.
    pub origin: &'static str,
    /// The host struct, e.g. `"AudioTasks"`.
    pub provider: &'static str,
    /// The scheduled method, e.g. `"heartbeat"`.
    pub method: &'static str,
    /// `TypeId::of::<Provider>()`, checked against
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders).
    pub provider_type_id: fn() -> TypeId,
    /// When this job fires.
    pub trigger: Trigger,
    /// The closure the scheduler invokes on each tick.
    pub run: RunFn,
    /// How long one run lasts before it is cut.
    pub timeout: Duration,
    /// How this job's data-layer work is settled — from the `transactional`
    /// key on its `#[every]` / `#[cron]` / `#[after]`, defaulting to one
    /// transaction per attempt.
    pub transaction: JobTransaction,
    /// How many replicas fire each occurrence — from the `replicas` key on its
    /// `#[every]` / `#[cron]`, defaulting to every replica.
    pub replicas: Replicas,
    /// The identity pinned by the `key` on its `#[every]` / `#[cron]`.
    pub key: Option<&'static str>,
}

::nest_rs_core::inventory::collect!(ScheduledMethod);
