//! Link-time registry of `#[scheduled]` method jobs, submitted by
//! `nestrs_schedule_macros::scheduled` on a per-method basis.
//!
//! `#[scheduled]` lets a single `#[injectable]` provider own several scheduled
//! methods sharing the same `#[inject]` deps. Each method submits one
//! [`ScheduledMethod`] here; [`crate::Scheduler`] drains the registry at boot
//! and filters by
//! [`ReachableProviders`](::nestrs_core::ReachableProviders) so a job whose
//! provider is not in the app's module tree is silently skipped — same
//! module-gating as the rest of the discovery system.
//!
//! The `attach_meta::<…, CronJobMeta>` path remains for direct, test-friendly
//! registration; [`crate::Scheduler`] merges both sources.

use std::any::TypeId;

use crate::meta::RunFn;
use crate::Trigger;

pub struct ScheduledMethod {
    /// `"ProviderType::method"` — the human-readable label `Scheduler` logs and
    /// the `name` field of the synthesized [`crate::CronJobMeta`].
    pub name: &'static str,
    /// `TypeId::of::<Provider>()` — checked against
    /// [`ReachableProviders`](::nestrs_core::ReachableProviders) so an
    /// unreachable provider's jobs do not fire.
    pub provider_type_id: fn() -> TypeId,
    pub trigger: Trigger,
    pub run: RunFn,
}

::nestrs_core::inventory::collect!(ScheduledMethod);
