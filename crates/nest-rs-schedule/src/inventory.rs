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

use nest_rs_core::Container;

use crate::Trigger;

/// The async closure a [`ScheduledMethod`] / [`CronJobMeta`] dispatches.
/// Resolves the provider from the assembled container and runs the method.
pub type RunFn =
    for<'a> fn(&'a Container) -> Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// The synthesized metadata one running job carries. Tests register this
/// directly via `attach_meta::<…, CronJobMeta>`; the `#[scheduled]` path
/// builds one from each [`ScheduledMethod`] at boot.
pub struct CronJobMeta {
    pub name: &'static str,
    pub trigger: Trigger,
    pub run: RunFn,
}

/// Link-time inventory entry submitted by `#[scheduled]` per `#[every]` /
/// `#[cron]` / `#[after]`-tagged method.
pub struct ScheduledMethod {
    /// `"ProviderType::method"` — the human-readable label `Scheduler` logs and
    /// the `name` field of the synthesized [`CronJobMeta`].
    pub name: &'static str,
    /// `TypeId::of::<Provider>()` — checked against
    /// [`ReachableProviders`](::nest_rs_core::ReachableProviders) so an
    /// unreachable provider's jobs do not fire.
    pub provider_type_id: fn() -> TypeId,
    pub trigger: Trigger,
    pub run: RunFn,
}

::nest_rs_core::inventory::collect!(ScheduledMethod);
