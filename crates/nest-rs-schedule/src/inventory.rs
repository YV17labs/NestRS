//! Link-time registry of `#[scheduled]` method jobs, submitted by
//! `nest_rs_schedule_macros::scheduled` on a per-method basis, plus the
//! synthesized [`CronJobMeta`] the [`Scheduler`](crate::Scheduler) builds
//! from each entry.
//!
//! `#[scheduled]` lets a single `#[injectable]` provider own several scheduled
//! methods sharing the same `#[inject]` deps. Each method submits one
//! [`ScheduledMethod`] here; [`crate::Scheduler`] drains the registry at boot
//! and filters by
//! [`ReachableProviders`](::nest_rs_core::ReachableProviders) so a job whose
//! provider is not in the app's module tree is silently skipped — same
//! module-gating as the rest of the discovery system.
//!
//! The `attach_meta::<…, CronJobMeta>` path remains for direct, test-friendly
//! registration; [`crate::Scheduler`] merges both sources.

use std::any::TypeId;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use nest_rs_core::Container;
use nest_rs_worker::JobTransaction;

use crate::{Replicas, Trigger};

/// The async closure a [`ScheduledMethod`] / [`CronJobMeta`] dispatches.
/// Resolves the provider from the assembled container and runs the method.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// The synthesized metadata one running job carries. Tests register this
/// directly via `attach_meta::<…, CronJobMeta>`; the `#[scheduled]` path
/// builds one from each [`ScheduledMethod`] at boot.
///
/// `provider` (the host struct) and `method` stay split rather than baked into
/// a single label so structured logs can filter/group on either alone — a
/// composite string would be unqueryable once the output is JSON.
pub struct CronJobMeta {
    /// `module_path!()` of the code declaring the job. Its first segment — the
    /// crate — is the first level of the job's identity unless `key` pins one: the
    /// same job declared in a crate several apps link coordinates its occurrences
    /// across all of them, two apps each declaring a same-named job in their own
    /// crates do not claim each other's, and moving the job's module inside its
    /// crate keeps it.
    pub origin: &'static str,
    /// The host struct, e.g. `"AudioTasks"`.
    pub provider: &'static str,
    /// The scheduled method, e.g. `"heartbeat"`.
    pub method: &'static str,
    /// When this job fires — resolved from the method's `#[every]` / `#[cron]`
    /// / `#[after]` attribute.
    pub trigger: Trigger,
    /// The closure the scheduler invokes on each tick — resolves the provider
    /// and calls the method.
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
    /// The identity a job firing once claims its occurrences under, pinned — from
    /// the `key` on its `#[every]` / `#[cron]`, a path of one or more identifiers
    /// (`"billing::InvoiceTasks::close_day"`). `None` derives it: the crate, the
    /// host struct and the method. Only a job declaring `replicas = "one"` may pin
    /// one, since a job firing on every replica claims nothing.
    pub key: Option<&'static str>,
}

/// Link-time inventory entry submitted by `#[scheduled]` per `#[every]` /
/// `#[cron]` / `#[after]`-tagged method.
pub struct ScheduledMethod {
    /// `module_path!()` of the module that declared it — read by
    /// [`is_framework_owned`](::nest_rs_core::is_framework_owned) to pick the
    /// report level, emitted as a field so a skip line names a type the
    /// developer can find, and copied to the synthesized [`CronJobMeta`], whose
    /// identity opens with its crate.
    pub origin: &'static str,
    /// The host struct (e.g. `"AudioTasks"`) — logged as its own field and
    /// copied to the synthesized [`CronJobMeta`].
    pub provider: &'static str,
    /// The scheduled method (e.g. `"heartbeat"`) — logged as its own field.
    pub method: &'static str,
    /// `TypeId::of::<Provider>()` — checked against
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so an
    /// unreachable provider's jobs do not fire.
    pub provider_type_id: fn() -> TypeId,
    /// When this job fires — the parsed `#[every]` / `#[cron]` / `#[after]`
    /// trigger, copied to the synthesized [`CronJobMeta`].
    pub trigger: Trigger,
    /// The closure the scheduler invokes on each tick.
    pub run: RunFn,
    /// How long one run lasts before it is cut, copied to the synthesized
    /// [`CronJobMeta`].
    pub timeout: Duration,
    /// How this job's data-layer work is settled — from the `transactional`
    /// key on its `#[every]` / `#[cron]` / `#[after]`, defaulting to one
    /// transaction per attempt.
    pub transaction: JobTransaction,
    /// How many replicas fire each occurrence — from the `replicas` key on its
    /// `#[every]` / `#[cron]`, defaulting to every replica.
    pub replicas: Replicas,
    /// The identity pinned by the `key` on its `#[every]` / `#[cron]`, copied to
    /// the synthesized [`CronJobMeta`].
    pub key: Option<&'static str>,
}

::nest_rs_core::inventory::collect!(ScheduledMethod);
